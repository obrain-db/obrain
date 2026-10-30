//! Sonde edges_from J4 (scratch) : le chemin exact du handler hierarchy.
use obrain_common::types::NodeId;
use obrain_engine::ObrainDB;
fn main() {
    let mut a = std::env::args().skip(1);
    let chemin = a.next().expect("sonde_edges_j4 <store> <tenant> <slot>");
    let tenant: u32 = a.next().unwrap().parse().unwrap();
    let slot: u64 = a.next().unwrap().parse().unwrap();
    let d = ObrainDB::open_read_only(&chemin).expect("open ro");
    if tenant != 999 {
        d.avec_principal(tenant).expect("principal");
    }
    let store = d.store().clone();
    let paires = store.edges_from(NodeId::new(slot), obrain_core::graph::Direction::Outgoing);
    println!("edges_from({slot}, Outgoing) sous vue {tenant} : {paires:?}");
    for (cible, eid) in paires {
        let t = store.edge_type(eid);
        println!("  -> cible {cible:?} type {t:?}");
    }
}
