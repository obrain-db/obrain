//! Fixed-size `#[repr(C)]` records for the Substrate on-disk format.
//!
//! See `docs/rfc/substrate/format-spec.md` for the bit-level layout.
//!
//! Invariants enforced at compile time:
//!   - `size_of::<NodeRecord>() == 40` (jalon 2 : colonne `tenant_id` + reserve)
//!   - `size_of::<EdgeRecord>() == 36` (bump T17f ; l'EdgeRecord ne porte PAS
//!     de tenant — arbitrage A1 : cascade par noeud, `_pad` reserve)
//!   - `align_of::<NodeRecord>() == 8`
//!   - `align_of::<EdgeRecord>() == 4`
//!   - Both are `#[repr(C)]` and `bytemuck::Pod + Zeroable`, so `bytemuck::cast_slice`
//!     from a 4 KiB page-aligned mmap region is legal.

use bytemuck::{Pod, Zeroable};

/// Unsigned 48-bit little-endian integer, stored as a `[u8; 6]`.
///
/// Used for file-internal offsets where `u32` is too small (records arrays can
/// exceed 4 GiB on megalaw-scale graphs) but `u64` wastes 2 B × hot-path density.
#[repr(C)]
#[derive(Copy, Clone, PartialEq, Eq, Hash, Pod, Zeroable)]
pub struct U48(pub [u8; 6]);

impl U48 {
    pub const ZERO: Self = Self([0; 6]);

    #[inline]
    pub fn from_u64(v: u64) -> Self {
        debug_assert!(v < (1u64 << 48), "U48::from_u64: value exceeds 2^48");
        let b = v.to_le_bytes();
        Self([b[0], b[1], b[2], b[3], b[4], b[5]])
    }

    #[inline]
    pub fn to_u64(self) -> u64 {
        let [a, b, c, d, e, f] = self.0;
        u64::from_le_bytes([a, b, c, d, e, f, 0, 0])
    }

    #[inline]
    pub fn is_zero(self) -> bool {
        self.0 == [0; 6]
    }
}

impl core::fmt::Debug for U48 {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "U48({})", self.to_u64())
    }
}

impl Default for U48 {
    fn default() -> Self {
        Self::ZERO
    }
}

// ---------------------------------------------------------------------------
// NodeRecord — 32 B, aligned to 8.
// ---------------------------------------------------------------------------

