//! Régression : un `UNIQUE (a, b)` est une contrainte sur le TUPLE, pas sur
//! chaque colonne prise séparément.
//!
//! Le contrôle d'unicité testait `props.iter().any(|p| p == key)` : une
//! contrainte composite se comportait donc comme « a unique ET b unique ».
//! Le défaut était de refuser TROP, pas trop peu : des lignes légitimes
//! (`(1,'x')` puis `(1,'y')`) étaient rejetées ; aucun doublon de tuple ne
//! passait, puisque chaque colonne était à elle seule tenue pour unique.
//!
//! Ce que ces tests garantissent : l'unicité du tuple À L'INSERTION
//! (`INSERT`/`CREATE`, et la création par `MERGE`), la persistance de la
//! contrainte, l'idempotence de sa redéclaration et son retrait par
//! `DROP CONSTRAINT`. Les autres chemins d'écriture (`SET`,
//! `MERGE … ON MATCH SET`, ajout de label, API directe du store) ne
//! contrôlent pas l'unicité et ne sont pas couverts ici.

#![cfg(feature = "cypher")]

use obrain_engine::ObrainDB;

#[test]
fn composite_unique_respecte_le_tuple_pas_les_colonnes() {
    let db = ObrainDB::new_in_memory();
    let session = db.session();
    session
        .execute_cypher("CREATE CONSTRAINT composite_unique FOR (n:T) REQUIRE (n.a, n.b) IS UNIQUE")
        .expect("création de la contrainte composite");

    // (1) deux tuples distincts partageant une colonne sont TOUS DEUX acceptés.
    session
        .execute("INSERT (:T {a: 1, b: 'x'})")
        .expect("(1,'x')");
    let r = session.execute("INSERT (:T {a: 1, b: 'y'})");
    assert!(
        r.is_ok(),
        "UNIQUE(a,b) doit accepter (1,'x') puis (1,'y') — reçu {:?}",
        r.err()
    );

    // (2) le même tuple exact est refusé.
    let r = session.execute("INSERT (:T {a: 1, b: 'x'})");
    assert!(
        r.is_err(),
        "UNIQUE(a,b) doit refuser (1,'x') une seconde fois"
    );

    // (3) un tuple ne différant que sur l'autre colonne est accepté.
    let r = session.execute("INSERT (:T {a: 2, b: 'x'})");
    assert!(
        r.is_ok(),
        "UNIQUE(a,b) doit accepter (2,'x') après (1,'x') — reçu {:?}",
        r.err()
    );
}

#[test]
fn contrainte_composite_survit_a_la_reouverture() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("c");

    // Process A : créer la contrainte, fermer.
    {
        let db = ObrainDB::open_substrate(&chemin).expect("ouverture A");
        let session = db.session();
        session
            .execute_cypher("CREATE CONSTRAINT cc FOR (n:T) REQUIRE (n.a, n.b) IS UNIQUE")
            .expect("CREATE CONSTRAINT A");
    }

    // Process B (réouverture) : la contrainte tient toujours.
    let db = ObrainDB::open_substrate(&chemin).expect("réouverture B");
    let session = db.session();
    session
        .execute("INSERT (:T {a: 1, b: 'x'})")
        .expect("(1,'x')");
    let r = session.execute("INSERT (:T {a: 1, b: 'y'})");
    assert!(
        r.is_ok(),
        "après réouverture UNIQUE(a,b) doit accepter (1,'y') — reçu {:?}",
        r.err()
    );
    let r = session.execute("INSERT (:T {a: 1, b: 'x'})");
    assert!(
        r.is_err(),
        "après réouverture UNIQUE(a,b) doit refuser (1,'x') une seconde fois"
    );
}

// ───────────────────────── outils d'observation ─────────────────────────

/// Nombre de copies d'une contrainte portées EN MÉMOIRE par le type `label`,
/// lu par `SHOW NODE TYPES` (colonne `constraints`, rendu `Debug` des
/// contraintes). `motif` est le début du rendu, p. ex. `Unique(`.
fn copies_en_memoire(session: &obrain_engine::Session, label: &str, motif: &str) -> usize {
    let r = session.execute("SHOW NODE TYPES").expect("SHOW NODE TYPES");
    r.rows
        .iter()
        .map(|ligne| format!("{ligne:?}"))
        .filter(|texte| texte.contains(&format!("\"{label}\"")))
        .map(|texte| texte.matches(motif).count())
        .sum()
}

/// Nombre de copies d'une contrainte PERSISTÉES pour le type `label` dans
/// `substrate.typecat` (une ligne `node\t<label>\t<JSON>` par type) : entrées
/// du tableau `constraints` dont la nature est `nature` (p. ex. `Unique`).
fn copies_persistees(chemin: &std::path::Path, label: &str, nature: &str) -> usize {
    let contenu = std::fs::read_to_string(chemin.join("substrate.typecat")).unwrap_or_default();
    let prefixe = format!("node\t{label}\t");
    contenu
        .lines()
        .filter_map(|l| l.strip_prefix(prefixe.as_str()))
        .map(|json| {
            let definition: serde_json::Value =
                serde_json::from_str(json).expect("ligne de typecat lisible");
            definition["constraints"].as_array().map_or(0, |cs| {
                cs.iter().filter(|c| c.get(nature).is_some()).count()
            })
        })
        .sum()
}

/// Nombre de nœuds `:T` dont la propriété `a` vaut `valeur`.
fn nombre_de_t_avec_a(session: &obrain_engine::Session, valeur: i64) -> usize {
    session
        .execute(&format!("MATCH (n:T {{a: {valeur}}}) RETURN n.a"))
        .expect("MATCH de comptage")
        .rows
        .len()
}

