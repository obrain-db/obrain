//! Aretes inter-bases V1 — ecriture GQL : `ref("slug", "clef")` en
//! CREATE/MERGE, refus enseignants, DETACH DELETE, durabilite.
//!
//! ```bash
//! cargo test -p obrain-engine --features "gql,wal" --test aretes_inter_bases
//! ```

#![cfg(feature = "gql")]

use obrain_engine::ObrainDB;

/// Le CREATE cible ref() ecrit une arete externe dans le store SOURCE.
#[test]
fn create_vers_ref_ecrit_une_arete_externe_locale() {
    let db = ObrainDB::new_in_memory();
    let session = db.session();
    session
        .execute("INSERT (:Note {name: 'doc agent'})")
        .expect("insert");
    session
        .execute(
            "MATCH (a:Note) CREATE (a)-[:DOCUMENTS]->(ref(\"task_7problems\", \"note_6b608fa4\"))",
        )
        .expect("create arete externe");

    let store = db.graph_store();
    let noeud = store.nodes_by_label("Note")[0];
    let externes = store.external_edges_from(noeud);
    assert_eq!(externes.len(), 1);
    assert_eq!(externes[0].edge_type, "DOCUMENTS");
    assert_eq!(externes[0].to_store, "task_7problems");
    assert_eq!(externes[0].to_key, "note_6b608fa4");
    assert_eq!(externes[0].to_host, None);
    // Registre inverse (inbound_refs).
    let inbound = store.external_inbound_refs("task_7problems", "note_6b608fa4");
    assert_eq!(inbound.len(), 1);
    assert_eq!(inbound[0].0, noeud);
}

/// La forme a trois arguments reserve l'hote (multi-host) : parse et
/// encode aujourd'hui, resolution reseau plus tard.
#[test]
fn ref_a_trois_arguments_encode_l_hote() {
    let db = ObrainDB::new_in_memory();
    let session = db.session();
    session.execute("INSERT (:Note {name: 'n'})").expect("insert");
    session
        .execute("MATCH (a:Note) CREATE (a)-[:MIRRORS]->(ref(\"dgx-2\", \"agent/system\", \"note_x\"))")
        .expect("create trois arguments");
    let store = db.graph_store();
    let noeud = store.nodes_by_label("Note")[0];
    let externes = store.external_edges_from(noeud);
    assert_eq!(externes[0].to_host.as_deref(), Some("dgx-2"));
    assert_eq!(externes[0].to_store, "agent/system");
}

/// MERGE est idempotent : une arete identique n'est pas dupliquee, une
/// arete differente (autre clef) s'ajoute.
#[test]
fn merge_vers_ref_est_idempotent() {
    let db = ObrainDB::new_in_memory();
    let session = db.session();
    session.execute("INSERT (:Note {name: 'n'})").expect("insert");
    for _ in 0..3 {
        session
            .execute("MATCH (a:Note) MERGE (a)-[:DOCUMENTS]->(ref(\"cible\", \"k1\"))")
            .expect("merge");
    }
    session
        .execute("MATCH (a:Note) MERGE (a)-[:DOCUMENTS]->(ref(\"cible\", \"k2\"))")
        .expect("merge autre clef");
    let store = db.graph_store();
    let noeud = store.nodes_by_label("Note")[0];
    assert_eq!(
        store.external_edges_from(noeud).len(),
        2,
        "3 MERGE identiques = 1 arete ; la clef differente en ajoute une"
    );
}

/// Refus PROPRE : la SOURCE d'une arete inter-bases doit vivre dans la
/// base courante. Le message enseigne le bon geste.
#[test]
fn ref_comme_source_est_refuse_avec_un_message_enseignant() {
    let db = ObrainDB::new_in_memory();
    let session = db.session();
    session.execute("INSERT (:Note {name: 'n'})").expect("insert");
    let err = session
        .execute("MATCH (a:Note) CREATE (ref(\"autre\", \"k\"))-[:DOCUMENTS]->(a)")
        .expect_err("la source distante doit etre refusee");
    let msg = format!("{err}");
    assert!(
        msg.contains("base courante") && msg.contains("WAL"),
        "le refus doit enseigner la regle une-seule-WAL, recu : {msg}"
    );
    // La fleche inversee met aussi la source ailleurs : meme refus.
    let err = session
        .execute("MATCH (a:Note) CREATE (a)<-[:DOCUMENTS]-(ref(\"autre\", \"k\"))")
        .expect_err("la source distante doit etre refusee");
    assert!(format!("{err}").contains("base courante"));
}

/// ref() n'est ni un motif de lecture ni un createur de noeud.
#[test]
fn ref_hors_cible_d_ecriture_est_refuse() {
    let db = ObrainDB::new_in_memory();
    let session = db.session();
    let err = session
        .execute("CREATE (ref(\"autre\", \"k\"))")
        .expect_err("ref() seul ne cree rien");
    assert!(format!("{err}").contains("CIBLE"));
}

/// V1 : pas de proprietes ni de variable d'arete sur une arete externe.
#[test]
fn arete_externe_riche_est_refusee_en_v1() {
    let db = ObrainDB::new_in_memory();
    let session = db.session();
    session.execute("INSERT (:Note {name: 'n'})").expect("insert");
    let err = session
        .execute("MATCH (a:Note) CREATE (a)-[r:DOCUMENTS]->(ref(\"autre\", \"k\"))")
        .expect_err("variable d'arete refusee en V1");
    assert!(format!("{err}").contains("V1"));
    let err = session
        .execute("MATCH (a:Note) CREATE (a)-[:DOCUMENTS {poids: 1}]->(ref(\"autre\", \"k\"))")
        .expect_err("proprietes refusees en V1");
    assert!(format!("{err}").contains("V1"));
}

