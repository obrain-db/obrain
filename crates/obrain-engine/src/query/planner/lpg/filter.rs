//! Filter planning with zone map pre-filtering and index lookups.

use std::collections::HashSet;

use super::{
    ApplyOperator, Arc, BinaryOp, EmptyOperator, ExpandDirection, ExpandOp, ExpressionPredicate,
    FilterExpression, FilterOp, FilterOperator, GraphStore, HashAggregateOperator,
    HashJoinOperator, HashMap, IndexSeekJoinOperator, JoinOp, JoinType, LogicalExpression,
    LogicalOperator, LogicalType, NestedLoopJoinOperator, NodeId, NodeListOperator, NodeScanOp,
    Operator, PathMode, PhysicalAggregateExpr, PhysicalJoinType, RangeBounds, Result, UnaryOp,
    Value, convert_binary_op, convert_filter_expression,
};

use obrain_core::execution::operators::{DistinctOperator, UnionOperator};

/// Plafond de sauts pour lesquels on retourne un motif à longueur VARIABLE.
///
/// Défaut mesuré le 2026-09-12 : `MATCH (a)-[*1..2]->(b) WHERE id(b) IN [315447]
/// RETURN count(a)` sur `runtime-dev` — 23 696 arêtes entrantes sur la cible —
/// **tournait encore après 150 s**, sans que le délai de requête l'arrête. Le
/// parcours partait de TOUS les nœuds du graphe ; amorcé par la cible et mené en
/// entrant, il ne touche plus que ce qui mène à elle.
///
/// Le plafond existe parce que le coût d'un retournement croît en O(degré^sauts)
/// DEPUIS LA GRAINE : au-delà de quelques sauts il n'est plus une garantie, et on
/// préfère laisser le plan générique que promettre une borne qu'on ne tient pas.
const PLAFOND_SAUTS_RETOURNEMENT: u32 = 4;

/// Égalité planifiée via le comparateur CANONIQUE — le même verdict que `=`
/// dans l'évaluateur (correctif E). L'ancien dialecte local (Int64↔Float64
/// par EPSILON absolu, tout le reste par `PartialEq` strict) faisait dépendre
/// la réponse de la présence d'un label dans le motif.
fn values_equal_coerced(a: &Value, b: &Value) -> bool {
    obrain_common::types::value_cmp::egaler(a, b) == Some(true)
}

/// ── LA GARDE « JAMAIS PIRE QU'UN BALAYAGE », EN UN SEUL ENDROIT ─────────
///
/// Un index qui rend BEAUCOUP de candidats coûte plus cher que le balayage
/// qu'il remplace : il matérialise k identifiants puis revérifie chacun par
/// un accès ALÉATOIRE, là où le balayage lit des slots consécutifs. Le défaut
/// est mesuré, pas supposé : `CONTAINS 'banc_obrain'` sur `runtime-dev`
/// (427 000 nœuds) coûtait **553 ms par l'index contre 80 ms au balayage**
/// pour les mêmes lignes — sept fois pire.
///
/// Le seuil est donc RELATIF au store (1 %, plancher 256) et non absolu : la
/// même requête doit se comporter pareil à mille et à dix millions de nœuds.
/// `variable` nomme la variable d'environnement qui le règle en POUR CENT,
/// pour que la mesure puisse déplacer le seuil sans recompiler.
///
/// `true` ⇒ l'appelant renonce à son index et rend la main au chemin
/// d'origine, balayage compris. Il perd une accélération ; il ne peut pas
/// gagner une ligne fausse, puisque le chemin d'origine est celui qui fait
/// autorité.
fn trop_de_candidats(candidats: usize, total: usize, variable: &str) -> bool {
    if total == 0 {
        return false;
    }
    let plafond = std::env::var(variable)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .map(|pct| total.saturating_mul(pct) / 100)
        .unwrap_or_else(|| (total / 100).max(256));
    candidats > plafond
}

impl super::Planner {
    /// Plans a filter operator.
    ///
    /// Uses zone map pre-filtering to potentially skip scans when predicates
    /// definitely won't match any data. Also uses property indexes when available
    /// for O(1) lookups instead of full scans. Complex EXISTS/NOT EXISTS subqueries
    /// are rewritten as semi-joins or anti-joins.

    /// (2026-09-07) Planifie une disjonction contenant des EXISTS par
    /// UNION-DISTINCT de ses branches : forme normale disjonctive (une
    /// branche par disjonctif), chaque branche planifiée séparément (le
    /// semi/anti-join EXISTS existant s'applique aux AND des branches), puis
    /// UNION + DISTINCT pour la sémantique OR. Sûr : ne s'applique qu'au cas
    /// détecté par `exists_under_or` (les OR purement scalaires restent sur
    /// le filtre unitaire).
    fn plan_or_with_exists(&self, filter: &FilterOp) -> Result<(Box<dyn Operator>, Vec<String>)> {
        use crate::query::plan::LogicalOperator;
        use obrain_common::utils::error::QueryError;
        use obrain_common::utils::error::QueryErrorKind;
        let clauses = Self::dnf_clauses(&filter.predicate);
        if clauses.len() < 2 || clauses.len() > 32 {
            return Err(QueryError::new(
                QueryErrorKind::Semantic,
                format!(
                    "OR-EXISTS : {} disjonctifs — réécriture UNION non applicable",
                    clauses.len()
                ),
            )
            .into());
        }
        let mut operators: Vec<Box<dyn Operator>> = Vec::with_capacity(clauses.len());
        let mut columns: Option<Vec<String>> = None;
        for clause in &clauses {
            let sub = FilterOp {
                predicate: clause.clone(),
                input: filter.input.clone(),
                pushdown_hint: None,
            };
            let (op, cols) = self.plan_filter(&sub)?;
            if columns.is_none() {
                columns = Some(cols);
            }
            operators.push(op);
        }
        let columns = columns.expect("au moins une branche");
        let schema = self.derive_schema_from_columns(&columns);
        let union = Box::new(UnionOperator::new(operators, schema.clone()));
        let distinct = Box::new(DistinctOperator::new(union, schema));
        let _ = &LogicalOperator::Filter; // (type utilisé via FilterOp ci-dessus)
        Ok((distinct, columns))
    }

    /// (2026-09-07) Forme normale disjonctive d'un prédicat : liste des
    /// conjonctions (aucune n'a d'OR au niveau racine). Sert au découpage
    /// OR-EXISTS en branches UNION. La taille reste bornée par l'appelant.
    fn dnf_clauses(expr: &LogicalExpression) -> Vec<LogicalExpression> {
        match expr {
            LogicalExpression::Binary {
                op: BinaryOp::Or,
                left,
                right,
            } => {
                let mut a = Self::dnf_clauses(left);
                a.extend(Self::dnf_clauses(right));
                a
            }
            LogicalExpression::Binary {
                op: BinaryOp::And,
                left,
                right,
            } => {
                let ls = Self::dnf_clauses(left);
                let rs = Self::dnf_clauses(right);
                let mut out = Vec::new();
                for l in &ls {
                    for r in &rs {
                        out.push(LogicalExpression::Binary {
                            left: Box::new(l.clone()),
                            op: BinaryOp::And,
                            right: Box::new(r.clone()),
                        });
                    }
                }
                out
            }
            _ => vec![expr.clone()],
        }
    }

    /// (2026-09-07) Un nœud `ExistsSubquery` est-il présent sous une
    /// disjonction (`Or`) dans l'arbre du prédicat ? Parcours récursif.
    fn exists_under_or(expr: &LogicalExpression) -> bool {
        match expr {
            LogicalExpression::ExistsSubquery(_) => false, // seul, pas sous OR
            LogicalExpression::Binary {
                op: BinaryOp::Or,
                left,
                right,
            } => Self::expr_contains_exists(left) || Self::expr_contains_exists(right),
            LogicalExpression::Binary {
                op: BinaryOp::And,
                left,
                right,
            } => Self::exists_under_or(left) || Self::exists_under_or(right),
            LogicalExpression::Unary { operand, .. } => Self::exists_under_or(operand),
            _ => false,
        }
    }

    /// Contient un `ExistsSubquery` (n'importe où dans le sous-arbre) ?
    fn expr_contains_exists(expr: &LogicalExpression) -> bool {
        match expr {
            LogicalExpression::ExistsSubquery(_) => true,
            LogicalExpression::Binary { left, right, .. } => {
                Self::expr_contains_exists(left) || Self::expr_contains_exists(right)
            }
            LogicalExpression::Unary { operand, .. } => Self::expr_contains_exists(operand),
            _ => false,
        }
    }

    pub(super) fn plan_filter(
        &self,
        filter: &FilterOp,
    ) -> Result<(Box<dyn Operator>, Vec<String>)> {
        // (2026-09-07) SUPPORT OR-EXISTS : une disjonction contenant des
        // EXISTS était évaluée de façon INCORRECTE et silencieuse (le bras
        // EXISTS ignoré — racines seules, mesuré sur runtime-dev). Réécriture
        // correcte : forme normale disjonctive → UNE branche planifiée par
        // disjonctif (chacune passe par le semi/anti-join EXISTS existant) →
        // UNION + DISTINCT (sémantique OR sans doublons).
        if Self::exists_under_or(&filter.predicate) {
            return self.plan_or_with_exists(filter);
        }
        // Check for complex EXISTS/NOT EXISTS patterns and rewrite as semi/anti join.
        // Simple single-hop EXISTS patterns are handled by the fast path in
        // convert_expression() -> extract_exists_pattern().
        let outer_vars = Self::collect_plan_variables(&filter.input);
        if let Some((subquery, is_negated, remaining)) =
            self.extract_complex_exists(&filter.predicate, &outer_vars)
        {
            return self.plan_exists_as_semi_join(&filter.input, subquery, is_negated, remaining);
        }

        // Check for COUNT subquery comparisons and rewrite as Apply + Aggregate + Filter.
        // Handles patterns like: COUNT { MATCH ... } > 5, COUNT { ... } = 0, etc.
        if let Some((subquery, op, threshold, remaining)) =
            Self::extract_count_comparison(&filter.predicate)
        {
            return self.plan_count_as_apply(&filter.input, subquery, op, threshold, remaining);
        }

        // Check zone maps for simple property predicates before scanning
        // If zone map says "definitely no matches", we can short-circuit
        if let Some(false) = self.check_zone_map_for_predicate(&filter.predicate) {
            // Zone map says no matches possible - return empty result
            let (_, columns) = self.plan_operator(&filter.input)?;
            let schema = self.derive_schema_from_columns(&columns);
            let empty_op = Box::new(EmptyOperator::new(schema));
            return Ok((empty_op, columns));
        }

        // Try to use direct node-by-id seek for `WHERE id(n) = X` patterns
        if let Some(result) = self.try_plan_filter_with_id_lookup(filter)? {
            return Ok(result);
        }

        // Try the label-first path for `n.prop IN [literals]` (T17i).
        if let Some(result) = self.try_plan_filter_with_in_list(filter)? {
            return Ok(result);
        }

        // Try to use property index for equality predicates on indexed properties
        if let Some(result) = self.try_plan_filter_with_property_index(filter)? {
            return Ok(result);
        }

        // ── EXISTENCE : `n.p IS NOT NULL` ────────────────────────────────
        //
        // APRÈS l'égalité, et ce n'est pas un hasard : quand les deux formes
        // coexistent dans un même prédicat, le seek d'égalité est plus
        // sélectif (une valeur contre toute une propriété). La route
        // d'égalité rend `None` quand il n'y a AUCUNE égalité à pousser : la
        // forme pure d'existence arrive donc ici, intacte.
        if let Some(result) = self.try_plan_filter_with_property_existence(filter)? {
            return Ok(result);
        }

        // (2026-08, mission perf DB, chantier #95) Jointure CORRELEE par
        // sonde d'index : le motif interne de `MATCH (a:A) MATCH (b:B {k: a.fk})`
        // porte un predicat Property = Property que le chemin par litteral
        // ci-dessus ne reconnait pas — il retombait sur un produit cartesien
        // ScanOperator x input (324 s mesurees en prod sur Evidence x Claim).
        // Ici : une sonde d'index par ligne externe.
        if let Some(result) = self.try_plan_correlated_index_seek(filter)? {
            return Ok(result);
        }

        // ── SOUS-CHAÎNES : `CONTAINS`, `STARTS WITH`, `ENDS WITH` ────────
        //
        // AVANT l'intervalle, et c'est une inversion motivée par la mesure :
        // pour un préfixe de chemin, l'index de trigrammes rend **371 candidats
        // pour 368 lignes attendues** (0,15 ms), là où l'index ordonné en
        // énumère **127 200** (80 ms) — sa clé est tronquée à 11 octets, et
        // tous les `qualified_name` partagent ce préfixe. Le chemin d'intervalle
        // reste le FILET : quand l'index de trigrammes se tait (index éteint,
        // tous les grammes trop communs), `Some(None)` n'est pas rendu et la
        // ligne suivante prend le relais, inchangée.
        if let Some(result) = self.try_plan_filter_with_text_index(filter)? {
            return Ok(result);
        }

        // ── LE SECOND CHEMIN DE `CONTAINS` : LA ZONE DE TRIGRAMMES ────────
        //
        // Le tronc avait ÉCARTÉ cette route (« deux index concurrents pour un
        // même prédicat seraient deux vérités à tenir »), et la consolidation
        // avait donc hérité d'un planificateur où `CONTAINS` n'était plus servi
        // QUE par le chemin ci-dessus. Or ce chemin-là est INERTE PAR DÉFAUT :
        // `trigrammes_des_la` vaut `u32::MAX`, donc son index n'est JAMAIS bâti
        // sans `OBRAIN_TRIGRAMMES_DES_LA=1`, et le prédicat retombait sur le
        // balayage. Mesuré sur le service en marche, même processus, même
        // requête : `runtime-prod` **0,47 ms** (zone de trigrammes), le même
        // prédicat par le seul chemin du tronc : balayage.
        //
        // Les deux routes rendent un résultat EXACT et revérifié valeur par
        // valeur ; elles ne diffèrent que par la source des candidats et le
        // coût quand cette source se tait. On les essaie donc DANS L'ORDRE DE
        // CE QUI SERT : celle du tronc d'abord (0,15 ms sur un préfixe, sa
        // persistance a ses ancres contrôlées), celle de la zone ensuite —
        // c'est elle qui sert les stores servis, dont les séries existent déjà.
        // Aucune des deux ne rend `Some(vec![])` par ignorance : le silence
        // passe la main, jamais une réponse vide.
        if let Some(result) = self.try_plan_filter_with_contains(filter)? {
            return Ok(result);
        }

        // Try to use range optimization for range predicates (>, <, >=, <=)
        if let Some(result) = self.try_plan_filter_with_range_index(filter)? {
            return Ok(result);
        }

        // CONTAINS est servi par `try_plan_filter_with_text_index` (module
        // `props_trigram_index`), placé après le chemin d'intervalle : sa
        // persistance a ses ancres contrôlées, et c'est la voie MESURÉE
        // (`OBRAIN_TRIGRAMMES_DES_LA`, `OBRAIN_TRIGRAMMES_PERSISTER`). La
        // variante par la zone d'égalité du chantier principal est écartée
        // ici, pas oubliée : deux index concurrents pour un même prédicat
        // seraient deux vérités à tenir — et la plus rapide n'est pas la plus
        // récente.

        // (chantier VOISINAGE-ENTRANT) Traversee 1-saut dont la CIBLE porte
        // la contrainte indexee : re-ancrage sur la cible, Expand inverse.
        if let Some(result) = self.try_plan_reancrage_entrant(filter)? {
            return Ok(result);
        }

        // ── RETOURNER LE MOTIF QUAND L'IDENTIFIANT DE LA CIBLE EST CONTRAINT ─
        // `id(b) = X` / `id(b) IN [...]` sur la CIBLE d'une expansion ne peut
        // pas descendre sous elle : `b` n'y est pas encore lié. Sans cette
        // branche, le motif entier retombe sur un SCAN COMPLET DE LA TABLE DES
        // ARÊTES — mesuré 216 à 923 ms sur `runtime-dev`, alors que la même
        // contrainte sur la SOURCE est poussée sous le scan et coûte 85 µs.
        if let LogicalOperator::Expand(expand) = filter.input.as_ref()
            && let Some(result) = self.plan_retournement_sur_cible_id(&filter.predicate, expand)?
        {
            return Ok(result);
        }

        // ── POUSSER LE FILTRE DANS LES DEUX CÔTÉS D'UNE JOINTURE ─────────────
        // Défaut mesuré le 2026-09-12 : deux écritures de la MÊME requête à
        // 360× d'écart sur `runtime-dev`. En chaîne, le filtre tombe entre les
        // expansions (88 ms) ; à plat, il restait AU-DESSUS de la jointure :
        //
        // ```text
        // Filter (id(b) In [3])
        //   Join (Inner)                       deux côtés scannent TOUT
        //     Expand (a)->(b) ← NodeScan (a)    puis on apparie
        //     Expand (b)->(c) ← NodeScan (b)   → 31 724 ms
        // ```
        //
        // soit un produit O(E)×O(E) avant de garder 19 968 lignes. En poussant le
        // prédicat dans chaque côté qui porte ses variables, chaque côté devient
        // un motif À CIBLE CONTRAINTE — donc le retournement ci-dessus s'y
        // applique, et les deux côtés partent de la GRAINE au lieu de tout
        // balayer.
        if let LogicalOperator::Join(_) = filter.input.as_ref()
            && let Some(result) = self.plan_filtre_pousse_dans_jointure(filter)?
        {
            return Ok(result);
        }
        // Plan the input operator first
        let (input_op, columns) = self.plan_operator(&filter.input)?;

        // Build variable to column index mapping
        let variable_columns: HashMap<String, usize> = columns
            .iter()
            .enumerate()
            .map(|(i, name)| (name.clone(), i))
            .collect();

        // Convert logical expression to filter expression
        let filter_expr = self.convert_expression(&filter.predicate)?;

        // Create the predicate
        let predicate = ExpressionPredicate::new(
            filter_expr,
            variable_columns,
            Arc::clone(&self.store) as Arc<dyn GraphStore>,
        )
        .with_transaction_context(self.viewing_epoch, self.transaction_id)
        .with_session_context(self.session_context.clone());

        // Create the filter operator
        let operator = Box::new(FilterOperator::new(input_op, Box::new(predicate)));

        Ok((operator, columns))
    }

