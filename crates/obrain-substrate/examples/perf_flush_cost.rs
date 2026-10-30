//! Le coût d'un `flush()` dépend-il de la TAILLE DE L'ARÈNE ou du VOLUME ÉCRIT ?
//!
//! Hypothèse (2026-08-17) : `BlobColumnWriter::sync` (blob_column.rs:411-450)
//! recalcule `arena_crc32` sur `dat[..arena_end]` À CHAQUE APPEL — donc relit
//! l'intégralité de l'arène, même si rien n'a changé depuis le dernier flush.
//! Or `SubstrateStore::flush` appelle `blob_columns.sync_all()`, et obrain-tmn
//! appelle `checkpoint_all()` toutes les 60 s sur jusqu'à 24 bases.
//!
//! En production : ~36,5 Go d'arènes vives sur la base `agent`, re-CRC toutes
//! les 60 s ⇒ ~608 Mo/s de lecture disque permanente, sous le verrou
//! d'écriture de chaque colonne (donc TOUTE lecture de cette colonne bloque).
//! Mesuré sur le service : 872 Mo/s soutenus, read_bytes ≫ rchar.
//!
//! Ce banc écrit une arène de taille croissante, puis mesure le coût d'un
//! flush qui ne modifie QU'UN SEUL octet de payload.
//!
//! Lecture du résultat :
//!   - coût de flush PROPORTIONNEL à l'arène → hypothèse confirmée, le CRC
//!     doit devenir incrémental (l'arène est append-only : `crc32_combine`).
//!   - coût de flush proportionnel au volume écrit → hypothèse fausse.

use obrain_substrate::blob_column::{BlobColSpec, BlobColumnWriter};
use obrain_substrate::file::SubstrateFile;
use obrain_substrate::vec_column::EntityKind;
use web_time::Instant;

fn main() {
    println!("=== coût d'un flush vs taille de l'arène ===\n");
    println!(
        "{:>12}  {:>12}  {:>14}  {:>16}",
        "arène", "slots", "flush no-op", "débit implicite"
    );
    println!("{}", "-".repeat(62));

    // Payload de 64 Ko : représentatif d'un gros état singleton (AgentSelfState,
    // ChainStoreSnapshot) — c'est ce qui atterrit dans une colonne blob.
    let payload = vec![0xABu8; 64 * 1024];

    for &n_slots in &[64usize, 256, 1024, 4096, 16384] {
        let sub = SubstrateFile::open_tempfile().expect("tempfile");
        let spec = BlobColSpec {
            prop_key_id: 7,
            entity_kind: EntityKind::Node,
        };
        let mut w = BlobColumnWriter::create(&sub, spec).expect("create");

        for slot in 0..n_slots {
            w.write_slot(slot as u32, &payload).expect("write");
        }
        w.sync().expect("sync initial");

        let arena = w.arena_end();

        // Un flush qui ne change QU'UN slot : le travail utile est de 64 Ko.
        // Tout le reste du coût est du re-calcul pur.
        let mut samples = Vec::new();
        for i in 0..5 {
            let mut p = payload.clone();
            p[0] = i as u8;
            w.write_slot(0, &p).expect("rewrite");
            let t = Instant::now();
            w.sync().expect("sync");
            samples.push(t.elapsed().as_secs_f64());
        }
        samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let med = samples[samples.len() / 2];

        println!(
            "{:>9.1} Mo  {n_slots:>12}  {:>11.2} ms  {:>13.0} Mo/s",
            arena as f64 / 1_048_576.0,
            med * 1000.0,
            (arena as f64 / 1_048_576.0) / med.max(1e-9),
        );
    }

    println!("\nVERDICT :");
    println!("  « flush no-op » qui CROÎT avec l'arène → le CRC est recalculé");
    println!("  intégralement à chaque flush. Le « débit implicite » est alors");
    println!("  la bande passante de relecture imposée en permanence par le");
    println!("  checkpoint périodique — à multiplier par le nombre de colonnes,");
    println!("  et à diviser par l'intervalle de checkpoint (60 s en prod).");
}
