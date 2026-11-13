//! Mutation operators for creating and deleting graph elements.
//!
//! These operators modify the graph structure:
//! - `CreateNodeOperator`: Creates new nodes
//! - `CreateEdgeOperator`: Creates new edges
//! - `DeleteNodeOperator`: Deletes nodes
//! - `DeleteEdgeOperator`: Deletes edges

use std::sync::Arc;

use obrain_common::types::{
    EdgeId, EpochId, LogicalType, NodeId, PropertyKey, TransactionId, Value,
};

use super::{Operator, OperatorError, OperatorResult, SharedWriteTracker};
use crate::execution::chunk::DataChunkBuilder;
use crate::graph::{GraphStore, GraphStoreMut};

/// Trait for validating schema constraints during mutation operations.
///
/// Implementors check type definitions, NOT NULL, and UNIQUE constraints
/// before data is written to the store.
pub trait ConstraintValidator: Send + Sync {
    /// Validates a single property value for a node with the given labels.
    ///
    /// Checks type compatibility and NOT NULL constraints.
    fn validate_node_property(
        &self,
        labels: &[String],
        key: &str,
        value: &Value,
    ) -> Result<(), OperatorError>;

    /// Validates that all required properties are present after creating a node.
    ///
    /// Checks NOT NULL constraints for properties that were not explicitly set.
    fn validate_node_complete(
        &self,
        labels: &[String],
        properties: &[(String, Value)],
    ) -> Result<(), OperatorError>;

    /// Checks UNIQUE constraints over the full property tuple of a node.
    ///
    /// UNIQUE and PRIMARY KEY constraints bind the WHOLE property list: a
    /// composite constraint on (a, b) must reject a node whose (a, b) tuple
    /// already exists, never a node sharing a single column. The full resolved
    /// property set is passed so the check can assemble the tuple.
    ///
    /// `properties` must be the values EFFECTIVELY WRITTEN, with at most one
    /// entry per key: a caller that merges several sources (a `MERGE` pattern
    /// overridden by `ON CREATE SET`) resolves the overrides before calling.
    ///
    /// `exclude` is the node being UPDATED, if any: a node is never a
    /// duplicate of itself, so rewriting its own tuple must not collide.
    /// `None` when the node is being created.
    fn check_unique_node_properties(
        &self,
        labels: &[String],
        properties: &[(String, Value)],
        exclude: Option<NodeId>,
    ) -> Result<(), OperatorError>;

    /// Properties bound by a UNIQUE or PRIMARY KEY constraint of any of
    /// `labels` (each once).
    ///
    /// This is what lets an UPDATE be checked at the cost of the constraint,
    /// not of the node: an update that touches none of these properties
    /// cannot create a duplicate and is not checked at all; one that does is
    /// checked against these properties only. A validator without uniqueness
    /// constraints keeps the default (nothing to check).
    fn unique_constrained_properties(&self, _labels: &[String]) -> Vec<String> {
        Vec::new()
    }

    /// Validates a single property value for an edge of the given type.
    fn validate_edge_property(
        &self,
        edge_type: &str,
        key: &str,
        value: &Value,
    ) -> Result<(), OperatorError>;

    /// Validates that all required properties are present after creating an edge.
    fn validate_edge_complete(
        &self,
        edge_type: &str,
        properties: &[(String, Value)],
    ) -> Result<(), OperatorError>;

    /// Validates that the node labels are allowed by the bound graph type.
    fn validate_node_labels_allowed(&self, labels: &[String]) -> Result<(), OperatorError> {
        let _ = labels;
        Ok(())
    }

    /// Validates that the edge type is allowed by the bound graph type.
    fn validate_edge_type_allowed(&self, edge_type: &str) -> Result<(), OperatorError> {
        let _ = edge_type;
        Ok(())
    }

    /// Validates that edge endpoints have the correct node type labels.
    fn validate_edge_endpoints(
        &self,
        edge_type: &str,
        source_labels: &[String],
        target_labels: &[String],
    ) -> Result<(), OperatorError> {
        let _ = (edge_type, source_labels, target_labels);
        Ok(())
    }

    /// Injects default values for properties that are defined in a type but
    /// not explicitly provided.
    fn inject_defaults(&self, labels: &[String], properties: &mut Vec<(String, Value)>) {
        let _ = (labels, properties);
    }
}
/// Verrou tenu le temps d'un « vérifier l'unicité puis écrire ».
///
/// Le contrôle d'unicité lit l'index, puis l'écriture le modifie : sans
/// exclusion mutuelle, deux écrivains du même tuple lisent tous deux « libre »
/// et écrivent tous deux (167 manches sur 400 mesurées sur deux `INSERT`
/// concurrents). Le verrou est celui que `MERGE` tient déjà pour son composé
/// chercher-puis-créer (`GraphStoreMut::merge_guard`, un mutex par store) :
/// INSERT, SET, ajout de label et MERGE s'excluent donc mutuellement.
///
/// Il n'est pris QUE par une écriture soumise à une contrainte d'unicité —
/// une écriture hors contrainte ne le voit jamais et reste parallèle.
pub(crate) struct VerrouUnicite(Option<Arc<std::sync::Mutex<()>>>);

impl VerrouUnicite {
    /// Le verrou du store, s'il en a un.
    pub(crate) fn du_store(store: &dyn GraphStoreMut) -> Self {
        Self(store.merge_guard())
    }

    /// Prend le verrou ; relâché à la fin de vie de la garde rendue.
    pub(crate) fn tenir(&self) -> Option<std::sync::MutexGuard<'_, ()>> {
        self.0
            .as_ref()
            .map(|m| m.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
    }
}

/// Ce qu'une mise à jour fait aux propriétés d'un nœud EXISTANT.
pub(crate) struct MiseAJourNoeud<'a> {
    /// Le nœud mis à jour (exclu de la recherche de doublon : il n'est pas
    /// son propre doublon).
    pub(crate) noeud: NodeId,
    /// Les labels dont les contraintes s'appliquent.
    pub(crate) labels: &'a [String],
    /// Propriétés écrites, dans l'ordre ; une valeur `Null` RETIRE la clé.
    pub(crate) ecrites: &'a [(String, Value)],
    /// Vrai si toute propriété non écrite est retirée (`SET n = {…}`).
    pub(crate) remplace_tout: bool,
    /// Vrai si le nœud ENTRE sous ces labels (ajout de label) : ses
    /// propriétés, inchangées, deviennent soumises aux contraintes.
    pub(crate) entre_sous_labels: bool,
}

/// Propriétés à contrôler pour cette mise à jour, ou `None` si elle ne peut
/// pas créer de doublon (aucune contrainte d'unicité sur ces labels, ou
/// aucune colonne contrainte touchée).
///
/// Séparé du contrôle lui-même pour que l'appelant sache, avant de lire quoi
/// que ce soit sur le nœud, si un contrôle a lieu.
pub(crate) fn colonnes_uniques_touchees(
    validator: &dyn ConstraintValidator,
    maj: &MiseAJourNoeud<'_>,
) -> Option<Vec<String>> {
    let contraintes = validator.unique_constrained_properties(maj.labels);
    if contraintes.is_empty() {
        return None;
    }
    let touchee = maj.entre_sous_labels
        || maj.remplace_tout
        || maj
            .ecrites
            .iter()
            .any(|(k, _)| contraintes.iter().any(|c| c == k));
    touchee.then_some(contraintes)
}

/// Contrôle l'unicité de l'état qu'AURA le nœud après la mise à jour.
///
/// Un `SET` d'une seule colonne d'un `UNIQUE (a, b)` doit être jugé sur le
/// tuple COMPLET : les colonnes contraintes non écrites sont lues sur le nœud.
/// Seules les colonnes contraintes sont lues — jamais le nœud entier.
pub(crate) fn controler_unicite_apres_mise_a_jour(
    validator: &dyn ConstraintValidator,
    store: &dyn GraphStore,
    maj: &MiseAJourNoeud<'_>,
    colonnes: Vec<String>,
) -> Result<(), OperatorError> {
    let etat: Vec<(String, Value)> = colonnes
        .into_iter()
        .filter_map(|colonne| {
            // La DERNIÈRE écriture d'une clé est celle qui reste.
            let valeur = match maj.ecrites.iter().rev().find(|(k, _)| *k == colonne) {
                Some((_, v)) => Some(v.clone()),
                None if maj.remplace_tout => None,
                None => store.get_node_property(maj.noeud, &PropertyKey::new(colonne.as_str())),
            }?;
            (!matches!(valeur, Value::Null)).then_some((colonne, valeur))
        })
        .collect();
    validator.check_unique_node_properties(maj.labels, &etat, Some(maj.noeud))
}

