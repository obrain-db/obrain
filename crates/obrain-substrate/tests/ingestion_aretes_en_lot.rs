//! Ingestion EN LOT des propriétés d'ARÊTES : `bulk_import_edge_props` doit
//! rendre EXACTEMENT ce que rend le chemin unitaire `set_edge_property`, après
//! réouverture à froid.
//!
//! Le jeu : 100 000 arêtes, deux à trois propriétés chacune, plus les cas
//! limites (arête absente, arête portant déjà une valeur, clé répétée dans un
//! même lot, valeur au-delà du seuil des colonnes blob).

use obrain_common::types::{EdgeId, PropertyKey, Value};
use obrain_core::graph::traits::{GraphStore, GraphStoreMut};
use obrain_substrate::SubstrateStore;

const ARETES: u32 = 100_000;
const NOEUDS: u32 = 1_000;

/// Arêtes (source, cible, type) : les nœuds sont les slots 1..=NOEUDS.
fn aretes() -> Vec<(u32, u32, &'static str)> {
    (0..ARETES)
        .map(|i| (i % NOEUDS + 1, (i * 7) % NOEUDS + 1, "CALLS"))
        .collect()
}

/// Propriétés attendues de l'arête de slot `i + 1`.
fn props_de(i: u32) -> Vec<(String, Value)> {
    let mut l = vec![
        ("line".to_string(), Value::Int64(i as i64)),
        (
            "name".to_string(),
            Value::String(format!("fonction_{i}").into()),
        ),
        ("exact".to_string(), Value::Bool(i.is_multiple_of(3))),
    ];
    if i.is_multiple_of(1_000) {
        // Au-delà du seuil des colonnes blob : chemin unitaire.
        l.push(("corps".to_string(), Value::String("c".repeat(700).into())));
    }
    l
}

fn en_lot(dir: &std::path::Path) -> SubstrateStore {
    let s = SubstrateStore::create(dir).unwrap();
    let labels: Vec<&str> = (0..NOEUDS).map(|_| "File").collect();
    s.bulk_import_labels_edges(&labels, &aretes()).unwrap();
    let possedees: Vec<Vec<(String, Value)>> = (0..ARETES).map(props_de).collect();
    let refs: Vec<Vec<(&str, Value)>> = possedees
        .iter()
        .map(|l| l.iter().map(|(k, v)| (k.as_str(), v.clone())).collect())
        .collect();
    // En deux paquets, pour éprouver `premier_slot`.
    let moitie = refs.len() / 2;
    s.bulk_import_edge_props(1, &refs[..moitie]).unwrap();
    s.bulk_import_edge_props(1 + moitie as u32, &refs[moitie..])
        .unwrap();
    s
}

#[test]
fn le_lot_d_aretes_relit_toutes_les_valeurs_apres_reouverture() {
    let d = tempfile::tempdir().unwrap();
    {
        let s = en_lot(d.path());
        s.checkpoint().unwrap();
    }
    let s = SubstrateStore::open(d.path()).unwrap();
    let mut comparees = 0u64;
    for i in 0..ARETES {
        let id = EdgeId(u64::from(i) + 1);
        for (k, v) in props_de(i) {
            let lu = s.get_edge_property(id, &PropertyKey::new(k.as_str()));
            assert_eq!(lu.as_ref(), Some(&v), "arête {}, {k}", i + 1);
            comparees += 1;
        }
    }
    assert!(
        comparees > 250_000,
        "le jeu couvre bien des centaines de milliers de valeurs ({comparees})"
    );
}

#[test]
fn une_arete_absente_ne_recoit_rien_et_le_lot_ne_plante_pas() {
    let d = tempfile::tempdir().unwrap();
    let s = SubstrateStore::create(d.path()).unwrap();
    let labels: Vec<&str> = vec!["File"; 3];
    s.bulk_import_labels_edges(
        &labels,
        &[(1, 2, "CALLS"), (2, 3, "CALLS"), (3, 1, "CALLS")],
    )
    .unwrap();
    // L'arête 2 disparaît avant le lot : elle est tombstonée sur disque.
    assert!(s.delete_edge(EdgeId(2)));
    let props: Vec<Vec<(&str, Value)>> = vec![
        vec![("line", Value::Int64(1))],
        vec![("line", Value::Int64(2))],
        vec![("line", Value::Int64(3))],
        // Slot au-delà du plus haut slot d'arête : absent aussi.
        vec![("line", Value::Int64(4))],
    ];
    s.bulk_import_edge_props(1, &props).unwrap();
    let cle = PropertyKey::new("line");
    assert_eq!(s.get_edge_property(EdgeId(1), &cle), Some(Value::Int64(1)));
    assert_eq!(
        s.get_edge_property(EdgeId(2), &cle),
        None,
        "arête supprimée"
    );
    assert_eq!(s.get_edge_property(EdgeId(3), &cle), Some(Value::Int64(3)));
    assert_eq!(
        s.get_edge_property(EdgeId(4), &cle),
        None,
        "slot hors plage"
    );
}

#[test]
fn une_arete_qui_porte_deja_des_valeurs_garde_les_siennes_et_la_derniere_gagne() {
    let d = tempfile::tempdir().unwrap();
    let s = SubstrateStore::create(d.path()).unwrap();
    let labels: Vec<&str> = vec!["File"; 2];
    s.bulk_import_labels_edges(&labels, &[(1, 2, "CALLS"), (2, 1, "CALLS")])
        .unwrap();
    // Arête 1 : déjà une valeur par le chemin unitaire.
    s.set_edge_property(EdgeId(1), "ancienne", Value::Int64(10));
    let props: Vec<Vec<(&str, Value)>> = vec![
        // Clé répétée dans le lot : la dernière valeur doit gagner.
        vec![
            ("nouvelle", Value::Int64(1)),
            ("nouvelle", Value::Int64(5)),
            ("autre", Value::Bool(true)),
        ],
        vec![("nouvelle", Value::Int64(7))],
    ];
    s.bulk_import_edge_props(1, &props).unwrap();
    assert_eq!(
        s.get_edge_property(EdgeId(1), &PropertyKey::new("ancienne")),
        Some(Value::Int64(10)),
        "la valeur déjà écrite est conservée"
    );
    assert_eq!(
        s.get_edge_property(EdgeId(1), &PropertyKey::new("nouvelle")),
        Some(Value::Int64(5)),
        "la dernière valeur du lot l'emporte"
    );
    assert_eq!(
        s.get_edge_property(EdgeId(1), &PropertyKey::new("autre")),
        Some(Value::Bool(true))
    );
    assert_eq!(
        s.get_edge_property(EdgeId(2), &PropertyKey::new("nouvelle")),
        Some(Value::Int64(7))
    );
}
