//! Index spatial : points géographiques `(lon, lat)` et requêtes par rayon ou
//! k plus proches, en MÈTRES.
//!
//! # Pourquoi un VP-Tree, et pourquoi la métrique est la distance elle-même
//!
//! Le banc (`docs/BENCHMARKS.md`, machine dgx, 21/11/2026) a mesuré la structure
//! sur la donnée réelle — des coordonnées quantifiées à trois décimales, donc
//! pleines d'ex æquo : **1,817 µs** pour un rayon sélectif sur 10⁶ points contre
//! **1,625 ms** au balayage (894×). L'arbre ne suppose rien de la distribution
//! et accepte une métrique quelconque, ce qui permet d'y mettre la distance
//! géographique ELLE-MÊME : l'élagage est alors exact, pas une approximation
//! qu'il faudrait rattraper après coup.
//!
//! # Trois étages
//!
//! - `arbre` : construit une fois ([`SpatialIndex::build`]), immuable. C'est la
//!   masse, et sa requête est en microsecondes.
//! - `surcouche` : les points écrits APRÈS la construction (création,
//!   déplacement). Un VP-Tree n'a pas d'insertion ici, et en ajouter une
//!   dégraderait son équilibre ; on empile donc, et on balaie cette petite
//!   liste à la requête.
//! - `tombes` : les identifiants supprimés, filtrés à la requête.
//!
//! [`SpatialIndex::doit_etre_rebati`] dit quand la surcouche a assez grossi
//! pour qu'une reconstruction vaille son coût (648 ms à 10⁶ au banc) : en deçà
//! du dixième de la base, balayer la surcouche coûte quelques microsecondes.

use std::collections::{HashMap, HashSet};

use crate::index::geo::{haversine_m, latitude_valide};
use crate::index::vp_tree::VpTree;

/// Marge de sécurité sur un rayon, en mètres.
///
/// L'arbre calcule ses distances en `f32` (c'est son contrat) alors que la
/// géographie se calcule en `f64`. Sur un rayon de 10⁶ m, la précision
/// relative du `f32` vaut ~1,2·10⁻⁷, soit 0,12 m : un point situé à la
/// limite pourrait être élagué par l'arrondi. On élargit donc la requête de
/// cette marge, puis on RETRANCHE exactement en `f64`. Une marge trop large
/// ne coûte que des candidats en plus, jamais une erreur.
#[must_use]
fn marge_m(rayon_m: f64) -> f64 {
    0.5 + rayon_m.abs() * 1e-6
}

/// La métrique de l'arbre : haversine, convertie en `f32` pour le VP-Tree.
fn haversine_f32(a: &[f32], b: &[f32]) -> f32 {
    // Le contrat de `VpTree` garantit une dimension constante ; on ne suppose
    // pas qu'elle vaut 2 (un appel mal formé doit rendre une distance, pas
    // paniquer).
    if a.len() < 2 || b.len() < 2 {
        return f32::INFINITY;
    }
    haversine_m(a[0] as f64, a[1] as f64, b[0] as f64, b[1] as f64) as f32
}

/// Un index spatial en mémoire : `(node id → (lon, lat))`, interrogeable par
/// rayon et par k plus proches.
pub struct SpatialIndex {
    /// L'arbre, dont la valeur est l'indice COMPACT dans `positions`
    /// (`u32` pour ne pas payer la taille d'un `NodeId` par point).
    arbre: VpTree<u32>,
    /// Indice compact → identifiant du nœud.
    ids: Vec<u32>,
    /// Indice compact → `[lon, lat]` (en `f64` : la vérité, pas le `f32` de
    /// l'arbre).
    positions: Vec<[f64; 2]>,
    /// Identifiant du nœud → indice compact. Sert à reconnaître la réécriture
    /// d'un point déjà dans l'arbre.
    par_id: HashMap<u32, u32>,
    /// Points écrits après la construction ; ils PRIMENT sur l'arbre.
    surcouche: HashMap<u32, [f64; 2]>,
    /// Identifiants supprimés.
    tombes: HashSet<u32>,
}

