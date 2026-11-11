//! ── L'INDEX VECTORIEL SURVIT À LA RÉOUVERTURE (O-VEC C3) ────────────────────
//!
//! Une base fermée puis rouverte sans écriture retrouve son index vectoriel
//! depuis ses zones nommées (`substrate.tier{0,1,2}.<label>.<prop>`) : zéro
//! reconstruction, mêmes résultats. Toute incohérence (écriture depuis, zone
//! corrompue ou absente, écriture qui a contourné le moteur) fait
//! RECONSTRUIRE — jamais lire un index périmé.
//!
//! Vecteurs 64-d structurés (recette du dépôt), base substrat sur répertoire
//! temporaire ; le chemin « projection apprise » de bout en bout est l'objet de
//! C4 (le moteur refuse encore dim > 384 à `create_vector_index`).

#![cfg(feature = "vector-index")]

use std::path::Path;

use obrain_common::types::{NodeId, Value};
use obrain_engine::ObrainDB;

const D: usize = 64;
const N: usize = 500;
const NQ: usize = 40;
const K: usize = 10;
const CENTRES: usize = 25;

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

fn ouvrir(dir: &Path) -> ObrainDB {
    ObrainDB::open_substrate(dir).expect("ouverture de la base substrat")
}

fn ecrire(db: &ObrainDB, corpus: &[Vec<f32>]) -> Vec<NodeId> {
    corpus
        .iter()
        .map(|v| {
            let id = db.create_node(&["Doc"]);
            db.set_node_property(id, "emb", Value::Vector(v.clone().into()));
            id
        })
        .collect()
}

fn chercher(db: &ObrainDB, requetes: &[Vec<f32>]) -> Vec<Vec<(u64, f32)>> {
    requetes
        .iter()
        .map(|q| {
            db.vector_search("Doc", "emb", q, K, None, None)
                .expect("vector_search")
                .into_iter()
                .map(|(id, d)| (id.as_u64(), d))
                .collect()
        })
        .collect()
}

fn zone(dir: &Path, niveau: u8) -> std::path::PathBuf {
    dir.join(format!("substrate.tier{niveau}.Doc.emb"))
}

/// Base peuplée, index créé, fermée proprement. Rend les résultats de
/// référence sur `requetes`.
fn base_de_depart(dir: &Path, corpus: &[Vec<f32>], requetes: &[Vec<f32>]) -> Vec<Vec<(u64, f32)>> {
    let db = ouvrir(dir);
    ecrire(&db, corpus);
    db.create_vector_index("Doc", "emb", Some(D), Some("cosine"), None, None)
        .expect("create_vector_index");
    assert_eq!(
        db.nb_index_vectoriel_persistes(),
        1,
        "l'index est persiste des sa creation"
    );
    let reference = chercher(&db, requetes);
    db.close().expect("close");
    for niveau in 0..3 {
        assert!(
            zone(dir, niveau).is_file(),
            "zone {} absente apres fermeture",
            zone(dir, niveau).display()
        );
    }
    assert!(
        !dir.join("substrate.vecproj.Doc.emb").exists(),
        "pas de zone de projection pour une dimension <= 384 (identite)"
    );
    reference
}

#[test]
fn reouverture_sans_ecriture_charge_l_index_sans_le_reconstruire() {
    let td = tempfile::tempdir().expect("tempdir");
    let dir = td.path().join("base");
    let tout = structures(N + NQ, 3);
    let (corpus, requetes) = tout.split_at(N);
    let reference = base_de_depart(&dir, corpus, requetes);

    let db = ouvrir(&dir);
    assert_eq!(
        db.nb_index_vectoriel_charges(),
        0,
        "rien n'est charge avant la premiere recherche"
    );
    let obtenu = chercher(&db, requetes);
    assert_eq!(
        obtenu, reference,
        "memes top-10 (identifiants ET scores) qu'avant la fermeture"
    );
    assert_eq!(
        db.nb_index_vectoriel_charges(),
        1,
        "l'index vient de ses zones"
    );
    assert_eq!(
        db.nb_index_vectoriel_reconstruits(),
        0,
        "aucune reconstruction"
    );
    // Une seconde recherche ne recharge rien : l'index est au registre.
    let _ = chercher(&db, &requetes[..1]);
    assert_eq!(db.nb_index_vectoriel_charges(), 1);
}

