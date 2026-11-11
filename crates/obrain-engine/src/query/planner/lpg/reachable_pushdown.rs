//! # Pushdown planner — chaînes homogènes `count(DISTINCT d)` → noyau
//! `reachable_count`
//!
//! Réécriture CONSERVATRICE, au moment de la planification physique, du motif
//!
//! ```text
//! MATCH (a:L)-[:E]->(b:L)-[:E]->…->(d:L)   (k sauts, k ∈ 1..=10)
//! RETURN count(DISTINCT d) AS x
//! ```
//!
//! vers le calcul ensembliste [`compute_reachable_counts`] (le noyau déjà
//! livré de `obrain.reachable_count`) : `|Fₖ|` nœuds distincts de label `L`
//! extrémités de marches de `k` arêtes `E`. La requête existante garde sa
//! sémantique — même ligne, même colonne — mais l'énumération des chemins
//! disparaît (sur `/tmp/engine.obrain`, k=3 : ~56 ms → ~1 ms, résultat 72
//! inchangé).
//!
//! ## Formes RÉÉCRITES (détection au niveau du plan logique, dans
//! [`try_plan_reachable_count_pushdown`], appelé depuis `plan_aggregate`)
//!
//! La forme acceptée est UNIQUEMENT la suivante, et TOUTES les conditions
//! doivent tenir :
//!
//! 1. **Aggrégat** : `AggregateOp` SANS `group_by`, SANS `having`, avec
//!    EXACTEMENT UN agrégat `count(DISTINCT v)` (`Count`/`CountNonNull`,
//!    `distinct: true`) dont l'argument est une simple `Variable` — et `v`
//!    est le nœud TERMINAL de la chaîne (aucune autre variable du motif
//!    n'est projetée : tout ce qui se trouve au-dessus de l'agrégat ne voit
//!    que sa colonne de sortie, donc rien d'autre ne peut fuiter).
//! 2. **Chaîne d'expansions** : `k` opérateurs `Expand` consécutifs imbriqués
//!    (`k ∈ 1..=10`), tous avec :
//!    * `direction == Outgoing` (sens sortant uniquement) ;
//!    * UN SEUL type d'arête (`edge_types.len() == 1`), identique à chaque
//!      saut ;
//!    * un saut unique (`min_hops == 1`, `max_hops == Some(1)`) ;
//!    * `path_alias == None` et `path_mode == Walk` (pas de sémantique
//!      Trail/Simple/Acyclic, pas de chemin nommé).
//! 3. **Labels homogènes** : chaque nœud du motif porte le MÊME label `L` —
//!    le scan de base est `NodeScan(v₀, label: Some(L))` SANS entrée propre,
//!    et chaque autre variable du motif est contrainte par un filtre
//!    `hasLabel(v, "L")` (formé par le traducteur GQL pour `(v:L)`) placé
//!    exactement au-dessus de l'expansion qui la produit. Un label
//!    manquant, un label différent, un `hasLabel` supplémentaire, ou une
//!    prédicat de filtre autre que `hasLabel` → REFUS.
//! 4. **Variables distinctes** : aucune variable n'apparaît deux fois dans
//!    la chaîne (pas de nœud réutilisé : `(a)-[:E]->(b)-[:E]->(a)` exigerait
//!    une égalité de fermeture que le noyau ne connaît pas).
//! 5. **Base de données substrate + lecture sûre** : le planificateur porte
//!    un `substrate_store` ; pas de registre multi-store (aucune arête
//!    externe inter-bases) ; requête LECTURE (`read_only`) sans écritures en
//!    attente — le noyau lit le substrate en direct (index-free), ce qui
//!    n'est équivalent au chemin générique que quand aucune visibilité
//!    MVCC transactionnelle n'est en jeu.
//!
//! ## Formes REFUSÉES (retour au plan normal, zéro régression)
//!
//! Toute autre forme — en particulier : labels hétérogènes ou absents sur un
//! nœud intermédiaire/terminal, plusieurs types d'arêtes (ou zéro), une
//! seule direction `Incoming`/`Both`, un saut variable (min≠1 ou
//! max≠1) ou `k ∉ 1..=10`, `count(d)` SANS `DISTINCT`, `count(*)`,
//! `count(DISTINCT d.prop)`, `GROUP BY`, `HAVING`, plusieurs agrégats, un
//! prédicat `WHERE`/`FILTER` dans la chaîne, un `path_alias`, un mode de
//! chemin non-Walk, une variable réutilisée, une base non-substrate ou une
//! requête en transaction explicite / avec écritures en attente / en
//! session multi-store — n'est PAS réécrite : `try_plan_…` rend `None` et
//! `plan_aggregate` poursuit avec le plan historique (factorisé ou
//! générique). En cas de doute : ne pas réécrire.
//!
//! ## Compteur
//!
//! [`REACHABLE_PUSHDOWN_FIRED`] est incrémenté à chaque réécriture effective
//! (tests d'observabilité du module).

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};

