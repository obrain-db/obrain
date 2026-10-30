//! Database compaction command.

use std::path::Path;

use anyhow::Result;
use serde::Serialize;

use crate::OutputFormat;
use crate::output::{self, Format, format_bytes};

/// Compaction result output.
#[derive(Serialize)]
struct CompactionOutput {
    dry_run: bool,
    before_size_bytes: usize,
    after_size_bytes: Option<usize>,
    wal_bytes_pruned: Option<usize>,
}

/// Run the compact command.
pub fn run(
    path: &Path,
    dry_run: bool,
    prune_wal: bool,
    format: OutputFormat,
    quiet: bool,
) -> Result<()> {
    let db = super::open_existing(path)?;
    let stats_before = db.detailed_stats();

    if dry_run {
        output::status("Dry run, no changes will be made", quiet);

        let result = CompactionOutput {
            dry_run: true,
            before_size_bytes: stats_before.memory_bytes,
            after_size_bytes: None,
            wal_bytes_pruned: None,
        };

        let fmt: Format = format.into();
        match fmt {
            Format::Json => {
                if !quiet {
                    println!("{}", serde_json::to_string_pretty(&result)?);
                }
            }
            Format::Table | Format::Csv => {
                let items = vec![
                    ("Mode", "Dry Run".to_string()),
                    ("Current Size", format_bytes(stats_before.memory_bytes)),
                    ("Nodes", stats_before.node_count.to_string()),
                    ("Edges", stats_before.edge_count.to_string()),
                ];
                output::print_key_value_table(&items, fmt, quiet);
            }
        }
    } else {
        output::status("Compacting database...", quiet);
        db.gc();

        // (2026-08-31) COMPACT DOIT AUSSI CHECKPOINTER LE WAL SUBSTRATE.
        //
        // `ObrainDB::checkpoint()` (engine) ne fait que `flush()` — il ne
        // TRONQUE jamais le WAL. La troncature vit dans
        // `SubstrateStore::checkpoint()` (flush + `checkpoint::checkpoint`
        // qui appelle `wal.truncate()` + re-ancrage des index), mais elle
        // n'était appelée par AUCUN chemin de production : le seul
        // déclencheur était le seuil auto de 512 Mio, jamais atteint sur des
        // bases importées. Résultat mesuré (bench 2026-08-31) : `substrate.wal`
        // = 17 Mo (aletheia) / 122 Mo (harness) rejoués à CHAQUE open, soit
        // open = 55-520 ms — le seul point où obrain perdait vs cbm (0,2 ms).
        //
        // `compact` est le chemin d'opérations : il appelle désormais le
        // checkpoint substrate complet. C'est sûr : les données sont déjà
        // durables (le WAL n'est tronqué qu'après flush + fsync des zones),
        // et idempotent (checkpoint sur WAL vide = no-op + re-ancrage).
        if let Some(sub) = db.substrate_handle() {
            match sub.checkpoint() {
                Ok(stats) => output::status(
                    &format!(
                        "WAL checkpointed (tronqué à 0, {} octets de WAL avant)",
                        stats.wal_offset_before
                    ),
                    quiet,
                ),
                Err(e) => {
                    output::status(&format!("Warning: WAL checkpoint failed: {e}"), false);
                }
            }
            // (2026-08-31) SHRINK DE LA ZONE v2 — la zone `substrate.props.v2`
            // est pré-allouée par croissance exponentielle ×1.5 : mesuré
            // 167 Mo sur disque pour 26 Mo réellement alloués (aletheia),
            // 849 Mo pour ~120 Mo (harness). Le padding au-delà de la
            // dernière page live est zéro-init et jamais adressé ; le
            // tronquer récupère le disque et raccourcit le msync/fsync au
            // prochain checkpoint/close. Sûr ici : le store est tenu
            // exclusivement par `compact`, et les ancres (page_idx) restent
            // dans l'étendue tronquée. `shrink_to_fit` est un no-op si la
            // zone est déjà à sa taille utile.
            if let Err(e) = sub.shrink_props_zone() {
                output::status(
                    &format!("Warning: props zone shrink failed (non fatal): {e}"),
                    false,
                );
            }
        }

        // T17 final cutover: the legacy tiered-storage epoch-file path
        // (`ObrainDB::compact()`) was retired; substrate handles its own
        // on-disk layout natively, so no epoch files are written here.

        // Prune old WAL files if requested
        let wal_bytes_pruned = if prune_wal {
            output::status("Pruning old WAL files...", quiet);
            match db.prune_wal() {
                Ok(freed) => {
                    if freed > 0 {
                        output::status(
                            &format!("Pruned WAL files, freed {}", format_bytes(freed)),
                            quiet,
                        );
                    } else {
                        output::status("No WAL files to prune", quiet);
                    }
                    Some(freed)
                }
                Err(e) => {
                    output::status(&format!("Warning: WAL prune failed: {e}"), false);
                    None
                }
            }
        } else {
            None
        };

        let stats_after = db.detailed_stats();

        let result = CompactionOutput {
            dry_run: false,
            before_size_bytes: stats_before.memory_bytes,
            after_size_bytes: Some(stats_after.memory_bytes),
            wal_bytes_pruned,
        };

        let fmt: Format = format.into();
        match fmt {
            Format::Json => {
                if !quiet {
                    println!("{}", serde_json::to_string_pretty(&result)?);
                }
            }
            Format::Table | Format::Csv => {
                let reclaimed = stats_before
                    .memory_bytes
                    .saturating_sub(stats_after.memory_bytes);
                let mut items = vec![
                    ("Before", format_bytes(stats_before.memory_bytes)),
                    ("After", format_bytes(stats_after.memory_bytes)),
                    ("Reclaimed", format_bytes(reclaimed)),
                ];
                if let Some(wal_freed) = wal_bytes_pruned {
                    items.push(("WAL Pruned", format_bytes(wal_freed)));
                }
                output::print_key_value_table(&items, fmt, quiet);
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_db(dir: &std::path::Path) {
        let db = obrain_engine::ObrainDB::open(dir).expect("create db");
        let n1 = db.create_node(&["Person"]);
        let n2 = db.create_node(&["Person"]);
        db.set_node_property(n1, "name", obrain_common::types::Value::from("Alix"));
        db.set_node_property(n2, "name", obrain_common::types::Value::from("Gus"));
        db.create_edge(n1, n2, "KNOWS");
        db.close().expect("close db");
    }

    #[test]
    fn test_compact_dry_run() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.obrain");
        create_test_db(&db_path);

        // Dry run should succeed without modifying the database
        run(&db_path, true, false, OutputFormat::Table, true).expect("dry run should succeed");
    }

    #[test]
    fn test_compact_actual() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.obrain");
        create_test_db(&db_path);

        // Actual compaction
        run(&db_path, false, false, OutputFormat::Table, true).expect("compaction should succeed");
    }

    #[test]
    fn test_compact_json_format() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("test.obrain");
        create_test_db(&db_path);

        run(&db_path, true, false, OutputFormat::Json, true)
            .expect("json format dry run should succeed");
        run(&db_path, false, false, OutputFormat::Json, true)
            .expect("json format compaction should succeed");
    }

    #[test]
    fn test_compact_nonexistent_database() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("nonexistent.obrain");

        let result = run(&db_path, true, false, OutputFormat::Table, true);
        assert!(result.is_err());
    }
}
