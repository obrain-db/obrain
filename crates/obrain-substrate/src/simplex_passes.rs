//! Passes de MATÉRIALISATION simpliciales (T5, plan « Topologie simpliciale
//! native », spec 271180c3 §4-§5) — payer à la publication, pas à la requête.
//!
//! Construit sur le stockage T4 ([`crate::simplex_zone`],
//! [`crate::simplex_incidence`]) : ce module DÉRIVE les simplexes du graphe
//! binaire, il n'invente aucun stockage nouveau.
//!
//! ## Les trois passes (portage cbm, fichiers:lignes de référence)
//!
//! * **Triangles** (store.c:4893-5160) — closed-wedge canonique `a<b<c`
//!   strict : les arêtes DIRIGÉES `a→b`, `a→c`, `b→c` existent toutes,
//!   sommets pairwise distincts. Node-iterator sur adjacence triée,
//!   intersection two-pointer `out(u)∩out(v)` restreinte à `w>v` — émission
//!   lexicographique, byte-identique pour graphes identiques. Le bit
//!   [`crate::record::simplex_flags::CYCLIC`] est posé si l'arête de fermeture `c→a`
//!   existe (orientation façon aletheia-n3, le « future link » de
//!   store.c:5304-5312 — un seek d'arête à la matérialisation, cbm ne
//!   stocke pas cette distinction).
//! * **Cliques K4/K5** (store.c:5315-5800) — extension du node-iterator :
//!   un K4 `(a,b,c,d)` est un triangle `(a,b,c)` étendu par un
//!   out-voisin commun `d > c` de `a`, `b` et `c` ; un K5 étend un K4.
//!   TOUTES les cliques, jamais seulement les maximales.
//! * **Hyperarêtes DISPATCH** (pass_hyperedges.c:1-317) — groupage des
//!   arêtes d'appel par `(source, ligne)` : un site d'appel qui résout
//!   vers ≥ 2 appelés distincts émet UNE hyperarête, membres
//!   `{appelant} ∪ {appelés}` (union ENSEMBLISTE — un auto-appel ne
//!   duplique pas l'appelant, I1 interdit les doublons), bornes I3
//!   3..=64 membres (groupe hors borne jeté ENTIER). La clé de groupe est
//!   PARAMÉTRABLE : type d'arête + propriété d'arête portant la ligne
//!   (chez cbm : `CALLS` + `line`).
//!
//! ## Sémantique de re-matérialisation (contrat cbm « replaces, never
//! duplicates », store.c:4932-4937)
//!
//! [`crate::store::SubstrateStore::materialiser_simplexes`] est IDEMPOTENTE et
//! RÉPARATRICE : l'ensemble attendu est dérivé du graphe, les simplexes
//! déjà conformes sont des no-ops sans WAL (I2), les manquants sont créés,
//! les périmés (un graphe rétréci) sont tombstonés. Deux bases construites
//! par la même histoire produisent des zones byte-identiques après
//! [`crate::store::SubstrateStore::compact_simplexes`] (I4). Le rebuild complet est une
//! commande OFFLINE (réparation) — jamais sur le chemin du commit (§4).
//!
//! ## Configuration persistante
//!
//! La configuration des passes (types d'arêtes/propriété de groupe, drapeau
//! de maintenance incrémentale) vit dans les octets réservés 16..26 de
//! l'en-tête de la zone `SimplexDir`, journalisée WAL-first
//! ([`crate::wal::WalPayload::SimplexPassesConfig`]) pour que le rejeu
//! depuis le WAL seul la restaure (invariant I7) — sans elle, la
//! maintenance au commit ne saurait pas quoi maintenir après réouverture.

use crate::error::{SubstrateError, SubstrateResult};
use crate::file::ZoneFile;
use crate::simplex_zone::SimplexId;

/// Offset de la configuration dans l'en-tête SPXD (octets réservés,
/// zéro-initialisés par `write_header` — 0 = « pas de config », d'où le
/// numéro de version qui commence à 1).
pub(crate) const CONFIG_OFFSET: usize = 16;
/// Taille encodée de la configuration (octets).
pub(crate) const CONFIG_LEN: usize = 18;
/// Version courante de l'encodage.
pub const SIMPLEX_CONFIG_VERSION: u8 = 1;
/// Sentinelle « pas de volet DISPATCH » pour les trois u16 du volet.
pub const DISPATCH_ABSENT: u16 = u16::MAX;

