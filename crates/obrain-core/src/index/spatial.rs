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
//! géographique elle-même. L'arbre travaille en `f32` : son élagage est
//! rendu CONSERVATIF par une borne dérivée (voir « Borne d'élagage »), puis
//! chaque candidat est filtré en `f64` par la fonction du balayage. Le
//! résultat est celui du balayage, au bit près — SOUS l'hypothèse libm
//! énoncée dans « Borne d'élagage », vérifiée sur des cas mesurés, pas
//! démontrée.
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
//! - `hors_domaine` : les points construits hors de [-180, 180] × [-90, 90],
//!   où l'erreur d'arrondi `f32` n'est plus bornée. Le balayage GQL les voit ;
//!   l'index les balaie donc exactement au lieu de les écarter.
//! - `non_points` : les identifiants dont la valeur n'est pas un point. Le
//!   balayage GQL REFUSE la requête dessus ; l'index les compte pour que le
//!   planificateur se replie sur le balayage (parité des erreurs).
//!
//! [`SpatialIndex::doit_etre_rebati`] dit quand la surcouche a assez grossi
//! pour qu'une reconstruction vaille son coût (648 ms à 10⁶ au banc) : en deçà
//! du dixième de la base, balayer la surcouche coûte quelques microsecondes.

use std::collections::{HashMap, HashSet};

use crate::index::geo::{RAYON_TERRE_M, haversine_m, latitude_valide};
use crate::index::vp_tree::VpTree;

// ── BORNE D'ÉLAGAGE (revue de #282, 21/11/2026) ───────────────────────────
//
// AVANT : une marge de `0,5 + 10⁻⁶·r` mètres, justifiée par la seule
// précision RELATIVE du `f32` sur les distances. Elle était fausse : l'arbre
// arrondit aussi les COORDONNÉES en `f32`, et cette erreur-là dépend de la
// grandeur de la coordonnée, pas du rayon. Près de ±180° le demi-pas d'un
// `f32` vaut 7,6·10⁻⁶° (≈ 0,85 m à l'équateur) : la revue a montré un point à
// 0,033 m omis d'un rayon de 0,1 m. Une marge supposée n'est pas une borne.
//
// Ce qui suit est une borne DÉRIVÉE. Notations : `h` la fonction de vérité
// (`haversine_m` en `f64`, celle du balayage GQL), `m` la distance sphérique
// exacte, `m̃` la métrique de l'arbre (`haversine_f32` : `h` sur les
// coordonnées arrondies en `f32`, puis rendue en `f32`), `q'`, `p'` les points
// arrondis, `D` la demi-circonférence, `u` l'arrondi unitaire du `f32`.
//
// 1. `E` majore `|h − m|` (voir `erreur_haversine_m`).
// 2. `δ` majore `m(p, p')` pour un point du domaine (`deplacement_f32_m`).
// 3. `ε = E + u·D` majore `|m̃ − m|` sur les points arrondis.
// 4. Élagage : la partition du `build` met à gauche `m̃(vp,p) ≤ μ`, à droite
//    `m̃(vp,p) ≥ μ`. Un sous-arbre n'est sauté que si `fl(dist − r_f) > μ`
//    (gauche) ou `fl(dist + r_f) < μ` (droite). L'inégalité triangulaire sur
//    `m` donne alors, pour tout `p` sauté : `m̃(q',p) > r_f − u·(D + r_f) − 3ε`.
// 5. Pour un point à `h(q,p) ≤ r` : `m̃(q',p') ≤ r + E + 2δ + ε`. Il n'est
//    donc jamais sauté dès que `r_f·(1 − u) ≥ r + E + 2δ + 4ε + u·D`.
// 6. `r_f = fl32(r')` vérifie `r_f ≥ r'·(1 − u)` : on prend
//    `r' = (r + E + 2δ + 4ε + u·D) / (1 − u)²`.
//
// Chaque candidat rendu est ensuite filtré par `h` EXACTEMENT : la marge ne
// coûte que des candidats en plus, jamais un faux positif. Valeur numérique :
// ≈ 17 m d'anneau supplémentaire, quel que soit le rayon (voir le test
// `la_borne_coute_moins_de_vingt_metres`).
//
// HYPOTHÈSE, la seule qui ne soit pas garantie par IEEE 754 : `sin`, `cos`
// et `asin` de la libm de la plateforme sont exactes à 1 ulp près (`sqrt` et
// les quatre opérations sont correctement arrondies par la norme).
//
// Ce qu'on en sait, et ses limites :
// - `erreur_haversine_bornee_contre_atan2` la VÉRIFIE sur 200 000 cas
//   adverses (écart max observé 0,217 m, borne 1,687 m). Des cas mesurés
//   valident ces cas-là ; ils ne démontrent pas l'hypothèse.
// - La libm dépend de la cible : macOS, glibc, musl et le port Rust de
//   wasm32 n'ont pas la même implémentation. La vérification ne vaut que là
//   où ce test tourne.
// - Le « ≈ 32 arrondis » de `a` (majoré à 64) est un décompte d'opérations
//   fait à la main, pas une preuve vérifiée par machine.
// Si l'hypothèse tombe, l'index peut à nouveau différer du balayage près de
// la frontière du rayon ; la marge de 7,8× entre l'observé et la borne est
// ce qui protège, pas une garantie.