const CC: &str = "CREATE CONSTRAINT cc IF NOT EXISTS FOR (n:T) REQUIRE (n.a, n.b) IS UNIQUE";

// ───────────────────── point 1 : redéclaration idempotente ─────────────────────

/// Une application qui rejoue son schéma à chaque démarrage ne doit laisser
/// qu'UNE copie de la contrainte, en mémoire comme sur disque.
#[test]
fn redeclaration_identique_ne_s_accumule_pas() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("c");

    for demarrage in 1..=3 {
        let db = ObrainDB::open_substrate(&chemin).expect("ouverture");
        let session = db.session();
        // Le schéma est rejoué deux fois par démarrage (deux sessions d'une
        // même application le font).
        for _ in 0..2 {
            session.execute_cypher(CC).expect("CREATE CONSTRAINT");
        }
        assert_eq!(
            copies_en_memoire(&session, "T", "Unique("),
            1,
            "démarrage {demarrage} : une seule copie en mémoire"
        );
        drop(session);
        drop(db);
        assert_eq!(
            copies_persistees(&chemin, "T", "Unique"),
            1,
            "démarrage {demarrage} : une seule copie persistée"
        );
    }

    // La contrainte, elle, tient toujours.
    let db = ObrainDB::open_substrate(&chemin).expect("ouverture finale");
    let session = db.session();
    session
        .execute("INSERT (:T {a: 1, b: 'x'})")
        .expect("(1,'x')");
    assert!(session.execute("INSERT (:T {a: 1, b: 'x'})").is_err());
}

/// Sans `IF NOT EXISTS`, redéclarer une contrainte équivalente est une erreur
/// NOMMÉE (même règle que `CREATE NODE TYPE` sur un type existant), et rien
/// n'est ajouté. Avec `IF NOT EXISTS`, c'est un « No change ».
#[test]
fn redeclaration_sans_if_not_exists_est_une_erreur_nommee() {
    let db = ObrainDB::new_in_memory();
    let session = db.session();
    session
        .execute_cypher("CREATE CONSTRAINT cc FOR (n:T) REQUIRE (n.a, n.b) IS UNIQUE")
        .expect("première déclaration");

    // Même nom, même définition.
    let e = session
        .execute_cypher("CREATE CONSTRAINT cc FOR (n:T) REQUIRE (n.a, n.b) IS UNIQUE")
        .expect_err("redéclaration sans IF NOT EXISTS");
    assert!(
        e.to_string().contains("already exists"),
        "erreur nommée attendue, reçu : {e}"
    );
    // Autre nom, même définition : équivalente, donc refusée aussi.
    let e = session
        .execute_cypher("CREATE CONSTRAINT autre FOR (n:T) REQUIRE (n.a, n.b) IS UNIQUE")
        .expect_err("contrainte équivalente sous un autre nom");
    assert!(e.to_string().contains("already exists"), "reçu : {e}");
    // Même nom, autre définition : le nom est pris.
    let e = session
        .execute_cypher("CREATE CONSTRAINT cc FOR (n:T) REQUIRE n.c IS UNIQUE")
        .expect_err("nom déjà pris par une autre définition");
    assert!(e.to_string().contains("already exists"), "reçu : {e}");

    // IF NOT EXISTS : aucun changement, aucune erreur.
    let r = session.execute_cypher(CC).expect("IF NOT EXISTS");
    assert!(
        format!("{r:?}").contains("No change"),
        "IF NOT EXISTS doit répondre « No change », reçu {r:?}"
    );

    assert_eq!(copies_en_memoire(&session, "T", "Unique("), 1);

    // L'ordre des colonnes fait partie de l'identité : (b, a) est une AUTRE
    // contrainte, acceptée sous un autre nom.
    session
        .execute_cypher("CREATE CONSTRAINT cc_inverse FOR (n:T) REQUIRE (n.b, n.a) IS UNIQUE")
        .expect("(b, a) est distincte de (a, b)");
    assert_eq!(copies_en_memoire(&session, "T", "Unique("), 2);
}

/// Une base déjà polluée par la tête précédente de cette PR (N copies de la
/// même contrainte, sans nom) s'ouvre, se répare en mémoire ET sur disque.
#[test]
fn base_polluee_par_des_doublons_se_repare_a_l_ouverture() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("c");
    {
        let db = ObrainDB::open_substrate(&chemin).expect("création");
        db.session()
            .execute("INSERT (:T {a: 1, b: 'x'})")
            .expect("donnée initiale");
    }
    // Format exact écrit par la tête 17a6cc5a : pas de nom, copies empilées.
    let copie = r#"{"Unique":["a","b"]}"#;
    let ligne = format!(
        "# catalogue de types obrain — genre\tnom\tdefinition(JSON)\n\
         node\tT\t{{\"name\":\"T\",\"properties\":[],\"constraints\":[{copie},{copie},{copie}],\"parent_types\":[]}}\n"
    );
    std::fs::write(chemin.join("substrate.typecat"), ligne).unwrap();
    assert_eq!(copies_persistees(&chemin, "T", "Unique"), 3);

    {
        let db = ObrainDB::open_substrate(&chemin).expect("la base polluée doit s'ouvrir");
        let session = db.session();
        assert_eq!(
            copies_en_memoire(&session, "T", "Unique("),
            1,
            "les copies sont fondues à l'hydratation"
        );
        assert!(
            session.execute("INSERT (:T {a: 1, b: 'x'})").is_err(),
            "la contrainte héritée tient"
        );
        // Rejouer le schéma ne rajoute rien.
        session.execute_cypher(CC).expect("rejeu du schéma");
        assert_eq!(copies_en_memoire(&session, "T", "Unique("), 1);
    }
    assert_eq!(
        copies_persistees(&chemin, "T", "Unique"),
        1,
        "le fichier est réparé"
    );

    // Une contrainte héritée sans nom reçoit le nom par défaut `<label>_<nature>_<colonnes>`
    // et peut donc être retirée.
    let db = ObrainDB::open_substrate(&chemin).expect("réouverture");
    let session = db.session();
    session
        .execute_cypher("DROP CONSTRAINT T_unique_a_b")
        .expect("DROP par le nom par défaut");
    assert_eq!(copies_en_memoire(&session, "T", "Unique("), 0);
}

