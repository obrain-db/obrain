//! Observateur de mutations substrate → `MutationBus` (câblage cognitif, 2026-11-12).
//!
//! ## Le trou que ce fichier bouche
//!
//! Le moteur cognitif (énergie, synapses, fabric, kernel…) est construit par
//! `with_substrate_store` et `with_config`, et ses listeners sont enregistrés
//! sur un `Scheduler` abonné à un `MutationBus`. Mais **personne ne publiait
//! sur ce bus** : le `post_commit_hook` de `Session` (prévu pour drainer un
//! `InstrumentedStore`) n'avait aucun appelant, et le store substrat n'était
//! jamais enveloppé. Mesuré avant ce correctif : énergie 0, synapses 0 après
//! n'importe quelle mutation GQL par l'API publique, alors qu'un câblage
//! manuel (`InstrumentedStore` + `drain_pending` + `publish`) faisait monter
//! l'énergie et apparaître des synapses — le moteur marchait, le fil manquait.
//!
//! ## Pourquoi un observateur substrat et pas `InstrumentedStore`
//!
//! Depuis T17 le substrat est l'unique backend, et il expose déjà
//! [`StoreMutationObserver`](obrain_substrate::StoreMutationObserver) — le
//! point par lequel passent le CDC (`CdcObserver`) et la persistance des index
//! vectoriels. S'y brancher couvre **toutes** les origines d'écriture (sessions
//! GQL, `graph_store_mut()` direct, import) sans envelopper les 143 méthodes du
//! trait `GraphStoreMut`, et sans dépendre du chemin de commit de la session.
//!
//! ## Ce que l'événement porte
//!
//! Des instantanés **minimaux** : identifiants seulement (ni étiquettes ni
//! propriétés). Tous les listeners du moteur (`EnergyListener`,
//! `SynapseListener`, `FabricListener`, `KernelListener`, `CoChangeDetector`)
//! n'extraient que les ids et `src`/`dst` ; hydrater les propriétés à chaque
//! mutation coûterait une lecture par événement sur le plan chaud pour rien.
//!
//! ## Plan chaud jamais bloqué
//!
//! `MutationBus::publish` est un `broadcast::send` : non bloquant, borné par
//! la capacité du canal. Le dispatch aux listeners se fait dans la tâche du
//! `Scheduler` (asynchrone, par lots). Si le scheduler prend du retard au-delà
//! de la capacité, les événements les plus anciens sont **perdus** (`Lagged`),
//! comptés par `Scheduler::events_lost` — jamais une attente côté écrivain.
//!
//! ## Référence faible
//!
//! Même raison que `CdcObserver` : l'observateur est posé sur le store, qui
//! vit aussi longtemps que la base ; tenir le bus en `Arc` fort ne crée pas de
//! cycle ici (le bus ne référence pas le store), mais un `Weak` garantit qu'un
//! store survivant à sa base (handle externe) ne publie pas dans le vide.

use std::sync::{Arc, Weak};

use obrain_common::types::{EdgeId, NodeId};
use obrain_reactive::{EdgeSnapshot, MutationBus, MutationEvent, NodeSnapshot};

/// Adaptateur : `SubstrateStore::StoreMutationObserver` → `MutationBus`.
pub struct CognitiveObserver {
    bus: Weak<MutationBus>,
}

impl CognitiveObserver {
    /// Crée l'observateur branché sur le bus du moteur cognitif de la base.
    #[must_use]
    pub fn new(bus: &Arc<MutationBus>) -> Self {
        Self {
            bus: Arc::downgrade(bus),
        }
    }

    fn publier(&self, event: MutationEvent) {
        if let Some(bus) = self.bus.upgrade() {
            // `false` = aucun abonné (moteur sans listener) : chemin zéro coût.
            let _ = bus.publish(event);
        }
    }

    fn noeud(id: NodeId) -> NodeSnapshot {
        NodeSnapshot {
            id,
            labels: Default::default(),
            properties: Vec::new(),
        }
    }

    fn arete(id: EdgeId, src: NodeId, dst: NodeId) -> EdgeSnapshot {
        EdgeSnapshot {
            id,
            src,
            dst,
            edge_type: Default::default(),
            properties: Vec::new(),
        }
    }
}

impl std::fmt::Debug for CognitiveObserver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CognitiveObserver")
            .field("bus_vivant", &(self.bus.strong_count() > 0))
            .finish()
    }
}

/// Ce que la cognition écrit ELLE-MÊME ne doit jamais la relancer.
///
/// Les stores cognitifs persistent par des arêtes `SYNAPSE` et des
/// propriétés `_cog_*`, `_kernel_embedding`, `_hilbert_features`,
/// `_st_embedding`… Si ces écritures publiaient des événements, chaque
/// renforcement Hebbien créerait une arête, donc un événement, donc un
/// nouveau renforcement : une boucle sans fin (vue à l'import : la
/// quiescence n'arrivait jamais). Convention retenue : toute propriété dont
/// la clé commence par `_` et toute arête de type `SYNAPSE` (ou commençant
/// par `_`) sont de la maintenance interne, pas une mutation du graphe
/// de l'utilisateur.
fn est_interne_propriete(key: &str) -> bool {
    key.starts_with('_')
}

fn est_interne_arete(edge_type: &str) -> bool {
    edge_type == obrain_cognitive::synapse::SYNAPSE_EDGE_TYPE || edge_type.starts_with('_')
}

impl obrain_substrate::StoreMutationObserver for CognitiveObserver {
    fn node_created(&self, id: NodeId, _epoch: u64) {
        self.publier(MutationEvent::NodeCreated {
            node: Self::noeud(id),
        });
    }

    /// Sans le type, impossible d'écarter les arêtes internes : on attend
    /// `edge_created_typed`, notifié juste après.
    fn edge_created(&self, _id: EdgeId, _src: NodeId, _dst: NodeId, _epoch: u64) {}

    fn edge_created_typed(&self, id: EdgeId, src: NodeId, dst: NodeId, edge_type: &str, _epoch: u64) {
        if est_interne_arete(edge_type) {
            return;
        }
        self.publier(MutationEvent::EdgeCreated {
            edge: Self::arete(id, src, dst),
        });
    }

    fn node_deleted(&self, id: NodeId) {
        self.publier(MutationEvent::NodeDeleted {
            node: Self::noeud(id),
        });
    }

    fn node_property_set(&self, id: NodeId, key: &str) {
        if est_interne_propriete(key) {
            return;
        }
        self.publier(MutationEvent::NodeUpdated {
            before: Self::noeud(id),
            after: Self::noeud(id),
        });
    }

    fn node_property_removed(&self, id: NodeId, key: &str) {
        if est_interne_propriete(key) {
            return;
        }
        self.publier(MutationEvent::NodeUpdated {
            before: Self::noeud(id),
            after: Self::noeud(id),
        });
    }

    fn edge_deleted(&self, id: EdgeId, src: NodeId, dst: NodeId, edge_type: &str) {
        if est_interne_arete(edge_type) {
            return;
        }
        self.publier(MutationEvent::EdgeDeleted {
            edge: Self::arete(id, src, dst),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn les_ecritures_internes_de_la_cognition_ne_sont_pas_des_mutations() {
        assert!(est_interne_arete("SYNAPSE"));
        assert!(est_interne_arete("_engram"));
        assert!(!est_interne_arete("LINKS_TO"));
        assert!(est_interne_propriete("_cog_energy"));
        assert!(est_interne_propriete("_kernel_embedding"));
        assert!(!est_interne_propriete("name"));
    }
}
