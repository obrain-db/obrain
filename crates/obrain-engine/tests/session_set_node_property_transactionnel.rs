//! obrain#120 pt 4 — `Session::set_node_property` est transactionnel : annulé
//! par `rollback()`, conservé par `commit()`, immédiat hors transaction.

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

#[test]
fn set_node_property_annule_par_rollback() {
    let db = ObrainDB::new_in_memory();
    let mut s = db.session();
    let id = s.create_node_with_props(&["Compte"], [("solde", Value::Int64(1000))]);

    s.begin_transaction().unwrap();
    s.set_node_property(id, "solde", Value::Int64(9999));
    s.set_node_property(id, "nouvelle", Value::Int64(1));
    assert_eq!(s.get_node_property(id, "solde"), Some(Value::Int64(9999)));
    s.rollback().unwrap();

    assert_eq!(s.get_node_property(id, "solde"), Some(Value::Int64(1000)));
    assert_eq!(s.get_node_property(id, "nouvelle"), None);
}

#[test]
fn set_node_property_conserve_par_commit() {
    let db = ObrainDB::new_in_memory();
    let mut s = db.session();
    let id = s.create_node_with_props(&["Compte"], [("solde", Value::Int64(1000))]);

    s.begin_transaction().unwrap();
    s.set_node_property(id, "solde", Value::Int64(9999));
    s.commit().unwrap();

    assert_eq!(s.get_node_property(id, "solde"), Some(Value::Int64(9999)));
}

#[test]
fn set_node_property_hors_transaction_est_immediat() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    let id = s.create_node(&["Compte"]);
    s.set_node_property(id, "solde", Value::Int64(7));
    assert_eq!(s.get_node_property(id, "solde"), Some(Value::Int64(7)));
}
