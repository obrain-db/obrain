//! Spike « cascade 768 » (T0 du plan O-HNSW, 10/11/2026). BRANCHE DE SPIKE,
//! NON DESTINÉE À LA FUSION.
//!
//! Question : le cascade par paliers (L0 128 bits → L1 512 bits → L2 f16 384)
//! atteint-il recall@10 ≥ 0,95 sur des embeddings 768-d, une fois corrigés ses
//! deux défauts connus — l'entrée TRONQUÉE à 384 (au lieu d'une projection) et
//! l'absence de rerank exact sur les vecteurs pleins — et à quelle latence ?
//!
//! Le test ne modifie rien dans la bibliothèque : il assemble les paliers avec
//! les builders publics de `tiers`, appelle `scan_tiered` et fait le rerank ici.
//!
//! Garde : sans `SPIKE_N` (taille synthétique) ni `SPIKE_DATA` (répertoire des
//! JSON vecbench), le test écrit une ligne et rend Ok — il n'est pas `#[ignore]`,
//! il n'a simplement rien à mesurer.
//!
//! Lancer (release, sinon les latences ne veulent rien dire) :
//!   SPIKE_N=100000 cargo test -p obrain-substrate --release --test spike_cascade_768 -- --nocapture
//!   SPIKE_DATA=/chemin/vecbench/data cargo test -p obrain-substrate --release --test spike_cascade_768 -- --nocapture

use obrain_substrate::tiered_scan::{scan_tiered, ScanConfig, TieredQuery};
use obrain_substrate::tiers::{Tier0, Tier0Builder, Tier1, Tier1Builder, Tier2, Tier2Builder, L2_DIM};
use rayon::prelude::*;
use std::time::Instant;

const DIM: usize = 768;
const K: usize = 10;
const NQ: usize = 100;
const SEED_L0: u64 = 0x305F_4E49_4152_4240;
const SEED_L1: u64 = 0x315F_4E49_4152_4240;

// ───────────────────────── aléatoire déterministe ─────────────────────────

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
        (-2.0 * (u1 as f64).ln()).sqrt() as f32 * (2.0 * std::f32::consts::PI * u2).cos()
    }
}

fn normalize(v: &mut [f32]) {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        for x in v.iter_mut() {
            *x /= n;
        }
    }
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

// ───────────────────────── jeux de données ─────────────────────────

