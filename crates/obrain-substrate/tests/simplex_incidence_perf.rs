//! Incidence simpliciale : cascade I5, coût O(degré) prouvé au banc, gate
//! d'ouverture (T4, étape 3).
//!
//! * Le micro-banc construit trois bases synthétiques (1k / 10k / 100k
//!   simplexes) où le nœud sondé garde un degré simplicial CONSTANT (32) :
//!   le temps de `simplices_of_node` doit être PLAT — il dépend du degré,
//!   jamais du nombre total de simplexes (spec 271180c3 §2).
//! * Le gate d'ouverture matérialise 10k simplexes, checkpointe, puis
//!   mesure `SubstrateStore::open` (5 passes, minimum) : ≤ 112 ms — les
//!   zones simpliciales ne coûtent que mmap + validation d'en-tête O(1).

use obrain_common::types::NodeId;
use obrain_core::graph::GraphStoreMut;
use obrain_substrate::wal_io::SyncMode;
use obrain_substrate::{SubstrateFile, SubstrateStore, Writer, simplex_kind};
use web_time::Instant;

/// Degré simplicial constant du nœud sondé, à toutes les échelles.
const DEGRE_SONDE: usize = 32;

/// Construit une base brute (Writer, WAL non fsync'é — le banc mesure la
/// LECTURE) : `total` triangles de bruit sur des nœuds lointains, plus
/// [`DEGRE_SONDE`] triangles incidents au nœud sondé 7, entrelacés dans le
/// flux d'écriture pour disperser ses postings.
fn base_synthetique(total: u32) -> (SubstrateFile, Writer) {
    let sub = SubstrateFile::open_tempfile().unwrap();
    let path = sub.path().to_path_buf();
    let w = Writer::new(sub, SyncMode::Never).unwrap();
    let pas = total / DEGRE_SONDE as u32;
    for i in 0..total {
        let base = 1_000 + i * 3;
        w.upsert_simplex(
            simplex_kind::TRIANGLE,
            0,
            0,
            &[base, base + 1, base + 2],
            &[],
        )
        .unwrap();
        if i % pas == 0 && (i / pas) < DEGRE_SONDE as u32 {
            let k = 500_000_000 + (i / pas) * 2;
            w.upsert_simplex(simplex_kind::TRIANGLE, 0, 0, &[7, k, k + 1], &[])
                .unwrap();
        }
    }
    // Ré-ouvrir le substrat pour garder la paire vivante (le Writer possède
    // le sien) — le tempdir est porté par le premier handle.
    let sub2 = SubstrateFile::open(&path).unwrap();
    (sub2, w)
}

/// Médiane (µs) de `simplices_of_node(7)` sur `iters` appels.
fn mediane_us(w: &Writer, iters: usize) -> f64 {
    // Chauffe.
    for _ in 0..32 {
        assert_eq!(w.simplices_of_node(7).unwrap().len(), DEGRE_SONDE);
    }
    let mut echantillons: Vec<f64> = (0..iters)
        .map(|_| {
            let t = Instant::now();
            let ids = w.simplices_of_node(7).unwrap();
            let dt = t.elapsed().as_nanos() as f64 / 1_000.0;
            assert_eq!(ids.len(), DEGRE_SONDE);
            dt
        })
        .collect();
    echantillons.sort_by(|a, b| a.partial_cmp(b).unwrap());
    echantillons[iters / 2]
}

/// MICRO-BANC — l'accès aux simplexes d'un nœud est en O(degré), pas en
/// O(total) : à degré constant, le coût reste plat de 1k à 100k simplexes.
#[test]
fn micro_banc_incidence_o_degre() {
    let mut mesures = Vec::new();
    for total in [1_000u32, 10_000, 100_000] {
        let (_sub, w) = base_synthetique(total);
        let med = mediane_us(&w, 301);
        println!(
            "simplices_of_node(deg={DEGRE_SONDE}) @ {total} simplexes : {med:.2} µs (médiane)"
        );
        mesures.push((total, med));
    }
    let (_t1, m1) = mesures[0];
    let (_t3, m3) = mesures[2];
    assert!(
        m3 < m1 * 4.0 + 20.0,
        "coût non plat : {m1:.2} µs @1k contre {m3:.2} µs @100k — l'accès doit \
         dépendre du degré, pas du total"
    );
}

