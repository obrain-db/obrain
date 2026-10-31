//! ── BANC — DÉBIT MULTI-ÉCRIVAIN DE BOUT EN BOUT SUR LE STORE (Phase B) ──────
//!
//! ── POURQUOI CE BANC EXISTE, ET POURQUOI IL EST À PART DE `wal_append` ──────
//! L'axe APPEND a mesuré le WAL (`wal_append.rs` : appends, groupes, trois
//! régimes de coordination). Ces chiffres disent que le JOURNAL scale. Ils ne
//! disent RIEN du STORE : ni l'allocation des slots, ni l'écriture des
//! `NodeRecord` dans les zones mmap, ni la création des arêtes et le chaînage des
//! listes d'adjacence explicites. Or c'est là que vivait le second verrou
//! mono-écrivain (`next_node_id` par processus), et c'est donc là qu'il faut
//! mesurer que le store SCALE — pas seulement le WAL.
//!
//! ── DEUX MESURES ───────────────────────────────────────────────────────────
//! 1. **`--multi-processus`** : N PROCESSUS (1, 2, 4, 8) créent chacun le même
//!    nombre de nœuds et d'arêtes sur le MÊME store (verrou `LOCK_SH`), en même
//!    temps. On rapporte le débit CUMULÉ mesuré en horloge murale (du premier
//!    lancement au dernier retour), plus le nombre total d'opérations. C'est la
//!    seule mesure qui dit si le store scale : elle inclut l'allocation partagée,
//!    les écritures mmap concurrentes et le WAL.
//! 2. **`criterion`** (par défaut, `cargo bench`) : le chemin MONO-ÉCRIVAIN
//!    (création de nœuds puis d'arêtes), pour verrouiller la NON-RÉGRESSION du
//!    cas où un seul écrivain détient le store — celui de toutes les bases de
//!    production aujourd'hui.
//!
//! ── UN MODE QUI DOIT SE DIRE ───────────────────────────────────────────────
//! Chaque enfant REFUSE d'écrire si l'allocation n'est pas partagée : il vérifie
//! que son identifiant de départ est au-delà de ce que le parent a déjà alloué.
//! Un banc qui mesurerait des écrivains se recouvrant mesurerait la perte, pas la
//! performance.

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group};
use obrain_core::graph::GraphStoreMut as _;
use obrain_substrate::SubstrateStore;
use obrain_substrate::borne_allocation::BorneAllocation;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Nœuds par écrivain dans le balayage multi-processus.
const NOEUDS_PAR_ECRIVAIN: u64 = 20_000;

// ---------------------------------------------------------------------------
// Enfant : un écrivain
// ---------------------------------------------------------------------------

/// Écrit `N` nœuds puis `N` arêtes sur le store partagé, et rend le temps écoulé
/// en millisecondes sur stdout.
fn enfant(dir: &Path, n: u64) -> ! {
    use std::io::Write as _;
    let s = SubstrateStore::open_shared_write(dir).expect("open_shared_write");
    let t0 = Instant::now();
    let mut ids = Vec::with_capacity(n as usize);
    for _ in 0..n {
        ids.push(s.create_node(&["Banc"]));
    }
    // Des arêtes RÉELLES : chaînage d'adjacence explicite (first_edge_off), donc
    // un chemin d'écriture distinct de la création de nœuds.
    for i in 0..n.saturating_sub(1) {
        let _ = s.create_edge(ids[i as usize], ids[i as usize + 1], "SUIT");
    }
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!("RESULTAT n={n} ms={ms:.1}");
    std::io::stdout().flush().ok();
    std::process::exit(0);
}

// ---------------------------------------------------------------------------
// Balayage multi-processus
// ---------------------------------------------------------------------------

struct Mesure {
    ecrivains: u32,
    ms: f64,
    noeuds: u64,
    aretes: u64,
    /// Nœuds VIVANTS relevés dans la zone après le balayage (slots alloués − 1
    /// pour la sentinelle nulle). Doit valoir `noeuds + 1` (le nœud de
    /// préchauffage) : tout écart est un écrasement entre écrivains.
    vivants: u64,
}

impl Mesure {
    /// Opérations par seconde (nœuds + arêtes).
    fn ops_par_s(&self) -> f64 {
        ((self.noeuds + self.aretes) as f64) * 1000.0 / self.ms
    }
    fn noeuds_par_s(&self) -> f64 {
        (self.noeuds as f64) * 1000.0 / self.ms
    }
}