impl SpatialIndex {
    /// Construit l'index depuis `(identifiant, lon, lat)`.
    ///
    /// Les points de latitude invalide (non finie, hors `[-90, 90]`) sont
    /// ÉCARTÉS : une coordonnée fausse ne doit pas contaminer la métrique de
    /// tout l'arbre (une latitude de 200° rend des cosinus négatifs).
    /// Un identifiant en double garde sa DERNIÈRE position.
    #[must_use]
    pub fn build(points: Vec<(u32, f64, f64)>) -> Self {
        let mut ids: Vec<u32> = Vec::with_capacity(points.len());
        let mut positions: Vec<[f64; 2]> = Vec::with_capacity(points.len());
        let mut par_id: HashMap<u32, u32> = HashMap::with_capacity(points.len());
        for (id, lon, lat) in points {
            if !latitude_valide(lat) || !lon.is_finite() {
                continue;
            }
            if let Some(&compact) = par_id.get(&id) {
                // Réécriture : la dernière position gagne.
                positions[compact as usize] = [lon, lat];
                continue;
            }
            par_id.insert(id, ids.len() as u32);
            ids.push(id);
            positions.push([lon, lat]);
        }
        let bruts: Vec<(u32, Vec<f32>)> = (0..ids.len())
            .map(|c| {
                let p = positions[c];
                (c as u32, vec![p[0] as f32, p[1] as f32])
            })
            .collect();
        let arbre = VpTree::build(bruts, haversine_f32);
        Self {
            arbre,
            ids,
            positions,
            par_id,
            surcouche: HashMap::new(),
            tombes: HashSet::new(),
        }
    }

    /// Nombre de points interrogeables (base vivante + surcouche).
    #[must_use]
    pub fn len(&self) -> usize {
        let base_vivante = self
            .par_id
            .len()
            .saturating_sub(self.tombes.len().min(self.par_id.len()));
        base_vivante + self.surcouche.len()
    }

    /// L'index est-il vide ?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Nombre de points dans l'arbre construit (la masse immuable).
    #[must_use]
    pub fn len_base(&self) -> usize {
        self.ids.len()
    }

    /// Enregistre un point (création ou déplacement).
    ///
    /// Une latitude invalide est IGNORÉE (rend `false`) : l'appelant qui écrit
    /// une donnée fausse doit pouvoir le dire, mais l'index ne se corrompt pas.
    pub fn inserer(&mut self, id: u32, lon: f64, lat: f64) -> bool {
        if !latitude_valide(lat) || !lon.is_finite() {
            return false;
        }
        self.tombes.remove(&id);
        // Le point est dans l'arbre ou pas : dans les deux cas la surcouche
        // le porte et prime, et elle est indexée par identifiant (donc une
        // réécriture remplace, elle n'empile pas).
        self.surcouche.insert(id, [lon, lat]);
        true
    }

    /// Retire un point. Rend `true` s'il était présent.
    pub fn retirer(&mut self, id: u32) -> bool {
        let dans_surcouche = self.surcouche.remove(&id).is_some();
        let dans_base = self.par_id.contains_key(&id);
        if dans_base {
            self.tombes.insert(id);
        }
        dans_surcouche || dans_base
    }

    /// Une reconstruction vaut-elle son coût ?
    ///
    /// Seuil : un dixième de la base, plancher à 1 024 entrées. En deçà, la
    /// surcouche est plus petite que le bruit d'un rebuild (648 ms à 10⁶).
    #[must_use]
    pub fn doit_etre_rebati(&self) -> bool {
        let seuil = 1_024usize.max(self.ids.len() / 10);
        self.surcouche.len() > seuil || self.tombes.len() > seuil
    }

    /// Les points à moins de `rayon_m` mètres, triés par distance croissante,
    /// avec leur distance EXACTE en `f64`.
    ///
    /// Un rayon négatif rend une liste vide (un rayon n'est pas signé).
    #[must_use]
    pub fn dans_rayon(&self, lon: f64, lat: f64, rayon_m: f64) -> Vec<(u32, f64)> {
        if rayon_m < 0.0 || !lon.is_finite() || !latitude_valide(lat) {
            return Vec::new();
        }
        let elargi = rayon_m + marge_m(rayon_m);
        let requete = [lon as f32, lat as f32];
        let mut out: Vec<(u32, f64)> = Vec::new();

        // 1. La masse : l'arbre, élagué sur la métrique exacte.
        for (compact, _) in self
            .arbre
            .range_search(&requete, elargi as f32, haversine_f32)
        {
            let Some(&id) = self.ids.get(compact as usize) else {
                continue;
            };
            // Masqué par une réécriture, ou supprimé.
            if self.surcouche.contains_key(&id) || self.tombes.contains(&id) {
                continue;
            }
            let p = self.positions[compact as usize];
            let d = haversine_m(lon, lat, p[0], p[1]);
            if d <= rayon_m {
                out.push((id, d));
            }
        }

        // 2. La surcouche : petite, on la balaie exactement.
        for (&id, p) in &self.surcouche {
            let d = haversine_m(lon, lat, p[0], p[1]);
            if d <= rayon_m {
                out.push((id, d));
            }
        }

        out.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        out
    }

