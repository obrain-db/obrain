//! Bridge between mutation operators and the transaction manager's write tracking.

use std::sync::Arc;

use obrain_common::types::{EdgeId, NodeId, TransactionId};
use obrain_core::execution::operators::{OperatorError, WriteTracker};

use super::TransactionManager;

/// Implements [`WriteTracker`] by forwarding to
/// [`TransactionManager::record_write_scoped`] avec le scope du store
/// (RFC multi-writer B1).
///
/// Created by the planner when a transaction is active, and passed to each
/// mutation operator so it can record writes for conflict detection. Le scope
/// = `GraphStoreMut::graph_scope()` du store que la requête mute : deux
/// transactions sur des graphes différents n'enregistrent jamais la même
/// `(scope, entité)` → pas de faux conflit inter-graphes.
pub struct TransactionWriteTracker {
    manager: Arc<TransactionManager>,
    scope: u64,
}

impl TransactionWriteTracker {
    /// Creates a new write tracker backed by the given transaction manager,
    /// scoped to `scope` (identité du graphe/store écrit).
    pub fn new(manager: Arc<TransactionManager>, scope: u64) -> Self {
        Self { manager, scope }
    }
}

impl WriteTracker for TransactionWriteTracker {
    fn record_node_write(
        &self,
        transaction_id: TransactionId,
        node_id: NodeId,
    ) -> Result<(), OperatorError> {
        self.manager
            .record_write_scoped(self.scope, transaction_id, node_id)
            .map_err(|e| OperatorError::WriteConflict(e.to_string()))
    }

    fn record_edge_write(
        &self,
        transaction_id: TransactionId,
        edge_id: EdgeId,
    ) -> Result<(), OperatorError> {
        self.manager
            .record_write_scoped(self.scope, transaction_id, edge_id)
            .map_err(|e| OperatorError::WriteConflict(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obrain_common::types::NodeId;

    fn tracker() -> (Arc<TransactionManager>, TransactionWriteTracker) {
        let manager = Arc::new(TransactionManager::new());
        let pont = TransactionWriteTracker::new(Arc::clone(&manager), 0);
        (manager, pont)
    }

    /// Le pont doit remonter un conflit write-write en `OperatorError::WriteConflict`
    /// quand une AUTRE transaction active a déjà écrit le même nœud
    /// (first-writer-wins, RFC multi-writer I8).
    #[test]
    fn conflit_write_write_remonte_par_le_pont() {
        let (manager, pont) = tracker();
        let tx1 = manager.begin();
        let tx2 = manager.begin();
        assert_eq!(manager.active_count(), 2);

        // 1er writer : autorisé.
        pont.record_node_write(tx1, NodeId::new(7))
            .expect("1er writer OK");

        // 2e writer sur le même nœud : conflit remonté en WriteConflict.
        let err = pont.record_node_write(tx2, NodeId::new(7)).unwrap_err();
        assert!(
            matches!(err, OperatorError::WriteConflict(_)),
            "attendu WriteConflict, got {err:?}"
        );

        // Un nœud DIFFÉRENT par le 2e writer reste autorisé (pas de sur-détection).
        pont.record_node_write(tx2, NodeId::new(8))
            .expect("nœud distinct OK");
    }

    /// Le pont ne doit pas masquer un état invalide (écriture hors transaction
    /// active) : le manager le refuse et le pont le propage.
    #[test]
    fn ecriture_sans_transaction_active_est_refusee() {
        let (manager, pont) = tracker();
        // Aucune transaction commencée : record_write échoue (transaction absente).
        let err = pont.record_node_write(TransactionId::new(999), NodeId::new(1));
        assert!(err.is_err(), "écriture hors transaction refusée");
    }

    /// Après commit, une écriture sur la transaction committée est un état
    /// invalide — le pont le propage (pas de WriteConflict silencieux).
    #[test]
    fn ecriture_apres_commit_est_refusee() {
        let (manager, pont) = tracker();
        let tx = manager.begin();
        pont.record_node_write(tx, NodeId::new(1))
            .expect("avant commit OK");
        manager.commit(tx).expect("commit");
        assert!(
            pont.record_node_write(tx, NodeId::new(2)).is_err(),
            "écriture après commit refusée"
        );
    }
}