/// Flags bit positions for [`NodeRecord::flags`].
pub mod node_flags {
    pub const TOMBSTONED: u16 = 1 << 0;
    pub const CENTRALITY_STALE: u16 = 1 << 1;
    pub const EMBEDDING_STALE: u16 = 1 << 2;
    pub const ENGRAM_SEED: u16 = 1 << 3;
    pub const HILBERT_DIRTY: u16 = 1 << 4;
    pub const IDENTITY: u16 = 1 << 5;
    /// (jalon 1 — RFC principal-natif-v2 §4-J1) Pose par TOUT writer de
    /// noeud a l'ecriture du record, jamais efface (un tombstone reste
    /// `ALLOCATED | TOMBSTONED`). Un slot a `flags & ALLOCATED == 0` est un
    /// creneau d'allocation jamais ecrit — un trou — inexistant pour tout
    /// chemin de lecture. Remplace l'heuristique « tout-a-zero » 8-champs
    /// (l'ambiguite `create_node_in_community(&[], 0)` ≡ trou a disparu).
    pub const ALLOCATED: u16 = 1 << 6;
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct NodeRecord {
    /// bit i set ⇒ node carries label interned as id `i` (up to 64 labels).
    pub label_bitset: u64,
    /// Offset into the `EdgeRecord` array of the first outgoing edge (linked list).
    pub first_edge_off: U48,
    /// Offset into the `PropertyPage` array of the first property page (linked list).
    pub first_prop_off: U48,
    /// LDleiden community id. `u32::MAX` ⇒ unassigned.
    pub community_id: u32,
    /// Energy in Q1.15 fixed-point; `0x8000` ≈ 1.0.
    pub energy: u16,
    /// Packed: scar(5b) | utility(5b) | affinity(5b) | dirty_flag(1b).
    pub scar_util_affinity: u16,
    /// Cached PageRank × 65535 (Q0.16). Stale flagged in `flags`.
    pub centrality_cached: u16,
    /// See [`node_flags`].
    pub flags: u16,
    /// (jalon 2 — RFC principal-natif-v2 §4-J2) Proprietaire du noeud.
    /// `tenant::PUBLIC` (0) = public, lisible de tous — valeur qui ne se
    /// SUBIT pas : elle n'est le defaut que d'un store qui l'a DECLARE
    /// (drapeau `meta_flags::MONO_TENANT_PUBLIC`, pose par la migration en
    /// bloc 2→3). `tenant::SENTINELLE_NON_MIGRE` (`u32::MAX`) = pas encore
    /// re-tenante, INVISIBLE de tout tenant (fail-closed, arbitrage A5) —
    /// reservee au re-tenanting par lots (§5.1), jamais posee par la
    /// migration 2→3. Toute autre valeur : id de tenant interne
    /// (`substrate.tenantcat`, jalons suivants). Le substrat SAIT a qui
    /// appartient chaque noeud ; il ne FILTRE rien avant le jalon 3.
    pub tenant_id: u32,
    /// Reserve : maintient 40 = multiple de l'alignement 8 et paie d'avance
    /// la prochaine extension. Toujours 0 en v3.
    pub _reserve: u32,
}

const _: [(); 1] = [(); (core::mem::size_of::<NodeRecord>() == 40) as usize];
const _: [(); 1] = [(); (core::mem::align_of::<NodeRecord>() == 8) as usize];

/// Number of `NodeRecord` slots per ALLOCATION page.
///
/// (jalon 2) Depuis le passage a 40 octets, 40 ∤ 4096 : la page
/// d'allocation n'est plus la page OS mais un multiple exact —
/// [`NODE_ALLOC_PAGE_BYTES`] = 20 480 octets = 5 pages OS de 4 KiB =
/// **512 records exactement, zero octet perdu** (arbitrage RFC §4-J2,
/// option recommandee). L'allocateur par communaute (T11 Step 3) et le
/// warden raisonnent en pages d'allocation ; l'adressage des slots reste
/// strictement lineaire (`offset = slot * SIZE`), des records chevauchent
/// donc des frontieres de pages OS — inoffensif pour la mmap.
pub const NODES_PER_PAGE: u32 = 512;

/// Taille de la page d'ALLOCATION des noeuds : 20 KiB = 5 pages OS.
pub const NODE_ALLOC_PAGE_BYTES: usize = 20480;

// Compile-time sanity checks: adjust together if NodeRecord grows.
// La page d'allocation est un multiple entier de pages OS ET de records —
// zero slack dans les deux directions.
const _: [(); 1] =
    [(); (NODES_PER_PAGE as usize * NodeRecord::SIZE == NODE_ALLOC_PAGE_BYTES) as usize];
const _: [(); 1] = [(); NODE_ALLOC_PAGE_BYTES.is_multiple_of(4096) as usize];

/// Identifiants de tenant reserves (jalon 2, RFC principal-natif-v2 §4-J2 / A5).
pub mod tenant {
    /// Public : lisible de tous les tenants. Ce defaut se DECLARE
    /// (meta_flags::MONO_TENANT_PUBLIC) — jamais implicite.
    pub const PUBLIC: u32 = 0;
    /// Pas encore re-tenante : INVISIBLE de tout tenant (fail-closed).
    /// Une migration par lots interrompue laisse de l'indisponible,
    /// jamais une fuite. Reservee — la migration 2→3 ne la pose pas.
    pub const SENTINELLE_NON_MIGRE: u32 = u32::MAX;

    /// LE predicat de portee tenant (jalon 3, arbitrage A3) — LA seule
    /// definition, partagee par toutes les primitives de lecture et par la
    /// cascade d'aretes. Pas de chemin rapide : il se teste toujours.
    ///
    /// * `portee = None` — AUCUN principal pose sur le store (tous les
    ///   usages actuels : viz, bridges, CLI). Tout est visible... sauf la
    ///   sentinelle : un noeud non migre n'existe pour PERSONNE, meme sans
    ///   principal (fail-closed A5).
    /// * `portee = Some(t)` — un principal de tenant `t` est pose. Visible =
    ///   le tenant du noeud est `t`, OU le noeud est PUBLIC. La sentinelle
    ///   n'egale jamais une portee (le constructeur de `PrincipalSubstrat`
    ///   refuse `u32::MAX`).
    ///
    /// Ce predicat ne parle QUE d'appartenance — la vivacite (`is_live`)
    /// est l'autre moitie, composee par [`NodeRecord::est_visible_pour`](crate::record::NodeRecord::est_visible_pour).
    #[inline]
    pub fn visible(tenant_noeud: u32, portee: Option<u32>) -> bool {
        if tenant_noeud == SENTINELLE_NON_MIGRE {
            return false;
        }
        match portee {
            None => true,
            Some(t) => tenant_noeud == t || tenant_noeud == PUBLIC,
        }
    }
}

/// Marqueur de format grave dans le `label_bitset` du SLOT 0 de la zone
/// nodes (slot reserve depuis toujours : les compteurs demarrent a 1).
///
/// (jalon 2) La zone v3 se decrit elle-meme : la migration 2→3 grave ce
/// marqueur en tete de la zone reecrite, et les etapes de migration s'en
/// servent pour connaitre le STRIDE d'une zone dont la version meta n'a pas
/// encore bumpe (kill entre le rename et le bump : la zone est deja 40 o,
/// la version dit encore 2 — sans marqueur, rejouer l'etape corromprait).
/// Le record sentinelle ne porte pas ALLOCATED : invisible de tout chemin
/// de lecture (`is_live()` faux), comme le slot 0 l'a toujours ete.
pub const NODES_ZONE_V3_MAGIC: u64 = u64::from_le_bytes(*b"NODESv3\0");

impl NodeRecord {
    pub const SIZE: usize = 40;

