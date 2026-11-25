//! Primitive compositionnelle native — construction canonique + résolution
//! des références, posées sur les primitives existantes du graphe (ObrainDB +
//! GraphStore). Ce n'est PAS un second moteur de graphe : une connaissance est
//! un nœud (label `structure`) relié à ses participants (`member`) et à ses
//! sous-connaissances référencées (`child`).
//!
//! Contrat d'identité : l'identité canonique dérive de (membres triés, enfants
//! triés). Deux constructions sémantiquement identiques retrouvent le même
//! nœud (déduplication par clé canonique).

use std::collections::{BTreeSet, VecDeque};

use obrain_common::types::{EdgeId, NodeId, PropertyKey, Value};
use obrain_core::graph::Direction;

use crate::database::ObrainDB;

pub const LABEL_KNOWLEDGE: &str = "structure";
pub const EDGE_MEMBER: &str = "member";
pub const EDGE_CHILD: &str = "child";
pub const KEY_CANONICAL: &str = "canonical_key";

/// Spécification d'une connaissance : participants feuilles + sous-connaissances
/// référencées. L'ordre n'importe pas (`BTreeSet` → identité canonique stable).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnowledgeSpec {
    pub members: BTreeSet<NodeId>,
    pub children: BTreeSet<NodeId>,
}

impl KnowledgeSpec {
    /// Clé canonique (membres et enfants triés).
    pub fn canonical_key(&self) -> String {
        let m: Vec<String> = self.members.iter().map(|n| n.0.to_string()).collect();
        let c: Vec<String> = self.children.iter().map(|n| n.0.to_string()).collect();
        format!("m[{}]c[{}]", m.join(","), c.join(","))
    }
}

/// Interface compositionnelle sur le graphe natif.
pub struct Composition<'a> {
    db: &'a ObrainDB,
}

impl<'a> Composition<'a> {
    pub fn new(db: &'a ObrainDB) -> Self {
        Self { db }
    }

    /// Construction canonique : deux spécifications identiques → même nœud.
    pub fn construct(&self, spec: &KnowledgeSpec) -> NodeId {
        let key = spec.canonical_key();
        if let Some(existing) = self.find_by_key(&key) {
            return existing;
        }
        let id = self.db.create_node(&[LABEL_KNOWLEDGE]);
        self.db.set_node_property(id, KEY_CANONICAL, Value::from(key));
        for m in &spec.members {
            self.db.create_edge(id, *m, EDGE_MEMBER);
        }
        for c in &spec.children {
            self.db.create_edge(id, *c, EDGE_CHILD);
        }
        id
    }

    /// Résolution des références : aplatit les sous-connaissances référencées
    /// en leurs participants feuilles (parcours transitif des arêtes `child`).
    pub fn resolve(&self, id: NodeId) -> BTreeSet<NodeId> {
        let mut members = BTreeSet::new();
        let mut visited = BTreeSet::new();
        let mut queue = VecDeque::from([id]);
        while let Some(k) = queue.pop_front() {
            if !visited.insert(k) {
                continue;
            }
            for (dst, eid) in self.outgoing(k) {
                match self.db.get_edge(eid).map(|e| e.edge_type.to_string()) {
                    Some(t) if t == EDGE_MEMBER => {
                        members.insert(dst);
                    }
                    Some(t) if t == EDGE_CHILD => {
                        queue.push_back(dst);
                    }
                    _ => {}
                }
            }
        }
        members
    }

    /// Clé canonique d'une connaissance existante.
    pub fn canonical_key_of(&self, id: NodeId) -> Option<String> {
        let node = self.db.get_node(id)?;
        node.properties
            .get(&PropertyKey::from(KEY_CANONICAL))
            .and_then(|v| v.as_str().map(|s| s.to_string()))
    }

    fn outgoing(&self, id: NodeId) -> Vec<(NodeId, EdgeId)> {
        self.db.data_store().edges_from(id, Direction::Outgoing)
    }

    fn find_by_key(&self, key: &str) -> Option<NodeId> {
        let store = self.db.data_store();
        for id in store.node_ids() {
            let Some(node) = self.db.get_node(id) else { continue };
            if !node.labels.iter().any(|l| l.as_str() == LABEL_KNOWLEDGE) {
                continue;
            }
            if let Some(v) = node.properties.get(&PropertyKey::from(KEY_CANONICAL)) {
                if v.as_str() == Some(key) {
                    return Some(id);
                }
            }
        }
        None
    }
}
