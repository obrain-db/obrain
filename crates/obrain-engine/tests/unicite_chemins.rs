//! L'unicité tient su(vérifier-puis-écrire) ; les chemins d'écriture que #99 ne couvre pas.
//!
//! #99 garantit l'unicité du tuple à l'`INSERT`, à `SET` (propriété, carte) et
//! à `MERGE` (création et `ON MATCH SET`) : voir `contrainte_set.rs`. Restaient,
//! sur `main` comme sur #99 :
//!
//! 1. l'ajout de label (`SET n:T`) : le nœud entre sous les contraintes de `T` ;
//! 2. `CREATE CONSTRAINT` sur des données qui la violent déjà ;
//! 3. deux `INSERT` concurrents du même tuple (vérifier-puis-écrire) ;
//! 4. l'API directe du store et l'import en masse : hors contrainte PAR
//!    CONTRAT, avec une vérification a posteriori
//!    ([`ObrainDB::verifier_contraintes`], `unicite_verification.rs`).
//!
//! Entier contre flottant et dates de fuseaux différents : voir
//! `unicite_valeurs_egales.rs`.

#![cfg(feature = "cypher")]

use std::sync::Arc;

use obrain_engine::{ObrainDB, Session};

fn base(contraintes: &[&str]) -> ObrainDB {
    let db = ObrainDB::new_in_memory();
    let session = db.session();
    for c in contraintes {
        session.execute_cypher(c).expect("CREATE CONSTRAINT");
    }
    db
}

const U_A: &str = "CREATE CONSTRAINT u_a FOR (n:T) REQUIRE n.a IS UNIQUE";
const U_AB: &str = "CREATE CONSTRAINT u_ab FOR (n:T) REQUIRE (n.a, n.b) IS UNIQUE";

/// Nombre de nœuds `:T` qui répondent au motif `{…}` donné.
fn nombre(session: &Session, motif: &str) -> usize {
    session
        .execute(&format!("MATCH (n:T {motif}) RETURN n.a"))
        .expect("MATCH de comptage")
        .rows
        .len()
}

/// `INSERT` est du GQL, le reste (`MATCH … SET`, `MERGE`) du Cypher.
fn lancer(
    session: &Session,
    requete: &str,
) -> obrain_common::utils::error::Result<obrain_engine::database::QueryResult> {
    if requete.trim_start().starts_with("INSERT") {
        session.execute(requete)
    } else {
        session.execute_cypher(requete)
    }
}

fn refuse(session: &Session, requete: &str, pourquoi: &str) {
    let r = lancer(session, requete);
    assert!(r.is_err(), "{pourquoi} — `{requete}` a répondu {r:?}");
    let texte = r.unwrap_err().to_string();
    assert!(
        texte.contains("UNIQUE"),
        "{pourquoi} — l'erreur doit nommer la contrainte d'unicité, reçu : {texte}"
    );
}

fn accepte(session: &Session, requete: &str, pourquoi: &str) {
    let r = lancer(session, requete);
    assert!(
        r.is_ok(),
        "{pourquoi} — `{requete}` a échoué : {:?}",
        r.err()
    );
}

/// Donner le label `T` à un nœud le fait entrer sous les contraintes de `T`.
#[test]
fn ajout_de_label_est_controle() {
    let db = base(&[U_AB]);
    let s = db.session();
    s.execute("INSERT (:T {a: 1, b: 'x'})").unwrap();
    s.execute("INSERT (:Autre {a: 1, b: 'x', qui: 'doublon'})")
        .unwrap();
    s.execute("INSERT (:Autre {a: 1, b: 'z', qui: 'neuf'})")
        .unwrap();

    refuse(
        &s,
        "MATCH (n:Autre {qui: 'doublon'}) SET n:T",
        "le nœud porte un tuple déjà pris sous :T",
    );
    assert_eq!(nombre(&s, "{a: 1, b: 'x'}"), 1, "le label n'a pas été posé");

    accepte(
        &s,
        "MATCH (n:Autre {qui: 'neuf'}) SET n:T",
        "tuple libre sous :T",
    );
    assert_eq!(nombre(&s, "{a: 1, b: 'z'}"), 1);
    // Reposer un label déjà porté : le nœud ne se heurte pas à lui-même.
    accepte(&s, "MATCH (n:T {b: 'z'}) SET n:T", "label déjà porté");
}

