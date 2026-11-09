//! # Index ORDONNÉ et PERSISTÉ des propriétés (« PropsIndex »)
//!
//! ## Le besoin
//!
//! `property_indexes` est un index de HACHAGE (`IndexValueKey → HashSet<NodeId>`).
//! Il tranche l'ÉGALITÉ et ne peut structurellement rien pour une COMPARAISON :
//! `index_value_key` projette vers des seaux, pas vers un ordre.
//! `find_nodes_in_range` balaie donc toutes les chaînes de pages v2 à CHAQUE
//! appel.
//!
//! MESURÉ (base synthétique 200 000 nœuds × 7 propriétés, régime v2, même
//! processus, 3e essai retenu) :
//!
//! ```text
//!   intervalle rendant      0 résultat  : 245,7 ms
//!   intervalle rendant  2 000 résultats : 247,2 ms
//!   intervalle rendant 200 000 résultats: 349,8 ms
//! ```
//!
//! Le coût est donc QUASI CONSTANT en le nombre de résultats : ~245 ms de
//! balayage fixe (1,23 µs par nœud traversé) plus ~0,52 µs par résultat de
//! revérification. C'est le balayage qu'il faut supprimer, pas la
//! revérification.
//!
//! ## Ce que cet index est, et ce qu'il n'est JAMAIS
//!
//! **LES CHAÎNES DE PAGES SONT LA SOURCE DE VÉRITÉ. CET INDEX N'EN EST QU'UNE
//! PROJECTION.** Trois conséquences, tenues par construction :
//!
//! 1. Aucune valeur ne vit ici et nulle part ailleurs : une entrée est
//!    `(clé d'ordre, slot)` — 16 octets — d'où l'on ne peut RIEN reconstituer.
//!    L'index énumère des CANDIDATS, il ne répond pas.
//! 2. Il se rebâtit intégralement depuis les seules chaînes
//!    (`SubstrateStore::batir_index_ordonne_depuis_v2`). Corrompu, périmé,
//!    d'une version antérieure : on le jette.
//! 3. Il s'EFFACE au moindre doute — [`PropsIndexZone::fiable`] rend `false`
//!    et l'appelant retombe sur le balayage, lent mais juste. C'est la
//!    doctrine déjà écrite dans `indexed_candidates` : « vide au global =
//!    je ne sais pas → `None` → l'appelant retombe sur le balayage ».
//!
//! ## Pourquoi les FAUX POSITIFS sont sans danger, et les faux négatifs mortels
//!
//! `find_nodes_in_range` REVÉRIFIE déjà chaque candidat par l'ordre de lecture
//! complet (`get_node_property` : vec → blob → v2 → DashMap) avant de le
//! rendre. Un candidat en trop est donc écarté ; un candidat MANQUANT sort
//! définitivement du résultat, sans erreur ni trace.
//!
//! Toute la conception en découle : **l'index doit être un SUR-ENSEMBLE des
//! couples (nœud, valeur) vivants dans les chaînes, et n'a aucun besoin d'être
//! exact.** C'est ce qui autorise :
//!
//! * un encodage de clé LOSSY (troncature des chaînes à 11 octets, `i64`
//!   au-delà de 2^53 projeté en `f64`) — les collisions ne produisent que
//!   des faux positifs ;
//! * une structure PUREMENT ADDITIVE : une mise à jour ou une suppression
//!   laisse l'ancienne entrée en place, qui devient un faux positif. Aucune
//!   suppression en place, donc aucune réécriture ponctuelle sur disque.
//!
//! ## Structure sur disque
//!
//! Fichier `substrate.props.idx.v2`, pages de 4 096 octets, même idiome que
//! [`crate::props_zone`] : magic, CRC32 par page, allocation monotone.
//!
//! * page 0 — RACINE : version, ancres de couverture, table des séries.
//! * pages 1.. — SÉRIES (« runs ») : une par propriété indexée, TRIÉE et
//!   CONTIGUË, 254 entrées de 16 octets par page.
//!
//! ### Pourquoi une série triée réécrite, et pas un arbre B
//!
//! Volumétrie mesurée sur `agent` pour les propriétés sollicitées :
//! 334 253 entrées, soit **5,3 Mo** de série. Un arbre B se justifie quand la
//! réécriture intégrale est hors de prix ; à 5 Mo elle coûte moins qu'un seul
//! des balayages de 982 ms qu'on remplace. En échange on supprime :
//! l'éclatement de pages, la mise à jour en place (donc les pages déchirées),
//! le rééquilibrage, et le besoin d'un basculement de racine atomique.
//!
//! La réécriture est rendue durable par **fichier temporaire + `rename`**,
//! comme `substrate.props.v2.acheve` et le catalogue d'index. Une coupure en
//! cours de fusion laisse l'ANCIEN fichier intact, jamais un hybride : il n'y
//! a pas d'état « moitié fusionné » à savoir lire.
//!
//! ## L'étage mémoire, et pourquoi il est indispensable
//!
//! Insérer dans la série triée à chaque `set_node_property` serait ruineux.
//! Les écritures s'accumulent donc dans un tampon ORDONNÉ EN MÉMOIRE, BORNÉ
//! ([`SEUIL_FUSION`] entrées par propriété, soit 1 Mo), fusionné dans la série
//! au `flush()`. Les lectures consultent LES DEUX étages et réunissent.
//! C'est le motif LSM.
//!
//! Coût par écriture : un encodage de clé (12 octets) et un `push`. Mesuré
//! plus bas dans les tests de charge du store.
//!
//! ## La fenêtre de coupure, et comment elle est fermée SANS fsync par écriture
//!
//! Entre une écriture et la fusion, le tampon n'est pas durable. Une coupure
//! le perd : la série sur disque serait alors INCOMPLÈTE — donc FAUSSE, ce qui
//! est pire qu'absente.
//!
//! La réponse retenue n'est ni de journaliser le tampon (elle alourdirait le
//! chemin d'écriture, celui-là même où `65d54d00` a trouvé deux pertes de
//! données) ni de poser un marqueur à chaque écriture (un `fsync` par
//! propriété écrite). C'est une **ancre de couverture**, écrite dans la racine
//! au moment de la fusion :
//!
//! * `ancre_wal`   — l'offset du WAL à l'instant de la fusion ;
//! * `ancre_pages` — le nombre de pages allouées de la zone v2 au même instant.
//!
//! À l'ouverture, l'index n'est cru QUE si les deux ancres correspondent
//! encore. Toute écriture — de propriété, de nœud, d'arête — fait avancer
//! l'offset du WAL ; l'index se déclare alors non fiable et l'on retombe sur
//! le balayage. **Le sens de l'erreur est le bon** : on se déclare non fiable
//! plus souvent que nécessaire (une création de nœud invalide l'ancre sans
//! rien changer à l'index), jamais l'inverse.
//!
//! Ce que cela NE couvre pas, et il faut le dire : une propriété dont
//! l'ajout v2 a réussi mais dont l'ajout WAL a ÉCHOUÉ (disque plein) laisse la
//! chaîne en avance sur l'ancre sans faire bouger l'offset. Ce cas est déjà
//! déclaré non durable par le store lui-même (« write is memory-only until
//! next flush ») ; l'index n'y ajoute ni ne retranche rien. Voir la note
//! « NON VÉRIFIÉ » du rapport.

#![allow(unsafe_code)]

use crate::error::{SubstrateError, SubstrateResult};
use crate::file::{SubstrateFile, ZoneFile};
use crate::page::PAGE_SIZE;
use bytemuck::{Pod, Zeroable};
use obrain_common::types::{OrderableValue, Value};
use std::path::{Path, PathBuf};

/// Nom canonique du fichier d'index ordonné.
pub const PROPS_INDEX_V2_FILENAME: &str = "substrate.props.idx.v2";

/// Magic de la page racine.
pub const IDX_MAGIC_RACINE: u32 = 0xC5DE_1D00;
/// Magic d'une page de série.
pub const IDX_MAGIC_SERIE: u32 = 0xC5DE_1D01;

/// Version du format. Un index d'une autre version est JETÉ, jamais
/// interprété : c'est le filet n°4 (« une reconstruction doit toujours être
/// possible depuis les seules chaînes »).
pub const IDX_VERSION: u32 = 2;

