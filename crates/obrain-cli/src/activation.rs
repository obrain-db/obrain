//! JOURNAL D'ACTIVATION — ce que l'engine consulte, en direct.
//!
//! Besoin : « le graph devrait s'activer en temps réel dès consultation de
//! l'engine ». L'obstacle n'est pas l'affichage, c'est la SOURCE : l'engine
//! obrain (serveur MCP) est un **processus séparé** du serveur viz. Un état
//! gardé en mémoire vive par le serveur viz ne verrait donc jamais ce que
//! l'engine consulte — on afficherait une activation décorative, ce qui est
//! exactement ce qu'il ne faut pas.
//!
//! D'où un journal APPEND-ONLY sur disque, un fichier par store :
//!
//! ```text
//! {"t":1789046000123,"qn":"…","kind":"subgraph"}
//! ```
//!
//! N'importe quel processus de l'engine peut y écrire (le serveur viz comme le
//! serveur MCP) ; le dashboard le relit et affiche l'énergie décrue. Le fichier
//! survit aux redémarrages et ne dépend d'aucune horloge partagée.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Demi-vie de l'activation : au bout de 20 s, un nœud consulté a perdu la
/// moitié de son énergie. Le graphe « respire » donc : ce qui vient d'être lu
/// brille, ce qui a été lu il y a une minute s'éteint.
pub const HALF_LIFE_MS: i64 = 20_000;

/// Fenêtre maximale relue (au-delà, tout est éteint de toute façon).
pub const WINDOW_MS: i64 = 120_000;

/// Taille au-delà de laquelle le journal est élagué (on garde la queue).
const MAX_BYTES: u64 = 2 * 1024 * 1024;

/// Horodatage courant en millisecondes.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Répertoire du journal (le même cache que les agrégats viz).
fn dir() -> PathBuf {
    std::env::var_os("OBRAIN_VIZ_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/obrain-viz-cache"))
        .join("activation")
}

/// Un fichier par store (chemin haché : un chemin peut contenir `/`).
pub fn log_path(store: &Path) -> PathBuf {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    store.to_string_lossy().hash(&mut h);
    dir().join(format!("{:016x}.jsonl", h.finish()))
}

/// Enregistre une consultation de l'engine. Best-effort : ce journal ne doit
/// JAMAIS faire échouer une requête utilisateur.
pub fn record(store: &Path, qns: &[String], kind: &str) {
    if qns.is_empty() {
        return;
    }
    let path = log_path(store);
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    // Élagage : on ne relit que la queue, le journal ne doit pas croître sans fin.
    if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > MAX_BYTES {
        if let Ok(text) = std::fs::read_to_string(&path) {
            let keep = text.len() / 2;
            let tail = text.get(keep..).unwrap_or("");
            let _ = std::fs::write(&path, tail);
        }
    }
    let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    let t = now_ms();
    let mut out = String::with_capacity(qns.len() * 80);
    for qn in qns.iter().take(400) {
        let line = serde_json::json!({ "t": t, "qn": qn, "kind": kind });
        out.push_str(&line.to_string());
        out.push('\n');
    }
    let _ = f.write_all(out.as_bytes());
}

/// Une activation relue, avec son énergie décrue.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Hit {
    pub qn: String,
    pub kind: String,
    pub t: i64,
    /// Énergie dans [0,1] : 1 = consulté à l'instant, 0.5 = il y a une demi-vie.
    pub energy: f64,
}

/// Relit les activations récentes, fusionnées par nœud (énergie maximale).
pub fn read(store: &Path, window_ms: i64) -> Vec<Hit> {
    let path = log_path(store);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let now = now_ms();
    let window = window_ms.clamp(1_000, WINDOW_MS);
    let mut best: HashMap<String, Hit> = HashMap::new();
    // On ne lit que la queue : le journal peut être long.
    let tail = if text.len() > 512 * 1024 {
        &text[text.len() - 512 * 1024..]
    } else {
        &text[..]
    };
    for line in tail.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue; // ligne tronquée par l'élagage : on l'ignore
        };
        let (Some(qn), Some(t)) = (
            v.get("qn").and_then(|x| x.as_str()),
            v.get("t").and_then(|x| x.as_i64()),
        ) else {
            continue;
        };
        let age = now - t;
        if age > window || age < 0 {
            continue;
        }
        let energy = 0.5_f64.powf(age as f64 / HALF_LIFE_MS as f64);
        let kind = v
            .get("kind")
            .and_then(|x| x.as_str())
            .unwrap_or("consultation")
            .to_string();
        match best.get(qn) {
            Some(prev) if prev.energy >= energy => {}
            _ => {
                best.insert(
                    qn.to_string(),
                    Hit {
                        qn: qn.to_string(),
                        kind,
                        t,
                        energy,
                    },
                );
            }
        }
    }
    let mut out: Vec<Hit> = best.into_values().collect();
    out.sort_by(|a, b| b.energy.partial_cmp(&a.energy).unwrap_or(std::cmp::Ordering::Equal));
    out
}