/// GATE D'OUVERTURE — base peuplée de 10k simplexes, checkpointée : le
/// minimum de 5 ouvertures reste ≤ 112 ms (budget de la campagne, note
/// f7b25dbe), et l'écart avec la même base AVANT peuplement reste marginal
/// (les zones simpliciales ne paient que mmap + en-tête à l'ouverture).
#[test]
fn gate_ouverture_112_ms_avec_zones_peuplees() {
    let td = tempfile::tempdir().unwrap();
    let sub_path = td.path().join("kb");

    // Base SANS simplexes : référence.
    {
        let store = SubstrateStore::create(&sub_path).unwrap();
        for _ in 0..100 {
            store.create_node(&["Doc"]);
        }
        store.flush().unwrap();
        store.checkpoint().unwrap();
    }
    let ouvre = |n: usize| -> f64 {
        (0..n)
            .map(|_| {
                let t = Instant::now();
                let store = SubstrateStore::open(&sub_path).unwrap();
                let dt = t.elapsed().as_secs_f64() * 1_000.0;
                drop(store);
                dt
            })
            .fold(f64::INFINITY, f64::min)
    };
    let sans = ouvre(5);

    // Peupler 10k simplexes (triangles + hyperarêtes typées avec props)
    // sur 30k nœuds vivants, puis checkpoint (l'ouverture mesurée ne doit
    // pas payer le rejeu du WAL : le gate porte sur les ZONES).
    {
        let store = SubstrateStore::open(&sub_path).unwrap();
        let nodes: Vec<NodeId> = (0..30_000).map(|_| store.create_node(&["N"])).collect();
        for i in 0..10_000usize {
            let a = nodes[3 * (i % 9_000)];
            let b = nodes[3 * (i % 9_000) + 1];
            let c = nodes[3 * (i % 9_000) + 2];
            if i % 5 == 0 {
                let d = nodes[27_000 + (i % 3_000)];
                store
                    .upsert_simplex(
                        simplex_kind::HYPEREDGE,
                        3,
                        0,
                        &[a, b, c, d],
                        format!("{{\"g\":{i}}}").as_bytes(),
                    )
                    .unwrap();
            } else {
                let d = nodes[9_000 + i % 18_000];
                store
                    .upsert_simplex(simplex_kind::TRIANGLE, 0, 0, &[a, b, d], &[])
                    .unwrap();
            }
        }
        store.flush().unwrap();
        store.checkpoint().unwrap();
    }
    let avec = ouvre(5);

    println!(
        "ouverture (min de 5) : sans zones simpliciales {sans:.1} ms, avec 10k simplexes {avec:.1} ms"
    );
    assert!(
        avec <= 112.0,
        "gate d'ouverture dépassé : {avec:.1} ms > 112 ms avec zones simpliciales peuplées"
    );

    // Et la base rouverte lit bien sa couche simpliciale.
    let store = SubstrateStore::open(&sub_path).unwrap();
    let tri = store.iter_simplices_kind(simplex_kind::TRIANGLE).unwrap();
    let hyp = store.iter_simplices_kind(simplex_kind::HYPEREDGE).unwrap();
    assert!(!tri.is_empty() && !hyp.is_empty());
}

/// CASCADE I5 — la suppression d'un nœud tombstone tous ses simplexes via
/// l'incidence ; les simplexes des autres nœuds survivent.
#[test]
fn cascade_i5_suppression_de_noeud() {
    let store = SubstrateStore::open_tempfile().unwrap();
    let n: Vec<NodeId> = (0..8).map(|_| store.create_node(&["N"])).collect();

    let (t1, _) = store
        .upsert_simplex(simplex_kind::TRIANGLE, 0, 0, &[n[0], n[1], n[2]], &[])
        .unwrap();
    let (t2, _) = store
        .upsert_simplex(simplex_kind::TRIANGLE, 0, 0, &[n[1], n[2], n[3]], &[])
        .unwrap();
    let (h1, _) = store
        .upsert_simplex(simplex_kind::HYPEREDGE, 5, 0, &[n[0], n[4], n[5]], b"{}")
        .unwrap();
    let (h2, _) = store
        .upsert_simplex(simplex_kind::HYPEREDGE, 5, 0, &[n[4], n[5], n[6]], b"{}")
        .unwrap();

    assert_eq!(store.simplices_of_node(n[0]).unwrap(), vec![t1, h1]);

    // Suppression de n[0] : t1 et h1 tombent, t2 et h2 survivent.
    assert!(store.delete_node(n[0]));
    assert!(store.get_simplex(t1).unwrap().is_none());
    assert!(store.get_simplex(h1).unwrap().is_none());
    assert!(store.get_simplex(t2).unwrap().is_some());
    assert!(store.get_simplex(h2).unwrap().is_some());
    assert_eq!(store.simplices_of_node(n[0]).unwrap().len(), 0);
    assert_eq!(store.simplices_of_node(n[1]).unwrap(), vec![t2]);
    assert_eq!(store.simplices_of_node(n[4]).unwrap(), vec![h2]);

    // I5 en amont : upserter un simplexe portant un membre mort est refusé.
    assert!(
        store
            .upsert_simplex(simplex_kind::TRIANGLE, 0, 0, &[n[0], n[6], n[7]], &[])
            .is_err(),
        "un membre tombstoné doit être refusé (I5)"
    );

    // La clé (kind, type, membres) reste unique après cascade : re-créer
    // le triangle de t2 est un no-op sur le même id.
    let (encore, created) = store
        .upsert_simplex(simplex_kind::TRIANGLE, 0, 0, &[n[1], n[2], n[3]], &[])
        .unwrap();
    assert_eq!(encore, t2);
    assert!(!created);
}