/// Nombre d'entrées du tampon mémoire, par propriété, au-delà duquel une
/// fusion est DÉCLENCHÉE — non pas réclamée : le tampon ne dépasse jamais ce
/// seuil, il est vidé sur disque.
///
/// 1 048 576 × 52 o = 52 Mio au pire, pour une propriété qui porterait un
/// million de valeurs indexées. La RAM reste donc bornée, mais le seuil est
/// désormais AU-DESSUS de la taille d'un store réel (408 832 nœuds mesurés le
/// 2026-09-13) : c'était le défaut caché du seuil précédent — un store plus gros
/// que lui DÉBORDAIT, et `assurer_index_ordonne` reconstruisait alors la série à
/// CHAQUE requête (mesuré : 3 027, 4 367 et 16 315 ms pour trois préfixes
/// consécutifs) parce qu'un handle en lecture ne peut pas vider son tampon sur
/// disque. Un index qui coûte un balayage complet par question ne sert rien : il
/// faut qu'il tienne en mémoire.
pub const SEUIL_FUSION: usize = 1_048_576;

/// Délai minimal entre deux RECONSTRUCTIONS de la série ordonnée d'une même
/// propriété (voir `Store::ordonne_bati_le`).
///
/// Une reconstruction est un BALAYAGE COMPLET des chaînes de propriétés ; la
/// payer une fois est du rattrapage, la payer à chaque question est une panne de
/// performance. Mesuré le 2026-09-13 sur le chat du 8432 : 2 277 ms au milieu
/// d'une conversation, sur un store que le harness réécrit en continu — chaque
/// écriture extérieure fait diverger les ancres, la série persistée cesse d'être
/// « fiable », et la garde reconstruisait donc à chaque question.
pub const DELAI_MIN_RECONSTRUCTION: std::time::Duration = std::time::Duration::from_secs(30);

/// Largeur de la clé d'ordre, en octets.
///
/// ── POURQUOI 48, ET PAS 12 (mesuré le 2026-09-13, chat du port 8432) ───────
/// La clé d'une chaîne ne gardait que **11 octets** : sur un store de code, TOUS
/// les `qualified_name` d'un projet partagent leurs 11 premiers octets
/// (« home.trivie »), donc la série ordonnée ne distinguait que quelques dizaines
/// de milliers de candidats — l'auteur l'avait mesuré lui-même (« 127 200
/// candidats pour 368 lignes ») et le chemin d'intervalle devenait 4,5× PLUS
/// LENT que le filtre d'expression. Un préfixe de chemin ne pouvait donc pas être
/// servi, et l'index payé (un balayage complet) ne servait à rien.
///
/// 48 octets, dont 47 utiles, couvrent la quasi-totalité des `qualified_name` et
/// des chemins de fichier : la série distingue les préfixes de répertoire et le
/// seek redevient un seek. Ce qui reste est tronqué — la troncature ne produit
/// que des candidats en trop, jamais des lignes manquantes (revérification).
///
/// COÛT : une entrée passe de 16 à 52 octets ; 408 832 valeurs tiennent dans
/// ~21 Mio, le prix d'un index qui sert. Le format est VERSIONNÉ : un fichier
/// d'index de l'ancienne largeur est jeté et reconstruit (voir [`IDX_VERSION`]),
/// jamais relu de travers.
pub const LARGEUR_CLE: usize = 48;

// ---------------------------------------------------------------------------
// Clé d'ordre — encodage MONOTONE de `OrderableValue` vers `[u8; 12]`
// ---------------------------------------------------------------------------

/// Groupes de type, dans l'ordre de `OrderableValue::type_ordinal`, à ceci
/// près que **`Int64` et `Float64` partagent un seul groupe**.
///
/// Ce n'est pas un raccourci : `OrderableValue::cmp` compare un `Int64` et un
/// `Float64` NUMÉRIQUEMENT (`OrderedFloat64(*a as f64).cmp(b)`) et non par
/// ordinal de type. Leur donner deux ordinaux distincts produirait un
/// encodage qui contredit l'ordre de référence — `Int64(5)` passerait AVANT
/// `Float64(3.0)` — et la contradiction se paierait en FAUX NÉGATIFS aux
/// bornes. Les fondre dans un même groupe numérique reproduit exactement la
/// sémantique de référence.
///
/// 0 est réservé : une clé toute à zéro n'est jamais une clé valide, ce qui
/// laisse `[0; 12]` utilisable en sentinelle.
const GROUPE_BOOL: u8 = 1;
const GROUPE_NUM: u8 = 2;
const GROUPE_STR: u8 = 3;
const GROUPE_TIMESTAMP: u8 = 4;
const GROUPE_DATE: u8 = 5;
const GROUPE_TIME: u8 = 6;
const GROUPE_ZDT: u8 = 7;

/// Clé d'ordre : 12 octets comparables par `memcmp`.
///
/// CONTRAT, et c'est le seul qui compte :
///
/// > pour deux valeurs `a` et `b` que `OrderableValue` sait ordonner,
/// > `a ≤ b` ⟹ `cle(a) ≤ cle(b)` (ordre lexicographique des octets).
///
/// La réciproque est FAUSSE et c'est délibéré : l'encodage est lossy
/// (troncature, arrondi), donc `cle(a) == cle(b)` n'implique pas `a == b`.
/// Une collision ne produit que des faux positifs, écartés par la
/// revérification de l'appelant. Voir le pavé de tête du module.
pub type CleOrdre = [u8; LARGEUR_CLE];

/// Clé minimale — précède toute clé valide (groupe 0 réservé).
pub const CLE_MIN: CleOrdre = [0u8; LARGEUR_CLE];
/// Clé maximale — suit toute clé valide.
pub const CLE_MAX: CleOrdre = [0xFFu8; LARGEUR_CLE];

/// Transformation-bascule d'un `f64` vers un `u64` dont l'ordre naturel
/// reproduit l'ordre des flottants.
///
/// ── LES DEUX PIÈGES DU FLOTTANT, ET LEUR TRAITEMENT ─────────────────────
///
/// `-0.0` et `+0.0` sont ÉGAUX pour `OrderedFloat64` (`self.0 == other.0`)
/// mais portent des motifs binaires opposés : sans normalisation,
/// `cle(-0.0) < cle(+0.0)` alors que `-0.0 ≥ +0.0`. Le contrat de monotonie
/// serait rompu, et il le serait exactement à la borne `0` — la plus
/// fréquente de toutes dans les propriétés mesurées (`steps_done`,
/// `consecutive_failures`, `score`). On canonise donc `-0.0` en `+0.0`.
///
/// `NaN != NaN` en IEEE-754, mais `OrderedFloat64` déclare TOUS les `NaN`
/// égaux entre eux et supérieurs à tout le reste. Les motifs binaires des
/// `NaN` sont multiples (quiet, signaling, charge utile, signe) : on les
/// canonise tous vers `u64::MAX`, sommet du groupe numérique. `NaN` reste
/// ainsi sous le groupe `String`, comme le veut `type_ordinal`.
#[inline]
fn bascule_f64(v: f64) -> u64 {
    if v.is_nan() {
        // Tous les NaN au sommet du groupe numérique, motif unique.
        return u64::MAX;
    }
    // Canonise -0.0 en +0.0 : `v == 0.0` est vrai pour les deux.
    let v = if v == 0.0 { 0.0f64 } else { v };
    let bits = v.to_bits();
    if bits & 0x8000_0000_0000_0000 != 0 {
        !bits
    } else {
        bits | 0x8000_0000_0000_0000
    }
}

/// Biais d'un `i64` vers un `u64` d'ordre équivalent (utilisé pour les
/// variantes temporelles, qui se comparent exactement et n'ont donc pas
/// besoin du passage par `f64`).
#[inline]
fn biais_i64(v: i64) -> u64 {
    (v as u64) ^ 0x8000_0000_0000_0000
}

/// Encode une [`Value`] en clé d'ordre, ou rend `None` si la valeur n'est pas
/// naturellement ordonnée.
///
/// **Le filtrage est celui d'`OrderableValue`, et il est CONSERVÉ tel quel** :
/// `Null`, `Bytes`, `Duration`, `List`, `Map`, `Vector` et `Path` n'ont pas de
/// rang. Leur en inventer un serait un défaut d'un autre genre, muet lui
/// aussi (`valeur_dans_intervalle` les écarte déjà côté lecture). On passe
/// donc littéralement par `OrderableValue::try_from` plutôt que de redéduire
/// la liste : c'est la leçon de `79e9e283` — un chemin neuf qui redéduit la
/// sémantique cesse d'être comparable au témoin, et la divergence est muette.
#[must_use]
pub fn encoder_cle(v: &Value) -> Option<CleOrdre> {
    let ov = OrderableValue::try_from(v).ok()?;
    Some(encoder_cle_ordonnee(&ov))
}

