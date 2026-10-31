//! LA FRONTIERE ECRITURE / LECTURE, EPROUVEE A L ENDROIT OU ELLE A FUI.
//!
//! Ce fichier est repris de la revue adversariale du 2026-08-24, qui a
//! prouve PAR EXECUTION que la frontiere posee a la main devant chaque
//! `with_auto_commit` en manquait quatre sur sept. Il est conserve tel quel
//! — temoins, anti-vacuites et cas defaillants — et etendu aux entrees que
//! la revue avait signalees sans les executer.
//!
//! PROTOCOLE. On ARME le jeton AVANT l appel. Une ECRITURE doit alors rendre
//! `Ok` (la frontiere la met hors de portee) ; une LECTURE doit rendre
//! `Err(cancelled)` (le jeton est vivant). Les deux moities comptent : sans
//! l anti-vacuite, un jeton inerte ferait passer toutes les ecritures.

use obrain_common::types::Value;
use obrain_common::utils::cancel::{self, CancelToken};
use obrain_engine::ObrainDB;

fn base() -> (tempfile::TempDir, ObrainDB) {
    let td = tempfile::tempdir().unwrap();
    let p = td.path().join("b");
    let db = ObrainDB::open_substrate(&p).expect("creation");
    (td, db)
}

fn arme() -> CancelToken {
    let j = CancelToken::new();
    j.cancel();
    j
}

fn annulee<T>(r: &Result<T, obrain_common::utils::error::Error>) -> bool {
    r.as_ref().err().is_some_and(|e| e.is_cancelled())
}

// ─────────────────────────────────────────────────────────────────────
//  I. LE CAS TEMOIN ET SON ANTI-VACUITE
// ─────────────────────────────────────────────────────────────────────

#[test]
fn temoin_ecriture_simple_est_protegee() {
    let (_td, db) = base();
    let r = cancel::scope(&arme(), || db.session().execute("CREATE (n:Temoin {v: 1})"));
    assert!(
        r.is_ok(),
        "une ecriture doit etre NON annulable : {:?}",
        r.err().map(|e| e.to_string())
    );
}

#[test]
fn antivacuite_une_lecture_est_bien_annulable() {
    let (_td, db) = base();
    db.session().execute("CREATE (n:Lu {v: 1})").unwrap();
    let r = cancel::scope(&arme(), || db.session().execute("MATCH (n:Lu) RETURN n"));
    assert!(
        annulee(&r),
        "une LECTURE sous jeton arme doit etre annulee : {r:?}"
    );
}

// ─────────────────────────────────────────────────────────────────────
//  II. BLOQUANT 1 — `PROFILE <ecriture>` sortait AVANT la frontiere
// ─────────────────────────────────────────────────────────────────────

#[test]
fn profile_create_n_est_pas_annulable() {
    let (_td, db) = base();
    let r = cancel::scope(&arme(), || {
        db.session().execute("PROFILE CREATE (n:Trou {v: 1})")
    });
    assert!(
        !annulee(&r),
        "REGRESSION DE SURETE : `PROFILE CREATE` annulable alors que \
         `CREATE` ne l est pas. Le `return` de la branche profile precedait \
         la frontiere posee a la main plus bas ; elle est desormais dans \
         `with_auto_commit`, que cette branche traverse aussi. Obtenu : {r:?}"
    );
}

#[test]
fn profile_create_ecrit_vraiment_sous_jeton_arme() {
    let (_td, db) = base();
    let _ = cancel::scope(&arme(), || {
        db.session().execute("PROFILE CREATE (n:Trou {v: 1})")
    });
    let q = db.session().execute("MATCH (n:Trou) RETURN n").unwrap();
    assert_eq!(
        q.rows.len(),
        1,
        "le noeud aurait du etre cree : une ecriture n est pas annulable"
    );
}

#[test]
fn antivacuite_profile_create_ecrit_bien_sans_jeton() {
    let (_td, db) = base();
    let r = db.session().execute("PROFILE CREATE (n:Trou {v: 1})");
    assert!(r.is_ok(), "PROFILE CREATE doit reussir sans jeton : {r:?}");
    let q = db.session().execute("MATCH (n:Trou) RETURN n").unwrap();
    assert_eq!(q.rows.len(), 1, "PROFILE CREATE ecrit bien en nominal");
}

