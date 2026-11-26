//! Primitive compositionnelle native — construction canonique + résolution des
//! références, posées sur les primitives existantes du graphe (ObrainDB +
//! GraphStore). Ce n'est PAS un second moteur de graphe : une connaissance est
//! un nœud (label `structure`) relié à ses participants (`member`), à ses
//! sous-connaissances référencées (`child`) et à ses relations orientées.
//!
//! # Périmètre des relations constitutives
//! La résolution suit UNIQUEMENT les arêtes `member` et `child`. Toute autre
//! arête sortante d'une connaissance est HORS périmètre et ignorée (elle n'est
//! pas constitutive de la composition) — on ne rejette pas arbitrairement.
//!
//! # Cinq notions d'identité — état d'implémentation (limites exactes)
//! 1. identité d'ENTITÉ = le `NodeId` — implémentée ;
//! 2. identité de DÉFINITION STRUCTURELLE = `structural_key()` — implémentée
//!    (encodage canonique versionné, non ambigu, injectif) ; l'IMMUTABILITÉ
//!    n'est pas encore imposée par le moteur (aucune barrière d'écriture) ;
//! 3. identité d'OCCURRENCE — `children` est une MULTIPLICITÉ
//!    `BTreeMap<ChildRef, usize>` : deux occurrences d'une même cible (même
//!    rôle ou rôles différents) SONT distinguées (rôle sur l'arête `child`,
//!    multiplicité = nombre d'arêtes) ; la projection ensembliste
//!    (`projection_children`) les réduit volontairement à une cible ;
//! 4. ÉQUIVALENCE selon une projection = `projection_members_key()` — membres
//!    DIRECTS seulement, pas la fermeture transitive (limite) ;
//! 5. ÉQUIVALENCE sémantique — déclarée explicitement, hors module.
//!
//! # Encodage et migration
//! `structural_key` est versionné `v3` (membres, enfants cible+rôle+multiplicité,
//! relations type+src+dst+rôle+multiplicité). Politique de compatibilité :
//! - un nœud encore en encodage HISTORIQUE `v2` (children sans rôle, sans
//!   multiplicité) est RECONNU par `KnowledgeSpec::legacy_v2_key` et MIGRÉ
//!   (clé réécrite en v3) à la première lecture — une même définition n'est
//!   donc pas dupliquée par une construction ultérieure ;
//! - deux définitions historiques DISTINCTES ont des clés v3 distinctes et ne
//!   fusionnent pas ;
//! - une clé qui n'est ni v3 ni l'encodage v2 de la structure est REFUSÉE
//!   (`KeyDivergence`) — jamais réécrite en silence.
//!
//! Ne pas confondre cette identité structurelle locale (fondée sur des `NodeId`)
//! avec une identité durable entre répliques (hors de ce module).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use obrain_common::types::{EdgeId, NodeId, PropertyKey, Value};
use obrain_core::graph::Direction;

use crate::database::ObrainDB;

/// Label des nœuds de connaissance.
pub const LABEL_KNOWLEDGE: &str = "structure";
/// Type d'arête reliant une connaissance à ses participants.
pub const EDGE_MEMBER: &str = "member";
/// Type d'arête reliant une connaissance à ses sous-connaissances référencées.
pub const EDGE_CHILD: &str = "child";
/// Clé de propriété stockant la clé canonique d'une connaissance.
pub const KEY_CANONICAL: &str = "canonical_key";
/// Clé de propriété stockant le rôle d'une relation.
pub const KEY_ROLE: &str = "role";
/// Label des nœuds d'occurrence relationnelle (réification).
pub const LABEL_RELATION: &str = "relation_occurrence";
/// Arête connaissance -> occurrence relationnelle (appartenance).
pub const EDGE_RELATION: &str = "relation";
/// Arête occurrence -> participant source.
pub const EDGE_SRC: &str = "src";
/// Arête occurrence -> participant destination.
pub const EDGE_DST: &str = "dst";
/// Clé de propriété stockant le type d'une relation.
pub const KEY_REL_TYPE: &str = "rel_type";
/// Clé de propriété d'ÉTAT de construction (`pending` / `complete`).
pub const KEY_STATE: &str = "construction_state";
/// Profondeur maximale de résolution (garde anti-épuisement de pile).
pub const MAX_RESOLVE_DEPTH: usize = 1024;

