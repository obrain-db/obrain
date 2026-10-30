//! `obrain compare <A> <B>` — diff structurel entre DEUX variantes d'un même
//! dépôt indexé (branches / worktrees / écarts), lecture seule sur les deux
//! stores. Une seule comparaison binaire n'est pas pertinente pour le code
//! réel → boucler `compare` par paires (main vs chaque variante) donne la vue
//! N-variantes (docs/PROPOSAL-COMPARATIF-MULTI-VARIANTES-2026-09-05.md).
//!
//! Identité des nœuds : (labels triés, propriété `qualified_name`) — les
//! index du même dépôt gardent des qn stables entre états. Attributs
//! comparés : `file_path`, `start_line`, `end_line`, `name`. Nœuds sans
//! `qualified_name` = comptés « sans identité » (non comparables).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use obrain_common::types::{NodeId, PropertyKey, Value};
use obrain_core::graph::GraphStore;
use obrain_substrate::SubstrateStore;

use crate::OutputFormat;
use crate::output;

const PROPS_COMPAREES: [&str; 4] = ["file_path", "start_line", "end_line", "name"];

/// Résultat agrégé du comparatif.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Comparaison {
    pub tot_a: usize,
    pub tot_b: usize,
    pub sans_a: usize,
    pub sans_b: usize,
    pub ajoutes: usize,
    pub retires: usize,
    pub modifies: usize,
    pub aretes_a: usize,
    pub aretes_b: usize,
    pub aretes_ajoutees: usize,
    pub aretes_retirees: usize,
    pub jaccard_noeuds: f64,
    pub jaccard_aretes: f64,
    /// type → (A, B)
    pub par_type: BTreeMap<String, (usize, usize)>,
    /// échantillons d'arêtes « +/‑ » (limité).
    pub echantillon: Vec<String>,
    pub elapsed_ms: u64,
}

/// Identité = (labels triés, qn normalisé). La normalisation retire le
/// préfixe `<projet>.` propre à chaque index CBM — deux worktrees/branches du
/// même dépôt indexés sous des noms de projet différents (ex. `obrain-engine.`
/// vs `tmp-grafeo-cbm.`) partagent alors leurs qn réels (chemins relatifs).
struct Snapshot {
    ident: Vec<Option<String>>,
    props: HashMap<String, Vec<Option<Value>>>,
    ident_set: BTreeSet<String>,
    total_live: usize,
    sans_identite: usize,
}

fn snapshot(store: &SubstrateStore, prefix: Option<&str>) -> Result<Snapshot> {
    let node_ids: Vec<NodeId> = GraphStore::node_ids(store);
    // Une seule requête par lot pour les propriétés utiles (qn + attributs) :
    // évite N×5 lectures dispersées.
    let keys: Vec<PropertyKey> = std::iter::once(PropertyKey::new("qualified_name"))
        .chain(PROPS_COMPAREES.iter().map(|k| PropertyKey::new(*k)))
        .collect();
    // Batch PAR CLÉ (sans allouer une map par nœud) : 5 passes vectorielles.
    let cols: Vec<Vec<Option<Value>>> = keys
        .iter()
        .map(|k| GraphStore::get_node_property_batch(store, &node_ids, k))
        .collect();

    let mut hw = 0usize;
    for n in &node_ids {
        hw = hw.max(n.0 as usize + 1);
    }
    let mut ident: Vec<Option<String>> = vec![None; hw];
    let mut props: HashMap<String, Vec<Option<Value>>> = HashMap::new();
    let mut ident_set: BTreeSet<String> = BTreeSet::new();
    let mut sans = 0usize;
    for (i, n) in node_ids.iter().enumerate() {
        let Some(qn) = &cols[0][i] else {
            sans += 1;
            continue;
        };
        let Value::String(qn) = qn else {
            sans += 1;
            continue;
        };
        let mut q = qn.to_string();
        if let Some(p) = prefix {
            let pfx = format!("{p}.");
            if q.starts_with(&pfx) {
                q = q[pfx.len()..].to_string();
            }
        }
        let labels = match store.get_node_labels(*n) {
            Some(l) => l,
            None => {
                sans += 1;
                continue;
            }
        };
        let mut ls: Vec<String> = labels.iter().map(|l| l.to_string()).collect();
        ls.sort_unstable();
        let id = format!("{}|{}", ls.join("+"), q);
        let vals: Vec<Option<Value>> = cols[1..].iter().map(|c| c[i].clone()).collect();
        props.insert(id.clone(), vals);
        ident_set.insert(id.clone());
        ident[n.0 as usize] = Some(id);
    }
    let total_live = ident_set.len() + sans;
    Ok(Snapshot {
        ident,
        props,
        ident_set,
        total_live,
        sans_identite: sans,
    })
}

fn jaccard(s1: &BTreeSet<String>, s2: &BTreeSet<String>) -> f64 {
    let inter = s1.intersection(s2).count();
    let union = s1.union(s2).count();
    if union == 0 {
        1.0
    } else {
        inter as f64 / union as f64
    }
}

