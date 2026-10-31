//! Conformité GQL — lot 02 : les COMPARAISONS (`<` `<=` `>` `>=` `=` `<>`).
//!
//! Principe de rédaction : **chaque assertion encode l'intention sémantique,
//! jamais l'observation**. Les valeurs sont posées À LA BORNE EXACTE — une
//! ligne pile sur le seuil — parce qu'un test de comparaison dont les données
//! sont éloignées passe trivialement et ne prouve rien.
//!
//! ## Divergences mesurées (2026-08-24, agent `gql-02-comparaison`)
//!
//! Les tests du bloc « DIVERGENCES » en fin de fichier sont `#[ignore]` : ils
//! énoncent ce que la sémantique DOIT donner, et ils échouent aujourd'hui.
//! Ils ne sont volontairement PAS ajustés au comportement observé — les figer
//! rendrait les défauts intouchables. `cargo test … -- --ignored` les rejoue.
//!
//! | # | Divergence | Cause |
//! |---|---|---|
//! | D1 | Une ligne peut n'être NI `=` NI `<>` à une valeur | `filter.rs:3418` `values_equal` compare les `Float64` par tolérance absolue `f64::EPSILON`, alors que `=` sur propriété reste exact |
//! | D2 | `<=` n'est pas `<` OU `=` : `NaN <= NaN` rend vrai | `filter.rs:3529` `compare_values` a un `if a<b / else if a>b / else Equal` sans branche NaN |
//! | D3 | Deux entiers distincts sont égaux au même flottant | `filter.rs:3420` convertit l'entier en `f64` avant comparaison ; 2^53+1 n'est pas représentable |
//! | D4 | `min()`/`max()` sur colonne CHAÎNE rendent `Int64(0)` | chemin `value_utils.rs` |
//! | D5 | STRING vs NUMBER est coercé au lieu d'être NULL, et `=` / `<=` ne coercent pas pareil | `values_equal:3424` parse en `i64`, `compare_values:3541` parse en `f64` |
#![cfg(feature = "gql")]

use obrain_common::types::Value;
use obrain_engine::{ObrainDB, Session};

// ---------------------------------------------------------------------------
// Harnais — les états sont construits par le seul code de production.
// ---------------------------------------------------------------------------

/// Exécute `query` et rend la 1re colonne triée, chaînes déballées.
///
/// Une requête refusée fait échouer le test avec la requête fautive : un test
/// qui avale une erreur de syntaxe est un test vide.
fn keys(s: &Session, query: &str) -> Vec<String> {
    let r = s
        .execute(query)
        .unwrap_or_else(|e| panic!("requête refusée par le moteur :\n  {query}\n  → {e}"));
    let mut v: Vec<String> = r
        .rows
        .iter()
        .map(|row| match &row[0] {
            Value::String(s) => s.to_string(),
            other => format!("{other:?}"),
        })
        .collect();
    v.sort();
    v
}

/// Évalue une expression scalaire GQL sur une ligne d'ancrage unique.
fn scalar(s: &Session, expr: &str) -> Value {
    let query = format!("MATCH (a:Ancre) RETURN {expr}");
    let r = s
        .execute(&query)
        .unwrap_or_else(|e| panic!("requête refusée par le moteur :\n  {query}\n  → {e}"));
    assert_eq!(
        r.rows.len(),
        1,
        "l'ancre doit produire exactement 1 ligne, sinon l'expression n'est pas évaluée : {query}"
    );
    r.rows[0][0].clone()
}

fn vrai(s: &Session, expr: &str) {
    assert_eq!(
        scalar(s, expr),
        Value::Bool(true),
        "l'expression `{expr}` doit valoir VRAI"
    );
}

fn faux(s: &Session, expr: &str) {
    assert_eq!(
        scalar(s, expr),
        Value::Bool(false),
        "l'expression `{expr}` doit valoir FAUX"
    );
}

fn base(inserts: &[&str]) -> (ObrainDB, ()) {
    let db = ObrainDB::new_in_memory();
    {
        let s = db.session();
        s.execute("INSERT (:Ancre {k: 'a'})").expect("ancre");
        for stmt in inserts {
            s.execute(stmt)
                .unwrap_or_else(|e| panic!("insertion refusée :\n  {stmt}\n  → {e}"));
        }
    }
    (db, ())
}

// ---------------------------------------------------------------------------
// A. LA BORNE EXACTE : `>=` contre `>`, `<=` contre `<`
//
// Le jeu contient une ligne PILE sur le seuil (v = 1). C'est le seul endroit
// où `>=` et `>` peuvent se distinguer ; tout le reste du domaine les rend
// indiscernables.
// ---------------------------------------------------------------------------

fn base_entiers() -> ObrainDB {
    base(&[
        "INSERT (:I {k: 'moins_un', v: -1})",
        "INSERT (:I {k: 'zero',     v: 0})",
        "INSERT (:I {k: 'borne',    v: 1})", // ← pile sur le seuil
        "INSERT (:I {k: 'deux',     v: 2})",
    ])
    .0
}

#[test]
fn ge_inclut_la_borne_gt_l_exclut() {
    let db = base_entiers();
    let s = db.session();

    assert_eq!(
        keys(&s, "MATCH (n:I) WHERE n.v >= 1 RETURN n.k"),
        vec!["borne", "deux"],
        ">= doit INCLURE la ligne posée exactement sur le seuil (v = 1)"
    );
    assert_eq!(
        keys(&s, "MATCH (n:I) WHERE n.v > 1 RETURN n.k"),
        vec!["deux"],
        "> doit EXCLURE la ligne posée exactement sur le seuil (v = 1)"
    );
    // Les deux ne doivent différer QUE de la ligne-borne : c'est la définition.
    assert_eq!(
        keys(&s, "MATCH (n:I) WHERE n.v >= 1 RETURN n.k").len()
            - keys(&s, "MATCH (n:I) WHERE n.v > 1 RETURN n.k").len(),
        1,
        ">= et > doivent différer d'exactement la ligne-borne"
    );
}

#[test]
fn le_inclut_la_borne_lt_l_exclut() {
    let db = base_entiers();
    let s = db.session();

    assert_eq!(
        keys(&s, "MATCH (n:I) WHERE n.v <= 1 RETURN n.k"),
        vec!["borne", "moins_un", "zero"],
        "<= doit INCLURE la ligne posée exactement sur le seuil"
    );
    assert_eq!(
        keys(&s, "MATCH (n:I) WHERE n.v < 1 RETURN n.k"),
        vec!["moins_un", "zero"],
        "< doit EXCLURE la ligne posée exactement sur le seuil"
    );
}