    /// Le record sentinelle du slot 0 d'une zone v3 (voir
    /// [`NODES_ZONE_V3_MAGIC`]). Flags a 0 : jamais vivant.
    #[inline]
    pub fn sentinelle_zone_v3() -> Self {
        Self {
            label_bitset: NODES_ZONE_V3_MAGIC,
            ..Self::default()
        }
    }

    #[inline]
    pub fn is_allocated(&self) -> bool {
        self.flags & node_flags::ALLOCATED != 0
    }

    /// LE predicat de vivacite des chemins de lecture (jalon 1) : vivant =
    /// reellement ecrit (ALLOCATED) et non tombstone. Un trou n'existe pas.
    ///
    /// ATTENTION (jalon 3) : la vivacite est la moitie SYSTEME du predicat.
    /// Les chemins de LECTURE passent par [`Self::est_visible_pour`], qui la
    /// compose avec la portee tenant ; `is_live()` nu reste le predicat des
    /// chemins internes (warden, compaction, recomptage, migration), qui
    /// voient TOUT — y compris les noeuds d'autrui et la sentinelle, qu'ils
    /// doivent preserver sans jamais les restituer.
    #[inline]
    pub fn is_live(&self) -> bool {
        self.flags & (node_flags::ALLOCATED | node_flags::TOMBSTONED) == node_flags::ALLOCATED
    }

    /// LE predicat des chemins de lecture depuis le jalon 3 : vivant ET
    /// dans la portee tenant. C'est le test qui vit DANS la meme boucle
    /// mmap que le bit tombstone (arbitrage A3 — pas une fonction de
    /// visibilite aval) : le record est deja dans le cache, la comparaison
    /// coute ce que coute le test tombstone.
    ///
    /// `portee` vient de `SubstrateStore::portee_tenant()` — la hisser HORS
    /// des boucles (une lecture du OnceLock par scan, pas par slot).
    #[inline]
    pub fn est_visible_pour(&self, portee: Option<u32>) -> bool {
        self.is_live() && tenant::visible(self.tenant_id, portee)
    }

    #[inline]
    pub fn is_tombstoned(&self) -> bool {
        self.flags & node_flags::TOMBSTONED != 0
    }

    #[inline]
    pub fn set_tombstoned(&mut self) {
        self.flags |= node_flags::TOMBSTONED;
    }

    #[inline]
    pub fn energy_q15(&self) -> f32 {
        q1_15_to_f32(self.energy)
    }

    #[inline]
    pub fn set_energy_f32(&mut self, v: f32) {
        self.energy = f32_to_q1_15(v);
    }

    /// Unpack `(scar, utility, affinity, dirty_flag)` each in 0..31 (or 0..1 for flag).
    #[inline]
    pub fn unpack_scar_util_aff(&self) -> PackedScarUtilAff {
        PackedScarUtilAff::unpack(self.scar_util_affinity)
    }

    #[inline]
    pub fn set_scar_util_aff(&mut self, p: PackedScarUtilAff) {
        self.scar_util_affinity = p.pack();
    }

    #[inline]
    pub fn centrality_f32(&self) -> f32 {
        self.centrality_cached as f32 / 65535.0
    }

    #[inline]
    pub fn set_centrality_f32(&mut self, v: f32) {
        self.centrality_cached = (v.clamp(0.0, 1.0) * 65535.0).round() as u16;
    }
}

/// Unpacked representation of `NodeRecord.scar_util_affinity`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct PackedScarUtilAff {
    pub scar: u8,     // 5 bits → 0..=31
    pub utility: u8,  // 5 bits
    pub affinity: u8, // 5 bits
    pub dirty: bool,  // 1 bit
}

impl PackedScarUtilAff {
    pub const fn new(scar: u8, utility: u8, affinity: u8, dirty: bool) -> Self {
        Self {
            scar,
            utility,
            affinity,
            dirty,
        }
    }

    #[inline]
    pub fn pack(self) -> u16 {
        let s = (self.scar & 0x1F) as u16;
        let u = (self.utility & 0x1F) as u16;
        let a = (self.affinity & 0x1F) as u16;
        let d = u16::from(self.dirty);
        s | (u << 5) | (a << 10) | (d << 15)
    }

    #[inline]
    pub fn unpack(x: u16) -> Self {
        Self {
            scar: (x & 0x1F) as u8,
            utility: ((x >> 5) & 0x1F) as u8,
            affinity: ((x >> 10) & 0x1F) as u8,
            dirty: (x >> 15) & 1 == 1,
        }
    }
}

