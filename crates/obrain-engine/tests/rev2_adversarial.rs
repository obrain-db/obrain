//! HARNAIS DE LA REVUE ADVERSARIALE — VAGUE 2 (croisement C×D).
//!
//! Fichier de revue : il éprouve les bords des correctifs C (parseur-queue)
//! et D (REMOVE réel / null résolu = retrait) sur l'arbre FUSIONNÉ.
//!
//! Angles couverts :
//!   1. Croisement C×D — clauses SET/REMOVE entrelacées (passe ordonnée de C)
//!      doivent produire un retrait RÉEL de clé, pas un NULL stocké.
//!   2. Sémantique « null résolu = retrait » de D — copie d'une propriété
//!      absente, carte avec valeur nulle dynamique.
//!   3. Refus de queue de C — la tolérance des `;` terminaux ne doit avaler
//!      aucun autre jeton d'erreur ; WITH…SET refusé franchement ; bords du
//!      mot-clef optionnel PROPERTY d'ALTER.
//!
//! Run : cargo test -p obrain-engine --test rev2_adversarial

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

// ---------------------------------------------------------------------------
// Outils (repris de gql_13_ecriture.rs).
// ---------------------------------------------------------------------------

fn ecrire(db: &ObrainDB, requetes: &[&str]) {
    let mut session = db.session();
    session.begin_transaction().unwrap();
    for r in requetes {
        session
            .execute(r)
            .unwrap_or_else(|e| panic!("l'écriture `{r}` doit s'exécuter, erreur : {e:?}"));
    }
    session.commit().unwrap();
}

fn lire(db: &ObrainDB, requete: &str) -> Vec<Vec<Value>> {
    let session = db.session();
    session
        .execute(requete)
        .unwrap_or_else(|e| panic!("la lecture `{requete}` doit s'exécuter, erreur : {e:?}"))
        .rows
}

fn scalaire(db: &ObrainDB, requete: &str) -> Value {
    let rows = lire(db, requete);
    assert_eq!(
        rows.len(),
        1,
        "`{requete}` doit rendre EXACTEMENT une ligne, obtenu {}",
        rows.len()
    );
    rows[0][0].clone()
}

fn en_ensemble(v: &Value) -> Vec<String> {
    match v {
        Value::List(items) => {
            let mut cles: Vec<String> = items
                .iter()
                .map(|i| match i {
                    Value::String(s) => s.to_string(),
                    autre => panic!("une clé doit être une chaîne, obtenu {autre:?}"),
                })
                .collect();
            cles.sort();
            cles
        }
        autre => panic!("keys() doit rendre une liste, obtenu {autre:?}"),
    }
}

fn cles(db: &ObrainDB, motif: &str) -> Vec<String> {
    en_ensemble(&scalaire(db, &format!("MATCH {motif} RETURN keys(n) AS c")))
}

/// Exécute `requete` et EXIGE un refus ; rend le texte de l'erreur.
fn erreur(db: &ObrainDB, requete: &str) -> String {
    let session = db.session();
    match session.execute(requete) {
        Ok(r) => panic!(
            "`{requete}` doit être REFUSÉE, elle a répondu {} ligne(s) — \
             la queue a été avalée en silence",
            r.rows.len()
        ),
        Err(e) => format!("{e:?}"),
    }
}

fn ens(items: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = items.iter().map(|s| s.to_string()).collect();
    v.sort();
    v
}

// ===========================================================================
// ANGLE 1 — CROISEMENT C×D : la passe ordonnée de C doit porter le REMOVE
// réel de D.
// ===========================================================================