/// Labels portés par un nœud, pour y chercher les contraintes applicables.
pub(crate) fn labels_du_noeud(store: &dyn GraphStore, noeud: NodeId) -> Vec<String> {
    store
        .get_node_labels(noeud)
        .map(|ls| ls.iter().map(|l| l.as_str().to_string()).collect())
        .unwrap_or_default()
}

/// Operator that creates new nodes.
///
/// For each input row, creates a new node with the specified labels
/// and properties, then outputs the row with the new node.
pub struct CreateNodeOperator {
    /// The graph store to modify.
    store: Arc<dyn GraphStoreMut>,
    /// Input operator.
    input: Option<Box<dyn Operator>>,
    /// Labels for the new nodes.
    labels: Vec<String>,
    /// Properties to set (name -> column index or constant value).
    properties: Vec<(String, PropertySource)>,
    /// Output schema.
    output_schema: Vec<LogicalType>,
    /// Column index for the created node variable.
    output_column: usize,
    /// Whether this operator has been executed (for no-input case).
    executed: bool,
    /// Epoch for MVCC versioning.
    viewing_epoch: Option<EpochId>,
    /// Transaction ID for MVCC versioning.
    transaction_id: Option<TransactionId>,
    /// Optional constraint validator for schema enforcement.
    validator: Option<Arc<dyn ConstraintValidator>>,
    /// Optional write tracker for conflict detection.
    write_tracker: Option<SharedWriteTracker>,
}

/// Source for a property value.
#[derive(Clone)]
pub enum PropertySource {
    /// Get value from an input column.
    Column(usize),
    /// Use a constant value.
    Constant(Value),
    /// Access a named property from a map/node/edge in an input column.
    PropertyAccess {
        /// The column containing the map, node ID, or edge ID.
        column: usize,
        /// The property name to extract.
        property: String,
    },
    /// (2026-08-30, vague 4a) Expression composée évaluée PAR LIGNE, sur
    /// l'état d'AVANT l'écriture : `SET n.x = n.x + 1` lit le `n.x` du
    /// moment de la résolution — les sources d'une ligne sont TOUTES
    /// résolues avant sa première écriture, aux sites d'appel de `resolve`.
    ///
    /// Avant cette variante, toute expression non repliable à la
    /// planification tombait en erreur `Internal` : `SET n.x = n.y` passait
    /// (accès nu) mais `SET n.x = n.y + 1` était refusé — une frontière
    /// indevinable depuis le langage (gql_13, set_par_expression).
    Expression(Arc<super::filter::ExpressionPredicate>),
}

impl std::fmt::Debug for PropertySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PropertySource::Column(c) => f.debug_tuple("Column").field(c).finish(),
            PropertySource::Constant(v) => f.debug_tuple("Constant").field(v).finish(),
            PropertySource::PropertyAccess { column, property } => f
                .debug_struct("PropertyAccess")
                .field("column", column)
                .field("property", property)
                .finish(),
            // `ExpressionPredicate` porte un verrou d'erreur, pas de Debug ;
            // le plan logique, lui, sait imprimer l'expression d'origine.
            PropertySource::Expression(_) => f.write_str("Expression(..)"),
        }
    }
}

impl PropertySource {
    /// Resolves a property value from a data chunk row.
    ///
    /// `Err` ne signale JAMAIS une absence de valeur (elle vaut
    /// `Value::Null`) : seule une faute d'évaluation d'expression —
    /// division par zéro, débordement i64, l'arithmétique gardée de la
    /// vague 1 — refuse la requête au lieu de se déguiser en NULL écrit.
    pub fn resolve(
        &self,
        chunk: &crate::execution::chunk::DataChunk,
        row: usize,
        store: &dyn GraphStore,
    ) -> Result<Value, OperatorError> {
        Ok(match self {
            PropertySource::Column(col_idx) => chunk
                .column(*col_idx)
                .and_then(|c| c.get_value(row))
                .unwrap_or(Value::Null),
            PropertySource::Constant(v) => v.clone(),
            PropertySource::Expression(predicate) => {
                let value = predicate.eval_at(chunk, row);
                if let Some(e) = predicate.take_eval_error() {
                    return Err(OperatorError::Execution(e));
                }
                value.unwrap_or(Value::Null)
            }
            PropertySource::PropertyAccess { column, property } => {
                let Some(col) = chunk.column(*column) else {
                    return Ok(Value::Null);
                };
                // Try node ID first, then edge ID, then map value
                if let Some(node_id) = col.get_node_id(row) {
                    store
                        .get_node(node_id)
                        .and_then(|node| node.get_property(property).cloned())
                        .unwrap_or(Value::Null)
                } else if let Some(edge_id) = col.get_edge_id(row) {
                    store
                        .get_edge(edge_id)
                        .and_then(|edge| edge.get_property(property).cloned())
                        .unwrap_or(Value::Null)
                } else if let Some(Value::Map(map)) = col.get_value(row) {
                    let key = PropertyKey::new(property);
                    map.get(&key).cloned().unwrap_or(Value::Null)
                } else {
                    Value::Null
                }
            }
        })
    }
}

impl CreateNodeOperator {
    /// Creates a new node creation operator.
    ///
    /// # Arguments
    /// * `store` - The graph store to modify.
    /// * `input` - Optional input operator (None for standalone CREATE).
    /// * `labels` - Labels to assign to created nodes.
    /// * `properties` - Properties to set on created nodes.
    /// * `output_schema` - Schema of the output.
    /// * `output_column` - Column index where the created node ID goes.
    pub fn new(
        store: Arc<dyn GraphStoreMut>,
        input: Option<Box<dyn Operator>>,
        labels: Vec<String>,
        properties: Vec<(String, PropertySource)>,
        output_schema: Vec<LogicalType>,
        output_column: usize,
    ) -> Self {
        Self {
            store,
            input,
            labels,
            properties,
            output_schema,
            output_column,
            executed: false,
            viewing_epoch: None,
            transaction_id: None,
            validator: None,
            write_tracker: None,
        }
    }

    /// Sets the transaction context for MVCC versioning.
    pub fn with_transaction_context(
        mut self,
        epoch: EpochId,
        transaction_id: Option<TransactionId>,
    ) -> Self {
        self.viewing_epoch = Some(epoch);
        self.transaction_id = transaction_id;
        self
    }

    /// Sets the constraint validator for schema enforcement.
    pub fn with_validator(mut self, validator: Arc<dyn ConstraintValidator>) -> Self {
        self.validator = Some(validator);
        self
    }

    /// Sets the write tracker for conflict detection.
    pub fn with_write_tracker(mut self, tracker: SharedWriteTracker) -> Self {
        self.write_tracker = Some(tracker);
        self
    }
}

impl CreateNodeOperator {
    /// Validates and sets properties on a newly created node.
    fn validate_and_set_properties(
        &self,
        node_id: NodeId,
        resolved_props: &mut Vec<(String, Value)>,
    ) -> Result<(), OperatorError> {
        // Une propriete declaree nulle a la creation — INSERT (:T {x: null})
        // ou valeur resolue a NULL — n'est PAS materialisee : « x: null »
        // declare que x n'a pas de valeur, donc le noeud a la meme forme que
        // si la cle n'avait jamais ete ecrite (gql_13, groupe 7 : la forme ne
        // memorise pas la route d'ecriture). Le filtre vient AVANT
        // inject_defaults : une cle fournie nulle est traitee comme non
        // fournie. Seul le semis direct (API create_node_with_props) sait
        // encore fabriquer l'etat « cle presente, valeur nulle ».
        resolved_props.retain(|(_, value)| !matches!(value, Value::Null));

        // Phase 0: Validate that node labels are allowed by the bound graph type
        if let Some(ref validator) = self.validator {
            validator.validate_node_labels_allowed(&self.labels)?;
        }

        // Phase 0.5: Inject defaults for properties not explicitly provided
        if let Some(ref validator) = self.validator {
            validator.inject_defaults(&self.labels, resolved_props);
        }

        // ── VÉRIFIER-PUIS-ÉCRIRE EST ATOMIQUE SOUS CONTRAINTE D'UNICITÉ ──
        // Le contrôle (Phase 1) lit l'index, l'écriture (Phase 3) le modifie.
        // Deux INSERT concurrents du même tuple lisaient tous deux « libre »
        // et écrivaient tous deux. Le verrou couvre donc les deux phases, et
        // il n'est pris que si ces labels portent une contrainte d'unicité.
        let verrou = VerrouUnicite::du_store(self.store.as_ref());
        let _unicite_tenue = self
            .validator
            .as_ref()
            .is_some_and(|v| !v.unique_constrained_properties(&self.labels).is_empty())
            .then(|| verrou.tenir())
            .flatten();

        // Phase 1: Validate each property value
        if let Some(ref validator) = self.validator {
            for (name, value) in resolved_props.iter() {
                validator.validate_node_property(&self.labels, name, value)?;
            }
            // Uniqueness is checked ONCE over the whole tuple (a composite
            // constraint binds several properties at once).
            validator.check_unique_node_properties(&self.labels, resolved_props, None)?;
            // Phase 2: Validate completeness (NOT NULL checks for missing required properties)
            validator.validate_node_complete(&self.labels, resolved_props)?;
        }

        // Phase 3: Write properties to the store
        if let Some(tid) = self.transaction_id {
            for (name, value) in resolved_props.iter() {
                self.store
                    .set_node_property_versioned(node_id, name, value.clone(), tid);
            }
        } else {
            for (name, value) in resolved_props.iter() {
                self.store.set_node_property(node_id, name, value.clone());
            }
        }
        Ok(())
    }
}