// ---------------------------------------------------------------------------
// Quantization helpers for the packed scar/utility/affinity sub-fields.
//
// Each sub-field is 5 bits (0..=31). The cognitive stores choose a fixed
// value range that maps into those 32 levels. Callers that want to persist
// a float into the column go through these helpers; callers that want to
// read the column back as a float use the inverse helpers.
//
// These scales are the canonical on-disk contract — changing them is a
// substrate format break. They intentionally mirror the default
// `ScarConfig::min_intensity` × `max_scars_per_node` bound, the default
// `UtilityConfig::max_utility`, and the [0, 1] cosine range for affinity.
// ---------------------------------------------------------------------------

/// Upper bound for the cumulative scar intensity represented in the 5-bit
/// `scar` sub-field. Values above are clamped.
pub const SCAR_MAX_INTENSITY_Q5: f32 = 4.0;

/// Upper bound for the utility score represented in the 5-bit `utility`
/// sub-field. Values above are clamped.
pub const UTILITY_MAX_SCORE_Q5: f32 = 5.0;

#[inline]
pub fn scar_to_q5(x: f32) -> u8 {
    let norm = (x.clamp(0.0, SCAR_MAX_INTENSITY_Q5) / SCAR_MAX_INTENSITY_Q5) * 31.0;
    norm.round() as u8
}

#[inline]
pub fn q5_to_scar(q: u8) -> f32 {
    (q.min(31) as f32 / 31.0) * SCAR_MAX_INTENSITY_Q5
}

#[inline]
pub fn utility_to_q5(x: f32) -> u8 {
    let norm = (x.clamp(0.0, UTILITY_MAX_SCORE_Q5) / UTILITY_MAX_SCORE_Q5) * 31.0;
    norm.round() as u8
}

#[inline]
pub fn q5_to_utility(q: u8) -> f32 {
    (q.min(31) as f32 / 31.0) * UTILITY_MAX_SCORE_Q5
}

#[inline]
pub fn affinity_to_q5(x: f32) -> u8 {
    (x.clamp(0.0, 1.0) * 31.0).round() as u8
}

#[inline]
pub fn q5_to_affinity(q: u8) -> f32 {
    q.min(31) as f32 / 31.0
}

// ---------------------------------------------------------------------------
// EdgeRecord — 32 B stride (28 B logical + 4 B pad), aligned to 4.
// ---------------------------------------------------------------------------

pub mod edge_flags {
    pub const TOMBSTONED: u8 = 1 << 0;
    pub const RICCI_STALE: u8 = 1 << 1;
    pub const COACT: u8 = 1 << 2;
    pub const SYNAPSE_ACTIVE: u8 = 1 << 3;
    pub const BRIDGE: u8 = 1 << 4;
}

/// Canonical edge-type name for cumulative coactivation edges (T7 Step 5).
///
/// Per RFC pillar 2 (substrate format-spec §2 — "Coactivation : edge type
/// COACT"), coactivation between two nodes is recorded as an explicit
/// edge type stored in [`EdgeRecord::edge_type`] (interned via the
/// dictionary), distinct from synapse-typed edges so that decay schedules
/// can differ:
///
/// * SYNAPSE edges decay fast (Hebbian short-term reinforcement),
/// * COACT   edges decay slowly (long-term coactivation evidence).
///
/// The associated [`edge_flags::COACT`] flag bit is orthogonal — it can
/// be set on any edge type as a quick boolean tag, but the canonical
/// "this is a coactivation edge" signal is `edge_type == coact_type_id`.
pub const COACT_EDGE_TYPE_NAME: &str = "COACT";

/// Canonical edge-type name for cognitive synapses (layer cognitif).
///
/// (2026-08-31) Les synapses substrate sont écrites par
/// [`crate::store::SubstrateStore::create_synapse_slot`] sous ce type, dans
/// des slots qui ne participent PAS à l'adjacence relationnelle (invisibles
/// aux jointures MATCH, lues uniquement via la colonne poids et
/// `scan_typed_edge_weights`).
pub const SYNAPSE_EDGE_TYPE_NAME: &str = "SYNAPSE";

#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct EdgeRecord {
    pub src: u32,
    pub dst: u32,
    pub edge_type: u16,
    /// Synapse weight, Q0.16 (Hebbian reinforcement).
    pub weight_u16: u16,
    /// Next `EdgeRecord` offset in the linked list of edges sharing this `src`.
    pub next_from: U48,
    /// Next `EdgeRecord` offset in the linked list of edges sharing this `dst`.
    pub next_to: U48,
    /// Offset into the `PropertyPage` array of the first property page for this
    /// edge (linked list head). `U48::ZERO` ⇒ no properties stored in
    /// PropsZone v2 yet (legacy DashMap sidecar remains the fallback).
    ///
    /// Added in T17f Step 1 (layout bump 32 B → 36 B). Byte offset inside
    /// the record is **24** — see [`Writer::update_edge_first_prop_off`](crate::writer::Writer::update_edge_first_prop_off).
    pub first_prop_off: U48,
    /// Quantized Ricci-Ollivier curvature in [-1, 1] → (x + 1) × 127.5.
    pub ricci_u8: u8,
    /// See [`edge_flags`].
    pub flags: u8,
    /// Interned engram-cluster id (0 = none).
    pub engram_tag: u16,
    /// Padding to reach 36 B stride (align 4).
    pub _pad: [u8; 2],
}

