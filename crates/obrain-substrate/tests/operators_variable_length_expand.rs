//! Integration tests for
//! `obrain_core::execution::operators::VariableLengthExpandOperator` against
//! the substrate backend.
//!
//! ## Why this lives in obrain-substrate and not in obrain-core
//!
//! Relocated as part of T17 Step 3 W2/Class-2 follow-up (decision
//! `b1dfe229`). `obrain-core` cannot take `obrain-substrate` as a
//! dev-dependency without creating two distinct compilation units of
//! `obrain-core` (gotcha `598dda40`). The forward direction
//! (`obrain-substrate → obrain-core`) has no cycle, so moving operator
//! tests into an integration test of `obrain-substrate` is the
//! post-T17-cutover home for the LPG-era fixtures.
//!
//! Run with:
//!
//! ```bash
//! cargo test -p obrain-substrate --test operators_variable_length_expand
//! ```

use obrain_common::types::{NodeId, Value};
use obrain_core::execution::operators::{
    ExecutionPathMode as PathMode, Operator, ScanOperator, VariableLengthExpandOperator,
};
use obrain_core::graph::Direction;
use obrain_core::graph::GraphStore;
use obrain_core::graph::traits::GraphStoreMut;
use obrain_substrate::SubstrateStore;
use std::sync::Arc;

fn new_store() -> Arc<dyn GraphStoreMut> {
    Arc::new(SubstrateStore::open_tempfile().unwrap())
}

#[test]
fn test_variable_length_expand_chain() {
    let store = new_store();

    // Create chain: a -> b -> c -> d
    let a = store.create_node(&["Node"]);
    let b = store.create_node(&["Node"]);
    let c = store.create_node(&["Node"]);
    let d = store.create_node(&["Node"]);

    store.set_node_property(a, "name", "a".into());
    store.set_node_property(b, "name", "b".into());
    store.set_node_property(c, "name", "c".into());
    store.set_node_property(d, "name", "d".into());

    store.create_edge(a, b, "NEXT");
    store.create_edge(b, c, "NEXT");
    store.create_edge(c, d, "NEXT");

    // Create scan for all nodes
    let scan = Box::new(ScanOperator::with_label(
        Arc::clone(&store) as Arc<dyn GraphStore>,
        "Node",
    ));

    // Expand 1-3 hops from all nodes
    let mut expand = VariableLengthExpandOperator::new(
        Arc::clone(&store) as Arc<dyn GraphStore>,
        scan,
        0,
        Direction::Outgoing,
        vec!["NEXT".to_string()],
        1,
        3,
    );

    let mut results = Vec::new();
    while let Ok(Some(chunk)) = expand.next() {
        for i in 0..chunk.row_count() {
            let src = chunk.column(0).unwrap().get_node_id(i).unwrap();
            let dst = chunk.column(2).unwrap().get_node_id(i).unwrap();
            results.push((src, dst));
        }
    }

    // From 'a', we should reach b (1 hop), c (2 hops), d (3 hops)
    let a_targets: Vec<NodeId> = results
        .iter()
        .filter(|(s, _)| *s == a)
        .map(|(_, t)| *t)
        .collect();
    assert!(a_targets.contains(&b), "a should reach b");
    assert!(a_targets.contains(&c), "a should reach c");
    assert!(a_targets.contains(&d), "a should reach d");
    assert_eq!(a_targets.len(), 3, "a should reach exactly 3 nodes");
}

