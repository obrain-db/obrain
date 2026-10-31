//! ROLLBACK n'efface que les créations de SA transaction.
//!
//! (2026-10-31) Mesuré sur n3 avant correctif : un ROLLBACK supprimait toute la
//! plage d'identifiants allouée depuis son `begin`, propriétaires compris — un
//! ROLLBACK d'une transaction VIDE effaçait le commit d'une session voisine.
//! Ces tests sont déterministes (deux sessions, un thread) : ils tombaient tous
//! avant le correctif, et ils tombent sans lui.

use obrain_engine::ObrainDB;

fn visibles(db: &ObrainDB, requete: &str) -> usize {
    db.session().execute(requete).unwrap().row_count()
}

/// Le cas le plus nu : B ouvre une transaction, A crée et commite, B annule
/// SANS avoir rien écrit. Le nœud de A doit survivre.
#[test]
fn un_rollback_vide_ne_touche_pas_au_commit_d_une_autre_session() {
    let db = ObrainDB::new_in_memory();
    let mut b = db.session();
    b.begin_transaction().unwrap();
    let mut a = db.session();
    a.begin_transaction().unwrap();
    a.execute("INSERT (:Iso {who: 'A'})").unwrap();
    a.commit().unwrap();
    assert_eq!(visibles(&db, "MATCH (n:Iso) RETURN n"), 1);
    b.rollback().unwrap();
    assert_eq!(
        visibles(&db, "MATCH (n:Iso) RETURN n"),
        1,
        "le ROLLBACK vide de B a effacé le nœud commité par A"
    );
}

/// B a inséré puis annule : SON nœud disparaît, celui de A reste.
#[test]
fn un_rollback_ne_retire_que_ses_propres_noeuds() {
    let db = ObrainDB::new_in_memory();
    let mut b = db.session();
    b.begin_transaction().unwrap();
    b.execute("INSERT (:Iso {who: 'B'})").unwrap();
    let mut a = db.session();
    a.begin_transaction().unwrap();
    a.execute("INSERT (:Iso {who: 'A'})").unwrap();
    a.commit().unwrap();
    b.rollback().unwrap();
    assert_eq!(
        visibles(&db, "MATCH (n:Iso {who: 'A'}) RETURN n"),
        1,
        "A perdu"
    );
    assert_eq!(
        visibles(&db, "MATCH (n:Iso {who: 'B'}) RETURN n"),
        0,
        "B survivant"
    );
}

/// Même contrat pour les arêtes : A crée deux nœuds et une arête, commite ; B
/// (ouverte avant, avec une arête à elle) annule. L'arête de A survit, celle
/// de B non.
#[test]
fn un_rollback_ne_retire_que_ses_propres_aretes() {
    let db = ObrainDB::new_in_memory();
    db.session()
        .execute("INSERT (:P {k: 'b1'}), (:P {k: 'b2'})")
        .unwrap();
    let mut b = db.session();
    b.begin_transaction().unwrap();
    b.execute("MATCH (x:P {k: 'b1'}), (y:P {k: 'b2'}) INSERT (x)-[:LIEN {who: 'B'}]->(y)")
        .unwrap();
    let mut a = db.session();
    a.begin_transaction().unwrap();
    a.execute("INSERT (:P {k: 'a1'})-[:LIEN {who: 'A'}]->(:P {k: 'a2'})")
        .unwrap();
    a.commit().unwrap();
    b.rollback().unwrap();
    assert_eq!(
        visibles(&db, "MATCH ()-[r:LIEN {who: 'A'}]->() RETURN r"),
        1,
        "l'arête commitée par A a été effacée par le ROLLBACK de B"
    );
    assert_eq!(
        visibles(&db, "MATCH ()-[r:LIEN {who: 'B'}]->() RETURN r"),
        0,
        "l'arête annulée par B survit"
    );
    assert_eq!(visibles(&db, "MATCH (n:P) RETURN n"), 4);
}

/// ROLLBACK TO SAVEPOINT passe par la même plage : A commite entre le
/// SAVEPOINT de B et son retour en arrière. Le nœud de A survit ; le nœud
/// que B a créé après le SAVEPOINT disparaît ; celui d'avant reste (jusqu'au
/// commit de B).
#[test]
fn un_retour_a_un_savepoint_ne_retire_que_ses_propres_creations() {
    let db = ObrainDB::new_in_memory();
    let mut b = db.session();
    b.begin_transaction().unwrap();
    b.execute("INSERT (:Iso {who: 'B-avant'})").unwrap();
    b.savepoint("sp").unwrap();
    b.execute("INSERT (:Iso {who: 'B-apres'})").unwrap();
    let mut a = db.session();
    a.begin_transaction().unwrap();
    a.execute("INSERT (:Iso {who: 'A'})").unwrap();
    a.commit().unwrap();
    b.rollback_to_savepoint("sp").unwrap();
    assert_eq!(
        visibles(&db, "MATCH (n:Iso {who: 'A'}) RETURN n"),
        1,
        "le retour au SAVEPOINT de B a effacé le nœud commité par A"
    );
    b.commit().unwrap();
    assert_eq!(visibles(&db, "MATCH (n:Iso {who: 'B-avant'}) RETURN n"), 1);
    assert_eq!(visibles(&db, "MATCH (n:Iso {who: 'B-apres'}) RETURN n"), 0);
}