/// Arrondi unitaire du `f32` : 2⁻²⁴.
const U_F32: f64 = 1.0 / 16_777_216.0;
/// Arrondi unitaire du `f64` : 2⁻⁵³.
const U_F64: f64 = 1.0 / 9_007_199_254_740_992.0;

/// La plus grande distance qu'une haversine puisse rendre, majorée d'un
/// millionième : `π·R` à l'arrondi près.
fn distance_max_m() -> f64 {
    std::f64::consts::PI * RAYON_TERRE_M * (1.0 + 1e-6)
}

/// Le domaine de l'arbre : là où le déplacement `f32` est borné par
/// [`deplacement_f32_m`]. Hors de lui, un point est balayé exactement.
fn dans_le_domaine(lon: f64, lat: f64) -> bool {
    lon.is_finite() && (-180.0..=180.0).contains(&lon) && latitude_valide(lat)
}

/// Déplacement maximal, en mètres, d'un point du domaine arrondi en `f32`.
///
/// Demi-pas du `f32` : 2⁻¹⁷ degré pour `|lon| ≤ 180` (binade [128, 256)),
/// 2⁻¹⁸ degré pour `|lat| ≤ 90` (binade [64, 128)). Le grand cercle est plus
/// court que le chemin « méridien puis parallèle », donc
/// `δ ≤ R·(Δφ + Δλ)` en radians.
fn deplacement_f32_m() -> f64 {
    RAYON_TERRE_M * (2f64.powi(-17) + 2f64.powi(-18)).to_radians()
}

/// Majorant de `|h − m|`, l'erreur absolue de [`haversine_m`] calculée.
///
/// `a` est une somme de produits de facteurs ≤ 1 : son erreur absolue est
/// d'environ 32 arrondis `f64` (sous l'hypothèse libm), majorée ici à 64.
/// `θ = asin(√a)` est ½-hölderienne : `a = sin²θ` et, pour θ₁ < θ₂ dans
/// [0, π/2], `a₂ − a₁ = sin(θ₂−θ₁)·sin(θ₂+θ₁) ≥ sin²(θ₂−θ₁) ≥ (2(θ₂−θ₁)/π)²`,
/// donc `Δθ ≤ (π/2)·√Δa` et `Δc = 2Δθ ≤ π·√Δa`. Près des antipodes c'est ce
/// terme qui domine (≈ 1,7 m) ; ailleurs l'erreur réelle est bien plus petite.
fn erreur_haversine_m() -> f64 {
    let pi = std::f64::consts::PI;
    let erreur_a = 64.0 * U_F64;
    let erreur_angle = pi * erreur_a.sqrt() + 8.0 * U_F64 * pi;
    RAYON_TERRE_M * erreur_angle + 4.0 * U_F64 * distance_max_m()
}

/// Le rayon à donner à l'arbre pour que TOUT point à `h ≤ rayon_m` lui
/// survive (étapes 4 à 6 de la borne ci-dessus).
fn rayon_arbre(rayon_m: f64) -> f32 {
    let d = distance_max_m();
    let e = erreur_haversine_m();
    let eps = e + U_F32 * d;
    let r = rayon_m + e + 2.0 * deplacement_f32_m() + 4.0 * eps + U_F32 * d;
    let r_prime = r / ((1.0 - U_F32) * (1.0 - U_F32)) * (1.0 + 8.0 * U_F64);
    r_prime as f32
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

/// Ordre de tous les résultats : distance croissante, puis identifiant.
/// Les distances sont finies (haversine borne `a` dans [0, 1]) ; l'ordre
/// total ne sert qu'à ne jamais dépendre de la traversée pour les ex æquo.
fn trier(v: &mut [(u32, f64)]) {
    v.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
}

/// Un index spatial en mémoire : `(node id → (lon, lat))`, interrogeable par
/// rayon et par k plus proches.
///
/// Contrat, SOUS l'hypothèse libm de « Borne d'élagage » :
/// [`SpatialIndex::dans_rayon`] rend les points que rendrait un balayage par
/// [`haversine_m`] (`h ≤ rayon`), et [`SpatialIndex::plus_proches`] les `k`
/// premiers dans l'ordre (distance, identifiant). Les points dont la
/// distance est NaN ne sont jamais rendus.
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
    /// Points construits HORS du domaine de l'arbre (longitude hors
    /// [-180, 180], latitude hors [-90, 90]) : le balayage GQL les voit, donc
    /// l'index aussi — balayés exactement, jamais arrondis.
    hors_domaine: HashMap<u32, [f64; 2]>,
    /// Points écrits après la construction ; ils PRIMENT sur l'arbre et sur
    /// `hors_domaine`.
    surcouche: HashMap<u32, [f64; 2]>,
    /// Identifiants supprimés de l'arbre (toujours un sous-ensemble de
    /// `par_id`, jamais dans `surcouche`).
    tombes: HashSet<u32>,
    /// Identifiants dont la propriété porte une valeur NON NULLE qui n'est
    /// PAS un point (`"oops"`, une liste de trois nombres…). Le balayage GQL
    /// REFUSE la requête sur une telle valeur (`distance() attend deux
    /// points`) ; l'index ne peut pas la refuser avec le même texte, il la
    /// COMPTE pour que le planificateur se replie sur le balayage tant qu'il
    /// en reste. Jamais visible : marquer retire le point.
    non_points: HashSet<u32>,
    /// Nombre de points visibles, tenu à jour à chaque écriture : `len()` en
    /// O(1). Chaque écriture compare la visibilité de l'identifiant avant et
    /// après — un déplacement ne compte jamais deux fois.
    vivants: usize,
}