// ───────────────────── point 2 : DROP CONSTRAINT ─────────────────────

#[test]
fn drop_constraint_retire_la_contrainte_et_elle_ne_revient_pas() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("c");
    {
        let db = ObrainDB::open_substrate(&chemin).expect("ouverture A");
        let session = db.session();
        session.execute_cypher(CC).expect("CREATE CONSTRAINT");
        session
            .execute("INSERT (:T {a: 1, b: 'x'})")
            .expect("(1,'x')");
        assert!(session.execute("INSERT (:T {a: 1, b: 'x'})").is_err());

        session
            .execute_cypher("DROP CONSTRAINT cc")
            .expect("DROP CONSTRAINT");
        assert_eq!(
            copies_en_memoire(&session, "T", "Unique("),
            0,
            "retirée du catalogue en mémoire"
        );
        let r = session.execute("INSERT (:T {a: 1, b: 'x'})");
        assert!(
            r.is_ok(),
            "après DROP le doublon est accepté dans le même processus — reçu {:?}",
            r.err()
        );
    }
    assert_eq!(
        copies_persistees(&chemin, "T", "Unique"),
        0,
        "retirée de la forme persistée"
    );

    let db = ObrainDB::open_substrate(&chemin).expect("réouverture B");
    let session = db.session();
    assert_eq!(
        copies_en_memoire(&session, "T", "Unique("),
        0,
        "la contrainte retirée ne revient pas à la réouverture"
    );
    let r = session.execute("INSERT (:T {a: 1, b: 'x'})");
    assert!(
        r.is_ok(),
        "après DROP + réouverture le doublon est accepté — reçu {:?}",
        r.err()
    );
}

#[test]
fn drop_constraint_inconnue_est_une_erreur_sauf_if_exists() {
    let db = ObrainDB::new_in_memory();
    let session = db.session();
    let e = session
        .execute_cypher("DROP CONSTRAINT nexiste_pas")
        .expect_err("DROP d'une contrainte inconnue");
    assert!(e.to_string().contains("nexiste_pas"), "reçu : {e}");
    session
        .execute_cypher("DROP CONSTRAINT nexiste_pas IF EXISTS")
        .expect("IF EXISTS : aucun changement, aucune erreur");
}

/// Le nom sert au DROP : deux contraintes du même label se retirent une à une.
#[test]
fn drop_constraint_ne_retire_que_la_contrainte_nommee() {
    let db = ObrainDB::new_in_memory();
    let session = db.session();
    session.execute_cypher(CC).expect("cc");
    session
        .execute_cypher("CREATE CONSTRAINT c_seul FOR (n:T) REQUIRE n.c IS UNIQUE")
        .expect("c_seul");
    session
        .execute_cypher("CREATE CONSTRAINT d_requis FOR (n:T) REQUIRE n.d IS NOT NULL")
        .expect("d_requis");
    session
        .execute("INSERT (:T {a: 1, b: 'x', c: 1, d: 1})")
        .expect("première ligne");

    session
        .execute_cypher("DROP CONSTRAINT c_seul")
        .expect("DROP c_seul");
    // c n'est plus unique…
    session
        .execute("INSERT (:T {a: 2, b: 'x', c: 1, d: 1})")
        .expect("c en double est accepté après DROP c_seul");
    // …mais (a, b) l'est toujours, et d est toujours requis.
    assert!(
        session
            .execute("INSERT (:T {a: 1, b: 'x', c: 9, d: 1})")
            .is_err()
    );
    assert!(session.execute("INSERT (:T {a: 3, b: 'x', c: 9})").is_err());

    session
        .execute_cypher("DROP CONSTRAINT d_requis")
        .expect("DROP d_requis");
    session
        .execute("INSERT (:T {a: 3, b: 'x', c: 9})")
        .expect("d absent est accepté après DROP d_requis");
}

// ───────────────────── point 3 : MERGE … ON CREATE SET ─────────────────────

/// `ON CREATE SET` peut réécrire une propriété du motif : le contrôle
/// d'unicité doit porter sur la valeur ÉCRITE, pas sur celle du motif.
#[test]
fn merge_on_create_set_controle_les_valeurs_ecrites() {
    let db = ObrainDB::new_in_memory();
    let session = db.session();
    session
        .execute_cypher("CREATE CONSTRAINT a_unique FOR (n:T) REQUIRE n.a IS UNIQUE")
        .expect("CREATE CONSTRAINT");
    session.execute("INSERT (:T {a: 2})").expect("a = 2");

    let r = session.execute_cypher("MERGE (n:T {a: 1}) ON CREATE SET n.a = 2");
    assert!(
        r.is_err(),
        "MERGE (a: 1) ON CREATE SET a = 2 écrit a = 2, déjà pris : doit être refusé"
    );
    assert_eq!(
        nombre_de_t_avec_a(&session, 2),
        1,
        "aucun doublon de a = 2 n'a été écrit"
    );

    // Le sens inverse : le motif heurte une valeur existante, mais la valeur
    // écrite est libre → ce n'est PAS une violation (a = 2 existe, on MERGE
    // sur a = 3 inexistant et on écrit a = 4).
    session
        .execute_cypher("MERGE (n:T {a: 3}) ON CREATE SET n.a = 4")
        .expect("la valeur écrite (4) est libre");
    assert_eq!(nombre_de_t_avec_a(&session, 4), 1);
    assert_eq!(nombre_de_t_avec_a(&session, 3), 0);
}

