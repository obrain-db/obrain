//! Contrôle cognitif sous migration slots 512 — à exécuter sur une COPIE.
//! Usage : cogcheck_slots512 <base_dir>
//! Vérifie que la couche cognitive (energy / scar-utility-affinity / community
//! / centrality / flags / labels des NodeRecords, tailles des zones engram_*,
//! community, cdc_*, degrees.*) est STRICTEMENT inchangée par une migration
//! slots complète : updates d'entités EXISTANTES (legacy), créations packées
//! nœuds+arêtes, suppressions, réouvertures ×2.
use obrain_common::types::{NodeId, PropertyKey, Value};
use obrain_core::graph::{GraphStore, GraphStoreMut};
use obrain_substrate::SubstrateStore;
use std::collections::BTreeMap;
use std::path::Path;

/// Digest des champs COGNITIFS des NodeRecords sur les slots `1..hw` (hors
/// propriétés : first_prop_off exclu — la migration slots ne doit toucher
/// QUE les chaînes de propriétés).
fn digest_cognitif(s: &SubstrateStore, hw: u32) -> (u64, u64) {
    let mut h: u64 = 0xC0FF_EE00_1122_3344;
    let mut compte = 0u64;
    for slot in 1..hw {
        if let Ok(Some(rec)) = s.writer().read_node(slot) {
            if rec.is_tombstoned() || rec.label_bitset == 0 {
                continue;
            }
            compte += 1;
            for c in [
                rec.label_bitset,
                u64::from(rec.community_id),
                u64::from(rec.energy),
                u64::from(rec.scar_util_affinity),
                u64::from(rec.centrality_cached),
                u64::from(rec.flags),
            ] {
                h ^= c.wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(slot % 64);
            }
        }
    }
    (h, compte)
}

fn est_zone_cognitive(n: &str) -> bool {
    n.starts_with("substrate.engram")
        || n.starts_with("substrate.community")
        || n.starts_with("substrate.cdc")
        || n.starts_with("substrate.degrees")
        || n.starts_with("substrate.tier")
        || n.starts_with("substrate.simplex")
}

fn tailles_zones(dir: &Path) -> BTreeMap<String, u64> {
    let mut m = BTreeMap::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let n = e.file_name().into_string().unwrap_or_default();
            if est_zone_cognitive(&n) {
                if let Ok(md) = e.metadata() {
                    m.insert(n, md.len());
                }
            }
        }
    }
    m
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let base = args.first().expect("usage: cogcheck_slots512 <base_dir>");
    let dir = Path::new(base);
    println!("=== COGCHECK slots512 : {base} ===");
    unsafe { std::env::set_var("OBRAIN_PROPS_SLOTS_512", "1"); }

    // Phase A — état cognitif initial.
    let (hw0, dig0, zones0) = {
        let s = SubstrateStore::open(dir).expect("open A");
        let hw = s.slot_high_water();
        let (h, n) = digest_cognitif(&s, hw);
        let z = tailles_zones(dir);
        println!("[A] hw={hw} · digest cognitif = {h:#x} ({n} nœuds vivants) · {} zones cognitives",
            z.len());
        (hw, (h, n), z)
    };

    // Migration slots : updates legacy existants + créations packées +
    // suppressions + réouvertures.
    {
        let s = SubstrateStore::open(dir).expect("open mig");
        let nodes: Vec<NodeId> = (1..hw0)
            .filter_map(|slot| {
                s.writer()
                    .read_node(slot)
                    .ok()
                    .flatten()
                    .filter(|r| !r.is_tombstoned() && r.label_bitset != 0)
                    .map(|_| NodeId(slot as u64))
            })
            .collect();
        for (i, &id) in nodes.iter().take(60).enumerate() {
            let key = format!("cog_{i}");
            s.set_node_property(id, &key, Value::Int64(i as i64));
            assert_eq!(s.get_node_property(id, &PropertyKey::new(&key)), Some(Value::Int64(i as i64)));
        }
        for i in 0..25u64 {
            let a = s.create_node(&["CogNew"]);
            s.set_node_property(a, "x", Value::Int64(i as i64));
            let b = s.create_node(&["CogNew2"]);
            let e = s.create_edge(a, b, "COG");
            s.set_edge_property(e, "w", Value::Int64(i as i64));
        }
        for (i, &id) in nodes.iter().take(20).enumerate() {
            let key = format!("cog_{i}");
            assert!(s.remove_node_property(id, &key).is_some());
        }
    }
    for c in 1..=2u32 {
        let s = SubstrateStore::open(dir).expect("reopen");
        println!("[E{c}] réouverture {c}/2 OK");
    }

    // Phase R — état cognitif final, restreint à l'ancien hw.
    let s = SubstrateStore::open(dir).expect("open R");
    let (digA, nA) = digest_cognitif(&s, hw0);
    let digest_initial = dig0.0;
    assert_eq!(
        digA, digest_initial,
        "digest cognitif CHANGÉ sur les nœuds préexistants : {digest_initial:#x} → {digA:#x}"
    );
    assert_eq!(nA, dig0.1, "compte de nœuds préexistants changé");
    let (_, nTot) = digest_cognitif(&s, s.slot_high_water());
    assert_eq!(nTot, dig0.1 + 50, "nouveaux nœuds packés attendus : +50");
    let zones1 = tailles_zones(dir);
    // Tous les fichiers cognitifs EXISTANTS doivent être strictement
    // identiques (taille). Les AJOUTS sont autorisés : créer des arêtes de
    // contrôle d'un type neuf (COG) matérialise son fichier de degrés.
    let mut ajouts = 0usize;
    for (k, v) in &zones0 {
        match zones1.get(k) {
            Some(w) => assert_eq!(*w, *v, "taille CHANGÉE sur la zone cognitive {k}"),
            None => panic!("zone cognitive {k} disparue"),
        }
    }
    for k in zones1.keys() {
        if !zones0.contains_key(k) {
            ajouts += 1;
        }
    }
    println!(
        "[R] digest cognitif INCHANGÉ ({digA:#x}, {} nœuds préexistants) · +50 nœuds packés · {} zones cognitives existantes identiques ({} ajout(s) de fichiers de degrés pour types d'arêtes de contrôle)",
        nA,
        zones0.len(),
        ajouts
    );
    println!("=== COGCHECK TERMINÉ : couche cognitive strictement intacte après migration slots ===");
}