/// Orientation d'une face (triangle) — le modèle aletheia-n3 porté par le
/// bit [`crate::record::simplex_flags::CYCLIC`] : un closed-wedge dont
/// l'arête de fermeture `c→a` existe est un 3-CYCLE DIRIGÉ, donc une face
/// ORIENTÉE de signe +1 dans l'ordre canonique `(a,b,c)` (les permutations
/// paires portent le même signe, les impaires le signe opposé) ; sans
/// fermeture, la face est non orientée. cbm ne stocke pas cette
/// distinction — c'est l'ajout de la spec §1, un seek d'arête payé à la
/// matérialisation, jamais à la requête.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrientationFace {
    /// Closed-wedge acyclique — pas d'orientation induite.
    NonOrientee,
    /// 3-cycle dirigé — orientation +1 dans l'ordre canonique.
    Cyclique,
}

/// Drapeaux de [`SimplexPassesConfig`].
pub mod passes_flags {
    /// La maintenance incrémentale au commit est active : les mutations
    /// d'arêtes recalculent LOCALEMENT les simplexes touchés (spec §4).
    pub const MAINTENANCE: u8 = 1 << 0;
}

/// Volet hyperarêtes DISPATCH de la configuration — tous les identifiants
/// sont des ids de dictionnaire ([`crate::dict`]) : type d'arête source,
/// clé de propriété portant la ligne d'appel (la clé de groupe), et id de
/// TYPE de l'hyperarête émise (interné dans le registre des clés de
/// propriétés — le « dict existant » de la spec §1, décision T5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DispatchConfig {
    /// Type d'arête groupée (cbm : `CALLS`).
    pub edge_type: u16,
    /// Clé de propriété d'arête servant de clé de groupe (cbm : `line`).
    pub group_prop_key: u16,
    /// `type_id` des hyperarêtes émises (cbm : `DISPATCH`).
    pub he_type: u16,
}

/// Configuration persistante des passes simpliciales.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimplexPassesConfig {
    /// Voir [`passes_flags`].
    pub flags: u8,
    /// Type d'arête des triangles/cliques (cbm : `CALLS`).
    pub clique_edge_type: u16,
    /// Masque de labels des EXTRÉMITÉS des arêtes de triangles/cliques :
    /// un nœud participe si `label_bitset & masque != 0` ; `0` = pas de
    /// filtre. Le portage cbm filtre sur Function|Method — le graphe
    /// d'appel que fetch_call_edges condense (store.c:2528-2539).
    pub clique_label_mask: u64,
    /// Volet hyperarêtes — `None` = passe DISPATCH désactivée.
    pub dispatch: Option<DispatchConfig>,
}

impl SimplexPassesConfig {
    /// Maintenance incrémentale au commit active ?
    pub fn maintenance_active(&self) -> bool {
        self.flags & passes_flags::MAINTENANCE != 0
    }

    /// Encode vers les octets d'en-tête (version, flags, 4×u16, u64 LE).
    pub(crate) fn encode(&self) -> [u8; CONFIG_LEN] {
        let (de, dp, dh) = match self.dispatch {
            Some(d) => (d.edge_type, d.group_prop_key, d.he_type),
            None => (DISPATCH_ABSENT, DISPATCH_ABSENT, DISPATCH_ABSENT),
        };
        let mut out = [0u8; CONFIG_LEN];
        out[0] = SIMPLEX_CONFIG_VERSION;
        out[1] = self.flags;
        out[2..4].copy_from_slice(&self.clique_edge_type.to_le_bytes());
        out[4..6].copy_from_slice(&de.to_le_bytes());
        out[6..8].copy_from_slice(&dp.to_le_bytes());
        out[8..10].copy_from_slice(&dh.to_le_bytes());
        out[10..18].copy_from_slice(&self.clique_label_mask.to_le_bytes());
        out
    }

