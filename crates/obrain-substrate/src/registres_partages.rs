//! ── REGISTRES PARTAGÉS ENTRE PROCESSUS (Phase B, pièce 3) ────────────────────
//!
//! ## Le défaut mesuré qu'elle ferme
//!
//! Trois registres internent des NOMS en INDICES numériques, et ces indices
//! partent dans les données :
//!
//! | registre    | indice         | où il part                              |
//! |-------------|----------------|-----------------------------------------|
//! | labels      | bit 0..=63     | `NodeRecord.label_bitset` (u64)         |
//! | types d'ar. | `edge_type` u16| `EdgeRecord.edge_type`                  |
//! | clés de pr. | `prop_key_id`  | pages de propriétés, colonnes Vec/Blob  |
//!
//! Or l'internement était une `HashMap` **PAR PROCESSUS** (`store.rs`,
//! `LabelRegistry::intern` : `self.bit_to_name.len()`, `EdgeTypeRegistry::intern`,
//! `PropertyKeyRegistry::intern`). L'indice était donc le **compteur d'arrivée de
//! CE processus**, pas une propriété du nom. Deux processus qui internent des noms
//! DIFFÉRENTS reçoivent le MÊME indice (`"Task"` → 1 chez l'un, `"Message"` → 1
//! chez l'autre), et leurs `label_bitset` / `key_id` désignent alors deux choses
//! distinctes. La divergence est DÉTECTÉE au `persist_dict` (fusion « préfixe ou
//! divergence nommée », `store.rs`) mais jamais réparée : la réparer exigerait de
//! réécrire les données.
//!
//! ## Le choix : table ouverte mmapée + compteur d'indices partagé
//!
//! Trois formes étaient possibles :
//!
//! * **(a) indice = hash du nom.** Déterministe sans aucun état partagé — mais
//!   change l'indice de TOUS les noms déjà internés : les `label_bitset`,
//!   `edge_type` et `prop_key_id` des stores existants désigneraient autre chose.
//!   C'est une réécriture complète des données, pas une pièce.
//! * **(b) registre partagé nom → indice** (ce fichier), **plus** le compteur
//!   d'indices réservé par `fetch_add` sur le même mapping — la famille d'atomes
//!   de `borne_allocation.rs` (pièce 2), pas un verrou chaud.
//! * **(c) réutiliser `substrate.alloc`.** Refusé : les indices de registres sont
//!   un espace d'identifiants DISTINCT (bits ≤ 63, u16 de types/clés) de celui des
//!   slots ; les mêler ferait dépendre l'un des épuisements de l'autre.
//!
//! (b) est le plus simple QUI PRÉSERVE les données : les noms déjà internés
//! gardent leur indice (le store les PUBLIE depuis le dictionnaire à l'ouverture,
//! `publier`), et seuls les noms neufs consomment le compteur partagé. Aucune
//! migration, aucun format inchangé sur disque (`substrate.dict` reste ce qu'il
//! est), seule une table d'INTERNEMENT s'ajoute à côté.
//!
//! ## Pourquoi c'est l'atome, et pas un verrou
//!
//! * **la réservation d'un indice neuf** est un `fetch_add` sur un `AtomicU64`
//!   partagé : deux processus ne peuvent pas recevoir le même (même garantie que
//!   `BorneAllocation::reserver_*`), sans syscall ni coordination.
//! * **l'entrée dans la table** est un `compare_exchange` sur l'état de la case
//!   (EMPTY → CLAIMED). Le perdant de la course ne perd pas son nom : il relit la
//!   case, y trouve le nom du gagnant — donc SON indice — et le rend. Deux
//!   processus qui internent le même nom **convergent sur le même indice** sans
//!   jamais se parler.
//! * **la lecture** (chemin chaud : un nom déjà interné) ne fait qu'un
//!   `Acquire` sur l'état, une comparaison de hash et une comparaison d'octets :
//!   aucun `compare_exchange`, aucune écriture, donc aucune invalidation de ligne
//!   de cache entre processus pour un nom déjà vu.
//!
//! Ce qui reste possible, et pourquoi ce n'est PAS un défaut :
//!
//! * `CLAIMED` sans `FILLED` (un processus tué au milieu d'une insertion) gèle
//!   une case. Tout le monde la sonde AU-DELÀ (borne de tours) ; la case est
//!   perdue, pas le registre. Une case par `SIGKILL` au pire.
//! * le compteur peut être consommé sans que l'indice serve (échec après
//!   réservation) : un indice est un NUMÉRO, le manquer ne coûte rien.
//! * si la table est pleine (impossible sous les bornes ci-dessous : 64 labels,
//!   `u16` de types et de clés), la réservation échoue BRUYAMMENT au lieu de
//!   rendre un indice faux.
//!
//! ## Une entrée dans la table est FILLED **avant** d'être vraie
//!
//! L'ordre d'écriture est : état = CLAIMED → hash/index/nom → état = FILLED
//! (`Release`). Un lecteur qui voit `FILLED` (`Acquire`) voit donc le nom et
//! l'indice ENTIERS. C'est le seul ordre qui rende la table lisible sans verrou.

#![allow(unsafe_code)]

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};

use memmap2::{MmapMut, MmapOptions};

use crate::error::SubstrateResult;

/// Nom du fichier des registres partagés, dans le répertoire du substrat.
///
/// Le nom dit ce que le fichier EST : l'INTERNEMENT (nom → indice), pas les
/// données. `substrate.dict` reste le dictionnaire persisté (indice → nom) ;
/// ce fichier-ci est ce que le dictionnaire ne peut pas être : CONSULTABLE ET
/// MODIFIABLE par plusieurs processus en même temps.
pub const FILENAME_REGISTRES_PARTAGES: &str = "substrate.noms";

/// Magie : « OBRA-NOM-1 ». Toute autre valeur = fichier d'une autre nature.
const MAGIE: u64 = u64::from_le_bytes(*b"OBRANOM1");

/// Version du LAYOUT. Un incrément = les décalages changent : on repart de zéro
/// plutôt que de lire des octets qui ne veulent plus rien dire.
///
/// v2 : ajout de la région dense index→nom (`NOMS_DENSES`) et de la borne O(1) du
/// filigrane (un `AtomicU64` par espace). Les décalages d'espace changent ; la
/// réinitialisation qui s'ensuit est republiée depuis le dictionnaire.
const VERSION_LAYOUT: u64 = 2;

/// Taille d'une case. Elle EST un multiple de 8 : c'est ce qui rend l'adresse de
/// chaque case alignée pour ses atomiques (`AtomicU64` du hash, `AtomicU32` de
/// l'indice, `AtomicU8` de l'état et de la longueur). Une taille non multiple de
/// 8 ferait qu'une case sur deux serait désalignée — le genre de faute qui ne se
/// voit que sur certaines architectures.
const TAILLE_CASE: usize = 88;

/// État d'une case.
const VIDE: u8 = 0;
const RESERVEE: u8 = 1;
const REMPLIE: u8 = 2;

/// Nombre de cases par espace. Il n'est PAS indexé par l'indice (l'indice vient
/// du compteur partagé) : il borne seulement le nombre de sondes. Il est donc
/// volontairement très supérieur au maximum d'entrées vivantes (64 labels,
/// 65 535 types, 65 535 clés), ce qui garde la charge sous 1 %.
const CASES: usize = 8192;

/// Nombre maximal de cases sondées avant d'abandonner une recherche.
///
/// Au-delà, on ne cherche plus : on PRÉFÈRE rendre un indice neuf (donc un nom
/// interné deux fois, visible et réparable) à boucler. La charge < 1 % rend ce
/// chemin statistiquement inatteignable ; il est là pour qu'aucune entrée ne
/// puisse transformer une recherche en blocage.
const SONDES_MAX: usize = 256;

/// Durée maximale d'attente d'une case `RESERVEE` par un pair.
///
/// Le remplissage d'une case est une écriture mémoire (quelques dizaines de ns) :
/// 2 s couvrent très largement une préemption de l'ordonnanceur, et bornent le cas
/// « le réserveur est mort avant d'écrire » — le seul qui reste, et qu'aucun délai
/// ne peut distinguer d'un pair lent sinon par la durée.
const ATTENTE_TOTALE: std::time::Duration = std::time::Duration::from_secs(2);

/// Alignement du mapping (`MAP_SHARED` sur page). 64 Kio couvrent toutes les
/// pages des plateformes visées (4 Kio Linux x86, 16 Kio macOS) — l'alignement
/// est ce qui rend `&AtomicU64` légitime sur ces octets.
const ALIGNEMENT: usize = 65536;

