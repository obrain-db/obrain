//! Résolution multi-critères — `find_nodes_by_properties` selon l'ORDRE des
//! conditions.
//!
//! POURQUOI CE BANC EXISTE. La vérification d'une contrainte d'unicité
//! composite `UNIQUE (a, b)` est un `find_nodes_by_properties([(a, …), (b, …)])`.
//! Tant que la résolution prenait la PREMIÈRE clé indexée, son coût dépendait
//! de l'ordre de déclaration : avec `a` peu sélective en tête (2 valeurs sur
//! N nœuds), N/2 candidats étaient relus un par un ; avec `b` unique en tête,
//! un seul. La résolution par le plus petit ensemble indexé doit rendre les
//! deux ordres équivalents. Ce banc mesure les deux, côte à côte.
//!
//! LIRE `BANCS.md` AVANT D'AJOUTER UNE MESURE ICI (contrôle à blanc, charge de
//! la machine).
//!
//! CE QUI EST MESURÉ
//!
//! * `unique_ab` — `a` (2 valeurs) en tête, `b` (unique) ensuite.
//! * `unique_ba` — l'ordre inverse.
//! * `unique_ab_nouveau` — même ordre que `unique_ab`, pour un couple que
//!   personne ne porte : le cas de l'insertion ACCEPTÉE.
//!
//! Les index des deux propriétés sont déclarés avant le peuplement ; la base
//! est montée UNE fois, hors de toute boucle de mesure.
//!
//! Lancer :
//!
//! ```bash
//! cargo bench -p obrain-substrate --bench resolution_selective
//! # autre taille :
//! RESOLUTION_BENCH_N=50000 cargo bench -p obrain-substrate --bench resolution_selective
//! ```

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use obrain_common::types::Value;
use obrain_core::graph::{GraphStore, GraphStoreMut};
use obrain_substrate::SubstrateStore;

fn nombre_de_noeuds() -> i64 {
    std::env::var("RESOLUTION_BENCH_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5_000)
}

fn terrain(n: i64) -> SubstrateStore {
    let store = SubstrateStore::open_tempfile().expect("base temporaire");
    store.create_property_index("a");
    store.create_property_index("b");
    for i in 0..n {
        let id = store.create_node(&["Ligne"]);
        store.set_node_property(id, "a", Value::Int64(i % 2));
        store.set_node_property(id, "b", Value::Int64(i));
    }
    store
}

fn bench_unicite_composite(c: &mut Criterion) {
    let n = nombre_de_noeuds();
    let store = terrain(n);
    // Un couple PORTÉ (un seul nœud) et un couple que personne ne porte.
    let milieu = n / 2;
    let a = ("a", Value::Int64(milieu % 2));
    let b = ("b", Value::Int64(milieu));
    let b_nouveau = ("b", Value::Int64(n + 1));
    let ab = [a.clone(), b.clone()];
    let ba = [b, a.clone()];
    let ab_nouveau = [a, b_nouveau];
    // Garde du banc : il mesure bien une recherche qui RÉPOND.
    assert_eq!(store.find_nodes_by_properties(&ab).len(), 1);
    assert_eq!(store.find_nodes_by_properties(&ba).len(), 1);
    assert!(store.find_nodes_by_properties(&ab_nouveau).is_empty());

    let mut g = c.benchmark_group("resolution_selective");
    g.bench_function(BenchmarkId::new("unique_ab", n), |bench| {
        bench.iter(|| black_box(store.find_nodes_by_properties(black_box(&ab))));
    });
    g.bench_function(BenchmarkId::new("unique_ba", n), |bench| {
        bench.iter(|| black_box(store.find_nodes_by_properties(black_box(&ba))));
    });
    g.bench_function(BenchmarkId::new("unique_ab_nouveau", n), |bench| {
        bench.iter(|| black_box(store.find_nodes_by_properties(black_box(&ab_nouveau))));
    });
    g.finish();
}

criterion_group!(benches, bench_unicite_composite);
criterion_main!(benches);
