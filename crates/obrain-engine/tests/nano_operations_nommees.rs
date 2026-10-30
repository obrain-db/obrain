//! CHEMIN NANOSECONDE — ce que les operations nommees promettent, et la
//! frontiere exacte ou elles cessent d'etre interchangeables avec GQL.
//!
//! La lecon de la nuit : « un plan perime qui rend un mauvais resultat est
//! pire que lent ». Ces tests EPINGLENT la semantique plutot que de la
//! supposer — en particulier la difference entre `label_counts()` (un nœud
//! multi-etiquettes compte DANS CHAQUE etiquette) et l'histogramme GQL bati
//! sur `labels(n)[0]` (chaque nœud compte pour sa PREMIERE etiquette).

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

/// Histogramme GQL de reference, tel que `/api/viz/overview` le batissait.
fn histo_gql(db: &ObrainDB) -> Vec<(String, u64)> {
    let r = db
        .session()
        .execute("MATCH (n) RETURN labels(n)[0] AS t, count(n) AS nbr ORDER BY nbr DESC")
        .expect("histogramme GQL");
    r.rows
        .iter()
        .filter_map(|row| match (row.first(), row.get(1)) {
            (Some(Value::String(t)), Some(Value::Int64(n))) => Some((t.to_string(), *n as u64)),
            _ => None,
        })
        .collect()
}

/// Store MONO-etiquette : les deux vues doivent coincider exactement.
#[test]
fn label_counts_egale_histogramme_gql_en_mono_etiquette() {
    let dir = tempfile::tempdir().unwrap();
    let db = ObrainDB::open(dir.path().join("mono.obrain")).unwrap();
    {
        let s = db.session();
        for i in 0..7 {
            s.execute(&format!("INSERT (:Alpha {{i: {i}}})")).unwrap();
        }
        for i in 0..3 {
            s.execute(&format!("INSERT (:Beta {{i: {i}}})")).unwrap();
        }
    }

    let direct = db.label_counts();
    let gql = histo_gql(&db);

    assert_eq!(direct, gql, "mono-etiquette : les deux vues doivent coincider");
    assert_eq!(direct, vec![("Alpha".into(), 7), ("Beta".into(), 3)]);
    // Et la somme ferme sur le compte total.
    assert_eq!(
        direct.iter().map(|(_, c)| c).sum::<u64>(),
        db.node_count() as u64
    );
}

/// Store MULTI-etiquette : les deux vues DIVERGENT, et c'est voulu. Ce test
/// existe pour qu'un futur remplacement de l'un par l'autre bute dessus.
#[test]
fn label_counts_diverge_de_labels_zero_en_multi_etiquette() {
    let dir = tempfile::tempdir().unwrap();
    let db = ObrainDB::open(dir.path().join("multi.obrain")).unwrap();
    {
        let s = db.session();
        s.execute("INSERT (:Alpha:Beta {i: 1})").unwrap();
        s.execute("INSERT (:Alpha {i: 2})").unwrap();
    }

    let direct: std::collections::HashMap<String, u64> = db.label_counts().into_iter().collect();
    // Le nœud multi-etiquettes est compte DANS LES DEUX.
    assert_eq!(direct.get("Alpha").copied(), Some(2));
    assert_eq!(direct.get("Beta").copied(), Some(1));

    // La somme DEPASSE le compte total : c'est la signature de la divergence.
    let somme: u64 = direct.values().sum();
    assert_eq!(db.node_count() as u64, 2);
    assert_eq!(somme, 3, "somme > node_count des qu'il y a du multi-etiquette");

    // L'histogramme GQL, lui, totalise exactement node_count.
    let somme_gql: u64 = histo_gql(&db).iter().map(|(_, c)| c).sum();
    assert_eq!(somme_gql, db.node_count() as u64);
}

/// Les compteurs nommes doivent dire la meme chose que leur equivalent GQL.
#[test]
fn compteurs_nommes_accordent_avec_gql() {
    let dir = tempfile::tempdir().unwrap();
    let db = ObrainDB::open(dir.path().join("cpt.obrain")).unwrap();
    {
        let s = db.session();
        for i in 0..11 {
            s.execute(&format!("INSERT (:Noeud {{i: {i}}})")).unwrap();
        }
        s.execute(
            "MATCH (a:Noeud {i: 0}), (b:Noeud {i: 1}) INSERT (a)-[:LIE]->(b)",
        )
        .unwrap();
    }

    let un_entier = |q: &str| -> u64 {
        let r = db.session().execute(q).unwrap();
        match r.rows.first().and_then(|x| x.first()) {
            Some(Value::Int64(n)) => *n as u64,
            autre => panic!("attendu Int64, recu {autre:?} pour {q}"),
        }
    };

    assert_eq!(db.node_count() as u64, un_entier("MATCH (n) RETURN count(n)"));
    assert_eq!(
        db.edge_count() as u64,
        un_entier("MATCH ()-[r]->() RETURN count(r)")
    );
    assert_eq!(
        db.node_count_by_label("Noeud"),
        un_entier("MATCH (n:Noeud) RETURN count(n)")
    );
    // Etiquette inconnue : zero, comme un balayage vide — pas une erreur.
    assert_eq!(db.node_count_by_label("Fantome"), 0);
}

/// `label_counts()` suit les ECRITURES : un compteur fige serait pire que lent.
#[test]
fn label_counts_suit_les_ecritures() {
    let dir = tempfile::tempdir().unwrap();
    let db = ObrainDB::open(dir.path().join("vif.obrain")).unwrap();
    let s = db.session();

    s.execute("INSERT (:Vif {i: 1})").unwrap();
    assert_eq!(db.label_counts(), vec![("Vif".to_string(), 1)]);

    s.execute("INSERT (:Vif {i: 2})").unwrap();
    assert_eq!(db.label_counts(), vec![("Vif".to_string(), 2)]);

    s.execute("MATCH (n:Vif {i: 1}) DELETE n").unwrap();
    assert_eq!(
        db.label_counts(),
        vec![("Vif".to_string(), 1)],
        "la suppression doit redescendre le compteur"
    );
}

/// Le cache de plans ne doit pas rendre un resultat PERIME apres une ecriture.
/// (Il cache le PLAN, pas le resultat — ce test epingle cette frontiere.)
#[test]
fn cache_de_plans_ne_rend_pas_de_resultat_perime() {
    let dir = tempfile::tempdir().unwrap();
    let db = ObrainDB::open(dir.path().join("cache.obrain")).unwrap();
    let s = db.session();

    s.execute("INSERT (:Item {i: 1})").unwrap();
    let q = "MATCH (n:Item) RETURN count(n)";
    let lire = || -> u64 {
        match db.session().execute(q).unwrap().rows.first().and_then(|r| r.first()) {
            Some(Value::Int64(n)) => *n as u64,
            autre => panic!("attendu Int64, recu {autre:?}"),
        }
    };

    assert_eq!(lire(), 1);
    s.execute("INSERT (:Item {i: 2})").unwrap();
    // MEME texte de requete -> plan en cache. Le resultat doit bouger.
    assert_eq!(lire(), 2, "le plan est cache, PAS le resultat");
    s.execute("MATCH (n:Item {i: 2}) DELETE n").unwrap();
    assert_eq!(lire(), 1, "la suppression doit se voir malgre le plan cache");
}
