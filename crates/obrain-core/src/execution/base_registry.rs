//! Registre de bases pour la session multi-store (aretes inter-bases V2).
//!
//! Une requete s'execute avec un store PRIMAIRE et des stores ATTACHES a la
//! demande (spec << aretes inter-bases natives >> paragraphe 2) : quand
//! l'expansion rencontre une arete externe, la suite du motif s'execute dans
//! le store cible via un seek par identifiant — jamais de scan cross-store
//! implicite (paragraphe 6, non-buts).
//!
//! Le registre vit dans `SessionContext` (un `Option<Arc<..>>`, absent par
//! defaut : une session mono-base ne paie rien et ne change pas de
//! comportement). L'OUVERTURE d'un store est injectee par le moteur via une
//! closure (`StoreOpener`) : obrain-core ne connait pas obrain-substrate.

use crate::graph::{ExternalEdge, GraphStore};
use obrain_common::types::{NodeId, PropertyKey, Value};
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Cle magique portant le slug de base d'un noeud/arete materialise en
/// `Value::Map` (pendant de `_id` / `_labels` de `node_to_map`).
pub const BASE_KEY: &str = "_base";
/// Propriete par laquelle une clef NON numerique de `ref()` est resolue
/// dans le store cible (lookup indexe T17b quand l'index existe).
pub const REF_KEY_PROPERTY: &str = "id";

/// Ouvre un store a partir d'un chemin. Injecte par le moteur
/// (SubstrateStore::open) — `None` si l'ouverture echoue.
pub type StoreOpener = dyn Fn(&Path) -> Option<Arc<dyn GraphStore>> + Send + Sync;

/// Registre `{slug -> store}` d'une session multi-store.
///
/// - le PRIMAIRE est enregistre sous son slug : une reference qui revient
///   vers lui (cycle A -> B -> A) resout sur la MEME instance, jamais une
///   seconde ouverture des memes fichiers ;
/// - `register_store` injecte un store deja ouvert (meme processus — le
///   pool du service) ; `register_path` + `root` donnent l'ouverture
///   PARESSEUSE par chemin au premier besoin ;
/// - un hote distant (`to_host = Some(..)`) n'est JAMAIS resolu ici :
///   compte dans `remote_refs_unresolved`, la traversee rend absent
///   (reste explicite V5 — resolution reseau) ;
/// - une cible introuvable (base absente, noeud disparu) n'est JAMAIS une
///   erreur : compteur `dangling_refs`, ligne absente (trivalue).
pub struct BaseRegistry {
    primary_slug: String,
    /// Stores ouverts, primaire compris (sous son slug).
    opened: parking_lot::RwLock<HashMap<String, Arc<dyn GraphStore>>>,
    /// Chemins declares explicitement (slug -> repertoire).
    paths: parking_lot::RwLock<HashMap<String, PathBuf>>,
    /// Racine optionnelle : `root/slug` est essaye si le slug n'est ni
    /// ouvert ni declare.
    root: Option<PathBuf>,
    opener: Box<StoreOpener>,
    /// References vers un hote distant rencontrees (jamais resolues en V2).
    pub remote_refs_unresolved: AtomicU64,
    /// References pendantes rencontrees a la traversee (cible absente).
    pub dangling_refs: AtomicU64,
    /// (V3) Franchissements REFUSES par la garde de droits. SEPARE de
    /// `dangling_refs` : le lecteur ne distingue pas << interdit >> de
    /// << inexistant >> (pas d'oracle), mais l'ADMIN, oui — via ce
    /// compteur et l'audit.
    pub access_denied_refs: AtomicU64,
}