/// FNV-1a 64 déterministe (identique entre stores pour le même qn).
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Mélange trois mots (src, type, dst) en une clé 64 bits.
fn mix3(a: u64, t: u64, b: u64) -> u64 {
    let mut x = a ^ (t.wrapping_mul(0x9E37_79B9_7F4A_7C15)) ^ b.rotate_left(32);
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Cache d'empreintes de nœuds par store (principe naturel « ne recalculer
/// que ce qui change » — mémoire/rétine) : une passe de lecture des props
/// `qualified_name` coûte ~70 % d'un compare ; le résumé (hash FNV par slot)
/// est persisté dans un fichier latéral `substrate.compare-hash[.<prefix>]`
/// et invalidé par mtime (toute réécriture du store invalide).
const CACHE_MAGIC: [u8; 4] = *b"CHSH";

fn cache_file(store_dir: &Path, prefix: Option<&str>) -> PathBuf {
    let sfx = match prefix {
        Some(p) => format!(".{}", fnv1a(p.as_bytes())),
        None => String::new(),
    };
    store_dir.join(format!("substrate.compare-hash{sfx}"))
}

/// Le cache est-il encore valide ? (aucun fichier du store plus récent que
/// le cache, hormis le cache lui-même)
fn cache_valide(dir: &Path, cache: &Path) -> bool {
    let cache_mt = match fs::metadata(cache).and_then(|m| m.modified()) {
        Ok(t) => t,
        Err(_) => return false,
    };
    let Ok(rd) = fs::read_dir(dir) else {
        return false;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p == cache {
            continue;
        }
        // Les fichiers de cache latéraux ne sont pas des données du store.
        if let Some(n) = p.file_name().and_then(|n| n.to_str()) {
            if n.starts_with("substrate.compare-hash")
                || n.starts_with("substrate.compare-result")
                || n.starts_with("substrate.compare-edges")
            {
                continue;
            }
        }
        if let Ok(mt) = e.metadata().and_then(|m| m.modified()) {
            // >= et non > : sur un FS à granularité temporelle grossière, une
            // écriture dans le MÊME tick que le cache serait sinon invisible
            // (changement manqué = bug silencieux). Correct avant rapide.
            if mt >= cache_mt {
                return false;
            }
        }
    }
    true
}

struct Empreintes {
    ident: Vec<u64>,
    membres: std::collections::HashSet<u64>,
    sans: usize,
    live: usize,
}

/// Charge ou construit le résumé de nœuds d'un store.
fn empreintes(store: &SubstrateStore, dir: &Path, prefix: Option<&str>) -> Result<Empreintes> {
    let cache = cache_file(dir, prefix);
    if cache_valide(dir, &cache) {
        if let Ok(bytes) = fs::read(&cache) {
            // En-tête : magic 4 + version u64 (4..12) + hw u64 (12..20) +
            // sans u64 (20..28) + live u64 (28..36) ; hashes à partir de 36.
            if bytes.len() >= 40 && bytes[..4] == CACHE_MAGIC {
                let n = u64::from_le_bytes(bytes[12..20].try_into().unwrap()) as usize;
                let sans = u64::from_le_bytes(bytes[20..28].try_into().unwrap()) as usize;
                let live = u64::from_le_bytes(bytes[28..36].try_into().unwrap()) as usize;
                if bytes.len() >= 36 + n * 8 {
                    let mut ident: Vec<u64> = Vec::with_capacity(n);
                    let mut membres: std::collections::HashSet<u64> =
                        std::collections::HashSet::with_capacity(live);
                    for i in 0..n {
                        let off = 36 + i * 8;
                        let h = u64::from_le_bytes(bytes[off..off + 8].try_into().unwrap());
                        ident.push(h);
                        if h != 0 {
                            membres.insert(h);
                        }
                    }
                    if std::env::var("OBRAIN_COMPARE_PROFILE").is_ok() {
                        eprintln!("compare.profile: empreintes CACHE hit ({})", dir.display());
                    }
                    return Ok(Empreintes {
                        ident,
                        membres,
                        sans,
                        live,
                    });
                }
            }
        }
    }
    if std::env::var("OBRAIN_COMPARE_PROFILE").is_ok() {
        eprintln!("compare.profile: empreintes CALCUL ({})", dir.display());
    }
    // Construction (chemin lent) : ids + batch qn + FNV.
    let ids: Vec<NodeId> = GraphStore::node_ids(store);
    let key = PropertyKey::new("qualified_name");
    let qns = GraphStore::get_node_property_batch(store, &ids, &key);
    let mut hw = 0usize;
    for n in &ids {
        hw = hw.max(n.0 as usize + 1);
    }
    let mut ident: Vec<u64> = vec![0; hw];
    let mut membres: std::collections::HashSet<u64> = std::collections::HashSet::new();
    let mut sans = 0usize;
    for (i, n) in ids.iter().enumerate() {
        let Some(Value::String(qn)) = &qns[i] else {
            sans += 1;
            continue;
        };
        let mut q = qn.to_string();
        if let Some(p) = prefix {
            let pfx = format!("{p}.");
            if q.starts_with(&pfx) {
                q = q[pfx.len()..].to_string();
            }
        }
        let h = fnv1a(q.as_bytes());
        ident[n.0 as usize] = h;
        membres.insert(h);
    }
    let live = membres.len() + sans;
    let mut out = Vec::with_capacity(32 + hw * 8);
    out.extend_from_slice(&CACHE_MAGIC);
    out.extend_from_slice(&1u64.to_le_bytes()); // version
    out.extend_from_slice(&(hw as u64).to_le_bytes());
    out.extend_from_slice(&(sans as u64).to_le_bytes());
    out.extend_from_slice(&(live as u64).to_le_bytes());
    for h in &ident {
        out.extend_from_slice(&h.to_le_bytes());
    }
    let _ = fs::write(&cache, out);
    Ok(Empreintes {
        ident,
        membres,
        sans,
        live,
    })
}

fn fp_path(dir: &Path, prefix: Option<&str>) -> PathBuf {
    cache_file(dir, prefix)
}

fn memo_path(
    dir_a: &Path,
    prefix_a: Option<&str>,
    dir_b: &Path,
    prefix_b: Option<&str>,
) -> PathBuf {
    let mut key = String::new();
    key.push_str(&dir_a.to_string_lossy());
    key.push('\0');
    key.push_str(prefix_a.unwrap_or(""));
    key.push('\0');
    key.push_str(&dir_b.to_string_lossy());
    key.push('\0');
    key.push_str(prefix_b.unwrap_or(""));
    dir_a.join(format!(
        "substrate.compare-result.{}",
        fnv1a(key.as_bytes())
    ))
}

fn fp_mtime(fp: &Path) -> Option<std::time::SystemTime> {
    fs::metadata(fp).and_then(|m| m.modified()).ok()
}

/// Tente de rejouer un résultat mémorisé (stores intacts + empreintes
/// inchangées) — coût répété ~µs/ms (principe « rétine »).
fn essayer_memo(
    dir_a: &Path,
    prefix_a: Option<&str>,
    dir_b: &Path,
    prefix_b: Option<&str>,
) -> Option<Comparaison> {
    let fa = fp_path(dir_a, prefix_a);
    let fb = fp_path(dir_b, prefix_b);
    if !fa.exists() || !fb.exists() {
        return None;
    }
    if !cache_valide(dir_a, &fa) || !cache_valide(dir_b, &fb) {
        return None;
    }
    let ma = fp_mtime(&fa)?;
    let mb = fp_mtime(&fb)?;
    let mp = memo_path(dir_a, prefix_a, dir_b, prefix_b);
    let ok = serde_json::from_slice::<serde_json::Value>(&fs::read(mp).ok()?).ok()?;
    let ta = ok.get("ta")?.as_u64()?;
    let tb = ok.get("tb")?.as_u64()?;
    let ma_u = ma.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as u64;
    let mb_u = mb.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as u64;
    if ta != ma_u || tb != mb_u {
        return None;
    }
    let mut c: Comparaison = serde_json::from_value(ok.get("r")?.clone()).ok()?;
    c.elapsed_ms = 0; // rejoué, pas recalculé
    Some(c)
}

fn sauver_memo(
    dir_a: &Path,
    prefix_a: Option<&str>,
    dir_b: &Path,
    prefix_b: Option<&str>,
    c: &Comparaison,
) {
    let ma = fp_mtime(&fp_path(dir_a, prefix_a));
    let mb = fp_mtime(&fp_path(dir_b, prefix_b));
    let (Some(ma), Some(mb)) = (ma, mb) else {
        return;
    };
    let v = serde_json::json!({
        "ta": ma.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64,
        "tb": mb.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64,
        "r": c,
    });
    let _ = fs::write(
        memo_path(dir_a, prefix_a, dir_b, prefix_b),
        serde_json::to_vec(&v).unwrap_or_default(),
    );
}

// ---------------------------------------------------------------------------
// Résumé d'arêtes par store (chantier 3) : `substrate.compare-edges[.<prefix>]`
// = triplets (src_h, type_nom_h, dst_h) dans l'ordre des slots + comptes par
// nom de type. Valide sous la même règle mtime (>=) que les empreintes de
// nœuds → quand un SEUL store a changé, l'autre côté est reclassé SANS
// rescan de zone (lecture fichier + filtres par membres).
// ---------------------------------------------------------------------------
const EDGE_MAGIC: [u8; 4] = *b"CEDS";

fn edge_fp_path(dir: &Path, prefix: Option<&str>) -> PathBuf {
    let sfx = match prefix {
        Some(p) => format!(".{}", fnv1a(p.as_bytes())),
        None => String::new(),
    };
    dir.join(format!("substrate.compare-edges{sfx}"))
}

struct EdgesVue {
    communs: Vec<u64>,
    propres: Vec<u64>,
    par_nom: BTreeMap<String, usize>,
    total: usize,
}

fn edge_fp_encode(triplets: &[(u64, u64, u64)], par_nom: &BTreeMap<String, usize>) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(&EDGE_MAGIC);
    out.extend_from_slice(&1u64.to_le_bytes()); // version
    out.extend_from_slice(&(triplets.len() as u64).to_le_bytes());
    for (sh, th, dh) in triplets {
        out.extend_from_slice(&sh.to_le_bytes());
        out.extend_from_slice(&th.to_le_bytes());
        out.extend_from_slice(&dh.to_le_bytes());
    }
    out.extend_from_slice(&(par_nom.len() as u64).to_le_bytes());
    for (name, c) in par_nom {
        out.extend_from_slice(&(name.len() as u64).to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&(*c as u64).to_le_bytes());
    }
    out
}

