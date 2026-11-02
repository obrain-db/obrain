//! Tissage synaptique durable — la **base synaptique de l'agent**.
//!
//! Conception : `docs/ADR-COSMOS-TOILE-2026-09-10.md`.
//!
//! La vue Cosmos représente un univers à trois plans — **connaissance**,
//! **self**, **utilisateur** — où chaque monde (base obrain) est relié aux
//! autres par des synapses fondées sur des PREUVES RÉELLES, jamais sur des
//! similarités inventées :
//!
//! - **entité partagée** : le même `qualified_name` vit dans ≥ 2 mondes
//!   (mesuré : 274 sessions dev↔prod, 400/400 fonctions, 400/400 fichiers) ;
//! - **plan traversant** : une entité du plan `self` (préfixe `self:`) ou du
//!   plan `utilisateur` (`Account`) présente dans un monde de connaissance
//!   tisse ce monde au self / à l'utilisateur ;
//! - **datation réelle** : `Session.createdAt` donne l'instant de naissance
//!   de chaque synapse datable → la toile se densifie sur l'horloge réelle de
//!   l'agent (341 sessions, ~15 jours) et non sur une animation arbitraire.
//!
//! Le résultat est PERSISTÉ dans un store obrain dédié — la base synaptique
//! de l'agent — rattachée au nœud `Self`. Elle survit aux redémarrages et se
//! **RENFORCE** (`reinforcement`) à chaque tissage au lieu d'être réécrite :
//! c'est ce qui la rend durable et personnelle.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use obrain_common::types::Value as OV;
use obrain_engine::ObrainDB;

/// Schéma du payload échangé avec le dashboard.
pub const WEAVE_SCHEMA: &str = "obrain-weave-v1";

/// Nombre de paliers de la courbe de densification (horloge réelle).
const DENSIFICATION_STEPS: usize = 48;

/// Plafond d'ancres persistées (la toile = ce qui RELIE, pas l'intérieur).
const MAX_PERSISTED_ANCHORS: usize = 4000;

/// Échantillon d'ancres conservé par synapse (pour dessiner les neurones).
// ÉCHANTILLONNAGE PORTÉ DE 24 À 160 (mesuré) : la toile compte jusqu'à 2 347
// ancres réelles par fibre et n'en envoyait que 24 — soit **1 % de sa
// substance**, ce qui donnait à la carte l'air d'un réseau à peine tissé alors
// qu'il est dense. Le coût est de la sérialisation, pas du calcul : la liste est
// déjà construite.
const SYNAPSE_SAMPLE: usize = 160;

/// Relations principales d'un monde interrogées pour la signature de flux.
const FLOW_REL_TYPES: usize = 10;

/// Types retenus de chaque côté (source / cible) par relation.
const FLOW_SIDE_LABELS: usize = 3;

/// Échantillon de flux partagés conservé par fibre.
// Idem pour les flux : 19 flux réels (jusqu'à 428 569 occurrences partagées)
// n'en envoyaient que 8, et le client ne les DESSINAIT pas — il en affichait le
// nombre. Une trajectoire réelle qui n'est pas tracée n'existe pas pour l'œil.
const FLOW_SAMPLE: usize = 24;

/// Labels servant d'ancres par défaut : ce qui relie réellement les mondes.
pub const DEFAULT_ANCHOR_LABELS: &[&str] = &[
    "Session",
    "SelfState",
    "Goal",
    "Skill",
    "Procedure",
    "Note",
    "Function",
    "File",
    "Struct",
    "Module",
];

// ---------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------

/// Un monde de l'univers : une base obrain nommée.
#[derive(Clone, Debug)]
pub struct WorldSpec {
    /// Nom affiché (ex. « Runtime dev (engrams) »).
    pub label: String,
    /// Chemin du store.
    pub path: PathBuf,
    /// Racine du territoire indexé par ce monde (ex. le dépôt git). Sert de
    /// preuve de tissage : une session dont le `cwd` tombe sous cette racine a
    /// réellement travaillé dans ce monde.
    pub root: Option<String>,
}

/// Paramètres d'un tissage.
#[derive(Clone, Debug)]
pub struct WeaveOptions {
    /// Mondes à tisser.
    pub worlds: Vec<WorldSpec>,
    /// `qualified_name` du nœud Self propriétaire de la base synaptique.
    pub self_qn: String,
    /// Store de la base synaptique de l'agent (créé si absent).
    pub out: PathBuf,
    /// Population d'ancres lue par label et par monde.
    pub per_label: usize,
    /// Labels d'ancrage.
    pub anchor_labels: Vec<String>,
}

// ---------------------------------------------------------------------------
// Modèle interne
// ---------------------------------------------------------------------------

/// Les trois plans que la toile relie.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Nature {
    /// Pur substrat de code : ni self ni utilisateur.
    Connaissance,
    /// Porte le self de l'agent (nœud `Self`).
    #[serde(rename = "self")]
    Soi,
    /// Porte l'utilisateur (`Account`) et/ou ses sessions.
    Utilisateur,
}

/// Une session réelle : le vécu de l'agent, daté.
#[derive(Clone, Debug)]
struct SessionRec {
    qn: String,
    created_at: i64,
    /// Répertoire de travail réel — la preuve de tissage vers les mondes de code.
    cwd: String,
}