/// Relation ORIENTÉE entre participants (type + src → dst + rôle optionnel).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Relation {
    /// Type de la relation (ex. « calls », « knows »).
    pub rel_type: String,
    /// Source orientée de la relation.
    pub src: NodeId,
    /// Destination orientée de la relation.
    pub dst: NodeId,
    /// Rôle optionnel de l'occurrence de cette relation.
    pub role: Option<String>,
}

/// Référence d'OCCURRENCE vers une sous-connaissance : cible + rôle contextuel.
/// Deux références vers la même cible avec des rôles différents sont deux
/// occurrences distinctes (identité d'occurrence) ; la projection ensembliste
/// (`KnowledgeSpec::projection_children`) les réduit à une seule cible.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ChildRef {
    /// Sous-connaissance référencée.
    pub target: NodeId,
    /// Rôle contextuel de l'occurrence (ex. « sujet », « objet »).
    pub role: Option<String>,
}

/// Spécification d'une connaissance : participants feuilles, sous-connaissances
/// référencées et relations orientées. L'ordre n'importe pas ; sous-connaissances
/// et relations portent une MULTIPLICITÉ (occurrences).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnowledgeSpec {
    /// Participants feuilles.
    pub members: BTreeSet<NodeId>,
    /// Sous-connaissances référencées : MULTIPLICITÉ de (cible, rôle). Deux
    /// occurrences de même cible ET même rôle ont une multiplicité de 2 — elles
    /// ne fusionnent pas. Ce champ porte la CARDINALITÉ (combien d'occurrences),
    /// PAS l'identité INDIVIDUELLE d'une occurrence : une occurrence n'a pas
    /// d'identifiant propre au-delà de son appartenance à la multiplicité. La
    /// projection ensembliste (`projection_children`) ne retient que les cibles.
    pub children: BTreeMap<ChildRef, usize>,
    /// Relations orientées : MULTIPLICITÉ de (type, src, dst, rôle). Deux
    /// occurrences identiques ont une multiplicité de 2.
    pub relations: BTreeMap<Relation, usize>,
}

/// Encodage canonique : chaque champ de longueur variable est préfixé par sa
/// longueur en octets (`<len>|<contenu>`). Aucun séparateur ambigu, injectif.
fn enc_str(out: &mut String, s: &str) {
    out.push_str(&s.len().to_string());
    out.push('|');
    out.push_str(s);
}

/// Option<string> : `n|` = None, `s|` + longueur-préfixé = Some. `n`/`s` ne sont
/// pas des chiffres, donc sans ambiguïté avec une longueur.
fn enc_opt(out: &mut String, o: Option<&str>) {
    match o {
        None => out.push_str("n|"),
        Some(s) => {
            out.push_str("s|");
            enc_str(out, s);
        }
    }
}

impl KnowledgeSpec {
    /// Clé de DÉFINITION STRUCTURELLE (v3, injective) : membres, enfants
    /// (cible+rôle+multiplicité) et relations (type+src+dst+rôle+multiplicité).
    /// A→B ≠ B→A ; None ≠ Some("") ; multiplicité distincte ⇒ clé distincte.
    pub fn structural_key(&self) -> String {
        let mut k = String::from("v3|");
        enc_str(&mut k, &self.members.len().to_string());
        for m in &self.members {
            enc_str(&mut k, &m.0.to_string());
        }
        enc_str(&mut k, &self.children.len().to_string());
        for (c, count) in &self.children {
            enc_str(&mut k, &c.target.0.to_string());
            enc_opt(&mut k, c.role.as_deref());
            enc_str(&mut k, &count.to_string());
        }
        enc_str(&mut k, &self.relations.len().to_string());
        for (r, count) in &self.relations {
            enc_str(&mut k, &r.rel_type);
            enc_str(&mut k, &r.src.0.to_string());
            enc_str(&mut k, &r.dst.0.to_string());
            enc_opt(&mut k, r.role.as_deref());
            enc_str(&mut k, &count.to_string());
        }
        k
    }

