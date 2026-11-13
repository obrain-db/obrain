//! LE COMPARATEUR CANONIQUE — l'unique définition de l'ordre et de l'égalité
//! entre deux [`Value`], pour TOUS les chemins de plan (correctif E, campagne
//! de correction 2026-08).
//!
//! La campagne de conformité a mesuré QUATRE dialectes de comparaison qui se
//! contredisaient (balayage par intervalle, évaluateur, tri, égalité
//! planifiée) : la même requête recevait deux réponses selon le plan choisi
//! (note 5e3ac92c). Ce module est désormais la seule autorité ; toute autre
//! implémentation doit déléguer ici ou disparaître.
//!
//! ── LA CONCEPTION, EN QUATRE RÈGLES ──
//!
//! 1. PRÉDICATS (`<`, `<=`, `>`, `>=`) — [`comparer`] : une comparaison n'est
//!    définie QUE dans une classe de comparabilité (booléens, nombres,
//!    chaînes, chaque type temporel). Deux classes disjointes rendent
//!    [`Comparaison::Incomparables`] → le prédicat vaut UNKNOWN (logique
//!    trivaluée, ISO/IEC 39075 §21). AUCUNE coercition chaîne↔nombre : le
//!    hack RDF (« les littéraux numériques RDF vivent dans des chaînes »)
//!    reste confiné au planificateur RDF (`planner/rdf/mod.rs`), il ne fuit
//!    plus dans LPG.
//! 2. NOMBRES — Int64 et Float64 forment UNE classe, comparée par VALEUR
//!    EXACTE ([`cmp_i64_f64`]) : pas de passage par `as f64` pour les entiers
//!    hors plage sûre — `2^53` et `2^53 + 1` restent distincts face à
//!    `9007199254740992.0`.
//! 3. NaN — au PRÉDICAT, NaN est [`Comparaison::NonOrdonnees`] avec tout
//!    nombre, lui-même compris : `<`, `<=`, `>`, `>=`, `=` rendent FAUX et
//!    `<>` rend VRAI (sémantique IEEE 754, cohérence `<=` ≡ `<` OU `=`).
//!    AU TRI ([`comparer_total`]), NaN a une place FIXE : PLUS GRAND que tout
//!    nombre (infinis compris) et ÉGAL à lui-même. L'égalité de `=` sur les
//!    flottants est l'égalité IEEE STRICTE — plus jamais d'EPSILON absolu :
//!    `1.0e-17 <> 0.0` et `inf = inf`.
//! 4. TRI — [`comparer_total`] est un VRAI ordre total sur TOUTES les
//!    variantes : par RANG de classe d'abord (Bool < nombres < String <
//!    Timestamp < Date < Time < ZonedDatetime < Duration < Bytes < List <
//!    Map < Vector < Path < Null), par valeur ensuite. Les rangs prolongent
//!    `OrderableValue::type_ordinal` (l'index de plage) et Null est PLUS
//!    GRAND que tout — la convention de la vague 1, alignée production.
//!
//! Les tests gardés des lots 02, 05, 09, 12 et 15 font foi : chaque règle
//! ci-dessus est celle qu'exigent leurs assertions.

use std::cmp::Ordering;

use super::value::Value;

/// Verdict d'une comparaison de prédicat GQL entre deux valeurs non nulles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Comparaison {
    /// Les deux valeurs appartiennent à la même classe et s'ordonnent.
    Ordonnees(Ordering),
    /// Même classe mais pas d'ordre (NaN) : les prédicats d'ordre et `=`
    /// rendent FAUX, `<>` rend VRAI.
    NonOrdonnees,
    /// Classes disjointes (ou un opérande NULL) : le prédicat vaut UNKNOWN.
    Incomparables,
}

/// Compare exactement un `i64` et un `f64`, sans perte de précision.
///
/// `None` si et seulement si `f` est NaN. La conversion naïve `i as f64`
/// arrondit au-delà de 2^53 ; ici l'entier reste exact : on tranche d'abord
/// par les plages, puis on compare la partie entière tronquée de `f` (exacte
/// en `i64` une fois les bornes écartées) et enfin sa partie fractionnaire.
#[must_use]
pub fn cmp_i64_f64(i: i64, f: f64) -> Option<Ordering> {
    if f.is_nan() {
        return None;
    }
    // 2^63 : premier f64 strictement au-dessus de i64::MAX. (i64::MAX lui-même
    // n'est pas représentable ; i64::MIN = -2^63 l'est.)
    if f >= 9_223_372_036_854_775_808.0 {
        return Some(Ordering::Less); // couvre aussi +inf
    }
    if f < -9_223_372_036_854_775_808.0 {
        return Some(Ordering::Greater); // couvre aussi -inf
    }
    let tronque = f.trunc();
    // Sûr : tronque ∈ [-2^63, 2^63) et est entier → conversion exacte.
    #[allow(clippy::cast_possible_truncation)]
    let tronque_i = tronque as i64;
    match i.cmp(&tronque_i) {
        Ordering::Equal => {
            // f = tronque + frac, frac ∈ (-1, 1) et du signe de f.
            let frac = f - tronque;
            if frac > 0.0 {
                Some(Ordering::Less) // i = trunc(f) < f
            } else if frac < 0.0 {
                Some(Ordering::Greater)
            } else {
                Some(Ordering::Equal)
            }
        }
        autre => Some(autre),
    }
}