// ───────────────────── point 4 : échec de persistance ─────────────────────

/// Une contrainte qui ne peut pas être persistée n'est PAS créée : la commande
/// échoue et le catalogue en mémoire est remis dans son état d'avant.
#[test]
fn echec_de_persistance_fait_echouer_la_commande() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("c");
    let db = ObrainDB::open_substrate(&chemin).expect("ouverture");
    let session = db.session();

    // `substrate.typecat` est écrit par tmp + rename : un RÉPERTOIRE à la place
    // du fichier temporaire fait échouer l'écriture, sans toucher aux droits.
    let obstacle = chemin.join("substrate.typecat.tmp");
    std::fs::create_dir(&obstacle).unwrap();

    let r = session.execute_cypher(CC);
    assert!(
        r.is_err(),
        "une contrainte non persistée ne doit pas répondre « Created » — reçu {r:?}"
    );
    assert_eq!(
        copies_en_memoire(&session, "T", "Unique("),
        0,
        "la contrainte non persistée n'est pas laissée en mémoire"
    );

    // L'obstacle levé, la même commande réussit et la contrainte est durable.
    std::fs::remove_dir(&obstacle).unwrap();
    session
        .execute_cypher(CC)
        .expect("CREATE après levée de l'obstacle");
    session
        .execute("INSERT (:T {a: 1, b: 'x'})")
        .expect("(1,'x')");

    // Même règle pour DROP : s'il ne peut pas être persisté, il échoue et la
    // contrainte reste en vigueur.
    std::fs::create_dir(&obstacle).unwrap();
    assert!(session.execute_cypher("DROP CONSTRAINT cc").is_err());
    assert!(
        session.execute("INSERT (:T {a: 1, b: 'x'})").is_err(),
        "un DROP non persisté laisse la contrainte en vigueur"
    );
    std::fs::remove_dir(&obstacle).unwrap();

    drop(session);
    drop(db);
    let db = ObrainDB::open_substrate(&chemin).expect("réouverture");
    assert!(db.session().execute("INSERT (:T {a: 1, b: 'x'})").is_err());
}

// ───────────────────── couverture relevée par la revue ─────────────────────

/// Un membre NULL ou absent du tuple : pas de conflit possible.
#[test]
fn valeur_nulle_ou_absente_dans_le_tuple_ne_fait_pas_conflit() {
    let db = ObrainDB::new_in_memory();
    let session = db.session();
    session.execute_cypher(CC).expect("CREATE CONSTRAINT");

    // b absent, deux fois.
    session
        .execute("INSERT (:T {a: 1})")
        .expect("(1, absent) n°1");
    session
        .execute("INSERT (:T {a: 1})")
        .expect("(1, absent) n°2");
    // b NULL explicite, deux fois.
    session
        .execute("INSERT (:T {a: 1, b: null})")
        .expect("(1, NULL) n°1");
    session
        .execute("INSERT (:T {a: 1, b: null})")
        .expect("(1, NULL) n°2");
    // a absent, deux fois.
    session
        .execute("INSERT (:T {b: 'x'})")
        .expect("(absent, 'x') n°1");
    session
        .execute("INSERT (:T {b: 'x'})")
        .expect("(absent, 'x') n°2");

    // Le tuple complet reste contrôlé malgré ces lignes partielles.
    session
        .execute("INSERT (:T {a: 1, b: 'x'})")
        .expect("(1,'x')");
    assert!(session.execute("INSERT (:T {a: 1, b: 'x'})").is_err());
}

/// Le doublon est cherché dans les données écrites AVANT la fermeture : c'est
/// l'état relu du disque qui est interrogé, pas celui de la session courante.
#[test]
fn doublon_contre_des_donnees_ecrites_avant_la_fermeture() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("c");
    {
        let db = ObrainDB::open_substrate(&chemin).expect("ouverture A");
        let session = db.session();
        session.execute_cypher(CC).expect("CREATE CONSTRAINT");
        session
            .execute("INSERT (:T {a: 1, b: 'x'})")
            .expect("(1,'x')");
        session
            .execute("INSERT (:T {a: 2, b: 'y'})")
            .expect("(2,'y')");
    }

    let db = ObrainDB::open_substrate(&chemin).expect("réouverture B");
    let session = db.session();
    // Première écriture du processus B : aucun index n'a été touché par B.
    assert!(
        session.execute("INSERT (:T {a: 1, b: 'x'})").is_err(),
        "(1,'x') a été écrit avant la fermeture : doublon"
    );
    assert!(
        session.execute("INSERT (:T {a: 2, b: 'y'})").is_err(),
        "(2,'y') a été écrit avant la fermeture : doublon"
    );
    session
        .execute("INSERT (:T {a: 1, b: 'y'})")
        .expect("(1,'y') est un tuple neuf");
    session
        .execute("INSERT (:T {a: 2, b: 'x'})")
        .expect("(2,'x') est un tuple neuf");
}

