//! Façade des comparaisons partagées par les opérateurs (agrégats, filtres,
//! tris, recherche).
//!
//! (Correctif E, 2026-08) Ce fichier portait DEUX des quatre dialectes de
//! comparaison mesurés par la campagne de conformité : un `compare_values`
//! qui parsait les chaînes numériquement (hack RDF fuyant dans LPG — il
//! faisait min('10','9') = '9' là où ORDER BY et `<` disent '10') et un
//! `compare_values_total` qui rendait `Equal` pour toute paire de types
//! différents — donc n'était PAS un ordre total, et laissait le tri inerte
//! sur colonne hétérogène. Les deux délèguent désormais au comparateur
//! canonique [`obrain_common::types::value_cmp`] ; les implémentations
//! locales sont SUPPRIMÉES.

use std::cmp::Ordering;

use obrain_common::types::{Value, value_cmp};

/// Converts a value to `f64` for numeric aggregations.
///
/// Le parse des chaînes reste ici À DESSEIN : les agrégats NUMÉRIQUES
/// (sum/avg/stddev…) servent aussi le chemin RDF, où les littéraux numériques
/// vivent dans des chaînes. Ce parse ne participe à AUCUNE comparaison —
/// min/max et les prédicats passent par `value_cmp`, qui ne coerce pas.
pub fn value_to_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Int64(i) => Some(*i as f64),
        Value::Float64(f) => Some(*f),
        // RDF stores numeric literals as strings - try to parse them
        Value::String(s) => s.parse::<f64>().ok(),
        _ => None,
    }
}

/// Comparaison de PRÉDICAT canonique : `None` pour les classes disjointes
/// (UNKNOWN) comme pour NaN. Chaîne↔nombre n'est PLUS comparé — le même
/// verdict que `<`/`<=`/`>`/`>=` dans l'évaluateur et que le balayage par
/// intervalle.
#[must_use]
pub fn compare_values(a: &Value, b: &Value) -> Option<Ordering> {
    match value_cmp::comparer(a, b) {
        value_cmp::Comparaison::Ordonnees(o) => Some(o),
        value_cmp::Comparaison::NonOrdonnees | value_cmp::Comparaison::Incomparables => None,
    }
}

/// L'ORDRE TOTAL canonique — utilisé par les tris (pull, push, externe,
/// fusion parallèle) et les incumbents de MIN/MAX. Vraiment total : types
/// rangés par classe (Bool < nombres < String < temporels < … < Null),
/// valeurs comparées dans la classe, NaN plus grand que tout nombre.
#[must_use]
pub fn compare_values_total(a: &Value, b: &Value) -> Ordering {
    value_cmp::comparer_total(a, b)
}

/// Returns `true` if `new` is less than `current` (for MIN aggregation).
///
/// Returns `true` when `current` is `None` (first value always wins).
/// L'ORDRE TOTAL garantit qu'un incumbent est TOUJOURS délogeable : plus
/// aucun min/max figé sur sa première valeur face à un type « incomparable ».
pub fn is_less_than(current: &Option<Value>, new: &Value) -> bool {
    match current {
        None => true,
        Some(curr) => value_cmp::comparer_total(new, curr) == Ordering::Less,
    }
}

/// Returns `true` if `new` is greater than `current` (for MAX aggregation).
///
/// Returns `true` when `current` is `None` (first value always wins).
pub fn is_greater_than(current: &Option<Value>, new: &Value) -> bool {
    match current {
        None => true,
        Some(curr) => value_cmp::comparer_total(new, curr) == Ordering::Greater,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_to_f64_int() {
        assert_eq!(value_to_f64(&Value::Int64(42)), Some(42.0));
    }

    #[test]
    fn value_to_f64_float() {
        assert_eq!(value_to_f64(&Value::Float64(2.72)), Some(2.72));
    }

    #[test]
    fn value_to_f64_numeric_string() {
        assert_eq!(value_to_f64(&Value::String("2.5".into())), Some(2.5));
    }

    #[test]
    fn value_to_f64_non_numeric_string() {
        assert_eq!(value_to_f64(&Value::String("abc".into())), None);
    }

    #[test]
    fn value_to_f64_null() {
        assert_eq!(value_to_f64(&Value::Null), None);
    }

    #[test]
    fn compare_same_type_int() {
        assert_eq!(
            compare_values(&Value::Int64(1), &Value::Int64(2)),
            Some(Ordering::Less)
        );
    }

    #[test]
    fn compare_cross_type_int_float() {
        assert_eq!(
            compare_values(&Value::Int64(2), &Value::Float64(2.0)),
            Some(Ordering::Equal)
        );
    }

    #[test]
    fn les_chaines_se_comparent_lexicographiquement_jamais_en_nombres() {
        // Fin du hack RDF : '10' < '9' parce que '1' < '9'. C'est l'ordre
        // qu'ORDER BY et `<` ont toujours rendu — désormais min/max aussi.
        assert_eq!(
            compare_values(&Value::String("10".into()), &Value::String("9".into())),
            Some(Ordering::Less)
        );
    }

    #[test]
    fn compare_incomparable() {
        assert_eq!(compare_values(&Value::Bool(true), &Value::Int64(1)), None);
    }

    #[test]
    fn l_ordre_total_tranche_les_types_disjoints_par_rang() {
        // Plus jamais `Equal` entre types : Bool < nombres < String, et la
        // transitivité tient (c'est elle que le tri exige).
        assert_eq!(
            compare_values_total(&Value::Bool(true), &Value::Int64(1)),
            Ordering::Less
        );
        assert_eq!(
            compare_values_total(&Value::Int64(3), &Value::String("m".into())),
            Ordering::Less
        );
        assert_eq!(
            compare_values_total(&Value::String("m".into()), &Value::Int64(-2)),
            Ordering::Greater
        );
    }

    #[test]
    fn is_less_than_none_always_true() {
        assert!(is_less_than(&None, &Value::Int64(5)));
    }

    #[test]
    fn is_less_than_smaller() {
        assert!(is_less_than(&Some(Value::Int64(10)), &Value::Int64(5)));
    }

    #[test]
    fn is_less_than_larger() {
        assert!(!is_less_than(&Some(Value::Int64(3)), &Value::Int64(5)));
    }

    #[test]
    fn is_greater_than_none_always_true() {
        assert!(is_greater_than(&None, &Value::Int64(5)));
    }

    #[test]
    fn is_greater_than_larger() {
        assert!(is_greater_than(&Some(Value::Int64(3)), &Value::Int64(5)));
    }

    #[test]
    fn is_greater_than_smaller() {
        assert!(!is_greater_than(&Some(Value::Int64(10)), &Value::Int64(5)));
    }

    #[test]
    fn l_incumbent_min_max_est_delogeable_entre_types() {
        // Le défaut « incumbent jamais délogé » : min sur [String, Int] devait
        // rendre l'Int (rang numérique < rang chaîne), pas rester figé.
        assert!(is_less_than(
            &Some(Value::String("zzz".into())),
            &Value::Int64(5)
        ));
        assert!(is_greater_than(
            &Some(Value::Int64(5)),
            &Value::String("aaa".into())
        ));
    }
}
