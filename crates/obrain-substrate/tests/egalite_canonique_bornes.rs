//! L'égalité CANONIQUE de l'index d'égalité, par l'API PUBLIQUE seule (PR #101).
//!
//! Ces scénarios n'utilisent aucune sonde interne : ils se rejouent à
//! l'identique sur `main`, sur une tête antérieure et sur la branche, ce qui
//! établit pour chacun s'il échoue SANS le correctif et réussit AVEC.
//!
//! Trois familles :
//! * entier contre flottant entier, et même instant sous deux écritures
//!   (le défaut d'origine : la clé d'index était plus stricte que l'égalité) ;
//! * dates DISTINCTES qui ne doivent pas devenir égales (un instant qui
//!   sature au-delà de 2262 ou se tronque sous la microseconde confondait
//!   des dates différentes — défaut introduit par une clé canonique naïve) ;
//! * existence : la série d'une propriété déclarée porte TOUS ses porteurs.

use obrain_common::types::{NodeId, Value};
use obrain_core::graph::traits::{GraphStore, GraphStoreMut};
use obrain_substrate::SubstrateStore;

fn store() -> (tempfile::TempDir, SubstrateStore) {
    let td = tempfile::tempdir().unwrap();
    let s = SubstrateStore::create(td.path().join("kb")).unwrap();
    (td, s)
}

#[test]
fn entier_et_flottant_entier_dans_les_deux_ordres() {
    let (_td, s) = store();
    s.create_property_index("a");
    s.create_property_index("b");
    let n = s.create_node(&["Pair"]);
    s.set_node_property(n, "a", Value::Int64(1));
    s.set_node_property(n, "b", Value::Float64(5.0));
    let m = s.create_node(&["Pair"]);
    s.set_node_property(m, "a", Value::Int64(2));
    s.set_node_property(m, "b", Value::Int64(7));
    for conditions in [
        [("a", Value::Int64(1)), ("b", Value::Int64(5))],
        [("b", Value::Int64(5)), ("a", Value::Int64(1))],
    ] {
        assert_eq!(s.find_nodes_by_properties(&conditions), vec![n]);
    }
    for conditions in [
        [("a", Value::Int64(2)), ("b", Value::Float64(7.0))],
        [("b", Value::Float64(7.0)), ("a", Value::Int64(2))],
    ] {
        assert_eq!(s.find_nodes_by_properties(&conditions), vec![m]);
    }
    assert_eq!(s.find_nodes_by_property("b", &Value::Int64(5)), vec![n]);
    assert_eq!(s.find_nodes_by_property("b", &Value::Float64(7.0)), vec![m]);
    assert!(
        s.find_nodes_by_property("b", &Value::Float64(5.5))
            .is_empty()
    );
}

#[test]
fn meme_instant_sous_deux_ecritures() {
    let (_td, s) = store();
    s.create_property_index("a");
    s.create_property_index("d");
    let n = s.create_node(&["Evt"]);
    s.set_node_property(n, "a", Value::Int64(1));
    s.set_node_property(n, "d", Value::from("2024-01-01T10:00:00Z"));
    let autre_ecriture = Value::from("2024-01-01T12:00:00+02:00");
    assert_eq!(s.find_nodes_by_property("d", &autre_ecriture), vec![n]);
    for conditions in [
        [("a", Value::Int64(1)), ("d", autre_ecriture.clone())],
        [("d", autre_ecriture.clone()), ("a", Value::Int64(1))],
    ] {
        assert_eq!(s.find_nodes_by_properties(&conditions), vec![n]);
    }
}

#[test]
fn dates_distinctes_restent_distinctes() {
    let (_td, s) = store();
    s.create_property_index("d");
    let dates = [
        "3000-01-01T00:00:00Z",
        "9999-12-31T23:59:59Z",
        "1500-06-01T00:00:00Z",
        "1000-06-01T00:00:00Z",
        "2024-01-01T00:00:00.0000001Z",
        "2024-01-01T00:00:00.0000002Z",
    ];
    let ids: Vec<NodeId> = dates
        .iter()
        .map(|d| {
            let id = s.create_node(&["Evt"]);
            s.set_node_property(id, "d", Value::from(*d));
            id
        })
        .collect();
    for (d, id) in dates.iter().zip(&ids) {
        assert_eq!(
            s.find_nodes_by_property("d", &Value::from(*d)),
            vec![*id],
            "`{d}` ne doit trouver que son porteur"
        );
    }
}

#[test]
fn existence_voit_tous_les_porteurs() {
    let td = tempfile::tempdir().unwrap();
    let racine = td.path().join("kb");
    let porteurs: Vec<NodeId>;
    {
        let s = SubstrateStore::create(&racine).unwrap();
        s.create_property_index("p");
        let valeurs = [
            Value::Int64(1),
            Value::Float64(0.5),
            Value::Null,
            Value::from("texte"),
        ];
        porteurs = valeurs
            .into_iter()
            .map(|v| {
                let id = s.create_node(&["N"]);
                s.set_node_property(id, "p", v);
                id
            })
            .collect();
        let _sans = s.create_node(&["N"]);
        assert_eq!(s.find_nodes_with_property("p"), Some(porteurs.clone()));
        s.flush().unwrap();
    }
    // Après RÉOUVERTURE, les valeurs ne sont plus dans la carte de session :
    // seule la série persistée répond. C'est là que les porteurs de valeurs
    // non projetables disparaissaient.
    let s = SubstrateStore::open(&racine).unwrap();
    assert_eq!(
        s.find_nodes_with_property("p"),
        Some(porteurs),
        "après réouverture, la série doit porter TOUS les porteurs"
    );
}
