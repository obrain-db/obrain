//! Fabrique la base du banc, une fois, dans `OBRAIN_BANC_BASE`.
//!
//!   cargo test --release --test fabrique_base -- --ignored

use obrain_common::types::Value;
use obrain_core::graph::traits::GraphStoreMut;

const NOEUDS: usize = 120_000;

#[test]
#[ignore = "fabrique — lance a la main"]
fn fabrique_base() {
    let base = std::env::var("OBRAIN_BANC_BASE").expect("OBRAIN_BANC_BASE non definie");
    let p = std::path::Path::new(&base);
    assert!(!p.exists(), "la base existe deja : {base}");
    let store = obrain_substrate::SubstrateStore::create(p).expect("creation");
    for i in 0..NOEUDS {
        let n = store.create_node(&["Grain"]);
        store.set_node_property(n, "v", Value::Int64(i as i64));
        store.set_node_property(
            n,
            "etiquette",
            Value::String(format!("grain-{i:06}").into()),
        );
    }
    drop(store);
    println!("base fabriquee : {NOEUDS} noeuds dans {base}");
}