const _: [(); 1] = [(); (core::mem::size_of::<EdgeRecord>() == 36) as usize];
const _: [(); 1] = [(); (core::mem::align_of::<EdgeRecord>() == 4) as usize];

impl EdgeRecord {
    pub const SIZE: usize = 36;

    /// Byte offset of [`EdgeRecord::first_prop_off`] inside the record.
    /// Used by `Writer::update_edge_first_prop_off` to patch only 6 bytes
    /// in place without rewriting the whole record.
    pub const FIRST_PROP_OFF_BYTES: usize = 24;

    #[inline]
    pub fn is_tombstoned(&self) -> bool {
        self.flags & edge_flags::TOMBSTONED != 0
    }

    #[inline]
    pub fn set_tombstoned(&mut self) {
        self.flags |= edge_flags::TOMBSTONED;
    }

    #[inline]
    pub fn weight_f32(&self) -> f32 {
        self.weight_u16 as f32 / 65535.0
    }

    #[inline]
    pub fn set_weight_f32(&mut self, v: f32) {
        self.weight_u16 = (v.clamp(0.0, 1.0) * 65535.0).round() as u16;
    }

    #[inline]
    pub fn ricci_f32(&self) -> f32 {
        (self.ricci_u8 as f32 / 127.5) - 1.0
    }

    #[inline]
    pub fn set_ricci_f32(&mut self, v: f32) {
        self.ricci_u8 = ((v.clamp(-1.0, 1.0) + 1.0) * 127.5).round() as u8;
    }
}

// ---------------------------------------------------------------------------
// SimplexRecord — 32 B, aligned to 2. (T4, plan « Topologie simpliciale
// native », décision 2d461b89 option B, spec 271180c3.)
// ---------------------------------------------------------------------------

/// Kinds de simplexes stockés dans [`SimplexRecord::kind`].
///
/// `EMPTY = 0` marque un slot jamais matérialisé (zone zero-remplie à la
/// croissance) — jamais écrit explicitement.
pub mod simplex_kind {
    /// Slot vide (jamais matérialisé).
    pub const EMPTY: u8 = 0;
    /// 2-simplexe : triangle closed-wedge (a<b<c strict), membres inline.
    pub const TRIANGLE: u8 = 2;
    /// 3-simplexe : clique K4 (v0<…<v3), membres inline.
    pub const K4: u8 = 3;
    /// 4-simplexe : clique K5 (v0<…<v4), membres inline.
    pub const K5: u8 = 4;
    /// Hyperarête typée n-aire : membres + propriétés dans le heap
    /// (`substrate.simplex_heap`), offset U48 dans le corps du record.
    pub const HYPEREDGE: u8 = 255;
}

/// Flags bit positions for [`SimplexRecord::flags`].
pub mod simplex_flags {
    /// Simplexe supprimé. Les slots ne sont jamais réutilisés — un id
    /// tombstoné reste tombstoné (I5, cascade ; réparation offline seulement).
    pub const TOMBSTONED: u8 = 1 << 0;
    /// Orientation (triangles uniquement) : l'arête de fermeture c→a existe
    /// aussi — 3-cycle dirigé, signe +1 dans l'ordre canonique.
    pub const CYCLIC: u8 = 1 << 1;
    /// Hyperarêtes uniquement : l'ordre d'émission est sémantique et gelé
    /// tel quel (séquences). La CLÉ d'unicité reste le hash des membres
    /// TRIÉS (spec §1 : « la clé reste le tri »).
    pub const ORDERED: u8 = 1 << 2;
}

/// Bornes cbm sur les hyperarêtes (store.h:254-256) — invariant I3.
/// Un groupe hors borne est rejeté ENTIER, jamais tronqué.
pub const SIMPLEX_HYPEREDGE_MIN_MEMBERS: usize = 3;
pub const SIMPLEX_HYPEREDGE_MAX_MEMBERS: usize = 64;