#[test]
fn test_variable_length_expand_min_hops() {
    let store = new_store();

    // Create chain: a -> b -> c
    let a = store.create_node(&["Node"]);
    let b = store.create_node(&["Node"]);
    let c = store.create_node(&["Node"]);

    store.create_edge(a, b, "NEXT");
    store.create_edge(b, c, "NEXT");

    let scan = Box::new(ScanOperator::with_label(
        Arc::clone(&store) as Arc<dyn GraphStore>,
        "Node",
    ));

    // Expand 2-3 hops only (skip 1 hop)
    let mut expand = VariableLengthExpandOperator::new(
        Arc::clone(&store) as Arc<dyn GraphStore>,
        scan,
        0,
        Direction::Outgoing,
        vec!["NEXT".to_string()],
        2, // min 2 hops
        3, // max 3 hops
    );

    let mut results = Vec::new();
    while let Ok(Some(chunk)) = expand.next() {
        for i in 0..chunk.row_count() {
            let src = chunk.column(0).unwrap().get_node_id(i).unwrap();
            let dst = chunk.column(2).unwrap().get_node_id(i).unwrap();
            results.push((src, dst));
        }
    }

    // From 'a', we should reach c (2 hops) but NOT b (1 hop)
    let a_targets: Vec<NodeId> = results
        .iter()
        .filter(|(s, _)| *s == a)
        .map(|(_, t)| *t)
        .collect();
    assert!(
        !a_targets.contains(&b),
        "a should NOT reach b with min_hops=2"
    );
    assert!(a_targets.contains(&c), "a should reach c");
}

#[test]
fn test_variable_length_expand_diamond() {
    let store = new_store();

    //     a
    //    / \
    //   b   c
    //    \ /
    //     d
    let a = store.create_node(&["Node"]);
    let b = store.create_node(&["Node"]);
    let c = store.create_node(&["Node"]);
    let d = store.create_node(&["Node"]);

    store.create_edge(a, b, "EDGE");
    store.create_edge(a, c, "EDGE");
    store.create_edge(b, d, "EDGE");
    store.create_edge(c, d, "EDGE");

    let scan = Box::new(ScanOperator::with_label(
        Arc::clone(&store) as Arc<dyn GraphStore>,
        "Node",
    ));
    let mut expand = VariableLengthExpandOperator::new(
        Arc::clone(&store) as Arc<dyn GraphStore>,
        scan,
        0,
        Direction::Outgoing,
        vec![],
        1,
        2,
    );

    let mut results = Vec::new();
    while let Ok(Some(chunk)) = expand.next() {
        for i in 0..chunk.row_count() {
            let src = chunk.column(0).unwrap().get_node_id(i).unwrap();
            let dst = chunk.column(2).unwrap().get_node_id(i).unwrap();
            results.push((src, dst));
        }
    }

    // From 'a': b (1 hop), c (1 hop), d (2 hops via b), d (2 hops via c)
    let a_targets: Vec<NodeId> = results
        .iter()
        .filter(|(s, _)| *s == a)
        .map(|(_, t)| *t)
        .collect();
    assert!(a_targets.contains(&b));
    assert!(a_targets.contains(&c));
    assert!(a_targets.contains(&d));
    // d appears twice (two paths)
    assert_eq!(a_targets.iter().filter(|&&t| t == d).count(), 2);
}

#[test]
fn test_variable_length_expand_no_matching_edges() {
    let store = new_store();

    let a = store.create_node(&["Node"]);
    let b = store.create_node(&["Node"]);
    store.create_edge(a, b, "KNOWS");

    let scan = Box::new(ScanOperator::with_label(
        Arc::clone(&store) as Arc<dyn GraphStore>,
        "Node",
    ));
    // Filter for LIKES edges (which don't exist)
    let mut expand = VariableLengthExpandOperator::new(
        Arc::clone(&store) as Arc<dyn GraphStore>,
        scan,
        0,
        Direction::Outgoing,
        vec!["LIKES".to_string()],
        1,
        3,
    );

    let result = expand.next().unwrap();
    assert!(result.is_none());
}

