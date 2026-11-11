//! Path, node, edge, and mutation pattern translation.

#[allow(clippy::wildcard_imports)]
use super::*;

/// Comment une arête est parcourue par `expand_edge`.
struct EdgeWalk<'a> {
    /// De la cible vers la source (chemin repris à rebours depuis son ancre).
    reversed: bool,
    /// Alias du chemin nommé, porté par sa dernière arête.
    path_alias: Option<&'a str>,
    path_mode: PathMode,
}

impl GqlTranslator {
    /// Predicat du motif qualifie par base (V2) : `base(var) = "slug"`.
    pub(super) fn predicat_base(variable: &str, slug: &str) -> LogicalExpression {
        LogicalExpression::Binary {
            left: Box::new(LogicalExpression::FunctionCall {
                name: "base".into(),
                args: vec![LogicalExpression::Variable(variable.to_string())],
                distinct: false,
            }),
            op: BinaryOp::Eq,
            right: Box::new(LogicalExpression::Literal(Value::from(slug))),
        }
    }

    /// Refus enseignant : IN "slug" est un motif de LECTURE.
    pub(super) fn erreur_in_ecriture() -> Error {
        Error::Query(QueryError::new(
            QueryErrorKind::Semantic,
            "IN \"slug\" qualifie un motif de LECTURE (MATCH). En ecriture, \
             la cible d'une autre base s'adresse par ref(\"slug\", \"clef\")",
        ))
    }

    /// Refus enseignant : la SOURCE d'une arete inter-bases doit vivre
    /// dans la base courante (une ecriture ne touche qu'UNE base, sa WAL).
    pub(super) fn erreur_source_ref() -> Error {
        Error::Query(QueryError::new(
            QueryErrorKind::Semantic,
            "la SOURCE d'une arete inter-bases doit vivre dans la base courante : \
             une ecriture ne touche qu'une seule base (sa WAL). Ouvre une session \
             sur la base source et ecris l'arete depuis elle : \
             CREATE (a)-[:TYPE]->(ref(\"slug\", \"clef\"))",
        ))
    }

    /// Refus enseignant : ref() n'est pas un motif de lecture ni un motif
    /// de creation de noeud.
    pub(super) fn erreur_ref_hors_cible() -> Error {
        Error::Query(QueryError::new(
            QueryErrorKind::Semantic,
            "ref(\"slug\", \"clef\") designe un noeud d'une AUTRE base et ne \
             s'utilise aujourd'hui que comme CIBLE d'une arete en CREATE/MERGE : \
             CREATE (a)-[:TYPE]->(ref(...)). En lecture, les aretes inter-bases \
             se traversent par un MATCH ordinaire sur les aretes existantes",
        ))
    }

    /// Refus enseignant : pas de proprietes ni de variable d'arete en V1.
    pub(super) fn erreur_ref_arete_riche() -> Error {
        Error::Query(QueryError::new(
            QueryErrorKind::Semantic,
            "une arete inter-bases ne porte pas encore de proprietes ni de \
             variable d'arete (V1) : ecris CREATE (a)-[:TYPE]->(ref(...)) sans \
             {props} ni [r:...]",
        ))
    }

    /// Refus enseignant : l'arete inter-bases est orientee.
    pub(super) fn erreur_ref_orientation() -> Error {
        Error::Query(QueryError::new(
            QueryErrorKind::Semantic,
            "une arete inter-bases est ORIENTEE et part de la base courante : \
             ecris (a)-[:TYPE]->(ref(...)). Pour l'autre sens, ecris depuis \
             l'autre base",
        ))
    }

