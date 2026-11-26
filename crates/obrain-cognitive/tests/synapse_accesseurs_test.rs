//! Synapse — accesseurs et construction restés non couverts (#414, périmètre C).
//!
//! `Synapse::new_at` (construction à instant explicite), `last_reinforced`
//! (suivi du renforcement) et `CrossBaseNodeId::new`.

#![cfg(feature = "synapse")]

use obrain_cognitive::{CrossBaseNodeId, Synapse};
use obrain_common::types::NodeId;
use std::time::Duration;
use web_time::Instant;

const H: Duration = Duration::from_secs(3600);

fn node(id: u64) -> NodeId {
    NodeId::new(id)
}

#[test]
fn synapse_new_at_ancre_la_decroissance_a_l_instant_donne() {
    let t0 = Instant::now();
    let s = Synapse::new_at(node(1), node(2), 1.0, H, t0);
    assert_eq!(s.source, node(1));
    assert_eq!(s.target, node(2));
    assert_eq!(s.created_at, t0);
    assert_eq!(s.reinforcement_count, 1);
    // Une demi-vie après l'ancre : poids 0,5.
    assert!((s.weight_at(t0 + H) - 0.5).abs() < 1e-12);
}

#[test]
fn synapse_last_reinforced_suit_le_renforcement() {
    let t0 = Instant::now();
    let mut s = Synapse::new_at(node(1), node(2), 1.0, H, t0);
    assert_eq!(s.last_reinforced(), t0);
    let t1 = t0 + H;
    s.reinforce_at(0.5, t1);
    assert_eq!(s.last_reinforced(), t1);
    assert_eq!(s.reinforcement_count, 2);
}

#[test]
fn cross_base_node_id_porte_les_deux_cles() {
    let xb = CrossBaseNodeId::new("db_a".to_string(), node(7));
    assert_eq!(xb.db_id, "db_a");
    assert_eq!(xb.node_id, node(7));
}