// Un espace = les cases + le mot du compteur, alignés.
//   cases (CASES × TAILLE_CASE) puis le mot du compteur, sur ALIGNEMENT.
const TAILLE_CASES: usize = CASES * TAILLE_CASE;
/// Une case par INDICE possible : `CAPACITE_INDICES` slots.
///
/// ── POURQUOI CETTE SECONDE TABLE EXISTE (mesuré au banc) ────────────────────
/// « Cet indice est-il déjà pris par un autre nom ? » est la question que la
/// collision d'indices pose, et un balayage des 8 192 cases de noms pour y
/// répondre coûtait **160 ms par nom neuf** (mesuré : `intern_registres/nom_neuf`
/// à 40–320 ms). L'ordre de grandeur est celui du balayage, pas de l'internement.
///
/// Ici la réponse est une lecture : `index[indice]` porte l'état (libre, réservé)
/// du seul indice qui nous intéresse. Le balayage disparaît du chemin chaud, et il
/// ne reste qu'un `compare_exchange` sur 4 octets.
const CAPACITE_INDICES: usize = 65_536;
/// Un slot par indice : le mot de REVENDICATION (4 o) et une réserve. C'est lui qui
/// rend l'anti-collision O(1) — une lecture et un `compare_exchange` sur cet octet
/// au lieu d'un balayage des 8 192 cases de noms (160 ms mesurés au banc).
const TAILLE_SLOT_INDICE: usize = 8;
const TAILLE_INDICES: usize = CAPACITE_INDICES * TAILLE_SLOT_INDICE;

/// Taille d'un nom dans la région dense : 1 octet de longueur + 63 octets de nom.
/// Multiple de 8 → chaque slot est aligné pour ses atomiques (le `AtomicU8` de
/// longueur), et `indice × 64` est une adresse directe, sans indirection.
const TAILLE_NOM_DENSE: usize = 64;
/// ── LA RÉGION DENSE INDEX→NOM (volet 2 du correctif) ────────────────────────
/// Un slot par INDICE : le nom de l'indice `i` est posé à `[i × 64]`. C'est elle
/// qui rend `noms_indexes` O(filigrane) au lieu d'un balayage des 8 192 cases de
/// noms (~4,1 µs/espace mesuré, même vide). La table hashée reste seule à servir
/// le sens nom→indice (chemin chaud) ; cette région est le sens inverse.
const TAILLE_NOMS_DENSES: usize = CAPACITE_INDICES * TAILLE_NOM_DENSE;
const TAILLE_ESPACE: usize =
    (TAILLE_CASES + TAILLE_INDICES + TAILLE_NOMS_DENSES).div_ceil(ALIGNEMENT) * ALIGNEMENT
        + ALIGNEMENT;

/// Décalages des trois espaces. Les labels sont EN PREMIER parce que ce sont eux
/// dont l'espace d'indices est le plus contraint (64) : le diagnostic d'un
/// fichier tronqué tombe alors sur l'espace le plus court.
const OFF_ESPACE_LABELS: usize = ALIGNEMENT;
const OFF_ESPACE_TYPES: usize = OFF_ESPACE_LABELS + TAILLE_ESPACE;
const OFF_ESPACE_CLES: usize = OFF_ESPACE_TYPES + TAILLE_ESPACE;

/// Taille totale du fichier.
const TAILLE: usize = OFF_ESPACE_CLES + TAILLE_ESPACE;

/// ── IDENTIFIANTS DES TROIS ESPACES ──────────────────────────────────────────
///
/// Les indice de départ divergent, et ce n'est pas un détail : le bit 0 est un
/// label légitime (`label_bitset` bit 0), tandis que l'indice 0 des types
/// d'arêtes et des clés est la SENTINELLE NULLE (`EdgeRecord.edge_type == 0` et
/// `prop_key_id == 0` sont déjà écrits dans les données). Le compteur part donc
/// de 1 pour ces deux-là — exactement comme `BorneAllocation` part de 1, et pour
/// la même raison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Espace {
    /// Labels : bits 0..=63 de `NodeRecord.label_bitset`. Aucune sentinelle.
    Labels,
    /// Types d'arêtes : `EdgeRecord.edge_type` (u16), 0 = pas d'arête.
    Types,
    /// Clés de propriétés : `prop_key_id` (u16), 0 = pas de propriété.
    Cles,
}

impl Espace {
    /// Premier indice distribuable.
    #[must_use]
    pub fn premier_indice(self) -> u64 {
        match self {
            Espace::Labels => 0,
            Espace::Types | Espace::Cles => 1,
        }
    }

    /// Dernier indice légal (inclus) : c'est ce que le TYPE de l'indice permet,
    /// et c'est ce qui borne la réservation. Une borne par espace plutôt qu'une
    /// borne de table : c'est l'indice qui casse les données, pas la table.
    #[must_use]
    pub fn dernier_indice(self) -> u64 {
        match self {
            Espace::Labels => 63,
            Espace::Types | Espace::Cles => u64::from(u16::MAX),
        }
    }

    /// Nom lisible de l'espace (messages d'erreur et diagnostics).
    #[must_use]
    pub fn nom(self) -> &'static str {
        match self {
            Espace::Labels => "labels",
            Espace::Types => "edge_types",
            Espace::Cles => "prop_keys",
        }
    }

    /// Décalage de la table « un slot par indice » dans l'espace.
    fn offset_indices(self) -> usize {
        self.offset_espace() + TAILLE_CASES
    }

    /// Décalage de la région dense index→nom (un slot de 64 o par indice).
    fn offset_noms_denses(self) -> usize {
        self.offset_espace() + TAILLE_CASES + TAILLE_INDICES
    }

    /// Décalage du mot de la borne du filigrane (dans le préambule).
    fn offset_filigrane(self) -> usize {
        match self {
            Espace::Labels => OFF_FILIGRANE_LABELS,
            Espace::Types => OFF_FILIGRANE_TYPES,
            Espace::Cles => OFF_FILIGRANE_CLES,
        }
    }

    fn offset_espace(self) -> usize {
        match self {
            Espace::Labels => OFF_ESPACE_LABELS,
            Espace::Types => OFF_ESPACE_TYPES,
            Espace::Cles => OFF_ESPACE_CLES,
        }
    }

    fn offsets_compteur(self) -> (usize, usize) {
        match self {
            Espace::Labels => (OFF_COMPTEUR_LABELS, OFF_CAPACITE_LABELS),
            Espace::Types => (OFF_COMPTEUR_TYPES, OFF_CAPACITE_TYPES),
            Espace::Cles => (OFF_COMPTEUR_CLES, OFF_CAPACITE_CLES),
        }
    }
}

// Décalages du préambule (les mots d'état du fichier).
const OFF_MAGIE: usize = 0;
const OFF_VERSION: usize = 8;
const OFF_BASE_LABELS: usize = 16;
const OFF_BASE_TYPES: usize = 24;
const OFF_BASE_CLES: usize = 32;
const OFF_CAPACITE_LABELS: usize = 40;
const OFF_CAPACITE_TYPES: usize = 48;
const OFF_CAPACITE_CLES: usize = 56;
// Mots des COMPTEURS d'indices (un par espace) — ceux que `fetch_add` avance.
const OFF_COMPTEUR_LABELS: usize = 64;
const OFF_COMPTEUR_TYPES: usize = 72;
const OFF_COMPTEUR_CLES: usize = 80;
// Mots de la BORNE DU FILIGRANE (un par espace) — ceux que `fetch_max(indice+1)`
// avance dans `remplir`, après l'écriture des octets, avant l'état REMPLIE. C'est
// le « 1 + plus grand indice NOMMÉ » lu en O(1) par `noms_indexes` : il suit les
// NOMS, jamais le compteur de réservations (qui, lui, avance dès `fetch_add`).
const OFF_FILIGRANE_LABELS: usize = 88;
const OFF_FILIGRANE_TYPES: usize = 96;
const OFF_FILIGRANE_CLES: usize = 104;

/// ── UNE VUE ATOMIQUE SUR UN MOT DU MAPPING ──────────────────────────────────
///
/// SAFETY (le seul `unsafe` du module, même contrat que `borne_allocation.rs`) :
/// tous les offsets sont des multiples de 8 et la base du mapping est alignée sur
/// `ALIGNEMENT` (multiple de 8) — l'adresse est donc alignée pour un `AtomicU64`.
/// Le mapping est `MAP_SHARED` et vit aussi longtemps que `map`, qui vit aussi
/// longtemps que la référence rendue (durée d'emprunt). `MmapMut` est emprunté
/// par `self`, donc aucune autre vue mutable n'existe sur ces octets.
fn mot(map: &MmapMut, offset: usize) -> &AtomicU64 {
    debug_assert_eq!(offset % 8, 0, "un AtomicU64 exige un offset aligné");
    debug_assert!(offset + 8 <= map.len(), "offset hors du mapping");
    // SAFETY: voir la doc ci-dessus — alignement et durée de vie garantis par la
    // construction de `RegistresPartages`.
    unsafe { &*(map.as_ptr().add(offset) as *const AtomicU64) }
}

/// Une case de la table, vue par un processus.
#[derive(Debug, Clone, Copy)]
struct Case {
    /// Adresse de la case DANS le mapping (base incluse).
    base: usize,
}

impl Case {
    fn etat(self, map: &MmapMut) -> u8 {
        // SAFETY: `base` est aligné sur 8 (TAILLE_CASE est un multiple de 8 et
        // l'espace aussi) et la case tient entièrement dans le mapping — garanti
        // par `case()` qui borne l'indice.
        unsafe { (*(map.as_ptr().add(self.base) as *const AtomicU8)).load(Ordering::Acquire) }
    }