    /// Décode depuis les octets d'en-tête. `None` si aucune config n'a
    /// jamais été écrite (version 0 — l'en-tête est né zéro-initialisé).
    pub(crate) fn decode(bytes: &[u8]) -> SubstrateResult<Option<Self>> {
        match bytes[0] {
            0 => Ok(None),
            SIMPLEX_CONFIG_VERSION => {
                let u16_at = |i: usize| u16::from_le_bytes([bytes[i], bytes[i + 1]]);
                let de = u16_at(4);
                let dispatch = if de == DISPATCH_ABSENT {
                    None
                } else {
                    Some(DispatchConfig {
                        edge_type: de,
                        group_prop_key: u16_at(6),
                        he_type: u16_at(8),
                    })
                };
                Ok(Some(Self {
                    flags: bytes[1],
                    clique_edge_type: u16_at(2),
                    clique_label_mask: u64::from_le_bytes(bytes[10..18].try_into().unwrap()),
                    dispatch,
                }))
            }
            v => Err(SubstrateError::WalBadFrame(format!(
                "config des passes simpliciales : version {v} inconnue (max \
                 {SIMPLEX_CONFIG_VERSION}) — base écrite par un binaire plus récent ?"
            ))),
        }
    }
}

/// Écrit la config dans l'en-tête SPXD — chemin RAW sans WAL, appelé par
/// le [`crate::writer::Writer`] (WAL-first) et par le rejeu.
pub(crate) fn write_config_raw(dir: &mut ZoneFile, cfg: &SimplexPassesConfig) {
    let bytes = cfg.encode();
    dir.as_slice_mut()[CONFIG_OFFSET..CONFIG_OFFSET + CONFIG_LEN].copy_from_slice(&bytes);
}

/// Lit la config depuis l'en-tête SPXD (zone déjà validée par l'appelant).
pub(crate) fn read_config_raw(dir: &ZoneFile) -> SubstrateResult<Option<SimplexPassesConfig>> {
    SimplexPassesConfig::decode(&dir.as_slice()[CONFIG_OFFSET..CONFIG_OFFSET + CONFIG_LEN])
}

// ---------------------------------------------------------------------------
// Spécification par noms (surface publique — les ids sont internés au moment
// de la configuration)
// ---------------------------------------------------------------------------

/// Volet DISPATCH de [`SimplexPassesSpec`], par noms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchSpec {
    /// Type d'arête groupée (cbm : `"CALLS"`).
    pub edge_type: String,
    /// Propriété d'arête portant la clé de groupe (cbm : `"line"`).
    pub group_prop_key: String,
    /// Nom du type d'hyperarête émise (cbm : `"DISPATCH"`).
    pub he_type: String,
}

/// Spécification des passes par NOMS — résolue en
/// [`SimplexPassesConfig`] (ids de dict) par
/// [`crate::store::SubstrateStore::configurer_passes_simpliciales`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimplexPassesSpec {
    /// Type d'arête des triangles/cliques (cbm : `"CALLS"`).
    pub clique_edge_type: String,
    /// Labels admis aux extrémités des arêtes de triangles/cliques
    /// (l'UNION des bits) ; vide = pas de filtre. Portage cbm :
    /// `["Function", "Method"]`.
    pub clique_labels: Vec<String>,
    /// Volet hyperarêtes — `None` = pas de passe DISPATCH.
    pub dispatch: Option<DispatchSpec>,
    /// Activer la maintenance incrémentale au commit (spec §4).
    pub maintenance: bool,
}

