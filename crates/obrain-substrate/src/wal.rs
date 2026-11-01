//! Write-Ahead Log — framed, length-prefixed, CRC32-protected records.
//!
//! See `docs/rfc/substrate/wal-spec.md`.
//!
//! Framing (28 B header + payload):
//!
//! ```text
//!   0..4   length      u32 LE     payload length
//!   4      kind        u8         WalKind discriminant
//!   5      flags       u8         bit 0 = commit marker, 1 = checkpoint
//!   6..8   reserved    u16 LE     zero
//!   8..16  lsn         u64 LE     monotonic log sequence number
//!   16..24 timestamp   i64 LE     unix micros
//!   24..28 crc32       u32 LE     crc32 of bytes [0..24] XOR payload
//! ```

use crate::error::SubstrateError;
use bincode::config::{Configuration, Fixint, LittleEndian};
use serde::{Deserialize, Serialize};

pub const WAL_HEADER_SIZE: usize = 28;

/// `bincode` configuration used for WAL payloads: little-endian, fixed-int encoding
/// so record size is stable across machines.
fn bincode_config() -> Configuration<LittleEndian, Fixint> {
    bincode::config::standard()
        .with_little_endian()
        .with_fixed_int_encoding()
}

// ---------------------------------------------------------------------------
// Kind enum (u8 discriminants documented in wal-spec.md §3)
// ---------------------------------------------------------------------------

#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum WalKind {
    NodeInsert = 0x01,
    NodeUpdate = 0x02,
    NodeDelete = 0x03,
    /// (jalon 2) NodeInsert portant le tenant — voir la variante payload.
    NodeInsertV3 = 0x04,
    /// (jalon 2) NodeUpdate portant le tenant — voir la variante payload.
    NodeUpdateV3 = 0x05,
    EdgeInsert = 0x10,
    EdgeUpdate = 0x11,
    EdgeDelete = 0x12,
    /// (2026-08-17, aretes inter-bases V1) Insertion d'une arete EXTERNE —
    /// cible dans un autre store, sidecar DashMap cote store. Rejouee par
    /// `SubstrateStore::merge_wal_external_edges` a l'ouverture, PAS par
    /// `replay_from` (aucune zone mmap concernee).
    ExternalEdgeInsert = 0x13,
    /// Suppression d'une arete externe — pendant de ExternalEdgeInsert,
    /// pour que le rejeu ne RESSUSCITE pas une arete supprimee apres le
    /// dernier snapshot.
    ExternalEdgeDelete = 0x14,
    PropSet = 0x20,
    PropDelete = 0x21,
    /// Mutate only the `first_prop_off` (U48) head pointer of a node — the
    /// chain anchor that points into the v2 props zone. Introduced by T17c
    /// Step 3b.2 so that property-chain mutations are durably replayed
    /// without rewriting the rest of the `NodeRecord`.
    NodePropHeadUpdate = 0x22,
    /// Mutate only the `first_prop_off` (U48) head pointer of an edge — the
    /// chain anchor that points into the v2 props zone. Introduced by T17f
    /// Step 3 (edge-properties migration) to mirror `NodePropHeadUpdate` for
    /// edges, so that edge-chain mutations are durably replayed without
    /// rewriting the rest of the `EdgeRecord`.
    EdgePropHeadUpdate = 0x23,
    /// GARANTIE MEMOIRE (2026-07-25). Valeur de propriete scalaire, INLINE
    /// dans le WAL. Voir la doc de la variante WalPayload::PropSetInline.
    PropSetInline = 0x24,
    /// Suppression de propriete, inline. Pendant de PropSetInline.
    PropRemoveInline = 0x25,
    /// PropSetInline pour les proprietes d ARETE (sidecar DashMap edge).
    EdgePropSetInline = 0x26,
    /// Pendant de PropRemoveInline pour les ARETES — trouve par la revue a
    /// charge du 25/07 : sans lui, le rejeu RESSUSCITAIT une propriete
    /// d arete supprimee apres le dernier snapshot.
    EdgePropRemoveInline = 0x27,
    StringIntern = 0x30,
    LabelIntern = 0x31,
    KeyIntern = 0x32,
    /// (2026-08-15, garantie zero perte) Internement d'un TYPE D'ARETE.
    /// LabelIntern/KeyIntern existaient au format sans que personne ne les
    /// ecrive ; les types d'aretes n'avaient meme pas de variante — un
    /// type interne dans la fenetre non checkpointee laissait des aretes
    /// au type sans nom apres rejeu.
    EdgeTypeIntern = 0x33,
    EnergyDecay = 0x40,
    EnergyReinforce = 0x41,
    SynapseReinforce = 0x42,
    ScarUtilAffinitySet = 0x43,
    CentralityUpdate = 0x44,
    SynapseDecay = 0x45,
    CommunityAssign = 0x50,
    HilbertRepermute = 0x51,
    RicciUpdate = 0x52,
    CompactCommunity = 0x53,
    Tier0Update = 0x60,
    Tier1Update = 0x61,
    Tier2Update = 0x62,
    EngramMembersSet = 0x70,
    EngramBitsetSet = 0x71,
    /// (2026-08-30, T4 simplicial) État absolu d'un simplexe — voir
    /// [`WalPayload::SimplexSet`]. Suit le gabarit 0x70/0x71.
    SimplexSet = 0x72,
    /// Tombstone d'un simplexe — pendant de SimplexSet, pour que le rejeu
    /// ne RESSUSCITE pas un simplexe supprimé après le dernier snapshot.
    SimplexTombstone = 0x73,
    /// (2026-08-30, T5 passes) Configuration des passes de matérialisation
    /// simpliciales — voir [`WalPayload::SimplexPassesConfig`].
    SimplexPassesConfig = 0x74,
    CoactDecay = 0x46,
    /// (2026-08-23, chantier SlotRemappable) Repermutation Hilbert V2,
    /// porteuse d'un `remap_epoch` — les side-cars slot-indexés sont
    /// enrôlés dans le remap (voir `crate::slot_remap`).
    HilbertRepermuteV2 = 0x54,
    /// (2026-08-23, chantier SlotRemappable) Compaction V2, porteuse d'un
    /// `remap_epoch` — pendant de `HilbertRepermuteV2`.
    CompactCommunityV2 = 0x55,
    /// (2026-11-01, D-DUR v0.0.1) TRANSACTIONS EN VOL AU REJEU. Le WAL du
    /// substrat est un journal PHYSIQUE : chaque création est écrite dans
    /// les zones à l'instant où elle a lieu, et le marqueur « en attente »
    /// qui la cache aux autres sessions ne vit qu'en mémoire. Un processus
    /// qui disparaît avec une transaction ouverte laissait donc ses nœuds et
    /// arêtes PUBLIÉS à la réouverture (mesuré : 50/50 sur
    /// `durabilite_commit_kill9`). Ces trois records donnent au rejeu la
    /// frontière de transaction qui lui manquait : une entité déclarée en
    /// attente par une transaction dont aucun `TxCommit` n'est rejoué est
    /// écartée (tombstone) en fin de rejeu. Opcodes en fin de table ; un
    /// binaire antérieur s'arrête dessus (règle « checkpoint forcé avant tout
    /// downgrade », comme pour 0x54/0x55).
    TxNoeudEnAttente = 0x56,
    TxAreteEnAttente = 0x57,
    TxCommit = 0x58,
    Checkpoint = 0xF0,
    NoOp = 0xFE,
    EndOfLog = 0xFF,
}

impl WalKind {
    pub fn from_u8(x: u8) -> Option<Self> {
        use WalKind::*;
        Some(match x {
            0x01 => NodeInsert,
            0x02 => NodeUpdate,
            0x03 => NodeDelete,
            0x04 => NodeInsertV3,
            0x05 => NodeUpdateV3,
            0x10 => EdgeInsert,
            0x11 => EdgeUpdate,
            0x12 => EdgeDelete,
            0x13 => ExternalEdgeInsert,
            0x14 => ExternalEdgeDelete,
            0x20 => PropSet,
            0x21 => PropDelete,
            0x22 => NodePropHeadUpdate,
            0x23 => EdgePropHeadUpdate,
            0x24 => PropSetInline,
            0x25 => PropRemoveInline,
            0x26 => EdgePropSetInline,
            0x27 => EdgePropRemoveInline,
            0x30 => StringIntern,
            0x31 => LabelIntern,
            0x32 => KeyIntern,
            0x33 => EdgeTypeIntern,
            0x40 => EnergyDecay,
            0x41 => EnergyReinforce,
            0x42 => SynapseReinforce,
            0x43 => ScarUtilAffinitySet,
            0x44 => CentralityUpdate,
            0x45 => SynapseDecay,
            0x50 => CommunityAssign,
            0x51 => HilbertRepermute,
            0x52 => RicciUpdate,
            0x53 => CompactCommunity,
            0x60 => Tier0Update,
            0x61 => Tier1Update,
            0x62 => Tier2Update,
            0x70 => EngramMembersSet,
            0x71 => EngramBitsetSet,
            0x72 => SimplexSet,
            0x73 => SimplexTombstone,
            0x74 => SimplexPassesConfig,
            0x46 => CoactDecay,
            0x54 => HilbertRepermuteV2,
            0x55 => CompactCommunityV2,
            0x56 => TxNoeudEnAttente,
            0x57 => TxAreteEnAttente,
            0x58 => TxCommit,
            0xF0 => Checkpoint,
            0xFE => NoOp,
            0xFF => EndOfLog,
            _ => return None,
        })
    }
}

