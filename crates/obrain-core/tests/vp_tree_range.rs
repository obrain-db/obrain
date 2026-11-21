//! Équivalence `VpTree::range_search` ≡ balayage exhaustif, sur des coordonnées
//! GÉO-QUANTIFIÉES (3 décimales), où les ex æquo de distance sont la norme.
//!
//! # Pourquoi ce fichier existe
//!
//! La découpe de `VpTree::build_recursive` range à GAUCHE les indices
//! `dists[..median_pos]` et à DROITE `dists[median_pos..]`, avec
//! `radius = dists[median_pos].1`. **En présence d'ex æquo**, l'élément
//! `dists[median_pos - 1]` peut être à la même distance que la médiane : un point
//! de la branche gauche est alors à distance `== mu` du point de vantage, et non
//! `< mu`. Or `range_search_recursive` élague la branche gauche dès
//! `dist - radius >= mu` (inégalité LARGE), ce qui est sûr seulement si gauche
//! implique `< mu`. Conséquence possible : un point situé EXACTEMENT au rayon
//! cherché, rangé à gauche, n'est jamais rendu.
//!
//! Ce défaut est invisible sur des points aléatoires en 64 dimensions (aucun ex
//! æquo), et systématique sur de la donnée géographique quantifiée à 3 décimales
//! (des dizaines d'entités partagent la même coordonnée, et les grilles créent des
//! équidistances en masse). D'où le choix des jeux de données ci-dessous.
//!
//! Ces tests sont le verrou : sur le code d'aujourd'hui ils doivent être soit
//! verts (l'élagage est sûr, ma lecture est fausse), soit rouges (le défaut est
//! réel et se corrige). Aucune troisième issue.

use obrain_core::index::vp_tree::VpTree;

fn euclid(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y) * (x - y))
        .sum::<f32>()
        .sqrt()
}

/// Générateur déterministe (aucun tirage dépendant de l'horloge).
fn lcg(state: &mut u64) -> f32 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    ((*state >> 33) as f32) / ((1u64 << 31) as f32)
}

/// Coordonnées quantifiées à 3 décimales, comme le pont d'appareil les rend
/// (≈ 111 m en latitude, ≈ 73 m en longitude à 49°N) : beaucoup d'ex æquo.
fn grille_quantifiee(n: usize, seed: u64) -> Vec<(u32, Vec<f32>)> {
    let mut s = seed | 1;
    (0..n)
        .map(|i| {
            let lat = 48.80 + (lcg(&mut s) * 0.12 * 1000.0).round() / 1000.0;
            let lon = 2.20 + (lcg(&mut s) * 0.20 * 1000.0).round() / 1000.0;
            (u32::try_from(i).expect("i < n"), vec![lat, lon])
        })
        .collect()
}

fn balayage(points: &[(u32, Vec<f32>)], q: &[f32], r: f32) -> Vec<u32> {
    let mut v: Vec<u32> = points
        .iter()
        .filter(|(_, p)| euclid(q, p) <= r)
        .map(|(id, _)| *id)
        .collect();
    v.sort_unstable();
    v
}

fn rendus(r: Vec<(u32, f32)>) -> Vec<u32> {
    let mut v: Vec<u32> = r.into_iter().map(|(id, _)| id).collect();
    v.sort_unstable();
    v
}

/// Différentiel : sur 8 graines et des rayons choisis PARMI les distances réelles
/// entre points (donc posés exactement sur la frontière, là où l'élagage tranche).
#[test]
fn range_search_equivaut_au_balayage_sur_une_grille_quantifiee() {
    for seed in 1..=8u64 {
        let points = grille_quantifiee(300, seed);
        let tree = VpTree::build(points.clone(), euclid);

        for i in 0..12usize {
            let q = &points[(i * 23 + 7) % points.len()].1;
            // Un rayon nul, un rayon « juste au point » et deux rayons mesurés
            // exactement sur une distance réelle : c'est la frontière qui compte.
            let mut rayons = vec![0.0_f32, 0.0005];
            for j in 0..2 {
                let p = &points[(i * 31 + j * 11 + 3) % points.len()].1;
                rayons.push(euclid(q, p));
            }
            for r in rayons {
                let attendu = balayage(&points, q, r);
                let obtenu = rendus(tree.range_search(q, r, euclid));
                assert_eq!(
                    attendu, obtenu,
                    "graine {seed}, requête {i}, rayon {r} : {} attendus, {} rendus",
                    attendu.len(),
                    obtenu.len()
                );
            }
        }
    }
}

/// Le cas géographique le plus courant : plusieurs entités à la MÊME coordonnée
/// (même adresse, même cellule de quantification). Un rayon nul doit toutes les rendre.
#[test]
fn duplicatas_a_la_meme_position_sont_tous_rendus() {
    let mut points: Vec<(u32, Vec<f32>)> = (0..60u32)
        .map(|i| (i, vec![48.856, 2.352]))
        .collect();
    let mut s = 7u64;
    for i in 60..120u32 {
        points.push((i, vec![48.80 + lcg(&mut s) * 0.1, 2.20 + lcg(&mut s) * 0.2]));
    }
    let tree = VpTree::build(points.clone(), euclid);

    for id in 0..60u32 {
        let q = vec![48.856, 2.352];
        assert_eq!(balayage(&points, &q, 0.0), rendus(tree.range_search(&q, 0.0, euclid)));
        let _ = id;
    }
}

/// Rayon posé exactement sur une distance réelle, série large : si l'élagage perd
/// des points, c'est ici que ça se voit.
#[test]
fn rayon_exactement_sur_une_distance_reelle() {
    let points = grille_quantifiee(500, 99);
    let tree = VpTree::build(points.clone(), euclid);
    let q = &points[17].1;

    for (j, p) in points.iter().enumerate().take(60) {
        let r = euclid(q, &p.1);
        let attendu = balayage(&points, q, r);
        let obtenu = rendus(tree.range_search(q, r, euclid));
        assert_eq!(attendu, obtenu, "rayon exact = distance au point {j}");
    }
}
