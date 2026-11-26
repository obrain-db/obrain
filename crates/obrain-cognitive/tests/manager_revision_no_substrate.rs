//! Identifiants larges utilisables EN MÉMOIRE sans backend substrate : la
//! validation u16/u32 est propre au miroir substrate, pas à l'état en cache.

#![cfg(all(feature = "engram", not(feature = "substrate")))]

use std::sync::Arc;

use obrain_cognitive::engram::traits::InMemoryVectorIndex;
use obrain_cognitive::engram::{Engram, EngramId, EngramManager, EngramManagerConfig};
use obrain_common::types::NodeId;

#[test]
fn identifiants_larges_sans_substrate() {
    let manager = EngramManager::new(
        EngramManagerConfig::default(),
        Arc::new(InMemoryVectorIndex::new()),
    );
    let big = NodeId(1 << 40); // > u32
    let a = NodeId(1);

    let id = manager.store().next_id();
    let engram = Engram::new(id, vec![(a, 1.0), (big, 1.0)]);
    manager.store().insert(engram);

    // mutate accepte le grand identifiant (pas de limite u32 sans substrate).
    let r = manager.store().mutate(id, |e| {
        e.ensemble[1].1 = 0.5;
    });
    assert!(
        r.is_ok(),
        "grand identifiant accepté en mémoire sans substrate"
    );

    let e = manager.store().get(id).unwrap();
    assert_eq!(e.ensemble.len(), 2, "deux membres conservés");
    assert!(
        e.ensemble.iter().any(|(n, _)| *n == big),
        "grand identifiant conservé"
    );
    let _ = EngramId(0); // silence éventuel sur l'import
}
