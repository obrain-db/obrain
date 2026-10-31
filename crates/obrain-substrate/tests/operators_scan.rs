//! Integration tests for `obrain_core::execution::operators::ScanOperator`
//! against the substrate backend.
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
//! ## MVCC note
//!
//! `SubstrateStore::create_node_versioned` is a thin stub that delegates
//! to `create_node` (substrate is topology-as-storage — no MVCC version
//! tracking). The LPG-era `test_scan_with_mvcc_context` exercised
//! epoch-based time-travel filtering which substrate does not
//! implement; it is preserved below with adjusted assertions that
//! reflect substrate's always-current semantics (all nodes are visible
//! regardless of the epoch passed to the scan). A proper MVCC test for
//! substrate will live under T17 Step 24 (W3d session + persistence
//! retype) once MVCC semantics are defined for the new backend.
//!
//! Run with:
//!
//! ```bash
//! cargo test -p obrain-substrate --test operators_scan
//! ```

use obrain_common::types::{EpochId, TransactionId};
use obrain_core::execution::operators::{Operator, ScanOperator};
use obrain_core::graph::GraphStore;
use obrain_core::graph::traits::GraphStoreMut;
use obrain_substrate::SubstrateStore;
use std::sync::Arc;

#[test]
fn test_scan_by_label() {
    let store: Arc<dyn GraphStoreMut> = Arc::new(SubstrateStore::open_tempfile().unwrap());

    store.create_node(&["Person"]);
    store.create_node(&["Person"]);
    store.create_node(&["Animal"]);

    let mut scan = ScanOperator::with_label(store.clone() as Arc<dyn GraphStore>, "Person");

    let chunk = scan.next().unwrap().unwrap();
    assert_eq!(chunk.row_count(), 2);

    // Should be exhausted
    let next = scan.next().unwrap();
    assert!(next.is_none());
}

#[test]
fn test_scan_reset() {
    let store: Arc<dyn GraphStoreMut> = Arc::new(SubstrateStore::open_tempfile().unwrap());
    store.create_node(&["Person"]);

    let mut scan = ScanOperator::with_label(store.clone() as Arc<dyn GraphStore>, "Person");

    // First scan
    let chunk1 = scan.next().unwrap().unwrap();
    assert_eq!(chunk1.row_count(), 1);

    // Reset
    scan.reset();

    // Second scan should work
    let chunk2 = scan.next().unwrap().unwrap();
    assert_eq!(chunk2.row_count(), 1);
}

#[test]
fn test_full_scan() {
    let store: Arc<dyn GraphStoreMut> = Arc::new(SubstrateStore::open_tempfile().unwrap());

    // Create nodes with different labels
    store.create_node(&["Person"]);
    store.create_node(&["Person"]);
    store.create_node(&["Animal"]);
    store.create_node(&["Place"]);

    // Full scan (no label filter) should return all nodes
    let mut scan = ScanOperator::new(store.clone() as Arc<dyn GraphStore>);

    let chunk = scan.next().unwrap().unwrap();
    assert_eq!(chunk.row_count(), 4, "Full scan should return all 4 nodes");

    // Should be exhausted
    let next = scan.next().unwrap();
    assert!(next.is_none());
}

#[test]
fn test_scan_filtre_bien_par_epoque() {
    // (2026-08-14) CE TEST S'APPELAIT `test_scan_with_mvcc_context_substrate_stub`
    // ET AFFIRMAIT LE CONTRAIRE.
    //
    // Il documentait le comportement de bouchon : « substrate stub ignores
    // epoch », « all nodes are always visible regardless of epoch ». C'etait
    // une description honnete de l'etat du code — `create_node_versioned`
    // jetait son argument `epoch`, et `get_node_at_epoch` rendait le present.
    //
    // Le test disait lui-meme ou le remplacer : « a substrate-native MVCC test
    // (if/when MVCC semantics are defined for the new backend) belongs under
    // T17 Step 24 ». Elles sont definies. Un test qui fige un bouchon devient,
    // le jour ou le bouchon saute, un test qui DEFEND le bug — et le rouge
    // qu'il produit ressemble a une regression alors que c'est une correction.
    let store: Arc<dyn GraphStoreMut> = Arc::new(SubstrateStore::open_tempfile().unwrap());

    // Deux noeuds nes a l'epoque 1, un a l'epoque 5.
    let epoch1 = EpochId::new(1);
    store.create_node_versioned(&["Person"], epoch1, TransactionId::SYSTEM);
    store.create_node_versioned(&["Person"], epoch1, TransactionId::SYSTEM);
    let epoch5 = EpochId::new(5);
    store.create_node_versioned(&["Person"], epoch5, TransactionId::SYSTEM);

    // A l'epoque 3, le troisieme N'EXISTE PAS ENCORE.
    let mut scan = ScanOperator::with_label(store.clone() as Arc<dyn GraphStore>, "Person")
        .with_transaction_context(EpochId::new(3), None);
    let chunk = scan.next().unwrap().unwrap();
    assert_eq!(
        chunk.row_count(),
        2,
        "a l'epoque 3, seuls les deux noeuds nes a l'epoque 1 existent"
    );

    // A l'epoque 5, les trois sont la.
    let mut scan_all = ScanOperator::with_label(store.clone() as Arc<dyn GraphStore>, "Person")
        .with_transaction_context(EpochId::new(5), None);
    let chunk_all = scan_all.next().unwrap().unwrap();
    assert_eq!(
        chunk_all.row_count(),
        3,
        "a l'epoque 5, le troisieme noeud est ne"
    );

    // Et a l'epoque 0, aucun : ils sont tous nes apres.
    let mut scan_avant = ScanOperator::with_label(store.clone() as Arc<dyn GraphStore>, "Person")
        .with_transaction_context(EpochId::new(0), None);
    let chunk_avant = scan_avant.next().unwrap();
    assert!(
        chunk_avant.is_none_or(|c| c.row_count() == 0),
        "avant la premiere naissance, le graphe est vide"
    );
}