// ---------------------------------------------------------------------------
// Payloads
// ---------------------------------------------------------------------------

/// Generic WAL payload — each variant maps to a [`WalKind`].
///
/// Payloads intentionally carry full state (not deltas) so that replay is
/// idempotent (re-applying an already-durable record is a no-op).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum WalPayload {
    NodeInsert {
        node_id: u32,
        label_bitset: u64,
    },
    NodeUpdate {
        node_id: u32,
        label_bitset: u64,
        energy: u16,
        scar_util_affinity: u16,
        centrality_cached: u16,
        flags: u16,
        community_id: u32,
    },
    NodeDelete {
        node_id: u32,
    },
    EdgeInsert {
        edge_id: u64,
        src: u32,
        dst: u32,
        edge_type: u16,
        weight_u16: u16,
    },
    EdgeUpdate {
        edge_id: u64,
        weight_u16: u16,
        ricci_u8: u8,
        flags: u8,
        engram_tag: u16,
    },
    EdgeDelete {
        edge_id: u64,
    },
    PropSet {
        node_id: u32,
        key_id: u16,
        value: PropValue,
    },
    PropDelete {
        node_id: u32,
        key_id: u16,
    },
    StringIntern {
        page_id: u32,
        offset: u32,
        bytes: Vec<u8>,
    },
    LabelIntern {
        label_id: u16,
        name: String,
    },
    KeyIntern {
        key_id: u16,
        name: String,
    },
    EnergyDecay {
        factor_q16: u16, // multiplicative factor in Q0.16
    },
    EnergyReinforce {
        node_id: u32,
        new_energy: u16,
    },
    SynapseReinforce {
        edge_id: u64,
        new_weight: u16,
    },
    SynapseDecay {
        factor_q16: u16, // multiplicative factor in Q0.16 applied to all live SYNAPSE edges
    },
    ScarUtilAffinitySet {
        node_id: u32,
        packed: u16,
    },
    CentralityUpdate {
        updates: Vec<(u32, u16)>, // (node_id, centrality_cached)
    },
    CommunityAssign {
        updates: Vec<(u32, u32)>, // (node_id, community_id)
    },
    HilbertRepermute {
        permutation: Vec<u32>,
    },
    RicciUpdate {
        updates: Vec<(u64, u8)>, // (edge_id, ricci_u8)
    },
    Tier0Update {
        node_id: u32,
        bits: [u64; 2], // 128 bits
    },
    Tier1Update {
        node_id: u32,
        bits: [u64; 8], // 512 bits
    },
    Tier2Update {
        node_id: u32,
        f16s: Vec<u16>, // 384 × f16 stored as raw u16
    },
    /// Full engram membership snapshot. The WAL carries the entire list of
    /// member node slot IDs so replay is idempotent (re-applying overwrites
    /// the same directory entry with the same bytes). `engram_id = 0` is
    /// reserved and rejected by the writer.
    EngramMembersSet {
        engram_id: u16,
        members: Vec<u32>,
    },
    /// Absolute value of a node's 64-bit engram signature (T7 Step 1).
    /// Replay overwrites the column slot with `bitset` verbatim — idempotent.
    EngramBitsetSet {
        node_id: u32,
        bitset: u64,
    },
    Checkpoint {
        at_lsn: u64,
    },
    NoOp,
    EndOfLog,
    /// Apply a Q0.16 multiplicative decay to every live edge whose
    /// `edge_type` equals `coact_type_id` (T7 Step 5).
    ///
    /// Distinct from [`WalPayload::SynapseDecay`] because COACT and
    /// SYNAPSE columns share the same `weight_u16` slot but follow
    /// different decay schedules — see RFC pillar 2 / format-spec §2.
    ///
    /// Idempotent under replay: re-applying the same factor multiplies
    /// twice (mirroring `SynapseDecay` semantics — replay deterministic
    /// because the records are ordered).
    ///
    /// **Wire-compat note**: this variant is intentionally appended at the
    /// tail of `WalPayload` to keep all preceding serde variant indices
    /// stable for older WAL files written before T7 Step 5.
    CoactDecay {
        factor_q16: u16,
        coact_type_id: u16,
    },
    /// Transactional per-community compaction (T11 Step 5).
    ///
    /// Relocates every live node of `community_id` from its scattered old
    /// slot to a fresh contiguous page-aligned range at the file tail.
    /// `relocations[i] = (old_slot, new_slot)` describes the move; the WAL
    /// record precedes the mmap mutation (WAL-first), so a crash between
    /// the two is recovered idempotently by replay:
    ///
    /// * On replay, the engine walks the mapping and, for each `(old, new)`:
    ///   - if the node at `old` carries `community_id == target` → copy to
    ///     `new`, zero-fill `old` (completes the move);
    ///   - if the node at `new` already carries `community_id == target` →
    ///     treat as already applied (no-op).
    ///
    /// * Edge remap is also idempotent: re-applying `src = old_to_new[src]`
    ///   only fires when `src` is still in the old-slot domain (i.e. still
    ///   tied to the pre-compaction layout) — after the first pass, `src`
    ///   has moved to the new range and the lookup misses, so the second
    ///   application is a no-op.
    ///
    /// `page_range` (`(first, last)`, half-open on `last`) documents the
    /// new contiguous page range for verification / debugging; the actual
    /// replay only depends on `relocations`.
    CompactCommunity {
        community_id: u32,
        relocations: Vec<(u32, u32)>,
        page_range: (u32, u32),
    },
    /// Update only the `first_prop_off` head pointer of a node (T17c
    /// Step 3b.2). The full U48 value is serialized as `u64` (upper 16 bits
    /// always zero) for a stable bincode wire width independent of `U48`'s
    /// internal byte layout.
    ///
    /// Idempotent under replay: re-applying overwrites the same 6-byte
    /// `first_prop_off` slot in the `NodeRecord` with the same value,
    /// leaving every other field untouched.
    ///
    /// **Wire-compat note**: appended at the tail of `WalPayload` — older
    /// WAL files (written before T17c) do not contain this variant and
    /// remain decodable because every preceding variant index is stable.
    NodePropHeadUpdate {
        node_id: u32,
        first_prop_off: u64,
    },
    /// Update only the `first_prop_off` head pointer of an edge (T17f
    /// Step 3). The full U48 value is serialized as `u64` (upper 16 bits
    /// always zero) for a stable bincode wire width independent of `U48`'s
    /// internal byte layout.
    ///
    /// Idempotent under replay: re-applying overwrites the same 6-byte
    /// `first_prop_off` slot in the `EdgeRecord` (byte offset 24) with the
    /// same value, leaving every other field untouched.
    ///
    /// **Wire-compat note**: appended at the tail of `WalPayload` — older
    /// WAL files (written before T17f) do not contain this variant and
    /// remain decodable because every preceding variant index is stable.
    EdgePropHeadUpdate {
        edge_idx: u32,
        first_prop_off: u64,
    },
    /// ── GARANTIE MEMOIRE (2026-07-25, operateur : « je veux la garantie
    /// que mon agent ne perdra plus de memoire ») ──
    ///
    /// Valeur de propriete scalaire journalisee INLINE, contenu compris.
    /// Contexte mesure en production : les proprietes scalaires vivent
    /// dans le sidecar DashMap, durable UNIQUEMENT via le snapshot
    /// substrate.props reecrit au flush ("a crash between two flushes
    /// loses the interim properties", props_snapshot.rs). Le WAL en
    /// EveryCommit rejouait donc les NOEUDS mais jamais leurs proprietes
    /// — des « coquilles » a labels sans contenu, constatees deux fois
    /// sur l etage teleologique de l agent (aspirations/saisons/objectifs).
    ///
    /// `value_bincode` est l encodage bincode (standard, fixint, LE) du
    /// `Value` du graphe — opaque pour le WAL, encode/decode par
    /// SubstrateStore (helpers wal_inline_*). La cle est inline en clair :
    /// AUCUNE dependance au registre de cles ni a la zone strings, pour
    /// que le rejeu fonctionne meme quand les registres (persistes
    /// seulement au flush, via persist_dict) sont en retard sur le WAL.
    ///
    /// Idempotent au rejeu : re-appliquer re-insere la meme paire
    /// (cle → valeur) dans le sidecar — ecrasement identique. L ordre du
    /// WAL donne le last-write-wins correct.
    ///
    /// **Wire-compat** : ajoute en QUEUE de l enum — les WAL anterieurs
    /// restent decodables (indices serde stables) ; un binaire ANTERIEUR
    /// qui lit un WAL contenant ces variantes s arrete a la premiere
    /// (kind inconnu) : ne pas retrograder le binaire apres ecritures.
    PropSetInline {
        node_id: u32,
        key: String,
        value_bincode: Vec<u8>,
    },
    /// Suppression d une propriete scalaire — pendant de PropSetInline,
    /// pour que le rejeu ne RESSUSCITE pas une propriete supprimee apres
    /// le dernier snapshot. Memes garanties d idempotence et de
    /// wire-compat.
    PropRemoveInline {
        node_id: u32,
        key: String,
    },
    /// PropSetInline pour les ARETES (sidecar edge_properties). Memes
    /// motivations, memes garanties.
    EdgePropSetInline {
        edge_id: u64,
        key: String,
        value_bincode: Vec<u8>,
    },
    /// Suppression d une propriete d ARETE — pendant de PropRemoveInline.
    /// Ajoute par la revue a charge du 25/07 (le trou aurait fait
    /// RESSUSCITER au rejeu toute propriete d arete supprimee apres le
    /// dernier snapshot). Memes garanties (idempotence, wire-compat queue).
    EdgePropRemoveInline {
        edge_id: u64,
        key: String,
    },
    /// (2026-08-15, garantie zero perte) Internement d'un TYPE D'ARETE —
    /// pendant de LabelIntern/KeyIntern, qui existaient au format sans
    /// que personne ne les ecrive.
    ///
    /// EN FIN D'ENUM, DELIBEREMENT ET POUR TOUJOURS : bincode/serde
    /// encode par INDICE de variante. Une variante inseree au milieu
    /// decalerait l'indice de toutes les suivantes et rendrait illisible
    /// chaque WAL deja ecrit. Toute variante future s'ajoute APRES
    /// celle-ci, jamais avant.
    EdgeTypeIntern {
        type_id: u16,
        name: String,
    },
    /// (2026-08-17, aretes inter-bases V1) Insertion d'une arete EXTERNE.
    /// EN FIN D'ENUM, comme l'exige la regle bincode-par-indice ci-dessus.
    /// Le type d'arete est porte EN CLAIR (String), pas via le registre
    /// interne : le rejeu n'a alors aucune dependance d'ordre avec les
    /// EdgeTypeIntern, et le volume d'aretes externes reste marginal.
    /// `to_host` est reserve multi-host (None = local) : serialise des
    /// aujourd'hui pour que le format n'ait jamais a migrer.
    ExternalEdgeInsert {
        edge_id: u64,
        src: u32,
        edge_type: String,
        to_host: Option<String>,
        to_store: String,
        to_key: String,
    },
    /// Suppression d'une arete externe — pendant de ExternalEdgeInsert.
    ExternalEdgeDelete {
        edge_id: u64,
    },
    /// (2026-08-23, chantier SlotRemappable) Repermutation Hilbert V2 —
    /// remplace [`WalPayload::HilbertRepermute`] pour les nouvelles
    /// écritures. EN FIN D'ENUM, comme l'exige la règle bincode-par-indice.
    ///
    /// `epoch` est le numéro monotone de repermutation (persisté dans la
    /// zone nommée `substrate.remap.epoch` et dans les mots réservés des
    /// en-têtes de chaque side-car remappé — voir `crate::slot_remap`).
    /// Au rejeu :
    ///
    /// * zones Nodes/Edges — ré-appliquées seulement si l'epoch durable de
    ///   la base est < `epoch` (sinon la permutation a déjà été appliquée
    ///   et la ré-appliquer corromprait) ;
    /// * side-cars à en-tête (tier0, degrees, veccol, blobcol) — chacun
    ///   se garde par son propre epoch d'en-tête + CRC : `epoch en-tête`
    ///   supérieur ou égal à `epoch` ⇒ skip, CRC invalide ⇒ laissé
    ///   « absent » (fail-safe), sinon remap + estampille ;
    /// * zones engram — leurs états ABSOLUS post-remap voyagent DANS ce
    ///   record (`engram_members`, `engram_bitsets`) et sont appliqués
    ///   sous la même garde que la permutation des zones.
    ///
    /// ── POURQUOI DANS LE PAYLOAD, ET NON EN RECORDS SÉPARÉS ──
    ///
    /// La première version émettait des `EngramMembersSet` /
    /// `EngramBitsetSet` juste après ce record. Deux défauts, relevés en
    /// revue : ces records n'avaient AUCUNE garde d'epoch, donc un rejeu
    /// multi-générations les ré-appliquait en numérotation perm-1 dans un
    /// monde perm-2 ; et, étant des records adressant des slots, ils
    /// alimentaient le drapeau de détection du rejeu, ce qui pouvait
    /// déclencher une seconde permutation. Les porter ici supprime les
    /// deux : ils partagent exactement la garde de la permutation.
    HilbertRepermuteV2 {
        permutation: Vec<u32>,
        epoch: u32,
        /// Listes de membres post-remap, par engram modifié.
        engram_members: Vec<(u16, Vec<u32>)>,
        /// Signatures post-remap, par slot modifié.
        engram_bitsets: Vec<(u32, u64)>,
    },
    /// (2026-08-23, chantier SlotRemappable) Compaction par communauté V2 —
    /// remplace [`WalPayload::CompactCommunity`] pour les nouvelles
    /// écritures. Mêmes champs que la V1 plus `epoch` ; mêmes contrats de
    /// rejeu que la V1 pour Nodes/Edges/side-colonnes, et contrat
    /// side-cars identique à [`WalPayload::HilbertRepermuteV2`] (la
    /// permutation totale équivalente est reconstruite depuis
    /// `relocations` : identité + échanges `old ↔ new`).
    CompactCommunityV2 {
        community_id: u32,
        relocations: Vec<(u32, u32)>,
        page_range: (u32, u32),
        epoch: u32,
        /// Mêmes champs et même rationale que
        /// [`WalPayload::HilbertRepermuteV2`].
        engram_members: Vec<(u16, Vec<u32>)>,
        engram_bitsets: Vec<(u32, u64)>,
    },
    /// (2026-08-30, T4 simplicial — décision 2d461b89, spec 271180c3 I7)
    /// État ABSOLU d'un simplexe : le WAL porte l'intégralité du contenu
    /// (membres dans l'ordre stocké — canonique, ou d'émission sous
    /// ORDERED — et octets de propriétés), jamais un delta, si bien que le
    /// rejeu est idempotent et que les zones SimplexDir/SimplexIncidence +
    /// side-cars sont reconstructibles depuis le WAL seul — même invariant
    /// que [`WalPayload::EngramMembersSet`] (0x70).
    ///
    /// `simplex_id` adresse le slot du répertoire, comme `engram_id` chez
    /// 0x70 : le rejeu écrit au slot exact, sans re-dériver l'allocation.
    ///
    /// EN FIN D'ENUM, comme l'exige la règle bincode-par-indice gravée sur
    /// EdgeTypeIntern : toute variante future s'ajoute APRÈS.
    SimplexSet {
        simplex_id: u32,
        kind: u8,
        flags: u8,
        type_id: u16,
        members: Vec<u32>,
        props: Vec<u8>,
    },
    /// Tombstone d'un simplexe. Idempotent au rejeu (re-poser le flag est
    /// un no-op) ; jamais annulé — les slots ne sont pas réutilisés.
    SimplexTombstone {
        simplex_id: u32,
    },
    /// (2026-08-30, T5 passes) Configuration des passes de matérialisation
    /// simpliciales — état absolu des octets 16..26 de l'en-tête SPXD
    /// (voir [`crate::simplex_passes`]), si bien que le rejeu est
    /// idempotent et que la configuration survit à une reconstruction
    /// depuis le WAL seul (I7) : sans elle, la maintenance incrémentale au
    /// commit ne saurait pas quoi maintenir après réouverture.
    ///
    /// Les ids sont des ids de dictionnaire (types d'arêtes / clés de
    /// propriétés) ; `u16::MAX` = volet DISPATCH absent.
    ///
    /// EN FIN D'ENUM, comme l'exige la règle bincode-par-indice gravée
    /// sur EdgeTypeIntern : toute variante future s'ajoute APRÈS.
    SimplexPassesConfig {
        flags: u8,
        clique_edge_type: u16,
        clique_label_mask: u64,
        dispatch_edge_type: u16,
        dispatch_prop_key: u16,
        dispatch_he_type: u16,
    },
    /// (jalon 2 — RFC principal-natif-v2 §4-J2) `NodeInsert` portant le
    /// TENANT du noeud, emis par `Writer::write_node` depuis le format 3.
    /// L'ancienne variante `NodeInsert` reste decodable et rejouable (un
    /// WAL anterieur a la migration peut la contenir) : son rejeu ne touche
    /// PAS le tenant du record (zone vierge -> 0 = public, coherent avec la
    /// migration en bloc declaree).
    ///
    /// EN FIN D'ENUM, comme l'exige la regle bincode-par-indice gravee sur
    /// EdgeTypeIntern : toute variante future s'ajoute APRES.
    NodeInsertV3 {
        node_id: u32,
        label_bitset: u64,
        tenant_id: u32,
    },
    /// (jalon 2) `NodeUpdate` portant le tenant — meme contrat de rejeu
    /// que `NodeInsertV3`. EN FIN D'ENUM.
    NodeUpdateV3 {
        node_id: u32,
        label_bitset: u64,
        energy: u16,
        scar_util_affinity: u16,
        centrality_cached: u16,
        flags: u16,
        community_id: u32,
        tenant_id: u32,
    },
    /// (2026-11-01, D-DUR) Un nœud créé par une transaction encore ouverte.
    /// Voir [`WalKind::TxNoeudEnAttente`]. EN FIN D'ENUM.
    TxNoeudEnAttente {
        tx: u64,
        node_id: u32,
    },
    /// (2026-11-01, D-DUR) Une arête créée par une transaction encore ouverte.
    TxAreteEnAttente {
        tx: u64,
        edge_id: u64,
    },
    /// (2026-11-01, D-DUR) La transaction `tx` est COMMITÉE : tout ce qu'elle
    /// a déclaré en attente est publié. Porte `FLAG_COMMIT` (donc `fsync`
    /// sous `SyncMode::EveryCommit`). Écrit par `Session::commit()` via
    /// `GraphStoreMut::seal_commit`.
    TxCommit {
        tx: u64,
    },
}