#[test]
fn test_variable_length_expand_single_hop() {
    let store = new_store();

    let a = store.create_node(&["Node"]);
    let b = store.create_node(&["Node"]);
    store.create_edge(a, b, "EDGE");

    let scan = Box::new(ScanOperator::with_label(
        Arc::clone(&store) as Arc<dyn GraphStore>,
        "Node",
    ));
    // Exactly 1 hop
    let mut expand = VariableLengthExpandOperator::new(
        Arc::clone(&store) as Arc<dyn GraphStore>,
        scan,
        0,
        Direction::Outgoing,
        vec![],
        1,
        1,
    );

    let mut results = Vec::new();
    while let Ok(Some(chunk)) = expand.next() {
        for i in 0..chunk.row_count() {
            let src = chunk.column(0).unwrap().get_node_id(i).unwrap();
            let dst = chunk.column(2).unwrap().get_node_id(i).unwrap();
            results.push((src, dst));
        }
    }

    // Only a -> b (1 hop)
    let a_results: Vec<_> = results.iter().filter(|(s, _)| *s == a).collect();
    assert_eq!(a_results.len(), 1);
    assert_eq!(a_results[0].1, b);
}

#[test]
fn test_variable_length_expand_with_path_length() {
    let store = new_store();

    let a = store.create_node(&["Node"]);
    let b = store.create_node(&["Node"]);
    let c = store.create_node(&["Node"]);
    store.create_edge(a, b, "EDGE");
    store.create_edge(b, c, "EDGE");

    let scan = Box::new(ScanOperator::with_label(
        Arc::clone(&store) as Arc<dyn GraphStore>,
        "Node",
    ));
    let mut expand = VariableLengthExpandOperator::new(
        Arc::clone(&store) as Arc<dyn GraphStore>,
        scan,
        0,
        Direction::Outgoing,
        vec![],
        1,
        2,
    )
    .with_path_length_output();

    let mut found_path_lengths = false;
    while let Ok(Some(chunk)) = expand.next() {
        // With path_length_output, there should be an extra column
        assert!(chunk.column_count() >= 4); // source, edge, target, path_length
        found_path_lengths = true;
    }
    assert!(found_path_lengths);
}

#[test]
fn test_variable_length_expand_reset() {
    let store = new_store();

    let a = store.create_node(&["Node"]);
    let b = store.create_node(&["Node"]);
    store.create_edge(a, b, "EDGE");

    let scan = Box::new(ScanOperator::with_label(
        Arc::clone(&store) as Arc<dyn GraphStore>,
        "Node",
    ));
    let mut expand = VariableLengthExpandOperator::new(
        Arc::clone(&store) as Arc<dyn GraphStore>,
        scan,
        0,
        Direction::Outgoing,
        vec![],
        1,
        1,
    );

    // First pass
    let mut count1 = 0;
    while let Ok(Some(chunk)) = expand.next() {
        count1 += chunk.row_count();
    }

    expand.reset();

    // Second pass
    let mut count2 = 0;
    while let Ok(Some(chunk)) = expand.next() {
        count2 += chunk.row_count();
    }

    assert_eq!(count1, count2);
}

#[test]
fn test_variable_length_expand_name() {
    let store = new_store();
    let scan = Box::new(ScanOperator::with_label(
        Arc::clone(&store) as Arc<dyn GraphStore>,
        "Node",
    ));
    let expand = VariableLengthExpandOperator::new(
        Arc::clone(&store) as Arc<dyn GraphStore>,
        scan,
        0,
        Direction::Outgoing,
        vec![],
        1,
        3,
    );
    assert_eq!(expand.name(), "VariableLengthExpand");
}

#[test]
fn test_variable_length_expand_empty_input() {
    let store = new_store();
    let scan = Box::new(ScanOperator::with_label(
        Arc::clone(&store) as Arc<dyn GraphStore>,
        "Nonexistent",
    ));
    let mut expand = VariableLengthExpandOperator::new(
        Arc::clone(&store) as Arc<dyn GraphStore>,
        scan,
        0,
        Direction::Outgoing,
        vec![],
        1,
        3,
    );

    let result = expand.next().unwrap();
    assert!(result.is_none());
}

