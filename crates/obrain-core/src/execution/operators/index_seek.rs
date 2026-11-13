//! Jointure par sonde d'index correlee (index-seek join).
//!
//! Remplace le produit cartesien `NestedLoopJoin(Cross)` + filtre par ligne
//! pour les jointures de la forme `MATCH (a:A) MATCH (b:B {k: a.fk})` :
//! pour CHAQUE ligne externe, la cle `a.fk` est evaluee puis l'index de
//! propriete est sonde — O(|externe| x sondes) au lieu de
//! O(|externe| x |label(B)|) paires materialisees. Mesure en production
//! (mission perf DB, chantier #95) : 324 s pour une jointure
//! Evidence x Claim en produit cartesien.
//!
//! Semantique :
//! - cle NULL ou absente → ZERO ligne pour cette ligne externe (logique
//!   trivaluee : `b.k = NULL` est UNKNOWN, on ne sonde JAMAIS l'index
//!   avec Null) ;
//! - cle Bool | Int64 | String, ou Float64 ENTIER → sonde
//!   `find_nodes_by_property` (repli scan transparent integre cote store si
//!   l'index n'existe pas — on ne court-circuite PAS sur
//!   `has_property_index == false`). Cote store la cle d'index est canonique
//!   vis-a-vis de l'egalite : une sonde `Int64(5)` TROUVE un `Float64(5.0)`
//!   stocke, et une sonde `Float64(5.0)` trouve un `Int64(5)` ;
//! - autre type de cle (Float64 NON entier, temporel, liste…) : l'index n'a
//!   pas de seau pour ces valeurs, et le store les chercherait par un
//!   balayage complet A CHAQUE ligne externe. Pour ne pas perdre de lignes,
//!   on retombe sur une comparaison generique coercee sur les candidats du
//!   label (materialises UNE fois, paresseusement) — jamais un resultat
//!   vide silencieux ;
//! - chaque candidat est reverifie : label (`node_has_label`, test de bit
//!   O(1)) puis visibilite MVCC par ligne (`is_node_visible_versioned` /
//!   `is_node_visible_at_epoch` — pas d'amortissement possible, l'epoque
//!   de vue est fixe mais les transactions non).

use std::sync::Arc;

use obrain_common::types::{EpochId, LogicalType, NodeId, TransactionId, Value};

use super::filter::ExpressionPredicate;
use super::{Operator, OperatorResult};
use crate::execution::DataChunk;
use crate::execution::chunk::DataChunkBuilder;
use crate::graph::GraphStore;

/// Égalité de jointure via le comparateur CANONIQUE — même verdict que `=`
/// dans l'évaluateur : Int64↔Float64 exact (plus d'EPSILON), flottants IEEE
/// stricts, classes disjointes → pas de correspondance.
fn values_equal_coerced(a: &Value, b: &Value) -> bool {
    obrain_common::types::value_cmp::egaler(a, b) == Some(true)
}

/// Vrai si le store sait chercher cette valeur PAR SON INDEX d'egalite :
/// Bool, Int64, String — et un Float64 EXACTEMENT egal a un entier, que la
/// cle canonique du substrat range avec cet entier
/// (`value_cmp::entier_egal_au_flottant`, la meme fonction des deux cotes).
///
/// Un Float64 non entier, lui, n'a pas de seau : le sonder couterait un
/// balayage complet par ligne externe, d'ou le chemin generique.
/// Une chaîne de plus de 256 octets est routée en COLONNE BLOB par le
/// substrat (`BLOB_COLUMN_THRESHOLD_BYTES`) et n'entre dans aucun index : la
/// sonder coûterait un balayage par ligne externe (revue 3, M2). Le seuil est
/// recopié ici parce que `obrain-core` ne dépend pas du substrat ; le test
/// `une_cle_flottante_entiere_se_sonde_par_l_index` le garde.
const SEUIL_CHAINE_SONDABLE: usize = 256;

fn is_index_probe_safe(v: &Value) -> bool {
    match v {
        Value::Bool(_) | Value::Int64(_) => true,
        Value::String(s) => s.len() <= SEUIL_CHAINE_SONDABLE,
        Value::Float64(f) => obrain_common::types::value_cmp::entier_egal_au_flottant(*f).is_some(),
        _ => false,
    }
}