fn edge_fp_decode(bytes: &[u8]) -> Option<(Vec<(u64, u64, u64)>, BTreeMap<String, usize>)> {
    if bytes.len() < 20 || bytes[..4] != EDGE_MAGIC {
        return None;
    }
    let n = u64::from_le_bytes(bytes[12..20].try_into().ok()?) as usize;
    let mut pos = 20usize;
    let mut triplets = Vec::with_capacity(n);
    for _ in 0..n {
        if pos + 24 > bytes.len() {
            return None;
        }
        let sh = u64::from_le_bytes(bytes[pos..pos + 8].try_into().ok()?);
        let th = u64::from_le_bytes(bytes[pos + 8..pos + 16].try_into().ok()?);
        let dh = u64::from_le_bytes(bytes[pos + 16..pos + 24].try_into().ok()?);
        triplets.push((sh, th, dh));
        pos += 24;
    }
    if pos + 8 > bytes.len() {
        return None;
    }
    let m = u64::from_le_bytes(bytes[pos..pos + 8].try_into().ok()?) as usize;
    pos += 8;
    let mut par_nom: BTreeMap<String, usize> = BTreeMap::new();
    for _ in 0..m {
        if pos + 8 > bytes.len() {
            return None;
        }
        let len = u64::from_le_bytes(bytes[pos..pos + 8].try_into().ok()?) as usize;
        pos += 8;
        if pos + len + 8 > bytes.len() {
            return None;
        }
        let name = std::str::from_utf8(&bytes[pos..pos + len])
            .ok()?
            .to_string();
        pos += len;
        let c = u64::from_le_bytes(bytes[pos..pos + 8].try_into().ok()?) as usize;
        pos += 8;
        par_nom.insert(name, c);
    }
    if pos != bytes.len() {
        return None;
    }
    Some((triplets, par_nom))
}

/// Arêtes d'un store : depuis le cache (store intact + empreinte de nœuds
/// présente) sinon scan de zone (qui re-écrit le cache). Retourne aussi si
/// le cache a servi.
fn edges_vue(
    store: &SubstrateStore,
    dir: &Path,
    prefix: Option<&str>,
    ident: &[u64],
    autres: &std::collections::HashSet<u64>,
) -> Result<(EdgesVue, bool)> {
    let path = edge_fp_path(dir, prefix);
    if fp_path(dir, prefix).exists() && cache_valide(dir, &path) {
        if let Ok(bytes) = fs::read(&path) {
            if let Some((triplets, par_nom)) = edge_fp_decode(&bytes) {
                let mut communs: Vec<u64> = Vec::new();
                let mut propres: Vec<u64> = Vec::new();
                for (sh, th, dh) in triplets {
                    let key = mix3(sh, th, dh);
                    if autres.contains(&sh) && autres.contains(&dh) {
                        communs.push(key);
                    } else {
                        propres.push(key);
                    }
                }
                communs.sort_unstable();
                propres.sort_unstable();
                communs.dedup();
                propres.dedup();
                let total = par_nom.values().sum::<usize>();
                return Ok((
                    EdgesVue {
                        communs,
                        propres,
                        par_nom,
                        total,
                    },
                    true,
                ));
            }
        }
    }
    // Scan de zone (verrou unique) + écriture du résumé.
    let mut communs: Vec<u64> = Vec::new();
    let mut propres: Vec<u64> = Vec::new();
    let mut par_nom: BTreeMap<String, usize> = BTreeMap::new();
    let mut triplets: Vec<(u64, u64, u64)> = Vec::new();
    // Clé = fnv du NOM de type (ids locaux ≠ entre stores) ; on garde aussi
    // le nom pour les comptes affichés par type.
    let mut type_info: std::collections::HashMap<u16, (u64, String)> =
        std::collections::HashMap::new();
    store
        .for_each_live_edge(|slot, rec| {
            let (th, name) = match type_info.get(&rec.edge_type) {
                Some((h, n)) => (*h, n.clone()),
                None => {
                    let name = GraphStore::edge_type(store, obrain_common::types::EdgeId(slot))
                        .map(|x| x.to_string())
                        .unwrap_or_else(|| format!("T{}", rec.edge_type));
                    let h = fnv1a(name.as_bytes());
                    type_info.insert(rec.edge_type, (h, name.clone()));
                    (h, name)
                }
            };
            let sh = ident.get(rec.src as usize).copied().unwrap_or(0);
            let dh = ident.get(rec.dst as usize).copied().unwrap_or(0);
            triplets.push((sh, th, dh));
            let key = mix3(sh, th, dh);
            if autres.contains(&sh) && autres.contains(&dh) {
                communs.push(key);
            } else {
                propres.push(key);
            }
            *par_nom.entry(name).or_insert(0) += 1;
        })
        .map_err(|e| anyhow::anyhow!("compare: scan edges: {e}"))?;
    communs.sort_unstable();
    propres.sort_unstable();
    communs.dedup();
    propres.dedup();
    let _ = fs::write(&path, edge_fp_encode(&triplets, &par_nom));
    Ok((
        EdgesVue {
            communs,
            propres,
            par_nom,
            total: triplets.len(),
        },
        false,
    ))
}