#[test]
fn test_variable_length_expand_with_chunk_capacity() {
    let store = new_store();

    // Create a star graph: center -> 5 outer nodes
    let center = store.create_node(&["Node"]);
    for _ in 0..5 {
        let outer = store.create_node(&["Node"]);
        store.create_edge(center, outer, "EDGE");
    }

    let scan = Box::new(ScanOperator::with_label(
        Arc::clone(&store) as Arc<dyn GraphStore>,
        "Node",
    ));
    let mut expand = VariableLengthExpandOperator::new(
        Arc::clone(&store) as Arc<dyn GraphStore>,
        scan,
        0,
        Direction::Outgoing,
        vec![],
        1,
        1,
    )
    .with_chunk_capacity(2);

    let mut total = 0;
    let mut chunk_count = 0;
    while let Ok(Some(chunk)) = expand.next() {
        chunk_count += 1;
        total += chunk.row_count();
    }

    assert_eq!(total, 5);
    assert!(chunk_count >= 2);
}

#[test]
fn test_trail_mode_no_repeated_edges() {
    let store = new_store();

    // Create cycle: a -> b -> a (same edge types)
    let a = store.create_node(&["Node"]);
    let b = store.create_node(&["Node"]);
    store.create_edge(a, b, "EDGE");
    store.create_edge(b, a, "EDGE");

    let scan = Box::new(ScanOperator::with_label(
        Arc::clone(&store) as Arc<dyn GraphStore>,
        "Node",
    ));
    let mut expand = VariableLengthExpandOperator::new(
        Arc::clone(&store) as Arc<dyn GraphStore>,
        scan,
        0,
        Direction::Outgoing,
        vec![],
        1,
        4,
    )
    .with_path_mode(PathMode::Trail);

    let mut results = Vec::new();
    while let Ok(Some(chunk)) = expand.next() {
        for i in 0..chunk.row_count() {
            let src = chunk.column(0).unwrap().get_node_id(i).unwrap();
            let dst = chunk.column(2).unwrap().get_node_id(i).unwrap();
            results.push((src, dst));
        }
    }

    // From 'a': Trail allows a->b (1 hop) and a->b->a (2 hops, different edges)
    // but NOT a->b->a->b (3 hops, would reuse the a->b edge)
    let a_results: Vec<_> = results.iter().filter(|(s, _)| *s == a).collect();
    assert_eq!(a_results.len(), 2, "Trail from a: a->b and a->b->a only");
}

#[test]
fn test_acyclic_mode_no_repeated_nodes() {
    let store = new_store();

    // Create cycle: a -> b -> a
    let a = store.create_node(&["Node"]);
    let b = store.create_node(&["Node"]);
    store.create_edge(a, b, "EDGE");
    store.create_edge(b, a, "EDGE");

    let scan = Box::new(ScanOperator::with_label(
        Arc::clone(&store) as Arc<dyn GraphStore>,
        "Node",
    ));
    let mut expand = VariableLengthExpandOperator::new(
        Arc::clone(&store) as Arc<dyn GraphStore>,
        scan,
        0,
        Direction::Outgoing,
        vec![],
        1,
        4,
    )
    .with_path_mode(PathMode::Acyclic);

    let mut results = Vec::new();
    while let Ok(Some(chunk)) = expand.next() {
        for i in 0..chunk.row_count() {
            let src = chunk.column(0).unwrap().get_node_id(i).unwrap();
            let dst = chunk.column(2).unwrap().get_node_id(i).unwrap();
            results.push((src, dst));
        }
    }

    // From 'a': Acyclic allows a->b only (cannot revisit a)
    let a_results: Vec<_> = results.iter().filter(|(s, _)| *s == a).collect();
    assert_eq!(a_results.len(), 1, "Acyclic from a: only a->b");
    assert_eq!(a_results[0].1, b);
}

