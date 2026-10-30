//! `obrain jump` — jump columns (la FORME UNIFICATRICE des connexions).
//!
//! * `jump materialize <db> --type A --type B [--type ...]` — matérialise
//!   les jumps 2-hop pour les paires de types d'arêtes données (les paires
//!   CHAUDES). Idempotent : la zone est entièrement réécrite.
//! * `jump list <db>` — liste les zones jump existantes.
//! * `jump stats <db> --type A --type B` — nombre de paires matérialisées.
//!
//! La requête multi-hop via une zone matérialisée est une lecture contiguë
//! (µs) au lieu d'une marche des chaînes (ms) — POC prouvé : 27,5 ms →
//! 7,3 µs (3 745×) sur CALLS∘CALLS.

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::Result;
use clap::Subcommand;
use obrain_substrate::jump;

use crate::OutputFormat;

/// Sous-commandes `jump`.
#[derive(Subcommand)]
pub enum JumpCommands {
    /// Matérialise les jumps 2-hop pour les paires de types données.
    /// Idempotent (la zone est réécrite entièrement). Les types par défaut
    /// couvrent les paires chaudes typiques (CALLS∘CALLS, IMPORTS∘IMPORTS).
    Materialize {
        /// Path to the database
        path: PathBuf,

        /// Edge types to materialize (repeated: --type A --type B)
        #[arg(long = "type", required = true)]
        types: Vec<String>,

        /// Quiet output (stats only)
        #[arg(long)]
        quiet: bool,
    },

    /// Liste les zones jump existantes (avec leur nombre de paires)
    List {
        /// Path to the database
        path: PathBuf,
    },

    /// Affiche le nombre de paires d'une zone jump
    Stats {
        /// Path to the database
        path: PathBuf,

        /// Edge type A
        #[arg(long)]
        type_a: String,

        /// Edge type B
        #[arg(long)]
        type_b: String,
    },
}

/// Point d'entrée du CLI `jump`.
pub fn run(cmd: JumpCommands, format: OutputFormat, quiet: bool) -> Result<()> {
    match cmd {
        JumpCommands::Materialize { path, types, quiet } => {
            materialize(&path, &types, format, quiet)
        }
        JumpCommands::List { path } => list(&path, format, quiet),
        JumpCommands::Stats {
            path,
            type_a,
            type_b,
        } => stats(&path, &type_a, &type_b, format, quiet),
    }
}

fn substrate(
    db: &obrain_engine::ObrainDB,
) -> Result<std::sync::Arc<obrain_substrate::SubstrateStore>> {
    db.substrate_handle().ok_or_else(|| {
        anyhow::anyhow!(
            "cette base n'expose pas de SubstrateStore : les jump columns \
             exigent le mode substrat (aucune dégradation silencieuse)"
        )
    })
}

fn materialize(path: &Path, types: &[String], format: OutputFormat, quiet: bool) -> Result<()> {
    if types.len() < 2 {
        anyhow::bail!("jump materialize: au moins 2 types d'arêtes requis (--type A --type B)");
    }
    let db = super::open_existing(path)?;
    let store = substrate(&db)?;

    // Toutes les paires (i, i+1) — les types consécutifs forment les paires.
    let mut results = Vec::new();
    let debut = Instant::now();
    for w in types.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        let t = Instant::now();
        match store.materialize_jump(a, b) {
            Ok(stats) => {
                let duree = t.elapsed().as_secs_f64() * 1000.0;
                results.push((a.clone(), b.clone(), stats, duree));
            }
            Err(e) => {
                if !quiet {
                    crate::output::status(&format!("jump {a}∘{b}: {e} (ignoré)"), true);
                }
            }
        }
    }
    let total = debut.elapsed().as_secs_f64() * 1000.0;

    if matches!(format, OutputFormat::Json) {
        let json: Vec<serde_json::Value> = results
            .iter()
            .map(|(a, b, s, d)| {
                serde_json::json!({
                    "zone": jump::zone_name(a, b),
                    "type_a": a,
                    "type_b": b,
                    "sources": s.sources,
                    "pairs": s.pairs,
                    "bytes": s.bytes,
                    "ms": d,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else if !quiet {
        for (a, b, s, d) in &results {
            crate::output::status(
                &format!(
                    "jump {a}∘{b}: {} sources, {} paires ({} Ko) en {d:.1} ms",
                    s.sources,
                    s.pairs,
                    s.bytes / 1024
                ),
                false,
            );
        }
        crate::output::status(&format!("{total:.1} ms au total"), quiet);
    }
    Ok(())
}

fn list(path: &Path, format: OutputFormat, quiet: bool) -> Result<()> {
    let db = super::open_existing(path)?;
    let store = substrate(&db)?;
    // Scanne le répertoire du store pour les zones `substrate.jump.*`.
    let sub = store.writer().substrate();
    let guard = sub.lock();
    let dir = guard.path().to_path_buf();
    drop(guard);

    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some((a, b)) = jump::parse_zone_name(&name) {
            let count = store.jump_count(&a, &b);
            entries.push((name, count));
        }
    }
    entries.sort();

    if matches!(format, OutputFormat::Json) {
        let json: Vec<serde_json::Value> = entries
            .iter()
            .map(|(n, c)| serde_json::json!({ "zone": n, "pairs": c }))
            .collect();
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else if !quiet {
        if entries.is_empty() {
            crate::output::status("aucune zone jump (lancer `obrain jump materialize`)", quiet);
        }
        for (n, c) in &entries {
            crate::output::status(&format!("{n}: {c} paires"), false);
        }
    }
    Ok(())
}

fn stats(path: &Path, type_a: &str, type_b: &str, format: OutputFormat, quiet: bool) -> Result<()> {
    let db = super::open_existing(path)?;
    let store = substrate(&db)?;
    let count = store.jump_count(type_a, type_b);
    if matches!(format, OutputFormat::Json) {
        println!(
            "{}",
            serde_json::json!({
                "zone": jump::zone_name(type_a, type_b),
                "pairs": count,
            })
        );
    } else if !quiet {
        crate::output::status(
            &format!("jump {type_a}∘{type_b}: {count} paires matérialisées"),
            quiet,
        );
    }
    Ok(())
}