impl SimplexPassesSpec {
    /// La configuration du portage cbm : triangles/cliques sur `CALLS`,
    /// hyperarêtes DISPATCH groupées par (`source`, `line`), maintenance
    /// incrémentale active.
    pub fn portage_cbm() -> Self {
        Self {
            clique_edge_type: "CALLS".into(),
            clique_labels: vec!["Function".into(), "Method".into()],
            dispatch: Some(DispatchSpec {
                edge_type: "CALLS".into(),
                group_prop_key: "line".into(),
                he_type: "DISPATCH".into(),
            }),
            maintenance: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Statistiques de matérialisation
// ---------------------------------------------------------------------------

/// Bilan d'une passe complète ([`crate::store::SubstrateStore::materialiser_simplexes`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MaterialiseStats {
    /// Triangles vivants attendus après la passe.
    pub triangles: u64,
    /// Dont porteurs du bit CYCLIC (3-cycle dirigé).
    pub triangles_cycliques: u64,
    /// Cliques K4 vivantes attendues.
    pub k4: u64,
    /// Cliques K5 vivantes attendues.
    pub k5: u64,
    /// Hyperarêtes DISPATCH vivantes attendues.
    pub hyperedges: u64,
    /// Somme des cardinaux des hyperarêtes.
    pub membres_hyperedges: u64,
    /// Groupes DISPATCH jetés ENTIERS pour bornes I3 (jamais tronqués).
    pub groupes_hors_bornes: u64,
    /// Simplexes créés par cette passe.
    pub crees: u64,
    /// Simplexes réécrits sous le même id (flags/props changés).
    pub reecrits: u64,
    /// Simplexes déjà conformes (no-op sans WAL).
    pub inchanges: u64,
    /// Simplexes périmés tombstonés (graphe rétréci).
    pub tombstones: u64,
}

// ---------------------------------------------------------------------------
// Dérivation pure : triangles (et, par extension, cliques) depuis une liste
// d'arêtes dirigées — cœur algorithmique séparé du store pour être testable
// à sec et réutilisable par le chemin incrémental.
// ---------------------------------------------------------------------------

/// Adjacence dirigée triée sur l'espace DENSE des slots (les slots de
/// nœuds sont l'identité des sommets — spec §1, 0-simplexes).
pub(crate) struct AdjacenceTriee {
    /// `head[s]..head[s+1]` = tranche des cibles de `s` dans `cibles`.
    head: Vec<u32>,
    cibles: Vec<u32>,
}

impl AdjacenceTriee {
    /// Construit depuis des paires `(src, dst)` DÉDUPLIQUÉES ou non — les
    /// doublons (multi-arêtes du même type) sont fusionnés : la sémantique
    /// closed-wedge est sur le graphe SIMPLE sous-jacent (cbm n'a pas de
    /// multi-arêtes CALLS : UNIQUE(source,target,type)).
    pub(crate) fn construire(paires: &mut Vec<(u32, u32)>) -> Self {
        paires.sort_unstable();
        paires.dedup();
        let n = paires
            .iter()
            .map(|&(s, d)| s.max(d))
            .max()
            .map_or(0, |m| m as usize + 1);
        let mut head = vec![0u32; n + 1];
        for &(s, _) in paires.iter() {
            head[s as usize + 1] += 1;
        }
        for i in 0..n {
            head[i + 1] += head[i];
        }
        let cibles = paires.iter().map(|&(_, d)| d).collect();
        Self { head, cibles }
    }

    /// Cibles sortantes triées croissantes de `s` (vide hors bornes).
    pub(crate) fn sortants(&self, s: u32) -> &[u32] {
        let s = s as usize;
        if s + 1 >= self.head.len() {
            return &[];
        }
        &self.cibles[self.head[s] as usize..self.head[s + 1] as usize]
    }

    /// L'arête dirigée `s → d` existe-t-elle ? (dichotomie sur la tranche
    /// triée — le seek d'orientation CYCLIC.)
    pub(crate) fn contient(&self, s: u32, d: u32) -> bool {
        self.sortants(s).binary_search(&d).is_ok()
    }

    /// Nombre de sommets adressables (= plus grand slot vu + 1).
    pub(crate) fn nb_sommets(&self) -> usize {
        self.head.len().saturating_sub(1)
    }
}

/// Énumère les triangles closed-wedge en ordre lexicographique strict
/// `(a,b,c)`, `a<b<c` — le node-iterator de cbm (store.c:4947-5076) :
/// pour chaque `u` croissant, pour chaque `v ∈ out(u)` avec `v>u`,
/// intersection two-pointer `out(u)∩out(v)` restreinte à `w>v`. Chaque
/// triangle est visité EXACTEMENT une fois. `f(a, b, c, cyclique)` —
/// `cyclique` = l'arête de fermeture `c→a` existe (un seek).
pub(crate) fn enumerer_triangles(adj: &AdjacenceTriee, mut f: impl FnMut(u32, u32, u32, bool)) {
    for u in 0..adj.nb_sommets() as u32 {
        let out_u = adj.sortants(u);
        for &v in out_u {
            if v <= u {
                continue;
            }
            let out_v = adj.sortants(v);
            let (mut qa, mut qb) = (0usize, 0usize);
            while qa < out_u.len() && qb < out_v.len() {
                let (wa, wb) = (out_u[qa], out_v[qb]);
                if wa < wb {
                    qa += 1;
                } else if wa > wb {
                    qb += 1;
                } else {
                    if wa > v {
                        f(u, v, wa, adj.contient(wa, u));
                    }
                    qa += 1;
                    qb += 1;
                }
            }
        }
    }
}

/// Intersection two-pointer de deux tranches TRIÉES, restreinte aux
/// éléments strictement supérieurs à `seuil`.
pub(crate) fn intersecter_bornee(a: &[u32], b: &[u32], seuil: u32, out: &mut Vec<u32>) {
    out.clear();
    let (mut qa, mut qb) = (0usize, 0usize);
    while qa < a.len() && qb < b.len() {
        let (xa, xb) = (a[qa], b[qb]);
        if xa < xb {
            qa += 1;
        } else if xa > xb {
            qb += 1;
        } else {
            if xa > seuil {
                out.push(xa);
            }
            qa += 1;
            qb += 1;
        }
    }
}

/// Énumère les cliques K4 puis K5 par EXTENSION des triangles — le
/// node-iterator localisé de cbm (store.c:5315-5800) : un K4 `(a,b,c,d)`
/// est un triangle `(a,b,c)` étendu par un out-voisin commun `d > c` de
/// `a`, `b` et `c` (les arêtes dirigées `v_i→v_j` existent alors pour
/// tout `i<j`) ; un K5 étend un K4 par `e > d` commun aux quatre.
/// Génération UNIQUE : chaque K4 n'est produit que depuis son triangle
/// préfixe `(v0,v1,v2)`, chaque K5 que depuis son K4 préfixe — et
/// l'itération des triangles en ordre lexicographique avec `d` (resp.
/// `e`) croissant émet les K4 puis les K5 en ordre lexicographique (I4).
/// TOUTES les cliques sont émises, jamais seulement les maximales : un K5
/// vient avec ses 5 sous-K4 (choix documenté de cbm).
pub(crate) fn enumerer_cliques(
    adj: &AdjacenceTriee,
    triangles: &[[u32; 3]],
) -> (Vec<[u32; 4]>, Vec<[u32; 5]>) {
    let mut k4s = Vec::new();
    let mut inter3 = Vec::new();
    let mut tampon = Vec::new();
    for &[a, b, c] in triangles {
        // out(a)∩out(b) (>c) puis ∩ out(c) — les trois arêtes manquantes
        // a→d, b→d, c→d du K4 candidat.
        intersecter_bornee(adj.sortants(a), adj.sortants(b), c, &mut tampon);
        intersecter_bornee(&tampon, adj.sortants(c), c, &mut inter3);
        for &d in &inter3 {
            k4s.push([a, b, c, d]);
        }
    }
    let mut k5s = Vec::new();
    let mut inter4 = Vec::new();
    for &[a, b, c, d] in &k4s {
        intersecter_bornee(adj.sortants(a), adj.sortants(b), d, &mut tampon);
        intersecter_bornee(&tampon, adj.sortants(c), d, &mut inter3);
        intersecter_bornee(&inter3, adj.sortants(d), d, &mut inter4);
        for &e in &inter4 {
            k5s.push([a, b, c, d, e]);
        }
    }
    (k4s, k5s)
}

/// Regroupe les arêtes DISPATCH `(src, clé_de_groupe, dst)` — la passe
/// hyperarêtes de cbm (pass_hyperedges.c) : tri par `(source, ligne,
/// cible)` (ph_cmp_call), un groupe = un site d'appel `(source, ligne)`,
/// cibles DISTINCTES, émission en ordre canonique. Rend
/// `(source, ligne, membres)` pour chaque groupe à ≥ 2 cibles distinctes,
/// membres = UNION ENSEMBLISTE `{source} ∪ {cibles}` triée (I1 : un
/// auto-appel ne duplique pas la source — divergence documentée avec cbm
/// qui appose la source sans dédupliquer ; son schéma
/// UNIQUE(hyperedge, node) la rejetterait de toute façon).
pub(crate) fn grouper_dispatch(
    appels: &mut Vec<(u32, i64, u32)>,
    mut f: impl FnMut(u32, i64, &[u32]),
) {
    appels.sort_unstable();
    appels.dedup();
    let mut i = 0usize;
    let mut membres = Vec::new();
    while i < appels.len() {
        let (src, ligne, _) = appels[i];
        let mut fin = i;
        while fin < appels.len() && appels[fin].0 == src && appels[fin].1 == ligne {
            fin += 1;
        }
        // Cibles distinctes du site (déjà triées/dédupliquées par le tri
        // global) ; ≥ 2 = un dispatch.
        if fin - i >= 2 {
            membres.clear();
            membres.extend(appels[i..fin].iter().map(|&(_, _, d)| d));
            match membres.binary_search(&src) {
                Ok(_) => {}
                Err(pos) => membres.insert(pos, src),
            }
            f(src, ligne, &membres);
        }
        i = fin;
    }
}

/// Clé d'identité d'un simplexe pour la réconciliation : (kind, type_id,
/// membres canoniques).
pub(crate) type CleSimplexe = (u8, u16, Vec<u32>);

/// Un simplexe attendu par la dérivation (l'ensemble cible de la passe).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SimplexeAttendu {
    pub kind: u8,
    pub type_id: u16,
    pub flags: u8,
    pub membres: Vec<u32>,
    pub props: Vec<u8>,
}

impl SimplexeAttendu {
    pub(crate) fn cle(&self) -> CleSimplexe {
        (self.kind, self.type_id, self.membres.clone())
    }
}

/// Erreur enseignante commune : passes non configurées.
pub(crate) fn erreur_non_configure() -> SubstrateError {
    SubstrateError::WalBadFrame(
        "passes simpliciales non configurées : appelez \
         configurer_passes_simpliciales(spec) d'abord (la configuration est \
         persistée dans l'en-tête SimplexDir et journalisée WAL-first)"
            .into(),
    )
}

// Suite du module : les passes elles-mêmes vivent en méthodes de
// `SubstrateStore` (accès aux scans de zones et au chemin WAL-first),
// voir la section « Passes simpliciales (T5) » de `store.rs`.

/// Id d'un simplexe (ré-export de commodité pour les appelants du module).
pub type IdSimplexe = SimplexId;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_encode_decode_roundtrip() {
        let cfg = SimplexPassesConfig {
            flags: passes_flags::MAINTENANCE,
            clique_edge_type: 7,
            clique_label_mask: (1 << 3) | (1 << 9),
            dispatch: Some(DispatchConfig {
                edge_type: 7,
                group_prop_key: 3,
                he_type: 12,
            }),
        };
        let bytes = cfg.encode();
        assert_eq!(SimplexPassesConfig::decode(&bytes).unwrap(), Some(cfg));

        let sans_dispatch = SimplexPassesConfig {
            flags: 0,
            clique_edge_type: 1,
            clique_label_mask: 0,
            dispatch: None,
        };
        let bytes = sans_dispatch.encode();
        assert_eq!(
            SimplexPassesConfig::decode(&bytes).unwrap(),
            Some(sans_dispatch)
        );
    }