    // ── RETOURNER LE MOTIF QUAND L'IDENTIFIANT DE LA CIBLE EST CONTRAINT ─────
    //
    // Défaut mesuré le 2026-09-12 sur `runtime-dev` (389 147 nœuds, 1 248 011
    // arêtes). Pour `MATCH (a)-[r]->(b) WHERE id(b) IN [1]`, le plan disait :
    //
    // ```text
    // Filter (id(b) In [1])        ← ne PEUT PAS descendre : `b` n'existe pas
    //   Expand (a)->[:*]->(b)         encore sous l'expansion
    //     NodeScan (a:*)           ← scan de TOUS les nœuds, puis O(E)
    // ```
    //
    // soit **216 à 923 ms**, quand la même contrainte sur la SOURCE
    // (`id(a) IN […]`) est poussée sous le scan et coûte **85 µs**. Le coût
    // était CONSTANT quel que soit le nœud visé — 849-850 ms pour quatre nœuds
    // différents, que la réponse fasse 0, 1 ou 2 lignes — ce qui interdisait de
    // l'attribuer à une requête ou à une donnée.
    //
    // On retourne donc l'ORDRE D'EXÉCUTION du motif : la CIBLE devient la graine
    // (le seek d'id existant s'applique alors de lui-même) et la traversée se
    // fait en ENTRANT — `edges_from(b, Incoming)`, donc `incoming_heads` +
    // chaîne `next_to`, en **O(degré)** au lieu de O(E). Aucune liaison ne
    // change : l'ensemble des triplets `(a, r, b)` vérifiant `(b)<-[r]-(a)` est
    // EXACTEMENT celui qui vérifie `(a)-[r]->(b)` — c'est d'ailleurs le plan que
    // produit déjà la forme écrite `MATCH (b)<-[r]-(a)`.
    //
    // Préconditions CONSERVATRICES : si l'une manque, on rend `None` et le plan
    // générique reste en place — donc jamais un résultat modifié.
    fn plan_retournement_sur_cible_id(
        &self,
        predicate: &LogicalExpression,
        expand: &ExpandOp,
    ) -> Result<Option<(Box<dyn Operator>, Vec<String>)>> {
        // ── UN REFUS DOIT DIRE POURQUOI ─────────────────────────────────────
        // Sans cela, un retournement qui ne se déclenche pas se diagnostique à
        // l'aveugle — c'est exactement ce qui s'est produit sur la longueur
        // variable. Poser `OBRAIN_DEBUG_FLIP=1` sur le processus hôte.
        let mut refus: &str = "";
        let sauts_max = expand.max_hops.unwrap_or(1);
        // ── L'ORDRE DES REFUS EST UN COÛT (2026-09-13) ─────────────────────
        //
        // `has_backward_adjacency()` ne lit pas un drapeau : il CONSTRUIT
        // l'index des arêtes entrantes. MESURÉ dans le log d'une requête à
        // motif d'arêtes :
        //
        // ```text
        // lazy build of incoming_heads completed elapsed_ms=309 edges_scanned=1158525
        // ```
        //
        // 309 ms pour balayer 1 158 525 arêtes, payés AU PLAN — et payés par
        // TOUTE requête à motif d'arêtes, puisque cette vérification était en
        // TÊTE de la chaîne de refus. Or la plupart des motifs n'ont aucune
        // contrainte d'id sur la cible (C6 du corpus réel) : le retournement est
        // refusé de toute façon, deux lignes plus bas, pour une raison purement
        // syntaxique. On payait donc 309 ms d'index pour rendre un refus qui ne
        // regarde même pas la base.
        //
        // Les refus GRATUITS sont donc examinés d'abord, et la capacité — qui
        // coûte — est demandée EN DERNIER, quand le retournement est sur le
        // point d'être construit. Le contrôle lui-même est inchangé : on ne
        // retourne toujours que sur une capacité CONSTATÉE.
        if expand.direction != ExpandDirection::Outgoing {
            refus = "sens non sortant";
        } else if expand.min_hops != 1 || sauts_max < 1 {
            refus = "borne basse de sauts";
        } else if sauts_max > PLAFOND_SAUTS_RETOURNEMENT {
            refus = "sauts au-delà du plafond de retournement";
        } else if expand.path_alias.is_some() {
            refus = "chemin nommé";
        } else if expand.path_mode != PathMode::Walk {
            refus = "mode de chemin non Walk";
        } else if expand.from_variable == expand.to_variable {
            refus = "auto-nœud";
        } else if sauts_max > 1 && expand.edge_variable.is_some() {
            refus = "arête liée sur une longueur variable (ordre du chemin)";
        } else if !matches!(expand.input.as_ref(), LogicalOperator::NodeScan(_)) {
            refus = "entrée non-NodeScan";
        } else if let LogicalOperator::NodeScan(scan) = expand.input.as_ref() {
            if scan.variable != expand.from_variable {
                refus = "scan d'une autre variable";
            } else if scan.input.is_some() {
                refus = "scan avec entrée propre";
            } else if !Self::contraint_l_id_de(predicate, &expand.to_variable) {
                refus = "aucune contrainte d'id sur la cible";
            }
        }
        // ── LE REFUS COÛTEUX, EN DERNIER ────────────────────────────────────
        // Voir le pavé en tête de la chaîne : cette vérification CONSTRUIT
        // l'index des arêtes entrantes (309 ms mesurés, 1 158 525 arêtes
        // balayées), et elle n'a de sens que si le retournement est par ailleurs
        // possible. Placée en tête, elle faisait payer ce balayage à toute
        // requête à motif d'arêtes — pour rendre, le plus souvent, un refus qui
        // ne regarde même pas la base.
        if refus.is_empty() && !self.store.has_backward_adjacency() {
            refus = "adjacence entrante non vérifiée sur cette base";
        }
        if std::env::var_os("OBRAIN_DEBUG_FLIP").is_some() {
            eprintln!(
                "[flip] {} — {} -> {} dir={:?} sauts={}..{:?} mode={:?} arete={:?}",
                if refus.is_empty() { "RETOURNÉ" } else { refus },
                expand.from_variable,
                expand.to_variable,
                expand.direction,
                expand.min_hops,
                expand.max_hops,
                expand.path_mode,
                expand.edge_variable,
            );
        }
        if !refus.is_empty() {
            return Ok(None);
        }
        // L'entrée doit être un `NodeScan` NU de la source : avec une entrée
        // propre, on abandonnerait un opérateur porteur de variables.
        let LogicalOperator::NodeScan(scan) = expand.input.as_ref() else {
            return Ok(None);
        };
        if scan.variable != expand.from_variable || scan.input.is_some() {
            return Ok(None);
        }
        // Le label de la graine n'est PAS une raison de renoncer : c'est un
        // simple filtre `hasLabel(a, L)`, que le plan retourné applique
        // AU-DESSUS de l'expansion — exactement le plan de la forme écrite
        // `MATCH (b)<-[r]-(a:L)`. Le refuser laissait 134 ms sur la table
        // (`MATCH (a:`Function`)-[r]->(b) WHERE id(b) IN […]`) là où la
        // retournement en coûte 14.
        let filtre_label = scan
            .label
            .as_ref()
            .map(|label| LogicalExpression::FunctionCall {
                name: "hasLabel".into(),
                args: vec![
                    LogicalExpression::Variable(expand.from_variable.clone()),
                    LogicalExpression::Literal(Value::from(label.as_str())),
                ],
                distinct: false,
            });
        if !Self::contraint_l_id_de(predicate, &expand.to_variable) {
            return Ok(None);
        }
        // Le prédicat se DÉCOUPE : ce qui contraint l'identifiant de la cible
        // amorce le seek, tout le reste reste AU-DESSUS de l'expansion
        // retournée, où `a`, `b` et `r` sont tous liés.
        let cible = expand.to_variable.clone();
        let mut conjoints = Vec::new();
        Self::decouper_conjoints(predicate, &mut conjoints);
        let (graine, mut reste): (Vec<_>, Vec<_>) = conjoints
            .into_iter()
            .partition(|c| Self::contraint_l_id_de(c, &cible));
        let Some(graine) = Self::recoler_conjoints(graine) else {
            return Ok(None);
        };
        if let Some(filtre) = filtre_label {
            reste.push(filtre);
        }
        let source = expand.from_variable.clone();
        let retourne = LogicalOperator::Expand(ExpandOp {
            from_variable: cible.clone(),
            to_variable: source,
            edge_variable: expand.edge_variable.clone(),
            direction: ExpandDirection::Incoming,
            edge_types: expand.edge_types.clone(),
            // Les SAUTS sont ceux du motif d'origine : le retournement change le
            // SENS du parcours, jamais la longueur des chemins demandés.
            min_hops: expand.min_hops,
            max_hops: expand.max_hops,
            input: Box::new(LogicalOperator::Filter(FilterOp {
                predicate: graine,
                pushdown_hint: None,
                input: Box::new(LogicalOperator::NodeScan(NodeScanOp {
                    variable: cible,
                    label: None,
                    input: None,
                    pushdown_hint: None,
                })),
            })),
            path_alias: None,
            path_mode: PathMode::Walk,
        });
        let retourne = match Self::recoler_conjoints(reste) {
            Some(restant) => LogicalOperator::Filter(FilterOp {
                predicate: restant,
                pushdown_hint: None,
                input: Box::new(retourne),
            }),
            None => retourne,
        };
        // On REPASSE par le planificateur : le seek d'id et l'expansion
        // entrante y sont déjà gérés — c'est le plan de la forme écrite en
        // miroir, on ne réimplémente rien.
        let (op, columns) = self.plan_operator(&retourne)?;
        Ok(Some((op, columns)))
    }

    /// Ce prédicat contraint-il l'identifiant de `var` ?
    ///
    /// On délègue aux appariements MAISON (`extract_id_in_list`,
    /// `extract_id_equality`), avec leurs gardes : une seconde définition, plus
    /// permissive, finirait par accepter une forme que le seek refuse, et
    /// changerait la sémantique au lieu de l'accélérer.
    fn contraint_l_id_de(predicate: &LogicalExpression, var: &str) -> bool {
        match predicate {
            LogicalExpression::Binary {
                left,
                op: BinaryOp::And,
                right,
            } => Self::contraint_l_id_de(left, var) || Self::contraint_l_id_de(right, var),
            _ => {
                Self::extract_id_in_list(predicate, var).is_some()
                    || Self::extract_id_equality(predicate, var).is_some()
            }
        }
    }

    /// Découpe un prédicat en conjoints de premier niveau (`A AND B AND C`).
    fn decouper_conjoints(expr: &LogicalExpression, out: &mut Vec<LogicalExpression>) {
        match expr {
            LogicalExpression::Binary {
                left,
                op: BinaryOp::And,
                right,
            } => {
                Self::decouper_conjoints(left, out);
                Self::decouper_conjoints(right, out);
            }
            _ => out.push(expr.clone()),
        }
    }

    /// Recolle des conjoints en un seul prédicat (`None` si la liste est vide).
    fn recoler_conjoints(mut c: Vec<LogicalExpression>) -> Option<LogicalExpression> {
        if c.is_empty() {
            return None;
        }
        let mut acc = c.remove(0);
        for suivant in c {
            acc = LogicalExpression::Binary {
                left: Box::new(acc),
                op: BinaryOp::And,
                right: Box::new(suivant),
            };
        }
        Some(acc)
    }

    /// ── POUSSER UN FILTRE DANS LES CÔTÉS D'UNE JOINTURE ─────────────────────
    ///
    /// `Filter(P) over Join(L, R)` : si `P` ne référence que des variables que
    /// `L` (resp. `R`) produit, on peut l'y évaluer — les colonnes traversent la
    /// jointure, donc le résultat est identique, mais le côté filtré produit
    /// beaucoup moins de lignes AVANT l'appariement. C'est le contrôle de
    /// l'explosion des liens : le coût passe d'un produit des deux scans à la
    /// taille réelle de ce qui est demandé.
    ///
    /// Refus EXPLICITES, tous par prudence et jamais par optimisation :
    ///   * une jointure qui n'est pas `Inner` — sur un `Left`, pousser un
    ///     prédicat du côté droit changerait les lignes complétées par NULL ;
    ///   * une jointure CROISÉE (sans condition) — pousser d'un côté ne réduit
    ///     pas le produit de l'autre ;
    ///   * un prédicat dont une forme n'est pas répertoriée (sous-requête,
    ///     compréhension de liste…) : OUBLIER une variable ferait évaluer le
    ///     prédicat là où elle n'est pas liée. On préfère ne pas pousser.
    /// Dans tous ces cas le plan d'origine reste en place : on perd une
    /// accélération, jamais un résultat.
    fn plan_filtre_pousse_dans_jointure(
        &self,
        filter: &FilterOp,
    ) -> Result<Option<(Box<dyn Operator>, Vec<String>)>> {
        let LogicalOperator::Join(join) = filter.input.as_ref() else {
            return Ok(None);
        };
        if join.join_type != JoinType::Inner || join.conditions.is_empty() {
            return Ok(None);
        }
        let mut variables = HashSet::new();
        if !Self::variables_du_predicat(&filter.predicate, &mut variables) {
            return Ok(None);
        }
        if variables.is_empty() {
            return Ok(None);
        }
        let cote_gauche = Self::collect_plan_variables(&join.left);
        let cote_droit = Self::collect_plan_variables(&join.right);
        let pousser_gauche = variables.is_subset(&cote_gauche);
        let pousser_droit = variables.is_subset(&cote_droit);
        if !pousser_gauche && !pousser_droit {
            return Ok(None);
        }
        // On ne reconstruit QUE ce qui change : un côté sans le prédicat est
        // laissé tel quel.
        let enfiler = |op: &LogicalOperator| {
            LogicalOperator::Filter(FilterOp {
                predicate: filter.predicate.clone(),
                pushdown_hint: None,
                input: Box::new(op.clone()),
            })
        };
        let nouveau = LogicalOperator::Join(JoinOp {
            left: Box::new(if pousser_gauche {
                enfiler(&join.left)
            } else {
                (*join.left).clone()
            }),
            right: Box::new(if pousser_droit {
                enfiler(&join.right)
            } else {
                (*join.right).clone()
            }),
            join_type: join.join_type,
            conditions: join.conditions.clone(),
        });
        let (operator, columns) = self.plan_operator(&nouveau)?;
        Ok(Some((operator, columns)))
    }

    /// Variables qu'un prédicat référence — et `false` dès qu'une forme n'est pas
    /// répertoriée.
    ///
    /// Le booléen est le point important : décider de pousser un prédicat exige
    /// d'être SÛR de ses variables. Une forme inconnue fait donc refuser le
    /// poussinage plutôt que risquer d'oublier une variable.
    fn variables_du_predicat(expr: &LogicalExpression, vars: &mut HashSet<String>) -> bool {
        match expr {
            LogicalExpression::Literal(_) | LogicalExpression::Parameter(_) => true,
            LogicalExpression::Variable(n)
            | LogicalExpression::Id(n)
            | LogicalExpression::Type(n)
            | LogicalExpression::Labels(n) => {
                vars.insert(n.clone());
                true
            }
            LogicalExpression::Property { variable, .. } => {
                vars.insert(variable.clone());
                true
            }
            LogicalExpression::Binary { left, right, .. } => {
                Self::variables_du_predicat(left, vars) && Self::variables_du_predicat(right, vars)
            }
            LogicalExpression::Unary { operand, .. } => Self::variables_du_predicat(operand, vars),
            LogicalExpression::FunctionCall { args, .. } => {
                args.iter().all(|a| Self::variables_du_predicat(a, vars))
            }
            LogicalExpression::List(items) => {
                items.iter().all(|i| Self::variables_du_predicat(i, vars))
            }
            LogicalExpression::Map(pairs) => pairs
                .iter()
                .all(|(_, v)| Self::variables_du_predicat(v, vars)),
            LogicalExpression::IndexAccess { base, index } => {
                Self::variables_du_predicat(base, vars) && Self::variables_du_predicat(index, vars)
            }
            LogicalExpression::SliceAccess { base, start, end } => {
                Self::variables_du_predicat(base, vars)
                    && start
                        .as_ref()
                        .map_or(true, |s| Self::variables_du_predicat(s, vars))
                    && end
                        .as_ref()
                        .map_or(true, |e| Self::variables_du_predicat(e, vars))
            }
            LogicalExpression::Case {
                operand,
                when_clauses,
                else_clause,
            } => {
                operand
                    .as_ref()
                    .map_or(true, |o| Self::variables_du_predicat(o, vars))
                    && when_clauses.iter().all(|(c, r)| {
                        Self::variables_du_predicat(c, vars) && Self::variables_du_predicat(r, vars)
                    })
                    && else_clause
                        .as_ref()
                        .map_or(true, |e| Self::variables_du_predicat(e, vars))
            }
            // Sous-requêtes, compréhensions, prédicats de liste, ou toute forme
            // ajoutée plus tard : on REFUSE de pousser (voir la doc ci-dessus).
            _ => false,
        }
    }

