//! Verification ADVERSARIALE des aretes inter-bases (V1 ecriture, V2
//! traversee). Chaque test affirme le comportement CORRECT attendu par la
//! spec (trivalue, jamais d'erreur, aucune arete perdue) : un echec designe
//! un trou reel, pas un test fragile.
//!
//! ```bash
//! cargo test -p obrain-engine --features "gql,wal" --test verif_adversariale
//! ```

#![cfg(feature = "gql")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

/// Deux bases en memoire attachees : alpha (primaire) -> beta.
/// beta porte un Doc {id: "doc_1", titre: "Guide"}.
fn deux_bases() -> (ObrainDB, ObrainDB) {
    let alpha = ObrainDB::new_in_memory();
    let beta = ObrainDB::new_in_memory();
    beta.session()
        .execute("INSERT (:Doc {id: 'doc_1', titre: 'Guide'})")
        .expect("insert beta");
    beta.graph_store().create_property_index("id");
    let sa = alpha.session();
    sa.execute("INSERT (:Note {name: 'note agent'})")
        .expect("insert alpha");
    sa.execute("MATCH (a:Note) CREATE (a)-[:DOCUMENTS]->(ref(\"beta\", \"doc_1\"))")
        .expect("create arete externe");
    alpha.attach_bases("alpha", None);
    alpha.attach_base("beta", &beta).expect("attach beta");
    (alpha, beta)
}

// ─── Point 1 : chemins de construction de session/planner ────────────────

/// execute_with_params passe par QueryProcessor, qui construit son PROPRE
/// Planner : le registre multi-store doit y voyager aussi.
#[test]
fn p1_execute_with_params_garde_le_registre() {
    let (alpha, _beta) = deux_bases();
    let s = alpha.session();
    let q = "MATCH (a:Note)-[:DOCUMENTS]->(b) RETURN b.titre";
    assert_eq!(s.execute(q).expect("execute").rows.len(), 1);
    let r = s
        .execute_with_params(q, std::collections::HashMap::new())
        .expect("execute_with_params");
    assert_eq!(
        r.rows.len(),
        1,
        "execute_with_params doit voir les memes aretes externes que execute"
    );
}

// ─── Point 2 : sites d'abaissement d'Expand ──────────────────────────────

/// VariableLengthExpandOperator n'a pas de bras externe : un motif *1..1
/// doit pourtant rendre le meme resultat que le motif simple.
#[test]
fn p2_longueur_variable_1_1_traverse_la_frontiere() {
    let (alpha, _beta) = deux_bases();
    let r = alpha
        .session()
        .execute("MATCH (a:Note)-[:DOCUMENTS*1..1]->(b) RETURN b")
        .expect("vle");
    assert_eq!(
        r.rows.len(),
        1,
        "un motif *1..1 doit voir la meme arete externe que le motif simple"
    );
}

/// Un noeud DISTANT (Map) en source d'un motif *1..2 : au pire absent,
/// JAMAIS une erreur (contrat trivalue).
#[test]
fn p2_longueur_variable_depuis_un_noeud_distant_ne_leve_jamais_d_erreur() {
    let (alpha, beta) = deux_bases();
    beta.session()
        .execute("MATCH (d:Doc) CREATE (d)-[:CITE]->(:Source {id: 'src_1'})")
        .expect("seed");
    let r = alpha
        .session()
        .execute("MATCH (a:Note)-[:DOCUMENTS]->(b)-[:CITE*1..2]->(c) RETURN c");
    assert!(
        r.is_ok(),
        "un noeud distant en source d'un motif de longueur variable ne doit \
         JAMAIS lever d'erreur (trivalue), recu : {:?}",
        r.err()
    );
}

// ─── Point 7 : semantique trivaluee, negations ───────────────────────────

/// Le predicat d'existence (fast path ExistsSubquery -> store.edges_from)
/// doit voir les aretes externes resolubles.
#[test]
fn p7_predicat_d_existence_positif_voit_l_arete_externe() {
    let (alpha, _beta) = deux_bases();
    let r = alpha
        .session()
        .execute("MATCH (a:Note) WHERE (a)-[:DOCUMENTS]->() RETURN a.name")
        .expect("predicat existence");
    assert_eq!(
        r.rows.len(),
        1,
        "l'arete externe resoluble doit rendre le predicat d'existence vrai"
    );
}

