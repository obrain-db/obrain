//! Migration tool: v1 .obrain → v2 (mmap-native) — RETIRED.
//!
//! The legacy single-file .obrain format (v1 bincode snapshot, v2 mmap
//! native) was decommissioned with the T17 substrate cutover, along with
//! the `ObrainDB::save()` API this tool relied on. Substrate is now the
//! single, directory-based storage backend.
//!
//! To migrate a pre-substrate database, use `obrain-migrate` (v0.0.1 or
//! earlier) to convert to the directory-based layout first, then open it
//! with a current release to complete the substrate conversion.

fn main() {
    eprintln!("migrate-v2 has been retired: the single-file v1/v2 .obrain formats");
    eprintln!("were decommissioned with the T17 substrate cutover.");
    eprintln!();
    eprintln!("Use `obrain-migrate` (<= v0.0.1) to convert legacy databases to the");
    eprintln!("directory-based layout, then open with a current release.");
    std::process::exit(1);
}