/// SET et REMOVE entrelacés (SET, REMOVE, SET) : la traduction passe par le
/// bras `ast::QueryClause::Remove` de la passe ordonnée introduite par C.
/// Après exécution, la clé retirée doit avoir DISPARU de keys(n) — pas
/// survécu comme NULL stocké — et les deux SET encadrants doivent avoir agi.
#[test]
fn croisement_cd_remove_entrelace_retire_reellement_la_cle() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:T1 {a: 1, b: 2})"]);
    ecrire(&db, &["MATCH (n:T1) SET n.x = 1 REMOVE n.a SET n.y = 2"]);
    assert_eq!(
        cles(&db, "(n:T1)"),
        ens(&["b", "x", "y"]),
        "REMOVE n.a entrelacé entre deux SET doit RETIRER la clé a ; \
         x et y doivent être posées"
    );
}

/// Ordre d'écriture respecté : REMOVE puis SET de la MÊME clé — la clé doit
/// exister à la fin (le SET vient après). L'ancien code SET-puis-REMOVE
/// aurait rendu l'inverse.
#[test]
fn croisement_cd_remove_puis_set_meme_cle_dans_l_ordre_ecrit() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:T2 {a: 1})"]);
    ecrire(&db, &["MATCH (n:T2) REMOVE n.a SET n.a = 9"]);
    assert_eq!(
        cles(&db, "(n:T2)"),
        ens(&["a"]),
        "REMOVE n.a PUIS SET n.a = 9 : la clé doit exister à la fin"
    );
    assert_eq!(
        scalaire(&db, "MATCH (n:T2) RETURN n.a AS v"),
        Value::Int64(9)
    );
}

/// Le miroir : SET puis REMOVE de la même clé — la clé doit avoir disparu.
#[test]
fn croisement_cd_set_puis_remove_meme_cle_dans_l_ordre_ecrit() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:T3 {b: 1})"]);
    ecrire(&db, &["MATCH (n:T3) SET n.a = 9 REMOVE n.a"]);
    assert_eq!(
        cles(&db, "(n:T3)"),
        ens(&["b"]),
        "SET n.a = 9 PUIS REMOVE n.a : la clé a doit avoir disparu"
    );
}

/// Le croisement sur ARÊTE : REMOVE r.a entrelacé entre deux SET — passe
/// ordonnée de C + retrait réel de D + keys(r) sur arête (corrigé par D).
#[test]
fn croisement_cd_remove_entrelace_sur_arete() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:Ea)-[:R {a: 1, b: 2}]->(:Eb)"]);
    ecrire(&db, &["MATCH (:Ea)-[r:R]->(:Eb) SET r.x = 1 REMOVE r.a SET r.y = 2"]);
    let k = en_ensemble(&scalaire(
        &db,
        "MATCH (:Ea)-[r:R]->(:Eb) RETURN keys(r) AS c",
    ));
    assert_eq!(
        k,
        ens(&["b", "x", "y"]),
        "sur ARÊTE : REMOVE r.a entrelacé doit retirer la clé a ; x, y posées ; \
         keys(r) doit rendre l'ensemble exact (pas NULL)"
    );
}

// ===========================================================================
// ANGLE 2 — « NULL RÉSOLU = RETRAIT » : les perdants possibles.
// ===========================================================================

/// Copie d'une propriété possiblement absente : `SET t.a = s.b` où s.b est
/// ABSENTE. L'expression se résout à NULL à l'exécution ; openCypher (et la
/// règle de D) commandent le RETRAIT de la clé a si elle existait.
/// C'est LE changement de comportement observable : avant D, la clé a
/// survivait avec un NULL stocké (discriminable par keys()).
#[test]
fn null_resolu_copie_de_propriete_absente_retire_la_cle_cible() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &[
        "INSERT (:Src4 {id: 1})",              // b JAMAIS écrite
        "INSERT (:Dst4 {id: 1, a: 42, garde: 0})",
    ]);
    ecrire(&db, &["MATCH (s:Src4), (t:Dst4) SET t.a = s.b"]);
    let k = en_ensemble(&scalaire(
        &db,
        "MATCH (t:Dst4) RETURN keys(t) AS c",
    ));
    assert_eq!(
        k,
        ens(&["garde", "id"]),
        "SET t.a = s.b avec s.b ABSENTE se résout à NULL : la clé a doit \
         être RETIRÉE (openCypher), pas conservée nulle ni laissée intacte"
    );
}