/// Comparaison de PRÉDICAT (`<`, `<=`, `>`, `>=`) — voir le pavé du module.
///
/// Ne coerce RIEN hors de la classe numérique Int64↔Float64. Un opérande
/// NULL rend [`Comparaison::Incomparables`] (UNKNOWN), comme deux classes
/// disjointes.
#[must_use]
#[allow(clippy::many_single_char_names)] // a, b (opérandes) déstructurés par variante, x, y, o : notation de comparateur
pub fn comparer(a: &Value, b: &Value) -> Comparaison {
    use Comparaison::{Incomparables, NonOrdonnees, Ordonnees};
    match (a, b) {
        (Value::Bool(a), Value::Bool(b)) => Ordonnees(a.cmp(b)),
        (Value::Int64(a), Value::Int64(b)) => Ordonnees(a.cmp(b)),
        (Value::Float64(a), Value::Float64(b)) => match a.partial_cmp(b) {
            Some(o) => Ordonnees(o),
            None => NonOrdonnees, // au moins un NaN
        },
        (Value::Int64(i), Value::Float64(f)) => match cmp_i64_f64(*i, *f) {
            Some(o) => Ordonnees(o),
            None => NonOrdonnees,
        },
        (Value::Float64(f), Value::Int64(i)) => match cmp_i64_f64(*i, *f) {
            Some(o) => Ordonnees(o.reverse()),
            None => NonOrdonnees,
        },
        // Deux chaînes datetime RFC3339 à décalage explicite s'ORDONNENT par
        // INSTANT d'abord (gql_15 D6bis/D7) : `'…10:00:00Z' < '….000001Z'`
        // est vrai quel que soit le rang lexicographique de `Z`. À instant
        // ÉGAL, l'ordre reste STRICT sur les écritures distinctes (départage
        // lexicographique) : `'…00+02:00' < '…00.000000000+02:00'` — la
        // stabilité qu'exige gql_02 (rfc3339_a_precision_variable). C'est un
        // ordre produit (instant, chaîne), donc total et transitif sur les
        // chaînes datetime. L'ÉGALITÉ `=`, elle, confond les écritures d'un
        // même instant ([`egaler`], gql_15 D12). Toute autre paire de chaînes
        // reste lexicographique — y compris une chaîne datetime face à une
        // chaîne ordinaire (elles restent des chaînes).
        (Value::String(a), Value::String(b)) => {
            match (instant_de_chaine(a), instant_de_chaine(b)) {
                (Some(x), Some(y)) => Ordonnees(x.cmp(&y).then_with(|| a.cmp(b))),
                _ => Ordonnees(a.cmp(b)),
            }
        }
        (Value::Timestamp(a), Value::Timestamp(b)) => Ordonnees(a.cmp(b)),
        (Value::Date(a), Value::Date(b)) => Ordonnees(a.cmp(b)),
        (Value::Time(a), Value::Time(b)) => Ordonnees(a.cmp(b)),
        (Value::ZonedDatetime(a), Value::ZonedDatetime(b)) => Ordonnees(a.cmp(b)),
        // Duration n'est que partiellement ordonnée (P1M vs P30D) : ordonnée
        // quand ses composantes s'accordent, NonOrdonnees sinon.
        (Value::Duration(a), Value::Duration(b)) => match a.partial_cmp(b) {
            Some(o) => Ordonnees(o),
            None => NonOrdonnees,
        },
        (Value::Bytes(a), Value::Bytes(b)) => Ordonnees(a.cmp(b)),
        // Null, classes disjointes, conteneurs : pas d'ordre de prédicat.
        _ => Incomparables,
    }
}