#[test]
fn les_quatre_operateurs_partitionnent_le_domaine_a_la_borne() {
    let db = base_entiers();
    let s = db.session();
    // < et >= sont complémentaires ; > et <= aussi. Sur un domaine sans NULL
    // la réunion doit rendre TOUTES les lignes et l'intersection AUCUNE.
    let total = keys(&s, "MATCH (n:I) RETURN n.k").len();
    let lt = keys(&s, "MATCH (n:I) WHERE n.v < 1 RETURN n.k").len();
    let ge = keys(&s, "MATCH (n:I) WHERE n.v >= 1 RETURN n.k").len();
    assert_eq!(lt + ge, total, "< et >= doivent partitionner le domaine");
    let gt = keys(&s, "MATCH (n:I) WHERE n.v > 1 RETURN n.k").len();
    let le = keys(&s, "MATCH (n:I) WHERE n.v <= 1 RETURN n.k").len();
    assert_eq!(gt + le, total, "> et <= doivent partitionner le domaine");
    assert_eq!(
        keys(&s, "MATCH (n:I) WHERE n.v < 1 AND n.v >= 1 RETURN n.k"),
        Vec::<String>::new(),
        "aucune ligne ne peut être à la fois < 1 et >= 1"
    );
}

#[test]
fn litteraux_a_la_borne() {
    let db = base_entiers();
    let s = db.session();
    vrai(&s, "1 <= 1");
    vrai(&s, "1 >= 1");
    faux(&s, "1 < 1");
    faux(&s, "1 > 1");
    vrai(&s, "1 = 1");
    faux(&s, "1 <> 1");
}

// ---------------------------------------------------------------------------
// B. `<>` contre `NOT (… = …)`
// ---------------------------------------------------------------------------

#[test]
fn ne_equivaut_a_not_eq_sur_valeurs_presentes() {
    let db = base_entiers();
    let s = db.session();
    let ne = keys(&s, "MATCH (n:I) WHERE n.v <> 1 RETURN n.k");
    let not_eq = keys(&s, "MATCH (n:I) WHERE NOT (n.v = 1) RETURN n.k");
    assert_eq!(
        ne, not_eq,
        "`<>` et `NOT (… = …)` doivent désigner exactement le même ensemble"
    );
    assert_eq!(
        ne,
        vec!["deux", "moins_un", "zero"],
        "`<> 1` doit rendre toutes les lignes sauf celle posée sur la borne"
    );
}

// ---------------------------------------------------------------------------
// C. Coercition ENTIER ↔ FLOTTANT à la borne
// ---------------------------------------------------------------------------

#[test]
fn entier_et_flottant_se_comparent_a_la_borne() {
    let db = base(&[
        "INSERT (:I {k: 'un_i',   v: 1})",
        "INSERT (:I {k: 'deux_i', v: 2})",
        "INSERT (:F {k: 'un_f',   v: 1.0})",
        "INSERT (:F {k: 'moitie', v: 1.5})",
    ])
    .0;
    let s = db.session();

    // Propriété ENTIÈRE confrontée à un littéral FLOTTANT valant la borne.
    assert_eq!(
        keys(&s, "MATCH (n:I) WHERE n.v = 1.0 RETURN n.k"),
        vec!["un_i"],
        "l'entier 1 doit être égal au flottant 1.0 (mêmes valeurs numériques)"
    );
    assert_eq!(
        keys(&s, "MATCH (n:I) WHERE n.v >= 1.0 RETURN n.k"),
        vec!["deux_i", "un_i"],
        ">= 1.0 doit inclure l'entier posé sur la borne"
    );
    assert_eq!(
        keys(&s, "MATCH (n:I) WHERE n.v < 1.5 RETURN n.k"),
        vec!["un_i"],
        "1 < 1.5 : la coercition doit se faire vers le flottant, pas par troncature"
    );

    // Propriété FLOTTANTE confrontée à un littéral ENTIER valant la borne.
    // Le sens inverse doit donner exactement la même réponse : la comparaison
    // ne doit pas dépendre de quel côté porte quel type.
    assert_eq!(
        keys(&s, "MATCH (n:F) WHERE n.v = 1 RETURN n.k"),
        vec!["un_f"],
        "le flottant 1.0 doit être égal à l'entier 1, dans ce sens aussi"
    );
    assert_eq!(
        keys(&s, "MATCH (n:F) WHERE n.v >= 1 RETURN n.k"),
        vec!["moitie", "un_f"],
        ">= 1 (entier) doit inclure le flottant posé sur la borne"
    );
    assert_eq!(
        keys(&s, "MATCH (n:F) WHERE n.v > 1 RETURN n.k"),
        vec!["moitie"],
        "> 1 (entier) doit exclure le flottant posé sur la borne"
    );

    vrai(&s, "1 = 1.0");
    vrai(&s, "1 < 1.5");
    faux(&s, "1 > 1.0");
    // Symétrie stricte : inverser les opérandes doit inverser le sens.
    vrai(&s, "1.0 = 1");
    vrai(&s, "1.5 > 1");
    faux(&s, "1.5 < 1");
}

// ---------------------------------------------------------------------------
// D. FLOTTANTS : zéro négatif, infinis
// ---------------------------------------------------------------------------

#[test]
fn zero_negatif_egale_zero_positif() {
    let db = base(&[
        "INSERT (:F {k: 'zero_pos', v: 0.0})",
        "INSERT (:F {k: 'zero_neg', v: -0.0})",
        "INSERT (:F {k: 'un',       v: 1.0})",
    ])
    .0;
    let s = db.session();

    // IEEE-754 : -0.0 == +0.0. Les deux lignes sont sur la MÊME borne.
    assert_eq!(
        keys(&s, "MATCH (n:F) WHERE n.v = 0.0 RETURN n.k"),
        vec!["zero_neg", "zero_pos"],
        "-0.0 et +0.0 sont la même valeur : `= 0.0` doit rendre les deux"
    );
    assert_eq!(
        keys(&s, "MATCH (n:F) WHERE n.v = -0.0 RETURN n.k"),
        vec!["zero_neg", "zero_pos"],
        "`= -0.0` doit rendre exactement le même ensemble que `= 0.0`"
    );
    assert_eq!(
        keys(&s, "MATCH (n:F) WHERE n.v < 0.0 RETURN n.k"),
        Vec::<String>::new(),
        "-0.0 n'est PAS strictement inférieur à 0.0"
    );
    assert_eq!(
        keys(&s, "MATCH (n:F) WHERE n.v >= 0.0 RETURN n.k").len(),
        3,
        ">= 0.0 doit inclure les deux zéros posés sur la borne"
    );
    vrai(&s, "0.0 = -0.0");
    faux(&s, "-0.0 < 0.0");
}