/// Record fixe 32 B de la zone `SimplexDir` (spec 271180c3 §2).
///
/// En-tête fixe 8 B : `kind`, `flags`, `type_id`, `member_count`,
/// `_reserved`. Le corps de 24 B est une UNION discriminée par `kind` :
///
/// * kinds 2-4 (triangle/K4/K5) — membres INLINE : 3-5 × u32 LE aux octets
///   `body[0..member_count*4]`, reste zéro. Zéro indirection ; le
///   `key_hash` (xxh3 des u32 triés) se recalcule à la demande sur ≤ 20 B —
///   il n'est PAS stocké, les membres inline SONT la clé.
/// * kind 255 (hyperarête) — `key_hash` u64 LE à `body[0..8]`,
///   `payload_off` U48 à `body[8..14]` (offset relatif au début de la
///   région payload du heap), `props_len` u32 LE à `body[14..18]`,
///   `body[18..24]` zéro. Les membres (`member_count` × u32 triés — ou en
///   ordre d'émission sous ORDERED) puis les `props_len` octets de
///   propriétés vivent dans le snapshot heap à `payload_off`.
///
/// `simplex_id` = index du record dans la zone (u32 — lève la borne u16
/// des engrams). Les slots ne sont jamais réutilisés.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct SimplexRecord {
    /// Voir [`simplex_kind`]. 0 ⇒ slot vide.
    pub kind: u8,
    /// Voir [`simplex_flags`].
    pub flags: u8,
    /// Type d'hyperarête interné au dict (0 pour les kinds 2-4).
    pub type_id: u16,
    /// Nombre de membres : 3/4/5 pour les kinds 2-4, 3..=64 pour les
    /// hyperarêtes (I3).
    pub member_count: u16,
    /// Réservé — doit rester zéro.
    pub _reserved: u16,
    /// Union discriminée par `kind` — voir la doc du type.
    pub body: [u8; 24],
}

const _: [(); 1] = [(); (core::mem::size_of::<SimplexRecord>() == 32) as usize];

impl SimplexRecord {
    pub const SIZE: usize = 32;
    /// Capacité inline du corps en membres u32 (kinds 2-4).
    pub const INLINE_MAX_MEMBERS: usize = 5;

    #[inline]
    pub fn is_empty_slot(&self) -> bool {
        self.kind == simplex_kind::EMPTY
    }

    #[inline]
    pub fn is_tombstoned(&self) -> bool {
        self.flags & simplex_flags::TOMBSTONED != 0
    }

    #[inline]
    pub fn set_tombstoned(&mut self) {
        self.flags |= simplex_flags::TOMBSTONED;
    }

    #[inline]
    pub fn is_hyperedge(&self) -> bool {
        self.kind == simplex_kind::HYPEREDGE
    }

    /// Membres inline (kinds 2-4 uniquement). Panique en debug sur une
    /// hyperarête — les membres d'une hyperarête vivent dans le heap.
    pub fn inline_members(&self) -> Vec<u32> {
        debug_assert!(!self.is_hyperedge(), "inline_members sur une hyperarête");
        let n = (self.member_count as usize).min(Self::INLINE_MAX_MEMBERS);
        (0..n)
            .map(|i| u32::from_le_bytes(self.body[i * 4..i * 4 + 4].try_into().unwrap()))
            .collect()
    }

    /// Écrit les membres inline (kinds 2-4). `members.len() ≤ 5` exigé.
    pub fn set_inline_members(&mut self, members: &[u32]) {
        debug_assert!(members.len() <= Self::INLINE_MAX_MEMBERS);
        self.body = [0; 24];
        for (i, &m) in members.iter().enumerate() {
            self.body[i * 4..i * 4 + 4].copy_from_slice(&m.to_le_bytes());
        }
        self.member_count = members.len() as u16;
    }

    /// `key_hash` stocké (hyperarêtes uniquement).
    #[inline]
    pub fn stored_key_hash(&self) -> u64 {
        u64::from_le_bytes(self.body[0..8].try_into().unwrap())
    }

    #[inline]
    pub fn set_stored_key_hash(&mut self, h: u64) {
        self.body[0..8].copy_from_slice(&h.to_le_bytes());
    }

    /// Offset U48 du snapshot payload dans le heap (hyperarêtes).
    #[inline]
    pub fn payload_off(&self) -> u64 {
        let mut b = [0u8; 6];
        b.copy_from_slice(&self.body[8..14]);
        U48(b).to_u64()
    }

    #[inline]
    pub fn set_payload_off(&mut self, off: u64) {
        self.body[8..14].copy_from_slice(&U48::from_u64(off).0);
    }

    /// Longueur en octets des propriétés dans le snapshot heap (hyperarêtes).
    #[inline]
    pub fn props_len(&self) -> u32 {
        u32::from_le_bytes(self.body[14..18].try_into().unwrap())
    }

    #[inline]
    pub fn set_props_len(&mut self, len: u32) {
        self.body[14..18].copy_from_slice(&len.to_le_bytes());
    }
}

// ---------------------------------------------------------------------------
// Q1.15 fixed-point helpers (for energy, synapse weights, affinity scores).
// ---------------------------------------------------------------------------

/// Convert a `f32` in `[0.0, 1.0]` to Q1.15 (`u16`). Values outside the range
/// are clamped.
#[inline]
pub fn f32_to_q1_15(v: f32) -> u16 {
    (v.clamp(0.0, 1.0) * 32768.0).round().min(65535.0) as u16
}