use obrain_common::types::{LogicalType, Value};
use obrain_core::execution::operators::Operator;

use super::{
    AggregateOp, ExpandDirection, LogicalAggregateFunction, LogicalExpression, LogicalOperator,
    PathMode, Planner, Result, ScalarResultOperator,
};
use crate::database::reachable_count::{REACHABLE_MAX_HOPS, compute_reachable_counts};

/// Compteur de réécritures `reachable_count` effectivement déclenchées.
pub(super) static REACHABLE_PUSHDOWN_FIRED: AtomicU64 = AtomicU64::new(0);

/// Chaîne homogène extraite du plan logique, prête pour le noyau.
struct HomogeneousChain {
    /// Label commun `L` de tous les nœuds du motif (le `from_label`).
    from_label: String,
    /// Type d'arête unique `E` des `k` sauts.
    edge_type: String,
    /// Nombre de sauts `k`.
    hops: usize,
    /// Variable liée au nœud terminal (l'extrémité comptée).
    terminal_var: String,
}

/// Décode un prédicat `hasLabel(v, "L")` littéral (forme exacte émise par le
/// traducteur GQL pour `(v:L)`). Toute autre forme → `None` (refus).
fn parse_has_label(pred: &LogicalExpression) -> Option<(&str, &str)> {
    let LogicalExpression::FunctionCall { name, args, .. } = pred else {
        return None;
    };
    if !name.eq_ignore_ascii_case("hasLabel") || args.len() != 2 {
        return None;
    }
    let (LogicalExpression::Variable(v), LogicalExpression::Literal(Value::String(l))) =
        (&args[0], &args[1])
    else {
        return None;
    };
    Some((v.as_str(), l.as_str()))
}