/// Même recette que `agent_memory_migration::recall_sur_vecteurs_structures_2k`
/// (centre + 0,25 × bruit, renormalisé), paramétrée : dimension, N, nombre de
/// centres = max(50, N / 200) pour que la densité de voisins reste réaliste.
fn synthetique(n: usize, seed: u64) -> Vec<f32> {
    let centres_n = (n / 200).max(50);
    let mut rng = SplitMix(seed);
    let mut centres = vec![0f32; centres_n * DIM];
    for c in centres.chunks_mut(DIM) {
        for x in c.iter_mut() {
            *x = rng.gauss();
        }
        normalize(c);
    }
    // Un flux de graines par vecteur : génération parallèle déterministe.
    (0..n)
        .into_par_iter()
        .flat_map_iter(|i| {
            let mut r = SplitMix(seed ^ (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
            let c = &centres[(i % centres_n) * DIM..(i % centres_n + 1) * DIM];
            let mut v: Vec<f32> = (0..DIM).map(|d| c[d] + 0.25 * r.gauss()).collect();
            normalize(&mut v);
            v
        })
        .collect()
}

#[derive(serde::Deserialize)]
struct Dataset {
    dim: usize,
    #[allow(dead_code)]
    ids: Vec<String>,
    embs: Vec<Vec<f32>>,
}

fn charger_vecbench(dir: &str) -> Vec<f32> {
    let mut out = Vec::new();
    for f in ["note_embeddings.json", "file_embeddings.json"] {
        let p = format!("{dir}/{f}");
        let raw = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("lecture {p}: {e}"));
        let ds: Dataset = serde_json::from_str(&raw).expect("JSON vecbench");
        assert_eq!(ds.dim, DIM, "{p}: dim {} attendue {DIM}", ds.dim);
        for mut v in ds.embs {
            assert_eq!(v.len(), DIM);
            normalize(&mut v);
            out.extend_from_slice(&v);
        }
    }
    out
}

// ───────────────────────── projections 768 → 384 ─────────────────────────

/// Témoin : les 384 premières coordonnées, renormalisées (ce que vecbench faisait).
fn tronquer(v: &[f32]) -> Vec<f32> {
    let mut t = v[..L2_DIM].to_vec();
    normalize(&mut t);
    t
}

/// PCA par itération de sous-espace sur un échantillon : moyenne, covariance,
/// puis Q ← orth(C·Q) répété. Rend (moyenne, Q^T aplati 384×768).
struct Pca {
    moyenne: Vec<f32>,
    qt: Vec<f32>, // L2_DIM lignes × DIM colonnes
}

impl Pca {
    fn ajuster(corpus: &[f32], n: usize, seed: u64) -> Pca {
        let ech_n = n.min(20_000);
        let pas = (n / ech_n).max(1);
        let ech: Vec<&[f32]> = (0..ech_n).map(|i| &corpus[i * pas * DIM..(i * pas + 1) * DIM]).collect();
        let mut moyenne = vec![0f32; DIM];
        for v in &ech {
            for d in 0..DIM {
                moyenne[d] += v[d];
            }
        }
        for m in moyenne.iter_mut() {
            *m /= ech_n as f32;
        }
        // Covariance 768×768 (somme parallèle des produits extérieurs).
        let cov: Vec<f32> = ech
            .par_iter()
            .fold(
                || vec![0f32; DIM * DIM],
                |mut acc, v| {
                    let c: Vec<f32> = (0..DIM).map(|d| v[d] - moyenne[d]).collect();
                    for i in 0..DIM {
                        let ci = c[i];
                        let row = &mut acc[i * DIM..(i + 1) * DIM];
                        for j in 0..DIM {
                            row[j] += ci * c[j];
                        }
                    }
                    acc
                },
            )
            .reduce(|| vec![0f32; DIM * DIM], |mut a, b| {
                for (x, y) in a.iter_mut().zip(b) {
                    *x += y;
                }
                a
            });
        // Q : 768 × 384, colonnes orthonormées, itération de sous-espace.
        let mut rng = SplitMix(seed);
        let mut q = vec![0f32; DIM * L2_DIM]; // ligne-major : q[i*L2_DIM + k]
        for x in q.iter_mut() {
            *x = rng.gauss();
        }
        gram_schmidt(&mut q);
        for _ in 0..10 {
            // Z = C · Q
            let qr: &[f32] = &q;
            let z: Vec<f32> = (0..DIM)
                .into_par_iter()
                .flat_map_iter(|i| {
                    let row = &cov[i * DIM..(i + 1) * DIM];
                    (0..L2_DIM).map(move |k| (0..DIM).map(|j| row[j] * qr[j * L2_DIM + k]).sum::<f32>())
                })
                .collect();
            q = z;
            gram_schmidt(&mut q);
        }
        // Q^T : 384 × 768
        let mut qt = vec![0f32; L2_DIM * DIM];
        for i in 0..DIM {
            for k in 0..L2_DIM {
                qt[k * DIM + i] = q[i * L2_DIM + k];
            }
        }
        Pca { moyenne, qt }
    }

    fn projeter(&self, v: &[f32]) -> Vec<f32> {
        let c: Vec<f32> = (0..DIM).map(|d| v[d] - self.moyenne[d]).collect();
        let mut p: Vec<f32> = self.qt.chunks_exact(DIM).map(|row| dot(row, &c)).collect();
        normalize(&mut p);
        p
    }
}

/// Gram-Schmidt modifié sur les colonnes de `q` (DIM lignes × L2_DIM colonnes, ligne-major).
fn gram_schmidt(q: &mut [f32]) {
    for k in 0..L2_DIM {
        for j in 0..k {
            let mut d = 0f32;
            for i in 0..DIM {
                d += q[i * L2_DIM + k] * q[i * L2_DIM + j];
            }
            for i in 0..DIM {
                q[i * L2_DIM + k] -= d * q[i * L2_DIM + j];
            }
        }
        let mut n = 0f32;
        for i in 0..DIM {
            n += q[i * L2_DIM + k] * q[i * L2_DIM + k];
        }
        let n = n.sqrt().max(1e-12);
        for i in 0..DIM {
            q[i * L2_DIM + k] /= n;
        }
    }
}

// ───────────────────────── vérité terrain ─────────────────────────

fn brute_top(corpus: &[f32], n: usize, q: &[f32], exclure: Option<usize>, k: usize) -> Vec<u32> {
    let mut top: Vec<(f32, u32)> = Vec::with_capacity(k + 1);
    for i in 0..n {
        if Some(i) == exclure {
            continue;
        }
        let s = dot(&corpus[i * DIM..(i + 1) * DIM], q);
        if top.len() < k {
            top.push((s, i as u32));
            top.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap().then(a.1.cmp(&b.1)));
        } else if s > top[k - 1].0 {
            top[k - 1] = (s, i as u32);
            top.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap().then(a.1.cmp(&b.1)));
        }
    }
    top.into_iter().map(|(_, i)| i).collect()
}

