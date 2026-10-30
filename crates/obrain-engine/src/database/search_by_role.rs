//! # search_by_role — « qui joue le même rôle structurel, ailleurs »
//!
//! Forme de recherche n°3 (objectif « formes de recherche pour l'IA ») : étant
//! donné un nœud, classe les autres nœuds par similarité de RÔLE STRUCTUREL —
//! indépendamment de leur contenu (labels, propriétés, valeurs).
//!
//! ## Sémantique (v1, proposition `docs/PROPOSAL-SEARCH-BY-ROLE-2026-09-05.md`)
//!
//! Descripteur de rôle du nœud `u` = **HKS 3 échelles** `[h(t_local),
//! h(t_meso), h(t_global)]` (diagonale du noyau de chaleur — `geometric.rs`),
//! invariant au contenu par construction (adjacence seule). Distance entre
//! rôles : `hks_distance` (plus petite = même rôle). `label` optionnel :
//! restreindre les candidats à un label (« trouve le twin dans l'autre
//! composant »).
//!
//! Lecture pure : CSR + diffusions HKS en mémoire, aucune mutation, aucun
//! refresh. Coût dominant = `compute_hks_descriptors` (CSR + 3 diffusions,
//! ~112 ms / 10⁶ arêtes de référence — voir la doc de `geometric.rs`).
//!
//! # Surfaces
//!
//! - [`ObrainDB::search_by_role`] — méthode moteur.
//! - `CALL obrain.search_by_role(node_id, limit, label)`
//!   YIELD node_id, distance — procédure GQL classée LECTURE.

use std::sync::Arc;

use obrain_common::types::Value;
use obrain_common::utils::error::{Error, Result};
use obrain_core::graph::GraphStore;
use obrain_substrate::SubstrateStore;

use crate::query::plan::LogicalExpression;

/// Cap de `limit`.
pub const SEARCH_BY_ROLE_MAX_LIMIT: usize = 10_000;
/// Défaut de `limit`.
pub const SEARCH_BY_ROLE_DEFAULT_LIMIT: usize = 20;

/// Une ligne de résultat `search_by_role`.
#[derive(Debug, Clone, PartialEq)]
pub struct RoleMatch {
    /// Nœud candidat (slot substrate).
    pub node_id: u64,
    /// Distance de rôle HKS (0 = rôle identique).
    pub distance: f32,
}

/// (2026-09-06) Cache in-process des descripteurs HKS par store (couche
/// « chaude » d'enrich-then-scan) : le premier `search_by_role` d'un process
/// paie le calcul global (~776 ms sur runtime-dev) ; les appels suivants sur
/// le même store (topologie inchangée — filigrane d'arêtes identique)
/// réutilisent les descripteurs → scan/rank ~ms. La persistance froide
/// (`enrich --hks`) est la suite prévue (proposition §6).
static HKS_CACHE: std::sync::Mutex<Option<(usize, u64, std::sync::Arc<Vec<[f32; 3]>>)>> =
    std::sync::Mutex::new(None);

/// Nom du sidecar de cache des descripteurs HKS (persistance froide).
const HKS_CACHE_FILENAME: &str = "substrate.hks_cache";
const HKS_CACHE_MAGIC: &[u8; 4] = b"HKS1";

/// Chemin du store (répertoire substrate).
fn store_dir(substrate: &SubstrateStore) -> std::path::PathBuf {
    let sub = substrate.writer().substrate();
    sub.lock().path().to_path_buf()
}