// ─────────────── 3. CREATE CONSTRAINT sur des données en violation : voir
// `contrainte_donnees_existantes.rs` (remonté dans la PR de base) ───────────────

/// Suppression puis recréation : la place est libérée ; `REMOVE` sort le
/// nœud de la contrainte ; un `SET` sur une relation ne consulte pas les
/// contraintes de nœud ; deux nœuds identiques dans UNE instruction ne passent
/// pas tous les deux.
#[test]
fn delete_remove_relation_et_instruction_unique() {
    let db = base(&[U_A]);
    let s = db.session();
    s.execute("INSERT (:T {a: 1, m: 'premier'})").unwrap();
    refuse(&s, "INSERT (:T {a: 1})", "a = 1 est pris");
    s.execute_cypher("MATCH (n:T {a: 1}) DELETE n").unwrap();
    accepte(&s, "INSERT (:T {a: 1, m: 'second'})", "la place est libre");

    // REMOVE : le nœud sort de la contrainte, un autre peut prendre a = 1 ;
    // lui rendre a = 1 ensuite est refusé.
    accepte(&s, "MATCH (n:T {a: 1}) REMOVE n.a", "REMOVE n.a");
    accepte(&s, "INSERT (:T {a: 1, m: 'troisieme'})", "a = 1 est libre");
    refuse(
        &s,
        "MATCH (n:T {m: 'second'}) SET n.a = 1",
        "a = 1 est de nouveau pris",
    );

    // SET sur une relation : aucune contrainte de nœud en jeu.
    s.execute("INSERT (:T {a: 50})-[:R]->(:T {a: 51})").unwrap();
    accepte(
        &s,
        "MATCH (:T {a: 50})-[r:R]->() SET r.poids = 3",
        "SET relation",
    );

    // Deux nœuds de même valeur dans UNE instruction.
    let r = s.execute("INSERT (:T {a: 99}), (:T {a: 99})");
    assert!(r.is_err(), "deux a = 99 dans une instruction : {r:?}");
    assert!(nombre(&s, "{a: 99}") <= 1, "au plus un porteur de a = 99");
}

// ─────────────── 4. deux INSERT concurrents du même tuple ───────────────

/// Vérifier-puis-écrire doit être atomique : de deux fils qui insèrent le
/// même tuple au même instant, UN SEUL passe. Répété, car la fenêtre est
/// étroite.
#[test]
fn deux_insert_concurrents_du_meme_tuple_ne_passent_pas_tous_les_deux() {
    deux_inserts_concurrents(Arc::new(base(&[U_AB])));
}

/// La même garantie sur une base SUR DISQUE, WAL durable (fsync à chaque
/// commit) : la fenêtre entre contrôle et écriture y est plus large.
#[test]
fn deux_insert_concurrents_sur_disque_wal_durable() {
    let td = tempfile::tempdir().unwrap();
    let db = ObrainDB::open_substrate(td.path().join("c")).expect("ouverture");
    db.session()
        .execute_cypher(U_AB)
        .expect("CREATE CONSTRAINT");
    deux_inserts_concurrents(Arc::new(db));
}