fn recall(verite: &[Vec<u32>], obtenu: &[Vec<u32>]) -> f64 {
    let mut hits = 0usize;
    for (v, o) in verite.iter().zip(obtenu) {
        hits += v.iter().filter(|x| o.contains(x)).count();
    }
    hits as f64 / (verite.len() * K) as f64
}

// ───────────────────────── cascade ─────────────────────────

struct Paliers {
    l0: Vec<Tier0>,
    l1: Vec<Tier1>,
    l2: Vec<Tier2>,
    b0: Tier0Builder,
    b1: Tier1Builder,
    b2: Tier2Builder,
}

/// `entree_l01` : vecteurs sur lesquels L0/L1 sont projetés (384 projetés ou 768 natifs) ;
/// `entree_l2` : vecteurs 384-d dont L2 prend le f16.
fn construire(entree_l01: &[f32], dim_l01: usize, entree_l2: &[f32], n: usize) -> (Paliers, f64) {
    let t = Instant::now();
    let b0 = Tier0Builder::new(SEED_L0, dim_l01);
    let b1 = Tier1Builder::new(SEED_L1, dim_l01);
    let b2 = Tier2Builder::new();
    let l0: Vec<Tier0> = (0..n).into_par_iter().map(|i| b0.project(&entree_l01[i * dim_l01..(i + 1) * dim_l01])).collect();
    let l1: Vec<Tier1> = (0..n).into_par_iter().map(|i| b1.project(&entree_l01[i * dim_l01..(i + 1) * dim_l01])).collect();
    let l2: Vec<Tier2> = (0..n).into_par_iter().map(|i| b2.project(&entree_l2[i * L2_DIM..(i + 1) * L2_DIM])).collect();
    (Paliers { l0, l1, l2, b0, b1, b2 }, t.elapsed().as_secs_f64())
}

struct Resultat {
    recall: f64,
    p50_ms: f64,
    p95_ms: f64,
}