#[test]
fn infinis_se_comparent_comme_bornes_du_domaine() {
    let db = base_entiers();
    let s = db.session();
    // Le moteur accepte l'infini (1.0/0.0). Il doit dominer tout fini.
    vrai(&s, "1.0/0.0 > 1.0e308");
    vrai(&s, "-1.0/0.0 < -1.0e308");
    vrai(&s, "1.0/0.0 > -1.0/0.0");
}

#[test]
fn nan_nest_ordonne_avec_rien() {
    let db = base_entiers();
    let s = db.session();
    // IEEE-754 : toute comparaison d'ordre impliquant NaN est fausse.
    faux(&s, "0.0/0.0 < 1.0");
    faux(&s, "0.0/0.0 > 1.0");
    faux(&s, "0.0/0.0 = 0.0/0.0");
}

// ---------------------------------------------------------------------------
// E. CHAÎNES : ordre lexicographique, casse, préfixe
// ---------------------------------------------------------------------------

fn base_chaines() -> ObrainDB {
    base(&[
        "INSERT (:S {k: 'maj_A',  v: 'A'})",
        "INSERT (:S {k: 'maj_Z',  v: 'Z'})",
        "INSERT (:S {k: 'min_a',  v: 'a'})",
        "INSERT (:S {k: 'ab',     v: 'ab'})", // ← préfixe de 'abc', et borne
        "INSERT (:S {k: 'abc',    v: 'abc'})",
    ])
    .0
}

#[test]
fn ordre_des_chaines_suit_les_points_de_code_et_la_casse_compte() {
    let db = base_chaines();
    let s = db.session();

    // Aucune collation n'est déclarée : l'ordre doit être celui des points de
    // code. 'Z' = U+005A, 'a' = U+0061, donc toutes les majuscules ASCII
    // précèdent toutes les minuscules ASCII.
    vrai(&s, "'Z' < 'a'");
    vrai(&s, "'A' < 'Z'");
    // La casse est significative : deux graphies ne sont pas la même valeur.
    faux(&s, "'a' = 'A'");
    vrai(&s, "'a' > 'A'");

    assert_eq!(
        keys(&s, "MATCH (n:S) WHERE n.v < 'a' RETURN n.k"),
        vec!["maj_A", "maj_Z"],
        "< 'a' doit rendre les majuscules ASCII et elles seules"
    );
    assert_eq!(
        keys(&s, "MATCH (n:S) WHERE n.v = 'A' RETURN n.k"),
        vec!["maj_A"],
        "l'égalité de chaînes doit être sensible à la casse"
    );
}

#[test]
fn prefixe_est_strictement_inferieur_a_ce_qu_il_prefixe() {
    let db = base_chaines();
    let s = db.session();

    // 'ab' préfixe 'abc' : la chaîne la plus courte doit précéder. C'est la
    // borne du test — 'ab' est PILE le seuil de `>= 'ab'`.
    vrai(&s, "'ab' < 'abc'");
    faux(&s, "'abc' < 'ab'");
    faux(&s, "'ab' = 'abc'");
    // La chaîne vide précède tout.
    vrai(&s, "'' < 'a'");
    vrai(&s, "'' <= ''");
    faux(&s, "'' < ''");

    assert_eq!(
        keys(&s, "MATCH (n:S) WHERE n.v >= 'ab' RETURN n.k"),
        vec!["ab", "abc"],
        ">= 'ab' doit INCLURE la chaîne 'ab' elle-même, posée sur la borne"
    );
    assert_eq!(
        keys(&s, "MATCH (n:S) WHERE n.v > 'ab' RETURN n.k"),
        vec!["abc"],
        "> 'ab' doit EXCLURE la chaîne 'ab' elle-même"
    );
}

#[test]
fn non_ascii_suit_les_points_de_code_sans_collation() {
    let db = base(&[
        "INSERT (:U {k: 'e',      v: 'e'})",
        "INSERT (:U {k: 'eacute', v: 'é'})",
        "INSERT (:U {k: 'z',      v: 'z'})",
        "INSERT (:U {k: 'emoji',  v: '😀'})",
    ])
    .0;
    let s = db.session();

    // Le moteur ne déclare aucune collation. L'ordre DOIT donc être celui des
    // points de code Unicode — c'est-à-dire, pour UTF-8, celui des octets :
    //   'e' U+0065 < 'z' U+007A < 'é' U+00E9 < '😀' U+1F600
    // Une collation linguistique rangerait 'é' juste après 'e' ; ce test
    // fixe l'absence de collation comme un CONTRAT explicite, pour qu'on ne
    // l'introduise pas par accident.
    vrai(&s, "'e' < 'z'");
    vrai(&s, "'z' < 'é'");
    vrai(&s, "'é' < '😀'");
    vrai(&s, "'é' > 'e'");

    assert_eq!(
        keys(&s, "MATCH (n:U) WHERE n.v > 'z' RETURN n.k"),
        vec!["eacute", "emoji"],
        "sans collation, 'é' et l'emoji se rangent APRÈS 'z'"
    );
    // La comparaison doit se faire par point de code entier, jamais par octet
    // isolé : 'é' fait deux octets (0xC3 0xA9) et '😀' quatre.
    assert_eq!(
        keys(&s, "MATCH (n:U) RETURN n.v ORDER BY n.v ASC").len(),
        4,
        "les quatre valeurs doivent rester comparables entre elles"
    );
}

#[test]
fn ordre_des_chaines_ne_derive_pas_vers_le_numerique() {
    // Trois chaînes qui RESSEMBLENT à des nombres. GQL compare des CHAÎNES :
    // l'ordre doit rester lexicographique. Un moteur qui les compare
    // numériquement rendrait '10' > '9' — l'inverse.
    let db = base(&[
        "INSERT (:N {k: 'dix',  v: '10'})",
        "INSERT (:N {k: 'deux', v: '2'})",
        "INSERT (:N {k: 'neuf', v: '9'})",
    ])
    .0;
    let s = db.session();

    vrai(&s, "'10' < '9'");
    vrai(&s, "'10' < '2'");
    faux(&s, "'10' > '9'");
    assert_eq!(
        keys(&s, "MATCH (n:N) WHERE n.v < '9' RETURN n.k"),
        vec!["deux", "dix"],
        "'10' et '2' précèdent '9' en ordre lexicographique — pas numérique"
    );
    assert_eq!(
        keys(&s, "MATCH (n:N) RETURN n.v ORDER BY n.v ASC"),
        vec!["10", "2", "9"],
        "ORDER BY sur une colonne CHAÎNE doit trier lexicographiquement"
    );
}

