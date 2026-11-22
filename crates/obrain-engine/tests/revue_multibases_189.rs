//! Revue #189 : arêtes externes (identité source), fonctions d'élément dans les
//! compréhensions et `reduce`, métadonnées non falsifiables. Attendus tirés à la main.
//!
//! alpha : Note n -[:L]-> Hub h -[:T]-> ref(beta, doc_1), h -[:M]-> Zed z (primaire).
//! beta  : Doc doc_1 (name 'remote', propriétés utilisateur `_base`/`_id` forgées)
//!         -[:X]-> ref(gamma, g1)  (arête externe SOURCE beta).
//! gamma : Doc g1 (name 'third').
#![cfg(all(feature = "gql", feature = "cypher"))]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

fn trois_bases() -> (ObrainDB, ObrainDB, ObrainDB) {
    let alpha = ObrainDB::new_in_memory();
    let beta = ObrainDB::new_in_memory();
    let gamma = ObrainDB::new_in_memory();
    gamma
        .session()
        .execute("INSERT (:Doc {id: 'g1', name: 'third'})")
        .expect("gamma g1");
    beta.session()
        .execute("INSERT (:Doc {id: 'doc_1', name: 'remote', _base: 'forged', _id: 42})")
        .expect("beta doc_1");
    beta.attach_bases("beta", None);
    beta.attach_base("gamma", &gamma).expect("beta -> gamma");
    beta.session()
        .execute("MATCH (d:Doc {id: 'doc_1'}) CREATE (d)-[:X]->(ref(\"gamma\", \"g1\"))")
        .expect("arete externe X");
    let sa = alpha.session();
    sa.execute("INSERT (:Note {name: 'primaire-n'})")
        .expect("note");
    sa.execute("INSERT (:Hub {name: 'primaire-h'})")
        .expect("hub");
    sa.execute("INSERT (:Zed {name: 'primaire-z'})")
        .expect("zed");
    sa.execute("MATCH (n:Note), (h:Hub) CREATE (n)-[:L]->(h)")
        .expect("L");
    sa.execute("MATCH (h:Hub), (z:Zed) CREATE (h)-[:M]->(z)")
        .expect("M");
    sa.execute("MATCH (h:Hub) CREATE (h)-[:T]->(ref(\"beta\", \"doc_1\"))")
        .expect("T");
    alpha.attach_bases("alpha", None);
    alpha.attach_base("beta", &beta).expect("alpha -> beta");
    alpha.attach_base("gamma", &gamma).expect("alpha -> gamma");
    (alpha, beta, gamma)
}

fn texte(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.to_string()),
        _ => None,
    }
}

fn liste_textes(v: &Value) -> Vec<Option<String>> {
    match v {
        Value::List(items) => items.iter().map(texte).collect(),
        autre => panic!("liste attendue : {autre:?}"),
    }
}

fn entier(v: &Value) -> i64 {
    match v {
        Value::Int64(i) => *i,
        autre => panic!("entier attendu : {autre:?}"),
    }
}

fn s(v: &[&str]) -> Vec<Option<String>> {
    v.iter().map(|x| Some((*x).to_string())).collect()
}

// ── 1. Arête externe SOURCE distante : identité (base source, id), pas de Null ─────

#[test]
fn arete_externe_depuis_base_distante_garde_son_identite() {
    let (alpha, _beta, _gamma) = trois_bases();
    let r = alpha
        .session()
        .execute(
            "MATCH WALK p = (n:Note)-[:L]->(h:Hub)-[:T]->(d)-[:X]->(g) \
             RETURN [r IN relationships(p) | type(r)], [r IN relationships(p) | r._base], \
                    [r IN relationships(p) | r._to_store], [x IN nodes(p) | x.name]",
        )
        .expect("chemin primaire -> B -> C");
    assert_eq!(r.rows.len(), 1, "{:?}", r.rows);
    assert_eq!(liste_textes(&r.rows[0][0]), s(&["L", "T", "X"]), "type(r)");
    // Pour L (primaire) : Null attendu. Pour X (externe, source beta) : identité beta.
    let bases = liste_textes(&r.rows[0][1]);
    assert_eq!(
        bases[2].as_deref(),
        Some("beta"),
        "_base de l'arête externe = base SOURCE"
    );
    assert_eq!(
        liste_textes(&r.rows[0][2])[2].as_deref(),
        Some("gamma"),
        "_to_store"
    );
    assert_eq!(
        liste_textes(&r.rows[0][3]),
        s(&["primaire-n", "primaire-h", "remote", "third"]),
        "noms des noeuds"
    );
}