/// Convert a Q1.15 `u16` back to `f32` in `[0.0, 2.0)`.
#[inline]
pub fn q1_15_to_f32(x: u16) -> f32 {
    x as f32 / 32768.0
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_asserts() {
        // (jalon 2) 32 → 40 : colonne tenant_id + reserve, align 8 conserve.
        assert_eq!(core::mem::size_of::<NodeRecord>(), 40);
        assert_eq!(core::mem::size_of::<EdgeRecord>(), 36);
        // La page d'allocation : 512 records de 40 o = 20 480 o = 5 pages OS,
        // zero octet perdu (arbitrage RFC §4-J2).
        assert_eq!(NODES_PER_PAGE, 512);
        assert_eq!(NODE_ALLOC_PAGE_BYTES, 20480);
        assert_eq!(
            NODES_PER_PAGE as usize * NodeRecord::SIZE,
            NODE_ALLOC_PAGE_BYTES
        );
        assert_eq!(NODE_ALLOC_PAGE_BYTES % 4096, 0);
        // tenant_id vit aux octets 32..36 — les champs v2 n'ont PAS bouge
        // (le tenant s'ajoute en QUEUE : first_prop_off reste a 14, flags
        // a 30 — les patchs-en-place de 6 octets du writer restent justes).
        let rec = NodeRecord {
            tenant_id: 0xA1B2_C3D4,
            ..Default::default()
        };
        let bytes = bytemuck::bytes_of(&rec);
        assert_eq!(
            &bytes[32..36],
            &0xA1B2_C3D4u32.to_le_bytes(),
            "tenant_id n'est pas aux octets 32..36 — le layout a derive"
        );
        assert_eq!(
            core::mem::offset_of!(NodeRecord, flags),
            30,
            "flags n'est plus a l'octet 30 — migration 1→2 et sondes disque faussees"
        );
        assert_eq!(core::mem::align_of::<NodeRecord>(), 8);
        assert_eq!(core::mem::align_of::<EdgeRecord>(), 4);
        assert_eq!(core::mem::size_of::<U48>(), 6);
        // T17f Step 1 — first_prop_off byte offset must stay at 24 so the
        // writer's patch-in-place of the 6-byte head pointer stays correct.
        assert_eq!(EdgeRecord::FIRST_PROP_OFF_BYTES, 24);
        // Runtime offset check: poke a sentinel pattern into first_prop_off
        // and verify the bytes land exactly at offset 24.
        let rec = EdgeRecord {
            first_prop_off: U48::from_u64(0x0000_00FF_FFFF_FF00),
            ..Default::default()
        };
        let bytes = bytemuck::bytes_of(&rec);
        // Bytes 24..30 must mirror the little-endian encoding of 0x00FFFFFFFF00.
        assert_eq!(
            &bytes[24..30],
            &[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0x00],
            "first_prop_off field is not at byte offset 24 — layout drifted"
        );
    }

    #[test]
    fn u48_roundtrip() {
        for v in [0u64, 1, 42, 0x00FFFF, 0x0001_0000, (1u64 << 48) - 1] {
            let x = U48::from_u64(v);
            assert_eq!(x.to_u64(), v, "roundtrip failed for {v}");
        }
    }

    #[test]
    fn u48_is_zero() {
        assert!(U48::ZERO.is_zero());
        assert!(!U48::from_u64(1).is_zero());
    }

    #[test]
    fn q1_15_roundtrip() {
        // Exact representations
        assert_eq!(f32_to_q1_15(0.0), 0);
        assert_eq!(f32_to_q1_15(1.0), 32768);
        assert!((q1_15_to_f32(f32_to_q1_15(0.5)) - 0.5).abs() < 1e-4);
        assert!((q1_15_to_f32(f32_to_q1_15(0.25)) - 0.25).abs() < 1e-4);
        // Clamping
        assert_eq!(f32_to_q1_15(-1.0), 0);
        assert_eq!(f32_to_q1_15(2.0), 32768);
    }

    #[test]
    fn scar_util_aff_pack_roundtrip() {
        for s in 0u8..=31 {
            for u in [0u8, 15, 31] {
                for a in [0u8, 7, 31] {
                    for d in [false, true] {
                        let p = PackedScarUtilAff::new(s, u, a, d);
                        let packed = p.pack();
                        let unpacked = PackedScarUtilAff::unpack(packed);
                        assert_eq!(unpacked, p, "roundtrip failed for {:?}", p);
                    }
                }
            }
        }
    }

    #[test]
    fn scar_util_aff_clips_to_5bits() {
        // Input beyond 5 bits is silently masked — documented behavior.
        let p = PackedScarUtilAff::new(0xFF, 0xFF, 0xFF, true);
        let r = PackedScarUtilAff::unpack(p.pack());
        assert_eq!(r.scar, 31);
        assert_eq!(r.utility, 31);
        assert_eq!(r.affinity, 31);
        assert!(r.dirty);
    }

    #[test]
    fn node_record_default_is_zeroed() {
        let n = NodeRecord::default();
        assert_eq!(n.label_bitset, 0);
        assert_eq!(n.community_id, 0);
        assert_eq!(n.energy, 0);
        assert_eq!(n.flags, 0);
        assert!(n.first_edge_off.is_zero());
        assert!(!n.is_tombstoned());
    }

    #[test]
    fn node_record_energy_accessor() {
        let mut n = NodeRecord::default();
        n.set_energy_f32(0.5);
        assert!((n.energy_q15() - 0.5).abs() < 1e-4);
    }

    #[test]
    fn node_record_centrality_accessor() {
        let mut n = NodeRecord::default();
        n.set_centrality_f32(0.75);
        assert!((n.centrality_f32() - 0.75).abs() < 1e-4);
    }

    #[test]
    fn edge_record_weight_accessor() {
        let mut e = EdgeRecord::default();
        e.set_weight_f32(0.333);
        assert!((e.weight_f32() - 0.333).abs() < 1e-4);
    }

    #[test]
    fn edge_record_ricci_accessor() {
        let mut e = EdgeRecord::default();
        e.set_ricci_f32(-0.5);
        assert!((e.ricci_f32() - (-0.5)).abs() < 1e-2);
        e.set_ricci_f32(0.5);
        assert!((e.ricci_f32() - 0.5).abs() < 1e-2);
    }

    #[test]
    fn node_record_pod_cast() {
        let n = NodeRecord {
            label_bitset: 0x1234_5678_9ABC_DEF0,
            first_edge_off: U48::from_u64(0x0000_00FF_FFFF),
            first_prop_off: U48::ZERO,
            community_id: 42,
            energy: 0x4000,
            scar_util_affinity: 0x1234,
            centrality_cached: 0xABCD,
            flags: node_flags::ENGRAM_SEED,
            tenant_id: 7,
            _reserve: 0,
        };
        let bytes: &[u8] = bytemuck::bytes_of(&n);
        assert_eq!(bytes.len(), 40);
        let n2: NodeRecord = *bytemuck::from_bytes::<NodeRecord>(bytes);
        assert_eq!(n, n2);
    }

    #[test]
    fn edge_record_pod_cast() {
        let e = EdgeRecord {
            src: 7,
            dst: 11,
            edge_type: 3,
            weight_u16: 0x8000,
            next_from: U48::from_u64(128),
            next_to: U48::from_u64(256),
            first_prop_off: U48::from_u64(0x9999),
            ricci_u8: 64,
            flags: edge_flags::SYNAPSE_ACTIVE | edge_flags::COACT,
            engram_tag: 17,
            _pad: [0; 2],
        };
        let bytes: &[u8] = bytemuck::bytes_of(&e);
        assert_eq!(bytes.len(), 36);
        let e2: EdgeRecord = *bytemuck::from_bytes::<EdgeRecord>(bytes);
        assert_eq!(e, e2);
    }

    /// T17f Step 1 — a fresh `EdgeRecord::default()` must have
    /// `first_prop_off == U48::ZERO`, guaranteeing that existing code paths
    /// that initialise edges without touching the new field behave exactly
    /// like the legacy 32 B layout: no property head → DashMap-only edge
    /// properties.
    #[test]
    fn edge_record_default_has_zero_first_prop_off() {
        let e = EdgeRecord::default();
        assert_eq!(e.first_prop_off, U48::ZERO);
        assert!(e.first_prop_off.is_zero());
    }

    /// T17f Step 1 — round-trip through bytemuck with every byte of every
    /// field set to a non-trivial value, to prove the 36 B layout is
    /// stable and that no field aliases another.
    #[test]
    fn edge_record_full_byte_roundtrip() {
        let e = EdgeRecord {
            src: 0xDEAD_BEEF,
            dst: 0xCAFE_BABE,
            edge_type: 0x1234,
            weight_u16: 0xFEED,
            next_from: U48::from_u64(0x0000_AABB_CCDD_EEFF),
            next_to: U48::from_u64(0x0000_1122_3344_5566),
            first_prop_off: U48::from_u64(0x0000_99AA_BBCC_DDEE),
            ricci_u8: 0x42,
            flags: 0x7F,
            engram_tag: 0x9ABC,
            _pad: [0xAB, 0xCD],
        };
        let bytes = bytemuck::bytes_of(&e).to_vec();
        assert_eq!(bytes.len(), 36);
        let decoded: EdgeRecord = *bytemuck::from_bytes::<EdgeRecord>(&bytes);
        assert_eq!(e, decoded);
        // Spot-check: src at offset 0, dst at offset 4, first_prop_off at 24.
        assert_eq!(&bytes[0..4], &0xDEAD_BEEF_u32.to_le_bytes());
        assert_eq!(&bytes[4..8], &0xCAFE_BABE_u32.to_le_bytes());
        assert_eq!(
            &bytes[24..30],
            &[0xEE, 0xDD, 0xCC, 0xBB, 0xAA, 0x99],
            "first_prop_off must be at offset 24 in little-endian U48 form"
        );
    }
}