/// (2026-09-06) Persistance FROIDE des descripteurs (enrich-then-scan) :
/// lecture d'un sidecar `substrate.hks_cache` si l'empreinte (edge_hw)
/// correspond, sinon calcul + écriture atomique (tmp+rename, dernier
/// écrivain gagne). Un process NEUF lit 1,7 Mo en ~ms au lieu de recalculer
/// (~845 ms sur runtime-dev).
fn hks_descriptors_cached(substrate: &SubstrateStore) -> Result<std::sync::Arc<Vec<[f32; 3]>>> {
    let ptr = (substrate as *const SubstrateStore) as usize;
    let hw = substrate.edge_slot_high_water();
    {
        let g = HKS_CACHE.lock().unwrap();
        if let Some((p, h, cached)) = &*g {
            if *p == ptr && *h == hw {
                return Ok(std::sync::Arc::clone(cached));
            }
        }
    }
    let dir = store_dir(substrate);
    let path = dir.join(HKS_CACHE_FILENAME);
    // Lecture du sidecar (empreinte identique → réutilisable).
    if let Ok(bytes) = std::fs::read(&path)
        && bytes.len() >= 20
        && &bytes[..4] == HKS_CACHE_MAGIC
        && u64::from_le_bytes(bytes[4..12].try_into().unwrap()) == hw
    {
        let n = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        if bytes.len() >= 16 + n * 12 {
            let mut desc = Vec::with_capacity(n);
            for i in 0..n {
                let o = 16 + i * 12;
                desc.push([
                    f32::from_le_bytes(bytes[o..o + 4].try_into().unwrap()),
                    f32::from_le_bytes(bytes[o + 4..o + 8].try_into().unwrap()),
                    f32::from_le_bytes(bytes[o + 8..o + 12].try_into().unwrap()),
                ]);
            }
            let arc = std::sync::Arc::new(desc);
            *HKS_CACHE.lock().unwrap() = Some((ptr, hw, std::sync::Arc::clone(&arc)));
            return Ok(arc);
        }
    }
    // Calcul + persistance atomique.
    let desc = obrain_substrate::geometric::compute_hks_descriptors(substrate)
        .map_err(|e| Error::Internal(format!("search_by_role: hks: {e}")))?;
    let arc = std::sync::Arc::new(desc);
    {
        let tmp = path.with_extension("hks_cache.tmp");
        let mut buf = Vec::with_capacity(16 + arc.len() * 12);
        buf.extend_from_slice(HKS_CACHE_MAGIC);
        buf.extend_from_slice(&hw.to_le_bytes());
        buf.extend_from_slice(&(arc.len() as u32).to_le_bytes());
        for d in arc.iter() {
            for v in d {
                buf.extend_from_slice(&v.to_le_bytes());
            }
        }
        let _ = std::fs::write(&tmp, &buf).and_then(|_| std::fs::rename(&tmp, &path));
    }
    let mut g = HKS_CACHE.lock().unwrap();
    if let Some((p, h, cached)) = &*g {
        if *p == ptr && *h == hw {
            return Ok(std::sync::Arc::clone(cached));
        }
    }
    *g = Some((ptr, hw, std::sync::Arc::clone(&arc)));
    Ok(arc)
}