/// NOT sur le meme predicat : l'arete externe EXISTE et RESOUT — NOT doit
/// etre faux. Un faux positif ici est le pire des mensonges trivalues.
#[test]
fn p7_not_existence_pas_de_faux_positif() {
    let (alpha, _beta) = deux_bases();
    let r = alpha
        .session()
        .execute("MATCH (a:Note) WHERE NOT (a)-[:DOCUMENTS]->() RETURN a.name")
        .expect("not existence");
    assert_eq!(
        r.rows.len(),
        0,
        "NOT (a)-[:DOCUMENTS]->() ne doit PAS matcher : l'arete externe existe et resout"
    );
}

/// count() avec refs pendantes melangees : la vivante compte 1, la
/// pendante 0 — jamais d'erreur.
#[test]
fn p7_count_avec_refs_pendantes_melangees() {
    let (alpha, beta) = deux_bases();
    let sb = beta.session();
    sb.execute("INSERT (:Doc {id: 'doc_mort', titre: 'Mort'})")
        .expect("insert doc_mort");
    let sa = alpha.session();
    sa.execute("INSERT (:Note {name: 'n2'})").expect("insert n2");
    sa.execute("MATCH (a:Note {name: 'n2'}) CREATE (a)-[:DOCUMENTS]->(ref(\"beta\", \"doc_mort\"))")
        .expect("create vers doc_mort");
    sb.execute("MATCH (d:Doc {id: 'doc_mort'}) DETACH DELETE d")
        .expect("suppression cible");
    let r = alpha
        .session()
        .execute("MATCH (a:Note)-[:DOCUMENTS]->(b) RETURN count(b)")
        .expect("count");
    assert_eq!(r.rows.len(), 1);
    assert_eq!(
        r.rows[0][0],
        Value::Int64(1),
        "1 ref vivante + 1 pendante = count 1"
    );
}

/// OPTIONAL MATCH sur la frontiere : la ref pendante rend une ligne avec
/// Null, la vivante rend la cible — jamais d'erreur.
#[test]
fn p7_optional_match_rend_null_pour_la_ref_pendante() {
    let (alpha, beta) = deux_bases();
    let sb = beta.session();
    sb.execute("INSERT (:Doc {id: 'doc_mort', titre: 'Mort'})")
        .expect("insert");
    let sa = alpha.session();
    sa.execute("INSERT (:Note {name: 'n2'})").expect("insert n2");
    sa.execute("MATCH (a:Note {name: 'n2'}) CREATE (a)-[:DOCUMENTS]->(ref(\"beta\", \"doc_mort\"))")
        .expect("create");
    sb.execute("MATCH (d:Doc {id: 'doc_mort'}) DETACH DELETE d")
        .expect("delete");
    let r = alpha
        .session()
        .execute("MATCH (a:Note) OPTIONAL MATCH (a)-[:DOCUMENTS]->(b) RETURN a.name, b")
        .expect("optional match ne doit jamais lever");
    assert_eq!(r.rows.len(), 2, "les deux notes doivent rendre une ligne");
    let nulls = r
        .rows
        .iter()
        .filter(|row| matches!(row[1], Value::Null))
        .count();
    assert_eq!(nulls, 1, "la ref pendante rend Null, la vivante rend la cible");
}

// ─── Point 3 : ambiguite de la clef to_key ───────────────────────────────

/// CAS D'ATTAQUE : le store cible a un noeud de NodeId N vivant ET un autre
/// noeud portant la propriete id = "N". La regle documentee (numerique pur
/// -> NodeId) doit s'appliquer de facon DETERMINISTE.
#[test]
fn p3_collision_nodeid_contre_propriete_id_le_nodeid_gagne() {
    let alpha = ObrainDB::new_in_memory();
    let beta = ObrainDB::new_in_memory();
    let sb = beta.session();
    sb.execute("INSERT (:Doc {titre: 'ParSlot'})").expect("insert 1");
    let slot = beta.graph_store().nodes_by_label("Doc")[0];
    sb.execute(&format!(
        "INSERT (:Doc {{id: '{}', titre: 'ParPropriete'}})",
        slot.0
    ))
    .expect("insert 2");
    beta.graph_store().create_property_index("id");

    let sa = alpha.session();
    sa.execute("INSERT (:Note {name: 'n'})").expect("insert");
    sa.execute(&format!(
        "MATCH (a:Note) CREATE (a)-[:VOIT]->(ref(\"beta\", \"{}\"))",
        slot.0
    ))
    .expect("create");
    alpha.attach_bases("alpha", None);
    alpha.attach_base("beta", &beta).expect("attach");

    let r = alpha
        .session()
        .execute("MATCH (a:Note)-[:VOIT]->(b) RETURN b.titre")
        .expect("traversee");
    assert_eq!(r.rows.len(), 1, "la resolution doit etre univoque : UNE ligne");
    assert_eq!(
        r.rows[0][0],
        Value::String("ParSlot".into()),
        "regle documentee : clef entierement numerique = NodeId brut, \
         la propriete id est MASQUEE"
    );
}

