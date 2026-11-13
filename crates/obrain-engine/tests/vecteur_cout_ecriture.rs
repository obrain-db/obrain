//! obrain#120 — COÛT D'UNE ÉCRITURE VECTORIELLE ACQUITTÉE, avant / après la
//! trame `VecColumnSet` (le vecteur au WAL).
//!
//! Mesure, pas un test de conformité : ne fait RIEN sans
//! `OBRAIN_MESURE_VEC_COUT=<répertoire de sortie>`. Écrit une ligne par série
//! dans `<répertoire>/vec_cout.tsv` :
//! `série  n  p50_us  p90_us  p99_us  octets_wal_par_ecriture`.
//!
//! Chaque écriture est un statement auto-commit (un `fsync` WAL acquitté) qui
//! crée un nœud portant un vecteur, sur une base fraîche dans un répertoire
//! temporaire. Les octets WAL par écriture sont la croissance de
//! `substrate.wal` sur les écritures mesurées, divisée par leur nombre (le
//! nœud, ses propriétés scalaires et les trames de transaction compris : c'est
//! l'ÉCART avant / après qui mesure la trame). Lancer la même mesure sur
//! l'arbre sans le correctif et sur l'arbre avec, en notant la charge
//! (`sysctl -n vm.loadavg`), et comparer.
#![cfg(feature = "wal")]

use obrain_engine::ObrainDB;
use std::io::Write as _;
use std::path::Path;
use std::time::Instant;

const ECHAUFFEMENT: usize = 30;
const MESURES: usize = 400;

fn quantile(trie: &[u128], q: f64) -> u128 {
    let i = ((trie.len() as f64 - 1.0) * q).round() as usize;
    trie[i]
}

/// Taille cumulée des `substrate.wal` sous `racine` (sous-dossiers compris).
fn taille_wal(racine: &Path) -> u64 {
    let mut total = 0;
    if let Ok(entrees) = std::fs::read_dir(racine) {
        for e in entrees.flatten() {
            let chemin = e.path();
            if chemin.is_dir() {
                total += taille_wal(&chemin);
            } else if chemin.file_name().is_some_and(|n| n == "substrate.wal") {
                total += e.metadata().map_or(0, |m| m.len());
            }
        }
    }
    total
}

fn serie(dim: usize, sortie: &mut std::fs::File) {
    let dir = tempfile::tempdir().expect("tempdir");
    let base = dir.path().join("base");
    let db = ObrainDB::open(&base).expect("open");
    let s = db.session();
    let mut durees = Vec::with_capacity(MESURES);
    let mut wal_debut = 0;
    for i in 0..(ECHAUFFEMENT + MESURES) {
        let v: Vec<String> = (0..dim)
            .map(|j| format!("{:.1}", (i * 7 + j) as f32 + 0.5))
            .collect();
        let requete = format!(
            "INSERT (:Mesure {{i: {i}, emb: vector([{}])}})",
            v.join(", ")
        );
        if i == ECHAUFFEMENT {
            wal_debut = taille_wal(&base);
        }
        let t = Instant::now();
        s.execute(&requete).expect("insert acquitté");
        if i >= ECHAUFFEMENT {
            durees.push(t.elapsed().as_micros());
        }
    }
    let octets = taille_wal(&base).saturating_sub(wal_debut) / MESURES as u64;
    durees.sort_unstable();
    writeln!(
        sortie,
        "vecteur_dim{dim}\t{}\t{}\t{}\t{}\t{octets}",
        durees.len(),
        quantile(&durees, 0.50),
        quantile(&durees, 0.90),
        quantile(&durees, 0.99)
    )
    .unwrap();
}

#[test]
fn mesure_cout_ecriture_vectorielle() {
    let Ok(dir) = std::env::var("OBRAIN_MESURE_VEC_COUT") else {
        return;
    };
    let mut sortie = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(Path::new(&dir).join("vec_cout.tsv"))
        .expect("fichier de sortie");
    for dim in [16, 384] {
        serie(dim, &mut sortie);
    }
}