/// Un flux RÉEL observé dans un monde : la relation `rel` y relie réellement
/// le type `src` au type `dst`.
///
/// LU PAR MARGES, et c'est un choix dicté par la mesure. Trois formes ont été
/// comparées sur trois stores (dev, self-demo, linux) :
///
/// | forme | résultat |
/// |---|---|
/// | `RETURN labels(a)[0], count(r)` **seul** | **FIABLE partout** |
/// | `RETURN labels(a)[0], labels(b)[0], count(r)` | **FAUX** : la cible est recopiée dans la source (`Step,Step` au lieu de `Turn→Step`, `Function,Function`, `Procedure,Procedure`) |
/// | `MATCH (a:Label)-[r]->(b)` ancré | correct sur dev/linux, **vide** sur self-demo (`count(a:Procedure)`=32 mais `(a:Procedure)-[r]->(b)`=∅) |
///
/// On ne peut donc pas lire aujourd'hui le compte JOINT d'une paire de labels.
/// Les volumes conservés sont les **marges réelles** (`src_count`, `dst_count`),
/// nommées comme telles : aucune paire n'est présentée comme mesurée
/// conjointement. La lecture par marges a d'ailleurs corrigé une conclusion
/// antérieure : le flux réel du self est `Self -[MASTERS]-> Procedure` (le Self
/// tisse vers ses connaissances), et non `Procedure→Procedure`.
#[derive(Clone, Debug, serde::Serialize)]
struct Flow {
    src: String,
    rel: String,
    dst: String,
    /// Marge réelle : arêtes `(src)-[rel]->(…)`.
    src_count: i64,
    /// Marge réelle : arêtes `(…)-[rel]->(dst)`.
    dst_count: i64,
}

impl Flow {
    /// Clé d'identité d'un flux (indépendante de son volume).
    fn key(&self) -> (String, String, String) {
        (self.src.clone(), self.rel.clone(), self.dst.clone())
    }

    /// Borne inférieure honnête du volume conjoint : le minimum des deux
    /// marges. Ne prétend pas être le compte exact de la paire.
    fn bound(&self) -> i64 {
        self.src_count.min(self.dst_count)
    }
}

/// Signature réelle d'un monde.
#[derive(Clone, Debug, Default)]
struct WorldMeta {
    label: String,
    path: String,
    slug: String,
    root: Option<String>,
    nodes: i64,
    edges: i64,
    types: Vec<(String, i64)>,
    has_self: bool,
    has_account: bool,
    /// Sessions réelles du monde.
    sessions: Vec<SessionRec>,
    /// Flux réels de types (preuve P4).
    flows: Vec<Flow>,
}

/// Une entité réelle partagée entre mondes : un « neurone » de la toile.
#[derive(Clone, Debug, Default)]
struct Anchor {
    qn: String,
    label: String,
    worlds: BTreeSet<String>,
    first_seen: Option<i64>,
    /// Vrai si l'ancre porte le self (préfixe `self:`).
    is_self: bool,
    /// Vrai si l'ancre représente l'utilisateur (label `Account`).
    is_user: bool,
}

/// Fibre agrégée entre deux mondes.
#[derive(Clone, Debug, Default)]
struct Fiber {
    a: String,
    b: String,
    /// Nombre d'ancres partagées (preuve P1/P3).
    anchors: Vec<String>,
    /// Nombre de ponts de vécu (preuve P2 : sessions exécutées dans le monde).
    bridged: usize,
    /// Flux de types RÉELLEMENT partagés (preuve P4) : `(src, rel, dst, n_a, n_b)`.
    /// C'est une parenté de STRUCTURE — plus faible que P1/P2, et présentée
    /// comme telle : jamais confondue avec une identité d'entité ni un vécu.
    flows: Vec<(String, String, String, i64, i64)>,
    /// Nombre TOTAL de flux partagés (avant échantillonnage).
    flow_count: usize,
    /// Toutes les dates réelles rattachées à cette fibre (naissance des
    /// synapses) — c'est l'horloge de la densification.
    events: Vec<i64>,
    first_seen: Option<i64>,
    last_spike: Option<i64>,
}

impl Fiber {
    /// Poids réel de la fibre : entités partagées + ponts de vécu.
    fn weight(&self) -> usize {
        self.anchors.len() + self.bridged
    }

    /// Enregistre une date réelle de naissance de synapse.
    fn spike(&mut self, t: i64) {
        self.events.push(t);
        self.first_seen = Some(self.first_seen.map_or(t, |x| x.min(t)));
        self.last_spike = Some(self.last_spike.map_or(t, |x| x.max(t)));
    }
}

// ---------------------------------------------------------------------------
// Lecture des mondes
// ---------------------------------------------------------------------------

