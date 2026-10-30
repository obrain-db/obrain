//! `wal_merkle` — chaîne de racines Merkle sur le WAL substrate.
//!
//! Chantier « racines on-chain » (obrain → aletheia) : la chaîne L1 ne
//! stocke pas le graphe — elle engage `root@lsn` par bloc (l'ordre et la
//! preuve sans le contenu, pattern `Engages`/`ArtifactRegistry` de la
//! spec aletheia). Le WAL substrate est le flux de deltas NATUREL :
//! append-only, LSN monotone, CRC par record, rejeu déterministe — deux
//! nœuds qui rejouent les mêmes records calculent la même racine.
//!
//! Chaîne : `root_i = SHA256(root_{i-1} ‖ record_i_bytes)` (encode
//! canonique, bincode fixed-int — stable inter-plateformes). Les records
//! marqués CHECKPOINT émettent un point d'ancrage `(lsn, root)`.
//!
//! Usage : `cargo run --release -p obrain-examples --bin wal-merkle -- <store-path>`

use sha2::{Digest, Sha256};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: wal-merkle <store-path>")?;
    let wal_path = std::path::Path::new(&path).join("substrate.wal");
    let wal = obrain_substrate::wal_io::WalReader::open(&wal_path)?;

    let mut root = [0u8; 32];
    let mut count: u64 = 0;
    let mut last_lsn: u64 = 0;
    let mut checkpoints: Vec<(u64, String)> = Vec::new();

    for item in wal.iter_from(0) {
        let (rec, _off, _len) = item?;
        if rec.lsn > last_lsn { last_lsn = rec.lsn; }
        count += 1;
        let bytes = rec.encode()?;
        let mut hasher = Sha256::new();
        hasher.update(root);
        hasher.update(&bytes);
        root = hasher.finalize().into();
        if rec.is_checkpoint() {
            checkpoints.push((rec.lsn, hex(&root)));
        }
    }

    println!("store: {}", path);
    println!("records: {}", count);
    println!("last_lsn: {}", last_lsn);
    println!("root@lsn: {}", hex(&root));
    println!("checkpoints: {}", checkpoints.len());
    for (lsn, r) in checkpoints.iter().take(8) {
        println!("  checkpoint root@{} = {}", lsn, r);
    }
    Ok(())
}

fn hex(b: &[u8; 32]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