    fn hash(self, map: &MmapMut) -> u64 {
        mot(map, self.base + 8).load(Ordering::Relaxed)
    }

    fn indice(self, map: &MmapMut) -> u32 {
        // SAFETY: même raison que `etat` — offset aligné sur 4 dans un mapping
        // page-aligné.
        unsafe { (*(map.as_ptr().add(self.base + 16) as *const AtomicU32)).load(Ordering::Relaxed) }
    }

    /// Lit un nom de longueur `len` (≤ 63) depuis la case.
    fn nom(self, map: &MmapMut, len: u8) -> Vec<u8> {
        let len = usize::from(len).min(63);
        // SAFETY: `base + 20 + len` reste dans la case (64 octets de nom), donc
        // dans le mapping.
        unsafe { std::slice::from_raw_parts(map.as_ptr().add(self.base + 20), len).to_vec() }
    }

    /// Longueur du nom publié, ou `None` si la publication est incohérente
    /// (longueur illégale) — un lecteur ne doit jamais lire hors de la case.
    fn longueur(self, map: &MmapMut) -> Option<u8> {
        // SAFETY: même raison que `etat` — offset 84 = état(0) + hash(8) +
        // indice(4) + 64 octets de nom, dans la case, donc dans le mapping.
        let l = unsafe {
            (*(map.as_ptr().add(self.base + 20 + 64) as *const AtomicU8)).load(Ordering::Acquire)
        };
        (l <= 63).then_some(l)
    }

    /// Tente de RÉSERVER la case (EMPTY → CLAIMED). `false` = un pair l'a eue.
    fn reserver(self, map: &MmapMut) -> bool {
        // SAFETY: même raison que `etat`.
        let a = unsafe { &*(map.as_ptr().add(self.base) as *const AtomicU8) };
        a.compare_exchange(VIDE, RESERVEE, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
}

/// ── LES REGISTRES PARTAGÉS D'UN SUBSTRAT ────────────────────────────────────
#[derive(Debug)]
pub struct RegistresPartages {
    chemin: PathBuf,
    map: MmapMut,
    /// Gardé pour la durée de vie du mapping (fermer le descripteur ne détruit
    /// pas un mapping sur Linux, mais le garder rend la structure lisible).
    _fichier: File,
}

impl RegistresPartages {
    /// Chemin du fichier pour un répertoire de substrat.
    #[must_use]
    pub fn chemin_pour(dir: &Path) -> PathBuf {
        dir.join(FILENAME_REGISTRES_PARTAGES)
    }

    /// Ouvre (ou crée) les registres partagés d'un répertoire de substrat.
    ///
    /// Un fichier d'une autre magie, d'une autre version de layout ou d'une
    /// taille inattendue est RÉINITIALISÉ. C'est sans danger : la table
    /// d'internement n'est pas la source de vérité (le dictionnaire et le WAL le
    /// sont), et le store la REPUBLIE depuis le dictionnaire juste après
    /// l'ouverture de chaque processus — le pire cas est un nom interné deux
    /// fois, visible, jamais une donnée fausse.
    pub fn ouvrir(dir: &Path) -> SubstrateResult<Self> {
        let chemin = Self::chemin_pour(dir);
        let fichier = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&chemin)?;
        if fichier.metadata()?.len() != TAILLE as u64 {
            // Taille différente (fichier neuf = 0, ou tronqué) : on la remet à
            // TAILLE. Aucun `set_len` plus petit n'arrive ici (TAILLE est
            // constante), donc on n'efface jamais une table existante par
            // accident de dimension.
            fichier.set_len(TAILLE as u64)?;
        }
        // SAFETY: le fichier vient d'être dimensionné à TAILLE octets ; la taille
        // ne bouge plus pour toute la vie de l'objet (aucun `set_len` postérieur),
        // donc l'adresse des atomes est stable — condition du partage entre
        // processus (un remap changerait l'adresse sous les pieds d'un pair).
        let map = unsafe { MmapOptions::new().len(TAILLE).map_mut(&fichier)? };
        let registres = Self {
            chemin,
            map,
            _fichier: fichier,
        };
        if mot(&registres.map, OFF_MAGIE).load(Ordering::Acquire) != MAGIE
            || mot(&registres.map, OFF_VERSION).load(Ordering::Acquire) != VERSION_LAYOUT
        {
            registres.initialiser();
        }
        Ok(registres)
    }

    /// (Ré)initialise le fichier : magie, version, compteurs à leur premier
    /// indice, borne du filigrane à zéro, et les TROIS tables (cases, revendications
    /// d'indices, région dense) remises à zéro. `fetch_max` sur une base DÉJÀ
    /// publiée ne recule pas — mais une réinitialisation ne peut pas être partielle :
    /// on écrit la magie EN DERNIER (`Release`), pour qu'un pair ne voie jamais un
    /// fichier à moitié initialisé comme valide.
    ///
    /// Le nettoyage des tables est ce qui rend le CHANGEMENT DE LAYOUT inoffensif :
    /// un fichier d'une version antérieure porte des cases/revendications/noms d'une
    /// AUTRE époque, et la republication depuis le dictionnaire buterait sur de
    /// fausses collisions (ou des noms fantômes dans la région dense) si elles
    /// n'étaient pas effacées. C'est le chemin « réinitialisation republiée depuis
    /// le dict » qu'un incrément de [`VERSION_LAYOUT`] déclenche.
    fn initialiser(&self) {
        for e in [Espace::Labels, Espace::Types, Espace::Cles] {
            let (off_mot, off_cap) = e.offsets_compteur();
            mot(&self.map, off_cap).store(e.dernier_indice(), Ordering::Relaxed);
            mot(&self.map, off_mot).store(e.premier_indice(), Ordering::Relaxed);
            // La borne du filigrane repart de zéro : rien n'est encore NOMMÉ.
            mot(&self.map, e.offset_filigrane()).store(0, Ordering::Relaxed);
            // Cases + revendications d'indices + région dense → zéro. Zéro est
            // l'état VIDE d'une case, l'état « libre » d'une revendication, et le
            // TROU (longueur 0) d'un slot dense : un seul memset remet les trois
            // tables dans l'état d'un fichier neuf.
            self.zero(e.offset_espace(), TAILLE_CASES + TAILLE_INDICES + TAILLE_NOMS_DENSES);
        }
        mot(&self.map, OFF_BASE_LABELS).store(0, Ordering::Relaxed);
        mot(&self.map, OFF_BASE_TYPES).store(0, Ordering::Relaxed);
        mot(&self.map, OFF_BASE_CLES).store(0, Ordering::Relaxed);
        mot(&self.map, OFF_VERSION).store(VERSION_LAYOUT, Ordering::Release);
        mot(&self.map, OFF_MAGIE).store(MAGIE, Ordering::Release);
        let _ = self.map.flush();
    }

    /// Remet à zéro une plage d'octets du mapping (`memset` 0).
    ///
    /// Appelée UNIQUEMENT depuis `initialiser`, où aucun pair ne peut lire un état
    /// valide (la magie n'est pas encore écrite) : aucune écriture atomique ne
    /// concurrence ce `memset`, donc aucun risque de course.
    fn zero(&self, base: usize, len: usize) {
        debug_assert!(base + len <= self.map.len());
        // SAFETY: la plage est bornée par la taille du mapping (`base + len ≤ TAILLE`),
        // alignée sur une page, et le mapping est `MAP_SHARED` exclusif pendant
        // l'initialisation. Même contrat que les écritures de `remplir`.
        unsafe {
            std::ptr::write_bytes(self.map.as_ptr().add(base).cast_mut(), 0u8, len);
        }
    }

    /// Chemin du fichier.
    #[must_use]
    pub fn chemin(&self) -> &Path {
        &self.chemin
    }

    /// Réserve un indice neuf dans un espace : `fetch_add` sur le compteur
    /// partagé — la valeur rendue est l'indice réservé. Deux processus ne peuvent
    /// pas recevoir le même, sans verrou.
    ///
    /// Marque un indice comme PRIS, ou rend `Err` s'il l'est déjà par un AUTRE nom.
    ///
    /// C'est la moitié « DÉTECTION » de la pièce : deux noms différents ne peuvent
    /// pas se partager un indice, et le refus est IMMÉDIAT — une lecture et un
    /// `compare_exchange` sur 4 octets, là où retrouver le nom par le hash coûtait
    /// un balayage des 8 192 cases (160 ms par nom neuf, mesuré au banc).
    fn revendiquer_indice(&self, e: Espace, indice: u32, nom: &str) -> SubstrateResult<()> {
        let a = self.mot_indice(e, indice);
        if a.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            return Ok(());
        }
        // Déjà marqué. Par NOUS (le même nom revient : rejeu du WAL, republication
        // du dictionnaire) — ou par un AUTRE nom, et c'est la collision. Ce chemin
        // est RARE, donc nommer le détenteur par la table de noms y est acceptable.
        let detenteur = self
            .nom_de_l_indice(e, indice)
            .unwrap_or_else(|| "<nom non publié>".to_string());
        if detenteur == nom {
            return Ok(());
        }
        Err(crate::error::SubstrateError::Internal(format!(
            "registres partagés : collision d'indices sur `{}` — l'indice {indice} est \
             DÉJÀ porté par {detenteur:?}, on refuse d'y inscrire {nom:?} : deux noms au \
             même indice rendraient les données illisibles (label_bitset, edge_type, \
             prop_key_id ne portent que l'indice). Repartir de la table (supprimer {}) \
             ou renommer l'un des deux.",
            e.nom(),
            FILENAME_REGISTRES_PARTAGES
        )))
    }