impl WalPayload {
    pub fn kind(&self) -> WalKind {
        use WalKind as K;
        use WalPayload::*;
        match self {
            NodeInsert { .. } => K::NodeInsert,
            NodeUpdate { .. } => K::NodeUpdate,
            NodeDelete { .. } => K::NodeDelete,
            NodeInsertV3 { .. } => K::NodeInsertV3,
            NodeUpdateV3 { .. } => K::NodeUpdateV3,
            EdgeInsert { .. } => K::EdgeInsert,
            EdgeUpdate { .. } => K::EdgeUpdate,
            EdgeDelete { .. } => K::EdgeDelete,
            PropSet { .. } => K::PropSet,
            PropDelete { .. } => K::PropDelete,
            StringIntern { .. } => K::StringIntern,
            LabelIntern { .. } => K::LabelIntern,
            EdgeTypeIntern { .. } => K::EdgeTypeIntern,
            ExternalEdgeInsert { .. } => K::ExternalEdgeInsert,
            ExternalEdgeDelete { .. } => K::ExternalEdgeDelete,
            KeyIntern { .. } => K::KeyIntern,
            EnergyDecay { .. } => K::EnergyDecay,
            EnergyReinforce { .. } => K::EnergyReinforce,
            SynapseReinforce { .. } => K::SynapseReinforce,
            SynapseDecay { .. } => K::SynapseDecay,
            ScarUtilAffinitySet { .. } => K::ScarUtilAffinitySet,
            CentralityUpdate { .. } => K::CentralityUpdate,
            CommunityAssign { .. } => K::CommunityAssign,
            HilbertRepermute { .. } => K::HilbertRepermute,
            RicciUpdate { .. } => K::RicciUpdate,
            Tier0Update { .. } => K::Tier0Update,
            Tier1Update { .. } => K::Tier1Update,
            Tier2Update { .. } => K::Tier2Update,
            EngramMembersSet { .. } => K::EngramMembersSet,
            EngramBitsetSet { .. } => K::EngramBitsetSet,
            CoactDecay { .. } => K::CoactDecay,
            CompactCommunity { .. } => K::CompactCommunity,
            HilbertRepermuteV2 { .. } => K::HilbertRepermuteV2,
            CompactCommunityV2 { .. } => K::CompactCommunityV2,
            SimplexSet { .. } => K::SimplexSet,
            SimplexTombstone { .. } => K::SimplexTombstone,
            SimplexPassesConfig { .. } => K::SimplexPassesConfig,
            NodePropHeadUpdate { .. } => K::NodePropHeadUpdate,
            EdgePropHeadUpdate { .. } => K::EdgePropHeadUpdate,
            PropSetInline { .. } => K::PropSetInline,
            PropRemoveInline { .. } => K::PropRemoveInline,
            EdgePropSetInline { .. } => K::EdgePropSetInline,
            EdgePropRemoveInline { .. } => K::EdgePropRemoveInline,
            TxNoeudEnAttente { .. } => K::TxNoeudEnAttente,
            TxAreteEnAttente { .. } => K::TxAreteEnAttente,
            TxCommit { .. } => K::TxCommit,
            Checkpoint { .. } => K::Checkpoint,
            NoOp => K::NoOp,
            EndOfLog => K::EndOfLog,
        }
    }
}