/// Échappe une chaîne pour un littéral GQL `'…'`.
fn lit(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// Slug stable pour un qn de monde.
///
/// `pub(crate)` : c'est la règle qui lie `world:<slug>` au libellé d'un monde.
/// L'ancrage du chat (`crate::ancrage`) doit appliquer **exactement la même**,
/// sinon le repli d'un monde pas encore tissé chercherait un slug que le tissage
/// n'aurait jamais produit — deux règles divergentes, deux résolutions fausses.
pub(crate) fn slug(label: &str) -> String {
    let mut s = String::new();
    for c in label.chars() {
        if c.is_ascii_alphanumeric() {
            s.extend(c.to_lowercase());
        } else if !s.ends_with('-') && !s.is_empty() {
            s.push('-');
        }
    }
    s.trim_matches('-').to_string()
}

/// Extrait une chaîne d'une valeur du moteur (tolérant aux variantes).
fn as_str(v: Option<&OV>) -> Option<String> {
    match v? {
        OV::String(s) => Some(s.to_string()),
        OV::Int64(x) => Some(x.to_string()),
        OV::Float64(x) => Some(x.to_string()),
        _ => None,
    }
}

/// Extrait un entier d'une valeur du moteur.
fn as_i64(v: Option<&OV>) -> Option<i64> {
    match v? {
        OV::Int64(x) => Some(*x),
        OV::Float64(x) => Some(*x as i64),
        OV::String(s) => s.parse().ok(),
        _ => None,
    }
}

/// Exécute une requête et renvoie ses lignes (erreur = liste vide, jamais un
/// crash : un monde sans tel label n'est pas une panne).
fn rows(db: &ObrainDB, gql: &str) -> Vec<Vec<OV>> {
    db.session()
        .execute(gql)
        .map(|r| r.rows)
        .unwrap_or_default()
}

/// Lit `count(n)` pour un label donné (0 si absent).
fn count_label(db: &ObrainDB, label: &str) -> i64 {
    rows(db, &format!("MATCH (n:{label}) RETURN count(n)"))
        .first()
        .and_then(|r| as_i64(r.first()))
        .unwrap_or(0)
}

/// Signature complète d'un monde.
fn read_world(spec: &WorldSpec) -> Result<WorldMeta> {
    let db = ObrainDB::open_read_only(&spec.path)
        .with_context(|| format!("weave : ouverture RO de {}", spec.path.display()))?;
    let mut m = WorldMeta {
        label: spec.label.clone(),
        path: spec.path.to_string_lossy().to_string(),
        slug: slug(&spec.label),
        root: spec.root.clone(),
        ..Default::default()
    };
    m.nodes = rows(&db, "MATCH (n) RETURN count(n)")
        .first()
        .and_then(|r| as_i64(r.first()))
        .unwrap_or(0);
    m.edges = rows(&db, "MATCH ()-[r]->() RETURN count(r)")
        .first()
        .and_then(|r| as_i64(r.first()))
        .unwrap_or(0);
    m.types = rows(
        &db,
        "MATCH (n) RETURN labels(n)[0] AS t, count(n) AS nbr ORDER BY nbr DESC LIMIT 10",
    )
    .into_iter()
    .filter_map(|r| Some((as_str(r.first())?, as_i64(r.get(1))?)))
    .collect();
    m.has_self = count_label(&db, "Self") > 0;
    m.has_account = count_label(&db, "Account") > 0;
    // Signature de flux RÉELLE (preuve P4) lue avant de fermer le handle.
    m.flows = read_flows(&db, FLOW_REL_TYPES, FLOW_SIDE_LABELS);
    // Horloge réelle du monde : la chronologie des sessions.
    m.sessions = rows(
        &db,
        "MATCH (n:Session) RETURN n.qualified_name AS q, n.createdAt AS t, n.cwd AS c LIMIT 2000",
    )
    .into_iter()
    .filter_map(|r| {
        let q = as_str(r.first())?;
        let t = as_i64(r.get(1))?;
        (t > 0).then(|| SessionRec {
            qn: q,
            created_at: t,
            cwd: as_str(r.get(2)).unwrap_or_default(),
        })
    })
    .collect();
    Ok(m)
}

impl WorldMeta {
    /// Nature dominante : self > utilisateur > connaissance.
    fn nature(&self) -> Nature {
        if self.has_self {
            Nature::Soi
        } else if self.has_account {
            Nature::Utilisateur
        } else {
            Nature::Connaissance
        }
    }
}

/// Lit les `qualified_name` d'un label dans un monde (population bornée).
fn read_qns(db: &ObrainDB, label: &str, limit: usize) -> Vec<String> {
    rows(
        db,
        &format!("MATCH (n:{label}) RETURN n.qualified_name AS q LIMIT {limit}"),
    )
    .into_iter()
    .filter_map(|r| as_str(r.first()))
    .filter(|q| !q.is_empty())
    .collect()
}

/// Labels de sortie sûrs pour une requête GQL (garde-fou anti-injection sur
/// un nom de label qui viendrait d'une donnée).
fn safe_label(l: &str) -> Option<&str> {
    (!l.is_empty() && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')).then_some(l)
}

/// Idem pour un nom de type de relation (les types réels portent des
/// underscores : `DEFINES_METHOD`, `HAS_STATE`).
fn safe_rel(t: &str) -> Option<&str> {
    (!t.is_empty() && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')).then_some(t)
}

/// Lit la signature de flux d'un monde, **par marges fiables** (voir `Flow`) :
/// pour ses principales relations, quels types elles relient réellement, de
/// chaque côté, avec les volumes réels.
fn read_flows(db: &ObrainDB, rel_types: usize, labels: usize) -> Vec<Flow> {
    let mut out: Vec<Flow> = Vec::new();
    // 1) les relations principales du monde (agrégation sur `type(r)` SEUL :
    //    forme vérifiée fiable sur les trois stores).
    let tops = rows(
        db,
        &format!(
            "MATCH ()-[r]->() RETURN type(r) AS t, count(r) AS w \
             ORDER BY w DESC LIMIT {rel_types}"
        ),
    );
    for row in tops {
        let Some(rel) = as_str(row.first()) else {
            continue;
        };
        if safe_rel(&rel).is_none() {
            continue;
        }
        // 2) marge SOURCE : quels types émettent cette relation.
        let srcs: Vec<(String, i64)> = rows(
            db,
            &format!(
                "MATCH (a)-[r:{rel}]->(b) RETURN labels(a)[0] AS s, count(r) AS w \
                 ORDER BY w DESC LIMIT {labels}"
            ),
        )
        .into_iter()
        .filter_map(|r| Some((as_str(r.first())?, as_i64(r.get(1))?)))
        .collect();
        // 3) marge CIBLE : quels types la reçoivent.
        let dsts: Vec<(String, i64)> = rows(
            db,
            &format!(
                "MATCH (a)-[r:{rel}]->(b) RETURN labels(b)[0] AS d, count(r) AS w \
                 ORDER BY w DESC LIMIT {labels}"
            ),
        )
        .into_iter()
        .filter_map(|r| Some((as_str(r.first())?, as_i64(r.get(1))?)))
        .collect();
        // Le croisement des marges borne la paire sans jamais la prétendre
        // mesurée : c'est la FORME de la relation dans ce monde.
        for (s, sw) in srcs.iter().take(2) {
            for (d, dw) in dsts.iter().take(2) {
                out.push(Flow {
                    src: s.clone(),
                    rel: rel.clone(),
                    dst: d.clone(),
                    src_count: *sw,
                    dst_count: *dw,
                });
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Calcul du tissage
// ---------------------------------------------------------------------------

/// Résultat complet du tissage (payload + agrégats de persistance).
#[derive(Debug)]
pub struct Weave {
    /// Payload JSON prêt pour le dashboard.
    pub payload: serde_json::Value,
    /// Nombre d'ancres de la toile.
    pub anchors: usize,
    /// Nombre de fibres persistées.
    pub fibers: usize,
}

/// Calcule la toile : lit les mondes, croise les preuves, agrège les fibres.
fn compute(opts: &WeaveOptions) -> Result<Weave> {
    let metas: Vec<WorldMeta> = opts
        .worlds
        .iter()
        .map(read_world)
        .collect::<Result<Vec<_>>>()?;

    // Chronologie globale (horloge réelle du tissage).
    let mut clock: Vec<i64> = metas
        .iter()
        .flat_map(|m| m.sessions.iter().map(|s| s.created_at))
        .collect();
    clock.sort_unstable();
    let t0 = clock.first().copied().unwrap_or(0);
    let t1 = clock.last().copied().unwrap_or(0);
    // Date de naissance d'une session : la vraie, ou inconnue.
    let session_date: BTreeMap<&str, i64> = metas
        .iter()
        .flat_map(|m| m.sessions.iter().map(|s| (s.qn.as_str(), s.created_at)))
        .collect();

    // ── ancres : entités réelles, croisées entre mondes ──
    let mut anchors: BTreeMap<String, Anchor> = BTreeMap::new();
    for (i, meta) in metas.iter().enumerate() {
        let db = ObrainDB::open_read_only(&opts.worlds[i].path)?;
        for label in &opts.anchor_labels {
            for qn in read_qns(&db, label, opts.per_label) {
                let is_self = qn.starts_with("self:");
                let is_user = label == "Account" || qn.starts_with("account:");
                let entry = anchors.entry(qn.clone()).or_insert_with(|| Anchor {
                    qn: qn.clone(),
                    label: label.clone(),
                    ..Default::default()
                });
                entry.worlds.insert(meta.slug.clone());
                entry.is_self |= is_self;
                entry.is_user |= is_user;
                if entry.first_seen.is_none()
                    && let Some(t) = session_date.get(qn.as_str())
                {
                    entry.first_seen = Some(*t);
                }
            }
        }
    }

    // La toile = ce qui RELIE : ancres présentes dans ≥ 2 mondes, plus tout ce
    // qui porte le self ou l'utilisateur (le self tisse même depuis un seul
    // monde, par définition).
    let mut weave_anchors: Vec<&Anchor> = anchors
        .values()
        .filter(|a| a.worlds.len() >= 2 || a.is_self || a.is_user)
        .collect();
    // ── LE SELF PASSE AVANT LE NOMBRE DE MONDES ─────────────────────────────
    // MESURÉ, et c'était une éviction silencieuse : le plafond de persistance
    // (`MAX_PERSISTED_ANCHORS`) est appliqué APRÈS ce tri, et le tri classait par
    // nombre de mondes décroissant. Les ancres qui portent le SELF vivent par
    // définition dans UN SEUL monde (« le self tisse même depuis un seul monde »,
    // dit le commentaire juste au-dessus) : elles se retrouvaient donc EN DERNIER
    // et étaient les premières évincées dès que la population dépassait le
    // plafond. Élargir le tissage faisait donc DISPARAÎTRE le self de la toile :
    // mesuré sur ce store, 22 labels à 1 500 au lieu de 10 à 400 → les arêtes
    // `WEAVES` (Self → ancres) tombaient de **400 à 0**, alors que le nombre
    // d'ancres tissées, lui, doublait (2 347 → 4 009). Un tissage plus large
    // perdait ce qui est le plus proprement à l'agent.
    //
    // Ce qui porte le self ou l'utilisateur passe donc AVANT le nombre de mondes :
    // le tri est un ORDRE DE PRIORITÉ d'éviction, et ce qu'on garde en dernier
    // doit être ce qu'on ne veut pas perdre.
    weave_anchors.sort_by(|a, b| {
        (b.is_self || b.is_user)
            .cmp(&(a.is_self || a.is_user))
            .then(b.worlds.len().cmp(&a.worlds.len()))
            .then(a.first_seen.cmp(&b.first_seen))
            .then(a.qn.cmp(&b.qn))
    });
    let total_anchors = weave_anchors.len();
    let persist_cap = total_anchors.min(MAX_PERSISTED_ANCHORS);

    // ── fibres : agrégat par paire de mondes ──
    let mut fibers: BTreeMap<(String, String), Fiber> = BTreeMap::new();
    for a in &weave_anchors {
        let ws: Vec<&String> = a.worlds.iter().collect();
        for i in 0..ws.len() {
            for j in (i + 1)..ws.len() {
                let key = (ws[i].clone(), ws[j].clone());
                let f = fibers.entry(key.clone()).or_insert_with(|| Fiber {
                    a: key.0.clone(),
                    b: key.1.clone(),
                    ..Default::default()
                });
                f.anchors.push(a.qn.clone());
                if let Some(t) = a.first_seen {
                    f.spike(t);
                }
            }
        }
    }

    // ── P2 : pont de VÉCU — une session exécutée dans un territoire relie le
    // monde qui l'a vécue au monde qui décrit ce territoire. Preuve réelle :
    // `Session.cwd` tombe sous la racine du monde cible, et la date de la
    // session date le tissage. C'est ce pont qui fait se densifier la toile
    // sur l'horloge réelle du travail de l'agent.
    for (i, src) in metas.iter().enumerate() {
        for s in &src.sessions {
            if s.cwd.is_empty() || s.cwd == "NULL" {
                continue;
            }
            for (j, dst) in metas.iter().enumerate() {
                if i == j {
                    continue;
                }
                let Some(root) = dst.root.as_deref().filter(|r| !r.is_empty()) else {
                    continue;
                };
                let under = s.cwd == root
                    || s.cwd.starts_with(&format!("{root}/"))
                    || root.starts_with(&format!("{}/", s.cwd));
                if !under {
                    continue;
                }
                let key = if src.slug <= dst.slug {
                    (src.slug.clone(), dst.slug.clone())
                } else {
                    (dst.slug.clone(), src.slug.clone())
                };
                let f = fibers.entry(key.clone()).or_insert_with(|| Fiber {
                    a: key.0.clone(),
                    b: key.1.clone(),
                    ..Default::default()
                });
                f.bridged += 1;
                f.spike(s.created_at);
            }
        }
    }

    // ── P4 : parenté de STRUCTURE — deux mondes portent réellement le même
    // flux de types (mêmes types, même relation), chacun avec son volume. Lu
    // PAR MARGES (voir `Flow`) : c'est la seule lecture fiable, l'ancrage
    // explicite étant vide sur certains stores. C'est la preuve la plus faible
    // des quatre : elle dit « ces mondes organisent la connaissance pareil »,
    // jamais « ces mondes partagent des entités ni un vécu » — d'où un rendu
    // distinct (trait discontinu) et un poids qui n'entre pas dans `weight`.
    for i in 0..metas.len() {
        for j in (i + 1)..metas.len() {
            let (a, b) = (&metas[i], &metas[j]);
            if a.flows.is_empty() || b.flows.is_empty() {
                continue;
            }
            let left: BTreeMap<(String, String, String), i64> =
                a.flows.iter().map(|f| (f.key(), f.bound())).collect();
            let mut shared: Vec<(String, String, String, i64, i64)> = b
                .flows
                .iter()
                .filter_map(|f| {
                    left.get(&f.key())
                        .map(|na| (f.src.clone(), f.rel.clone(), f.dst.clone(), *na, f.bound()))
                })
                .collect();
            if shared.is_empty() {
                continue;
            }
            shared.sort_by(|x, y| (y.3.min(y.4)).cmp(&(x.3.min(x.4))));
            let total = shared.len();
            shared.truncate(FLOW_SAMPLE);
            let key = if a.slug <= b.slug {
                (a.slug.clone(), b.slug.clone())
            } else {
                (b.slug.clone(), a.slug.clone())
            };
            let f = fibers.entry(key.clone()).or_insert_with(|| Fiber {
                a: key.0.clone(),
                b: key.1.clone(),
                ..Default::default()
            });
            f.flows = shared;
            f.flow_count = total;
        }
    }

    // ── masses & gravités ──
    // masse = log10(1 + nœuds), normalisée : c'est ce qui fait s'affaisser la
    // nappe et ce qui donne l'orbite.
    let max_log = metas
        .iter()
        .map(|m| (1.0 + m.nodes as f64).log10())
        .fold(0.0_f64, f64::max)
        .max(f64::EPSILON);
    // gravité d'un monde = intensité réelle de ses liens avec les autres
    // (nb d'ancres partagées) — l'attraction n'est pas décorative.
    let mut link_strength: BTreeMap<&str, i64> = BTreeMap::new();
    for f in fibers.values() {
        *link_strength.entry(f.a.as_str()).or_default() += f.weight() as i64;
        *link_strength.entry(f.b.as_str()).or_default() += f.weight() as i64;
    }
    let max_link = link_strength.values().copied().max().unwrap_or(1).max(1) as f64;

    let world_json: Vec<serde_json::Value> = metas
        .iter()
        .map(|m| {
            let mass = (1.0 + m.nodes as f64).log10() / max_log;
            let strength = *link_strength.get(m.slug.as_str()).unwrap_or(&0) as f64;
            let gravity = strength / max_link;
            serde_json::json!({
                "qn": format!("world:{}", m.slug),
                "slug": m.slug,
                "label": m.label,
                "path": m.path,
                "nature": m.nature(),
                "nodes": m.nodes,
                "edges": m.edges,
                "types": m.types.iter().map(|(l, c)| serde_json::json!({"label": l, "count": c})).collect::<Vec<_>>(),
                "sessions": m.sessions.len(),
                // La signature de flux RÉELLE du monde : ce que ce monde FAIT
                // (ses types, ses relations, ses volumes). C'est la preuve P4.
                "flows": m.flows.iter().map(|f| serde_json::json!({
                    "src": f.src, "rel": f.rel, "dst": f.dst,
                    "src_count": f.src_count, "dst_count": f.dst_count,
                    "bound": f.bound(),
                })).collect::<Vec<_>>(),
                "mass": (mass * 1000.0).round() / 1000.0,
                "gravity": (gravity * 1000.0).round() / 1000.0,
                // Orbite : plus un monde est attiré par le foyer, plus il est
                // proche du centre. Un monde sans lien reste en périphérie.
                "orbit": ((1.0 - gravity) * 0.85 + 0.15),
            })
        })
        .collect();

    // ── densification : la toile se tisse sur l'horloge RÉELLE ──
    // Chaque palier compte les synapses nées avant lui (preuve datée).
    let mut steps: Vec<serde_json::Value> = Vec::new();
    if t1 > t0 {
        let span = (t1 - t0) as f64;
        for k in 0..=DENSIFICATION_STEPS {
            let t = t0 + (span * k as f64 / DENSIFICATION_STEPS as f64) as i64;
            let mut count = 0usize;
            let mut weight = 0i64;
            for f in fibers.values() {
                let born = f.events.iter().filter(|e| **e <= t).count();
                if born > 0 {
                    count += 1;
                    weight += born as i64;
                }
            }
            steps.push(serde_json::json!({
                "t": t, "fibers": count, "weight": weight,
                "ratio": if t1 > t0 { (t - t0) as f64 / span } else { 1.0 },
            }));
        }
    }

    let fiber_json: Vec<serde_json::Value> = fibers
        .values()
        .map(|f| {
            serde_json::json!({
                "a": format!("world:{}", f.a),
                "b": format!("world:{}", f.b),
                "weight": f.weight(),
                "shared": f.anchors.len(),
                "bridged": f.bridged,
                "flows": f.flow_count,
                "flow_sample": f.flows.iter().map(|(s, r, d, na, nb)| serde_json::json!({
                    "src": s, "rel": r, "dst": d, "count_a": na, "count_b": nb,
                })).collect::<Vec<_>>(),
                "dated": f.events.len(),
                "first_seen": f.first_seen,
                "last_spike": f.last_spike,
                "sample": f.anchors.iter().take(SYNAPSE_SAMPLE).cloned().collect::<Vec<_>>(),
            })
        })
        .collect();

    let anchor_json: Vec<serde_json::Value> = weave_anchors
        .iter()
        .take(persist_cap)
        .map(|a| {
            serde_json::json!({
                "qn": a.qn, "label": a.label,
                "worlds": a.worlds.iter().cloned().collect::<Vec<_>>(),
                "first_seen": a.first_seen,
                "self": a.is_self, "user": a.is_user,
            })
        })
        .collect();

    let payload = serde_json::json!({
        "schema": WEAVE_SCHEMA,
        "generated_ms": now_ms(),
        "self": { "qn": opts.self_qn },
        "worlds": world_json,
        "fibers": fiber_json,
        "anchors": anchor_json,
        "clock": { "t0": t0, "t1": t1, "sessions": clock.len() },
        "densification": { "t0": t0, "t1": t1, "steps": steps },
        "counts": {
            "worlds": metas.len(),
            "anchors_scanned": anchors.len(),
            "anchors_woven": total_anchors,
            "anchors_persisted": persist_cap,
            "fibers": fibers.len(),
            "flows_scanned": metas.iter().map(|m| m.flows.len()).sum::<usize>(),
            "flow_ties": fibers.values().filter(|f| f.weight() == 0 && f.flow_count > 0).count(),
        },
    });

    Ok(Weave {
        payload,
        anchors: persist_cap,
        fibers: fibers.len(),
    })
}

/// Horodatage courant en millisecondes.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Persistance : la base synaptique de l'agent
// ---------------------------------------------------------------------------

/// Persiste la toile dans le store `out` — ouvert en lecture-écriture
/// (créé s'il n'existe pas), **renforcé** (jamais écrasé) puis flushé.
fn persist(out: &Path, payload: &serde_json::Value, self_qn: &str) -> Result<serde_json::Value> {
    // Ouverture EXCLUSIVE, volontairement. Le mode SHARED-WRITE (conçu pour
    // coexister avec les lecteurs RO du service) a été essayé puis écarté :
    // mesuré, les propriétés d'arête écrites après le dernier flush ne
    // survivent pas à la réouverture — le moteur l'annonce lui-même
    // (« husk signature — properties written after the last flush were
    // lost »), et le RENFORCEMENT des synapses ne s'incrémentait donc jamais
    // (constaté : reinforcement figé à 1 après trois tissages). Une synapse
    // qui ne s'épaissit pas n'est pas durable. `scripts/weave.sh` arrête le
    // service, tisse, puis le relance : le tissage est franc.
    let db = ObrainDB::open_substrate(out).with_context(|| {
        format!(
            "weave : ouverture RW de {} — si le dashboard tourne, il tient la base \
                 ouverte en lecture seule : utilisez `scripts/weave.sh` (arrêt, tissage, \
                 relance) ou arrêtez `obrain viz serve`",
            out.display()
        )
    })?;
    let now = now_ms();
    let mut written = 0usize;

    let run = |gql: &str| -> bool { db.session().execute(gql).is_ok() };

    // Le Self propriétaire de la base synaptique.
    let _ = run(&format!(
        "MERGE (s:Self {{qn: {}}}) SET s.role = 'agent', s.updated_ms = {} RETURN s",
        lit(self_qn),
        now
    ));
    written += 1;

    // Mondes : masse, gravité, orbite, nature.
    for w in payload["worlds"].as_array().into_iter().flatten() {
        let qn = w["qn"].as_str().unwrap_or_default();
        let gql = format!(
            "MERGE (w:World {{qn: {}}}) SET w.slug = {}, w.label = {}, w.path = {}, w.nature = {}, \
             w.nodes = {}, w.edges = {}, w.mass = {}, w.gravity = {}, w.orbit = {}, \
             w.types = {}, w.flows = {}, w.updated_ms = {} RETURN w",
            lit(qn),
            lit(w["slug"].as_str().unwrap_or_default()),
            lit(w["label"].as_str().unwrap_or_default()),
            lit(w["path"].as_str().unwrap_or_default()),
            lit(w["nature"].as_str().unwrap_or_default()),
            w["nodes"].as_i64().unwrap_or(0),
            w["edges"].as_i64().unwrap_or(0),
            w["mass"].as_f64().unwrap_or(0.0),
            w["gravity"].as_f64().unwrap_or(0.0),
            w["orbit"].as_f64().unwrap_or(1.0),
            lit(&w["types"].to_string()),
            lit(&w["flows"].to_string()),
            now
        );
        if run(&gql) {
            written += 1;
        }
        // Le self PORTE ses mondes : le rattachement est explicite.
        let _ = run(&format!(
            "MATCH (s:Self {{qn: {}}}), (w:World {{qn: {}}}) \
             MERGE (s)-[r:OWNS]->(w) SET r.role = 'monde' RETURN r",
            lit(self_qn),
            lit(qn)
        ));
    }

    // Ancres (les neurones de la toile) + appartenance à leurs mondes.
    let mut anchor_worlds: BTreeMap<String, (String, Vec<String>, bool)> = BTreeMap::new();
    for a in payload["anchors"].as_array().into_iter().flatten() {
        let qn = a["qn"].as_str().unwrap_or_default();
        let worlds: Vec<String> = a["worlds"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| x.as_str().map(String::from))
            .collect();
        let is_self = a["self"].as_bool().unwrap_or(false);
        let first_seen = a["first_seen"].as_i64().unwrap_or(0);
        let gql = format!(
            "MERGE (n:Anchor {{qn: {}}}) SET n.label = {}, n.first_seen = {}, \
             n.is_self = {}, n.world_count = {}, n.updated_ms = {} RETURN n",
            lit(qn),
            lit(a["label"].as_str().unwrap_or_default()),
            first_seen,
            is_self,
            worlds.len(),
            now
        );
        if run(&gql) {
            written += 1;
        }
        for ws in &worlds {
            let _ = run(&format!(
                "MATCH (n:Anchor {{qn: {}}}), (w:World {{qn: {}}}) \
                 MERGE (n)-[r:IN_WORLD]->(w) SET r.last_spike = {} RETURN r",
                lit(qn),
                lit(&format!("world:{ws}")),
                if first_seen > 0 { first_seen } else { now }
            ));
        }
        if is_self || worlds.is_empty() {
            // Ce qui porte le self est tissé DIRECTEMENT au Self.
            let _ = run(&format!(
                "MATCH (s:Self {{qn: {}}}), (n:Anchor {{qn: {}}}) \
                 MERGE (s)-[r:WEAVES]->(n) SET r.weight = 1, r.last_spike = {} RETURN r",
                lit(self_qn),
                lit(qn),
                now
            ));
        }
        anchor_worlds.insert(qn.to_string(), (String::new(), worlds, is_self));
    }

    // Fibres := synapses agrégées entre mondes. RENFORCÉES : chaque tissage
    // ajoute la différence observée, il ne remet pas le compteur à zéro.
    for f in payload["fibers"].as_array().into_iter().flatten() {
        let a = f["a"].as_str().unwrap_or_default();
        let b = f["b"].as_str().unwrap_or_default();
        let weight = f["weight"].as_i64().unwrap_or(0);
        let sample = f["sample"].to_string();
        // UNE seule instruction, et le RENFORCEMENT EN PREMIER : mesuré, lire
        // `r.reinforcement` APRÈS d'autres `SET` de la même clause renvoie null
        // (binding périmé) — le compteur restait alors figé à 1 et la synapse
        // ne s'épaississait jamais (vérifié : 1 au lieu de 2 après deux
        // tissages). L'ordre des affectations est donc porteur de sens ici.
        let gql = format!(
            "MATCH (a:World {{qn: {}}}), (b:World {{qn: {}}}) \
             MERGE (a)-[r:SYNAPSE]->(b) \
             SET r.reinforcement = coalesce(r.reinforcement, 0) + 1, \
                 r.weight = {}, r.anchors = {}, r.shared = {}, \
                 r.bridged = {}, r.flows = {}, r.flow_sample = {}, r.dated = {}, r.sample = {}, \
                 r.last_spike = {}, r.updated_ms = {} RETURN r",
            lit(a),
            lit(b),
            weight,
            weight,
            f["shared"].as_i64().unwrap_or(0),
            f["bridged"].as_i64().unwrap_or(0),
            f["flows"].as_i64().unwrap_or(0),
            lit(&f["flow_sample"].to_string()),
            f["dated"].as_i64().unwrap_or(0),
            lit(&sample),
            now,
            now
        );
        if run(&gql) {
            written += 1;
        }
    }

    // Courbe de densification persistée (une seule ligne, JSON).
    let _ = run(&format!(
        "MERGE (d:Densification {{qn: 'densification'}}) \
         SET d.steps = {}, d.t0 = {}, d.t1 = {}, d.sessions = {}, d.generated_ms = {} RETURN d",
        lit(&payload["densification"].to_string()),
        payload["clock"]["t0"].as_i64().unwrap_or(0),
        payload["clock"]["t1"].as_i64().unwrap_or(0),
        payload["clock"]["sessions"].as_i64().unwrap_or(0),
        now
    ));

    // Durabilité : le WAL est journalisé par le moteur, on force le flush.
    db.flush_wal().context("weave : flush du WAL")?;

    Ok(serde_json::json!({
        "store": out.to_string_lossy(),
        "writes": written,
        "generated_ms": now,
    }))
}

// ---------------------------------------------------------------------------
// Entrée publique
// ---------------------------------------------------------------------------

/// Tisse les mondes et persiste la base synaptique. Renvoie le payload complet
/// (celui que sert ensuite `/api/viz/weave`, augmenté du bloc `persisted`).
pub fn run_weave(opts: &WeaveOptions, quiet: bool) -> Result<serde_json::Value> {
    let weave = compute(opts)?;
    let persisted = persist(&opts.out, &weave.payload, &opts.self_qn)?;
    let mut payload = weave.payload;
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("persisted".into(), persisted);
    }
    if !quiet {
        let c = &payload["counts"];
        eprintln!(
            "weave : {} mondes · {} ancres tissées ({} persistées) · {} fibres · \
             horloge {} sessions → {}",
            c["worlds"],
            c["anchors_woven"],
            c["anchors_persisted"],
            c["fibers"],
            payload["clock"]["sessions"],
            opts.out.display()
        );
    }
    Ok(payload)
}

/// Relit la toile persistée (source de vérité durable) — utilisé par le
/// serveur viz. Renvoie `None` si la base synaptique n'existe pas encore.
pub fn read_persisted(out: &Path) -> Option<serde_json::Value> {
    let db = ObrainDB::open_read_only(out).ok()?;
    let self_qn = rows(&db, "MATCH (s:Self) RETURN s.qn LIMIT 1")
        .first()
        .and_then(|r| as_str(r.first()))
        .unwrap_or_default();
    let worlds: Vec<serde_json::Value> = rows(
        &db,
        "MATCH (w:World) RETURN w.qn, w.label, w.path, w.nature, w.nodes, w.edges, \
         w.mass, w.gravity, w.orbit, w.types, w.flows, w.slug",
    )
    .into_iter()
    .filter_map(|r| {
        let j = |i: usize| {
            as_str(r.get(i))
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .unwrap_or(serde_json::json!([]))
        };
        Some(serde_json::json!({
            "qn": as_str(r.first())?,
            "label": as_str(r.get(1))?,
            "path": as_str(r.get(2))?,
            "nature": as_str(r.get(3))?,
            "nodes": as_i64(r.get(4))?,
            "edges": as_i64(r.get(5)).unwrap_or(0),
            "types": j(9),
            "flows": j(10),
            // Le client s'appuie sur `slug` pour ses phases/inclinaisons : on le
            // relit plutôt que de le laisser retomber sur `qn` (contrat stable).
            "slug": as_str(r.get(11)).unwrap_or_default(),
            "mass": r.get(6).and_then(as_f64).unwrap_or(0.0),
            "gravity": r.get(7).and_then(as_f64).unwrap_or(0.0),
            "orbit": r.get(8).and_then(as_f64).unwrap_or(1.0),
        }))
    })
    .collect();
    let fibers: Vec<serde_json::Value> = rows(
        &db,
        "MATCH (a:World)-[r:SYNAPSE]->(b:World) RETURN a.qn, b.qn, r.weight, \
         r.reinforcement, r.shared, r.bridged, r.flows, r.dated, r.sample, r.flow_sample",
    )
    .into_iter()
    .filter_map(|r| {
        let j = |i: usize| {
            as_str(r.get(i))
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .unwrap_or(serde_json::json!([]))
        };
        Some(serde_json::json!({
            "a": as_str(r.first())?,
            "b": as_str(r.get(1))?,
            "weight": as_i64(r.get(2)).unwrap_or(0),
            "reinforcement": as_i64(r.get(3)).unwrap_or(0),
            "shared": as_i64(r.get(4)).unwrap_or(0),
            "bridged": as_i64(r.get(5)).unwrap_or(0),
            "flows": as_i64(r.get(6)).unwrap_or(0),
            "dated": as_i64(r.get(7)).unwrap_or(0),
            "sample": j(8),
            "flow_sample": j(9),
        }))
    })
    .collect();
    let densification = rows(&db, "MATCH (d:Densification) RETURN d.steps LIMIT 1")
        .first()
        .and_then(|r| as_str(r.first()))
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .unwrap_or_else(|| serde_json::json!({ "steps": [] }));
    // L'horloge réelle fait partie du contrat du dashboard (curseur de
    // densification) : on la relit, elle n'est pas un détail d'affichage.
    let clock = rows(
        &db,
        "MATCH (d:Densification) RETURN d.t0, d.t1, d.sessions LIMIT 1",
    )
    .first()
    .map(|r| {
        serde_json::json!({
            "t0": as_i64(r.first()).unwrap_or(0),
            "t1": as_i64(r.get(1)).unwrap_or(0),
            "sessions": as_i64(r.get(2)).unwrap_or(0),
        })
    })
    .unwrap_or_else(|| serde_json::json!({ "t0": 0, "t1": 0, "sessions": 0 }));
    let anchors_count = rows(&db, "MATCH (n:Anchor) RETURN count(n)")
        .first()
        .and_then(|r| as_i64(r.first()))
        .unwrap_or(0);
    // Le contrat du dashboard est stable : on expose les MÊMES clés de `counts`
    // que le tissage, y compris les fibres de parenté seule — sinon le panneau
    // affiche des trous après un simple redémarrage.
    let flow_ties = fibers
        .iter()
        .filter(|f| f["weight"].as_i64().unwrap_or(0) == 0 && f["flows"].as_i64().unwrap_or(0) > 0)
        .count();

    Some(serde_json::json!({
        "schema": WEAVE_SCHEMA,
        "generated_ms": now_ms(),
        "source": "persisted",
        "self": { "qn": self_qn },
        "worlds": worlds,
        "fibers": fibers,
        "densification": densification,
        "clock": clock,
        "counts": {
            "worlds": worlds.len(),
            "anchors_persisted": anchors_count,
            "anchors_woven": anchors_count,
            "fibers": fibers.len(),
            "flow_ties": flow_ties,
        },
        "persisted": { "store": out.to_string_lossy() },
    }))
}

/// Lit un flottant d'une valeur moteur.
fn as_f64(v: &OV) -> Option<f64> {
    match v {
        OV::Float64(x) => Some(*x),
        OV::Int64(x) => Some(*x as f64),
        OV::String(s) => s.parse().ok(),
        _ => None,
    }
}