/// Jointure indexee correlee : `outer ⋈ (b:label) ON b.property = key_expr(ligne)`.
///
/// Modele d'execution calque sur `NestedLoopJoinOperator` (trait [`Operator`],
/// production par [`DataChunk`], schema de sortie = colonnes externes + Node),
/// mais le cote interne n'est jamais materialise : une sonde d'index par
/// ligne externe.
pub struct IndexSeekJoinOperator {
    /// Plan externe (le premier MATCH).
    outer: Box<dyn Operator>,
    /// Expression de cle, evaluee par ligne externe (colonnes externes seules).
    key_expr: ExpressionPredicate,
    /// Propriete sondee sur le cote interne.
    property: String,
    /// Label optionnel du cote interne (reverifie par candidat).
    label: Option<String>,
    /// Store pour les sondes d'index et les tests de visibilite.
    store: Arc<dyn GraphStore>,
    /// Epoque de vue MVCC.
    viewing_epoch: EpochId,
    /// Transaction courante (visibilite versionnee par ligne).
    transaction_id: Option<TransactionId>,
    /// Schema de sortie : types des colonnes externes + `LogicalType::Node`.
    output_schema: Vec<LogicalType>,

    // ---- Etat d'iteration ----
    /// Chunk externe en cours.
    current_outer_chunk: Option<DataChunk>,
    /// Indices selectionnes du chunk externe en cours.
    outer_rows: Vec<usize>,
    /// Position dans `outer_rows`.
    current_outer_pos: usize,
    /// Candidats internes de la ligne externe courante (deja label + MVCC).
    current_matches: Vec<NodeId>,
    /// Position dans `current_matches`.
    current_match_pos: usize,
    /// Repli pour cles non projetables : candidats du label, materialises
    /// UNE seule fois au premier besoin (None = jamais demande).
    fallback_pool: Option<Vec<NodeId>>,
}

impl IndexSeekJoinOperator {
    /// Cree la jointure indexee correlee.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        outer: Box<dyn Operator>,
        key_expr: ExpressionPredicate,
        property: impl Into<String>,
        label: Option<String>,
        store: Arc<dyn GraphStore>,
        viewing_epoch: EpochId,
        transaction_id: Option<TransactionId>,
        output_schema: Vec<LogicalType>,
    ) -> Self {
        Self {
            outer,
            key_expr,
            property: property.into(),
            label,
            store,
            viewing_epoch,
            transaction_id,
            output_schema,
            current_outer_chunk: None,
            outer_rows: Vec::new(),
            current_outer_pos: 0,
            current_matches: Vec::new(),
            current_match_pos: 0,
            fallback_pool: None,
        }
    }

    /// Candidats pour une cle NON projetable dans l'index : comparaison
    /// generique coercee sur le pool du label. Le pool (ids seulement) est
    /// materialise une fois ; les proprietes sont relues par ligne car la
    /// visibilite MVCC depend de la transaction.
    fn generic_candidates(&mut self, key: &Value) -> Vec<NodeId> {
        if self.fallback_pool.is_none() {
            let pool = match &self.label {
                Some(label) => self.store.nodes_by_label(label),
                None => self.store.node_ids(),
            };
            self.fallback_pool = Some(pool);
        }
        let epoch = self.viewing_epoch;
        let tx = self.transaction_id;
        let property = self.property.clone();
        self.fallback_pool
            .as_ref()
            .expect("fallback_pool vient d'etre materialise")
            .iter()
            .copied()
            .filter(|&id| {
                let node = if let Some(tx) = tx {
                    self.store.get_node_versioned(id, epoch, tx)
                } else {
                    self.store.get_node_at_epoch(id, epoch)
                };
                node.is_some_and(|n| {
                    n.get_property(&property)
                        .is_some_and(|v| values_equal_coerced(v, key))
                })
            })
            .collect()
    }

    /// Calcule les candidats internes pour une ligne externe.
    ///
    /// Cle NULL/absente → vide (trivalue). Sinon sonde d'index (ou repli
    /// generique), puis reverification label + visibilite MVCC.
    fn matches_for_row(&mut self, chunk: &DataChunk, row: usize) -> Vec<NodeId> {
        let key = match self.key_expr.eval_at(chunk, row) {
            None | Some(Value::Null) => return Vec::new(),
            Some(v) => v,
        };

        let candidates = if is_index_probe_safe(&key) {
            // `find_nodes_by_property` integre son propre repli scan quand
            // l'index n'existe pas — pas de court-circuit ici.
            let hits = self.store.find_nodes_by_property(&self.property, &key);
            // L'index ignore les labels : reverification par test de bit,
            // O(k) sur les candidats — jamais de balayage du label.
            match &self.label {
                Some(label) => hits
                    .into_iter()
                    .filter(|&n| self.store.node_has_label(n, label).unwrap_or(false))
                    .collect(),
                None => hits,
            }
        } else {
            // Valeur non projetable par `index_value_key` : le pool est
            // deja restreint au label.
            self.generic_candidates(&key)
        };

        // Visibilite MVCC : l'index peut porter des noeuds invisibles a
        // cette epoque / transaction — y compris les ecritures NON COMMITEES
        // d'une autre session (gql_13, vague 4a). Le filtre PAR LOT est le
        // MEME chemin que celui de `ScanOperator` : un seul verrou pour tous
        // les candidats, et la carte `en_attente_noeuds` consultee — la ou
        // les tests unitaires par candidat prenaient un verrou par noeud.
        let epoch = self.viewing_epoch;
        if let Some(tx) = self.transaction_id {
            self.store
                .filter_visible_node_ids_versioned(&candidates, epoch, tx)
        } else {
            self.store.filter_visible_node_ids(&candidates, epoch)
        }
    }

    /// Emet `(ligne externe ++ node)` dans le builder.
    fn produce_row(
        builder: &mut DataChunkBuilder,
        outer_chunk: &DataChunk,
        outer_row: usize,
        node: NodeId,
    ) {
        for col_idx in 0..outer_chunk.column_count() {
            if let (Some(src), Some(dst)) =
                (outer_chunk.column(col_idx), builder.column_mut(col_idx))
            {
                if let Some(val) = src.get_value(outer_row) {
                    dst.push_value(val);
                } else {
                    dst.push_value(Value::Null);
                }
            }
        }
        let node_col = outer_chunk.column_count();
        if let Some(dst) = builder.column_mut(node_col) {
            dst.push_node_id(node);
        }
        builder.advance_row();
    }
}