    /// Extracts an EXISTS or NOT EXISTS subquery from a filter predicate for
    /// semi-join rewriting.
    ///
    /// Returns `(subquery, is_negated, remaining_predicate)`.
    ///
    /// At the top level (standalone EXISTS / NOT EXISTS), only complex patterns
    /// that cannot use the inline fast path in `extract_exists_pattern()` are
    /// extracted. Within AND trees, ALL EXISTS patterns are extracted regardless
    /// of complexity: when multiple EXISTS subqueries share a WHERE clause, every
    /// one must go through the semi-join path so the recursive handler in
    /// `plan_exists_as_semi_join` can chain them.
    ///
    /// The Cypher translator builds left-leaning AND trees from sequential WHERE
    /// predicates, so EXISTS nodes sit at the right child of AND nodes at various
    /// depths. The recursive semi-join handler (`plan_exists_as_semi_join`) calls
    /// this function again on each remaining predicate, peeling off one EXISTS
    /// per level until only scalar predicates remain.
    fn extract_complex_exists<'a>(
        &self,
        predicate: &'a LogicalExpression,
        outer_vars: &HashSet<String>,
    ) -> Option<(&'a LogicalOperator, bool, Option<LogicalExpression>)> {
        match predicate {
            LogicalExpression::ExistsSubquery(subplan) => {
                // Top-level EXISTS: only use semi-join for complex patterns.
                // Simple single-hop patterns use the fast path in convert_expression().
                // (2026-08, bug #95-2) EXCEPTION : extremite du motif LIEE au
                // scope externe. La fast path ExistsSubquery ne retient que
                // (start_var, direction, type) — `NOT (ev)-[:SUPPORTS]->(c)`
                // avec c lie devenait « ev n'a AUCUNE arete SUPPORTS ». Le
                // semi/anti-join, lui, joint sur TOUTES les colonnes
                // partagees, extremite comprise : seul chemin correct.
                if self.extract_exists_pattern(subplan).is_err()
                    || Self::exists_pattern_endpoint_is_bound(subplan, outer_vars)
                {
                    Some((subplan.as_ref(), false, None))
                } else {
                    None
                }
            }
            LogicalExpression::Unary {
                op: UnaryOp::Not,
                operand,
            } => {
                if let LogicalExpression::ExistsSubquery(subplan) = operand.as_ref() {
                    // (2026-08, bug #95-2) Meme exception que ci-dessus :
                    // extremite liee -> anti-join, jamais la fast path.
                    if self.extract_exists_pattern(subplan).is_err()
                        || Self::exists_pattern_endpoint_is_bound(subplan, outer_vars)
                    {
                        Some((subplan.as_ref(), true, None))
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            LogicalExpression::Binary {
                op: BinaryOp::And,
                left,
                right,
            } => {
                // In AND context, extract ANY EXISTS (simple or complex).
                // When multiple EXISTS appear in the same WHERE, extracting the
                // first one (even if simple) lets the recursive semi-join handler
                // find and extract the remaining complex ones from the rest.
                if let Some((subplan, negated)) = Self::extract_exists_from_expr(left) {
                    return Some((subplan, negated, Some(right.as_ref().clone())));
                }
                if let Some((subplan, negated)) = Self::extract_exists_from_expr(right) {
                    return Some((subplan, negated, Some(left.as_ref().clone())));
                }
                // Recurse into left subtree (handles left-leaning AND trees where
                // EXISTS nodes are buried deeper than immediate children)
                if let Some((subplan, negated, inner_remaining)) =
                    self.extract_complex_exists(left, outer_vars)
                {
                    let remaining = match inner_remaining {
                        Some(inner) => LogicalExpression::Binary {
                            left: Box::new(inner),
                            op: BinaryOp::And,
                            right: right.clone(),
                        },
                        None => right.as_ref().clone(),
                    };
                    return Some((subplan, negated, Some(remaining)));
                }
                // Recurse into right subtree
                if let Some((subplan, negated, inner_remaining)) =
                    self.extract_complex_exists(right, outer_vars)
                {
                    let remaining = match inner_remaining {
                        Some(inner) => LogicalExpression::Binary {
                            left: left.clone(),
                            op: BinaryOp::And,
                            right: Box::new(inner),
                        },
                        None => left.as_ref().clone(),
                    };
                    return Some((subplan, negated, Some(remaining)));
                }
                None
            }
            _ => None,
        }
    }

    /// (bug #95-2) Vrai quand l'extremite du motif EXISTS simple est une
    /// variable deja liee dans le scope externe.
    ///
    /// La fast path `FilterExpression::ExistsSubquery` PERD cette variable
    /// (elle ne code que start_var/direction/type/labels d'extremite) : elle
    /// testerait « une arete existe », pas « une arete vers CE noeud ». Meme
    /// marche que `extract_exists_pattern` : Expand au sommet, eventuellement
    /// sous des NodeScan chaines.
    fn exists_pattern_endpoint_is_bound(
        subplan: &LogicalOperator,
        outer_vars: &HashSet<String>,
    ) -> bool {
        match subplan {
            LogicalOperator::Expand(expand) => outer_vars.contains(&expand.to_variable),
            LogicalOperator::NodeScan(scan) => scan
                .input
                .as_deref()
                .is_some_and(|i| Self::exists_pattern_endpoint_is_bound(i, outer_vars)),
            _ => false,
        }
    }

    /// Sur-approximation syntaxique des variables liees par un plan logique.
    ///
    /// Sert au routage EXISTS fast-path / semi-join AVANT toute
    /// planification physique. Sur-approximer est sans danger : au pire un
    /// motif simple part en semi-join (correct, un peu moins direct) ;
    /// sous-approximer raterait une extremite liee et garderait la fast
    /// path fausse.
    fn collect_plan_variables(op: &LogicalOperator) -> HashSet<String> {
        let mut vars = HashSet::new();
        Self::collect_plan_variables_into(op, &mut vars);
        vars
    }

    fn collect_plan_variables_into(op: &LogicalOperator, vars: &mut HashSet<String>) {
        match op {
            LogicalOperator::NodeScan(s) => {
                vars.insert(s.variable.clone());
            }
            LogicalOperator::Expand(e) => {
                vars.insert(e.from_variable.clone());
                vars.insert(e.to_variable.clone());
                if let Some(ev) = &e.edge_variable {
                    vars.insert(ev.clone());
                }
            }
            LogicalOperator::Unwind(u) => {
                vars.insert(u.variable.clone());
                if let Some(o) = &u.ordinality_var {
                    vars.insert(o.clone());
                }
            }
            LogicalOperator::ParameterScan(p) => {
                vars.extend(p.columns.iter().cloned());
            }
            LogicalOperator::Return(r) => {
                for item in &r.items {
                    if let Some(alias) = &item.alias {
                        vars.insert(alias.clone());
                    } else if let LogicalExpression::Variable(name) = &item.expression {
                        vars.insert(name.clone());
                    }
                }
            }
            _ => {}
        }
        for child in op.children() {
            Self::collect_plan_variables_into(child, vars);
        }
    }

    /// Helper: extracts EXISTS or NOT EXISTS from a single expression node.
    /// Returns `(subplan, is_negated)`.
    fn extract_exists_from_expr(expr: &LogicalExpression) -> Option<(&LogicalOperator, bool)> {
        match expr {
            LogicalExpression::ExistsSubquery(subplan) => Some((subplan.as_ref(), false)),
            LogicalExpression::Unary {
                op: UnaryOp::Not,
                operand,
            } => {
                if let LogicalExpression::ExistsSubquery(subplan) = operand.as_ref() {
                    Some((subplan.as_ref(), true))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Plans a complex EXISTS/NOT EXISTS as a hash-based semi-join or anti-join.
    ///
    /// The inner subquery is planned as a full operator tree via `plan_operator()`.
    /// Variables shared between the outer input and inner subquery become equi-join
    /// keys. Uses `HashJoinOperator` for efficient O(N + M) evaluation.
    ///
    /// When the inner subquery contains a `ParameterScan` (indicating correlated
    /// variable references from the outer scope), a correlated `ApplyOperator`
    /// with EXISTS mode is used instead of a hash-based semi-join.
    fn plan_exists_as_semi_join(
        &self,
        outer_input: &LogicalOperator,
        subquery: &LogicalOperator,
        is_negated: bool,
        remaining_predicate: Option<LogicalExpression>,
    ) -> Result<(Box<dyn Operator>, Vec<String>)> {
        // Detect correlated subquery (contains ParameterScan from translator)
        if let Some(param_vars) = Self::extract_parameter_scan_vars(subquery) {
            return self.plan_correlated_exists(
                outer_input,
                subquery,
                &param_vars,
                is_negated,
                remaining_predicate,
            );
        }

        let (left_op, left_columns) = self.plan_operator(outer_input)?;
        let (right_op, right_columns) = self.plan_operator(subquery)?;

        // Semi/anti join only outputs left (outer) columns
        let output_columns = left_columns.clone();

        // Find shared variables for equi-join keys
        let mut probe_keys = Vec::new();
        let mut build_keys = Vec::new();
        for (right_idx, right_col) in right_columns.iter().enumerate() {
            if let Some(left_idx) = left_columns.iter().position(|c| c == right_col) {
                probe_keys.push(left_idx);
                build_keys.push(right_idx);
            }
        }

        let output_schema = self.derive_schema_from_columns(&output_columns);

        let join_type = if is_negated {
            PhysicalJoinType::Anti
        } else {
            PhysicalJoinType::Semi
        };

        let join_op: Box<dyn Operator> = Box::new(HashJoinOperator::new(
            left_op,
            right_op,
            probe_keys,
            build_keys,
            join_type,
            output_schema,
        ));

        // If there's a remaining predicate (from AND splitting), check if it
        // contains more EXISTS subqueries that need semi-join rewriting.
        if let Some(ref remaining) = remaining_predicate {
            // Recursively handle nested EXISTS in the remaining predicate
            let outer_vars: HashSet<String> = output_columns.iter().cloned().collect();
            if let Some((nested_sub, nested_neg, nested_rest)) =
                self.extract_complex_exists(remaining, &outer_vars)
            {
                return self.plan_exists_as_semi_join_with_input(
                    join_op,
                    &output_columns,
                    nested_sub,
                    nested_neg,
                    nested_rest,
                );
            }

            let variable_columns: HashMap<String, usize> = output_columns
                .iter()
                .enumerate()
                .map(|(i, name)| (name.clone(), i))
                .collect();
            let filter_expr = self.convert_expression(remaining)?;
            let predicate = ExpressionPredicate::new(
                filter_expr,
                variable_columns,
                Arc::clone(&self.store) as Arc<dyn GraphStore>,
            )
            .with_transaction_context(self.viewing_epoch, self.transaction_id)
            .with_session_context(self.session_context.clone());
            let filter_op = Box::new(FilterOperator::new(join_op, Box::new(predicate)));
            return Ok((filter_op, output_columns));
        }

        Ok((join_op, output_columns))
    }

    /// Extracts `ParameterScan` variable names from a logical plan, if present.
    ///
    /// Returns `Some(vars)` when the plan contains a `ParameterScan` node
    /// (indicating a correlated subquery from the translator).
    fn extract_parameter_scan_vars(plan: &LogicalOperator) -> Option<Vec<String>> {
        match plan {
            LogicalOperator::ParameterScan(ps) => Some(ps.columns.clone()),
            LogicalOperator::Filter(f) => Self::extract_parameter_scan_vars(&f.input),
            LogicalOperator::Join(j) => Self::extract_parameter_scan_vars(&j.left)
                .or_else(|| Self::extract_parameter_scan_vars(&j.right)),
            LogicalOperator::NodeScan(s) => s
                .input
                .as_ref()
                .and_then(|i| Self::extract_parameter_scan_vars(i)),
            LogicalOperator::Expand(e) => Self::extract_parameter_scan_vars(&e.input),
            _ => None,
        }
    }

    /// Plans a correlated EXISTS/NOT EXISTS using `ApplyOperator` with EXISTS mode.
    ///
    /// The inner subquery contains a `ParameterScan` for outer variable references.
    /// For each outer row, the inner subquery is executed with the outer values
    /// injected via `ParameterState`. Semi-join keeps rows where inner has results,
    /// anti-join keeps rows where inner has no results.
    fn plan_correlated_exists(
        &self,
        outer_input: &LogicalOperator,
        subquery: &LogicalOperator,
        param_vars: &[String],
        is_negated: bool,
        remaining_predicate: Option<LogicalExpression>,
    ) -> Result<(Box<dyn Operator>, Vec<String>)> {
        let (left_op, left_columns) = self.plan_operator(outer_input)?;

        // Set up ParameterState for correlated variables
        let param_state = std::sync::Arc::new(
            obrain_core::execution::operators::ParameterState::new(param_vars.to_vec()),
        );
        let param_col_indices: Vec<usize> = param_vars
            .iter()
            .map(|var| left_columns.iter().position(|c| c == var).unwrap_or(0))
            .collect();

        // Plan inner subquery with correlated context
        *self.correlated_param_state.borrow_mut() = Some(std::sync::Arc::clone(&param_state));
        let (inner_op, _inner_columns) = self.plan_operator(subquery)?;
        *self.correlated_param_state.borrow_mut() = None;

        // Create Apply with EXISTS mode (semi or anti join)
        let op = ApplyOperator::new_correlated(left_op, inner_op, param_state, param_col_indices)
            .with_exists_mode(!is_negated);

        let output_columns = left_columns;
        let mut result: Box<dyn Operator> = Box::new(op);

        // Handle remaining predicate (from AND splitting)
        if let Some(ref remaining) = remaining_predicate {
            let outer_vars: HashSet<String> = output_columns.iter().cloned().collect();
            if let Some((nested_sub, nested_neg, nested_rest)) =
                self.extract_complex_exists(remaining, &outer_vars)
            {
                return self.plan_exists_as_semi_join_with_input(
                    result,
                    &output_columns,
                    nested_sub,
                    nested_neg,
                    nested_rest,
                );
            }

            let variable_columns: HashMap<String, usize> = output_columns
                .iter()
                .enumerate()
                .map(|(i, name)| (name.clone(), i))
                .collect();
            let filter_expr = self.convert_expression(remaining)?;
            let predicate = ExpressionPredicate::new(
                filter_expr,
                variable_columns,
                Arc::clone(&self.store) as Arc<dyn GraphStore>,
            )
            .with_transaction_context(self.viewing_epoch, self.transaction_id)
            .with_session_context(self.session_context.clone());
            result = Box::new(FilterOperator::new(result, Box::new(predicate)));
        }

        Ok((result, output_columns))
    }

    /// Plans an EXISTS/NOT EXISTS semi-join with an already-planned outer input.
    ///
    /// Used when the remaining predicate from a prior semi-join contains additional
    /// EXISTS subqueries that also need semi-join rewriting (nested NOT EXISTS).
    fn plan_exists_as_semi_join_with_input(
        &self,
        left_op: Box<dyn Operator>,
        left_columns: &[String],
        subquery: &LogicalOperator,
        is_negated: bool,
        remaining_predicate: Option<LogicalExpression>,
    ) -> Result<(Box<dyn Operator>, Vec<String>)> {
        let (right_op, right_columns) = self.plan_operator(subquery)?;

        let output_columns = left_columns.to_vec();

        let mut probe_keys = Vec::new();
        let mut build_keys = Vec::new();
        for (right_idx, right_col) in right_columns.iter().enumerate() {
            if let Some(left_idx) = left_columns.iter().position(|c| c == right_col) {
                probe_keys.push(left_idx);
                build_keys.push(right_idx);
            }
        }

        let output_schema = self.derive_schema_from_columns(&output_columns);
        let join_type = if is_negated {
            PhysicalJoinType::Anti
        } else {
            PhysicalJoinType::Semi
        };

        let join_op: Box<dyn Operator> = Box::new(HashJoinOperator::new(
            left_op,
            right_op,
            probe_keys,
            build_keys,
            join_type,
            output_schema,
        ));

        // Recursively handle any further EXISTS in the remaining predicate
        if let Some(ref remaining) = remaining_predicate {
            let outer_vars: HashSet<String> = output_columns.iter().cloned().collect();
            if let Some((nested_sub, nested_neg, nested_rest)) =
                self.extract_complex_exists(remaining, &outer_vars)
            {
                return self.plan_exists_as_semi_join_with_input(
                    join_op,
                    &output_columns,
                    nested_sub,
                    nested_neg,
                    nested_rest,
                );
            }

            let variable_columns: HashMap<String, usize> = output_columns
                .iter()
                .enumerate()
                .map(|(i, name)| (name.clone(), i))
                .collect();
            let filter_expr = self.convert_expression(remaining)?;
            let predicate = ExpressionPredicate::new(
                filter_expr,
                variable_columns,
                Arc::clone(&self.store) as Arc<dyn GraphStore>,
            )
            .with_transaction_context(self.viewing_epoch, self.transaction_id)
            .with_session_context(self.session_context.clone());
            let filter_op = Box::new(FilterOperator::new(join_op, Box::new(predicate)));
            return Ok((filter_op, output_columns));
        }

        Ok((join_op, output_columns))
    }

    /// Extracts a COUNT subquery comparison from a filter predicate.
    ///
    /// Recognizes patterns like:
    /// - `COUNT { MATCH ... } > 5`
    /// - `COUNT { MATCH ... } = 0`
    /// - `5 < COUNT { MATCH ... }` (reversed operands)
    ///
    /// Returns `(subquery, comparison_op, threshold_value, remaining_predicate)`.
    fn extract_count_comparison(
        predicate: &LogicalExpression,
    ) -> Option<(
        &LogicalOperator,
        BinaryOp,
        &LogicalExpression,
        Option<&LogicalExpression>,
    )> {
        match predicate {
            LogicalExpression::Binary { left, op, right } => {
                // Check for AND-combined: extract COUNT comparison from either side
                if *op == BinaryOp::And {
                    if let Some(result) = Self::extract_count_from_binary(left) {
                        return Some((result.0, result.1, result.2, Some(right)));
                    }
                    if let Some(result) = Self::extract_count_from_binary(right) {
                        return Some((result.0, result.1, result.2, Some(left)));
                    }
                    return None;
                }

                // Direct comparison: COUNT { ... } op value
                Self::extract_count_from_binary(predicate)
                    .map(|(sub, op, threshold)| (sub, op, threshold, None))
            }
            _ => None,
        }
    }

    /// Helper: extracts COUNT subquery comparison from a binary expression.
    fn extract_count_from_binary(
        expr: &LogicalExpression,
    ) -> Option<(&LogicalOperator, BinaryOp, &LogicalExpression)> {
        if let LogicalExpression::Binary { left, op, right } = expr {
            match op {
                BinaryOp::Eq
                | BinaryOp::Ne
                | BinaryOp::Gt
                | BinaryOp::Ge
                | BinaryOp::Lt
                | BinaryOp::Le => {
                    // COUNT { ... } op literal
                    if let LogicalExpression::CountSubquery(subplan) = left.as_ref() {
                        return Some((subplan.as_ref(), *op, right.as_ref()));
                    }
                    // literal op COUNT { ... } (flip the operator)
                    if let LogicalExpression::CountSubquery(subplan) = right.as_ref() {
                        let flipped = match op {
                            BinaryOp::Gt => BinaryOp::Lt,
                            BinaryOp::Ge => BinaryOp::Le,
                            BinaryOp::Lt => BinaryOp::Gt,
                            BinaryOp::Le => BinaryOp::Ge,
                            other => *other, // Eq/Ne are symmetric
                        };
                        return Some((subplan.as_ref(), flipped, left.as_ref()));
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// Plans a COUNT subquery comparison as Join + Aggregate + Filter.
    ///
    /// Rewrites `WHERE COUNT { MATCH pattern } > N` into:
    /// 1. Inner join on shared variables to get all matches per outer row
    /// 2. Aggregate(COUNT) grouped by outer columns
    /// 3. Filter(count > N) on the aggregated result
    fn plan_count_as_apply(
        &self,
        outer_input: &LogicalOperator,
        subquery: &LogicalOperator,
        op: BinaryOp,
        threshold: &LogicalExpression,
        remaining_predicate: Option<&LogicalExpression>,
    ) -> Result<(Box<dyn Operator>, Vec<String>)> {
        let (left_op, left_columns) = self.plan_operator(outer_input)?;
        let (right_op, right_columns) = self.plan_operator(subquery)?;

        let output_columns = left_columns.clone();

        // Find shared variables for equi-join keys
        let mut probe_keys = Vec::new();
        let mut build_keys = Vec::new();
        for (right_idx, right_col) in right_columns.iter().enumerate() {
            if let Some(left_idx) = left_columns.iter().position(|c| c == right_col) {
                probe_keys.push(left_idx);
                build_keys.push(right_idx);
            }
        }

        // Left join to preserve outer rows with no matches (needed for COUNT = 0).
        // The join physically outputs both left and right columns.
        let mut join_columns = left_columns.clone();
        join_columns.extend(right_columns.iter().cloned());
        let join_schema = self.derive_schema_from_columns(&join_columns);
        let join_op: Box<dyn Operator> = Box::new(HashJoinOperator::new(
            left_op,
            right_op,
            probe_keys,
            build_keys,
            PhysicalJoinType::Left,
            join_schema,
        ));

        // Aggregate: COUNT(right_col) grouped by all outer columns.
        // Using COUNT on a right-side column so nulls (no match) produce 0 instead of 1.
        let count_alias = "_count_subquery_".to_string();
        let mut agg_columns = output_columns.clone();
        agg_columns.push(count_alias.clone());

        let group_keys: Vec<usize> = (0..output_columns.len()).collect();
        // Pick the first right-side column for COUNT (index = left_columns.len())
        let right_col_idx = left_columns.len();
        let agg_exprs = vec![PhysicalAggregateExpr::count(right_col_idx)];
        let agg_schema = self.derive_schema_from_columns(&agg_columns);

        let agg_op: Box<dyn Operator> = Box::new(HashAggregateOperator::new(
            join_op, group_keys, agg_exprs, agg_schema,
        ));

        // Filter: _count_ op threshold
        let threshold_expr = convert_filter_expression(threshold)?;
        let count_var_columns: HashMap<String, usize> = agg_columns
            .iter()
            .enumerate()
            .map(|(i, name)| (name.clone(), i))
            .collect();
        let filter_op_code = convert_binary_op(op)?;
        let count_filter = FilterExpression::Binary {
            left: Box::new(FilterExpression::Variable(count_alias)),
            op: filter_op_code,
            right: Box::new(threshold_expr),
        };

        let predicate = ExpressionPredicate::new(
            count_filter,
            count_var_columns.clone(),
            Arc::clone(&self.store) as Arc<dyn GraphStore>,
        )
        .with_transaction_context(self.viewing_epoch, self.transaction_id)
        .with_session_context(self.session_context.clone());
        let mut result_op: Box<dyn Operator> =
            Box::new(FilterOperator::new(agg_op, Box::new(predicate)));

        // If there's a remaining predicate, apply it too
        if let Some(remaining) = remaining_predicate {
            let remaining_expr = self.convert_expression(remaining)?;
            let remaining_predicate = ExpressionPredicate::new(
                remaining_expr,
                count_var_columns,
                Arc::clone(&self.store) as Arc<dyn GraphStore>,
            )
            .with_transaction_context(self.viewing_epoch, self.transaction_id)
            .with_session_context(self.session_context.clone());
            result_op = Box::new(FilterOperator::new(
                result_op,
                Box::new(remaining_predicate),
            ));
        }

        Ok((result_op, output_columns))
    }

    /// Checks zone maps for a predicate to see if we can skip the scan entirely.
    ///
    /// Returns:
    /// - `Some(false)` if zone map proves no matches possible (can skip)
    /// - `Some(true)` if zone map says matches might exist
    /// - `None` if zone map check not applicable
    pub(super) fn check_zone_map_for_predicate(
        &self,
        predicate: &LogicalExpression,
    ) -> Option<bool> {
        use obrain_core::graph::lpg::CompareOp;

        match predicate {
            LogicalExpression::Binary { left, op, right } => {
                // Check for AND/OR first (compound conditions)
                match op {
                    BinaryOp::And => {
                        let left_result = self.check_zone_map_for_predicate(left);
                        let right_result = self.check_zone_map_for_predicate(right);

                        return match (left_result, right_result) {
                            // If either side definitely won't match, the AND won't match
                            (Some(false), _) | (_, Some(false)) => Some(false),
                            // If both might match, might match overall
                            (Some(true), Some(true)) => Some(true),
                            // Otherwise, can't determine
                            _ => None,
                        };
                    }
                    BinaryOp::Or => {
                        let left_result = self.check_zone_map_for_predicate(left);
                        let right_result = self.check_zone_map_for_predicate(right);

                        return match (left_result, right_result) {
                            // Both sides definitely won't match
                            (Some(false), Some(false)) => Some(false),
                            // At least one side might match
                            (Some(true), _) | (_, Some(true)) => Some(true),
                            // Otherwise, can't determine
                            _ => None,
                        };
                    }
                    _ => {}
                }

                // Simple property comparison: n.property op value
                let (property, compare_op, value) = match (left.as_ref(), right.as_ref()) {
                    (
                        LogicalExpression::Property { property, .. },
                        LogicalExpression::Literal(val),
                    ) => {
                        let cmp = match op {
                            BinaryOp::Eq => CompareOp::Eq,
                            BinaryOp::Ne => CompareOp::Ne,
                            BinaryOp::Lt => CompareOp::Lt,
                            BinaryOp::Le => CompareOp::Le,
                            BinaryOp::Gt => CompareOp::Gt,
                            BinaryOp::Ge => CompareOp::Ge,
                            _ => return None,
                        };
                        (property.clone(), cmp, val.clone())
                    }
                    (
                        LogicalExpression::Literal(val),
                        LogicalExpression::Property { property, .. },
                    ) => {
                        // Flip comparison for reversed operands
                        let cmp = match op {
                            BinaryOp::Eq => CompareOp::Eq,
                            BinaryOp::Ne => CompareOp::Ne,
                            BinaryOp::Lt => CompareOp::Gt, // val < prop means prop > val
                            BinaryOp::Le => CompareOp::Ge,
                            BinaryOp::Gt => CompareOp::Lt,
                            BinaryOp::Ge => CompareOp::Le,
                            _ => return None,
                        };
                        (property.clone(), cmp, val.clone())
                    }
                    _ => return None,
                };

                // Check zone map for node properties
                let might_match =
                    self.store
                        .node_property_might_match(&property.into(), compare_op, &value);

                Some(might_match)
            }

            _ => None,
        }
    }

    /// Tries to use a property index for filter optimization.
    ///
    /// When a filter predicate is an equality check on an indexed property,
    /// and the input is a simple NodeScan, we can use the index to look up
    /// matching nodes directly instead of scanning all nodes.
    /// Optimizes `WHERE id(n) = X` into a direct O(1) node seek.
    ///
    /// Instead of scanning all nodes and filtering, this directly looks up
    /// the node by its internal ID using `store.get_node()`.
    ///
    /// Also handles `WHERE id(n) = X AND <other predicates>` by extracting
    /// the id condition and wrapping remaining predicates in a FilterOperator.
    ///
    /// Also handles range patterns like `WHERE id(n) < X` or `WHERE id(n) >= X AND id(n) < Y`.
    pub(super) fn try_plan_filter_with_id_lookup(
        &self,
        filter: &FilterOp,
    ) -> Result<Option<(Box<dyn Operator>, Vec<String>)>> {
        // Only optimize if input is a simple NodeScan (not nested)
        let scan_variable = match filter.input.as_ref() {
            LogicalOperator::NodeScan(scan) if scan.input.is_none() => scan.variable.clone(),
            _ => return Ok(None),
        };

        // Try to extract id() equality from the predicate
        if let Some((variable, node_id)) =
            Self::extract_id_equality(&filter.predicate, &scan_variable)
        {
            // Direct O(1) lookup
            let node_exists = if let Some(tx) = self.transaction_id {
                self.store
                    .get_node_versioned(node_id, self.viewing_epoch, tx)
                    .is_some()
            } else {
                self.store
                    .get_node_at_epoch(node_id, self.viewing_epoch)
                    .is_some()
            };

            let columns = vec![variable.clone()];

            if !node_exists {
                let schema = vec![LogicalType::Node];
                return Ok(Some((Box::new(EmptyOperator::new(schema)), columns)));
            }

            let node_list_op: Box<dyn Operator> =
                Box::new(NodeListOperator::new(vec![node_id], 2048));

            // Check for remaining predicates (e.g., `id(n) = 42 AND n.age > 30`)
            if let Some(remaining) =
                Self::extract_remaining_after_id(&filter.predicate, &scan_variable)
            {
                let variable_columns: HashMap<String, usize> = columns
                    .iter()
                    .enumerate()
                    .map(|(i, name)| (name.clone(), i))
                    .collect();
                let filter_expr = self.convert_expression(&remaining)?;
                let predicate = ExpressionPredicate::new(
                    filter_expr,
                    variable_columns,
                    Arc::clone(&self.store) as Arc<dyn GraphStore>,
                )
                .with_transaction_context(self.viewing_epoch, self.transaction_id)
                .with_session_context(self.session_context.clone());
                let filtered = Box::new(FilterOperator::new(node_list_op, Box::new(predicate)));
                return Ok(Some((filtered, columns)));
            }

            return Ok(Some((node_list_op, columns)));
        }

        // Try to extract id() membership: id(n) IN [a, b, c] (multi-seed seek).
        // Sans cette branche, le motif entier retombe sur un scan complet de
        // la table des arêtes (~137 ms par requête, même pour 1 seul id sur
        // 4,8 M arêtes) au lieu de l'expansion par adjacence utilisée pour
        // `id(n) = X` (0,35 ms) — c'est le goulot des BFS multi-centres.
        if let Some((variable, list_ids)) =
            Self::extract_id_in_list(&filter.predicate, &scan_variable)
        {
            // Filtre MVCC (même sémantique que le seek égalité ci-dessus).
            let epoch = self.viewing_epoch;
            let tx_id = self.transaction_id;
            let visible_ids: Vec<NodeId> = list_ids
                .into_iter()
                .filter(|&id| {
                    if let Some(tx) = tx_id {
                        self.store.get_node_versioned(id, epoch, tx).is_some()
                    } else {
                        self.store.get_node_at_epoch(id, epoch).is_some()
                    }
                })
                .collect();

            let columns = vec![variable];

            // Prédicats restants éventuels (`id(n) IN [...] AND n.age > 30`) :
            // le membre In reste dans `remaining` (redondant mais correct — le
            // NodeListOperator a déjà filtré sur la liste).
            if let Some(remaining) =
                Self::extract_remaining_after_id(&filter.predicate, &scan_variable)
            {
                let variable_columns: HashMap<String, usize> = columns
                    .iter()
                    .enumerate()
                    .map(|(i, name)| (name.clone(), i))
                    .collect();
                let filter_expr = self.convert_expression(&remaining)?;
                let predicate = ExpressionPredicate::new(
                    filter_expr,
                    variable_columns,
                    Arc::clone(&self.store) as Arc<dyn GraphStore>,
                )
                .with_transaction_context(self.viewing_epoch, self.transaction_id)
                .with_session_context(self.session_context.clone());
                let node_list_op: Box<dyn Operator> =
                    Box::new(NodeListOperator::new(visible_ids, 2048));
                let filtered = Box::new(FilterOperator::new(node_list_op, Box::new(predicate)));
                return Ok(Some((filtered, columns)));
            }

            let node_list_op: Box<dyn Operator> =
                Box::new(NodeListOperator::new(visible_ids, 2048));
            return Ok(Some((node_list_op, columns)));
        }

        // Try to extract id() range conditions: id(n) < X, id(n) >= X, etc.
        if let Some((variable, range_ids)) =
            self.extract_id_range(&filter.predicate, &scan_variable)
        {
            // Filter by MVCC visibility
            let epoch = self.viewing_epoch;
            let tx_id = self.transaction_id;
            let visible_ids: Vec<NodeId> = range_ids
                .into_iter()
                .filter(|&id| {
                    if let Some(tx) = tx_id {
                        self.store.get_node_versioned(id, epoch, tx).is_some()
                    } else {
                        self.store.get_node_at_epoch(id, epoch).is_some()
                    }
                })
                .collect();

            let columns = vec![variable];
            let node_list_op: Box<dyn Operator> =
                Box::new(NodeListOperator::new(visible_ids, 2048));
            return Ok(Some((node_list_op, columns)));
        }

        Ok(None)
    }

    /// Extracts `id(variable) = <int_literal>` from a predicate.
    ///
    /// Handles both top-level equality and equality within AND chains.
    /// Returns `(variable_name, NodeId)` if found.
    fn extract_id_equality(
        predicate: &LogicalExpression,
        target_variable: &str,
    ) -> Option<(String, NodeId)> {
        match predicate {
            // Simple case: id(n) = 42
            LogicalExpression::Binary {
                left,
                op: BinaryOp::Eq,
                right,
            } => Self::match_id_eq_literal(left, right, target_variable),

            // AND chain: id(n) = 42 AND ...
            LogicalExpression::Binary {
                left,
                op: BinaryOp::And,
                right,
            } => Self::extract_id_equality(left, target_variable)
                .or_else(|| Self::extract_id_equality(right, target_variable)),

            _ => None,
        }
    }

    /// Extracts the variable name from an id() expression.
    ///
    /// Handles both representations:
    /// - `LogicalExpression::Id("n")` (GraphQL/Gremlin translators)
    /// - `FunctionCall { name: "id", args: [Variable("n")] }` (GQL/Cypher translators)
    fn extract_id_variable(expr: &LogicalExpression) -> Option<&str> {
        match expr {
            LogicalExpression::Id(var) => Some(var.as_str()),
            LogicalExpression::FunctionCall { name, args, .. }
                if name.eq_ignore_ascii_case("id")
                    && args.len() == 1
                    && matches!(&args[0], LogicalExpression::Variable(_)) =>
            {
                if let LogicalExpression::Variable(var) = &args[0] {
                    Some(var.as_str())
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Matches `id(var) = Literal(Int64)` or the reverse.
    ///
    /// Handles both `Id("n")` and `FunctionCall("id", [Variable("n")])` forms.
    fn match_id_eq_literal(
        left: &LogicalExpression,
        right: &LogicalExpression,
        target_variable: &str,
    ) -> Option<(String, NodeId)> {
        // id(n) = 42
        if let Some(var) = Self::extract_id_variable(left)
            && var == target_variable
            && let LogicalExpression::Literal(Value::Int64(id)) = right
            && *id >= 0
        {
            return Some((var.to_string(), NodeId::new(*id as u64)));
        }
        // 42 = id(n)
        if let Some(var) = Self::extract_id_variable(right)
            && var == target_variable
            && let LogicalExpression::Literal(Value::Int64(id)) = left
            && *id >= 0
        {
            return Some((var.to_string(), NodeId::new(*id as u64)));
        }
        None
    }

    /// Extracts `id(var) IN [literals]` from a predicate (top-level ou dans
    /// une chaîne AND). Retourne `(variable, ids)` quand le prédicat porte un
    /// membre `In` sur la variable cible.
    fn extract_id_in_list(
        predicate: &LogicalExpression,
        target_variable: &str,
    ) -> Option<(String, Vec<NodeId>)> {
        match predicate {
            LogicalExpression::Binary {
                left,
                op: BinaryOp::And,
                right,
            } => Self::extract_id_in_list(left, target_variable)
                .or_else(|| Self::extract_id_in_list(right, target_variable)),
            LogicalExpression::Binary {
                left,
                op: BinaryOp::In,
                right,
            } => Self::match_id_in_list(left, right, target_variable),
            _ => None,
        }
    }

    /// Matches `id(var) IN [1, 2, 3]` (id() à gauche, liste littérale à
    /// droite). Ne convertit que des littéraux entiers ≥ 0 ; tout autre
    /// élément (ou une liste vide au-delà de la taille de garde) fait retomber
    /// sur le chemin générique pour ne jamais changer la sémantique.
    fn match_id_in_list(
        left: &LogicalExpression,
        right: &LogicalExpression,
        target_variable: &str,
    ) -> Option<(String, Vec<NodeId>)> {
        let var = Self::extract_id_variable(left)?;
        if var != target_variable {
            return None;
        }
        let LogicalExpression::List(items) = right else {
            return None;
        };
        // Garde : une liste démesurée ne doit pas matérialiser un vecteur fou
        // (miroir du plafond de `extract_id_range`).
        if items.len() > 100_000 {
            return None;
        }
        let mut ids: Vec<NodeId> = Vec::with_capacity(items.len());
        for item in items {
            match item {
                LogicalExpression::Literal(Value::Int64(id)) if *id >= 0 => {
                    ids.push(NodeId::new(*id as u64));
                }
                _ => return None,
            }
        }
        Some((var.to_string(), ids))
    }

    /// Checks if an expression is an id() call on the target variable.
    #[allow(dead_code)]
    fn is_id_expression(expr: &LogicalExpression, target_variable: &str) -> bool {
        Self::extract_id_variable(expr).is_some_and(|var| var == target_variable)
    }

    /// Extracts remaining predicates after removing the `id(n) = X` part.
    fn extract_remaining_after_id(
        predicate: &LogicalExpression,
        target_variable: &str,
    ) -> Option<LogicalExpression> {
        match predicate {
            LogicalExpression::Binary {
                left,
                op: BinaryOp::And,
                right,
            } => {
                let left_remaining = Self::extract_remaining_after_id(left, target_variable);
                let right_remaining = Self::extract_remaining_after_id(right, target_variable);

                match (left_remaining, right_remaining) {
                    (Some(l), Some(r)) => Some(LogicalExpression::Binary {
                        left: Box::new(l),
                        op: BinaryOp::And,
                        right: Box::new(r),
                    }),
                    (Some(l), None) => Some(l),
                    (None, Some(r)) => Some(r),
                    (None, None) => None,
                }
            }
            LogicalExpression::Binary {
                left,
                op: BinaryOp::Eq,
                right,
            } => {
                // If this is the id() = X condition, remove it
                if Self::match_id_eq_literal(left, right, target_variable).is_some() {
                    None
                } else {
                    Some(predicate.clone())
                }
            }
            _ => Some(predicate.clone()),
        }
    }

    /// Extracts id() range conditions like `id(n) < 1000` or `id(n) >= 0 AND id(n) < 1000`.
    ///
    /// Returns `(variable, Vec<NodeId>)` with all IDs in the range.
    /// Only handles ranges up to 100_000 to avoid materializing huge vectors.
    fn extract_id_range(
        &self,
        predicate: &LogicalExpression,
        target_variable: &str,
    ) -> Option<(String, Vec<NodeId>)> {
        let mut lower: i64 = 0;
        let mut upper: Option<i64> = None;
        let mut variable = String::new();

        if !Self::collect_id_range_bounds(
            predicate,
            target_variable,
            &mut lower,
            &mut upper,
            &mut variable,
        ) {
            return None;
        }

        // Only optimize small ranges (avoid materializing millions of IDs)
        let max_range = 100_000i64;
        let upper_val = upper?;
        if upper_val - lower > max_range || upper_val <= lower {
            return None;
        }

        let ids: Vec<NodeId> = (lower..upper_val).map(|i| NodeId::new(i as u64)).collect();
        Some((variable, ids))
    }

    /// Recursively collects id() range bounds from AND chains.
    fn collect_id_range_bounds(
        predicate: &LogicalExpression,
        target_variable: &str,
        lower: &mut i64,
        upper: &mut Option<i64>,
        variable: &mut String,
    ) -> bool {
        match predicate {
            LogicalExpression::Binary {
                left,
                op: BinaryOp::And,
                right,
            } => {
                let l =
                    Self::collect_id_range_bounds(left, target_variable, lower, upper, variable);
                let r =
                    Self::collect_id_range_bounds(right, target_variable, lower, upper, variable);
                l || r
            }
            LogicalExpression::Binary { left, op, right } => {
                // Match id(var) <op> literal or literal <op> id(var)
                // Use extract_id_variable to handle both LogicalExpression::Id and FunctionCall("id", [Variable])
                if let (Some(var), LogicalExpression::Literal(Value::Int64(val))) =
                    (Self::extract_id_variable(left), right.as_ref())
                    && var == target_variable
                {
                    *variable = var.to_string();
                    return match op {
                        BinaryOp::Lt => {
                            *upper = Some(*val);
                            true
                        }
                        BinaryOp::Le => {
                            *upper = Some(*val + 1);
                            true
                        }
                        BinaryOp::Gt => {
                            *lower = *val + 1;
                            true
                        }
                        BinaryOp::Ge => {
                            *lower = *val;
                            true
                        }
                        _ => false,
                    };
                }
                if let (LogicalExpression::Literal(Value::Int64(val)), Some(var)) =
                    (left.as_ref(), Self::extract_id_variable(right))
                    && var == target_variable
                {
                    *variable = var.to_string();
                    return match op {
                        BinaryOp::Lt => {
                            *lower = *val + 1;
                            true
                        }
                        BinaryOp::Le => {
                            *lower = *val;
                            true
                        }
                        BinaryOp::Gt => {
                            *upper = Some(*val);
                            true
                        }
                        BinaryOp::Ge => {
                            *upper = Some(*val + 1);
                            true
                        }
                        _ => false,
                    };
                }
                false
            }
            _ => false,
        }
    }

    ///
    /// Returns `Ok(Some((operator, columns)))` if optimization was applied,
    /// `Ok(None)` if not applicable, or `Err` on error.
    /// Plans `MATCH (n:L) WHERE n.prop IN [lit, …]` as k index probes.
    ///
    /// An `IN` is a disjunction. The equality path already probed the
    /// property index; `IN` did not, and fell through to
    /// `ScanOperator → DataChunk → FilterOperator`.
    ///
    /// Measured on the live `agent` base 2026-08-06, 31 357
    /// `SupervisorObjective` nodes, alternating shots under equal load :
    ///
    /// ```text
    /// WHERE n.id = "…"       →  28 ms   (index probe)
    /// WHERE n.status = "…"   → 791 ms   (no index on status → full scan)
    /// WHERE n.id IN ["…"]    → 585 ms   (no probe: full scan)
    /// ```
    ///
    /// The contrast between the two equalities is what identifies the
    /// cause: `id` is indexed on that store, `status` is not. A first
    /// version of this function scanned the label instead of probing, by
    /// symmetry with the NON-indexed equality branch — and measured
    /// SLOWER than the generic path it replaced. Hence the hard rule
    /// below: no index, no rewrite.
    ///
    /// Bails out (→ generic path) when anything about the shape is not
    /// certain : non-literal element, NULL in the list (three-valued
    /// logic), compound predicate, nested scan, unindexed property.
    pub(super) fn try_plan_filter_with_in_list(
        &self,
        filter: &FilterOp,
    ) -> Result<Option<(Box<dyn Operator>, Vec<String>)>> {
        let (scan_variable, scan_label) = match filter.input.as_ref() {
            LogicalOperator::NodeScan(scan) if scan.input.is_none() => {
                (scan.variable.clone(), scan.label.clone())
            }
            _ => return Ok(None),
        };

        let LogicalExpression::Binary {
            left,
            op: BinaryOp::In,
            right,
        } = &filter.predicate
        else {
            return Ok(None);
        };
        let LogicalExpression::Property { variable, property } = left.as_ref() else {
            return Ok(None);
        };
        if variable != &scan_variable {
            return Ok(None);
        }
        let LogicalExpression::List(items) = right.as_ref() else {
            return Ok(None);
        };

        let mut wanted: Vec<Value> = Vec::with_capacity(items.len());
        for item in items {
            match item {
                // A NULL element makes the predicate three-valued for
                // non-matching rows. UNKNOWN and FALSE both exclude a row
                // here, but the generic evaluator is the authority on that
                // — we do not re-derive it.
                LogicalExpression::Literal(v) if !v.is_null() => wanted.push(v.clone()),
                _ => return Ok(None),
            }
        }

        // ── Sans index : NE RIEN FAIRE ──
        //
        // La première version de ce chemin balayait le label et lisait la
        // propriété nœud par nœud, par symétrie avec la branche NON indexée
        // du chemin d'égalité. Mesuré sur la base `agent` : c'était PLUS
        // LENT que le chemin générique (585 ms contre 499 ms sur `id`).
        // Sans gain démontré, on rend la main.
        if !self.store.has_property_index(property) {
            return Ok(None);
        }

        // ── Avec index : une sonde par valeur, puis union ──
        //
        // Un `IN` est une disjonction : k sondes d'index au lieu d'un
        // balayage de tout le label. C'est ce qui manquait — le chemin
        // d'égalité utilisait l'index, le mien l'ignorait.
        let mut seen: std::collections::HashSet<NodeId> = std::collections::HashSet::new();
        let mut matching_nodes: Vec<NodeId> = Vec::new();
        for value in &wanted {
            for node_id in self.store.find_nodes_by_property(property, value) {
                if seen.insert(node_id) {
                    matching_nodes.push(node_id);
                }
            }
        }

        // L'index ignore les labels : on recoupe. PAS via `nodes_by_label`
        // — ce serait réintroduire le balayage O(N) que les sondes viennent
        // d'éviter (mesuré : 2 000 nœuds → 46 µs, 20 000 → 374 µs, soit
        // toujours linéaire). `node_has_label` teste UN candidat, et il y
        // en a au plus k.
        if let Some(label) = &scan_label {
            matching_nodes.retain(|n| self.store.node_has_label(*n, label).unwrap_or(false));
        }

        // Visibilité MVCC : l'index peut porter des nœuds non visibles à
        // cette époque. Même précaution que la branche indexée voisine.
        // (2026-08-16, mission perf DB) AVANT : get_node_* hydratait chaque
        // candidat (clone de PropertyMap) juste pour un test d existence.
        // Les surcharges is_node_visible_* (test de bit + cartes d epoque)
        // existent precisement pour ca — meme semantique, zero hydratation.
        let epoch = self.viewing_epoch;
        if let Some(tx) = self.transaction_id {
            matching_nodes.retain(|id| self.store.is_node_visible_versioned(*id, epoch, tx));
        } else {
            matching_nodes.retain(|id| self.store.is_node_visible_at_epoch(*id, epoch));
        }

        let columns = vec![scan_variable];
        let op: Box<dyn Operator> = Box::new(NodeListOperator::new(matching_nodes, 2048));
        Ok(Some((op, columns)))
    }

    pub(super) fn try_plan_filter_with_property_index(
        &self,
        filter: &FilterOp,
    ) -> Result<Option<(Box<dyn Operator>, Vec<String>)>> {
        // ── (2026-08-15) LE MOTIF AVEC INPUT PROFITE AUSSI DE L'INDEX ──
        //
        // La garde historique `scan.input.is_none()` reservait l'index au
        // PREMIER motif d'un MATCH. Le second motif de
        // `MATCH (a:X {id}), (b:Y {id})` retombait donc sur un
        // ScanOperator COMPLET croise avec a — mesure en production :
        // 15-27 s par arete de tissage (391 k Perceptions materialisees),
        // 2 193 requetes lentes en un boot, les pages de l'operateur
        // ecrasees par ses propres ecritures de fond. Le scan complet est
        // remplace par la NodeList INDEXEE, croisee avec l'input — meme
        // forme de jointure que plan_node_scan.
        let (scan_variable, scan_label, scan_input) = match filter.input.as_ref() {
            LogicalOperator::NodeScan(scan) => (
                scan.variable.clone(),
                scan.label.clone(),
                scan.input.as_deref(),
            ),
            _ => return Ok(None),
        };

        // Extract property equality conditions from the predicate
        // Handles both simple (n.prop = val) and compound (n.a = 1 AND n.b = 2)
        let conditions = self.extract_equality_conditions(&filter.predicate, &scan_variable);

        if conditions.is_empty() {
            return Ok(None);
        }

        // Check if at least one condition has an index
        let has_indexed_condition = conditions
            .iter()
            .any(|(prop, _)| self.store.has_property_index(prop));

        // Without an index we can still optimize when there's a label constraint:
        // label-first scan + property check avoids DataChunk/expression overhead.
        if !has_indexed_condition && scan_label.is_none() {
            return Ok(None);
        }

        let mut matching_nodes = if has_indexed_condition {
            // Use index-based batch lookup
            let conditions_ref: Vec<(&str, Value)> = conditions
                .iter()
                .map(|(p, v)| (p.as_str(), v.clone()))
                .collect();
            let mut nodes = self.store.find_nodes_by_properties(&conditions_ref);

            // Intersect with label if present.
            //
            // (2026-08-16, mission perf DB) AVANT : on matérialisait TOUT
            // nodes_by_label(label) — un balayage mmap du high-water mark
            // complet (536k slots mesurés en prod, ~300 ms à froid, 8-20 s
            // sous contention) — pour revérifier le label de k candidats
            // (k=1 dans le cas MERGE-par-id !). node_has_label est le test
            // de bit pur T13, déjà surchargé côté substrate : O(k) réel.
            if let Some(label) = &scan_label {
                nodes.retain(|n| self.store.node_has_label(*n, label).unwrap_or(false));
            }

            // ── LA MÊME GARDE QUE LE CHEMIN DE TEXTE (2026-09-14) ────────
            //
            // UN INDEX QUI REND BEAUCOUP DE CANDIDATS COÛTE PLUS CHER QUE LE
            // BALAYAGE. L'index d'égalité matériallise k `NodeId` puis
            // REVÉRIFIE chaque candidat par une lecture de propriété en accès
            // ALÉATOIRE — là où le balayage lit des slots CONSÉCUTIFS et ne
            // paie la propriété que pour les lignes qu'il visite. Au-delà
            // d'une certaine sélectivité, le seek perd : c'est le défaut déjà
            // mesuré et gardé sur `CONTAINS` (35 115 candidats : 553 ms par
            // l'index contre 80 ms au balayage, sept fois pire, mêmes lignes).
            //
            // Le seuil est RELATIF au store (1 %, plancher 256) : la même
            // requête doit se comporter pareil à mille et à dix millions de
            // nœuds. `OBRAIN_EGALITE_CANDIDATS_MAX` (pour cent) le règle pour
            // la mesure. On rend `Ok(None)` : le chemin d'origine — balayage
            // compris, revérifié — reprend INCHANGÉ.
            if trop_de_candidats(
                nodes.len(),
                self.store.node_count(),
                "OBRAIN_EGALITE_CANDIDATS_MAX",
            ) {
                tracing::debug!(
                    candidats = nodes.len(),
                    "égalité indexée : trop de candidats — la main au balayage"
                );
                return Ok(None);
            }
            nodes
        } else {
            // No index but we have a label: scan label first, then check properties.
            // This is more efficient than ScanOperator → DataChunk → FilterOperator
            // because it avoids DataChunk materialization and expression evaluation.
            let label = scan_label.as_ref().expect("label checked above");

            // ── LE PLAFOND (2026-09-14) — voir `elagage_actif` ─────────────
            //
            // La phrase ci-dessus est vraie de l'EXÉCUTION et fausse de la
            // PLANIFICATION : elle décrit une épargne de `DataChunk`, pas le
            // balayage du label ni l'hydratation d'un `Node` PAR nœud qui la
            // paient. Mesuré sur 71 445 `Message` : 192 ms de planification, à
            // chaque appel (1er et 2e appel du même texte : 192 859 µs et
            // 192 306 µs — la planification physique n'est pas mémoïsée, et ce
            // n'est pas elle qui coûte). Au-delà du plafond, la liste n'est pas
            // construite ici : `Ok(None)` rend la main au chemin générique, qui
            // balaie le label et applique le prédicat À L'EXÉCUTION.
            //
            // Le compteur est O(1) sur le substrat (index de labels dense) et
            // ne construit rien ; sur un backend qui n'a que le défaut du trait
            // (`nodes_by_label(...).len()`), il coûte le balayage que ce bloc
            // allait faire de toute façon — jamais davantage.
            if elagage_actif()
                && self.store.node_count_by_label(label) as u64 > plafond_materialisation()
            {
                return Ok(None);
            }

            // ── (2026-11-08, C1) UNE PROPRIÉTÉ, PAS LE NŒUD ─────────────────
            //
            // AVANT : pour CHAQUE candidat du label, `get_node_at_epoch` /
            // `get_node_versioned` construisait le `Node` complet — labels
            // résolus + TOUTES les propriétés (`read_all_properties_for_node`,
            // quatre zones, tri LWW) — pour lire ensuite UNE clef. Mesuré sur
            // le banc `query_node_lookup_by_property` (1 000 `Person`, sans
            // index) : 503 µs par requête, dont ~99 % en planification
            // (`OBRAIN_PHASES=1` : planification 845 µs, exécution 10 µs,
            // sous charge 20-30).
            //
            // APRÈS : une lecture PAR LOT de la seule propriété testée,
            // sous la MÊME visibilité que l'accès nœud qu'elle remplace —
            // `get_node_property_batch_at_epoch` est le jumeau de
            // `get_node_at_epoch(..).get_property(..)`,
            // `get_node_property_batch_versioned` celui de
            // `get_node_versioned(..)` (contrat du trait, test
            // `filtre_sans_index_mvcc`). Les conditions d'un AND sont
            // appliquées l'une après l'autre sur un lot qui rétrécit : la
            // seconde propriété n'est lue que pour les survivants de la
            // première. Résultat : même ensemble de candidats qu'avant —
            // « visible ET toutes les propriétés égales » — sans aucune
            // hydratation.
            //
            // Le SECOND passage de visibilité (ci-dessous) reste NÉCESSAIRE,
            // exactement comme avant : `get_node_at_epoch` ignore
            // `en_attente_noeuds` (un nœud créé par une transaction ouverte
            // ailleurs est rendu — gql_13 `un_predicat_de_propriete_ne_lit_
            // pas_l_ecriture_non_commitee_d_autrui`), et `get_node_versioned`
            // ignore l'époque ET la transaction (« MVCC deferred to T5 »).
            // C'est `is_node_visible_*` qui porte l'isolation ; les lectures
            // ci-dessus n'en portent que la part qu'elles portaient déjà.
            let mut candidats = self.store.nodes_by_label(label);
            let epoch = self.viewing_epoch;
            let tx_id = self.transaction_id;
            for (prop, val) in &conditions {
                if candidats.is_empty() {
                    break;
                }
                let key = obrain_common::types::PropertyKey::new(prop.as_str());
                let valeurs = match tx_id {
                    Some(tx) => self
                        .store
                        .get_node_property_batch_versioned(&candidats, &key, epoch, tx),
                    None => self
                        .store
                        .get_node_property_batch_at_epoch(&candidats, &key, epoch),
                };
                debug_assert_eq!(valeurs.len(), candidats.len());
                candidats = candidats
                    .into_iter()
                    .zip(valeurs)
                    .filter_map(|(id, v)| {
                        v.is_some_and(|v| values_equal_coerced(&v, val))
                            .then_some(id)
                    })
                    .collect();
            }
            candidats
        };

        // MVCC visibility: filter out nodes not visible at the current epoch/tx.
        // Without this, rolled-back or uncommitted nodes could leak through.
        // (2026-08-16, mission perf DB) AVANT : get_node_* hydratait chaque
        // candidat (clone de PropertyMap) juste pour un test d existence.
        // Les surcharges is_node_visible_* (test de bit + cartes d epoque)
        // existent precisement pour ca — meme semantique, zero hydratation.
        let epoch = self.viewing_epoch;
        if let Some(tx) = self.transaction_id {
            matching_nodes.retain(|id| self.store.is_node_visible_versioned(*id, epoch, tx));
        } else {
            matching_nodes.retain(|id| self.store.is_node_visible_at_epoch(*id, epoch));
        }

        let node_list_op: Box<dyn Operator> = Box::new(NodeListOperator::new(matching_nodes, 2048));

        // Le cas avec input : on plane l'input et on le CROISE avec la
        // liste indexee — k candidats au lieu du label entier. Variable
        // deja liee dans l'input (CALL { WITH var MATCH (var)... }) : on
        // rend la main au chemin generique, exactement comme
        // plan_node_scan le fait pour son propre raccourci.
        let (node_list_op, columns): (Box<dyn Operator>, Vec<String>) =
            if let Some(input) = scan_input {
                let (input_op, mut input_columns) = self.plan_operator(input)?;
                if input_columns.contains(&scan_variable) {
                    return Ok(None);
                }
                let mut output_schema: Vec<LogicalType> =
                    input_columns.iter().map(|_| LogicalType::Any).collect();
                output_schema.push(LogicalType::Node);
                input_columns.push(scan_variable.clone());
                (
                    Box::new(NestedLoopJoinOperator::new(
                        input_op,
                        node_list_op,
                        None,
                        PhysicalJoinType::Cross,
                        output_schema,
                    )),
                    input_columns,
                )
            } else {
                (node_list_op, vec![scan_variable.clone()])
            };

        // Check for remaining predicate parts that weren't pushed down
        // (e.g., range conditions in a compound predicate like `n.name = 'Alix' AND n.age > 30`)
        if let Some(remaining) =
            self.extract_remaining_predicate(&filter.predicate, &scan_variable, &conditions)
        {
            let variable_columns: HashMap<String, usize> = columns
                .iter()
                .enumerate()
                .map(|(i, name)| (name.clone(), i))
                .collect();
            let filter_expr = self.convert_expression(&remaining)?;
            let predicate = ExpressionPredicate::new(
                filter_expr,
                variable_columns,
                Arc::clone(&self.store) as Arc<dyn GraphStore>,
            )
            .with_transaction_context(self.viewing_epoch, self.transaction_id)
            .with_session_context(self.session_context.clone());
            let filtered = Box::new(FilterOperator::new(node_list_op, Box::new(predicate)));
            Ok(Some((filtered, columns)))
        } else {
            Ok(Some((node_list_op, columns)))
        }
    }

    /// ── EXISTENCE : `n.p IS NOT NULL` SERVI PAR LA SÉRIE DE PROPRIÉTÉ ────
    ///
    /// LE DÉFAUT MESURÉ (2026-09-14, `runtime-dev`, 433 618 nœuds) :
    /// `MATCH (n) WHERE n.id IS NOT NULL RETURN count(n)` coûtait **514 ms et
    /// 433 633 slots lus** pour rendre **110** lignes. `IS NOT NULL` compile
    /// en `UnaryFilterOp::IsNotNull`, un filtre d'expression NU au-dessus
    /// d'un balayage complet : aucune route ne pouvait le servir, car
    /// l'égalité exige une VALEUR à hacher, et une existence n'en a pas.
    ///
    /// CE QUE CE CHEMIN FAIT. Il demande au substrat les nœuds portant la
    /// propriété ([`GraphStore::find_nodes_with_property`]), qui les obtient
    /// par la SÉRIE de la propriété déclarée au catalogue, puis les
    /// REVÉRIFIE sur les valeurs courantes : la liste rendue est EXACTE.
    ///
    /// CE QU'IL NE FAIT PAS, ET C'EST DÉLIBÉRÉ. Il ne traite que
    /// `IsNotNull(Property(scan_variable, p))` — un `n.p IS NOT NULL` mêlé à
    /// d'autres conditions voit TOUT le reste conservé au-dessus de la liste
    /// (voir le résidu plus bas) ; il refuse un `NodeScan` PORTANT UN INPUT
    /// (même retenue que la route d'égalité) ; et il ne touche pas à
    /// `IS NULL`, qui demande le COMPLÉMENT d'une série — donc le store
    /// entier — et n'a rien à gagner d'un index partiel.
    ///
    /// LA GARDE — « JAMAIS PIRE QU'UN BALAYAGE ». Une existence NON
    /// sélective (`n.qualified_name IS NOT NULL` : ~300 000 nœuds sur
    /// 433 618) paie une revérification par candidat. Le seuil est le même
    /// modèle que celui du texte — RELATIF au store, 1 %, plancher 256,
    /// réglable par `OBRAIN_EXISTENCE_CANDIDATS_MAX` (pour cent) — et il est
    /// appliqué sur le nombre de candidats APRÈS le label et la visibilité,
    /// c'est-à-dire sur ce que l'index aurait vraiment coûté. Au-delà : `Ok(None)`,
    /// et le balayage d'origine reprend, INCHANGÉ.
    ///
    /// CE QUE LA GARDE NE COUVRE PAS, ET IL FAUT LE DIRE : l'énumération de
    /// la série est payée AVANT la garde (le substrat la rend complète et
    /// revérifiée, c'est son contrat). Un refus coûte donc une énumération
    /// inutile — bornée par le nombre de porteurs de la propriété, et suivie
    /// du balayage qui est le chemin le PLUS cher des deux. Le pire cas reste
    /// le balayage ; il n'est jamais doublé d'une réponse fausse.
    pub(super) fn try_plan_filter_with_property_existence(
        &self,
        filter: &FilterOp,
    ) -> Result<Option<(Box<dyn Operator>, Vec<String>)>> {
        // Un seul motif, sans input : le balayage que l'on remplace est
        // celui-là, et lui seul.
        let (scan_variable, scan_label) = match filter.input.as_ref() {
            LogicalOperator::NodeScan(scan) if scan.input.is_none() => {
                (scan.variable.clone(), scan.label.clone())
            }
            _ => return Ok(None),
        };

        // Les conjoints d'existence du prédicat, dans l'ordre du texte.
        let mut servies: Vec<String> = Vec::new();
        Self::collecte_existence(&filter.predicate, &scan_variable, &mut servies);
        if servies.is_empty() {
            return Ok(None);
        }

        // UNE propriété suffit à borner la liste ; les autres conjoints
        // restent au-dessus, dans le résidu. On essaie dans l'ordre du texte
        // et on prend la PREMIÈRE que le substrat sait servir — un seul appel
        // par propriété essayée, l'énumération n'est pas gratuite.
        let mut servie = String::new();
        let mut candidats: Vec<NodeId> = Vec::new();
        for p in &servies {
            if let Some(nodes) = self.store.find_nodes_with_property(p) {
                servie.clone_from(p);
                candidats = nodes;
                break;
            }
        }
        if servie.is_empty() {
            return Ok(None);
        }

        // Étiquette du motif : test de bit par candidat, jamais la
        // matérialisation du label entier (même leçon que la route
        // d'égalité, 2026-08-16).
        if let Some(label) = &scan_label {
            candidats.retain(|n| self.store.node_has_label(*n, label).unwrap_or(false));
        }
        // Visibilité MVCC : mêmes retenues que la route d'égalité.
        let epoch = self.viewing_epoch;
        if let Some(tx) = self.transaction_id {
            candidats.retain(|id| self.store.is_node_visible_versioned(*id, epoch, tx));
        } else {
            candidats.retain(|id| self.store.is_node_visible_at_epoch(*id, epoch));
        }

        // ── LA GARDE ── (voir le pavé de la fonction)
        if trop_de_candidats(
            candidats.len(),
            self.store.node_count(),
            "OBRAIN_EXISTENCE_CANDIDATS_MAX",
        ) {
            tracing::debug!(
                propriete = %servie,
                candidats = candidats.len(),
                total = self.store.node_count(),
                "existence : trop de candidats — la main au balayage (mesuré moins cher)"
            );
            return Ok(None);
        }

        let node_list_op: Box<dyn Operator> = Box::new(NodeListOperator::new(candidats, 2048));
        let columns = vec![scan_variable.clone()];

        // Le résidu : tout ce qui n'est pas la forme servie — y compris un
        // SECOND `IS NOT NULL` sur la même variable, et le prédicat entier
        // dès qu'il n'est pas une simple conjonction. Le filtre est perdu si
        // on le laisse tomber : la liste indexée est exacte pour la propriété
        // SERVIE, pas pour ses conjoints.
        if let Some(remaining) =
            Self::extract_remaining_after_existence(&filter.predicate, &scan_variable, &servie)
        {
            let variable_columns: HashMap<String, usize> = columns
                .iter()
                .enumerate()
                .map(|(i, name)| (name.clone(), i))
                .collect();
            let filter_expr = self.convert_expression(&remaining)?;
            let predicate = ExpressionPredicate::new(
                filter_expr,
                variable_columns,
                Arc::clone(&self.store) as Arc<dyn GraphStore>,
            )
            .with_transaction_context(self.viewing_epoch, self.transaction_id)
            .with_session_context(self.session_context.clone());
            let filtered = Box::new(FilterOperator::new(node_list_op, Box::new(predicate)));
            return Ok(Some((filtered, columns)));
        }
        Ok(Some((node_list_op, columns)))
    }

    /// Collecte les propriétés testées par `IS NOT NULL` sur `variable`, dans
    /// l'ordre du texte, à travers les conjonctions.
    fn collecte_existence(expr: &LogicalExpression, variable: &str, out: &mut Vec<String>) {
        match expr {
            LogicalExpression::Binary {
                left,
                op: BinaryOp::And,
                right,
            } => {
                Self::collecte_existence(left, variable, out);
                Self::collecte_existence(right, variable, out);
            }
            LogicalExpression::Unary {
                op: UnaryOp::IsNotNull,
                operand,
            } => {
                if let LogicalExpression::Property {
                    variable: v,
                    property,
                } = operand.as_ref()
                    && v == variable
                    && !out.iter().any(|p| p == property)
                {
                    out.push(property.clone());
                }
            }
            _ => {}
        }
    }

    /// Retire du prédicat la SEULE forme servie (`variable.propriete IS NOT
    /// NULL`) et rend ce qui reste — ou `None` si tout a été servi.
    ///
    /// Sœur de [`Self::extract_remaining_predicate`], qui fait le même
    /// travail pour l'égalité : ce qui est poussé dans l'index ne doit pas
    /// être réévalué, et ce qui ne l'est pas doit l'être.
    fn extract_remaining_after_existence(
        predicate: &LogicalExpression,
        target_variable: &str,
        servie: &str,
    ) -> Option<LogicalExpression> {
        match predicate {
            LogicalExpression::Binary {
                left,
                op: BinaryOp::And,
                right,
            } => {
                let l = Self::extract_remaining_after_existence(left, target_variable, servie);
                let r = Self::extract_remaining_after_existence(right, target_variable, servie);
                match (l, r) {
                    (Some(l), Some(r)) => Some(LogicalExpression::Binary {
                        left: Box::new(l),
                        op: BinaryOp::And,
                        right: Box::new(r),
                    }),
                    (Some(l), None) => Some(l),
                    (None, Some(r)) => Some(r),
                    (None, None) => None,
                }
            }
            LogicalExpression::Unary {
                op: UnaryOp::IsNotNull,
                operand,
            } => match operand.as_ref() {
                LogicalExpression::Property { variable, property }
                    if variable == target_variable && property == servie =>
                {
                    None
                }
                _ => Some(predicate.clone()),
            },
            _ => Some(predicate.clone()),
        }
    }

    /// (chantier VOISINAGE-ENTRANT, 2026-09-12) Re-ancre une traversee
    /// 1-saut sur le noeud CIBLE quand c'est lui qui porte la contrainte
    /// d'egalite indexee.
    ///
    /// Le traducteur ancre TOUJOURS sur la source du motif
    /// (`translate_path_pattern_with_alias`) : pour
    /// `MATCH (a)-[r]->(b {qualified_name: 'QN'})`, le plan logique est
    /// `Filter(b.qn = 'QN', Expand(a->b, NodeScan(a)))` — balayage COMPLET
    /// des noeuds, expansion de toutes leurs aretes, filtre a la fin.
    /// Mesure du superviseur sur runtime-dev : 319 572 ms, contre 0,19 ms
    /// pour la MEME requete dans le sens sortant (facteur ~1,7 million).
    ///
    /// La traversee inverse est pourtant NATIVE : `edges_from(b, Incoming)`
    /// remonte la chaine entrante de b dans le substrat
    /// (`walk_incoming_chain`), et `ExpandOperator` la sert deja pour la
    /// syntaxe `(b {qn})<-[r]-(a)`, qui place b en source. Ce qui manquait
    /// est la REGLE DE PLAN qui symetrise les deux ecritures : reconnaitre
    ///
    ///   Filter*(preds, Expand{a->b, 1 saut, input: NodeScan(a) LIBRE})
    ///
    /// ou au moins un predicat porte une egalite INDEXEE sur b, et
    /// re-planifier
    ///
    ///   Filter*(preds sur a/r, Expand{b->a, direction INVERSEE,
    ///                        input: Filter*(preds sur b, NodeScan(b))})
    ///
    /// Le `Filter(NodeScan(b))` interne est alors pris en charge par
    /// `try_plan_filter_with_property_index` : seek indexe, k candidats au
    /// lieu de N noeuds.
    ///
    /// Garde-fous, dans l'ordre des pieges du chantier :
    /// - UN saut exact (min=1, max=1) sans alias de chemin — inverser un
    ///   saut variable est un autre chantier ;
    /// - `NodeScan(a)` LIBRE : sans label, sans input, sans hint. Si a est
    ///   contraint (piege n3 : deux extremites indexees), la forme ne
    ///   matche pas et le plan historique — ancre sur a — fait foi. Choix
    ///   conservateur : sans cardinalites, on ne demenage pas un ancrage
    ///   qui fonctionne ;
    /// - la contrainte de b doit etre une egalite sur propriete INDEXEE :
    ///   sans index, ancrer sur b echangerait un balayage contre un autre ;
    /// - les predicats referencant a ou r RESTENT au-dessus de l'Expand
    ///   inverse (piege n2) : un LIMIT au sommet ne coupe qu'apres eux —
    ///   le meme desarmement que le chantier SCAN, obtenu par la structure
    ///   et non par un compteur. Le plan reconstruit ne porte AUCUN hint
    ///   de cap : `LimitOperator` garde seul le contrat de sortie.
    ///
    /// L'ordre des colonnes change ([b, r, a] au lieu de [a, r, b]) ; les
    /// consommateurs resolvent par NOM via le vecteur de colonnes rendu,
    /// jamais par position.
    fn try_plan_reancrage_entrant(
        &self,
        filter: &FilterOp,
    ) -> Result<Option<(Box<dyn Operator>, Vec<String>)>> {
        // 1. Descendre la pile de Filters jusqu'a un Expand, predicats en main.
        let mut preds: Vec<&LogicalExpression> = vec![&filter.predicate];
        let mut cursor: &LogicalOperator = filter.input.as_ref();
        while let LogicalOperator::Filter(f) = cursor {
            preds.push(&f.predicate);
            cursor = f.input.as_ref();
        }
        let LogicalOperator::Expand(expand) = cursor else {
            return Ok(None);
        };

        // 2. La forme : un saut exact, sans alias, source LIBRE et distincte.
        if expand.min_hops != 1 || expand.max_hops != Some(1) || expand.path_alias.is_some() {
            return Ok(None);
        }
        if expand.from_variable == expand.to_variable {
            return Ok(None);
        }
        let LogicalOperator::NodeScan(scan) = expand.input.as_ref() else {
            return Ok(None);
        };
        if scan.label.is_some()
            || scan.input.is_some()
            || scan.pushdown_hint.is_some()
            || scan.variable != expand.from_variable
        {
            return Ok(None);
        }
        let cible = &expand.to_variable;

        // 3. Partition des predicats : ceux qui ne referencent QUE b
        //    descendront sous l'Expand inverse ; tous les autres (a, r,
        //    melanges, ou sans variable) restent au-dessus.
        let mut preds_cible: Vec<LogicalExpression> = Vec::new();
        let mut preds_dessus: Vec<LogicalExpression> = Vec::new();
        for p in preds {
            let mut vars = HashSet::new();
            Self::collect_expression_variables(p, &mut vars);
            if !vars.is_empty() && vars.iter().all(|v| v == cible) {
                preds_cible.push(p.clone());
            } else {
                preds_dessus.push(p.clone());
            }
        }

        // 4. Au moins une egalite INDEXEE sur b — sinon, statu quo.
        let indexee = preds_cible.iter().any(|p| {
            self.extract_equality_conditions(p, cible)
                .iter()
                .any(|(prop, _)| self.store.has_property_index(prop))
        });
        if !indexee {
            return Ok(None);
        }

        // 5. Reconstruction, ancree sur b. Les predicats sont re-empiles
        //    dans leur ordre d'origine (collectes haut->bas, re-poses
        //    bas->haut).
        let mut interne = LogicalOperator::NodeScan(NodeScanOp {
            variable: cible.clone(),
            label: None,
            input: None,
            pushdown_hint: None,
        });
        for p in preds_cible.into_iter().rev() {
            interne = LogicalOperator::Filter(FilterOp {
                predicate: p,
                input: Box::new(interne),
                pushdown_hint: None,
            });
        }

        let direction_inverse = match expand.direction {
            ExpandDirection::Outgoing => ExpandDirection::Incoming,
            ExpandDirection::Incoming => ExpandDirection::Outgoing,
            ExpandDirection::Both => ExpandDirection::Both,
        };
        let mut plan = LogicalOperator::Expand(ExpandOp {
            from_variable: cible.clone(),
            to_variable: expand.from_variable.clone(),
            edge_variable: expand.edge_variable.clone(),
            direction: direction_inverse,
            edge_types: expand.edge_types.clone(),
            min_hops: 1,
            max_hops: Some(1),
            input: Box::new(interne),
            path_alias: None,
            path_mode: expand.path_mode,
        });
        for p in preds_dessus.into_iter().rev() {
            plan = LogicalOperator::Filter(FilterOp {
                predicate: p,
                input: Box::new(plan),
                pushdown_hint: None,
            });
        }

        // Le plan re-ancre repasse par la planification normale : le
        // Filter(NodeScan(b)) interne y devient une NodeList indexee, et
        // l'Expand inverse un ExpandOperator(Incoming) — les deux chemins
        // existants, simplement recombines.
        let (op, columns) = self.plan_operator(&plan)?;
        Ok(Some((op, columns)))
    }

    /// (chantier #95) Planifie une jointure correlee par sonde d'index.
    ///
    /// Reconnait `Filter(pred, NodeScan{var, label, input: Some(outer)})` ou
    /// `pred` contient un conjoint `var.prop = expr` avec `expr` ne
    /// referencant QUE des colonnes de `outer` (cas type :
    /// `MATCH (ev:Evidence) MATCH (c:Claim {id: ev.claim_id})`). Construit un
    /// [`IndexSeekJoinOperator`] : une sonde `find_nodes_by_property` par
    /// ligne externe au lieu du produit cartesien
    /// `NestedLoopJoin(Cross, ScanOperator)` — le repli historique
    /// materialisait |externe| x |label| paires puis filtrait par ligne.
    ///
    /// Garde-fous :
    /// - propriete NON indexee → `Ok(None)` (une sonde non indexee serait un
    ///   scan complet PAR LIGNE externe, pire que le produit croise) ;
    /// - `expr` sans variable (litteral, dont NULL) → `Ok(None)`, le chemin
    ///   par litteral et la logique trivaluee du chemin generique s'en
    ///   chargent ;
    /// - variable du scan deja liee dans l'externe → `Ok(None)` (meme regle
    ///   que `plan_node_scan`) ;
    /// - la coercition de type et les cles non projetables (Float64…) sont
    ///   gerees a l'EXECUTION par l'operateur (repli generique, jamais un
    ///   resultat vide silencieux) ;
    /// - le predicat residuel (autres conjoints) s'evalue au-dessus de la
    ///   jointure avec la carte de colonnes complete.
    pub(super) fn try_plan_correlated_index_seek(
        &self,
        filter: &FilterOp,
    ) -> Result<Option<(Box<dyn Operator>, Vec<String>)>> {
        let LogicalOperator::NodeScan(scan) = filter.input.as_ref() else {
            return Ok(None);
        };
        let Some(outer) = scan.input.as_deref() else {
            return Ok(None);
        };
        let scan_variable = scan.variable.clone();

        let Some((property, key_logical, remaining)) =
            Self::split_correlated_equality(&filter.predicate, &scan_variable)
        else {
            return Ok(None);
        };

        if !self.store.has_property_index(&property) {
            return Ok(None);
        }

        // La cle doit etre correlee : au moins une variable, et rien que des
        // variables externes. Un litteral pur (zero variable) appartient au
        // chemin d'index par litteral.
        let mut key_vars: HashSet<String> = HashSet::new();
        Self::collect_expression_variables(&key_logical, &mut key_vars);
        if key_vars.is_empty() {
            return Ok(None);
        }

        let (outer_op, outer_columns) = self.plan_operator(outer)?;
        if outer_columns.contains(&scan_variable) {
            // Variable deja liee (CALL { WITH var MATCH (var)... }) : retour
            // au chemin generique, comme plan_node_scan.
            return Ok(None);
        }
        if !key_vars.iter().all(|v| outer_columns.contains(v)) {
            return Ok(None);
        }

        let outer_variable_columns: HashMap<String, usize> = outer_columns
            .iter()
            .enumerate()
            .map(|(i, name)| (name.clone(), i))
            .collect();
        let key_expr = ExpressionPredicate::new(
            self.convert_expression(&key_logical)?,
            outer_variable_columns,
            Arc::clone(&self.store) as Arc<dyn GraphStore>,
        )
        .with_transaction_context(self.viewing_epoch, self.transaction_id)
        .with_session_context(self.session_context.clone());

        let mut output_schema: Vec<LogicalType> =
            outer_columns.iter().map(|_| LogicalType::Any).collect();
        output_schema.push(LogicalType::Node);
        let mut columns = outer_columns;
        columns.push(scan_variable.clone());

        let join_op: Box<dyn Operator> = Box::new(IndexSeekJoinOperator::new(
            outer_op,
            key_expr,
            property,
            scan.label.clone(),
            Arc::clone(&self.store) as Arc<dyn GraphStore>,
            self.viewing_epoch,
            self.transaction_id,
            output_schema,
        ));

        // Predicat residuel au-dessus de la jointure, avec la carte complete.
        if let Some(remaining) = remaining {
            let variable_columns: HashMap<String, usize> = columns
                .iter()
                .enumerate()
                .map(|(i, name)| (name.clone(), i))
                .collect();
            let predicate = ExpressionPredicate::new(
                self.convert_expression(&remaining)?,
                variable_columns,
                Arc::clone(&self.store) as Arc<dyn GraphStore>,
            )
            .with_transaction_context(self.viewing_epoch, self.transaction_id)
            .with_session_context(self.session_context.clone());
            let filtered = Box::new(FilterOperator::new(join_op, Box::new(predicate)));
            return Ok(Some((filtered, columns)));
        }

        Ok(Some((join_op, columns)))
    }

    /// Cherche dans un arbre AND un conjoint `scan_var.prop = expr` (ou son
    /// symetrique) avec `expr` ne referencant pas `scan_var`.
    ///
    /// Retourne `(prop, expr, predicat_residuel)` — le residuel est le reste
    /// de l'arbre AND, `None` quand le conjoint etait le predicat entier.
    fn split_correlated_equality(
        predicate: &LogicalExpression,
        scan_variable: &str,
    ) -> Option<(String, LogicalExpression, Option<LogicalExpression>)> {
        match predicate {
            LogicalExpression::Binary {
                left,
                op: BinaryOp::And,
                right,
            } => {
                if let Some((prop, key, rem)) = Self::split_correlated_equality(left, scan_variable)
                {
                    let remaining = match rem {
                        Some(r) => LogicalExpression::Binary {
                            left: Box::new(r),
                            op: BinaryOp::And,
                            right: right.clone(),
                        },
                        None => right.as_ref().clone(),
                    };
                    return Some((prop, key, Some(remaining)));
                }
                if let Some((prop, key, rem)) =
                    Self::split_correlated_equality(right, scan_variable)
                {
                    let remaining = match rem {
                        Some(r) => LogicalExpression::Binary {
                            left: left.clone(),
                            op: BinaryOp::And,
                            right: Box::new(r),
                        },
                        None => left.as_ref().clone(),
                    };
                    return Some((prop, key, Some(remaining)));
                }
                None
            }
            LogicalExpression::Binary {
                left,
                op: BinaryOp::Eq,
                right,
            } => {
                let mut references_scan_var = |expr: &LogicalExpression| {
                    let mut vars = HashSet::new();
                    Self::collect_expression_variables(expr, &mut vars);
                    vars.contains(scan_variable)
                };
                match (left.as_ref(), right.as_ref()) {
                    (LogicalExpression::Property { variable, property }, other)
                        if variable == scan_variable && !references_scan_var(other) =>
                    {
                        Some((property.clone(), other.clone(), None))
                    }
                    (other, LogicalExpression::Property { variable, property })
                        if variable == scan_variable && !references_scan_var(other) =>
                    {
                        Some((property.clone(), other.clone(), None))
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Collecte les variables referencees par une expression logique
    /// (recursif, conservateur : toute variable rencontree est retenue).
    fn collect_expression_variables(expr: &LogicalExpression, vars: &mut HashSet<String>) {
        match expr {
            LogicalExpression::Variable(name) => {
                vars.insert(name.clone());
            }
            LogicalExpression::Property { variable, .. } => {
                vars.insert(variable.clone());
            }
            LogicalExpression::Binary { left, right, .. } => {
                Self::collect_expression_variables(left, vars);
                Self::collect_expression_variables(right, vars);
            }
            LogicalExpression::Unary { operand, .. } => {
                Self::collect_expression_variables(operand, vars);
            }
            LogicalExpression::FunctionCall { args, .. } => {
                for a in args {
                    Self::collect_expression_variables(a, vars);
                }
            }
            LogicalExpression::List(items) => {
                for i in items {
                    Self::collect_expression_variables(i, vars);
                }
            }
            LogicalExpression::Map(entries) => {
                for (_, v) in entries {
                    Self::collect_expression_variables(v, vars);
                }
            }
            LogicalExpression::IndexAccess { base, index } => {
                Self::collect_expression_variables(base, vars);
                Self::collect_expression_variables(index, vars);
            }
            LogicalExpression::SliceAccess { base, start, end } => {
                Self::collect_expression_variables(base, vars);
                if let Some(s) = start {
                    Self::collect_expression_variables(s, vars);
                }
                if let Some(e) = end {
                    Self::collect_expression_variables(e, vars);
                }
            }
            LogicalExpression::Labels(v)
            | LogicalExpression::Type(v)
            | LogicalExpression::Id(v) => {
                vars.insert(v.clone());
            }
            LogicalExpression::Case {
                operand,
                when_clauses,
                else_clause,
            } => {
                if let Some(op) = operand {
                    Self::collect_expression_variables(op, vars);
                }
                for (c, r) in when_clauses {
                    Self::collect_expression_variables(c, vars);
                    Self::collect_expression_variables(r, vars);
                }
                if let Some(e) = else_clause {
                    Self::collect_expression_variables(e, vars);
                }
            }
            // Formes non listees (sous-requetes, comprehensions…) : on
            // retient un temoin impossible pour desactiver le seek — mieux
            // vaut le chemin generique qu'une cle mal analysee.
            LogicalExpression::ExistsSubquery(_)
            | LogicalExpression::CountSubquery(_)
            | LogicalExpression::ValueSubquery(_)
            | LogicalExpression::PatternComprehension { .. }
            | LogicalExpression::Reduce { .. }
            | LogicalExpression::ListComprehension { .. }
            | LogicalExpression::ListPredicate { .. }
            | LogicalExpression::MapProjection { .. } => {
                vars.insert("\u{0}__opaque__".to_string());
            }
            _ => {}
        }
    }

    /// Extracts the remaining predicate after removing pushed-down equality conditions.
    ///
    /// Given `n.name = 'Alix' AND n.age > 30` with pushed conditions `[("name", "Alix")]`,
    /// returns `Some(n.age > 30)`. Returns `None` when all conditions were pushed down.
    pub(super) fn extract_remaining_predicate(
        &self,
        predicate: &LogicalExpression,
        target_variable: &str,
        pushed_conditions: &[(String, Value)],
    ) -> Option<LogicalExpression> {
        match predicate {
            LogicalExpression::Binary {
                left,
                op: BinaryOp::And,
                right,
            } => {
                let left_remaining =
                    self.extract_remaining_predicate(left, target_variable, pushed_conditions);
                let right_remaining =
                    self.extract_remaining_predicate(right, target_variable, pushed_conditions);

                match (left_remaining, right_remaining) {
                    (Some(l), Some(r)) => Some(LogicalExpression::Binary {
                        left: Box::new(l),
                        op: BinaryOp::And,
                        right: Box::new(r),
                    }),
                    (Some(l), None) => Some(l),
                    (None, Some(r)) => Some(r),
                    (None, None) => None,
                }
            }
            LogicalExpression::Binary {
                left,
                op: BinaryOp::Eq,
                right,
            } => {
                // Check if this equality was pushed down
                if let Some((var, prop, val)) = self.extract_property_equality(left, right)
                    && var == target_variable
                    && pushed_conditions
                        .iter()
                        .any(|(p, v)| *p == prop && *v == val)
                {
                    None // Already handled at the store level
                } else {
                    Some(predicate.clone())
                }
            }
            _ => Some(predicate.clone()),
        }
    }

    /// Extracts equality conditions (property = literal) from a predicate.
    ///
    /// Handles both simple predicates and AND chains:
    /// - `n.name = "Alix"` → `[("name", "Alix")]`
    /// - `n.name = "Alix" AND n.age = 30` → `[("name", "Alix"), ("age", 30)]`
    pub(super) fn extract_equality_conditions(
        &self,
        predicate: &LogicalExpression,
        target_variable: &str,
    ) -> Vec<(String, Value)> {
        let mut conditions = Vec::new();
        self.collect_equality_conditions(predicate, target_variable, &mut conditions);
        conditions
    }

    /// Recursively collects equality conditions from AND expressions.
    pub(super) fn collect_equality_conditions(
        &self,
        expr: &LogicalExpression,
        target_variable: &str,
        conditions: &mut Vec<(String, Value)>,
    ) {
        match expr {
            // Handle AND: recurse into both sides
            LogicalExpression::Binary {
                left,
                op: BinaryOp::And,
                right,
            } => {
                self.collect_equality_conditions(left, target_variable, conditions);
                self.collect_equality_conditions(right, target_variable, conditions);
            }

            // Handle equality: extract property and value
            LogicalExpression::Binary {
                left,
                op: BinaryOp::Eq,
                right,
            } => {
                if let Some((var, prop, val)) = self.extract_property_equality(left, right)
                    && var == target_variable
                {
                    conditions.push((prop, val));
                }
            }

            _ => {}
        }
    }

    /// Extracts (variable, property, value) from a property equality expression.
    ///
    /// Returns `None` for NULL literals: `property = NULL` is UNKNOWN in
    /// three-valued logic and must never be optimized into an index lookup.
    pub(super) fn extract_property_equality(
        &self,
        left: &LogicalExpression,
        right: &LogicalExpression,
    ) -> Option<(String, String, Value)> {
        match (left, right) {
            (
                LogicalExpression::Property { variable, property },
                LogicalExpression::Literal(val),
            ) if !val.is_null() => Some((variable.clone(), property.clone(), val.clone())),
            (
                LogicalExpression::Literal(val),
                LogicalExpression::Property { variable, property },
            ) if !val.is_null() => Some((variable.clone(), property.clone(), val.clone())),
            _ => None,
        }
    }

    /// Tries to optimize a filter using range queries on properties.
    ///
    /// This optimization is applied when:
    /// - The input is a simple NodeScan (no nested operations)
    /// - The predicate contains range comparisons (>, <, >=, <=)
    /// - The same variable and property are being filtered
    ///
    /// Handles both simple range predicates (`n.age > 30`) and BETWEEN patterns
    /// (`n.age >= 30 AND n.age <= 50`).
    ///
    /// Returns `Ok(Some((operator, columns)))` if optimization was applied,
    /// `Ok(None)` if not applicable, or `Err` on error.
    pub(super) fn try_plan_filter_with_range_index(
        &self,
        filter: &FilterOp,
    ) -> Result<Option<(Box<dyn Operator>, Vec<String>)>> {
        // Only optimize if input is a simple NodeScan (not nested)
        let (scan_variable, scan_label) = match filter.input.as_ref() {
            LogicalOperator::NodeScan(scan) if scan.input.is_none() => {
                (scan.variable.clone(), scan.label.clone())
            }
            _ => return Ok(None),
        };

        // Try to extract BETWEEN pattern first (more efficient)
        if let Some((variable, property, min, max, min_inc, max_inc)) =
            self.extract_between_predicate(&filter.predicate)
            && variable == scan_variable
        {
            return self.plan_range_filter(
                &scan_variable,
                &scan_label,
                &property,
                RangeBounds {
                    min: Some(&min),
                    max: Some(&max),
                    min_inclusive: min_inc,
                    max_inclusive: max_inc,
                },
            );
        }

        // Try to extract simple range predicate
        if let Some((variable, property, op, value)) =
            self.extract_range_predicate(&filter.predicate)
            && variable == scan_variable
        {
            let (min, max, min_inc, max_inc) = match op {
                BinaryOp::Lt => (None, Some(value), false, false),
                BinaryOp::Le => (None, Some(value), false, true),
                BinaryOp::Gt => (Some(value), None, false, false),
                BinaryOp::Ge => (Some(value), None, true, false),
                _ => return Ok(None),
            };
            return self.plan_range_filter(
                &scan_variable,
                &scan_label,
                &property,
                RangeBounds {
                    min: min.as_ref(),
                    max: max.as_ref(),
                    min_inclusive: min_inc,
                    max_inclusive: max_inc,
                },
            );
        }

        // (2026-09-11, chantier COUT-PROPORTIONNEL) `n.prop STARTS WITH 'p'`
        // est l'intervalle `[p, haute(p))` et rejoint donc le chemin ci-dessus
        // — index ordonne quand la propriete est declaree, balayage sinon,
        // l'arbitrage etant rendu par `find_nodes_in_range` lui-meme.
        //
        // `plage_de_prefixe` rend `None` quand aucune plage ne represente
        // FIDELEMENT le prefixe : borne haute inexistante (prefixe vide,
        // `char::MAX`, bord des surrogates) ou bornes analysables en datetime,
        // auquel cas `comparer_ordonnable` cesse d'ordonner par octets et
        // l'equivalence tombe. On retombe alors sur le filtre generique, qui
        // est correct. Le detail, la preuve et les tests : `prefix_range`.
        // L'INTERRUPTEUR DU TRONC, PORTÉ SUR LE CHEMIN DE j6 (2026-09-14).
        //
        // Les deux lignées réécrivent `STARTS WITH` en intervalle, chacune à sa
        // manière : le tronc en ligne, avec `successeur_de_prefixe` ; j6 par le
        // module `prefix_range`, qui REFUSE en plus les préfixes analysables en
        // date-heure (l'ordre cesse alors d'être celui des octets, donc
        // l'équivalence tombe). C'est la version de j6 qui sert ici.
        //
        // Ce qui manquait, ce n'était pas la réécriture mais son INSTRUMENT :
        // `OBRAIN_PREFIXE_INTERVALLE=0` la coupe, et c'est ce qui permet de
        // prouver que les lignes rendues sont IDENTIQUES avec et sans, puis de
        // mesurer les deux sous la même charge, dans le MÊME binaire. Il est
        // armé dans l'unité de déploiement ; sans ce report il y était INERTE.
        if prefixe_intervalle_actif()
            && let Some((variable, property, prefixe)) =
                super::prefix_range::extraire_prefixe(&filter.predicate)
            && variable == scan_variable
            // GARDE AJOUTÉE ENSUITE (mesurée le 2026-09-13) : la série ordonnée
            // doit être DÉJÀ LÀ. `index_ordonne_disponible` ne construit RIEN ;
            // la variante qui construisait payait un balayage complet (3 027 à
            // 16 315 ms sur 408 832 nœuds) pour emprunter ensuite un chemin plus
            // lent que le balayage quand la clef tronquée de l'index n'est pas
            // sélective — ce qui est le cas de TOUT préfixe de chemin. Sans la
            // série, le filtre d'expression garde la main et répond en 65 ms.
            && self.store.index_ordonne_disponible(&property)
            && let Some((min, max)) = super::prefix_range::plage_de_prefixe(&prefixe)
        {
            let min = Value::String(min.into());
            let max = Value::String(max.into());
            return self.plan_range_filter(
                &scan_variable,
                &scan_label,
                &property,
                RangeBounds {
                    min: Some(&min),
                    max: Some(&max),
                    min_inclusive: true,
                    max_inclusive: false,
                },
            );
        }
        Ok(None)
    }

    /// `n.propriete STARTS WITH 'litteral'` → (variable, propriété, préfixe).
    ///
    /// Aucune autre forme n'est retenue : ni l'opérande calculé, ni le littéral
    /// non textuel, ni le préfixe à droite (`'p' STARTS WITH n.prop`, qui n'est
    /// pas un test de préfixe sur la propriété). Tout le reste suit le chemin
    /// d'origine, donc aucune sémantique n'est réécrite à l'aveugle.
    fn extract_prefix_predicate(
        &self,
        predicate: &LogicalExpression,
    ) -> Option<(String, String, String)> {
        let LogicalExpression::Binary {
            left,
            op: BinaryOp::StartsWith,
            right,
        } = predicate
        else {
            return None;
        };
        match (left.as_ref(), right.as_ref()) {
            (
                LogicalExpression::Property { variable, property },
                LogicalExpression::Literal(Value::String(prefixe)),
            ) => Some((variable.clone(), property.clone(), prefixe.to_string())),
            _ => None,
        }
    }

    /// Plans a range filter using `find_nodes_in_range`.
    pub(super) fn plan_range_filter(
        &self,
        scan_variable: &str,
        scan_label: &Option<String>,
        property: &str,
        bounds: RangeBounds<'_>,
    ) -> Result<Option<(Box<dyn Operator>, Vec<String>)>> {
        // ── LA MÊME GARDE QUE LE PRÉFIXE, AU MÊME ENDROIT (2026-09-14) ────
        //
        // `try_plan_filter_with_range_index` arme cette fonction SANS garde
        // pour `n.prop >= x` et pour BETWEEN : mesuré sur une copie de
        // `runtime-dev`, `MATCH (n:Message) WHERE n.seq >= 1 RETURN count(n)`
        // coûte 289 000 à 468 000 µs de PLANIFICATION, à chaque appel (1er et
        // 2e identiques). `seq` n'a pas de série ordonnée disponible :
        // `find_nodes_in_range` retombe alors sur un balayage vérifié de toute
        // la propriété — c'est-à-dire, ici, une lecture de données complète
        // faite pour décider d'un plan.
        //
        // Le chemin `STARTS WITH` (appelant, plus haut) posait DÉJÀ cette
        // garde, et sa justification est la même, mesurée par son auteur :
        // « réécrire un prédicat en intervalle sur une propriété SANS index
        // ordonné ne rend rien plus rapide : `find_nodes_in_range` retombe sur
        // un balayage vérifié, 290 ms contre 65 ms pour le filtre d'expression ».
        // Elle manquait au chemin le plus direct — celui de la comparaison
        // nue. La garde est posée ICI, au point de passage unique des trois
        // appelants, pour qu'aucun quatrième ne puisse l'oublier.
        //
        // `index_ordonne_disponible` ne CONSTRUIT rien : elle constate
        // (mémoire ou série fiable sur disque) et se tait sinon. Sans série,
        // on rend la main au filtre d'expression, qui rend les mêmes lignes en
        // balayant une fois, à l'exécution, sans hydrater de `Node`.
        if elagage_actif() && !self.store.index_ordonne_disponible(property) {
            return Ok(None);
        }

        // Use the store's range query method
        let mut matching_nodes = self.store.find_nodes_in_range(
            property,
            bounds.min,
            bounds.max,
            bounds.min_inclusive,
            bounds.max_inclusive,
        );

        // Recoupement de label.
        //
        // (2026-08-21) AVANT : `nodes_by_label(label)` matérialisait le label
        // ENTIER dans un `HashSet` pour recouper k candidats que l'index de
        // plage venait déjà de rendre. C'est le balayage mmap du high-water
        // mark complet — 536 k créneaux mesurés en production, ~300 ms à froid
        // et 8 à 20 s sous contention — payé pour vérifier une appartenance.
        // `node_has_label` est le test de bit pur T13, déjà surchargé côté
        // substrat : O(k) réel.
        //
        // Le correctif du 2026-08-16 a fait exactement cette conversion sur la
        // branche indexée (l. 1286) et sur le chemin IN-list (l. 1390). Ce
        // chemin-ci a été oublié. Ce qui suit en est la transposition
        // littérale, branchement sur la transaction compris.
        if let Some(label) = scan_label {
            matching_nodes.retain(|n| self.store.node_has_label(*n, label).unwrap_or(false));
        }

        // Visibilité MVCC : l'index de plage peut porter des nœuds non visibles
        // à cette époque.
        //
        // (2026-08-21) AVANT : `get_node_versioned(...).is_some()` — un CLONE
        // de `PropertyMap` par candidat, pour un simple test d'existence. Les
        // surcharges `is_node_visible_*` existent précisément pour cela : même
        // sémantique, zéro hydratation. Même oubli que ci-dessus, même
        // correctif que chez les deux voisines.
        let epoch = self.viewing_epoch;
        if let Some(tx) = self.transaction_id {
            matching_nodes.retain(|id| self.store.is_node_visible_versioned(*id, epoch, tx));
        } else {
            matching_nodes.retain(|id| self.store.is_node_visible_at_epoch(*id, epoch));
        }

        // Create a NodeListOperator with the matching nodes
        let node_list_op = Box::new(NodeListOperator::new(matching_nodes, 2048));
        let columns = vec![scan_variable.to_string()];

        Ok(Some((node_list_op, columns)))
    }

    /// (chantier NS-PARTOUT, 2026-09-13) `n.prop CONTAINS 'litteral'` par
    /// l'index de trigrammes — candidats indexes, verification exacte
    /// derriere.
    ///
    /// ── POURQUOI PAS LE CHEMIN GENERIQUE ──
    ///
    /// Un CONTAINS tombait sur `FilterOperator` : balayage integral avec
    /// hydratation de chaque noeud (297-1145 ms sur 333 262 noeuds au banc
    /// driversnet, pour 44 lignes). Or la sous-chaine est decidable par
    /// index : « y sous-chaine de x » implique que TOUS les trigrammes
    /// d'octets de y figurent parmi ceux de x. L'intersection des postings
    /// des trigrammes du motif est donc un SUR-ENSEMBLE des reponses, que
    /// le store reverifie valeur par valeur (`str::contains` sur la valeur
    /// courante) : le resultat est EXACT, l'index n'est jamais cru sur
    /// parole (patron identique a `indexed_candidates`).
    ///
    /// ── GARDES, dans l'ordre des pieges ──
    ///
    /// * predicat COMPLET uniquement — `n.prop CONTAINS 'lit'` seul, pas
    ///   de descente dans les AND : meme prudence que `extraire_prefixe`
    ///   (STARTS WITH) et `extract_range_predicate` ;
    /// * litteral CHAINE uniquement : l'evaluateur (`BinaryFilterOp::
    ///   Contains`) fait `as_str()?` des deux cotes — sur tout autre type
    ///   il ne retient AUCUNE ligne, et une route indexee lui inventerait
    ///   des reponses ;
    /// * scan LIBRE uniquement (pas d'input) : le motif avec input part au
    ///   chemin generique, correct ;
    /// * `None` du store (« je ne sais pas ») → chemin generique. JAMAIS
    ///   `Some(vec![])` par ignorance — c'est le contrat du trait ;
    /// * la LIMITE en aval recoit une liste COMPLETE (NodeListOperator) :
    ///   aucun plafond amont a desarmer, le piege du scan plafonne ne peut
    ///   pas se reintroduire ici (test dans contains_indexe.rs).
    fn try_plan_filter_with_contains(
        &self,
        filter: &FilterOp,
    ) -> Result<Option<(Box<dyn Operator>, Vec<String>)>> {
        let LogicalOperator::NodeScan(scan) = filter.input.as_ref() else {
            return Ok(None);
        };
        if scan.input.is_some() {
            return Ok(None);
        }
        let LogicalExpression::Binary {
            left,
            op: BinaryOp::Contains,
            right,
        } = &filter.predicate
        else {
            return Ok(None);
        };
        let (
            LogicalExpression::Property { variable, property },
            LogicalExpression::Literal(Value::String(needle)),
        ) = (left.as_ref(), right.as_ref())
        else {
            return Ok(None);
        };
        if *variable != scan.variable {
            return Ok(None);
        }
        let Some(mut nodes) = self.store.find_nodes_by_property_contains(property, needle) else {
            return Ok(None);
        };

        // ── LA MÊME GARDE QUE LE CHEMIN D'INDEX DE TEXTE (2026-09-14) ───────
        //
        // DEUX chemins de planification servent `CONTAINS` — celui-ci par la
        // zone, et `try_plan_filter_with_text_index` par les trigrammes du
        // substrat. Un index qui rend BEAUCOUP de candidats coûte plus cher que
        // le balayage : MESURÉ sur `runtime-dev` (427 000 nœuds), aiguille
        // banale `banc_obrain` (35 115 lignes), **553 ms ici contre 80 ms au
        // balayage** — sept fois pire, mêmes lignes. La garde vaut donc pour les
        // DEUX chemins, sinon elle ne sert à rien selon celui qui gagne.
        //
        // Le seuil est RELATIF au store (1 %, plancher 256) : la même requête
        // doit se comporter pareil à mille et à dix millions de nœuds.
        // `OBRAIN_TEXTE_CANDIDATS_MAX` (pour cent) le règle pour la mesure.
        let total = self.store.node_count();
        let plafond = std::env::var("OBRAIN_TEXTE_CANDIDATS_MAX")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .map(|pct| total.saturating_mul(pct) / 100)
            .unwrap_or_else(|| (total / 100).max(256));
        if total > 0 && nodes.len() > plafond {
            tracing::debug!(
                candidats = nodes.len(),
                total,
                plafond,
                "contains : trop de candidats — la main au balayage (mesuré moins cher)"
            );
            return Ok(None);
        }
        // Etiquette du motif : test de bit par candidat, jamais la
        // materialisation du label entier (meme lecon que la route
        // d'egalite, 2026-08-16).
        if let Some(label) = &scan.label {
            nodes.retain(|n| self.store.node_has_label(*n, label).unwrap_or(false));
        }
        // Visibilite MVCC : memes retenues que la route d'egalite.
        let epoch = self.viewing_epoch;
        if let Some(tx) = self.transaction_id {
            nodes.retain(|id| self.store.is_node_visible_versioned(*id, epoch, tx));
        } else {
            nodes.retain(|id| self.store.is_node_visible_at_epoch(*id, epoch));
        }
        let op: Box<dyn Operator> = Box::new(NodeListOperator::new(nodes, 2048));
        Ok(Some((op, vec![scan.variable.clone()])))
    }

    /// ── SOUS-CHAÎNES : `CONTAINS` SERVI PAR L'INDEX DE TRIGRAMMES ────────
    ///
    /// LE DÉFAUT MESURÉ. Sur le banc figé (347 970 nœuds), les requêtes du
    /// journal MCP du 2026-09-12 — `n.name CONTAINS 'iam'`, puis les quatre
    /// termes `auth/Auth/token/Token` — coûtaient 148 à 190 ms **de balayage**
    /// pour rendre 12 à 489 lignes. Aucune structure de propriété ne pouvait
    /// les servir : l'égalité n'est pas une sous-chaîne, l'ordre ne classe pas
    /// une occurrence en milieu de valeur, et le BM25 du moteur cherche des
    /// MOTS (« iam » y manquerait `Miami` — un faux négatif, interdit).
    ///
    /// CE QUE CE CHEMIN FAIT. Il demande au substrat les nœuds qui satisfont le
    /// prédicat (`GraphStore::noeuds_par_predicat_texte`), qui les obtient par
    /// son index de trigrammes : candidats d'index, puis REVÉRIFICATION par le
    /// verdict texte sur les octets. La liste rendue est donc EXACTE.
    ///
    /// CE QU'IL NE FAIT PAS, ET C'EST DÉLIBÉRÉ. Il ne prend QUE si tout le
    /// prédicat est une disjonction de `CONTAINS` sur la même variable. Un
    /// `CONTAINS … AND n.x > 3` garderait un conjoint que ce plan ne servirait
    /// pas : la liste serait un sur-ensemble du résultat et le filtre serait
    /// perdu — des lignes fausses, en silence. Dans ce cas, et dès qu'une seule
    /// condition n'est pas servie par l'index, on rend `Ok(None)` : le plan
    /// d'origine, balayage compris, reprend **inchangé**.
    ///
    /// Mesuré (banc figé, seuil 4096) : `iam` sur `qualified_name` passe de
    /// 844 ms à 0,95 ms pour 386 = 386 lignes ; `auth` de 384 ms à 1,14 ms pour
    /// 1 226 = 1 226 lignes. Mêmes lignes, à chaque fois.
    pub(super) fn try_plan_filter_with_text_index(
        &self,
        filter: &FilterOp,
    ) -> Result<Option<(Box<dyn Operator>, Vec<String>)>> {
        // Le motif doit être un scan NU : sous un `input`, la liste de nœuds
        // qu'on rendrait ignorerait la jointure.
        let (scan_variable, scan_label) = match filter.input.as_ref() {
            LogicalOperator::NodeScan(scan) if scan.input.is_none() => {
                (scan.variable.clone(), scan.label.clone())
            }
            _ => return Ok(None),
        };

        // ── LE PRÉDICAT SE COUPE EN DEUX : la partie TEXTE, et le RESTE ────
        //
        // Jusqu'ici le crochet exigeait que TOUT le prédicat soit du texte, et
        // rendait la main sinon. C'est ce qui laissait C4/C8 du corpus réel —
        // `STARTS WITH … AND (n:Module OR n:Struct OR …)`, mesurés 139 à 280 ms
        // — sur un balayage complet : UN SEUL conjoint non textuel suffisait à
        // tout perdre.
        //
        // On pousse désormais la partie texte, et le RESTE (le prédicat
        // RÉSIDUEL) s'évalue AU-DESSUS, sur les seuls candidats. La correction
        // ne repose sur aucune approximation : rien n'est retiré du prédicat,
        // la liste de candidats devient l'entrée du filtre résiduel.
        let Some((conditions, residu)) =
            self.collecte_texte_et_residu(&filter.predicate, &scan_variable)
        else {
            return Ok(None);
        };

        // UNION des conditions, et non intersection : le prédicat est une
        // DISJONCTION. Chaque condition est servie exactement, donc l'union
        // l'est aussi ; le `dedup` absorbe les nœuds qui satisfont plusieurs
        // termes (`iam` et `IAM` sur la même valeur, par exemple).
        let mut trouves: Vec<NodeId> = Vec::new();
        for (propriete, op, aiguille) in &conditions {
            let cle = obrain_common::types::PropertyKey::new(propriete);
            // L'opérateur LOGIQUE devient le prédicat du substrat : c'est lui
            // qui servira de verdict de revérification, donc la sémantique
            // exacte de la requête (et non « contient », qui serait plus large).
            let predicat = match op {
                BinaryOp::Contains => obrain_core::graph::traits::PredicatTexte::Contient(aiguille),
                BinaryOp::StartsWith => {
                    obrain_core::graph::traits::PredicatTexte::CommencePar(aiguille)
                }
                BinaryOp::EndsWith => obrain_core::graph::traits::PredicatTexte::FiniPar(aiguille),
                _ => return Ok(None),
            };
            match self.store.noeuds_par_predicat_texte(&cle, &predicat) {
                // Une seule condition non servie ⇒ on ne planifie RIEN : servir
                // les autres rendrait une liste AMPUTÉE de ce terme-là.
                None => return Ok(None),
                Some(mut ids) => trouves.append(&mut ids),
            }
        }
        trouves.sort_unstable();
        trouves.dedup();

        // ── LA GARDE QUI EMPÊCHE L'INDEX DE NUIRE (2026-09-14, MESURÉ) ──────
        //
        // POURQUOI ELLE EXISTE. Un index qui rend BEAUCOUP de candidats est plus
        // LENT que le balayage : chaque candidat est revérifié individuellement,
        // alors que le balayage lit tout d'un trait avec la lecture par lot
        // (~187 ns/ligne). MESURÉ sur `runtime-dev` (427 000 nœuds), aiguille
        // banale `banc_obrain` (35 115 lignes) : **790 ms par l'index contre
        // 80 ms au balayage** — dix fois PIRE. Mêmes lignes des deux côtés (le
        // contrôle d'équivalence a été fait) : ce n'est pas une question de
        // justesse, c'est une question de coût.
        //
        // CE QU'ELLE FAIT. Passé le seuil, on REND LA MAIN (`Ok(None)`) : le
        // planificateur retombe sur le filtre d'expression, donc sur le
        // balayage — exactement ce qu'on veut. L'index ne peut donc plus jamais
        // coûter plus cher que son absence : c'est la condition pour l'activer
        // PAR DÉFAUT.
        //
        // LE SEUIL EST RELATIF AU STORE, jamais un nombre de lignes fixe : la
        // même requête doit se comporter pareil à mille et à dix millions de
        // nœuds. Le point d'équilibre mesuré est vers **1 %** (chaque candidat
        // coûte ~22 µs contre ~0,19 µs par ligne balayée) ; `OBRAIN_TEXTE_CANDIDATS_MAX`
        // (en pour cent) le règle pour la mesure, et un plancher de 256 évite
        // d'abandonner l'index sur les petits stores.
        let total = self.store.node_count();
        let plafond = std::env::var("OBRAIN_TEXTE_CANDIDATS_MAX")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .map(|pct| total.saturating_mul(pct) / 100)
            .unwrap_or_else(|| (total / 100).max(256));
        if total > 0 && trouves.len() > plafond {
            tracing::debug!(
                candidats = trouves.len(),
                total,
                plafond,
                "index de texte : trop de candidats — la main au balayage (mesuré moins cher)"
            );
            return Ok(None);
        }

        // Recoupement de label puis visibilité MVCC : la transposition exacte
        // de `plan_range_filter`, pour que les deux chemins d'index ne
        // divergent pas sur ces deux règles.
        let mut matching_nodes = trouves;
        if let Some(label) = &scan_label {
            matching_nodes.retain(|n| self.store.node_has_label(*n, label).unwrap_or(false));
        }
        let epoch = self.viewing_epoch;
        if let Some(tx) = self.transaction_id {
            matching_nodes.retain(|id| self.store.is_node_visible_versioned(*id, epoch, tx));
        } else {
            matching_nodes.retain(|id| self.store.is_node_visible_at_epoch(*id, epoch));
        }

        let node_list_op: Box<dyn Operator> = Box::new(NodeListOperator::new(matching_nodes, 2048));
        let columns = vec![scan_variable];

        // Pas de résidu : la liste de candidats EST la réponse (le substrat a
        // revérifié chaque candidat avec le prédicat exact).
        let Some(residu) = residu else {
            return Ok(Some((node_list_op, columns)));
        };

        // Avec un résidu : il s'évalue AU-DESSUS des candidats, avec la même
        // machinerie que le chemin générique — donc la même sémantique que si
        // le plan n'avait pas été poussé. `variable_columns` est la carte du
        // `NodeListOperator` : une seule colonne, la variable scannée.
        let variable_columns: HashMap<String, usize> = columns
            .iter()
            .enumerate()
            .map(|(i, nom)| (nom.clone(), i))
            .collect();
        let filtre_residuel = self.convert_expression(&residu)?;
        let predicat = ExpressionPredicate::new(
            filtre_residuel,
            variable_columns,
            Arc::clone(&self.store) as Arc<dyn GraphStore>,
        )
        .with_transaction_context(self.viewing_epoch, self.transaction_id)
        .with_session_context(self.session_context.clone());
        Ok(Some((
            Box::new(FilterOperator::new(node_list_op, Box::new(predicat))),
            columns,
        )))
    }

    /// Coupe un prédicat en **(partie texte poussable, prédicat résiduel)**.
    ///
    /// `None` quand AUCUN conjoint n'est un prédicat de sous-chaîne : le
    /// crochet n'a alors rien à proposer, et le plan d'origine suit.
    ///
    /// Le PREMIER conjoint entièrement textuel porte la poussée ; tous les
    /// autres — y compris un second conjoint textuel — forment le résidu. On
    /// n'essaie donc jamais d'INTERSECTER deux parties texte : intersecter
    /// serait correct mais coûteux à écrire, alors que laisser le second au
    /// résidu est exact et déjà payé par le filtre.
    fn collecte_texte_et_residu(
        &self,
        predicate: &LogicalExpression,
        variable: &str,
    ) -> Option<(Vec<(String, BinaryOp, String)>, Option<LogicalExpression>)> {
        let mut conjoints: Vec<&LogicalExpression> = Vec::new();
        Self::aplatir_conjonctions(predicate, &mut conjoints);

        let mut choisi: Option<usize> = None;
        for (i, c) in conjoints.iter().enumerate() {
            let mut conditions: Vec<(String, BinaryOp, String)> = Vec::new();
            if self.collecte_contains(c, variable, &mut conditions) && !conditions.is_empty() {
                choisi = Some(i);
                break;
            }
        }
        let i = choisi?;

        let mut conditions: Vec<(String, BinaryOp, String)> = Vec::new();
        if !self.collecte_contains(conjoints[i], variable, &mut conditions) {
            return None;
        }

        let mut residu: Option<LogicalExpression> = None;
        for (j, c) in conjoints.iter().enumerate() {
            if j == i {
                continue;
            }
            residu = Some(match residu {
                None => (*c).clone(),
                Some(r) => LogicalExpression::Binary {
                    left: Box::new(r),
                    op: BinaryOp::And,
                    right: Box::new((*c).clone()),
                },
            });
        }
        Some((conditions, residu))
    }

    /// Aplatit un arbre de `AND` en ses conjoints, dans l'ordre d'écriture.
    fn aplatir_conjonctions<'a>(expr: &'a LogicalExpression, out: &mut Vec<&'a LogicalExpression>) {
        match expr {
            LogicalExpression::Binary {
                left,
                op: BinaryOp::And,
                right,
            } => {
                Self::aplatir_conjonctions(left, out);
                Self::aplatir_conjonctions(right, out);
            }
            autre => out.push(autre),
        }
    }

    /// Rassemble les `(propriété, opérateur, aiguille)` d'un prédicat qui n'est
    /// QUE des prédicats de SOUS-CHAÎNE sur `variable`, liés par des `OR`.
    ///
    /// Les trois formes sont prises — `CONTAINS`, `STARTS WITH`, `ENDS WITH` —
    /// parce que l'index de trigrammes les sert toutes les trois : les grammes
    /// de l'aiguille donnent un sur-ensemble dans chaque cas, et le verdict
    /// tranche. MESURÉ : `STARTS WITH` n'avait aucune poussée avant ceci
    /// (84 à 349 ms sur le corpus réel, avec ou sans index).
    ///
    /// Rend `false` — donc « ne touche à rien » — dès qu'un nœud de l'arbre
    /// n'est pas de cette forme : un `AND`, un `NOT`, une autre variable, une
    /// aiguille qui n'est pas un littéral de chaîne, ou tout autre opérateur.
    /// C'est CONSERVATEUR À DESSEIN : le chemin lent reste juste, un plan qui
    /// ne servirait qu'une partie du prédicat serait faux.
    fn collecte_contains(
        &self,
        expr: &LogicalExpression,
        variable: &str,
        out: &mut Vec<(String, BinaryOp, String)>,
    ) -> bool {
        match expr {
            LogicalExpression::Binary { left, op, right }
                if matches!(
                    op,
                    BinaryOp::Contains | BinaryOp::StartsWith | BinaryOp::EndsWith
                ) =>
            {
                let op = *op;
                let _ = op;
                if let (
                    LogicalExpression::Property {
                        variable: v,
                        property,
                    },
                    LogicalExpression::Literal(Value::String(aiguille)),
                ) = (left.as_ref(), right.as_ref())
                {
                    if v == variable {
                        out.push((property.clone(), op, aiguille.to_string()));
                        return true;
                    }
                }
                false
            }
            LogicalExpression::Binary {
                left,
                op: BinaryOp::Or,
                right,
            } => {
                // Les deux côtés doivent être servis : `a CONTAINS x OR b > 3`
                // n'est PAS planifiable ici.
                self.collecte_contains(left, variable, out)
                    && self.collecte_contains(right, variable, out)
            }
            _ => false,
        }
    }

    /// Extracts a simple range predicate (>, <, >=, <=) from an expression.
    ///
    /// Returns `(variable, property, operator, value)` if found.
    pub(super) fn extract_range_predicate(
        &self,
        predicate: &LogicalExpression,
    ) -> Option<(String, String, BinaryOp, Value)> {
        match predicate {
            LogicalExpression::Binary { left, op, right } => {
                match op {
                    BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => {
                        // Three-valued logic: `x <op> NULL` is UNKNOWN for every
                        // row, so it must never be turned into an index range
                        // scan. `find_nodes_in_range` cannot represent a NULL
                        // bound (`OrderableValue::try_from(Null)` fails) and
                        // silently drops it, degenerating the predicate into
                        // "property IS NOT NULL". Bail out so the generic
                        // FilterOperator path evaluates it and yields UNKNOWN.
                        if matches!(
                            (left.as_ref(), right.as_ref()),
                            (LogicalExpression::Literal(Value::Null), _)
                                | (_, LogicalExpression::Literal(Value::Null))
                        ) {
                            return None;
                        }

                        // Try property on left: n.age > 30
                        if let (
                            LogicalExpression::Property { variable, property },
                            LogicalExpression::Literal(val),
                        ) = (left.as_ref(), right.as_ref())
                        {
                            return Some((variable.clone(), property.clone(), *op, val.clone()));
                        }

                        // Try property on right: 30 < n.age (flip operator)
                        if let (
                            LogicalExpression::Literal(val),
                            LogicalExpression::Property { variable, property },
                        ) = (left.as_ref(), right.as_ref())
                        {
                            let flipped_op = match op {
                                BinaryOp::Lt => BinaryOp::Gt,
                                BinaryOp::Le => BinaryOp::Ge,
                                BinaryOp::Gt => BinaryOp::Lt,
                                BinaryOp::Ge => BinaryOp::Le,
                                _ => return None,
                            };
                            return Some((
                                variable.clone(),
                                property.clone(),
                                flipped_op,
                                val.clone(),
                            ));
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        None
    }

    /// Extracts a BETWEEN pattern from compound predicates.
    ///
    /// Recognizes patterns like:
    /// - `n.age >= 30 AND n.age <= 50`
    /// - `n.age > 30 AND n.age < 50`
    ///
    /// Returns `(variable, property, min_value, max_value, min_inclusive, max_inclusive)`.
    pub(super) fn extract_between_predicate(
        &self,
        predicate: &LogicalExpression,
    ) -> Option<(String, String, Value, Value, bool, bool)> {
        // Must be an AND expression
        let (left, right) = match predicate {
            LogicalExpression::Binary {
                left,
                op: BinaryOp::And,
                right,
            } => (left.as_ref(), right.as_ref()),
            _ => return None,
        };

        // Extract range predicates from both sides
        let left_range = self.extract_range_predicate(left);
        let right_range = self.extract_range_predicate(right);

        let (left_var, left_prop, left_op, left_val) = left_range?;
        let (right_var, right_prop, right_op, right_val) = right_range?;

        // Must be same variable and property
        if left_var != right_var || left_prop != right_prop {
            return None;
        }

        // Determine which is lower bound and which is upper bound
        let (min_val, max_val, min_inc, max_inc) = match (left_op, right_op) {
            // n.x >= min AND n.x <= max
            (BinaryOp::Ge, BinaryOp::Le) => (left_val, right_val, true, true),
            // n.x >= min AND n.x < max
            (BinaryOp::Ge, BinaryOp::Lt) => (left_val, right_val, true, false),
            // n.x > min AND n.x <= max
            (BinaryOp::Gt, BinaryOp::Le) => (left_val, right_val, false, true),
            // n.x > min AND n.x < max
            (BinaryOp::Gt, BinaryOp::Lt) => (left_val, right_val, false, false),
            // Reversed order: n.x <= max AND n.x >= min
            (BinaryOp::Le, BinaryOp::Ge) => (right_val, left_val, true, true),
            // n.x < max AND n.x >= min
            (BinaryOp::Lt, BinaryOp::Ge) => (right_val, left_val, true, false),
            // n.x <= max AND n.x > min
            (BinaryOp::Le, BinaryOp::Gt) => (right_val, left_val, false, true),
            // n.x < max AND n.x > min
            (BinaryOp::Lt, BinaryOp::Gt) => (right_val, left_val, false, false),
            _ => return None,
        };

        Some((left_var, left_prop, min_val, max_val, min_inc, max_inc))
    }
}

/// Plus petite chaîne STRICTEMENT supérieure à toutes celles qui commencent par
/// `prefixe` — ou `None` quand elle n'existe pas (voir la démonstration dans
/// `try_plan_filter_with_range_index`).
///
/// Le DERNIER point de code est remplacé par le suivant VALIDE : les substituts
/// U+D800-U+DFFF ne sont pas des caractères, donc le suivant de U+D7FF est
/// U+E000 — sauter ce trou n'est pas une approximation, c'est la définition du
/// successeur dans l'ensemble des chaînes UTF-8. Rendre U+E000 plutôt que de
/// retomber sur l'avant-dernier caractère est ce qui garde la borne EXACTE : un
/// `STARTS WITH` réécrit en intervalle doit rendre exactement les mêmes lignes.
#[must_use]
pub fn successeur_de_prefixe(prefixe: &str) -> Option<String> {
    let mut points: Vec<char> = prefixe.chars().collect();
    while let Some(dernier) = points.pop() {
        let mut suivant = u32::from(dernier) + 1;
        if (0xD800..=0xDFFF).contains(&suivant) {
            suivant = 0xE000;
        }
        if let Some(c) = char::from_u32(suivant) {
            points.push(c);
            return Some(points.into_iter().collect());
        }
    }
    None
}

/// `OBRAIN_PREFIXE_INTERVALLE=0` coupe la réécriture `STARTS WITH` → intervalle.
///
/// L'interrupteur vient du tronc, où il servait à prouver l'équivalence des
/// lignes rendues puis à mesurer les deux chemins sous la même charge, dans le
/// MÊME binaire. Il est armé dans l'unité de déploiement : le binaire servi doit
/// donc le LIRE, sans quoi il est inerte — et un réglage inerte fait attribuer
/// au réglage un comportement qui vient d'ailleurs.
///
/// Lu UNE fois par processus (`OnceLock`) : ce chemin est chaud, et la variable
/// ne change pas en cours de route.
fn prefixe_intervalle_actif() -> bool {
    static ACTIF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ACTIF.get_or_init(|| std::env::var("OBRAIN_PREFIXE_INTERVALLE").map_or(true, |v| v != "0"))
}

/// `OBRAIN_PLAN_ELAGAGE=0` rend la main aux heuristiques de planification qui
/// MATÉRIALISENT des nœuds À LA PLANIFICATION.
///
/// ── POURQUOI CE QUI SUIT EXISTE (2026-09-14, `banc_plan_froid`) ────────
///
/// La ventilation `OBRAIN_PHASES` sépare, dans une exécution, la
/// PLANIFICATION de l'EXÉCUTION. Mesuré sur une copie de `runtime-dev`
/// (432 910 nœuds, label `Message` = 71 445 nœuds), textes UNIQUES :
///
/// ```text
///   MATCH (n) RETURN count(n)                                   planif      1 µs
///   MATCH (n:Message) WHERE n.session = '…' RETURN count(n)     planif  192 859 µs
///   MATCH (n:Message) WHERE n.seq >= 1 RETURN count(n)          planif  339 098 µs
///   MATCH (n:Class) WHERE n.qualified_name = '…' RETURN count(n) planif     27 µs
/// ```
///
/// Le dernier cas est INDEXÉ au catalogue et coûte 27 µs ; les deux premiers
/// ne le sont pas et coûtent 200 à 340 ms. L'écart n'est pas un coût de
/// « décision d'accès » : c'est un coût de LECTURE. Ces heuristiques
/// matérialisent à la planification ce que l'exécution lirait mieux :
///
/// * sans index, l'égalité sur propriété balaie le label ENTIER et HYDRATE
///   chaque nœud (`get_node_at_epoch`, clone de `PropertyMap`) pour tester la
///   propriété — 71 445 × ~2,7 µs = 192 ms, mesuré, à CHAQUE appel ;
/// * sans série ordonnée, `find_nodes_in_range` retombe sur un balayage
///   vérifié — 289 à 468 ms mesurés, à CHAQUE appel.
///
/// Ce prix est payé pour rendre une liste de nœuds que l'exécution aurait
/// constituée pour ~0,2 µs par nœud (balayage + filtre d'expression, mesuré
/// 432 910 nœuds en 78 ms sur le chemin sans label). La matérialisation
/// n'est donc PAS une épargne reportée : elle est un surcoût immédiat, et
/// elle est payée deux fois — à la planification, puis à chaque appel.
///
/// ── CE QUE LES GARDES NE CHANGENT PAS ─────────────────────────────────
///
/// Elles ne changent AUCUNE ligne rendue : au-dessus du plafond, la
/// planification rend la main (`Ok(None)`) au chemin GÉNÉRIQUE, qui balaie et
/// applique le filtre à l'exécution — le chemin d'origine de toutes les autres
/// requêtes, déjà couvert par la suite de tests. `OBRAIN_PLAN_ELAGAGE=0`
/// rétablit les heuristiques dans le MÊME binaire : c'est l'instrument qui
/// permet de comparer les lignes rendues avec et sans, puis de mesurer les
/// deux sous la même charge (patron de `OBRAIN_PREFIXE_INTERVALLE`).
fn elagage_actif() -> bool {
    static ACTIF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ACTIF.get_or_init(|| std::env::var("OBRAIN_PLAN_ELAGAGE").map_or(true, |v| v != "0"))
}

/// Nombre de nœuds qu'une heuristique de planification a le droit de
/// matérialiser avant de rendre la main au chemin générique.
///
/// Réglé à 1024 : au coût mesuré de l'hydratation (~2,7 µs/nœud), la
/// matérialisation reste sous ~3 ms — l'ordre de grandeur d'une planification
/// ordinaire — au lieu des 192 ms que coûte un label de 71 445 nœuds.
///
/// (2026-11-08, C1) Sous le plafond, l'égalité sans index ne paie plus
/// l'hydratation : UNE propriété par candidat, en lot
/// (`get_node_property_batch_at_epoch`). Le plafond reste à 1024, non
/// remesuré — le relever est une décision à prendre sur une mesure du coût
/// par nœud du lot, pas une conséquence automatique de C1.
/// `OBRAIN_PLAN_MATERIALISATION_MAX=<n>` règle le plafond (0 = jamais
/// matérialiser) ; lire l'interrupteur ne coûte rien, il est lu une fois par
/// processus.
fn plafond_materialisation() -> u64 {
    static PLAFOND: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *PLAFOND.get_or_init(|| {
        std::env::var("OBRAIN_PLAN_MATERIALISATION_MAX")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(1024)
    })
}
