//! # Index d'ÉGALITÉ persisté des propriétés (« PropsEqIndex »)
//!
//! ## Le besoin
//!
//! `property_indexes` est un index de HACHAGE en RAM
//! (`IndexValueKey → HashSet<NodeId>`) qui tranche l'ÉGALITÉ. Il MEURT AVEC
//! LE PROCESSUS : chaque ouverture le rebâtit par une passe complète sur les
//! chaînes PropsZone v2 (`batir_index_depuis_v2`) — à l'ouverture pour les
//! clés du catalogue, à la première requête pour les autres. Mesuré : 8,8 s
//! sur `messages` (454 073 nœuds), des minutes sur les bases larges — c'est
//! le grief des démarrages qui repaient indéfiniment le même balayage.
//!
//! Cette zone persiste une PROJECTION de cet index : des couples
//! `(hachage de valeur, slot)`, triés, par propriété. À la réouverture, si
//! les ancres de couverture correspondent, la passe de reconstruction est
//! ÉVITÉE : les candidats d'égalité se lisent ici, et la revérification
//! déjà en place chez l'appelant (`indexed_candidates` relit chaque candidat
//! par l'ordre de lecture complet) écarte collisions et entrées périmées.
//!
//! ## Ce que cet index est, et ce qu'il n'est JAMAIS
//!
//! **LES CHAÎNES DE PAGES SONT LA SOURCE DE VÉRITÉ. CET INDEX N'EN EST
//! QU'UNE PROJECTION.** Mêmes trois conséquences que pour l'index ordonné
//! ([`crate::props_index`]), tenues par construction :
//!
//! 1. Une entrée est `(hachage 8 o, slot 4 o)` — on ne peut RIEN en
//!    reconstituer. L'index énumère des CANDIDATS, il ne répond pas.
//! 2. Il se rebâtit intégralement depuis les seules chaînes
//!    (`batir_index_depuis_v2`). Corrompu, périmé, d'une autre version : on
//!    le jette.
//! 3. Il s'EFFACE au moindre doute — [`PropsEqIndexZone::fiable`] rend
//!    `false` et l'appelant retombe sur la reconstruction par processus,
//!    lente mais juste. `None`, JAMAIS `Some(vec![])` par ignorance.
//!
//! ## Pourquoi les FAUX POSITIFS sont sans danger, et les faux négatifs mortels
//!
//! `indexed_candidates` REVÉRIFIE chaque candidat (liveness + valeur
//! courante par `get_node_property`) avant de le rendre. Une collision de
//! hachage ou une entrée périmée ne produit qu'un candidat en trop, écarté.
//! Un candidat MANQUANT sortirait du résultat sans erreur ni trace — d'où :
//!
//! * l'index doit être un SUR-ENSEMBLE des couples (nœud, valeur) vivants ;
//! * une série n'est écrite QUE si son contenu est connu COMPLET ;
//! * la CLÉ hachée doit être CANONIQUE vis-à-vis de l'égalité de
//!   revérification : deux valeurs que `valeurs_egales_gql` tient pour
//!   égales DOIVENT partager leur hachage, sinon le porteur de l'une manque
//!   aux candidats de l'autre. `Float64(5.0)` se range donc sous l'entier 5,
//!   et deux chaînes datetime RFC3339 d'un même INSTANT sous cet instant
//!   (version 2 du format — la version 1 hachait la valeur telle quelle et
//!   perdait ces porteurs) ;
//! * le DOMAINE de projection doit être EXACTEMENT celui de
//!   `index_value_key` (Bool, entier ou flottant entier, chaîne ordinaire,
//!   chaîne datetime — rien d'autre). C'est pourquoi
//!   ce module n'expose PAS de `hacher(&Value)` : il expose un hachage par
//!   VARIANTE ([`hachage_bool`], [`hachage_int`], [`hachage_chaine`],
//!   [`hachage_instant`]) et le
//!   store hache depuis son `IndexValueKey` déjà projeté — l'équivalence de
//!   domaine est alors vraie PAR CONSTRUCTION, pas par convention. Un
//!   `hacher(&Value)` parallèle qui accepterait demain un type que
//!   `index_value_key` refuse ferait répondre `Some(vec![])` là où il faut
//!   balayer : faux négatif silencieux, le défaut exact que cette conception
//!   interdit (leçon de `79e9e283` : un chemin neuf qui redéduit la
//!   sémantique cesse d'être comparable au témoin).
//!
//! ## Le hachage, et son contrat de stabilité
//!
//! FNV-1a 64 bits sur `tag ‖ charge canonique` (tag 0x01 Bool, 0x02 Int64
//! petit-boutiste, 0x03 UTF-8, 0x04 instant en nanosecondes sur 16 octets
//! petit-boutiste, 0x05 sans charge pour le seau « autre »). Il est écrit SUR DISQUE : il doit rendre les
//! MÊMES octets d'une version du binaire à l'autre — c'est pourquoi ni
//! `DefaultHasher` (graine par processus) ni un dépendance externe ne
//! conviennent. Changer ce hachage impose d'incrémenter [`EQ_VERSION`], et
//! le filet n°2 fait le reste : l'index d'une autre version est JETÉ.
//!
//! 64 bits pour 10⁷ entrées ⇒ ~10⁻⁵ de probabilité d'une collision quelque
//! part — et une collision n'est qu'un faux positif revérifié.
//!
//! ## Structure sur disque, fenêtre de coupure
//!
//! Fichier `substrate.props.eq.idx.v2` — MÊME idiome que
//! `substrate.props.idx.v2` dont il réutilise les en-têtes
//! ([`EnteteRacine`], [`EnteteSerie`], [`DescSerie`]) sous d'autres magics :
//! page racine (version, ancres, table des séries), séries TRIÉES par
//! `(hachage, slot)` et CONTIGUËS, 254 entrées de 16 octets par page,
//! CRC32 par page, réécriture intégrale par fichier temporaire + `rename`.
//!
//! Les ancres de couverture (`ancre_wal`, `ancre_pages`) ferment la fenêtre
//! de coupure exactement comme pour l'index ordonné : à l'ouverture l'index
//! n'est cru que si RIEN ne s'est écrit depuis la fusion. Le sens de
//! l'erreur est conservateur — on se déclare non fiable plus souvent que
//! nécessaire, jamais l'inverse.

#![allow(unsafe_code)]

use crate::error::{SubstrateError, SubstrateResult};
use crate::file::{SubstrateFile, ZoneFile};
use crate::page::PAGE_SIZE;
use crate::props_index::{DescSerie, EnteteRacine, EnteteSerie, SERIES_MAX};
use bytemuck::{Pod, Zeroable};
use std::path::{Path, PathBuf};

/// Nom canonique du fichier d'index d'égalité.
pub const PROPS_EQ_INDEX_FILENAME: &str = "substrate.props.eq.idx.v2";

/// Magic de la page racine.
pub const EQ_MAGIC_RACINE: u32 = 0xC5DE_1D10;
/// Magic d'une page de série.
pub const EQ_MAGIC_SERIE: u32 = 0xC5DE_1D11;

/// Version du format. Un index d'une autre version est JETÉ, jamais
/// interprété. Incrémenter si le hachage ou la disposition change.
///
/// * 1 — clé hachée = la valeur telle quelle (Bool / Int64 / String).
/// * 2 — clé CANONIQUE vis-à-vis de l'égalité : un `Float64` entier est
///   rangé sous l'entier égal (`hachage_int`), une chaîne datetime RFC3339
///   sous son instant (`hachage_instant`, tag 0x04), et toute valeur que
///   l'égalité ne sait pas ranger sous le seau unique [`hachage_autre`] (tag
///   0x05) — ce qui fait de la série la liste COMPLÈTE des porteurs de la
///   propriété. La disposition des octets est INCHANGÉE ; c'est le CONTENU
///   qui change.
///
/// Une zone de version 1 n'est PAS jetée : voir [`EQ_VERSION_HERITEE`].
pub const EQ_VERSION: u32 = 2;

/// La version d'AVANT la clé canonique, toujours LUE.
///
/// Jeter une zone v1 obligerait à rebâtir tous les index depuis les chaînes
/// — un coût proportionnel à la base, payé à l'ouverture ou à la première
/// requête. Or une zone v1 reste EXACTE pour une partie des recherches : les
/// hachages de Bool, d'Int64 et de chaîne ordinaire sont IDENTIQUES dans les
/// deux versions. Ce qui lui MANQUE, ce sont les entrées que la v1 ne
/// projetait pas ou projetait autrement : flottants entiers (sous l'entier),
/// chaînes datetime (sous l'instant), valeurs non projetables (seau
/// « autre »).
///
/// Une zone v1 s'ouvre donc comme HÉRITÉE ([`PropsEqIndexZone::canonique`]
/// rend `false`). Le store s'en sert pour ce qu'elle sait servir exactement,
/// et bâtit le COMPLÉMENT (les seules entrées manquantes) hors du chemin des
/// requêtes ; la fusion suivante écrit une zone v2. Elle n'est jamais
/// réécrite en v1.
pub const EQ_VERSION_HERITEE: u32 = 1;