    /// Clé au format v2 ORIGINAL (avant rôle de child ET avant multiplicité) :
    /// enfants = cible seule, relations sans multiplicité. `None` si la structure
    /// n'est PAS représentable ainsi (un enfant porte un rôle, ou une
    /// multiplicité ≠ 1) : on ne l'accepte donc pas comme historique.
    pub fn legacy_v2_original_key(&self) -> Option<String> {
        if self.children.keys().any(|c| c.role.is_some())
            || self.children.values().any(|&n| n != 1)
            || self.relations.values().any(|&n| n != 1)
        {
            return None;
        }
        let mut k = String::from("v2|");
        enc_str(&mut k, &self.members.len().to_string());
        for m in &self.members {
            enc_str(&mut k, &m.0.to_string());
        }
        enc_str(&mut k, &self.children.len().to_string());
        for c in self.children.keys() {
            enc_str(&mut k, &c.target.0.to_string());
        }
        enc_str(&mut k, &self.relations.len().to_string());
        for r in self.relations.keys() {
            enc_str(&mut k, &r.rel_type);
            enc_str(&mut k, &r.src.0.to_string());
            enc_str(&mut k, &r.dst.0.to_string());
            enc_opt(&mut k, r.role.as_deref());
        }
        Some(k)
    }

    /// Clé au format v2 INTERMÉDIAIRE (720e189 : rôle de child, SANS
    /// multiplicité). `None` si une multiplicité ≠ 1.
    pub fn legacy_v2_intermediate_key(&self) -> Option<String> {
        if self.children.values().any(|&n| n != 1) || self.relations.values().any(|&n| n != 1) {
            return None;
        }
        let mut k = String::from("v2|");
        enc_str(&mut k, &self.members.len().to_string());
        for m in &self.members {
            enc_str(&mut k, &m.0.to_string());
        }
        enc_str(&mut k, &self.children.len().to_string());
        for c in self.children.keys() {
            enc_str(&mut k, &c.target.0.to_string());
            enc_opt(&mut k, c.role.as_deref());
        }
        enc_str(&mut k, &self.relations.len().to_string());
        for r in self.relations.keys() {
            enc_str(&mut k, &r.rel_type);
            enc_str(&mut k, &r.src.0.to_string());
            enc_str(&mut k, &r.dst.0.to_string());
            enc_opt(&mut k, r.role.as_deref());
        }
        Some(k)
    }

    /// Clé de PROJECTION « membres directs » (relations et références ignorées).
    pub fn projection_members_key(&self) -> String {
        let mut k = String::from("pm|");
        enc_str(&mut k, &self.members.len().to_string());
        for m in &self.members {
            enc_str(&mut k, &m.0.to_string());
        }
        k
    }

    /// PROJECTION ensembliste des sous-connaissances : les cibles seules, rôles
    /// et multiplicités ignorés. Deux occurrences d'une même cible se réduisent
    /// à UNE cible — perte documentée, par opposition à la représentation
    /// constitutive (`children`).
    pub fn projection_children(&self) -> BTreeSet<NodeId> {
        self.children.keys().map(|c| c.target).collect()
    }

    /// Nombre TOTAL d'occurrences de sous-connaissances (somme des multiplicités).
    pub fn children_total(&self) -> usize {
        self.children.values().sum()
    }

    /// Nombre TOTAL d'occurrences relationnelles (somme des multiplicités).
    pub fn relations_total(&self) -> usize {
        self.relations.values().sum()
    }
}

/// Erreurs de résolution : la résolution ne doit jamais échouer silencieusement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    /// La racine n'existe pas.
    RootMissing(NodeId),
    /// La racine existe mais n'est pas une connaissance (label absent).
    RootNotKnowledge(NodeId),
    /// Une arête `child` pointe vers un nœud inexistant.
    MissingReference(NodeId),
    /// Une arête `child` pointe vers un nœud existant mais non-connaissance.
    NotKnowledge(NodeId),
    /// Une arête `member` pointe vers un nœud inexistant.
    MissingParticipant(NodeId),
    /// Cycle de références interdit (trace du chemin).
    Cycle(Vec<NodeId>),
    /// Profondeur de résolution dépassée (garde anti-épuisement de pile).
    DepthExceeded,
    /// Occurrence relationnelle à plusieurs sources.
    MultipleSources,
    /// Occurrence relationnelle à plusieurs destinations.
    MultipleDestinations,
    /// Occurrence relationnelle sans source.
    MissingSource,
    /// Occurrence relationnelle sans destination.
    MissingDestination,
    /// Nœud référencé sans label `relation_occurrence`.
    WrongRelationLabel,
    /// Occurrence relationnelle sans type.
    MissingRelationType,
    /// Rôle présent mais non textuel.
    NonTextualRole,
    /// Clé recalculée ≠ clé persistée (corruption ou structure incomplète).
    KeyDivergence,
    /// Une clé est occupée par une construction incomplète non abandonnée.
    IncompleteConstruction(NodeId),
    /// Abandon refusé : preuve d'abandon insuffisante ou nœud référencé.
    AbandonRefused(NodeId),
    /// Échec de la suppression lors d'un abandon explicite.
    AbandonFailed(NodeId),
    /// Multiplicité nulle (une occurrence comptée zéro fois n'a pas de sens).
    InvalidMultiplicity,
    /// Écriture demandée sur une base ouverte en lecture seule.
    ReadOnly,
}

