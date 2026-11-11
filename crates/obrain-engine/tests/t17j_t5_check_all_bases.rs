//! T17j T5 — check chain↔rec.src coherence on PO / Wikipedia / Megalaw.
//! Read-only (no repair) — just diagnoses whether the bug is
//! universal or specific to PO.
//!
//! ```bash
//! cargo test -p obrain-engine --release --features cypher \
//!   --test t17j_t5_check_all_bases -- --nocapture
//! ```

#![cfg(feature = "cypher")]

use obrain_bases_banc::{Base, base_lecture};
use obrain_core::graph::GraphStore;
use obrain_substrate::SubstrateStore;

fn check(name: &str, base: Base) {
    let Some(path) = base_lecture(base) else {
        return;
    };
    let subs = SubstrateStore::open_read_only(&path).expect("open");
    println!("\n═══ {name} ═══");
    for label in ["File", "Function", "Article", "Decision", "Concept"] {
        let nodes = subs.nodes_by_label(label);
        if nodes.is_empty() {
            continue;
        }
        let sample = nodes.len().min(50);
        let mut walked = 0u64;
        let mut mismatches = 0u64;
        for nid in nodes.iter().take(sample) {
            let (w, _m, mm, _) = subs.diagnose_chain_vs_rec_src(*nid);
            walked += w;
            mismatches += mm;
        }
        let pct = if walked > 0 {
            (mismatches as f64 / walked as f64) * 100.0
        } else {
            0.0
        };
        println!(
            "  :{:<12}  sample={:<3}  walked={:>6}  mismatches={:>6}  ({:.1}% broken)",
            label, sample, walked, mismatches, pct
        );
    }
}

#[test]
fn check_chain_consistency_three_corpora() {
    check("PO (repaired)", Base::Po);
    check("Wikipedia", Base::Wikipedia);
    check("Megalaw", Base::Megalaw);
}