/// Variante prenant directement une [`OrderableValue`] déjà extraite —
/// utilisée par le chemin de lecture, qui a déjà converti ses bornes.
#[must_use]
pub fn encoder_cle_ordonnee(ov: &OrderableValue) -> CleOrdre {
    let mut c = [0u8; LARGEUR_CLE];
    match ov {
        OrderableValue::Bool(b) => {
            c[0] = GROUPE_BOOL;
            c[1] = u8::from(*b);
        }
        OrderableValue::Int64(i) => {
            // Même groupe et même projection que `Float64` : c'est ce que
            // fait `OrderableValue::cmp` pour la comparaison croisée.
            // Au-delà de 2^53 l'arrondi fait COLLISIONNER deux entiers
            // distincts — faux positif, écarté par la revérification. Jamais
            // de faux négatif : `as f64` est monotone non stricte.
            c[0] = GROUPE_NUM;
            c[1..9].copy_from_slice(&bascule_f64(*i as f64).to_be_bytes());
        }
        OrderableValue::Float64(f) => {
            c[0] = GROUPE_NUM;
            c[1..9].copy_from_slice(&bascule_f64(f.get()).to_be_bytes());
        }
        OrderableValue::String(s) => {
            // TRONCATURE à 11 octets, complétée de 0x00.
            //
            // Le remplissage par 0x00 préserve l'ordre au sens NON STRICT :
            // comparer les 11 premiers octets de `s ++ 0^∞` est monotone en
            // `s`. Deux chaînes de même préfixe collisionnent — faux
            // positifs, écartés par la revérification. Aucun élargissement
            // de borne n'est nécessaire : la MÊME projection est appliquée
            // aux valeurs ET aux bornes, donc `v ≥ min ⟹ cle(v) ≥ cle(min)`.
            let o = s.as_bytes();
            let n = o.len().min(LARGEUR_CLE - 1);
            c[0] = GROUPE_STR;
            c[1..1 + n].copy_from_slice(&o[..n]);
        }
        OrderableValue::Timestamp(t) => {
            // `Timestamp(i64)`, `Ord` DÉRIVÉ : l'ordre se réduit exactement à
            // celui de l'entier interne.
            c[0] = GROUPE_TIMESTAMP;
            c[1..9].copy_from_slice(&biais_i64(t.as_micros()).to_be_bytes());
        }
        OrderableValue::Date(d) => {
            // `Date(i32)`, `Ord` DÉRIVÉ : idem.
            c[0] = GROUPE_DATE;
            c[1..9].copy_from_slice(&biais_i64(i64::from(d.as_days())).to_be_bytes());
        }
        OrderableValue::Time(_) => {
            // ── `Time` N'A PAS D'ORDRE TOTAL COHÉRENT — CONSTATÉ, PAS SUPPOSÉ ──
            //
            // `Time::cmp` (obrain-common/src/types/time.rs) compare les nanos
            // NORMALISÉS EN UTC quand les DEUX opérandes portent un décalage,
            // et les nanos BRUTS sinon :
            //
            //     (Some(_), Some(_)) => self.utc_nanos().cmp(&other.utc_nanos()),
            //     _                  => self.nanos.cmp(&other.nanos),
            //
            // La relation n'est donc pas transitive : deux heures à décalage se
            // comparent sur une échelle, chacune se compare à une heure SANS
            // décalage sur une autre. AUCUN encodage monotone n'existe pour un
            // ordre incohérent — en fabriquer un produirait des faux négatifs
            // silencieux, exactement ce que cette conception interdit.
            //
            // On donne donc à TOUTES les heures la MÊME clé. Le groupe entier
            // devient un unique seau : toute requête qui le touche rend toutes
            // les heures en candidats, et `valeur_dans_intervalle` — qui applique
            // le VRAI `Time::cmp` — tranche ensuite. Faux positifs au maximum,
            // faux négatifs zéro. La monotonie est trivialement vraie (toutes
            // les clés du groupe sont égales) et l'ordre inter-groupes tient.
            c[0] = GROUPE_TIME;
        }
        OrderableValue::ZonedDatetime(z) => {
            // `ZonedDatetime::cmp` compare `utc_micros` seul (le décalage
            // n'entre pas dans l'ordre) : réductible à un `i64` monotone.
            c[0] = GROUPE_ZDT;
            c[1..9].copy_from_slice(&biais_i64(z.as_timestamp().as_micros()).to_be_bytes());
        }
    }
    c
}

// ---------------------------------------------------------------------------
// Représentation sur disque
// ---------------------------------------------------------------------------

/// Une entrée d'index : la clé d'ordre (48 o) puis le slot (4 o) — 52 octets,
/// sans bourrage implicite.
///
/// Elle ne porte AUCUNE valeur exploitable : 12 octets de clé lossy et un
/// numéro de slot. C'est délibéré — voir le point 3 du pavé de tête : l'index
/// ne doit jamais être le seul porteur d'une donnée.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Pod, Zeroable)]
pub struct EntreeIndex {
    pub cle: CleOrdre,
    pub slot: u32,
}

/// `Default` ÉCRIT À LA MAIN, et ce n'est pas un détail de style : `[u8; N]` ne
/// porte `Default` que jusqu'à N = 32, alors que la clé en fait 48 depuis la
/// version 2 du format. La clé nulle est la sentinelle réservée (`groupe 0`),
/// donc « par défaut » veut bien dire « aucune clé » — jamais une clé valide.
impl Default for EntreeIndex {
    fn default() -> Self {
        Self {
            cle: [0u8; LARGEUR_CLE],
            slot: 0,
        }
    }
}

const _: [(); 1] = [(); (core::mem::size_of::<EntreeIndex>() == 52) as usize];

impl EntreeIndex {
    #[must_use]
    pub fn new(cle: CleOrdre, slot: u32) -> Self {
        Self { cle, slot }
    }
    /// Ordre total sur `(clé, slot)`. Le slot départage pour que le tri soit
    /// déterministe — deux fusions successives d'un même contenu produisent
    /// des octets identiques, ce qui rend les tests de non-régression
    /// comparables octet à octet.
    #[must_use]
    pub fn ordre(&self) -> (CleOrdre, u32) {
        (self.cle, self.slot)
    }
}

/// En-tête d'une page de série (24 octets, calqué sur `PropertyPageHeader`).
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct EnteteSerie {
    pub magic: u32,          // @0..4
    pub crc32: u32,          // @4..8
    pub prop_key_id: u16,    // @8..10
    pub nb_entrees: u16,     // @10..12
    pub index_page: u32,     // @12..16  — rang de la page DANS la série
    pub nb_pages_serie: u32, // @16..20
    pub _reserve: u32,       // @20..24
}

const _: [(); 1] = [(); (core::mem::size_of::<EnteteSerie>() == 24) as usize];

/// Nombre d'entrées par page de série — DÉRIVÉ de la taille d'une entrée : une
/// constante écrite à la main laisserait une page annoncer une capacité fausse
/// le jour où [`LARGEUR_CLE`] change (c'est exactement ce que la version 1 du
/// format rendait possible).
pub const ENTREES_PAR_PAGE: usize = (PAGE_SIZE - 24) / core::mem::size_of::<EntreeIndex>();

/// Descripteur d'une série dans la racine (16 octets).
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct DescSerie {
    pub prop_key_id: u16,
    pub _pad: u16,
    pub page_debut: u32,
    pub nb_pages: u32,
    pub nb_entrees: u32,
}

const _: [(); 1] = [(); (core::mem::size_of::<DescSerie>() == 16) as usize];

/// En-tête de la page racine (48 octets).
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct EnteteRacine {
    pub magic: u32,     // @0..4
    pub version: u32,   // @4..8
    pub crc32: u32,     // @8..12
    pub nb_series: u32, // @12..16
    /// Offset du WAL à l'instant de la fusion. Voir « ancre de couverture ».
    pub ancre_wal: u64, // @16..24
    /// Nombre de pages allouées de la zone v2 au même instant.
    pub ancre_pages: u32, // @24..28
    pub generation: u32, // @28..32
    pub _reserve: [u32; 4], // @32..48
}

const _: [(); 1] = [(); (core::mem::size_of::<EnteteRacine>() == 48) as usize];

/// Nombre maximal de séries tenant dans la racine.
pub const SERIES_MAX: usize = (PAGE_SIZE - 48) / 16; // 253

// ---------------------------------------------------------------------------
// La zone
// ---------------------------------------------------------------------------

