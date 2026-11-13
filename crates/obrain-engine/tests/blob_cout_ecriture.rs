//! obrain#120 — COÛT D'UNE ÉCRITURE ACQUITTÉE, valeur courte et valeur longue.
//!
//! Mesure, pas un test de conformité : ne fait RIEN sans
//! `OBRAIN_MESURE_BLOB_COUT=<répertoire de sortie>`. Écrit une ligne par série
//! dans `<répertoire>/blob_cout.tsv` : `série  n  p50_us  p90_us  p99_us`.
//!
//! Chaque écriture est un statement auto-commit (un `fsync` WAL acquitté), sur
//! une base fraîche dans un répertoire temporaire. Une valeur courte (< 256 o)
//! prend le chemin scalaire, une longue (585 o) la colonne blob. Lancer la
//! même mesure sur l'arbre sans le correctif et sur l'arbre avec, sur une
//! machine calme (charge < 2), et comparer.
#![cfg(feature = "wal")]

use obrain_engine::ObrainDB;
use std::io::Write as _;
use std::time::Instant;

const ECHAUFFEMENT: usize = 30;
const MESURES: usize = 400;

fn quantile(trie: &[u128], q: f64) -> u128 {
    let i = ((trie.len() as f64 - 1.0) * q).round() as usize;
    trie[i]
}

fn serie(nom: &str, valeur: &dyn Fn(usize) -> String, sortie: &mut std::fs::File) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = ObrainDB::open(dir.path().join("base")).expect("open");
    let s = db.session();
    let mut durees = Vec::with_capacity(MESURES);
    for i in 0..(ECHAUFFEMENT + MESURES) {
        let v = valeur(i);
        let t = Instant::now();
        s.execute(&format!("INSERT (:Mesure {{i: {i}, v: '{v}'}})"))
            .expect("insert acquitté");
        if i >= ECHAUFFEMENT {
            durees.push(t.elapsed().as_micros());
        }
    }
    durees.sort_unstable();
    writeln!(
        sortie,
        "{nom}\t{}\t{}\t{}\t{}",
        durees.len(),
        quantile(&durees, 0.50),
        quantile(&durees, 0.90),
        quantile(&durees, 0.99)
    )
    .unwrap();
}

#[test]
fn mesure_cout_ecriture_courte_et_longue() {
    let Ok(dir) = std::env::var("OBRAIN_MESURE_BLOB_COUT") else {
        return;
    };
    let mut sortie = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(std::path::Path::new(&dir).join("blob_cout.tsv"))
        .expect("fichier de sortie");
    serie("courte_32o", &|i| format!("{i:032}"), &mut sortie);
    serie(
        "longue_585o",
        &|i| format!("{i:08}").repeat(73) + "x",
        &mut sortie,
    );
}