impl SpatialIndex {
    /// Construit l'index depuis `(identifiant, lon, lat)`.
    ///
    /// Une coordonnée NON FINIE écarte le point : le balayage ne peut pas le
    /// rendre non plus (sa distance n'est jamais `≤ r`). Une coordonnée finie
    /// hors du domaine de l'arbre va dans `hors_domaine`. Un identifiant en
    /// double garde sa DERNIÈRE position, et une dernière position non finie
    /// le retire.
    #[must_use]
    pub fn build(points: Vec<(u32, f64, f64)>) -> Self {
        let mut ordre: Vec<u32> = Vec::with_capacity(points.len());
        let mut vus: HashSet<u32> = HashSet::with_capacity(points.len());
        let mut derniere: HashMap<u32, [f64; 2]> = HashMap::with_capacity(points.len());
        for (id, lon, lat) in points {
            if !lon.is_finite() || !lat.is_finite() {
                derniere.remove(&id);
                continue;
            }
            derniere.insert(id, [lon, lat]);
            if vus.insert(id) {
                ordre.push(id);
            }
        }
        let mut ids: Vec<u32> = Vec::with_capacity(ordre.len());
        let mut positions: Vec<[f64; 2]> = Vec::with_capacity(ordre.len());
        let mut par_id: HashMap<u32, u32> = HashMap::with_capacity(ordre.len());
        let mut hors_domaine: HashMap<u32, [f64; 2]> = HashMap::new();
        for id in ordre {
            let Some(&p) = derniere.get(&id) else {
                continue;
            };
            if dans_le_domaine(p[0], p[1]) {
                par_id.insert(id, ids.len() as u32);
                ids.push(id);
                positions.push(p);
            } else {
                hors_domaine.insert(id, p);
            }
        }
        let bruts: Vec<(u32, Vec<f32>)> = (0..ids.len())
            .map(|c| {
                let p = positions[c];
                (c as u32, vec![p[0] as f32, p[1] as f32])
            })
            .collect();
        let arbre = VpTree::build(bruts, haversine_f32);
        let vivants = ids.len() + hors_domaine.len();
        Self {
            arbre,
            ids,
            positions,
            par_id,
            hors_domaine,
            surcouche: HashMap::new(),
            tombes: HashSet::new(),
            non_points: HashSet::new(),
            vivants,
        }
    }

    /// L'identifiant est-il visible d'une requête ?
    fn visible(&self, id: u32) -> bool {
        self.surcouche.contains_key(&id)
            || self.hors_domaine.contains_key(&id)
            || (self.par_id.contains_key(&id) && !self.tombes.contains(&id))
    }

    /// Ajuste `vivants` après une écriture sur `id`, visible `avant` ou non.
    fn compter(&mut self, id: u32, avant: bool) {
        match (avant, self.visible(id)) {
            (false, true) => self.vivants += 1,
            (true, false) => self.vivants -= 1,
            _ => {}
        }
    }

    /// Un point de l'arbre est-il masqué (déplacé ou supprimé) ?
    fn base_masquee(&self, id: u32) -> bool {
        self.surcouche.contains_key(&id) || self.tombes.contains(&id)
    }