    /// Le mot de REVENDICATION de l'indice `indice` (0 = libre, 1 = pris).
    fn mot_indice(&self, e: Espace, indice: u32) -> &AtomicU32 {
        let off = e.offset_indices() + (indice as usize % CAPACITE_INDICES) * TAILLE_SLOT_INDICE;
        debug_assert!(off + 4 <= self.map.len());
        // SAFETY: offset aligné sur 4 dans un mapping aligné sur une page, et borné
        // par la taille de la table d'indices — la vue vit aussi longtemps que `map`.
        unsafe { &*(self.map.as_ptr().add(off) as *const AtomicU32) }
    }

    /// Réserve un indice neuf ET le revendique (la revendication refuse la
    /// collision avec un nom DÉJÀ en place). Le compteur peut être consommé sans que
    /// l'indice serve — un indice est un NUMÉRO, le manquer ne coûte rien.
    ///
    /// `Err` si l'espace est épuisé (u16 plein, ou 64 bits pour les labels) : on
    /// refuse BRUYAMMENT plutôt que de rendre un indice qui ne tiendrait pas dans le
    /// type des données.
    fn reserver_indice(&self, e: Espace, nom: &str) -> SubstrateResult<u32> {
        let (off_mot, off_cap) = e.offsets_compteur();
        let capacite = mot(&self.map, off_cap).load(Ordering::Relaxed);
        let v = mot(&self.map, off_mot).fetch_add(1, Ordering::AcqRel);
        if v > capacite {
            // Deux formulations, une seule vérité : l'espace de CE type est plein.
            // Les clés et les types d'arêtes s'arrêtent au `u16` (`prop_key_id`,
            // `edge_type`) ; les labels s'arrêtent au 64ᵉ BIT, et l'appelant de
            // `add_label` documente cette limite sous le nom « label registry
            // overflow » — on la NOMME donc comme lui.
            return Err(crate::error::SubstrateError::Internal(match e {
                Espace::Labels => format!(
                    "label registry overflow: more than 64 distinct labels — le bit {v} \
                     ne tient pas dans `NodeRecord.label_bitset` (64 bits). La table \
                     PARTAGÉE refuse le même 65ᵉ nom que le registre local : une base ne \
                     porte pas plus d'étiquettes, quel que soit le nombre d'écrivains."
                ),
                Espace::Types | Espace::Cles => format!(
                    "registres partagés : espace d'indices `{}` ÉPUISÉ ({v} > {capacite}). \
                     L'indice ne tiendrait pas dans le type des données — on refuse plutôt \
                     que d'écrire un identifiant faux.",
                    e.nom()
                ),
            }));
        }
        let indice = v as u32;
        self.revendiquer_indice(e, indice, nom)?;
        Ok(indice)
    }

    /// Adresse du slot dense de l'indice `indice` (`[indice × 64]` dans la région).
    fn base_dense(&self, e: Espace, indice: u32) -> usize {
        debug_assert!(indice < CAPACITE_INDICES as u32, "indice hors région dense");
        let off = e.offset_noms_denses() + indice as usize * TAILLE_NOM_DENSE;
        debug_assert!(off + TAILLE_NOM_DENSE <= self.map.len());
        off
    }

    /// Le mot de la borne du filigrane d'un espace.
    fn borne_filigrane(&self, e: Espace) -> &AtomicU64 {
        mot(&self.map, e.offset_filigrane())
    }

    /// ── PUBLIE UN COUPLE (INDICE, NOM) DANS UNE CASE RÉSERVÉE ─────────────────
    ///
    /// C'est le point d'écriture UNIQUE de tout nom : `interner` (nom neuf) et
    /// `publier` (dictionnaire) passent par ici. L'ordre des écritures est ce qui
    /// rend la table lisible sans verrou, et il s'étend désormais à DEUX lecteurs :
    ///
    /// 1. la CASE (hash, indice, nom, longueur) est publiée par l'état `REMPLIE`
    ///    (`Release`) — c'est ce que lisent `interner_si` et `indice_de` ;
    /// 2. la RÉGION DENSE (`[indice × 64]`) est publiée par sa propre LONGUEUR
    ///    (`Release`) — c'est ce que lisent `noms_indexes` et `nom_de_l_indice`.
    ///
    /// La BORNE du filigrane est avancée APRÈS l'écriture des octets denses
    /// (`fetch_max(indice + 1)`), et AVANT l'état `REMPLIE`. Un lecteur qui voit la
    /// borne lit donc un nom dense ENTIER ; un lecteur qui voit `REMPLIE` lit une
    /// case ENTIÈRE. La borne suit les NOMS, jamais le compteur de réservations.
    fn remplir(&self, e: Espace, c: Case, hash: u64, indice: u32, nom: &[u8]) {
        let map = &self.map;
        let len = nom.len().min(63) as u8;

        // ── 1. La case : hash, indice, octets, longueur (publication par l'état). ─
        mot(map, c.base + 8).store(hash, Ordering::Relaxed);
        // SAFETY: offsets alignés sur 4/8 et dans la case, mapping page-aligné.
        unsafe {
            (*(map.as_ptr().add(c.base + 16) as *const AtomicU32))
                .store(indice, Ordering::Relaxed);
            let p = map.as_ptr().add(c.base + 20).cast_mut();
            std::ptr::copy_nonoverlapping(nom.as_ptr(), p, usize::from(len));
            (*(map.as_ptr().add(c.base + 20 + 64) as *const AtomicU8))
                .store(len, Ordering::Relaxed);
        }

        // ── 2. La région dense : le nom à `[indice × 64]`, publié par sa LONGUEUR. ─
        let slot = self.base_dense(e, indice);
        // SAFETY: `slot` est aligné sur 8 (multiple de 64) et tient dans le mapping.
        unsafe {
            let p = map.as_ptr().add(slot + 1).cast_mut();
            std::ptr::copy_nonoverlapping(nom.as_ptr(), p, usize::from(len));
            // La LONGUEUR du slot est l'atome qui publie les octets denses : elle
            // est écrite en dernier, `Release`, donc un lecteur qui la voit (`Acquire`)
            // lit un nom ENTIER. `0` = trou (indice jamais nommé).
            (*(map.as_ptr().add(slot) as *const AtomicU8)).store(len, Ordering::Release);
        }

        // ── 3. La borne du filigrane : `1 + le plus grand indice NOMMÉ`. ────────
        // `fetch_max` (RMW) : monotone, insensible à l'ordre des écrivains, et son
        // `Release` s'insère dans la release sequence du mot — un lecteur `Acquire`
        // voit tous les noms déjà posés.
        self.borne_filigrane(e)
            .fetch_max(u64::from(indice) + 1, Ordering::AcqRel);

        // ── 4. L'état REMPLIE, en dernier (`Release` publie la case). ────────────
        // SAFETY: même raison que `Case::etat`.
        unsafe {
            (*(map.as_ptr().add(c.base) as *const AtomicU8)).store(REMPLIE, Ordering::Release);
        }
    }

    /// Relève la borne d'indices depuis une base (`fetch_max`) — utilisé quand un
    /// dictionnaire porte des indices plus hauts que ce que la table a vu (un
    /// pair a interné puis est mort sans flush : c'est le WAL, pas la table, qui
    /// porte son nom, mais l'indice qu'il a consommé ne doit pas être redistribué
    /// à un nom neuf).
    pub fn relever_base(&self, e: Espace, base: u64) {
        let (off_mot, _) = e.offsets_compteur();
        let off_base = match e {
            Espace::Labels => OFF_BASE_LABELS,
            Espace::Types => OFF_BASE_TYPES,
            Espace::Cles => OFF_BASE_CLES,
        };
        // Deux relevés, deux rôles : la BASE publiée (diagnostic / vérification de
        // divergence) et le COMPTEUR (ce qui empêche une redistribution).
        mot(&self.map, off_base).fetch_max(base, Ordering::AcqRel);
        mot(&self.map, off_mot).fetch_max(base, Ordering::AcqRel);
    }

    /// Nombre de réservations publiées sur le compteur d'un espace (diagnostic
    /// de banc : « la table a-t-elle servi ? »).
    #[must_use]
    pub fn reservations(&self, e: Espace) -> u64 {
        let (off_mot, _) = e.offsets_compteur();
        mot(&self.map, off_mot).load(Ordering::Relaxed)
    }

    fn case(&self, e: Espace, i: usize) -> Case {
        let base = e.offset_espace() + (i % CASES) * TAILLE_CASE;
        debug_assert!(base + TAILLE_CASE <= TAILLE);
        Case { base }
    }

