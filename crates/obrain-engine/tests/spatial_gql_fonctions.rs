//! Spatial exposé en GQL, étape G1 : `point(lon, lat)` et `distance(a, b)`.
//!
//! Le rouge sur `origin/main` est le comportement mesuré : la fonction
//! n'existe pas. Ce fichier n'utilise que `execute` (le chemin GQL) et
//! `execute_cypher`, pour que le rouge soit une erreur de requête et non une
//! erreur de compilation. GQL est le langage de référence ; Cypher n'a qu'un
//! test de parité.
//!
//! Convention (décision c6a44d3e) : `point(lon, lat)` rend une liste de deux
//! `Float64` — aucun format disque ni codec touché. `distance` rend des
//! MÈTRES, formule de haversine, rayon moyen 6 371 008,8 m.

use obrain_common::types::Value;
use obrain_engine::ObrainDB;

/// Exécute `RETURN <expr> AS d` EN GQL et rend la seule valeur, ou panique
/// avec la requête et l'erreur pour que le rouge se lise.
fn evaluer(expr: &str) -> Value {
    let db = ObrainDB::new_in_memory();
    let requete = format!("RETURN {expr} AS d");
    let r = db
        .execute(&requete)
        .unwrap_or_else(|e| panic!("`{requete}` doit s'exécuter : {e}"));
    assert_eq!(r.rows.len(), 1, "une ligne attendue pour `{requete}`");
    r.rows[0][0].clone()
}

fn metres(expr: &str) -> f64 {
    evaluer(expr)
        .as_float64()
        .unwrap_or_else(|| panic!("`{expr}` doit rendre un Float64"))
}

/// Paris (2,3522 ; 48,8566) → Lyon (4,8357 ; 45,7640) : ≈ 391,5 km à vol
/// d'oiseau (haversine calculé à la main, rayon moyen). Tolérance ≈ 0,5 %.
#[test]
fn distance_paris_lyon() {
    let d = metres("distance(point(2.3522, 48.8566), point(4.8357, 45.7640))");
    assert!(
        (389_500.0..=393_500.0).contains(&d),
        "Paris–Lyon doit faire ≈ 391,5 km, obtenu {d} m"
    );
}

/// Deux points identiques : exactement 0, pas un résidu d'arrondi.
#[test]
fn distance_d_un_point_a_lui_meme_est_nulle() {
    assert_eq!(
        metres("distance(point(2.3522, 48.8566), point(2.3522, 48.8566))"),
        0.0
    );
}

/// Symétrie : d(a, b) = d(b, a) au bit près.
#[test]
fn distance_est_symetrique() {
    let ab = metres("distance(point(2.3522, 48.8566), point(4.8357, 45.7640))");
    let ba = metres("distance(point(4.8357, 45.7640), point(2.3522, 48.8566))");
    assert_eq!(ab.to_bits(), ba.to_bits(), "d(a,b)={ab} d(b,a)={ba}");
}

/// Antipodes : la demi-circonférence, π·R ≈ 20 015 087 m. C'est le cas où un
/// `asin` non borné rend NaN à cause d'un arrondi > 1.
#[test]
fn distance_aux_antipodes_est_finie() {
    let d = metres("distance(point(0.0, 0.0), point(180.0, 0.0))");
    assert!(
        d.is_finite(),
        "antipodes : valeur finie attendue, obtenu {d}"
    );
    assert!(
        (20_000_000.0..=20_030_000.0).contains(&d),
        "antipodes ≈ 20 015 km, obtenu {d} m"
    );
}

/// Le pôle : toutes les longitudes y désignent le même point.
#[test]
fn distance_au_pole_ignore_la_longitude() {
    let d = metres("distance(point(0.0, 90.0), point(123.0, 90.0))");
    assert!(d < 1.0, "même pôle, longitudes différentes : {d} m");
}