    /// Nombre de points interrogeables, en O(1).
    #[must_use]
    pub fn len(&self) -> usize {
        self.vivants
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
    /// Une coordonnée NON FINIE retire le point et rend `false` : le balayage
    /// ne le rendrait plus, l'index ne doit pas le garder à son ancienne
    /// place. Toute coordonnée finie est acceptée (la surcouche est balayée
    /// exactement, domaine ou pas).
    pub fn inserer(&mut self, id: u32, lon: f64, lat: f64) -> bool {
        if !lon.is_finite() || !lat.is_finite() {
            self.retirer(id);
            return false;
        }
        let avant = self.visible(id);
        self.non_points.remove(&id);
        self.tombes.remove(&id);
        // La surcouche prime sur l'arbre et sur `hors_domaine` ; indexée par
        // identifiant, une réécriture remplace, elle n'empile pas.
        self.surcouche.insert(id, [lon, lat]);
        self.compter(id, avant);
        true
    }

    /// Retire un point. Rend `true` s'il était présent.
    pub fn retirer(&mut self, id: u32) -> bool {
        let avant = self.visible(id);
        self.non_points.remove(&id);
        let dans_surcouche = self.surcouche.remove(&id).is_some();
        let dans_hors = self.hors_domaine.remove(&id).is_some();
        let dans_base = self.par_id.contains_key(&id);
        let base_vivante = dans_base && self.tombes.insert(id);
        self.compter(id, avant);
        dans_surcouche || dans_hors || base_vivante
    }

    /// Marque `id` comme porteur d'une valeur non nulle qui n'est pas un
    /// point : il sort de l'index (comme un retrait) et compte dans
    /// [`SpatialIndex::nombre_non_points`].
    pub fn marquer_non_point(&mut self, id: u32) {
        self.retirer(id);
        self.non_points.insert(id);
    }

    /// Nombre de nœuds dont la propriété n'est pas un point. Tant qu'il n'est
    /// pas nul, un prédicat `distance()` sur cette propriété doit être laissé
    /// au balayage, qui le refusera.
    #[must_use]
    pub fn nombre_non_points(&self) -> usize {
        self.non_points.len()
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

    /// Les points balayés exactement à chaque requête : surcouche, puis
    /// `hors_domaine` non masqués.
    fn hors_arbre(&self) -> impl Iterator<Item = (u32, [f64; 2])> + '_ {
        self.surcouche.iter().map(|(&id, &p)| (id, p)).chain(
            self.hors_domaine
                .iter()
                .filter(|(id, _)| !self.surcouche.contains_key(id))
                .map(|(&id, &p)| (id, p)),
        )
    }

    /// Les points de l'arbre vivants, en balayage (requête hors domaine).
    fn base_vivante(&self) -> impl Iterator<Item = (u32, [f64; 2])> + '_ {
        self.ids
            .iter()
            .zip(&self.positions)
            .filter(|(id, _)| !self.base_masquee(**id))
            .map(|(&id, &p)| (id, p))
    }

    /// Les points à `h ≤ rayon_m` mètres, triés par (distance, identifiant),
    /// avec leur distance en `f64` — ce que rendrait le balayage, sous
    /// l'hypothèse libm de « Borne d'élagage ».
    ///
    /// Un rayon négatif ou NaN rend une liste vide (le balayage aussi :
    /// `d ≤ NaN` est faux).
    #[must_use]
    pub fn dans_rayon(&self, lon: f64, lat: f64, rayon_m: f64) -> Vec<(u32, f64)> {
        if rayon_m.is_nan() || rayon_m < 0.0 || !lon.is_finite() || !lat.is_finite() {
            return Vec::new();
        }
        let mut out: Vec<(u32, f64)> = Vec::new();
        if dans_le_domaine(lon, lat) {
            // 1. La masse : l'arbre, avec le rayon élargi par la borne dérivée,
            //    puis le filtre EXACT.
            let requete = [lon as f32, lat as f32];
            for (compact, _) in
                self.arbre
                    .range_search(&requete, rayon_arbre(rayon_m), haversine_f32)
            {
                let Some(&id) = self.ids.get(compact as usize) else {
                    continue;
                };
                if self.base_masquee(id) {
                    continue;
                }
                let p = self.positions[compact as usize];
                let d = haversine_m(lon, lat, p[0], p[1]);
                if d <= rayon_m {
                    out.push((id, d));
                }
            }
        } else {
            // Requête hors domaine : la borne ne s'applique pas, on balaie.
            for (id, p) in self.base_vivante() {
                let d = haversine_m(lon, lat, p[0], p[1]);
                if d <= rayon_m {
                    out.push((id, d));
                }
            }
        }
        // 2. Ce qui n'est pas dans l'arbre : balayé exactement.
        for (id, p) in self.hors_arbre() {
            let d = haversine_m(lon, lat, p[0], p[1]);
            if d <= rayon_m {
                out.push((id, d));
            }
        }
        trier(&mut out);
        out
    }