/// `NOT NULL` et `NODE KEY` passent par la même persistance que `UNIQUE`.
#[test]
fn not_null_et_node_key_survivent_a_la_reouverture() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("c");
    {
        let db = ObrainDB::open_substrate(&chemin).expect("ouverture A");
        let session = db.session();
        session
            .execute_cypher("CREATE CONSTRAINT k_cle FOR (n:K) REQUIRE (n.a, n.b) IS NODE KEY")
            .expect("NODE KEY");
        session
            .execute_cypher("CREATE CONSTRAINT r_requis FOR (n:R) REQUIRE n.nom IS NOT NULL")
            .expect("NOT NULL");
        session
            .execute("INSERT (:K {a: 1, b: 'x'})")
            .expect("K (1,'x')");
    }

    let db = ObrainDB::open_substrate(&chemin).expect("réouverture B");
    let session = db.session();
    // NODE KEY = unicité du tuple + présence de chaque membre.
    assert!(
        session.execute("INSERT (:K {a: 1, b: 'x'})").is_err(),
        "NODE KEY : tuple en double refusé après réouverture"
    );
    session
        .execute("INSERT (:K {a: 1, b: 'y'})")
        .expect("NODE KEY : tuple neuf accepté");
    assert!(
        session.execute("INSERT (:K {a: 5})").is_err(),
        "NODE KEY : membre absent refusé après réouverture"
    );
    // NOT NULL.
    assert!(
        session.execute("INSERT (:R {autre: 1})").is_err(),
        "NOT NULL : propriété absente refusée après réouverture"
    );
    session
        .execute("INSERT (:R {nom: 'n'})")
        .expect("NOT NULL satisfait");
}

// ─────────────────── relecture de #99 : DDL, types implicites, modes ───────────────────

/// Nombre de contraintes persistées (tableau `constraints` de la ligne du type).
fn contraintes_persistees(chemin: &std::path::Path, label: &str) -> usize {
    let contenu = std::fs::read_to_string(chemin.join("substrate.typecat")).unwrap_or_default();
    let prefixe = format!("node\t{label}\t");
    contenu
        .lines()
        .filter_map(|l| l.strip_prefix(prefixe.as_str()))
        .map(|json| {
            let d: serde_json::Value = serde_json::from_str(json).expect("ligne lisible");
            d["constraints"].as_array().map_or(0, Vec::len)
        })
        .sum()
}

/// Le type `label` a-t-il une ligne dans `substrate.typecat` ?
fn type_persiste(chemin: &std::path::Path, label: &str) -> bool {
    std::fs::read_to_string(chemin.join("substrate.typecat"))
        .unwrap_or_default()
        .lines()
        .any(|l| l.starts_with(&format!("node\t{label}\t")))
}

/// C1 : deux `CREATE CONSTRAINT` simultanés ne se perdent pas l'un l'autre,
/// ni sur le même label (l'un écrivait sa définition périmée après l'autre),
/// ni sur deux labels (lire-modifier-réécrire du fichier entier).
#[test]
fn create_constraint_concurrents_ne_se_perdent_pas() {
    const MANCHES: usize = 60;
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("c");
    {
        let db = std::sync::Arc::new(ObrainDB::open_substrate(&chemin).expect("ouverture"));
        let depart = std::sync::Arc::new(std::sync::Barrier::new(3));
        let mut fils = Vec::new();
        for numero in 0..3usize {
            let (db, depart) = (std::sync::Arc::clone(&db), std::sync::Arc::clone(&depart));
            fils.push(std::thread::spawn(move || {
                let s = db.session();
                for m in 0..MANCHES {
                    depart.wait();
                    // Fils 0 et 1 : deux contraintes du MÊME label ; fils 2 : un
                    // autre label, en même temps.
                    let ddl = match numero {
                        0 => format!("CREATE CONSTRAINT ca{m} FOR (n:L{m}) REQUIRE n.a IS UNIQUE"),
                        1 => format!("CREATE CONSTRAINT cb{m} FOR (n:L{m}) REQUIRE n.b IS UNIQUE"),
                        _ => format!("CREATE CONSTRAINT cc{m} FOR (n:M{m}) REQUIRE n.c IS UNIQUE"),
                    };
                    s.execute_cypher(&ddl).expect("CREATE CONSTRAINT");
                }
            }));
        }
        for f in fils {
            f.join().unwrap();
        }
    }
    for m in 0..MANCHES {
        assert_eq!(
            contraintes_persistees(&chemin, &format!("L{m}")),
            2,
            "L{m} : les deux contraintes doivent être sur disque"
        );
        assert_eq!(contraintes_persistees(&chemin, &format!("M{m}")), 1, "M{m}");
    }
    // Et elles reviennent à la réouverture.
    let db = ObrainDB::open_substrate(&chemin).expect("réouverture");
    let s = db.session();
    assert_eq!(copies_en_memoire(&s, "L7", "Unique("), 2);
}

/// C1 : un `DROP CONSTRAINT` concurrent d'un `CREATE CONSTRAINT` du même label
/// laisse le disque et la mémoire d'accord.
#[test]
fn drop_et_create_concurrents_laissent_memoire_et_disque_d_accord() {
    const MANCHES: usize = 60;
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("c");
    {
        let db = std::sync::Arc::new(ObrainDB::open_substrate(&chemin).expect("ouverture"));
        for m in 0..MANCHES {
            db.session()
                .execute_cypher(&format!(
                    "CREATE CONSTRAINT ca{m} FOR (n:L{m}) REQUIRE n.a IS UNIQUE"
                ))
                .unwrap();
        }
        let depart = std::sync::Arc::new(std::sync::Barrier::new(2));
        let (db1, d1) = (std::sync::Arc::clone(&db), std::sync::Arc::clone(&depart));
        let createur = std::thread::spawn(move || {
            let s = db1.session();
            for m in 0..MANCHES {
                d1.wait();
                s.execute_cypher(&format!(
                    "CREATE CONSTRAINT cb{m} FOR (n:L{m}) REQUIRE n.b IS UNIQUE"
                ))
                .unwrap();
            }
        });
        let (db2, d2) = (std::sync::Arc::clone(&db), std::sync::Arc::clone(&depart));
        let retireur = std::thread::spawn(move || {
            let s = db2.session();
            for m in 0..MANCHES {
                d2.wait();
                s.execute_cypher(&format!("DROP CONSTRAINT ca{m}")).unwrap();
            }
        });
        createur.join().unwrap();
        retireur.join().unwrap();
        let s = db.session();
        for m in 0..MANCHES {
            assert_eq!(
                copies_en_memoire(&s, &format!("L{m}"), "Unique("),
                1,
                "L{m}"
            );
        }
    }
    for m in 0..MANCHES {
        assert_eq!(
            contraintes_persistees(&chemin, &format!("L{m}")),
            1,
            "L{m} : disque et mémoire d'accord (seule cb{m} reste)"
        );
    }
}