/// Property value types stored in WAL records.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PropValue {
    Null,
    Bool(bool),
    I64(i64),
    F64(f64),
    StringRef { page_id: u32, offset: u32 },
    BytesRef { page_id: u32, offset: u32 },
}

// ---------------------------------------------------------------------------
// Framed record
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct WalRecord {
    pub lsn: u64,
    pub timestamp: i64,
    pub flags: u8,
    pub payload: WalPayload,
}

impl WalRecord {
    /// Mark this record as a transaction commit (`flags.bit(0)`).
    pub const FLAG_COMMIT: u8 = 1 << 0;
    /// Mark this record as a checkpoint boundary (`flags.bit(1)`).
    pub const FLAG_CHECKPOINT: u8 = 1 << 1;
    /// ── EXIGENCE DE SYNCHRONISATION (`flags.bit(2)`) — LE BIT LIBRE ──────────
    ///
    /// POURQUOI CE BIT EXISTE (group commit à needle de durabilité, 2026-10-06).
    /// Le `fsync` n'est plus indexé sur le LOT mais sur une NEEDLE (N records /
    /// 4 Mio / 2 ms, voir `wal_io::GroupCommit`). Conséquence directe : un
    /// appelant qui veut la durabilité AVANT le retour de son `append` ne peut
    /// plus l'exprimer — `FLAG_COMMIT` ne suffit plus à déclencher le `fsync`,
    /// puisque c'est précisément ce que la needle a remplacé. Ce bit le dit.
    ///
    /// ── IL N'EST PAS PERSISTÉ, ET C'EST DÉLIBÉRÉ ────────────────────────────
    /// L'octet `flags` écrit sur disque reste EXACTEMENT celui d'aujourd'hui :
    /// `flags_disque()` masque ce bit avant l'encodage. Aucune frame existante ne
    /// change d'un octet, aucun rejeu n'a de bit inconnu à ignorer, et le format
    /// reste bit-à-bit celui d'avant. Le bit est une DEMANDE adressée à
    /// l'écrivain — il est consommé par `WalWriter` (qui flushe alors tout de
    /// suite), pas par le format.
    pub const FLAG_SYNC_ATTENDU: u8 = 1 << 2;

    /// Encode the record to its on-disk byte representation.
    pub fn encode(&self) -> Result<Vec<u8>, SubstrateError> {
        let mut out = Vec::with_capacity(WAL_HEADER_SIZE + 64);
        self.encode_into(&mut out)?;
        Ok(out)
    }

    /// Encode the record into a caller-provided buffer (append-only).
    ///
    /// `out` is cleared then filled with the exact same bytes as
    /// [`WalRecord::encode`] — the on-disk format is unchanged (bincode
    /// payload + 28-octet header + CRC). The single allocation of `encode`
    /// (payload Vec + header Vec) disappears when the caller reuses `out`
    /// across records, which the hot append path does (thread-local buffer).
    /// Mesuré 2026-09-06 : 0,108 µs/record (encode, 2 allocs) → 0,005 µs
    /// (écriture directe) — ×21 sur l'encode seul, sans toucher au format.
    pub fn encode_into(&self, out: &mut Vec<u8>) -> Result<(), SubstrateError> {
        out.clear();
        self.encode_append_into(out)
    }