#[test]
fn des_ecritures_par_l_api_rendent_l_index_a_jour_et_persistable() {
    let td = tempfile::tempdir().expect("tempdir");
    let dir = td.path().join("base");
    let tout = structures(N + NQ + 40, 5);
    let (corpus, reste) = tout.split_at(N);
    let (requetes, nouveaux) = reste.split_at(NQ);
    base_de_depart(&dir, corpus, requetes);

    // Réouverture : des écritures AVANT toute recherche (le registre est vide,
    // l'index persisté ne les a pas vues) ⇒ l'empreinte ne colle plus ⇒
    // reconstruction à la première recherche, qui voit les nouveaux noeuds.
    let ids_avant: Vec<NodeId>;
    {
        let db = ouvrir(&dir);
        ids_avant = ecrire(&db, &nouveaux[..30]);
        let hits = db
            .vector_search("Doc", "emb", &nouveaux[0], 1, None, None)
            .unwrap();
        assert_eq!(
            hits[0].0, ids_avant[0],
            "le noeud ecrit avant la recherche est retrouve"
        );
        assert_eq!(db.nb_index_vectoriel_charges(), 0);
        assert_eq!(db.nb_index_vectoriel_reconstruits(), 1);
        // Des écritures APRÈS : l'index en mémoire est entretenu par l'API.
        let ids_apres = ecrire(&db, &nouveaux[30..]);
        let hits = db
            .vector_search("Doc", "emb", &nouveaux[35], 1, None, None)
            .unwrap();
        assert_eq!(hits[0].0, ids_apres[5]);
        db.close().expect("close");
        // Toutes les écritures sont passées par le moteur ⇒ persisté à la
        // fermeture (persistances : reconstruction + fermeture).
    }
    {
        let db = ouvrir(&dir);
        let hits = db
            .vector_search("Doc", "emb", &nouveaux[35], 1, None, None)
            .unwrap();
        assert_eq!(
            hits[0].0.as_u64(),
            ids_avant[0].as_u64() + 35,
            "le dernier ecrit est retrouve"
        );
        assert_eq!(
            db.nb_index_vectoriel_charges(),
            1,
            "charge, puisque l'index avait tout vu"
        );
        assert_eq!(db.nb_index_vectoriel_reconstruits(), 0);
    }
}

#[test]
fn une_ecriture_gql_est_vue_par_l_index_puis_persistee_et_rechargee() {
    let td = tempfile::tempdir().expect("tempdir");
    let dir = td.path().join("base");
    let tout = structures(N + NQ, 7);
    let (corpus, requetes) = tout.split_at(N);
    base_de_depart(&dir, corpus, requetes);

    // GQL `SET` n'emprunte pas l'API du moteur (défaut corrigé par T5) : c'est
    // l'observateur du store qui entretient l'index. Les deux compteurs
    // coïncident, l'index est persisté À JOUR à la fermeture et rechargé tel
    // quel — jamais périmé, jamais reconstruit pour rien.
    let cible: Vec<f32> = {
        let mut v = vec![0f32; D];
        v[0] = 1.0;
        v
    };
    let litteral = cible
        .iter()
        .map(|x| format!("{x:.1}"))
        .collect::<Vec<_>>()
        .join(", ");
    {
        let db = ouvrir(&dir);
        let _ = chercher(&db, &requetes[..1]);
        assert_eq!(db.nb_index_vectoriel_charges(), 1);
        db.execute(&format!("MATCH (n:Doc) SET n.emb = vector([{litteral}])"))
            .expect("SET vectoriel en GQL");
        db.close().expect("close");
    }
    {
        let db = ouvrir(&dir);
        let hits = db
            .vector_search("Doc", "emb", &cible, K, None, None)
            .unwrap();
        assert_eq!(
            db.nb_index_vectoriel_charges(),
            1,
            "l'index a vu l'ecriture GQL : persiste a jour, recharge"
        );
        assert_eq!(
            db.nb_index_vectoriel_reconstruits(),
            0,
            "aucune reconstruction"
        );
        assert_eq!(hits.len(), K);
        for (_, distance) in &hits {
            assert!(
                *distance < 1e-3,
                "tous les vecteurs valent la cible : distance attendue ~0, obtenu {distance}"
            );
        }
    }
}

#[test]
fn une_zone_corrompue_fait_reconstruire_avec_les_memes_resultats() {
    let td = tempfile::tempdir().expect("tempdir");
    let dir = td.path().join("base");
    let tout = structures(N + NQ, 9);
    let (corpus, requetes) = tout.split_at(N);
    let reference = base_de_depart(&dir, corpus, requetes);

    // Un octet du payload de tier0 bascule : CRC faux.
    {
        let chemin = zone(&dir, 0);
        let mut octets = std::fs::read(&chemin).unwrap();
        octets[64 + 8] ^= 0xFF;
        std::fs::write(&chemin, octets).unwrap();
    }
    let db = ouvrir(&dir);
    let obtenu = chercher(&db, requetes);
    assert_eq!(
        obtenu, reference,
        "reconstruit depuis la colonne : memes resultats"
    );
    assert_eq!(db.nb_index_vectoriel_charges(), 0);
    assert_eq!(db.nb_index_vectoriel_reconstruits(), 1);
    assert!(
        db.nb_index_vectoriel_persistes() >= 1,
        "reecrit apres reconstruction"
    );
    db.close().unwrap();

    // Et la réouverture suivante charge ce qui vient d'être réécrit.
    let db = ouvrir(&dir);
    assert_eq!(chercher(&db, requetes), reference);
    assert_eq!(db.nb_index_vectoriel_charges(), 1);
    assert_eq!(db.nb_index_vectoriel_reconstruits(), 0);
}