/// Interface compositionnelle sur le graphe natif (construire, résoudre).
pub struct Composition<'a> {
    db: &'a ObrainDB,
}

impl<'a> Composition<'a> {
    /// Construit un accès compositionnel sur la base `db`.
    pub fn new(db: &'a ObrainDB) -> Self {
        Self { db }
    }

    /// Construction canonique : deux spécifications structurellement identiques
    /// → même nœud. Sous-connaissances et relations portent une MULTIPLICITÉ.
    ///
    /// Complétude ≠ atomicité : `read_spec` (`KeyDivergence`) est un MARQUEUR de
    /// structure complète, PAS une publication atomique durable. `construct`
    /// n'est pas transactionnel : `create_node`, `canonical_key`, marqueur d'état
    /// `pending`, puis les arêtes une à une, enfin marqueur `complete`. Une
    /// interruption laisse un nœud à l'état `pending`. `construct` ne le SUPPRIME
    /// JAMAIS : il signale l'occupation (`IncompleteConstruction`). L'abandon est
    /// explicite et prouvé (`abandon_incomplete` : état `pending` + non référencé),
    /// puis la clé peut être réoccupée. Une publication atomique exigerait une
    /// transaction substrat (hors de ce module).
    ///
    /// Concurrence : deux `construct` simultanés de la même définition ne sont
    /// PAS sérialisés (pas de verrou/transaction) — limite de #419, ce module
    /// n'introduit PAS de seconde autorité de réservation.
    pub fn construct(&self, spec: &KnowledgeSpec) -> Result<NodeId, ResolveError> {
        // Rejette explicitement les multiplicités nulles.
        if spec.children.values().any(|&n| n == 0) || spec.relations.values().any(|&n| n == 0) {
            return Err(ResolveError::InvalidMultiplicity);
        }
        let key = spec.structural_key();
        if let Some(existing) = self.find_by_key_or_legacy(&key) {
            // Un nœud lisible est réutilisé (migration legacy EXPLICITE ensuite).
            // Une erreur de lecture NE prouve JAMAIS un abandon : corruption,
            // incompatibilité de version, lecture concurrente, référence
            // temporairement inaccessible sont indistinguables. On ne SUPPRIME
            // donc jamais ici ; on SIGNALE l'occupation. L'abandon est une
            // décision séparée, explicite et prouvée (`abandon_incomplete`).
            match self.read_spec(existing) {
                Ok(_) => {
                    self.migrate_encoding(existing)?;
                    return Ok(existing);
                }
                Err(_) => return Err(ResolveError::IncompleteConstruction(existing)),
            }
        }
        let id = self.db.create_node(&[LABEL_KNOWLEDGE]);
        self.db
            .set_node_property(id, KEY_CANONICAL, Value::from(key));
        self.db
            .set_node_property(id, KEY_STATE, Value::from("pending"));
        for m in &spec.members {
            self.db.create_edge(id, *m, EDGE_MEMBER);
        }
        for (c, count) in &spec.children {
            for _ in 0..*count {
                let eid = self.db.create_edge(id, c.target, EDGE_CHILD);
                if let Some(role) = &c.role {
                    self.db
                        .set_edge_property(eid, KEY_ROLE, Value::from(role.clone()));
                }
            }
        }
        for (rel, count) in &spec.relations {
            for _ in 0..*count {
                let occ = self.db.create_node(&[LABEL_RELATION]);
                self.db
                    .set_node_property(occ, KEY_REL_TYPE, Value::from(rel.rel_type.clone()));
                if let Some(role) = &rel.role {
                    self.db
                        .set_node_property(occ, KEY_ROLE, Value::from(role.clone()));
                }
                self.db.create_edge(occ, rel.src, EDGE_SRC);
                self.db.create_edge(occ, rel.dst, EDGE_DST);
                self.db.create_edge(id, occ, EDGE_RELATION);
            }
        }
        self.db
            .set_node_property(id, KEY_STATE, Value::from("complete"));
        Ok(id)
    }