    /// Comme [`WalRecord::encode_into`], mais SANS vider `out` : la frame est
    /// ajoutée à la fin de ce qui s'y trouve déjà.
    ///
    /// ── POURQUOI CETTE VARIANTE EXISTE (group commit, T7) ───────────────────
    /// Le group commit doit poser N records dans UN SEUL `write`. Encoder chaque
    /// record dans son propre buffer puis concaténer ferait une copie de plus par
    /// record — exactement ce que `encode_into` avait supprimé (0,108 µs →
    /// 0,005 µs). Cette variante est le seul moyen d'encoder un LOT dans le
    /// buffer thread-local déjà réutilisé, sans copie intermédiaire.
    ///
    /// Les octets produits sont STRICTEMENT identiques à ceux de `encode_into`
    /// (même format : en-tête de 28 octets + payload bincode + CRC) — un lot de N
    /// records est, sur le disque, N frames consécutives indiscernables de N
    /// appends séparés.
    pub fn encode_append_into(&self, out: &mut Vec<u8>) -> Result<(), SubstrateError> {
        self.encode_append_with_lsn_into(out, self.lsn, self.flags_disque())
    }

    /// Comme [`WalRecord::encode_append_into`], mais avec un LSN et un octet de
    /// drapeaux FOURNIS par l'appelant.
    ///
    /// ── POURQUOI (pièce 2, 2026-10-06) ──────────────────────────────────────
    /// Le group commit estampille un LSN PRÉDIT (voir `wal_io::GroupCommit`) sans
    /// posséder le record : il lui faut donc encoder SANS écrire dans la copie du
    /// record. Passer par `let mut r = rec.clone()` aurait rendu à l'appelant les
    /// allocations que la pièce 2 supprime (payload cloné) — et un `Vec<WalRecord>`
    /// est exactement ce qu'on retire du chemin chaud. L'appelant fournit donc les
    /// deux seules valeurs qui dépendent de la position ; tout le reste (format,
    /// CRC, en-tête) est le code d'encodage UNIQUE, partagé avec `encode_into`.
    pub fn encode_append_with_lsn_into(
        &self,
        out: &mut Vec<u8>,
        lsn: u64,
        flags: u8,
    ) -> Result<(), SubstrateError> {
        let cfg = bincode_config();
        // ── LE BIT D'EXIGENCE EST MASQUÉ ICI, PAS AILLEURS ──────────────────
        // Le masquage est fait DANS cette fonction pour qu'AUCUN appelant ne
        // puisse le contourner (le chemin du group commit passe `rec.flags`
        // brut). L'octet `flags` sur disque reste donc celui d'avant, quel que
        // soit le chemin — c'est ce qui garantit que `FLAG_SYNC_ATTENDU` est une
        // demande adressée à l'écrivain, jamais un état du journal.
        let flags = flags & !Self::FLAG_SYNC_ATTENDU;
        encoder_frame(
            out,
            lsn,
            self.timestamp,
            flags,
            self.payload.kind() as u8,
            |o| {
                bincode::serde::encode_into_std_write(&self.payload, o, cfg)
                    .map(|_| ())
                    .map_err(|e| SubstrateError::WalEncode(e.to_string()))
            },
        )
    }

    /// Octet de drapeaux ÉCRIT SUR DISQUE : `FLAG_SYNC_ATTENDU` est une demande
    /// adressée à l'écrivain, jamais un état du journal (voir la doc du bit).
    fn flags_disque(&self) -> u8 {
        self.flags & !Self::FLAG_SYNC_ATTENDU
    }

    /// Ce record EXIGE-T-IL que son `append` soit durable avant de rendre la main ?
    pub fn exige_sync(&self) -> bool {
        self.flags & Self::FLAG_SYNC_ATTENDU != 0
    }

    /// Decode a record from `bytes`. Returns the decoded record and the number of bytes
    /// consumed.
    pub fn decode(bytes: &[u8]) -> Result<(Self, usize), SubstrateError> {
        if bytes.len() < WAL_HEADER_SIZE {
            return Err(SubstrateError::WalShortRead {
                needed: WAL_HEADER_SIZE,
                got: bytes.len(),
            });
        }
        let length = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let kind_byte = bytes[4];
        let flags = bytes[5];
        // bytes[6..8] reserved — check zero
        if bytes[6..8] != [0, 0] {
            return Err(SubstrateError::WalBadFrame(
                "reserved bytes at offset 6..8 are non-zero".into(),
            ));
        }
        let lsn = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
        let timestamp = i64::from_le_bytes(bytes[16..24].try_into().unwrap());
        let claimed_crc = u32::from_le_bytes(bytes[24..28].try_into().unwrap());

        let total = WAL_HEADER_SIZE + length;
        if bytes.len() < total {
            return Err(SubstrateError::WalShortRead {
                needed: total,
                got: bytes.len(),
            });
        }
        let payload_bytes = &bytes[WAL_HEADER_SIZE..total];
        let actual_crc = crc_over(&bytes[0..24], payload_bytes);
        if actual_crc != claimed_crc {
            return Err(SubstrateError::WalCrcMismatch {
                claimed: claimed_crc,
                actual: actual_crc,
            });
        }
        // Sanity check: kind byte matches payload decode.
        let _kind = WalKind::from_u8(kind_byte)
            .ok_or_else(|| SubstrateError::WalBadFrame(format!("unknown kind byte {kind_byte}")))?;

        let cfg = bincode_config();
        let (payload, _used): (WalPayload, usize) =
            bincode::serde::decode_from_slice(payload_bytes, cfg)
                .map_err(|e| SubstrateError::WalDecode(e.to_string()))?;

        Ok((
            Self {
                lsn,
                timestamp,
                flags,
                payload,
            },
            total,
        ))
    }

    pub fn is_commit(&self) -> bool {
        self.flags & Self::FLAG_COMMIT != 0
    }

    pub fn is_checkpoint(&self) -> bool {
        self.flags & Self::FLAG_CHECKPOINT != 0
    }
}

/// Compute CRC32 over `header_24` XOR `payload` (per wal-spec framing).
///
/// "XOR" here means the CRC is computed over the concatenation in a way that
/// tampering with either section changes the output. In practice we just feed
/// both slices to a standard streaming CRC32 — the "XOR" language in the spec
/// captures the design intent (cover both regions).
///
/// ── DEUX TRANCHES : CHEMINS DE LECTURE ET DE RÉ-ESTAMPILLAGE SEULEMENT ─────
/// Cette forme reste nécessaire là où l'en-tête et le payload ne sont PAS
/// contigus en mémoire : le `decode` (qui lit une frame déjà posée, avec son
/// champ CRC au milieu) et le ré-estampillage d'un lot encodé (voir
/// `wal_io::restampfer_lot`, chemin d'ANOMALIE). Le chemin d'ENCODAGE, lui, passe
/// par [`crc_une_passe`] — mesuré ×1,9 sur ce seul poste.
pub(crate) fn crc_over(header_24: &[u8], payload: &[u8]) -> u32 {
    let mut h = crc32fast::Hasher::new();
    h.update(header_24);
    h.update(payload);
    h.finalize()
}

/// ── CRC D'UNE FRAME EN UNE SEULE PASSE (pièce 3, 2026-10-06) ────────────────
///
/// MESURÉ : 41,2 ns/record quand le CRC est haché en DEUX `update` (24 o puis
/// payload) contre 22,1 ns/record en UNE passe — sur le même nombre d'octets. Le
/// coût n'est donc pas dans les octets hachés mais dans le second appel
/// (ré-amorçage du pipeline PCLMULQDQ).
///
/// La parade ne touche NI au CRC NI au format : la frame est encodée
/// `[en-tête 24 o][payload]` — contiguë, donc hachable d'un seul `update` — puis
/// le payload est DÉCALÉ de 4 octets pour loger le champ CRC. Le `memmove` qui
/// remplace le second appel est moins cher par octet que le CRC lui-même (copie
/// ≈ 2-3× plus rapide que PCLMULQDQ en cache chaud), donc le remplacement gagne
/// aussi sur les gros payloads, pas seulement sur les petits.
///
/// La valeur produite est IDENTIQUE à celle de [`crc_over`] (même entrée, même
/// fonction de hachage, même CRC32 IEEE) — ce que le test
/// `crc_une_passe_egale_le_crc_en_deux_tranches` vérifie sur des frames de
/// toutes tailles, et ce que les tests de corruption existants
/// (`tampered_header_fails_crc`, `tampered_payload_fails_crc`,
/// `reader_stops_at_crc_corruption`) gardent de bout en bout.
#[inline]
fn crc_une_passe(entree_contigue: &[u8]) -> u32 {
    let mut h = crc32fast::Hasher::new();
    h.update(entree_contigue);
    h.finalize()
}

