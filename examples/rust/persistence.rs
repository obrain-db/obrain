//! Persistence: WAL-backed storage.
//!
//! Run with: `cargo run -p obrain-examples --bin persistence --features storage`
//!
//! Note: the legacy byte-array snapshot export/import API
//! (`export_snapshot` / `import_snapshot`) was retired with the T17
//! substrate cutover. For backups, copy the database directory
//! (`cp -r`, `tar`, `rsync`) while the database is closed.

use obrain::ObrainDB;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ── WAL-backed persistence ────────────────────────────────────
    // Create a persistent database that writes to disk via WAL
    // (write-ahead log). Data survives process restarts.
    let temp_dir = std::env::temp_dir().join("obrain_persistence_example");

    // Clean up from any previous run
    if temp_dir.exists() {
        std::fs::remove_dir_all(&temp_dir)?;
    }

    println!("Creating persistent database at: {}", temp_dir.display());

    // Open creates the directory and WAL files automatically
    let db = ObrainDB::open(&temp_dir)?;
    let session = db.session();

    // Insert some data
    session.execute("INSERT (:Person {name: 'Alix', city: 'Utrecht'})")?;
    session.execute("INSERT (:Person {name: 'Gus', city: 'Leiden'})")?;
    session.execute(
        "MATCH (a:Person {name: 'Alix'}), (b:Person {name: 'Gus'})
         INSERT (a)-[:KNOWS]->(b)",
    )?;

    let count: i64 = session
        .execute("MATCH (p:Person) RETURN COUNT(p)")?
        .scalar()?;
    println!("Inserted {count} people");

    // Close the database (flushes WAL)
    db.close()?;
    println!("Database closed\n");

    // Reopen the same path: data should still be there
    let db2 = ObrainDB::open(&temp_dir)?;
    let session2 = db2.session();

    let count: i64 = session2
        .execute("MATCH (p:Person) RETURN COUNT(p)")?
        .scalar()?;
    println!("Reopened database: {count} people found");

    let result = session2.execute(
        "MATCH (p:Person)
         RETURN p.name, p.city
         ORDER BY p.name",
    )?;
    for row in result.iter() {
        let name = row[0].as_str().unwrap_or("?");
        let city = row[1].as_str().unwrap_or("?");
        println!("  {} ({})", name, city);
    }

    db2.close()?;

    // Clean up the temp directory
    if temp_dir.exists() {
        std::fs::remove_dir_all(&temp_dir)?;
    }
    println!("\nCleaned up temp directory");

    println!("\nDone!");
    Ok(())
}
