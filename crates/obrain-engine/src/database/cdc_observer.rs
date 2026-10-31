//! Observateur de mutations substrate → journal CDC (2026-09-05, CDC×GQL).
//!
//! Le CDC n'enregistrait que les écritures passant par l'API moteur (`crud`) ;
//! les mutations GQL/executor (CREATE/MERGE de `import_events`, chemin réel du
//! runtime) écrivent directement dans `SubstrateStore` sans enregistrement →
//! `search_delta`/`search_surprise` vides sur les stores runtime. Cet
//! adaptateur, installé sur le store substrate (feature `cdc`), reçoit chaque
//! création effective (nœud/arête) et l'enregistre dans `CdcLog`
//! (ChangeEvent kind=Create, before/after absents — la granularité minimale
//! du diff/surprise ; les updates de propriétés restent portés par `crud`).

use std::sync::Arc;
use web_time::{SystemTime, UNIX_EPOCH};

use obrain_common::types::{EdgeId, EpochId, NodeId};

use crate::cdc::{CdcLog, ChangeEvent, ChangeKind, EntityId};

/// Adaptateur engine : `SubstrateStore::StoreMutationObserver` → `CdcLog`.
pub struct CdcObserver {
    /// ── UNE RÉFÉRENCE FAIBLE, ET C'EST UN CORRECTIF DE FUITE ────────────────
    /// Défaut mesuré le 2026-09-16 (`second_writer_exclusif_refuse_pendant_
    /// shared_write`, visible seulement avec la feature `cdc`) : l'observateur
    /// est POSÉ SUR LE STORE (`typed.set_mutation_observer`), et il tenait un
    /// `Arc<CdcLog>` fort. Or le CDC tient un `CdcPersist`, qui tient le
    /// `Arc<SubstrateStore>`. Cycle complet :
    ///
    /// ```text
    /// store ──(observateur)──► CdcObserver ──► CdcLog ──► CdcPersist ──► store
    /// ```
    ///
    /// Conséquence MESURÉE, et pas seulement théorique : après la chute d'une
    /// base, **tous ses fichiers restaient ouverts** (`/proc/self/fd` :
    /// `substrate.meta`, `substrate.wal`, les zones de propriétés…) et son
    /// `flock` EXCLUSIF restait tenu pour la vie du processus. Le registre des
    /// verrous partageait donc cette part exclusive à la réouverture : un
    /// « shared-write » redevenait un écrivain exclusif, et un SECOND écrivain
    /// exclusif était accepté — exactement l'invariant I4 que ce fichier-là
    /// protège.
    ///
    /// Une référence faible coupe le cycle : l'observateur n'a pas à empêcher
    /// la mort de la base qu'il observe. Quand la base est partie, l'événement
    /// n'a plus personne à informer — on l'ignore, ce qui est la seule conduite
    /// honnête.
    log: std::sync::Weak<CdcLog>,
}

impl CdcObserver {
    /// Crée l'observateur branché sur le journal CDC de la base.
    pub fn new(log: Arc<CdcLog>) -> Self {
        Self {
            log: Arc::downgrade(&log),
        }
    }

    fn record(&self, entity_id: EntityId, epoch: u64) {
        let event = ChangeEvent {
            entity_id,
            kind: ChangeKind::Create,
            epoch: EpochId(epoch),
            timestamp: now_millis(),
            before: None,
            after: None,
        };
        if let Some(log) = self.log.upgrade() {
            log.record(event);
        }
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl obrain_substrate::StoreMutationObserver for CdcObserver {
    fn node_created(&self, id: NodeId, epoch: u64) {
        self.record(EntityId::Node(id), epoch);
    }

    fn edge_created(&self, _id: EdgeId, _src: NodeId, _dst: NodeId, epoch: u64) {
        self.record(EntityId::Edge(_id), epoch);
    }
}