/// Clef numerique qui deborde u64 (20 chiffres) : elle est "entierement
/// numerique" donc route NodeId, qui ne peut pas exister -> absente, MEME
/// si un noeud porte exactement cette propriete id. Pas d'erreur.
#[test]
fn p3_clef_numerique_debordant_u64_absente_sans_erreur() {
    let alpha = ObrainDB::new_in_memory();
    let beta = ObrainDB::new_in_memory();
    beta.session()
        .execute("INSERT (:Doc {id: '99999999999999999999', titre: 'Deborde'})")
        .expect("insert");
    beta.graph_store().create_property_index("id");
    let sa = alpha.session();
    sa.execute("INSERT (:Note {name: 'n'})").expect("insert");
    sa.execute(
        "MATCH (a:Note) CREATE (a)-[:VOIT]->(ref(\"beta\", \"99999999999999999999\"))",
    )
    .expect("create");
    alpha.attach_bases("alpha", None);
    alpha.attach_base("beta", &beta).expect("attach");
    let r = alpha
        .session()
        .execute("MATCH (a:Note)-[:VOIT]->(b) RETURN b.titre")
        .expect("jamais d'erreur");
    // PIEGE DOCUMENTE PAR CE TEST : la propriete id="9999...9" (20 chiffres)
    // est inatteignable par ref() — la clef route NodeId et le parse u64
    // echoue en silence.
    assert_eq!(r.rows.len(), 0, "comportement actuel : absent (piege a documenter)");
}

/// Zeros de tete : "007" route NodeId(7) — la propriete id="007" est
/// masquee. On fige le comportement actuel et on documente le piege.
#[test]
fn p3_clef_avec_zeros_de_tete_route_nodeid_et_masque_la_propriete() {
    let alpha = ObrainDB::new_in_memory();
    let beta = ObrainDB::new_in_memory();
    beta.session()
        .execute("INSERT (:Doc {id: '007', titre: 'Bond'})")
        .expect("insert");
    beta.graph_store().create_property_index("id");
    let sa = alpha.session();
    sa.execute("INSERT (:Note {name: 'n'})").expect("insert");
    sa.execute("MATCH (a:Note) CREATE (a)-[:VOIT]->(ref(\"beta\", \"007\"))")
        .expect("create");
    alpha.attach_bases("alpha", None);
    alpha.attach_base("beta", &beta).expect("attach");
    let r = alpha
        .session()
        .execute("MATCH (a:Note)-[:VOIT]->(b) RETURN b.titre")
        .expect("jamais d'erreur");
    // NodeId(7) n'existe pas dans beta (2 noeuds max) : absent, la
    // propriete "007" est masquee par la regle numerique.
    assert_eq!(
        r.rows.len(),
        0,
        "comportement actuel : '007' = NodeId(7) absent, propriete masquee (piege)"
    );
}

/// Clef vide : absente, pas d'erreur, pas de resolution fantaisiste.
#[test]
fn p3_clef_vide_absente_sans_erreur() {
    let (alpha, _beta) = deux_bases();
    alpha
        .session()
        .execute("MATCH (a:Note) CREATE (a)-[:VIDE]->(ref(\"beta\", \"\"))")
        .expect("create clef vide");
    let r = alpha
        .session()
        .execute("MATCH (a:Note)-[:VIDE]->(b) RETURN b")
        .expect("clef vide : jamais d'erreur");
    assert_eq!(r.rows.len(), 0);
}