fn deux_inserts_concurrents(db: Arc<ObrainDB>) {
    const MANCHES: usize = 400;
    const FILS: usize = 2;
    let depart = Arc::new(std::sync::Barrier::new(FILS));
    let mut fils = Vec::new();
    for _ in 0..FILS {
        let db = Arc::clone(&db);
        let depart = Arc::clone(&depart);
        fils.push(std::thread::spawn(move || {
            let s = db.session();
            let mut reussites = Vec::with_capacity(MANCHES);
            for manche in 0..MANCHES {
                depart.wait();
                let r = s.execute(&format!("INSERT (:T {{a: {manche}, b: 'x'}})"));
                reussites.push(r.is_ok());
            }
            reussites
        }));
    }
    let resultats: Vec<Vec<bool>> = fils.into_iter().map(|f| f.join().unwrap()).collect();
    eprintln!(
        "deux INSERT concurrents : {MANCHES} manches x {FILS} fils, réussites par fil = {:?}",
        resultats
            .iter()
            .map(|r| r.iter().filter(|ok| **ok).count())
            .collect::<Vec<_>>()
    );

    let s = db.session();
    let mut doubles = Vec::new();
    let mut aucun = Vec::new();
    for manche in 0..MANCHES {
        let passes = resultats.iter().filter(|r| r[manche]).count();
        let porteurs = nombre(&s, &format!("{{a: {manche}, b: 'x'}}"));
        if passes != 1 || porteurs != 1 {
            if porteurs > 1 {
                doubles.push(manche);
            } else {
                aucun.push((manche, passes, porteurs));
            }
        }
    }
    eprintln!(
        "deux INSERT concurrents : manches avec deux porteurs = {}",
        doubles.len()
    );
    assert!(
        doubles.is_empty(),
        "{} manches sur {MANCHES} ont laissé DEUX porteurs du même tuple (ex. {:?})",
        doubles.len(),
        &doubles[..doubles.len().min(5)]
    );
    assert!(
        aucun.is_empty(),
        "manches sans exactement un vainqueur (manche, réussites, porteurs) : {:?}",
        &aucun[..aucun.len().min(5)]
    );
}

/// Même garantie quand l'un des deux chemins est un `SET`.
#[test]
fn insert_et_set_concurrents_ne_convergent_pas() {
    insert_et_set_concurrents(Arc::new(base(&[U_A])));
}

/// Idem sur disque, WAL durable.
#[test]
fn insert_et_set_concurrents_sur_disque_wal_durable() {
    let td = tempfile::tempdir().unwrap();
    let db = ObrainDB::open_substrate(td.path().join("c")).expect("ouverture");
    db.session().execute_cypher(U_A).expect("CREATE CONSTRAINT");
    insert_et_set_concurrents(Arc::new(db));
}

fn insert_et_set_concurrents(db: Arc<ObrainDB>) {
    const MANCHES: usize = 300;
    {
        let s = db.session();
        for manche in 0..MANCHES {
            s.execute(&format!(
                "INSERT (:T {{a: -{}, source: {manche}}})",
                manche + 1
            ))
            .unwrap();
        }
    }
    let depart = Arc::new(std::sync::Barrier::new(2));
    let (db1, d1) = (Arc::clone(&db), Arc::clone(&depart));
    let inserteur = std::thread::spawn(move || {
        let s = db1.session();
        for manche in 0..MANCHES {
            d1.wait();
            let _ = s.execute(&format!("INSERT (:T {{a: {manche}}})"));
        }
    });
    let (db2, d2) = (Arc::clone(&db), Arc::clone(&depart));
    let modifieur = std::thread::spawn(move || {
        let s = db2.session();
        for manche in 0..MANCHES {
            d2.wait();
            let _ = s.execute_cypher(&format!(
                "MATCH (n:T {{source: {manche}}}) SET n.a = {manche}"
            ));
        }
    });
    inserteur.join().unwrap();
    modifieur.join().unwrap();

    let s = db.session();
    let doubles: Vec<usize> = (0..MANCHES)
        .filter(|m| nombre(&s, &format!("{{a: {m}}}")) > 1)
        .collect();
    assert!(
        doubles.is_empty(),
        "{} manches ont laissé deux porteurs de la même valeur (ex. {:?})",
        doubles.len(),
        &doubles[..doubles.len().min(5)]
    );
}