/// Comparatif STRUCTUREL rapide : identité par `qualified_name` (hash FNV),
/// arêtes comme clés u64 triées (diff two-pointer), comptes par type.
/// Pas de lecture d'attributs ni de libellés par nœud (mode `--attrs` pour
/// la profondeur) — typiquement ms (µs par opération).
pub fn comparer_structurel(
    a: &SubstrateStore,
    dir_a: &Path,
    b: &SubstrateStore,
    dir_b: &Path,
    prefix_a: Option<&str>,
    prefix_b: Option<&str>,
) -> Result<Comparaison> {
    let t0 = std::time::Instant::now();

    // ── Nœuds : empreintes par store (cache latéral à mtime, sinon calcul) ──
    let (ea, eb) = std::thread::scope(|sc| -> Result<(Empreintes, Empreintes)> {
        let ta = sc.spawn(|| empreintes(a, dir_a, prefix_a));
        let tb = sc.spawn(|| empreintes(b, dir_b, prefix_b));
        Ok((ta.join().unwrap()?, tb.join().unwrap()?))
    })?;
    let (ident_a, membres_a, sans_a) = (ea.ident, ea.membres, ea.sans);
    let (ident_b, membres_b, sans_b) = (eb.ident, eb.membres, eb.sans);
    let tot_a = ea.live;
    let tot_b = eb.live;
    let ajoutes = membres_b.difference(&membres_a).count();
    let retires = membres_a.difference(&membres_b).count();

    type ResE = (EdgesVue, bool);
    let (ra, rb) = std::thread::scope(|sc| -> Result<(ResE, ResE)> {
        let ta = sc.spawn(|| edges_vue(a, dir_a, prefix_a, &ident_a, &membres_b));
        let tb = sc.spawn(|| edges_vue(b, dir_b, prefix_b, &ident_b, &membres_a));
        Ok((ta.join().unwrap()?, tb.join().unwrap()?))
    })?;
    let (va, _ca) = ra;
    let (vb, _cb) = rb;
    let (com_a, own_a_n, tot_ea) = (va.communs, va.propres.len(), va.total);
    let (com_b, own_b_n, tot_eb) = (vb.communs, vb.propres.len(), vb.total);
    let (par_nom_a, par_nom_b) = (va.par_nom, vb.par_nom);
    // Diff two-pointer sur les clés communes.
    let mut i = 0usize;
    let mut j = 0usize;
    let mut aj_commun = 0usize;
    let mut ret_commun = 0usize;
    while i < com_a.len() && j < com_b.len() {
        match com_a[i].cmp(&com_b[j]) {
            std::cmp::Ordering::Less => {
                ret_commun += 1;
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                aj_commun += 1;
                j += 1;
            }
            std::cmp::Ordering::Equal => {
                i += 1;
                j += 1;
            }
        }
    }
    ret_commun += com_a.len() - i;
    aj_commun += com_b.len() - j;
    let aretes_ajoutees = aj_commun + own_b_n;
    let aretes_retirees = ret_commun + own_a_n;

    // Jaccards.
    let jn = {
        let inter = membres_a.intersection(&membres_b).count();
        let union = membres_a.union(&membres_b).count();
        if union == 0 {
            1.0
        } else {
            inter as f64 / union as f64
        }
    };
    // Jaccard arêtes sur les ENSEMBLES UNIQUES (dédup).
    let comm = com_a.len() as u64 - ret_commun as u64;
    let ua = com_a.len() as u64 + own_a_n as u64;
    let ub = com_b.len() as u64 + own_b_n as u64;
    let union = ua + ub - comm;
    let je = if union == 0 {
        1.0
    } else {
        comm as f64 / union as f64
    };
    // Par-type (comptes BRUTS) par NOM — plus de résolution par scan.
    let mut par_type: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for name in par_nom_a.keys().chain(par_nom_b.keys()) {
        par_type.insert(
            name.clone(),
            (
                *par_nom_a.get(name).unwrap_or(&0),
                *par_nom_b.get(name).unwrap_or(&0),
            ),
        );
    }
    Ok(Comparaison {
        tot_a,
        tot_b,
        sans_a,
        sans_b,
        ajoutes,
        retires,
        modifies: 0,
        aretes_a: tot_ea,
        aretes_b: tot_eb,
        aretes_ajoutees,
        aretes_retirees,
        jaccard_noeuds: jn,
        jaccard_aretes: je,
        par_type,
        echantillon: Vec::new(),
        elapsed_ms: t0.elapsed().as_millis() as u64,
    })
}