    /// Translates a MERGE clause into a MergeOp.
    pub(super) fn translate_merge(
        &self,
        merge_clause: &ast::MergeClause,
        input: LogicalOperator,
    ) -> Result<LogicalOperator> {
        let (variable, labels, match_properties) = match &merge_clause.pattern {
            ast::Pattern::Node(node) => {
                if node.reference.is_some() {
                    return Err(Self::erreur_ref_hors_cible());
                }
                if node.base_qualifier.is_some() {
                    return Err(Self::erreur_in_ecriture());
                }
                let var = node
                    .variable
                    .clone()
                    .unwrap_or_else(|| format!("_anon_{}", rand_id()));
                let labels = node.labels.clone();
                let props: Vec<(String, LogicalExpression)> = node
                    .properties
                    .iter()
                    .map(|(k, v)| Ok((k.clone(), self.translate_expression(v)?)))
                    .collect::<Result<_>>()?;
                (var, labels, props)
            }
            ast::Pattern::Path(path) if !path.edges.is_empty() => {
                return self.translate_merge_relationship(path, merge_clause, input);
            }
            ast::Pattern::Path(path) => {
                // Path with no edges is just a node pattern
                let var = path
                    .source
                    .variable
                    .clone()
                    .unwrap_or_else(|| format!("_anon_{}", rand_id()));
                let labels = path.source.labels.clone();
                let props: Vec<(String, LogicalExpression)> = path
                    .source
                    .properties
                    .iter()
                    .map(|(k, v)| Ok((k.clone(), self.translate_expression(v)?)))
                    .collect::<Result<_>>()?;
                (var, labels, props)
            }
            ast::Pattern::Quantified { .. }
            | ast::Pattern::Union(_)
            | ast::Pattern::MultisetUnion(_) => {
                return Err(Error::Query(QueryError::new(
                    QueryErrorKind::Semantic,
                    "MERGE does not support quantified or union patterns",
                )));
            }
        };

        let on_create: Vec<(String, LogicalExpression)> = merge_clause
            .on_create
            .as_ref()
            .map(|assignments| {
                assignments
                    .iter()
                    .map(|a| Ok((a.property.clone(), self.translate_expression(&a.value)?)))
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?
            .unwrap_or_default();

        let on_match: Vec<(String, LogicalExpression)> = merge_clause
            .on_match
            .as_ref()
            .map(|assignments| {
                assignments
                    .iter()
                    .map(|a| Ok((a.property.clone(), self.translate_expression(&a.value)?)))
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?
            .unwrap_or_default();

        Ok(LogicalOperator::Merge(MergeOp {
            variable,
            labels,
            match_properties,
            on_create,
            on_match,
            input: Box::new(input),
        }))
    }

    /// Translates a MERGE with a relationship pattern.
    pub(super) fn translate_merge_relationship(
        &self,
        path: &ast::PathPattern,
        merge_clause: &ast::MergeClause,
        input: LogicalOperator,
    ) -> Result<LogicalOperator> {
        let mut current_input = input;

        if path.source.reference.is_some() {
            return Err(Self::erreur_source_ref());
        }
        if path.source.base_qualifier.is_some()
            || path.edges.iter().any(|e| e.target.base_qualifier.is_some())
        {
            return Err(Self::erreur_in_ecriture());
        }

        let source_variable = path.source.variable.clone().ok_or_else(|| {
            Error::Query(QueryError::new(
                QueryErrorKind::Semantic,
                "MERGE relationship pattern requires a source node variable",
            ))
        })?;

        // If source node has labels or properties, emit a MergeOp for it
        if !path.source.labels.is_empty() || !path.source.properties.is_empty() {
            let node_props: Vec<(String, LogicalExpression)> = path
                .source
                .properties
                .iter()
                .map(|(k, v)| Ok((k.clone(), self.translate_expression(v)?)))
                .collect::<Result<Vec<_>>>()?;

            current_input = LogicalOperator::Merge(MergeOp {
                variable: source_variable.clone(),
                labels: path.source.labels.clone(),
                match_properties: node_props,
                on_create: Vec::new(),
                on_match: Vec::new(),
                input: Box::new(current_input),
            });
        }

        let edge = path.edges.first().ok_or_else(|| {
            Error::Query(QueryError::new(
                QueryErrorKind::Semantic,
                "MERGE relationship pattern is empty",
            ))
        })?;

        // Aretes inter-bases (V1) : MERGE (a)-[:T]->(ref(...)) — variante
        // idempotente de CREATE, une arete identique n'est pas dupliquee.
        if let Some(reference) = &edge.target.reference {
            if path.edges.len() != 1 {
                return Err(Error::Query(QueryError::new(
                    QueryErrorKind::Semantic,
                    "un ref(...) termine un motif d'ecriture : rien ne peut \
                     s'enchainer apres une cible d'une autre base",
                )));
            }
            match edge.direction {
                ast::EdgeDirection::Outgoing => {}
                ast::EdgeDirection::Incoming => return Err(Self::erreur_source_ref()),
                ast::EdgeDirection::Undirected => {
                    return Err(Self::erreur_ref_orientation());
                }
            }
            if !edge.properties.is_empty() || edge.variable.is_some() {
                return Err(Self::erreur_ref_arete_riche());
            }
            if merge_clause.on_create.is_some() || merge_clause.on_match.is_some() {
                return Err(Self::erreur_ref_arete_riche());
            }
            let edge_type = edge.types.first().cloned().ok_or_else(|| {
                Error::Query(QueryError::new(
                    QueryErrorKind::Semantic,
                    "une arete inter-bases exige un type : \
                     MERGE (a)-[:TYPE]->(ref(...))",
                ))
            })?;
            return Ok(LogicalOperator::CreateExternalEdge(CreateExternalEdgeOp {
                from_variable: source_variable,
                edge_type,
                to_host: reference.host.clone(),
                to_store: reference.store.clone(),
                to_key: reference.key.clone(),
                merge: true,
                input: Box::new(current_input),
            }));
        }

        let variable = edge
            .variable
            .clone()
            .unwrap_or_else(|| format!("_merge_rel_{}", rand_id()));

        let edge_type = edge.types.first().cloned().ok_or_else(|| {
            Error::Query(QueryError::new(
                QueryErrorKind::Semantic,
                "MERGE relationship pattern requires an edge type",
            ))
        })?;

        let target_variable = edge.target.variable.clone().ok_or_else(|| {
            Error::Query(QueryError::new(
                QueryErrorKind::Semantic,
                "MERGE relationship pattern requires a target node variable",
            ))
        })?;

        // If target node has labels or properties, emit a MergeOp for it
        if !edge.target.labels.is_empty() || !edge.target.properties.is_empty() {
            let node_props: Vec<(String, LogicalExpression)> = edge
                .target
                .properties
                .iter()
                .map(|(k, v)| Ok((k.clone(), self.translate_expression(v)?)))
                .collect::<Result<Vec<_>>>()?;

            current_input = LogicalOperator::Merge(MergeOp {
                variable: target_variable.clone(),
                labels: edge.target.labels.clone(),
                match_properties: node_props,
                on_create: Vec::new(),
                on_match: Vec::new(),
                input: Box::new(current_input),
            });
        }

        let match_properties: Vec<(String, LogicalExpression)> = edge
            .properties
            .iter()
            .map(|(k, v)| Ok((k.clone(), self.translate_expression(v)?)))
            .collect::<Result<Vec<_>>>()?;

        let on_create: Vec<(String, LogicalExpression)> = merge_clause
            .on_create
            .as_ref()
            .map(|assignments| {
                assignments
                    .iter()
                    .map(|a| Ok((a.property.clone(), self.translate_expression(&a.value)?)))
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?
            .unwrap_or_default();

        let on_match: Vec<(String, LogicalExpression)> = merge_clause
            .on_match
            .as_ref()
            .map(|assignments| {
                assignments
                    .iter()
                    .map(|a| Ok((a.property.clone(), self.translate_expression(&a.value)?)))
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?
            .unwrap_or_default();

        Ok(LogicalOperator::MergeRelationship(MergeRelationshipOp {
            variable,
            source_variable,
            target_variable,
            edge_type,
            match_properties,
            on_create,
            on_match,
            input: Box::new(current_input),
        }))
    }

    /// Translates CREATE patterns to create operators.
    pub(super) fn translate_create_patterns(
        &self,
        patterns: &[ast::Pattern],
        mut plan: LogicalOperator,
    ) -> Result<LogicalOperator> {
        for pattern in patterns {
            match pattern {
                ast::Pattern::Node(node) => {
                    if node.reference.is_some() {
                        return Err(Self::erreur_ref_hors_cible());
                    }
                    if node.base_qualifier.is_some() {
                        return Err(Self::erreur_in_ecriture());
                    }
                    let variable = node
                        .variable
                        .clone()
                        .unwrap_or_else(|| format!("_anon_{}", rand_id()));
                    let properties: Vec<(String, LogicalExpression)> = node
                        .properties
                        .iter()
                        .map(|(k, v)| Ok((k.clone(), self.translate_expression(v)?)))
                        .collect::<Result<_>>()?;

                    // (correctif parseur-queue) en tete de requete (plan Empty),
                    // le CreateNode demarre sans entree — comme le chemin
                    // DataModification — au lieu d'emboiter un plan vide que le
                    // planificateur refuse.
                    let entree = std::mem::replace(&mut plan, LogicalOperator::Empty);
                    let entree = if matches!(entree, LogicalOperator::Empty) {
                        None
                    } else {
                        Some(Box::new(entree))
                    };
                    plan = LogicalOperator::CreateNode(CreateNodeOp {
                        variable,
                        labels: node.labels.clone(),
                        properties,
                        input: entree,
                    });
                }
                ast::Pattern::Path(path) => {
                    if path.source.reference.is_some() {
                        return Err(Self::erreur_source_ref());
                    }
                    if path.source.base_qualifier.is_some()
                        || path.edges.iter().any(|e| e.target.base_qualifier.is_some())
                    {
                        return Err(Self::erreur_in_ecriture());
                    }
                    // First create the source node if it has labels (new node)
                    let source_var = path
                        .source
                        .variable
                        .clone()
                        .unwrap_or_else(|| format!("_anon_{}", rand_id()));

                    // If source has labels, it's a new node to create
                    if !path.source.labels.is_empty() {
                        let source_props: Vec<(String, LogicalExpression)> = path
                            .source
                            .properties
                            .iter()
                            .map(|(k, v)| Ok((k.clone(), self.translate_expression(v)?)))
                            .collect::<Result<_>>()?;

                        // (correctif parseur-queue) meme repli que pour le motif
                        // de noeud : pas d'entree quand le plan est encore vide.
                        let entree = std::mem::replace(&mut plan, LogicalOperator::Empty);
                        let entree = if matches!(entree, LogicalOperator::Empty) {
                            None
                        } else {
                            Some(Box::new(entree))
                        };
                        plan = LogicalOperator::CreateNode(CreateNodeOp {
                            variable: source_var.clone(),
                            labels: path.source.labels.clone(),
                            properties: source_props,
                            input: entree,
                        });
                    }

                    // Create edges and target nodes
                    for (edge_idx, edge) in path.edges.iter().enumerate() {
                        // Aretes inter-bases (V1) : cible = ref(...) —
                        // l'arete vit dans la base courante, la cible est
                        // une ADRESSE, jamais resolue a l'ecriture.
                        if let Some(reference) = &edge.target.reference {
                            match edge.direction {
                                ast::EdgeDirection::Outgoing => {}
                                ast::EdgeDirection::Incoming => {
                                    return Err(Self::erreur_source_ref());
                                }
                                ast::EdgeDirection::Undirected => {
                                    return Err(Self::erreur_ref_orientation());
                                }
                            }
                            if !edge.properties.is_empty() || edge.variable.is_some() {
                                return Err(Self::erreur_ref_arete_riche());
                            }
                            if edge_idx + 1 != path.edges.len() {
                                return Err(Error::Query(QueryError::new(
                                    QueryErrorKind::Semantic,
                                    "un ref(...) termine un motif d'ecriture : rien ne \
                                     peut s'enchainer apres une cible d'une autre base",
                                )));
                            }
                            let edge_type = edge.types.first().cloned().ok_or_else(|| {
                                Error::Query(QueryError::new(
                                    QueryErrorKind::Semantic,
                                    "une arete inter-bases exige un type : \
                                         CREATE (a)-[:TYPE]->(ref(...))",
                                ))
                            })?;
                            plan = LogicalOperator::CreateExternalEdge(CreateExternalEdgeOp {
                                from_variable: source_var.clone(),
                                edge_type,
                                to_host: reference.host.clone(),
                                to_store: reference.store.clone(),
                                to_key: reference.key.clone(),
                                merge: false,
                                input: Box::new(plan),
                            });
                            continue;
                        }
                        let target_var = edge
                            .target
                            .variable
                            .clone()
                            .unwrap_or_else(|| format!("_anon_{}", rand_id()));

                        // If target has labels, create it
                        if !edge.target.labels.is_empty() {
                            let target_props: Vec<(String, LogicalExpression)> = edge
                                .target
                                .properties
                                .iter()
                                .map(|(k, v)| Ok((k.clone(), self.translate_expression(v)?)))
                                .collect::<Result<_>>()?;

                            plan = LogicalOperator::CreateNode(CreateNodeOp {
                                variable: target_var.clone(),
                                labels: edge.target.labels.clone(),
                                properties: target_props,
                                input: Some(Box::new(plan)),
                            });
                        }

                        // Create the edge
                        let edge_type = edge.types.first().cloned().unwrap_or_default();
                        let edge_var = edge.variable.clone();
                        let edge_props: Vec<(String, LogicalExpression)> = edge
                            .properties
                            .iter()
                            .map(|(k, v)| Ok((k.clone(), self.translate_expression(v)?)))
                            .collect::<Result<_>>()?;

                        // Determine direction
                        let (from_var, to_var) = match edge.direction {
                            ast::EdgeDirection::Outgoing => (source_var.clone(), target_var),
                            ast::EdgeDirection::Incoming => {
                                let tv = edge
                                    .target
                                    .variable
                                    .clone()
                                    .unwrap_or_else(|| format!("_anon_{}", rand_id()));
                                (tv, source_var.clone())
                            }
                            ast::EdgeDirection::Undirected => (source_var.clone(), target_var),
                        };

                        plan = LogicalOperator::CreateEdge(CreateEdgeOp {
                            variable: edge_var,
                            from_variable: from_var,
                            to_variable: to_var,
                            edge_type,
                            properties: edge_props,
                            input: Box::new(plan),
                        });
                    }
                }
                ast::Pattern::Quantified { .. }
                | ast::Pattern::Union(_)
                | ast::Pattern::MultisetUnion(_) => {
                    return Err(Error::Query(QueryError::new(
                        QueryErrorKind::Semantic,
                        "CREATE does not support quantified or union patterns",
                    )));
                }
            }
        }
        Ok(plan)
    }

    pub(super) fn translate_node_pattern(
        &self,
        node: &ast::NodePattern,
        input: Option<LogicalOperator>,
    ) -> Result<LogicalOperator> {
        let variable = node
            .variable
            .clone()
            .unwrap_or_else(|| format!("_anon_{}", rand_id()));

        // Use first label from colon syntax for the NodeScan optimization,
        // or first label from IS expression if it's a simple label.
        let label = if let Some(ref label_expr) = node.label_expression {
            if let ast::LabelExpression::Label(name) = label_expr {
                Some(name.clone())
            } else {
                None
            }
        } else {
            node.labels.first().cloned()
        };

        // Si `input` lie déjà la variable, le planificateur ne rejoue pas le
        // scan : le label du scan ne serait pas vérifié. Il l'est alors par un
        // filtre, avec les autres.
        let deja_liee = input
            .as_ref()
            .is_some_and(|input| visible_variables(input).contains_key(&variable));

        let mut plan = LogicalOperator::NodeScan(NodeScanOp {
            variable: variable.clone(),
            label,
            input: input.map(Box::new),
            pushdown_hint: None,
        });

        // Add hasLabel filters for additional colon-syntax labels (AND semantics).
        // First label is used in NodeScan for scan-time filtering; remaining
        // labels are checked via post-scan Filter.
        let premier_verifie = usize::from(!deja_liee).min(node.labels.len());
        if node.labels.len() > premier_verifie {
            plan = Self::add_extra_label_filters(plan, &variable, &node.labels[premier_verifie..]);
        }

        // Add label expression filter for complex IS expressions
        if let Some(ref label_expr) = node.label_expression {
            // Only add filter for non-simple expressions (simple Label already used in NodeScan)
            if !matches!(label_expr, ast::LabelExpression::Label(_)) {
                let predicate = Self::translate_label_expression(&variable, label_expr);
                plan = wrap_filter(plan, predicate);
            }
        }

        // Add filter for node pattern properties (e.g., {name: 'Alix'})
        if !node.properties.is_empty() {
            let predicate = self.build_property_predicate(&variable, &node.properties)?;
            plan = wrap_filter(plan, predicate);
        }

        // Add element pattern WHERE clause (e.g., (n WHERE n.age > 30))
        if let Some(ref where_expr) = node.where_clause {
            let predicate = self.translate_expression(where_expr)?;
            plan = wrap_filter(plan, predicate);
        }

        // Aretes inter-bases (V2) : motif qualifie par base.
        if let Some(ref slug) = node.base_qualifier {
            plan = wrap_filter(plan, Self::predicat_base(&variable, slug));
        }

        Ok(plan)
    }

    /// Translates a label expression into a filter predicate using hasLabel() calls.
    pub(super) fn translate_label_expression(
        variable: &str,
        expr: &ast::LabelExpression,
    ) -> LogicalExpression {
        match expr {
            ast::LabelExpression::Label(name) => LogicalExpression::FunctionCall {
                name: "hasLabel".into(),
                args: vec![
                    LogicalExpression::Variable(variable.to_string()),
                    LogicalExpression::Literal(Value::from(name.as_str())),
                ],
                distinct: false,
            },
            ast::LabelExpression::Conjunction(operands) => {
                let mut iter = operands.iter();
                let first = Self::translate_label_expression(
                    variable,
                    iter.next().expect("conjunction has at least one operand"),
                );
                iter.fold(first, |acc, op| LogicalExpression::Binary {
                    left: Box::new(acc),
                    op: BinaryOp::And,
                    right: Box::new(Self::translate_label_expression(variable, op)),
                })
            }
            ast::LabelExpression::Disjunction(operands) => {
                let mut iter = operands.iter();
                let first = Self::translate_label_expression(
                    variable,
                    iter.next().expect("disjunction has at least one operand"),
                );
                iter.fold(first, |acc, op| LogicalExpression::Binary {
                    left: Box::new(acc),
                    op: BinaryOp::Or,
                    right: Box::new(Self::translate_label_expression(variable, op)),
                })
            }
            ast::LabelExpression::Negation(inner) => LogicalExpression::Unary {
                op: UnaryOp::Not,
                operand: Box::new(Self::translate_label_expression(variable, inner)),
            },
            ast::LabelExpression::Wildcard => LogicalExpression::Literal(Value::Bool(true)),
        }
    }

    /// Wraps a plan with AND-combined `hasLabel` filters for extra labels beyond the
    /// first (which is already used in `NodeScan` for scan-time filtering).
    fn add_extra_label_filters(
        mut plan: LogicalOperator,
        variable: &str,
        extra_labels: &[String],
    ) -> LogicalOperator {
        for label in extra_labels {
            plan = wrap_filter(
                plan,
                LogicalExpression::FunctionCall {
                    name: "hasLabel".into(),
                    args: vec![
                        LogicalExpression::Variable(variable.to_string()),
                        LogicalExpression::Literal(Value::String(label.clone().into())),
                    ],
                    distinct: false,
                },
            );
        }
        plan
    }

    /// Builds a predicate expression for property filters like {name: 'Alix', age: 30}.
    pub(super) fn build_property_predicate(
        &self,
        variable: &str,
        properties: &[(String, ast::Expression)],
    ) -> Result<LogicalExpression> {
        let predicates = properties
            .iter()
            .map(|(prop_name, prop_value)| {
                let left = LogicalExpression::Property {
                    variable: variable.to_string(),
                    property: prop_name.clone(),
                };
                let right = self.translate_expression(prop_value)?;
                Ok(LogicalExpression::Binary {
                    left: Box::new(left),
                    op: BinaryOp::Eq,
                    right: Box::new(right),
                })
            })
            .collect::<Result<Vec<_>>>()?;

        combine_with_and(predicates)
    }

    /// Traduit un chemin `(n0)-[e1]-(n1)…-[ek]-(nk)`.
    ///
    /// Une variable de nœud que `input` lie déjà, ou que le chemin a déjà
    /// posée, n'est jamais liée une seconde fois :
    ///
    /// - le chemin REPART du premier nœud déjà lié à un nœud (`anchor_index`) :
    ///   les arêtes qui le suivent sont parcourues dans leur sens, celles qui
    ///   le précèdent à rebours. `MATCH (c2)-[:T]->(f)` avec `f` lié est une
    ///   expansion depuis `f`, pas un balayage de `c2` suivi d'une jointure ;
    /// - tout AUTRE nœud déjà lié est une fermeture : l'expansion arrive sur
    ///   une variable interne, qu'un filtre égale à la variable liée
    ///   (`expand_edge`). C'est aussi ce qui ferme un cycle `(a)-->(b)-->(a)`.
    pub(super) fn translate_path_pattern_with_alias(
        &self,
        path: &ast::PathPattern,
        input: Option<LogicalOperator>,
        path_alias: Option<&str>,
        path_mode: PathMode,
    ) -> Result<LogicalOperator> {
        let bound = input.as_ref().map(visible_variables).unwrap_or_default();
        let anchor = Self::anchor_index(path, &bound, path_alias.is_some()).unwrap_or(0);

        // Le nœud de rang i : le départ, puis la cible de chaque arête.
        let node_at = |rang: usize| -> &ast::NodePattern {
            if rang == 0 {
                &path.source
            } else {
                &path.edges[rang - 1].target
            }
        };

        // Nœud d'ancrage. Au rang 0, c'est le `NodeScan` habituel ; si `input`
        // lie déjà sa variable, le planificateur réutilise la colonne sans
        // rejouer le scan, et TOUTES ses contraintes passent en filtres.
        // Ailleurs, la variable existe déjà : seules ses contraintes s'ajoutent.
        let (mut plan, anchor_variable) = if anchor == 0 {
            let source_var = path
                .source
                .variable
                .clone()
                .unwrap_or_else(|| format!("_anon_{}", rand_id()));
            let deja_liee = bound.contains_key(&source_var);
            let plan = LogicalOperator::NodeScan(NodeScanOp {
                variable: source_var.clone(),
                label: path.source.labels.first().cloned(),
                input: input.map(Box::new),
                pushdown_hint: None,
            });
            let plan = self.node_constraints(&path.source, &source_var, plan, !deja_liee)?;
            (plan, source_var)
        } else {
            let node = node_at(anchor);
            let variable = node.variable.clone().expect("un nœud d'ancrage est nommé");
            let input = input.expect("un ancrage hors du départ vient de `input`");
            (
                self.node_constraints(node, &variable, input, false)?,
                variable,
            )
        };

        // Variables de nœud déjà posées — par `input` ou par ce motif. Une
        // arête qui retombe sur l'une d'elles est une fermeture.
        let mut seen: HashSet<String> = bound.keys().cloned().collect();
        seen.insert(anchor_variable.clone());

        // En avant depuis l'ancre.
        let mut current = anchor_variable.clone();
        for (idx, edge) in path.edges.iter().enumerate().skip(anchor) {
            let (next_plan, next) = self.expand_edge(
                edge,
                &edge.target,
                EdgeWalk {
                    reversed: false,
                    path_alias: (idx + 1 == path.edges.len())
                        .then_some(path_alias)
                        .flatten(),
                    path_mode,
                },
                current,
                plan,
                &mut seen,
            )?;
            plan = next_plan;
            current = next;
        }

        // À rebours depuis l'ancre, jusqu'au départ.
        let mut current = anchor_variable;
        for rang in (0..anchor).rev() {
            let (next_plan, next) = self.expand_edge(
                &path.edges[rang],
                node_at(rang),
                EdgeWalk {
                    reversed: true,
                    path_alias: None,
                    path_mode,
                },
                current,
                plan,
                &mut seen,
            )?;
            plan = next_plan;
            current = next;
        }

        Ok(plan)
    }

    /// Rang du nœud du chemin d'où repartir : le premier nœud dont la variable
    /// est déjà liée À UN NŒUD (0 = le départ, `i` = la cible de `edges[i - 1]`).
    ///
    /// Repartir d'un nœud qui n'est pas le départ parcourt à REBOURS les arêtes
    /// qui le précèdent : impossible pour une arête de longueur variable ou
    /// optionnelle (`->?`), ou pour un chemin nommé (`forward_only`), dont
    /// l'ordre compte.
    fn anchor_index(
        path: &ast::PathPattern,
        bound: &HashMap<String, VariableKind>,
        forward_only: bool,
    ) -> Option<usize> {
        let est_noeud_lie = |node: &ast::NodePattern| {
            node.variable
                .as_ref()
                .is_some_and(|v| bound.get(v) == Some(&VariableKind::Node))
        };
        if est_noeud_lie(&path.source) {
            return Some(0);
        }
        if forward_only {
            return None;
        }
        let rang = path
            .edges
            .iter()
            .position(|edge| est_noeud_lie(&edge.target))?;
        path.edges[..=rang]
            .iter()
            .all(|edge| edge.min_hops.is_none() && edge.max_hops.is_none() && !edge.questioned)
            .then_some(rang + 1)
    }

    /// Contraintes qu'un motif de nœud écrit (labels, propriétés, `WHERE`
    /// d'élément, base), posées en filtres sur une variable qui existe dans
    /// `plan`. `skip_first_label` : le premier label de la forme `:Label` est
    /// déjà celui du `NodeScan`.
    fn node_constraints(
        &self,
        node: &ast::NodePattern,
        variable: &str,
        mut plan: LogicalOperator,
        skip_first_label: bool,
    ) -> Result<LogicalOperator> {
        if let Some(ref label_expr) = node.label_expression {
            let predicate = Self::translate_label_expression(variable, label_expr);
            plan = wrap_filter(plan, predicate);
        } else {
            let premier = usize::from(skip_first_label).min(node.labels.len());
            plan = Self::add_extra_label_filters(plan, variable, &node.labels[premier..]);
        }
        if !node.properties.is_empty() {
            let predicate = self.build_property_predicate(variable, &node.properties)?;
            plan = wrap_filter(plan, predicate);
        }
        if let Some(ref where_expr) = node.where_clause {
            let predicate = self.translate_expression(where_expr)?;
            plan = wrap_filter(plan, predicate);
        }
        if let Some(ref slug) = node.base_qualifier {
            plan = wrap_filter(plan, Self::predicat_base(variable, slug));
        }
        Ok(plan)
    }

    /// Parcourt une arête depuis `from_variable` jusqu'au nœud `to`.
    ///
    /// `reversed` : l'arête est parcourue de sa cible vers sa source (le chemin
    /// repart d'un nœud lié qui n'est pas son départ) ; sa direction est
    /// inversée, ses contraintes sont les mêmes.
    ///
    /// Si la variable de `to` est déjà posée (`seen`), l'expansion arrive sur
    /// une variable interne et un filtre l'égale à la variable posée : l'arête
    /// est VÉRIFIÉE entre deux nœuds connus. Rend le plan et le nom de la
    /// variable du nœud d'arrivée.
    fn expand_edge(
        &self,
        edge: &ast::EdgePattern,
        to: &ast::NodePattern,
        walk: EdgeWalk<'_>,
        from_variable: String,
        plan: LogicalOperator,
        seen: &mut HashSet<String>,
    ) -> Result<(LogicalOperator, String)> {
        let mut plan = plan;
        let closes_on = to.variable.as_ref().filter(|v| seen.contains(*v)).cloned();
        let target_var = match (&closes_on, &to.variable) {
            (None, Some(variable)) => variable.clone(),
            _ => format!("_anon_{}", rand_id()),
        };

        // Une arête anonyme qui porte des propriétés reçoit une variable :
        // sans elle, ses propriétés n'étaient pas filtrées du tout.
        let single_hop = edge.min_hops.is_none() && edge.max_hops.is_none();
        let edge_var = edge.variable.clone().or_else(|| {
            (single_hop && !edge.properties.is_empty()).then(|| format!("_anon_edge_{}", rand_id()))
        });
        let edge_types = edge.types.clone();

        let direction = match (edge.direction, walk.reversed) {
            (ast::EdgeDirection::Outgoing, false) | (ast::EdgeDirection::Incoming, true) => {
                ExpandDirection::Outgoing
            }
            (ast::EdgeDirection::Incoming, false) | (ast::EdgeDirection::Outgoing, true) => {
                ExpandDirection::Incoming
            }
            (ast::EdgeDirection::Undirected, _) => ExpandDirection::Both,
        };

        let edge_var_for_filter = edge_var.clone();
        let expand_path_alias = walk.path_alias.map(String::from);

        let min_hops = edge.min_hops.unwrap_or(1);
        let max_hops = if single_hop { Some(1) } else { edge.max_hops };
        let is_variable_length = min_hops != 1 || max_hops.is_none() || max_hops != Some(1);

        // For variable-length edges with a named edge variable, auto-generate
        // a path alias if none exists, so path detail columns are available
        // for horizontal aggregation (GE09).
        let expand_path_alias =
            if is_variable_length && edge_var_for_filter.is_some() && expand_path_alias.is_none() {
                Some(format!("_auto_path_{}", rand_id()))
            } else {
                expand_path_alias
            };

        // Track group-list variables for horizontal aggregation detection
        if is_variable_length
            && let (Some(ev), Some(pa)) = (&edge_var_for_filter, &expand_path_alias)
        {
            self.group_list_variables
                .borrow_mut()
                .insert(ev.clone(), pa.clone());
        }

        // For questioned edges (->?), save the plan BEFORE the expand so the
        // left side of the LeftJoin contains only the required patterns.
        let pre_expand_plan = if edge.questioned {
            Some(plan.clone())
        } else {
            None
        };

        plan = LogicalOperator::Expand(ExpandOp {
            from_variable,
            to_variable: target_var.clone(),
            edge_variable: edge_var,
            direction,
            edge_types,
            min_hops,
            max_hops,
            input: Box::new(plan),
            path_alias: expand_path_alias,
            path_mode: walk.path_mode,
        });

        if let Some(ref bound_variable) = closes_on {
            plan = wrap_filter(
                plan,
                LogicalExpression::Binary {
                    left: Box::new(LogicalExpression::Variable(target_var.clone())),
                    op: BinaryOp::Eq,
                    right: Box::new(LogicalExpression::Variable(bound_variable.clone())),
                },
            );
        }

        // Add filter for edge properties
        if !edge.properties.is_empty()
            && let Some(ref ev) = edge_var_for_filter
        {
            let predicate = self.build_property_predicate(ev, &edge.properties)?;
            plan = wrap_filter(plan, predicate);
        }

        // Add element WHERE clause for edge
        if let Some(ref where_expr) = edge.where_clause {
            let predicate = self.translate_expression(where_expr)?;
            plan = wrap_filter(plan, predicate);
        }

        // Contraintes du nœud d'arrivée (TOUS ses labels, ses propriétés, son
        // `WHERE`, sa base).
        plan = self.node_constraints(to, &target_var, plan, false)?;

        // Questioned edge: wrap expand + all filters in a LeftJoin so the
        // edge is optional (rows without a matching edge are preserved with nulls).
        if let Some(left) = pre_expand_plan {
            plan = LogicalOperator::LeftJoin(LeftJoinOp {
                left: Box::new(left),
                right: Box::new(plan),
                condition: None,
            });
        }

        let arrival = closes_on.unwrap_or(target_var);
        seen.insert(arrival.clone());
        Ok((plan, arrival))
    }
}
