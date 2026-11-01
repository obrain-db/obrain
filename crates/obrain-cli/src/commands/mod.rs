//! CLI command implementations.

use std::path::Path;

use anyhow::{Context, Result};
use obrain_engine::ObrainDB;

pub mod backup;
pub mod compact;
pub mod compare;
pub mod continuity;
pub mod data;
pub mod enrich;
pub mod entretien_cmd;
pub mod iam;
pub mod import_journal;
pub mod index;
pub mod info;
pub mod init;
pub mod journal;
pub mod jump;
pub mod machines;
pub mod memoire;
pub mod native_import;
pub mod query;
pub mod resonance;
pub mod repair;
pub mod schema;
pub mod stats;
pub mod topo;
pub mod validate;
pub mod version;
pub mod viz;
pub mod wal;

/// Open an existing database, returning a clear error if the path does not exist.
///
/// Use this instead of `ObrainDB::open` for commands that expect a pre-existing database.
/// The `init` command should use `ObrainDB::open` directly since it intentionally creates.
pub fn open_existing(path: &Path) -> Result<ObrainDB> {
    if !path.exists() {
        anyhow::bail!(
            "Database not found: {}\n\
             Use `obrain init {}` to create a new database.",
            path.display(),
            path.display()
        );
    }
    ObrainDB::open(path).with_context(|| format!("Failed to open database at {}", path.display()))
}