/// Diff complet entre deux stores (RO). O(N+E) lectures, maps en mémoire.
pub fn comparer(
    a: &SubstrateStore,
    b: &SubstrateStore,
    limit: usize,
    prefix_a: Option<&str>,
    prefix_b: Option<&str>,
) -> Result<Comparaison> {
    let t0 = std::time::Instant::now();
    let profile = std::env::var("OBRAIN_COMPARE_PROFILE").is_ok();
    let m = |nom: &str, t: &std::time::Instant| {
        if profile {
            eprintln!("compare.profile: {nom}: {} ms", t.elapsed().as_millis());
        }
    };
    let sa = snapshot(a, prefix_a)?;
    let sb = snapshot(b, prefix_b)?;
    m("nodes", &t0);

    // Arêtes par identité : scan SÉQUENTIEL unique de la zone edges
    // (writer().read_edge) + cache du nom de type par id — un seul passage
    // O(E), sans adjacence par nœud ni résolution par arête.
    let edge_sets = |store: &SubstrateStore,
                     ident: &[Option<String>]|
     -> Result<(BTreeSet<String>, BTreeMap<String, usize>, usize)> {
        let mut set: BTreeSet<String> = BTreeSet::new();
        let mut per_type: BTreeMap<String, usize> = BTreeMap::new();
        let mut type_names: HashMap<u16, String> = HashMap::new();
        let mut total = 0usize;
        let hw = store.edge_slot_high_water();
        for slot in 1..hw {
            let Some(rec) = store.writer().read_edge(slot)? else {
                continue;
            };
            if rec.is_tombstoned() {
                continue;
            }
            let ty = match type_names.get(&rec.edge_type) {
                Some(t) => t.clone(),
                None => {
                    let t = GraphStore::edge_type(store, obrain_common::types::EdgeId(slot))
                        .map(|x| x.to_string())
                        .unwrap_or_else(|| format!("T{}", rec.edge_type));
                    type_names.insert(rec.edge_type, t.clone());
                    t
                }
            };
            let s = ident
                .get(rec.src as usize)
                .cloned()
                .flatten()
                .unwrap_or_else(|| format!("slot{}", rec.src));
            let d = ident
                .get(rec.dst as usize)
                .cloned()
                .flatten()
                .unwrap_or_else(|| format!("slot{}", rec.dst));
            set.insert(format!("{}\t{}\t{}", s, ty, d));
            *per_type.entry(ty).or_insert(0) += 1;
            total += 1;
        }
        Ok((set, per_type, total))
    };
    let (set_a, per_a, tot_a_e) = edge_sets(a, &sa.ident)?;
    let (set_b, per_b, tot_b_e) = edge_sets(b, &sb.ident)?;
    m("edges", &t0);

    // Nœuds.
    let ajoutes = sb.ident_set.difference(&sa.ident_set).count();
    let retires = sa.ident_set.difference(&sb.ident_set).count();
    let mut modifies = 0usize;
    for id in sa.ident_set.intersection(&sb.ident_set) {
        if sa.props.get(id) != sb.props.get(id) {
            modifies += 1;
        }
    }
    // Arêtes.
    let aretes_ajoutees = set_b.difference(&set_a).count();
    let aretes_retirees = set_a.difference(&set_b).count();
    m("diff", &t0);
    let mut echantillon: Vec<String> = Vec::new();
    for k in set_b.difference(&set_a).take(limit) {
        echantillon.push(format!("+ {k}"));
    }
    for k in set_a.difference(&set_b).take(limit) {
        echantillon.push(format!("- {k}"));
    }
    let mut par_type: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for t in per_a.keys().chain(per_b.keys()) {
        par_type.insert(
            t.clone(),
            (*per_a.get(t).unwrap_or(&0), *per_b.get(t).unwrap_or(&0)),
        );
    }
    Ok(Comparaison {
        tot_a: sa.total_live,
        tot_b: sb.total_live,
        sans_a: sa.sans_identite,
        sans_b: sb.sans_identite,
        ajoutes,
        retires,
        modifies,
        aretes_a: tot_a_e,
        aretes_b: tot_b_e,
        aretes_ajoutees,
        aretes_retirees,
        jaccard_noeuds: jaccard(&sa.ident_set, &sb.ident_set),
        jaccard_aretes: jaccard(&set_a, &set_b),
        par_type,
        echantillon,
        elapsed_ms: t0.elapsed().as_millis() as u64,
    })
}

/// Étape 3 (journal d'import) : préconstruit les résumés de comparaison
/// (empreintes de nœuds + résumé d'arêtes) d'un store AU MOMENT de l'import —
/// `compare` les réutilise ensuite sans rescan (caches valides tant que le
/// store ne bouge plus). Retourne (nœuds identifiés, arêtes).
pub(crate) fn prebuild_summaries(dir: &Path, prefix: Option<&str>) -> Result<(usize, usize)> {
    let store = SubstrateStore::open_read_only(dir)
        .map_err(|e| anyhow::anyhow!("prebuild: ouverture {}: {e}", dir.display()))?;
    let e = empreintes(&store, dir, prefix)?;
    let autres: std::collections::HashSet<u64> = std::collections::HashSet::new();
    let (v, _) = edges_vue(&store, dir, prefix, &e.ident, &autres)?;
    Ok((e.live, v.total))
}

