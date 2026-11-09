//! Persistance des index a travers une fermeture/reouverture de la base.
//!
//! Un index est une structure DERIVEE : on sait toujours la rebatir depuis les
//! donnees. Ce qui ne se rebatit pas, c'est l'INTENTION — savoir qu'un index
//! DOIT exister. Ces tests portent sur l'intention, pas sur la structure.
//!
//! Avant ce travail, l'intention ne vivait nulle part : `obrain-tmn` recreait
//! ses huit index de propriete EN DUR a chaque ouverture, ce qui marchait pour
//! ces huit-la et pour aucun autre. Un index cree a l'execution repartait en
//! balayage complet, silencieusement.

use obrain_common::types::Value;
use obrain_engine::{Config, ObrainDB};

// (2026-08-30, vague 4a) GATING DE COMPILATION, pas de semantique : ces
// tests appellent des methodes elles-memes derriere `vector-index` /
// `text-index` ; sans la meme garde, `cargo test` en features PAR DEFAUT
// ne compilait plus AUCUN test d'integration du crate (E0599, casse
// introduite avec le catalogue d'index durable). Aucune assertion touchee.
#[cfg(feature = "vector-index")]
const DIM: usize = 384;

#[cfg(feature = "vector-index")]
fn vecteur(graine: u64) -> Value {
    let mut v = Vec::with_capacity(DIM);
    let mut x = graine.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1);
    for _ in 0..DIM {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        v.push((x >> 40) as f32 / 16_777_216.0 - 0.5);
    }
    Value::Vector(v.into())
}

#[cfg(feature = "vector-index")]
fn requete(graine: u64) -> Vec<f32> {
    match vecteur(graine) {
        Value::Vector(v) => v.to_vec(),
        _ => unreachable!(),
    }
}

/// Ouvre une base persistante dans un repertoire temporaire.
fn base(dir: &std::path::Path) -> ObrainDB {
    ObrainDB::with_config(Config::persistent(dir)).expect("ouverture")
}

// ─────────────────────────────────────────────────────────────────────────

/// Un index de propriete cree A L'EXECUTION doit exister apres reouverture.
///
/// C'est le cas que la liste en dur d'`obrain-tmn` ne couvrait pas : une
/// propriete indexee sur une zone nee apres l'ecriture du code.
#[test]
fn index_de_propriete_survit_a_la_reouverture() {
    let tmp = tempfile::tempdir().expect("tempdir");

    {
        let db = base(tmp.path());
        for i in 0..50u64 {
            let n = db.create_node(&["Doc"]);
            db.set_node_property(n, "categorie", Value::String(format!("c{}", i % 5).into()));
        }
        db.create_property_index("categorie");
        assert!(
            db.graph_store().has_property_index("categorie"),
            "l'index doit exister apres creation"
        );
    }

    // Reouverture : rien ne recree l'index en dur.
    let db = base(tmp.path());
    assert!(
        db.graph_store().has_property_index("categorie"),
        "l'index de propriete doit avoir ete reconstruit depuis le catalogue"
    );
    // ── (2026-08-15) EXISTER NE SUFFIT PAS : L'INDEX DOIT REPONDRE ──
    //
    // La regression 7927edc1 passait ce test tel quel : l'index EXISTAIT
    // apres reouverture — enregistre a l'ouverture, mais bati AVANT
    // l'hydratation des proprietes, donc VIDE. Toute egalite indexee
    // rendait alors zero ligne sans erreur (WHERE n.id muet, MERGE
    // dupliquant ~1 326 noeuds en production). Un test de persistance
    // d'index qui ne LIT PAS au travers de l'index rouvert ne teste que
    // le catalogue, pas l'index.
    let via_index = db
        .graph_store()
        .find_nodes_by_property("categorie", &Value::String("c0".into()));
    assert_eq!(
        via_index.len(),
        10,
        "l'index rouvert doit RENDRE les 10 noeuds c0 — un index present          mais vide est pire qu'aucun index (il court-circuite le balayage)"
    );
}

