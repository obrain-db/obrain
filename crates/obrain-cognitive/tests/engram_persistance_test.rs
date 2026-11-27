//! Persistance JSON via `CognitiveStorage` : write-through, rechargement
//! (`load_from_graph`), réconciliation des ids et de la révision, et erreurs de
//! lecture. Couverture #414 — périmètre A : persistance cognitive.

#![cfg(feature = "engram")]

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use obrain_cognitive::engram::traits::{
    CognitiveEdge, CognitiveFilter, CognitiveNode, CognitiveStorage,
};
use obrain_cognitive::engram::{Engram, EngramId, EngramStore};
use obrain_common::types::{EdgeId, NodeId, Value};

/// Miroir des constantes privées de `store.rs` — seules ces clés importent
/// pour le contrat de persistance.
const ENGRAM_LABEL: &str = ":Engram";
const PROP_ENGRAM_DATA: &str = "_engram_data";

/// `CognitiveStorage` en mémoire : un `Vec<CognitiveNode>` sous verrou.
/// Suffit à exercer le write-through (`insert`) et la relecture
/// (`load_from_graph`), y compris les nœuds illisibles.
struct MemStorage {
    next_node: AtomicU64,
    nodes: Mutex<Vec<CognitiveNode>>,
}

impl MemStorage {
    fn new() -> Self {
        Self {
            next_node: AtomicU64::new(1000),
            nodes: Mutex::new(Vec::new()),
        }
    }

    /// Injecte un nœud brut (nœud corrompu / legacy) sans passer par
    /// `insert` — simule ce qu'un ancien schéma a pu laisser en base.
    fn inject(&self, label: &str, properties: HashMap<String, Value>) -> NodeId {
        let id = NodeId(self.next_node.fetch_add(1, Ordering::Relaxed));
        self.nodes.lock().unwrap().push(CognitiveNode {
            id,
            label: label.to_string(),
            properties,
        });
        id
    }
}

impl CognitiveStorage for MemStorage {
    fn create_node(&self, label: &str, properties: &HashMap<String, Value>) -> NodeId {
        let id = NodeId(self.next_node.fetch_add(1, Ordering::Relaxed));
        self.nodes.lock().unwrap().push(CognitiveNode {
            id,
            label: label.to_string(),
            properties: properties.clone(),
        });
        id
    }

    fn create_edge(
        &self,
        _from: NodeId,
        _to: NodeId,
        _rel_type: &str,
        _properties: &HashMap<String, Value>,
    ) -> EdgeId {
        EdgeId(0)
    }

    fn query_nodes(&self, label: &str, _filter: Option<&CognitiveFilter>) -> Vec<CognitiveNode> {
        self.nodes
            .lock()
            .unwrap()
            .iter()
            .filter(|n| n.label == label)
            .cloned()
            .collect()
    }

    fn update_node(&self, _id: NodeId, _properties: &HashMap<String, Value>) {}

    fn delete_node(&self, _id: NodeId) {}

    fn delete_edge(&self, _id: EdgeId) {}

    fn query_edges(&self, _from: NodeId, _rel_type: &str) -> Vec<CognitiveEdge> {
        Vec::new()
    }
}

fn engram(id: u64, nodes: &[(u64, f64)]) -> Engram {
    Engram::new(
        EngramId(id),
        nodes.iter().map(|(n, w)| (NodeId(*n), *w)).collect(),
    )
}

// ────────────────────────────────────────────────────────────────────────────
// Rechargement + réconciliation
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn load_from_graph_recharge_les_engrammes_apres_reouverture() {
    let storage = Arc::new(MemStorage::new());

    // Session 1 : write-through vers le storage.
    {
        let store = EngramStore::new(Some(Arc::clone(&storage) as Arc<dyn CognitiveStorage>));
        store.insert(engram(7, &[(1, 1.0), (2, 0.5)]));
        store.insert(engram(3, &[(3, 1.0)]));
    } // store1 dropped — cache perdu.

    // Session 2 : réouverture depuis le même storage.
    let store2 = EngramStore::new(Some(storage as Arc<dyn CognitiveStorage>));
    let loaded = store2.load_from_graph();
    assert_eq!(
        loaded, 2,
        "les deux engrammes persistés doivent être rechargés"
    );

    assert_eq!(store2.count(), 2, "le cache est réhydraté");
    assert!(store2.get(EngramId(7)).is_some());
    assert!(store2.get(EngramId(3)).is_some());

    let e7 = store2.get(EngramId(7)).unwrap();
    assert_eq!(
        e7.ensemble.len(),
        2,
        "l'ensemble est reconstitué fidèlement"
    );
    // Le rappel par nœud fonctionne après chargement (réconciliation du cache).
    assert_eq!(store2.find_by_node(NodeId(1)), vec![EngramId(7)]);
}

