//! `obrain journal migrate` — importe le journal de session depuis des
//! fichiers JSONL (un par session) dans les zones substrate dédiées.
//!
//! La migration est BULK : un seul process, store RW ouvert une fois, chaque
//! fichier est lu et encodé bincode directement dans la zone (pas de JSON
//! transporté par un bridge, pas de spawn par session). Le fichier JSONL
//! contient les lignes brutes du log (`{type,seq,time,data}`, une par ligne).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::OutputFormat;

#[derive(clap::Subcommand, Debug, Clone)]
pub enum JournalCommands {
    /// Importe les journaux de session (fichiers <session-id>.jsonl) dans les
    /// zones substrate obrain.
    Migrate {
        /// Répertoire contenant les fichiers <session-id>.jsonl.
        dir: PathBuf,
        /// Store obrain cible.
        #[arg(long)]
        store: PathBuf,
    },
}

pub fn run(cmd: JournalCommands, _format: OutputFormat, quiet: bool) -> Result<()> {
    match cmd {
        JournalCommands::Migrate { dir, store } => migrate(&dir, &store, quiet),
    }
}

fn migrate(dir: &Path, store: &Path, quiet: bool) -> Result<()> {
    let db = obrain_engine::ObrainDB::open(store)
        .with_context(|| format!("ouverture du store {}", store.display()))?;

    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("lecture du répertoire {}", dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "jsonl").unwrap_or(false))
        .collect();
    files.sort();

    let mut imported = 0usize;
    let mut total_lines = 0usize;
    let t0 = std::time::Instant::now();
    for path in &files {
        let fname = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let session_id = fname.trim_end_matches(".jsonl");
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("lecture de {}", path.display()))?;
        let lines: Vec<String> = content
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
        if lines.is_empty() {
            continue;
        }
        total_lines += lines.len();
        db.write_session_journal(session_id, &lines)
            .map_err(|e| anyhow::anyhow!("écriture du journal de {session_id}: {e}"))?;
        imported += 1;
        if !quiet && imported % 20 == 0 {
            eprintln!("  {imported} sessions…");
        }
    }
    let dt = t0.elapsed();
    if !quiet {
        eprintln!("migré : {imported} sessions, {total_lines} events en {dt:.2?}");
    }
    Ok(())
}
