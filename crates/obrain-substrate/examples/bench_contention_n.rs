//! ── BANC — CONTENTION MULTI-ÉCRIVAIN (N PROCESSUS, MÊME STORE) ──────────────
//!
//! Mesure le débit CUMULÉ de N processus qui créent des nœuds + arêtes sur le
//! MÊME store (`open_shared_write`). C'est la mesure de CONTENTION du checkpoint
//! élu : le store multi-écrivain partage la borne d'allocation (bulk bump), les
//! zones mmap et le WAL — exactement les ressources que le checkpoint touche.
//!
//! NE dépend PAS de `slot_high_water` pour vérifier la correction : l'allocation
//! par blocs (TAILLE_BLOC_ALLOCATION=256) avance la borne de K à chaque bloc, donc
//! « high_water - 1 » n'est PLUS le nombre de nœuds vivants (il y a des trous).
//!
//! Usage :
//!   cargo run --release -p obrain-substrate --example bench_contention_n

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Instant;

const NOEUDS_PAR_ECRIVAIN: u64 = 20_000;

fn enfant(dir: &str, n: u64) -> ! {
    use obrain_core::graph::GraphStoreMut as _;
    use obrain_substrate::SubstrateStore;
    use std::io::Write as _;
    let s = SubstrateStore::open_shared_write(dir).expect("open_shared_write");
    let t0 = Instant::now();
    let mut ids = Vec::with_capacity(n as usize);
    for _ in 0..n {
        ids.push(s.create_node(&["Banc"]));
    }
    for i in 0..n.saturating_sub(1) {
        let _ = s.create_edge(ids[i as usize], ids[i as usize + 1], "SUIT");
    }
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!("RESULTAT n={n} ms={ms:.1}");
    std::io::stdout().flush().ok();
    std::process::exit(0);
}

fn balayage(dir: &PathBuf) {
    println!();
    println!("╔══════════════════════════════════════════════════════════════════════╗");
    println!("║  CONTENTION MULTI-ÉCRIVAIN — N processus, même store (nœuds+arêtes)   ║");
    println!("╠══════╦═════════════╦═════════════╦═════════════╦══════════════════════╣");
    println!("║   N  ║   nœuds/s   ║   ops/s     ║   ms murale ║  ×N=1 (scale)        ║");
    println!("╠══════╬═════════════╬═════════════╬═════════════╬══════════════════════╣");

    let mut reference: Option<f64> = None;
    for n in [1u32, 2, 4, 8] {
        let _ = std::fs::remove_dir_all(dir);
        // Pré-chauffage : dimensionne la zone Nodes et pose le dictionnaire.
        {
            use obrain_core::graph::GraphStoreMut as _;
            use obrain_substrate::SubstrateStore;
            let s = SubstrateStore::create(dir).expect("créer");
            let _ = s.create_node(&["Banc"]);
            s.flush().expect("flush initial");
        }
        let moi = std::env::current_exe().expect("current_exe");
        let t0 = Instant::now();
        let enfants: Vec<Child> = (0..n)
            .map(|_| {
                Command::new(&moi)
                    .env("CONT_ROLE", "ecrivain")
                    .env("CONT_DIR", dir.display().to_string())
                    .env("CONT_N", NOEUDS_PAR_ECRIVAIN.to_string())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::inherit())
                    .spawn()
                    .expect("lancer l'écrivain")
            })
            .collect();
        let mut ms_max: f64 = 0.0;
        for e in enfants {
            let sortie = e.wait_with_output().expect("attendre");
            assert!(sortie.status.success(), "un écrivain a échoué");
            let texte = String::from_utf8_lossy(&sortie.stdout);
            let ms: f64 = texte
                .lines()
                .find(|l| l.starts_with("RESULTAT "))
                .and_then(|l| l.split_whitespace().find_map(|c| c.strip_prefix("ms=")))
                .and_then(|v| v.parse().ok())
                .expect("sortie d'écrivain illisible");
            ms_max = ms_max.max(ms);
        }
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        let noeuds = NOEUDS_PAR_ECRIVAIN * u64::from(n);
        let aretes = (NOEUDS_PAR_ECRIVAIN - 1) * u64::from(n);
        let ops_s = ((noeuds + aretes) as f64) * 1000.0 / ms;
        let noeuds_s = (noeuds as f64) * 1000.0 / ms;
        let base = *reference.get_or_insert(ops_s);
        println!(
            "║ {:>4} ║ {:>9.0}   ║ {:>9.0}   ║ {:>10.0}  ║ ×{:<16.2} ║",
            n,
            noeuds_s,
            ops_s,
            ms,
            ops_s / base
        );
    }
    println!("╚══════╩═════════════╩═════════════╩═════════════╩══════════════════════╝");
    println!(
        "  (chaque écrivain crée {NOEUDS_PAR_ECRIVAIN} nœuds + {} arêtes ; débit CUMULÉ en horloge murale)",
        NOEUDS_PAR_ECRIVAIN - 1
    );
    println!();
}

fn main() {
    // Rôle enfant : re-exécution du binaire par le balayage.
    if std::env::var("CONT_ROLE").as_deref() == Ok("ecrivain") {
        let dir = std::env::var("CONT_DIR").expect("CONT_DIR");
        let n: u64 = std::env::var("CONT_N")
            .expect("CONT_N")
            .parse()
            .expect("CONT_N entier");
        enfant(&dir, n);
    }
    let dir = std::env::var("CONTENTION_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp/obrain-bench-checkpoint.obrain"));
    std::fs::create_dir_all(&dir).expect("créer le répertoire");
    let store = dir.join("contention-store");
    balayage(&store);
}