/// Calcule le classement des nœuds de rôle structurel le plus proche de
/// `anchor` (HKS 3 échelles). `label = Some(l)` restreint les candidats aux
/// nœuds portant le label `l`. L'ancre elle-même est exclue. O(CSR+HKS) +
/// O(k log k).
pub fn compute_search_by_role(
    substrate: &SubstrateStore,
    anchor: obrain_common::types::NodeId,
    limit: usize,
    label: Option<&str>,
) -> Result<Vec<RoleMatch>> {
    let descriptors = hks_descriptors_cached(substrate)?;
    let hw = descriptors.len();
    if anchor.0 == 0 || (anchor.0 as usize) >= hw {
        return Err(Error::Internal(format!(
            "search_by_role: nœud {} hors registre (0 < slot < {hw})",
            anchor.0
        )));
    }
    let anchor_desc = descriptors[anchor.0 as usize];
    // Descripteur dégénéré (NaN/±inf — diffusion instable ou nœud hors
    // graphe) : l'ancre n'est pas comparable → erreur explicite.
    if !anchor_desc.iter().all(|v| v.is_finite()) {
        return Err(Error::Internal(format!(
            "search_by_role: l'ancre {} n'a pas de descripteur HKS fini (nœud isolé ou diffusion instable)",
            anchor.0
        )));
    }

    // Candidats : tous les slots vivants 1..hw (descripteurs FINIS
    // uniquement — les nœuds dégénérés ne sont pas classables), ou le label.
    let mut candidates: Vec<u64> = Vec::new();
    if let Some(lbl) = label {
        candidates = substrate
            .nodes_by_label(lbl)
            .into_iter()
            .map(|n| n.0)
            .filter(|&n| n != anchor.0 && (n as usize) < hw)
            .collect();
    } else {
        candidates.reserve(hw.saturating_sub(1));
        for slot in 1..hw {
            if slot as u64 != anchor.0
                && descriptors[slot].iter().all(|v| v.is_finite())
            {
                candidates.push(slot as u64);
            }
        }
    }

    // Distance brute HKS (le bruit de l'estimateur Hutchinson ~0,1-0,2 ne
    // sépare pas seul deux formes voisines : cf. proposition §5 R1).
    let mut raw: Vec<(u64, f32)> = candidates
        .into_iter()
        .map(|n| {
            (
                n,
                obrain_substrate::geometric::hks_distance(
                    &anchor_desc,
                    &descriptors[n as usize],
                ),
            )
        })
        .collect();
    // Comparateur TOTAL (total_cmp) : les descripteurs HKS peuvent contenir
    // des NaN (nœuds isolés) — partial_cmp rendrait un ordre non total et le
    // tri paniquerait (« user-provided comparison function »).
    raw.sort_by(|a, b| {
        a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0))
    });

    // PRÉ-RANK BORNÉ puis RE-RANK augmenté : la distance de rôle finale =
    // hks + λ·|clustering(ancre) − clustering(candidat)| — la clustering
    // sépare nettement hub de clique (≈1) de hub d'étoile (0) là où la seule
    // HKS reste dans le bruit. Clustering calculée sur le pré-rank (2×limit)
    // seulement → coût borné (voisinages locaux, degré plafonné).
    const CLUSTER_LAMBDA: f32 = 0.25;
    const CLUSTER_MAX_DEGREE: usize = 2048;
    let cluster_of = |sub: &SubstrateStore, n: u64| -> f32 {
        let mut nbrs: Vec<u64> = Vec::new();
        {
            let mut vu = std::collections::HashSet::new();
            for (t, _) in sub.edges_from(obrain_common::types::NodeId(n), obrain_core::graph::Direction::Both) {
                if t.0 != n && vu.insert(t.0) && vu.len() > CLUSTER_MAX_DEGREE {
                    break;
                }
            }
            nbrs = vu.into_iter().collect();
        }
        let d = nbrs.len();
        if d < 2 {
            return 0.0;
        }
        // Degré trop grand → cluster inconnu (neutre) plutôt qu'un O(Σd) fou.
        if d > CLUSTER_MAX_DEGREE {
            return 0.5;
        }
        let set: std::collections::HashSet<u64> = nbrs.iter().copied().collect();
        let mut triangles: u64 = 0;
        for a in &nbrs {
            for (t, _) in sub.edges_from(obrain_common::types::NodeId(*a), obrain_core::graph::Direction::Both) {
                if t.0 != n && set.contains(&t.0) {
                    triangles += 1;
                }
            }
        }
        let pairs = d as f64 * (d as f64 - 1.0);
        (triangles as f64 / pairs) as f32 // triangles comptés 2× (a→b et b→a)
    };
    let anchor_cluster = cluster_of(substrate, anchor.0);

    let keep = raw.len().min((limit.max(1) * 2).min(SEARCH_BY_ROLE_MAX_LIMIT));
    let mut out: Vec<RoleMatch> = raw
        .into_iter()
        .take(keep)
        .map(|(n, hks)| RoleMatch {
            node_id: n,
            distance: hks + CLUSTER_LAMBDA * (anchor_cluster - cluster_of(substrate, n)).abs(),
        })
        .collect();
    out.sort_by(|a, b| {
        a.distance.total_cmp(&b.distance).then_with(|| a.node_id.cmp(&b.node_id))
    });
    out.truncate(limit.min(SEARCH_BY_ROLE_MAX_LIMIT));
    Ok(out)
}