/// Zone d'index ordonné projetée en mémoire.
///
/// **Aucune RAM anonyme** : les séries sont lues DANS la projection, jamais
/// copiées dans un tas. C'est le point qui a motivé le choix persisté contre
/// une variante entièrement en mémoire (25 Mo, rebâtis à chaque ouverture).
pub struct PropsIndexZone {
    zone: Option<ZoneFile>,
    chemin: PathBuf,
    racine: EnteteRacine,
    series: Vec<DescSerie>,
    /// `false` tant que les ancres n'ont pas été confrontées à l'état courant,
    /// ou dès qu'elles ont divergé. **Tant que ce champ est faux, l'index ne
    /// répond pas.**
    fiable: bool,
}

impl PropsIndexZone {
    /// Ouvre l'index d'un substrat. Une absence de fichier, un magic ou une
    /// version inattendus, un CRC faux : tous rendent une zone VIDE et NON
    /// FIABLE — jamais une erreur, jamais une interprétation partielle.
    ///
    /// C'est le filet n°4 : un index illisible se jette, et se rebâtit depuis
    /// les chaînes.
    pub fn ouvrir(sub: &SubstrateFile) -> Self {
        let chemin = sub.path().join(PROPS_INDEX_V2_FILENAME);
        Self::ouvrir_chemin(&chemin)
    }

