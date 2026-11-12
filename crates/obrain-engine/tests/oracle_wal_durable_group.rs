//! Verrou : l'oracle de durabilité n'est pas muet en mode `SyncMode::Group`.
//!
//! Issue obrain#120 point 6 : « `wal_octets_durables` reste à 0 après
//! `flush_wal` ; seul `wal_fsyncs` bouge ». Après `flush_wal()`, la ligne
//! durable doit couvrir la fin de `substrate.wal`, qu'il y ait ou non des
//! frames en attente dans le tampon du group commit.
#![cfg(feature = "wal")]

use obrain_engine::ObrainDB;
use obrain_substrate::wal_io::SyncMode;

fn taille_wal(base: &std::path::Path) -> u64 {
    std::fs::metadata(base.join("substrate.wal")).unwrap().len()
}

#[test]
fn flush_wal_en_mode_group_avance_la_ligne_durable() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("kb");
    let db = ObrainDB::open_substrate_with_mode(&base, SyncMode::Group).unwrap();
    let s = db.session();
    for i in 0..30 {
        s.execute(&format!("INSERT (:Durable {{k: {i}}})")).unwrap();
    }
    let fsyncs_avant = db.wal_fsyncs().unwrap();

    db.flush_wal().unwrap();
    let durable = db.wal_octets_durables().unwrap();
    let fin = taille_wal(&base);
    eprintln!(
        "MESURE group: durable={durable} fin_wal={fin} fsyncs={}->{}",
        fsyncs_avant,
        db.wal_fsyncs().unwrap()
    );
    assert!(fin > 0, "des commits ont dû écrire dans le WAL");
    assert!(
        db.wal_fsyncs().unwrap() > fsyncs_avant,
        "flush_wal doit fsyncer"
    );
    assert!(
        durable > 0,
        "oracle muet : durable=0 après flush_wal (fin_wal={fin})"
    );
    assert!(durable >= fin, "ligne durable {durable} < fin du WAL {fin}");

    // Tampon vide : un second flush_wal ne doit pas faire reculer la ligne.
    db.flush_wal().unwrap();
    assert!(db.wal_octets_durables().unwrap() >= durable);
}

#[test]
fn flush_wal_en_every_commit_ligne_durable_couvre_le_wal() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("kb");
    let db = ObrainDB::open_substrate_with_mode(&base, SyncMode::EveryCommit).unwrap();
    let s = db.session();
    for i in 0..30 {
        s.execute(&format!("INSERT (:Durable {{k: {i}}})")).unwrap();
    }
    db.flush_wal().unwrap();
    let durable = db.wal_octets_durables().unwrap();
    let fin = taille_wal(&base);
    eprintln!("MESURE every: durable={durable} fin_wal={fin}");
    assert!(durable > 0 && durable >= fin);
}