    /// Première case sondée pour un hash. Le pas de sondage est IMPAIR (donc
    /// premier avec une puissance de deux) : la suite des cases visitées couvre
    /// toutes les cases avant de se répéter.
    #[inline]
    fn sonde(cle: u64, tour: usize) -> usize {
        let depart = (cle as usize) & (CASES - 1);
        let pas = ((cle >> 32) as usize) | 1;
        (depart + tour.wrapping_mul(pas)) & (CASES - 1)
    }

    /// ── INTERNE UN NOM, EN LE CRÉANT SI BESOIN ───────────────────────────────
    ///
    /// L'entrée publique : elle crée le nom s'il n'existe pas (c'est ce que fait un
    /// écrivain), et rend son indice — le même pour tout processus, quel que soit
    /// l'ordre d'arrivée.
    pub fn interner(&self, e: Espace, nom: &str) -> SubstrateResult<Option<u32>> {
        self.interner_si(e, nom, false)
    }

    /// ── INTERNE UN NOM (le cœur de la pièce) ─────────────────────────────────
    ///
    /// Rend l'indice du nom — LE MÊME pour tout processus, quel que soit l'ordre
    /// d'arrivée :
    ///
    /// 1. la table porte déjà le nom → son indice, terminé (chemin chaud : une
    ///    lecture, aucune écriture, aucune invalidation entre processus) ;
    /// 2. la table ne le porte pas → on réserve un indice NEUF sur le compteur
    ///    partagé, puis on écrit (nom, indice) dans la première case libre.
    ///
    /// `interdire` est là pour le chemin qui ne doit PAS créer de nom : la
    /// relecture du WAL et la publication du dictionnaire COMPLÈTENT une table,
    /// elles ne l'étendent pas (`publier`).
    fn interner_si(&self, e: Espace, nom: &str, interdire: bool) -> SubstrateResult<Option<u32>> {
        // Un nom vide n'est pas internable : les dictionnaires s'en servent comme
        // TROU (voir `store.rs`), et l'interner ferait converger tous les trous
        // sur une seule case.
        if nom.is_empty() {
            return Ok(None);
        }
        let octets = nom.as_bytes();
        if octets.len() > 63 {
            return Err(crate::error::SubstrateError::Internal(format!(
                "registres partagés : nom de {} octets > 63 ({}…), impossible à \
                 internerner — la case ne le porterait pas",
                octets.len(),
                nom.chars().take(16).collect::<String>()
            )));
        }
        let cle = hachage(octets);
        let mut libre: Option<u32> = None;
        for tour in 0..SONDES_MAX {
            let c = self.case(e, Self::sonde(cle, tour));
            match c.etat(&self.map) {
                REMPLIE => {
                    if c.hash(&self.map) != cle {
                        continue;
                    }
                    let Some(len) = c.longueur(&self.map) else {
                        continue;
                    };
                    if c.nom(&self.map, len) == octets {
                        return Ok(Some(c.indice(&self.map)));
                    }
                }
                RESERVEE => {
                    // Un pair est EN TRAIN d'écrire cette case, et on ne peut pas
                    // savoir si c'est notre nom avant qu'il ait fini : l'attente est
                    // INCONDITIONNELLE (parier sur le hash serait parier sur la clé
                    // de placement d'un nom qu'on ne connaît pas encore).
                    return self.attendre(e, cle, octets, interdire);
                }
                _ => {
                    // Une case LIBRE : candidate, mais on termine la sonde avant
                    // d'y écrire. S'arrêter ici serait un pari — le nom peut être
                    // dans une case plus loin, atteinte par un autre chemin de sonde.
                    libre = Some(tour as u32);
                    break;
                }
            }
        }
        let Some(tour) = libre else {
            return Err(crate::error::SubstrateError::Internal(format!(
                "registres partagés : aucune case libre pour {:?} après {} sondes \
                 (espace `{}`) — la table est saturée ; on refuse plutôt que de \
                 rendre un indice non partagé",
                nom,
                SONDES_MAX,
                e.nom()
            )));
        };
        if interdire {
            return Ok(None);
        }
        self.ecrire(e, cle, octets, Self::sonde(cle, tour as usize))
    }

    /// Réserve l'indice (compteur partagé) puis publie (nom, indice) dans la case.
    fn ecrire(
        &self,
        e: Espace,
        cle: u64,
        octets: &[u8],
        tour: usize,
    ) -> SubstrateResult<Option<u32>> {
        let c = self.case(e, tour);
        if !c.reserver(&self.map) {
            // Course perdue : un pair a réservé CETTE case. On relit la table —
            // c'est ce mécanisme, et lui seul, qui fait converger deux processus
            // sur le même indice sans qu'ils se parlent.
            return self.attendre(e, cle, octets, false);
        }
        let indice = self.reserver_indice(e, &String::from_utf8_lossy(octets))?;
        self.remplir(e, c, cle, indice, octets);
        Ok(Some(indice))
    }

    /// ── CE QUE FAIT UN INTERNEMENT QUI A TROUVÉ UN PAIR EN COURS ──────────────
    ///
    /// Une case `RESERVEE` par un pair vivant veut dire « un nom est en train
    /// d'entrer dans la table, et je ne sais pas encore lequel ». Tant qu'il ne
    /// l'a pas publié, on ne peut NI rendre un indice (on ne connaît pas le nom),
    /// NI en poser un ailleurs : deux cases pour un même nom voudraient dire deux
    /// indices, et l'un des deux désignerait le mauvais libellé dans les données.
    ///
    /// On attend donc — BORNÉMENT — puis on repose la question. Cette fonction
    /// n'écrit JAMAIS dans une case libre : elle observe ou elle écrit dans la
    /// case que le pair a réservée. Le cas qui reste (le pair est MORT au milieu
    /// de son insertion, sa case reste `RESERVEE` à jamais) est rendu BRUYAMMENT :
    /// on refuse plutôt que de créer un second indice. Une insertion gelée par un
    /// `SIGKILL` coûte un nom à réinterner à la prochaine ouverture, pas une
    /// donnée fausse.
    fn attendre(
        &self,
        e: Espace,
        cle: u64,
        octets: &[u8],
        interdire: bool,
    ) -> SubstrateResult<Option<u32>> {
        let debut = std::time::Instant::now();
        loop {
            for tour in 0..SONDES_MAX {
                let c = self.case(e, Self::sonde(cle, tour));
                match c.etat(&self.map) {
                    REMPLIE => {
                        if c.hash(&self.map) != cle {
                            continue;
                        }
                        let Some(len) = c.longueur(&self.map) else {
                            continue;
                        };
                        if c.nom(&self.map, len) == octets {
                            return Ok(Some(c.indice(&self.map)));
                        }
                        // Même hash, nom différent : un AUTRE nom a été publié sur
                        // notre sonde. Collision de placement ordinaire, on continue.
                    }
                    RESERVEE => break,
                    _ => {
                        // La sonde se termine sur une case libre : le nom n'est
                        // TOUJOURS pas dans la table, et plus aucun pair n'écrit
                        // dessus. C'est le seul endroit d'où l'on peut écrire sans
                        // risquer un second indice.
                        if interdire {
                            return Ok(None);
                        }
                        return self.ecrire(e, cle, octets, Self::sonde(cle, tour));
                    }
                }
            }
            if debut.elapsed() >= ATTENTE_TOTALE {
                return Err(crate::error::SubstrateError::Internal(format!(
                    "registres partagés : un pair est resté plus de {:?} sur une case \
                     `RESERVEE` de `{}` — son insertion est GELÉE (processus tué). On \
                     refuse de créer un second indice pour ce nom : il sera réinterné \
                     à la prochaine ouverture, où la table repart du dictionnaire.",
                    ATTENTE_TOTALE,
                    e.nom()
                )));
            }
            std::hint::spin_loop();
            std::thread::yield_now();
        }
    }