/// Parcourt le sous-arbre d'entrée de l'agrégat et reconnaît la chaîne
/// homogène décrite dans l'en-tête du module. `None` = forme non réécrite.
///
/// La marche descend depuis l'entrée de l'agrégat : filtre `hasLabel(v)`
/// (obligatoire pour le terminal et pour chaque nœud intermédiaire, avec le
/// même label partout), puis expansion sortante à un saut qui produit `v`,
/// etc., jusqu'au `NodeScan` de base portant le label.
fn parse_homogeneous_chain(agg_input: &LogicalOperator) -> Option<HomogeneousChain> {
    let mut cur: &LogicalOperator = agg_input;
    // Variable attendue pour le prochain filtre / nœud de base : posée par le
    // terminal, re-posée par chaque expansion (sa variable source).
    let mut cur_var: Option<&str> = None;
    // Variable du nœud TERMINAL : celle du premier filtre rencontré (le plus
    // haut de la chaîne, celui que l'agrégat compte).
    let mut terminal: Option<&str> = None;
    // Label commun observé (homogénéité).
    let mut label: Option<&str> = None;
    // Type d'arête commun observé.
    let mut edge_type: Option<&str> = None;
    // Variables déjà liées par la chaîne (refus des nœuds réutilisés).
    let mut bound: HashSet<&str> = HashSet::new();
    let mut hops: usize = 0;
    // Après une expansion, la variable source doit être contrainte par un
    // `hasLabel` AVANT la prochaine expansion (les nœuds intermédiaires du
    // motif doivent tous porter le label commun — le noyau l'exige à chaque
    // saut). Seule la base `NodeScan` (qui porte le label dans le scan)
    // satisfait la contrainte sans filtre.
    let mut need_filter: bool = false;

    loop {
        match cur {
            LogicalOperator::Filter(filter) => {
                let (v, l) = parse_has_label(&filter.predicate)?;
                // Le filtre doit porter sur la variable courante (le nœud que
                // l'expansion juste au-dessus vient de produire / produira).
                if let Some(expected) = cur_var {
                    if v != expected {
                        return None;
                    }
                } else {
                    cur_var = Some(v);
                    terminal = Some(v);
                }
                // Homogénéité du label sur tous les nœuds filtrés.
                if let Some(prev) = label {
                    if prev != l {
                        return None;
                    }
                } else {
                    label = Some(l);
                }
                need_filter = false;
                cur = &filter.input;
            }
            LogicalOperator::Expand(expand) => {
                // Une expansion ne peut pas consommer une variable
                // intermédiaire non contrainte par un hasLabel.
                if need_filter {
                    return None;
                }
                let single_hop = expand.min_hops == 1 && expand.max_hops == Some(1);
                if !single_hop
                    || !matches!(expand.direction, ExpandDirection::Outgoing)
                    || expand.path_alias.is_some()
                    || !matches!(expand.path_mode, PathMode::Walk)
                    || expand.edge_types.len() != 1
                {
                    return None;
                }
                // L'expansion doit produire la variable courante.
                let to = cur_var?;
                if expand.to_variable != to {
                    return None;
                }
                let et = expand.edge_types[0].as_str();
                if let Some(prev) = edge_type {
                    if prev != et {
                        return None;
                    }
                } else {
                    edge_type = Some(et);
                }
                // Variables liées toutes distinctes.
                if !bound.insert(expand.to_variable.as_str()) {
                    return None;
                }
                hops += 1;
                cur_var = Some(expand.from_variable.as_str());
                // La variable source de cette expansion (nœud intermédiaire,
                // sauf si c'est la base) doit être contrainte par un hasLabel
                // avant la prochaine expansion.
                need_filter = true;
                cur = &expand.input;
            }
            LogicalOperator::NodeScan(scan) => {
                if scan.input.is_some() {
                    return None;
                }
                let scan_var = scan.variable.as_str();
                if bound.contains(scan_var) || cur_var != Some(scan_var) {
                    return None;
                }
                // Le scan porte le label commun (F₀ du noyau).
                let scan_label = scan.label.as_deref()?;
                if let Some(l) = label
                    && scan_label != l
                {
                    return None;
                }
                if !(1..=REACHABLE_MAX_HOPS).contains(&hops) {
                    return None;
                }
                // Le terminal est la variable du premier filtre (le plus haut
                // de la chaîne), capturée avant la descente.
                let terminal = terminal?;
                return Some(HomogeneousChain {
                    from_label: scan_label.to_string(),
                    edge_type: edge_type?.to_string(),
                    hops,
                    terminal_var: terminal.to_string(),
                });
            }
            // Tout autre opérateur dans la chaîne (WHERE, LIMIT, …) → refus.
            _ => return None,
        }
    }
}

