//! Primitive compositionnelle native — construction canonique + résolution des
//! références, posées sur les primitives existantes du graphe (ObrainDB +
//! GraphStore). Ce n'est PAS un second moteur de graphe : une connaissance est
//! un nœud (label `structure`) relié à ses participants (`member`), à ses
//! sous-connaissances référencées (`child`) et à ses relations orientées.
//!
//! Cinq notions d'identité DISTINCTES (jamais réduites à une seule clé) :
//!   1. identité d'ENTITÉ           = le `NodeId` lui-même ;
//!   2. identité de DÉFINITION STRUCTURELLE = `KnowledgeSpec::structural_key()`
//!      (membres + enfants + relations orientées, immuable pour une version) ;
//!   3. identité d'OCCURRENCE        = une référence `child` (cible + rôle),
//!      distinguée de l'identité de sa cible — plusieurs occurrences d'une même
//!      cible dans une composition forment un diamant légitime ;
//!   4. ÉQUIVALENCE selon une projection = `projection_members_key()` (membres
//!      seuls, relations/références ignorées) ;
//!   5. ÉQUIVALENCE sémantique       = déclarée explicitement (hors module).
//!
//! La résolution signale les références absentes, les types d'arête inattendus
//! et les cycles interdits ; un diamant (même sous-structure référencée deux
//! fois) n'est PAS un cycle.

use std::collections::{BTreeSet, HashMap};

use obrain_common::types::{EdgeId, NodeId, PropertyKey, Value};
use obrain_core::graph::Direction;

use crate::database::ObrainDB;

pub const LABEL_KNOWLEDGE: &str = "structure";
pub const EDGE_MEMBER: &str = "member";
pub const EDGE_CHILD: &str = "child";
pub const KEY_CANONICAL: &str = "canonical_key";

/// Relation ORIENTÉE entre participants (type + src → dst + rôle optionnel).
/// L'orientation fait partie de l'identité : A→B ≠ B→A.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Relation {
    pub rel_type: String,
    pub src: NodeId,
    pub dst: NodeId,
    pub role: Option<String>,
}

/// Spécification d'une connaissance : participants feuilles, sous-connaissances
/// référencées, relations orientées. L'ordre n'importe pas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnowledgeSpec {
    pub members: BTreeSet<NodeId>,
    pub children: BTreeSet<NodeId>,
    pub relations: BTreeSet<Relation>,
}

impl KnowledgeSpec {
    /// Clé de DÉFINITION STRUCTURELLE (immuable pour une version donnée) :
    /// inclut les relations orientées, donc A→B ≠ B→A à membres identiques.
    pub fn structural_key(&self) -> String {
        let m: Vec<String> = self.members.iter().map(|n| n.0.to_string()).collect();
        let c: Vec<String> = self.children.iter().map(|n| n.0.to_string()).collect();
        let r: Vec<String> = self
            .relations
            .iter()
            .map(|rel| {
                let role = rel.role.as_deref().unwrap_or("");
                format!("{}:{}->{}#{}", rel.rel_type, rel.src.0, rel.dst.0, role)
            })
            .collect();
        format!("m[{}]c[{}]r[{}]", m.join(","), c.join(","), r.join(","))
    }

    /// Clé de PROJECTION « membres seuls » (relations et références ignorées) :
    /// deux connaissances équivalentes sous cette projection partagent la clé.
    pub fn projection_members_key(&self) -> String {
        let m: Vec<String> = self.members.iter().map(|n| n.0.to_string()).collect();
        format!("m[{}]", m.join(","))
    }
}

/// Erreurs de résolution : la résolution ne doit jamais échouer silencieusement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    /// Une arête `child` pointe vers un nœud absent ou non-connaissance.
    MissingReference(NodeId),
    /// Une arête sortante d'un type inattendu (ni member ni child).
    InvalidEdgeType(String),
    /// Cycle de références interdit (la trace est le chemin emprunté).
    Cycle(Vec<NodeId>),
}

/// Interface compositionnelle sur le graphe natif.
pub struct Composition<'a> {
    db: &'a ObrainDB,
}

impl<'a> Composition<'a> {
    pub fn new(db: &'a ObrainDB) -> Self {
        Self { db }
    }

    /// Construction canonique : deux spécifications structurellement identiques
    /// → même nœud (déduplication par clé structurelle).
    pub fn construct(&self, spec: &KnowledgeSpec) -> NodeId {
        let key = spec.structural_key();
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
        // Relations : arêtes orientées ENTRE participants, type = rel_type.
        for rel in &spec.relations {
            self.db.create_edge(rel.src, rel.dst, &rel.rel_type);
        }
        id
    }

    /// Résolution des références : aplatit transitivement les `child` en
    /// participants feuilles. Signale référence absente, type inattendu, cycle.
    /// Un diamant (même cible référencée deux fois) est toléré.
    pub fn resolve(&self, id: NodeId) -> Result<BTreeSet<NodeId>, ResolveError> {
        let mut members = BTreeSet::new();
        let mut state: HashMap<NodeId, u8> = HashMap::new(); // 1 = en cours, 2 = fait
        let mut path = Vec::new();
        self.resolve_dfs(id, &mut members, &mut state, &mut path)?;
        Ok(members)
    }

    /// Clé structurelle d'une connaissance existante.
    pub fn structural_key_of(&self, id: NodeId) -> Option<String> {
        let node = self.db.get_node(id)?;
        node.properties
            .get(&PropertyKey::from(KEY_CANONICAL))
            .and_then(|v| v.as_str().map(|s| s.to_string()))
    }

    fn resolve_dfs(
        &self,
        id: NodeId,
        members: &mut BTreeSet<NodeId>,
        state: &mut HashMap<NodeId, u8>,
        path: &mut Vec<NodeId>,
    ) -> Result<(), ResolveError> {
        match state.get(&id) {
            Some(1) => return Err(ResolveError::Cycle(path.clone())),
            Some(2) => return Ok(()), // diamant : déjà résolu, pas un cycle
            _ => {}
        }
        state.insert(id, 1);
        path.push(id);
        for (dst, eid) in self.outgoing(id) {
            let edge = self
                .db
                .get_edge(eid)
                .ok_or(ResolveError::MissingReference(dst))?;
            match edge.edge_type.as_str() {
                EDGE_MEMBER => {
                    members.insert(dst);
                }
                EDGE_CHILD => {
                    if !self.is_knowledge(dst) {
                        return Err(ResolveError::MissingReference(dst));
                    }
                    self.resolve_dfs(dst, members, state, path)?;
                }
                other => return Err(ResolveError::InvalidEdgeType(other.to_string())),
            }
        }
        path.pop();
        state.insert(id, 2);
        Ok(())
    }

    fn is_knowledge(&self, id: NodeId) -> bool {
        self.db
            .get_node(id)
            .map(|n| n.labels.iter().any(|l| l.as_str() == LABEL_KNOWLEDGE))
            .unwrap_or(false)
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