/// `q_l01` / `q_l2` : transformations de la requête ; `rerank` : sur les vecteurs 768 pleins.
#[allow(clippy::too_many_arguments)]
fn mesurer(
    p: &Paliers,
    corpus768: &[f32],
    n: usize,
    requetes: &[usize],
    verite: &[Vec<u32>],
    q_l01: &dyn Fn(&[f32]) -> Vec<f32>,
    q_l2: &dyn Fn(&[f32]) -> Vec<f32>,
    k0: usize,
    k1: usize,
    rerank: bool,
) -> Resultat {
    let cfg = ScanConfig { k0, k1, ..ScanConfig::default() };
    let mut lat = Vec::with_capacity(requetes.len());
    let mut obtenu = Vec::with_capacity(requetes.len());
    for &qi in requetes {
        let q768 = &corpus768[qi * DIM..(qi + 1) * DIM];
        let t = Instant::now();
        let a = q_l01(q768);
        let b = q_l2(q768);
        let tq = TieredQuery { l0: p.b0.project(&a), l1: p.b1.project(&a), l2: p.b2.project(&b) };
        // On demande k1 + 1 résultats (le +1 absorbe l'auto-correspondance) puis on
        // rerank exactement sur 768 si demandé, sinon on garde l'ordre L2.
        let demande = if rerank { k1 + 1 } else { K + 1 };
        let hits = scan_tiered(&tq, &p.l0, &p.l1, &p.l2, demande.min(n), cfg);
        let mut cand: Vec<(f32, u32)> = hits
            .into_iter()
            .filter(|h| h.node_offset as usize != qi)
            .map(|h| {
                let s = if rerank {
                    dot(&corpus768[h.node_offset as usize * DIM..(h.node_offset as usize + 1) * DIM], q768)
                } else {
                    h.cosine
                };
                (s, h.node_offset)
            })
            .collect();
        cand.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap().then(a.1.cmp(&b.1)));
        cand.truncate(K);
        lat.push(t.elapsed().as_secs_f64() * 1000.0);
        obtenu.push(cand.into_iter().map(|(_, i)| i).collect::<Vec<u32>>());
    }
    lat.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Resultat {
        recall: recall(verite, &obtenu),
        p50_ms: lat[lat.len() / 2],
        p95_ms: lat[(lat.len() * 95 / 100).min(lat.len() - 1)],
    }
}

fn charge() -> String {
    std::process::Command::new("uptime")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.split("load averages:").nth(1).or_else(|| s.split("load average:").nth(1)).map(|x| x.trim().to_string()))
        .unwrap_or_else(|| "?".into())
}