/// Tente la réécriture pushdown. `None` = forme refusée (OU échec d'exécution
/// du noyau — dans les deux cas le plan normal s'exécute, sémantique
/// préservée). `Some(Ok((op, cols)))` = forme acceptée et calculée par la
/// fermeture par frontières.
pub(super) fn try_plan_reachable_count_pushdown(
    planner: &Planner,
    agg: &AggregateOp,
) -> Option<Result<(Box<dyn Operator>, Vec<String>)>> {
    // ── Gardes d'environnement (lecture directe du substrate) ─────────────
    if planner.substrate_store.is_none()
        || planner.session_context.base_registry.is_some()
        || !planner.read_only
        || planner.store.has_pending_writes()
    {
        return None;
    }

    // ── Forme de l'agrégat : count(DISTINCT var) sans group/having unique ──
    if !agg.group_by.is_empty() || agg.having.is_some() || agg.aggregates.len() != 1 {
        return None;
    }
    let agg_expr = &agg.aggregates[0];
    let is_count_distinct = matches!(
        agg_expr.function,
        LogicalAggregateFunction::Count | LogicalAggregateFunction::CountNonNull
    ) && agg_expr.distinct;
    if !is_count_distinct {
        return None;
    }
    let LogicalExpression::Variable(counted) = agg_expr.expression.as_ref()? else {
        // count(*), count(d.prop), … → non réécrit.
        return None;
    };

    // ── Reconnaissance de la chaîne homogène ───────────────────────────────
    let chain = parse_homogeneous_chain(&agg.input)?;
    if counted.as_str() != chain.terminal_var.as_str() {
        return None;
    }

    // ── Exécution ensembliste (noyau reachable_count, k = min = max) ──────
    let substrate = planner.substrate_store.as_ref().expect("gardé plus haut");
    let rows = compute_reachable_counts(
        substrate,
        &chain.from_label,
        &chain.edge_type,
        chain.hops,
        chain.hops,
    )
    .ok()?; // forme valide ⇒ le noyau ne peut pas échouer (bornes ok) ; au
    // pire on retombe sur le plan normal.
    let distinct_count = rows
        .iter()
        .find(|r| r.hop_count == chain.hops)
        .map_or(0, |r| r.distinct_count) as i64;

    REACHABLE_PUSHDOWN_FIRED.fetch_add(1, Ordering::Relaxed);

    // Même nommage de colonne que le chemin générique de `plan_aggregate` :
    // alias explicite ou nom par défaut `"{:?}(...)".to_lowercase()`.
    let alias = agg_expr.alias.clone().unwrap_or_else(|| {
        crate::query::planner::common::default_aggregate_column_name(agg_expr.function)
    });
    // Colonne scalaire (Int64) : le RETURN au-dessus la passe sans NodeResolve.
    planner.scalar_columns.borrow_mut().insert(alias.clone());
    let op: Box<dyn Operator> = Box::new(ScalarResultOperator::new(
        vec![Value::Int64(distinct_count)],
        vec![LogicalType::Int64],
    ));
    Some(Ok((op, vec![alias])))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use obrain_common::types::NodeId;
    use obrain_core::graph::GraphStoreMut;
    use obrain_substrate::SubstrateStore;

    use crate::query::executor::Executor;
    use crate::query::plan::{AggregateExpr, ExpandOp, FilterOp, NodeScanOp, ReturnItem, ReturnOp};

    fn has_label(var: &str, label: &str) -> LogicalExpression {
        LogicalExpression::FunctionCall {
            name: "hasLabel".to_string(),
            args: vec![
                LogicalExpression::Variable(var.to_string()),
                LogicalExpression::Literal(Value::from(label)),
            ],
            distinct: false,
        }
    }

    fn scan(var: &str, label: &str) -> LogicalOperator {
        LogicalOperator::NodeScan(NodeScanOp {
            variable: var.to_string(),
            label: Some(label.to_string()),
            input: None,
            pushdown_hint: None,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn expand(
        from: &str,
        to: &str,
        edge_type: &str,
        direction: ExpandDirection,
        input: LogicalOperator,
        min_hops: u32,
        max_hops: Option<u32>,
        path_alias: Option<&str>,
        path_mode: PathMode,
    ) -> LogicalOperator {
        LogicalOperator::Expand(ExpandOp {
            from_variable: from.to_string(),
            to_variable: to.to_string(),
            edge_variable: None,
            direction,
            edge_types: vec![edge_type.to_string()],
            min_hops,
            max_hops,
            input: Box::new(input),
            path_alias: path_alias.map(String::from),
            path_mode,
        })
    }

    fn filter(predicate: LogicalExpression, input: LogicalOperator) -> LogicalOperator {
        LogicalOperator::Filter(FilterOp {
            predicate,
            input: Box::new(input),
            pushdown_hint: None,
        })
    }

    /// Construit le plan logique de
    /// `MATCH (v0:L)-[:E]->…-[:E]->(vk:L) RETURN count(DISTINCT vk)` (labels
    /// filtrés par `hasLabel` comme le traducteur GQL).
    ///
    /// La boucle imite `translate_path_pattern_with_alias` : chaque filtre
    /// `hasLabel` de la cible est empilé AU-DESSUS de l'expansion qui la
    /// produit → le nœud final du plan est le filtre du terminal `vk`.
    fn chain_plan(k: usize, label: &str, edge: &str) -> LogicalOperator {
        let vars: Vec<String> = (0..=k).map(|i| format!("v{i}")).collect();
        let mut plan = scan(&vars[0], label);
        for i in 1..=k {
            let inner = expand(
                &vars[i - 1],
                &vars[i],
                edge,
                ExpandDirection::Outgoing,
                plan,
                1,
                Some(1),
                None,
                PathMode::Walk,
            );
            plan = filter(has_label(&vars[i], label), inner);
        }
        plan
    }

    fn aggregate_over(plan: LogicalOperator, distinct: bool, alias: &str) -> AggregateOp {
        AggregateOp {
            group_by: vec![],
            aggregates: vec![AggregateExpr {
                function: LogicalAggregateFunction::CountNonNull,
                expression: Some(LogicalExpression::Variable(terminal_of(&plan).to_string())),
                expression2: None,
                distinct,
                alias: Some(alias.to_string()),
                percentile: None,
                separator: None,
            }],
            input: Box::new(plan),
            having: None,
        }
    }

    /// Nom de la variable terminale du plan de chaîne construit par
    /// [`chain_plan`] (`v{k}`) — lue depuis le filtre le plus haut.
    fn terminal_of(plan: &LogicalOperator) -> &str {
        let LogicalOperator::Filter(f) = plan else {
            panic!("plan attendu: chaîne commençant par un filtre hasLabel");
        };
        let LogicalExpression::FunctionCall { args, .. } = &f.predicate else {
            panic!("filtre hasLabel attendu");
        };
        let LogicalExpression::Variable(v) = &args[0] else {
            panic!("variable attendue");
        };
        v
    }

    fn count_distinct_plan(k: usize) -> AggregateOp {
        aggregate_over(chain_plan(k, "L", "T"), true, "d")
    }

    /// Verrou de sérialisation des tests qui lisent le compteur global
    /// [`REACHABLE_PUSHDOWN_FIRED`] (les tests du module tournent en
    /// parallèle par défaut).
    static FIRE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    // ── Détection : formes acceptées ──────────────────────────────────────

    #[test]
    fn parse_accepte_chaines_homogenes_k_1_a_10() {
        for k in 1..=10 {
            let plan = count_distinct_plan(k);
            // L'entrée de l'agrégat est le hasLabel du terminal → filtrage.
            let parsed = parse_homogeneous_chain(&plan.input);
            let chain = parsed.expect("chaîne homogène acceptée");
            assert_eq!(chain.hops, k, "k = {k}");
            assert_eq!(chain.from_label, "L");
            assert_eq!(chain.edge_type, "T");
            assert_eq!(chain.terminal_var, format!("v{k}"), "terminal, k = {k}");
        }
    }

    #[test]
    fn parse_refuse_labels_heterogenes() {
        // (a:L)-[:T]->(b:M)-[:T]->(c:M)
        let mut plan = scan("a", "L");
        plan = expand(
            "a",
            "b",
            "T",
            ExpandDirection::Outgoing,
            plan,
            1,
            Some(1),
            None,
            PathMode::Walk,
        );
        plan = filter(has_label("b", "M"), plan);
        plan = expand(
            "b",
            "c",
            "T",
            ExpandDirection::Outgoing,
            plan,
            1,
            Some(1),
            None,
            PathMode::Walk,
        );
        plan = filter(has_label("c", "M"), plan);
        assert!(parse_homogeneous_chain(&plan).is_none());
    }

    #[test]
    fn parse_refuse_deux_types_d_aretes() {
        // (a:L)-[:T]->(b:L)-[:U]->(c:L)
        let mut plan = scan("a", "L");
        plan = expand(
            "a",
            "b",
            "T",
            ExpandDirection::Outgoing,
            plan,
            1,
            Some(1),
            None,
            PathMode::Walk,
        );
        plan = filter(has_label("b", "L"), plan);
        plan = expand(
            "b",
            "c",
            "U",
            ExpandDirection::Outgoing,
            plan,
            1,
            Some(1),
            None,
            PathMode::Walk,
        );
        plan = filter(has_label("c", "L"), plan);
        assert!(parse_homogeneous_chain(&plan).is_none());
    }

    #[test]
    fn parse_refuse_direction_entrante() {
        // (a:L)<-[:T]-(b:L)<-[:T]-(c:L) — direction Incoming.
        let mut plan = scan("a", "L");
        plan = expand(
            "a",
            "b",
            "T",
            ExpandDirection::Incoming,
            plan,
            1,
            Some(1),
            None,
            PathMode::Walk,
        );
        plan = filter(has_label("b", "L"), plan);
        plan = expand(
            "b",
            "c",
            "T",
            ExpandDirection::Incoming,
            plan,
            1,
            Some(1),
            None,
            PathMode::Walk,
        );
        plan = filter(has_label("c", "L"), plan);
        assert!(parse_homogeneous_chain(&plan).is_none());
    }

    #[test]
    fn parse_refuse_longueur_variable_et_k_hors_borne() {
        // Variable-length (*1..2) → refus.
        let mut plan = scan("a", "L");
        plan = expand(
            "a",
            "b",
            "T",
            ExpandDirection::Outgoing,
            plan,
            1,
            Some(2),
            None,
            PathMode::Walk,
        );
        plan = filter(has_label("b", "L"), plan);
        assert!(parse_homogeneous_chain(&plan).is_none());

        // k = 0 (un seul nœud, pas d'expansion) → refus.
        assert!(parse_homogeneous_chain(&scan("a", "L")).is_none());

        // k = 11 → refus (borne du noyau REACHABLE_MAX_HOPS).
        assert!(parse_homogeneous_chain(&chain_plan(11, "L", "T")).is_none());
    }

    #[test]
    fn parse_refuse_variable_reutilisee() {
        // (a:L)-[:T]->(b:L)-[:T]->(a:L) — a lié deux fois.
        let mut plan = scan("a", "L");
        plan = expand(
            "a",
            "b",
            "T",
            ExpandDirection::Outgoing,
            plan,
            1,
            Some(1),
            None,
            PathMode::Walk,
        );
        plan = filter(has_label("b", "L"), plan);
        plan = expand(
            "b",
            "a",
            "T",
            ExpandDirection::Outgoing,
            plan,
            1,
            Some(1),
            None,
            PathMode::Walk,
        );
        plan = filter(has_label("a", "L"), plan);
        assert!(parse_homogeneous_chain(&plan).is_none());
    }

    #[test]
    fn parse_refuse_where_et_chemins_nommes() {
        // WHERE sur une variable intermédiaire : prédicat non-hasLabel.
        let mut plan = scan("a", "L");
        plan = expand(
            "a",
            "b",
            "T",
            ExpandDirection::Outgoing,
            plan,
            1,
            Some(1),
            None,
            PathMode::Walk,
        );
        plan = filter(
            LogicalExpression::Binary {
                left: Box::new(LogicalExpression::Variable("b".to_string())),
                op: crate::query::plan::BinaryOp::Eq,
                right: Box::new(LogicalExpression::Variable("c".to_string())),
            },
            plan,
        );
        plan = expand(
            "b",
            "c",
            "T",
            ExpandDirection::Outgoing,
            plan,
            1,
            Some(1),
            None,
            PathMode::Walk,
        );
        plan = filter(has_label("c", "L"), plan);
        assert!(parse_homogeneous_chain(&plan).is_none());

        // path_alias → refus.
        let mut plan = scan("a", "L");
        plan = expand(
            "a",
            "b",
            "T",
            ExpandDirection::Outgoing,
            plan,
            1,
            Some(1),
            Some("p"),
            PathMode::Walk,
        );
        plan = filter(has_label("b", "L"), plan);
        assert!(parse_homogeneous_chain(&plan).is_none());
    }

    #[test]
    fn parse_refuse_scan_sans_label() {
        // (a)-[:T]->(b:L) — source sans label.
        let mut plan = LogicalOperator::NodeScan(NodeScanOp {
            variable: "a".to_string(),
            label: None,
            input: None,
            pushdown_hint: None,
        });
        plan = expand(
            "a",
            "b",
            "T",
            ExpandDirection::Outgoing,
            plan,
            1,
            Some(1),
            None,
            PathMode::Walk,
        );
        plan = filter(has_label("b", "L"), plan);
        assert!(parse_homogeneous_chain(&plan).is_none());
    }

    #[test]
    fn parse_refuse_terminal_sans_label() {
        // (a:L)-[:T]->(b:L)-[:T]->(c) — le terminal c n'est pas filtré par un
        // hasLabel : le noyau (qui exige le label à chaque saut) ne peut pas
        // répondre → refus.
        let mut plan = scan("a", "L");
        plan = expand(
            "a",
            "b",
            "T",
            ExpandDirection::Outgoing,
            plan,
            1,
            Some(1),
            None,
            PathMode::Walk,
        );
        plan = filter(has_label("b", "L"), plan);
        plan = expand(
            "b",
            "c",
            "T",
            ExpandDirection::Outgoing,
            plan,
            1,
            Some(1),
            None,
            PathMode::Walk,
        );
        assert!(parse_homogeneous_chain(&plan).is_none());
    }

    #[test]
    fn parse_refuse_noeud_intermediaire_sans_label() {
        // (a:L)-[:T]->(b)-[:T]->(c:L) — b non contraint : la marche pourrait
        // passer par un nœud hors label → refus.
        let mut plan = scan("a", "L");
        plan = expand(
            "a",
            "b",
            "T",
            ExpandDirection::Outgoing,
            plan,
            1,
            Some(1),
            None,
            PathMode::Walk,
        );
        plan = expand(
            "b",
            "c",
            "T",
            ExpandDirection::Outgoing,
            plan,
            1,
            Some(1),
            None,
            PathMode::Walk,
        );
        plan = filter(has_label("c", "L"), plan);
        assert!(parse_homogeneous_chain(&plan).is_none());
    }

    // ── Réécriture de bout en bout (planner + executor) ───────────────────

    fn substrate_planner(store: &Arc<SubstrateStore>) -> Planner {
        let erased: Arc<dyn GraphStoreMut> = Arc::clone(store) as Arc<dyn GraphStoreMut>;
        Planner::new(erased)
            .with_read_only(true)
            .with_substrate_store(Arc::clone(store))
    }

    /// Exécute `Return(count(DISTINCT vk))` sur l'agrégat donné et rend
    /// `(valeur Int64, delta du compteur de réécriture)`.
    fn run_count(planner: &Planner, agg: AggregateOp) -> (i64, u64) {
        let before = REACHABLE_PUSHDOWN_FIRED.load(Ordering::Relaxed);
        let logical = crate::query::plan::LogicalPlan::new(LogicalOperator::Return(ReturnOp {
            items: vec![ReturnItem {
                expression: LogicalExpression::Variable("d".to_string()),
                alias: None,
            }],
            distinct: false,
            input: Box::new(LogicalOperator::Aggregate(agg)),
        }));
        let mut physical = planner.plan(&logical).expect("plan");
        let executor = Executor::with_columns(physical.columns.clone());
        let result = executor
            .execute(physical.operator.as_mut())
            .expect("execute");
        let value = match &result.rows[0][0] {
            Value::Int64(n) => *n,
            other => panic!("valeur inattendue: {other:?}"),
        };
        let after = REACHABLE_PUSHDOWN_FIRED.load(Ordering::Relaxed);
        (value, after - before)
    }

    fn edge(store: &SubstrateStore, a: u64, b: u64) {
        store.create_edge(NodeId(a), NodeId(b), "T");
    }

    /// Graphe connu du module reachable_count : cycle 1→2→3→1 + branche
    /// 2→4→5→6 + chaîne détachée 7→8→9, tous label L.
    fn build_graph(store: &SubstrateStore) {
        for _ in 1..=9u64 {
            store.create_node(&["L"]);
        }
        edge(store, 1, 2);
        edge(store, 2, 3);
        edge(store, 3, 1);
        edge(store, 2, 4);
        edge(store, 4, 5);
        edge(store, 5, 6);
        edge(store, 7, 8);
        edge(store, 8, 9);
    }

    #[test]
    fn reecriture_fire_et_vaut_la_fermeture_par_sauts() {
        let _guard = FIRE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // k = 1..4 sur le graphe connu : |Fk| attendu 8,7,6,6 (voir le module
        // reachable_count). Le compteur prouve que la réécriture a eu lieu.
        let expected = [(1usize, 8i64), (2, 7), (3, 6), (4, 6)];
        for (k, want) in expected {
            // Graphe neuf par k (les handles substrate sont globaux au store).
            let store = Arc::new(SubstrateStore::open_tempfile().expect("temp"));
            build_graph(&store);
            let planner = substrate_planner(&store);
            let (got, fired) = run_count(&planner, count_distinct_plan(k));
            assert_eq!(got, want, "count(DISTINCT) réécrit, k={k}");
            assert_eq!(
                fired, 1,
                "la réécriture doit se déclencher exactement une fois pour k={k}"
            );
        }
    }

    #[test]
    fn formes_refusees_gardent_l_ancien_chemin() {
        let _guard = FIRE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // count SANS DISTINCT : l'agrégat n'est pas réécrit (le compteur ne
        // bouge pas) et le résultat suit l'ancien chemin (nombre de marches).
        let store = Arc::new(SubstrateStore::open_tempfile().expect("temp"));
        // 1→2→3 et 4→2 : le nœud 3 est l'extrémité de DEUX marches de 2 sauts
        // (1→2→3, 4→2→3) ; une seule extrémité distincte {3}.
        store.create_node(&["L"]); // 1
        store.create_node(&["L"]); // 2
        store.create_node(&["L"]); // 3
        store.create_node(&["L"]); // 4
        edge(&store, 1, 2);
        edge(&store, 2, 3);
        edge(&store, 4, 2);
        let planner = substrate_planner(&store);

        // count(d) sans DISTINCT → chemin générique : 2 marches, 0 réécriture.
        let no_distinct = aggregate_over(chain_plan(2, "L", "T"), false, "d");
        let (paths, fired) = run_count(&planner, no_distinct);
        assert_eq!(paths, 2, "2 marches vers v2");
        assert_eq!(fired, 0, "count sans DISTINCT : pas de réécriture");

        // count(DISTINCT d) : 1 (dédoublonné), et réécrit.
        let (distinct, fired) = run_count(&planner, count_distinct_plan(2));
        assert_eq!(distinct, 1, "1 extrémité distincte");
        assert_eq!(fired, 1);
    }
}
