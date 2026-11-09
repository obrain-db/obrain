//! # reachable_count — « combien de nœuds distincts à exactement k sauts ? »
//!
//! Comptage ENSEMBLISTE des extrémités de marches de longueur `k` le long d'un
//! type d'arête unique, depuis n'importe quel nœud d'un label — le pendant
//! natif substrate de `MATCH (a:L)-[:E]->(b:L)-[:E]->…->(d:L) RETURN
//! count(DISTINCT d)` pour un motif de `k` sauts.
//!
//! ## Pourquoi (motivation mesurée)
//!
//! Sur le store code réel (`/tmp/engine.obrain`, ~4 127 nœuds Function, ~2 536
//! arêtes CALLS), la requête GQL
//!
//! ```text
//! MATCH (a:Function)-[:CALLS]->(b:Function)-[:CALLS]->(c:Function)-[:CALLS]->(d:Function)
//! RETURN count(DISTINCT d) AS d
//! ```
//!
//! prend ~55 ms moteur alors qu'elle ne rend que 72 : le moteur ÉNUMÈRE les
//! 279 092 chemins de longueur 3 (49 ms) puis dédoublonne. Cette procédure
//! remplace l'énumération par une fermeture par FRONTIÈRES DISTINCTES
//! (semi-naïve, un passage d'adjacence index-free par saut) — le résultat 72
//! est rendu à l'ordre de la milliseconde ou moins.
//!
//! ## Sémantique exacte (alignée sur `count(DISTINCT d)`)
//!
//! Soit `L` = `from_label` et `E` = `edge_type`. On pose
//!
//! ```text
//! F₀ = { nœuds vivants portant le label L }            (la « source » : le
//!                                                       scan (a:L) du motif)
//! Fₕ = { v : v porte L et ∃ u ∈ Fₕ₋₁, arête u ─E→ v }  (h ≥ 1)
//! ```
//!
//! Chaque `Fₕ` est un ENSEMBLE (dédoublonné par slot) — on n'énumère jamais
//! les chemins. `|Fₖ|` est exactement le `count(DISTINCT d)` du motif à `k`
//! sauts : `d ∈ Fₖ` ssi il existe une marche `a→b→…→d` de longueur `k` dont
//! tous les sommets portent `L` et toutes les arêtes sont de type `E`
//! (les sommets peuvent se répéter — mêmes sémantiques de marche que le
//! comptage `DISTINCT` d'extrémités ; un nœud peut donc rester dans la
//! frontière d'un saut à l'autre via un cycle, il n'est PAS marqué visité).
//!
//! Coût : `O(k × Σ_{u∈Fₕ} deg⁺(u))` — un seul passage par saut sur les chaînes
//! d'adjacence sortantes du substrate (index-free, `GraphStore::edges_from`),
//! jamais de produit cartésien de chemins.
//!
//! # Surfaces
//!
//! - [`ObrainDB::reachable_count`](crate::ObrainDB::reachable_count) — méthode moteur.
//! - `CALL obrain.reachable_count({from_label, edge_type, min_hops, max_hops})
//!   YIELD hop_count, distinct_count` — procédure GQL classée LECTURE
//!   (exécutable en read-only). Forme positionnelle équivalente :
//!   `CALL obrain.reachable_count(from_label, edge_type, min_hops, max_hops)`.
//!   `min_hops` (défaut 1) et `max_hops` (défaut 3) dans `1..=10`,
//!   `min_hops ≤ max_hops`. Une ligne par saut `h` : `hop_count = h`,
//!   `distinct_count = |Fₕ|`.

use std::sync::Arc;

use obrain_common::types::Value;
use obrain_common::utils::error::{Error, Result};
use obrain_core::graph::{Direction, GraphStore};
use obrain_substrate::SubstrateStore;

use crate::query::plan::LogicalExpression;

