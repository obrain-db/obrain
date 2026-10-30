//! V3 — garde de resolution par droits : les droits du LECTEUR s'evaluent
//! sur le store CIBLE a CHAQUE franchissement de frontiere (spec paragraphe 3 et
//! paragraphe 8 — consultation de l'entite : << une requete publique ne doit pas
//! remonter vers un noeud prive via l'arete >>).
//!
//! ```bash
//! cargo test -p obrain-engine --features "gql,wal" --test droits_inter_bases
//! ```

#![cfg(feature = "gql")]

use obrain_common::types::Value;
use obrain_core::execution::{AccessContext, AccessDenied};
use obrain_engine::ObrainDB;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

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

/// Contexte de droits : `reader` n'a acces qu'aux slugs listes.
fn contexte(reader: &str, autorises: &[&str]) -> Arc<AccessContext> {
    let autorises: Vec<String> = autorises.iter().map(|s| s.to_string()).collect();
    Arc::new(AccessContext::new(
        reader,
        Box::new(move |_lecteur, slug| autorises.iter().any(|s| s == slug)),
    ))
}

#[test]
fn le_lecteur_autorise_traverse() {
    let (alpha, _beta) = deux_bases();
    let s = alpha.session();
    s.set_access_context(contexte("agent", &["alpha", "beta"]));
    let r = s
        .execute("MATCH (a:Note)-[:DOCUMENTS]->(b) RETURN b.titre, base(b)")
        .expect("traversee autorisee");
    assert_eq!(r.rows.len(), 1);
    assert_eq!(r.rows[0][0], Value::String("Guide".into()));
    let reg = alpha.base_registry().expect("registre");
    assert_eq!(reg.access_denied_refs.load(Ordering::Relaxed), 0);
}

#[test]
fn le_lecteur_non_autorise_recoit_absent_compteur_et_audit() {
    let (alpha, _beta) = deux_bases();
    let evenements: Arc<Mutex<Vec<AccessDenied>>> = Arc::new(Mutex::new(Vec::new()));
    let capte = Arc::clone(&evenements);
    let ctx = Arc::new(
        AccessContext::new("lecteur_public", Box::new(|_, slug| slug == "alpha"))
            .with_audit(Box::new(move |e| capte.lock().unwrap().push(e.clone()))),
    );
    let s = alpha.session();
    s.set_access_context(ctx);

    let r = s
        .execute("MATCH (a:Note)-[:DOCUMENTS]->(b) RETURN b.titre")
        .expect("un refus de droit ne leve JAMAIS d'erreur");
    assert_eq!(r.rows.len(), 0, "refus de droit = ABSENT trivalue");

    // Compteur admin DEDIE, separe des refs pendantes.
    let reg = alpha.base_registry().expect("registre");
    assert!(reg.access_denied_refs.load(Ordering::Relaxed) > 0);
    assert_eq!(
        reg.dangling_refs.load(Ordering::Relaxed),
        0,
        "un refus de droit n'est PAS une ref pendante"
    );

    // Audit : lecteur, store cible, clef, requete tronquee.
    let evs = evenements.lock().unwrap();
    assert!(!evs.is_empty(), "chaque refus doit etre audite");
    let e = &evs[0];
    assert_eq!(e.reader, "lecteur_public");
    assert_eq!(e.to_store, "beta");
    assert_eq!(e.to_key, "doc_1");
    assert!(
        e.query.as_deref().unwrap_or("").contains("MATCH"),
        "l'audit doit porter la requete tronquee, recu : {:?}",
        e.query
    );
}

/// Le lecteur non autorise ne doit pas pouvoir distinguer << interdit >>
/// de << inexistant >> — sinon fuite d'information par oracle.
#[test]
fn interdit_et_inexistant_sont_indistinguables_pour_le_lecteur() {
    // Cas A : cible interdite.
    let (alpha_a, _beta_a) = deux_bases();
    let sa = alpha_a.session();
    sa.set_access_context(contexte("public", &["alpha"]));
    let interdit = sa
        .execute("MATCH (a:Note)-[:DOCUMENTS]->(b) RETURN b.titre")
        .expect("jamais d'erreur");

    // Cas B : cible inexistante (ref pendante), lecteur TOUT-PUISSANT.
    let (alpha_b, beta_b) = deux_bases();
    beta_b
        .session()
        .execute("MATCH (d:Doc) DETACH DELETE d")
        .expect("suppression cible");
    let sb = alpha_b.session();
    let inexistant = sb
        .execute("MATCH (a:Note)-[:DOCUMENTS]->(b) RETURN b.titre")
        .expect("jamais d'erreur");

    // Meme reponse exactement : zero ligne, memes colonnes, pas d'erreur.
    assert_eq!(interdit.rows, inexistant.rows);
    assert_eq!(interdit.columns, inexistant.columns);

    // Mais l'ADMIN, lui, voit la difference par les compteurs.
    let reg_a = alpha_a.base_registry().expect("registre");
    let reg_b = alpha_b.base_registry().expect("registre");
    assert!(reg_a.access_denied_refs.load(Ordering::Relaxed) > 0);
    assert_eq!(reg_a.dangling_refs.load(Ordering::Relaxed), 0);
    assert_eq!(reg_b.access_denied_refs.load(Ordering::Relaxed), 0);
    assert!(reg_b.dangling_refs.load(Ordering::Relaxed) > 0);
}