/// C2 : `CREATE CONSTRAINT` sur un label sans type crée un type IMPLICITE.
/// `CREATE NODE TYPE` ensuite le COMPLÈTE (au lieu de « already exists »), et
/// retirer la dernière contrainte retire le type implicite.
#[test]
fn le_type_implicite_est_complete_par_create_node_type_et_retire_au_dernier_drop() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("c");
    {
        let db = ObrainDB::open_substrate(&chemin).expect("ouverture");
        let s = db.session();
        s.execute_cypher("CREATE CONSTRAINT ca FOR (n:T) REQUIRE n.a IS UNIQUE")
            .unwrap();
        assert!(type_persiste(&chemin, "T"));

        // CREATE NODE TYPE complète le type implicite, la contrainte reste.
        s.execute("CREATE NODE TYPE T (a INTEGER, b STRING)")
            .expect("CREATE NODE TYPE sur un type implicite");
        let types = format!("{:?}", s.execute("SHOW NODE TYPES").unwrap().rows);
        assert!(types.contains("a INT64"), "{types}");
        assert_eq!(
            copies_en_memoire(&s, "T", "Unique("),
            1,
            "contrainte conservée"
        );
        s.execute("INSERT (:T {a: 1, b: 'x'})").unwrap();
        assert!(s.execute("INSERT (:T {a: 1, b: 'y'})").is_err());

        // Le type est maintenant EXPLICITE : DROP de sa dernière contrainte le garde.
        s.execute_cypher("DROP CONSTRAINT ca").unwrap();
        assert!(type_persiste(&chemin, "T"), "type explicite : conservé");
        assert!(
            s.execute("CREATE NODE TYPE T (a INTEGER)").is_err(),
            "type explicite : « already exists »"
        );

        // Un type IMPLICITE disparaît avec sa dernière contrainte.
        s.execute_cypher("CREATE CONSTRAINT cu FOR (n:U) REQUIRE n.a IS UNIQUE")
            .unwrap();
        assert!(type_persiste(&chemin, "U"));
        s.execute_cypher("DROP CONSTRAINT cu").unwrap();
        assert!(
            !type_persiste(&chemin, "U"),
            "le type vide ne survit pas au DROP"
        );
        let types = format!("{:?}", s.execute("SHOW NODE TYPES").unwrap().rows);
        assert!(!types.contains("\"U\""), "{types}");
        s.execute("CREATE NODE TYPE U (a INTEGER)")
            .expect("CREATE CONSTRAINT → DROP → CREATE NODE TYPE");
        // Avec IF NOT EXISTS, le type implicite est aussi complété (pas un « No change »
        // qui perdrait les propriétés typées).
        s.execute_cypher("CREATE CONSTRAINT cv FOR (n:V) REQUIRE n.a IS UNIQUE")
            .unwrap();
        s.execute("CREATE NODE TYPE IF NOT EXISTS V (a INTEGER)")
            .unwrap();
        let types = format!("{:?}", s.execute("SHOW NODE TYPES").unwrap().rows);
        assert!(types.contains("a INT64"), "{types}");
    }
    // Réouverture : mêmes états.
    let db = ObrainDB::open_substrate(&chemin).expect("réouverture");
    let s = db.session();
    assert_eq!(copies_en_memoire(&s, "V", "Unique("), 1);
}

/// C3 : `CREATE OR REPLACE NODE TYPE` remplace les propriétés, pas les
/// contraintes — ni en mémoire ni sur disque.
#[test]
fn create_or_replace_node_type_conserve_les_contraintes() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("c");
    {
        let db = ObrainDB::open_substrate(&chemin).expect("ouverture");
        let s = db.session();
        s.execute("CREATE NODE TYPE T (a INTEGER)").unwrap();
        s.execute_cypher("CREATE CONSTRAINT ca FOR (n:T) REQUIRE n.a IS UNIQUE")
            .unwrap();
        s.execute("CREATE OR REPLACE NODE TYPE T (a INTEGER, b STRING)")
            .unwrap();
        assert_eq!(copies_en_memoire(&s, "T", "Unique("), 1, "en mémoire");
        s.execute("INSERT (:T {a: 1, b: 'x'})").unwrap();
        assert!(s.execute("INSERT (:T {a: 1, b: 'y'})").is_err());
    }
    assert_eq!(contraintes_persistees(&chemin, "T"), 1, "sur disque");
    let db = ObrainDB::open_substrate(&chemin).expect("réouverture");
    let s = db.session();
    assert!(
        s.execute("INSERT (:T {a: 1, b: 'z'})").is_err(),
        "après réouverture"
    );
}