/// Borne haute de `max_hops` (garde-fou : le coût est linéaire par saut, mais
/// on borne l'effort demandé à une procédure unique).
pub const REACHABLE_MAX_HOPS: usize = 10;
/// Défaut de `min_hops`.
pub const REACHABLE_DEFAULT_MIN_HOPS: usize = 1;
/// Défaut de `max_hops` (le cas mesuré à l'origine du chantier : k = 3).
pub const REACHABLE_DEFAULT_MAX_HOPS: usize = 3;

/// Une ligne de résultat `reachable_count`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReachableCountRow {
    /// Nombre de sauts (longueur exacte des marches considérées).
    pub hop_count: usize,
    /// Nombre de nœuds distincts de `from_label` extrémités d'une marche de
    /// `hop_count` sauts le long d'arêtes `edge_type` (taille de `Fₕ`).
    pub distinct_count: u64,
}

/// Comptage ensembliste par saut (BFS semi-naïf par frontières distinctes).
///
/// Ne touche que des lectures substrate index-free :
/// [`GraphStore::nodes_by_label`] (une fois), puis par saut
/// [`GraphStore::edges_from`] sur chaque nœud de la frontière (O(degré)),
/// filtrées par type d'arête ([`GraphStore::edge_type`]) et label de cible
/// ([`GraphStore::node_has_label`]). Aucune énumération de chemins.
pub fn compute_reachable_counts(
    substrate: &SubstrateStore,
    from_label: &str,
    edge_type: &str,
    min_hops: usize,
    max_hops: usize,
) -> Result<Vec<ReachableCountRow>> {
    if from_label.is_empty() {
        return Err(Error::Internal(
            "obrain.reachable_count: from_label must be a non-empty string".to_string(),
        ));
    }
    if edge_type.is_empty() {
        return Err(Error::Internal(
            "obrain.reachable_count: edge_type must be a non-empty string".to_string(),
        ));
    }
    let min_hops = min_hops.max(1);
    let max_hops = max_hops.clamp(1, REACHABLE_MAX_HOPS);
    if min_hops > max_hops {
        return Err(Error::Internal(format!(
            "obrain.reachable_count: min_hops ({min_hops}) must be ≤ max_hops ({max_hops})"
        )));
    }

    // Frontière de départ : TOUS les nœuds vivants du label (le scan (a:L)
    // du motif — un nœud sans arête sortante ne contribue simplement rien).
    let frontier0: Vec<u64> = substrate
        .nodes_by_label(from_label)
        .into_iter()
        .map(|n| n.0)
        .collect();

    // Frontières ensemblistes : `next` n'accepte chaque slot qu'une fois par
    // saut. Pas de « visited » global — un nœud peut légitimement rester
    // extrémité de marches de longueur h et h+1 (cycles, marches qui se
    // répètent), exactement comme le `count(DISTINCT d)` du motif.
    let mut frontier: hashbrown::HashSet<u64> = frontier0.into_iter().collect();
    let mut out: Vec<ReachableCountRow> = Vec::with_capacity(max_hops.saturating_sub(min_hops) + 1);

    for hop in 1..=max_hops {
        let mut next: hashbrown::HashSet<u64> =
            hashbrown::HashSet::with_capacity(frontier.len() * 2 + 8);
        for u in &frontier {
            for (v, eid) in
                substrate.edges_from(obrain_common::types::NodeId(*u), Direction::Outgoing)
            {
                // Filtre du type d'arête du motif ([:CALLS]).
                match substrate.edge_type(eid) {
                    Some(t) if t.as_str() == edge_type => {}
                    _ => continue,
                }
                // Chaque sommet de la marche doit porter `from_label` (les
                // (b:L), (c:L), … du motif) — le test exclut aussi les slots
                // morts (None).
                if substrate.node_has_label(v, from_label) != Some(true) {
                    continue;
                }
                next.insert(v.0);
            }
        }
        if hop >= min_hops {
            out.push(ReachableCountRow {
                hop_count: hop,
                distinct_count: next.len() as u64,
            });
        }
        if next.is_empty() {
            // Plus aucune extrémité aux sauts suivants : les comptes restants
            // sont nuls.
            for h in (hop + 1).max(min_hops)..=max_hops {
                out.push(ReachableCountRow {
                    hop_count: h,
                    distinct_count: 0,
                });
            }
            break;
        }
        frontier = next;
    }
    Ok(out)
}