// ---------------------------------------------------------------------------
// F. Ordre des chaînes et dates RFC3339 à précision variable
//
// Fait transmis, ici RECOUPÉ par mesure directe et non repris de confiance :
// '+' (0x2B) précède '.' (0x2E) et tout chiffre (0x30-0x39). C'est ce qui
// rend correcte la comparaison de deux RFC3339 dont l'un porte des décimales
// et l'autre non.
// ---------------------------------------------------------------------------

#[test]
fn plus_precede_le_point_qui_precede_les_chiffres() {
    let db = base_chaines();
    let s = db.session();
    vrai(&s, "'+' < '.'"); // 0x2B < 0x2E
    vrai(&s, "'.' < '0'"); // 0x2E < 0x30
    vrai(&s, "'+' < '0'");
    vrai(&s, "'+' < '9'");
    // Et 'Z' (0x5A) se range APRÈS les trois — conséquence directe, et piège :
    // un horodatage en 'Z' ne se range pas où on croit face à un décalage.
    vrai(&s, "'Z' > '+'");
    vrai(&s, "'Z' > '.'");
    vrai(&s, "'Z' > '9'");
}

#[test]
fn rfc3339_a_precision_variable_reste_correctement_ordonne() {
    let db = base(&[
        "INSERT (:D {k: 'sec',  v: '2026-08-24T12:00:00+02:00'})",
        "INSERT (:D {k: 'nano', v: '2026-08-24T12:00:00.000000000+02:00'})",
        "INSERT (:D {k: 'suiv', v: '2026-08-24T12:00:01+02:00'})",
    ])
    .0;
    let s = db.session();

    // Même instant, deux précisions d'écriture : la forme sans décimales doit
    // précéder ou égaler la forme à neuf décimales — jamais la suivre, sinon
    // un filtre `>= borne` raterait des lignes.
    vrai(
        &s,
        "'2026-08-24T12:00:00+02:00' < '2026-08-24T12:00:00.000000000+02:00'",
    );
    // Et surtout : l'instant SUIVANT doit rester au-dessus des deux formes.
    vrai(
        &s,
        "'2026-08-24T12:00:00.000000000+02:00' < '2026-08-24T12:00:01+02:00'",
    );
    vrai(
        &s,
        "'2026-08-24T12:00:00.5+02:00' < '2026-08-24T12:00:01+02:00'",
    );

    // Le filtre à la borne : `>= '…T12:00:00+02:00'` doit ramasser les DEUX
    // écritures du même instant, pas seulement la forme exacte.
    assert_eq!(
        keys(
            &s,
            "MATCH (n:D) WHERE n.v >= '2026-08-24T12:00:00+02:00' RETURN n.k"
        ),
        vec!["nano", "sec", "suiv"],
        "un seuil posé sur la forme courte doit inclure la forme à décimales \
         du même instant"
    );
}

// ---------------------------------------------------------------------------
// G. BORNES DES ENTIERS
// ---------------------------------------------------------------------------

#[test]
fn bornes_i64_exactes() {
    let db = base(&[
        "INSERT (:B {k: 'max',   v: 9223372036854775807})",
        "INSERT (:B {k: 'max_1', v: 9223372036854775806})",
        "INSERT (:B {k: 'min',   v: -9223372036854775807})",
        "INSERT (:B {k: 'zero',  v: 0})",
    ])
    .0;
    let s = db.session();

    // À la borne haute, deux entiers consécutifs doivent rester distincts.
    assert_eq!(
        keys(&s, "MATCH (n:B) WHERE n.v = 9223372036854775806 RETURN n.k"),
        vec!["max_1"],
        "i64::MAX et i64::MAX-1 doivent rester discernables"
    );
    assert_eq!(
        keys(&s, "MATCH (n:B) WHERE n.v > 9223372036854775806 RETURN n.k"),
        vec!["max"],
        "> i64::MAX-1 ne doit rendre que i64::MAX"
    );
    assert_eq!(
        keys(
            &s,
            "MATCH (n:B) WHERE n.v >= -9223372036854775807 RETURN n.k"
        )
        .len(),
        4,
        ">= la borne basse doit rendre toutes les lignes"
    );
    // Le zéro n'est ni positif ni négatif : il est exactement sur sa borne.
    assert_eq!(
        keys(&s, "MATCH (n:B) WHERE n.v >= 0 RETURN n.k"),
        vec!["max", "max_1", "zero"],
        ">= 0 doit inclure le zéro lui-même"
    );
    assert_eq!(
        keys(&s, "MATCH (n:B) WHERE n.v > 0 RETURN n.k"),
        vec!["max", "max_1"],
        "> 0 doit exclure le zéro lui-même"
    );
    vrai(&s, "-1 < 0");
    vrai(&s, "0 > -1");
    faux(&s, "-1 > 0");
}

// ---------------------------------------------------------------------------
// H. PROPRIÉTÉ ABSENTE — logique à trois valeurs
// ---------------------------------------------------------------------------

