//! Extracteur de sous-graphes substrate pour la couche viz (P0-v04).
//!
//! **Une seule implémentation** partagée entre la CLI (`obrain viz`) et le
//! futur tool bridge MCP `viz_subgraph` (v05) — la parité des comptes est
//! garantie par construction (même code, même canal).
//!
//! Canal de lecture : le moteur GQL (`ObrainDB::session().execute`) pour la
//! résolution qn → id, le parcours BFS borné (nœuds/arêtes voisines dans les
//! deux directions, avec `type(r)` et `id(r)`) et les rôles structurels
//! (`CALL obrain.strongly_connected_components()`). Les **signaux persistés**
//! (energy Q1.15, ricci_u8, weight, stale, courbure nodale) sont lus sur le
//! `SubstrateStore` sous-jacent via `ObrainDB::substrate_handle()` quand il
//! existe (base substrate) — sinon les champs restent `None` (dégradation
//! propre, jamais d'erreur).
//!
//! Bornes (Décision 1 de la roadmap) : `hops ∈ [1, 3]`, cap global de nœuds
//! (`limit_nodes`) → `truncated: true` + `dropped` compté ; cap par requête
//! de voisinage pour ne jamais matérialiser un hub entier (5000 par direction).

use std::collections::{HashMap, HashSet, VecDeque};

use anyhow::{Result, bail};
use obrain_common::types::{NodeId, Value as OV};
use obrain_engine::ObrainDB;
use obrain_substrate::{SubstrateStore, edge_flags};
use obrain_viz::{Subgraph, VizEdge, VizNode};

/// Nombre de sauts maximum en mode voisinage (`hops`). `hops = 0` = arbre
/// complet (closure BFS jusqu'à épuisement, borné par `limit_nodes`).
pub const MAX_HOPS: u8 = 8;
/// Cap par direction/requête de voisinage (hub → échantillon borné).
/// Plafond des arêtes récupérées PAR NŒUD pendant le parcours.
///
/// Il valait 5 000 — et il COUPAIT la donnée : le store obrain a un hub à
/// 11 077 arêtes, dont 6 077 étaient donc invisibles, sans trace ni mention.
/// Un plafond qui tronque la structure d'un nœud fausse la topologie qu'on
/// prétend montrer. On le porte au niveau du plafond de la requête batchée.
const PER_QUERY_CAP: u64 = 500_000;

/// Options d'extraction d'un sous-graphe.
#[derive(Debug, Clone)]
pub struct ExtractOptions {
    /// Sauts : `0` = arbre complet (closure), sinon `1..=8`.
    pub hops: u8,
    /// Bornage optionnel de la PROFONDEUR de closure (`hops = 0`) : sans lui,
    /// une composante quasi infinie descend sans fin jusqu'au cap de nœuds.
    /// La réponse est alors marquée `truncated` (jamais une erreur).
    pub closure_max_depth: Option<u8>,
    /// Cap global de nœuds (aucun si `None`).
    pub limit_nodes: Option<usize>,
    /// Filtre sur les types d'arêtes (aucun filtre si `None`).
    pub edge_types: Option<Vec<String>>,
    /// Étendre vers les arêtes sortantes.
    pub outbound: bool,
    /// Étendre vers les arêtes entrantes.
    pub inbound: bool,
    /// Lire les signaux persistés (scc/energy/ricci/weight/curvature).
    pub signals: bool,
    /// Résolution automatique des suffixes ambigus : choisit déterministiquement
    /// le premier candidat (tri par qn). La CLI reste stricte (défaut false) ;
    /// le serveur de visualisation passe true pour ne jamais échouer.
    pub auto_suffix: bool,
}

impl Default for ExtractOptions {
    fn default() -> Self {
        Self {
            hops: 2,
            closure_max_depth: None,
            limit_nodes: None,
            edge_types: None,
            outbound: true,
            inbound: true,
            signals: true,
            auto_suffix: false,
        }
    }
}

/// Compte-rendu d'extraction (statistiques pour l'en-tête/le log).
#[derive(Debug, Clone)]
pub struct ExtractReport {
    pub center: NodeId,
    pub center_qn: Option<String>,
    pub nodes: usize,
    pub edges: usize,
    pub truncated: bool,
    /// Nœuds voisins rencontrés mais non inclus (cap global atteint).
    pub dropped: usize,
    /// Nœuds visités au total (inclut les vus une fois).
    pub visited: usize,
    /// Où le temps est réellement passé, en µs.
    ///
    /// En MICROsecondes, pas en millisecondes : un voisinage à 1 saut doit se
    /// compter en µs, et un compteur en ms afficherait « 0 » là où il faut
    /// justement voir le chiffre. Même réflexe que la décomposition du contexte
    /// — mesuré le 2026-09-12 : la MÊME fonction rendait une voisinage en
    /// **0,5 ms** par `/api/viz/subgraph` et en **~900 ms** par `/api/engine/ask`
    /// (répartition en ms : `voisinage: 922`, tout le reste à 0), sans qu'aucune
    /// des deux routes ne puisse dire quelle phase payait ces 900 ms.
    pub phases: Option<PhasesUs>,
}

/// La répartition du temps d'une extraction, en microsecondes.
///
/// `None` (et non zéro) sur les chemins qui ne décomposent pas : un zéro se
/// lirait comme une mesure faite, et ici un chiffre faux est pire qu'absent.
#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub struct PhasesUs {
    /// Résolution des centres (id / qn exact / suffixe).
    pub resolution: u64,
    /// Parcours des arêtes (la frontière, saut par saut).
    pub expansion: u64,
    /// Construction des nœuds et des arêtes rendus (signaux compris).
    pub construction: u64,
    /// Calcul des composantes fortement connexes locales.
    pub scc: u64,
}

fn bail_invalid_node_arg(arg: &str) -> Result<NodeId> {
    bail!("viz: nœud introuvable ou id invalide : {arg:?} (id numérique ou qualified_name exact)")
}

/// Résout un nœud : id numérique (`u64`) ou `qualified_name` exact (via le
/// canal GQL, index d'égalité substrate quand présent).
/// (NATIF-J5C, cible 2) Resolution STRICTE d'un centre pour l'endpoint HTTP :
/// id numerique OU qualified_name EXACT, rien d'autre. Contrairement a
/// [`resolve_node_opts`], AUCUN repli par nom ni par suffixe : un centre qui
/// ne correspond a aucun qualified_name exact est REFUSE (l'endpoint rend
/// alors 400 "centre introuvable"), plutot que de resoudre silencieusement
/// sur un noeud arbitraire et de lancer une expansion couteuse sur le mauvais
/// noeud.
pub(crate) fn resolve_center_strict(db: &ObrainDB, node_arg: &str) -> Result<NodeId> {
    if let Ok(n) = node_arg.parse::<u64>() {
        return Ok(NodeId(n));
    }
    if node_arg.contains(['\'', '"', '\\']) {
        bail!("centre invalide (guillemets/backslash interdits) : {node_arg}");
    }
    let exact = format!("MATCH (n) WHERE n.qualified_name = '{node_arg}' RETURN id(n) LIMIT 1");
    if let Ok(res) = db.session().execute(&exact)
        && let Some(OV::Int64(i)) = res.rows.first().and_then(|row| row.first())
    {
        return Ok(NodeId(*i as u64));
    }
    bail!("centre introuvable : {node_arg}")
}

pub fn resolve_node(db: &ObrainDB, node_arg: &str) -> Result<NodeId> {
    resolve_node_opts(db, node_arg, false)
}