/// C4 : le mode `Config::persistent` (réputé « adossé au WAL ») N'EST PAS un
/// second moteur : `with_config` y délègue à `open_substrate` (le code du WAL
/// `LpgWal` n'y est jamais atteint). Les contraintes y sont donc persistées par
/// le même chemin que partout ailleurs — « Created » tient après réouverture.
/// Seule la base en mémoire (`new_in_memory`, substrat jetable) n'est pas
/// durable, par contrat.
#[test]
fn le_mode_config_persistent_persiste_les_contraintes() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("c");
    {
        let db = ObrainDB::with_config(obrain_engine::Config::persistent(&chemin)).unwrap();
        let s = db.session();
        s.execute_cypher("CREATE CONSTRAINT ca FOR (n:T) REQUIRE (n.a, n.b) IS UNIQUE")
            .unwrap();
        s.execute("INSERT (:T {a: 1, b: 'x'})").unwrap();
    }
    assert_eq!(contraintes_persistees(&chemin, "T"), 1, "sur disque");
    let db = ObrainDB::with_config(obrain_engine::Config::persistent(&chemin)).unwrap();
    let s = db.session();
    assert!(
        s.execute("INSERT (:T {a: 1, b: 'x'})").is_err(),
        "la contrainte tient après réouverture en mode Config::persistent"
    );
    s.execute("INSERT (:T {a: 1, b: 'y'})").expect("tuple neuf");
}

/// M1 : `IF NOT EXISTS` avec une contrainte équivalente sous un AUTRE nom, ou
/// avec un nom qui en désigne une autre, ne crée rien (comme Neo4j) — et ne
/// touche pas aux contraintes existantes.
#[test]
fn if_not_exists_avec_un_autre_nom_ne_cree_rien() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    s.execute_cypher("CREATE CONSTRAINT premiere FOR (n:T) REQUIRE n.a IS UNIQUE")
        .unwrap();
    let r = s
        .execute_cypher("CREATE CONSTRAINT autre_nom IF NOT EXISTS FOR (n:T) REQUIRE n.a IS UNIQUE")
        .unwrap();
    assert!(format!("{r:?}").contains("No change"), "{r:?}");
    // Le nom `premiere` désigne une contrainte DIFFÉRENTE de celle demandée.
    let r = s
        .execute_cypher("CREATE CONSTRAINT premiere IF NOT EXISTS FOR (n:T) REQUIRE n.b IS UNIQUE")
        .unwrap();
    assert!(format!("{r:?}").contains("No change"), "{r:?}");
    let rows = s.execute("SHOW CONSTRAINTS").unwrap().rows;
    assert_eq!(rows.len(), 1, "rien n'a été créé : {rows:?}");
    assert!(format!("{rows:?}").contains("premiere"));
}

/// M2 : `NOT NULL (a, b)` est UNE instruction, un nom : le `DROP` retire les
/// deux contraintes qu'elle a posées.
#[test]
fn un_nom_partage_par_plusieurs_contraintes_les_retire_toutes() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    s.execute_cypher("CREATE CONSTRAINT requis FOR (n:T) REQUIRE (n.a, n.b) IS NOT NULL")
        .unwrap();
    assert_eq!(s.execute("SHOW CONSTRAINTS").unwrap().rows.len(), 2);
    assert!(s.execute("INSERT (:T {a: 1})").is_err());
    s.execute_cypher("DROP CONSTRAINT requis").unwrap();
    assert_eq!(s.execute("SHOW CONSTRAINTS").unwrap().rows.len(), 0);
    s.execute("INSERT (:T {a: 1})")
        .expect("les deux NOT NULL sont retirées");
}

/// M4 : le DDL de contrainte est refusé en lecture seule, nommément, et ne
/// réécrit pas `substrate.typecat`.
#[test]
fn ddl_de_contrainte_refuse_en_lecture_seule() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("c");
    {
        let db = ObrainDB::open_substrate(&chemin).expect("ouverture");
        db.session()
            .execute_cypher("CREATE CONSTRAINT ca FOR (n:T) REQUIRE n.a IS UNIQUE")
            .unwrap();
    }
    let avant = std::fs::read(chemin.join("substrate.typecat")).unwrap();
    let db =
        ObrainDB::with_config(obrain_engine::Config::read_only(&chemin)).expect("lecture seule");
    let s = db.session();
    for ddl in [
        "CREATE CONSTRAINT cb FOR (n:T) REQUIRE n.b IS UNIQUE",
        "DROP CONSTRAINT ca",
    ] {
        let e = s.execute_cypher(ddl).expect_err(ddl);
        assert!(
            e.to_string().to_lowercase().contains("read"),
            "`{ddl}` : erreur nommée « lecture seule » attendue, reçu : {e}"
        );
    }
    assert_eq!(
        std::fs::read(chemin.join("substrate.typecat")).unwrap(),
        avant,
        "le fichier n'a pas été réécrit"
    );
}

/// M5 : un `substrate.typecat` illisible fait ÉCHOUER la déclaration au lieu
/// d'être réécrit avec la seule nouvelle ligne (ce qui effaçait les autres types).
#[test]
fn typecat_illisible_fait_echouer_la_declaration_sans_rien_effacer() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("c");
    {
        let db = ObrainDB::open_substrate(&chemin).expect("ouverture");
        let s = db.session();
        s.execute("CREATE NODE TYPE Garde (a INTEGER)").unwrap();
    }
    let fichier = chemin.join("substrate.typecat");
    let mut corrompu = std::fs::read_to_string(&fichier).unwrap();
    corrompu.push_str("ligne sans tabulation\n");
    std::fs::write(&fichier, &corrompu).unwrap();

    let db = ObrainDB::open_substrate(&chemin).expect("la base s'ouvre malgré la ligne");
    let s = db.session();
    let e = s
        .execute_cypher("CREATE CONSTRAINT ca FOR (n:T) REQUIRE n.a IS UNIQUE")
        .expect_err("fichier illisible : la contrainte ne peut pas être rendue durable");
    assert!(e.to_string().contains("typecat"), "{e}");
    assert_eq!(
        copies_en_memoire(&s, "T", "Unique("),
        0,
        "rien d'actif en mémoire"
    );
    assert_eq!(
        std::fs::read_to_string(&fichier).unwrap(),
        corrompu,
        "le fichier est intact : le type `Garde` n'est pas effacé"
    );
}