    /// Les `k` points les plus proches, triés par distance croissante.
    ///
    /// `k == 0` rend une liste vide.
    #[must_use]
    pub fn plus_proches(&self, lon: f64, lat: f64, k: usize) -> Vec<(u32, f64)> {
        if k == 0 || !lon.is_finite() || !latitude_valide(lat) {
            return Vec::new();
        }
        let requete = [lon as f32, lat as f32];
        let mut candidats: Vec<(u32, f64)> = Vec::new();

        // On demande un peu plus que k à l'arbre : une partie de ce qu'il rend
        // peut être masquée par la surcouche ou par une tombe.
        let demande = k.saturating_add(self.surcouche.len()).saturating_add(8);
        for (compact, _) in self.arbre.knn(&requete, demande, haversine_f32) {
            let Some(&id) = self.ids.get(compact as usize) else {
                continue;
            };
            if self.surcouche.contains_key(&id) || self.tombes.contains(&id) {
                continue;
            }
            let p = self.positions[compact as usize];
            candidats.push((id, haversine_m(lon, lat, p[0], p[1])));
        }
        for (&id, p) in &self.surcouche {
            candidats.push((id, haversine_m(lon, lat, p[0], p[1])));
        }

        candidats.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        candidats.truncate(k);
        candidats
    }