/// Résolution avec option de repli automatique (serveur de visualisation).
pub(crate) fn resolve_node_opts(
    db: &ObrainDB,
    node_arg: &str,
    auto_suffix: bool,
) -> Result<NodeId> {
    if let Ok(n) = node_arg.parse::<u64>() {
        return Ok(NodeId(n));
    }
    if node_arg.contains(['\'', '"', '\\']) {
        bail!("viz: qualified_name invalide (guillemets/backslash interdits)");
    }
    let session = db.session();
    // 1) L'identité exacte, PAR L'UNE OU L'AUTRE CONVENTION — O(1)/µs.
    //
    // ── DEUX REQUÊTES, PAS UN `OR` (défaut mesuré le 2026-09-17) ────────────
    // Première écriture de cette correction : un seul prédicat
    // `WHERE n.qualified_name = 'x' OR n.qn = 'x'`. Elle est FAUSSE en coût, et
    // la mesure le dit sans ambiguïté sur la fixture du banc (`make_hub`, 8
    // nœuds `Function`) :
    //
    // ```text
    // MATCH (n) WHERE n.qualified_name = 'cible'            → 0 slot
    // MATCH (n) WHERE n.qn = 'cible'                        → 0 slot
    // MATCH (n) WHERE n.qualified_name = 'cible' OR n.qn = …→ 8 slots  ← tout le label
    // ```
    //
    // Un `OR` entre deux prédicats indexés n'est pas deux recherches d'index :
    // le planificateur ne peut plus ancrer sur l'un, et il balaie. On fait donc
    // les deux RECHERCHES, l'une après l'autre — même couverture, deux index.
    //
    // Le dépôt porte deux conventions, et c'est mesuré : `qn` (mémoire, registre
    // des machines — `memoire.rs`, `remote.rs`) et `qualified_name` (graphe de
    // code, indexeurs). Ce résolveur ne cherchait que la seconde : un nœud écrit
    // par la mémoire ou par la passe des machines — `machine:n3`, `signal:…`,
    // tout ce que produit `import_events` — était **introuvable** pour la
    // visualisation, alors que c'est exactement ce qu'on veut regarder. On
    // cherche donc les deux, dans la même requête (deux index d'égalité, aucune
    // des deux branches ne balaie).
    for cle in ["qualified_name", "qn"] {
        let exact = format!("MATCH (n) WHERE n.{cle} = '{node_arg}' RETURN id(n) LIMIT 1");
        if let Ok(res) = session.execute(&exact)
            && let Some(OV::Int64(i)) = res.rows.first().and_then(|row| row.first())
        {
            return Ok(NodeId(*i as u64));
        }
    }
    // 2-3) Repli V4-D par PALIERS, zéro scan tant qu'un index répond :
    //      nom exact (index d'égalité sur `name`, posé par l'indexeur code
    //      au même titre que qualified_name) PUIS suffixe de qn (`ENDS WITH
    //      '.node_arg'` — scan complet sur gros store, 1,2-3,4 s sur 1,55 M
    //      nœuds : on ne l'atteint que si le nom n'a rien donné).
    let tiers = [
        format!("MATCH (n) WHERE n.name = '{node_arg}' RETURN id(n), n.qualified_name LIMIT 10"),
        format!(
            "MATCH (n) WHERE n.qualified_name ENDS WITH '.{node_arg}'          RETURN id(n), n.qualified_name LIMIT 10"
        ),
        // Même repli par SUFFIXE, côté `qn`. DEUX séparateurs, et ce n'est pas
        // un luxe : les deux conventions ne découpent pas pareil. Le graphe de
        // code sépare par `.` (`test.repo.f`), la mémoire et le registre par `:`
        // (`machine:n3`, `signal:surprise:1`, `skill:…`). Un repli qui ne
        // connaîtrait que le point rendrait « n3:viz » introuvable — mesuré en
        // écrivant ce test. Le séparateur est là pour écarter les faux
        // positifs (« f » ne doit pas ramener « x.f » par un simple `ENDS WITH`),
        // donc on garde un séparateur, mais on les essaie tous les deux.
        format!(
            "MATCH (n) WHERE n.qn ENDS WITH ':{node_arg}'          RETURN id(n), n.qn LIMIT 10"
        ),
        format!(
            "MATCH (n) WHERE n.qn ENDS WITH '.{node_arg}'          RETURN id(n), n.qn LIMIT 10"
        ),
    ];
    for q in tiers {
        let Ok(res) = session.execute(&q) else {
            continue;
        };
        let mut by_qn: Vec<(u64, String)> = Vec::new();
        for row in res.rows {
            if let (Some(OV::Int64(i)), Some(OV::String(qq))) = (row.first(), row.get(1)) {
                // Doublons d'import : plusieurs slots portent le même qn — on
                // ne garde que le premier par qn (ambiguïté réelle = qn
                // distincts).
                let qs = qq.to_string();
                if !by_qn.iter().any(|(_, q2)| q2.as_str() == qs.as_str()) {
                    by_qn.push((*i as u64, qs));
                }
            }
        }
        if by_qn.is_empty() {
            continue;
        }
        match by_qn.len() {
            1 => return Ok(NodeId(by_qn[0].0)),
            _ if auto_suffix => {
                // Choix déterministe (premier qn trié) — la visualisation ne
                // doit jamais échouer sur une ambiguïté ; la CLI reste stricte.
                by_qn.sort_by(|a, b| a.1.cmp(&b.1));
                return Ok(NodeId(by_qn[0].0));
            }
            _ => {
                let cands = by_qn
                    .iter()
                    .map(|(_, qq)| qq.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                bail!(
                    "viz: suffixe ambigu {node_arg:?} — {} candidats : {cands} (donner un qualified_name exact)",
                    by_qn.len()
                );
            }
        }
    }
    bail_invalid_node_arg(node_arg)
}

/// Composante fortement connexe de chaque nœud (une passe moteur).
/// Composantes FORTEMENT CONNES du SOUS-GRAPHE retourné (Kosaraju, itératif).
///
/// Le calcul était GLOBAL (`CALL obrain.strongly_connected_components()`) et
/// ABANDONNÉ au-dessus de 300 000 nœuds, parce qu'il coûte des minutes sur un
/// gros store. Mesuré : c'est le cas de TOUTES les bases réelles —
/// runtime-dev 372 750 nœuds, linux-full3 1 553 071. `scc_id` restait donc vide
/// et l'overlay `roles` ne posait RIEN : un paramètre qui promet une topologie
/// (cycles, frontières de communautés) et n'en fournit aucune.
///
/// Or un identifiant GLOBAL de composante n'a aucun sens dans une projection de
/// 300 nœuds : ce qui compte pour le moteur qui lit la projection, c'est la
/// structure cyclique DE CE QU'ON LUI MONTRE. On la calcule donc localement, sur
/// les arêtes déjà collectées : O(V+E) sur quelques centaines de nœuds, soit des
/// microsecondes au lieu de minutes.
///
/// Déterministe : les racines sont parcourues dans l'ordre STABLE de `order`, et
/// les identifiants sont attribués dans l'ordre de découverte — deux appels sur
/// le même sous-graphe rendent les mêmes composantes.
fn scc_locales(order: &[u64], edges: &[(u64, u64, String, Option<u64>)]) -> HashMap<u64, u64> {
    let presents: HashSet<u64> = order.iter().copied().collect();
    let mut sortants: HashMap<u64, Vec<u64>> = HashMap::new();
    let mut entrants: HashMap<u64, Vec<u64>> = HashMap::new();
    for (a, b, _, _) in edges {
        // Les boucles sur soi ne créent pas de cycle au sens fort, et une arête
        // vers un nœud hors projection ne relie rien DANS la projection.
        if a == b || !presents.contains(a) || !presents.contains(b) {
            continue;
        }
        sortants.entry(*a).or_default().push(*b);
        entrants.entry(*b).or_default().push(*a);
    }
    // 1) ordre de sortie du DFS sur le graphe direct (itératif : pas de
    //    récursion, donc pas de débordement de pile sur un long chemin).
    let mut vu: HashSet<u64> = HashSet::new();
    let mut sortie: Vec<u64> = Vec::with_capacity(order.len());
    for racine in order {
        if !vu.insert(*racine) {
            continue;
        }
        let mut pile: Vec<(u64, usize)> = vec![(*racine, 0)];
        while let Some((n, i)) = pile.pop() {
            let vides: [u64; 0] = [];
            let voisins = sortants.get(&n).map_or(vides.as_slice(), Vec::as_slice);
            if i < voisins.len() {
                pile.push((n, i + 1));
                let v = voisins[i];
                if vu.insert(v) {
                    pile.push((v, 0));
                }
            } else {
                sortie.push(n);
            }
        }
    }
    // 2) composantes sur le graphe INVERSÉ, en dépilant l'ordre de sortie.
    let mut compo: HashMap<u64, u64> = HashMap::new();
    let mut suivant = 0u64;
    while let Some(n) = sortie.pop() {
        if compo.contains_key(&n) {
            continue;
        }
        let id = suivant;
        suivant += 1;
        compo.insert(n, id);
        let mut pile = vec![n];
        while let Some(x) = pile.pop() {
            if let Some(voisins) = entrants.get(&x) {
                for v in voisins {
                    if !compo.contains_key(v) {
                        compo.insert(*v, id);
                        pile.push(*v);
                    }
                }
            }
        }
    }
    compo
}

fn scc_map(db: &ObrainDB) -> HashMap<u64, u64> {
    let mut map = HashMap::new();
    let session = db.session();
    if let Ok(res) = session.execute(
        "CALL obrain.strongly_connected_components() YIELD node_id, component_id RETURN node_id, component_id",
    ) {
        for row in res.rows {
            if let (Some(OV::Int64(n)), Some(OV::Int64(c))) = (row.first(), row.get(1)) {
                map.insert(*n as u64, *c as u64);
            }
        }
    }
    map
}

/// Premier label d'une valeur `labels(n)` renvoyée par le moteur.
///
/// Le moteur rend une liste (`List([String("Function")])`) ; un nœud sans label
/// rend une liste vide. On prend le premier, et on n'invente rien sinon.
fn premier_label(v: Option<&OV>) -> Option<String> {
    match v? {
        OV::List(items) => items.iter().find_map(|x| match x {
            OV::String(s) => Some(s.to_string()),
            _ => None,
        }),
        OV::String(s) => Some(s.to_string()),
        _ => None,
    }
}

fn outbound_query(id: u64) -> String {
    format!(
        "MATCH (a)-[r]->(b) WHERE id(a) = {id} RETURN id(b), type(r), id(r) LIMIT {PER_QUERY_CAP}"
    )
}

fn inbound_query(id: u64) -> String {
    format!(
        "MATCH (a)-[r]->(b) WHERE id(b) = {id} RETURN id(a), type(r), id(r) LIMIT {PER_QUERY_CAP}"
    )
}

/// Énergie (Q1.15 décodée) + stale d'un nœud via le record substrate.
#[derive(Default)]
struct NodeSignals {
    energy: Option<f64>,
    curvature: Option<f64>,
    community: Option<u64>,
}

/// Signaux d'un nœud via le record substrate : energy, curvature, community.
/// La communauté (colonne substrate) est le support de la représentation
/// « spectrale » des voisinages.
fn node_signal(store: &SubstrateStore, id: u64) -> Option<NodeSignals> {
    // Ids substrate slot-based en u32 dans les records.
    let Ok(id32) = u32::try_from(id) else {
        return None;
    };
    let rec = store.writer().read_node(id32).ok().flatten()?;
    if rec.is_tombstoned() {
        return None;
    }
    let mut out = NodeSignals::default();
    out.energy = Some(f64::from(rec.energy_q15()));
    // Courbure nodale agrégée (ricci incident moyen) — lecture seule, sans
    // recompute ; None si nœud isolé.
    out.curvature = obrain_substrate::node_curvature(store, NodeId(id)).map(f64::from);
    // u32::MAX = non assigné (sentinel substrate).
    if rec.community_id != u32::MAX {
        out.community = Some(u64::from(rec.community_id));
    }
    Some(out)
}

/// (ricci, weight, stale) d'une arête via le record substrate.
fn edge_signal(store: &SubstrateStore, eid: u64) -> Option<(f64, f64, Option<bool>)> {
    let rec = store.writer().read_edge(eid).ok().flatten()?;
    if rec.is_tombstoned() {
        return None;
    }
    // ricci_u8 == 0 est la sentinelle « non calculé » (-1.0 par convention de
    // quantisation) — on ne la propage pas comme une vraie courbure.
    let ricci = if rec.ricci_u8 == 0 {
        0.0
    } else {
        f64::from(rec.ricci_f32())
    };
    let weight = f64::from(rec.weight_f32());
    let stale = (rec.flags & edge_flags::RICCI_STALE) != 0;
    Some((ricci, weight, stale.then_some(true)))
}

fn prop_f64(props: &obrain_common::types::PropertyMap, key: &str) -> Option<f64> {
    let (_, v) = props.iter().find(|(k, _)| k.as_str() == key)?;
    match v {
        OV::Float64(f) => Some(*f),
        OV::Int64(i) => Some(*i as f64),
        OV::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

fn prop_str(props: &obrain_common::types::PropertyMap, key: &str) -> Option<String> {
    props
        .iter()
        .find(|(k, _)| k.as_str() == key)
        .and_then(|(_, v)| match v {
            OV::String(s) => Some(s.to_string()),
            _ => None,
        })
}

/// Construit le nœud `VizNode` (props moteur + signaux substrate optionnels).
fn build_node(
    db: &ObrainDB,
    store: Option<&SubstrateStore>,
    id: u64,
    scc: &HashMap<u64, u64>,
    kinds: &HashMap<u64, String>,
) -> VizNode {
    let mut node = VizNode::new(id.to_string());
    if let Some(nd) = db.session().get_node(NodeId(id)) {
        if let Some(qn) = prop_str(&nd.properties, "qualified_name") {
            node.qn = Some(qn);
        }
        if let Some(name) = prop_str(&nd.properties, "name") {
            node.label = Some(name);
        }
        if let Some(fp) = prop_str(&nd.properties, "file_path") {
            node.file_path = Some(fp);
        }
        // Signaux volatils portés par propriétés persistées (V1-A) : présents
        // sur les stores qui les écrivent (enrichisseurs cognitifs) — absents
        // ailleurs → overlays fsrs/engrams sans effet (dégradation propre).
        node.engram_id = prop_str(&nd.properties, "engram_id");
        node.fsrs_stability = prop_f64(&nd.properties, "fsrs_stability");
        node.fsrs_difficulty = prop_f64(&nd.properties, "fsrs_difficulty");
    }
    if let Some(c) = scc.get(&id) {
        node.scc_id = Some(*c);
    }
    node.kind = kinds.get(&id).cloned();
    if let Some(store) = store
        && let Some(sig) = node_signal(store, id)
    {
        node.energy = sig.energy;
        node.curvature = sig.curvature;
        node.community_id = sig.community;
    }
    node
}

/// Extrait un sous-graphe rendable autour de `center` (BFS deux directions,
/// borné). `hops = 0` = arbre complet depuis le centre.
pub fn extract_subgraph(
    db: &ObrainDB,
    center: &str,
    opts: &ExtractOptions,
) -> Result<(Subgraph, ExtractReport)> {
    extract_subgraph_centers(db, std::slice::from_ref(&center.to_string()), opts)
}

/// Variante multi-centres : union des sous-graphes de chaque centre (mêmes
/// bornes, même graphe de sortie — les centres sont les seeds du BFS).
/// Journalise une expansion d'arêtes quand `OBRAIN_DEBUG_EXPAND` est posé.
///
/// Défaut mesuré le 2026-09-12 : la MÊME fonction rendait un voisinage en
/// **0,5 ms** par `/api/viz/subgraph` et en **~890 ms** par `/api/engine/ask`, et
/// le cache de requêtes du moteur (clé = texte normalisé, stable dans le temps :
/// 866 ms puis 1,0 ms sur la même requête) rendait 3 ms au second appel du même
/// texte. Savoir QUEL texte partait, dans quel sens, et combien il coûtait,
/// n'était observable nulle part — seulement le total. D'où ce journal.
fn debug_expansion(sens: &str, ids: &str, us: u128, lignes: usize) {
    if std::env::var_os("OBRAIN_DEBUG_EXPAND").is_some() {
        eprintln!("[expand] sens={sens} ids=[{ids}] {us} µs · {lignes} ligne(s)");
    }
}

pub fn extract_subgraph_centers(
    db: &ObrainDB,
    centers: &[String],
    opts: &ExtractOptions,
) -> Result<(Subgraph, ExtractReport)> {
    if centers.is_empty() {
        bail!("viz: au moins un centre requis");
    }
    if opts.hops > MAX_HOPS {
        bail!("viz: hops doit être 0 (arbre complet) ou entre 1 et {MAX_HOPS}");
    }
    if !opts.outbound && !opts.inbound {
        bail!("viz: au moins une direction (outbound ou inbound) requise");
    }
    // Résolution de TOUS les centres (id / qn exact / suffixe non ambigu).
    let t_phases = std::time::Instant::now();
    let mut seed_ids: Vec<u64> = Vec::new();
    let mut first_qn: Option<String> = None;
    for c in centers {
        let id = resolve_node_opts(db, c, opts.auto_suffix)?;
        if !seed_ids.contains(&id.0) {
            if seed_ids.is_empty() {
                // qn informatif du premier centre (recherché dans le graphe final)
                first_qn = Some(c.clone());
            }
            seed_ids.push(id.0);
        }
    }
    let mut phases = PhasesUs {
        resolution: t_phases.elapsed().as_micros() as u64,
        ..Default::default()
    };
    let store = if opts.signals {
        db.substrate_handle()
    } else {
        None
    };
    // SCC = composantes FORTEMENT connexes du graphe ENTIER : sur les très
    // gros stores (kernel ~4,8M arêtes) c'est des minutes par requête. On ne
    // le calcule que sous un seuil de taille — sinon `scc_id` reste vide et
    // l'overlay `roles` se dégrade proprement : la visualisation doit
    // répondre en instantané, pas attendre un SCC global.
    let closure = opts.hops == 0;

    let mut seen: HashSet<u64> = HashSet::new();
    let mut order: Vec<u64> = Vec::new();
    let mut edge_set: HashSet<(u64, u64, String)> = HashSet::new();
    // TYPE DE CHAQUE NŒUD, collecté au passage par le parcours batché : la
    // requête d'expansion tourne DÉJÀ, on y ajoute `labels(…)` — coût nul.
    let mut kinds: HashMap<u64, String> = HashMap::new();
    let mut edges: Vec<(u64, u64, String, Option<u64>)> = Vec::new();
    let mut truncated = false;
    let mut dropped = 0usize;
    let cap = opts.limit_nodes.unwrap_or(usize::MAX);

    for id in &seed_ids {
        if order.len() >= cap {
            truncated = true;
            dropped += 1;
            continue;
        }
        seen.insert(*id);
        order.push(*id);
    }

    // Implémentation effective : requêtes batchées renvoyant les deux
    // extrémités (a et b) pour préserver la direction réelle.
    let mut frontier: Vec<u64> = order.clone();
    let mut depth = 0u8;
    // Profondeur maximale : `hops` en mode voisinage ; en closure, la borne
    // optionnelle (sinon illimité — la composante entière jusqu'au cap).
    let max_depth = if closure {
        opts.closure_max_depth.unwrap_or(u8::MAX)
    } else {
        opts.hops
    };
    let t_exp = std::time::Instant::now();
    while !frontier.is_empty() && depth < max_depth {
        // (NATIF-J5C) ARRET ANTICIPE : si le cap de noeuds est deja atteint,
        // aucune planche supplementaire ne peut ajouter de noeud — l expanser
        // ne ferait que materialiser sa frontiere d aretes pour rien (c est la
        // cause des 27 s a limit=2000 : la frontiere profonde de ~2000 noeuds
        // etait entierement expansee alors que `order` etait plein). On tronque
        // et on sort. Sans troncature (cap = usize::MAX) cette garde ne se
        // declenche jamais : le resultat est identique au comportement d avant.
        if order.len() >= cap {
            truncated = true;
            break;
        }
        depth += 1;
        let mut next: Vec<u64> = Vec::new();
        if opts.outbound {
            for chunk in frontier.chunks(500) {
                let ids = chunk
                    .iter()
                    .map(u64::to_string)
                    .collect::<Vec<_>>()
                    .join(",");
                let q = format!(
                    "MATCH (a)-[r]->(b) WHERE id(a) IN [{ids}]                      RETURN id(a), id(b), type(r), id(r), labels(a), labels(b) LIMIT 500000"
                );
                let t_q = std::time::Instant::now();
                let res = db.session().execute(&q);
                if let Ok(res) = res {
                    debug_expansion("sortant", &ids, t_q.elapsed().as_micros(), res.rows.len());
                    for row in res.rows {
                        let (Some(OV::Int64(sa)), Some(OV::Int64(sb)), Some(OV::String(ty)), ev) =
                            (row.first(), row.get(1), row.get(2), row.get(3))
                        else {
                            continue;
                        };
                        // Types d'entité, sans requête supplémentaire.
                        if let Some(k) = premier_label(row.get(4)) {
                            kinds.entry(*sa as u64).or_insert(k);
                        }
                        if let Some(k) = premier_label(row.get(5)) {
                            kinds.entry(*sb as u64).or_insert(k);
                        }
                        let (sa, sb) = (*sa as u64, *sb as u64);
                        let ty = ty.to_string();
                        if let Some(types) = &opts.edge_types
                            && !types.iter().any(|t| t == &ty)
                        {
                            continue;
                        }
                        let eid = match ev {
                            Some(OV::Int64(e)) => Some(*e as u64),
                            _ => None,
                        };
                        if !edge_set.insert((sa, sb, ty.clone())) {
                            continue;
                        }
                        edges.push((sa, sb, ty, eid));
                        if !seen.contains(&sb) {
                            if order.len() >= cap {
                                truncated = true;
                                dropped += 1;
                                continue;
                            }
                            seen.insert(sb);
                            order.push(sb);
                            next.push(sb);
                        }
                    }
                }
            }
        }
        if opts.inbound {
            for chunk in frontier.chunks(500) {
                let ids = chunk
                    .iter()
                    .map(u64::to_string)
                    .collect::<Vec<_>>()
                    .join(",");
                let q = format!(
                    // (NATIF-J5C) SYNTAXE INVERSEE : `(b)<-[r]-(a)` ancre sur b et
                    // remonte sa chaine entrante NATIVE (walk_incoming_chain, la
                    // meme route que le re-ancrage livre en J5) — 0 slot. La forme
                    // `(a)-[r]->(b) WHERE id(b) IN` balayait TOUTE la zone (316511
                    // slots par chunk, meme pour un seul id). a et b gardent leurs
                    // roles (a=source, b=cible), donc les colonnes sont inchangees.
                    "MATCH (b)<-[r]-(a) WHERE id(b) IN [{ids}]                      RETURN id(a), id(b), type(r), id(r) LIMIT 500000"
                );
                let t_q = std::time::Instant::now();
                let res = db.session().execute(&q);
                if let Ok(res) = res {
                    debug_expansion("entrant", &ids, t_q.elapsed().as_micros(), res.rows.len());
                    for row in res.rows {
                        let (Some(OV::Int64(sa)), Some(OV::Int64(sb)), Some(OV::String(ty)), ev) =
                            (row.first(), row.get(1), row.get(2), row.get(3))
                        else {
                            continue;
                        };
                        let (sa, sb) = (*sa as u64, *sb as u64);
                        let ty = ty.to_string();
                        if let Some(types) = &opts.edge_types
                            && !types.iter().any(|t| t == &ty)
                        {
                            continue;
                        }
                        let eid = match ev {
                            Some(OV::Int64(e)) => Some(*e as u64),
                            _ => None,
                        };
                        if !edge_set.insert((sa, sb, ty.clone())) {
                            continue;
                        }
                        edges.push((sa, sb, ty, eid));
                        if !seen.contains(&sa) {
                            if order.len() >= cap {
                                truncated = true;
                                dropped += 1;
                                continue;
                            }
                            seen.insert(sa);
                            order.push(sa);
                            next.push(sa);
                        }
                    }
                }
            }
        }
        frontier = next;
    }
    // Closure interrompue par la borne de profondeur (frontier non vide) :
    // ce n'est pas une erreur — la réponse est marquée tronquée comme pour le
    // cap de nœuds. En mode voisinage (hops), une frontière restante est
    // normale (dernier saut non expansé) → jamais tronquée pour ça.
    if closure && !frontier.is_empty() && !truncated {
        truncated = true;
    }
    phases.expansion = t_exp.elapsed().as_micros() as u64;

    // ── LE SOUS-GRAPHE EST INDUIT : AUCUNE ARETE ORPHELINE (2026-09-15) ─────
    //
    // `edges` est collecté PENDANT l'expansion, AVANT que le plafond de nœuds
    // n'ait décidé si le voisin entre dans `order` (le `edges.push` précède le
    // test `order.len() >= cap`). Une arête dont un bout n'est pas servi décrit
    // donc un lien que le RENDU NE PEUT PAS TRACER.
    //
    // Mesuré sur `runtime-dev`, centre
    // `adr:dsh-runtime/architecture/relief-complet-et-propagation`,
    // `hops=1&limit=500` : **500 nœuds, 24 502 arêtes, dont 24 003 (98,0 %)
    // pointaient vers un nœud absent** — le centre a 24 502 sorties et le
    // plafond n'en sert que 500. La charge faisait 2 102 520 octets ; le
    // sous-graphe induit en fait 499 arêtes, soit ~43 Ko. Et le client ne
    // gaspillait pas que la bande passante : `adjacency(edges)` alimente ses
    // triangles et ses cliques, qui tournaient donc sur 24 003 nœuds fantômes.
    //
    // Le filtre est posé APRÈS que `order` est définitif, donc au même endroit
    // pour les deux branches d'expansion (sortante et entrante) : c'est
    // l'invariant « les arêtes servies sont celles ENTRE les nœuds servis ».
    // Quand rien n'est tronqué, tout bout est déjà dans `order` : le filtre est
    // un NO-OP (vérifié par `identite_a_limit_non_tronquant`).
    //
    // `truncated` est armé si des arêtes tombent : perdre des liens sans le
    // dire serait exactement le mensonge que ce drapeau existe pour éviter.
    {
        let servis: std::collections::HashSet<u64> = order.iter().copied().collect();
        let avant = edges.len();
        edges.retain(|(a, b, _, _)| servis.contains(a) && servis.contains(b));
        if edges.len() != avant {
            truncated = true;
        }
    }

    // LE CALCUL LOCAL SE FAIT ICI : les arêtes sont toutes collectées, et c'est
    // exactement la structure qu'on s'apprête à montrer (voir `scc_locales`).
    let t_scc = std::time::Instant::now();
    let scc = scc_locales(&order, &edges);
    phases.scc = t_scc.elapsed().as_micros() as u64;
    let t_con = std::time::Instant::now();
    let nodes: Vec<VizNode> = order
        .iter()
        .map(|id| build_node(db, store.as_deref(), *id, &scc, &kinds))
        .collect();

    let mut out_edges: Vec<VizEdge> = Vec::with_capacity(edges.len());
    for (src, dst, ty, eid) in &edges {
        let mut e = VizEdge::new(src.to_string(), dst.to_string(), ty.clone());
        if let (Some(store), Some(eid)) = (&store, eid)
            && let Some((ricci, weight, stale)) = edge_signal(store, *eid)
        {
            e.ricci = Some(ricci);
            e.weight = Some(weight);
            e.stale = stale;
        }
        out_edges.push(e);
    }
    phases.construction = t_con.elapsed().as_micros() as u64;

    let sg = Subgraph {
        nodes,
        edges: out_edges,
        truncated,
    }
    .canonical();

    let center_id = *seed_ids.first().unwrap_or(&u64::MAX);
    let center_qn = first_qn
        .as_ref()
        .and_then(|qn| {
            sg.nodes
                .iter()
                .find(|n| n.qn.as_deref() == Some(qn.as_str()))
                .map(|n| n.qn.clone().unwrap())
        })
        .or_else(|| {
            sg.nodes
                .iter()
                .find(|n| n.id == center_id.to_string())
                .and_then(|n| n.qn.clone())
        });
    let report = ExtractReport {
        center: obrain_common::types::NodeId(center_id),
        center_qn,
        nodes: sg.nodes.len(),
        edges: sg.edges.len(),
        truncated,
        dropped,
        visited: order.len(),
        phases: Some(phases),
    };
    Ok((sg, report))
}

/// Export du store **entier** (nœuds + arêtes vivants) en un sous-graphe.
/// Matérialisation en mémoire — le writer streaming viendra si le bench
/// (v06) le justifie.
pub fn export_all(db: &ObrainDB, opts: &ExtractOptions) -> Result<(Subgraph, ExtractReport)> {
    let store = if opts.signals {
        db.substrate_handle()
    } else {
        None
    };
    // SCC = composantes FORTEMENT connexes du graphe ENTIER : sur les très
    // gros stores (kernel ~4,8M arêtes) c'est des minutes par requête. On ne
    // le calcule que sous un seuil de taille — sinon `scc_id` reste vide et
    // l'overlay `roles` se dégrade proprement : la visualisation doit
    // répondre en instantané, pas attendre un SCC global.
    const SCC_MAX_STORE_NODES: u32 = 300_000;
    let scc = if store
        .as_ref()
        .is_some_and(|st| st.node_count_live_estimate() <= SCC_MAX_STORE_NODES)
    {
        scc_map(db)
    } else {
        HashMap::new()
    };
    let session = db.session();

    let mut order: Vec<u64> = Vec::new();
    // Le TYPE D'ENTITÉ est lu par la MÊME requête (coût nul) : sans lui, un
    // client ne peut pas adapter sa vue à ce qu'il regarde.
    let mut kinds: HashMap<u64, String> = HashMap::new();
    if let Ok(res) = session.execute("MATCH (n) RETURN id(n), labels(n)") {
        for row in res.rows {
            if let Some(OV::Int64(i)) = row.first() {
                order.push(*i as u64);
                if let Some(k) = premier_label(row.get(1)) {
                    kinds.insert(*i as u64, k);
                }
            }
        }
    }
    let nodes: Vec<VizNode> = order
        .iter()
        .map(|id| build_node(db, store.as_deref(), *id, &scc, &kinds))
        .collect();

    let mut edge_set: HashSet<(u64, u64, String)> = HashSet::new();
    let mut edges: Vec<(u64, u64, String, Option<u64>)> = Vec::new();
    if let Ok(res) = session.execute("MATCH (a)-[r]->(b) RETURN id(a), type(r), id(b), id(r)") {
        for row in res.rows {
            let (Some(OV::Int64(s)), Some(OV::String(t)), Some(OV::Int64(d)), ev) =
                (row.first(), row.get(1), row.get(2), row.get(3))
            else {
                continue;
            };
            let (s, d) = (*s as u64, *d as u64);
            let ty = t.to_string();
            if let Some(types) = &opts.edge_types
                && !types.iter().any(|x| x == &ty)
            {
                continue;
            }
            let eid = match ev {
                Some(OV::Int64(e)) => Some(*e as u64),
                _ => None,
            };
            if edge_set.insert((s, d, ty.clone())) {
                edges.push((s, d, ty, eid));
            }
        }
    }

    let mut out_edges: Vec<VizEdge> = Vec::with_capacity(edges.len());
    for (src, dst, ty, eid) in &edges {
        let mut e = VizEdge::new(src.to_string(), dst.to_string(), ty.clone());
        if let (Some(store), Some(eid)) = (&store, eid)
            && let Some((ricci, weight, stale)) = edge_signal(store, *eid)
        {
            e.ricci = Some(ricci);
            e.weight = Some(weight);
            e.stale = stale;
        }
        out_edges.push(e);
    }

    let sg = Subgraph {
        nodes,
        edges: out_edges,
        truncated: false,
    }
    .canonical();
    let report = ExtractReport {
        center: NodeId(u64::MAX), // non applicable
        center_qn: None,
        nodes: sg.nodes.len(),
        edges: sg.edges.len(),
        truncated: false,
        dropped: 0,
        visited: sg.nodes.len(),
        // Ce chemin ne décompose pas ses phases : `None`, jamais un zéro qui se
        // lirait comme une mesure.
        phases: None,
    };
    Ok((sg, report))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    /// Mini graphe substrate jetable : a →(CALLS) b, b →(CALLS) c, a →(IMPORTS) d.
    fn make_db() -> (ObrainDB, String, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "obrain-viz-test-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let store = dir.join("t.obrain");
        let db = ObrainDB::open_substrate(store.to_str().unwrap()).expect("open substrate");
        let a = db.create_node(&["Function"]);
        db.set_node_property(a, "qualified_name", OV::from("test.a".to_string()));
        let b = db.create_node(&["Function"]);
        db.set_node_property(b, "qualified_name", OV::from("test.b".to_string()));
        let c = db.create_node(&["Function"]);
        db.set_node_property(c, "qualified_name", OV::from("test.c".to_string()));
        let d = db.create_node(&["Struct"]);
        db.set_node_property(d, "qualified_name", OV::from("test.d".to_string()));
        let _ = db.create_edge(a, b, "CALLS");
        let _ = db.create_edge(b, c, "CALLS");
        let _ = db.create_edge(a, d, "IMPORTS");
        db.create_property_index("qualified_name");
        db.checkpoint().unwrap();
        (db, store.to_string_lossy().to_string(), dir)
    }

    fn ids(sg: &Subgraph) -> Vec<String> {
        sg.nodes.iter().map(|n| n.id.clone()).collect()
    }

    #[test]
    fn resolve_by_qn_and_id() {
        let (db, _, _) = make_db();
        assert_eq!(resolve_node(&db, "test.b").unwrap().0, 2);
        assert_eq!(resolve_node(&db, "2").unwrap().0, 2);
        assert!(resolve_node(&db, "missing.qn").is_err());
        assert!(resolve_node(&db, "bad'qn").is_err());
    }

    #[test]
    fn one_hop_undirected_includes_neighbors_and_edge_types() {
        let (db, _, _) = make_db();
        let opts = ExtractOptions {
            hops: 1,
            ..Default::default()
        };
        let (sg, report) = extract_subgraph(&db, "test.a", &opts).unwrap();
        assert_eq!(report.nodes, 3); // a + b (CALLS) + d (IMPORTS)
        assert_eq!(report.edges, 2);
        assert!(!report.truncated);
        assert!(sg.validate().is_ok());
        // Signaux : a porte un scc (moteur SCC appelé quand signals = true).
        assert!(
            sg.nodes
                .iter()
                .find(|n| n.qn.as_deref() == Some("test.a"))
                .unwrap()
                .scc_id
                .is_some()
        );
        let types: Vec<&str> = sg.edges.iter().map(|e| e.edge_type.as_str()).collect();
        assert!(types.contains(&"CALLS") && types.contains(&"IMPORTS"));
    }

    #[test]
    fn two_hops_undirected_reaches_transitive_caller() {
        let (db, _, _) = make_db();
        let opts = ExtractOptions {
            hops: 2,
            ..Default::default()
        };
        let (sg, report) = extract_subgraph(&db, "test.a", &opts).unwrap();
        assert_eq!(report.nodes, 4); // a,b,c,d
        assert_eq!(report.edges, 3);
        assert_eq!(ids(&sg).len(), 4);
        let has_qn = |q: &str| sg.nodes.iter().any(|n| n.qn.as_deref() == Some(q));
        assert!(has_qn("test.c"));
        assert!(has_qn("test.d"));
    }

    #[test]
    fn edge_type_filter_keeps_only_calls() {
        let (db, _, _) = make_db();
        let opts = ExtractOptions {
            hops: 1,
            edge_types: Some(vec!["CALLS".into()]),
            ..Default::default()
        };
        let (sg, _) = extract_subgraph(&db, "test.a", &opts).unwrap();
        assert_eq!(sg.edges.len(), 1);
        assert_eq!(sg.edges[0].edge_type, "CALLS");
        assert_eq!(sg.nodes.len(), 2); // d exclu (IMPORTS filtrée)
    }

    #[test]
    fn outbound_only_does_not_include_callers() {
        let (db, _, _) = make_db();
        let opts = ExtractOptions {
            hops: 2,
            inbound: false,
            ..Default::default()
        };
        let (sg, _) = extract_subgraph(&db, "test.c", &opts).unwrap();
        // c n'a pas d'arêtes sortantes → seul c (déjà vu) ; depuis a : a→b→c.
        let (sg2, _) = extract_subgraph(&db, "test.a", &opts).unwrap();
        assert!(sg.nodes.len() == 1);
        assert!(sg2.nodes.len() >= 3);
    }

    #[test]
    fn limit_truncates_and_marks_dropped() {
        let (db, _, _) = make_db();
        let opts = ExtractOptions {
            hops: 2,
            limit_nodes: Some(2),
            ..Default::default()
        };
        let (sg, report) = extract_subgraph(&db, "test.a", &opts).unwrap();
        assert!(sg.truncated);
        assert!(report.truncated);
        assert_eq!(sg.nodes.len(), 2);
        assert!(report.dropped >= 1);
    }

    #[test]
    fn export_all_matches_store_counts() {
        let (db, _, _) = make_db();
        let (sg, report) = export_all(&db, &ExtractOptions::default()).unwrap();
        assert_eq!(sg.nodes.len(), 4);
        assert_eq!(sg.edges.len(), 3);
        assert_eq!(report.nodes, 4);
        assert_eq!(report.edges, 3);
        assert!(sg.validate().is_ok());
    }

    #[test]
    fn volatile_signals_read_from_persisted_properties() {
        let (db, _, _) = make_db();
        // make_db : a = id 1. Persistons des signaux volatils en propriétés.
        db.set_node_property(
            obrain_common::types::NodeId(1),
            "fsrs_stability",
            OV::from(5.0),
        );
        db.set_node_property(
            obrain_common::types::NodeId(1),
            "fsrs_difficulty",
            OV::from(3.0),
        );
        db.set_node_property(
            obrain_common::types::NodeId(1),
            "engram_id",
            OV::from("e7".to_string()),
        );
        db.checkpoint().unwrap();
        let opts = ExtractOptions {
            hops: 1,
            ..Default::default()
        };
        let (sg, _) = extract_subgraph(&db, "test.a", &opts).unwrap();
        let a = sg
            .nodes
            .iter()
            .find(|n| n.qn.as_deref() == Some("test.a"))
            .unwrap();
        assert_eq!(a.fsrs_stability, Some(5.0), "{a:#?}");
        assert_eq!(a.fsrs_difficulty, Some(3.0), "{a:#?}");
        assert_eq!(a.engram_id.as_deref(), Some("e7"), "{a:#?}");
        // L'overlay fsrs (lib) rend alors un color_hint sur ce nœud.
        let (styled, _) = obrain_viz::apply_overlay(&sg, obrain_viz::Overlay::Fsrs);
        let sa = styled
            .nodes
            .iter()
            .find(|n| n.qn.as_deref() == Some("test.a"))
            .unwrap();
        assert!(sa.color_hint.is_some(), "{sa:#?}");
    }

    #[test]
    fn closure_hops0_reaches_full_component() {
        let (db, _, _) = make_db();
        // a->b->c (CALLS) + a->d (IMPORTS) : closure depuis a atteint tout.
        let opts = ExtractOptions {
            hops: 0,
            ..Default::default()
        };
        let (sg, report) = extract_subgraph(&db, "test.a", &opts).unwrap();
        assert_eq!(report.nodes, 4, "{report:?}");
        assert_eq!(sg.edges.len(), 3);
        assert!(!report.truncated);
    }

    #[test]
    fn multi_centers_union_subgraphs() {
        let (db, _, _) = make_db();
        // test.b et test.d : union 1-hop -> b,c et d (déjà vu via b non relié).
        let centers = vec!["test.b".to_string(), "test.d".to_string()];
        let opts = ExtractOptions {
            hops: 1,
            ..Default::default()
        };
        let (sg, report) = extract_subgraph_centers(&db, &centers, &opts).unwrap();
        let has = |q: &str| sg.nodes.iter().any(|n| n.qn.as_deref() == Some(q));
        assert!(
            has("test.b") && has("test.c") && has("test.d"),
            "{report:?}"
        );
        assert_eq!(report.nodes, sg.nodes.len());
    }

    #[test]
    fn invalid_hops_rejected() {
        let (db, _, _) = make_db();
        // hops > MAX_HOPS (8) rejeté ; hops = 0 (arbre complet) est valide.
        let opts = ExtractOptions {
            hops: 9,
            ..Default::default()
        };
        assert!(extract_subgraph(&db, "test.a", &opts).is_err());
    }

    // ═══════════════════ CONTRAT NATIF-J5C ═══════════════════
    //
    // Deux cibles relevees par l'orchestrateur sur le banc :
    //   1. PERF : l'expansion multi-hop du subgraph explose (hops=2 = 27 s a
    //      limit=2000, timeout >45 s a limit=200000 sur copie runtime-prod).
    //      Cause mesuree aux compteurs de slots : l'expansion ENTRANTE
    //      (`MATCH (a)-[r]->(b) WHERE id(b) IN [...]`) BALAIE toute la zone
    //      (316 511 slots par chunk, meme pour un seul id), et le `limit`
    //      n'arrete PAS les requetes d'expansion (toute la frontiere est
    //      materialisee avant troncature). Le SORTANT, lui, est deja 0-slot.
    //   2. ROBUSTESSE : un centre non resolu par qualified_name EXACT resout
    //      silencieusement par suffixe sur un noeud arbitraire au lieu de 400.

    use obrain_substrate::compteurs::mesurer_slots;

    /// Banc hub : un centre `hub` avec `fanout` voisins SORTANTS, chacun avec
    /// `sous` voisins sortants (le 2-hop est donc large), et une `cible` qui
    /// recoit `fanout` aretes ENTRANTES depuis les voisins. Rend (db, dir).
    fn make_hub(fanout: usize, sous: usize) -> (ObrainDB, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "obrain-viz-hub-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = ObrainDB::open_substrate(dir.join("t.obrain").to_str().unwrap())
            .expect("open substrate");
        // ── L'INDEX AVANT LES NŒUDS, SINON LE BANC MESURE AUTRE CHOSE ───────
        //
        // Sans index de propriété, RÉSOUDRE LE CENTRE par `qualified_name`
        // balaie tout le label — mesuré sur cette fixture : 8 slots, soit ses
        // 8 nœuds `Function`. Le test `expansion_entrante_ne_balaie_pas_la_zone`
        // attribuait ces 8 slots à l'EXPANSION (« pas balayer la zone »), alors
        // que la sonde des formes de requête montre que les trois écritures de
        // la traversée entrante (`id(b) =`, `id(b) IN`, `(b)<-[r]-(a)`) lisent
        // **0 slot** : le coût venait de la résolution, faite DANS
        // `extract_subgraph` après la résolution du test.
        //
        // L'index est créé AVANT le premier nœud, comme dans la fixture du test
        // moteur qui passe (`obrain-engine/tests/voisinage_entrant.rs::terrain`).
        // Créé après coup, il ne couvre pas ce qui précède — vérifié : la même
        // correction posée après `hub` laissait les 8 slots.
        let hub = db.create_node(&["Function"]);
        db.set_node_property(hub, "qualified_name", OV::from("hub".to_string()));
        let cible = db.create_node(&["Function"]);
        db.set_node_property(cible, "qualified_name", OV::from("cible".to_string()));
        for i in 0..fanout {
            let v = db.create_node(&["Function"]);
            db.set_node_property(v, "qualified_name", OV::from(format!("v{i}")));
            let _ = db.create_edge(hub, v, "CALLS"); // hub -> v (sortant)
            let _ = db.create_edge(v, cible, "CALLS"); // v -> cible (entrant de cible)
            for j in 0..sous {
                let w = db.create_node(&["Function"]);
                db.set_node_property(w, "qualified_name", OV::from(format!("w{i}_{j}")));
                let _ = db.create_edge(v, w, "CALLS"); // v -> w (2-hop sortant)
            }
        }
        db.create_property_index("qualified_name");
        db.checkpoint().unwrap();
        (db, dir)
    }

    /// CIBLE 1a — l'expansion ENTRANTE doit router par la marche entrante
    /// indexee (0-slot), pas balayer la zone. On mesure les slots lus par une
    /// extraction 1-hop ENTRANTE de `cible` (fanout aretes entrantes) : la
    /// route indexee lit 0 slot ; le balayage (defaut actuel) en lit N. Le
    /// TEMOIN prouve que l'instrument compte (un scan lit > 0).
    #[test]
    fn expansion_entrante_ne_balaie_pas_la_zone() {
        let (db, _dir) = make_hub(6, 0);
        // Precondition du banc : « cible » existe et se resout. Le temoin ne
        // s'en sert plus (il balaie un label), mais la resolution reste
        // verifiee ici pour que le banc echoue BRUYAMMENT si la fixture casse.
        let _cible = resolve_node(&db, "cible").unwrap().0;
        // ── TEMOIN : IL DEVAIT CHANGER, ET VOICI POURQUOI (2026-09-15) ───────
        //
        // Il etait `MATCH (a)-[r]->(b) WHERE id(b) IN [{cible}]` : la forme que
        // l'expansion entrante utilisait AVANT de passer par l'adjacence
        // indexee. Il servait deux fois — temoin (« l'instrument compte ») et
        // rappel du defaut corrige.
        //
        // Le planificateur resout desormais cet id DIRECTEMENT : ce motif lit
        // **0 slot**. L'optimisation est un progres du moteur, mais elle a
        // retire au temoin sa raison d'etre — une lecture qui ne lit rien ne
        // prouve plus que l'instrument compte. Ce n'etait pas un effet de bord
        // de l'extraction : l'assertion porte sur une requete BRUTE, jouee
        // AVANT tout appel a `extract_subgraph`.
        //
        // Le remplacement n'est pas choisi au hasard : c'est le temoin deja
        // EPROUVE par `obrain-engine/tests/contrat_j5b_planificateur.rs`
        // (`MATCH (n:D) RETURN n.qualified_name`, avec une assertion identique
        // et le meme instrument). Une projection de propriete sur tout un
        // label ne peut etre servie par aucun index — elle balaie forcement la
        // zone noeuds, donc elle incremente `slots_noeuds_lus`.
        //
        // A NE PAS CONFONDRE avec `MATCH (n) RETURN count(n)`, qui rend 0 slot
        // lui aussi : le compte est servi par les COMPTEURS du store, sans
        // parcours. Un temoin qui ne balaie pas ne prouve rien — c'est
        // exactement le piege dans lequel le temoin precedent est tombe.
        let (_, slots_temoin) = mesurer_slots(|| {
            db.session()
                .execute("MATCH (n:Function) RETURN n.qualified_name")
                .unwrap()
        });
        assert!(
            slots_temoin > 0,
            "temoin : un balayage de la zone noeuds doit lire des slots \
             ({slots_temoin} lus) — sinon l'instrument ne compte rien, et le \
             0 de l'expansion entrante ne voudrait rien dire"
        );

        let opts = ExtractOptions {
            hops: 1,
            inbound: true,
            outbound: false,
            signals: false,
            ..Default::default()
        };
        let (sg, slots) = mesurer_slots(|| extract_subgraph(&db, "cible", &opts).unwrap());
        // cible + ses 6 voisins entrants.
        assert_eq!(sg.0.nodes.len(), 7, "voisinage entrant complet");
        assert_eq!(
            slots, 0,
            "l'expansion entrante doit passer par l'adjacence indexee (0 slot), \
             pas balayer la zone ({slots} slots lus)"
        );
    }

    /// ── LE SOUS-GRAPHE SERVI DOIT ÊTRE INDUIT : ZÉRO ARÊTE ORPHELINE ─────────
    ///
    /// Le `limit` borne les NŒUDS ; les arêtes sont collectées PENDANT
    /// l'expansion, avant que le plafond ne tranche sur le voisin. Sans le
    /// filtre d'induit, une extraction bornée sert des arêtes dont un bout
    /// n'est pas dans `nodes[]` : le rendu ne peut pas les tracer, et
    /// `adjacency(edges)` — qui alimente triangles et cliques côté client —
    /// calcule sur des nœuds fantômes.
    ///
    /// Mesuré avant correction, sur le store réel : 98,0 % d'orphelines à
    /// `limit=500` (24 003 sur 24 502). Ce test le verrouille sur une étoile
    /// synthétique, pour plusieurs plafonds.
    #[test]
    fn aucune_arete_orpheline_quand_le_limit_tronque() {
        let (db, _dir) = make_hub(30, 5);
        for cap in [2usize, 5, 20] {
            let opts = ExtractOptions {
                hops: 2,
                outbound: true,
                inbound: false,
                signals: false,
                limit_nodes: Some(cap),
                ..Default::default()
            };
            let (sg, rep) = extract_subgraph(&db, "hub", &opts).unwrap();
            let ids: std::collections::HashSet<&str> =
                sg.nodes.iter().map(|n| n.id.as_str()).collect();
            let orphelines = sg
                .edges
                .iter()
                .filter(|e| !ids.contains(e.src.as_str()) || !ids.contains(e.dst.as_str()))
                .count();
            assert_eq!(
                orphelines,
                0,
                "cap={cap} : {orphelines} arêtes orphelines sur {} — le rendu \
                 ne peut pas les tracer",
                sg.edges.len()
            );
            assert!(
                rep.truncated,
                "cap={cap} : une vue amputée (nœuds ou liens) doit se déclarer tronquée"
            );
        }
    }

    /// CIBLE 1b — le `limit` doit s'appliquer AU FIL : une extraction bornee
    /// ne materialise pas toute la frontiere. Sur un hub 30x5 (2-hop = 30 +
    /// 150 noeuds), hops=2 limit=5 rend 5 noeuds, `truncated`, et un nombre
    /// d'aretes BORNE — jamais les ~180 aretes du 2-hop complet.
    #[test]
    fn limit_arrete_l_expansion_au_fil() {
        let (db, _dir) = make_hub(30, 5);
        // Reference NON tronquante : le 2-hop complet.
        let plein = ExtractOptions {
            hops: 2,
            outbound: true,
            inbound: false,
            signals: false,
            limit_nodes: Some(100_000),
            ..Default::default()
        };
        let (sg_plein, _) = extract_subgraph(&db, "hub", &plein).unwrap();
        let n_plein = sg_plein.nodes.len();
        let e_plein = sg_plein.edges.len();
        assert!(
            n_plein > 150,
            "le 2-hop complet doit etre large : {n_plein}"
        );

        // Borne serree : arret anticipe.
        let borne = ExtractOptions {
            hops: 2,
            outbound: true,
            inbound: false,
            signals: false,
            limit_nodes: Some(5),
            ..Default::default()
        };
        let (sg_borne, rep) = extract_subgraph(&db, "hub", &borne).unwrap();
        assert_eq!(sg_borne.nodes.len(), 5, "noeuds bornes au limit");
        assert!(rep.truncated, "resultat tronque doit etre signale");
        // Arret anticipe STRICT : une fois le cap atteint (des depth 1, ou le
        // hub emet 30 aretes), AUCUNE expansion plus profonde ne doit tourner
        // — sinon la frontiere depth-2 (ici 4 voisins x 6 aretes = 24 aretes)
        // est materialisee pour rien. Le code d avant materialise 30+24 = 54 ;
        // l arret anticipe s en tient aux 30 aretes de la planche qui a rempli
        // le cap. Le seuil 40 distingue les deux.
        assert!(
            sg_borne.edges.len() <= 40,
            "arret anticipe manque : {} aretes materialisees (attendu <= 40, \
             le 2-hop complet en a {e_plein}) — la frontiere profonde est \
             expansee alors que le cap interdit tout nouveau noeud",
            sg_borne.edges.len()
        );
    }

    /// CIBLE 1c — IDENTITE a limit non tronquant : la version rapide rend
    /// EXACTEMENT le meme graphe que sans limite (memes noeuds, memes aretes).
    #[test]
    fn identite_a_limit_non_tronquant() {
        let (db, _dir) = make_hub(8, 3);
        let sans = ExtractOptions {
            hops: 2,
            outbound: true,
            inbound: true,
            signals: false,
            ..Default::default()
        };
        let avec = ExtractOptions {
            hops: 2,
            outbound: true,
            inbound: true,
            signals: false,
            limit_nodes: Some(1_000_000),
            ..Default::default()
        };
        let (a, _) = extract_subgraph(&db, "hub", &sans).unwrap();
        let (b, _) = extract_subgraph(&db, "hub", &avec).unwrap();
        let mut na: Vec<_> = a.nodes.iter().map(|n| n.id.clone()).collect();
        let mut nb: Vec<_> = b.nodes.iter().map(|n| n.id.clone()).collect();
        na.sort();
        nb.sort();
        assert_eq!(na, nb, "memes noeuds a limit non tronquant");
        assert_eq!(a.edges.len(), b.edges.len(), "memes aretes");
    }

    /// CIBLE 2 — resolution STRICTE de centre : id numerique ou qualified_name
    /// EXACT seulement. Un suffixe/nom qui resout en mode viz doit ETRE REFUSE
    /// par le resolveur strict (l'endpoint rendra 400).
    #[test]
    fn resolution_stricte_refuse_les_replis() {
        let (db, _, _) = make_db();
        // Exact et id : acceptes.
        assert_eq!(resolve_center_strict(&db, "test.b").unwrap().0, 2);
        assert_eq!(resolve_center_strict(&db, "2").unwrap().0, 2);
        // Le repli suffixe resout en mode VIZ (auto_suffix)...
        assert!(
            resolve_node_opts(&db, "b", true).is_ok(),
            "pre-condition : le repli suffixe resout bien 'b' -> test.b"
        );
        // ... mais le resolveur STRICT le refuse.
        assert!(
            resolve_center_strict(&db, "b").is_err(),
            "strict : un suffixe ne doit pas resoudre silencieusement"
        );
        // Absent : refuse.
        assert!(resolve_center_strict(&db, "nexiste.pas").is_err());
    }
}

#[cfg(test)]
mod tests_conventions_identite {
    use super::*;
    use obrain_common::types::Value as OV;

    /// **LA VISUALISATION DOIT TROUVER AUSSI CE QUE LA MÉMOIRE A ÉCRIT.**
    ///
    /// Le résolveur ne cherchait que `qualified_name` (et `name`, et son suffixe).
    /// Or la mémoire et la passe des machines nomment leurs nœuds `qn`
    /// (`memoire.rs`, `remote.rs` — `machines.rs` écrit les deux justement pour
    /// qu'aucun lecteur ne perde un nœud). Un centre écrit par `qn` seul était
    /// donc INTROUVABLE : `machine:n3`, un `signal:…`, un `compte:…` — ce qu'on
    /// veut le plus regarder. Mesuré le 2026-09-17.
    #[test]
    fn un_centre_ecrit_en_qn_seul_est_resolu() {
        let dir = std::env::temp_dir().join(format!(
            "obrain-viz-ident-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = ObrainDB::open_substrate(dir.join("t.obrain").to_str().unwrap()).unwrap();
        db.create_property_index("qualified_name");
        db.create_property_index("qn");

        let machine = db.create_node(&["Machine"]);
        db.set_node_property(machine, "qn", OV::from("machine:n3".to_string()));
        let service = db.create_node(&["Service"]);
        db.set_node_property(service, "qn", OV::from("service:n3:viz".to_string()));
        let _ = db.create_edge(machine, service, "HEBERGE");
        db.checkpoint().unwrap();

        // 1. Résolution par l'identité EXACTE (celle que porte le nœud).
        let id = resolve_node_opts(&db, "machine:n3", false).expect("résolu par `qn`");
        assert_eq!(id.0, machine.0);
        // 2. Et un nœud du graphe de code reste résolu par `qualified_name`.
        let code = db.create_node(&["Function"]);
        db.set_node_property(code, "qualified_name", OV::from("test.repo.f".to_string()));
        let id2 =
            resolve_node_opts(&db, "test.repo.f", false).expect("résolu par `qualified_name`");
        assert_eq!(id2.0, code.0);
        // 3. Le suffixe, côté `qn` : on peut nommer la fin d'une identité préfixée.
        let id3 = resolve_node_opts(&db, "n3:viz", true).expect("résolu par suffixe de `qn`");
        assert_eq!(id3.0, service.0, "le repli par suffixe doit couvrir `qn`");

        let _ = std::fs::remove_dir_all(dir);
    }
}