impl super::ObrainDB {
    /// (2026-09-08) Nombre de nœuds distincts de `from_label` extrémités de
    /// marches de longueur `hop ∈ [min_hops, max_hops]` le long d'arêtes
    /// sortantes `edge_type` (équivalent ensembliste de
    /// `MATCH (a:L)-[:E]->…(k sauts)…->(d:L) RETURN count(DISTINCT d)`).
    /// Nécessite un store substrate.
    pub fn reachable_count(
        &self,
        from_label: &str,
        edge_type: &str,
        min_hops: usize,
        max_hops: usize,
    ) -> Result<Vec<ReachableCountRow>> {
        let substrate = self.substrate_store.as_ref().ok_or_else(|| {
            Error::Internal("reachable_count requires a substrate-backed database".to_string())
        })?;
        compute_reachable_counts(substrate, from_label, edge_type, min_hops, max_hops)
    }
}

/// Exécute `CALL obrain.reachable_count(...)` si le nom matche.
///
/// `Ok(None)` si le nom n'est pas une procédure reachable_count. Erreur
/// explicite si la base n'est pas substrate ou les arguments sont invalides.
pub fn try_execute_reachable_count_procedure(
    resolved_name: &str,
    arguments: &[LogicalExpression],
    substrate: Option<&Arc<SubstrateStore>>,
) -> Result<Option<obrain_adapters::plugins::AlgorithmResult>> {
    if resolved_name != "obrain.reachable_count" && resolved_name != "reachable_count" {
        return Ok(None);
    }
    let result = execute_reachable_count(arguments, substrate)?;
    Ok(Some(result))
}

/// Lit un `Value` littéral comme chaîne.
fn expr_string(name: &str, e: &LogicalExpression) -> Result<String> {
    match e {
        LogicalExpression::Literal(Value::String(s)) => Ok(s.to_string()),
        _ => Err(Error::Internal(format!(
            "obrain.reachable_count: {name} must be a string"
        ))),
    }
}

/// Lit un `Value` littéral comme entier non négatif.
fn expr_usize(name: &str, e: &LogicalExpression) -> Result<usize> {
    match e {
        LogicalExpression::Literal(Value::Int64(v)) if *v >= 0 => Ok(*v as usize),
        _ => Err(Error::Internal(format!(
            "obrain.reachable_count: {name} must be a non-negative integer"
        ))),
    }
}

/// Cherche la valeur d'une clé dans une entrée de map littérale.
fn map_value<'a>(
    entries: &'a [(String, LogicalExpression)],
    key: &str,
) -> Option<&'a LogicalExpression> {
    entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

/// Paramètres décodés de l'appel (forme map OU positionnelle).
struct ReachableArgs {
    from_label: String,
    edge_type: String,
    min_hops: usize,
    max_hops: usize,
}