    #[test]
    fn config_version_zero_est_absente_et_inconnue_refusee() {
        assert_eq!(
            SimplexPassesConfig::decode(&[0u8; CONFIG_LEN]).unwrap(),
            None
        );
        let mut bytes = [0u8; CONFIG_LEN];
        bytes[0] = 250;
        assert!(SimplexPassesConfig::decode(&bytes).is_err());
    }

    #[test]
    fn cliques_k4_k5_par_extension() {
        // K5 dirigé canonique complet sur 1..=5 : toutes les arêtes
        // low→high. Attendu : C(5,3)=10 triangles, C(5,4)=5 K4, 1 K5 —
        // TOUTES les cliques, pas seulement les maximales.
        let mut paires = Vec::new();
        for a in 1u32..=5 {
            for b in (a + 1)..=5 {
                paires.push((a, b));
            }
        }
        let adj = AdjacenceTriee::construire(&mut paires);
        let mut triangles = Vec::new();
        enumerer_triangles(&adj, |a, b, c, _| triangles.push([a, b, c]));
        assert_eq!(triangles.len(), 10);
        let (k4s, k5s) = enumerer_cliques(&adj, &triangles);
        assert_eq!(k4s.len(), 5);
        assert_eq!(k5s, vec![[1, 2, 3, 4, 5]]);
        let mut k4_tries = k4s.clone();
        k4_tries.sort_unstable();
        assert_eq!(k4s, k4_tries, "émission lexicographique (I4)");

        // Une arête à REBOURS (4→2 au lieu de 2→4) ne fait pas une clique
        // dirigée : le K4 {2,3,4,5} exige 2→4.
        let mut paires2: Vec<(u32, u32)> = Vec::new();
        for a in 1u32..=5 {
            for b in (a + 1)..=5 {
                if (a, b) == (2, 4) {
                    paires2.push((4, 2));
                } else {
                    paires2.push((a, b));
                }
            }
        }
        let adj2 = AdjacenceTriee::construire(&mut paires2);
        let mut tris2 = Vec::new();
        enumerer_triangles(&adj2, |a, b, c, _| tris2.push([a, b, c]));
        let (k4s2, k5s2) = enumerer_cliques(&adj2, &tris2);
        assert!(k4s2.iter().all(|q| !(q.contains(&2) && q.contains(&4))));
        assert!(k5s2.is_empty(), "résultat attendu vide");
    }