impl Operator for CreateNodeOperator {
    fn next(&mut self) -> OperatorResult {
        // Get transaction context for versioned creation
        let epoch = self
            .viewing_epoch
            .unwrap_or_else(|| self.store.current_epoch());
        let tx = self.transaction_id.unwrap_or(TransactionId::SYSTEM);

        if let Some(ref mut input) = self.input {
            // For each input row, create a node
            if let Some(chunk) = input.next()? {
                let mut builder =
                    DataChunkBuilder::with_capacity(&self.output_schema, chunk.row_count());

                for row in chunk.selected_indices() {
                    // Resolve all property values first (before creating node)
                    let mut resolved_props: Vec<(String, Value)> = self
                        .properties
                        .iter()
                        .map(|(name, source)| {
                            let value = source.resolve(
                                &chunk,
                                row,
                                self.store.as_ref() as &dyn GraphStore,
                            )?;
                            Ok((name.clone(), value))
                        })
                        .collect::<Result<_, OperatorError>>()?;

                    // Create the node with MVCC versioning
                    let label_refs: Vec<&str> = self.labels.iter().map(String::as_str).collect();
                    // ── LA CAPACITÉ D'ÉTIQUETTES SE DEMANDE AVANT DE CRÉER ──
                    // `create_node` est infaillible par contrat : dépasser les 64
                    // étiquettes distinctes d'une base faisait donc ABANDONNER le
                    // processus (`.expect("label registry overflow")`). Une
                    // requête — ou un agent avec droit d'écriture — pouvait tuer
                    // le démon. On refuse ici, proprement, avec un message qui dit
                    // la limite et la sortie.
                    self.store
                        .valider_etiquettes(&label_refs)
                        .map_err(OperatorError::Execution)?;
                    let node_id = self.store.create_node_versioned(&label_refs, epoch, tx);

                    // Record write for conflict detection
                    if let (Some(tracker), Some(tid)) = (&self.write_tracker, self.transaction_id) {
                        tracker.record_node_write(tid, node_id)?;
                    }

                    // Validate and set properties
                    self.validate_and_set_properties(node_id, &mut resolved_props)?;

                    // Copy input columns to output
                    for col_idx in 0..chunk.column_count() {
                        if col_idx < self.output_column
                            && let (Some(src), Some(dst)) =
                                (chunk.column(col_idx), builder.column_mut(col_idx))
                        {
                            if let Some(val) = src.get_value(row) {
                                dst.push_value(val);
                            } else {
                                dst.push_value(Value::Null);
                            }
                        }
                    }

                    // Add the new node ID
                    if let Some(dst) = builder.column_mut(self.output_column) {
                        dst.push_value(Value::Int64(node_id.0 as i64));
                    }

                    builder.advance_row();
                }

                return Ok(Some(builder.finish()));
            }
            Ok(None)
        } else {
            // No input - create a single node
            if self.executed {
                return Ok(None);
            }
            self.executed = true;

            // Resolve constant properties
            let mut resolved_props: Vec<(String, Value)> = self
                .properties
                .iter()
                .filter_map(|(name, source)| {
                    if let PropertySource::Constant(value) = source {
                        Some((name.clone(), value.clone()))
                    } else {
                        None
                    }
                })
                .collect();

            // Create the node with MVCC versioning
            let label_refs: Vec<&str> = self.labels.iter().map(String::as_str).collect();
            self.store
                .valider_etiquettes(&label_refs)
                .map_err(OperatorError::Execution)?;
            let node_id = self.store.create_node_versioned(&label_refs, epoch, tx);

            // Record write for conflict detection
            if let (Some(tracker), Some(tid)) = (&self.write_tracker, self.transaction_id) {
                tracker.record_node_write(tid, node_id)?;
            }

            // Validate and set properties
            self.validate_and_set_properties(node_id, &mut resolved_props)?;

            // Build output chunk with just the node ID
            let mut builder = DataChunkBuilder::with_capacity(&self.output_schema, 1);
            if let Some(dst) = builder.column_mut(self.output_column) {
                dst.push_value(Value::Int64(node_id.0 as i64));
            }
            builder.advance_row();

            Ok(Some(builder.finish()))
        }
    }

    fn reset(&mut self) {
        if let Some(ref mut input) = self.input {
            input.reset();
        }
        self.executed = false;
    }

    fn name(&self) -> &'static str {
        "CreateNode"
    }
}

/// Operator that creates new edges.
pub struct CreateEdgeOperator {
    /// The graph store to modify.
    store: Arc<dyn GraphStoreMut>,
    /// Input operator.
    input: Box<dyn Operator>,
    /// Column index for the source node.
    from_column: usize,
    /// Column index for the target node.
    to_column: usize,
    /// Edge type.
    edge_type: String,
    /// Properties to set.
    properties: Vec<(String, PropertySource)>,
    /// Output schema.
    output_schema: Vec<LogicalType>,
    /// Column index for the created edge variable (if any).
    output_column: Option<usize>,
    /// Epoch for MVCC versioning.
    viewing_epoch: Option<EpochId>,
    /// Transaction ID for MVCC versioning.
    transaction_id: Option<TransactionId>,
    /// Optional constraint validator for schema enforcement.
    validator: Option<Arc<dyn ConstraintValidator>>,
    /// Optional write tracker for conflict detection.
    write_tracker: Option<SharedWriteTracker>,
}

impl CreateEdgeOperator {
    /// Creates a new edge creation operator.
    ///
    /// Use builder methods to set additional options:
    /// - [`with_properties`](Self::with_properties) - set edge properties
    /// - [`with_output_column`](Self::with_output_column) - output the created edge ID
    /// - [`with_transaction_context`](Self::with_transaction_context) - set transaction context
    pub fn new(
        store: Arc<dyn GraphStoreMut>,
        input: Box<dyn Operator>,
        from_column: usize,
        to_column: usize,
        edge_type: String,
        output_schema: Vec<LogicalType>,
    ) -> Self {
        Self {
            store,
            input,
            from_column,
            to_column,
            edge_type,
            properties: Vec::new(),
            output_schema,
            output_column: None,
            viewing_epoch: None,
            transaction_id: None,
            validator: None,
            write_tracker: None,
        }
    }

    /// Sets the properties to assign to created edges.
    pub fn with_properties(mut self, properties: Vec<(String, PropertySource)>) -> Self {
        self.properties = properties;
        self
    }

    /// Sets the output column for the created edge ID.
    pub fn with_output_column(mut self, column: usize) -> Self {
        self.output_column = Some(column);
        self
    }

    /// Sets the transaction context for MVCC versioning.
    pub fn with_transaction_context(
        mut self,
        epoch: EpochId,
        transaction_id: Option<TransactionId>,
    ) -> Self {
        self.viewing_epoch = Some(epoch);
        self.transaction_id = transaction_id;
        self
    }

    /// Sets the constraint validator for schema enforcement.
    pub fn with_validator(mut self, validator: Arc<dyn ConstraintValidator>) -> Self {
        self.validator = Some(validator);
        self
    }

    /// Sets the write tracker for conflict detection.
    pub fn with_write_tracker(mut self, tracker: SharedWriteTracker) -> Self {
        self.write_tracker = Some(tracker);
        self
    }
}