impl std::fmt::Debug for BaseRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BaseRegistry")
            .field("primary_slug", &self.primary_slug)
            .field("attached", &self.opened.read().len())
            .field("dangling_refs", &self.dangling_refs.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl BaseRegistry {
    /// Cree le registre d'une session : slug + store primaires, et la
    /// closure d'ouverture des stores attaches.
    pub fn new(
        primary_slug: impl Into<String>,
        primary: Arc<dyn GraphStore>,
        opener: Box<StoreOpener>,
    ) -> Self {
        let primary_slug = primary_slug.into();
        let mut opened = HashMap::new();
        opened.insert(primary_slug.clone(), primary);
        Self {
            primary_slug,
            opened: parking_lot::RwLock::new(opened),
            paths: parking_lot::RwLock::new(HashMap::new()),
            root: None,
            opener,
            remote_refs_unresolved: AtomicU64::new(0),
            dangling_refs: AtomicU64::new(0),
            access_denied_refs: AtomicU64::new(0),
        }
    }

    /// Fixe la racine des bases : `root/slug` devient un chemin candidat.
    #[must_use]
    pub fn with_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.root = Some(root.into());
        self
    }

    /// Slug de la base primaire.
    #[must_use]
    pub fn primary_slug(&self) -> &str {
        &self.primary_slug
    }

    /// Le slug designe-t-il la base primaire (hote local seulement) ?
    #[must_use]
    pub fn is_primary(&self, host: Option<&str>, slug: &str) -> bool {
        host.is_none() && slug == self.primary_slug
    }

    /// Injecte un store DEJA ouvert (pool du service, autre ObrainDB du
    /// meme processus). Ecrase une entree precedente du meme slug.
    pub fn register_store(&self, slug: impl Into<String>, store: Arc<dyn GraphStore>) {
        self.opened.write().insert(slug.into(), store);
    }

    /// Declare le chemin d'un slug pour l'ouverture paresseuse.
    pub fn register_path(&self, slug: impl Into<String>, path: impl Into<PathBuf>) {
        self.paths.write().insert(slug.into(), path.into());
    }

    /// Resout le store d'un slug — ouverture PARESSEUSE au premier besoin.
    ///
    /// `None` (jamais une erreur) quand : hote distant (compte
    /// `remote_refs_unresolved`), slug inconnu, ouverture impossible.
    pub fn store_for(&self, host: Option<&str>, slug: &str) -> Option<Arc<dyn GraphStore>> {
        if host.is_some() {
            // Multi-host : reserve. La reference est une VALEUR (absent),
            // jamais une erreur — resolution reseau a venir (V5).
            self.remote_refs_unresolved.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        if let Some(store) = self.opened.read().get(slug) {
            return Some(Arc::clone(store));
        }
        let path = self
            .paths
            .read()
            .get(slug)
            .cloned()
            .or_else(|| self.root.as_ref().map(|r| r.join(slug)))?;
        let store = (self.opener)(&path)?;
        let mut opened = self.opened.write();
        // Un concurrent a pu ouvrir entre-temps : la premiere instance gagne.
        let entry = opened
            .entry(slug.to_string())
            .or_insert_with(|| Arc::clone(&store));
        Some(Arc::clone(entry))
    }

    /// Note une reference pendante rencontree a la traversee.
    pub fn note_dangling(&self) {
        self.dangling_refs.fetch_add(1, Ordering::Relaxed);
    }

    /// (Service, eviction du pool) DETACHE un slug : retire instance et
    /// chemin du registre. L Arc du store est relache — la memoire d une
    /// base evincee peut se demapper au lieu d etre retenue par les
    /// registres des autres bases. Les references vers elle redeviennent
    /// simplement absentes (trivalue) jusqu a une re-attache.
    pub fn detach_store(&self, slug: &str) {
        self.opened.write().remove(slug);
        self.paths.write().remove(slug);
    }

    /// (V3) Resolution GARDEE — LA porte unique des operateurs.
    ///
    /// Les droits du LECTEUR s'evaluent sur le store CIBLE a CHAQUE
    /// franchissement de frontiere, pas seulement au point d'entree
    /// (exigence de l'entite, spec paragraphe 8 : << une requete publique ne
    /// doit pas remonter vers un noeud prive via l'arete >>).
    ///
    /// - refus de droit = ABSENT trivalue, INDISTINGUABLE d'une ref
    ///   pendante pour le lecteur (pas d'oracle interdit/inexistant) ;
    ///   compte a part (`access_denied_refs`, visible de l'admin) et
    ///   audite via le hook injecte dans [`AccessContext`] ;
    /// - base locale introuvable = `dangling_refs`, compte ICI (les
    ///   appelants n'ont plus a le faire — ils ne savent pas distinguer) ;
    /// - `access = None` = acces total, comportement historique : le cout
    ///   est UN test d'Option par franchissement.
    pub fn store_for_reader(
        &self,
        access: Option<&AccessContext>,
        host: Option<&str>,
        slug: &str,
        key: &str,
    ) -> Option<Arc<dyn GraphStore>> {
        if let Some(ctx) = access
            && !ctx.allows(slug)
        {
            self.access_denied_refs.fetch_add(1, Ordering::Relaxed);
            ctx.emit_audit(slug, key);
            return None;
        }
        let store = self.store_for(host, slug);
        if store.is_none() && host.is_none() {
            self.dangling_refs.fetch_add(1, Ordering::Relaxed);
        }
        store
    }
}

// ---------------------------------------------------------------------------
// Contexte de droits (V3)
// ---------------------------------------------------------------------------

/// Decideur de droits : `(lecteur, slug du store cible) -> autorise ?`.
/// Injecte par la couche service (roles/propriete) — le moteur ne connait
/// aucune politique, il APPLIQUE.
pub type AccessDecider = dyn Fn(&str, &str) -> bool + Send + Sync;

/// Evenement d'audit : un franchissement REFUSE par la garde.
#[derive(Debug, Clone)]
pub struct AccessDenied {
    /// Identite du lecteur refuse.
    pub reader: String,
    /// Slug du store cible.
    pub to_store: String,
    /// Clef de la reference visee (vide quand la resolution portait sur un
    /// noeud deja materialise, ex. reprise de motif depuis un noeud distant).
    pub to_key: String,
    /// Requete en cours, TRONQUEE (best effort : notee par les chemins
    /// execute/execute_with_params/execute_cypher ; None ailleurs).
    pub query: Option<String>,
}

/// Hook d'audit injectable — pas de dependance dure du core a un logger.
pub type AuditHook = dyn Fn(&AccessDenied) + Send + Sync;

/// (V3) Contexte de droits d'une session : identite du LECTEUR, decideur
/// injecte, hook d'audit optionnel. Voyage dans `SessionContext`
/// (`None` = acces total, comportement historique, zero changement
/// mono-base). Evalue par [`BaseRegistry::store_for_reader`] a chaque
/// franchissement.
pub struct AccessContext {
    reader: String,
    decide: Box<AccessDecider>,
    audit: Option<Box<AuditHook>>,
    /// Requete en cours (tronquee), notee par la session pour l'audit.
    current_query: parking_lot::Mutex<Option<String>>,
}

impl std::fmt::Debug for AccessContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccessContext")
            .field("reader", &self.reader)
            .field("audit", &self.audit.is_some())
            .finish_non_exhaustive()
    }
}