/// L'ÉGALITÉ de l'opérateur `=` : `Some(true)`/`Some(false)` quand elle est
/// décidée, `None` (UNKNOWN) quand les classes sont disjointes ou qu'un
/// opérande est NULL.
///
/// Flottants : égalité IEEE stricte (NaN ≠ NaN, `inf = inf`, `-0.0 = 0.0`).
/// Int64↔Float64 : égalité numérique EXACTE. Conteneurs (LIST, MAP, PATH) :
/// égalité STRUCTURELLE décidée — `[1] = ['1']` rend FAUX, pas UNKNOWN (spec
/// des tests gardés gql_12), un élément NULL rend l'égalité indécidable
/// seulement si le reste ne tranche pas déjà.
#[must_use]
#[allow(clippy::many_single_char_names)] // a, b (opérandes) déstructurés par variante, x, y, i, j : notation de comparateur
pub fn egaler(a: &Value, b: &Value) -> Option<bool> {
    match (a, b) {
        (Value::Null, _) | (_, Value::Null) => None,
        (Value::List(a), Value::List(b)) => {
            Some(a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| egal_element(x, y)))
        }
        (Value::Map(a), Value::Map(b)) => Some(
            a.len() == b.len()
                && a.iter()
                    .zip(b.iter())
                    .all(|((k1, v1), (k2, v2))| k1 == k2 && egal_element(v1, v2)),
        ),
        (
            Value::Path {
                nodes: n1,
                edges: e1,
            },
            Value::Path {
                nodes: n2,
                edges: e2,
            },
        ) => Some(
            n1.len() == n2.len()
                && e1.len() == e2.len()
                && n1.iter().zip(n2.iter()).all(|(x, y)| egal_element(x, y))
                && e1.iter().zip(e2.iter()).all(|(x, y)| egal_element(x, y)),
        ),
        (Value::Vector(a), Value::Vector(b)) => Some(a == b),
        // Un CONTENEUR face à une autre variante : l'égalité est DÉCIDÉE et
        // fausse — `[1] = 1` rend FAUX (test vert gql_12, LIST et INTEGER
        // sont deux types), là où deux SCALAIRES de classes disjointes
        // rendent UNKNOWN (spec gql_02 D5).
        (Value::List(_) | Value::Map(_) | Value::Path { .. } | Value::Vector(_), _)
        | (_, Value::List(_) | Value::Map(_) | Value::Path { .. } | Value::Vector(_)) => {
            Some(false)
        }
        // Deux chaînes datetime RFC3339 : l'égalité est celle des INSTANTS
        // (gql_15 D12) — `'…10:00:00Z' = '…12:00:00+02:00'` est VRAI. Le
        // même couple reste STRICTEMENT ordonné par [`comparer`] (départage
        // lexicographique, exigence gql_02) : `=` est ici plus grossière que
        // l'équivalence de l'ordre des prédicats, et coïncide avec celle du
        // tri ([`comparer_total`]) et d'[`egalite_structurelle`].
        (Value::String(x), Value::String(y)) => {
            Some(match (instant_de_chaine(x), instant_de_chaine(y)) {
                (Some(i), Some(j)) => i == j,
                _ => x == y,
            })
        }
        _ => match comparer(a, b) {
            Comparaison::Ordonnees(o) => Some(o == Ordering::Equal),
            Comparaison::NonOrdonnees => Some(false), // NaN = x → FAUX
            Comparaison::Incomparables => None,       // classes disjointes → UNKNOWN
        },
    }
}

/// Égalité d'ÉLÉMENT de conteneur : décidée (jamais UNKNOWN). Deux éléments
/// de classes disjointes sont simplement DIFFÉRENTS ; NULL n'est égal qu'à
/// NULL (sémantique structurelle, comme DISTINCT).
fn egal_element(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Null, _) | (_, Value::Null) => false,
        _ => egaler(a, b) == Some(true),
    }
}

/// Égalité STRUCTURELLE pour DISTINCT, GROUP BY, `IN`, CASE simple, NULLIF :
/// toujours décidée, `NULL = NULL` est VRAI (classes d'équivalence), NaN est
/// égal à LUI-MÊME (une valeur appartient à sa propre classe), `-0.0 = 0.0`,
/// Int64↔Float64 par valeur exacte, et AUCUNE coercition chaîne↔nombre.
#[must_use]
pub fn egalite_structurelle(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Null, _) | (_, Value::Null) => false,
        (Value::Float64(x), Value::Float64(y)) => x == y || (x.is_nan() && y.is_nan()),
        (Value::List(a), Value::List(b)) => {
            a.len() == b.len()
                && a.iter()
                    .zip(b.iter())
                    .all(|(x, y)| egalite_structurelle(x, y))
        }
        (Value::Map(a), Value::Map(b)) => {
            a.len() == b.len()
                && a.iter()
                    .zip(b.iter())
                    .all(|((k1, v1), (k2, v2))| k1 == k2 && egalite_structurelle(v1, v2))
        }
        (
            Value::Path {
                nodes: n1,
                edges: e1,
            },
            Value::Path {
                nodes: n2,
                edges: e2,
            },
        ) => {
            n1.len() == n2.len()
                && e1.len() == e2.len()
                && n1
                    .iter()
                    .zip(n2.iter())
                    .all(|(x, y)| egalite_structurelle(x, y))
                && e1
                    .iter()
                    .zip(e2.iter())
                    .all(|(x, y)| egalite_structurelle(x, y))
        }
        _ => egaler(a, b) == Some(true),
    }
}

/// Rang de classe pour l'ordre total — prolonge `OrderableValue::type_ordinal`
/// (Bool < nombres < String < temporels), qui gouverne déjà l'index de plage,
/// puis place les variantes restantes, Null en DERNIER (« null plus grand »,
/// convention de la vague 1 alignée sur la production).
const fn rang_de_classe(v: &Value) -> u8 {
    match v {
        Value::Bool(_) => 0,
        Value::Int64(_) | Value::Float64(_) => 1, // UNE classe numérique
        Value::String(_) => 2,
        Value::Timestamp(_) => 3,
        Value::Date(_) => 4,
        Value::Time(_) => 5,
        Value::ZonedDatetime(_) => 6,
        Value::Duration(_) => 7,
        Value::Bytes(_) => 8,
        Value::List(_) => 9,
        Value::Map(_) => 10,
        Value::Vector(_) => 11,
        Value::Path { .. } => 12,
        Value::Null => 13,
    }
}

