//! `substrate.meta` — 4 KiB file header + dictionary section.
//!
//! See `docs/rfc/substrate/format-spec.md` §2 and §9.

use bytemuck::{Pod, Zeroable};

pub const SUBSTRATE_MAGIC: [u8; 8] = *b"SUBSTRT\0";
/// v2 (jalon 1, RFC principal-natif-v2 plan A) : `node_flags::ALLOCATED`
/// obligatoire sur tout record de noeud reellement ecrit ; les creneaux de
/// bourrage jamais ecrits ne le portent pas. Layout 32 B INCHANGE — le bit
/// loge dans le u16 `flags` existant. Migration 1→2 : `migration::table()`.
///
/// v3 (jalon 2, RFC principal-natif-v2 §4-J2) : `NodeRecord` 32 → 40 octets
/// — colonne `tenant_id: u32` + `_reserve: u32` en QUEUE de record (les
/// offsets v2 ne bougent pas). Page d'allocation 20 480 o = 512 records
/// exactement. La zone nodes v3 porte un marqueur de stride dans son slot 0
/// ([`crate::record::NODES_ZONE_V3_MAGIC`]). Migration 2→3 : reecriture de
/// la zone nodes vers un fichier temporaire + rename atomique, tenant 0
/// (public DECLARE : [`meta_flags::MONO_TENANT_PUBLIC`]) sur chaque record.
pub const SUBSTRATE_FORMAT_VERSION: u32 = 3;
pub const META_HEADER_SIZE: usize = 96;
pub const META_FILE_SIZE: usize = 4096;

pub mod meta_flags {
    pub const COGNITIVE_ACTIVE: u32 = 1 << 0;
    pub const HILBERT_SORTED: u32 = 1 << 1;
    /// (jalon 2 — RFC principal-natif-v2 §5.2 / A5) Ce store a ETE DECLARE
    /// mono-tenant public : `tenant_id = 0` y est un choix d'exploitation,
    /// pas un defaut subi. Pose par la migration en bloc 2→3 (qui met tout
    /// noeud existant a 0) — jamais implicite. Un store multi-tenant ne
    /// portera pas ce drapeau ; ses sessions sans Principal obtiendront la
    /// portee VIDE au jalon 4 (fail-closed).
    pub const MONO_TENANT_PUBLIC: u32 = 1 << 2;
}

/// Fixed-size header portion of `substrate.meta` (96 B).
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct MetaHeader {
    pub magic: [u8; 8],
    pub format_version: u32,
    pub flags: u32,
    pub node_count: u64,
    pub edge_count: u64,
    pub property_page_count: u64,
    pub string_heap_size: u64,
    pub created_at: i64,
    pub last_checkpoint: i64,
    pub last_wal_offset: u64,
    pub tier0_dim_bits: u32,
    pub tier1_dim_bits: u32,
    pub tier2_dim: u32,
    pub hilbert_order: u32,
    pub schema_crc32: u32,
    pub embedding_matrix_seed: u32,
}

const _: [(); 1] = [(); (core::mem::size_of::<MetaHeader>() == META_HEADER_SIZE) as usize];

impl MetaHeader {
    pub fn new_blank() -> Self {
        Self {
            magic: SUBSTRATE_MAGIC,
            format_version: SUBSTRATE_FORMAT_VERSION,
            flags: 0,
            node_count: 0,
            edge_count: 0,
            property_page_count: 0,
            string_heap_size: 0,
            created_at: now_unix_seconds(),
            last_checkpoint: 0,
            last_wal_offset: 0,
            tier0_dim_bits: 128,
            tier1_dim_bits: 512,
            tier2_dim: 384,
            hilbert_order: 0,
            schema_crc32: 0,
            embedding_matrix_seed: 0,
        }
    }

    pub fn is_valid_magic(&self) -> bool {
        self.magic == SUBSTRATE_MAGIC
    }
}

fn now_unix_seconds() -> i64 {
    // wasm32 (`wasm32-unknown-unknown`, la seule cible wasm qu'Obrain vise)
    // n'a AUCUNE horloge : `SystemTime::now()` y panique — « time not
    // implemented on this platform » — et cette panique tombait dès la
    // CONSTRUCTION du store. `ObrainDB::new_in_memory()` ne rendait donc
    // jamais la main en WASM.
    //
    // Zéro est ici la réponse HONNÊTE, pas un substitut : `created_at` n'est
    // jamais relu (déclaration, initialisation, valeur de test — rien
    // d'autre), il documente la date de création du store pour un lecteur
    // humain ou un outil externe. Aucune décision du substrat n'en dépend —
    // l'ordre du WAL vient du `lsn`. Le jour où un lecteur en dépendra, c'est
    // CE commentaire qu'il faudra démentir, et une horloge qu'il faudra
    // fournir (l'hôte JS en a une : `Date.now()`).
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
    #[cfg(target_arch = "wasm32")]
    {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_header_layout() {
        assert_eq!(core::mem::size_of::<MetaHeader>(), 96);
    }

    #[test]
    fn meta_header_blank() {
        let m = MetaHeader::new_blank();
        assert!(m.is_valid_magic());
        assert_eq!(m.format_version, SUBSTRATE_FORMAT_VERSION);
        assert_eq!(m.tier0_dim_bits, 128);
        assert_eq!(m.tier1_dim_bits, 512);
        assert_eq!(m.tier2_dim, 384);
    }

    #[test]
    fn meta_header_pod_roundtrip() {
        let m = MetaHeader {
            magic: SUBSTRATE_MAGIC,
            format_version: SUBSTRATE_FORMAT_VERSION,
            flags: meta_flags::COGNITIVE_ACTIVE,
            node_count: 12_345,
            edge_count: 67_890,
            property_page_count: 111,
            string_heap_size: 2048,
            created_at: 1_700_000_000,
            last_checkpoint: 1_700_000_042,
            last_wal_offset: 123_456,
            tier0_dim_bits: 128,
            tier1_dim_bits: 512,
            tier2_dim: 384,
            hilbert_order: 1,
            schema_crc32: 0xDEAD_BEEF,
            embedding_matrix_seed: 42,
        };
        let bytes: &[u8] = bytemuck::bytes_of(&m);
        assert_eq!(bytes.len(), 96);
        let m2: MetaHeader = *bytemuck::from_bytes::<MetaHeader>(bytes);
        assert_eq!(m, m2);
    }
}
