//! ── BANC — CHECKPOINT ÉLU : FENCE + TRONCATURE + CHEMIN COMPLET (2026-10-06) ──
//!
//! Décompose le coût du checkpoint élu en postes MESURÉS séparément :
//!   0. **fsync unitaire** : l'unité de coût (sync_all d'un fichier de 32 o).
//!   1. **fence**   : `ElectionCheckpoint::tenter`/`drop` (open + flock LOCK_EX|NB).
//!   2. **truncate** : `WalWriter::truncate` (ftruncate + publier_ancre_sur = 2 fsync).
//!   3. **checkpoint_elu** : chemin complet (flush zones + meta + truncate + marqueur).
//!
//! Usage :
//!   cargo run --release -p obrain-substrate --example bench_checkpoint_elu
//!   CHECKPOINT_DIR=/tmp/obrain-bench-checkpoint.obrain cargo run ... (défaut)
//!
//! Chaque mesure tourne SÉQUENTIELLEMENT (aucun banc lourd en parallèle) et rend
//! la MÉDIANE, pas une moyenne (l'fsync a une queue longue).

use obrain_substrate::checkpoint::{CheckpointOutcome, checkpoint_elu};
use obrain_substrate::record::{NodeRecord, PackedScarUtilAff, U48, f32_to_q1_15};
use obrain_substrate::wal::{WalPayload, WalRecord};
use obrain_substrate::wal_io::{ANCRE_INITIALE, ElectionCheckpoint, SyncMode, WalWriter};
use obrain_substrate::{SubstrateFile, Writer};
use std::path::PathBuf;
use web_time::Instant;

fn mediane(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn node(i: u32) -> NodeRecord {
    NodeRecord {
        label_bitset: i as u64 + 1,
        first_edge_off: U48::default(),
        first_prop_off: U48::default(),
        community_id: 0,
        energy: f32_to_q1_15(0.5),
        scar_util_affinity: PackedScarUtilAff::new(0, 0, 0, false).pack(),
        centrality_cached: 0,
        flags: 0,
        tenant_id: 0,
        _reserve: 0,
    }
}

fn rec(seq: u64) -> WalRecord {
    WalRecord {
        lsn: 0,
        timestamp: 0,
        flags: WalRecord::FLAG_COMMIT,
        payload: WalPayload::PropSetInline {
            node_id: seq as u32,
            key: format!("k{}", seq % 64),
            value_bincode: (seq as i64).to_le_bytes().to_vec(),
        },
    }
}

fn base() -> PathBuf {
    std::env::var("CHECKPOINT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp/obrain-bench-checkpoint.obrain"))
}

/// Étend un WAL (SyncMode::Never, aucun fsync) jusqu'à `cible` octets.
fn gonfler(w: &WalWriter, cible: u64) {
    let mut seq = 0u64;
    while w.offset() < cible {
        w.append(rec(seq)).unwrap();
        seq += 1;
    }
}