fn campagne(nom: &str, corpus: Vec<f32>, n: usize) {
    println!("\n### {nom} — N = {n}, dim = {DIM}, requêtes = {NQ}, k = {K}, threads rayon = {}, charge = {}", rayon::current_num_threads(), charge());
    let mut rng = SplitMix(0xC0FFEE);
    let requetes: Vec<usize> = (0..NQ).map(|_| (rng.next_u64() as usize) % n).collect();

    // Vérité terrain + contrôle brute/brute.
    let t = Instant::now();
    let verite: Vec<Vec<u32>> = requetes.par_iter().map(|&qi| brute_top(&corpus, n, &corpus[qi * DIM..(qi + 1) * DIM], Some(qi), K)).collect();
    let t_brute = t.elapsed().as_secs_f64() * 1000.0 / NQ as f64;
    let ctrl: Vec<Vec<u32>> = requetes.par_iter().map(|&qi| brute_top(&corpus, n, &corpus[qi * DIM..(qi + 1) * DIM], Some(qi), K)).collect();
    println!("contrôle brute/brute recall@{K} = {:.4} ; exact (rayon sur les requêtes) ≈ {t_brute:.2} ms/requête amorti", recall(&verite, &ctrl));

    // Projections.
    let t = Instant::now();
    let tronc: Vec<f32> = (0..n).into_par_iter().flat_map_iter(|i| tronquer(&corpus[i * DIM..(i + 1) * DIM])).collect();
    println!("troncature 768→384 : {:.1} s", t.elapsed().as_secs_f64());
    let t = Instant::now();
    let pca = Pca::ajuster(&corpus, n, 0x9CA);
    let proj: Vec<f32> = (0..n).into_par_iter().flat_map_iter(|i| pca.projeter(&corpus[i * DIM..(i + 1) * DIM])).collect();
    println!("PCA 768→384 (échantillon {}, 10 itérations) + projection : {:.1} s", n.min(20_000), t.elapsed().as_secs_f64());

    println!("\n| variante | L0/L1 sur | L2 sur | k0 | k1 | rerank 768 | recall@10 | p50 ms | p95 ms | build s |");
    println!("|---|---|---|---|---|---|---|---|---|---|");
    let grille = [(1_000usize, 100usize), (4_000, 400), (16_000, 1_600)];

    // A — témoin : troncature partout, défauts, sans rerank (reproduit vecbench).
    let (pa, ba) = construire(&tronc, L2_DIM, &tronc, n);
    let r = mesurer(&pa, &corpus, n, &requetes, &verite, &|q: &[f32]|tronquer(q), &|q: &[f32]|tronquer(q), 1_000, 100, false);
    println!("| A témoin | tronc 384 | tronc 384 | 1000 | 100 | non | {:.4} | {:.3} | {:.3} | {ba:.1} |", r.recall, r.p50_ms, r.p95_ms);
    drop(pa);

    // B/C — PCA partout, sans puis avec rerank, grille de largeurs.
    let (pb, bb) = construire(&proj, L2_DIM, &proj, n);
    for &(k0, k1) in &grille {
        let r = mesurer(&pb, &corpus, n, &requetes, &verite, &|q: &[f32]|pca.projeter(q), &|q: &[f32]|pca.projeter(q), k0, k1, false);
        println!("| B pca | pca 384 | pca 384 | {k0} | {k1} | non | {:.4} | {:.3} | {:.3} | {bb:.1} |", r.recall, r.p50_ms, r.p95_ms);
        let r = mesurer(&pb, &corpus, n, &requetes, &verite, &|q: &[f32]|pca.projeter(q), &|q: &[f32]|pca.projeter(q), k0, k1, true);
        println!("| C pca+rerank | pca 384 | pca 384 | {k0} | {k1} | oui | {:.4} | {:.3} | {:.3} | {bb:.1} |", r.recall, r.p50_ms, r.p95_ms);
    }
    drop(pb);

    // D — L0/L1 sur les 768 natifs, L2 sur PCA 384, rerank.
    let (pd, bd) = construire(&corpus, DIM, &proj, n);
    for &(k0, k1) in &grille {
        let r = mesurer(&pd, &corpus, n, &requetes, &verite, &|q: &[f32]|q.to_vec(), &|q: &[f32]|pca.projeter(q), k0, k1, true);
        println!("| D natif768+rerank | natif 768 | pca 384 | {k0} | {k1} | oui | {:.4} | {:.3} | {:.3} | {bd:.1} |", r.recall, r.p50_ms, r.p95_ms);
    }
    println!("charge en fin de campagne = {}", charge());
}

#[test]
fn spike_cascade_768() {
    let n_env = std::env::var("SPIKE_N").ok();
    let data_env = std::env::var("SPIKE_DATA").ok();
    if n_env.is_none() && data_env.is_none() {
        println!("spike_cascade_768 : rien à mesurer (ni SPIKE_N ni SPIKE_DATA) — Ok.");
        return;
    }
    if let Some(dir) = data_env {
        let corpus = charger_vecbench(&dir);
        let n = corpus.len() / DIM;
        campagne("vecbench réel (notes + fichiers PO)", corpus, n);
    }
    if let Some(n) = n_env {
        let n: usize = n.parse().expect("SPIKE_N entier");
        let t = Instant::now();
        let corpus = synthetique(n, 0x5EED);
        println!("synthétique {n} × {DIM} généré en {:.1} s", t.elapsed().as_secs_f64());
        campagne("synthétique structuré (centres + 0,25 bruit)", corpus, n);
    }
}