/// `SET n += {{carte}}` avec entrée à NULL littéral : l'entrée nulle RETIRE
/// la clé si présente, ne pose JAMAIS la clé si absente ; les entrées non
/// nulles de la même carte sont posées.
#[test]
fn null_resolu_carte_litterale_retire_et_ne_pose_pas() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &[
        "INSERT (:M5 {present: 1, cible: 2})", // mort_nee JAMAIS écrite
    ]);
    ecrire(&db, &[
        "MATCH (n:M5) SET n += {cible: null, neuf: 7, mort_nee: null}",
    ]);
    assert_eq!(
        cles(&db, "(n:M5)"),
        ens(&["neuf", "present"]),
        "carte += : cible (NULL) doit être RETIRÉE, mort_nee (NULL sur clé \
         absente) ne doit PAS apparaître, neuf posée, present intacte"
    );
}

/// TÉMOIN (préexistant, hors périmètre D) : une carte `+=` à valeurs NON
/// littérales est refusée par le planificateur depuis la 0.1.0
/// (`planner/lpg/mutation.rs:1204`, « Unsupported expression type for
/// property source »). Le refus est franc — jamais un résultat faux en
/// silence. Si ce test casse un jour parce que la carte dynamique devient
/// SUPPORTÉE, il faudra alors éprouver la règle « NULL résolu = retrait »
/// sur ce chemin-là aussi.
#[test]
fn temoin_carte_dynamique_encore_refusee_par_le_planificateur() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:M5b {present: 1, cible: 2})"]);
    let msg = erreur(
        &db,
        "MATCH (n:M5b) SET n += {cible: n.fantome, neuf: 7}",
    );
    assert!(
        msg.contains("Unsupported expression type for property source"),
        "le refus attendu est celui du planificateur, obtenu : {msg}"
    );
    assert_eq!(
        cles(&db, "(n:M5b)"),
        ens(&["cible", "present"]),
        "une requête refusée ne doit laisser aucune trace"
    );
}

/// Auto-copie d'une clé absente : `SET n.a = n.a` où a est absente — ne doit
/// PAS matérialiser la clé (avant D, elle apparaissait nulle ou rien selon la
/// route ; la règle unifiée : rien).
#[test]
fn null_resolu_auto_copie_de_cle_absente_ne_materialise_rien() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:A6 {garde: 1})"]);
    ecrire(&db, &["MATCH (n:A6) SET n.a = n.a"]);
    assert_eq!(
        cles(&db, "(n:A6)"),
        ens(&["garde"]),
        "SET n.a = n.a avec a absente ne doit pas faire naître la clé a"
    );
}

// ===========================================================================
// ANGLE 3 — REFUS DE QUEUE : les bords de la tolérance `;`.
// ===========================================================================

/// Le `;` terminal reste toléré — seul cas admis de jeton d'erreur en queue.
#[test]
fn queue_point_virgule_terminal_tolere() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:Q7 {x: 1})"]);
    let rows = lire(&db, "MATCH (n:Q7) RETURN n.x AS x;");
    assert_eq!(rows.len(), 1, "un `;` final doit rester accepté");
    let rows = lire(&db, "MATCH (n:Q7) RETURN n.x AS x ;;");
    assert_eq!(rows.len(), 1, "plusieurs `;` finaux doivent rester acceptés");
}