    /// ── PUBLIE UN COUPLE (INDICE, NOM) DÉJÀ CONNU ────────────────────────────
    ///
    /// C'est le chemin de la COMPATIBILITÉ : un store existant a ses noms dans
    /// `substrate.dict` AVEC leurs indices. Les republier tels quels dans la
    /// table fait que les noms déjà internés gardent leur indice — aucune
    /// migration, aucune donnée réécrite. Seuls les noms neufs consomment le
    /// compteur.
    ///
    /// Rend `Ok(true)` si la table portait déjà ce nom au MÊME indice (le cas
    /// normal), `Ok(false)` si le nom manquait et a été publié à cet indice.
    /// Une case occupée par un AUTRE nom au même indice est une COLLISION
    /// d'identifiants : elle est rendue en `Err` pour être DITE, jamais tue.
    pub fn publier(
        &self,
        e: Espace,
        indice: u32,
        nom: &str,
    ) -> SubstrateResult<bool> {
        // Un nom vide est un TROU du dictionnaire (indice consommé, nom jamais
        // connu de ce processus) : on relève la base pour ne pas le redistribuer,
        // sans écrire de case.
        if nom.is_empty() {
            self.relever_base(e, u64::from(indice) + 1);
            return Ok(true);
        }
        // La région dense ne porte que `CAPACITE_INDICES` slots : un indice au-delà
        // n'a pas de case dense où poser son nom, et `remplir` écrirait hors du
        // mapping. Le dictionnaire ne peut pas en produire (u16 / 64 bits) ; ce
        // garde-fou transforme un fichier bricolé en erreur dite, pas en écriture
        // sauvage.
        if indice as usize >= CAPACITE_INDICES {
            return Err(crate::error::SubstrateError::Internal(format!(
                "registres partagés : indice {indice} hors de la région dense de `{}` \
                 (≤ {}) — le dictionnaire ne peut pas porter un tel indice",
                e.nom(),
                CAPACITE_INDICES - 1
            )));
        }
        let octets = nom.as_bytes();
        if octets.len() > 63 {
            return Err(crate::error::SubstrateError::Internal(format!(
                "registres partagés : nom de {} octets > 63 pour l'indice {indice} \
                 de `{}` — publier un nom tronqué ferait diverger les registres",
                octets.len(),
                e.nom()
            )));
        }
        let cle = hachage(octets);
        // On republie un couple que le DICTIONNAIRE connaît. La seule chose qui
        // puisse faire échouer cette publication est qu'un AUTRE nom occupe DÉJÀ cet
        // indice — la collision que cette pièce existe pour empêcher. On la cherche
        // donc sur TOUT l'espace, et pas seulement sur la sonde de ce nom : deux noms
        // différents ne commencent pas leur sonde au même endroit, donc une collision
        // peut vivre entièrement hors du chemin de sonde de l'un des deux.
        //
        // LE COÛT EST CELUI DE L'OUVERTURE, ET IL EST BORNÉ : `publier` n'est appelée
        // que pour les noms DU DICTIONNAIRE, une fois par processus à l'ouverture.
        // Les espaces sont dimensionnés (8 192 cases) pour que ce balayage reste une
        // lecture de quelques mégaoctets, là où le contourner par un pari sur la sonde
        // laisserait passer exactement le défaut que la pièce ferme (mesuré : deux
        // noms divergents tombant dans des cases non adjacentes).
        // ── L'INDICE EST-IL DÉJÀ PRIS PAR UN AUTRE NOM ? ─────────────────────
        // La réponse est une lecture (table « un mot par indice »), pas un
        // balayage : c'était 160 ms par nom neuf au banc. `revendiquer_indice`
        // refuse la collision en nommant le nom déjà en place.
        self.revendiquer_indice(e, indice, nom)?;
        let debut = std::time::Instant::now();
        loop {
            for tour in 0..SONDES_MAX {
                let c = self.case(e, Self::sonde(cle, tour));
                match c.etat(&self.map) {
                    REMPLIE => {
                        let Some(len) = c.longueur(&self.map) else {
                            continue;
                        };
                        let porte = c.nom(&self.map, len);
                        if porte == octets {
                            let connu = c.indice(&self.map);
                            if connu != indice {
                                return Err(crate::error::SubstrateError::Internal(format!(
                                    "registres partagés : le nom {:?} est DÉJÀ interné à \
                                     l'indice {connu} de `{}`, et le dictionnaire le place \
                                     à {indice} — deux processus ont interné des noms \
                                     différents au même indice ; publier celui-ci \
                                     écraserait l'autre.",
                                    nom,
                                    e.nom()
                                )));
                            }
                            self.relever_base(e, u64::from(indice) + 1);
                            return Ok(true);
                        }
                        if c.indice(&self.map) == indice {
                            // Cette case porte NOTRE indice sous un AUTRE nom —
                            // c'est la collision, déjà cherchée juste avant. On la
                            // rend ici aussi pour ne jamais écrire un second nom sur
                            // un identifiant pris.
                            return Err(crate::error::SubstrateError::Internal(format!(
                                "registres partagés : collision d'indices sur `{}` — \
                                 l'indice {indice} désigne {:?} dans la table et {:?} \
                                 dans le dictionnaire",
                                e.nom(),
                                String::from_utf8_lossy(&porte),
                                nom
                            )));
                        }
                        continue;
                    }
                    RESERVEE => {
                        // Un pair écrit cette case. On attend sa publication, puis on
                        // recommence la sonde — mais on ne pose JAMAIS ce couple dans
                        // une case neuve pendant ce temps : deux cases pour un même nom
                        // voudraient dire deux indices, donc deux libellés pour un bit.
                        if debut.elapsed() < ATTENTE_TOTALE {
                            std::hint::spin_loop();
                            std::thread::yield_now();
                            // On reprend TOUTE la sonde (le pair peut avoir publié un
                            // nom au-delà du tour courant).
                            break;
                        }
                        // Insertion gelée (processus tué). Le dictionnaire porte
                        // encore ce nom à cet indice : on relève la base et on rend
                        // « déjà là » — le nom sera republié à la prochaine
                        // ouverture, quand la table repartira de zéro.
                        self.relever_base(e, u64::from(indice) + 1);
                        return Ok(true);
                    }
                    _ => {
                        // Case libre : le nom n'est pas dans la table, on peut le
                        // publier à l'indice que le dictionnaire IMPOSE. Aucune
                        // réservation de compteur ici — cet indice a déjà été consommé
                        // par celui qui l'a écrit ; le compteur est seulement RELEVÉ
                        // (jamais reculé).
                        if !c.reserver(&self.map) {
                            break;
                        }
                        self.remplir(e, c, cle, indice, octets);
                        self.relever_base(e, u64::from(indice) + 1);
                        return Ok(false);
                    }
                }
            }
            if debut.elapsed() >= ATTENTE_TOTALE {
                return Err(crate::error::SubstrateError::Internal(format!(
                    "registres partagés : impossible de publier {:?} (indice {indice}, \
                     espace `{}`) — une case est restée `RESERVEE` plus de {:?}, et \
                     aucune n'est libre dans la sonde",
                    nom,
                    e.nom(),
                    ATTENTE_TOTALE
                )));
            }
        }
    }

    /// Le NOM que la table associe à `indice` — une lecture de la région dense.
    ///
    /// Sert à deux choses, et la seconde est ce qui rend la table utile EN LECTURE :
    ///
    /// 1. `publier` s'en sert pour refuser une collision d'indices (deux noms pour
    ///    un identifiant) ;
    /// 2. le magasin s'en sert pour RÉSOUDRE un indice qu'il n'a jamais interné
    ///    lui-même — le cas d'un pair qui a nommé un bit APRÈS notre ouverture. Le
    ///    dictionnaire, lui, n'est relu qu'à l'ouverture : sans cette résolution, un
    ///    nœud vivant écrit par le pair et portant ce bit afficherait un libellé
    ///    VIDE alors que son nom est dans la table, à côté.
    ///
    /// La réponse est la région dense `[indice × 64]` : une lecture `Acquire` de la
    /// LONGUEUR du slot (qui publie les octets, `Release` dans `remplir`), puis des
    /// octets du nom. O(1) au lieu du balayage des 8 192 cases.
    #[must_use]
    pub fn nom_de_l_indice(&self, e: Espace, indice: u32) -> Option<String> {
        if indice >= CAPACITE_INDICES as u32 {
            return None;
        }
        let slot = self.base_dense(e, indice);
        // SAFETY: `slot` est aligné et `slot + 64` tient dans la région dense.
        let len = unsafe { (*(self.map.as_ptr().add(slot) as *const AtomicU8)).load(Ordering::Acquire) };
        if len == 0 || len > 63 {
            // Un trou (indice réservé/relevé mais jamais nommé), ou une publication
            // incohérente : on ne lit pas hors du slot.
            return None;
        }
        // SAFETY: `len ≤ 63` donc `slot + 1 + len ≤ slot + 64`, dans la région.
        let nom = unsafe {
            std::slice::from_raw_parts(self.map.as_ptr().add(slot + 1), usize::from(len))
        };
        Some(String::from_utf8_lossy(nom).into_owned())
    }