#[test]
fn propriete_absente_ne_satisfait_aucune_comparaison() {
    let db = base(&[
        "INSERT (:P {k: 'sans'})", // ← aucune propriété v
        "INSERT (:P {k: 'avec', v: 5})",
        "INSERT (:P {k: 'autre', v: 9})",
    ])
    .0;
    let s = db.session();

    // Une propriété absente vaut NULL. Toute comparaison rend UNKNOWN, et
    // WHERE ne retient que TRUE : la ligne 'sans' ne doit apparaître NULLE
    // PART — ni dans `=`, ni dans `<>`, ni dans les inégalités, ni dans NOT.
    for pred in [
        "n.v = 5", "n.v <> 5", "n.v < 5", "n.v <= 5", "n.v > 5", "n.v >= 5",
    ] {
        let r = keys(&s, &format!("MATCH (n:P) WHERE {pred} RETURN n.k"));
        assert!(
            !r.contains(&"sans".to_string()),
            "la ligne sans propriété ne doit pas satisfaire `{pred}` (NULL → UNKNOWN), \
             obtenu : {r:?}"
        );
    }
    // NOT (UNKNOWN) reste UNKNOWN : la négation ne doit pas ressusciter la ligne.
    let r = keys(&s, "MATCH (n:P) WHERE NOT (n.v = 5) RETURN n.k");
    assert!(
        !r.contains(&"sans".to_string()),
        "NOT(UNKNOWN) vaut UNKNOWN, pas TRUE : la ligne sans propriété doit rester \
         hors du résultat, obtenu : {r:?}"
    );
    // …mais les lignes qui ONT la propriété doivent, elles, être filtrées.
    assert_eq!(
        keys(&s, "MATCH (n:P) WHERE n.v = 5 RETURN n.k"),
        vec!["avec"],
        "la comparaison doit fonctionner normalement sur les lignes qui ont la propriété"
    );
    assert_eq!(
        keys(&s, "MATCH (n:P) WHERE n.v <> 5 RETURN n.k"),
        vec!["autre"],
        "`<> 5` doit rendre la ligne à 9, et elle seule"
    );
}

// ---------------------------------------------------------------------------
// I. Propagation d'UNKNOWN issu d'une COMPARAISON à travers AND / OR / NOT
//
// (`null_and_coercion.rs` couvre déjà `= NULL`, `<> NULL` et CASE ; le lot 04
// couvre la distinction propriété absente / Value::Null stocké. Ici : ce que
// devient l'UNKNOWN *produit par une comparaison* quand il traverse les
// connecteurs booléens, et la différence WHERE (filtre) / RETURN (projette).)
// ---------------------------------------------------------------------------

#[test]
fn and_avec_unknown_issu_dune_comparaison() {
    let db = base(&["INSERT (:P {k: 'sans'})", "INSERT (:P {k: 'avec', v: 9})"]).0;
    let s = db.session();

    // UNKNOWN AND FALSE = FALSE. Aucune ligne ne passe : ni 'sans'
    // (indéterminé), ni 'avec' (TRUE AND FALSE = FALSE).
    assert_eq!(
        keys(&s, "MATCH (n:P) WHERE (n.v > 5) AND (1 = 2) RETURN n.k"),
        Vec::<String>::new(),
        "UNKNOWN AND FALSE et TRUE AND FALSE valent tous deux FALSE"
    );

    // UNKNOWN AND TRUE = UNKNOWN : 'sans' reste dehors, 'avec' passe car
    // 9 > 5 est vrai.
    assert_eq!(
        keys(&s, "MATCH (n:P) WHERE (n.v > 5) AND (1 = 1) RETURN n.k"),
        vec!["avec"],
        "UNKNOWN AND TRUE vaut UNKNOWN (ligne exclue) ; TRUE AND TRUE vaut \
         TRUE (ligne retenue)"
    );

    // NOT (UNKNOWN) = UNKNOWN : la négation ne ressuscite pas la ligne.
    assert_eq!(
        keys(&s, "MATCH (n:P) WHERE NOT (n.v > 5) RETURN n.k"),
        Vec::<String>::new(),
        "NOT(UNKNOWN) vaut UNKNOWN et NOT(TRUE) vaut FALSE : aucune ligne"
    );
}

#[test]
fn where_filtre_l_unknown_mais_return_doit_le_projeter() {
    let db = base(&["INSERT (:P {k: 'sans'})", "INSERT (:P {k: 'avec', v: 9})"]).0;
    let s = db.session();

    // WHERE ne retient que TRUE.
    assert_eq!(
        keys(&s, "MATCH (n:P) WHERE n.v > 5 RETURN n.k"),
        vec!["avec"],
        "WHERE ne doit retenir que les lignes dont la comparaison vaut TRUE"
    );

    // RETURN, lui, doit PROJETER l'indétermination : NULL, pas false. Confondre
    // les deux fait disparaître la différence entre « comparaison fausse » et
    // « comparaison impossible ».
    let r = s
        .execute("MATCH (n:P) WHERE n.k = 'sans' RETURN n.v > 5")
        .expect("requête refusée");
    assert_eq!(
        r.rows.len(),
        1,
        "la ligne doit être projetée, seul son calcul est indéterminé"
    );
    assert_eq!(
        r.rows[0][0],
        Value::Null,
        "`n.v > 5` sur une propriété absente doit se PROJETER en NULL, \
         pas en FALSE : WHERE filtre l'UNKNOWN, RETURN le montre"
    );
}

// ===========================================================================
// DIVERGENCES — ces tests énoncent l'intention et ÉCHOUENT aujourd'hui.
// Ils sont `#[ignore]`, jamais ajustés au comportement observé.
//   cargo test -p obrain-engine --features gql --test gql_02_comparaison -- --ignored
// ===========================================================================

/// D1 — Une ligne doit être `=` ou `<>` à une valeur donnée, jamais ni l'un ni
/// l'autre. `values_equal` compare les flottants par tolérance absolue
/// `f64::EPSILON` alors que `=` sur propriété reste exact : 1.0e-17 tombe dans
/// le trou entre les deux.
#[test]
// CORRIGÉ par ce commit (correctif E, comparateur canonique value_cmp) — défaut d'origine : DIVERGENCE D1 : `=` (exact sur propriété) et `<>` (tolérance EPSILON)  laissent 1.0e-17 hors des DEUX ensembles — filter.rs:3418
fn tiers_exclu_entre_eq_et_ne_sur_flottants() {
    let db = base(&[
        "INSERT (:E {k: 'zero', v: 0.0})",
        "INSERT (:E {k: 'tiny', v: 1.0e-17})", // ← sous f64::EPSILON, mais ≠ 0
        "INSERT (:E {k: 'eps',  v: 2.220446049250313e-16})",
    ])
    .0;
    let s = db.session();

    let eq = keys(&s, "MATCH (n:E) WHERE n.v = 0.0 RETURN n.k");
    let ne = keys(&s, "MATCH (n:E) WHERE n.v <> 0.0 RETURN n.k");
    assert_eq!(
        eq.len() + ne.len(),
        3,
        "toute ligne non nulle doit être soit `= 0.0` soit `<> 0.0` ; \
         `=` a rendu {eq:?} et `<>` a rendu {ne:?} — il manque une ligne"
    );
    assert!(
        ne.contains(&"tiny".to_string()),
        "1.0e-17 est DIFFÉRENT de 0.0 : `<> 0.0` doit le rendre, obtenu {ne:?}"
    );
    // Et sur littéraux, l'égalité des flottants doit être EXACTE, pas tolérante.
    faux(&s, "1.0e-17 = 0.0");
    vrai(&s, "1.0e-17 <> 0.0");
    vrai(&s, "1.0e-17 > 0.0");
    // Cohérence : `>` et `<>` ne peuvent pas se contredire.
    assert_eq!(
        scalar(&s, "1.0e-17 > 0.0"),
        scalar(&s, "1.0e-17 <> 0.0"),
        "si a > b alors a <> b : les deux opérateurs doivent s'accorder"
    );
}

