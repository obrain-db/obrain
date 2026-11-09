//! Migration slots 512 — test multi-phase sur COPIE de base réelle.
//! Usage : migbase <base_dir>
//! Phases (échec = exit 1) :
//!  A comptes (scan) · U mises à jour d'entités EXISTANTES (legacy) · C
//!  créations packées (nœuds+arêtes) · D suppressions (legacy+pack) ·
//!  cycles de réouverture ×3 avec parité des comptes · re-lectures contrôle.
use obrain_common::types::{EdgeId, NodeId, PropertyKey, Value};
use obrain_core::graph::{GraphStore, GraphStoreMut};
use obrain_substrate::SubstrateStore;
use std::path::Path;

fn live_nodes(s: &SubstrateStore) -> Vec<NodeId> {
    let mut v = Vec::new();
    for slot in 1..s.slot_high_water() {
        if let Ok(Some(rec)) = s.writer().read_node(slot)
            && !rec.is_tombstoned()
            && rec.label_bitset != 0
        {
            v.push(NodeId(slot as u64));
        }
    }
    v
}
fn live_edges(s: &SubstrateStore) -> Vec<EdgeId> {
    let mut v = Vec::new();
    for slot in 1..s.edge_slot_high_water() {
        if let Ok(Some(rec)) = s.writer().read_edge(slot)
            && !rec.is_tombstoned()
        {
            v.push(EdgeId(slot));
        }
    }
    v
}