    /// Les `k` points les plus proches, dans l'ordre (distance, identifiant).
    ///
    /// L'arbre ne sert qu'à trouver `k` points VIVANTS, donc une borne
    /// supérieure `b` de la k-ième distance exacte ; la réponse est ensuite
    /// les `k` premiers de [`SpatialIndex::dans_rayon`]`(b)`, qui est exacte.
    /// L'ordre `f32` de l'arbre (ex æquo, points confondus) ne décide donc
    /// jamais du résultat, et les tombes ne peuvent pas l'épuiser : la
    /// demande double tant qu'elle ne rend pas `k` vivants.
    ///
    /// Un point dont la distance à la requête n'est pas un nombre n'est
    /// jamais rendu (même contrat que [`SpatialIndex::dans_rayon`] et que le
    /// balayage). `k == 0` rend une liste vide.
    #[must_use]
    pub fn plus_proches(&self, lon: f64, lat: f64, k: usize) -> Vec<(u32, f64)> {
        if k == 0 || !lon.is_finite() || !lat.is_finite() {
            return Vec::new();
        }
        let mut distances: Vec<f64> = self
            .hors_arbre()
            .map(|(_, p)| haversine_m(lon, lat, p[0], p[1]))
            .collect();
        if dans_le_domaine(lon, lat) && !self.ids.is_empty() {
            let requete = [lon as f32, lat as f32];
            let mut demande = k.saturating_add(8).min(self.ids.len());
            loop {
                let vivants: Vec<f64> = self
                    .arbre
                    .knn(&requete, demande, haversine_f32)
                    .into_iter()
                    .filter_map(|(compact, _)| {
                        let id = *self.ids.get(compact as usize)?;
                        if self.base_masquee(id) {
                            return None;
                        }
                        let p = self.positions[compact as usize];
                        Some(haversine_m(lon, lat, p[0], p[1]))
                    })
                    .collect();
                if vivants.len() >= k || demande >= self.ids.len() {
                    distances.extend(vivants);
                    break;
                }
                demande = demande.saturating_mul(2).min(self.ids.len());
            }
        } else {
            distances.extend(
                self.base_vivante()
                    .map(|(_, p)| haversine_m(lon, lat, p[0], p[1])),
            );
        }
        // Distance NaN (seul cas : `lon2 − lon1` déborde en infini) : le
        // balayage ne rend jamais un tel point (`NaN <= r` est faux), le kNN
        // ne le classe donc pas — et surtout, il ne doit pas devenir la borne
        // (`total_cmp` le range en dernier ; pris comme borne, il viderait la
        // réponse).
        distances.retain(|d| !d.is_nan());
        if distances.is_empty() {
            return Vec::new();
        }
        distances.sort_by(f64::total_cmp);
        // Moins de k candidats : ce sont TOUS les vivants (l'arbre a été
        // épuisé), la plus grande distance les couvre tous.
        let borne = distances[k.min(distances.len()) - 1];
        let mut out = self.dans_rayon(lon, lat, borne);
        out.truncate(k);
        out
    }