impl Operator for CreateEdgeOperator {
    fn next(&mut self) -> OperatorResult {
        // Get transaction context for versioned creation
        let epoch = self
            .viewing_epoch
            .unwrap_or_else(|| self.store.current_epoch());
        let tx = self.transaction_id.unwrap_or(TransactionId::SYSTEM);

        if let Some(chunk) = self.input.next()? {
            let mut builder =
                DataChunkBuilder::with_capacity(&self.output_schema, chunk.row_count());

            for row in chunk.selected_indices() {
                // Get source and target node IDs
                let from_id = chunk
                    .column(self.from_column)
                    .and_then(|c| c.get_value(row))
                    .ok_or_else(|| {
                        OperatorError::ColumnNotFound(format!("from column {}", self.from_column))
                    })?;

                let to_id = chunk
                    .column(self.to_column)
                    .and_then(|c| c.get_value(row))
                    .ok_or_else(|| {
                        OperatorError::ColumnNotFound(format!("to column {}", self.to_column))
                    })?;

                // Extract node IDs
                let from_node_id = match from_id {
                    Value::Int64(id) => NodeId(id as u64),
                    _ => {
                        return Err(OperatorError::TypeMismatch {
                            expected: "Int64 (node ID)".to_string(),
                            found: format!("{from_id:?}"),
                        });
                    }
                };

                let to_node_id = match to_id {
                    Value::Int64(id) => NodeId(id as u64),
                    _ => {
                        return Err(OperatorError::TypeMismatch {
                            expected: "Int64 (node ID)".to_string(),
                            found: format!("{to_id:?}"),
                        });
                    }
                };

                // Validate graph type and edge endpoint constraints
                if let Some(ref validator) = self.validator {
                    validator.validate_edge_type_allowed(&self.edge_type)?;

                    // Look up source and target node labels for endpoint validation
                    let source_labels: Vec<String> = self
                        .store
                        .get_node(from_node_id)
                        .map(|n| n.labels.iter().map(|l| l.to_string()).collect())
                        .unwrap_or_default();
                    let target_labels: Vec<String> = self
                        .store
                        .get_node(to_node_id)
                        .map(|n| n.labels.iter().map(|l| l.to_string()).collect())
                        .unwrap_or_default();
                    validator.validate_edge_endpoints(
                        &self.edge_type,
                        &source_labels,
                        &target_labels,
                    )?;
                }

                // Resolve property values. Une propriete declaree nulle a la
                // creation n'est pas materialisee — meme regle que pour les
                // noeuds : « p: null » declare que p n'a pas de valeur.
                let resolved_props: Vec<(String, Value)> = self
                    .properties
                    .iter()
                    .map(|(name, source)| {
                        let value =
                            source.resolve(&chunk, row, self.store.as_ref() as &dyn GraphStore)?;
                        Ok((name.clone(), value))
                    })
                    .collect::<Result<Vec<_>, OperatorError>>()?
                    .into_iter()
                    .filter(|(_, value)| !matches!(value, Value::Null))
                    .collect();

                // Validate constraints before writing
                if let Some(ref validator) = self.validator {
                    for (name, value) in &resolved_props {
                        validator.validate_edge_property(&self.edge_type, name, value)?;
                    }
                    validator.validate_edge_complete(&self.edge_type, &resolved_props)?;
                }

                // Create the edge with MVCC versioning
                let edge_id = self.store.create_edge_versioned(
                    from_node_id,
                    to_node_id,
                    &self.edge_type,
                    epoch,
                    tx,
                );

                // Record write for conflict detection
                if let (Some(tracker), Some(tid)) = (&self.write_tracker, self.transaction_id) {
                    tracker.record_edge_write(tid, edge_id)?;
                }

                // Set properties
                if let Some(tid) = self.transaction_id {
                    for (name, value) in resolved_props {
                        self.store
                            .set_edge_property_versioned(edge_id, &name, value, tid);
                    }
                } else {
                    for (name, value) in resolved_props {
                        self.store.set_edge_property(edge_id, &name, value);
                    }
                }

                // Copy input columns
                for col_idx in 0..chunk.column_count() {
                    if let (Some(src), Some(dst)) =
                        (chunk.column(col_idx), builder.column_mut(col_idx))
                    {
                        if let Some(val) = src.get_value(row) {
                            dst.push_value(val);
                        } else {
                            dst.push_value(Value::Null);
                        }
                    }
                }

                // Add edge ID if requested
                if let Some(out_col) = self.output_column
                    && let Some(dst) = builder.column_mut(out_col)
                {
                    dst.push_value(Value::Int64(edge_id.0 as i64));
                }

                builder.advance_row();
            }

            return Ok(Some(builder.finish()));
        }
        Ok(None)
    }

    fn reset(&mut self) {
        self.input.reset();
    }

    fn name(&self) -> &'static str {
        "CreateEdge"
    }
}

/// Operator that deletes nodes.
pub struct DeleteNodeOperator {
    /// The graph store to modify.
    store: Arc<dyn GraphStoreMut>,
    /// Input operator.
    input: Box<dyn Operator>,
    /// Column index for the node to delete.
    node_column: usize,
    /// Output schema.
    output_schema: Vec<LogicalType>,
    /// Whether to detach (delete connected edges) before deleting.
    detach: bool,
    /// (V3) Vrai en transaction EXPLICITE : DETACH DELETE d'un noeud
    /// porteur d'aretes inter-bases est refuse (registre externe hors
    /// undo-log — un ROLLBACK ressusciterait le noeud, pas ses aretes).
    refuser_detach_externes: bool,
    /// Epoch for MVCC versioning.
    viewing_epoch: Option<EpochId>,
    /// Transaction ID for MVCC versioning.
    transaction_id: Option<TransactionId>,
    /// Optional write tracker for conflict detection.
    write_tracker: Option<SharedWriteTracker>,
}

impl DeleteNodeOperator {
    /// Creates a new node deletion operator.
    pub fn new(
        store: Arc<dyn GraphStoreMut>,
        input: Box<dyn Operator>,
        node_column: usize,
        output_schema: Vec<LogicalType>,
        detach: bool,
    ) -> Self {
        Self {
            store,
            input,
            node_column,
            output_schema,
            detach,
            refuser_detach_externes: false,
            viewing_epoch: None,
            transaction_id: None,
            write_tracker: None,
        }
    }

    /// (V3) Active le refus du DETACH DELETE porteur d'aretes
    /// inter-bases (transaction explicite — registre hors undo-log).
    #[must_use]
    pub fn with_detach_externes_refuses(mut self, refuser: bool) -> Self {
        self.refuser_detach_externes = refuser;
        self
    }

    /// Sets the transaction context for MVCC versioning.
    pub fn with_transaction_context(
        mut self,
        epoch: EpochId,
        transaction_id: Option<TransactionId>,
    ) -> Self {
        self.viewing_epoch = Some(epoch);
        self.transaction_id = transaction_id;
        self
    }

    /// Sets the write tracker for conflict detection.
    pub fn with_write_tracker(mut self, tracker: SharedWriteTracker) -> Self {
        self.write_tracker = Some(tracker);
        self
    }
}

impl Operator for DeleteNodeOperator {
    fn next(&mut self) -> OperatorResult {
        // Get transaction context for versioned deletion
        let epoch = self
            .viewing_epoch
            .unwrap_or_else(|| self.store.current_epoch());
        let tx = self.transaction_id.unwrap_or(TransactionId::SYSTEM);

        if let Some(chunk) = self.input.next()? {
            let mut builder =
                DataChunkBuilder::with_capacity(&self.output_schema, chunk.row_count());

            for row in chunk.selected_indices() {
                let node_val = chunk
                    .column(self.node_column)
                    .and_then(|c| c.get_value(row))
                    .ok_or_else(|| {
                        OperatorError::ColumnNotFound(format!("node column {}", self.node_column))
                    })?;

                let node_id = match node_val {
                    Value::Int64(id) => NodeId(id as u64),
                    _ => {
                        return Err(OperatorError::TypeMismatch {
                            expected: "Int64 (node ID)".to_string(),
                            found: format!("{node_val:?}"),
                        });
                    }
                };

                if self.detach {
                    // (V3) Refus enseignant : en transaction explicite, un
                    // noeud porteur d'aretes inter-bases ne se DETACH
                    // DELETE pas — le registre externe est hors undo-log.
                    if self.refuser_detach_externes {
                        let externes = self.store.external_edges_from(node_id).len();
                        if externes > 0 {
                            return Err(OperatorError::Execution(format!(
                                "ce noeud porte {externes} arete(s) inter-bases, HORS \
                                 journal d'annulation (V1) : DETACH DELETE est refuse \
                                 dans une transaction explicite — un ROLLBACK \
                                 ressusciterait le noeud mais pas ses aretes externes. \
                                 Execute la suppression hors transaction (auto-commit), \
                                 ou supprime d'abord ses aretes externes"
                            )));
                        }
                    }
                    // Delete all connected edges first, using versioned deletion
                    // so rollback can restore them
                    let outgoing = self
                        .store
                        .edges_from(node_id, crate::graph::Direction::Outgoing);
                    let incoming = self
                        .store
                        .edges_from(node_id, crate::graph::Direction::Incoming);
                    for (_, edge_id) in outgoing.into_iter().chain(incoming) {
                        self.store.delete_edge_versioned(edge_id, epoch, tx);
                        if let (Some(tracker), Some(tid)) =
                            (&self.write_tracker, self.transaction_id)
                        {
                            tracker.record_edge_write(tid, edge_id)?;
                        }
                    }
                    // Aretes inter-bases (V1) : les SORTANTES vivent chez
                    // ce noeud — elles partent avec lui (spec paragraphe 4).
                    // Hors MVCC : le registre externe ne participe pas au
                    // rollback (limitation V1, documentee).
                    for e in self.store.external_edges_from(node_id) {
                        self.store.delete_external_edge(e.id);
                    }
                } else {
                    // NODETACH: check that node has no connected edges
                    let degree = self.store.out_degree(node_id)
                        + self.store.in_degree(node_id)
                        + self.store.external_edges_from(node_id).len();
                    if degree > 0 {
                        return Err(OperatorError::ConstraintViolation(format!(
                            "Cannot delete node with {} connected edge(s). Use DETACH DELETE.",
                            degree
                        )));
                    }
                }

                // Delete the node with MVCC versioning
                self.store.delete_node_versioned(node_id, epoch, tx);

                // Record write for conflict detection
                if let (Some(tracker), Some(tid)) = (&self.write_tracker, self.transaction_id) {
                    tracker.record_node_write(tid, node_id)?;
                }

                // Pass through all input columns so downstream RETURN can
                // reference the variable (e.g., count(n) after DELETE n).
                for col_idx in 0..chunk.column_count() {
                    if let (Some(src), Some(dst)) =
                        (chunk.column(col_idx), builder.column_mut(col_idx))
                    {
                        if let Some(val) = src.get_value(row) {
                            dst.push_value(val);
                        } else {
                            dst.push_value(Value::Null);
                        }
                    }
                }
                builder.advance_row();
            }

            return Ok(Some(builder.finish()));
        }
        Ok(None)
    }