// ---------------------------------------------------------------------------
// Hachage — FNV-1a 64, par VARIANTE (voir le pavé de tête : pas de
// `hacher(&Value)` pour que le domaine reste celui d'`index_value_key`
// par construction)
// ---------------------------------------------------------------------------

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

#[inline]
fn fnv1a(tag: u8, charge: &[u8]) -> u64 {
    let mut h = FNV_OFFSET;
    h ^= u64::from(tag);
    h = h.wrapping_mul(FNV_PRIME);
    for &o in charge {
        h ^= u64::from(o);
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

/// Hachage d'un `IndexValueKey::Bool`.
#[must_use]
pub fn hachage_bool(b: bool) -> u64 {
    fnv1a(0x01, &[u8::from(b)])
}

/// Hachage d'un `IndexValueKey::Int`.
#[must_use]
pub fn hachage_int(i: i64) -> u64 {
    fnv1a(0x02, &i.to_le_bytes())
}

/// Hachage d'un `IndexValueKey::Str`.
#[must_use]
pub fn hachage_chaine(s: &str) -> u64 {
    fnv1a(0x03, s.as_bytes())
}

/// Hachage d'un `IndexValueKey::Instant` — l'instant, en NANOSECONDES (`i128`),
/// d'une chaîne datetime RFC3339. Toutes les écritures d'un même instant
/// (quel que soit le fuseau) rendent le même hachage.
#[must_use]
pub fn hachage_instant(nanos: i128) -> u64 {
    fnv1a(0x04, &nanos.to_le_bytes())
}

/// Hachage du seau « autre » : toutes les valeurs qu'aucune recherche par
/// égalité ne sonde (flottant non entier, liste, temporel typé, Null…)
/// partagent CE hachage. Il ne sert pas à trouver une valeur — il fait de la
/// série la liste complète des PORTEURS de la propriété (existence).
#[must_use]
pub fn hachage_autre() -> u64 {
    fnv1a(0x05, &[])
}

// ---------------------------------------------------------------------------
// Représentation sur disque
// ---------------------------------------------------------------------------

/// Une entrée d'index : 16 octets, sans bourrage implicite.
///
/// Elle ne porte AUCUNE valeur exploitable : 8 octets de hachage et un
/// numéro de slot — l'index ne doit jamais être le seul porteur d'une
/// donnée.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct EntreeEgalite {
    pub hachage: u64,
    pub slot: u32,
    pub _pad: u32,
}

const _: [(); 1] = [(); (core::mem::size_of::<EntreeEgalite>() == 16) as usize];

impl EntreeEgalite {
    #[must_use]
    pub fn new(hachage: u64, slot: u32) -> Self {
        Self {
            hachage,
            slot,
            _pad: 0,
        }
    }
    /// Ordre total sur `(hachage, slot)` — le slot départage pour que deux
    /// fusions d'un même contenu produisent des octets identiques.
    #[must_use]
    pub fn ordre(&self) -> (u64, u32) {
        (self.hachage, self.slot)
    }
}

/// Réponse de [`PropsEqIndexZone::sonder`].
#[derive(Copy, Clone, Debug)]
pub enum SondeZone {
    /// La zone ne sait pas : non fiable, propriété absente, page illisible.
    /// L'appelant ne doit RIEN en conclure (jamais « zéro résultat »).
    Inconnue,
    /// La propriété a une série, VIDE : aucun porteur à la dernière fusion.
    SerieVide,
    /// Le hachage est localisé.
    Plage(PlageZone),
}

/// Position d'un hachage dans une série : le rang de la première entrée de
/// hachage ≥ celui cherché. Valable tant que la zone n'est pas refusionnée —
/// l'appelant la consomme sous le MÊME verrou de lecture que la sonde.
#[derive(Copy, Clone, Debug)]
pub struct PlageZone {
    desc: DescSerie,
    debut: u64,
    hachage: u64,
}

/// Nombre d'entrées par page de série (même en-tête de 24 octets que
/// l'index ordonné).
pub const ENTREES_EQ_PAR_PAGE: usize = (PAGE_SIZE - 24) / 16; // 254

// ---------------------------------------------------------------------------
// La zone
// ---------------------------------------------------------------------------

/// Zone d'index d'égalité projetée en mémoire. Les séries sont lues DANS la
/// projection, jamais copiées dans un tas — sauf par [`Self::entrees_serie`],
/// réservé à la fusion.
pub struct PropsEqIndexZone {
    zone: Option<ZoneFile>,
    chemin: PathBuf,
    racine: EnteteRacine,
    series: Vec<DescSerie>,
    /// `true` pour une zone de version courante (clé canonique), `false`
    /// pour une zone HÉRITÉE (version 1) : voir [`EQ_VERSION_HERITEE`].
    canonique: bool,
    /// `false` tant que les ancres n'ont pas été confrontées à l'état
    /// courant, ou dès qu'elles ont divergé. **Tant que ce champ est faux,
    /// l'index ne répond pas.**
    fiable: bool,
}

impl PropsEqIndexZone {
    /// Ouvre l'index d'égalité d'un substrat. Une absence de fichier, un
    /// magic ou une version inattendus, un CRC faux : tous rendent une zone
    /// VIDE et NON FIABLE — jamais une erreur, jamais une interprétation
    /// partielle. L'index illisible se jette et se rebâtit depuis les
    /// chaînes.
    pub fn ouvrir(sub: &SubstrateFile) -> Self {
        let chemin = sub.path().join(PROPS_EQ_INDEX_FILENAME);
        Self::ouvrir_chemin(&chemin)
    }

    /// Ouvre un fichier de zone par son chemin — pour les tests du store qui
    /// fabriquent un fichier d'une autre version.
    #[cfg(test)]
    pub(crate) fn ouvrir_fichier_pour_test(chemin: &Path) -> Self {
        Self::ouvrir_chemin(chemin)
    }

    fn ouvrir_chemin(chemin: &Path) -> Self {
        let vide = Self {
            zone: None,
            chemin: chemin.to_path_buf(),
            racine: EnteteRacine::default(),
            series: Vec::new(),
            canonique: true,
            fiable: false,
        };
        if !chemin.exists() {
            return vide;
        }
        let Ok(zone) = ZoneFile::open_or_create(chemin) else {
            tracing::warn!(
                ?chemin,
                "index d'égalité illisible — ignoré (reconstruction)"
            );
            return vide;
        };
        let octets = zone.as_slice();
        if octets.len() < PAGE_SIZE {
            return vide;
        }
        let racine: EnteteRacine = *bytemuck::from_bytes(&octets[..48]);
        if racine.magic != EQ_MAGIC_RACINE
            || (racine.version != EQ_VERSION && racine.version != EQ_VERSION_HERITEE)
        {
            tracing::warn!(
                magic = format!("{:#x}", racine.magic),
                version = racine.version,
                "index d'égalité d'un autre format — jeté"
            );
            return vide;
        }
        if racine.nb_series as usize > SERIES_MAX {
            return vide;
        }
        // CRC sur la page racine entière, champ `crc32` mis à zéro.
        let mut copie = [0u8; PAGE_SIZE];
        copie.copy_from_slice(&octets[..PAGE_SIZE]);
        copie[8..12].copy_from_slice(&0u32.to_le_bytes());
        if crc32fast::hash(&copie) != racine.crc32 {
            tracing::warn!("racine d'index d'égalité : CRC faux — index jeté");
            return vide;
        }
        let mut series = Vec::with_capacity(racine.nb_series as usize);
        for i in 0..racine.nb_series as usize {
            let d = 48 + i * 16;
            series.push(*bytemuck::from_bytes::<DescSerie>(&octets[d..d + 16]));
        }
        // Une série qui déborde du fichier est un index corrompu — jeté.
        let pages_totales = (octets.len() / PAGE_SIZE) as u32;
        for s in &series {
            if s.page_debut == 0 || s.page_debut.saturating_add(s.nb_pages) > pages_totales {
                tracing::warn!("série d'index d'égalité hors zone — index jeté");
                return vide;
            }
        }
        Self {
            zone: Some(zone),
            chemin: chemin.to_path_buf(),
            racine,
            series,
            canonique: racine.version == EQ_VERSION,
            fiable: false, // confronté aux ancres par `confronter_ancres`
        }
    }

    /// Confronte les ancres de couverture à l'état COURANT du substrat.
    /// C'est ici, et uniquement ici, qu'un index devient fiable — même
    /// contrat que l'index ordonné.
    pub fn confronter_ancres(&mut self, offset_wal: u64, pages_v2: u32) {
        let ok = self.zone.is_some()
            && self.racine.ancre_wal == offset_wal
            && self.racine.ancre_pages == pages_v2;
        if !ok && self.zone.is_some() {
            tracing::info!(
                ancre_wal = self.racine.ancre_wal,
                offset_wal,
                ancre_pages = self.racine.ancre_pages,
                pages_v2,
                "index d'égalité : ancres divergentes — NON FIABLE, reconstruction par processus"
            );
        }
        self.fiable = ok;
    }

    /// L'index répond-il ? `false` ⇒ l'appelant DOIT reconstruire (ou
    /// balayer).
    #[must_use]
    pub fn fiable(&self) -> bool {
        self.fiable
    }

    /// La zone porte-t-elle la clé CANONIQUE (version courante) ? `false`
    /// pour une zone héritée : ses séries ne contiennent ni les flottants
    /// entiers, ni les instants, ni le seau « autre ». Voir
    /// [`EQ_VERSION_HERITEE`].
    #[must_use]
    pub fn canonique(&self) -> bool {
        self.canonique
    }

    /// ── ADOPTION D'ANCRES : LA FENÊTRE POST-FUSION ÉTAIT SANS EFFET ──────
    ///
    /// Repose les ancres de couverture sur l'état COURANT, en mémoire ET sur
    /// disque, et rend l'index fiable. L'appelant ne doit y venir QUE muni
    /// d'une preuve : `SubstrateStore::fenetre_wal_neutre_pour_index` a établi
    /// que la fenêtre `[ancre, offset)` ne contient que des trames dont le
    /// rejeu ne touche pas la chaîne PropsZone. Sans cette preuve, adopter
    /// ferait servir un index INCOMPLET — la faute que `confronter_ancres`
    /// existe pour empêcher.
    ///
    /// ── POURQUOI ON N'ÉCRIT QUE L'EN-TÊTE ────────────────────────────────
    ///
    /// `fusionner` réécrit tout le fichier (racine + toutes les pages de
    /// séries, 50 Mo sur le banc) : c'est le prix d'une fusion, pas celui d'un
    /// constat. Ici les SÉRIES ne changent pas d'un octet — seule
    /// `ancre_wal`/`ancre_pages` bouge — donc l'écriture se limite aux 48
    /// octets de la racine, avec le même CRC32 que `fusionner` et la même
    /// discipline (`sync_all` avant de rendre la main, `generation`
    /// incrémentée).
    ///
    /// Ce n'est PAS une écriture de données : aucune série n'est modifiée. Si
    /// le processus meurt pendant ces 48 octets, le CRC de la racine ne
    /// correspond plus et `ouvrir_chemin` rend une zone VIDE : l'index est
    /// reconstructible, jamais cru à moitié.
    ///
    /// La persistance est un CONFORT, pas la correction : elle évite de
    /// rescanner la fenêtre à chaque ouverture. Elle échoue proprement en
    /// LECTURE SEULE (le fichier n'est pas ouvert en écriture) — et l'adoption
    /// reste valable pour CE processus, ce qui est exactement ce qu'il faut :
    /// un lecteur ne modifie pas le store qu'il lit.
    pub fn adopter_ancres(&mut self, offset_wal: u64, pages_v2: u32) {
        self.racine.ancre_wal = offset_wal;
        self.racine.ancre_pages = pages_v2;
        self.racine.generation = self.racine.generation.wrapping_add(1);
        // ── LE CRC COUVRE LA PAGE RACINE ENTIÈRE, PAS SES 48 PREMIERS OCTETS ──
        //
        // `ouvrir_chemin` vérifie le CRC sur les 4 096 octets de la page
        // racine (en-tête + table des séries), comme `fusionner` l'écrit. La
        // première version de cette adoption le calculait sur l'en-tête SEUL :
        // la racine réécrite était donc rejetée à l'ouverture suivante (« CRC
        // faux — index jeté »), et l'adoption, valable en mémoire, ne
        // survivait jamais au processus — chaque redémarrage rebâtissait.
        self.racine.crc32 = 0;
        let mut page = [0u8; PAGE_SIZE];
        if let Some(z) = self.zone.as_ref()
            && let Some(courante) = z.as_slice().get(..PAGE_SIZE)
        {
            page.copy_from_slice(courante);
        }
        page[..48].copy_from_slice(bytemuck::bytes_of(&self.racine));
        let crc = crc32fast::hash(&page);
        self.racine.crc32 = crc;
        let mut racine = [0u8; 48];
        racine.copy_from_slice(bytemuck::bytes_of(&self.racine));

        // La racine vit au premier octet de la zone : 48 octets à l'offset 0.
        if let Some(z) = self.zone.as_ref() {
            match z.ecrire_a(0, &racine) {
                Ok(()) => {
                    // Même ordre que `fusionner` : les pages modifiées d'abord
                    // (`msync`), puis le descripteur (`fsync`). L'écriture passe
                    // par `write_all_at`, donc le mmap du fichier doit être
                    // poussé séparément.
                    let _ = z.msync();
                    let _ = z.fsync();
                }
                Err(e) => {
                    tracing::debug!(
                        erreur = %e,
                        "index d'égalité : ancres adoptées en mémoire seulement (zone non inscriptible — lecture seule ?)"
                    );
                }
            }
        }
        // L'index couvre désormais l'état courant : il répond.
        self.fiable = true;
    }

    #[must_use]
    pub fn generation(&self) -> u32 {
        self.racine.generation
    }

    /// Ancre WAL inscrite dans la racine à la dernière fusion — miroir de
    /// `PropsIndexZone::ancre_wal` (l'égalité partage le même en-tête).
    #[must_use]
    pub fn ancre_wal(&self) -> u64 {
        self.racine.ancre_wal
    }

    /// Ancre de pages v2 inscrite dans la racine à la dernière fusion.
    #[must_use]
    pub fn ancre_pages(&self) -> u32 {
        self.racine.ancre_pages
    }

    #[must_use]
    pub fn nb_entrees_totales(&self) -> u64 {
        self.series.iter().map(|s| u64::from(s.nb_entrees)).sum()
    }

    /// Propriétés (identifiants internes) présentes dans l'index sur disque.
    #[must_use]
    pub fn proprietes(&self) -> Vec<u16> {
        self.series.iter().map(|s| s.prop_key_id).collect()
    }

    #[must_use]
    pub fn chemin(&self) -> &Path {
        &self.chemin
    }

    fn desc(&self, prop_key_id: u16) -> Option<&DescSerie> {
        self.series.iter().find(|s| s.prop_key_id == prop_key_id)
    }

    /// Entrées de la page de rang `p` d'une série, LUES DANS LA PROJECTION.
    fn entrees_page(&self, d: &DescSerie, p: u32) -> Option<&[EntreeEgalite]> {
        let zone = self.zone.as_ref()?;
        let octets = zone.as_slice();
        let debut = (d.page_debut + p) as usize * PAGE_SIZE;
        let page = octets.get(debut..debut + PAGE_SIZE)?;
        let e: &EnteteSerie = bytemuck::from_bytes(&page[..24]);
        if e.magic != EQ_MAGIC_SERIE || e.prop_key_id != d.prop_key_id {
            return None;
        }
        let n = (e.nb_entrees as usize).min(ENTREES_EQ_PAR_PAGE);
        Some(bytemuck::cast_slice(&page[24..24 + n * 16]))
    }

    /// Énumère les slots dont le hachage vaut `hachage`.
    ///
    /// Rend `None` — et JAMAIS `Some(vec![])` — si l'index n'est pas fiable,
    /// si la propriété n'y figure pas, ou si une série est vide ou illisible :
    /// un ensemble vide rendu par ignorance court-circuiterait la
    /// reconstruction chez l'appelant et affirmerait « zéro résultat »,
    /// définitif, sans erreur.
    ///
    /// Le résultat est un SUR-ENSEMBLE des slots dont la valeur courante
    /// égale la valeur cherchée : collisions et entrées périmées y figurent.
    /// L'appelant revérifie.
    pub fn candidats(&self, prop_key_id: u16, hachage: u64) -> Option<Vec<u32>> {
        match self.sonder(prop_key_id, hachage) {
            SondeZone::Plage(plage) => {
                let mut out = Vec::new();
                self.slots_de(&plage, &mut out).then_some(out)
            }
            SondeZone::Inconnue | SondeZone::SerieVide => None,
        }
    }

    /// Entrée de RANG `rang` d'une série (0 = la plus petite). Les pages
    /// d'une série sont contiguës et toutes PLEINES sauf la dernière
    /// (`fusionner` les découpe ainsi) : le rang donne la page par division,
    /// sans parcours. `None` = page illisible ou rang hors série.
    #[inline]
    fn entree_de_rang(&self, d: &DescSerie, rang: u64) -> Option<EntreeEgalite> {
        let page = u32::try_from(rang / ENTREES_EQ_PAR_PAGE as u64).ok()?;
        self.entrees_page(d, page)?
            .get((rang % ENTREES_EQ_PAR_PAGE as u64) as usize)
            .copied()
    }

    /// Localise le hachage dans la série d'une propriété SANS rien copier.
    ///
    /// C'est la moitié « où » d'une recherche : elle rend le rang de la
    /// première entrée de hachage ≥ `hachage`. [`Self::taille_plafonnee`] dit
    /// ensuite combien d'entrées portent ce hachage (pour élire la clé la
    /// plus sélective), [`Self::slots_de`] les énumère (pour la clé élue).
    ///
    /// COÛT. Dichotomie sur les PAGES (la dernière entrée d'une page borne la
    /// suivante), puis dans la page : log₂(n / 254) pages lues, aucune copie.
    /// Les niveaux hauts de la dichotomie sont les MÊMES pour toutes les
    /// recherches : ils restent en cache, et seuls les derniers niveaux
    /// coûtent un défaut de cache. Une sonde par interpolation (hachages FNV
    /// uniformes) a été essayée et mesurée PLUS LENTE à 1 M d'entrées — elle
    /// touche davantage de pages distinctes — d'où ce retour à la dichotomie.
    #[must_use]
    pub fn sonder(&self, prop_key_id: u16, hachage: u64) -> SondeZone {
        if !self.fiable {
            return SondeZone::Inconnue;
        }
        let Some(d) = self.desc(prop_key_id).copied() else {
            return SondeZone::Inconnue;
        };
        if d.nb_entrees == 0 {
            return SondeZone::SerieVide;
        }
        let (mut lo, mut hi) = (0u32, d.nb_pages);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let Some(e) = self.entrees_page(&d, mid) else {
                return SondeZone::Inconnue;
            };
            match e.last() {
                Some(der) if der.hachage < hachage => lo = mid + 1,
                _ => hi = mid,
            }
        }
        // `lo` : première page dont la dernière entrée est ≥ au hachage. Si
        // aucune (`lo == nb_pages`), toutes les entrées sont plus petites.
        let debut = if lo >= d.nb_pages {
            u64::from(d.nb_entrees)
        } else {
            let Some(entrees) = self.entrees_page(&d, lo) else {
                return SondeZone::Inconnue;
            };
            u64::from(lo) * ENTREES_EQ_PAR_PAGE as u64
                + entrees.partition_point(|e| e.hachage < hachage) as u64
        };
        SondeZone::Plage(PlageZone {
            desc: d,
            debut,
            hachage,
        })
    }

    /// Nombre d'entrées de la plage, PLAFONNÉ : rend `min(taille, plafond + 1)`.
    ///
    /// Sert à ÉLIRE la clé la plus sélective, pas à compter : savoir qu'une
    /// plage dépasse `plafond` suffit à l'écarter, et coûte log₂(plafond)
    /// lectures au lieu d'une copie de toute la plage. `None` = page
    /// illisible (l'appelant ne doit alors rien conclure).
    #[must_use]
    pub fn taille_plafonnee(&self, plage: &PlageZone, plafond: u64) -> Option<u64> {
        let n = u64::from(plage.desc.nb_entrees);
        let porte = |r: u64| -> Option<bool> {
            if r >= n {
                return Some(false);
            }
            self.entree_de_rang(&plage.desc, r)
                .map(|e| e.hachage == plage.hachage)
        };
        if !porte(plage.debut)? {
            return Some(0);
        }
        // Cas dominant — une valeur unique : l'entrée suivante est déjà un autre
        // hachage (même page, déjà en cache).
        if plafond >= 1 && !porte(plage.debut.saturating_add(1))? {
            return Some(1);
        }
        // Au-delà du plafond : la plage est « trop grande », sans plus.
        if porte(plage.debut.saturating_add(plafond))? {
            return Some(plafond.saturating_add(1));
        }
        // La fin est dans ]debut, debut + plafond] : dichotomie.
        let (mut bas, mut haut) = (plage.debut + 1, plage.debut.saturating_add(plafond));
        while bas < haut {
            let milieu = bas + (haut - bas) / 2;
            if porte(milieu)? {
                bas = milieu + 1;
            } else {
                haut = milieu;
            }
        }
        Some(bas - plage.debut)
    }

    /// Ajoute à `out` les slots de la plage. Rend `false` — et laisse `out`
    /// dans un état que l'appelant doit JETER — si une page est illisible :
    /// un partiel ne doit jamais passer pour complet.
    #[must_use]
    pub fn slots_de(&self, plage: &PlageZone, out: &mut Vec<u32>) -> bool {
        let n = u64::from(plage.desc.nb_entrees);
        let par_page = ENTREES_EQ_PAR_PAGE as u64;
        let mut rang = plage.debut;
        while rang < n {
            let Ok(page) = u32::try_from(rang / par_page) else {
                return false;
            };
            let Some(entrees) = self.entrees_page(&plage.desc, page) else {
                return false;
            };
            let debut = (rang % par_page) as usize;
            if debut >= entrees.len() {
                return false;
            }
            for e in &entrees[debut..] {
                if e.hachage != plage.hachage {
                    return true;
                }
                out.push(e.slot);
            }
            rang = (u64::from(page) + 1) * par_page;
        }
        true
    }

    /// Copie les entrées d'une série existante — RÉSERVÉ À LA FUSION, jamais
    /// sur un chemin de lecture. Rend un vecteur vide si l'index n'est pas
    /// fiable ou si la propriété n'y figure pas : **on ne reporte JAMAIS
    /// dans un index frais le contenu d'un index dont on doute**, car les
    /// ancres neuves le béniraient (même garde, éprouvée par mutation, que
    /// `PropsIndexZone::entrees_serie`).
    #[must_use]
    pub fn entrees_serie(&self, prop_key_id: u16) -> Vec<EntreeEgalite> {
        if !self.fiable {
            return Vec::new();
        }
        let Some(d) = self.desc(prop_key_id).copied() else {
            return Vec::new();
        };
        let mut out = Vec::with_capacity(d.nb_entrees as usize);
        for p in 0..d.nb_pages {
            match self.entrees_page(&d, p) {
                Some(e) => out.extend_from_slice(e),
                // Page illisible sous `fiable` = index corrompu : rien ne
                // sort, la reconstruction depuis les chaînes est le remède.
                None => return Vec::new(),
            }
        }
        out
    }

    // ═══════════════════════════════════════════════════════════════════
    // (2026-09-09, P1 ouverture rapide — W1-bis) EXTENSION DE FENÊTRE
    // ═══════════════════════════════════════════════════════════════════

    /// Lit une série existante EN BRUT — pages lues dans la projection,
    /// CRC32 vérifié page à page — SANS le garde `fiable`.
    ///
    /// Même justification que `PropsIndexZone::entrees_serie_brutes_verifiees` :
    /// le fichier présent sous des ancres périmées a été écrit par une fusion
    /// antérieure VALIDÉE ; son contenu reste un sur-ensemble valide de l'état
    /// qu'il couvrait à SES ancres. [`Self::etendre_apres_fenetre`] le reprend
    /// verbatim et n'y ajoute que la fenêtre expliquée par l'appelant.
    ///
    /// Rend `Err` — REFUS d'étendre — si une page de la série est hors
    /// bornes, mal en-têtée ou au CRC faux : un tel fichier est à REJETER
    /// (reconstruction par processus), jamais à bénir.
    fn entrees_serie_brutes_verifiees(&self, d: &DescSerie) -> SubstrateResult<Vec<EntreeEgalite>> {
        let zone = self.zone.as_ref().ok_or_else(|| {
            SubstrateError::Internal("index d'égalité : pas de zone à relire".into())
        })?;
        let octets = zone.as_slice();
        let mut out = Vec::with_capacity(d.nb_entrees as usize);
        for p in 0..d.nb_pages {
            let debut = (d.page_debut + p) as usize * PAGE_SIZE;
            let page = octets.get(debut..debut + PAGE_SIZE).ok_or_else(|| {
                SubstrateError::Internal(format!(
                    "index d'égalité : page de série {debut} hors zone — extension refusée"
                ))
            })?;
            let mut copie = [0u8; PAGE_SIZE];
            copie.copy_from_slice(page);
            copie[4..8].copy_from_slice(&0u32.to_le_bytes());
            let attendu = u32::from_le_bytes(page[4..8].try_into().expect("4 octets"));
            if crc32fast::hash(&copie) != attendu {
                return Err(SubstrateError::Internal(format!(
                    "index d'égalité : CRC faux sur la page de série {debut} — extension refusée"
                )));
            }
            let e: &EnteteSerie = bytemuck::from_bytes(&page[..24]);
            if e.magic != EQ_MAGIC_SERIE || e.prop_key_id != d.prop_key_id {
                return Err(SubstrateError::Internal(format!(
                    "index d'égalité : en-tête de page de série inattendu (page {debut}) — \
                     extension refusée"
                )));
            }
            let n = (e.nb_entrees as usize).min(ENTREES_EQ_PAR_PAGE);
            out.extend_from_slice(bytemuck::cast_slice(&page[24..24 + n * 16]));
        }
        Ok(out)
    }

    /// Étend l'index d'égalité persisté par les entrées AJOUTÉES depuis sa
    /// dernière fusion — la fenêtre expliquée à l'ouverture — au lieu de la
    /// reconstruction par processus (le poste ~1 s du P1).
    ///
    /// CONTRAT D'EXACTITUDE, identique à celui de la fusion de `flush()` :
    ///
    /// * `ajouts` porte, propriété par propriété, les couples
    ///   `(hachage de la valeur vivante, slot)` des nœuds touchés par la
    ///   fenêtre post-fusion — relus dans les chaînes par l'appelant ;
    /// * chaque série du disque est reprise VERBATIM, MOINS les entrées des
    ///   slots touchés, PLUS les ajouts — triée par `(hachage, slot)`,
    ///   dédupliquée, puis réécrite par le chemin ATOMIQUE habituel avec des
    ///   ancres NEUVES ;
    /// * une propriété ABSENTE du disque n'est JAMAIS créée ici (série au
    ///   contenu d'origine inconnue = jamais bénie). L'appelant la laisse
    ///   aux reconstructions par processus.
    ///
    /// Rend `Ok(false)` sans rien modifier quand la zone est absente ou que
    /// l'écart n'est pas strictement FORWARD. Rend `Err` quand le fichier de
    /// base ne se relit pas sainement ou que l'écriture échoue : l'appelant
    /// retombe alors sur la reconstruction, jamais sur un index étendu à
    /// moitié.
    pub(crate) fn etendre_apres_fenetre(
        &mut self,
        ajouts: &[(u16, Vec<EntreeEgalite>)],
        ancre_wal: u64,
        ancre_pages: u32,
    ) -> SubstrateResult<bool> {
        if self.zone.is_none() || !self.canonique {
            // Une zone HÉRITÉE ne s'étend pas : `fusionner` la réécrirait en
            // version courante sans son complément.
            return Ok(false);
        }
        if self.racine.ancre_wal > ancre_wal || self.racine.ancre_pages > ancre_pages {
            tracing::info!(
                ancre_wal = self.racine.ancre_wal,
                fourni_wal = ancre_wal,
                ancre_pages = self.racine.ancre_pages,
                fourni_pages = ancre_pages,
                "index d'égalité : écart NON FORWARD — extension refusée (reconstruction)"
            );
            return Ok(false);
        }
        let touches: std::collections::HashMap<u16, std::collections::HashSet<u32>> = ajouts
            .iter()
            .map(|(pk, e)| (*pk, e.iter().map(|x| x.slot).collect()))
            .collect();
        let nb_ajouts: usize = ajouts.iter().map(|(_, v)| v.len()).sum();
        let mut series: Vec<(u16, Vec<EntreeEgalite>)> = Vec::with_capacity(self.series.len());
        for d in &self.series {
            let base = self.entrees_serie_brutes_verifiees(d)?;
            let mut fusion: Vec<EntreeEgalite> = match touches.get(&d.prop_key_id) {
                Some(t) => base.into_iter().filter(|e| !t.contains(&e.slot)).collect(),
                None => base,
            };
            if let Some((_, aj)) = ajouts.iter().find(|(pk, _)| *pk == d.prop_key_id) {
                fusion.extend(aj.iter().copied());
            }
            fusion.sort_unstable_by_key(EntreeEgalite::ordre);
            fusion.dedup();
            series.push((d.prop_key_id, fusion));
        }
        if series.is_empty() {
            return Ok(false);
        }
        let t = web_time::Instant::now();
        // `fusionner` vérifie le tri, écrit par fichier temporaire + rename,
        // reprojette et pose les ancres fournies : l'index devient FIABLE
        // pour l'état courant.
        self.fusionner(&series, ancre_wal, ancre_pages)?;
        tracing::info!(
            entrees_ajoutees = nb_ajouts,
            series = series.len(),
            elapsed_ms = t.elapsed().as_millis() as u64,
            "index d'égalité : étendu depuis la fenêtre post-fusion (reconstruction évitée)"
        );
        Ok(true)
    }

    /// Réécrit INTÉGRALEMENT l'index : fichier temporaire, `fsync`,
    /// `rename`, `fsync` du répertoire, puis reprojection. Une coupure
    /// laisse l'ANCIEN fichier, lisible et cohérent, que ses ancres
    /// déclareront simplement périmé.
    ///
    /// `series` doit être TRIÉE par `(hachage, slot)` pour chaque propriété ;
    /// une série mal triée rendrait la recherche binaire silencieusement
    /// fausse — le faux négatif que tout ceci interdit — donc la fusion la
    /// REFUSE plutôt que d'écrire un index menteur.
    ///
    /// # Errors
    /// Propage un échec d'écriture ou de `rename`. Le fichier PRÉCÉDENT
    /// reste alors en place et intact.
    pub fn fusionner(
        &mut self,
        series: &[(u16, Vec<EntreeEgalite>)],
        ancre_wal: u64,
        ancre_pages: u32,
    ) -> SubstrateResult<()> {
        self.fusionner_en(series, ancre_wal, ancre_pages, true)
    }

    /// [`Self::fusionner`], en précisant ce que le CONTENU fourni garantit :
    /// `canonique = true` écrit une zone de version courante (les séries
    /// contiennent flottants entiers, instants et seau « autre ») ;
    /// `false` réécrit une zone HÉRITÉE (version 1) — pour l'appelant dont le
    /// complément n'est pas encore bâti et qui ne doit pas bénir comme
    /// canonique un contenu qui ne l'est pas.
    ///
    /// # Errors
    /// Comme [`Self::fusionner`].
    pub fn fusionner_en(
        &mut self,
        series: &[(u16, Vec<EntreeEgalite>)],
        ancre_wal: u64,
        ancre_pages: u32,
        canonique: bool,
    ) -> SubstrateResult<()> {
        if series.len() > SERIES_MAX {
            return Err(SubstrateError::Internal(format!(
                "index d'égalité : {} séries demandées, {SERIES_MAX} au plus",
                series.len()
            )));
        }
        for (pk, e) in series {
            if e.windows(2).any(|w| w[0].ordre() > w[1].ordre()) {
                return Err(SubstrateError::Internal(format!(
                    "index d'égalité : série {pk} non triée — fusion REFUSÉE"
                )));
            }
        }

        let mut descs: Vec<DescSerie> = Vec::with_capacity(series.len());
        let mut pages: Vec<[u8; PAGE_SIZE]> = Vec::new();
        let mut page_courante = 1u32; // page 0 = racine

        for (pk, entrees) in series {
            let nb_pages = entrees.len().div_ceil(ENTREES_EQ_PAR_PAGE).max(1) as u32;
            descs.push(DescSerie {
                prop_key_id: *pk,
                _pad: 0,
                page_debut: page_courante,
                nb_pages,
                nb_entrees: u32::try_from(entrees.len()).map_err(|_| {
                    SubstrateError::Internal("index d'égalité : série > 2^32 entrées".into())
                })?,
            });
            for p in 0..nb_pages {
                let mut page = [0u8; PAGE_SIZE];
                let d = p as usize * ENTREES_EQ_PAR_PAGE;
                let f = (d + ENTREES_EQ_PAR_PAGE).min(entrees.len());
                let n = f.saturating_sub(d);
                let entete = EnteteSerie {
                    magic: EQ_MAGIC_SERIE,
                    crc32: 0,
                    prop_key_id: *pk,
                    nb_entrees: n as u16,
                    index_page: p,
                    nb_pages_serie: nb_pages,
                    _reserve: 0,
                };
                page[..24].copy_from_slice(bytemuck::bytes_of(&entete));
                if n > 0 {
                    page[24..24 + n * 16].copy_from_slice(bytemuck::cast_slice(&entrees[d..f]));
                }
                let crc = crc32fast::hash(&page);
                page[4..8].copy_from_slice(&crc.to_le_bytes());
                pages.push(page);
            }
            page_courante += nb_pages;
        }

        let mut racine = [0u8; PAGE_SIZE];
        let entete = EnteteRacine {
            magic: EQ_MAGIC_RACINE,
            version: if canonique {
                EQ_VERSION
            } else {
                EQ_VERSION_HERITEE
            },
            crc32: 0,
            nb_series: descs.len() as u32,
            ancre_wal,
            ancre_pages,
            generation: self.racine.generation.wrapping_add(1),
            _reserve: [0; 4],
        };
        racine[..48].copy_from_slice(bytemuck::bytes_of(&entete));
        for (i, d) in descs.iter().enumerate() {
            racine[48 + i * 16..48 + (i + 1) * 16].copy_from_slice(bytemuck::bytes_of(d));
        }
        let crc = crc32fast::hash(&racine);
        racine[8..12].copy_from_slice(&crc.to_le_bytes());

        // ── ÉCRITURE ATOMIQUE ── (nom temporaire dérivé du PID : deux
        // processus fusionnant la même base ne s'écrasent pas en cours
        // d'écriture ; le `rename` reste le point de sérialisation.)
        let tmp = self
            .chemin
            .with_extension(format!("tmp.{}", crate::wal_io::pid_portable()));
        {
            use std::io::Write as _;
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&racine)?;
            for p in &pages {
                f.write_all(p)?;
            }
            f.sync_all()?; // les octets AVANT le rename.
        }
        // La zone doit être relâchée avant que le `rename` ne remplace le
        // fichier sous sa projection.
        self.zone = None;
        std::fs::rename(&tmp, &self.chemin)?;
        if let Some(rep) = self.chemin.parent()
            && let Ok(d) = std::fs::File::open(rep)
        {
            let _ = d.sync_all();
        }

        // Reprojection.
        let rouvert = Self::ouvrir_chemin(&self.chemin);
        self.zone = rouvert.zone;
        self.racine = rouvert.racine;
        self.series = rouvert.series;
        self.canonique = rouvert.canonique;
        // Les ancres viennent d'être posées : l'index est fiable jusqu'à la
        // prochaine écriture, que `confronter_ancres` détectera.
        self.fiable = self.zone.is_some();
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Trigrammes — le code d'un trigramme est ses trois OCTETS, tels quels.
// Pas de hachage : 24 bits distinguent tous les trigrammes possibles, et
// l'absence de projection supprime toute question de collision.
// ---------------------------------------------------------------------------

/// Code d'un trigramme d'octets — stable sur disque par construction
/// (aucun hachage, les octets eux-mêmes).
#[inline]
#[must_use]
pub fn code_trigramme(t: [u8; 3]) -> u64 {
    (u64::from(t[0]) << 16) | (u64::from(t[1]) << 8) | u64::from(t[2])
}

/// Codes des trigrammes d'OCTETS d'une chaîne, DISTINCTS et triés.
///
/// On fenêtre les octets UTF-8, pas les points de code : `x CONTAINS y` est un
/// `str::contains`, c'est-à-dire une recherche de SOUS-CHAÎNE d'octets — tout
/// trigramme d'octets de `y` figure donc parmi ceux de `x`. C'est cette
/// implication, et elle seule, qui rend l'index complet ; elle est vraie pour
/// l'UTF-8 multi-octets sans traitement particulier.
///
/// Chaîne de moins de 3 octets : aucun trigramme — l'appelant ne peut pas
/// router par l'index et doit balayer.
#[must_use]
pub fn trigrammes_de(s: &str) -> Vec<u64> {
    let o = s.as_bytes();
    let mut v: Vec<u64> = o
        .windows(3)
        .map(|w| code_trigramme([w[0], w[1], w[2]]))
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zone_temporaire() -> (SubstrateFile, PropsEqIndexZone) {
        let sf = SubstrateFile::open_tempfile().unwrap();
        let z = PropsEqIndexZone::ouvrir(&sf);
        (sf, z)
    }

    fn serie(n: u32) -> Vec<EntreeEgalite> {
        let mut v: Vec<EntreeEgalite> = (0..n)
            .map(|i| EntreeEgalite::new(hachage_int(i64::from(i)), i))
            .collect();
        v.sort_by_key(EntreeEgalite::ordre);
        v
    }

    // ─────────────────────────────────────────────────────────────────────
    // LE HACHAGE : stabilité et séparation des variantes
    // ─────────────────────────────────────────────────────────────────────

    /// Le hachage est écrit SUR DISQUE : ces valeurs sont un CONTRAT. Si ce
    /// test rougit, c'est que le hachage a changé — il faut alors
    /// incrémenter `EQ_VERSION`, pas ajuster les constantes.
    #[test]
    fn le_hachage_est_stable_d_une_version_a_l_autre() {
        assert_eq!(hachage_bool(false), fnv1a(0x01, &[0]));
        assert_eq!(hachage_int(0), fnv1a(0x02, &0i64.to_le_bytes()));
        assert_eq!(hachage_chaine(""), fnv1a(0x03, b""));
        assert_eq!(hachage_instant(0), fnv1a(0x04, &0i128.to_le_bytes()));
        // Témoins FIGÉS, calculés hors de ce code (FNV-1a 64 déroulé à la
        // main sur `tag ‖ charge`) : ils rougissent si le tag, la largeur de
        // la charge ou son boutisme changent.
        assert_eq!(hachage_instant(0), 0x129d_3d1e_1e96_bdd3);
        assert_eq!(hachage_instant(-1), 0x9bf6_f237_9453_31c3);
        // 2024-01-01T10:00:00Z, en nanosecondes.
        assert_eq!(
            hachage_instant(1_704_103_200_000_000_000),
            0x51a3_2fa6_9903_e0cb
        );
        assert_eq!(hachage_autre(), 0xaf63_b84c_8601_af60);
        // Témoins figés (calculés une fois, jamais réajustés sans changer
        // EQ_VERSION).
        assert_eq!(hachage_bool(true), 0x082f_2307_b4e8_8e77);
        assert_eq!(hachage_chaine("compare_values"), {
            // même calcul, chemin indépendant : tag puis octets
            let mut h = FNV_OFFSET;
            for &o in [&[0x03u8][..], b"compare_values"].concat().iter() {
                h ^= u64::from(o);
                h = h.wrapping_mul(FNV_PRIME);
            }
            h
        });
    }

    /// Les variantes ne doivent pas se confondre : `Int(1)` n'est pas
    /// `Bool(true)` n'est pas `Str("\x01")` — le tag les sépare.
    #[test]
    fn les_variantes_ne_collisionnent_pas_entre_elles() {
        assert_ne!(hachage_int(1), hachage_bool(true));
        // L'instant et l'entier de même charge ne se confondent pas.
        assert_ne!(hachage_instant(1), hachage_int(1));
        // Le seau « autre » ne se confond avec aucune valeur réelle.
        assert_ne!(hachage_autre(), hachage_chaine(""));
        assert_ne!(hachage_autre(), hachage_bool(false));
        assert_ne!(hachage_chaine("\u{1}"), hachage_bool(true));
        assert_ne!(hachage_int(49), hachage_chaine("1"));
    }

    /// Deux chaînes NE partageant qu'un préfixe ont des hachages distincts —
    /// c'est tout l'intérêt face à la clé tronquée à 11 octets de l'index
    /// ordonné, inutilisable pour l'égalité sur des `qualified_name` à long
    /// préfixe commun.
    #[test]
    fn les_longs_prefixes_communs_ne_collisionnent_pas() {
        let a = hachage_chaine("private-tmp-grafeo-carto.src.module_alpha");
        let b = hachage_chaine("private-tmp-grafeo-carto.src.module_beta");
        assert_ne!(a, b);
    }

    // ─────────────────────────────────────────────────────────────────────
    // LA ZONE
    // ─────────────────────────────────────────────────────────────────────

    #[test]
    fn un_index_absent_n_est_pas_fiable_et_ne_repond_pas() {
        let (_d, mut z) = zone_temporaire();
        z.confronter_ancres(0, 0);
        assert!(!z.fiable());
        assert_eq!(z.candidats(1, hachage_int(0)), None);
    }

    #[test]
    fn fusion_puis_relecture_rend_les_slots_du_hachage_exact() {
        let (_d, mut z) = zone_temporaire();
        z.fusionner(&[(7u16, serie(1000))], 42, 3).unwrap();
        assert!(z.fiable());
        let c = z.candidats(7, hachage_int(137)).unwrap();
        assert_eq!(c, vec![137]);
        // Hachage absent d'une série NON vide : Some(vec![]) est une vraie
        // réponse (sur-ensemble complet ⇒ zéro candidat ⇒ zéro résultat).
        let vides: Vec<u32> = z.candidats(7, hachage_chaine("absente")).unwrap();
        // `is_empty()` et non `assert_eq!(vides, vec![])` : ce dernier NE COMPILE
        // PAS ici — `u32` implémente aussi `PartialEq<serde_json::Value>`, donc
        // le type des éléments de `vec![]` n'est pas déductible (E0282/E0283).
        // Conséquence mesurée : la cible de test de ce crate ne se construisait
        // plus, donc AUCUN test de ce fichier ne tournait.
        assert!(vides.is_empty(), "résultat attendu vide");
        // Propriété inconnue : `None`, JAMAIS `Some(vec![])`.
        assert_eq!(z.candidats(9, hachage_int(0)), None);
    }

    #[test]
    fn les_doublons_de_hachage_rendent_tous_leurs_slots() {
        let (_d, mut z) = zone_temporaire();
        // 400 slots portant la MÊME valeur (donc le même hachage) + du bruit,
        // à cheval sur plusieurs pages.
        let h = hachage_chaine("name-partage");
        let mut e: Vec<EntreeEgalite> = (0..400u32).map(|s| EntreeEgalite::new(h, s)).collect();
        e.extend((400..900u32).map(|s| EntreeEgalite::new(hachage_int(i64::from(s)), s)));
        e.sort_by_key(EntreeEgalite::ordre);
        z.fusionner(&[(2u16, e)], 0, 0).unwrap();
        let mut c = z.candidats(2, h).unwrap();
        c.sort_unstable();
        assert_eq!(c, (0..400).collect::<Vec<_>>());
    }

    #[test]
    fn la_serie_traverse_plusieurs_pages_sans_perdre_d_entree() {
        let (_d, mut z) = zone_temporaire();
        let n = (ENTREES_EQ_PAR_PAGE * 5 + 3) as u32;
        z.fusionner(&[(1u16, serie(n))], 0, 0).unwrap();
        for cherche in [0u32, 253, 254, 255, n - 1] {
            assert_eq!(
                z.candidats(1, hachage_int(i64::from(cherche))).unwrap(),
                vec![cherche],
                "slot {cherche} perdu aux frontières de pages"
            );
        }
    }

    #[test]
    fn les_ancres_qui_divergent_rendent_l_index_non_fiable() {
        let (_d, mut z) = zone_temporaire();
        z.fusionner(&[(1u16, serie(10))], 500, 12).unwrap();
        z.confronter_ancres(500, 12);
        assert!(z.fiable(), "ancres identiques : fiable");
        z.confronter_ancres(501, 12);
        assert!(!z.fiable(), "le WAL a avancé : NON fiable");
        assert_eq!(z.candidats(1, hachage_int(0)), None);
        z.confronter_ancres(500, 13);
        assert!(!z.fiable(), "la zone v2 a grandi : NON fiable");
    }

    /// Même garde éprouvée par mutation que sur l'index ordonné : une série
    /// dont on doute ne se reporte JAMAIS dans une fusion — les ancres
    /// neuves la béniraient comme complète.
    #[test]
    fn une_serie_dont_on_doute_ne_se_reporte_pas() {
        let (_d, mut z) = zone_temporaire();
        z.fusionner(&[(4u16, serie(80))], 10, 2).unwrap();
        assert_eq!(z.entrees_serie(4).len(), 80, "fiable : la série se lit");
        z.confronter_ancres(11, 2); // le WAL a avancé : on ne sait plus.
        assert!(!z.fiable());
        assert!(z.entrees_serie(4).is_empty(), "résultat attendu vide");
    }

    #[test]
    fn la_fusion_refuse_une_serie_non_triee() {
        let (_d, mut z) = zone_temporaire();
        let mut s = serie(10);
        s.swap(0, 9);
        assert!(
            z.fusionner(&[(1u16, s)], 0, 0).is_err(),
            "un index menteur doit être refusé, pas écrit"
        );
    }

    #[test]
    fn une_racine_corrompue_fait_jeter_l_index_entier() {
        let sf = SubstrateFile::open_tempfile().unwrap();
        let c = sf.path().join(PROPS_EQ_INDEX_FILENAME);
        let mut z = PropsEqIndexZone::ouvrir(&sf);
        z.fusionner(&[(1u16, serie(50))], 1, 1).unwrap();
        drop(z);
        let mut o = std::fs::read(&c).unwrap();
        o[60] ^= 0xFF;
        std::fs::write(&c, &o).unwrap();
        let mut z2 = PropsEqIndexZone::ouvrir_chemin(&c);
        z2.confronter_ancres(1, 1);
        assert!(!z2.fiable(), "CRC faux ⇒ index jeté ⇒ reconstruction");
    }

    /// Une zone de VERSION 1 (écrite avant la clé canonique), racine et pages
    /// au CRC valide : elle s'ouvre, elle est FIABLE une fois ses ancres
    /// confrontées, elle répond — et elle se déclare HÉRITÉE.
    #[test]
    fn une_zone_de_version_1_s_ouvre_comme_heritee() {
        let sf = SubstrateFile::open_tempfile().unwrap();
        let mut z = PropsEqIndexZone::ouvrir(&sf);
        z.fusionner_en(&[(3u16, serie(700))], 77, 9, false).unwrap();
        assert!(!z.canonique());
        drop(z);
        let c = sf.path().join(PROPS_EQ_INDEX_FILENAME);
        let o = std::fs::read(&c).unwrap();
        assert_eq!(
            u32::from_le_bytes(o[4..8].try_into().unwrap()),
            EQ_VERSION_HERITEE
        );
        let mut z2 = PropsEqIndexZone::ouvrir(&sf);
        z2.confronter_ancres(77, 9);
        assert!(z2.fiable(), "une zone v1 saine n'est pas jetée");
        assert!(!z2.canonique(), "… mais elle n'est pas canonique");
        assert_eq!(z2.candidats(3, hachage_int(699)).unwrap(), vec![699]);
        // Elle ne s'ÉTEND pas : l'extension la réécrirait en version courante
        // sans son complément.
        assert!(!z2.etendre_apres_fenetre(&[], 78, 9).unwrap());
        // Une fusion en version courante la remplace.
        z2.fusionner(&[(3u16, serie(700))], 77, 9).unwrap();
        assert!(z2.canonique());
    }

    /// Une version INCONNUE, elle, est jetée — et c'est bien la VERSION qui
    /// la fait jeter : le CRC de la racine est recalculé, donc valide.
    #[test]
    fn une_version_inconnue_au_crc_valide_est_jetee() {
        let sf = SubstrateFile::open_tempfile().unwrap();
        let c = sf.path().join(PROPS_EQ_INDEX_FILENAME);
        let mut z = PropsEqIndexZone::ouvrir(&sf);
        z.fusionner(&[(1u16, serie(50))], 1, 1).unwrap();
        drop(z);
        let ecrire_version = |version: u32| {
            let mut o = std::fs::read(&c).unwrap();
            o[4..8].copy_from_slice(&version.to_le_bytes());
            o[8..12].copy_from_slice(&0u32.to_le_bytes());
            let crc = crc32fast::hash(&o[..PAGE_SIZE]);
            o[8..12].copy_from_slice(&crc.to_le_bytes());
            std::fs::write(&c, &o).unwrap();
        };
        // Témoin : la même manipulation, vers une version CONNUE, laisse un
        // fichier accepté — le CRC recalculé est donc bien valide.
        ecrire_version(EQ_VERSION_HERITEE);
        let mut temoin = PropsEqIndexZone::ouvrir_chemin(&c);
        temoin.confronter_ancres(1, 1);
        assert!(temoin.fiable(), "témoin : CRC recalculé accepté");
        drop(temoin);
        ecrire_version(3);
        let mut z2 = PropsEqIndexZone::ouvrir_chemin(&c);
        z2.confronter_ancres(1, 1);
        assert!(!z2.fiable(), "version inconnue ⇒ index jeté");
    }

    /// La sonde, le décompte plafonné et l'énumération rendent ce que rend
    /// un parcours naïf — sur une série à valeurs distinctes ET sur une
    /// série peu sélective (longues plages d'un même hachage, qui défont
    /// l'estimation par interpolation).
    #[test]
    fn sonde_decompte_et_enumeration_egalent_le_parcours_naif() {
        let sf = SubstrateFile::open_tempfile().unwrap();
        let mut z = PropsEqIndexZone::ouvrir(&sf);
        // Série 1 : 5 000 hachages distincts, étalés. Série 2 : trois
        // hachages seulement, dont un porté par 3 000 slots.
        let mut distincte: Vec<EntreeEgalite> = (0..5_000u32)
            .map(|i| EntreeEgalite::new(hachage_int(i64::from(i)), i))
            .collect();
        distincte.sort_unstable_by_key(EntreeEgalite::ordre);
        let mut lourde: Vec<EntreeEgalite> = (0..4_000u32)
            .map(|i| {
                let h = match i {
                    0..=2_999 => hachage_int(0),
                    3_000..=3_998 => hachage_int(1),
                    _ => hachage_int(2),
                };
                EntreeEgalite::new(h, i)
            })
            .collect();
        lourde.sort_unstable_by_key(EntreeEgalite::ordre);
        z.fusionner(&[(1u16, distincte.clone()), (2u16, lourde.clone())], 1, 1)
            .unwrap();
        let naif = |serie: &[EntreeEgalite], h: u64| -> Vec<u32> {
            serie
                .iter()
                .filter(|e| e.hachage == h)
                .map(|e| e.slot)
                .collect()
        };
        let mut sondes: Vec<(u16, &[EntreeEgalite], u64)> = Vec::new();
        for i in [0i64, 1, 2, 17, 2_499, 4_999, 5_000, 123_456] {
            sondes.push((1, &distincte, hachage_int(i)));
            sondes.push((2, &lourde, hachage_int(i)));
        }
        sondes.push((1, &distincte, 0));
        sondes.push((1, &distincte, u64::MAX));
        sondes.push((2, &lourde, 0));
        sondes.push((2, &lourde, u64::MAX));
        for (pk, serie, h) in sondes {
            let attendu = naif(serie, h);
            let SondeZone::Plage(plage) = z.sonder(pk, h) else {
                panic!("la sonde doit localiser (série {pk}, hachage {h:#x})");
            };
            let mut slots = Vec::new();
            assert!(z.slots_de(&plage, &mut slots));
            assert_eq!(slots, attendu, "énumération, série {pk}, hachage {h:#x}");
            for plafond in [0u64, 1, 8, 128, 2_999, 3_000, 1_000_000] {
                let taille = z.taille_plafonnee(&plage, plafond).unwrap();
                assert_eq!(
                    taille,
                    (attendu.len() as u64).min(plafond + 1),
                    "décompte plafonné à {plafond}, série {pk}, hachage {h:#x}"
                );
            }
        }
        // Propriété absente, zone non fiable : la sonde ne conclut rien.
        assert!(matches!(z.sonder(9, hachage_int(1)), SondeZone::Inconnue));
        z.confronter_ancres(2, 2);
        assert!(matches!(z.sonder(1, hachage_int(1)), SondeZone::Inconnue));
    }

    /// L'adoption d'ancres (48 octets réécrits, aucune série touchée) doit
    /// SURVIVRE à la réouverture : la racine qu'elle écrit porte un CRC que
    /// l'ouverture accepte, et l'index répond sous ses ancres neuves.
    #[test]
    fn une_adoption_d_ancres_survit_a_la_reouverture() {
        let sf = SubstrateFile::open_tempfile().unwrap();
        let mut z = PropsEqIndexZone::ouvrir(&sf);
        z.fusionner(&[(3u16, serie(700)), (5u16, serie(20))], 77, 9)
            .unwrap();
        z.adopter_ancres(1_234, 42);
        assert!(z.fiable());
        let g = z.generation();
        drop(z);
        let mut z2 = PropsEqIndexZone::ouvrir(&sf);
        z2.confronter_ancres(1_234, 42);
        assert!(
            z2.fiable(),
            "la racine réécrite par l'adoption doit être acceptée à l'ouverture"
        );
        assert_eq!(z2.generation(), g);
        assert_eq!(z2.candidats(3, hachage_int(699)).unwrap(), vec![699]);
        assert_eq!(z2.candidats(5, hachage_int(19)).unwrap(), vec![19]);
        // Et sous les ANCIENNES ancres, elle ne répond plus.
        let mut z3 = PropsEqIndexZone::ouvrir(&sf);
        z3.confronter_ancres(77, 9);
        assert!(!z3.fiable());
    }

    #[test]
    fn une_fusion_survit_a_la_reouverture() {
        let sf = SubstrateFile::open_tempfile().unwrap();
        let mut z = PropsEqIndexZone::ouvrir(&sf);
        z.fusionner(&[(3u16, serie(700))], 77, 9).unwrap();
        let g = z.generation();
        drop(z);
        // AUCUNE reconstruction : on relit les octets, c'est tout le point.
        let mut z2 = PropsEqIndexZone::ouvrir(&sf);
        z2.confronter_ancres(77, 9);
        assert!(z2.fiable());
        assert_eq!(z2.generation(), g);
        assert_eq!(z2.candidats(3, hachage_int(699)).unwrap(), vec![699]);
    }

    // ─────────────────────────────────────────────────────────────────────
    // (2026-09-09, P1 ouverture rapide — W1-bis) EXTENSION DE FENÊTRE
    // ─────────────────────────────────────────────────────────────────────

    /// Une fusion, puis une réouverture dont la fenêtre post-fusion a ajouté
    /// une entrée : l'index est ÉTENDU (ancres reposées, FIABLE), et le
    /// candidat du hachage neuf répond — exactement comme après un rebuild.
    #[test]
    fn une_fenetre_expliquee_etend_l_index_sans_reconstruction() {
        let sf = SubstrateFile::open_tempfile().unwrap();
        // Fusion initiale : ancres (100, 10), slots 1..=50, hachage i*10.
        let mut entrees: Vec<EntreeEgalite> = (1..=50)
            .map(|i| EntreeEgalite::new(hachage_int(i64::from(i * 10)), i))
            .collect();
        entrees.sort_by_key(EntreeEgalite::ordre);
        {
            let mut z = PropsEqIndexZone::ouvrir(&sf);
            z.fusionner(&[(1u16, entrees)], 100, 10).unwrap();
        }
        // Réouverture APRÈS des écritures : ancres divergentes → NON fiable.
        let mut z2 = PropsEqIndexZone::ouvrir(&sf);
        z2.confronter_ancres(120, 11);
        assert!(!z2.fiable());
        assert_eq!(z2.candidats(1, hachage_int(510)), None);
        // La fenêtre a ajouté le slot 51 (hachage 510) : on l'étend.
        let ajouts = vec![(1u16, vec![EntreeEgalite::new(hachage_int(510), 51)])];
        assert!(z2.etendre_apres_fenetre(&ajouts, 120, 11).unwrap());
        assert!(z2.fiable(), "ancres reposées ⇒ FIABLE");
        assert_eq!(z2.ancre_wal(), 120);
        assert_eq!(z2.ancre_pages(), 11);
        assert_eq!(
            z2.candidats(1, hachage_int(510)).unwrap(),
            vec![51],
            "le nouvel état répond"
        );
        assert_eq!(
            z2.candidats(1, hachage_int(300)).unwrap(),
            vec![30],
            "l'ancien état est conservé"
        );
        assert_eq!(z2.nb_entrees_totales(), 51);
        assert_eq!(z2.entrees_serie(1).len(), 51);
    }

    /// Écart NON FORWARD : l'extension refuse et ne modifie rien.
    #[test]
    fn une_fenetre_non_forward_refuse_l_extension() {
        let (_d, mut z) = zone_temporaire();
        z.fusionner(&[(1u16, serie(10))], 100, 5).unwrap();
        assert!(!z.etendre_apres_fenetre(&[], 90, 5).unwrap());
        assert!(z.fiable(), "ancres inchangées : toujours fiable");
        z.confronter_ancres(100, 4);
        assert!(!z.fiable());
        assert!(!z.etendre_apres_fenetre(&[], 100, 4).unwrap());
        assert!(!z.fiable(), "refus ⇒ NON FIABLE ⇒ reconstruction");
    }

    /// Série de base au CRC faux : l'extension est REFUSÉE en erreur — on ne
    /// bénit jamais un fichier dont le contenu n'est pas garanti.
    #[test]
    fn une_serie_de_base_corrompue_fait_refuser_l_extension() {
        let sf = SubstrateFile::open_tempfile().unwrap();
        let c = sf.path().join(PROPS_EQ_INDEX_FILENAME);
        {
            let mut z = PropsEqIndexZone::ouvrir(&sf);
            z.fusionner(&[(1u16, serie(300))], 100, 5).unwrap();
        }
        let mut o = std::fs::read(&c).unwrap();
        o[PAGE_SIZE + 100] ^= 0x5A;
        std::fs::write(&c, &o).unwrap();
        let mut z2 = PropsEqIndexZone::ouvrir_chemin(&c);
        z2.confronter_ancres(150, 6);
        assert!(!z2.fiable());
        assert!(
            z2.etendre_apres_fenetre(&[(1u16, serie(3))], 150, 6)
                .is_err(),
            "CRC faux sur le base ⇒ extension REFUSÉE, jamais béni"
        );
        assert!(!z2.fiable());
    }

    /// L'index ordonné et l'index d'égalité coexistent dans le même
    /// répertoire sans se lire l'un l'autre : magics distincts.
    #[test]
    fn il_ne_lit_pas_le_fichier_de_l_index_ordonne() {
        let sf = SubstrateFile::open_tempfile().unwrap();
        let mut ord = crate::props_index::PropsIndexZone::ouvrir(&sf);
        ord.fusionner(
            &[(
                1u16,
                vec![crate::props_index::EntreeIndex::new(
                    [1u8; crate::props_index::LARGEUR_CLE],
                    1,
                )],
            )],
            5,
            5,
        )
        .unwrap();
        // On fait passer le fichier de l'ordonné pour celui de l'égalité.
        std::fs::copy(
            sf.path().join(crate::props_index::PROPS_INDEX_V2_FILENAME),
            sf.path().join(PROPS_EQ_INDEX_FILENAME),
        )
        .unwrap();
        let mut z = PropsEqIndexZone::ouvrir(&sf);
        z.confronter_ancres(5, 5);
        assert!(!z.fiable(), "magic étranger ⇒ jeté");
    }
}