/// D2 — `<=` est par définition `<` OU `=`. Sur NaN, `<` est faux et `=` est
/// faux, donc `<=` DOIT être faux.
#[test]
// CORRIGÉ par ce commit (correctif E, comparateur canonique value_cmp) — défaut d'origine : DIVERGENCE D2 : `NaN <= NaN` et `NaN >= NaN` rendent VRAI alors que  `NaN < NaN` et `NaN = NaN` rendent FAUX — filter.rs:3529, le `else`  du if/else avale NaN et le déclare Equal
fn le_est_lt_ou_eq_meme_sur_nan() {
    let db = base_entiers();
    let s = db.session();

    faux(&s, "0.0/0.0 <= 0.0/0.0");
    faux(&s, "0.0/0.0 >= 0.0/0.0");
    faux(&s, "0.0/0.0 <= 1.0");
    faux(&s, "0.0/0.0 >= 1.0");
    // L'identité qui doit tenir pour TOUT couple de valeurs :
    // (a <= b) ≡ (a < b) OR (a = b)
    for (a, b) in [
        ("0.0/0.0", "0.0/0.0"),
        ("0.0/0.0", "1.0"),
        ("1.0", "0.0/0.0"),
    ] {
        assert_eq!(
            scalar(&s, &format!("{a} <= {b}")),
            scalar(&s, &format!("({a} < {b}) OR ({a} = {b})")),
            "`{a} <= {b}` doit valoir `({a} < {b}) OR ({a} = {b})`"
        );
    }
}

/// D3 — Deux entiers distincts ne peuvent pas être tous deux égaux au même
/// flottant. Au-delà de 2^53 la conversion `i64 as f64` les confond.
#[test]
// CORRIGÉ par ce commit (correctif E, comparateur canonique value_cmp) — défaut d'origine : DIVERGENCE D3 : 2^53 et 2^53+1, distincts en Int64, sont tous deux  `= 9007199254740992.0` — filter.rs:3420 convertit l'entier en f64
fn egalite_reste_transitive_au_dela_de_2_puissance_53() {
    let db = base(&[
        "INSERT (:B {k: 'p53',   v: 9007199254740992})", // 2^53
        "INSERT (:B {k: 'p53p1', v: 9007199254740993})", // 2^53+1, non représentable en f64
    ])
    .0;
    let s = db.session();

    // Les deux entiers sont distincts…
    vrai(&s, "9007199254740992 <> 9007199254740993");
    // …donc au plus UN d'entre eux peut être égal à un flottant donné.
    let r = keys(&s, "MATCH (n:B) WHERE n.v = 9007199254740992.0 RETURN n.k");
    assert_eq!(
        r,
        vec!["p53"],
        "un seul entier peut être égal à 9007199254740992.0 ; obtenu {r:?} — \
         l'égalité n'est plus transitive"
    );
}

/// D4 — `min()`/`max()` sur une colonne CHAÎNE doivent rendre une CHAÎNE, et
/// s'accorder avec `ORDER BY` sur la même colonne.
#[test]
// CORRIGÉ par ce commit (correctif E, comparateur canonique value_cmp) — défaut d'origine : DIVERGENCE D4 : min()/max() sur colonne CHAÎNE rendent Int64(0)  au lieu d'une chaîne, alors qu'ORDER BY trie correctement
fn min_max_sur_chaines_saccordent_avec_order_by() {
    let db = base(&[
        "INSERT (:N {k: 'dix',  v: '10'})",
        "INSERT (:N {k: 'deux', v: '2'})",
        "INSERT (:N {k: 'neuf', v: '9'})",
    ])
    .0;
    let s = db.session();

    let premier = keys(&s, "MATCH (n:N) RETURN n.v ORDER BY n.v ASC LIMIT 1");
    let dernier = keys(&s, "MATCH (n:N) RETURN n.v ORDER BY n.v DESC LIMIT 1");
    assert_eq!(premier, vec!["10"], "le minimum lexicographique est '10'");
    assert_eq!(dernier, vec!["9"], "le maximum lexicographique est '9'");

    assert_eq!(
        scalar(&s, "1"), // ancre inutilisée, on interroge séparément
        Value::Int64(1)
    );
    let min = s.execute("MATCH (n:N) RETURN min(n.v)").unwrap().rows[0][0].clone();
    let max = s.execute("MATCH (n:N) RETURN max(n.v)").unwrap().rows[0][0].clone();
    assert_eq!(
        min,
        Value::String("10".into()),
        "min() sur une colonne CHAÎNE doit rendre la même valeur qu'ORDER BY … ASC LIMIT 1"
    );
    assert_eq!(
        max,
        Value::String("9".into()),
        "max() sur une colonne CHAÎNE doit rendre la même valeur qu'ORDER BY … DESC LIMIT 1"
    );
}

/// D5 — Une CHAÎNE et un NOMBRE ne sont pas du même type : la comparaison doit
/// rendre NULL, uniformément pour les six opérateurs. Le moteur coerce, et les
/// deux comparateurs qu'il emploie ne coercent pas de la même façon.
#[test]
// CORRIGÉ par ce commit (correctif E, comparateur canonique value_cmp) — défaut d'origine : DIVERGENCE D5 : STRING vs NUMBER est coercé (hack RDF) ; '5.0' est  à la fois >= 5 et <= 5 sans être = 5, et la chaîne 'inf' devient  l'infini IEEE — values_equal:3424 parse i64, compare_values:3541 parse f64
fn chaine_et_nombre_ne_sont_pas_comparables() {
    let db = base_entiers();
    let s = db.session();

    // Types incomparables → UNKNOWN, pour les six opérateurs sans exception.
    for op in ["=", "<>", "<", "<=", ">", ">="] {
        assert_eq!(
            scalar(&s, &format!("'5' {op} 5")),
            Value::Null,
            "comparer la chaîne '5' à l'entier 5 avec `{op}` doit rendre NULL, \
             pas coercer la chaîne en nombre"
        );
        assert_eq!(
            scalar(&s, &format!("'abc' {op} 5")),
            Value::Null,
            "comparer la chaîne 'abc' à l'entier 5 avec `{op}` doit rendre NULL"
        );
    }
    // Et surtout : le moteur ne doit pas se contredire d'un opérateur à l'autre.
    // '5.0' est aujourd'hui >= 5 ET <= 5 tout en n'étant PAS = 5.
    assert_eq!(
        scalar(&s, "'5.0' >= 5 AND '5.0' <= 5"),
        scalar(&s, "'5.0' = 5"),
        "(a >= b AND a <= b) doit valoir (a = b) — quel que soit le typage retenu"
    );
    // La chaîne 'inf' est une chaîne, pas l'infini IEEE.
    assert_eq!(
        scalar(&s, "'inf' > 999999999"),
        Value::Null,
        "la chaîne 'inf' ne doit pas être interprétée comme l'infini flottant"
    );
}

