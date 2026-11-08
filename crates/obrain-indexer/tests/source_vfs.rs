//! L'indexeur lit-il un arbre virtuel comme un disque ?
//!
//! C'est la promesse de [`obrain_indexer::Source`] : un VFS n'est pas un
//! indexeur à part, c'est **la même indexation** avec une autre façon de lire.
//! Ces tests le vérifient par égalité de graphe, pas par inspection.

use obrain_indexer::Memoire;
use obrain_indexer::discovery::{DiscoveryOptions, discover, discover_depuis};
use obrain_indexer::index::{
    IndexOptions, IndexOutcome, index_repository, index_repository_depuis,
};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

fn ecrire_projet(racine: &Path) {
    fs::create_dir_all(racine.join("src/util")).unwrap();
    fs::write(
        racine.join("src/main.rs"),
        r#"
mod util;
use util::compte::Compte;

fn main() {
    let mut c = Compte::nouveau("a");
    c.verser(10);
    println!("{}", c.solde());
}
"#,
    )
    .unwrap();
    fs::write(racine.join("src/util/mod.rs"), "pub mod compte;\n").unwrap();
    fs::write(
        racine.join("src/util/compte.rs"),
        r#"
pub struct Compte { nom: String, solde: u64 }

impl Compte {
    pub fn nouveau(nom: &str) -> Self {
        Self { nom: nom.to_string(), solde: 0 }
    }
    pub fn verser(&mut self, n: u64) { self.solde += n; }
    pub fn solde(&self) -> u64 { self.solde }
}
"#,
    )
    .unwrap();
    // Un artefact : il doit être écarté des DEUX côtés, sinon le test ne
    // prouve rien sur les exclusions.
    fs::create_dir_all(racine.join("node_modules/x")).unwrap();
    fs::write(racine.join("node_modules/x/i.js"), "module.exports = 1;\n").unwrap();
}

fn qns(out: &IndexOutcome) -> BTreeSet<String> {
    out.graph
        .nodes()
        .iter()
        .map(|n| n.qualified_name.clone())
        .collect()
}

/// Charge un arbre réel **entier** dans une source mémoire — artefacts
/// compris. C'est la seule façon honnête de comparer les exclusions : si la
/// source virtuelle ne contient pas ce qui doit être écarté, elle n'a rien à
/// écarter, et l'égalité ne prouverait rien.
fn charger_tout(dir: &Path, memoire: &mut Memoire) {
    for entree in fs::read_dir(dir).unwrap().flatten() {
        let chemin = entree.path();
        if chemin.is_dir() {
            charger_tout(&chemin, memoire);
        } else {
            memoire.ajouter(chemin.clone(), fs::read(&chemin).unwrap());
        }
    }
}

#[test]
fn la_decouverte_ne_depend_pas_de_la_source() {
    let td = tempfile::tempdir().unwrap();
    let racine = td.path();
    ecrire_projet(racine);

    let disque = discover(racine, &DiscoveryOptions::default());
    let mut memoire = Memoire::new();
    charger_tout(racine, &mut memoire);
    let virtuel = discover_depuis(&memoire, racine, &DiscoveryOptions::default());

    assert_eq!(disque.files, virtuel.files, "mêmes fichiers retenus");
    let mut exclusions_disque: Vec<_> = disque
        .excluded
        .iter()
        .map(|e| (e.chemin.clone(), e.raison))
        .collect();
    let mut exclusions_virtuel: Vec<_> = virtuel
        .excluded
        .iter()
        .map(|e| (e.chemin.clone(), e.raison))
        .collect();
    exclusions_disque.sort();
    exclusions_virtuel.sort();
    assert_eq!(
        exclusions_disque, exclusions_virtuel,
        "mêmes exclusions : artefacts et budget ne dépendent pas de la source"
    );
}

#[test]
fn le_vfs_et_le_disque_donnent_le_meme_graphe() {
    let td = tempfile::tempdir().unwrap();
    let racine = td.path();
    ecrire_projet(racine);

    let opts = IndexOptions::default();
    let sur_disque = index_repository(racine, &opts);

    let fichiers = discover(racine, &DiscoveryOptions::default()).files;
    let mut memoire = Memoire::depuis_dossier(fichiers).unwrap();
    // Entrée supplémentaire, absente du disque : elle doit être écartée par
    // la MÊME règle (artefact), pas par hasard.
    memoire.ajouter(racine.join("node_modules/x/i.js"), "module.exports = 1;\n");

    let en_memoire = index_repository_depuis(&memoire, racine, &opts);

    assert_eq!(
        sur_disque.files_scanned, en_memoire.files_scanned,
        "même nombre de fichiers lus"
    );
    assert_eq!(qns(&sur_disque), qns(&en_memoire), "mêmes nœuds qualifiés");
    assert_eq!(
        sur_disque.graph.edges().len(),
        en_memoire.graph.edges().len(),
        "mêmes arêtes"
    );

    // Le graphe n'est pas vide : sinon l'égalité ne prouverait rien.
    assert!(
        qns(&sur_disque).len() > 5,
        "graphe trop pauvre pour prouver quoi que ce soit : {:?}",
        qns(&sur_disque)
    );
    assert!(!qns(&en_memoire).iter().any(|q| q.contains("node_modules")));
}

#[test]
fn une_source_virtuelle_n_ecrit_aucun_noeud_git() {
    let td = tempfile::tempdir().unwrap();
    let racine = td.path();
    ecrire_projet(racine);

    let memoire =
        Memoire::depuis_dossier(discover(racine, &DiscoveryOptions::default()).files).unwrap();
    let out = index_repository_depuis(&memoire, racine, &IndexOptions::default());

    let branches = out
        .graph
        .nodes()
        .iter()
        .filter(|n| n.kind == obrain_indexer::NodeKind::Branch)
        .count();
    assert_eq!(branches, 0, "un VFS n'a pas d'historique git à tresser");
}