/// ── ENCODAGE D'UNE FRAME — LE SEUL ENDROIT QUI CONNAÎT LE FORMAT ────────────
///
/// `[length u32][kind u8][flags u8][réservé 2][lsn u64][timestamp i64][crc u32]
/// [payload bincode]`, soit un en-tête de [`WAL_HEADER_SIZE`] = 28 octets.
///
/// POURQUOI UNE FONCTION PLUTÔT QU'UNE MÉTHODE. Deux producteurs de frames
/// coexistent : le payload POSSÉDÉ (`WalPayload`, via `WalRecord`) et le payload
/// EMPRUNTÉ (`PropSetInlineRef`, pièce 2 — zéro `String`/`Vec` par record). S'ils
/// écrivaient l'en-tête et le CRC chacun de leur côté, la moindre divergence
/// (ordre des champs, passe de CRC, largeur des entiers) produirait des frames
/// indécodables. Ici le format n'est écrit QU'UNE fois, et `ecrire_payload` ne
/// fournit que les octets du corps.
fn encoder_frame(
    out: &mut Vec<u8>,
    lsn: u64,
    timestamp: i64,
    flags: u8,
    kind: u8,
    ecrire_payload: impl FnOnce(&mut Vec<u8>) -> Result<(), SubstrateError>,
) -> Result<(), SubstrateError> {
    let debut = out.len();
    // ── 24 OCTETS D'EN-TÊTE, PAS 28 ─────────────────────────────────────────
    // Le champ CRC (24..28) n'existe pas encore : le payload se place donc JUSTE
    // après l'en-tête, sans le trou de 4 octets qui obligeait à hacher en deux
    // tranches. Le payload n'est écrit qu'UNE fois (`encode()` le copiait 2× :
    // payload Vec puis out).
    out.resize(debut + 24, 0);

    ecrire_payload(out)?;
    let payload_len = out.len() - debut - 24;
    let length = payload_len as u32;

    out[debut..debut + 4].copy_from_slice(&length.to_le_bytes()); // 0..4
    out[debut + 4] = kind; // 4
    out[debut + 5] = flags; // 5
    // 6..8 reserved (zéros déjà posés par resize)
    out[debut + 8..debut + 16].copy_from_slice(&lsn.to_le_bytes()); // 8..16
    out[debut + 16..debut + 24].copy_from_slice(&timestamp.to_le_bytes()); // 16..24

    // ── LE CRC, UNE SEULE PASSE, SUR UNE ENTRÉE CONTIGUË ────────────────────
    // À cet instant `[debut, debut + 24 + payload_len)` EST exactement l'entrée du
    // CRC (en-tête 24 o puis payload). C'est la passe mesurée à 22,1 ns/record.
    let crc = crc_une_passe(&out[debut..debut + 24 + payload_len]);

    // On ouvre les 4 octets du champ CRC (simple `resize` : le buffer de lot est
    // pré-réservé, donc aucun réalloc) et on y DÉCALE le payload d'un memmove.
    out.resize(debut + WAL_HEADER_SIZE + payload_len, 0);
    if payload_len > 0 {
        out.copy_within(
            debut + 24..debut + 24 + payload_len,
            debut + WAL_HEADER_SIZE,
        );
    }
    out[debut + 24..debut + 28].copy_from_slice(&crc.to_le_bytes()); // 24..28
    debug_assert_eq!(out.len(), debut + WAL_HEADER_SIZE + payload_len);
    Ok(())
}

/// Longueur `u64` LE puis contenu — la sérialisation bincode `standard()` +
/// `with_fixed_int_encoding()` d'un `String` ou d'un `Vec<u8>`.
#[inline]
fn ecrire_octets_bincode(out: &mut Vec<u8>, octets: &[u8]) {
    out.extend_from_slice(&(octets.len() as u64).to_le_bytes());
    out.extend_from_slice(octets);
}

// ---------------------------------------------------------------------------
// Payload emprunté — zéro allocation par record (pièce 2)
// ---------------------------------------------------------------------------

/// ── `PropSetInline` SANS `String` NI `Vec` : LA VARIANTE D'ENCODAGE EMPRUNTÉE ─
///
/// POURQUOI ELLE EXISTE (pièce 2, 2026-10-06). Construire un
/// `WalPayload::PropSetInline` coûte DEUX allocations par record
/// (`key: String`, `value_bincode: Vec<u8>`) — 46,4 ns/record mesurés sur le
/// chemin d'écriture de propriété. Or l'appelant a DÉJÀ la clé (`&str`) et les
/// octets de valeur (produits par le codec du store) : ces allocations ne servent
/// qu'à satisfaire le type POSSÉDÉ de l'enum, que l'encodage relit aussitôt.
///
/// ── LES OCTETS SONT IDENTIQUES, ET C'EST UN TEST QUI LE PROUVE ─────────────
/// Même index de variante serde (`INDICE_SERDE` = 32, figé par l'ordre de
/// déclaration — les variantes de `WalPayload` s'ajoutent en QUEUE, jamais
/// insérées), même `u32` pour `node_id`, même `u64` de longueur + octets pour la
/// clé et la valeur : `payload_emprunte_encode_les_memes_octets` compare les
/// frames des deux chemins octet par octet, sur plusieurs tailles. Un changement
/// d'ordre de l'enum le ferait échouer AVANT qu'un disque ne voie une frame
/// fausse — c'est le garde-fou du "wire-compat" de cet enum.
#[derive(Debug, Clone, Copy)]
pub struct PropSetInlineRef<'a> {
    pub node_id: u32,
    pub key: &'a str,
    pub value_bincode: &'a [u8],
}