    #[test]
    fn dispatch_groupes_et_union_ensembliste() {
        // Site (10, ligne 7) → cibles {20, 21} ; site (10, ligne 8) → une
        // seule cible (pas un dispatch) ; site (11, ligne 7) → {12, 30}
        // avec doublon d'arête ; site (40, ligne 3) → auto-appel + {41} :
        // l'union {40,41} < 3 membres, le GROUPE est quand même émis ici —
        // le seuil I3 est appliqué par l'appelant (bornes 3..=64).
        let mut appels = vec![
            (10, 7, 21),
            (10, 7, 20),
            (10, 8, 99),
            (11, 7, 30),
            (11, 7, 12),
            (11, 7, 12),
            (40, 3, 40),
            (40, 3, 41),
        ];
        let mut vus = Vec::new();
        grouper_dispatch(&mut appels, |src, ligne, membres| {
            vus.push((src, ligne, membres.to_vec()));
        });
        assert_eq!(
            vus,
            vec![
                (10, 7, vec![10, 20, 21]),
                (11, 7, vec![11, 12, 30]),
                (40, 3, vec![40, 41]), // auto-appel dédupliqué (union)
            ]
        );
    }

    #[test]
    fn triangles_closed_wedge_et_cycle() {
        // Wedge fermé acyclique : 1→2, 1→3, 2→3 (pas de 3→1).
        let mut paires = vec![(1, 2), (1, 3), (2, 3)];
        let adj = AdjacenceTriee::construire(&mut paires);
        let mut vus = Vec::new();
        enumerer_triangles(&adj, |a, b, c, cyc| vus.push((a, b, c, cyc)));
        assert_eq!(vus, vec![(1, 2, 3, false)]);

        // 3-cycle dirigé : 1→2, 2→3, 3→1 SEULES — PAS un closed-wedge
        // (il manque 1→3) : cbm ne le matérialise pas non plus.
        let mut paires = vec![(1, 2), (2, 3), (3, 1)];
        let adj = AdjacenceTriee::construire(&mut paires);
        let mut n = 0;
        enumerer_triangles(&adj, |_, _, _, _| n += 1);
        assert_eq!(n, 0);

        // Closed-wedge + fermeture : 1→2, 1→3, 2→3, 3→1 ⇒ CYCLIC.
        let mut paires = vec![(1, 2), (1, 3), (2, 3), (3, 1)];
        let adj = AdjacenceTriee::construire(&mut paires);
        let mut vus = Vec::new();
        enumerer_triangles(&adj, |a, b, c, cyc| vus.push((a, b, c, cyc)));
        assert_eq!(vus, vec![(1, 2, 3, true)]);
    }