    /// ── L'ÉTENDUE NOMMÉE D'UN ESPACE, ET CE QU'ELLE NOMME ───────────────────
    ///
    /// Rend un vecteur DENSE indexé par indice : `rendu[i]` est le nom que la table
    /// porte à l'indice `i`, ou `None` si elle n'y nomme rien. Sa LONGUEUR est le
    /// FILIGRANE de l'espace : `1 +` le plus grand indice NOMMÉ (`0` si la table
    /// n'y nomme rien).
    ///
    /// ── POURQUOI CETTE LECTURE EXISTE (le défaut mesuré) ────────────────────
    /// La fusion du dictionnaire (`store.rs::fusionner_noms`) bornait son étendue à
    /// `max(disque.len(), memoire.len())`. Elle comblait donc les trous DANS cette
    /// étendue, mais ne pouvait pas l'ÉTENDRE vers un indice que seule la table
    /// nomme — c'est-à-dire vers le nom d'un pair interné APRÈS notre ouverture
    /// (notre registre local, lui, ne l'apprend qu'au rejeu du WAL, donc à la
    /// prochaine ouverture). Le processus au registre le plus COURT écrivait alors
    /// en dernier et AMPUTAIT la queue du dictionnaire : le nom du pair disparaissait
    /// du disque alors que ses nœuds portaient son bit.
    ///
    /// Mesuré (`mw_registres_r1`, ~30 % des runs) : dictionnaire final
    /// `["MWR", "Alpha"]` (ou `["MWR", "Beta"]`) alors que la table nommait les
    /// trois noms — l'un des deux écrivains perdu à chaque fois.
    ///
    /// ── POURQUOI ON LIT LES NOMS, ET PAS LE COMPTEUR ────────────────────────
    /// Le compteur partagé dit combien d'indices ont été RÉSERVÉS, pas combien sont
    /// NOMMÉS : une réservation dont l'internement n'a pas abouti (échec après
    /// réservation, ou processus tué entre les deux) le laisse DEVANT, et une
    /// étendue tirée du compteur ferait écrire une queue de trous à chaque
    /// publication — un dictionnaire qui enfle sans qu'aucun nom n'existe. Le
    /// filigrane cherché est celui des NOMS ; c'est donc la BORNE du filigrane
    /// qu'on lit, avancée par `fetch_max(indice + 1)` dans `remplir` — jamais le
    /// compteur de réservations.
    ///
    /// ── POURQUOI C'EST O(1) DÉSORMAIS ───────────────────────────────────────
    /// Deux lectures suffisent : la BORNE (le filigrane, un `AtomicU64` par espace)
    /// puis la RÉGION DENSE `[indice × 64]`, de 0 à la borne. Le balayage des 8 192
    /// cases qui la précédait coûtait ~4,1 µs/espace même VIDE, +~42 ns/nom — un
    /// coût fixe par espace et par publication du dictionnaire. Il disparaît : le
    /// coût est désormais celui de la borne (un seul mot) plus une lecture linéaire
    /// de `filigrane` slots, sans indirection.
    #[must_use]
    pub fn noms_indexes(&self, e: Espace) -> Vec<Option<String>> {
        // La borne = `1 + le plus grand indice NOMMÉ`, lue en O(1). `min(CAPACITE_INDICES)`
        // borne la lecture à la région dense (un fichier bricolé ne peut pas faire
        // lire hors du mapping).
        let filigrane = (self.borne_filigrane(e).load(Ordering::Acquire) as usize)
            .min(CAPACITE_INDICES);
        if filigrane == 0 {
            return Vec::new();
        }
        let base = e.offset_noms_denses();
        let mut out = Vec::with_capacity(filigrane);
        for i in 0..filigrane {
            let slot = base + i * TAILLE_NOM_DENSE;
            // SAFETY: `slot` est aligné sur 8 (multiple de 64) et `slot + 64` reste
            // dans la région dense (`i < filigrane ≤ CAPACITE_INDICES`).
            let len = unsafe {
                (*(self.map.as_ptr().add(slot) as *const AtomicU8)).load(Ordering::Acquire)
            };
            if len == 0 || len > 63 {
                // Un TROU (indice réservé/relevé, jamais nommé) : `None`, jamais une
                // chaîne vide — le contrat de `publier` (un nom vide n'est pas un nom).
                out.push(None);
                continue;
            }
            // SAFETY: `len ≤ 63` donc `slot + 1 + len ≤ slot + 64`, dans la région.
            let nom = unsafe {
                std::slice::from_raw_parts(self.map.as_ptr().add(slot + 1), usize::from(len))
            };
            out.push(Some(String::from_utf8_lossy(nom).into_owned()));
        }
        out
    }

    /// L'indice partagé d'un nom, s'il est connu — sans RIEN créer.
    ///
    /// C'est la question du chemin chaud : « un pair a-t-il déjà interné ce nom
    /// sous un indice que je ne connais pas ? ».
    #[must_use]
    pub fn indice_de(&self, e: Espace, nom: &str) -> Option<u32> {
        if nom.is_empty() || nom.len() > 63 {
            return None;
        }
        let octets = nom.as_bytes();
        let cle = hachage(octets);
        for tour in 0..SONDES_MAX {
            let c = self.case(e, Self::sonde(cle, tour));
            match c.etat(&self.map) {
                REMPLIE => {
                    if c.hash(&self.map) != cle {
                        continue;
                    }
                    // Une longueur illégale veut dire « case gelée à moitié
                    // écrite » : on CONTINUE la sonde. Rendre `None` ici ferait
                    // croire que le nom n'existe pas — donc qu'il faut en réserver
                    // un SECOND indice, ce que toute la pièce existe pour empêcher.
                    let Some(len) = c.longueur(&self.map) else {
                        continue;
                    };
                    if c.nom(&self.map, len) == octets {
                        return Some(c.indice(&self.map));
                    }
                }
                RESERVEE | VIDE => return None,
                _ => continue,
            }
        }
        None
    }

    /// Nombre de cases occupées d'un espace (diagnostic et tests).
    #[must_use]
    pub fn cases_occupees(&self, e: Espace) -> usize {
        (0..CASES)
            .filter(|i| self.case(e, *i).etat(&self.map) != VIDE)
            .count()
    }

    /// Force les pages sales vers le disque.
    ///
    /// Ce n'est PAS une exigence de correction : la table est reconstruite depuis
    /// le dictionnaire et le WAL à l'ouverture. C'est une politesse — sans elle,
    /// un redémarrage propre republierait tout le dictionnaire pour rien.
    pub fn synchroniser(&self) {
        let _ = self.map.flush();
    }
}

