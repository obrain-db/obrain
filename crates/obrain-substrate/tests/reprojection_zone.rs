//! Un lecteur ouvert AVANT qu'un écrivain agrandisse la zone de propriétés
//! doit lire les valeurs écrites APRÈS — sans être rouvert.
//!
//! Symptôme mesuré le 2026-09-16 sur runtime-dev : « StringRef {page_id=18073}
//! out of range » chez les lecteurs au long cours (viz, harness), alors que le
//! tas comptait 27 938 pages sur disque — la propriété disparaissait de la vue.

use obrain_common::types::{NodeId, PropertyKey, Value};
use obrain_core::graph::traits::{GraphStore, GraphStoreMut};
use obrain_substrate::SubstrateStore;

#[test]
fn un_lecteur_voit_ce_qu_un_ecrivain_ecrit_apres_son_ouverture() {
    let d = tempfile::tempdir().unwrap();
    let n = 50u64;
    {
        let s = SubstrateStore::create(d.path()).unwrap();
        for i in 0..n {
            let id = s.create_node(&["Function"]);
            s.set_node_property(id, "qualified_name", Value::String(format!("q{i}").into()));
        }
        s.checkpoint().unwrap();
    }
    let ecrivain = SubstrateStore::open_shared_write(d.path()).unwrap();
    let lecteur = SubstrateStore::open_read_only(d.path()).unwrap();

    // L'écrivain fait grandir le tas et les pages bien au-delà de la
    // projection du lecteur (≈ 2 Mo de chaînes).
    for i in 0..n {
        let id = NodeId(i + 1);
        for j in 0..40 {
            ecrivain.set_node_property(
                id,
                &format!("longue_{j}"),
                Value::String(format!("{i}-{j}-{}", "x".repeat(200)).into()),
            );
        }
        // Clé EXISTANTE chez le lecteur (comme `qualified_name` dans le
        // harness) : sa nouvelle valeur vit dans une page de tas récente.
        ecrivain.set_node_property(
            id,
            "qualified_name",
            Value::String(format!("tard-{i}").into()),
        );
    }
    ecrivain.flush().ok();

    let mut vues = 0;
    for i in 0..n {
        let v = lecteur.get_node_property(NodeId(i + 1), &PropertyKey::new("qualified_name"));
        if v == Some(Value::String(format!("tard-{i}").into())) {
            vues += 1;
        }
    }
    assert_eq!(
        vues, n,
        "le lecteur doit voir les propriétés écrites après son ouverture"
    );
}