fn execute_reachable_count(
    arguments: &[LogicalExpression],
    substrate: Option<&Arc<SubstrateStore>>,
) -> Result<obrain_adapters::plugins::AlgorithmResult> {
    // Forme map : `{from_label: 'L', edge_type: 'E', min_hops: 1, max_hops: 3}`
    let args = if arguments.len() == 1
        && let LogicalExpression::Map(entries) = &arguments[0]
    {
        let from_label = match map_value(entries, "from_label") {
            Some(e) => expr_string("from_label", e)?,
            None => {
                return Err(Error::Internal(
                    "obrain.reachable_count: missing key 'from_label'".to_string(),
                ));
            }
        };
        let edge_type = match map_value(entries, "edge_type") {
            Some(e) => expr_string("edge_type", e)?,
            None => {
                return Err(Error::Internal(
                    "obrain.reachable_count: missing key 'edge_type'".to_string(),
                ));
            }
        };
        let min_hops = match map_value(entries, "min_hops") {
            Some(e) => expr_usize("min_hops", e)?,
            None => REACHABLE_DEFAULT_MIN_HOPS,
        };
        let max_hops = match map_value(entries, "max_hops") {
            Some(e) => expr_usize("max_hops", e)?,
            None => REACHABLE_DEFAULT_MAX_HOPS,
        };
        ReachableArgs {
            from_label,
            edge_type,
            min_hops,
            max_hops,
        }
    } else {
        // Forme positionnelle : (from_label, edge_type, [min_hops], [max_hops]).
        let err = || {
            Error::Internal(
                "obrain.reachable_count: expects at least 2 arguments \
                 (from_label, edge_type[, min_hops, max_hops])"
                    .to_string(),
            )
        };
        let arg0 = arguments.first().ok_or_else(err)?;
        let arg1 = arguments.get(1).ok_or_else(err)?;
        let from_label = expr_string("from_label (arg 0)", arg0)?;
        let edge_type = expr_string("edge_type (arg 1)", arg1)?;
        let min_hops = match arguments.get(2) {
            Some(e) => expr_usize("min_hops (arg 2)", e)?,
            None => REACHABLE_DEFAULT_MIN_HOPS,
        };
        let max_hops = match arguments.get(3) {
            Some(e) => expr_usize("max_hops (arg 3)", e)?,
            None => REACHABLE_DEFAULT_MAX_HOPS,
        };
        ReachableArgs {
            from_label,
            edge_type,
            min_hops,
            max_hops,
        }
    };

    let Some(sub) = substrate else {
        return Err(Error::Internal(
            "obrain.reachable_count requires a substrate-backed database".to_string(),
        ));
    };

    let rows = compute_reachable_counts(
        sub,
        &args.from_label,
        &args.edge_type,
        args.min_hops,
        args.max_hops,
    )?;

    let mut result = obrain_adapters::plugins::AlgorithmResult::new(vec![
        "hop_count".to_string(),
        "distinct_count".to_string(),
    ]);
    for r in rows {
        result.add_row(vec![
            Value::Int64(r.hop_count as i64),
            Value::Int64(r.distinct_count as i64),
        ]);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use obrain_common::types::NodeId;
    use obrain_core::graph::GraphStoreMut;

    /// Compte les extrémités `d` par énumération naïve (marches de longueur
    /// `k`) sur le store — l'oracle du test unitaire, indépendant de la
    /// fermeture par frontières.
    fn oracle_distinct_endpoints(
        store: &SubstrateStore,
        label: &str,
        edge_type: &str,
        k: usize,
    ) -> u64 {
        let mut frontier: Vec<Vec<u64>> = vec![
            store
                .nodes_by_label(label)
                .into_iter()
                .map(|n| n.0)
                .collect(),
        ];
        for _ in 0..k {
            let mut next: Vec<u64> = Vec::new();
            for u in &frontier[frontier.len() - 1] {
                for (v, eid) in store.edges_from(NodeId(*u), Direction::Outgoing) {
                    match store.edge_type(eid) {
                        Some(t) if t.as_str() == edge_type => {}
                        _ => continue,
                    }
                    if store.node_has_label(v, label) == Some(true) {
                        next.push(v.0);
                    }
                }
            }
            frontier.push(next);
        }
        let last = frontier.last().unwrap();
        let mut sorted = last.clone();
        sorted.sort_unstable();
        sorted.dedup();
        sorted.len() as u64
    }

    fn edge(s: &SubstrateStore, a: u64, b: u64) {
        s.create_edge(NodeId(a), NodeId(b), "T");
    }

    #[test]
    fn reachable_count_egale_oracle_marches_cycle_et_branche() {
        let store = SubstrateStore::open_tempfile().expect("temp substrate");
        // Cycle 1→2→3→1 + branche 2→4→5→6 (6 puits) + chaîne détachée
        // 7→8→9 (7 source pure) — tous label "L".
        for _ in 1..=9 {
            store.create_node(&["L"]);
        }
        edge(&store, 1, 2);
        edge(&store, 2, 3);
        edge(&store, 3, 1);
        edge(&store, 2, 4);
        edge(&store, 4, 5);
        edge(&store, 5, 6);
        edge(&store, 7, 8);
        edge(&store, 8, 9);

        let rows = compute_reachable_counts(&store, "L", "T", 1, 4).expect("compute");
        // Attendus calculés à la main (sémantique de marche, sommets
        // répétables) :
        //   F1 = {1,2,3,4,5,6,8,9}        → 8   (7 exclu : source pure)
        //   F2 = {1,2,3,4,5,6,9}          → 7   (8 sorti : 7 ∉ F1)
        //   F3 = {1,2,3,4,5,6}            → 6   (9 sorti : 8 ∉ F2)
        //   F4 = {1,2,3,4,5,6}            → 6   (le cycle maintient la base)
        let expected: Vec<(usize, u64)> = vec![(1, 8), (2, 7), (3, 6), (4, 6)];
        let got: Vec<(usize, u64)> = rows
            .iter()
            .map(|r| (r.hop_count, r.distinct_count))
            .collect();
        assert_eq!(got, expected, "comptes par saut");

        // Parité avec l'oracle (marches naïves dédupliquées), saut par saut.
        for k in 1..=4 {
            let oracle = oracle_distinct_endpoints(&store, "L", "T", k);
            let row = rows
                .iter()
                .find(|r| r.hop_count == k)
                .expect("row présente");
            assert_eq!(
                row.distinct_count, oracle,
                "saut {k} : fermeture par frontières vs oracle"
            );
        }
    }

    #[test]
    #[allow(clippy::many_single_char_names)] // nœuds du graphe de test nommés a, b, c, p, q comme dans le schéma
    fn reachable_count_filtre_label_et_type_d_arete() {
        let store = SubstrateStore::open_tempfile().expect("temp substrate");
        // Trois nœuds X (a,b,c) + deux nœuds Y (p,q), handles explicites.
        let (a, b, c) = (
            store.create_node(&["X"]).0,
            store.create_node(&["X"]).0,
            store.create_node(&["X"]).0,
        );
        let (p, q) = (store.create_node(&["Y"]).0, store.create_node(&["Y"]).0);
        // Chaîne X : a→b→c (T) ; croisements X→Y : a→p, c→p (T) ;
        // arête interne Y : p→q (T).
        edge(&store, a, b);
        edge(&store, b, c);
        edge(&store, a, p);
        edge(&store, c, p);
        edge(&store, p, q);

        // Depuis X le long de T : à 1 saut, les targets X = {b, c} (p/q
        // portent Y → exclus) ; à 2 sauts, targets X de {b,c} = {c} (p ∉ F1).
        let rows = compute_reachable_counts(&store, "X", "T", 1, 2).expect("compute");
        let got: Vec<(usize, u64)> = rows
            .iter()
            .map(|r| (r.hop_count, r.distinct_count))
            .collect();
        assert_eq!(got, vec![(1, 2), (2, 1)]);
    }

    #[test]
    fn reachable_count_valide_les_bornes() {
        let store = SubstrateStore::open_tempfile().expect("temp substrate");
        store.create_node(&["L"]);
        // max_hops > 10 → plafonné (clamp), pas d'erreur.
        let rows = compute_reachable_counts(&store, "L", "T", 1, 99).expect("clamp");
        assert_eq!(rows.len(), REACHABLE_MAX_HOPS);
        // min_hops > max_hops → erreur explicite.
        assert!(compute_reachable_counts(&store, "L", "T", 5, 3).is_err());
        // from_label / edge_type vides → erreur explicite.
        assert!(compute_reachable_counts(&store, "", "T", 1, 1).is_err());
        assert!(compute_reachable_counts(&store, "L", "", 1, 1).is_err());
        // Label inconnu → comptes nuls, pas d'erreur.
        let rows = compute_reachable_counts(&store, "Absent", "T", 1, 3).expect("absent");
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().all(|r| r.distinct_count == 0));
    }
}