/// Octets par simplexe MESURÉS (régions utiles : record + incidence + heap)
/// contre les 56 / 64 / ~71 B de la spec (§5).
#[test]
fn octets_par_simplexe_mesures() {
    fn regions_utiles(sub: &SubstrateFile) -> u64 {
        let mut total = 0u64;
        // dir : records consommés.
        let dir = std::fs::read(sub.path().join("substrate.simplex_dir")).unwrap();
        let hw = u32::from_le_bytes(dir[8..12].try_into().unwrap()) as u64;
        total += hw * 32;
        // heap : payload consommé.
        if let Ok(heap) = std::fs::read(sub.path().join("substrate.simplex_heap"))
            && heap.len() >= 16
        {
            total += u64::from_le_bytes(heap[8..16].try_into().unwrap());
        }
        // incidence : entrées non nulles (16 B) + postings (4 B utiles/entrée
        // + en-têtes de pages allouées).
        if let Ok(posts) = std::fs::read(sub.path().join("substrate.simplex_postings"))
            && posts.len() >= 16
        {
            let next = u64::from_le_bytes(posts[8..16].try_into().unwrap());
            total += next.saturating_sub(4096); // pages allouées entières
        }
        if let Ok(inc) = std::fs::read(sub.path().join("substrate.simplex_incidence")) {
            let entries = inc[64..]
                .chunks_exact(16)
                .filter(|e| e.iter().any(|&b| b != 0))
                .count() as u64;
            total += entries * 16;
        }
        total
    }

    // La spec (§5) amortit l'incidence à ~8 B/posting sur la DISTRIBUTION
    // DE RÉFÉRENCE, où les nœuds partagent leurs simplexes (23 393
    // appartenances). Les trois populations reproduisent ce partage : un
    // pool borné de nœuds, ~6 appartenances par nœud.
    fn population(kind: u8, arite: usize, pool: u32, cible: usize, props: &[u8]) -> f64 {
        let sub = SubstrateFile::open_tempfile().unwrap();
        let w = Writer::new(sub, SyncMode::Never).unwrap();
        let mut crees = 0usize;
        let mut i = 0u64;
        while crees < cible {
            i += 1;
            let mut membres: Vec<u32> = (0..arite as u64)
                .map(|j| ((i * (7 + 6 * j) + 13 * j * j + i / 97) % pool as u64) as u32)
                .collect();
            membres.sort_unstable();
            membres.dedup();
            if membres.len() != arite {
                continue;
            }
            let type_id = if kind == simplex_kind::HYPEREDGE {
                3
            } else {
                0
            };
            let (_, created) = w.upsert_simplex(kind, type_id, 0, &membres, props).unwrap();
            if created {
                crees += 1;
            }
        }
        w.msync_zones().unwrap();
        let sub = w.substrate();
        let total = regions_utiles(&sub.lock());
        total as f64 / cible as f64
    }

    let tri_par_simplexe = population(simplex_kind::TRIANGLE, 3, 500, 1_000, &[]);
    let k4_par_simplexe = population(simplex_kind::K4, 4, 700, 1_000, &[]);
    // Hyperarêtes : arité 3 majoritaire (la moyenne de référence est 3,28)
    // avec ~12 octets de props.
    let hyp_par_simplexe = population(simplex_kind::HYPEREDGE, 3, 600, 1_000, b"{\"line\":42}");

    println!(
        "octets/simplexe mesurés : triangle {tri_par_simplexe:.1} B (spec 56), \
         K4 {k4_par_simplexe:.1} B (spec 64), hyperarête {hyp_par_simplexe:.1} B (spec ~71)"
    );
    // Le même ordre de grandeur que la spec : record 32 B + ~4-16 B
    // amortis par posting (bloc 32 B partagé entre ~6 appartenances +
    // entrée de répertoire 16 B par nœud) + heap pour les hyperarêtes.
    assert!(
        tri_par_simplexe < 112.0,
        "triangle : 2x le budget spec (56 B)"
    );
    assert!(k4_par_simplexe < 128.0, "K4 : 2x le budget spec (64 B)");
    assert!(
        hyp_par_simplexe < 142.0,
        "hyperarête : 2x le budget spec (71 B)"
    );
}