    #[test]
    fn triangles_multi_aretes_fusionnees_et_boucles_exclues() {
        // Multi-arête 1→2 dupliquée + boucle 1→1 : le graphe SIMPLE
        // sous-jacent n'a qu'un triangle.
        let mut paires = vec![(1, 2), (1, 2), (1, 1), (1, 3), (2, 3)];
        let adj = AdjacenceTriee::construire(&mut paires);
        let mut vus = Vec::new();
        enumerer_triangles(&adj, |a, b, c, _| vus.push((a, b, c)));
        assert_eq!(vus, vec![(1, 2, 3)]);
    }

    #[test]
    fn triangles_ordre_lexicographique() {
        // Deux triangles partageant l'arête 1→2 + un triangle disjoint.
        let mut paires = vec![
            (1, 2),
            (1, 3),
            (2, 3),
            (1, 4),
            (2, 4),
            (5, 6),
            (5, 7),
            (6, 7),
        ];
        let adj = AdjacenceTriee::construire(&mut paires);
        let mut vus = Vec::new();
        enumerer_triangles(&adj, |a, b, c, _| vus.push((a, b, c)));
        assert_eq!(vus, vec![(1, 2, 3), (1, 2, 4), (5, 6, 7)]);
        let mut tries = vus.clone();
        tries.sort_unstable();
        assert_eq!(vus, tries, "émission lexicographique (I4)");
    }
}