    /// Résolution : aplatit transitivement les `child` en participants feuilles.
    /// Signale racine absente/invalide, référence absente/non-connaissance,
    /// participant absent, cycle, profondeur excessive. Un diamant (même cible
    /// référencée deux fois) est toléré.
    pub fn resolve(&self, id: NodeId) -> Result<BTreeSet<NodeId>, ResolveError> {
        let Some(root) = self.db.get_node(id) else {
            return Err(ResolveError::RootMissing(id));
        };
        if !root.labels.iter().any(|l| l.as_str() == LABEL_KNOWLEDGE) {
            return Err(ResolveError::RootNotKnowledge(id));
        }
        let mut members = BTreeSet::new();
        let mut state: HashMap<NodeId, u8> = HashMap::new();
        let mut path = Vec::new();
        self.resolve_dfs(id, &mut members, &mut state, &mut path, 0)?;
        Ok(members)
    }

    /// Clé structurelle d'une connaissance existante, si elle en a une.
    pub fn structural_key_of(&self, id: NodeId) -> Option<String> {
        let node = self.db.get_node(id)?;
        node.properties
            .get(&PropertyKey::from(KEY_CANONICAL))
            .and_then(|v| v.as_str().map(|s| s.to_string()))
    }

    /// Reconstruction de la DÉFINITION STRUCTURELLE depuis le graphe persistant
    /// (membres directs, enfants directs avec multiplicité, occurrences
    /// relationnelles avec direction/type/rôle et multiplicité). Ne vérifie PAS
    /// la clé persistée (voir `read_spec`).
    fn reconstruct_structure(&self, id: NodeId) -> Result<KnowledgeSpec, ResolveError> {
        let root = self.db.get_node(id).ok_or(ResolveError::RootMissing(id))?;
        if !root.labels.iter().any(|l| l.as_str() == LABEL_KNOWLEDGE) {
            return Err(ResolveError::RootNotKnowledge(id));
        }
        let mut members = BTreeSet::new();
        let mut children: BTreeMap<ChildRef, usize> = BTreeMap::new();
        let mut relations: BTreeMap<Relation, usize> = BTreeMap::new();
        for (dst, eid) in self.outgoing(id) {
            let edge = self
                .db
                .get_edge(eid)
                .ok_or(ResolveError::MissingReference(dst))?;
            match edge.edge_type.as_str() {
                EDGE_MEMBER => {
                    if self.db.get_node(dst).is_none() {
                        return Err(ResolveError::MissingParticipant(dst));
                    }
                    members.insert(dst);
                }
                EDGE_CHILD => {
                    let Some(child) = self.db.get_node(dst) else {
                        return Err(ResolveError::MissingReference(dst));
                    };
                    if !child.labels.iter().any(|l| l.as_str() == LABEL_KNOWLEDGE) {
                        return Err(ResolveError::NotKnowledge(dst));
                    }
                    let role = match edge.properties.get(&PropertyKey::from(KEY_ROLE)) {
                        None => None,
                        Some(v) => match v.as_str() {
                            Some(s) => Some(s.to_string()),
                            None => return Err(ResolveError::NonTextualRole),
                        },
                    };
                    *children.entry(ChildRef { target: dst, role }).or_insert(0) += 1;
                }
                EDGE_RELATION => {
                    let rel = self.read_relation_occurrence(dst)?;
                    *relations.entry(rel).or_insert(0) += 1;
                }
                _ => {}
            }
        }
        let spec = KnowledgeSpec {
            members,
            children,
            relations,
        };
        Ok(spec)
    }

    /// Lecture d'une connaissance. LECTURE SEULE : n'écrit JAMAIS (pas de
    /// migration pendant une lecture). Accepte l'encodage courant v3 OU un
    /// encodage historique valide (v2 original / v2 intermédiaire, compatibles
    /// avec la structure) ; refuse toute autre clé (`KeyDivergence`).
    pub fn read_spec(&self, id: NodeId) -> Result<KnowledgeSpec, ResolveError> {
        let spec = self.reconstruct_structure(id)?;
        let persisted = self.structural_key_of(id);
        if self.key_encoding_matches(&spec, persisted.as_deref()) {
            return Ok(spec);
        }
        Err(ResolveError::KeyDivergence)
    }