/// Clefs avec espace et unicode : resolvent par la propriete id.
#[test]
fn p3_clef_espace_et_unicode_resolvent_par_propriete() {
    let alpha = ObrainDB::new_in_memory();
    let beta = ObrainDB::new_in_memory();
    let sb = beta.session();
    sb.execute("INSERT (:Doc {id: 'doc 1', titre: 'AvecEspace'})")
        .expect("insert espace");
    sb.execute("INSERT (:Doc {id: 'clé-α', titre: 'Unicode'})")
        .expect("insert unicode");
    beta.graph_store().create_property_index("id");
    let sa = alpha.session();
    sa.execute("INSERT (:Note {name: 'n'})").expect("insert");
    sa.execute("MATCH (a:Note) CREATE (a)-[:E1]->(ref(\"beta\", \"doc 1\"))")
        .expect("create espace");
    sa.execute("MATCH (a:Note) CREATE (a)-[:E2]->(ref(\"beta\", \"clé-α\"))")
        .expect("create unicode");
    alpha.attach_bases("alpha", None);
    alpha.attach_base("beta", &beta).expect("attach");
    let s = alpha.session();
    let r = s
        .execute("MATCH (a:Note)-[:E1]->(b) RETURN b.titre")
        .expect("espace");
    assert_eq!(r.rows.len(), 1);
    assert_eq!(r.rows[0][0], Value::String("AvecEspace".into()));
    let r = s
        .execute("MATCH (a:Note)-[:E2]->(b) RETURN b.titre")
        .expect("unicode");
    assert_eq!(r.rows.len(), 1);
    assert_eq!(r.rows[0][0], Value::String("Unicode".into()));
}

// ─── Point 6 : ROLLBACK et registre externe ──────────────────────────────

/// (V3) Tant que le registre externe est HORS undo-log, les ecritures
/// inter-bases sont REFUSEES en transaction explicite — un ROLLBACK ne
/// saurait pas les defaire. Le message enseigne le bon geste, et la meme
/// ecriture passe hors transaction (auto-commit).
#[test]
fn p6_rollback_annule_l_arete_externe_creee_dans_la_transaction() {
    let (alpha, _beta) = deux_bases();
    let store = alpha.graph_store();
    assert_eq!(store.external_edge_count(), 1);
    let s = alpha.session();
    s.execute("START TRANSACTION").expect("begin");
    let err = s
        .execute("MATCH (a:Note) CREATE (a)-[:TEMP]->(ref(\"beta\", \"doc_1\"))")
        .expect_err("(V3) CREATE ref() doit etre refuse en transaction explicite");
    let msg = format!("{err}");
    assert!(
        msg.contains("transaction") && msg.contains("annulation"),
        "le refus doit enseigner le bon geste (hors transaction / undo-log), recu : {msg}"
    );
    s.execute("ROLLBACK").expect("rollback");
    assert_eq!(store.external_edge_count(), 1, "rien ne doit avoir ete ecrit");
    // Hors transaction (auto-commit) : la meme ecriture passe.
    s.execute("MATCH (a:Note) CREATE (a)-[:TEMP]->(ref(\"beta\", \"doc_1\"))")
        .expect("hors transaction : accepte");
    assert_eq!(store.external_edge_count(), 2);
}

/// Apres un ROLLBACK de DETACH DELETE : quel que soit le sort de l'arete,
/// AUCUNE corruption — registre, inverse et traversee restent coherents
/// entre eux, et rien ne leve d'erreur.
#[test]
fn p6_rollback_de_detach_delete_ne_corrompt_pas_les_registres() {
    let (alpha, _beta) = deux_bases();
    let store = alpha.graph_store();
    let s = alpha.session();
    s.execute("START TRANSACTION").expect("begin");
    // (V3) Le noeud porte une arete inter-bases (hors undo-log) : DETACH
    // DELETE est REFUSE dans la transaction explicite, avec enseignement.
    let err = s
        .execute("MATCH (a:Note) DETACH DELETE a")
        .expect_err("(V3) DETACH DELETE porteur d aretes externes refuse en transaction");
    assert!(
        format!("{err}").contains("inter-bases"),
        "le refus doit nommer les aretes inter-bases, recu : {err}"
    );
    s.execute("ROLLBACK").expect("rollback");

    // Le noeud est-il revenu ? (MVCC pour les noeuds)
    let notes = s.execute("MATCH (a:Note) RETURN a.name").expect("match");
    // Coherence interne : compte global == somme des sortantes == inverse.
    let count = store.external_edge_count();
    let par_noeud: u64 = store
        .nodes_by_label("Note")
        .iter()
        .map(|n| store.external_edges_from(*n).len() as u64)
        .sum();
    assert_eq!(
        count, par_noeud,
        "external_edge_count et external_edges_from doivent rester coherents"
    );
    let inbound = store.external_inbound_refs("beta", "doc_1");
    assert_eq!(
        inbound.len() as u64,
        count,
        "le registre inverse doit rester le miroir exact du registre direct"
    );
    // Et la traversee ne leve jamais.
    let r = s
        .execute("MATCH (a:Note)-[:DOCUMENTS]->(b) RETURN b")
        .expect("traversee apres rollback");
    assert!(r.rows.len() <= 1);
    let _ = notes;
}