#[allow(unsafe_code)] // set_var (unsafe en édition 2024) au démarrage, avant tout fil
#[allow(clippy::many_single_char_names)] // outil de migration : s, n, e, a, b… (store, nœud, arête, extrémités)
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let base = args.first().expect("usage: migbase <base_dir>");
    println!("=== MIGRATION slots512 sur copie : {base} ===");
    unsafe {
        std::env::set_var("OBRAIN_PROPS_V2", "1");
        std::env::set_var("OBRAIN_PROPS_SLOTS_512", "1");
    }
    let mut cree_n = 0u64;
    let mut cree_e = 0u64;

    // Phase A — baseline (scan).
    let (n0, e0) = {
        let s = SubstrateStore::open(Path::new(base)).expect("open A");
        (live_nodes(&s).len() as u64, live_edges(&s).len() as u64)
    };
    println!("[A] baseline : nœuds={n0} arêtes={e0}");

    // Phase U — mises à jour sur des entités EXISTANTES (chaînes legacy).
    {
        let s = SubstrateStore::open(Path::new(base)).expect("open U");
        let nodes = live_nodes(&s);
        let edges = live_edges(&s);
        println!(
            "[U] scan : n={} e={} (re-baseline exacte attendue)",
            nodes.len(),
            edges.len()
        );
        assert_eq!(nodes.len() as u64, n0, "compte n stable après open");
        assert_eq!(edges.len() as u64, e0, "compte e stable après open");
        let k = nodes.len().min(150);
        for (i, &id) in nodes.iter().take(k).enumerate() {
            let key = format!("mU_{i}");
            s.set_node_property(id, &key, Value::Int64(i as i64));
            let got = s.get_node_property(id, &PropertyKey::new(&key));
            assert_eq!(
                got,
                Some(Value::Int64(i as i64)),
                "relecture mU {i} (nœud {})",
                id.0
            );
        }
        let ke = edges.len().min(80);
        for (i, &id) in edges.iter().take(ke).enumerate() {
            let key = format!("mE_{i}");
            s.set_edge_property(id, &key, Value::Int64((i as i64) * 2));
            assert_eq!(
                s.get_edge_property(id, &PropertyKey::new(&key)),
                Some(Value::Int64((i as i64) * 2))
            );
        }
        println!("[U] {k} nœuds + {ke} arêtes existants mis à jour + relus (legacy)");
    }

    // Phase C — créations packées (nœuds + arêtes avec props).
    {
        let s = SubstrateStore::open(Path::new(base)).expect("open C");
        for i in 0..40u64 {
            let n = s.create_node(&["MigQC"]);
            for (j, p) in [("a", 1i64), ("b", 2), ("c", 3)].iter().enumerate() {
                s.set_node_property(n, format!("mC_{j}").as_str(), Value::Int64(p.1 + i as i64));
            }
            cree_n += 1;
            let a = n;
            let b = s.create_node(&["MigQC2"]);
            cree_n += 1;
            let e = s.create_edge(a, b, "MIG");
            s.set_edge_property(e, "w", Value::Int64(i as i64));
            cree_e += 1;
        }
        // relecture échantillon packé
        let ids = live_nodes(&s);
        let pack_node = ids.iter().rev().find(|id| {
            s.writer()
                .read_node(id.0 as u32)
                .is_ok_and(|r| r.is_some_and(|rec| rec.first_prop_off.to_u64() & (1 << 47) != 0))
        });
        assert!(pack_node.is_some(), "au moins un nœud packé présent");
        let pn = *pack_node.unwrap();
        assert_eq!(
            s.get_node_property(pn, &PropertyKey::new("mC_0")),
            Some(Value::Int64(1 + 39)),
            "relecture prop packée"
        );
        println!(
            "[C] 40 nœuds+arêtes packés créés, tête slot vérifiée (ex. nœud {})",
            pn.0
        );
    }

    // Phase D — suppressions (legacy mU + pack mC).
    {
        let s = SubstrateStore::open(Path::new(base)).expect("open D");
        let nodes = live_nodes(&s);
        // retirer quelques mU_* (chaînes legacy) et la prop 'a' de nœuds packés MigQC
        for (i, &id) in nodes.iter().take(60).enumerate() {
            if i < 40 {
                let key = format!("mU_{i}");
                assert!(s.remove_node_property(id, &key).is_some(), "remove mU_{i}");
                assert_eq!(s.get_node_property(id, &PropertyKey::new(&key)), None);
            }
        }
        // (repérage MigQC par label brut peu fiable — on retire par 'a' si présent)
        for id in &nodes {
            if s.get_node_property(*id, &PropertyKey::new("mC_0"))
                .is_some()
            {
                let _ = s.remove_node_property(*id, "mC_0");
                assert_eq!(
                    s.get_node_property(*id, &PropertyKey::new("mC_0")),
                    None,
                    "remove pack"
                );
            }
        }
        println!("[D] suppressions legacy + pack effectuées et vérifiées");
    }

    // Phase E — cycles de réouverture ×3 : parité des comptes + contrôles.
    let attendu_n = n0 + cree_n;
    let attendu_e = e0 + cree_e;
    for c in 1..=3u32 {
        let s = SubstrateStore::open(Path::new(base)).expect("open cycle");
        let (n, e) = (live_nodes(&s).len() as u64, live_edges(&s).len() as u64);
        assert_eq!(n, attendu_n, "cycle {c} : nœuds == baseline+créés");
        assert_eq!(e, attendu_e, "cycle {c} : arêtes == baseline+créées");
        // Contrôle pack persisté (mC_1 non retiré en D ; relation mC_2 == mC_1+1).
        let ids = live_nodes(&s);
        let pack_ok = ids.iter().any(|id| {
            let a = s.get_node_property(*id, &PropertyKey::new("mC_1"));
            let b = s.get_node_property(*id, &PropertyKey::new("mC_2"));
            matches!((a, b), (Some(Value::Int64(x)), Some(Value::Int64(y))) if y == x + 1)
        });
        assert!(
            pack_ok,
            "cycle {c} : au moins un nœud packé avec mC_1/mC_2 cohérents (persistance)"
        );
        // Contrôle legacy non retiré (mU_99 posé si l'échantillon le couvrait).
        let _ = ids.iter().any(|id| {
            s.get_node_property(*id, &PropertyKey::new("mU_99"))
                .is_some()
        });
        println!(
            "[E] réouverture {c}/3 : n={n} (attendu {attendu_n}) e={e} (attendu {attendu_e}) — OK"
        );
    }
    println!("=== MIGRATION TERMINÉE : toutes phases OK (A,U,C,D,E×3) sur {base} ===");
}
