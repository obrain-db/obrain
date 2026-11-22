//! Chemins nommés composés à travers deux bases (obrain#189).
//!
//! Les éléments distants sont émis en `Map` hydratée (`_id`, `_base`, propriétés) et
//! le dédoublonnage TRAIL / SIMPLE / ACYCLIC porte sur la clé (base, id) : deux
//! éléments de bases distinctes de même identifiant brut ne sont jamais confondus.
//! Les propriétés projetées (`[x IN nodes(p) | x.name]`) sont celles de l'élément
//! réellement traversé, primaire comme distant.
//!
//! ```bash
//! cargo test -p obrain-engine --features gql --test chemins_multibases_189
//! ```
#![cfg(all(feature = "gql", feature = "cypher"))]

use std::sync::Arc;

use obrain_common::types::Value;
use obrain_core::execution::AccessContext;
use obrain_engine::ObrainDB;

/// alpha (primaire) : Note n -[:L {k:'l'}]-> Hub h -[:T]-> ref(beta, doc_1) (V1 : pas de propriété),
/// h -[:M {k:'m'}]-> Zed z, h -[:TD]-> ref(beta, missing) (nœud distant absent).
/// beta : Doc doc_1 (name 'remote') -[:R {k:'remote-r'}]-> Doc doc_2 (name 'remote2').
fn deux_bases() -> (ObrainDB, ObrainDB) {
    let alpha = ObrainDB::new_in_memory();
    let beta = ObrainDB::new_in_memory();
    beta.session()
        .execute("INSERT (:Doc {id: 'doc_1', name: 'remote'})")
        .expect("beta doc_1");
    beta.session()
        .execute("INSERT (:Doc {id: 'doc_2', name: 'remote2'})")
        .expect("beta doc_2");
    beta.session()
        .execute("MATCH (a:Doc {id: 'doc_1'}), (b:Doc {id: 'doc_2'}) CREATE (a)-[:R {k: 'remote-r'}]->(b)")
        .expect("arete beta R");
    let sa = alpha.session();
    sa.execute("INSERT (:Note {name: 'primaire-n'})")
        .expect("note");
    sa.execute("INSERT (:Hub {name: 'primaire-h'})")
        .expect("hub");
    sa.execute("INSERT (:Zed {name: 'primaire-z'})")
        .expect("zed");
    sa.execute("MATCH (n:Note), (h:Hub) CREATE (n)-[:L {k: 'l'}]->(h)")
        .expect("L");
    sa.execute("MATCH (h:Hub), (z:Zed) CREATE (h)-[:M {k: 'm'}]->(z)")
        .expect("M");
    sa.execute("MATCH (h:Hub) CREATE (h)-[:T]->(ref(\"beta\", \"doc_1\"))")
        .expect("T externe");
    sa.execute("MATCH (h:Hub) CREATE (h)-[:TD]->(ref(\"beta\", \"missing\"))")
        .expect("TD pendante");
    alpha.attach_bases("alpha", None);
    alpha.attach_base("beta", &beta).expect("attach beta");
    (alpha, beta)
}

fn id_brut(db: &ObrainDB, requete: &str) -> i64 {
    match &db.session().execute(requete).expect("id").rows[0][0] {
        Value::Int64(i) => *i,
        autre => panic!("identifiant attendu : {autre:?}"),
    }
}

fn textes(v: &Value) -> Vec<Option<String>> {
    match v {
        Value::List(items) => items
            .iter()
            .map(|x| match x {
                Value::String(s) => Some(s.to_string()),
                Value::Null => None,
                autre => panic!("texte attendu : {autre:?}"),
            })
            .collect(),
        autre => panic!("liste attendue : {autre:?}"),
    }
}

fn attendu(v: &[&str]) -> Vec<Option<String>> {
    v.iter().map(|s| Some((*s).to_string())).collect()
}

#[test]
fn precondition_identifiants_bruts_identiques() {
    // Scénario de collision réel : le Note primaire et le Doc distant partagent leur id brut.
    let (alpha, beta) = deux_bases();
    let note = id_brut(&alpha, "MATCH (n:Note) RETURN id(n)");
    let doc = id_brut(&beta, "MATCH (d:Doc {id: 'doc_1'}) RETURN id(d)");
    assert_eq!(note, doc, "fixture invalide : identifiants bruts distincts");
}

#[test]
fn walk_inter_bases_proprietes_exactes() {
    let (alpha, _beta) = deux_bases();
    let r = alpha
        .session()
        .execute("MATCH WALK p = (n:Note)-[:L]->(h:Hub)-[:T]->(d) RETURN length(p), [x IN nodes(p) | x.name]")
        .expect("walk inter-bases");
    assert_eq!(r.rows.len(), 1, "{:?}", r.rows);
    assert_eq!(r.rows[0][0], Value::Int64(2), "longueur");
    assert_eq!(
        textes(&r.rows[0][1]),
        attendu(&["primaire-n", "primaire-h", "remote"])
    );
}