// ─── Point 8 : double ouverture par chemin ───────────────────────────────

/// (V3, parade en place) Attacher PAR CHEMIN une base deja ouverte dans
/// le meme processus REUTILISE l'instance vivante (registre processus
/// {chemin canonique -> Weak<SubstrateStore>} consulte par l'ouvreur
/// paresseux) : les ecritures via la premiere instance restent visibles
/// par la traversee — plus de sidecars divergents.
#[cfg(feature = "wal")]
#[test]
fn p8_double_ouverture_par_chemin_les_ecritures_restent_visibles() {
    let dir = tempfile::tempdir().expect("tempdir");
    let chemin_beta = dir.path().join("beta");
    let beta = ObrainDB::open(&chemin_beta).expect("open beta");
    beta.session()
        .execute("INSERT (:Doc {id: 'doc_1', titre: 'V1'})")
        .expect("insert");

    let alpha = ObrainDB::new_in_memory();
    let sa = alpha.session();
    sa.execute("INSERT (:Note {name: 'n'})").expect("insert");
    sa.execute("MATCH (a:Note) CREATE (a)-[:DOCUMENTS]->(ref(\"beta\", \"doc_1\"))")
        .expect("create");
    alpha.attach_bases("alpha", None);
    alpha
        .attach_base_path("beta", &chemin_beta)
        .expect("attach par chemin");

    // Premiere traversee : declenche l'ouverture PARESSEUSE d'une SECONDE
    // instance des memes fichiers, pendant que `beta` reste ouverte.
    let r1 = alpha
        .session()
        .execute("MATCH (a:Note)-[:DOCUMENTS]->(b) RETURN b.titre")
        .expect("traversee 1");
    assert_eq!(
        r1.rows.len(),
        1,
        "l'etat de beta AVANT l'attache doit se voir via le chemin"
    );

    // Ecriture via la PREMIERE instance apres l'ouverture paresseuse.
    beta.session()
        .execute("MATCH (d:Doc {id: 'doc_1'}) SET d.titre = 'V2'")
        .expect("set");
    let r2 = alpha
        .session()
        .execute("MATCH (a:Note)-[:DOCUMENTS]->(b) RETURN b.titre")
        .expect("traversee 2");
    assert_eq!(
        r2.rows[0][0],
        Value::String("V2".into()),
        "DANGER double-ouverture : l'ecriture via la premiere instance est \
         invisible via la seconde (sidecars divergents)"
    );
}

// ─── Point 9 : plans mono-base vs multi-base (impression) ────────────────

/// Impression des plans EXPLAIN pour verification manuelle : la session
/// mono-base doit garder ses chemins historiques.
#[test]
fn p9_explain_mono_et_multi_base_impression() {
    // Mono-base : deux sauts, aggregat — candidats aux chemins factorises.
    let mono = ObrainDB::new_in_memory();
    let sm = mono.session();
    sm.execute("CREATE (:A {x: 1})-[:R]->(:B {x: 2})-[:S]->(:C {x: 3})")
        .expect("seed");
    let r = sm
        .execute("EXPLAIN MATCH (a:A)-[:R]->(b)-[:S]->(c) RETURN count(c)")
        .expect("explain mono");
    println!("PLAN MONO-BASE (2 sauts, count):\n{:?}", r.rows);

    let (alpha, _beta) = deux_bases();
    let r = alpha
        .session()
        .execute("EXPLAIN MATCH (a:Note)-[:DOCUMENTS]->(b)-[:S]->(c) RETURN count(c)")
        .expect("explain multi");
    println!("PLAN MULTI-BASE (2 sauts, count):\n{:?}", r.rows);
}

/// Depuis une source LOCALE, un motif *1..2 doit voir l'arete externe du
/// premier saut (le motif simple la voit).
#[test]
fn p2_longueur_variable_1_2_depuis_source_locale_voit_l_arete_externe() {
    let (alpha, _beta) = deux_bases();
    let r = alpha
        .session()
        .execute("MATCH (a:Note)-[:DOCUMENTS*1..2]->(b) RETURN b")
        .expect("vle 1..2");
    assert_eq!(
        r.rows.len(),
        1,
        "le motif *1..2 doit voir l'arete externe que le motif simple voit"
    );
}