impl PropSetInlineRef<'_> {
    /// Position de la variante `PropSetInline` dans `WalPayload`. Écrite à la
    /// main parce que serde n'expose pas l'index d'une variante à données ; la
    /// preuve qu'elle est juste est le test d'identité d'octets.
    pub const INDICE_SERDE: u32 = 32;

    /// Encode la frame complète (en-tête + payload + CRC) dans `out`, exactement
    /// comme le ferait un `WalRecord` portant le même payload possédé.
    pub fn encode_append_with_lsn_into(
        &self,
        out: &mut Vec<u8>,
        lsn: u64,
        timestamp: i64,
        flags: u8,
    ) -> Result<(), SubstrateError> {
        encoder_frame(
            out,
            lsn,
            timestamp,
            flags,
            WalKind::PropSetInline as u8,
            |o| {
                o.extend_from_slice(&Self::INDICE_SERDE.to_le_bytes());
                o.extend_from_slice(&self.node_id.to_le_bytes());
                ecrire_octets_bincode(o, self.key.as_bytes());
                ecrire_octets_bincode(o, self.value_bincode);
                Ok(())
            },
        )
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_rec(lsn: u64) -> WalRecord {
        WalRecord {
            lsn,
            timestamp: 1_700_000_000,
            flags: WalRecord::FLAG_COMMIT,
            payload: WalPayload::NodeInsert {
                node_id: 42,
                label_bitset: 0b1011,
            },
        }
    }

    /// (jalon 2) Les trames noeud V3 portent le tenant et font
    /// l'aller-retour ; l'ANCIENNE trame NodeInsert reste decodable a
    /// l'identique (regle bincode-par-indice : V3 ajoutees en fin d'enum).
    /// Les kinds sont distincts — le prefiltre resynchronisant du scan WAL
    /// discerne les deux generations.
    #[test]
    fn trames_noeud_v3_roundtrip_et_anciennes_stables() {
        let v3 = [
            WalPayload::NodeInsertV3 {
                node_id: 42,
                label_bitset: 0b1011,
                tenant_id: 7,
            },
            WalPayload::NodeUpdateV3 {
                node_id: 42,
                label_bitset: 0b1011,
                energy: 0x8000,
                scar_util_affinity: 3,
                centrality_cached: 9,
                flags: crate::record::node_flags::ALLOCATED,
                community_id: 5,
                tenant_id: u32::MAX,
            },
        ];
        for payload in v3 {
            let rec = WalRecord {
                lsn: 11,
                timestamp: 2,
                flags: 0,
                payload,
            };
            let bytes = rec.encode().unwrap();
            let (back, used) = WalRecord::decode(&bytes).unwrap();
            assert_eq!(used, bytes.len());
            assert_eq!(rec, back);
        }
        // Ancienne generation : toujours encodable/decodable (un WAL
        // anterieur a la migration 2->3 en contient).
        let ancien = sample_rec(4);
        let bytes = ancien.encode().unwrap();
        assert_eq!(bytes[4], WalKind::NodeInsert as u8);
        let (back, _) = WalRecord::decode(&bytes).unwrap();
        assert_eq!(ancien, back);
        // Kinds distincts, dans la famille noeud.
        assert_eq!(WalKind::NodeInsertV3 as u8, 0x04);
        assert_eq!(WalKind::NodeUpdateV3 as u8, 0x05);
        assert_eq!(WalKind::TxNoeudEnAttente as u8, 0x56);
        assert_eq!(WalKind::TxAreteEnAttente as u8, 0x57);
        assert_eq!(WalKind::TxCommit as u8, 0x58);
        assert_eq!(WalKind::from_u8(0x58), Some(WalKind::TxCommit));
    }

    /// Chantier SlotRemappable — les variantes V2 font l'aller-retour, et
    /// leur `epoch` survit à l'encodage.
    #[test]
    fn v2_repermutations_roundtrip() {
        for payload in [
            WalPayload::HilbertRepermuteV2 {
                permutation: vec![0, 3, 1, 2],
                epoch: 7,
                engram_members: vec![(1u16, vec![3u32, 1])],
                engram_bitsets: vec![(3u32, 0xF0u64)],
            },
            WalPayload::CompactCommunityV2 {
                community_id: 4,
                relocations: vec![(1, 64), (2, 65)],
                page_range: (1, 2),
                epoch: 9,
                engram_members: Vec::new(),
                engram_bitsets: vec![(64u32, 1u64)],
            },
        ] {
            let rec = WalRecord {
                lsn: 3,
                timestamp: 1,
                flags: 0,
                payload,
            };
            let bytes = rec.encode().unwrap();
            let (back, used) = WalRecord::decode(&bytes).unwrap();
            assert_eq!(used, bytes.len());
            assert_eq!(rec, back);
        }
    }

    /// RÈGLE DE SURVIE DU FORMAT : bincode/serde encode par INDICE de
    /// variante. Les variantes V2 ayant été ajoutées EN FIN D'ENUM, tout
    /// enregistrement antérieur doit rester décodable à l'identique — un
    /// ajout au milieu décalerait l'indice de toutes les suivantes et
    /// rendrait illisible chaque WAL déjà écrit.
    ///
    /// Ce test verrouille les indices en encodant une variante ANCIENNE
    /// (`HilbertRepermute` V1) et en vérifiant l'aller-retour, ainsi que
    /// la stabilité de l'octet de discriminant vis-à-vis d'un octet
    /// gravé en dur.
    #[test]
    fn les_variantes_anterieures_gardent_leur_indice() {
        let v1 = WalRecord {
            lsn: 1,
            timestamp: 1,
            flags: 0,
            payload: WalPayload::HilbertRepermute {
                permutation: vec![0, 2, 1],
            },
        };
        let bytes = v1.encode().unwrap();
        let (back, _) = WalRecord::decode(&bytes).unwrap();
        assert_eq!(
            v1, back,
            "un WAL V1 doit rester décodable après l'ajout des V2"
        );

        // `HilbertRepermute` est la 19e variante déclarée (indice 18) —
        // valeur GRAVÉE ICI : si ce test casse, c'est qu'une variante a
        // été insérée AVANT elle et que tous les WAL existants viennent
        // d'être rendus illisibles.
        let payload_bytes = bincode::serde::encode_to_vec(
            &WalPayload::HilbertRepermute {
                permutation: vec![0],
            },
            bincode_config(),
        )
        .unwrap();
        assert_eq!(
            payload_bytes[0], 18,
            "l'indice de variante de HilbertRepermute a bougé : \
             tout WAL deja ecrit devient illisible"
        );
    }

    #[test]
    fn encode_decode_roundtrip() {
        let rec = sample_rec(1);
        let bytes = rec.encode().unwrap();
        let (rec2, used) = WalRecord::decode(&bytes).unwrap();
        assert_eq!(used, bytes.len());
        assert_eq!(rec, rec2);
    }

    #[test]
    fn encode_into_bit_identical_to_encode() {
        // Le format disque ne doit pas changer : encode_into (buffer réutilisé)
        // produit EXACTEMENT les octets d'encode (1 alloc). Testé sur un
        // échantillon de variantes de payload.
        let payloads = [
            WalPayload::NodeInsert {
                node_id: 7,
                label_bitset: 0xDEAD,
            },
            WalPayload::NodeUpdate {
                node_id: 3,
                label_bitset: 1,
                energy: 2,
                scar_util_affinity: 3,
                centrality_cached: 4,
                flags: 5,
                community_id: 6,
            },
            WalPayload::KeyIntern {
                key_id: 99,
                name: "label".into(),
            },
            WalPayload::PropSet {
                node_id: 1,
                key_id: 4,
                value: PropValue::I64(7),
            },
            WalPayload::Checkpoint { at_lsn: 55 },
            WalPayload::NoOp,
        ];
        for payload in payloads {
            let rec = WalRecord {
                lsn: 123,
                timestamp: 1_700_000_000,
                flags: WalRecord::FLAG_COMMIT,
                payload,
            };
            let expected = rec.encode().unwrap();
            let mut buf = Vec::new();
            rec.encode_into(&mut buf).unwrap();
            assert_eq!(
                buf, expected,
                "encode_into diverge d'encode pour {:?}",
                rec.payload
            );
            // Buffer réutilisé : le second encode écrase proprement.
            buf.clear();
            let autre = WalRecord {
                lsn: 999,
                timestamp: 2,
                flags: 0,
                payload: WalPayload::NoOp,
            };
            autre.encode_into(&mut buf).unwrap();
            assert_eq!(buf, autre.encode().unwrap());
        }
    }

    #[test]
    fn encode_multiple_and_decode_sequence() {
        let mut buf = Vec::new();
        for i in 1..=5 {
            let r = sample_rec(i);
            buf.extend_from_slice(&r.encode().unwrap());
        }
        let mut cursor = 0;
        for i in 1..=5 {
            let (rec, used) = WalRecord::decode(&buf[cursor..]).unwrap();
            assert_eq!(rec.lsn, i);
            cursor += used;
        }
        assert_eq!(cursor, buf.len());
    }

    #[test]
    fn tampered_payload_fails_crc() {
        let rec = sample_rec(1);
        let mut bytes = rec.encode().unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        let err = WalRecord::decode(&bytes).unwrap_err();
        matches!(err, SubstrateError::WalCrcMismatch { .. });
    }

    #[test]
    fn tampered_header_fails_crc() {
        let rec = sample_rec(1);
        let mut bytes = rec.encode().unwrap();
        bytes[8] ^= 0xFF; // corrupt LSN byte
        let err = WalRecord::decode(&bytes).unwrap_err();
        matches!(err, SubstrateError::WalCrcMismatch { .. });
    }

    #[test]
    fn short_tail_returns_short_read() {
        let rec = sample_rec(1);
        let bytes = rec.encode().unwrap();
        let truncated = &bytes[..bytes.len() - 3];
        let err = WalRecord::decode(truncated).unwrap_err();
        matches!(err, SubstrateError::WalShortRead { .. });
    }

    #[test]
    fn all_kinds_roundtrip() {
        // Smoke test: every WalPayload variant encodes and decodes identically.
        let payloads = vec![
            WalPayload::NodeInsert {
                node_id: 1,
                label_bitset: 0xAAAA_BBBB_CCCC_DDDD,
            },
            WalPayload::NodeUpdate {
                node_id: 2,
                label_bitset: 1,
                energy: 0x8000,
                scar_util_affinity: 0x1234,
                centrality_cached: 0xFFFF,
                flags: 0x0001,
                community_id: 7,
            },
            WalPayload::NodeDelete { node_id: 3 },
            WalPayload::EdgeInsert {
                edge_id: 0xDEAD_BEEF,
                src: 10,
                dst: 20,
                edge_type: 5,
                weight_u16: 0x4000,
            },
            WalPayload::EdgeUpdate {
                edge_id: 99,
                weight_u16: 0xFFFF,
                ricci_u8: 128,
                flags: 0x02,
                engram_tag: 17,
            },
            WalPayload::EdgeDelete { edge_id: 100 },
            WalPayload::PropSet {
                node_id: 1,
                key_id: 5,
                value: PropValue::I64(-42),
            },
            WalPayload::PropSet {
                node_id: 1,
                key_id: 5,
                value: PropValue::StringRef {
                    page_id: 2,
                    offset: 128,
                },
            },
            WalPayload::PropDelete {
                node_id: 1,
                key_id: 5,
            },
            WalPayload::StringIntern {
                page_id: 0,
                offset: 16,
                bytes: b"hello-world".to_vec(),
            },
            WalPayload::LabelIntern {
                label_id: 1,
                name: "Person".into(),
            },
            WalPayload::KeyIntern {
                key_id: 10,
                name: "age".into(),
            },
            WalPayload::EnergyDecay { factor_q16: 32768 },
            WalPayload::EnergyReinforce {
                node_id: 1,
                new_energy: 0x7FFF,
            },
            WalPayload::SynapseReinforce {
                edge_id: 7,
                new_weight: 0xC000,
            },
            WalPayload::SynapseDecay { factor_q16: 65000 },
            WalPayload::ScarUtilAffinitySet {
                node_id: 1,
                packed: 0xABCD,
            },
            WalPayload::CentralityUpdate {
                updates: vec![(1, 100), (2, 200)],
            },
            WalPayload::CommunityAssign {
                updates: vec![(1, 7), (2, 7), (3, 11)],
            },
            WalPayload::HilbertRepermute {
                permutation: vec![3, 1, 2, 0],
            },
            WalPayload::RicciUpdate {
                updates: vec![(1, 64), (2, 192)],
            },
            WalPayload::Tier0Update {
                node_id: 42,
                bits: [0xDEAD_BEEF, 0xCAFE_BABE],
            },
            WalPayload::Tier1Update {
                node_id: 42,
                bits: [1, 2, 3, 4, 5, 6, 7, 8],
            },
            WalPayload::Tier2Update {
                node_id: 42,
                f16s: vec![0x3C00; 384], // f16 1.0
            },
            WalPayload::EngramMembersSet {
                engram_id: 7,
                members: vec![10, 20, 30, 40, 50],
            },
            WalPayload::EngramBitsetSet {
                node_id: 42,
                bitset: 0xAAAA_BBBB_CCCC_DDDD,
            },
            WalPayload::SimplexSet {
                simplex_id: 7,
                kind: 255,
                flags: 0b100,
                type_id: 3,
                members: vec![10, 20, 30, 40],
                props: b"{\"line\":42}".to_vec(),
            },
            WalPayload::SimplexTombstone { simplex_id: 7 },
            WalPayload::Checkpoint { at_lsn: 123 },
            WalPayload::NoOp,
            WalPayload::EndOfLog,
            WalPayload::CoactDecay {
                factor_q16: 64880,
                coact_type_id: 7,
            },
            WalPayload::CompactCommunity {
                community_id: 3,
                relocations: vec![(100, 256), (101, 257), (102, 258)],
                page_range: (2, 4),
            },
            WalPayload::NodePropHeadUpdate {
                node_id: 42,
                first_prop_off: (1u64 << 48) - 1, // max U48 value
            },
            WalPayload::NodePropHeadUpdate {
                node_id: 0,
                first_prop_off: 0,
            },
        ];
        for (i, p) in payloads.into_iter().enumerate() {
            let rec = WalRecord {
                lsn: i as u64 + 1,
                timestamp: 0,
                flags: 0,
                payload: p,
            };
            let bytes = rec.encode().unwrap();
            let (rec2, _) = WalRecord::decode(&bytes).unwrap();
            assert_eq!(rec, rec2, "roundtrip failed at index {i}");
        }
    }

    // =======================================================================
    // PIÈCE 3 (2026-10-06) — LE CRC EN UNE PASSE CHANGE-T-IL LES OCTETS ?
    // =======================================================================
    //
    // NON, et ce test le PROUVE au lieu de l'affirmer : il calcule la valeur par
    // les deux chemins sur les mêmes octets. `crc_over` (deux tranches) reste la
    // référence — c'est LUI que le `decode` utilise pour valider une frame. Si
    // l'encodage en une passe produisait un autre CRC, les frames deviendraient
    // INDÉCODABLES (le rejeu s'arrêterait à la première) : ce test est donc le
    // garde-fou du format, pas un détail d'implémentation.
    #[test]
    fn crc_une_passe_egale_le_crc_en_deux_tranches() {
        for taille_payload in [
            0usize, 1, 2, 3, 4, 7, 15, 16, 17, 63, 64, 65, 255, 4096, 100_000,
        ] {
            let mut frame = Vec::new();
            frame.extend_from_slice(&[0xA5u8; 24]); // en-tête
            frame.extend_from_slice(&vec![0x5Au8; taille_payload]); // payload contigu
            let une = crc_une_passe(&frame);
            let deux = crc_over(&frame[..24], &frame[24..]);
            assert_eq!(
                une, deux,
                "CRC divergent pour un payload de {taille_payload} o : \
                 l'encodage en une passe n'écrit plus le même disque"
            );

            // Et le CRC doit être STABLE vis-à-vis du découpage : hacher en 3
            // morceaux quelconques donne la même valeur (propriété du CRC
            // streaming, dont dépend le `decode`).
            let mut h = crc32fast::Hasher::new();
            h.update(&frame[..3]);
            h.update(&frame[3..17]);
            h.update(&frame[17..]);
            assert_eq!(une, h.finalize());
        }
    }

    // =======================================================================
    // PIÈCE 2 (2026-10-06) — LE PAYLOAD EMPRUNTÉ ENCODE-T-IL LES MÊMES OCTETS ?
    // =======================================================================
    //
    // C'EST LE TEST QUI AUTORISE L'EXISTENCE DE `PropSetInlineRef`. L'encodage
    // emprunté écrit l'index de variante serde À LA MAIN (32) : se tromper d'un
    // seul octet produirait une frame dont le payload ne se décode pas — un WAL
    // corrompu, écrit par un chemin qui n'alloue rien et ne se plaint de rien.
    // On compare donc les DEUX frames octet par octet, en-tête (LSN, timestamp,
    // flags, kind, longueur) ET payload ET CRC compris.
    #[test]
    fn payload_emprunte_encode_les_memes_octets() {
        for (node_id, key, valeur) in [
            (0u32, "", Vec::new()),
            (7u32, "k", vec![1u8, 2, 3]),
            (
                4_294_967_295u32,
                "une-cle-un-peu-plus-longue",
                vec![0u8; 300],
            ),
            (42u32, "clé-utf8-éàü", (0..255u8).collect::<Vec<u8>>()),
        ] {
            let possede = WalRecord {
                lsn: 1234,
                timestamp: -1_700_000_000_000,
                flags: WalRecord::FLAG_COMMIT,
                payload: WalPayload::PropSetInline {
                    node_id,
                    key: key.to_string(),
                    value_bincode: valeur.clone(),
                },
            };
            let attendu = possede.encode().unwrap();

            let emprunte = PropSetInlineRef {
                node_id,
                key,
                value_bincode: &valeur,
            };
            let mut obtenu = Vec::new();
            emprunte
                .encode_append_with_lsn_into(
                    &mut obtenu,
                    possede.lsn,
                    possede.timestamp,
                    possede.flags,
                )
                .unwrap();

            assert_eq!(
                attendu, obtenu,
                "les deux encodages divergent (node_id={node_id}, clé={key:?}) — \
                 l'index serde de PropSetInline a-t-il bougé dans WalPayload ?"
            );
            // Et la frame empruntée se DÉCODE comme la possédée : la preuve n'est
            // pas seulement une égalité d'octets, c'est un aller-retour réussi.
            let (relu, utilises) = WalRecord::decode(&obtenu).unwrap();
            assert_eq!(relu, possede);
            assert_eq!(utilises, obtenu.len());
        }
    }

    /// L'index serde écrit à la main est celui que bincode lit vraiment dans
    /// l'enum : on l'extrait des octets d'une frame produite par le chemin
    /// POSSÉDÉ (premier `u32` du payload, juste après l'en-tête de 28 octets).
    #[test]
    fn indice_serde_de_prop_set_inline_est_celui_de_l_enum() {
        let rec = WalRecord {
            lsn: 1,
            timestamp: 2,
            flags: 0,
            payload: WalPayload::PropSetInline {
                node_id: 0,
                key: String::new(),
                value_bincode: Vec::new(),
            },
        };
        let b = rec.encode().unwrap();
        let lu = u32::from_le_bytes(b[WAL_HEADER_SIZE..WAL_HEADER_SIZE + 4].try_into().unwrap());
        assert_eq!(
            lu,
            PropSetInlineRef::INDICE_SERDE,
            "l'index serde de PropSetInline a changé : une variante a été INSÉRÉE \
             dans WalPayload avant lui. Les WAL existants ne se relisent plus — \
             c'est exactement ce que la règle « ajouter en QUEUE » interdit."
        );
        assert_eq!(WalKind::from_u8(b[4]), Some(WalKind::PropSetInline));
    }

    /// Le bit d'exigence de synchronisation est une DEMANDE : il ne doit PAS
    /// atteindre le disque (l'octet `flags` de la frame reste celui d'aujourd'hui,
    /// sinon toute frame « WAIT » changerait de format pour un lecteur ancien).
    #[test]
    fn flag_sync_attendu_n_est_pas_persiste() {
        let base = WalRecord {
            lsn: 9,
            timestamp: 3,
            flags: WalRecord::FLAG_COMMIT,
            payload: WalPayload::NoOp,
        };
        let demandeur = WalRecord {
            flags: WalRecord::FLAG_COMMIT | WalRecord::FLAG_SYNC_ATTENDU,
            ..base.clone()
        };
        assert!(demandeur.exige_sync());
        assert!(!base.exige_sync());
        assert_eq!(
            base.encode().unwrap(),
            demandeur.encode().unwrap(),
            "le bit WAIT a fui sur le disque : le format n'est plus celui d'avant"
        );
        // Le bit est bien présent dans la valeur EN MÉMOIRE du record, lui.
        assert_eq!(demandeur.flags, WalRecord::FLAG_COMMIT | 0b100);
    }
}
