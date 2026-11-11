//! Régressions de la recherche par propriété et de la recherche vectorielle
//! filtrée sur base substrat FICHIER.
//!
//! - `find_nodes_by_property` rend exactement les nœuds porteurs.
//! - Une recherche vectorielle filtrée est EXACTE : un filtre valide ne rend
//!   pas zéro résultat sous prétexte que les voisins les plus proches sont
//!   tous hors liste blanche (défaut relevé le 11/11/2026 : données groupées,
//!   les 40 premiers voisins étaient tous du groupe 0, filtre sur le groupe 1,
//!   résultat vide ; l'ancien sur-échantillonnage fixe ×4 ne suffisait pas).

use std::collections::HashMap;

use obrain_common::types::{NodeId, Value};
use obrain_engine::ObrainDB;

#[test]
fn find_par_int64_sur_base_fichier_rend_les_noeuds_porteurs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = ObrainDB::open_substrate(dir.path().join("b")).expect("base");
    db.create_property_index("grp");
    let mut attendus: Vec<NodeId> = Vec::new();
    for i in 0..200 {
        let id = db.create_node(&["Doc"]);
        db.set_node_property(id, "grp", Value::Int64((i % 2) as i64));
        if i % 2 == 1 {
            attendus.push(id);
        }
    }
    let trouves = db.find_nodes_by_property("grp", &Value::Int64(1));
    assert_eq!(trouves.len(), attendus.len());
}

#[test]
fn find_par_int64_survit_a_une_reouverture() {
    let dir = tempfile::tempdir().expect("tempdir");
    let chemin = dir.path().join("b");
    {
        let db = ObrainDB::open_substrate(&chemin).expect("base");
        db.create_property_index("grp");
        for i in 0..200 {
            let id = db.create_node(&["Doc"]);
            db.set_node_property(id, "emb", Value::Vector(vec![1.0, 0.0, 0.0, 0.0].into()));
            db.set_node_property(id, "grp", Value::Int64((i % 2) as i64));
        }
        db.close().expect("close");
    }
    let db = ObrainDB::open_substrate(&chemin).expect("réouverture");
    let trouves = db.find_nodes_by_property("grp", &Value::Int64(1));
    assert_eq!(trouves.len(), 100);
}

/// Données GROUPÉES : 50 centres, le groupe d'un point est sa parité ; les
/// points d'un même centre partagent donc le même groupe. Une requête proche
/// d'un centre a pour voisins les plus proches uniquement des points du groupe
/// de ce centre : le filtre sur l'autre groupe doit quand même rendre k
/// résultats (la recherche filtrée élargit son top-k jusqu'à trouver k permis).
#[test]
fn filtre_exact_sur_donnees_groupees() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = ObrainDB::open_substrate(dir.path().join("b")).expect("base");
    db.create_property_index("grp");
    let d = 64usize;
    let centres = 50usize;
    let mut graine: u64 = 9;
    let mut alea = || {
        graine = graine
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((graine >> 33) as f32) / (1u64 << 31) as f32 - 0.5
    };
    let cs: Vec<Vec<f32>> = (0..centres)
        .map(|_| (0..d).map(|_| alea()).collect())
        .collect();
    for i in 0..2000usize {
        let id = db.create_node(&["Doc"]);
        let v: Vec<f32> = (0..d).map(|k| cs[i % centres][k] + 0.25 * alea()).collect();
        db.set_node_property(id, "emb", Value::Vector(v.into()));
        db.set_node_property(id, "grp", Value::Int64((i % 2) as i64));
    }
    db.create_vector_index("Doc", "emb", Some(d), Some("cosine"), None, None)
        .unwrap();
    let q: Vec<f32> = (0..d).map(|k| cs[0][k] + 0.25 * alea()).collect();
    let mut filtre = HashMap::new();
    filtre.insert("grp".to_string(), Value::Int64(1));
    let hits = db
        .vector_search("Doc", "emb", &q, 10, None, Some(&filtre))
        .unwrap();
    assert_eq!(hits.len(), 10, "un filtre valide doit rendre k résultats");
    let grp = obrain_common::types::PropertyKey::new("grp");
    for (id, _) in &hits {
        assert_eq!(
            db.get_node(*id).unwrap().properties.get(&grp).cloned(),
            Some(Value::Int64(1)),
            "un candidat hors liste blanche est sorti"
        );
    }
}