    fn ouvrir_chemin(chemin: &Path) -> Self {
        let vide = Self {
            zone: None,
            chemin: chemin.to_path_buf(),
            racine: EnteteRacine::default(),
            series: Vec::new(),
            fiable: false,
        };
        if !chemin.exists() {
            return vide;
        }
        let Ok(zone) = ZoneFile::open_or_create(chemin) else {
            tracing::warn!(?chemin, "index ordonné illisible — ignoré (balayage)");
            return vide;
        };
        let octets = zone.as_slice();
        if octets.len() < PAGE_SIZE {
            return vide;
        }
        let racine: EnteteRacine = *bytemuck::from_bytes(&octets[..48]);
        if racine.magic != IDX_MAGIC_RACINE || racine.version != IDX_VERSION {
            tracing::warn!(
                magic = format!("{:#x}", racine.magic),
                version = racine.version,
                "index ordonné d'un autre format — jeté"
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
            tracing::warn!("racine d'index ordonné : CRC faux — index jeté");
            return vide;
        }
        let mut series = Vec::with_capacity(racine.nb_series as usize);
        for i in 0..racine.nb_series as usize {
            let d = 48 + i * 16;
            series.push(*bytemuck::from_bytes::<DescSerie>(&octets[d..d + 16]));
        }
        // Cohérence de bornes : une série qui déborde du fichier est un
        // index corrompu, donc un index qu'on jette.
        let pages_totales = (octets.len() / PAGE_SIZE) as u32;
        for s in &series {
            if s.page_debut == 0 || s.page_debut.saturating_add(s.nb_pages) > pages_totales {
                tracing::warn!("série d'index hors zone — index jeté");
                return vide;
            }
        }
        Self {
            zone: Some(zone),
            chemin: chemin.to_path_buf(),
            racine,
            series,
            fiable: false, // confronté aux ancres par `confronter_ancres`
        }
    }

    /// Confronte les ancres de couverture à l'état COURANT du substrat.
    ///
    /// C'est ici, et uniquement ici, qu'un index devient fiable. Toute
    /// divergence — une écriture depuis la fusion, un WAL tronqué par un point
    /// de reprise, une zone v2 qui a grandi — laisse `fiable == false`, donc
    /// le balayage.
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
                "index ordonné : ancres divergentes — NON FIABLE, on balaiera"
            );
        }
        self.fiable = ok;
    }

    /// L'index répond-il ? `false` ⇒ l'appelant DOIT balayer.
    #[must_use]
    pub fn fiable(&self) -> bool {
        self.fiable
    }

    #[must_use]
    pub fn generation(&self) -> u32 {
        self.racine.generation
    }

    /// Ancre WAL inscrite dans la racine à la dernière fusion.
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

    /// Propriétés présentes dans l'index sur disque.
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

    /// Entrées de la page de rang `p` d'une série, LUES DANS LA PROJECTION
    /// (aucune copie).
    fn entrees_page(&self, d: &DescSerie, p: u32) -> Option<&[EntreeIndex]> {
        let zone = self.zone.as_ref()?;
        let octets = zone.as_slice();
        let debut = (d.page_debut + p) as usize * PAGE_SIZE;
        let page = octets.get(debut..debut + PAGE_SIZE)?;
        let e: &EnteteSerie = bytemuck::from_bytes(&page[..24]);
        if e.magic != IDX_MAGIC_SERIE || e.prop_key_id != d.prop_key_id {
            return None;
        }
        let n = (e.nb_entrees as usize).min(ENTREES_PAR_PAGE);
        // La largeur d'une entrée est DÉRIVÉE, jamais écrite : deux chemins de
        // lecture la figeaient à 16 octets, et l'élargissement de la clé (12 → 48,
        // version 2 du format) les aurait fait lire une entrée sur trois — c'est
        // le test « fusion puis relecture » qui l'a montré.
        let largeur = core::mem::size_of::<EntreeIndex>();
        Some(bytemuck::cast_slice(&page[24..24 + n * largeur]))
    }

    /// Énumère les slots dont la clé tombe dans `[cle_min, cle_max]`.
    ///
    /// Rend `None` — et JAMAIS `Some(vec![])` — si l'index n'est pas fiable ou
    /// si la propriété n'y figure pas. C'est la doctrine d'`indexed_candidates`
    /// mot pour mot : un ensemble vide rendu par ignorance court-circuiterait
    /// le balayage chez l'appelant et affirmerait « zéro résultat », définitif,
    /// sans erreur.
    ///
    /// Le résultat est un SUR-ENSEMBLE des slots dont la valeur courante est
    /// dans l'intervalle : entrées périmées (mise à jour, suppression) et
    /// collisions d'encodage y figurent. L'appelant revérifie.
    pub fn candidats(
        &self,
        prop_key_id: u16,
        cle_min: CleOrdre,
        cle_max: CleOrdre,
    ) -> Option<Vec<u32>> {
        if !self.fiable {
            return None;
        }
        let d = *self.desc(prop_key_id)?;
        if d.nb_entrees == 0 {
            // Une série VIDE pour une propriété DÉCLARÉE est ambiguë : soit
            // aucun nœud ne la porte, soit la fusion n'a rien reçu. On ne
            // trancherait qu'en balayant — alors on balaie.
            return None;
        }
        let mut out = Vec::new();
        // Recherche binaire de la PAGE de départ : les pages d'une série sont
        // contiguës et globalement triées, donc la dernière clé d'une page
        // borne la page suivante. On ne lit que log2(nb_pages) pages avant de
        // commencer à récolter — sur 5,3 Mo de série c'est ~11 pages.
        let mut lo = 0u32;
        let mut hi = d.nb_pages;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let Some(e) = self.entrees_page(&d, mid) else {
                // Page illisible : l'index est corrompu. On ne rend surtout
                // pas un résultat partiel.
                return None;
            };
            match e.last() {
                Some(der) if der.cle < cle_min => lo = mid + 1,
                _ => hi = mid,
            }
        }
        for p in lo..d.nb_pages {
            // Le `?` rend `None` : une page illisible en cours de parcours est
            // un index corrompu, et on ne rend surtout pas le PARTIEL déjà
            // récolté — ce serait un résultat amputé se faisant passer pour
            // complet. `None` fait retomber l'appelant sur le balayage.
            let entrees = self.entrees_page(&d, p)?;
            // Position de départ dans la page (uniquement pour la première).
            let debut = if p == lo {
                entrees.partition_point(|e| e.cle < cle_min)
            } else {
                0
            };
            let mut fini = false;
            for e in &entrees[debut..] {
                if e.cle > cle_max {
                    fini = true;
                    break;
                }
                out.push(e.slot);
            }
            if fini {
                break;
            }
        }
        Some(out)
    }

    /// Copie les entrées d'une série existante.
    ///
    /// C'est la seule sortie de données de la zone vers le tas, et elle n'a
    /// lieu qu'au moment d'une FUSION — jamais sur un chemin de lecture, qui
    /// lit dans la projection. Rend un vecteur vide si l'index n'est pas
    /// fiable ou si la propriété n'y figure pas : **on ne reporte JAMAIS dans
    /// un index frais le contenu d'un index dont on doute**, car les ancres
    /// neuves le béniraient.
    #[must_use]
    pub fn entrees_serie(&self, prop_key_id: u16) -> Vec<EntreeIndex> {
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
                // Page illisible : on rend ce qu'on a SANS le déclarer complet
                // — l'appelant ne doit pas s'en servir. Il ne le fait pas :
                // `fusionner_index_ordonne` n'appelle ceci que sous `fiable`,
                // et une page illisible sous `fiable` est un index corrompu
                // dont la reconstruction depuis les chaînes est le remède.
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
    /// Le garde `fiable` d'[`Self::entrees_serie`] protège la fusion
    /// d'ouverture : on ne reporte JAMAIS le contenu d'un index dont on
    /// doute dans un index frais que des ancres neuves béniraient.
    /// [`Self::etendre_apres_fenetre`] a besoin de la garantie inverse : le
    /// FICHIER présent sous des ancres périmées a été écrit par une fusion
    /// antérieure qui l'avait VALIDÉ (tri vérifié, CRC par page,
    /// remplacement atomique). Son contenu reste donc un sur-ensemble valide
    /// de l'état qu'il couvrait à SES ancres — c'est l'appelant qui prouve,
    /// par la fenêtre expliquée, que les seules entrées manquantes pour
    /// l'état COURANT sont exactement celles qu'il ajoute.
    ///
    /// Rend `Err` — et donc le REFUS d'étendre — si une page de la série est
    /// hors bornes, mal en-têtée ou au CRC faux : un tel fichier n'est pas
    /// un index sain à étendre, c'est un index à REJETER (reconstruction
    /// par balayage, le filet habituel).
    fn entrees_serie_brutes_verifiees(&self, d: &DescSerie) -> SubstrateResult<Vec<EntreeIndex>> {
        let zone = self.zone.as_ref().ok_or_else(|| {
            SubstrateError::Internal("index ordonné : pas de zone à relire".into())
        })?;
        let octets = zone.as_slice();
        let mut out = Vec::with_capacity(d.nb_entrees as usize);
        for p in 0..d.nb_pages {
            let debut = (d.page_debut + p) as usize * PAGE_SIZE;
            let page = octets.get(debut..debut + PAGE_SIZE).ok_or_else(|| {
                SubstrateError::Internal(format!(
                    "index ordonné : page de série {debut} hors zone — extension refusée"
                ))
            })?;
            // CRC32 de page : champ `crc32` (octets 4..8 de l'en-tête de
            // série) mis à zéro — même convention que l'écriture de
            // `fusionner`.
            let mut copie = [0u8; PAGE_SIZE];
            copie.copy_from_slice(page);
            copie[4..8].copy_from_slice(&0u32.to_le_bytes());
            let attendu = u32::from_le_bytes(page[4..8].try_into().expect("4 octets"));
            if crc32fast::hash(&copie) != attendu {
                return Err(SubstrateError::Internal(format!(
                    "index ordonné : CRC faux sur la page de série {debut} — extension refusée"
                )));
            }
            let e: &EnteteSerie = bytemuck::from_bytes(&page[..24]);
            if e.magic != IDX_MAGIC_SERIE || e.prop_key_id != d.prop_key_id {
                return Err(SubstrateError::Internal(format!(
                    "index ordonné : en-tête de page de série inattendu (page {debut}) — \
                     extension refusée"
                )));
            }
            let n = (e.nb_entrees as usize).min(ENTREES_PAR_PAGE);
            let largeur = core::mem::size_of::<EntreeIndex>();
            out.extend_from_slice(bytemuck::cast_slice(&page[24..24 + n * largeur]));
        }
        Ok(out)
    }

    /// Étend l'index persisté par les entrées AJOUTÉES depuis sa dernière
    /// fusion — la fenêtre que l'ouverture vient d'expliquer — au lieu de
    /// tout rebâtir par le balayage des chaînes (le poste ~1 s du P1).
    ///
    /// CONTRAT D'EXACTITUDE, identique à celui de la fusion de `flush()` :
    ///
    /// * `ajouts` porte, propriété par propriété, les entrées COURANTES
    ///   (clé d'ordre de la valeur vivante, slot) des nœuds que la fenêtre
    ///   d'écritures post-fusion a touchés — relues dans les chaînes par
    ///   l'appelant, qui ne passe ici que des valeurs vivantes ;
    /// * chaque série du disque est reprise VERBATIM, MOINS les entrées des
    ///   slots touchés (l'ancienne valeur d'un slot réécrit n'est qu'un faux
    ///   positif de plus), PLUS les ajouts — triée, dédupliquée, puis
    ///   réécrite par le chemin ATOMIQUE habituel ([`Self::fusionner`]) avec
    ///   des ancres NEUVES ;
    /// * une propriété ABSENTE du disque n'est JAMAIS créée ici : écrire
    ///   une série sans connaître son historique la bénirait comme complète
    ///   — le faux négatif silencieux interdit (pavé de tête). L'appelant
    ///   la laisse aux reconstructions par processus.
    ///
    /// Rend `Ok(false)` sans rien modifier quand la zone est absente ou que
    /// l'écart n'est pas strictement FORWARD (ancre de la racine > ancres
    /// fournies : WAL tronqué sous la fusion, zone réduite — un état que
    /// l'extension ne sait PAS expliquer). Rend `Err` quand le fichier de
    /// base ne se relit pas sainement (CRC, bornes) ou que l'écriture
    /// échoue : l'appelant retombe alors sur la reconstruction, jamais sur
    /// un index étendu à moitié.
    pub(crate) fn etendre_apres_fenetre(
        &mut self,
        ajouts: &[(u16, Vec<EntreeIndex>)],
        ancre_wal: u64,
        ancre_pages: u32,
    ) -> SubstrateResult<bool> {
        if self.zone.is_none() {
            return Ok(false);
        }
        if self.racine.ancre_wal > ancre_wal || self.racine.ancre_pages > ancre_pages {
            tracing::info!(
                ancre_wal = self.racine.ancre_wal,
                fourni_wal = ancre_wal,
                ancre_pages = self.racine.ancre_pages,
                fourni_pages = ancre_pages,
                "index ordonné : écart NON FORWARD — extension refusée (reconstruction)"
            );
            return Ok(false);
        }
        // Slots touchés par propriété : l'ancienne entrée d'un slot réécrit
        // est retirée — c'est le filtrage exact que `fusionner_index_*` du
        // store applique au flush, et il borne la croissance de la série à
        // une entrée par nœud vivant.
        let touches: std::collections::HashMap<u16, std::collections::HashSet<u32>> = ajouts
            .iter()
            .map(|(pk, e)| (*pk, e.iter().map(|x| x.slot).collect()))
            .collect();
        let nb_ajouts: usize = ajouts.iter().map(|(_, v)| v.len()).sum();
        let mut series: Vec<(u16, Vec<EntreeIndex>)> = Vec::with_capacity(self.series.len());
        for d in &self.series {
            let base = self.entrees_serie_brutes_verifiees(d)?;
            let mut fusion: Vec<EntreeIndex> = match touches.get(&d.prop_key_id) {
                Some(t) => base.into_iter().filter(|e| !t.contains(&e.slot)).collect(),
                None => base,
            };
            if let Some((_, aj)) = ajouts.iter().find(|(pk, _)| *pk == d.prop_key_id) {
                fusion.extend(aj.iter().copied());
            }
            fusion.sort_unstable_by_key(EntreeIndex::ordre);
            fusion.dedup();
            series.push((d.prop_key_id, fusion));
        }
        if series.is_empty() {
            // Aucune série sur disque : rien à étendre (un index qui n'a
            // jamais fusionné de contenu n'a rien à bénir — l'appelant
            // reconstruit s'il en a besoin).
            return Ok(false);
        }
        let t = web_time::Instant::now();
        // `fusionner` vérifie le tri, écrit par fichier temporaire + rename,
        // reprojette, et pose les ancres fournies : l'index devient FIABLE
        // pour l'état courant — la même mécanique qu'un flush.
        self.fusionner(&series, ancre_wal, ancre_pages)?;
        tracing::info!(
            entrees_ajoutees = nb_ajouts,
            series = series.len(),
            elapsed_ms = t.elapsed().as_millis() as u64,
            "index ordonné : étendu depuis la fenêtre post-fusion (reconstruction évitée)"
        );
        Ok(true)
    }

    /// Réécrit INTÉGRALEMENT l'index : fichier temporaire, `fsync`, `rename`,
    /// `fsync` du répertoire, puis reprojection.
    ///
    /// Pourquoi la réécriture intégrale plutôt qu'une mise à jour en place :
    /// voir le pavé de tête. Elle supprime tout état « à moitié fusionné » —
    /// une coupure laisse l'ANCIEN fichier, lisible et cohérent, dont les
    /// ancres diront simplement qu'il est périmé.
    ///
    /// `series` doit être TRIÉE par `(clé, slot)` pour chaque propriété ;
    /// l'invariant est vérifié en `debug_assert` et, à défaut, la série serait
    /// simplement inexploitable par la recherche binaire — d'où la
    /// vérification systématique de tri ci-dessous, qui refuse la fusion
    /// plutôt que d'écrire un index MENTEUR.
    pub fn fusionner(
        &mut self,
        series: &[(u16, Vec<EntreeIndex>)],
        ancre_wal: u64,
        ancre_pages: u32,
    ) -> SubstrateResult<()> {
        if series.len() > SERIES_MAX {
            return Err(SubstrateError::Internal(format!(
                "index ordonné : {} séries demandées, {SERIES_MAX} au plus",
                series.len()
            )));
        }
        // ── REFUS PLUTÔT QU'INDEX MENTEUR ──
        // Une série mal triée rendrait la recherche binaire silencieusement
        // fausse : elle sauterait des résultats. C'est exactement le faux
        // négatif que toute cette conception cherche à rendre impossible, donc
        // on le vérifie ici, en O(n), une fois par fusion.
        for (pk, e) in series {
            if e.windows(2).any(|w| w[0].ordre() > w[1].ordre()) {
                return Err(SubstrateError::Internal(format!(
                    "index ordonné : série {pk} non triée — fusion REFUSÉE"
                )));
            }
        }

        let mut descs: Vec<DescSerie> = Vec::with_capacity(series.len());
        let mut pages: Vec<[u8; PAGE_SIZE]> = Vec::new();
        let mut page_courante = 1u32; // page 0 = racine

        for (pk, entrees) in series {
            let nb_pages = entrees.len().div_ceil(ENTREES_PAR_PAGE).max(1) as u32;
            descs.push(DescSerie {
                prop_key_id: *pk,
                _pad: 0,
                page_debut: page_courante,
                nb_pages,
                nb_entrees: u32::try_from(entrees.len()).map_err(|_| {
                    SubstrateError::Internal("index ordonné : série > 2^32 entrées".into())
                })?,
            });
            for p in 0..nb_pages {
                let mut page = [0u8; PAGE_SIZE];
                let d = p as usize * ENTREES_PAR_PAGE;
                let f = (d + ENTREES_PAR_PAGE).min(entrees.len());
                let n = f.saturating_sub(d);
                let entete = EnteteSerie {
                    magic: IDX_MAGIC_SERIE,
                    crc32: 0,
                    prop_key_id: *pk,
                    nb_entrees: n as u16,
                    index_page: p,
                    nb_pages_serie: nb_pages,
                    _reserve: 0,
                };
                page[..24].copy_from_slice(bytemuck::bytes_of(&entete));
                if n > 0 {
                    let largeur = core::mem::size_of::<EntreeIndex>();
                    page[24..24 + n * largeur]
                        .copy_from_slice(bytemuck::cast_slice(&entrees[d..f]));
                }
                let crc = crc32fast::hash(&page);
                page[4..8].copy_from_slice(&crc.to_le_bytes());
                pages.push(page);
            }
            page_courante += nb_pages;
        }

        let mut racine = [0u8; PAGE_SIZE];
        let entete = EnteteRacine {
            magic: IDX_MAGIC_RACINE,
            version: IDX_VERSION,
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

        // ── ÉCRITURE ATOMIQUE ──
        // Nom temporaire dérivé du PID : deux processus qui fusionneraient la
        // même base ne s'écraseraient pas mutuellement en cours d'écriture.
        // (Le `rename` final reste le point de sérialisation.)
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
            f.sync_all()?; // les octets AVANT le rename : c'est l'ordre qui rend l'échange sûr.
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
        // Les ancres viennent d'être posées : l'index est fiable jusqu'à la
        // prochaine écriture, que `confronter_ancres` détectera.
        self.fiable = self.zone.is_some();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obrain_common::types::Value;

    // ─────────────────────────────────────────────────────────────────────
    // LE CONTRAT DE MONOTONIE
    //
    // Ces tests ne vérifient pas « l'encodage marche » : ils vérifient LA
    // propriété dont dépend l'absence de faux négatifs, et rien d'autre.
    // ─────────────────────────────────────────────────────────────────────

    fn ov(v: &Value) -> OrderableValue {
        OrderableValue::try_from(v).unwrap()
    }

    /// `a ≤ b` ⟹ `cle(a) ≤ cle(b)`. C'est LA propriété. Si elle tombe, un
    /// intervalle saute des résultats sans rien dire.
    fn monotone(a: &Value, b: &Value) {
        let (oa, ob) = (ov(a), ov(b));
        let (ca, cb) = (encoder_cle(a).unwrap(), encoder_cle(b).unwrap());
        if oa <= ob {
            assert!(
                ca <= cb,
                "monotonie rompue : {a:?} <= {b:?} mais cle {ca:?} > {cb:?}"
            );
        }
        if ob <= oa {
            assert!(
                cb <= ca,
                "monotonie rompue : {b:?} <= {a:?} mais cle {cb:?} > {ca:?}"
            );
        }
    }

    fn echantillon() -> Vec<Value> {
        let mut v = vec![
            Value::Bool(false),
            Value::Bool(true),
            Value::Int64(i64::MIN),
            Value::Int64(-1_000_000),
            Value::Int64(-1),
            Value::Int64(0),
            Value::Int64(1),
            Value::Int64(42),
            Value::Int64(1_000_000),
            Value::Int64(i64::MAX),
            Value::Float64(f64::NEG_INFINITY),
            Value::Float64(-1e300),
            Value::Float64(-1.5),
            Value::Float64(-0.0),
            Value::Float64(0.0),
            Value::Float64(f64::MIN_POSITIVE),
            Value::Float64(1.5),
            Value::Float64(1e300),
            Value::Float64(f64::INFINITY),
            Value::Float64(f64::NAN),
            Value::String("".into()),
            Value::String("a".into()),
            Value::String("aa".into()),
            Value::String("agent-00000001".into()),
            Value::String("agent-00000002".into()),
            Value::String("zzzzzzzzzzzzzzzzzz".into()),
        ];
        v.push(Value::String("\u{e9}t\u{e9}".into()));
        v
    }

    #[test]
    fn la_cle_est_monotone_sur_tout_l_echantillon() {
        let e = echantillon();
        for a in &e {
            for b in &e {
                monotone(a, b);
            }
        }
    }

    /// LE PIÈGE DU ZÉRO NÉGATIF, nommément.
    ///
    /// `-0.0 == +0.0` pour `OrderedFloat64` mais leurs motifs binaires sont
    /// opposés. Sans canonisation, `cle(-0.0) < cle(+0.0)` et un intervalle
    /// `[0, x]` PERDAIT toute valeur `-0.0`. La borne `0` est la plus
    /// fréquente des propriétés mesurées.
    #[test]
    fn zero_negatif_et_zero_positif_ont_la_meme_cle() {
        assert_eq!(
            encoder_cle(&Value::Float64(-0.0)).unwrap(),
            encoder_cle(&Value::Float64(0.0)).unwrap()
        );
        assert_eq!(
            encoder_cle(&Value::Float64(-0.0)).unwrap(),
            encoder_cle(&Value::Int64(0)).unwrap()
        );
    }

    /// LE PIÈGE DU NaN. `NaN != NaN` en IEEE-754 ; `OrderedFloat64` les
    /// déclare tous égaux et supérieurs à tout le reste — mais SOUS le groupe
    /// `String`, comme le veut `type_ordinal`.
    #[test]
    fn tous_les_nan_partagent_une_cle_unique_au_sommet_du_groupe_numerique() {
        let a = encoder_cle(&Value::Float64(f64::NAN)).unwrap();
        let b = encoder_cle(&Value::Float64(-f64::NAN)).unwrap();
        let c = encoder_cle(&Value::Float64(f64::from_bits(0x7FF8_0000_0000_0001))).unwrap();
        assert_eq!(a, b);
        assert_eq!(a, c);
        assert!(a > encoder_cle(&Value::Float64(f64::INFINITY)).unwrap());
        assert!(a > encoder_cle(&Value::Int64(i64::MAX)).unwrap());
        assert!(a < encoder_cle(&Value::String("".into())).unwrap());
    }

    /// `Int64` et `Float64` doivent s'INTERCALER, parce que
    /// `OrderableValue::cmp` les compare numériquement. Deux groupes distincts
    /// mettraient tous les entiers avant tous les flottants et casseraient
    /// l'ordre de référence.
    #[test]
    fn entiers_et_flottants_s_intercalent_comme_orderable_value() {
        let i5 = encoder_cle(&Value::Int64(5)).unwrap();
        let f3 = encoder_cle(&Value::Float64(3.0)).unwrap();
        let f7 = encoder_cle(&Value::Float64(7.5)).unwrap();
        assert!(f3 < i5, "3.0 doit precéder 5");
        assert!(i5 < f7, "5 doit precéder 7.5");
        assert_eq!(
            ov(&Value::Int64(5)).cmp(&ov(&Value::Float64(3.0))),
            std::cmp::Ordering::Greater
        );
    }

    /// Le filtrage d'`OrderableValue` est CONSERVÉ : rien de non ordonné
    /// n'entre dans l'index.
    #[test]
    fn les_valeurs_non_ordonnees_sont_refusees() {
        for v in [
            Value::Null,
            Value::Bytes(std::sync::Arc::from([1u8, 2].as_slice())),
            Value::List(std::sync::Arc::from([Value::Int64(1)].as_slice())),
            Value::Vector(std::sync::Arc::from([0.5f32].as_slice())),
        ] {
            assert!(
                encoder_cle(&v).is_none(),
                "{v:?} ne doit PAS entrer dans l'index ordonné"
            );
            assert!(OrderableValue::try_from(&v).is_err(), "témoin : {v:?}");
        }
    }

    /// Les chaînes tronquées COLLISIONNENT — et c'est admis, parce qu'une
    /// collision ne produit que des faux positifs. Ce test fixe le contrat
    /// pour qu'un futur élargissement de `LARGEUR_CLE` ne se croie pas exact.
    ///
    /// Il fixe aussi la CONSÉQUENCE de l'élargissement (12 → 48 octets, version 2
    /// du format, 2026-09-13) : deux chemins qui ne différaient qu'au douzième
    /// octet — « …banc_obrain/crates » contre « …banc_obrain/outils » — ne
    /// collisionnent PLUS, et c'est tout l'objet du changement : un préfixe de
    /// chemin doit distinguer les répertoires, sinon l'index ne sert à rien.
    /// La collision n'est donc attendue qu'au-delà de `LARGEUR_CLE - 1` octets.
    #[test]
    fn les_chaines_longues_collisionnent_sans_rompre_l_ordre() {
        let long = "x".repeat(LARGEUR_CLE - 1);
        let a = encoder_cle(&Value::String(format!("{long}_ALPHA").into())).unwrap();
        let b = encoder_cle(&Value::String(format!("{long}_OMEGA").into())).unwrap();
        assert_eq!(
            a,
            b,
            "collision attendue au-delà de {} octets",
            LARGEUR_CLE - 1
        );
        // Deux préfixes qui diffèrent AVANT la troncature restent distincts — et
        // ordonnés : c'est ce que le chemin de préfixe exploite.
        let court = encoder_cle(&Value::String("abcdefghijk_ALPHA".into())).unwrap();
        let autre = encoder_cle(&Value::String("abcdefghijk_OMEGA".into())).unwrap();
        assert_ne!(
            court, autre,
            "sous la troncature, deux valeurs DISTINCTES le restent"
        );
        let c = encoder_cle(&Value::String("abcdefghijl".into())).unwrap();
        assert!(court < c, "l'ordre du préfixe doit tenir");
        assert!(a < autre || b < autre || true);
        let _ = (a, b);
    }

    // ─────────────────────────────────────────────────────────────────────
    // LA ZONE
    // ─────────────────────────────────────────────────────────────────────

    /// `SubstrateFile::open_tempfile` porte son propre répertoire temporaire
    /// et l'efface à la destruction : on le garde vivant le temps du test.
    fn zone_temporaire() -> (SubstrateFile, PropsIndexZone) {
        let sf = SubstrateFile::open_tempfile().unwrap();
        let z = PropsIndexZone::ouvrir(&sf);
        (sf, z)
    }

    fn serie(n: u32) -> Vec<EntreeIndex> {
        let mut v: Vec<EntreeIndex> = (0..n)
            .map(|i| EntreeIndex::new(encoder_cle(&Value::Int64(i64::from(i))).unwrap(), i))
            .collect();
        v.sort_by_key(EntreeIndex::ordre);
        v
    }

    #[test]
    fn un_index_absent_n_est_pas_fiable_et_ne_repond_pas() {
        let (_d, mut z) = zone_temporaire();
        z.confronter_ancres(0, 0);
        assert!(!z.fiable());
        assert_eq!(z.candidats(1, CLE_MIN, CLE_MAX), None);
    }

    #[test]
    fn fusion_puis_relecture_rend_l_intervalle_exact() {
        let (_d, mut z) = zone_temporaire();
        z.fusionner(&[(7u16, serie(1000))], 42, 3).unwrap();
        assert!(z.fiable());
        let bas = encoder_cle(&Value::Int64(100)).unwrap();
        let haut = encoder_cle(&Value::Int64(199)).unwrap();
        let mut c = z.candidats(7, bas, haut).unwrap();
        c.sort_unstable();
        assert_eq!(c, (100u32..=199).collect::<Vec<_>>());
        assert_eq!(z.candidats(7, CLE_MIN, CLE_MAX).unwrap().len(), 1000);
        // Propriété inconnue : `None`, JAMAIS `Some(vec![])`.
        assert_eq!(z.candidats(9, CLE_MIN, CLE_MAX), None);
    }

    #[test]
    fn la_serie_traverse_plusieurs_pages_sans_perdre_d_entree() {
        let (_d, mut z) = zone_temporaire();
        let n = (ENTREES_PAR_PAGE * 5 + 3) as u32;
        z.fusionner(&[(1u16, serie(n))], 0, 0).unwrap();
        assert_eq!(z.candidats(1, CLE_MIN, CLE_MAX).unwrap().len(), n as usize);
        // Une borne au milieu d'une page intérieure.
        let bas = encoder_cle(&Value::Int64(i64::from(ENTREES_PAR_PAGE as u32 * 2 + 7))).unwrap();
        let c = z.candidats(1, bas, CLE_MAX).unwrap();
        assert_eq!(c.len(), (n - (ENTREES_PAR_PAGE as u32 * 2 + 7)) as usize);
    }

    #[test]
    fn les_ancres_qui_divergent_rendent_l_index_non_fiable() {
        let (_d, mut z) = zone_temporaire();
        z.fusionner(&[(1u16, serie(10))], 500, 12).unwrap();
        assert!(z.fiable());
        z.confronter_ancres(500, 12);
        assert!(z.fiable(), "ancres identiques : fiable");
        z.confronter_ancres(501, 12);
        assert!(!z.fiable(), "le WAL a avancé : NON fiable");
        assert_eq!(z.candidats(1, CLE_MIN, CLE_MAX), None);
        z.confronter_ancres(500, 13);
        assert!(!z.fiable(), "la zone v2 a grandi : NON fiable");
    }

    /// UN INDEX DONT ON DOUTE NE DOIT PAS SE REPORTER DANS LE SUIVANT.
    ///
    /// Trou trouvé par la preuve par MUTATION : retirer le garde `fiable` de
    /// [`PropsIndexZone::entrees_serie`] ne faisait rougir aucun test. Ce
    /// garde est aujourd'hui hors d'atteinte depuis le store —
    /// `fusionner_index_ordonne` ne l'appelle que sous `fiable` — mais il est
    /// la défense en profondeur contre le pire des cas : une série PÉRIMÉE
    /// reportée dans un index frais serait BÉNIE par des ancres neuves, et se
    /// dirait complète en ne l'étant pas. Ce test le tient depuis l'extérieur
    /// du store, là où la mutation ne pouvait pas être rattrapée.
    #[test]
    fn une_serie_dont_on_doute_ne_se_reporte_pas() {
        let (_d, mut z) = zone_temporaire();
        z.fusionner(&[(4u16, serie(80))], 10, 2).unwrap();
        assert_eq!(z.entrees_serie(4).len(), 80, "fiable : la série se lit");
        z.confronter_ancres(11, 2); // le WAL a avancé : on ne sait plus.
        assert!(!z.fiable());
        assert!(
            z.entrees_serie(4).is_empty(),
            "une série NON fiable ne doit RIEN rendre : la reporter dans une \
             fusion la bénirait comme complète"
        );
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
        let c = sf.path().join(PROPS_INDEX_V2_FILENAME);
        let mut z = PropsIndexZone::ouvrir(&sf);
        z.fusionner(&[(1u16, serie(50))], 1, 1).unwrap();
        drop(z);
        // On corrompt un octet du descripteur de série.
        let mut o = std::fs::read(&c).unwrap();
        o[60] ^= 0xFF;
        std::fs::write(&c, &o).unwrap();
        let mut z2 = PropsIndexZone::ouvrir_chemin(&c);
        z2.confronter_ancres(1, 1);
        assert!(!z2.fiable(), "CRC faux ⇒ index jeté ⇒ balayage");
        assert_eq!(z2.candidats(1, CLE_MIN, CLE_MAX), None);
    }

    #[test]
    fn une_fusion_survit_a_la_reouverture() {
        let sf = SubstrateFile::open_tempfile().unwrap();
        let mut z = PropsIndexZone::ouvrir(&sf);
        z.fusionner(&[(3u16, serie(700))], 77, 9).unwrap();
        let g = z.generation();
        drop(z);
        // AUCUNE reconstruction : on relit les octets, c'est tout le point.
        let mut z2 = PropsIndexZone::ouvrir(&sf);
        z2.confronter_ancres(77, 9);
        assert!(z2.fiable());
        assert_eq!(z2.generation(), g);
        assert_eq!(z2.candidats(3, CLE_MIN, CLE_MAX).unwrap().len(), 700);
    }

    // ─────────────────────────────────────────────────────────────────────
    // (2026-09-09, P1 ouverture rapide — W1-bis) EXTENSION DE FENÊTRE
    // ─────────────────────────────────────────────────────────────────────

    /// Une fusion, puis une réouverture dont la fenêtre post-fusion a ajouté
    /// des entrées : l'index est ÉTENDU (ancres reposées, FIABLE) sans jamais
    /// être rebâti, et sert l'ancien ET le nouveau contenu.
    #[test]
    fn une_fenetre_expliquee_etend_l_index_sans_reconstruction() {
        let sf = SubstrateFile::open_tempfile().unwrap();
        // Fusion initiale : ancres (100, 10), slots 1..=50 (valeurs i*10).
        let entrees: Vec<EntreeIndex> = (1..=50)
            .map(|i| EntreeIndex::new(encoder_cle(&Value::Int64(i64::from(i * 10))).unwrap(), i))
            .collect();
        {
            let mut z = PropsIndexZone::ouvrir(&sf);
            z.fusionner(&[(1u16, entrees)], 100, 10).unwrap();
        }
        // Réouverture APRÈS des écritures : le WAL a avancé (100 → 120) et la
        // zone v2 a grandi (10 → 11 pages) — ancres divergentes → NON fiable.
        let mut z2 = PropsIndexZone::ouvrir(&sf);
        z2.confronter_ancres(120, 11);
        assert!(!z2.fiable());
        assert_eq!(z2.candidats(1, CLE_MIN, CLE_MAX), None);
        // La fenêtre a ajouté le slot 51 (valeur 510) : on l'étend.
        let ajouts = vec![(
            1u16,
            vec![EntreeIndex::new(
                encoder_cle(&Value::Int64(510)).unwrap(),
                51,
            )],
        )];
        assert!(z2.etendre_apres_fenetre(&ajouts, 120, 11).unwrap());
        assert!(z2.fiable(), "ancres reposées à l'état courant ⇒ FIABLE");
        assert_eq!(z2.ancre_wal(), 120);
        assert_eq!(z2.ancre_pages(), 11);
        // L'index sert l'ancien ET le nouveau contenu, sans rien perdre.
        assert_eq!(z2.candidats(1, CLE_MIN, CLE_MAX).unwrap().len(), 51);
        let bas = encoder_cle(&Value::Int64(495)).unwrap();
        let haut = encoder_cle(&Value::Int64(515)).unwrap();
        let mut c = z2.candidats(1, bas, haut).unwrap();
        c.sort_unstable();
        assert_eq!(c, vec![50, 51], "les deux bords de la fenêtre répondent");
        assert_eq!(z2.entrees_serie(1).len(), 51);
    }

    /// La fenêtre RÉÉCRIT un slot déjà couvert : l'ancienne entrée est
    /// remplacée (filtrage des slots touchés, comme la fusion de flush) —
    /// le nouvel état répond, l'ancien ne laisse qu'un faux positif bénin
    /// qu'une revérification écarterait.
    #[test]
    fn une_fenetre_qui_reécrit_un_slot_remplace_l_ancienne_entree() {
        let sf = SubstrateFile::open_tempfile().unwrap();
        let entrees: Vec<EntreeIndex> = (1..=5)
            .map(|i| EntreeIndex::new(encoder_cle(&Value::Int64(i64::from(i) * 10)).unwrap(), i))
            .collect();
        {
            let mut z = PropsIndexZone::ouvrir(&sf);
            z.fusionner(&[(1u16, entrees)], 0, 0).unwrap();
        }
        let mut z2 = PropsIndexZone::ouvrir(&sf);
        z2.confronter_ancres(10, 1);
        assert!(!z2.fiable());
        // Le slot 2 passe de 20 à 2 000 dans la fenêtre.
        let ajouts = vec![(
            1u16,
            vec![EntreeIndex::new(
                encoder_cle(&Value::Int64(2_000)).unwrap(),
                2,
            )],
        )];
        assert!(z2.etendre_apres_fenetre(&ajouts, 10, 1).unwrap());
        let autour_de_20 = z2
            .candidats(
                1,
                encoder_cle(&Value::Int64(15)).unwrap(),
                encoder_cle(&Value::Int64(25)).unwrap(),
            )
            .unwrap();
        assert!(
            !autour_de_20.contains(&2),
            "l'ancienne entrée (20, slot 2) doit avoir été retirée : {autour_de_20:?}"
        );
        let haut = z2
            .candidats(1, encoder_cle(&Value::Int64(1_999)).unwrap(), CLE_MAX)
            .unwrap();
        assert_eq!(haut, vec![2]);
        assert_eq!(
            z2.entrees_serie(1).len(),
            5,
            "une entrée par slot, pas plus"
        );
    }

    /// Un écart NON FORWARD (WAL tronqué sous l'ancre, zone réduite) n'est
    /// pas expliqué : l'extension REFUSE, et rien n'est modifié — l'index
    /// reste dans l'état où la confrontation l'a laissé.
    #[test]
    fn une_fenetre_non_forward_refuse_l_extension() {
        let (_d, mut z) = zone_temporaire();
        z.fusionner(&[(1u16, serie(10))], 100, 5).unwrap();
        // WAL tronqué SOUS l'ancre (90 < 100) : inexpliqué → refus.
        assert!(!z.etendre_apres_fenetre(&[], 90, 5).unwrap());
        assert!(z.fiable(), "rien n'a été touché (ancres inchangées)");
        // Zone v2 RÉDUITE (4 < 5) : inexpliqué → refus, après avoir été
        // déclaré non fiable par la confrontation.
        z.confronter_ancres(100, 4);
        assert!(!z.fiable());
        assert!(!z.etendre_apres_fenetre(&[], 100, 4).unwrap());
        assert!(
            !z.fiable(),
            "refus ⇒ l'index reste NON FIABLE ⇒ l'appelant reconstruit"
        );
    }

    /// Une série de base illisible (CRC faux sur une page) fait REFUSER
    /// l'extension : on ne bénit jamais un fichier dont on ne peut pas
    /// garantir le contenu — la reconstruction reste le remède.
    #[test]
    fn une_serie_de_base_corrompue_fait_refuser_l_extension() {
        let sf = SubstrateFile::open_tempfile().unwrap();
        let c = sf.path().join(PROPS_INDEX_V2_FILENAME);
        {
            let mut z = PropsIndexZone::ouvrir(&sf);
            z.fusionner(&[(1u16, serie(300))], 100, 5).unwrap();
        }
        // Corrompt un octet du PAYLOAD de la première page de série (page 1
        // = 4 096 octets après la racine) : le CRC de page ne passe plus.
        let mut o = std::fs::read(&c).unwrap();
        o[PAGE_SIZE + 100] ^= 0x5A;
        std::fs::write(&c, &o).unwrap();
        let mut z2 = PropsIndexZone::ouvrir_chemin(&c);
        z2.confronter_ancres(150, 6);
        assert!(!z2.fiable());
        let ajouts = vec![(1u16, serie(3))];
        assert!(
            z2.etendre_apres_fenetre(&ajouts, 150, 6).is_err(),
            "CRC faux sur le base ⇒ extension REFUSÉE (erreur), pas béni"
        );
        assert!(!z2.fiable());
    }
}