/// La declaration d'un index vectoriel survit, et l'index se rebatit A LA
/// DEMANDE — pas a l'ouverture.
///
/// La distinction est deliberee : batir un cascade en 384 dimensions au
/// demarrage d'une grosse base ajouterait un cout non borne, paye a chaque
/// redemarrage. On paie a la premiere recherche, une seule fois.
#[cfg(feature = "vector-index")]
#[test]
fn index_vectoriel_se_rebatit_a_la_premiere_recherche() {
    let tmp = tempfile::tempdir().expect("tempdir");

    {
        let db = base(tmp.path());
        for i in 0..40u64 {
            let n = db.create_node(&["Doc"]);
            db.set_node_property(n, "emb", vecteur(i));
        }
        db.create_vector_index("Doc", "emb", Some(DIM), Some("cosine"), None, None)
            .expect("creation");
        let r = db
            .vector_search("Doc", "emb", &requete(3), 5, None, None)
            .expect("recherche avant fermeture");
        assert_eq!(r.len(), 5);
    }

    // Reouverture : le REGISTRE est vide, mais le CATALOGUE se souvient.
    let db = base(tmp.path());
    let r = db
        .vector_search("Doc", "emb", &requete(3), 5, None, None)
        .expect("la recherche doit rebatir l'index depuis la declaration");
    assert_eq!(r.len(), 5, "l'index rebati doit rendre les memes voisins");
}

/// Idem pour le texte.
#[cfg(feature = "text-index")]
#[test]
fn index_texte_se_rebatit_a_la_premiere_recherche() {
    let tmp = tempfile::tempdir().expect("tempdir");

    {
        let db = base(tmp.path());
        for i in 0..30u64 {
            let n = db.create_node(&["Doc"]);
            db.set_node_property(
                n,
                "corps",
                Value::String(format!("document numero {i} sur les substrats").into()),
            );
        }
        db.create_text_index("Doc", "corps").expect("creation");
        assert!(
            !db.text_search("Doc", "corps", "substrats", 10)
                .expect("recherche")
                .is_empty()
        );
    }

    let db = base(tmp.path());
    let r = db
        .text_search("Doc", "corps", "substrats", 10)
        .expect("la recherche doit rebatir l'index depuis la declaration");
    assert!(
        !r.is_empty(),
        "l'index texte rebati doit trouver les documents"
    );
}

/// `DROP` doit valoir POUR DE BON — y compris sur une base rouverte, ou
/// l'index n'est pas encore en memoire.
///
/// C'est le piege symetrique : si le retrait ne visait que le registre, un
/// `DROP` juste apres une reouverture ne retirerait rien, et l'index
/// reapparaitrait a la recherche suivante.
#[cfg(feature = "vector-index")]
#[test]
fn drop_retire_aussi_la_declaration() {
    let tmp = tempfile::tempdir().expect("tempdir");

    {
        let db = base(tmp.path());
        for i in 0..20u64 {
            let n = db.create_node(&["Doc"]);
            db.set_node_property(n, "emb", vecteur(i));
        }
        db.create_vector_index("Doc", "emb", Some(DIM), Some("cosine"), None, None)
            .expect("creation");
    }

    {
        // Reouverture PUIS drop, sans avoir cherche : l'index n'est donc pas
        // en memoire au moment du drop.
        let db = base(tmp.path());
        db.drop_vector_index("Doc", "emb");
    }

    let db = base(tmp.path());
    assert!(
        db.vector_search("Doc", "emb", &requete(1), 5, None, None)
            .is_err(),
        "apres DROP, la recherche doit refuser — la declaration ne doit pas \
         ressusciter l'index"
    );
}

/// Le catalogue ne doit pas se remplir de doublons : declarer deux fois le
/// meme index laisse une seule entree.
#[test]
fn declarer_deux_fois_ne_duplique_pas() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let db = base(tmp.path());
    let n = db.create_node(&["Doc"]);
    db.set_node_property(n, "titre", Value::String("a".into()));

    db.create_property_index("titre");
    db.create_property_index("titre");
    db.create_property_index("titre");

    let catalogue = std::fs::read_to_string(tmp.path().join("substrate.indexcat"))
        .expect("le catalogue doit exister");
    let lignes = catalogue
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .count();
    assert_eq!(lignes, 1, "une seule declaration pour trois appels");
}