#[test]
fn modes_inter_bases_collision_d_identifiant_sans_faux_rejet() {
    // Même id brut entre n (primaire) et doc_1 (distant) : aucun mode ne doit rejeter à tort.
    let (alpha, _beta) = deux_bases();
    for mode in ["TRAIL", "SIMPLE", "ACYCLIC"] {
        let r = alpha
            .session()
            .execute(&format!(
                "MATCH {mode} p = (n:Note)-[:L]->(h:Hub)-[:T]->(d) RETURN [x IN nodes(p) | x.name]"
            ))
            .unwrap_or_else(|e| panic!("{mode} : {e}"));
        assert_eq!(r.rows.len(), 1, "{mode} : {:?}", r.rows);
        assert_eq!(
            textes(&r.rows[0][0]),
            attendu(&["primaire-n", "primaire-h", "remote"]),
            "{mode}"
        );
    }
}

#[test]
fn relations_distantes_proprietes_exactes() {
    let (alpha, _beta) = deux_bases();
    let r = alpha
        .session()
        .execute(
            "MATCH WALK p = (n:Note)-[:L]->(h:Hub)-[:T]->(d)-[:R]->(e) \
             RETURN [x IN nodes(p) | x.name], [r IN relationships(p) | r.k]",
        )
        .expect("relations inter-bases");
    assert_eq!(r.rows.len(), 1, "{:?}", r.rows);
    assert_eq!(
        textes(&r.rows[0][0]),
        attendu(&["primaire-n", "primaire-h", "remote", "remote2"])
    );
    assert_eq!(
        textes(&r.rows[0][1]),
        vec![Some("l".to_string()), None, Some("remote-r".to_string())],
        "l'arête de frontière (V1) ne porte pas de propriété : Null, jamais une valeur fabriquée"
    );
}

#[test]
fn primaire_seul_avec_registre_inchange() {
    let (alpha, _beta) = deux_bases();
    let r = alpha
        .session()
        .execute(
            "MATCH WALK p = (n:Note)-[:L]->(h:Hub)-[:M]->(z:Zed) RETURN [x IN nodes(p) | x.name]",
        )
        .expect("primaire avec registre");
    assert_eq!(r.rows.len(), 1, "{:?}", r.rows);
    assert_eq!(
        textes(&r.rows[0][0]),
        attendu(&["primaire-n", "primaire-h", "primaire-z"])
    );
}

#[test]
fn p_declare_non_utilise_resultat_exact() {
    let (alpha, _beta) = deux_bases();
    let r = alpha
        .session()
        .execute("MATCH TRAIL p = (n:Note)-[:L]->(h:Hub)-[:M]->(z:Zed) RETURN n.name, z.name")
        .expect("p non utilisé");
    assert_eq!(r.rows.len(), 1);
    assert_eq!(r.rows[0][0], Value::String("primaire-n".into()));
    assert_eq!(r.rows[0][1], Value::String("primaire-z".into()));
}

#[test]
fn noeud_distant_absent_ligne_ecartee_sans_valeur_fabriquee() {
    // Cible distante absente (référence pendante) : la ligne est ÉCARTÉE, comme pour
    // toute cible absente du store. Aucune valeur n'est fabriquée (ni Null, ni nom).
    let (alpha, _beta) = deux_bases();
    let r = alpha
        .session()
        .execute("MATCH WALK p = (n:Note)-[:L]->(h:Hub)-[:TD]->(d) RETURN [x IN nodes(p) | x.name]")
        .expect("absent");
    assert_eq!(r.rows.len(), 0, "{:?}", r.rows);
}

#[test]
fn acces_refuse_au_franchissement_ne_rend_rien() {
    let (alpha, _beta) = deux_bases();
    let session = alpha.session();
    session.set_access_context(Arc::new(AccessContext::new(
        "agent",
        Box::new(|_lecteur, slug| slug == "alpha"),
    )));
    let r = session
        .execute("MATCH WALK p = (n:Note)-[:L]->(h:Hub)-[:T]->(d) RETURN [x IN nodes(p) | x.name]")
        .expect("accès refusé");
    assert_eq!(r.rows.len(), 0, "{:?}", r.rows);
}

#[test]
fn controle_mono_base_composes_exact() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    s.execute("INSERT (:Note {name: 'n'})").expect("note");
    s.execute("INSERT (:Hub {name: 'h'})").expect("hub");
    s.execute("INSERT (:Doc {name: 'd'})").expect("doc");
    s.execute("MATCH (n:Note), (h:Hub) CREATE (n)-[:L]->(h)")
        .expect("L");
    s.execute("MATCH (h:Hub), (d:Doc) CREATE (h)-[:T]->(d)")
        .expect("T");
    let r = db
        .session()
        .execute("MATCH WALK p = (n:Note)-[:L]->(h:Hub)-[:T]->(d) RETURN length(p), [x IN nodes(p) | x.name]")
        .expect("mono-base");
    assert_eq!(r.rows.len(), 1, "{:?}", r.rows);
    assert_eq!(r.rows[0][0], Value::Int64(2));
    assert_eq!(textes(&r.rows[0][1]), attendu(&["n", "h", "d"]));
}

#[test]
fn cypher_primaire_avec_registre_inchange() {
    let (alpha, _beta) = deux_bases();
    let r = alpha
        .session()
        .execute_cypher(
            "MATCH p = (n:Note)-[:L]->(h:Hub)-[:M]->(z:Zed) RETURN [x IN nodes(p) | x.name]",
        )
        .expect("cypher primaire avec registre");
    assert_eq!(r.rows.len(), 1, "{:?}", r.rows);
    assert_eq!(
        textes(&r.rows[0][0]),
        attendu(&["primaire-n", "primaire-h", "primaire-z"])
    );
}