    /// Vérifie l'invariant « un identifiant n'est jamais à la fois enterré et
    /// vivant » et rend le nombre d'anomalies. Sert aux tests et au diagnostic.
    #[must_use]
    pub fn anomalies(&self) -> usize {
        self.tombes
            .iter()
            .filter(|id| self.surcouche.contains_key(id))
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn points_grille(n: u32, pas: f64) -> Vec<(u32, f64, f64)> {
        (0..n)
            .map(|i| (i, 2.0 + f64::from(i) * pas, 48.0 + f64::from(i) * pas))
            .collect()
    }

    #[test]
    fn rayon_zero_ne_rend_que_les_points_exactement_la() {
        let idx = SpatialIndex::build(vec![(1, 2.35, 48.85), (2, 2.35, 48.85), (3, 2.36, 48.85)]);
        let mut ids: Vec<u32> = idx
            .dans_rayon(2.35, 48.85, 0.0)
            .iter()
            .map(|(i, _)| *i)
            .collect();
        ids.sort_unstable();
        assert_eq!(ids, vec![1, 2], "deux points confondus, tous deux rendus");
    }

    #[test]
    fn le_rayon_est_inclusif() {
        // 0,001° de longitude à 48,85° ≈ 73 m : on pose le rayon EXACTEMENT
        // sur cette distance, mesurée par la même fonction.
        let d = haversine_m(2.35, 48.85, 2.351, 48.85);
        let idx = SpatialIndex::build(vec![(1, 2.35, 48.85), (2, 2.351, 48.85)]);
        let r = idx.dans_rayon(2.35, 48.85, d);
        assert_eq!(
            r.len(),
            2,
            "un point à exactement le rayon est dedans : {r:?}"
        );
    }

    #[test]
    fn surcouche_et_tombes_masquent_la_base() {
        let mut idx = SpatialIndex::build(vec![(1, 2.35, 48.85), (2, 2.36, 48.85)]);
        idx.inserer(1, 2.40, 48.90); // déplacement
        idx.retirer(2); // suppression
        idx.inserer(3, 2.3501, 48.8501); // création
        // La position D'ORIGINE de 1 (2.35, 48.85) est à 0 m de la requête ;
        // sa nouvelle position est à ~3,6 km. Si la surcouche ne masquait pas
        // la base, 1 sortirait avec une distance de 0 m.
        let r = idx.dans_rayon(2.35, 48.85, 1_000.0);
        let ids: Vec<u32> = r.iter().map(|(i, _)| *i).collect();
        assert_eq!(ids, vec![3], "la base masquée ne rend pas 1 : {r:?}");
        assert!(!ids.contains(&2), "le 2 a été retiré");
        // Un rayon assez large retrouve 1, à sa NOUVELLE distance.
        let large = idx.dans_rayon(2.35, 48.85, 10_000.0);
        let d1 = large.iter().find(|(i, _)| *i == 1).map(|(_, d)| *d);
        assert!(
            d1.is_some_and(|d| d > 3_000.0),
            "1 à sa nouvelle position : {large:?}"
        );
        assert_eq!(idx.anomalies(), 0);
    }

    #[test]
    fn plus_proches_ordonne_et_respecte_la_surcouche() {
        let mut idx = SpatialIndex::build(points_grille(50, 0.001));
        idx.inserer(999, 2.0005, 48.0005);
        // Requête SUR le point de surcouche : il doit sortir premier, devant
        // le point 0 de la base qui est à ~55 m.
        let r = idx.plus_proches(2.0005, 48.0005, 3);
        assert_eq!(r[0].0, 999, "le point de la surcouche est le plus proche");
        assert!(r.windows(2).all(|w| w[0].1 <= w[1].1), "trié");
        assert_eq!(r.len(), 3);
    }

    #[test]
    fn latitude_invalide_est_ignoree_pas_fatale() {
        let idx = SpatialIndex::build(vec![(1, 2.35, 200.0), (2, 2.35, 48.85)]);
        assert_eq!(idx.len(), 1);
        assert_eq!(idx.dans_rayon(2.35, 48.85, 1.0).len(), 1);
    }

    #[test]
    fn equivalence_au_balayage_sur_une_grille_quantifiee() {
        // Le vrai cas : des coordonnées arrondies à 3 décimales, donc des
        // ex æquo partout. L'index doit rendre exactement le balayage.
        let mut points: Vec<(u32, f64, f64)> = Vec::new();
        for i in 0..40u32 {
            for j in 0..25u32 {
                let lon = (2_000.0 + f64::from(i)) / 1_000.0;
                let lat = (48_000.0 + f64::from(j)) / 1_000.0;
                points.push((i * 25 + j, lon, lat));
            }
        }
        let idx = SpatialIndex::build(points.clone());
        for (i, rayon) in [0.0_f64, 150.0, 700.0, 5_000.0].iter().enumerate() {
            let lon = 2.012 + i as f64 * 0.001;
            let lat = 48.010;
            let mut attendu: Vec<(u32, f64)> = points
                .iter()
                .map(|&(id, plon, plat)| (id, haversine_m(lon, lat, plon, plat)))
                .filter(|(_, d)| *d <= *rayon)
                .collect();
            attendu.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
            let obtenu = idx.dans_rayon(lon, lat, *rayon);
            assert_eq!(
                obtenu.len(),
                attendu.len(),
                "rayon {rayon} : {} attendus, {} rendus",
                attendu.len(),
                obtenu.len()
            );
            // L'ordre entre distances ÉGALES n'est pas un contrat (le tri est
            // stable, mais l'ordre d'insertion vient de la traversée) : on
            // compare donc les ensembles, et la monotonie séparément.
            let mut ids_a: Vec<u32> = attendu.iter().map(|(i, _)| *i).collect();
            let mut ids_o: Vec<u32> = obtenu.iter().map(|(i, _)| *i).collect();
            ids_a.sort_unstable();
            ids_o.sort_unstable();
            assert_eq!(ids_o, ids_a, "rayon {rayon} : ensembles différents");
            assert!(
                obtenu.windows(2).all(|w| w[0].1 <= w[1].1),
                "rayon {rayon} : distances non monotones"
            );
        }
    }

    #[test]
    fn doit_etre_rebati_apres_trop_de_surcouche() {
        let mut idx = SpatialIndex::build(points_grille(20_000, 0.0001));
        assert!(!idx.doit_etre_rebati());
        for i in 0..2_500u32 {
            idx.inserer(100_000 + i, 2.5, 48.9);
        }
        assert!(
            idx.doit_etre_rebati(),
            "2 500 sur 20 000 : au-delà du dixième"
        );
    }
}