/// D6 — L'infini est une valeur flottante ordinaire et il est égal à lui-même.
/// `values_equal` calcule `(a - b).abs() < f64::EPSILON` : pour `a = b = +inf`
/// la soustraction rend NaN, et toute comparaison avec NaN est fausse. Le
/// moteur déclare donc l'infini différent de lui-même.
#[test]
// CORRIGÉ par ce commit (correctif E, comparateur canonique value_cmp) — défaut d'origine : DIVERGENCE D6 : `1.0/0.0 = 1.0/0.0` rend FAUX — values_equal:3418  calcule (inf - inf).abs(), soit NaN, et NaN < EPSILON est faux
fn infini_est_egal_a_lui_meme() {
    let db = base_entiers();
    let s = db.session();

    vrai(&s, "1.0/0.0 = 1.0/0.0");
    vrai(&s, "-1.0/0.0 = -1.0/0.0");
    faux(&s, "1.0/0.0 <> 1.0/0.0");
    faux(&s, "1.0/0.0 = -1.0/0.0");

    // Et le même trou que D1 : `=` et `<>` doivent partitionner.
    assert_eq!(
        scalar(&s, "1.0/0.0 = 1.0/0.0"),
        scalar(&s, "NOT (1.0/0.0 <> 1.0/0.0)"),
        "`=` et `NOT (<>)` doivent s'accorder, y compris sur l'infini"
    );
}

/// D7 — `UNKNOWN OR TRUE` vaut TRUE : c'est la règle de la logique ternaire, et
/// la branche vraie suffit à elle seule. Le moteur perd pourtant la ligne dont
/// la comparaison est indéterminée, alors que la disjonction est vraie pour
/// TOUTES les lignes sans exception.
#[test]
// CORRIGÉ par ce commit (correctif E, comparateur canonique value_cmp) — défaut d'origine : DÉFAUT D7 — `WHERE (n.v > 5) OR (1 = 1)` perd la ligne dont `n.v`  est absent, alors que la disjonction est vraie partout ; voir atelier  gql02-unknown-or-true-perd-la-ligne
fn unknown_or_true_vaut_true() {
    let db = base(&["INSERT (:P {k: 'sans'})", "INSERT (:P {k: 'avec', v: 9})"]).0;
    let s = db.session();

    // DÉFAUT: rend ["avec"] — la ligne 'sans' est filtrée alors que la branche
    // droite est vraie inconditionnellement.
    assert_eq!(
        keys(&s, "MATCH (n:P) WHERE (n.v > 5) OR (1 = 1) RETURN n.k"),
        vec!["avec", "sans"],
        "UNKNOWN OR TRUE vaut TRUE : les DEUX lignes doivent passer"
    );
    // Le même énoncé, opérandes inversées, doit donner le même ensemble :
    // `OR` est commutatif.
    assert_eq!(
        keys(&s, "MATCH (n:P) WHERE (1 = 1) OR (n.v > 5) RETURN n.k"),
        keys(&s, "MATCH (n:P) WHERE (n.v > 5) OR (1 = 1) RETURN n.k"),
        "`OR` doit être commutatif : l'ordre des opérandes ne change pas le \
         résultat"
    );
    // Et une disjonction dont l'autre branche est un prédicat ordinaire vrai
    // pour la ligne indéterminée doit, elle aussi, la retenir.
    assert_eq!(
        keys(
            &s,
            "MATCH (n:P) WHERE (n.v > 5) OR (n.k = 'sans') RETURN n.k"
        ),
        vec!["avec", "sans"],
        "une branche vraie suffit : la comparaison indéterminée ne doit pas \
         annuler la disjonction"
    );
}

// ---------------------------------------------------------------------------
// J. TABLE DE VÉRITÉ COMPLÈTE autour de l'UNKNOWN produit par une comparaison
//
// Cause connue (remontée par le lot 04, et c'est le cœur de D7) : le dispatch
// `Binary` de `filter.rs` fait `eval_expr(left)?` — une propriété ABSENTE y
// remonte un `None` Rust, et le `?` court-circuite TOUTE l'expression AVANT
// que la table de vérité ternaire ne soit consultée (filter.rs:477-478,
// table :427-455). Le lot 04 en couvre un cas ; ici la table entière, pour
// que le défaut soit cerné et pas seulement constaté.
//
// Le générateur d'UNKNOWN est `n.v > 5` évalué sur la ligne dont `v` est
// absente. On PROJETTE l'expression (RETURN) au lieu de la filtrer (WHERE) :
// la projection montre la valeur ternaire elle-même, sans la confondre avec
// la décision de garder ou non la ligne.
// ---------------------------------------------------------------------------

/// Projette `expr` sur la ligne `:P {k: 'sans'}`, dont `v` est ABSENTE.
fn ternaire(s: &Session, expr: &str) -> Value {
    let query = format!("MATCH (n:P) WHERE n.k = 'sans' RETURN {expr}");
    let r = s
        .execute(&query)
        .unwrap_or_else(|e| panic!("requête refusée :\n  {query}\n  → {e}"));
    assert_eq!(
        r.rows.len(),
        1,
        "la ligne 'sans' doit être projetée quelle que soit la valeur de \
         l'expression : {query}"
    );
    r.rows[0][0].clone()
}

fn base_ternaire() -> ObrainDB {
    base(&["INSERT (:P {k: 'sans'})", "INSERT (:P {k: 'avec', v: 9})"]).0
}