/// `DROP CONSTRAINT <nom> IF EXISTS` (Cypher, ce qu'écrit l'application cliente)
/// et `DROP CONSTRAINT IF EXISTS <nom>` (GQL) sont acceptés par leur parseur.
#[test]
fn drop_constraint_if_exists_dans_les_deux_syntaxes() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    let r = s
        .execute_cypher("DROP CONSTRAINT absente IF EXISTS")
        .expect("Cypher : nom puis IF EXISTS");
    assert!(format!("{r:?}").contains("No change"), "{r:?}");
    let r = s
        .execute("DROP CONSTRAINT IF EXISTS absente")
        .expect("GQL : IF EXISTS puis nom");
    assert!(format!("{r:?}").contains("No change"), "{r:?}");
    s.execute_cypher("CREATE CONSTRAINT presente FOR (n:T) REQUIRE n.a IS UNIQUE")
        .unwrap();
    s.execute("DROP CONSTRAINT IF EXISTS presente").unwrap();
    assert_eq!(copies_en_memoire(&s, "T", "Unique("), 0);
}

/// LIMITE FIXÉE : `SET n:T` n'est pas contrôlé ici — un nœud `:U` qui porte
/// un tuple déjà pris sous `:T` reçoit le label `T` sans erreur. Le contrôle
/// de l'ajout de label est porté par la PR empilée (chemins d'écriture) ; ce
/// test rougira alors, c'est voulu.
#[test]
fn limite_ajout_de_label_non_controle() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    s.execute_cypher("CREATE CONSTRAINT ua FOR (n:T) REQUIRE n.a IS UNIQUE")
        .unwrap();
    s.execute("INSERT (:T {a: 1})").unwrap();
    s.execute("INSERT (:U {a: 1})").unwrap();
    s.execute_cypher("MATCH (m:U) SET m:T")
        .expect("limite : l'ajout de label passe");
    assert_eq!(
        s.execute("MATCH (n:T {a: 1}) RETURN n.a")
            .unwrap()
            .rows
            .len(),
        2,
        "deux :T portent a = 1 — doublon par ajout de label (limite connue)"
    );
}

/// Une base ouverte en LECTURE SEULE ne répare rien sur disque : un typecat à
/// copies dupliquées sans nom est fondu en mémoire, mais le fichier reste
/// octet pour octet celui d'avant (un lecteur sous verrou partagé ne doit pas
/// écraser le DDL d'un écrivain d'un autre processus). La réouverture en
/// écriture, elle, répare.
#[test]
fn la_reparation_a_l_ouverture_n_ecrit_pas_en_lecture_seule() {
    let td = tempfile::tempdir().unwrap();
    let chemin = td.path().join("c");
    {
        let db = ObrainDB::open_substrate(&chemin).expect("création");
        db.session()
            .execute("INSERT (:T {a: 1, b: 'x'})")
            .expect("donnée initiale");
    }
    let copie = r#"{"Unique":["a","b"]}"#;
    let contenu = format!(
        "# catalogue de types obrain — genre\tnom\tdefinition(JSON)\n\
         node\tT\t{{\"name\":\"T\",\"properties\":[],\"constraints\":[{copie},{copie}],\"parent_types\":[]}}\n"
    );
    let fichier = chemin.join("substrate.typecat");
    std::fs::write(&fichier, &contenu).unwrap();
    let avant = std::fs::read(&fichier).unwrap();

    {
        let db = ObrainDB::with_config(obrain_engine::Config::read_only(&chemin))
            .expect("ouverture en lecture seule");
        // (`SHOW NODE TYPES` est refusé en lecture seule comme tout DDL : la
        // réparation en mémoire n'est pas observable ici, seul le disque l'est.)
        db.session()
            .execute("MATCH (n:T) RETURN n.a")
            .expect("lecture");
    }
    assert_eq!(
        std::fs::read(&fichier).unwrap(),
        avant,
        "en lecture seule, substrate.typecat n'est pas réécrit"
    );

    drop(ObrainDB::open_substrate(&chemin).expect("réouverture en écriture"));
    assert_eq!(
        copies_persistees(&chemin, "T", "Unique"),
        1,
        "en écriture, le fichier est réparé"
    );
}

/// Cas COMPOSITE de l'égalité entier/flottant, avec une colonne chaîne
/// sélective : `UNIQUE (b, a)`, un nœud `{a: 5.0, b: 'x'}`, puis
/// `{a: 5, b: 'x'}`. (Régression de 153be091, voir aussi
/// `contrainte_egalite_tuple.rs` pour les dates-heures et les flottants.)
#[test]
fn entier_flottant_composite_avec_colonne_chaine_est_refuse() {
    for contrainte in [
        "CREATE CONSTRAINT u FOR (n:T) REQUIRE (n.b, n.a) IS UNIQUE",
        "CREATE CONSTRAINT u FOR (n:T) REQUIRE (n.a, n.b) IS UNIQUE",
    ] {
        let db = ObrainDB::new_in_memory();
        let s = db.session();
        s.execute_cypher(contrainte).unwrap();
        for i in 0..30 {
            s.execute(&format!("INSERT (:T {{a: {}, b: 'y{i}'}})", 100 + i))
                .unwrap();
        }
        s.execute("INSERT (:T {a: 5.0, b: 'x'})").unwrap();
        assert!(
            s.execute("INSERT (:T {a: 5, b: 'x'})").is_err(),
            "`{contrainte}` : (5, 'x') = (5.0, 'x'), doublon"
        );
        assert_eq!(db.verifier_contraintes().violations_totales, 0);
    }
}
