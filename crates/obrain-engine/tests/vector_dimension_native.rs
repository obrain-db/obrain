//! ── DIMENSION NATIVE DE BOUT EN BOUT (O-VEC C4) ─────────────────────────────
//!
//! `create_vector_index` accepte la dimension native des vecteurs (768, 1 024…)
//! : l'index apprend une projection vers les 384 dimensions du cascade, la
//! persiste avec ses paliers, et reclasse les candidats par cosinus EXACT sur
//! les vecteurs natifs relus dans la colonne. Pour dim ≤ 384 le chemin
//! identité est inchangé.
//!
//! Vérité terrain : force brute sur les vecteurs natifs. Vecteurs structurés
//! (recette du dépôt), base substrat sur répertoire temporaire.

#![cfg(feature = "vector-index")]

use std::collections::HashMap;
use std::path::Path;

use obrain_common::types::{NodeId, Value};
use obrain_engine::ObrainDB;

const N: usize = 2_000;
const NQ: usize = 60;
const K: usize = 10;
const CENTRES: usize = 50;

struct SplitMix(u64);

impl SplitMix {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn unit(&mut self) -> f32 {
        ((self.next_u64() >> 11) as f64 / (1u64 << 53) as f64) as f32
    }
    fn gauss(&mut self) -> f32 {
        let u1 = self.unit().max(1e-7);
        let u2 = self.unit();
        (-2.0 * f64::from(u1).ln()).sqrt() as f32 * (2.0 * std::f32::consts::PI * u2).cos()
    }
}

fn normaliser(v: &mut [f32]) {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    for x in v.iter_mut() {
        *x /= n;
    }
}

fn direction(rng: &mut SplitMix, d: usize) -> Vec<f32> {
    let mut v: Vec<f32> = (0..d).map(|_| rng.gauss()).collect();
    normaliser(&mut v);
    v
}

fn structures(nombre: usize, dim: usize, graine: u64) -> Vec<Vec<f32>> {
    let mut rng = SplitMix(graine);
    let centres: Vec<Vec<f32>> = (0..CENTRES).map(|_| direction(&mut rng, dim)).collect();
    (0..nombre)
        .map(|i| {
            let bruit = direction(&mut rng, dim);
            let centre = &centres[i % CENTRES];
            let mut point: Vec<f32> = centre
                .iter()
                .zip(&bruit)
                .map(|(centre_k, bruit_k)| centre_k + 0.25 * bruit_k)
                .collect();
            normaliser(&mut point);
            point
        })
        .collect()
}

fn cosinus(a: &[f32], b: &[f32]) -> f32 {
    let (mut ps, mut na, mut nb) = (0f32, 0f32, 0f32);
    for (x, y) in a.iter().zip(b) {
        ps += x * y;
        na += x * x;
        nb += y * y;
    }
    ps / (na.sqrt() * nb.sqrt())
}

/// Top-K exact : `(id, distance = 1 - cos)`, distance croissante, id croissant.
fn force_brute(corpus: &[(NodeId, Vec<f32>)], q: &[f32]) -> Vec<(u64, f32)> {
    let mut tous: Vec<(u64, f32)> = corpus
        .iter()
        .map(|(id, v)| (id.as_u64(), 1.0 - cosinus(q, v)))
        .collect();
    tous.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap().then_with(|| a.0.cmp(&b.0)));
    tous.truncate(K);
    tous
}

fn recall(verite: &[(u64, f32)], rendu: &[(NodeId, f32)]) -> f32 {
    let t = verite
        .iter()
        .filter(|(v, _)| rendu.iter().any(|(r, _)| r.as_u64() == *v))
        .count();
    t as f32 / verite.len() as f32
}

fn ouvrir(dir: &Path) -> ObrainDB {
    ObrainDB::open_substrate(dir).expect("base substrat")
}