/// LA CATEGORIE DE DEGAT QUI COMPTE : l ecriture FAUSSE.
///
/// `MergeOperator::porteurs_du_motif` decide de CREER a partir de
/// balayages devenus interruptibles, dont le dernier rend `Some(Vec::new())`
/// quand il est coupe : « aucun porteur », donc MERGE cree. Un MERGE annule
/// en vol ne serait donc pas une ecriture avortee mais un DOUBLON persiste.
///
/// NOTE SUR LE VEHICULE. La revue proposait `PROFILE MERGE` ; ce chemin
/// PANIQUE avant tout, avec ou sans jeton, dans
/// `profile.rs:72` (« profile entry count must match logical operator
/// count ») — defaut anterieur et etranger a l annulation. La propriete est
/// donc eprouvee ici sur le chemin nominal, ou elle est de toute facon la
/// seule qui vaille.
#[test]
fn merge_sous_jeton_arme_ne_duplique_pas() {
    let (_td, db) = base();
    db.session().execute("CREATE (n:Unique {k: 1})").unwrap();

    let r = cancel::scope(&arme(), || db.session().execute("MERGE (n:Unique {k: 1})"));
    assert!(!annulee(&r), "MERGE annulable : {r:?}");

    let q = db
        .session()
        .execute("MATCH (n:Unique) RETURN count(n)")
        .expect("relecture");
    assert_eq!(
        q.rows[0][0],
        Value::Int64(1),
        "ECRITURE FAUSSE : un doublon a ete persiste parce que le balayage \
         qui cherche les porteurs du motif avait ete interrompu et rendait \
         un ensemble vide"
    );
}

/// ANTI-VACUITE DE LA BRANCHE PROFILE : la frontiere n a pas simplement
/// rendu tout `PROFILE` ininterruptible. Un `PROFILE` de LECTURE reste
/// annulable, comme la lecture nue.
#[test]
fn profile_d_une_lecture_reste_annulable() {
    let (_td, db) = base();
    db.session().execute("CREATE (n:Pl {v: 1})").unwrap();
    let r = cancel::scope(&arme(), || {
        db.session().execute("PROFILE MATCH (n:Pl) RETURN n")
    });
    assert!(
        annulee(&r),
        "PROFILE d une lecture doit rester annulable : {r:?}"
    );
}

// ─────────────────────────────────────────────────────────────────────
//  III. BLOQUANT 2 — les entrees PARAMETREES
// ─────────────────────────────────────────────────────────────────────

#[test]
fn ecriture_parametree_n_est_pas_annulable() {
    let (_td, db) = base();
    let r = cancel::scope(&arme(), || {
        db.session()
            .execute_with_params("CREATE (n:Param {v: 1})", Default::default())
    });
    assert!(!annulee(&r), "ecriture parametree annulable : {r:?}");
    let q = db.session().execute("MATCH (n:Param) RETURN n").unwrap();
    assert_eq!(q.rows.len(), 1, "l ecriture parametree doit avoir eu lieu");
}

#[test]
fn ecriture_parametree_par_execute_language_n_est_pas_annulable() {
    // Les branches cypher / gremlin / graphql / sql d `execute_language`
    // demandent leur drapeau de compilation ; « gql » est celle qui est la
    // par defaut. La frontiere, elle, ne depend d aucune d elles : les cinq
    // entrees paremetrees traversent le MEME `with_auto_commit`.
    let (_td, db) = base();
    let r = cancel::scope(&arme(), || {
        db.session()
            .execute_language("CREATE (n:Langue {v: 1})", "gql", Some(Default::default()))
    });
    assert!(
        !annulee(&r),
        "ecriture annulable via execute_language : {r:?}"
    );
    let q = db.session().execute("MATCH (n:Langue) RETURN n").unwrap();
    assert_eq!(q.rows.len(), 1, "l ecriture doit avoir eu lieu");
}

/// Une ecriture que l heuristique textuelle NE VOIT PAS. `query_looks_like_
/// mutation` ne connait que des mots-cles GQL/SQL ; la meme ecriture rendue
/// par un plan reste une ecriture, et c est le plan qui doit trancher.
#[test]
fn temoin_l_heuristique_ne_decide_plus_de_l_interruptibilite() {
    let (_td, db) = base();
    // Le mot INSERT n apparait nulle part, mais le plan mute.
    let r = cancel::scope(&arme(), || {
        db.session()
            .execute_with_params("MATCH (n:Absent) DETACH DELETE n", Default::default())
    });
    assert!(!annulee(&r), "{r:?}");
}