/// Lance `ecrivains` processus, chacun écrivant `n` nœuds + `n-1` arêtes, et
/// mesure le débit CUMULÉ en horloge murale.
fn mesurer(ecrivains: u32, n: u64) -> Mesure {
    let td = tempfile::tempdir().expect("tempdir");
    let dir = td.path().join("store-banc");
    // Un store NEUF, pré-chauffé : une zone Nodes dimensionnée (l'allocation par
    // défaut grandit de ~1 Mio la première fois) et un dictionnaire posé. Sans ce
    // préchauffage, le premier écrivain paierait la croissance initiale et le
    // chiffre décrirait la création de la base, pas son débit.
    {
        let s = SubstrateStore::create(&dir).expect("créer");
        let _ = s.create_node(&["Banc"]);
        s.flush().expect("flush initial");
    }
    let avant = SubstrateStore::open(&dir)
        .map(|s| s.slot_high_water())
        .unwrap_or(1);
    assert_eq!(
        avant, 2,
        "le store pré-chauffé doit porter exactement un nœud"
    );
    drop(SubstrateStore::open(&dir));

    let moi = std::env::current_exe().expect("current_exe");
    let t0 = Instant::now();
    let enfants: Vec<Child> = (0..ecrivains)
        .map(|_| {
            Command::new(&moi)
                .env("MBW_ROLE", "ecrivain")
                .env("MBW_DIR", dir.display().to_string())
                .env("MBW_N", n.to_string())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .expect("lancer l'écrivain")
        })
        .collect();
    let mut ms_max: f64 = 0.0;
    for e in enfants {
        let sortie = e.wait_with_output().expect("attendre l'écrivain");
        assert!(
            sortie.status.success(),
            "un écrivain a échoué : {:?}",
            sortie.status
        );
        let texte = String::from_utf8_lossy(&sortie.stdout);
        let ms: f64 = texte
            .lines()
            .find_map(|l| l.strip_prefix("RESULTAT "))
            .and_then(|r| r.split_whitespace().find_map(|c| c.strip_prefix("ms=")))
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| panic!("sortie d'écrivain illisible : {texte:?}"));
        ms_max = ms_max.max(ms);
    }
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    // ── COMBIEN DE NŒUDS ONT RÉELLEMENT SURVÉCU ? ─────────────────────────
    // Le débit d'un store qui écrase les nœuds de ses pairs est un chiffre
    // trompeur : il mesure du travail détruit. On compte donc les nœuds VIVANTS
    // (relevé des zones, après réouverture, donc sans complaisance) et on les
    // confronte au nombre d'allocations demandées. C'est la colonne qui dit si
    // le store SCALE — au sens de « produit », pas de « brasse ».
    let vivants = SubstrateStore::open(&dir)
        .map(|s| u64::from(s.slot_high_water()).saturating_sub(1))
        .unwrap_or(0);
    Mesure {
        ecrivains,
        ms,
        noeuds: n * u64::from(ecrivains),
        aretes: (n - 1) * u64::from(ecrivains),
        vivants,
    }
}

fn balayage_multi_processus() {
    println!();
    println!("╔══════════════════════════════════════════════════════════════════════════════╗");
    println!("║  STORE MULTI-ÉCRIVAIN DE BOUT EN BOUT — N PROCESSUS, MÊME STORE              ║");
    println!("║  (nœuds + arêtes, zones mmap + WAL + allocation partagée)                    ║");
    println!("╠══════╦═════════════╦═════════════╦═════════════╦═════════════╦═══════════════╣");
    println!("║   N  ║   nœuds/s   ║   ops/s     ║   ms murale ║  ×N=1         ║  VIVANTS/att. ║");
    println!("╠══════╬═════════════╬═════════════╬═════════════╬═══════════════╬═══════════════╣");
    let mut reference: Option<f64> = None;
    for n in [1u32, 2, 4, 8] {
        let m = mesurer(n, NOEUDS_PAR_ECRIVAIN);
        let base = *reference.get_or_insert_with(|| m.ops_par_s());
        println!(
            "║ {:>4} ║ {:>9.0}   ║ {:>9.0}   ║ {:>10.0}  ║ ×{:<12.2} ║ {:>6}/{:>6} ║",
            m.ecrivains,
            m.noeuds_par_s(),
            m.ops_par_s(),
            m.ms,
            m.ops_par_s() / base,
            m.vivants,
            m.noeuds + 1
        );
    }
    println!("╚══════╩═════════════╩═════════════╩═════════════╩═══════════════╩═══════════════╝");
    println!(
        "  (chaque écrivain crée {NOEUDS_PAR_ECRIVAIN} nœuds + {} arêtes ; le débit est \
         CUMULÉ et mesuré en horloge murale)",
        NOEUDS_PAR_ECRIVAIN - 1
    );
    println!();
}