/// `point` se lit comme une valeur : deux flottants, dans l'ordre (lon, lat).
#[test]
fn point_rend_deux_flottants_lon_puis_lat() {
    let v = evaluer("point(2.5, 48.5)");
    let l = v.as_list().expect("point() doit rendre une liste");
    assert_eq!(l.len(), 2);
    assert_eq!(l[0].as_float64(), Some(2.5), "lon en premier");
    assert_eq!(l[1].as_float64(), Some(48.5), "lat en second");
}

/// Une latitude hors de [-90, 90] est une ERREUR, pas une distance fausse :
/// un `distance()` qui répond sur une coordonnée absurde ment en silence.
#[test]
fn point_refuse_une_latitude_invalide() {
    let db = ObrainDB::new_in_memory();
    let r = db.execute("RETURN point(2.0, 95.0) AS p");
    let erreur = match r {
        Ok(res) => panic!("latitude 95 : erreur attendue, obtenu {:?}", res.rows),
        Err(e) => e.to_string(),
    };
    // Garde-fou contre un vert pour la mauvaise raison : « fonction inconnue »
    // est aussi une erreur, mais ne prouve rien sur la validation.
    assert!(
        !erreur.contains("inconnue"),
        "point() doit exister ; l'erreur doit venir de la validation : {erreur}"
    );
    assert!(
        erreur.to_lowercase().contains("lat"),
        "l'erreur doit nommer la latitude : {erreur}"
    );
}

/// Parité : le traducteur Cypher rend la même valeur que GQL (même exécuteur,
/// pas une seconde implémentation).
#[cfg(feature = "cypher")]
#[test]
fn cypher_rend_la_meme_distance_que_gql() {
    let expr = "distance(point(2.3522, 48.8566), point(4.8357, 45.7640))";
    let gql = metres(expr);
    let db = ObrainDB::new_in_memory();
    let r = db
        .execute_cypher(&format!("RETURN {expr} AS d"))
        .expect("Cypher doit exécuter la même requête");
    assert_eq!(
        r.rows[0][0].as_float64().map(f64::to_bits),
        Some(gql.to_bits())
    );
}

/// L'usage réel : un géofence en `WHERE` sur des nœuds. Un nœud SANS
/// coordonnées (propriété absente → Null) est écarté sans faire échouer la
/// requête — sur un graphe réel, toutes les lignes n'ont pas de position.
#[test]
fn geofence_en_where_ecarte_les_noeuds_sans_coordonnees() {
    let db = ObrainDB::new_in_memory();
    // Paris, Versailles (≈ 14 km), Lyon (≈ 392 km), et un lieu sans position.
    for (nom, pos) in [
        ("paris", Some((2.3522, 48.8566))),
        ("versailles", Some((2.1204, 48.8049))),
        ("lyon", Some((4.8357, 45.7640))),
        ("inconnu", None),
    ] {
        let n = db.create_node(&["Lieu"]);
        db.set_node_property(n, "nom", Value::String(nom.into()));
        if let Some((lon, lat)) = pos {
            db.set_node_property(n, "lon", Value::Float64(lon));
            db.set_node_property(n, "lat", Value::Float64(lat));
        }
    }
    let r = db
        .execute(
            "MATCH (n:Lieu) \
             WHERE distance(point(n.lon, n.lat), point(2.3522, 48.8566)) < 50000.0 \
             RETURN n.nom AS nom ORDER BY nom",
        )
        .unwrap_or_else(|e| panic!("le géofence doit s'exécuter : {e}"));
    let noms: Vec<String> = r
        .rows
        .iter()
        .map(|l| l[0].as_str().expect("nom").to_string())
        .collect();
    assert_eq!(
        noms,
        ["paris", "versailles"],
        "rayon 50 km autour de Paris : ni Lyon, ni le lieu sans position"
    );
}

/// `distance` d'un Null rend Null (comme les autres fonctions de GQL), et ne
/// lève pas d'erreur.
#[test]
fn distance_d_un_null_est_null() {
    assert_eq!(evaluer("distance(point(2.0, 48.0), null)"), Value::Null);
}