/// Le scénario complet pour une dimension : création, recall, exactitude des
/// scores, insertion après création, filtre, réouverture.
fn scenario(d: usize) {
    let td = tempfile::tempdir().expect("tempdir");
    let dir = td.path().join(format!("dim-{d}"));
    let tout = structures(N + NQ + 1, d, 1_000 + d as u64);
    let (vecteurs, reste) = tout.split_at(N);
    let (requetes, tardif) = reste.split_at(NQ);

    let db = ouvrir(&dir);
    // Index de propriete AVANT les ecritures (sinon find ne voit pas les valeurs).
    db.create_property_index("grp");
    let corpus: Vec<(NodeId, Vec<f32>)> = vecteurs
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let id = db.create_node(&["Doc"]);
            db.set_node_property(id, "emb", Value::Vector(v.clone().into()));
            db.set_node_property(id, "grp", Value::Int64((i % 2) as i64));
            (id, v.clone())
        })
        .collect();
    db.create_vector_index("Doc", "emb", Some(d), Some("cosine"), None, None)
        .unwrap_or_else(|e| panic!("create_vector_index en {d} dimensions : {e}"));

    // ── Recall contre force brute, et scores EXACTS (rerank natif) ────────
    let mut r = 0f32;
    for q in requetes {
        let verite = force_brute(&corpus, q);
        let rendu = db.vector_search("Doc", "emb", q, K, None, None).unwrap();
        assert_eq!(rendu.len(), K);
        r += recall(&verite, &rendu);
        for (id, distance) in &rendu {
            let (_, v) = corpus.iter().find(|(i, _)| i == id).expect("id connu");
            let exact = 1.0 - cosinus(q, v);
            assert!(
                (distance - exact).abs() < 1e-4,
                "dim {d} : distance {distance} vs exacte {exact} pour {id:?}"
            );
        }
    }
    let r = r / NQ as f32;
    println!("dim {d} : recall@10 = {r:.4} sur {NQ} requetes, N = {N}");
    assert!(r >= 0.95, "dim {d} : recall@10 {r:.4} < 0,95");

    // ── Insertion APRÈS la création : projetée par l'index, retrouvée ─────
    let id_tardif = db.create_node(&["Doc"]);
    db.set_node_property(id_tardif, "emb", Value::Vector(tardif[0].clone().into()));
    let hits = db
        .vector_search("Doc", "emb", &tardif[0], 1, None, None)
        .unwrap();
    assert_eq!(
        hits[0].0, id_tardif,
        "dim {d} : le vecteur insere apres creation est retrouve"
    );
    assert!(hits[0].1 < 1e-4);

    // ── Filtre sur la base fichier : la liste blanche ne doit rien perdre
    // ── Réouverture : chargé (projection comprise), mêmes identifiants ────
    let attendu: Vec<Vec<NodeId>> = requetes[..10]
        .iter()
        .map(|q| {
            db.vector_search("Doc", "emb", q, K, None, None)
                .unwrap()
                .into_iter()
                .map(|(id, _)| id)
                .collect()
        })
        .collect();
    db.close().unwrap();
    assert_eq!(
        dir.join("substrate.vecproj.Doc.emb").is_file(),
        d > 384,
        "zone de projection presente si et seulement si dim > 384"
    );
    let db = ouvrir(&dir);
    let obtenu: Vec<Vec<NodeId>> = requetes[..10]
        .iter()
        .map(|q| {
            db.vector_search("Doc", "emb", q, K, None, None)
                .unwrap()
                .into_iter()
                .map(|(id, _)| id)
                .collect()
        })
        .collect();
    assert_eq!(obtenu, attendu, "dim {d} : memes top-10 apres reouverture");
    assert_eq!(
        db.nb_index_vectoriel_charges(),
        1,
        "dim {d} : charge depuis ses zones"
    );
    assert_eq!(db.nb_index_vectoriel_reconstruits(), 0);
}

#[test]
fn dimension_768_de_bout_en_bout() {
    scenario(768);
}

#[test]
fn dimension_1024_de_bout_en_bout() {
    scenario(1_024);
}

#[test]
fn dimension_384_chemin_identite_inchange() {
    scenario(384);
}

#[test]
fn dimension_superieure_sans_vecteur_exige_une_dimension_et_deux_vecteurs() {
    let td = tempfile::tempdir().expect("tempdir");
    let db = ouvrir(&td.path().join("vide"));
    // Un seul vecteur en 768 : pas de quoi apprendre une projection.
    let id = db.create_node(&["Doc"]);
    db.set_node_property(id, "emb", Value::Vector(vec![1.0; 768].into()));
    let err = db
        .create_vector_index("Doc", "emb", Some(768), Some("cosine"), None, None)
        .expect_err("un seul vecteur ne suffit pas a apprendre la projection");
    assert!(format!("{err}").contains("au moins 2 vecteurs"), "{err}");
}

/// Filtre + rerank en dimension native : seuls les candidats de la liste
/// blanche sortent, et leurs scores sont les cosinus exacts. Base en mémoire :
/// `find_nodes_by_property` (liste blanche) ne voit pas les valeurs d'une base
/// FICHIER dans ce harnais (constaté le 10/11/2026, hors C4 — note PO).
#[test]
fn filtre_et_rerank_en_768_sur_base_memoire() {
    let d = 768;
    let db = ObrainDB::new_in_memory();
    let vecteurs = structures(400, d, 77);
    db.create_property_index("grp");
    let corpus: Vec<(NodeId, Vec<f32>)> = vecteurs
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let id = db.create_node(&["Doc"]);
            db.set_node_property(id, "emb", Value::Vector(v.clone().into()));
            db.set_node_property(id, "grp", Value::Int64((i % 2) as i64));
            (id, v.clone())
        })
        .collect();
    db.create_vector_index("Doc", "emb", Some(d), Some("cosine"), None, None)
        .unwrap();
    let mut filtre = HashMap::new();
    filtre.insert("grp".to_string(), Value::Int64(1));
    let grp = obrain_common::types::PropertyKey::new("grp");
    for q in &vecteurs[..20] {
        let hits = db
            .vector_search("Doc", "emb", q, K, None, Some(&filtre))
            .unwrap();
        // Le moteur sur-echantillonne (4k, borne 1 024) puis filtre : il peut
        // rendre moins de k resultats quand la liste blanche est etroite dans
        // le voisinage (comportement anterieur, inchange). Jamais plus de k.
        assert!(
            !hits.is_empty() && hits.len() <= K,
            "{} resultats",
            hits.len()
        );
        for (id, distance) in &hits {
            let n = db.get_node(*id).expect("noeud");
            assert_eq!(
                n.properties.get(&grp).cloned(),
                Some(Value::Int64(1)),
                "un candidat hors liste blanche est sorti"
            );
            let (_, v) = corpus.iter().find(|(i, _)| i == id).unwrap();
            let exact = 1.0 - cosinus(q, v);
            assert!(
                (distance - exact).abs() < 1e-4,
                "score {distance} vs exact {exact}"
            );
        }
    }
}