    /// Vrai si la clé persistée est l'encodage courant OU un legacy COMPATIBLE.
    fn key_encoding_matches(&self, spec: &KnowledgeSpec, persisted: Option<&str>) -> bool {
        if persisted == Some(spec.structural_key().as_str()) {
            return true;
        }
        [
            spec.legacy_v2_original_key(),
            spec.legacy_v2_intermediate_key(),
        ]
        .iter()
        .flatten()
        .any(|k| persisted == Some(k.as_str()))
    }

    /// MIGRE explicitement la clé d'une connaissance vers v3. Écriture EXPLICITE
    /// (jamais pendant une lecture). Retourne `true` si réécrite, `false` si déjà
    /// en v3. Refuse une clé non legacy (`KeyDivergence`) et toute écriture sur
    /// une base en LECTURE SEULE (`ReadOnly`). Une erreur de persistance du
    /// substrat est propagée par le substrat lui-même (pas masquée).
    pub fn migrate_encoding(&self, id: NodeId) -> Result<bool, ResolveError> {
        let spec = self.reconstruct_structure(id)?;
        let recomputed = spec.structural_key();
        let persisted = self.structural_key_of(id);
        if persisted.as_deref() == Some(recomputed.as_str()) {
            return Ok(false);
        }
        if !self.key_encoding_matches(&spec, persisted.as_deref()) {
            return Err(ResolveError::KeyDivergence);
        }
        if self.db.is_read_only() {
            return Err(ResolveError::ReadOnly);
        }
        self.db
            .set_node_property(id, KEY_CANONICAL, Value::from(recomputed));
        Ok(true)
    }

    /// Lit une occurrence relationnelle : type, rôle, source, destination.
    fn read_relation_occurrence(&self, occ: NodeId) -> Result<Relation, ResolveError> {
        let node = self
            .db
            .get_node(occ)
            .ok_or(ResolveError::MissingReference(occ))?;
        if !node.labels.iter().any(|l| l.as_str() == LABEL_RELATION) {
            return Err(ResolveError::WrongRelationLabel);
        }
        let rel_type = node
            .properties
            .get(&PropertyKey::from(KEY_REL_TYPE))
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .ok_or(ResolveError::MissingRelationType)?;
        let role = match node.properties.get(&PropertyKey::from(KEY_ROLE)) {
            None => None,
            Some(v) => match v.as_str() {
                Some(s) => Some(s.to_string()),
                None => return Err(ResolveError::NonTextualRole),
            },
        };
        let mut src: Option<NodeId> = None;
        let mut dst: Option<NodeId> = None;
        for (target, eid) in self.outgoing(occ) {
            let edge = self
                .db
                .get_edge(eid)
                .ok_or(ResolveError::MissingReference(target))?;
            match edge.edge_type.as_str() {
                EDGE_SRC => {
                    if src.is_some() {
                        return Err(ResolveError::MultipleSources);
                    }
                    if self.db.get_node(target).is_none() {
                        return Err(ResolveError::MissingParticipant(target));
                    }
                    src = Some(target);
                }
                EDGE_DST => {
                    if dst.is_some() {
                        return Err(ResolveError::MultipleDestinations);
                    }
                    if self.db.get_node(target).is_none() {
                        return Err(ResolveError::MissingParticipant(target));
                    }
                    dst = Some(target);
                }
                _ => {}
            }
        }
        let src = src.ok_or(ResolveError::MissingSource)?;
        let dst = dst.ok_or(ResolveError::MissingDestination)?;
        Ok(Relation {
            rel_type,
            src,
            dst,
            role,
        })
    }