/// `T` = `(1 = 1)`, `F` = `(1 = 2)`, `U` = `(n.v > 5)` sur la ligne sans `v`.
const T: &str = "(1 = 1)";
const F: &str = "(1 = 2)";
const U: &str = "(n.v > 5)";

/// Vérifie une table entière et rapporte TOUS les écarts d'un coup — un seul
/// écart signalé masquerait les autres.
fn verifier_table(s: &Session, cas: &[(String, Value, &str)]) {
    let mut ecarts = Vec::new();
    for (expr, attendu, legende) in cas {
        let obtenu = ternaire(s, expr);
        if obtenu != *attendu {
            ecarts.push(format!(
                "  {legende:<24} `{expr}`\n      attendu {attendu:?}, obtenu {obtenu:?}"
            ));
        }
    }
    assert!(
        ecarts.is_empty(),
        "{} écart(s) à la table de vérité ternaire :\n{}",
        ecarts.len(),
        ecarts.join("\n")
    );
}

#[test]
fn table_de_verite_and_hors_absorption() {
    let db = base_ternaire();
    let s = db.session();
    verifier_table(
        &s,
        &[
            (format!("{T} AND {T}"), Value::Bool(true), "T AND T = T"),
            (format!("{T} AND {F}"), Value::Bool(false), "T AND F = F"),
            (format!("{F} AND {T}"), Value::Bool(false), "F AND T = F"),
            (format!("{F} AND {F}"), Value::Bool(false), "F AND F = F"),
            (format!("{U} AND {T}"), Value::Null, "U AND T = U"),
            (format!("{T} AND {U}"), Value::Null, "T AND U = U"),
            (format!("{U} AND {U}"), Value::Null, "U AND U = U"),
        ],
    );
}

#[test]
fn table_de_verite_or_hors_absorption() {
    let db = base_ternaire();
    let s = db.session();
    verifier_table(
        &s,
        &[
            (format!("{T} OR {T}"), Value::Bool(true), "T OR T = T"),
            (format!("{T} OR {F}"), Value::Bool(true), "T OR F = T"),
            (format!("{F} OR {T}"), Value::Bool(true), "F OR T = T"),
            (format!("{F} OR {F}"), Value::Bool(false), "F OR F = F"),
            (format!("{U} OR {F}"), Value::Null, "U OR F = U"),
            (format!("{F} OR {U}"), Value::Null, "F OR U = U"),
            (format!("{U} OR {U}"), Value::Null, "U OR U = U"),
        ],
    );
}

/// D7 — L'ABSORPTION est le seul endroit où la table ternaire se distingue
/// d'une simple propagation : `FALSE AND ?` vaut FALSE et `TRUE OR ?` vaut
/// TRUE **sans qu'il soit besoin de connaître l'autre opérande**. Le moteur
/// ne les honore pas : une propriété ABSENTE dans l'une ou l'autre branche
/// rend l'expression ENTIÈRE indéterminée.
///
/// Les quatre cas échouent, et dans LES DEUX ORDRES d'opérandes — ce n'est
/// donc pas un court-circuit gauche-droite mais une perte inconditionnelle :
/// le dispatch `Binary` de `filter.rs` évalue ses deux opérandes avec `?`
/// (`filter.rs:477-478`), et une propriété absente y remonte un `None` Rust
/// qui sort de la fonction AVANT que la table de vérité (`filter.rs:427-455`)
/// ne soit consultée.
///
/// Conséquence directe et mesurée : `WHERE (n.v > 5) OR (1 = 1)` perd la
/// ligne dont `v` est absente, alors que la disjonction est vraie partout.
#[test]
// CORRIGÉ par ce commit (correctif E, comparateur canonique value_cmp) — défaut d'origine : DÉFAUT D7 — l'absorption ternaire est perdue : `U AND F` et `U OR T`  rendent NULL au lieu de FALSE et TRUE, dans les deux ordres  d'opérandes ; voir atelier gql02-unknown-or-true-perd-la-ligne
fn table_de_verite_absorption_par_false_et_true() {
    let db = base_ternaire();
    let s = db.session();
    verifier_table(
        &s,
        &[
            // DÉFAUT: rendent Null au lieu de Bool(false).
            (format!("{U} AND {F}"), Value::Bool(false), "U AND F = F"),
            (format!("{F} AND {U}"), Value::Bool(false), "F AND U = F"),
            // DÉFAUT: rendent Null au lieu de Bool(true).
            (format!("{U} OR {T}"), Value::Bool(true), "U OR T = T"),
            (format!("{T} OR {U}"), Value::Bool(true), "T OR U = T"),
        ],
    );
}

#[test]
fn table_de_verite_not_avec_unknown() {
    let db = base_ternaire();
    let s = db.session();
    verifier_table(
        &s,
        &[
            (format!("NOT {T}"), Value::Bool(false), "NOT T = F"),
            (format!("NOT {F}"), Value::Bool(true), "NOT F = T"),
            (format!("NOT {U}"), Value::Null, "NOT U = U"),
        ],
    );
}

#[test]
fn where_applique_la_meme_table_que_return() {
    // Test de COHÉRENCE entre deux chemins, pas de valeur : quelle que soit
    // la valeur ternaire que le moteur calcule, WHERE doit retenir la ligne
    // si et seulement si RETURN la projette en TRUE. Il ne fige donc aucun
    // comportement — il échouerait si les deux chemins divergeaient, ce qui
    // est précisément ce qu'on veut interdire. (Il passe : la perte de ligne
    // de D7 s'explique ENTIÈREMENT par la projection à NULL, sans défaut
    // supplémentaire côté filtre.)
    let db = base_ternaire();
    let s = db.session();

    for (expr, legende) in [
        (format!("{U} OR {T}"), "U OR T"),
        (format!("{U} AND {F}"), "U AND F"),
        (format!("{U} OR {F}"), "U OR F"),
        (format!("NOT {U}"), "NOT U"),
    ] {
        let projete = ternaire(&s, &expr);
        let filtre = keys(
            &s,
            &format!("MATCH (n:P) WHERE n.k = 'sans' AND ({expr}) RETURN n.k"),
        );
        let attendu: Vec<String> = if projete == Value::Bool(true) {
            vec!["sans".to_string()]
        } else {
            Vec::new()
        };
        assert_eq!(
            filtre, attendu,
            "`{legende}` se projette en {projete:?} : WHERE doit retenir la ligne \
             si et seulement si la projection vaut TRUE"
        );
    }
}