/// Instant UTC EXACT (en NANOSECONDES depuis l'époque Unix) d'une chaîne
/// RFC3339 datetime À DÉCALAGE EXPLICITE (`2026-01-01T10:00:00Z`,
/// `…+02:00`) — `None` pour toute autre chaîne.
///
/// ── EXACT : NI SATURATION, NI TRONCATURE ──
///
/// Cet instant décide de l'ÉGALITÉ de deux chaînes datetime ([`egaler`]) et
/// de leur seau dans l'index d'égalité du substrat : deux dates DISTINCTES ne
/// doivent jamais rendre le même instant. L'ancienne version passait par
/// `Timestamp` (nanosecondes sur `i64`, qui SATURE hors de 1677 → 2262) puis
/// par `as_micros` (qui TRONQUE) : `3000-01-01T00:00:00Z` égalait
/// `9999-12-31T23:59:59Z`, et `…00.0000001Z` égalait `…00.0000002Z`. On
/// calcule donc ici sur `i128`, depuis la date et l'heure analysées, sans
/// passer par un type qui borne.
///
/// La précision est la nanoseconde (neuf décimales). Une fraction qui porte
/// un chiffre NON NUL au-delà de la neuvième décimale n'a pas d'instant
/// exact ici : la fonction rend `None`, et la chaîne se compare alors comme
/// une chaîne ordinaire — distincte de toute autre écriture, jamais égale par
/// arrondi.
///
/// Sert le PLONGEMENT TEMPOREL de l'ordre total : la production stocke des
/// instants dans des chaînes (`to_rfc3339()`), et une colonne qui mélange
/// horodatages typés et dates en chaîne doit se trier CHRONOLOGIQUEMENT
/// (test gardé gql_15 D5). Une telle chaîne se classe donc, AU TRI SEULEMENT,
/// dans la classe Timestamp (rang 3), par son instant. C'est une fonction de
/// la VALEUR — pas de la paire — donc l'ordre reste total et transitif. Les
/// PRÉDICATS (`<`, `=`) plongent AUSSI, mais seulement entre DEUX chaînes
/// datetime (correctif vague 4a, gql_15 D6bis/D7/D12) : le verdict d'un
/// prédicat ne dépend plus de l'orthographe du décalage. Une chaîne datetime
/// face à une chaîne ordinaire reste lexicographique aux prédicats (même
/// classe : les chaînes) — l'ordre n'y est alors transitif qu'à corpus
/// homogène, ce que la production garantit (to_rfc3339 uniforme).
///
/// Publique : le balayage par intervalle du substrat
/// (`valeur_dans_intervalle`) applique la même règle, et l'index ordonné se
/// DÉSISTE quand une borne est une chaîne datetime (sa projection sur 11
/// octets n'est monotone que pour l'ordre lexicographique).
///
/// Le reniflage écarte à coût quasi nul toute chaîne qui n'a pas la forme
/// `AAAA-MM-JJT…` ; seules les chaînes d'allure datetime paient le parse.
#[must_use]
pub fn instant_de_chaine(s: &str) -> Option<i128> {
    let b = s.as_bytes();
    if b.len() < 16 || b[4] != b'-' || b[7] != b'-' || (b[10] != b'T' && b[10] != b't') {
        return None;
    }
    if !(b[0].is_ascii_digit()
        && b[1].is_ascii_digit()
        && b[2].is_ascii_digit()
        && b[3].is_ascii_digit()
        && b[5].is_ascii_digit()
        && b[6].is_ascii_digit()
        && b[8].is_ascii_digit()
        && b[9].is_ascii_digit())
    {
        return None;
    }
    let (date, heure) = (&s[..10], &s[11..]);
    let date = super::Date::parse(date)?;
    let temps = super::Time::parse(heure)?;
    // Décalage EXPLICITE exigé (même règle que `ZonedDatetime::parse`).
    let decalage = temps.offset_seconds()?;
    if fraction_au_dela_de_la_nanoseconde(heure) {
        return None;
    }
    Some(
        i128::from(date.as_days()) * 86_400_000_000_000 + i128::from(temps.as_nanos())
            - i128::from(decalage) * 1_000_000_000,
    )
}

/// Vrai si la partie fractionnaire des secondes porte un chiffre non nul
/// APRÈS la neuvième décimale — une précision que `Time` (nanosecondes) ne
/// conserve pas.
fn fraction_au_dela_de_la_nanoseconde(heure: &str) -> bool {
    let Some(point) = heure.find('.') else {
        return false;
    };
    heure[point + 1..]
        .bytes()
        .take_while(u8::is_ascii_digit)
        .skip(9)
        .any(|c| c != b'0')
}

/// L'entier `i` tel que `Int64(i)` ÉGALE `Float64(f)` au sens de ce
/// comparateur, s'il existe (`None` pour un flottant non entier, hors de la
/// plage `i64`, infini ou NaN ; `-0.0` rend `0`).
///
/// C'est la CLASSE D'ÉQUIVALENCE numérique de l'égalité, exposée pour les
/// structures qui rangent des valeurs par clé (index d'égalité du substrat,
/// sonde d'index des jointures) : elles doivent ranger `Float64(5.0)` avec
/// `Int64(5)`. Le verdict est rendu par [`cmp_i64_f64`] lui-même, pas par une
/// règle redéduite : la clé ne peut pas diverger de l'égalité.
#[must_use]
pub fn entier_egal_au_flottant(f: f64) -> Option<i64> {
    // Conversion SATURANTE (NaN → 0) : elle ne décide de rien, elle propose
    // le seul candidat possible, que `cmp_i64_f64` confirme ou rejette.
    #[allow(clippy::cast_possible_truncation)]
    let candidat = f as i64;
    (cmp_i64_f64(candidat, f) == Some(Ordering::Equal)).then_some(candidat)
}