impl super::ObrainDB {
    /// (2026-09-05) Nœuds au rôle structurel le plus proche de `node_id`
    /// (HKS 3 échelles, invariant au contenu). `label` restreint les
    /// candidats. Nécessite un store substrate.
    pub fn search_by_role(
        &self,
        node_id: u64,
        limit: usize,
        label: Option<&str>,
    ) -> Result<Vec<RoleMatch>> {
        let substrate = self.substrate_store.as_ref().ok_or_else(|| {
            Error::Internal("search_by_role requires a substrate-backed database".to_string())
        })?;
        compute_search_by_role(
            substrate,
            obrain_common::types::NodeId(node_id),
            limit,
            label,
        )
    }
}

/// Exécute `CALL obrain.search_by_role(...)` si le nom matche.
///
/// `Ok(None)` si le nom n'est pas une procédure search_by_role. Erreur
/// explicite si la base n'est pas substrate ou le nœud hors registre.
pub fn try_execute_search_by_role_procedure(
    resolved_name: &str,
    arguments: &[LogicalExpression],
    substrate: Option<&Arc<SubstrateStore>>,
) -> Result<Option<obrain_adapters::plugins::AlgorithmResult>> {
    if resolved_name != "obrain.search_by_role" && resolved_name != "search_by_role" {
        return Ok(None);
    }
    let result = execute_search_by_role(arguments, substrate)?;
    Ok(Some(result))
}

fn arg_u64(arguments: &[LogicalExpression], i: usize, name: &str) -> Result<u64> {
    match arguments.get(i) {
        Some(LogicalExpression::Literal(Value::Int64(v))) if *v >= 0 => Ok(*v as u64),
        _ => Err(Error::Internal(format!(
            "obrain.search_by_role(): argument {i} ({name}) must be a non-negative integer"
        ))),
    }
}

fn arg_usize(arguments: &[LogicalExpression], i: usize, name: &str, default: usize) -> Result<usize> {
    match arguments.get(i) {
        None => Ok(default),
        Some(LogicalExpression::Literal(Value::Int64(v))) if *v >= 0 => Ok(*v as usize),
        _ => Err(Error::Internal(format!(
            "obrain.search_by_role(): argument {i} ({name}) must be a non-negative integer"
        ))),
    }
}

fn arg_string(
    arguments: &[LogicalExpression],
    i: usize,
    name: &str,
) -> Result<Option<String>> {
    match arguments.get(i) {
        None => Ok(None),
        Some(LogicalExpression::Literal(Value::String(s))) => Ok(Some(s.to_string())),
        Some(LogicalExpression::Literal(Value::Null)) | Some(LogicalExpression::Literal(Value::Bool(false))) => Ok(None),
        _ => Err(Error::Internal(format!(
            "obrain.search_by_role(): argument {i} ({name}) must be a string"
        ))),
    }
}

fn execute_search_by_role(
    arguments: &[LogicalExpression],
    substrate: Option<&Arc<SubstrateStore>>,
) -> Result<obrain_adapters::plugins::AlgorithmResult> {
    let node_id = arg_u64(arguments, 0, "node_id")?;
    let limit = arg_usize(arguments, 1, "limit", SEARCH_BY_ROLE_DEFAULT_LIMIT)?;
    let label = arg_string(arguments, 2, "label")?;

    let Some(sub) = substrate else {
        return Err(Error::Internal(
            "obrain.search_by_role requires a substrate-backed database".to_string(),
        ));
    };

    let matches = compute_search_by_role(
        sub,
        obrain_common::types::NodeId(node_id),
        limit,
        label.as_deref(),
    )?;

    let mut result = obrain_adapters::plugins::AlgorithmResult::new(vec![
        "node_id".to_string(),
        "distance".to_string(),
    ]);
    for m in matches {
        result.add_row(vec![
            Value::Int64(m.node_id as i64),
            Value::Float64(f64::from(m.distance)),
        ]);
    }
    Ok(result)
}
