//! Mesures de l'index de labels (obrain#120, point 5).
//!
//! Pour N nœuds (défaut : 1 000 000 et 5 000 000) importés en bloc — un label
//! par nœud : `Rare` sur un nœud sur 10 000, `Frequent` sur la moitié, `Autre`
//! sur le reste — puis flush, fermeture, et pour chaque mesure la charge
//! (`vm.loadavg`, macOS) :
//!
//!   * ouverture (`SubstrateStore::open`), médiane de 3 ;
//!   * première requête après ouverture = construction de l'index + requête ;
//!   * `nodes_by_label` par l'index et par le balayage (l'ancien chemin, même
//!     code), label rare et label fréquent, médianes ;
//!   * `nodes_by_label_capped(…, 5)` sur le label fréquent ;
//!   * le coût des crochets d'écriture (créations, `add_label`).
//!
//!   cargo run --release -p obrain-substrate --example index_labels_mesures [N…]

use obrain_core::graph::traits::{GraphStore, GraphStoreMut};
use obrain_substrate::SubstrateStore;
use std::time::{Duration, Instant};

fn charge() -> String {
    std::process::Command::new("sysctl")
        .args(["-n", "vm.loadavg"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "?".into())
}

fn mediane(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

fn chrono<R>(f: impl FnOnce() -> R) -> (R, Duration) {
    let t = Instant::now();
    let r = f();
    (r, t.elapsed())
}

fn mesurer(nom: &str, reps: usize, mut f: impl FnMut() -> usize) {
    let mut t = Vec::new();
    let mut n = 0;
    for _ in 0..reps {
        let (r, d) = chrono(&mut f);
        n = r;
        t.push(d);
    }
    println!(
        "  {nom:<44} {:>12.3?}  (médiane de {reps}, {n} nœuds)  charge {}",
        mediane(t),
        charge()
    );
}

fn main() {
    let tailles: Vec<usize> = {
        let a: Vec<usize> = std::env::args()
            .skip(1)
            .filter_map(|x| x.parse().ok())
            .collect();
        if a.is_empty() {
            vec![1_000_000, 5_000_000]
        } else {
            a
        }
    };
    for n in tailles {
        let td = tempfile::tempdir().unwrap();
        let base = td.path().join("kb");
        println!("== N = {n}  (charge au départ {})", charge());
        {
            let s = SubstrateStore::create(&base).unwrap();
            let labels: Vec<&str> = (0..n)
                .map(|i| {
                    if i % 10_000 == 0 {
                        "Rare"
                    } else if i % 2 == 0 {
                        "Frequent"
                    } else {
                        "Autre"
                    }
                })
                .collect();
            let (_, d) = chrono(|| s.bulk_import_labels_edges(&labels, &[]).unwrap());
            println!("  import en bloc                               {d:>12.3?}");
            s.flush().unwrap();
            s.close().unwrap();
        }
        let mut ouvertures = Vec::new();
        for _ in 0..3 {
            let (s, d) = chrono(|| SubstrateStore::open(&base).unwrap());
            ouvertures.push(d);
            s.close().unwrap();
        }
        println!(
            "  ouverture (SubstrateStore::open)             {:>12.3?}  (médiane de 3)  charge {}",
            mediane(ouvertures),
            charge()
        );
        let s = SubstrateStore::open(&base).unwrap();
        let (r, d) = chrono(|| s.nodes_by_label("Rare").len());
        println!(
            "  1re requête Rare = construction + requête    {d:>12.3?}  ({r} nœuds)  charge {}",
            charge()
        );
        let diag = s.diagnostic_index_labels();
        println!(
            "  index : {} entrées, {} Mo réservés, {} construction(s)",
            diag.entrees,
            diag.octets / (1 << 20),
            diag.constructions
        );
        mesurer("index    nodes_by_label(Rare)", 21, || {
            s.nodes_by_label("Rare").len()
        });
        mesurer("balayage nodes_by_label(Rare)", 5, || {
            s.nodes_by_label_balayage("Rare").len()
        });
        mesurer("index    nodes_by_label(Frequent)", 5, || {
            s.nodes_by_label("Frequent").len()
        });
        mesurer("balayage nodes_by_label(Frequent)", 5, || {
            s.nodes_by_label_balayage("Frequent").len()
        });
        mesurer("index    nodes_by_label_capped(Frequent, 5)", 21, || {
            s.nodes_by_label_capped("Frequent", 5).len()
        });
        assert_eq!(s.nodes_by_label("Rare"), s.nodes_by_label_balayage("Rare"));
        assert_eq!(
            s.nodes_by_label("Frequent"),
            s.nodes_by_label_balayage("Frequent")
        );
        drop(s);
    }
    // Coût des crochets d'écriture : 200 000 créations + 200 000 `add_label`,
    // index jamais construit puis index construit (store neuf à chaque fois).
    for construit in [false, true] {
        let s = SubstrateStore::open_tempfile().unwrap();
        s.create_node(&["Amorce"]);
        if construit {
            let _ = s.nodes_by_label("Amorce");
        }
        let (ids, d) = chrono(|| {
            (0..200_000)
                .map(|_| s.create_node(&["X"]))
                .collect::<Vec<_>>()
        });
        println!(
            "  200 000 create_node, index construit={construit:<5}  {d:>12.3?}  charge {}",
            charge()
        );
        let (_, d) = chrono(|| {
            for id in &ids {
                s.add_label(*id, "Y");
            }
        });
        println!(
            "  200 000 add_label,   index construit={construit:<5}  {d:>12.3?}  charge {}",
            charge()
        );
        assert_eq!(s.diagnostic_index_labels().construit, construit);
    }
}