impl Operator for IndexSeekJoinOperator {
    fn next(&mut self) -> OperatorResult {
        let mut builder = DataChunkBuilder::with_capacity(&self.output_schema, 2048);

        loop {
            super::verifier_annulation()?;
            // Charger un chunk externe si necessaire.
            if self.current_outer_chunk.is_none() {
                match self.outer.next()? {
                    Some(chunk) => {
                        self.outer_rows = chunk.selected_indices().collect();
                        self.current_outer_chunk = Some(chunk);
                        self.current_outer_pos = 0;
                        self.current_matches.clear();
                        self.current_match_pos = 0;
                    }
                    None => {
                        return if builder.row_count() > 0 {
                            Ok(Some(builder.finish()))
                        } else {
                            Ok(None)
                        };
                    }
                }
            }

            while self.current_outer_pos < self.outer_rows.len() {
                let outer_row = self.outer_rows[self.current_outer_pos];

                // Premiere visite de cette ligne externe : sonder.
                if self.current_match_pos == 0 && self.current_matches.is_empty() {
                    let chunk = self
                        .current_outer_chunk
                        .take()
                        .expect("chunk externe present : charge dans la boucle ci-dessus");
                    self.current_matches = self.matches_for_row(&chunk, outer_row);
                    self.current_outer_chunk = Some(chunk);
                }

                let outer_chunk = self
                    .current_outer_chunk
                    .as_ref()
                    .expect("chunk externe present : charge dans la boucle ci-dessus");

                while self.current_match_pos < self.current_matches.len() {
                    let node = self.current_matches[self.current_match_pos];
                    Self::produce_row(&mut builder, outer_chunk, outer_row, node);
                    self.current_match_pos += 1;

                    if builder.is_full() {
                        return Ok(Some(builder.finish()));
                    }
                }

                // Ligne externe epuisee.
                self.current_outer_pos += 1;
                self.current_matches.clear();
                self.current_match_pos = 0;
            }

            // Chunk externe epuise.
            self.current_outer_chunk = None;

            if builder.row_count() > 0 {
                return Ok(Some(builder.finish()));
            }
        }
    }

    fn reset(&mut self) {
        self.outer.reset();
        self.current_outer_chunk = None;
        self.outer_rows.clear();
        self.current_outer_pos = 0;
        self.current_matches.clear();
        self.current_match_pos = 0;
        self.fallback_pool = None;
    }

    fn name(&self) -> &'static str {
        "IndexSeekJoin"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Une clé `Float64` ENTIÈRE se sonde par l'index, comme l'entier
    /// qu'elle égale ; un flottant non entier, NaN ou infini passe par le
    /// chemin générique (il n'a pas de seau d'index).
    #[test]
    fn une_cle_flottante_entiere_se_sonde_par_l_index() {
        assert!(is_index_probe_safe(&Value::Int64(5)));
        assert!(is_index_probe_safe(&Value::Bool(true)));
        assert!(is_index_probe_safe(&Value::String("x".into())));
        assert!(!is_index_probe_safe(&Value::String("x".repeat(300).into())));
        assert!(is_index_probe_safe(&Value::Float64(5.0)));
        assert!(is_index_probe_safe(&Value::Float64(-0.0)));
        assert!(!is_index_probe_safe(&Value::Float64(5.5)));
        assert!(!is_index_probe_safe(&Value::Float64(f64::NAN)));
        assert!(!is_index_probe_safe(&Value::Float64(f64::INFINITY)));
        assert!(!is_index_probe_safe(&Value::Float64(1.0e300)));
        assert!(!is_index_probe_safe(&Value::Null));
    }
}
