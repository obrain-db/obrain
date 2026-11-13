//! L'API directe du store et l'import en masse sont HORS CONTRAINTE par
//! contrat : ils n'appliquent aucun contrôle d'unicité (c'est ce qui permet
//! une migration de plusieurs millions de nœuds). La contrepartie est une
//! vérification a posteriori, [`ObrainDB::verifier_contraintes`], qui dit si
//! les contraintes déclarées tiennent sur la base et nomme ce qui les viole.

#![cfg(feature = "cypher")]

use obrain_common::types::{PropertyKey, Value};
use obrain_engine::ObrainDB;

fn base(contraintes: &[&str]) -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let session = db.session();
    for c in contraintes {
        session.execute_cypher(c).expect("CREATE CONSTRAINT");
    }
    db
}

const U_AB: &str = "CREATE CONSTRAINT u_ab FOR (n:T) REQUIRE (n.a, n.b) IS UNIQUE";

/// L'API directe du store et l'import en masse n'appliquent PAS les
/// contraintes (contrat documenté) ; `verifier_contraintes` dit après coup si
/// elles tiennent, et nomme ce qui les viole.
#[test]
fn verification_a_posteriori_trouve_les_violations_d_un_import() {
    let db = base(&[
        U_AB,
        "CREATE CONSTRAINT r FOR (n:T) REQUIRE n.requis IS NOT NULL",
        "CREATE CONSTRAINT k FOR (n:K) REQUIRE (n.id) IS NODE KEY",
    ]);
    assert!(
        db.verifier_contraintes().violations.is_empty(),
        "base vide : rien à signaler"
    );

    let store = db.graph_store();
    let p = |k: &str, v: Value| (PropertyKey::new(k), v);
    // Import « en masse » par l'API directe : 500 nœuds sains…
    for i in 0..500i64 {
        store.create_node_with_props(
            &["T"],
            &[
                p("a", Value::from(i % 7)),
                p("b", Value::from(format!("v{i}"))),
                p("requis", Value::from(true)),
            ],
        );
        store.create_node_with_props(&["K"], &[p("id", Value::from(i))]);
    }
    let rapport = db.verifier_contraintes();
    assert!(
        rapport.violations.is_empty(),
        "import sain : {:?}",
        rapport.violations
    );
    assert_eq!(rapport.noeuds_examines, 1000);

    // …puis ce que les contraintes auraient refusé.
    let d1 = store.create_node_with_props(
        &["T"],
        &[
            p("a", Value::from(3i64)),
            p("b", Value::from("v3")),
            p("requis", Value::from(true)),
        ],
    );
    let sans_requis = store.create_node_with_props(
        &["T"],
        &[p("a", Value::from(100i64)), p("b", Value::from("seul"))],
    );
    let cle_double = store.create_node_with_props(&["K"], &[p("id", Value::from(42i64))]);
    let cle_absente = store.create_node_with_props(&["K"], &[p("autre", Value::from(1i64))]);

    let rapport = db.verifier_contraintes();
    let v = &rapport.violations;
    assert_eq!(v.len(), 4, "quatre violations attendues : {v:?}");
    let trouve = |nom: &str, noeud: obrain_common::types::NodeId| {
        v.iter().any(|x| x.contrainte == nom && x.noeud == noeud)
    };
    assert!(trouve("u_ab", d1), "doublon de tuple : {v:?}");
    assert!(trouve("r", sans_requis), "NOT NULL : {v:?}");
    assert!(trouve("k", cle_double), "NODE KEY en double : {v:?}");
    assert!(trouve("k", cle_absente), "NODE KEY incomplet : {v:?}");
    // Un doublon nomme le nœud qu'il répète.
    let doublon = v.iter().find(|x| x.contrainte == "u_ab").unwrap();
    assert!(doublon.premier_porteur.is_some(), "{doublon:?}");
    assert!(doublon.to_string().contains("v3"), "{doublon}");
}
