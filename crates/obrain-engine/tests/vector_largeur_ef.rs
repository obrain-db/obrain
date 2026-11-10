//! C2 (plan O-VEC) — le paramètre `ef` de `vector_search` est la largeur de
//! candidats du cascade du substrat (`k0`, avec `k1 = k0 / 10`) : plus large,
//! plus de recall ; `None` = largeurs par défaut de l'index (4 000 / 400).
//!
//! Vecteurs 64-d structurés (recette « centres + 0,25·direction » du dépôt),
//! vérité terrain par force brute, base substrat sur répertoire temporaire.

#![cfg(feature = "vector-index")]

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

const D: usize = 64;
const N: usize = 3_000;
const NQ: usize = 50;
const K: usize = 10;
const CENTRES: usize = 60;

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

fn direction(rng: &mut SplitMix) -> Vec<f32> {
    let mut v: Vec<f32> = (0..D).map(|_| rng.gauss()).collect();
    normaliser(&mut v);
    v
}

fn structures(n: usize, graine: u64) -> Vec<Vec<f32>> {
    let mut rng = SplitMix(graine);
    let centres: Vec<Vec<f32>> = (0..CENTRES).map(|_| direction(&mut rng)).collect();
    (0..n)
        .map(|i| {
            let u = direction(&mut rng);
            let c = &centres[i % CENTRES];
            let mut v: Vec<f32> = (0..D).map(|k| c[k] + 0.25 * u[k]).collect();
            normaliser(&mut v);
            v
        })
        .collect()
}

fn cosinus(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>()
}

fn force_brute(corpus: &[Vec<f32>], q: &[f32]) -> Vec<usize> {
    let mut tous: Vec<(usize, f32)> = corpus
        .iter()
        .enumerate()
        .map(|(i, v)| (i, cosinus(q, v)))
        .collect();
    tous.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap().then_with(|| a.0.cmp(&b.0)));
    tous.into_iter().take(K).map(|(i, _)| i).collect()
}

fn recall(verite: &[u64], rendu: &[(obrain_common::types::NodeId, f32)]) -> f32 {
    let trouves = verite
        .iter()
        .filter(|v| rendu.iter().any(|(id, _)| id.as_u64() == **v))
        .count();
    trouves as f32 / verite.len() as f32
}

#[test]
fn ef_est_la_largeur_de_candidats_du_cascade() {
    let dir = tempfile::tempdir().expect("repertoire temporaire");
    let db = ObrainDB::open_substrate(dir.path()).expect("base substrat");
    let tout = structures(N + NQ, 11);
    let (corpus, requetes) = tout.split_at(N);

    let mut ids = Vec::with_capacity(N);
    for v in corpus {
        let id = db.create_node(&["Doc"]);
        db.set_node_property(id, "emb", Value::Vector(v.clone().into()));
        ids.push(id.as_u64());
    }
    db.create_vector_index("Doc", "emb", Some(D), Some("cosine"), None, None)
        .expect("create_vector_index");

    let mut r_etroit = 0f32;
    let mut r_defaut = 0f32;
    let mut r_large = 0f32;
    for q in requetes {
        let verite: Vec<u64> = force_brute(corpus, q).into_iter().map(|i| ids[i]).collect();
        let etroit = db
            .vector_search("Doc", "emb", q, K, Some(20), None)
            .expect("ef = 20");
        let defaut = db
            .vector_search("Doc", "emb", q, K, None, None)
            .expect("ef = None");
        let quatre_mille = db
            .vector_search("Doc", "emb", q, K, Some(4_000), None)
            .expect("ef = 4 000");
        let large = db
            .vector_search("Doc", "emb", q, K, Some(16_000), None)
            .expect("ef = 16 000");
        assert_eq!(etroit.len(), K);
        assert_eq!(large.len(), K);
        assert_eq!(
            defaut, quatre_mille,
            "ef = None doit valoir les largeurs par defaut de l'index (4 000 / 400)"
        );
        // Contrat : distances CROISSANTES.
        assert!(
            large.windows(2).all(|w| w[0].1 <= w[1].1),
            "distances non croissantes : {large:?}"
        );
        r_etroit += recall(&verite, &etroit);
        r_defaut += recall(&verite, &defaut);
        r_large += recall(&verite, &large);
    }
    let (r_etroit, r_defaut, r_large) = (
        r_etroit / NQ as f32,
        r_defaut / NQ as f32,
        r_large / NQ as f32,
    );
    println!("recall@10 : ef = 20 -> {r_etroit:.4}, None -> {r_defaut:.4}, 16 000 -> {r_large:.4}");
    assert!(
        r_large >= r_defaut && r_defaut >= r_etroit,
        "elargir ne doit jamais faire perdre du recall ({r_etroit:.4} / {r_defaut:.4} / {r_large:.4})"
    );
    assert!(
        r_etroit < r_large,
        "ef n'agit pas : {r_etroit:.4} == {r_large:.4}"
    );
    assert!(
        r_large >= 0.95,
        "ef = 16 000 sur 3 000 vecteurs 64-d : recall {r_large:.4} < 0,95"
    );
}
