//! T27 — le cache d'extraction ne doit JAMAIS changer le graphe produit.
//! Chaque graphe indexé avec cache est comparé à une indexation SANS cache.

use obrain_indexer::index::{IndexOptions, index_repository};
use std::path::Path;

fn ecrire(racine: &Path, rel: &str, contenu: &str) {
    let p = racine.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, contenu).unwrap();
}

fn depot(racine: &Path) {
    ecrire(racine, "src/a.c", "int aide(int x) { for (int i=0;i<x;i++) {} return x; }\nint appel(void) { return aide(3); }\n");
    ecrire(racine, "src/b.c", "struct point { int x; int y; };\nint b(void) { return appel(); }\n");
    ecrire(racine, "lib/c.rs", "pub fn helper() -> u32 { 1 }\npub struct P {}\nimpl P { pub fn run(&self) -> u32 { helper() } }\n");
}

/// Signature complète et ordonnée : nœuds (type, FQN, propriétés) et arêtes
/// (type, source, cible).
fn signature(opts: &IndexOptions, racine: &Path) -> (Vec<String>, usize, usize) {
    let r = index_repository(racine, opts);
    let g = &r.graph;
    let mut lignes: Vec<String> = g
        .nodes()
        .iter()
        .map(|n| format!("N {:?} {} {}", n.kind, n.qualified_name, serde_json::to_value(&n.props).unwrap()))
        .collect();
    for e in g.edges() {
        let qn = |id| g.node(id).map(|n| n.qualified_name.clone()).unwrap_or_default();
        lignes.push(format!("E {:?} {} -> {}", e.kind, qn(e.source), qn(e.target)));
    }
    lignes.sort();
    (lignes, r.files_scanned, r.fichiers_depuis_cache)
}

fn sans_cache() -> IndexOptions {
    IndexOptions::default()
}

fn avec_cache(dossier: &Path) -> IndexOptions {
    IndexOptions { cache_dir: Some(dossier.to_path_buf()), ..Default::default() }
}

#[test]
fn la_seconde_indexation_vient_du_cache_et_rend_le_meme_graphe() {
    let (d, c) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    depot(d.path());
    let reference = signature(&sans_cache(), d.path());
    let premiere = signature(&avec_cache(c.path()), d.path());
    assert_eq!(premiere.2, 0, "premier passage : rien en cache");
    let seconde = signature(&avec_cache(c.path()), d.path());
    assert_eq!(seconde.2, seconde.1, "second passage : TOUT vient du cache");
    assert!(seconde.1 >= 3);
    assert_eq!(premiere.0, reference.0, "le premier passage ne change rien");
    assert_eq!(seconde.0, reference.0, "le passage servi par le cache ne change rien");
}

#[test]
fn un_fichier_modifie_est_reanalyse() {
    let (d, c) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    depot(d.path());
    signature(&avec_cache(c.path()), d.path());
    ecrire(d.path(), "src/b.c", "int b(void) { return aide(1); }\nint nouvelle(void) { return 0; }\n");
    let reference = signature(&sans_cache(), d.path());
    let apres = signature(&avec_cache(c.path()), d.path());
    assert_eq!(apres.2, apres.1 - 1, "seul le fichier modifié est réanalysé");
    assert_eq!(apres.0, reference.0, "le graphe reflète la modification");
    assert!(apres.0.iter().any(|l| l.contains(".nouvelle ")), "la nouvelle fonction est là");
}

#[test]
fn un_fichier_supprime_disparait_et_le_cache_suit() {
    let (d, c) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    depot(d.path());
    signature(&avec_cache(c.path()), d.path());
    std::fs::remove_file(d.path().join("src/a.c")).unwrap();
    let reference = signature(&sans_cache(), d.path());
    let apres = signature(&avec_cache(c.path()), d.path());
    assert_eq!(apres.0, reference.0);
    assert_eq!(apres.2, apres.1, "les fichiers restants viennent du cache");
}

#[test]
fn un_cache_corrompu_est_ignore() {
    let (d, c) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    depot(d.path());
    signature(&avec_cache(c.path()), d.path());
    for f in std::fs::read_dir(c.path()).unwrap() {
        let p = f.unwrap().path();
        let mut octets = std::fs::read(&p).unwrap();
        let n = octets.len();
        octets.truncate(n - 7); // entrée tronquée
        std::fs::write(&p, octets).unwrap();
    }
    let reference = signature(&sans_cache(), d.path());
    let apres = signature(&avec_cache(c.path()), d.path());
    assert_eq!(apres.2, 0, "cache illisible : tout est réanalysé");
    assert_eq!(apres.0, reference.0);
}

#[test]
fn sans_dossier_aucun_cache_n_est_ecrit() {
    let d = tempfile::tempdir().unwrap();
    depot(d.path());
    if std::env::var("OBRAIN_INDEX_CACHE_DIR").is_ok() {
        return; // environnement qui l'active : ce test ne peut rien prouver
    }
    let a = signature(&sans_cache(), d.path());
    let b = signature(&sans_cache(), d.path());
    assert_eq!((a.2, b.2), (0, 0), "aucun cache sans dossier désigné");
}