#[test]
fn une_zone_absente_fait_reconstruire() {
    let td = tempfile::tempdir().expect("tempdir");
    let dir = td.path().join("base");
    let tout = structures(N + NQ, 13);
    let (corpus, requetes) = tout.split_at(N);
    let reference = base_de_depart(&dir, corpus, requetes);
    std::fs::remove_file(zone(&dir, 1)).unwrap();

    let db = ouvrir(&dir);
    assert_eq!(chercher(&db, requetes), reference);
    assert_eq!(db.nb_index_vectoriel_charges(), 0);
    assert_eq!(db.nb_index_vectoriel_reconstruits(), 1);
}

#[test]
fn drop_vector_index_retire_les_zones() {
    let td = tempfile::tempdir().expect("tempdir");
    let dir = td.path().join("base");
    let tout = structures(60, 17);
    let db = ouvrir(&dir);
    ecrire(&db, &tout);
    db.create_vector_index("Doc", "emb", Some(D), Some("cosine"), None, None)
        .unwrap();
    assert!(zone(&dir, 0).is_file());
    assert!(db.drop_vector_index("Doc", "emb"));
    for niveau in 0..3 {
        assert!(
            !zone(&dir, niveau).exists(),
            "zone {niveau} encore presente apres drop"
        );
    }
    db.close().unwrap();
    let db = ouvrir(&dir);
    assert!(
        db.vector_search("Doc", "emb", &tout[0], 1, None, None)
            .is_err(),
        "plus d'index : la recherche le dit"
    );
}

/// Mesure (pas un verdict) : temps de CHARGEMENT depuis les zones contre
/// temps de RECONSTRUCTION depuis la colonne, pour `OBRAIN_MESURE_REOUVERTURE`
/// vecteurs 64-d. Sans la variable, le test écrit une ligne et rend Ok —
/// jamais `#[ignore]`. À lire en `--release --nocapture`, machine calme.
#[test]
fn mesure_chargement_contre_reconstruction() {
    let Some(n) = std::env::var("OBRAIN_MESURE_REOUVERTURE")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
    else {
        println!(
            "mesure_chargement_contre_reconstruction : OBRAIN_MESURE_REOUVERTURE absent — rien à mesurer."
        );
        return;
    };
    let td = tempfile::tempdir().expect("tempdir");
    let dir = td.path().join("base");
    let corpus = structures(n, 21);
    let q = corpus[0].clone();
    {
        let db = ouvrir(&dir);
        let debut = std::time::Instant::now();
        ecrire(&db, &corpus);
        let t_ecriture = debut.elapsed();
        let debut = std::time::Instant::now();
        db.create_vector_index("Doc", "emb", Some(D), Some("cosine"), None, None)
            .unwrap();
        let t_construction = debut.elapsed();
        println!(
            "N = {n} : ecriture {t_ecriture:?}, construction + persistance {t_construction:?}"
        );
        db.close().unwrap();
    }
    // Chargement.
    let t_chargement = {
        let db = ouvrir(&dir);
        let debut = std::time::Instant::now();
        let _ = db.vector_search("Doc", "emb", &q, K, None, None).unwrap();
        let t = debut.elapsed();
        assert_eq!(db.nb_index_vectoriel_charges(), 1);
        db.close().unwrap();
        t
    };
    // Reconstruction (zone tier1 retirée).
    std::fs::remove_file(zone(&dir, 1)).unwrap();
    let t_reconstruction = {
        let db = ouvrir(&dir);
        let debut = std::time::Instant::now();
        let _ = db.vector_search("Doc", "emb", &q, K, None, None).unwrap();
        let t = debut.elapsed();
        assert_eq!(db.nb_index_vectoriel_reconstruits(), 1);
        t
    };
    println!(
        "N = {n} : chargement {t_chargement:?} contre reconstruction {t_reconstruction:?} (rapport {:.1}x)",
        t_reconstruction.as_secs_f64() / t_chargement.as_secs_f64().max(1e-9)
    );
}
