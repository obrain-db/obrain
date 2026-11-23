//! Géométrie de la sphère : le modèle PUR du spatial (aucune dépendance).
//!
//! Une seule définition de la distance géographique, partagée par :
//! - la fonction GQL `distance(point(lon, lat), point(lon, lat))` ;
//! - l'index spatial (étape G2), qui DOIT mesurer avec la même formule pour
//!   que la revérification exacte des candidats rende les mêmes lignes qu'un
//!   balayage.
//!
//! Deux définitions de « distance » dans le même moteur rendraient la réponse
//! dépendante du chemin choisi par le planificateur : exactement ce qu'un
//! index ne doit jamais changer.
//!
//! ── CHOIX ─────────────────────────────────────────────────────────────────
//! Haversine sur une sphère de rayon moyen [`RAYON_TERRE_M`]. Erreur par
//! rapport à l'ellipsoïde WGS-84 : jusqu'à ≈ 0,5 % — acceptable pour un
//! géofence et un « le plus proche », PAS pour du cadastre. La haversine est
//! une vraie métrique sur la sphère (inégalité triangulaire), donc un VP-Tree
//! l'élague correctement.

use obrain_common::types::Value;

/// Rayon moyen de la Terre, en mètres (valeur IUGG, 6 371 008,8 m).
pub const RAYON_TERRE_M: f64 = 6_371_008.8;

/// Une latitude est-elle valide ? Finie et dans [-90, 90] degrés.
#[must_use]
pub fn latitude_valide(lat: f64) -> bool {
    lat.is_finite() && (-90.0..=90.0).contains(&lat)
}

/// Distance géographique entre deux points `(lon, lat)` en degrés, en MÈTRES.
///
/// Symétrique au bit près : `d(a, b) == d(b, a)` (les seules opérations qui
/// dépendent de l'ordre sont des carrés de sinus et un produit commutatif).
/// Rend exactement `0.0` pour deux points identiques. `a` est ramené dans
/// [0, 1] avant l'`asin`, car des antipodes peuvent produire
/// `1.0000000000000002`.
///
/// NaN : rendu pour toute entrée non finie, ET pour des entrées finies dont la
/// différence de longitudes DÉBORDE (`lon2 − lon1` hors de `±f64::MAX`, par
/// exemple `-f64::MAX` et `f64::MAX`) : `sin(∞)` vaut NaN. C'est le seul cas
/// pour des entrées finies — `to_radians` multiplie par π/180 < 1, et la
/// différence des latitudes en radians ne peut pas déborder. Le prédicat GQL
/// `distance(..) <= r` est alors faux, et l'index spatial suit ce contrat.
///
/// Grandes magnitudes finies : la fonction reste définie, mais la réduction
/// d'argument de `sin`/`cos` sur des angles énormes n'a plus de sens
/// géographique. Le résultat est celui de cette formule, rien de plus.
#[must_use]
pub fn haversine_m(lon1: f64, lat1: f64, lon2: f64, lat2: f64) -> f64 {
    let (phi1, phi2) = (lat1.to_radians(), lat2.to_radians());
    let dphi = phi2 - phi1;
    let dlambda = (lon2 - lon1).to_radians();
    let sin_dphi = (dphi / 2.0).sin();
    let sin_dlambda = (dlambda / 2.0).sin();
    let a = sin_dphi * sin_dphi + phi1.cos() * phi2.cos() * sin_dlambda * sin_dlambda;
    let c = 2.0 * a.clamp(0.0, 1.0).sqrt().asin();
    RAYON_TERRE_M * c
}

/// Un nombre GQL (entier ou flottant) en `f64`. `None` pour tout autre type.
#[must_use]
pub fn nombre_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Float64(f) => Some(*f),
        Value::Int64(i) => Some(*i as f64),
        _ => None,
    }
}

/// `(lon, lat)` d'un point tel que `point()` le produit : une liste de deux
/// nombres. `None` si la valeur n'en est pas un.
///
/// C'est LA définition de ce qu'est un point dans obrain — partagée par la
/// fonction GQL `distance()`, par l'index spatial et par les procédures
/// `CALL`. Deux définitions divergeraient sur ce qu'est une coordonnée, et
/// l'index indexerait alors autre chose que ce que la requête interroge.
#[must_use]
pub fn point_lon_lat(v: &Value) -> Option<(f64, f64)> {
    match v {
        Value::List(l) if l.len() == 2 => Some((nombre_f64(&l[0])?, nombre_f64(&l[1])?)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paris_lyon_environ_391_5_km() {
        let d = haversine_m(2.3522, 48.8566, 4.8357, 45.7640);
        assert!((389_500.0..=393_500.0).contains(&d), "{d}");
    }

    #[test]
    fn point_identique_donne_zero_exact() {
        assert_eq!(haversine_m(2.3522, 48.8566, 2.3522, 48.8566), 0.0);
    }

    #[test]
    fn symetrie_au_bit_pres_sur_mille_paires() {
        // Petit générateur congruentiel : déterministe, sans dépendance.
        let mut etat: u64 = 0x2545_F491_4F6C_DD1D;
        let mut tirer = |min: f64, max: f64| {
            etat = etat
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            min + (max - min) * ((etat >> 11) as f64 / (1u64 << 53) as f64)
        };
        for _ in 0..1000 {
            let (lon_a, lat_a, lon_b, lat_b) = (
                tirer(-180.0, 180.0),
                tirer(-90.0, 90.0),
                tirer(-180.0, 180.0),
                tirer(-90.0, 90.0),
            );
            assert_eq!(
                haversine_m(lon_a, lat_a, lon_b, lat_b).to_bits(),
                haversine_m(lon_b, lat_b, lon_a, lat_a).to_bits(),
                "({lon_a},{lat_a}) ({lon_b},{lat_b})"
            );
        }
    }

    #[test]
    fn antipodes_finis_et_demi_perimetre() {
        let d = haversine_m(0.0, 0.0, 180.0, 0.0);
        assert!(d.is_finite());
        assert!(
            (d - std::f64::consts::PI * RAYON_TERRE_M).abs() < 1.0,
            "{d}"
        );
        // Antipodes sur un parallèle quelconque : le cas qui dépasse 1.
        let d = haversine_m(10.0, 37.0, -170.0, -37.0);
        assert!(d.is_finite(), "{d}");
    }

    #[test]
    fn pole_ignore_la_longitude() {
        assert!(haversine_m(0.0, 90.0, 123.0, 90.0) < 1.0);
    }

    #[test]
    fn longitude_est_periodique() {
        let a = haversine_m(179.9, 10.0, -179.9, 10.0);
        let b = haversine_m(179.9, 10.0, 180.1, 10.0);
        assert!((a - b).abs() < 1e-6, "{a} vs {b}");
    }

    #[test]
    fn validation_de_latitude() {
        assert!(latitude_valide(0.0) && latitude_valide(90.0) && latitude_valide(-90.0));
        assert!(!latitude_valide(90.0001) && !latitude_valide(f64::NAN));
        assert!(!latitude_valide(f64::INFINITY));
    }
}
