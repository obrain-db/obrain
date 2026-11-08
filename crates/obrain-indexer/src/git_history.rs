//! Historique git → couplage de changement — équivalent Rust de
//! pass_githistory.c du C.
//!
//! Parse `git log --name-only` (1 an, max 10000 commits), calcule :
//! - métadonnées temporelles par fichier (change_count, last_modified) ;
//! - paires de fichiers co-modifiés (FILE_CHANGES_WITH) avec
//!   {co_changes, coupling_score, last_co_change} — score = co_count /
//!   min(freq_a, freq_b), seuils : ≥3 co-changements, score ≥ 0,3, commits
//!   de ≤20 fichiers (bruit merge ignoré).

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

/// Seuil minimal de co-changements pour créer une arête.
const GH_MIN_COMMITS: usize = 3;
/// Score de couplage minimal.
const MIN_COUPLING_SCORE: f64 = 0.3;
/// Commits avec plus de fichiers = merge/refactor bruité → ignorés.
const GH_MAX_FILES: usize = 20;

/// Un commit et ses fichiers modifiés.
struct CommitFiles {
    timestamp: i64,
    files: Vec<String>,
}

/// Métadonnées temporelles d'un fichier.
#[derive(Debug, Clone)]
pub struct FileTemporal {
    pub file_path: String,
    pub change_count: u32,
    pub last_modified: i64,
}

/// Une paire de fichiers co-évoluants.
#[derive(Debug, Clone)]
pub struct ChangeCoupling {
    pub file_a: String,
    pub file_b: String,
    pub co_change_count: u32,
    pub coupling_score: f64,
    pub last_co_change: i64,
}

/// Résultat de l'analyse git.
#[derive(Debug, Default)]
pub struct GitHistory {
    pub file_temporal: Vec<FileTemporal>,
    pub couplings: Vec<ChangeCoupling>,
}

fn git(repo: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Parse la sortie de `git log --name-only --pretty=format:COMMIT:%H:%ct`.
fn parse_git_log(repo: &Path) -> Vec<CommitFiles> {
    let raw = match git(
        repo,
        &[
            "log",
            "--name-only",
            "--pretty=format:COMMIT:%H:%ct",
            "--since=1 year ago",
            "--max-count=10000",
        ],
    ) {
        Some(r) => r,
        None => return Vec::new(),
    };
    let mut commits = Vec::new();
    let mut current: Option<CommitFiles> = None;
    for line in raw.lines() {
        let line = line.trim_end_matches(['\n', '\r']);
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("COMMIT:") {
            if let Some(cur) = current.take()
                && !cur.files.is_empty()
            {
                commits.push(cur);
            }
            let mut parts = rest.split(':');
            let _sha = parts.next();
            let ts = parts
                .next()
                .and_then(|t| t.parse::<i64>().ok())
                .unwrap_or(0);
            current = Some(CommitFiles {
                timestamp: ts,
                files: Vec::new(),
            });
            continue;
        }
        if let Some(c) = current.as_mut() {
            // Fichier modifié (relatif au repo).
            let f = line.trim();
            if !f.is_empty() && !f.starts_with("COMMIT:") && !f.contains("=>") {
                c.files.push(f.to_string());
            }
        }
    }
    if let Some(cur) = current
        && !cur.files.is_empty()
    {
        commits.push(cur);
    }
    commits
}

/// Analyse l'historique git du repo : couplages + temporalité.
pub fn analyze(repo: &Path) -> GitHistory {
    let mut result = GitHistory::default();
    let commits = parse_git_log(repo);
    if commits.is_empty() {
        return result;
    }

    // Fréquence par fichier + temporalité.
    let mut file_counts: HashMap<String, u32> = HashMap::new();
    let mut file_last: HashMap<String, i64> = HashMap::new();
    // Paires : "a\u{1}b" → (count, last_ts).
    let mut pair_counts: HashMap<String, (u32, i64)> = HashMap::new();

    for c in &commits {
        if c.files.len() > GH_MAX_FILES {
            continue;
        }
        for f in &c.files {
            *file_counts.entry(f.clone()).or_default() += 1;
            let e = file_last.entry(f.clone()).or_default();
            if c.timestamp > *e {
                *e = c.timestamp;
            }
        }
        for i in 0..c.files.len() {
            for j in (i + 1)..c.files.len() {
                let (a, b) = if c.files[i] < c.files[j] {
                    (&c.files[i], &c.files[j])
                } else {
                    (&c.files[j], &c.files[i])
                };
                let key = format!("{a}\u{1}{b}");
                let e = pair_counts.entry(key).or_insert((0, 0));
                e.0 += 1;
                if c.timestamp > e.1 {
                    e.1 = c.timestamp;
                }
            }
        }
    }

    // Temporalité par fichier.
    for (f, count) in &file_counts {
        result.file_temporal.push(FileTemporal {
            file_path: f.clone(),
            change_count: *count,
            last_modified: file_last.get(f).copied().unwrap_or(0),
        });
    }

    // Couplages.
    for (key, (co_count, last_ts)) in &pair_counts {
        if *co_count < GH_MIN_COMMITS as u32 {
            continue;
        }
        let Some(sep) = key.find('\u{1}') else {
            continue;
        };
        let file_a = key[..sep].to_string();
        let file_b = key[sep + 1..].to_string();
        let (Some(ca), Some(cb)) = (file_counts.get(&file_a), file_counts.get(&file_b)) else {
            continue;
        };
        let min_total = (*ca).min(*cb);
        if min_total == 0 {
            continue;
        }
        let score = *co_count as f64 / min_total as f64;
        if score < MIN_COUPLING_SCORE {
            continue;
        }
        result.couplings.push(ChangeCoupling {
            file_a,
            file_b,
            co_change_count: *co_count,
            coupling_score: score,
            last_co_change: *last_ts,
        });
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn init_repo() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().to_path_buf();
        Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&p)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "t@t"])
            .current_dir(&p)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "t"])
            .current_dir(&p)
            .output()
            .unwrap();
        (dir, p)
    }

    fn commit(p: &Path, files: &[&str], seq: u32) {
        for f in files {
            std::fs::write(p.join(f), format!("x{seq}\n")).unwrap();
        }
        Command::new("git")
            .args(["add", "."])
            .current_dir(p)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-qm", "c"])
            .current_dir(p)
            .output()
            .unwrap();
    }

    #[test]
    fn analyse_detecte_couplage() {
        let (_dir, p) = init_repo();
        // 3 commits modifiant a.rs + b.rs ensemble → couplage ≥ 3.
        for i in 0..3 {
            commit(&p, &["a.rs", "b.rs"], i + 1);
        }
        // 1 commit isolé sur c.rs.
        commit(&p, &["c.rs"], 9);
        let hist = analyze(&p);
        assert!(
            hist.couplings.iter().any(|c| c.co_change_count >= 3),
            "couplage a-b attendu: {:?}",
            hist.couplings
        );
        let a = hist
            .file_temporal
            .iter()
            .find(|t| t.file_path == "a.rs")
            .expect("a.rs doit être suivi");
        assert_eq!(a.change_count, 3);
    }

    #[test]
    fn pas_de_couplage_sans_co_evolution() {
        let (_dir, p) = init_repo();
        commit(&p, &["a.rs"], 1);
        commit(&p, &["b.rs"], 2);
        commit(&p, &["c.rs"], 3);
        let hist = analyze(&p);
        assert!(
            hist.couplings.is_empty(),
            "pas de co-évolution: {:?}",
            hist.couplings
        );
    }
}