#[test]
fn arete_externe_trail_une_ligne_sans_faux_rejet() {
    let (alpha, _beta, _gamma) = trois_bases();
    let r = alpha
        .session()
        .execute("MATCH TRAIL p = (n:Note)-[:L]->(h:Hub)-[:T]->(d)-[:X]->(g) RETURN length(p)")
        .expect("TRAIL externe");
    assert_eq!(r.rows.len(), 1, "{:?}", r.rows);
    assert_eq!(entier(&r.rows[0][0]), 3);
}

// ── 2. Fonctions d'élément dans les compréhensions : résultats exacts ───────────────

#[test]
fn comprehension_labels_id_type_nœuds_et_arêtes_distants() {
    let (alpha, beta, _gamma) = trois_bases();
    let id_n = match &alpha
        .session()
        .execute("MATCH (n:Note) RETURN id(n)")
        .unwrap()
        .rows[0][0]
    {
        Value::Int64(i) => *i,
        autre => panic!("{autre:?}"),
    };
    let id_doc = match &beta
        .session()
        .execute("MATCH (d:Doc {id: 'doc_1'}) RETURN id(d)")
        .unwrap()
        .rows[0][0]
    {
        Value::Int64(i) => *i,
        autre => panic!("{autre:?}"),
    };
    let r = alpha
        .session()
        .execute(
            "MATCH WALK p = (n:Note)-[:L]->(h:Hub)-[:T]->(d) \
             RETURN [x IN nodes(p) | labels(x)], [x IN nodes(p) | id(x)], [x IN relationships(p) | type(x)]",
        )
        .expect("fonctions d'élément en compréhension");
    assert_eq!(r.rows.len(), 1, "{:?}", r.rows);
    // labels : primaire -> libellés du store, distant -> _labels.
    let labels = match &r.rows[0][0] {
        Value::List(items) => items.iter().map(liste_textes).collect::<Vec<_>>(),
        autre => panic!("{autre:?}"),
    };
    assert_eq!(
        labels,
        vec![s(&["Note"]), s(&["Hub"]), s(&["Doc"])],
        "labels"
    );
    // id : primaire -> identifiant ; distant -> _id (identifiant brut de beta).
    let ids = match &r.rows[0][1] {
        Value::List(items) => items.iter().map(entier).collect::<Vec<_>>(),
        autre => panic!("{autre:?}"),
    };
    assert_eq!(ids[0], id_n, "id du noeud primaire");
    assert_eq!(
        ids[2], id_doc,
        "id du noeud distant (identifiant brut de beta)"
    );
    assert_eq!(
        liste_textes(&r.rows[0][2]),
        s(&["L", "T"]),
        "type des arêtes"
    );
}

#[test]
fn reduce_et_type_sur_chemin_primaire() {
    let (alpha, _beta, _gamma) = trois_bases();
    let r = alpha
        .session()
        .execute_cypher(
            "MATCH p = (n:Note)-[:L]->(h:Hub)-[:M]->(z:Zed) \
             RETURN reduce(acc = '', x IN relationships(p) | acc + type(x))",
        )
        .expect("reduce + type");
    assert_eq!(r.rows.len(), 1, "{:?}", r.rows);
    assert_eq!(texte(&r.rows[0][0]).as_deref(), Some("LM"));
}

// ── 3. Métadonnées non falsifiables par les propriétés utilisateur ────────────────

#[test]
fn proprietes_utilisateur_ne_falsifient_pas_les_metadonnees() {
    let (alpha, beta, _gamma) = trois_bases();
    let id_doc = match &beta
        .session()
        .execute("MATCH (d:Doc {id: 'doc_1'}) RETURN id(d)")
        .unwrap()
        .rows[0][0]
    {
        Value::Int64(i) => *i,
        autre => panic!("{autre:?}"),
    };
    let r = alpha
        .session()
        .execute(
            "MATCH WALK p = (n:Note)-[:L]->(h:Hub)-[:T]->(d) \
             RETURN [x IN nodes(p) | x._base], [x IN nodes(p) | x._id], [x IN nodes(p) | x.name]",
        )
        .expect("métadonnées");
    let bases = liste_textes(&r.rows[0][0]);
    assert_eq!(
        bases[2].as_deref(),
        Some("beta"),
        "_base : slug réel, pas 'forged'"
    );
    let ids = match &r.rows[0][1] {
        Value::List(items) => items
            .iter()
            .map(|v| if let Value::Int64(i) = v { *i } else { -1 })
            .collect::<Vec<_>>(),
        autre => panic!("{autre:?}"),
    };
    assert_eq!(ids[2], id_doc, "_id : identifiant réel, pas 42");
    assert_eq!(liste_textes(&r.rows[0][2])[2].as_deref(), Some("remote"));
}