/// Un jeton d'erreur du lexer qui n'est PAS `;` ne doit jamais être avalé.
#[test]
fn queue_jetons_d_erreur_non_point_virgule_refuses() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:Q8 {x: 1})"]);
    for q in [
        "MATCH (n:Q8) RETURN n @",
        "MATCH (n:Q8) RETURN n $",
        "MATCH (n:Q8) RETURN n 'chaine-non-fermee",
        "MATCH (n:Q8) RETURN n ; @",
        "MATCH (n:Q8) RETURN n ; $",
    ] {
        let msg = erreur(&db, q);
        assert!(
            !msg.to_lowercase().contains("internal"),
            "`{q}` : le refus doit être une erreur de syntaxe nommée, \
             pas une Internal — obtenu : {msg}"
        );
    }
}

/// Deux instructions séparées par `;` : la seconde ne doit être NI exécutée
/// NI jetée en silence — refus explicite.
#[test]
fn queue_deuxieme_instruction_apres_point_virgule_refusee() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:Q9 {x: 1})"]);
    let msg = erreur(&db, "MATCH (n:Q9) RETURN n.x AS x ; MATCH (m:Q9) RETURN m");
    assert!(
        msg.contains("MATCH") || msg.to_lowercase().contains("unexpected"),
        "le refus doit nommer le jeton en trop, obtenu : {msg}"
    );
}

/// WITH … SET : refus FRANC (message de syntaxe nommant le jeton), pas une
/// Internal, et surtout pas un succès tronqué.
#[test]
fn queue_with_puis_set_refuse_franchement() {
    let db = ObrainDB::new_in_memory();
    ecrire(&db, &["INSERT (:Q10 {x: 1})"]);
    let msg = erreur(&db, "MATCH (n:Q10) WITH n SET n.y = 2 RETURN n");
    assert!(
        !msg.to_lowercase().contains("internal"),
        "WITH…SET doit être une erreur de syntaxe franche, obtenu : {msg}"
    );
    // Le SET ne doit pas avoir agi.
    assert_eq!(
        cles(&db, "(n:Q10)"),
        ens(&["x"]),
        "le SET d'une requête refusée ne doit laisser AUCUNE trace"
    );
}

// ===========================================================================
// ANGLE 3c — ALTER, bords du mot-clef optionnel PROPERTY.
// ===========================================================================

/// Une propriété littéralement nommée PROPERTY : `ADD PROPERTY PROPERTY type`
/// doit créer la propriété `PROPERTY`, et `DROP PROPERTY PROPERTY` la retirer.
#[test]
fn alter_propriete_nommee_property() {
    let db = ObrainDB::new_in_memory();
    let mut session = db.session();
    session.execute("CREATE NODE TYPE Rev2Cap (id INT)").unwrap();
    session
        .execute("ALTER NODE TYPE Rev2Cap ADD PROPERTY PROPERTY STRING")
        .unwrap_or_else(|e| {
            panic!("ADD PROPERTY PROPERTY STRING doit s'analyser (propriété nommée PROPERTY) : {e:?}")
        });
    session
        .execute("ALTER NODE TYPE Rev2Cap DROP PROPERTY PROPERTY")
        .unwrap_or_else(|e| {
            panic!("DROP PROPERTY PROPERTY doit retirer la propriété nommée PROPERTY : {e:?}")
        });
}

/// `DROP PROPERTY` SANS identifiant suivant : le mot PROPERTY ne doit pas
/// être silencieusement pris pour le nom de la propriété à retirer — ou s'il
/// l'est, le retrait d'une propriété inexistante doit être refusé. Dans les
/// deux cas : une ERREUR, jamais un succès muet.
#[test]
fn alter_drop_property_sans_identifiant_est_refuse() {
    let db = ObrainDB::new_in_memory();
    let mut session = db.session();
    session.execute("CREATE NODE TYPE Rev2Nu (id INT)").unwrap();
    let res = session.execute("ALTER NODE TYPE Rev2Nu DROP PROPERTY");
    assert!(
        res.is_err(),
        "ALTER … DROP PROPERTY sans nom doit être refusé (soit à l'analyse, \
         soit comme retrait d'une propriété inexistante), pas accepté en silence"
    );
}