impl AccessContext {
    /// Cree le contexte de droits d'un lecteur.
    pub fn new(reader: impl Into<String>, decide: Box<AccessDecider>) -> Self {
        Self {
            reader: reader.into(),
            decide,
            audit: None,
            current_query: parking_lot::Mutex::new(None),
        }
    }

    /// Attache le hook d'audit (chaque refus lui est transmis).
    #[must_use]
    pub fn with_audit(mut self, hook: Box<AuditHook>) -> Self {
        self.audit = Some(hook);
        self
    }

    /// Identite du lecteur.
    #[must_use]
    pub fn reader(&self) -> &str {
        &self.reader
    }

    /// Le lecteur a-t-il le droit de resoudre dans `slug` ?
    #[must_use]
    pub fn allows(&self, slug: &str) -> bool {
        (self.decide)(&self.reader, slug)
    }

    /// Note la requete en cours (tronquee a 200 caracteres) pour l'audit.
    pub fn note_requete(&self, query: &str) {
        let tronquee: String = query.chars().take(200).collect();
        *self.current_query.lock() = Some(tronquee);
    }

    /// Emet l'evenement d'audit d'un refus (no-op sans hook).
    pub(crate) fn emit_audit(&self, to_store: &str, to_key: &str) {
        if let Some(hook) = &self.audit {
            hook(&AccessDenied {
                reader: self.reader.clone(),
                to_store: to_store.to_string(),
                to_key: to_key.to_string(),
                query: self.current_query.lock().clone(),
            });
        }
    }
}