    fn resolve_dfs(
        &self,
        id: NodeId,
        members: &mut BTreeSet<NodeId>,
        state: &mut HashMap<NodeId, u8>,
        path: &mut Vec<NodeId>,
        depth: usize,
    ) -> Result<(), ResolveError> {
        if depth > MAX_RESOLVE_DEPTH {
            return Err(ResolveError::DepthExceeded);
        }
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
                    if self.db.get_node(dst).is_none() {
                        return Err(ResolveError::MissingParticipant(dst));
                    }
                    members.insert(dst);
                }
                EDGE_CHILD => {
                    let Some(child) = self.db.get_node(dst) else {
                        return Err(ResolveError::MissingReference(dst));
                    };
                    if !child.labels.iter().any(|l| l.as_str() == LABEL_KNOWLEDGE) {
                        return Err(ResolveError::NotKnowledge(dst));
                    }
                    self.resolve_dfs(dst, members, state, path, depth + 1)?;
                }
                // Autre type d'arête : hors périmètre constitutif, ignoré.
                _ => {}
            }
        }
        path.pop();
        state.insert(id, 2);
        Ok(())
    }

    fn outgoing(&self, id: NodeId) -> Vec<(NodeId, EdgeId)> {
        self.db.data_store().edges_from(id, Direction::Outgoing)
    }

    /// Trouve un nœud par clé, y compris un nœud LEGACY (clé v2) dont la
    /// structure reconstruite donne `key` en v3. LECTURE SEULE : ne migre pas
    /// (la migration est explicite, via `migrate_encoding`). Évite qu'une même
    /// définition soit dupliquée après changement d'encodage.
    fn find_by_key_or_legacy(&self, key: &str) -> Option<NodeId> {
        if let Some(id) = self.find_by_key(key) {
            return Some(id);
        }
        let store = self.db.data_store();
        for id in store.node_ids() {
            let Some(node) = self.db.get_node(id) else {
                continue;
            };
            if !node.labels.iter().any(|l| l.as_str() == LABEL_KNOWLEDGE) {
                continue;
            }
            let Some(persisted) = self.structural_key_of(id) else {
                continue;
            };
            if persisted == key {
                continue;
            }
            if let Ok(spec) = self.reconstruct_structure(id)
                && spec.structural_key() == key
            {
                return Some(id);
            }
        }
        None
    }

    /// ABANDON EXPLICITE d'une construction incomplète. C'est la SEULE voie qui
    /// supprime un nœud de composition ; JAMAIS automatique, JAMAIS fondée sur
    /// une simple erreur de lecture. Conditions PROUVÉES, sinon refus :
    /// - marqueur d'état EXPLICITE `pending` (construction non scellée) ;
    /// - aucune connaissance ne le référence par une arête `child`.
    ///   Refus → `AbandonRefused` ; échec de suppression → `AbandonFailed`.
    pub fn abandon_incomplete(&self, id: NodeId) -> Result<(), ResolveError> {
        let Some(node) = self.db.get_node(id) else {
            return Err(ResolveError::RootMissing(id));
        };
        if !node.labels.iter().any(|l| l.as_str() == LABEL_KNOWLEDGE) {
            return Err(ResolveError::RootNotKnowledge(id));
        }
        if self.construction_state_of(id).as_deref() != Some("pending") {
            return Err(ResolveError::AbandonRefused(id));
        }
        if self.is_referenced(id) {
            return Err(ResolveError::AbandonRefused(id));
        }
        if !self.db.delete_node(id) {
            return Err(ResolveError::AbandonFailed(id));
        }
        Ok(())
    }

    /// État de construction explicite d'une connaissance (`pending`/`complete`).
    pub fn construction_state_of(&self, id: NodeId) -> Option<String> {
        let node = self.db.get_node(id)?;
        node.properties
            .get(&PropertyKey::from(KEY_STATE))
            .and_then(|v| v.as_str().map(|s| s.to_string()))
    }

    /// Vrai si une AUTRE connaissance référence `id` par une arête `child`.
    fn is_referenced(&self, id: NodeId) -> bool {
        self.db
            .data_store()
            .edges_from(id, Direction::Incoming)
            .into_iter()
            .any(|(src, eid)| {
                src != id
                    && self
                        .db
                        .get_edge(eid)
                        .is_some_and(|e| e.edge_type.as_str() == EDGE_CHILD)
            })
    }

    fn find_by_key(&self, key: &str) -> Option<NodeId> {
        let store = self.db.data_store();
        for id in store.node_ids() {
            let Some(node) = self.db.get_node(id) else {
                continue;
            };
            if !node.labels.iter().any(|l| l.as_str() == LABEL_KNOWLEDGE) {
                continue;
            }
            if node
                .properties
                .get(&PropertyKey::from(KEY_CANONICAL))
                .and_then(|v| v.as_str())
                == Some(key)
            {
                return Some(id);
            }
        }
        None
    }
}