    fn reset(&mut self) {
        self.input.reset();
    }

    fn name(&self) -> &'static str {
        "DeleteNode"
    }
}

/// Operator that deletes edges.
pub struct DeleteEdgeOperator {
    /// The graph store to modify.
    store: Arc<dyn GraphStoreMut>,
    /// Input operator.
    input: Box<dyn Operator>,
    /// Column index for the edge to delete.
    edge_column: usize,
    /// Output schema.
    output_schema: Vec<LogicalType>,
    /// Epoch for MVCC versioning.
    viewing_epoch: Option<EpochId>,
    /// Transaction ID for MVCC versioning.
    transaction_id: Option<TransactionId>,
    /// Optional write tracker for conflict detection.
    write_tracker: Option<SharedWriteTracker>,
}

impl DeleteEdgeOperator {
    /// Creates a new edge deletion operator.
    pub fn new(
        store: Arc<dyn GraphStoreMut>,
        input: Box<dyn Operator>,
        edge_column: usize,
        output_schema: Vec<LogicalType>,
    ) -> Self {
        Self {
            store,
            input,
            edge_column,
            output_schema,
            viewing_epoch: None,
            transaction_id: None,
            write_tracker: None,
        }
    }

    /// Sets the transaction context for MVCC versioning.
    pub fn with_transaction_context(
        mut self,
        epoch: EpochId,
        transaction_id: Option<TransactionId>,
    ) -> Self {
        self.viewing_epoch = Some(epoch);
        self.transaction_id = transaction_id;
        self
    }

    /// Sets the write tracker for conflict detection.
    pub fn with_write_tracker(mut self, tracker: SharedWriteTracker) -> Self {
        self.write_tracker = Some(tracker);
        self
    }
}

impl Operator for DeleteEdgeOperator {
    fn next(&mut self) -> OperatorResult {
        // Get transaction context for versioned deletion
        let epoch = self
            .viewing_epoch
            .unwrap_or_else(|| self.store.current_epoch());
        let tx = self.transaction_id.unwrap_or(TransactionId::SYSTEM);

        if let Some(chunk) = self.input.next()? {
            let mut builder =
                DataChunkBuilder::with_capacity(&self.output_schema, chunk.row_count());

            for row in chunk.selected_indices() {
                let edge_val = chunk
                    .column(self.edge_column)
                    .and_then(|c| c.get_value(row))
                    .ok_or_else(|| {
                        OperatorError::ColumnNotFound(format!("edge column {}", self.edge_column))
                    })?;

                let edge_id = match edge_val {
                    Value::Int64(id) => EdgeId(id as u64),
                    _ => {
                        return Err(OperatorError::TypeMismatch {
                            expected: "Int64 (edge ID)".to_string(),
                            found: format!("{edge_val:?}"),
                        });
                    }
                };

                // Delete the edge with MVCC versioning
                self.store.delete_edge_versioned(edge_id, epoch, tx);

                // Record write for conflict detection
                if let (Some(tracker), Some(tid)) = (&self.write_tracker, self.transaction_id) {
                    tracker.record_edge_write(tid, edge_id)?;
                }

                // Pass through all input columns
                for col_idx in 0..chunk.column_count() {
                    if let (Some(src), Some(dst)) =
                        (chunk.column(col_idx), builder.column_mut(col_idx))
                    {
                        if let Some(val) = src.get_value(row) {
                            dst.push_value(val);
                        } else {
                            dst.push_value(Value::Null);
                        }
                    }
                }
                builder.advance_row();
            }

            return Ok(Some(builder.finish()));
        }
        Ok(None)
    }

    fn reset(&mut self) {
        self.input.reset();
    }

    fn name(&self) -> &'static str {
        "DeleteEdge"
    }
}

/// Operator that adds labels to nodes.
pub struct AddLabelOperator {
    /// The graph store.
    store: Arc<dyn GraphStoreMut>,
    /// Child operator providing nodes.
    input: Box<dyn Operator>,
    /// Column index containing node IDs.
    node_column: usize,
    /// Labels to add.
    labels: Vec<String>,
    /// Output schema.
    output_schema: Vec<LogicalType>,
    /// Epoch for MVCC versioning.
    viewing_epoch: Option<EpochId>,
    /// Transaction ID for undo log tracking.
    transaction_id: Option<TransactionId>,
    /// Optional write tracker for conflict detection.
    write_tracker: Option<SharedWriteTracker>,
    /// Optional constraint validator: a node that RECEIVES a label enters
    /// under that label's uniqueness constraints.
    validator: Option<Arc<dyn ConstraintValidator>>,
}

impl AddLabelOperator {
    /// Creates a new add label operator.
    pub fn new(
        store: Arc<dyn GraphStoreMut>,
        input: Box<dyn Operator>,
        node_column: usize,
        labels: Vec<String>,
        output_schema: Vec<LogicalType>,
    ) -> Self {
        Self {
            store,
            input,
            node_column,
            labels,
            output_schema,
            viewing_epoch: None,
            transaction_id: None,
            write_tracker: None,
            validator: None,
        }
    }

    /// Sets the transaction context for versioned label mutations.
    pub fn with_transaction_context(
        mut self,
        epoch: EpochId,
        transaction_id: Option<TransactionId>,
    ) -> Self {
        self.viewing_epoch = Some(epoch);
        self.transaction_id = transaction_id;
        self
    }

    /// Sets the constraint validator for schema enforcement.
    pub fn with_validator(mut self, validator: Arc<dyn ConstraintValidator>) -> Self {
        self.validator = Some(validator);
        self
    }

    /// Sets the write tracker for conflict detection.
    pub fn with_write_tracker(mut self, tracker: SharedWriteTracker) -> Self {
        self.write_tracker = Some(tracker);
        self
    }
}