/// Rang effectif d'une valeur dans l'ordre total : celui de sa classe, sauf
/// pour une chaîne datetime RFC3339, plongée dans la classe Timestamp.
fn rang_effectif(v: &Value) -> u8 {
    match v {
        Value::String(s) if instant_de_chaine(s).is_some() => 3,
        _ => rang_de_classe(v),
    }
}

/// Ordre total sur les flottants : IEEE pour les ordonnés, NaN PLUS GRAND que
/// tout (infinis compris) et égal à lui-même.
fn total_f64(a: f64, b: f64) -> Ordering {
    a.partial_cmp(&b).unwrap_or_else(|| {
        match (a.is_nan(), b.is_nan()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            // partial_cmp n'échoue que sur NaN.
            (false, false) => unreachable!("partial_cmp(f64) n'échoue que sur NaN"),
        }
    })
}

/// L'ORDRE TOTAL canonique — tri (`ORDER BY`), incumbents de MIN/MAX, fusions
/// de tri externes. Total sur TOUTES les variantes : par rang de classe
/// d'abord (`rang_de_classe`, privé), par valeur dans la classe ensuite.
///
/// C'est un ordre (réflexif, antisymétrique, transitif) au sens des classes
/// d'équivalence de [`egalite_structurelle`] : `Equal` n'est rendu que pour
/// des valeurs interchangeables au tri (dont `Int64(1)`/`Float64(1.0)`,
/// `-0.0`/`0.0`, NaN/NaN).
#[must_use]
#[allow(clippy::many_single_char_names)] // a, b (opérandes) déstructurés par variante, x, y : notation de comparateur
pub fn comparer_total(a: &Value, b: &Value) -> Ordering {
    match (a, b) {
        (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
        (Value::Int64(a), Value::Int64(b)) => a.cmp(b),
        (Value::Float64(a), Value::Float64(b)) => total_f64(*a, *b),
        // Classe numérique : par valeur exacte ; NaN plus grand que tout
        // nombre, donc plus grand que tout Int64.
        (Value::Int64(i), Value::Float64(f)) => cmp_i64_f64(*i, *f).unwrap_or(Ordering::Less),
        (Value::Float64(f), Value::Int64(i)) => {
            cmp_i64_f64(*i, *f).map_or(Ordering::Greater, Ordering::reverse)
        }
        // Chaînes : lexicographique — SAUF deux chaînes datetime RFC3339,
        // comparées par INSTANT (plongement temporel, cf. instant_de_chaine).
        // Même instant sous deux écritures → Equal (pas de départage
        // lexicographique : Timestamp(x) est Equal aux deux, l'équivalence
        // doit rester transitive). Une chaîne datetime face à une chaîne
        // ordinaire relève des rangs : 3 contre 2.
        (Value::String(a), Value::String(b)) => {
            match (instant_de_chaine(a), instant_de_chaine(b)) {
                (Some(x), Some(y)) => x.cmp(&y),
                (Some(_), None) => Ordering::Greater,
                (None, Some(_)) => Ordering::Less,
                (None, None) => a.cmp(b),
            }
        }
        // Chaîne datetime contre horodatage typé : par instant, dans la même
        // classe de rang 3. Chaîne ordinaire : rang 2 < rang 3.
        (Value::String(s), Value::Timestamp(t)) => match instant_de_chaine(s) {
            Some(x) => x.cmp(&i128::from(t.as_nanos())),
            None => Ordering::Less,
        },
        (Value::Timestamp(t), Value::String(s)) => match instant_de_chaine(s) {
            Some(x) => i128::from(t.as_nanos()).cmp(&x),
            None => Ordering::Greater,
        },
        (Value::Timestamp(a), Value::Timestamp(b)) => a.cmp(b),
        (Value::Date(a), Value::Date(b)) => a.cmp(b),
        (Value::Time(a), Value::Time(b)) => a.cmp(b),
        (Value::ZonedDatetime(a), Value::ZonedDatetime(b)) => a.cmp(b),
        // Duration : l'ordre partiel quand il tranche, sinon la clé
        // structurelle (months, days, nanos) — déterministe et documentée.
        (Value::Duration(a), Value::Duration(b)) => a
            .partial_cmp(b)
            .unwrap_or_else(|| a.cle_de_tri().cmp(&b.cle_de_tri())),
        (Value::Bytes(a), Value::Bytes(b)) => a.cmp(b),
        (Value::List(a), Value::List(b)) => comparer_sequences(a, b),
        (Value::Map(a), Value::Map(b)) => {
            for ((k1, v1), (k2, v2)) in a.iter().zip(b.iter()) {
                match k1.cmp(k2).then_with(|| comparer_total(v1, v2)) {
                    Ordering::Equal => {}
                    autre => return autre,
                }
            }
            a.len().cmp(&b.len())
        }
        (Value::Vector(a), Value::Vector(b)) => {
            for (x, y) in a.iter().zip(b.iter()) {
                let o = x
                    .partial_cmp(y)
                    .unwrap_or_else(|| match (x.is_nan(), y.is_nan()) {
                        (true, true) => Ordering::Equal,
                        (true, false) => Ordering::Greater,
                        _ => Ordering::Less,
                    });
                if o != Ordering::Equal {
                    return o;
                }
            }
            a.len().cmp(&b.len())
        }
        (
            Value::Path {
                nodes: n1,
                edges: e1,
            },
            Value::Path {
                nodes: n2,
                edges: e2,
            },
        ) => comparer_sequences(n1, n2).then_with(|| comparer_sequences(e1, e2)),
        (Value::Null, Value::Null) => Ordering::Equal,
        // Classes disjointes : le rang tranche, toujours — le rang EFFECTIF,
        // pour que la chaîne datetime garde son rang 3 face à Date, Time,
        // ZonedDatetime… (transitivité du plongement).
        _ => rang_effectif(a).cmp(&rang_effectif(b)),
    }
}

/// Ordre lexicographique élément par élément, longueur en départage.
fn comparer_sequences(a: &[Value], b: &[Value]) -> Ordering {
    for (x, y) in a.iter().zip(b.iter()) {
        match comparer_total(x, y) {
            Ordering::Equal => {}
            autre => return autre,
        }
    }
    a.len().cmp(&b.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn i64_f64_exact_au_dela_de_2p53() {
        // 2^53 = 9007199254740992 ; 2^53 + 1 n'est pas représentable en f64.
        assert_eq!(
            cmp_i64_f64(9_007_199_254_740_992, 9_007_199_254_740_992.0),
            Some(Ordering::Equal)
        );
        assert_eq!(
            cmp_i64_f64(9_007_199_254_740_993, 9_007_199_254_740_992.0),
            Some(Ordering::Greater)
        );
        assert_eq!(
            cmp_i64_f64(i64::MAX, 9_223_372_036_854_775_808.0),
            Some(Ordering::Less)
        );
        assert_eq!(
            cmp_i64_f64(i64::MIN, -9_223_372_036_854_775_808.0),
            Some(Ordering::Equal)
        );
        assert_eq!(cmp_i64_f64(0, f64::INFINITY), Some(Ordering::Less));
        assert_eq!(cmp_i64_f64(0, f64::NEG_INFINITY), Some(Ordering::Greater));
        assert_eq!(cmp_i64_f64(3, 3.5), Some(Ordering::Less));
        assert_eq!(cmp_i64_f64(-3, -3.5), Some(Ordering::Greater));
        assert_eq!(cmp_i64_f64(0, f64::NAN), None);
    }

    #[test]
    fn chaine_et_nombre_incomparables() {
        let s = Value::String("5".into());
        let i = Value::Int64(5);
        assert_eq!(comparer(&s, &i), Comparaison::Incomparables);
        assert_eq!(egaler(&s, &i), None);
        // 'inf' est une chaîne, pas l'infini IEEE.
        assert_eq!(
            comparer(&Value::String("inf".into()), &Value::Int64(999_999_999)),
            Comparaison::Incomparables
        );
    }

    #[test]
    fn nan_non_ordonne_au_predicat_place_fixe_au_tri() {
        let nan = Value::Float64(f64::NAN);
        let un = Value::Float64(1.0);
        assert_eq!(comparer(&nan, &nan), Comparaison::NonOrdonnees);
        assert_eq!(comparer(&nan, &un), Comparaison::NonOrdonnees);
        assert_eq!(egaler(&nan, &nan), Some(false));
        // Au tri : NaN > inf > 1.0 > Int64(1) == Float64(1.0), NaN == NaN.
        assert_eq!(comparer_total(&nan, &nan), Ordering::Equal);
        assert_eq!(
            comparer_total(&nan, &Value::Float64(f64::INFINITY)),
            Ordering::Greater
        );
        assert_eq!(comparer_total(&Value::Int64(7), &nan), Ordering::Less);
        assert!(egalite_structurelle(&nan, &nan));
    }

    #[test]
    fn egalite_ieee_stricte_sans_epsilon() {
        assert_eq!(
            egaler(&Value::Float64(1.0e-17), &Value::Float64(0.0)),
            Some(false)
        );
        assert_eq!(
            egaler(
                &Value::Float64(f64::INFINITY),
                &Value::Float64(f64::INFINITY)
            ),
            Some(true)
        );
        assert_eq!(
            egaler(&Value::Float64(-0.0), &Value::Float64(0.0)),
            Some(true)
        );
        assert_eq!(
            egaler(
                &Value::Int64(9_007_199_254_740_993),
                &Value::Float64(9_007_199_254_740_992.0)
            ),
            Some(false)
        );
    }

    #[test]
    fn listes_sans_coercition() {
        let l_int = Value::List(vec![Value::Int64(1)].into());
        let l_str = Value::List(vec![Value::String("1".into())].into());
        assert_eq!(egaler(&l_int, &l_str), Some(false));
        assert_eq!(egaler(&l_int, &l_int.clone()), Some(true));
    }

    #[test]
    fn ordre_total_transitif_sur_melange() {
        // Bool < nombres < String < temporels < … < Null, et par valeur dans
        // chaque classe : la sous-suite des entiers reste croissante.
        let vals = [
            Value::Int64(3),
            Value::String("m".into()),
            Value::Int64(-2),
            Value::Bool(true),
            Value::Null,
            Value::Float64(0.5),
        ];
        let mut tries = vals.to_vec();
        tries.sort_by(comparer_total);
        let entiers: Vec<i64> = tries
            .iter()
            .filter_map(|v| match v {
                Value::Int64(i) => Some(*i),
                _ => None,
            })
            .collect();
        assert_eq!(entiers, vec![-2, 3]);
        assert!(matches!(tries[0], Value::Bool(true)));
        assert!(matches!(tries.last(), Some(Value::Null)));
    }

    #[test]
    fn plongement_temporel_au_tri_seulement() {
        let t8 = Value::Timestamp(crate::types::Timestamp::from_micros(8 * 3_600_000_000));
        let t10 = Value::Timestamp(crate::types::Timestamp::from_micros(10 * 3_600_000_000));
        let s9 = Value::String("1970-01-01T09:00:00Z".into());
        let brut = Value::String("abc".into());
        // Au TRI : la chaîne datetime s'interclasse par instant…
        assert_eq!(comparer_total(&t8, &s9), Ordering::Less);
        assert_eq!(comparer_total(&s9, &t10), Ordering::Less);
        // …la chaîne ordinaire reste avant toute la classe temporelle…
        assert_eq!(comparer_total(&brut, &s9), Ordering::Less);
        assert_eq!(comparer_total(&brut, &t8), Ordering::Less);
        // …et deux écritures du même instant sont équivalentes.
        assert_eq!(
            comparer_total(
                &Value::String("1970-01-01T09:00:00Z".into()),
                &Value::String("1970-01-01T11:00:00+02:00".into())
            ),
            Ordering::Equal
        );
        // Aux PRÉDICATS : rien ne plonge — chaîne vs horodatage typé reste
        // incomparable, chaîne vs chaîne reste lexicographique.
        assert_eq!(comparer(&s9, &t10), Comparaison::Incomparables);
        assert_eq!(
            comparer(
                &Value::String("1970-01-01T09:00:00Z".into()),
                &Value::String("1970-01-01T11:00:00+02:00".into())
            ),
            Comparaison::Ordonnees(Ordering::Less)
        );
    }

    #[test]
    fn egalite_temporelle_reflexive() {
        let t = Value::Timestamp(crate::types::Timestamp::from_micros(1_000_001));
        assert_eq!(egaler(&t, &t.clone()), Some(true));
        assert_eq!(
            comparer(&t, &t.clone()),
            Comparaison::Ordonnees(Ordering::Equal)
        );
    }

    // ─────────────────────────────────────────────────────────────────────
    // L'INSTANT D'UNE CHAÎNE DATETIME : EXACT, NI SATURÉ NI TRONQUÉ
    // ─────────────────────────────────────────────────────────────────────

    fn chaine(s: &str) -> Value {
        Value::String(s.into())
    }

    /// Deux dates distinctes hors de la plage d'un `Timestamp` (1677 → 2262)
    /// rendaient le MÊME instant saturé, donc étaient « égales ».
    #[test]
    fn deux_dates_hors_plage_timestamp_ne_sont_pas_egales() {
        for (x, y) in [
            ("3000-01-01T00:00:00Z", "9999-12-31T23:59:59Z"),
            ("2262-04-12T00:00:00Z", "2262-04-13T00:00:00Z"),
            ("1500-06-01T00:00:00Z", "1000-06-01T00:00:00Z"),
            ("1677-09-20T00:00:00Z", "1677-09-19T00:00:00Z"),
        ] {
            let (ix, iy) = (instant_de_chaine(x), instant_de_chaine(y));
            assert!(ix.is_some() && iy.is_some(), "{x} et {y} sont des instants");
            assert_ne!(ix, iy, "{x} et {y} : instants distincts");
            assert_eq!(egaler(&chaine(x), &chaine(y)), Some(false), "{x} = {y}");
            assert!(!egalite_structurelle(&chaine(x), &chaine(y)));
            assert_ne!(comparer_total(&chaine(x), &chaine(y)), Ordering::Equal);
        }
        // L'ordre reste chronologique à ces bornes.
        assert_eq!(
            comparer(
                &chaine("3000-01-01T00:00:00Z"),
                &chaine("9999-12-31T23:59:59Z")
            ),
            Comparaison::Ordonnees(Ordering::Less)
        );
        assert_eq!(
            comparer_total(
                &chaine("1000-06-01T00:00:00Z"),
                &chaine("1500-06-01T00:00:00Z")
            ),
            Ordering::Less
        );
        // Et le même instant écrit dans un autre fuseau reste ÉGAL, là aussi.
        assert_eq!(
            egaler(
                &chaine("3000-01-01T00:00:00Z"),
                &chaine("3000-01-01T02:00:00+02:00")
            ),
            Some(true)
        );
    }

    /// Sous la microseconde : l'ancien `as_micros` tronquait, et deux
    /// instants distants de 100 ns étaient « égaux ».
    #[test]
    fn deux_instants_sous_la_microseconde_ne_sont_pas_egaux() {
        let (x, y) = (
            "2024-01-01T00:00:00.0000001Z",
            "2024-01-01T00:00:00.0000002Z",
        );
        assert_eq!(egaler(&chaine(x), &chaine(y)), Some(false));
        assert_eq!(
            instant_de_chaine(y).unwrap() - instant_de_chaine(x).unwrap(),
            100,
            "100 nanosecondes d'écart"
        );
        // La nanoseconde est la précision : deux écritures du même instant
        // à neuf décimales restent égales.
        assert_eq!(
            egaler(
                &chaine("2024-01-01T00:00:00.000000001Z"),
                &chaine("2024-01-01T02:00:00.000000001+02:00")
            ),
            Some(true)
        );
        // Des zéros de queue ne changent pas l'instant.
        assert_eq!(
            instant_de_chaine("2024-01-01T00:00:00.5Z"),
            instant_de_chaine("2024-01-01T00:00:00.500000000000Z")
        );
    }

    /// Au-delà de la nanoseconde, pas d'instant exact : la chaîne redevient
    /// une chaîne ordinaire — distincte de toute autre écriture, jamais
    /// égale par arrondi.
    #[test]
    fn une_fraction_au_dela_de_la_nanoseconde_n_est_pas_arrondie() {
        let (x, y) = (
            "2024-01-01T00:00:00.0000000001Z",
            "2024-01-01T00:00:00.0000000002Z",
        );
        assert_eq!(instant_de_chaine(x), None);
        assert_eq!(egaler(&chaine(x), &chaine(y)), Some(false));
        assert_eq!(egaler(&chaine(x), &chaine(x)), Some(true));
        assert_eq!(
            egaler(&chaine(x), &chaine("2024-01-01T00:00:00Z")),
            Some(false)
        );
    }

    /// L'instant exact de l'époque, et sa cohérence avec `Timestamp` DANS la
    /// plage que `Timestamp` représente.
    #[test]
    fn l_instant_exact_concorde_avec_timestamp_dans_sa_plage() {
        assert_eq!(instant_de_chaine("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            instant_de_chaine("1970-01-01T00:00:01+00:00"),
            Some(1_000_000_000)
        );
        assert_eq!(
            instant_de_chaine("1969-12-31T23:59:59Z"),
            Some(-1_000_000_000)
        );
        for s in [
            "2024-01-01T10:00:00Z",
            "2026-06-15T10:30:00.123456+05:30",
            "1999-12-31T23:59:59-08:00",
        ] {
            let typé = crate::types::ZonedDatetime::parse(s).unwrap();
            assert_eq!(
                instant_de_chaine(s),
                Some(i128::from(typé.as_timestamp().as_nanos())),
                "{s}"
            );
            // Chaîne datetime et horodatage typé du même instant : Equal au tri.
            assert_eq!(
                comparer_total(&chaine(s), &Value::Timestamp(typé.as_timestamp())),
                Ordering::Equal
            );
        }
    }

    /// `entier_egal_au_flottant` aux bornes : il rend `i` exactement quand
    /// `Int64(i) = Float64(f)` est vrai.
    #[test]
    fn entier_egal_au_flottant_aux_bornes() {
        assert_eq!(entier_egal_au_flottant(0.0), Some(0));
        assert_eq!(entier_egal_au_flottant(-0.0), Some(0));
        assert_eq!(entier_egal_au_flottant(5.0), Some(5));
        assert_eq!(entier_egal_au_flottant(-5.0), Some(-5));
        assert_eq!(entier_egal_au_flottant(5.5), None);
        assert_eq!(entier_egal_au_flottant(f64::NAN), None);
        assert_eq!(entier_egal_au_flottant(f64::INFINITY), None);
        assert_eq!(entier_egal_au_flottant(f64::NEG_INFINITY), None);
        // -2^63 est un i64 ; 2^63 ne l'est pas.
        assert_eq!(
            entier_egal_au_flottant(-9_223_372_036_854_775_808.0),
            Some(i64::MIN)
        );
        assert_eq!(entier_egal_au_flottant(9_223_372_036_854_775_808.0), None);
        assert_eq!(entier_egal_au_flottant(1.0e300), None);
        // Au-delà de 2^53 : le flottant est un entier exact, pas son voisin.
        assert_eq!(
            entier_egal_au_flottant(9_007_199_254_740_992.0),
            Some(9_007_199_254_740_992)
        );
        // Cohérence avec l'égalité, sur tout l'échantillon.
        for f in [
            0.0,
            -0.0,
            5.0,
            5.5,
            1.0e18,
            -1.0e18,
            9_007_199_254_740_992.0,
            9_223_372_036_854_775_808.0,
            -9_223_372_036_854_775_808.0,
            f64::NAN,
            f64::INFINITY,
        ] {
            match entier_egal_au_flottant(f) {
                Some(i) => assert_eq!(
                    egaler(&Value::Int64(i), &Value::Float64(f)),
                    Some(true),
                    "{f}"
                ),
                None => {
                    for i in [0i64, 5, i64::MAX, i64::MIN, 9_007_199_254_740_993] {
                        assert_ne!(
                            egaler(&Value::Int64(i), &Value::Float64(f)),
                            Some(true),
                            "{f} n'égale aucun entier, donc pas {i}"
                        );
                    }
                }
            }
        }
    }
}