/// La garde ne gate que les FRONTIERES : une session mono-base (meme avec
/// un contexte de droits qui interdit tout) garde ses requetes locales.
#[test]
fn mono_base_et_requetes_locales_inchangees() {
    let db = ObrainDB::new_in_memory();
    let s = db.session();
    s.execute("INSERT (:Note {name: 'locale'})").expect("insert");
    s.set_access_context(contexte("personne", &[]));
    let r = s
        .execute("MATCH (n:Note) RETURN n.name")
        .expect("requete locale");
    assert_eq!(r.rows.len(), 1, "aucun franchissement : la garde ne s'applique pas");
}

/// Les droits s'evaluent a CHAQUE saut : un chemin multi-bases s'arrete au
/// premier franchissement refuse — y compris le RETOUR d'un cycle A->B->A.
#[test]
fn les_droits_s_evaluent_a_chaque_saut_y_compris_le_cycle() {
    let (alpha, beta) = deux_bases();
    // beta -> alpha : le Doc annote la Note (cycle A -> B -> A).
    let note_id = {
        use obrain_core::graph::GraphStore;
        alpha.graph_store().nodes_by_label("Note")[0]
    };
    beta.attach_bases("beta", None);
    beta.session()
        .execute(&format!(
            "MATCH (d:Doc) CREATE (d)-[:ANNOTE]->(ref(\"alpha\", \"{}\"))",
            note_id.0
        ))
        .expect("create beta -> alpha");

    // Lecteur autorise sur beta mais PAS sur alpha : le premier saut passe,
    // le RETOUR vers alpha est refuse — c absent, b visible.
    let s = alpha.session();
    s.set_access_context(contexte("demi_droits", &["beta"]));
    let r = s
        .execute("MATCH (a:Note)-[:DOCUMENTS]->(b) RETURN b.titre")
        .expect("premier saut");
    assert_eq!(r.rows.len(), 1, "le saut alpha->beta est autorise");
    let r = s
        .execute("MATCH (a:Note)-[:DOCUMENTS]->(b)-[:ANNOTE]->(c) RETURN c")
        .expect("cycle");
    assert_eq!(
        r.rows.len(),
        0,
        "le retour beta->alpha doit etre refuse : les droits s'evaluent a \
         CHAQUE franchissement, pas seulement au point d'entree"
    );
    let reg = alpha.base_registry().expect("registre");
    assert!(reg.access_denied_refs.load(Ordering::Relaxed) > 0);
}

/// Le predicat d'existence respecte la garde : pour le lecteur non
/// autorise, l'arete externe est ABSENTE — NOT redevient vrai, sans que
/// cela distingue interdit d'inexistant.
#[test]
fn le_predicat_d_existence_respecte_la_garde() {
    let (alpha, _beta) = deux_bases();
    let s = alpha.session();
    s.set_access_context(contexte("public", &["alpha"]));
    let r = s
        .execute("MATCH (a:Note) WHERE (a)-[:DOCUMENTS]->() RETURN a.name")
        .expect("exists");
    assert_eq!(r.rows.len(), 0, "l'arete est invisible au lecteur refuse");
    let r = s
        .execute("MATCH (a:Note) WHERE NOT (a)-[:DOCUMENTS]->() RETURN a.name")
        .expect("not exists");
    assert_eq!(r.rows.len(), 1, "NOT est vrai : l'arete est absente POUR LUI");
}

/// Le trou P1 ne doit pas se recreer : les requetes PARAMETREES portent le
/// contexte de droits comme execute().
#[test]
fn les_requetes_parametrees_portent_les_droits() {
    let (alpha, _beta) = deux_bases();
    let q = "MATCH (a:Note)-[:DOCUMENTS]->(b) RETURN b.titre";

    let s = alpha.session();
    s.set_access_context(contexte("public", &["alpha"]));
    let r = s
        .execute_with_params(q, std::collections::HashMap::new())
        .expect("parametree refusee sans erreur");
    assert_eq!(r.rows.len(), 0, "execute_with_params doit appliquer la garde");

    let s2 = alpha.session();
    s2.set_access_context(contexte("agent", &["alpha", "beta"]));
    let r = s2
        .execute_with_params(q, std::collections::HashMap::new())
        .expect("parametree autorisee");
    assert_eq!(r.rows.len(), 1, "execute_with_params doit laisser passer l'autorise");
}