/// Exécute `obrain compare <pathA> <pathB>` (lecture seule sur les deux).
pub fn run_compare(
    path_a: &Path,
    path_b: &Path,
    limit: usize,
    attrs: bool,
    prefix_a: Option<String>,
    prefix_b: Option<String>,
    format: OutputFormat,
    quiet: bool,
) -> Result<()> {
    let a = SubstrateStore::open_read_only(path_a)
        .with_context(|| format!("compare: ouverture A {}", path_a.display()))?;
    let b = SubstrateStore::open_read_only(path_b)
        .with_context(|| format!("compare: ouverture B {}", path_b.display()))?;
    let (memo, c) = if attrs {
        (
            false,
            comparer(&a, &b, limit, prefix_a.as_deref(), prefix_b.as_deref())?,
        )
    } else {
        match essayer_memo(path_a, prefix_a.as_deref(), path_b, prefix_b.as_deref()) {
            Some(m) => (true, m),
            None => {
                let c = comparer_structurel(
                    &a,
                    path_a,
                    &b,
                    path_b,
                    prefix_a.as_deref(),
                    prefix_b.as_deref(),
                )?;
                sauver_memo(path_a, prefix_a.as_deref(), path_b, prefix_b.as_deref(), &c);
                (false, c)
            }
        }
    };

    if !quiet {
        println!(
            "compare: {} vs {} [{}]",
            path_a.display(),
            path_b.display(),
            if memo { "mémorisé" } else { "recalculé" }
        );
        println!(
            "nœuds  A={} B={} (sans identité A={} B={}) | ajoutés {} · retirés {} · modifiés {} | Jaccard {:.4}",
            c.tot_a,
            c.tot_b,
            c.sans_a,
            c.sans_b,
            c.ajoutes,
            c.retires,
            c.modifies,
            c.jaccard_noeuds
        );
        if !attrs {
            println!("(mode structurel rapide — utilisez --attrs pour les attributs modifiés)");
        }
        println!(
            "arêtes A={} B={} | ajoutées {} · retirées {} | Jaccard {:.4} | {} ms",
            c.aretes_a,
            c.aretes_b,
            c.aretes_ajoutees,
            c.aretes_retirees,
            c.jaccard_aretes,
            c.elapsed_ms
        );
        println!("-- arêtes par type (A → B) --");
        for (t, (ca, cb)) in &c.par_type {
            println!("  {t}: {ca} → {cb}");
        }
        if !c.echantillon.is_empty() {
            println!("-- échantillons (limite {limit}) --");
            for l in &c.echantillon {
                println!("  {l}");
            }
        }
    }
    output::print_result_table(
        &[
            "nœuds_ajoutés".into(),
            "nœuds_retirés".into(),
            "nœuds_modifiés".into(),
            "arêtes_ajoutées".into(),
            "arêtes_retirées".into(),
            "jaccard_nœuds".into(),
            "jaccard_arêtes".into(),
            "elapsed_ms".into(),
        ],
        &[vec![
            c.ajoutes.to_string(),
            c.retires.to_string(),
            c.modifies.to_string(),
            c.aretes_ajoutees.to_string(),
            c.aretes_retirees.to_string(),
            format!("{:.4}", c.jaccard_noeuds),
            format!("{:.4}", c.jaccard_aretes),
            c.elapsed_ms.to_string(),
        ]],
        format.into(),
        quiet,
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use obrain_core::graph::{GraphStore as _, GraphStoreMut as _};
    use obrain_substrate::SubstrateStore;

    fn ecrire_store(path: &std::path::Path, qns: &[&str], calls: &[(usize, usize)], shift: i64) {
        let s = SubstrateStore::create(path).unwrap();
        let mut ids: Vec<NodeId> = Vec::new();
        for (i, qn) in qns.iter().enumerate() {
            let id = s.create_node(&["Function"]);
            s.set_node_property(id, "qualified_name", Value::String((*qn).into()));
            s.set_node_property(
                id,
                "file_path",
                Value::String(format!("src/f{i}.rs").into()),
            );
            s.set_node_property(id, "start_line", Value::Int64(10 + shift + i as i64));
            ids.push(id);
        }
        for (i, j) in calls {
            s.create_edge(ids[*i], ids[*j], "CALLS");
        }
        s.flush().unwrap();
    }

    /// PROPRIÉTÉ : sur graphes aléatoires divergés (nœuds qn UNIQUES, un seul
    /// label), les comptes du mode structurel et du mode complet doivent être
    /// identiques (arêtes/nœuds/Jaccard/per-type) — garde-fou anti-régression
    /// entre les deux chemins.
    #[test]
    fn structurel_et_complet_alignent_sur_graphes_aleatoires() {
        let td = tempfile::tempdir().unwrap();
        let pa = td.path().join("a");
        let pb = td.path().join("b");
        // RNG déterministe (xorshift64).
        let mut st: u64 = 0x1234_5678_9abc_def0;
        let mut rnd = move || {
            st ^= st << 13;
            st ^= st >> 7;
            st ^= st << 17;
            st
        };
        for graine in 0..40u64 {
            st = 0x1234_5678_9abc_def0 ^ graine.wrapping_mul(0x9E37_79B9);
            let mut qns_a: Vec<String> = Vec::new();
            let mut edges_a: Vec<(usize, usize)> = Vec::new();
            let base = 20 + (rnd() % 20) as usize;
            for i in 0..base {
                qns_a.push(format!("f{i}"));
            }
            for _ in 0..(20 + (rnd() % 40)) {
                let i = (rnd() % base as u64) as usize;
                let j = (rnd() % base as u64) as usize;
                if i != j {
                    edges_a.push((i, j));
                }
            }
            ecrire_store(
                &pa,
                &qns_a.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                &edges_a,
                0,
            );
            // Variante B : copie + retraits/ajouts aléatoires (nœuds et arêtes).
            let mut qns_b = qns_a.clone();
            let mut edges_b: Vec<(usize, usize)> = Vec::new();
            for (i, j) in &edges_a {
                if (rnd() % 100) >= 15 {
                    edges_b.push((*i, *j));
                }
            }
            let extra = rnd() % 4;
            for k in 0..extra {
                let n = qns_b.len();
                qns_b.push(format!("fx{graine}_{k}"));
                edges_b.push(((rnd() % n as u64) as usize, n));
            }
            for _ in 0..(rnd() % 6) {
                let n = qns_b.len();
                let (i, j) = ((rnd() % n as u64) as usize, (rnd() % n as u64) as usize);
                if i != j {
                    edges_b.push((i, j));
                }
            }
            ecrire_store(
                &pb,
                &qns_b.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                &edges_b,
                (graine % 3) as i64,
            );
            let a = SubstrateStore::open_read_only(&pa).unwrap();
            let b = SubstrateStore::open_read_only(&pb).unwrap();
            let st_c = comparer_structurel(&a, &pa, &b, &pb, None, None).unwrap();
            let f = comparer(&a, &b, 100, None, None).unwrap();
            assert_eq!(st_c.ajoutes, f.ajoutes, "graine {graine}: nœuds ajoutés");
            assert_eq!(st_c.retires, f.retires, "graine {graine}: nœuds retirés");
            assert_eq!(
                st_c.aretes_retirees, f.aretes_retirees,
                "graine {graine}: arêtes retirées"
            );
            assert!(
                (st_c.jaccard_noeuds - f.jaccard_noeuds).abs() < 1e-9,
                "graine {graine}: jaccard nœuds"
            );
            assert!(
                (st_c.jaccard_aretes - f.jaccard_aretes).abs() < 1e-9,
                "graine {graine}: jaccard arêtes"
            );
            assert_eq!(st_c.par_type, f.par_type, "graine {graine}: par type");
            // Assainit pour la graine suivante.
            let _ = fs::remove_dir_all(&pa);
            let _ = fs::remove_dir_all(&pb);
            fs::create_dir_all(&pa).unwrap();
            fs::create_dir_all(&pb).unwrap();
        }
    }

    #[test]
    fn memo_rejoue_le_resultat_puis_sinvalide_au_changement() {
        let td = tempfile::tempdir().unwrap();
        let pa = td.path().join("a");
        let pb = td.path().join("b");
        ecrire_store(&pa, &["f1", "f2", "f3"], &[(0, 1)], 0);
        ecrire_store(&pb, &["f1", "f2", "f3", "f4"], &[(0, 1), (0, 3)], 1);
        let a = SubstrateStore::open_read_only(&pa).unwrap();
        let b = SubstrateStore::open_read_only(&pb).unwrap();
        let c = comparer_structurel(&a, &pa, &b, &pb, None, None).unwrap();
        sauver_memo(&pa, None, &pb, None, &c);
        let m = essayer_memo(&pa, None, &pb, None).expect("mémorisé");
        assert_eq!(m.ajoutes, c.ajoutes);
        assert_eq!(m.aretes_ajoutees, c.aretes_ajoutees);
        assert_eq!(m.par_type, c.par_type);
        assert!((m.jaccard_noeuds - c.jaccard_noeuds).abs() < 1e-9);
        // Invalidation : le store A change (nouveau fichier plus récent que
        // l'empreinte) → plus de rejeu. Granularité temporelle du FS : on
        // attend que le nouveau fichier soit STRICTEMENT plus récent.
        let fp = fp_path(&pa, None);
        let ref_mt = fp_mtime(&fp).expect("empreinte");
        std::fs::write(pa.join("extra.txt"), b"x").unwrap();
        // L'invalidation (>=) doit être immédiate : le nouveau fichier a un
        // mtime ≥ à l'empreinte (même tick inclus).
        let extra_mt = fs::metadata(pa.join("extra.txt"))
            .and_then(|m| m.modified())
            .unwrap();
        assert!(
            extra_mt >= ref_mt && essayer_memo(&pa, None, &pb, None).is_none(),
            "store modifié → résultat mémorisé caduc (extra {extra_mt:?} vs empreinte {ref_mt:?})"
        );
    }

    /// PARITÉ : lire l'empreinte depuis le cache (2e appel) doit donner
    /// EXACTEMENT le même résultat que le calcul (1er appel) — protège contre
    /// les bugs de format/en-tête (offsets, magic, version).
    /// Chantier 3 — PARITÉ : après un premier compare (caches écrits), un
    /// second compare doit rejouer nœuds ET arêtes depuis les caches avec un
    /// résultat identique ; puis quand UN SEUL store change, le résultat
    /// recalcule et reste aligné sur le mode complet.
    #[test]
    fn edges_cache_rejoue_puis_suit_un_changement_unilateral() {
        let td = tempfile::tempdir().unwrap();
        let pa = td.path().join("a");
        let pb = td.path().join("b");
        ecrire_store(&pa, &["f1", "f2", "f3"], &[(0, 1)], 0);
        {
            use obrain_substrate::SubstrateStore;
            let s = SubstrateStore::create(&pb).unwrap();
            let f1 = s.create_node(&["Function"]);
            s.set_node_property(f1, "qualified_name", Value::String("f1".into()));
            let f2 = s.create_node(&["Function"]);
            s.set_node_property(f2, "qualified_name", Value::String("f2".into()));
            let f4 = s.create_node(&["Function"]);
            s.set_node_property(f4, "qualified_name", Value::String("f4".into()));
            s.create_edge(f1, f2, "CALLS");
            s.create_edge(f1, f4, "CALLS");
            s.flush().unwrap();
        }
        let a = SubstrateStore::open_read_only(&pa).unwrap();
        let b = SubstrateStore::open_read_only(&pb).unwrap();
        let c1 = comparer_structurel(&a, &pa, &b, &pb, None, None).unwrap();
        let c2 = comparer_structurel(&a, &pa, &b, &pb, None, None).unwrap();
        // Parité cache == calcul (nœuds ET arêtes via compare-edges).
        assert_eq!(c1.ajoutes, c2.ajoutes);
        assert_eq!(c1.aretes_ajoutees, c2.aretes_ajoutees);
        assert_eq!(c1.par_type, c2.par_type);
        assert!((c1.jaccard_aretes - c2.jaccard_aretes).abs() < 1e-9);
        drop(a);
        drop(b);
        // Changement UNILATÉRAL : B gagne f5 et f2→f5 (A reste intact).
        {
            use obrain_substrate::SubstrateStore;
            let s = SubstrateStore::open(&pb).unwrap();
            let f2 = GraphStore::node_ids(&s)
                .into_iter()
                .find(|n| {
                    matches!(
                        s.get_node_property(*n, &PropertyKey::new("qualified_name")),
                        Some(Value::String(q)) if q.as_str() == "f2"
                    )
                })
                .expect("f2");
            let f5 = s.create_node(&["Function"]);
            s.set_node_property(f5, "qualified_name", Value::String("f5".into()));
            s.create_edge(f2, f5, "CALLS");
            s.flush().unwrap();
        }
        let a = SubstrateStore::open_read_only(&pa).unwrap();
        let b = SubstrateStore::open_read_only(&pb).unwrap();
        let st = comparer_structurel(&a, &pa, &b, &pb, None, None).unwrap();
        let f = comparer(&a, &b, 100, None, None).unwrap();
        assert_eq!(st.ajoutes, f.ajoutes, "f4 + f5");
        assert_eq!(st.aretes_ajoutees, f.aretes_ajoutees, "f1→f4 + f2→f5");
        assert_eq!(st.aretes_retirees, f.aretes_retirees);
        assert_eq!(st.par_type, f.par_type);
        assert!((st.jaccard_noeuds - f.jaccard_noeuds).abs() < 1e-9);
        assert!((st.jaccard_aretes - f.jaccard_aretes).abs() < 1e-9);
    }

    #[test]
    fn empreintes_cache_egal_au_recalcul() {
        let td = tempfile::tempdir().unwrap();
        let pa = td.path().join("a");
        {
            use obrain_substrate::SubstrateStore;
            let s = SubstrateStore::create(&pa).unwrap();
            let n1 = s.create_node(&["Function"]);
            s.set_node_property(n1, "qualified_name", Value::String("f1".into()));
            let n2 = s.create_node(&["Function"]);
            s.set_node_property(n2, "qualified_name", Value::String("f2".into()));
            // Nœud SANS qn → « sans identité ».
            let _n3 = s.create_node(&["Function"]);
            s.flush().unwrap();
        }
        let s = SubstrateStore::open_read_only(&pa).unwrap();
        let e1 = empreintes(&s, &pa, Some("p")).unwrap(); // calcul + écrit cache
        let e2 = empreintes(&s, &pa, Some("p")).unwrap(); // lit le cache
        assert_eq!(
            e1.ident, e2.ident,
            "vecteur d'empreintes identique (cache vs calcul)"
        );
        assert_eq!(e1.membres, e2.membres);
        assert_eq!(e1.sans, e2.sans);
        assert_eq!(e1.live, e2.live);
        assert_eq!(e1.sans, 1, "un nœud sans qn");
        // Sans cache (supprimé) → recalcul identique.
        let _ = fs::remove_file(cache_file(&pa, Some("p")));
        let e3 = empreintes(&s, &pa, Some("p")).unwrap();
        assert_eq!(e1.ident, e3.ident);
        assert_eq!(e1.membres, e3.membres);
        assert_eq!(e1.sans, e3.sans);
        assert_eq!(e1.live, e3.live);
    }

    #[test]
    fn structurel_aligne_sur_le_mode_complet() {
        let td = tempfile::tempdir().unwrap();
        let pa = td.path().join("a");
        let pb = td.path().join("b");
        ecrire_store(&pa, &["f1", "f2", "f3"], &[(0, 1)], 0);
        {
            use obrain_substrate::SubstrateStore;
            let s = SubstrateStore::create(&pb).unwrap();
            let f1 = s.create_node(&["Function"]);
            s.set_node_property(f1, "qualified_name", Value::String("f1".into()));
            s.set_node_property(f1, "file_path", Value::String("src/f0.rs".into()));
            s.set_node_property(f1, "start_line", Value::Int64(99));
            let f2 = s.create_node(&["Function"]);
            s.set_node_property(f2, "qualified_name", Value::String("f2".into()));
            s.set_node_property(f2, "file_path", Value::String("src/f1.rs".into()));
            s.set_node_property(f2, "start_line", Value::Int64(11));
            let f4 = s.create_node(&["Function"]);
            s.set_node_property(f4, "qualified_name", Value::String("f4".into()));
            s.set_node_property(f4, "file_path", Value::String("src/f4.rs".into()));
            s.set_node_property(f4, "start_line", Value::Int64(13));
            s.create_edge(f1, f2, "CALLS");
            s.create_edge(f1, f4, "CALLS");
            s.flush().unwrap();
        }
        let a = SubstrateStore::open_read_only(&pa).unwrap();
        let b = SubstrateStore::open_read_only(&pb).unwrap();
        let f = comparer_structurel(&a, &pa, &b, &pb, None, None).unwrap();
        assert_eq!(f.ajoutes, 1);
        assert_eq!(f.retires, 1);
        assert_eq!(f.aretes_ajoutees, 1);
        assert_eq!(f.aretes_retirees, 0);
        assert_eq!(f.aretes_a, 1);
        assert_eq!(f.aretes_b, 2);
        assert_eq!(f.par_type.get("CALLS"), Some(&(1, 2)));
        // Identique → vide.
        let f2 = comparer_structurel(&a, &pa, &a, &pa, None, None).unwrap();
        assert_eq!(f2.ajoutes, 0);
        assert_eq!(f2.aretes_ajoutees, 0);
        assert!((f2.jaccard_noeuds - 1.0).abs() < 1e-9);
    }

    #[test]
    fn comparer_detecte_ajouts_retraits_modifs_et_deltas() {
        let td = tempfile::tempdir().unwrap();
        let pa = td.path().join("a");
        let pb = td.path().join("b");
        // Base : f1→f2, f3 isolé — lignes 10,11,12.
        ecrire_store(&pa, &["f1", "f2", "f3"], &[(0, 1)], 0);
        // Variante : f1→f2 + f1→f4 (nouvelle) ; f3 retiré ; f1 déplacé (ligne 99).
        {
            let s = SubstrateStore::create(&pb).unwrap();
            let f1 = s.create_node(&["Function"]);
            s.set_node_property(f1, "qualified_name", Value::String("f1".into()));
            s.set_node_property(f1, "file_path", Value::String("src/f0.rs".into()));
            s.set_node_property(f1, "start_line", Value::Int64(99));
            let f2 = s.create_node(&["Function"]);
            s.set_node_property(f2, "qualified_name", Value::String("f2".into()));
            s.set_node_property(f2, "file_path", Value::String("src/f1.rs".into()));
            s.set_node_property(f2, "start_line", Value::Int64(11));
            let f4 = s.create_node(&["Function"]);
            s.set_node_property(f4, "qualified_name", Value::String("f4".into()));
            s.set_node_property(f4, "file_path", Value::String("src/f4.rs".into()));
            s.set_node_property(f4, "start_line", Value::Int64(13));
            s.create_edge(f1, f2, "CALLS");
            s.create_edge(f1, f4, "CALLS");
            s.flush().unwrap();
        }
        let a = SubstrateStore::open_read_only(&pa).unwrap();
        let b = SubstrateStore::open_read_only(&pb).unwrap();
        let c = comparer(&a, &b, 20, None, None).unwrap();
        assert_eq!(c.ajoutes, 1, "f4 ajouté");
        assert_eq!(c.retires, 1, "f3 retiré");
        assert_eq!(c.modifies, 1, "f1 modifié (file_path + start_line)");
        assert_eq!(c.aretes_ajoutees, 1, "f1→f4 ajoutée");
        assert_eq!(c.aretes_retirees, 0);
        assert_eq!(c.aretes_a, 1);
        assert_eq!(c.aretes_b, 2);
        assert_eq!(c.par_type.get("CALLS"), Some(&(1, 2)));
        // Identiques → diff vide, Jaccard 1.
        let c2 = comparer(&a, &a, 20, None, None).unwrap();
        assert_eq!(c2.ajoutes, 0);
        assert_eq!(c2.retires, 0);
        assert_eq!(c2.modifies, 0);
        assert_eq!(c2.aretes_ajoutees, 0);
        assert_eq!(c2.aretes_retirees, 0);
        assert!((c2.jaccard_noeuds - 1.0).abs() < 1e-9);
        assert!((c2.jaccard_aretes - 1.0).abs() < 1e-9);
    }
}