#[test]
fn load_from_graph_reconcilie_le_compteur_d_ids() {
    let storage = Arc::new(MemStorage::new());
    {
        let store = EngramStore::new(Some(Arc::clone(&storage) as Arc<dyn CognitiveStorage>));
        store.insert(engram(7, &[(1, 1.0)]));
    }
    let store2 = EngramStore::new(Some(storage as Arc<dyn CognitiveStorage>));
    store2.load_from_graph();
    // L'id suivant est AU-DESSUS du max persisté (7) : pas de collision.
    assert_eq!(
        store2.next_id(),
        EngramId(8),
        "le compteur d'ids est réconcilié"
    );
}

#[test]
fn load_from_graph_incremente_la_revision() {
    let storage = Arc::new(MemStorage::new());
    let store = EngramStore::new(Some(storage as Arc<dyn CognitiveStorage>));
    let rev_avant = store.revision();
    store.load_from_graph();
    // UN CHARGEMENT EST UNE MUTATION : la révision le dit (invalidation des
    // caches de motifs / position — voir le commentaire dans store.rs).
    assert_eq!(store.revision(), rev_avant + 1);
}

#[test]
fn load_from_graph_sans_storage_retourne_zero() {
    let store = EngramStore::new(None);
    assert_eq!(store.load_from_graph(), 0);
}

#[test]
fn load_from_graph_ignore_les_noeuds_illisibles() {
    let storage = Arc::new(MemStorage::new());

    // Un nœud valide (écrit par le write-through).
    {
        let store = EngramStore::new(Some(Arc::clone(&storage) as Arc<dyn CognitiveStorage>));
        store.insert(engram(1, &[(10, 1.0)]));
    }

    // Trois nœuds illisibles injectés directement :
    // 1. JSON corrompu (désérialisation en échec).
    let mut props = HashMap::new();
    props.insert(
        PROP_ENGRAM_DATA.to_string(),
        Value::String("pas du json".into()),
    );
    storage.inject(ENGRAM_LABEL, props);
    // 2. `_engram_data` du mauvais type (pas une String).
    let mut props = HashMap::new();
    props.insert(PROP_ENGRAM_DATA.to_string(), Value::Int64(42));
    storage.inject(ENGRAM_LABEL, props);
    // 3. `_engram_data` absent.
    storage.inject(ENGRAM_LABEL, HashMap::new());

    let store2 = EngramStore::new(Some(storage as Arc<dyn CognitiveStorage>));
    let loaded = store2.load_from_graph();
    assert_eq!(
        loaded, 1,
        "seul le nœud valide est chargé ; les illisibles sont ignorés"
    );
    assert_eq!(store2.count(), 1);
    assert!(store2.get(EngramId(1)).is_some());
    // Les nœuds illisibles n'ont pas pollué le compteur d'ids.
    assert_eq!(store2.next_id(), EngramId(2));
}

// ────────────────────────────────────────────────────────────────────────────
// Projection dérivée (signature spectrale) — reconstruction sans mutation
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn rehydrate_signature_reconstruit_la_projection_sans_reviser_l_engramme() {
    let store = EngramStore::new(None);
    let id = store.next_id();
    store.insert(engram(id.0, &[(1, 1.0), (2, 0.5)]));
    let rev_avant = store.revision();

    let sig = vec![0.1, 0.2, 0.3];
    store.rehydrate_signature(id, sig.clone());
    // Projection DÉRIVÉE : cache seul, pas de write-through durable.
    assert_eq!(store.get(id).unwrap().spectral_signature, sig);
    // Invalide le cache Hopfield (révision +1) sans autre mutation.
    assert_eq!(store.revision(), rev_avant + 1);
}

#[test]
fn rehydrate_signature_absente_est_un_noop() {
    let store = EngramStore::new(None);
    let rev_avant = store.revision();
    store.rehydrate_signature(EngramId(999_999), vec![9.9]);
    assert_eq!(store.revision(), rev_avant, "id absent : aucune révision");
}