impl Operator for AddLabelOperator {
    fn next(&mut self) -> OperatorResult {
        if let Some(chunk) = self.input.next()? {
            let mut builder =
                DataChunkBuilder::with_capacity(&self.output_schema, chunk.row_count());

            for row in chunk.selected_indices() {
                let node_val = chunk
                    .column(self.node_column)
                    .and_then(|c| c.get_value(row))
                    .ok_or_else(|| {
                        OperatorError::ColumnNotFound(format!("node column {}", self.node_column))
                    })?;

                let node_id = match node_val {
                    Value::Int64(id) => NodeId(id as u64),
                    _ => {
                        return Err(OperatorError::TypeMismatch {
                            expected: "Int64 (node ID)".to_string(),
                            found: format!("{node_val:?}"),
                        });
                    }
                };

                // Record write for conflict detection
                if let (Some(tracker), Some(tid)) = (&self.write_tracker, self.transaction_id) {
                    tracker.record_node_write(tid, node_id)?;
                }

                // ── LE NŒUD ENTRE SOUS LES CONTRAINTES DES LABELS REÇUS ──
                // Ses propriétés ne changent pas, mais elles deviennent
                // soumises aux contraintes d'unicité de ces labels : poser
                // `:T` sur un nœud qui porte un tuple déjà pris sous `:T`
                // créait un doublon sans aucun contrôle. Vérifier et poser
                // le label se font sous le même verrou.
                let verrou = VerrouUnicite::du_store(self.store.as_ref());
                let mut _unicite_tenue = None;
                if let Some(ref validator) = self.validator {
                    let maj = MiseAJourNoeud {
                        noeud: node_id,
                        labels: &self.labels,
                        ecrites: &[],
                        remplace_tout: false,
                        entre_sous_labels: true,
                    };
                    if let Some(colonnes) = colonnes_uniques_touchees(validator.as_ref(), &maj) {
                        _unicite_tenue = verrou.tenir();
                        controler_unicite_apres_mise_a_jour(
                            validator.as_ref(),
                            self.store.as_ref() as &dyn GraphStore,
                            &maj,
                            colonnes,
                        )?;
                    }
                }

                // Add all labels
                for label in &self.labels {
                    if let Some(tid) = self.transaction_id {
                        self.store.add_label_versioned(node_id, label, tid);
                    } else {
                        self.store.add_label(node_id, label);
                    }
                }

                // Copy input columns to output
                for col_idx in 0..chunk.column_count() {
                    if let (Some(src), Some(dst)) =
                        (chunk.column(col_idx), builder.column_mut(col_idx))
                    {
                        if let Some(val) = src.get_value(row) {
                            dst.push_value(val);
                        } else {
                            dst.push_value(Value::Null);
                        }
                    }
                }

                builder.advance_row();
            }

            return Ok(Some(builder.finish()));
        }
        Ok(None)
    }

    fn reset(&mut self) {
        self.input.reset();
    }

    fn name(&self) -> &'static str {
        "AddLabel"
    }
}

/// Operator that removes labels from nodes.
pub struct RemoveLabelOperator {
    /// The graph store.
    store: Arc<dyn GraphStoreMut>,
    /// Child operator providing nodes.
    input: Box<dyn Operator>,
    /// Column index containing node IDs.
    node_column: usize,
    /// Labels to remove.
    labels: Vec<String>,
    /// Output schema.
    output_schema: Vec<LogicalType>,
    /// Epoch for MVCC versioning.
    viewing_epoch: Option<EpochId>,
    /// Transaction ID for undo log tracking.
    transaction_id: Option<TransactionId>,
    /// Optional write tracker for conflict detection.
    write_tracker: Option<SharedWriteTracker>,
}

impl RemoveLabelOperator {
    /// Creates a new remove label operator.
    pub fn new(
        store: Arc<dyn GraphStoreMut>,
        input: Box<dyn Operator>,
        node_column: usize,
        labels: Vec<String>,
        output_schema: Vec<LogicalType>,
    ) -> Self {
        Self {
            store,
            input,
            node_column,
            labels,
            output_schema,
            viewing_epoch: None,
            transaction_id: None,
            write_tracker: None,
        }
    }

    /// Sets the transaction context for versioned label mutations.
    pub fn with_transaction_context(
        mut self,
        epoch: EpochId,
        transaction_id: Option<TransactionId>,
    ) -> Self {
        self.viewing_epoch = Some(epoch);
        self.transaction_id = transaction_id;
        self
    }

    /// Sets the write tracker for conflict detection.
    pub fn with_write_tracker(mut self, tracker: SharedWriteTracker) -> Self {
        self.write_tracker = Some(tracker);
        self
    }
}

impl Operator for RemoveLabelOperator {
    fn next(&mut self) -> OperatorResult {
        if let Some(chunk) = self.input.next()? {
            let mut builder =
                DataChunkBuilder::with_capacity(&self.output_schema, chunk.row_count());

            for row in chunk.selected_indices() {
                let node_val = chunk
                    .column(self.node_column)
                    .and_then(|c| c.get_value(row))
                    .ok_or_else(|| {
                        OperatorError::ColumnNotFound(format!("node column {}", self.node_column))
                    })?;

                let node_id = match node_val {
                    Value::Int64(id) => NodeId(id as u64),
                    _ => {
                        return Err(OperatorError::TypeMismatch {
                            expected: "Int64 (node ID)".to_string(),
                            found: format!("{node_val:?}"),
                        });
                    }
                };

                // Record write for conflict detection
                if let (Some(tracker), Some(tid)) = (&self.write_tracker, self.transaction_id) {
                    tracker.record_node_write(tid, node_id)?;
                }

                // Remove all labels
                for label in &self.labels {
                    if let Some(tid) = self.transaction_id {
                        self.store.remove_label_versioned(node_id, label, tid);
                    } else {
                        self.store.remove_label(node_id, label);
                    }
                }

                // Copy input columns to output
                for col_idx in 0..chunk.column_count() {
                    if let (Some(src), Some(dst)) =
                        (chunk.column(col_idx), builder.column_mut(col_idx))
                    {
                        if let Some(val) = src.get_value(row) {
                            dst.push_value(val);
                        } else {
                            dst.push_value(Value::Null);
                        }
                    }
                }

                builder.advance_row();
            }

            return Ok(Some(builder.finish()));
        }
        Ok(None)
    }

    fn reset(&mut self) {
        self.input.reset();
    }

    fn name(&self) -> &'static str {
        "RemoveLabel"
    }
}

/// Operator that sets properties on nodes or edges.
///
/// This operator reads node/edge IDs from a column and sets the
/// specified properties on each entity.
pub struct SetPropertyOperator {
    /// The graph store.
    store: Arc<dyn GraphStoreMut>,
    /// Child operator providing entities.
    input: Box<dyn Operator>,
    /// Column index containing entity IDs (node or edge).
    entity_column: usize,
    /// Whether the entity is an edge (false = node).
    is_edge: bool,
    /// Properties to set (name -> source).
    properties: Vec<(String, PropertySource)>,
    /// Output schema.
    output_schema: Vec<LogicalType>,
    /// Whether to replace all properties (true) or merge (false) for map assignments.
    replace: bool,
    /// Whether the listed properties are REMOVED (real key deletion) instead
    /// of assigned. Set by `REMOVE n.x` / `REMOVE r.p` : the store emits a
    /// tombstone and a Remove*Property WAL record — never a stored NULL.
    remove: bool,
    /// Optional constraint validator for schema enforcement.
    validator: Option<Arc<dyn ConstraintValidator>>,
    /// Entity labels (for node constraint validation).
    labels: Vec<String>,
    /// Edge type (for edge constraint validation).
    edge_type_name: Option<String>,
    /// Epoch for MVCC versioning.
    viewing_epoch: Option<EpochId>,
    /// Transaction ID for undo log tracking.
    transaction_id: Option<TransactionId>,
    /// Optional write tracker for conflict detection.
    write_tracker: Option<SharedWriteTracker>,
}

impl SetPropertyOperator {
    /// Creates a new set property operator for nodes.
    pub fn new_for_node(
        store: Arc<dyn GraphStoreMut>,
        input: Box<dyn Operator>,
        node_column: usize,
        properties: Vec<(String, PropertySource)>,
        output_schema: Vec<LogicalType>,
    ) -> Self {
        Self {
            store,
            input,
            entity_column: node_column,
            is_edge: false,
            properties,
            output_schema,
            replace: false,
            remove: false,
            validator: None,
            labels: Vec::new(),
            edge_type_name: None,
            viewing_epoch: None,
            transaction_id: None,
            write_tracker: None,
        }
    }

    /// Creates a new set property operator for edges.
    pub fn new_for_edge(
        store: Arc<dyn GraphStoreMut>,
        input: Box<dyn Operator>,
        edge_column: usize,
        properties: Vec<(String, PropertySource)>,
        output_schema: Vec<LogicalType>,
    ) -> Self {
        Self {
            store,
            input,
            entity_column: edge_column,
            is_edge: true,
            properties,
            output_schema,
            replace: false,
            remove: false,
            validator: None,
            labels: Vec::new(),
            edge_type_name: None,
            viewing_epoch: None,
            transaction_id: None,
            write_tracker: None,
        }
    }

    /// Sets whether this operator replaces all properties (for map assignment).
    pub fn with_replace(mut self, replace: bool) -> Self {
        self.replace = replace;
        self
    }

    /// Sets whether this operator REMOVES the listed properties (for
    /// `REMOVE n.x` / `REMOVE r.p`) instead of assigning them.
    pub fn with_remove(mut self, remove: bool) -> Self {
        self.remove = remove;
        self
    }

