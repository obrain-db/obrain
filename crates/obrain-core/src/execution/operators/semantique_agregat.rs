//! Sémantique des agrégats par dialecte — obrain#196, famille D4.
//!
//! Les spécifications divergent sur la valeur d'un agrégat scalaire appliqué à un
//! ensemble **vide** :
//!
//! | fonction | SQL / ISO GQL | openCypher | SPARQL 1.1 |
//! |---|---|---|---|
//! | `sum` | NULL | **0** | 0 |
//! | `avg` | NULL | NULL | — |
//! | `min` / `max` | NULL | NULL | NULL |
//! | `collect` | NULL | liste vide | — |
//! | `count` | 0 | 0 | 0 |
//!
//! Jusqu'ici le moteur décidait **en dur** dans `AggregateState::finalize`
//! (`operators/aggregate.rs`) : quatre bras faisant `if count == 0 { Value::Null }`,
//! sans aucune notion de dialecte. Un test d'un seul dialecte
//! (`gql_07_agregats`) gouvernait donc la sémantique de **tous** les dialectes
//! servis (Cypher, GQL, SQL/PGQ, SPARQL, TinkerPop) — et produisait une
//! divergence mesurée contre Neo4j (issue #196, D4).
//!
//! Ce module isole la décision en **un seul point**, pur et testable sans base,
//! sans plan et sans requête. Les étapes suivantes de la RFC
//! (`AggregateSemantics` porté par la requête, `finalize(sem)`) viennent s'y
//! brancher sans le modifier.

use obrain_common::types::Value;

use super::accumulator::AggregateFunction;

/// Profil de spécification appliqué à une requête.
///
/// Le défaut est [`AggregateSemantics::OpenCypher`] : l'objectif produit
/// d'obrain est de remplacer Neo4j, donc la lecture de référence est celle de
/// Neo4j quand aucun dialecte n'est précisé.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AggregateSemantics {
    /// openCypher / Neo4j — `sum()` sur ensemble vide vaut `0`.
    #[default]
    OpenCypher,
    /// SQL / ISO GQL — `SUM` sur ensemble vide vaut `NULL`.
    Gql,
    /// SPARQL 1.1 — `SUM` sur ensemble vide vaut `0`.
    Sparql,
    /// TinkerPop / Gremlin — `sum()` sur ensemble vide vaut `0`.
    TinkerPop,
}

impl AggregateSemantics {
    /// Nom stable, pour les messages et la sérialisation.
    #[must_use]
    pub fn nom(self) -> &'static str {
        match self {
            Self::OpenCypher => "openCypher",
            Self::Gql => "GQL",
            Self::Sparql => "SPARQL",
            Self::TinkerPop => "TinkerPop",
        }
    }
}

/// Valeur d'un agrégat scalaire appliqué à un ensemble **vide**.
///
/// C'est le point de décision UNIQUE : le moteur ne doit plus décider ailleurs.
#[must_use]
pub fn valeur_sur_ensemble_vide(
    fonction: AggregateFunction,
    semantique: AggregateSemantics,
) -> Value {
    use AggregateFunction::{Collect, Count, CountNonNull, Sum};
    match (fonction, semantique) {
        // `count` vaut 0 dans toutes les spécifications.
        (Count | CountNonNull, _) => Value::Int64(0),
        // `sum` : NULL en SQL/GQL, 0 en openCypher, SPARQL et TinkerPop.
        (Sum, AggregateSemantics::Gql) => Value::Null,
        (Sum, _) => Value::Int64(0),
        // `collect` rend une liste vide, jamais NULL.
        (Collect, _) => Value::List(Vec::new().into()),
        // `avg`, `min`, `max`, écarts-types, variances, fonctions binaires :
        // NULL partout — il n'y a ni valeur à moyenner ni extremum.
        _ => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use AggregateFunction::{Avg, Collect, Count, CountNonNull, Max, Min, Sum};

    #[test]
    fn sum_sur_ensemble_vide_depend_du_dialecte() {
        // openCypher : 0. C'est la lecture de Neo4j, donc celle qu'attend PO.
        assert_eq!(
            valeur_sur_ensemble_vide(Sum, AggregateSemantics::OpenCypher),
            Value::Int64(0)
        );
        assert_eq!(
            valeur_sur_ensemble_vide(Sum, AggregateSemantics::Sparql),
            Value::Int64(0)
        );
        assert_eq!(
            valeur_sur_ensemble_vide(Sum, AggregateSemantics::TinkerPop),
            Value::Int64(0)
        );
        // SQL / ISO GQL : NULL.
        assert_eq!(
            valeur_sur_ensemble_vide(Sum, AggregateSemantics::Gql),
            Value::Null
        );
    }

    #[test]
    fn count_sur_ensemble_vide_vaut_zero_dans_tous_les_dialectes() {
        for sem in [
            AggregateSemantics::OpenCypher,
            AggregateSemantics::Gql,
            AggregateSemantics::Sparql,
            AggregateSemantics::TinkerPop,
        ] {
            assert_eq!(valeur_sur_ensemble_vide(Count, sem), Value::Int64(0));
            assert_eq!(valeur_sur_ensemble_vide(CountNonNull, sem), Value::Int64(0));
        }
    }

    #[test]
    fn avg_min_et_max_sur_ensemble_vide_rendent_null_partout() {
        for sem in [
            AggregateSemantics::OpenCypher,
            AggregateSemantics::Gql,
            AggregateSemantics::Sparql,
            AggregateSemantics::TinkerPop,
        ] {
            assert_eq!(valeur_sur_ensemble_vide(Avg, sem), Value::Null);
            assert_eq!(valeur_sur_ensemble_vide(Min, sem), Value::Null);
            assert_eq!(valeur_sur_ensemble_vide(Max, sem), Value::Null);
        }
    }

    #[test]
    fn collect_sur_ensemble_vide_rend_une_liste_vide_jamais_null() {
        for sem in [
            AggregateSemantics::OpenCypher,
            AggregateSemantics::Gql,
            AggregateSemantics::Sparql,
            AggregateSemantics::TinkerPop,
        ] {
            match valeur_sur_ensemble_vide(Collect, sem) {
                Value::List(l) => assert!(l.is_empty(), "collect vide doit rendre une liste vide"),
                autre => panic!("collect vide doit rendre une liste, pas {autre:?}"),
            }
        }
    }

    #[test]
    fn le_defaut_est_open_cypher() {
        assert_eq!(
            AggregateSemantics::default(),
            AggregateSemantics::OpenCypher
        );
        assert_eq!(AggregateSemantics::default().nom(), "openCypher");
    }
}
