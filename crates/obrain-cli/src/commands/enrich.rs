//! `obrain enrich` — relief structurel post-import (chantier « import de code
//! enrichi », docs/PROPOSAL-IMPORT-RELIEF-2026-09-05.md).
//!
//! Passe(s) d'enrichissement ADDITIVES et IDEMPOTENTES sur un store existant :
//!
//! - `--ricci` : refresh batch de la courbure de Ricci-Ollivier (Jost-Liu)
//!   sur TOUTES les arêtes vivantes, persistée dans l'EdgeRecord
//!   (`ricci_f32`, via `update_edge` + WAL) — débloque `node_curvature` et
//!   les signaux bottleneck/core (CALLs), enrichit le relief du projet
//!   importé. Idempotent : relancer ne fait que re-confirmer les valeurs.
//!
//! Tout se passe en Rust (aucun script), aucune modification des formats ni
//! des chemins d'import existants.

use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::OutputFormat;
use crate::output;

/// Exécute les passes demandées sur le store. Au moins une passe est requise.
pub fn run_enrich(path: &Path, ricci: bool, format: OutputFormat, quiet: bool) -> Result<()> {
    if !ricci {
        bail!("enrich: aucune passe demandée — options: --ricci (courbure de Ricci persistée)");
    }
    let db = obrain_substrate::SubstrateStore::open(path)
        .with_context(|| format!("enrich: ouverture du store {}", path.display()))?;

    let mut rapport = Vec::new();
    if ricci {
        if !quiet {
            eprintln!("enrich: refresh Ricci (Jost-Liu) sur toutes les arêtes vivantes…");
        }
        let t0 = std::time::Instant::now();
        let stats = obrain_substrate::geometric::refresh_all_ricci(&db)
            .with_context(|| "enrich: refresh_all_ricci")?;
        // Durabilité : fi ge les zones (WAL déjà journalisé par update_edge).
        db.flush().with_context(|| "enrich: flush")?;
        let ms = t0.elapsed().as_millis() as u64;
        if !quiet {
            eprintln!(
                "enrich: {ms} ms — arêtes visitées {} / mises à jour {} / dangling {} (max|κ| {:.4}, négatives {:.3}, positives {:.3})",
                stats.edges_visited,
                stats.edges_updated,
                stats.edges_skipped_dangling,
                stats.max_abs_curvature,
                stats.negative_fraction,
                stats.positive_fraction
            );
        }
        output::print_result_table(
            &[
                "edges_visited".to_string(),
                "edges_updated".to_string(),
                "edges_skipped_dangling".to_string(),
                "max_abs_curvature".to_string(),
                "negative_fraction".to_string(),
                "positive_fraction".to_string(),
                "elapsed_ms".to_string(),
            ],
            &[vec![
                stats.edges_visited.to_string(),
                stats.edges_updated.to_string(),
                stats.edges_skipped_dangling.to_string(),
                format!("{:.4}", stats.max_abs_curvature),
                format!("{:.3}", stats.negative_fraction),
                format!("{:.3}", stats.positive_fraction),
                ms.to_string(),
            ]],
            format.into(),
            quiet,
        );
        rapport.push(format!("ricci: {ms} ms"));
    }
    Ok(())
}