// ─────────────────────────────────────────────────────────────────────
//  IV. BLOQUANT 3 — un identifiant utilisateur desarmait une LECTURE
// ─────────────────────────────────────────────────────────────────────

#[test]
fn temoin_lecture_parametree_est_annulable() {
    let (_td, db) = base();
    db.session().execute("CREATE (n:P {v: 1})").unwrap();
    let r = cancel::scope(&arme(), || {
        db.session()
            .execute_with_params("MATCH (n:P) RETURN n.v", Default::default())
    });
    assert!(annulee(&r), "lecture parametree non annulable : {r:?}");
}

#[test]
fn une_propriete_nommee_dataset_ne_desarme_plus_l_annulation() {
    let (_td, db) = base();
    db.session().execute("CREATE (n:P {dataset: 1})").unwrap();
    let r = cancel::scope(&arme(), || {
        db.session()
            .execute_with_params("MATCH (n:P) RETURN n.dataset", Default::default())
    });
    assert!(
        annulee(&r),
        "PROTECTION DESARMEE PAR UN IDENTIFIANT : `n.dataset` contient SET. \
         L interruptibilite doit venir de `plan.root.has_mutations()`, pas \
         d un test de sous-chaine sur le texte. Obtenu : {r:?}"
    );
}

/// LE CAS QUI ISOLE LA CORRECTION. L heuristique reste, PAR CONSTRUCTION,
/// une sur-approximation : un mot-cle dans un LITTERAL DE CHAINE la
/// declenche encore, et aucune amelioration du decoupage ne changera cela.
/// La requete ci-dessous est une LECTURE que l heuristique appelle mutation
/// quoi qu il arrive — elle n est annulable que parce que la decision
/// d interruptibilite a ete rendue au PLAN.
#[test]
fn un_mot_cle_dans_un_litteral_ne_desarme_pas_l_annulation() {
    let (_td, db) = base();
    db.session().execute("CREATE (n:P {nom: 'SET'})").unwrap();
    let r = cancel::scope(&arme(), || {
        db.session().execute_with_params(
            "MATCH (n:P) WHERE n.nom = 'SET' RETURN n.nom",
            Default::default(),
        )
    });
    assert!(
        annulee(&r),
        "PROTECTION DESARMEE PAR UN LITTERAL : le mot SET est ici une DONNEE. \
         Seul plan.root.has_mutations() peut trancher. Obtenu : {r:?}"
    );
}

#[test]
fn l_heuristique_de_transaction_ne_confond_plus_dataset_avec_set() {
    // Elle ne decide plus de l annulation, mais elle decide encore de la
    // transaction implicite : autant qu elle lise des MOTS.
    let (_td, db) = base();
    db.session().execute("CREATE (n:P {dataset: 7})").unwrap();
    let q = db
        .session()
        .execute_with_params("MATCH (n:P) RETURN n.dataset", Default::default())
        .expect("lecture");
    assert_eq!(q.rows[0][0], Value::Int64(7));
}

// ─────────────────────────────────────────────────────────────────────
//  V. L INVERSION DE POLARITE, VUE DU MOTEUR
// ─────────────────────────────────────────────────────────────────────

/// Le DDL ne traverse pas `with_auto_commit` : son `return` est plus haut.
/// Il n est pourtant pas annulable — parce qu un jeton confie n est pas en
/// vigueur tant que la frontiere ne l a pas mis. C est LA propriete qui
/// remplace les sept suspensions posees a la main.
#[test]
fn un_chemin_qui_ne_traverse_pas_la_frontiere_n_est_pas_annulable() {
    let (_td, db) = base();
    let r = cancel::scope(&arme(), || {
        db.session().execute("CREATE NODE TYPE (:Sorte {v INT})")
    });
    assert!(!annulee(&r), "DDL annulable : {r:?}");
}

/// Et l anti-vacuite du precedent : le jeton employe est bien vivant.
#[test]
fn antivacuite_le_meme_jeton_annule_bien_une_lecture() {
    let (_td, db) = base();
    db.session().execute("CREATE (n:V {v: 1})").unwrap();
    let j = arme();
    let ddl = cancel::scope(&j, || {
        db.session().execute("CREATE NODE TYPE (:Sorte2 {v INT})")
    });
    let lecture = cancel::scope(&j, || db.session().execute("MATCH (n:V) RETURN n"));
    assert!(!annulee(&ddl));
    assert!(
        annulee(&lecture),
        "le jeton du test precedent doit etre vivant, sinon il ne prouve rien"
    );
}