/// `*0..n` : le chemin de longueur 0 existe (obrain-db/obrain#189). Depuis
/// `a` de degré 2 (`a -> b`, `a -> c`), `*0..1` rend trois chemins : `a`
/// lui-même (longueur 0, arête NULL, `nodes = [a]`, `edges = []`), puis `b`
/// et `c`. `*0..0` rend exactement `a`. Les modes Trail et Acyclic aussi :
/// un chemin vide ne répète rien.
#[test]
fn test_zero_hop_path_is_emitted() {
    let store = new_store();
    let a = store.create_node(&["Start"]);
    let b = store.create_node(&["Node"]);
    let c = store.create_node(&["Node"]);
    store.create_edge(a, b, "NEXT");
    store.create_edge(a, c, "NEXT");

    let collect = |min: u32, max: u32, mode: PathMode| {
        let scan = Box::new(ScanOperator::with_label(
            Arc::clone(&store) as Arc<dyn GraphStore>,
            "Start",
        ));
        let mut expand = VariableLengthExpandOperator::new(
            Arc::clone(&store) as Arc<dyn GraphStore>,
            scan,
            0,
            Direction::Outgoing,
            vec!["NEXT".to_string()],
            min,
            max,
        )
        .with_path_mode(mode)
        .with_path_length_output()
        .with_path_detail_output();
        // Schéma : [source, edge, target, path_length, path_nodes, path_edges, path]
        let mut rows = Vec::new();
        while let Ok(Some(chunk)) = expand.next() {
            for i in 0..chunk.row_count() {
                let edge = chunk.column(1).unwrap().get_edge_id(i);
                let target = chunk.column(2).unwrap().get_node_id(i).unwrap();
                let length = chunk.column(3).unwrap().get_value(i);
                let nodes = chunk.column(4).unwrap().get_value(i);
                let edges = chunk.column(5).unwrap().get_value(i);
                rows.push((edge, target, length, nodes, edges));
            }
        }
        rows
    };

    for mode in [PathMode::Walk, PathMode::Trail, PathMode::Acyclic] {
        let rows = collect(0, 1, mode);
        assert_eq!(rows.len(), 3, "*0..1 en {mode:?} : a lui-même, b, c");
        let zero: Vec<_> = rows.iter().filter(|r| r.1 == a).collect();
        assert_eq!(
            zero.len(),
            1,
            "exactement un chemin de longueur 0 ({mode:?})"
        );
        let (edge, _, length, nodes, edges) = zero[0];
        assert!(edge.is_none(), "l'arête du chemin vide est NULL ({mode:?})");
        assert!(
            matches!(length, Some(Value::Int64(0))),
            "length = 0 ({mode:?}) : {length:?}"
        );
        match nodes {
            Some(Value::List(items)) => assert_eq!(
                items.as_ref(),
                &[Value::Int64(a.0 as i64)],
                "nodes(p) = [a] ({mode:?})"
            ),
            autre => panic!("nodes(p) doit être une liste ({mode:?}) : {autre:?}"),
        }
        match edges {
            Some(Value::List(items)) => {
                assert!(items.is_empty(), "relationships(p) = [] ({mode:?})");
            }
            autre => panic!("relationships(p) doit être une liste ({mode:?}) : {autre:?}"),
        }
        let mut reached: Vec<NodeId> = rows.iter().filter(|r| r.1 != a).map(|r| r.1).collect();
        reached.sort();
        let mut attendu = vec![b, c];
        attendu.sort();
        assert_eq!(
            reached, attendu,
            "les voisins à 1 saut restent là ({mode:?})"
        );
    }

    let rows = collect(0, 0, PathMode::Walk);
    assert_eq!(rows.len(), 1, "*0..0 rend exactement le nœud lui-même");
    assert_eq!(rows[0].1, a);

    let rows = collect(1, 1, PathMode::Walk);
    assert_eq!(
        rows.len(),
        2,
        "*1..1 : pas de chemin vide quand min_hops = 1"
    );
}