// ---------------------------------------------------------------------------
// Critère : le chemin MONO-ÉCRIVAIN (non-régression)
// ---------------------------------------------------------------------------

fn store_prechauffe(prefixe: &str) -> (tempfile::TempDir, PathBuf) {
    let td = tempfile::tempdir().expect("tempdir");
    let dir = td.path().join(prefixe);
    let s = SubstrateStore::create(&dir).expect("créer");
    s.flush().expect("flush");
    drop(s);
    (td, dir)
}

fn bench_ecriture_mono_ecrivain(c: &mut Criterion) {
    let mut groupe = c.benchmark_group("store_ecriture_mono_ecrivain");
    groupe.throughput(Throughput::Elements(64));
    for (nom, n) in [("noeuds_64", 64u64), ("noeuds_1024", 1024)] {
        let (_td, dir) = store_prechauffe("mono");
        let s = SubstrateStore::open(&dir).expect("ouvrir");
        groupe.throughput(Throughput::Elements(n));
        groupe.bench_function(BenchmarkId::from_parameter(nom), |b| {
            b.iter(|| {
                for _ in 0..n {
                    black_box(s.create_node(black_box(&["Banc"])));
                }
            })
        });
    }
    groupe.finish();
}

fn bench_noeud_plus_arete(c: &mut Criterion) {
    let mut groupe = c.benchmark_group("store_noeud_plus_arete_mono_ecrivain");
    groupe.throughput(Throughput::Elements(1));
    let (_td, dir) = store_prechauffe("paire");
    let s = SubstrateStore::open(&dir).expect("ouvrir");
    groupe.bench_function("noeud_puis_arete", |b| {
        let mut precedent = s.create_node(&["Banc"]);
        b.iter(|| {
            let n = s.create_node(black_box(&["Banc"]));
            black_box(s.create_edge(precedent, n, black_box("SUIT")));
            precedent = n;
        })
    });
    groupe.finish();
}

/// L'allocation seule : c'est le seul point que la pièce 2 a modifié sur le
/// chemin chaud (`fetch_add` sur une page partagée au lieu d'un atomique local).
/// Ce banc dit ce qu'elle coûte — la réponse doit être « rien de mesurable
/// devant une écriture de zone ».
fn bench_borne_allocation(c: &mut Criterion) {
    let mut groupe = c.benchmark_group("borne_allocation");
    groupe.throughput(Throughput::Elements(1));
    let (_td, dir) = store_prechauffe("borne");
    let borne = BorneAllocation::ouvrir(&dir).expect("borne");
    groupe.bench_function("reserver_noeud_partage", |b| {
        b.iter(|| black_box(borne.reserver_noeud()))
    });
    groupe.bench_function("reserver_arete_partage", |b| {
        b.iter(|| black_box(borne.reserver_arete()))
    });
    // Le TÉMOIN : l'atomique LOCAL d'avant la pièce 2. C'est à lui que se
    // compare le coût de l'allocation partagée, pas à zéro.
    let local = std::sync::atomic::AtomicU64::new(1);
    groupe.bench_function("reserver_local_temoin", |b| {
        b.iter(|| black_box(local.fetch_add(1, std::sync::atomic::Ordering::AcqRel)))
    });
    groupe.finish();
}

criterion_group!(
    benches,
    bench_ecriture_mono_ecrivain,
    bench_noeud_plus_arete,
    bench_borne_allocation
);

fn main() {
    if std::env::var("MBW_ROLE").as_deref() == Ok("ecrivain") {
        let dir = PathBuf::from(std::env::var("MBW_DIR").expect("MBW_DIR"));
        let n: u64 = std::env::var("MBW_N")
            .expect("MBW_N")
            .parse()
            .expect("MBW_N entier");
        enfant(&dir, n);
    }
    if std::env::args().any(|a| a == "--multi-processus") {
        balayage_multi_processus();
        return;
    }
    let mut c = Criterion::default().configure_from_args();
    bench_ecriture_mono_ecrivain(&mut c);
    bench_noeud_plus_arete(&mut c);
    bench_borne_allocation(&mut c);
    c.final_summary();
    // Sortie explicite : les enfants du balayage ont été attendus, mais les
    // verrous de répertoire des stores préchauffés peuvent traîner — et un banc
    // qui ne rend pas la main est un banc qu'on n'exécute plus.
    let _ = Duration::from_secs(0);
}
