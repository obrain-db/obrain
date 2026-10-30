//! RFC zorder-superposition — capacité C1 : `SubstrateStore::window_query_2d`.
//!
//! Fenêtre 2D `(centrality_cached × out-degree)` scoped à une communauté.
//! Ce test vérifie la sémantique EXACTE du scan (baseline de référence des
//! benches B2) : bornes inclusives, exclusion tombstones, communautés
//! étrangères dans le range borné (layout non compacté, page-aligned),
//! et indépendance vis-à-vis des side-columns Hilbert/Z-order (hints).
//!
//! Tous les attendus sont dérivés des VRAIS slots retournés par le store
//! (l'allocateur page-aligne les communautés : jamais de numérotation
//! 1..n contiguë garantie).

use obrain_common::NodeId;
use obrain_substrate::{SubstrateStore, record::node_flags};

/// (cid, centrality f32, slot, tombstoned) table built from what the store
/// actually created — the oracle reads the same records the scan reads.
#[derive(Clone)]
struct NodeFact {
    cid: u32,
    q16: u16,
    slot: u32,
}

fn create_with_centrality(store: &SubstrateStore, cid: u32, q16: u16) -> u64 {
    let node = store.create_node_in_community(&["L"], cid);
    store
        .update_centrality_batch_f32(vec![(node, q16 as f32 / u16::MAX as f32)])
        .unwrap();
    node.0
}

fn brute(
    facts: &[NodeFact],
    deg: &std::collections::HashMap<u32, u32>,
    tomb: &std::collections::HashSet<u32>,
    cid: u32,
    c_lo: u16,
    c_hi: u16,
    d_lo: u32,
    d_hi: u32,
) -> Vec<u32> {
    let mut out: Vec<u32> = facts
        .iter()
        .filter(|f| {
            f.cid == cid
                && !tomb.contains(&f.slot)
                && f.q16 >= c_lo
                && f.q16 <= c_hi
                && deg.get(&f.slot).copied().unwrap_or(0) >= d_lo
                && deg.get(&f.slot).copied().unwrap_or(0) <= d_hi
        })
        .map(|f| f.slot)
        .collect();
    out.sort_unstable();
    out
}

#[test]
fn window_query_2d_matches_bruteforce() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("wq");
    let store = SubstrateStore::create(&path).unwrap();

    // Interleaved communities (never bulk-sorted → bounded ranges contain
    // foreign slots). Values match the classic T11 test layout.
    let layout: Vec<(u32, u16)> = vec![
        (2, 1000),
        (0, 2000),
        (2, 3000),
        (1, 500),
        (0, 1500),
        (1, 800),
        (2, 2500),
        (0, 100),
        (1, 9000),
        (0, 1200),
        (2, 4000),
        (1, 6000),
    ];
    let mut facts: Vec<NodeFact> = Vec::new();
    let mut ids: Vec<NodeId> = Vec::new();
    for (cid, q16) in &layout {
        let node = create_with_centrality(&store, *cid, *q16);
        ids.push(NodeId(node));
        facts.push(NodeFact {
            cid: *cid,
            q16: *q16,
            slot: node as u32,
        });
    }

    // Edges by index-pairs over the created list (topology of the classic
    // test), translated to real slots.
    let idx_edges: Vec<(usize, usize)> = vec![
        (0, 2),
        (2, 7),
        (7, 0),
        (1, 4),
        (4, 8),
        (8, 1),
        (3, 6),
        (6, 11),
        (11, 3),
        (5, 9),
        (0, 1),
        (10, 9),
    ];
    for &(a, b) in &idx_edges {
        store
            .create_synapse_slot(ids[a], ids[b], 0.9)
            .unwrap();
    }

    // Tombstone the 7th created node (whatever its community).
    let tomb_slot = facts[6].slot;
    {
        let w = store.writer();
        let mut rec = w.read_node(tomb_slot).unwrap().unwrap();
        rec.flags |= node_flags::TOMBSTONED;
        w.write_node(tomb_slot, rec).unwrap();
    }
    store.flush().unwrap();

    // The synapse-edge path above writes edges but does not increment the
    // persisted degree column. Drop the store and delete the (empty)
    // column sidecar so the fresh instance rehydrates it from the edge
    // zone scan — the same lazy path a real engine write session uses.
    drop(store);
    let deg_file = path.join(obrain_substrate::degree_column::DEGREE_COLUMN_FILENAME);
    if deg_file.exists() {
        std::fs::remove_file(&deg_file).unwrap();
    }
    let store = SubstrateStore::open(&path).unwrap();

    // Oracle data: degrees per real slot, tombstone set, live facts.
    let mut deg: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
    for &(a, b) in &idx_edges {
        *deg.entry(facts[a].slot).or_insert(0) += 1;
        let _ = facts[b].slot;
    }
    let tomb: std::collections::HashSet<u32> = [tomb_slot].into_iter().collect();

    // Sanity: quantized centrality stayed within ±1 of the target.
    let w = store.writer();
    for f in &facts {
        let rec = w.read_node(f.slot).unwrap().unwrap();
        assert!(
            (rec.centrality_cached as i32 - f.q16 as i32).abs() <= 1,
            "centrality quantization drift at slot {}",
            f.slot
        );
    }

    let cases: Vec<(u32, u16, u16, u32, u32)> = vec![
        (0, 0, u16::MAX, 0, u32::MAX),
        (1, 0, u16::MAX, 0, u32::MAX),
        (2, 0, u16::MAX, 0, u32::MAX),
        (0, 1200, u16::MAX, 1, u32::MAX), // hubs of community 0
        (2, 4000, 4000, 1, 1),            // thin window
        (1, 0, 800, 0, u32::MAX),
        (42, 0, u16::MAX, 0, u32::MAX), // no such community
    ];
    for (cid, c_lo, c_hi, d_lo, d_hi) in cases {
        let got = store.window_query_2d(cid, c_lo, c_hi, d_lo, d_hi).unwrap();
        let want = brute(&facts, &deg, &tomb, cid, c_lo, c_hi, d_lo, d_hi);
        assert_eq!(got, want, "window cid={cid} c=[{c_lo},{c_hi}] d=[{d_lo},{d_hi}]");
    }
    // Reversed bounds are normalized.
    let got = store
        .window_query_2d(0, u16::MAX, 1200, u32::MAX, 1)
        .unwrap();
    let want = brute(&facts, &deg, &tomb, 0, 1200, u16::MAX, 1, u32::MAX);
    assert_eq!(got, want, "reversed bounds must be normalized");
}

/// Store-created communities: each all-open window returns exactly its own
/// live nodes, whatever page-aligned slots the allocator chose.
#[test]
fn window_query_2d_community_isolation() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("wq3");
    let store = SubstrateStore::create(&path).unwrap();

    let mut a: Vec<u32> = Vec::new();
    for i in 1..=5u32 {
        let node = create_with_centrality(&store, 9, 5000 + i as u16 * 100);
        a.push(node as u32);
    }
    let mut b: Vec<u32> = Vec::new();
    for _ in 6..=8u32 {
        let node = create_with_centrality(&store, 10, 100);
        b.push(node as u32);
    }
    a.sort_unstable();
    b.sort_unstable();
    store.flush().unwrap();

    let got_a = store.window_query_2d(9, 0, u16::MAX, 0, u32::MAX).unwrap();
    assert_eq!(got_a, a, "community 9 isolation");
    let got_b = store.window_query_2d(10, 0, u16::MAX, 0, u32::MAX).unwrap();
    assert_eq!(got_b, b, "community 10 isolation");
}
