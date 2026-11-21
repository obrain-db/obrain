//! PROFILE statement: per-operator execution metrics.
//!
//! After a profiled query executes, the results are collected into a
//! [`ProfileNode`] tree that mirrors the physical operator tree, annotated
//! with actual row counts, timing, and call counts.

use std::collections::HashMap;
use std::fmt::Write;
use std::sync::Arc;

use obrain_common::types::{LogicalType, Value};
use obrain_core::execution::profile::{ProfileStats, SharedProfileStats};
use parking_lot::Mutex;

use super::plan::LogicalOperator;
use crate::database::QueryResult;

/// A node in the profile output tree, corresponding to one physical operator.
#[derive(Debug)]
pub struct ProfileNode {
    /// Operator name (e.g., "NodeScan", "Filter", "Expand").
    pub name: String,
    /// Display label (e.g., "(n:Person)", "(n.age > 25) [label-first]").
    pub label: String,
    /// Shared stats handle, populated during execution.
    pub stats: SharedProfileStats,
    /// Child nodes.
    pub children: Vec<ProfileNode>,
    /// `false` when no physical operator carries a counter for this logical
    /// node: the planner fused it into its parent (filter absorbed into an
    /// expansion, label filter absorbed into a scan, ...). Its rows are not
    /// measured on their own.
    pub mesure: bool,
}

/// An entry collected during physical planning, used to build the profile tree.
pub struct ProfileEntry {
    /// Identity of the logical operator (its address in the plan tree), see
    /// [`logical_key`].
    pub key: usize,
    /// Operator name from `Operator::name()`.
    pub name: String,
    /// Human-readable label from the logical operator.
    pub label: String,
    /// Shared stats handle passed to the `ProfiledOperator` wrapper.
    pub stats: SharedProfileStats,
}

impl ProfileEntry {
    /// Creates a new profile entry with fresh (empty) stats, for the logical
    /// operator `logical` (which must belong to the plan later given to
    /// [`build_profile_tree`]).
    pub fn new(
        logical: &LogicalOperator,
        name: &str,
        label: String,
    ) -> (Self, SharedProfileStats) {
        let stats = Arc::new(Mutex::new(ProfileStats::default()));
        let entry = Self {
            key: logical_key(logical),
            name: name.to_string(),
            label,
            stats: Arc::clone(&stats),
        };
        (entry, stats)
    }
}

/// Identity of a logical operator inside its plan tree: its address.
///
/// The planner and [`build_profile_tree`] both walk the same tree, so an
/// operator keeps its address between planning and tree building.
pub fn logical_key(logical: &LogicalOperator) -> usize {
    logical as *const LogicalOperator as usize
}

/// Builds a `ProfileNode` tree from the logical plan and the [`ProfileEntry`]
/// items collected during physical planning.
///
/// Entries are matched to logical operators by identity (see
/// [`logical_key`]), not by position: a logical operator that the planner
/// fused into its parent has no entry and is reported with `mesure == false`
/// instead of breaking the count.
pub fn build_profile_tree(
    logical: &LogicalOperator,
    entries: impl IntoIterator<Item = ProfileEntry>,
) -> ProfileNode {
    let mut par_cle: HashMap<usize, ProfileEntry> = HashMap::new();
    for entry in entries {
        par_cle.entry(entry.key).or_insert(entry);
    }
    construit_noeud(logical, &mut par_cle)
}

fn construit_noeud(
    logical: &LogicalOperator,
    par_cle: &mut HashMap<usize, ProfileEntry>,
) -> ProfileNode {
    let children: Vec<ProfileNode> = logical
        .children()
        .into_iter()
        .map(|child| construit_noeud(child, par_cle))
        .collect();

    match par_cle.remove(&logical_key(logical)) {
        Some(entry) => ProfileNode {
            name: entry.name,
            label: entry.label,
            stats: entry.stats,
            children,
            mesure: true,
        },
        None => ProfileNode {
            name: "fusionné".to_string(),
            label: logical.display_label(),
            stats: Arc::new(Mutex::new(ProfileStats::default())),
            children,
            mesure: false,
        },
    }
}

/// Formats a `ProfileNode` tree into a human-readable text representation
/// and wraps it in a `QueryResult` with a single "profile" column.
pub fn profile_result(root: &ProfileNode, total_time_ms: f64) -> QueryResult {
    let mut output = String::new();
    format_node(&mut output, root, 0);
    let _ = writeln!(output);
    let _ = write!(output, "Total time: {total_time_ms:.2}ms");

    QueryResult {
        columns: vec!["profile".to_string()],
        column_types: vec![LogicalType::String],
        rows: vec![vec![Value::String(output.into())]],
        execution_time_ms: Some(total_time_ms),
        rows_scanned: None,
        status_message: None,
        gql_status: obrain_common::utils::GqlStatus::SUCCESS,
    }
}

/// Recursively formats a profile node with indentation.
fn format_node(out: &mut String, node: &ProfileNode, depth: usize) {
    let indent = "  ".repeat(depth);

    // Compute self-time before locking stats (self_time_ns also locks).
    let self_time_ns = self_time_ns(node);
    let self_time_ms = self_time_ns as f64 / 1_000_000.0;

    if node.mesure {
        let rows_out = node.stats.lock().rows_out;
        let _ = writeln!(
            out,
            "{indent}{name} ({label})  rows={rows}  time={time:.2}ms",
            name = node.name,
            label = node.label,
            rows = rows_out,
            time = self_time_ms,
        );
    } else {
        let _ = writeln!(
            out,
            "{indent}{name} ({label})  rows=non mesuré (fusionné dans l'opérateur parent)",
            name = node.name,
            label = node.label,
        );
    }

    for child in &node.children {
        format_node(out, child, depth + 1);
    }
}

/// Computes self-time: wall time minus children's wall time.
fn self_time_ns(node: &ProfileNode) -> u64 {
    let own_time = node.stats.lock().time_ns;
    let child_time: u64 = node.children.iter().map(|c| c.stats.lock().time_ns).sum();
    own_time.saturating_sub(child_time)
}