fn main() {
    let root = base();
    std::fs::create_dir_all(&root).expect("créer le répertoire de banc");
    println!("=== CHECKPOINT ÉLU — fence / truncate / chemin complet ===");
    println!("répertoire : {}", root.display());

    // ── 0. ÉTALON : un sync_all (l'unité de coût) ───────────────────────────
    {
        let etalon_path = root.join("etalon.bin");
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&etalon_path)
            .unwrap();
        f.write_all(&[0u8; 32]).unwrap();
        let mut s = Vec::with_capacity(200);
        for _ in 0..200 {
            let t = Instant::now();
            f.sync_all().unwrap();
            s.push(t.elapsed().as_secs_f64() * 1e6);
        }
        println!("[0] fsync unitaire (sync_all 32 o)      : {:>8.1} µs", mediane(s));
    }

    // ── 1. FENCE : élection flock ───────────────────────────────────────────
    {
        let fence_wal = root.join("fence.wal");
        let mut s = Vec::with_capacity(20_000);
        for _ in 0..20_000 {
            let t = Instant::now();
            let e = ElectionCheckpoint::tenter(&fence_wal).expect("élu");
            drop(e);
            s.push(t.elapsed().as_secs_f64() * 1e6);
        }
        println!("[1] fence élection (tenter+drop)       : {:>8.2} µs", mediane(s));
    }

    // ── 2. TRONCATURE : WalWriter::truncate selon la taille du WAL ──────────
    println!("[2] truncate() selon la taille du WAL (médiane, N=30) :");
    for taille in [0u64, 8 * 1024, 1024 * 1024, 8 * 1024 * 1024] {
        let wal_path = root.join(format!("trunc-{}.wal", taille));
        let _ = std::fs::remove_file(&wal_path);
        let _ = std::fs::remove_file(root.join(format!("trunc-{}.wal.lsn", taille)));
        let w = WalWriter::open(&wal_path, SyncMode::Never, ANCRE_INITIALE).unwrap();
        gonfler(&w, taille);
        let mut s = Vec::with_capacity(30);
        for _ in 0..30 {
            let t = Instant::now();
            w.truncate().unwrap();
            s.push(t.elapsed().as_secs_f64() * 1e6);
            gonfler(&w, taille); // hors chrono : re-gonfler pour l'échantillon suivant
        }
        let ko = taille as f64 / 1024.0;
        println!("    WAL {:>7.1} Ko  ->  truncate {:>8.1} µs", ko, mediane(s));
    }

    // ── 3. CHECKPOINT ÉLU complet : coût vs volume écrit (nœuds) ────────────
    println!("[3] checkpoint_elu complet selon le volume écrit (médiane, N=12) :");
    for n_noeuds in [100u32, 2_000, 20_000] {
        let mut s = Vec::with_capacity(12);
        let mut fait = 0usize;
        let mut non_elu = 0usize;
        let mut offset_avant = 0u64;
        for rep in 0..12 {
            let dir = root.join(format!("kb-{}-{}", n_noeuds, rep));
            let _ = std::fs::remove_dir_all(&dir);
            let sub = SubstrateFile::create(&dir).unwrap();
            let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();
            for i in 0..n_noeuds {
                w.write_node(i, node(i)).unwrap();
            }
            w.commit().unwrap();
            let wal_writer = w.wal();
            offset_avant = wal_writer.offset();
            drop(w);
            let mut sub2 = SubstrateFile::open(&dir).unwrap();
            let t = Instant::now();
            match checkpoint_elu(&mut sub2, &wal_writer).unwrap() {
                CheckpointOutcome::Fait(_) => fait += 1,
                CheckpointOutcome::NonElu { .. } => non_elu += 1,
            }
            s.push(t.elapsed().as_secs_f64() * 1e6);
            drop(sub2);
            drop(wal_writer);
        }
        println!(
            "    {n_noeuds:>6} nœuds  (WAL {:>8.1} Ko) ->  checkpoint_elu {:>9.1} µs  (fait={fait}, non_elu={non_elu})",
            offset_avant as f64 / 1024.0,
            mediane(s)
        );
    }
    // ── 4. FLUSH DES ZONES SEUL (l'étape 1 du checkpoint, isolée) ───────────
    // Réplique EXACTEMENT la boucle de zones de checkpoint_sous_fence, pour
    // séparer le coût de l'écriture des zones mmap de celui du reste (meta +
    // ancre + marqueur + scellage). Un store « nœuds seulement » n'a qu'une zone
    // non vide (Nodes) : c'est elle qui porte toute la donnée sale.
    println!("[4] flush des zones seules (boucle 12 zones + 2 side-cars, N=12) :");
    for n_noeuds in [2_000u32, 20_000] {
        let mut s = Vec::with_capacity(12);
        for rep in 0..12 {
            let dir = root.join(format!("zon-{}-{}", n_noeuds, rep));
            let _ = std::fs::remove_dir_all(&dir);
            let sub = SubstrateFile::create(&dir).unwrap();
            let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();
            for i in 0..n_noeuds {
                w.write_node(i, node(i)).unwrap();
            }
            w.commit().unwrap();
            drop(w);
            let mut sub2 = SubstrateFile::open(&dir).unwrap();
            let t = Instant::now();
            for zone in [
                obrain_substrate::Zone::Nodes,
                obrain_substrate::Zone::Edges,
                obrain_substrate::Zone::Props,
                obrain_substrate::Zone::Strings,
                obrain_substrate::Zone::Tier0,
                obrain_substrate::Zone::Tier1,
                obrain_substrate::Zone::Tier2,
                obrain_substrate::Zone::Hilbert,
                obrain_substrate::Zone::Community,
                obrain_substrate::Zone::Zorder,
                obrain_substrate::Zone::SimplexDir,
                obrain_substrate::Zone::SimplexIncidence,
            ] {
                let zf = sub2.open_zone(zone).unwrap();
                if !zf.is_empty() {
                    zf.msync().unwrap();
                    zf.fsync().unwrap();
                }
            }
            for name in [
                obrain_substrate::SIMPLEX_HEAP_FILENAME,
                obrain_substrate::SIMPLEX_POSTINGS_FILENAME,
            ] {
                let zf = sub2.open_named_zone(name).unwrap();
                if !zf.is_empty() {
                    zf.msync().unwrap();
                    zf.fsync().unwrap();
                }
            }
            s.push(t.elapsed().as_secs_f64() * 1e6);
            drop(sub2);
        }
        println!("    {n_noeuds:>6} nœuds  ->  flush zones {:.1} µs", mediane(s));
    }

    // ── 5. CHECKPOINT SUR STORE VIDE (le plancher fixe, sans donnée sale) ───
    println!("[5] checkpoint_elu sur store VIDE (0 nœud, N=12) :");
    {
        let mut s = Vec::with_capacity(12);
        for rep in 0..12 {
            let dir = root.join(format!("vide-{}", rep));
            let _ = std::fs::remove_dir_all(&dir);
            let sub = SubstrateFile::create(&dir).unwrap();
            let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();
            w.commit().unwrap();
            let wal_writer = w.wal();
            drop(w);
            let mut sub2 = SubstrateFile::open(&dir).unwrap();
            let t = Instant::now();
            let _ = checkpoint_elu(&mut sub2, &wal_writer).unwrap();
            s.push(t.elapsed().as_secs_f64() * 1e6);
            drop(sub2);
            drop(wal_writer);
        }
        println!("    0 nœud          ->  checkpoint_elu {:.1} µs (plancher fixe)", mediane(s));
    }

    // ── 6. LE PLOMB DU COÛT FIXE : fsync de RÉPERTOIRE + publication atomique ──
    // Le checkpoint fait 3 publications atomiques (meta + ancre×2), chacune =
    // [write tmp + sync_all(fichier) + rename + sync_all(RÉPERTOIRE)]. On mesure
    // les deux opérations qui n'apparaissent pas dans l'étalon [0] : le fsync de
    // répertoire, et le motif complet de publication.
    println!("[6] décomposition du coût fixe (médiane, N=100) :");
    {
        let fdir = std::fs::File::open(&root).unwrap();
        let mut s = Vec::with_capacity(100);
        for _ in 0..100 {
            let t = Instant::now();
            fdir.sync_all().unwrap();
            s.push(t.elapsed().as_secs_f64() * 1e6);
        }
        println!("    fsync RÉPERTOIRE (sync_all)        : {:>8.1} µs", mediane(s));

        let final_path = root.join("meta-probe");
        let tmp = root.join("meta-probe.tmp");
        let mut s = Vec::with_capacity(100);
        for _ in 0..100 {
            let t = Instant::now();
            {
                use std::io::Write as _;
                let mut f = std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .open(&tmp)
                    .unwrap();
                f.write_all(&[0u8; 4096]).unwrap();
                f.sync_all().unwrap();
            }
            std::fs::rename(&tmp, &final_path).unwrap();
            fdir.sync_all().unwrap();
            s.push(t.elapsed().as_secs_f64() * 1e6);
        }
        println!(
            "    publication atomique (tmp+fsync+rename+dir) : {:>8.1} µs",
            mediane(s)
        );
    }

    println!("=== FIN ===");
}