    /// Vérifie l'invariant « un identifiant n'est jamais à la fois enterré et
    /// vivant » et rend le nombre d'anomalies. Sert aux tests et au diagnostic.
    #[must_use]
    pub fn anomalies(&self) -> usize {
        let tombes_fausses = self
            .tombes
            .iter()
            .filter(|id| self.surcouche.contains_key(id) || !self.par_id.contains_key(id))
            .count();
        let non_points_visibles = self
            .non_points
            .iter()
            .filter(|id| self.visible(**id))
            .count();
        tombes_fausses + non_points_visibles
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
    fn les_accesseurs_disent_ce_que_l_index_porte() {
        // `is_empty` et `len_base` sont publics et n'étaient appelés par aucun
        // test : une méthode publique non exercée est une promesse non tenue.
        let vide = SpatialIndex::build(Vec::new());
        assert!(vide.is_empty(), "un index sans point est vide");
        assert_eq!(vide.len(), 0);
        assert_eq!(vide.len_base(), 0);

        let idx = SpatialIndex::build(points_grille(4, 0.001));
        assert!(!idx.is_empty());
        assert_eq!(idx.len(), 4);
        assert_eq!(idx.len_base(), 4, "la base construite porte les quatre");
    }

    #[test]
    fn une_coordonnee_non_finie_retire_le_point_et_le_dit() {
        // Contrat documenté sur `inserer` : le balayage ne rendrait plus un
        // point non fini, l'index ne doit pas le garder à son ancienne place.
        let mut idx = SpatialIndex::build(vec![(1, 2.35, 48.85), (2, 2.36, 48.85)]);
        assert!(!idx.inserer(1, f64::NAN, 48.85), "non finie → refusée");
        assert_eq!(idx.len(), 1, "le point est SORTI de l'index");
        assert!(!idx.inserer(2, 2.36, f64::INFINITY), "non finie → refusée");
        assert_eq!(idx.len(), 0, "plus aucun point vivant");
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
    fn hors_domaine_est_garde_et_balaye_comme_le_balayage() {
        // AVANT (jusqu'au 21/11) : une latitude de 200° était ÉCARTÉE, alors
        // que le balayage GQL calcule sa distance et peut la rendre — l'index
        // changeait le résultat. Elle est maintenant gardée hors de l'arbre.
        let pts = vec![
            (1, 2.35, 200.0),
            (2, 2.35, 48.85),
            (3, 540.0, 0.0),
            (4, f64::NAN, 1.0),
        ];
        let idx = SpatialIndex::build(pts.clone());
        assert_eq!(idx.len(), 3, "seul le point non fini est écarté");
        for (lon, lat, r) in [(2.35, 48.85, 1.0), (180.0, 0.0, 10.0), (2.35, 200.0, 0.0)] {
            let mut attendu: Vec<(u32, f64)> = pts
                .iter()
                .map(|&(id, a, b)| (id, haversine_m(lon, lat, a, b)))
                .filter(|(_, d)| *d <= r)
                .collect();
            trier(&mut attendu);
            assert_eq!(idx.dans_rayon(lon, lat, r), attendu, "({lon}, {lat}, {r})");
        }
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
    fn non_point_sort_de_lindex_et_se_compte() {
        let mut idx = SpatialIndex::build(vec![(1, 2.35, 48.85), (2, 2.36, 48.85)]);
        idx.marquer_non_point(1);
        assert_eq!(idx.nombre_non_points(), 1);
        assert_eq!(idx.len(), 1);
        assert_eq!(
            idx.dans_rayon(2.35, 48.85, 1.0),
            Vec::new(),
            "un non-point n'est pas rendu"
        );
        // Une valeur redevenue point sort du compte.
        assert!(idx.inserer(1, 2.35, 48.85));
        assert_eq!(idx.nombre_non_points(), 0);
        assert_eq!(idx.len(), 2);
        // Un retrait aussi.
        idx.marquer_non_point(2);
        assert!(!idx.retirer(2), "un non-point n'était pas visible");
        assert_eq!(idx.nombre_non_points(), 0);
        assert_eq!(idx.anomalies(), 0);
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

#[cfg(test)]
mod regression_revue_282 {
    use super::*;

    #[test]
    fn plage_f64_bord_arrondi_f32() {
        let index = SpatialIndex::build(vec![(1, 179.999_992_5, 0.0)]);
        let distance = haversine_m(179.999_992_2, 0.0, 179.999_992_5, 0.0);
        assert!(distance < 0.1);
        assert_eq!(index.dans_rayon(179.999_992_2, 0.0, 0.1).len(), 1);
    }

    #[test]
    fn nearest_apres_plus_de_huit_suppressions() {
        let mut index =
            SpatialIndex::build((0..20).map(|i| (i, f64::from(i) * 0.001, 0.0)).collect());
        for id in 0..9 {
            assert!(index.retirer(id));
        }
        let r = index.plus_proches(0.0, 0.0, 1);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, 9);
    }

    #[test]
    fn nearest_discrimine_les_points_confondus_en_f32() {
        let points = (0..20)
            .map(|i| (i, 180.0 + if i == 1 { 0.000_000_1 } else { 0.000_001 }, 0.0))
            .collect();
        let index = SpatialIndex::build(points);
        let r = index.plus_proches(180.0, 0.0, 1);
        assert_eq!(r.len(), 1);
        assert_eq!(
            r[0].0, 1,
            "le point réellement le plus proche doit gagner en f64"
        );
    }
}

#[cfg(test)]
mod exactitude {
    //! Verrous différentiels : l'index contre le balayage, sur les cas qui
    //! ont cassé (bords d'arrondi `f32`, ±180°, pôles, antipodes, points
    //! confondus, ex æquo, tombes, surcouche, hors domaine).
    use super::*;

    /// Générateur déterministe : aucun hasard non reproductible.
    struct Lcg(u64);
    impl Lcg {
        fn suivant(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
        fn entre(&mut self, a: f64, b: f64) -> f64 {
            a + (b - a) * self.suivant()
        }
    }

    /// Un jeu adverse : grappes qui se confondent en `f32` autour de ±180°,
    /// des pôles et de l'origine, grille quantifiée (ex æquo), points
    /// quelconques, quelques points hors domaine.
    fn jeu(g: &mut Lcg, n: u32) -> Vec<(u32, f64, f64)> {
        let centres = [
            (180.0, 0.0),
            (-180.0, 0.0),
            (179.999_99, 89.999),
            (0.0, -90.0),
            (0.0, 0.0),
            (2.35, 48.85),
        ];
        (0..n)
            .map(|i| {
                let (lon, lat) = match i % 5 {
                    0 | 1 => {
                        let (cl, cp) = centres[(i as usize / 5) % centres.len()];
                        let lat = (cp + g.entre(-2e-6, 2e-6)).clamp(-90.0, 90.0);
                        let lon = (cl + g.entre(-2e-6, 2e-6)).clamp(-180.0, 180.0);
                        (lon, lat)
                    }
                    2 => (
                        (g.entre(-180.0, 180.0) * 1_000.0).round() / 1_000.0,
                        (g.entre(-90.0, 90.0) * 1_000.0).round() / 1_000.0,
                    ),
                    3 => (g.entre(-180.0, 180.0), g.entre(-90.0, 90.0)),
                    _ => {
                        if i % 45 == 4 {
                            (g.entre(-720.0, 720.0), g.entre(-200.0, 200.0))
                        } else {
                            (g.entre(-180.0, 180.0), g.entre(-90.0, 90.0))
                        }
                    }
                };
                (i, lon, lat)
            })
            .collect()
    }

    fn balayage(pts: &HashMap<u32, [f64; 2]>, lon: f64, lat: f64, r: f64) -> Vec<(u32, f64)> {
        let mut v: Vec<(u32, f64)> = pts
            .iter()
            .map(|(&id, p)| (id, haversine_m(lon, lat, p[0], p[1])))
            .filter(|(_, d)| *d <= r)
            .collect();
        trier(&mut v);
        v
    }

    fn plus_proches_balayage(
        pts: &HashMap<u32, [f64; 2]>,
        lon: f64,
        lat: f64,
        k: usize,
    ) -> Vec<(u32, f64)> {
        let mut v: Vec<(u32, f64)> = pts
            .iter()
            .map(|(&id, p)| (id, haversine_m(lon, lat, p[0], p[1])))
            .filter(|(_, d)| !d.is_nan())
            .collect();
        trier(&mut v);
        v.truncate(k);
        v
    }

    #[test]
    fn index_egal_balayage_rayon_et_knn_avec_tombes_et_surcouche() {
        let mut g = Lcg(0x5EED_0282);
        let pts = jeu(&mut g, 1_500);
        let mut idx = SpatialIndex::build(pts.clone());
        let mut verite: HashMap<u32, [f64; 2]> =
            pts.iter().map(|&(id, a, b)| (id, [a, b])).collect();
        // Mutations : suppressions, déplacements (dans et hors domaine),
        // créations — la vérité suit les mêmes écritures.
        for i in 0..400u32 {
            let id = (g.suivant() * 1_800.0) as u32;
            match i % 3 {
                0 => {
                    idx.retirer(id);
                    verite.remove(&id);
                }
                1 => {
                    let (lon, lat) = (180.0 - g.entre(0.0, 3e-6), g.entre(-1e-6, 1e-6));
                    idx.inserer(id, lon, lat);
                    verite.insert(id, [lon, lat]);
                }
                _ => {
                    let (lon, lat) = (g.entre(-190.0, 190.0), g.entre(-95.0, 95.0));
                    idx.inserer(id, lon, lat);
                    verite.insert(id, [lon, lat]);
                }
            }
            assert_eq!(idx.len(), idx.recompter(), "compteur après l'écriture {i}");
            assert_eq!(idx.len(), verite.len(), "len après l'écriture {i}");
        }
        assert_eq!(idx.anomalies(), 0);
        assert_eq!(idx.len(), verite.len());
        let requetes = [
            (180.0, 0.0),
            (-180.0, 0.0),
            (179.999_992_2, 0.0),
            (0.0, 90.0),
            (0.0, -90.0),
            (0.0, 0.0),
            (2.35, 48.85),
            (-0.000_001, 0.000_001),
            (500.0, 10.0),
        ];
        for &(lon, lat) in &requetes {
            for r in [0.0, 0.05, 0.1, 0.5, 2.0, 30.0, 1_000.0, 5e5, 2.1e7] {
                assert_eq!(
                    idx.dans_rayon(lon, lat, r),
                    balayage(&verite, lon, lat, r),
                    "rayon ({lon}, {lat}, {r})"
                );
            }
            for k in [1usize, 2, 7, 50, 2_000] {
                assert_eq!(
                    idx.plus_proches(lon, lat, k),
                    plus_proches_balayage(&verite, lon, lat, k),
                    "knn ({lon}, {lat}, {k})"
                );
            }
        }
        // Requêtes au hasard, y compris au ras des points.
        for _ in 0..200 {
            let base = verite
                .values()
                .nth((g.suivant() * verite.len() as f64) as usize % verite.len())
                .copied()
                .unwrap_or([0.0, 0.0]);
            let lon = (base[0] + g.entre(-1e-6, 1e-6)).clamp(-180.0, 180.0);
            let lat = (base[1] + g.entre(-1e-6, 1e-6)).clamp(-90.0, 90.0);
            let r = [0.0, 0.03, 0.2, 3.0, 100.0][(g.suivant() * 5.0) as usize % 5];
            assert_eq!(
                idx.dans_rayon(lon, lat, r),
                balayage(&verite, lon, lat, r),
                "rayon ({lon}, {lat}, {r})"
            );
            let k = 1 + (g.suivant() * 6.0) as usize;
            assert_eq!(
                idx.plus_proches(lon, lat, k),
                plus_proches_balayage(&verite, lon, lat, k),
                "knn ({lon}, {lat}, {k})"
            );
        }
    }

    /// L'hypothèse libm (`sin`, `cos`, `asin` à 1 ulp) n'est pas garantie par
    /// IEEE 754 : on la VÉRIFIE contre une formule bien conditionnée partout
    /// (atan2, dite de Vincenty sur la sphère), aux pires endroits pour la
    /// haversine — antipodes, quasi-antipodes, très courtes distances.
    #[test]
    fn erreur_haversine_bornee_contre_atan2() {
        fn atan2_m(lon1: f64, lat1: f64, lon2: f64, lat2: f64) -> f64 {
            let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
            let dl = (lon2 - lon1).to_radians();
            let num = ((p2.cos() * dl.sin()).powi(2)
                + (p1.cos() * p2.sin() - p1.sin() * p2.cos() * dl.cos()).powi(2))
            .sqrt();
            let den = p1.sin() * p2.sin() + p1.cos() * p2.cos() * dl.cos();
            RAYON_TERRE_M * num.atan2(den)
        }
        let borne = erreur_haversine_m();
        let mut g = Lcg(0xA27_1D0);
        let mut pire = 0.0f64;
        for _ in 0..200_000 {
            let (lon, lat) = (g.entre(-180.0, 180.0), g.entre(-90.0, 90.0));
            let cas = (g.suivant() * 3.0) as u32;
            let ecart = 10f64.powf(g.entre(-9.0, 0.0));
            let (lon2, lat2) = match cas {
                0 => (
                    lon + 180.0 + g.entre(-ecart, ecart),
                    -lat + g.entre(-ecart, ecart),
                ),
                1 => (lon + g.entre(-ecart, ecart), lat + g.entre(-ecart, ecart)),
                _ => (g.entre(-180.0, 180.0), g.entre(-90.0, 90.0)),
            };
            let lat2 = lat2.clamp(-90.0, 90.0);
            let e = (haversine_m(lon, lat, lon2, lat2) - atan2_m(lon, lat, lon2, lat2)).abs();
            pire = pire.max(e);
        }
        assert!(
            pire <= borne,
            "écart observé {pire} m au-delà de la borne {borne} m"
        );
    }

    /// Revue du 21/11 (complément) : `lon2 − lon1` déborde en infini,
    /// `haversine_m` rend NaN pour le second point. Le balayage GQL ne le rend
    /// jamais (`NaN <= r` est faux) ; l'index ne doit ni le classer, ni perdre
    /// pour autant le point à distance 0.
    #[test]
    fn knn_distance_non_finie_ne_perd_pas_le_point_a_zero() {
        assert!(haversine_m(-f64::MAX, 0.0, f64::MAX, 0.0).is_nan());
        let idx = SpatialIndex::build(vec![(1, -f64::MAX, 0.0), (2, f64::MAX, 0.0)]);
        assert_eq!(idx.plus_proches(-f64::MAX, 0.0, 2), vec![(1, 0.0)]);
        assert_eq!(
            idx.dans_rayon(-f64::MAX, 0.0, f64::INFINITY),
            vec![(1, 0.0)]
        );
    }

    impl SpatialIndex {
        /// Recompte en O(n) ce que `vivants` tient en O(1).
        fn recompter(&self) -> usize {
            let ids: HashSet<u32> = self
                .ids
                .iter()
                .copied()
                .chain(self.hors_domaine.keys().copied())
                .chain(self.surcouche.keys().copied())
                .collect();
            ids.into_iter().filter(|&id| self.visible(id)).count()
        }
    }

    /// Grandes magnitudes finies : hors domaine, donc balayées exactement par
    /// la même fonction que le balayage GQL — y compris quand elle rend NaN.
    #[test]
    fn grandes_magnitudes_egal_balayage_et_nan_seulement_au_debordement() {
        let mut g = Lcg(0xB16_F64);
        let magnitude = |g: &mut Lcg| -> f64 {
            let signe = if g.suivant() < 0.5 { -1.0 } else { 1.0 };
            match (g.suivant() * 4.0) as u32 {
                0 => signe * f64::MAX,
                1 => signe * f64::MAX * g.entre(0.4, 0.6),
                2 => signe * 10f64.powf(g.entre(0.0, 308.0)),
                _ => g.entre(-180.0, 180.0),
            }
        };
        let mut pts: Vec<(u32, f64, f64)> = Vec::new();
        for i in 0..300u32 {
            let lon = magnitude(&mut g);
            let lat = if i % 3 == 0 {
                magnitude(&mut g)
            } else {
                g.entre(-90.0, 90.0)
            };
            pts.push((i, lon, lat));
        }
        let idx = SpatialIndex::build(pts.clone());
        let verite: HashMap<u32, [f64; 2]> = pts.iter().map(|&(id, a, b)| (id, [a, b])).collect();
        assert_eq!(idx.len(), verite.len());
        for _ in 0..60 {
            let (lon, lat) = (
                magnitude(&mut g),
                if g.suivant() < 0.5 {
                    magnitude(&mut g)
                } else {
                    g.entre(-90.0, 90.0)
                },
            );
            for p in verite.values() {
                let d = haversine_m(lon, lat, p[0], p[1]);
                let deborde = (p[0] - lon).is_infinite();
                assert_eq!(
                    d.is_nan(),
                    deborde,
                    "NaN seulement au débordement : ({lon}, {lat}) → {p:?}"
                );
            }
            for r in [0.0, 1e3, 1e7, f64::INFINITY] {
                assert_eq!(
                    idx.dans_rayon(lon, lat, r),
                    balayage(&verite, lon, lat, r),
                    "rayon ({lon}, {lat}, {r})"
                );
            }
            for k in [1usize, 5, 400] {
                assert_eq!(
                    idx.plus_proches(lon, lat, k),
                    plus_proches_balayage(&verite, lon, lat, k),
                    "knn ({lon}, {lat}, {k})"
                );
            }
        }
    }

    #[test]
    fn la_borne_coute_moins_de_vingt_metres() {
        // Ce que l'exactitude coûte : un anneau fixe autour du rayon demandé.
        // Il est documenté ; s'il grossit, c'est une régression de coût.
        for r in [0.0, 1.0, 300.0, 1_600.0] {
            let anneau = f64::from(rayon_arbre(r)) - r;
            assert!(
                anneau > 0.0 && anneau < 20.0,
                "anneau {anneau} m pour r = {r}"
            );
        }
    }
}
