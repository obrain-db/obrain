//! Contexte git d'un repo indexé — équivalent Rust de git_context.c du C.
//!
//! Capture HEAD sha, branche courante, base (merge-base upstream), état
//! worktree — pour créer le nœud Branch et tisser le code à son historique.

use std::path::Path;
use std::process::Command;

/// Contexte git d'un repo (champs vides si pas git).
#[derive(Debug, Clone, Default)]
pub struct GitContext {
    pub is_git: bool,
    pub git_dir: String,
    pub git_common_dir: String,
    pub head_sha: String,
    pub branch: String,
    pub is_detached: bool,
    pub is_worktree: bool,
    pub base_sha: String,
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
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Capture le contexte git du repo (best-effort : pas git → tout vide).
pub fn capture(repo: &Path) -> GitContext {
    let mut ctx = GitContext::default();
    let git_dir = git(repo, &["rev-parse", "--git-dir"]);
    let Some(git_dir) = git_dir else {
        return ctx; // pas un repo git
    };
    ctx.is_git = true;
    ctx.git_dir = git_dir;
    ctx.git_common_dir = git(repo, &["rev-parse", "--git-common-dir"]).unwrap_or_default();
    ctx.head_sha = git(repo, &["rev-parse", "--verify", "HEAD"]).unwrap_or_default();
    match git(repo, &["symbolic-ref", "--quiet", "--short", "HEAD"]) {
        Some(b) => {
            ctx.branch = b;
            ctx.is_detached = false;
        }
        None => {
            ctx.branch = "DETACHED".to_string();
            ctx.is_detached = true;
        }
    }
    ctx.is_worktree = !ctx.git_dir.is_empty()
        && !ctx.git_common_dir.is_empty()
        && ctx.git_dir != ctx.git_common_dir;
    ctx.base_sha = git(repo, &["merge-base", "HEAD", "@{upstream}"]).unwrap_or_default();
    ctx
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// Crée un mini-repo git temporaire pour tester la capture.
    fn tmp_git_repo() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn a() {}\n").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args([
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-qm",
                "init",
            ])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let p = dir.path().to_path_buf();
        (dir, p)
    }

    #[test]
    fn capture_detecte_git_et_head() {
        let (dir, path) = tmp_git_repo();
        let ctx = capture(path.as_path());
        assert!(ctx.is_git, "doit être git");
        assert_eq!(ctx.branch, "main");
        assert!(!ctx.head_sha.is_empty(), "HEAD doit exister");
        assert_eq!(ctx.head_sha.len(), 40, "sha complet");
        drop(dir);
    }

    #[test]
    fn capture_pas_git() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = capture(dir.path());
        assert!(!ctx.is_git);
        drop(dir);
    }
}