    /// Removes one property of the target entity — the REAL removal: the
    /// store writes a tombstone and the WAL carries a Remove*Property record.
    /// Removing an absent key is a no-op, which makes REMOVE idempotent.
    fn remove_entity_property(&self, entity_id: u64, key: &str) {
        match (self.is_edge, self.transaction_id) {
            (true, Some(tid)) => {
                self.store
                    .remove_edge_property_versioned(EdgeId(entity_id), key, tid);
            }
            (true, None) => {
                self.store.remove_edge_property(EdgeId(entity_id), key);
            }
            (false, Some(tid)) => {
                self.store
                    .remove_node_property_versioned(NodeId(entity_id), key, tid);
            }
            (false, None) => {
                self.store.remove_node_property(NodeId(entity_id), key);
            }
        }
    }

    /// Sets the constraint validator for schema enforcement.
    pub fn with_validator(mut self, validator: Arc<dyn ConstraintValidator>) -> Self {
        self.validator = Some(validator);
        self
    }

    /// Sets the entity labels (for node constraint validation).
    pub fn with_labels(mut self, labels: Vec<String>) -> Self {
        self.labels = labels;
        self
    }

    /// Sets the edge type name (for edge constraint validation).
    pub fn with_edge_type(mut self, edge_type: String) -> Self {
        self.edge_type_name = Some(edge_type);
        self
    }

    /// Sets the transaction context for versioned property mutations.
    ///
    /// When a transaction ID is provided, property changes are recorded in
    /// an undo log so they can be restored on rollback.
    pub fn with_transaction_context(
        mut self,
        epoch: EpochId,
        transaction_id: Option<TransactionId>,
    ) -> Self {
        self.viewing_epoch = Some(epoch);
        self.transaction_id = transaction_id;
        self
    }

    /// Sets the write tracker for conflict detection.
    pub fn with_write_tracker(mut self, tracker: SharedWriteTracker) -> Self {
        self.write_tracker = Some(tracker);
        self
    }
}

impl Operator for SetPropertyOperator {
    fn next(&mut self) -> OperatorResult {
        if let Some(chunk) = self.input.next()? {
            let mut builder =
                DataChunkBuilder::with_capacity(&self.output_schema, chunk.row_count());

            for row in chunk.selected_indices() {
                let entity_val = chunk
                    .column(self.entity_column)
                    .and_then(|c| c.get_value(row))
                    .ok_or_else(|| {
                        OperatorError::ColumnNotFound(format!(
                            "entity column {}",
                            self.entity_column
                        ))
                    })?;

                let entity_id = match entity_val {
                    Value::Int64(id) => id as u64,
                    _ => {
                        return Err(OperatorError::TypeMismatch {
                            expected: "Int64 (entity ID)".to_string(),
                            found: format!("{entity_val:?}"),
                        });
                    }
                };

                // Record write for conflict detection
                if let (Some(tracker), Some(tid)) = (&self.write_tracker, self.transaction_id) {
                    if self.is_edge {
                        tracker.record_edge_write(tid, EdgeId(entity_id))?;
                    } else {
                        tracker.record_node_write(tid, NodeId(entity_id))?;
                    }
                }

                // REMOVE n.x / REMOVE r.p : retrait REEL de chaque cle
                // listee — tombstone en stockage, enregistrement WAL
                // Remove*Property — jamais un NULL stocke. Les expressions
                // de valeur ne sont pas resolues : seule la cle compte, et
                // retirer une cle absente ne change rien (idempotence).
                if self.remove {
                    for (prop_name, _source) in &self.properties {
                        self.remove_entity_property(entity_id, prop_name);
                    }

                    // Copy input columns to output (pass-through)
                    for col_idx in 0..chunk.column_count() {
                        if let (Some(src), Some(dst)) =
                            (chunk.column(col_idx), builder.column_mut(col_idx))
                        {
                            if let Some(val) = src.get_value(row) {
                                dst.push_value(val);
                            } else {
                                dst.push_value(Value::Null);
                            }
                        }
                    }
                    builder.advance_row();
                    continue;
                }

                // Resolve all property values — TOUTES avant la premiere
                // ecriture de la ligne : une expression (`SET n.x = n.x + 1`)
                // lit ainsi l'etat d'AVANT l'ecriture, jamais le sien.
                let resolved_props: Vec<(String, Value)> = self
                    .properties
                    .iter()
                    .map(|(name, source)| {
                        let value =
                            source.resolve(&chunk, row, self.store.as_ref() as &dyn GraphStore)?;
                        Ok((name.clone(), value))
                    })
                    .collect::<Result<_, OperatorError>>()?;

                // Validate constraints before writing
                //
                // Le verrou d'unicité, s'il est pris, est tenu jusqu'à la fin
                // de l'itération : il couvre le contrôle ET les écritures de
                // cette ligne.
                let verrou = VerrouUnicite::du_store(self.store.as_ref());
                let mut _unicite_tenue = None;
                if let Some(ref validator) = self.validator {
                    if self.is_edge {
                        if let Some(ref et) = self.edge_type_name {
                            for (name, value) in &resolved_props {
                                validator.validate_edge_property(et, name, value)?;
                            }
                        }
                    } else {
                        for (name, value) in &resolved_props {
                            validator.validate_node_property(&self.labels, name, value)?;
                        }
                        // ── L'UNICITÉ SE JUGE SUR LES LABELS DU NŒUD ──
                        // `self.labels` est vide : le planificateur ne connaît
                        // pas les labels d'un nœud lié par un MATCH. Le
                        // contrôle d'unicité recevait donc une liste vide, ne
                        // trouvait aucune contrainte, et un SET n'était JAMAIS
                        // contrôlé. Les labels sont lus sur le nœud.
                        //
                        // `SET n = {…}` et `SET n += {…}` arrivent sous la clé
                        // `*` avec une carte : elle est dépliée en écritures.
                        let noeud = NodeId(entity_id);
                        let store_lecture = self.store.as_ref() as &dyn GraphStore;
                        let labels = if self.labels.is_empty() {
                            labels_du_noeud(store_lecture, noeud)
                        } else {
                            self.labels.clone()
                        };
                        let mut ecrites: Vec<(String, Value)> =
                            Vec::with_capacity(resolved_props.len());
                        let mut remplace_tout = false;
                        for (name, value) in &resolved_props {
                            match (name.as_str(), value) {
                                ("*", Value::Map(carte)) => {
                                    remplace_tout |= self.replace;
                                    ecrites.extend(
                                        carte
                                            .iter()
                                            .map(|(k, v)| (k.as_str().to_string(), v.clone())),
                                    );
                                }
                                ("*", _) => {}
                                _ => ecrites.push((name.clone(), value.clone())),
                            }
                        }
                        let maj = MiseAJourNoeud {
                            noeud,
                            labels: &labels,
                            ecrites: &ecrites,
                            remplace_tout,
                            entre_sous_labels: false,
                        };
                        if let Some(colonnes) = colonnes_uniques_touchees(validator.as_ref(), &maj)
                        {
                            _unicite_tenue = verrou.tenir();
                            controler_unicite_apres_mise_a_jour(
                                validator.as_ref(),
                                store_lecture,
                                &maj,
                                colonnes,
                            )?;
                        }
                    }
                }

                // Write all properties (use versioned methods when inside a transaction)
                let tx_id = self.transaction_id;
                for (prop_name, value) in resolved_props {
                    if prop_name == "*" {
                        // Map assignment: value should be a Map
                        if let Value::Map(map) = value {
                            if self.replace {
                                // Replace: remove all existing properties first
                                if self.is_edge {
                                    if let Some(edge) = self.store.get_edge(EdgeId(entity_id)) {
                                        let keys: Vec<String> = edge
                                            .properties
                                            .iter()
                                            .map(|(k, _)| k.as_str().to_string())
                                            .collect();
                                        for key in keys {
                                            if let Some(tid) = tx_id {
                                                self.store.remove_edge_property_versioned(
                                                    EdgeId(entity_id),
                                                    &key,
                                                    tid,
                                                );
                                            } else {
                                                self.store
                                                    .remove_edge_property(EdgeId(entity_id), &key);
                                            }
                                        }
                                    }
                                } else if let Some(node) = self.store.get_node(NodeId(entity_id)) {
                                    let keys: Vec<String> = node
                                        .properties
                                        .iter()
                                        .map(|(k, _)| k.as_str().to_string())
                                        .collect();
                                    for key in keys {
                                        if let Some(tid) = tx_id {
                                            self.store.remove_node_property_versioned(
                                                NodeId(entity_id),
                                                &key,
                                                tid,
                                            );
                                        } else {
                                            self.store
                                                .remove_node_property(NodeId(entity_id), &key);
                                        }
                                    }
                                }
                            }
                            // Set each map entry
                            for (key, val) in map.iter() {
                                // Une entree nulle de carte declare « pas de
                                // valeur » : meme regle que SET n.x = null,
                                // la cle est retiree, jamais ecrite a NULL.
                                // En mode replace elle est deja partie — le
                                // retrait est alors un no-op.
                                if matches!(val, Value::Null) {
                                    self.remove_entity_property(entity_id, key.as_str());
                                    continue;
                                }
                                if self.is_edge {
                                    if let Some(tid) = tx_id {
                                        self.store.set_edge_property_versioned(
                                            EdgeId(entity_id),
                                            key.as_str(),
                                            val.clone(),
                                            tid,
                                        );
                                    } else {
                                        self.store.set_edge_property(
                                            EdgeId(entity_id),
                                            key.as_str(),
                                            val.clone(),
                                        );
                                    }
                                } else if let Some(tid) = tx_id {
                                    self.store.set_node_property_versioned(
                                        NodeId(entity_id),
                                        key.as_str(),
                                        val.clone(),
                                        tid,
                                    );
                                } else {
                                    self.store.set_node_property(
                                        NodeId(entity_id),
                                        key.as_str(),
                                        val.clone(),
                                    );
                                }
                            }
                        }
                    } else if matches!(value, Value::Null) {
                        // SET n.x = null (litteral OU expression resolue a
                        // NULL) declare « x n'a pas de valeur » : la cle est
                        // RETIREE, comme le ferait un remplacement par une
                        // carte sans cette cle. Ecrire un NULL stocke
                        // laisserait le noeud memoriser la ROUTE d'ecriture
                        // au lieu de l'etat declare (cf. gql_13, groupe 7).
                        self.remove_entity_property(entity_id, &prop_name);
                    } else if self.is_edge {
                        if let Some(tid) = tx_id {
                            self.store.set_edge_property_versioned(
                                EdgeId(entity_id),
                                &prop_name,
                                value,
                                tid,
                            );
                        } else {
                            self.store
                                .set_edge_property(EdgeId(entity_id), &prop_name, value);
                        }
                    } else if let Some(tid) = tx_id {
                        self.store.set_node_property_versioned(
                            NodeId(entity_id),
                            &prop_name,
                            value,
                            tid,
                        );
                    } else {
                        self.store
                            .set_node_property(NodeId(entity_id), &prop_name, value);
                    }
                }

                // Copy input columns to output
                for col_idx in 0..chunk.column_count() {
                    if let (Some(src), Some(dst)) =
                        (chunk.column(col_idx), builder.column_mut(col_idx))
                    {
                        if let Some(val) = src.get_value(row) {
                            dst.push_value(val);
                        } else {
                            dst.push_value(Value::Null);
                        }
                    }
                }

                builder.advance_row();
            }

            return Ok(Some(builder.finish()));
        }
        Ok(None)
    }

