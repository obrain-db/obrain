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

    // Une contrainte héritée sans nom reçoit le nom par défaut `<label>_unique`
    // et peut donc être retirée.
    let db = ObrainDB::open_substrate(&chemin).expect("réouverture");
    let session = db.session();
    session
        .execute_cypher("DROP CONSTRAINT T_unique")
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