/// Hash d'un nom → clé de case.
///
/// FNV-1a 64 bits : gratuit (pas de table, pas de clé secrète), et surtout STABLE
/// pour toujours — c'est une fonction de PLACEMENT, pas de sécurité, et un nom
/// doit tomber dans la même case d'un processus à l'autre ET d'une version à
/// l'autre (sinon la réouverture ne retrouverait plus ses noms).
#[inline]
fn hachage(octets: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in octets {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    // 0 est l'état VIDE d'une case : un hash nul rendrait la case indistinguable
    // d'une case libre.
    if h == 0 { 1 } else { h }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meme_nom_meme_indice_et_idempotent() {
        let td = tempfile::tempdir().unwrap();
        let r = RegistresPartages::ouvrir(td.path()).unwrap();
        let a = r.interner(Espace::Labels, "Task").unwrap().unwrap();
        let b = r.interner(Espace::Labels, "Task").unwrap().unwrap();
        assert_eq!(a, b, "le même nom doit rendre le même indice");
        assert_eq!(r.cases_occupees(Espace::Labels), 1);
    }

    #[test]
    fn noms_distincts_indices_distincts() {
        let td = tempfile::tempdir().unwrap();
        let r = RegistresPartages::ouvrir(td.path()).unwrap();
        let a = r.interner(Espace::Labels, "Task").unwrap().unwrap();
        let b = r.interner(Espace::Labels, "Node").unwrap().unwrap();
        assert_ne!(a, b);
        assert_eq!(r.cases_occupees(Espace::Labels), 2);
    }

    #[test]
    fn les_types_et_cles_commencent_a_1() {
        let td = tempfile::tempdir().unwrap();
        let r = RegistresPartages::ouvrir(td.path()).unwrap();
        // 0 est la sentinelle nulle des données : jamais distribué.
        assert_eq!(r.interner(Espace::Types, "PART_OF").unwrap(), Some(1));
        assert_eq!(r.interner(Espace::Cles, "titre").unwrap(), Some(1));
        // Les labels, eux, ont un bit 0 légitime.
        assert_eq!(r.interner(Espace::Labels, "N").unwrap(), Some(0));
    }

    /// ── LE FILIGRANE D'UN ESPACE : CE QUE LA FUSION DU DICTIONNAIRE LIT ─────
    ///
    /// `noms_indexes` rend l'étendue NOMMÉE de la table (sa longueur) ET les noms
    /// qu'elle porte. C'est la troisième source de `fusionner_noms` (`store.rs`) :
    /// sans elle, un nom qu'un pair a interné APRÈS notre ouverture tombait
    /// au-delà de l'étendue de la fusion et le dictionnaire publié en était
    /// AMPUTÉ (`mw_registres_r1`, ~30 % des runs avant le correctif).
    #[test]
    fn noms_indexes_donne_le_filigrane_et_les_noms() {
        let td = tempfile::tempdir().unwrap();
        let r = RegistresPartages::ouvrir(td.path()).unwrap();
        // Table vide : ni filigrane, ni nom. C'est ce qui fait que la fusion
        // n'ajoute RIEN quand il n'y a pas de pair (le cas mono-écrivain).
        assert!(r.noms_indexes(Espace::Labels).is_empty());

        let a = r.interner(Espace::Labels, "Alpha").unwrap().unwrap();
        let b = r.interner(Espace::Labels, "Beta").unwrap().unwrap();
        assert_eq!((a, b), (0, 1));
        assert_eq!(
            r.noms_indexes(Espace::Labels),
            vec![Some("Alpha".to_string()), Some("Beta".to_string())],
            "un indice nommé est rendu à SA place, et la longueur est le filigrane"
        );

        // Un nom publié à un indice ESPACÉ (ce que le dictionnaire d'un pair peut
        // porter) étend le filigrane et laisse des TROUS, pas des noms vides.
        r.publier(Espace::Labels, 5, "Zeta").unwrap();
        let v = r.noms_indexes(Espace::Labels);
        assert_eq!(
            v.len(),
            6,
            "le filigrane est `1 + le plus grand indice NOMMÉ` (5), pas le nombre de noms"
        );
        assert_eq!(v[5], Some("Zeta".to_string()));
        assert!(
            v[2].is_none() && v[3].is_none() && v[4].is_none(),
            "un indice non nommé est un TROU (`None`), jamais une chaîne vide : {v:?}"
        );

        // Les types et les clés commencent à l'indice 1 (0 est la sentinelle
        // nulle des données) : le filigrane suit l'INDICE, pas le rang.
        assert!(r.noms_indexes(Espace::Types).is_empty());
        r.interner(Espace::Types, "PART_OF").unwrap();
        assert_eq!(
            r.noms_indexes(Espace::Types),
            vec![None, Some("PART_OF".to_string())],
            "l'indice 0 des types est un trou, pas un nom"
        );
    }

    /// ── LE FILIGRANE SUIT LES NOMS, PAS LE COMPTEUR ─────────────────────────
    ///
    /// Le compteur partagé avance à la RÉSERVATION, pas à la publication : il peut
    /// donc être DEVANT les noms (une réservation dont l'internement n'a pas abouti,
    /// ou un `relever_base` sur un indice consommé). S'il servait de filigrane à la
    /// fusion du dictionnaire, chaque publication écrirait une queue de trous —
    /// un dictionnaire qui enfle sans qu'aucun nom n'existe. Ce test fige le choix :
    /// on lit les NOMS.
    #[test]
    fn le_filigrane_ne_suit_pas_le_compteur_de_reservations() {
        let td = tempfile::tempdir().unwrap();
        let r = RegistresPartages::ouvrir(td.path()).unwrap();
        r.relever_base(Espace::Cles, 9);
        assert_eq!(
            r.reservations(Espace::Cles),
            9,
            "le compteur est bien devant (indices 1..=8 réservés, aucun nommé)"
        );
        assert!(
            r.noms_indexes(Espace::Cles).is_empty(),
            "aucun nom n'est publié : le filigrane doit rester VIDE malgré le compteur"
        );
    }

    /// ── LA BORNE DU FILIGRANE SUIT LES NOMS, PAS LE COMPTEUR (volet 1) ──────
    ///
    /// Le compteur peut être relevé très loin sans qu'aucun nom n'existe : la borne
    /// du filigrane doit rester à zéro. Puis un nom publié à un indice ESPACÉ la
    /// fait avancer EXACTEMENT à `indice + 1` — pas à `compteur + 1`. C'est la
    /// distinction que le correctif O(1) doit préserver en passant d'un balayage des
    /// cases à une borne avancée par `fetch_max`.
    #[test]
    fn la_borne_du_filigrane_suit_les_noms_pas_le_compteur() {
        let td = tempfile::tempdir().unwrap();
        let r = RegistresPartages::ouvrir(td.path()).unwrap();
        r.relever_base(Espace::Cles, 100);
        assert!(
            r.noms_indexes(Espace::Cles).is_empty(),
            "un relevé de base n'est pas un nom : le filigrane reste vide"
        );
        // Un nom publié à l'indice 7 (espacé) : la borne devient 8, pas 101.
        assert!(!r.publier(Espace::Cles, 7, "sept").unwrap());
        let v = r.noms_indexes(Espace::Cles);
        assert_eq!(
            v.len(),
            8,
            "filigrane = 1 + 7 (nommé), pas 1 + 100 (compteur)"
        );
        assert_eq!(v[7], Some("sept".to_string()));
        assert!(v[0].is_none(), "l'indice 0 des clés est un trou : {v:?}");
        assert!(v[1..7].iter().all(Option::is_none), "trous : {v:?}");
    }

    #[test]
    fn publier_preserve_l_indice_du_dictionnaire() {
        let td = tempfile::tempdir().unwrap();
        let r = RegistresPartages::ouvrir(td.path()).unwrap();
        // Un dictionnaire existant : "A" au bit 0, "B" au bit 1.
        assert!(r.publier(Espace::Labels, 0, "A").unwrap() == false);
        assert!(r.publier(Espace::Labels, 1, "B").unwrap() == false);
        // Republier est idempotent (vrai = déjà là, au même indice).
        assert!(r.publier(Espace::Labels, 0, "A").unwrap());
        // Et un nom NEUF après publication prend l'indice suivant, pas 0.
        assert_eq!(r.interner(Espace::Labels, "C").unwrap(), Some(2));
    }

    #[test]
    fn publier_un_nom_a_un_indice_divergent_le_DIT() {
        let td = tempfile::tempdir().unwrap();
        let r = RegistresPartages::ouvrir(td.path()).unwrap();
        r.publier(Espace::Labels, 0, "A").unwrap();
        let e = r.publier(Espace::Labels, 3, "A").unwrap_err();
        let m = e.to_string();
        assert!(m.contains("DÉJÀ interné"), "message inattendu : {m}");
    }

    #[test]
    fn collision_d_indice_entre_deux_noms_le_DIT() {
        let td = tempfile::tempdir().unwrap();
        let r = RegistresPartages::ouvrir(td.path()).unwrap();
        r.publier(Espace::Types, 7, "PART_OF").unwrap();
        // Un nom DIFFÉRENT au même indice : c'est le défaut que la pièce ferme.
        let e = r.publier(Espace::Types, 7, "KNOWS").unwrap_err();
        assert!(e.to_string().contains("collision d'indices"), "{e}");
    }

    #[test]
    fn interdire_ne_cree_aucun_nom() {
        let td = tempfile::tempdir().unwrap();
        let r = RegistresPartages::ouvrir(td.path()).unwrap();
        // `interner_si(.., true)` est le chemin de la RELECTURE (WAL, dictionnaire) :
        // il complète une table, il ne l'étend pas.
        assert_eq!(r.interner_si(Espace::Cles, "jamais", true).unwrap(), None);
        assert_eq!(r.cases_occupees(Espace::Cles), 0);
        assert_eq!(
            r.reservations(Espace::Cles),
            1,
            "le compteur d'indices ne doit PAS avoir bougé (départ à 1)"
        );
        // Et un nom qui EXISTE est rendu même en mode « ne crée pas ».
        r.interner(Espace::Cles, "connu").unwrap();
        assert_eq!(
            r.interner_si(Espace::Cles, "connu", true).unwrap(),
            Some(1),
            "un nom déjà interné doit être rendu, même quand on interdit de créer"
        );
    }

    #[test]
    fn un_nom_de_plus_de_63_octets_est_refuse() {
        let td = tempfile::tempdir().unwrap();
        let r = RegistresPartages::ouvrir(td.path()).unwrap();
        let long = "x".repeat(64);
        assert!(r.interner(Espace::Cles, &long).is_err());
    }

    #[test]
    fn un_nom_vide_n_est_pas_un_nom() {
        let td = tempfile::tempdir().unwrap();
        let r = RegistresPartages::ouvrir(td.path()).unwrap();
        assert_eq!(r.interner(Espace::Labels, "").unwrap(), None);
        assert_eq!(r.indice_de(Espace::Labels, ""), None);
        assert_eq!(r.cases_occupees(Espace::Labels), 0);
    }

    #[test]
    fn la_table_survit_a_une_reouverture_du_meme_processus() {
        let td = tempfile::tempdir().unwrap();
        let i = {
            let r = RegistresPartages::ouvrir(td.path()).unwrap();
            let i = r.interner(Espace::Cles, "alpha").unwrap().unwrap();
            r.synchroniser();
            i
        };
        let r2 = RegistresPartages::ouvrir(td.path()).unwrap();
        assert_eq!(r2.indice_de(Espace::Cles, "alpha"), Some(i));
        // …et un nom neuf ne RECOUVRE pas l'ancien.
        let j = r2.interner(Espace::Cles, "beta").unwrap().unwrap();
        assert_ne!(i, j);
    }

    #[test]
    fn le_hash_est_stable_et_jamais_nul() {
        assert_eq!(hachage(b"Task"), hachage(b"Task"));
        assert_ne!(hachage(b"Task"), hachage(b"Node"));
        assert_ne!(hachage(b""), 0);
    }

    #[test]
    fn la_sonde_couvre_toutes_les_cases() {
        // Le pas est impair : la suite ne visite chaque case qu'une fois avant de
        // boucler. Sans cela, deux noms pourraient se disputer indéfiniment un
        // sous-ensemble de cases.
        let cle = 0x0123_4567_89ab_cdefu64;
        let mut vues = std::collections::HashSet::new();
        for tour in 0..CASES {
            assert!(vues.insert(RegistresPartages::sonde(cle, tour)), "case revisitée");
        }
        assert_eq!(vues.len(), CASES);
    }
}