    fn reset(&mut self) {
        self.input.reset();
    }

    fn name(&self) -> &'static str {
        "SetProperty"
    }
}

// ---------------------------------------------------------------------------
// CreateExternalEdgeOperator — aretes inter-bases (V1)
// ---------------------------------------------------------------------------

/// Cree une arete EXTERNE `(source locale) -[TYPE]-> ref(hote?, slug, clef)`
/// pour chaque ligne d'entree, puis laisse passer la ligne inchangee.
///
/// Contrats (spec << aretes inter-bases natives >>) :
/// - la SOURCE doit etre un noeud VIVANT de la base courante — une ecriture
///   ne touche qu'une base (sa WAL). Toute autre valeur en colonne source
///   est refusee avec une erreur enseignante, jamais un silence ;
/// - la CIBLE n'est PAS resolue a l'ecriture : c'est une adresse. Une cible
///   inexistante rendra simplement << absent >> a la traversee (trivalue) ;
/// - `merge = true` : idempotent — une arete identique (type, hote, slug,
///   clef) deja portee par la source n'est pas dupliquee.
pub struct CreateExternalEdgeOperator {
    store: Arc<dyn GraphStoreMut>,
    input: Box<dyn Operator>,
    from_column: usize,
    from_variable: String,
    edge_type: String,
    to_host: Option<String>,
    to_store: String,
    to_key: String,
    merge: bool,
    output_schema: Vec<LogicalType>,
}

impl CreateExternalEdgeOperator {
    /// Creates a new external-edge creation operator.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        store: Arc<dyn GraphStoreMut>,
        input: Box<dyn Operator>,
        from_column: usize,
        from_variable: String,
        edge_type: String,
        to_host: Option<String>,
        to_store: String,
        to_key: String,
        merge: bool,
        output_schema: Vec<LogicalType>,
    ) -> Self {
        Self {
            store,
            input,
            from_column,
            from_variable,
            edge_type,
            to_host,
            to_store,
            to_key,
            merge,
            output_schema,
        }
    }
}

impl Operator for CreateExternalEdgeOperator {
    fn next(&mut self) -> OperatorResult {
        if let Some(chunk) = self.input.next()? {
            let mut builder =
                DataChunkBuilder::with_capacity(&self.output_schema, chunk.row_count());

            for row in chunk.selected_indices() {
                let from_id = chunk
                    .column(self.from_column)
                    .and_then(|c| c.get_value(row))
                    .ok_or_else(|| {
                        OperatorError::ColumnNotFound(format!("from column {}", self.from_column))
                    })?;

                let from_node_id = match from_id {
                    Value::Int64(id) => NodeId(id as u64),
                    autre => {
                        return Err(OperatorError::Execution(format!(
                            "la source '{}' d'une arete inter-bases doit etre un noeud \
                             de la base COURANTE (une ecriture ne touche qu'une base, \
                             sa WAL) — recu {autre:?}. Pour ecrire depuis une autre \
                             base, ouvre une session sur elle et fais-y le CREATE",
                            self.from_variable
                        )));
                    }
                };

                if self.store.get_node_labels(from_node_id).is_none() {
                    return Err(OperatorError::Execution(format!(
                        "la source '{}' (noeud {}) n'existe pas dans la base \
                         courante — une arete inter-bases s'ecrit depuis la base \
                         qui porte sa source",
                        self.from_variable, from_node_id.0
                    )));
                }

                let deja = self.merge
                    && self
                        .store
                        .external_edges_from(from_node_id)
                        .iter()
                        .any(|e| {
                            e.edge_type == self.edge_type
                                && e.to_host.as_deref() == self.to_host.as_deref()
                                && e.to_store == self.to_store
                                && e.to_key == self.to_key
                        });

                if !deja
                    && self
                        .store
                        .create_external_edge(
                            from_node_id,
                            &self.edge_type,
                            self.to_host.as_deref(),
                            &self.to_store,
                            &self.to_key,
                        )
                        .is_none()
                {
                    return Err(OperatorError::Execution(
                        "le backend de stockage courant ne materialise pas les \
                         aretes inter-bases (registre externe absent) — base \
                         substrate requise"
                            .to_string(),
                    ));
                }

                // La ligne passe inchangee.
                for col_idx in 0..chunk.column_count() {
                    if let (Some(src), Some(dst)) =
                        (chunk.column(col_idx), builder.column_mut(col_idx))
                    {
                        if let Some(val) = src.get_value(row) {
                            dst.push_value(val);
                        } else {
                            dst.push_value(Value::Null);
                        }
                    }
                }
                builder.advance_row();
            }

            Ok(Some(builder.finish()))
        } else {
            Ok(None)
        }
    }

    fn reset(&mut self) {
        self.input.reset();
    }

    fn name(&self) -> &'static str {
        "CreateExternalEdge"
    }
}

// ── Tests ────────────────────────────────────────────────────────────────
//
// The `#[cfg(test)] mod tests { ... }` block that previously lived here has
// been relocated to `crates/obrain-substrate/tests/operators_mutation.rs`
// as part of T17 W4.p4. Rationale: the LpgStore fixture is being retired
// and the substrate-backed replacement cannot live inside `obrain-core`
// because of a dev-dep cycle — adding `obrain-substrate` to the `[dev-dependencies]`
// of `obrain-core` produces two distinct compilation units of `obrain-core`,
// breaking the `Arc<SubstrateStore> as Arc<dyn GraphStoreMut>` trait cast.
//
// See:
//   - decision 0a84cf59 (T17 W4.p4 test-relocation strategy)
//   - gotcha   598dda40-a186-4be3-97f3-c75053af4e6e (dev-dep cycle diagnosis)
//
// Migrated tests run under:
//   cargo test -p obrain-substrate --test operators_mutation