/// Resout la clef d'un `ref()` en identifiant de noeud dans `store`.
///
/// - clef entierement numerique -> `NodeId` brut (verifie vivant) ;
/// - sinon -> premier noeud vivant dont la propriete
///   [`REF_KEY_PROPERTY`] vaut la clef (`find_nodes_by_property`, indexe
///   T17b quand l'index existe — O(1)).
///
/// `None` = absent (trivalue), jamais une erreur.
///
/// PIEGES DOCUMENTES (verification adversariale, figes par
/// `verif_adversariale.rs` p3_*) :
/// - une clef numerique qui DEBORDE u64 (ex. 20 chiffres) route quand
///   meme NodeId : le parse echoue en silence -> absente, MEME si un
///   noeud porte exactement cette propriete `id` ;
/// - les zeros de tete routent NodeId ("007" -> NodeId(7)) : une
///   propriete id = "007" est MASQUEE par la regle numerique ;
/// - en cas de collision (NodeId N vivant ET propriete id = "N"), le
///   NodeId gagne, deterministe.
/// Regle de survie pour l'outillage : des clefs applicatives JAMAIS
/// entierement numeriques (prefixe type, ex. "note_...").
#[must_use]
pub fn resolve_node_key(store: &dyn GraphStore, key: &str) -> Option<NodeId> {
    if !key.is_empty() && key.bytes().all(|b| b.is_ascii_digit()) {
        let id = NodeId(key.parse().ok()?);
        return store.get_node_labels(id).map(|_| id);
    }
    store
        .find_nodes_by_property(REF_KEY_PROPERTY, &Value::String(key.into()))
        .into_iter()
        .find(|id| store.get_node_labels(*id).is_some())
}

/// Materialise un noeud DISTANT en `Value::Map` : `_id`, `_labels`,
/// `_base` (slug) + toutes ses proprietes, hydratees a la frontiere pour
/// que l'aval (WHERE, projections) lise sans repasser par le store.
/// `None` si le noeud n'existe pas/plus (absent trivalue).
#[must_use]
pub fn remote_node_value(slug: &str, store: &dyn GraphStore, id: NodeId) -> Option<Value> {
    let node = store.get_node(id)?;
    let mut map = BTreeMap::new();
    map.insert(
        PropertyKey::new("_id"),
        Value::Int64(node.id.as_u64() as i64),
    );
    let labels: Vec<Value> = node
        .labels
        .iter()
        .map(|l| Value::String(l.clone()))
        .collect();
    map.insert(PropertyKey::new("_labels"), Value::List(labels.into()));
    map.insert(PropertyKey::new(BASE_KEY), Value::String(slug.into()));
    for (key, value) in &node.properties {
        map.insert(key.clone(), value.clone());
    }
    Some(Value::Map(Arc::new(map)))
}

/// Materialise une arete EXTERNE en `Value::Map` (`_id`, `_type`, `_base`
/// = slug de la base SOURCE qui la porte, `_to_store`, `_to_key`).
#[must_use]
pub fn external_edge_value(edge: &ExternalEdge, source_slug: &str) -> Value {
    let mut map = BTreeMap::new();
    map.insert(
        PropertyKey::new("_id"),
        Value::Int64(edge.id.as_u64() as i64),
    );
    map.insert(
        PropertyKey::new("_type"),
        Value::String(edge.edge_type.as_str().into()),
    );
    map.insert(
        PropertyKey::new(BASE_KEY),
        Value::String(source_slug.into()),
    );
    if let Some(host) = &edge.to_host {
        map.insert(
            PropertyKey::new("_to_host"),
            Value::String(host.as_str().into()),
        );
    }
    map.insert(
        PropertyKey::new("_to_store"),
        Value::String(edge.to_store.as_str().into()),
    );
    map.insert(
        PropertyKey::new("_to_key"),
        Value::String(edge.to_key.as_str().into()),
    );
    Value::Map(Arc::new(map))
}

/// Materialise une arete INTERNE d'un store DISTANT en `Value::Map`
/// (`_id`, `_type`, `_base`, `_source`, `_target`).
#[must_use]
pub fn remote_edge_value(
    slug: &str,
    store: &dyn GraphStore,
    edge_id: obrain_common::types::EdgeId,
    src: NodeId,
    dst: NodeId,
) -> Value {
    let mut map = BTreeMap::new();
    map.insert(
        PropertyKey::new("_id"),
        Value::Int64(edge_id.as_u64() as i64),
    );
    if let Some(t) = store.edge_type(edge_id) {
        map.insert(PropertyKey::new("_type"), Value::String(t));
    }
    map.insert(PropertyKey::new(BASE_KEY), Value::String(slug.into()));
    map.insert(
        PropertyKey::new("_source"),
        Value::Int64(src.as_u64() as i64),
    );
    map.insert(
        PropertyKey::new("_target"),
        Value::Int64(dst.as_u64() as i64),
    );
    Value::Map(Arc::new(map))
}