/// DETACH DELETE d'un noeud emporte ses aretes externes SORTANTES (elles
/// vivent chez lui) ; celles des autres noeuds ne bougent pas.
#[test]
fn detach_delete_emporte_les_aretes_externes_sortantes() {
    let db = ObrainDB::new_in_memory();
    let session = db.session();
    session.execute("INSERT (:Note {name: 'a'})").expect("insert");
    session.execute("INSERT (:Autre {name: 'b'})").expect("insert");
    session
        .execute("MATCH (a:Note) CREATE (a)-[:DOCUMENTS]->(ref(\"cible\", \"k1\"))")
        .expect("create");
    session
        .execute("MATCH (b:Autre) CREATE (b)-[:DOCUMENTS]->(ref(\"cible\", \"k2\"))")
        .expect("create");
    let store = db.graph_store();
    assert_eq!(store.external_edge_count(), 2);

    session
        .execute("MATCH (a:Note) DETACH DELETE a")
        .expect("detach delete");
    assert_eq!(
        store.external_edge_count(),
        1,
        "les sortantes du noeud supprime partent avec lui, celles de b restent"
    );
    assert!(store.external_inbound_refs("cible", "k1").is_empty());
    assert_eq!(store.external_inbound_refs("cible", "k2").len(), 1);
}

/// Persistance de bout en bout par le moteur : close puis reopen.
/// (Le kill-sans-flush est prouve au niveau substrate :
/// crates/obrain-substrate/tests/extedges_recovery.rs.)
#[cfg(feature = "wal")]
#[test]
fn les_aretes_externes_survivent_a_une_reouverture_moteur() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("base_source");
    {
        let db = ObrainDB::open(&path).expect("open");
        let session = db.session();
        session.execute("INSERT (:Note {name: 'n'})").expect("insert");
        session
            .execute("MATCH (a:Note) CREATE (a)-[:DOCUMENTS]->(ref(\"agent/system\", \"note_1\"))")
            .expect("create");
        db.close().expect("close");
    }
    {
        let db = ObrainDB::open(&path).expect("reopen");
        let store = db.graph_store();
        assert_eq!(store.external_edge_count(), 1);
        let noeud = store.nodes_by_label("Note")[0];
        let externes = store.external_edges_from(noeud);
        assert_eq!(externes[0].to_store, "agent/system");
        db.close().expect("close");
    }
}

/// La suppression d'une CIBLE ne touche rien ailleurs : supprimer un noeud
/// reference par une autre base ne modifie pas le store qui le reference
/// (la resolution rendra simplement absent — V2).
#[test]
fn supprimer_la_cible_ne_touche_pas_le_store_source() {
    // Store source : pointe vers "cible"/42.
    let source = ObrainDB::new_in_memory();
    let s_source = source.session();
    s_source.execute("INSERT (:Note {name: 'n'})").expect("insert");
    s_source
        .execute("MATCH (a:Note) CREATE (a)-[:DOCUMENTS]->(ref(\"cible\", \"42\"))")
        .expect("create");

    // Store cible : vit sa vie, supprime son noeud.
    let cible = ObrainDB::new_in_memory();
    let s_cible = cible.session();
    s_cible.execute("INSERT (:Doc {name: 'd'})").expect("insert");
    s_cible.execute("MATCH (d:Doc) DETACH DELETE d").expect("delete");

    // Rien n'a bouge cote source : l'adresse reste, la ref est pendante.
    let store = source.graph_store();
    assert_eq!(store.external_edge_count(), 1);
}

/// Forme de plan : le CREATE vers ref() se planifie en CreateExternalEdge
/// (et non CreateNode+CreateEdge), avec la cible en clair.
#[test]
fn explain_montre_l_operateur_create_external_edge() {
    let db = ObrainDB::new_in_memory();
    let session = db.session();
    session.execute("INSERT (:Note {name: \x27n\x27})").expect("insert");
    let r = session
        .execute(
            "EXPLAIN MATCH (a:Note) CREATE (a)-[:DOCUMENTS]->(ref(\"agent/system\", \"note_1\"))",
        )
        .expect("explain");
    let plan = format!("{:?}", r.rows);
    assert!(
        plan.contains("CreateExternalEdge"),
        "le plan doit porter la frontiere en clair, recu : {plan}"
    );
    assert!(plan.contains("agent/system"));
    // La variante MERGE est marquee comme telle.
    let r = session
        .execute("EXPLAIN MATCH (a:Note) MERGE (a)-[:DOCUMENTS]->(ref(\"agent/system\", \"note_1\"))")
        .expect("explain merge");
    let plan = format!("{:?}", r.rows);
    assert!(plan.contains("CreateExternalEdge (merge)"), "recu : {plan}");
}

/// CREATE autonome (chemin translate_insert) : source creee et arete
/// externe posee dans le meme statement.
#[test]
fn create_autonome_source_et_ref_dans_le_meme_statement() {
    let db = ObrainDB::new_in_memory();
    let session = db.session();
    session
        .execute("CREATE (a:Note {name: \x27n\x27})-[:DOCUMENTS]->(ref(\"beta\", \"doc_1\"))")
        .expect("create autonome");
    let store = db.graph_store();
    let noeud = store.nodes_by_label("Note")[0];
    let externes = store.external_edges_from(noeud);
    assert_eq!(externes.len(), 1);
    assert_eq!(externes[0].to_store, "beta");
}
