//! Bail d'écriture de la bibliothèque : UN SEUL écrivain par base et par
//! processus (décision D4, 353636bc).
//!
//! Pourquoi ici et pas dans le moteur : `ObrainDB::open_shared_write` admet
//! sciemment plusieurs écrivains sur la même base, dans un même processus
//! comme entre processus (tests `writer_node_count_partage`,
//! `vector_index_deux_processus`, `ecriture_multi_processus` — MP-6/MP-8).
//! Le contrat « un seul écrivain » promis aux consommateurs de l'ABI est donc
//! tenu par cette bibliothèque : un registre des chemins canoniques ouverts
//! en écriture, pris par `obrain_open_write`, rendu par `obrain_close` ou
//! `obrain_free_database`.
//!
//! Portée : le registre est propre au processus. Entre processus, c'est le
//! verrou partagé du substrat qui s'applique (plusieurs écrivains partagés
//! coexistent avec les lecteurs ; voir README.md, « Bail »).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

static BAUX: LazyLock<Mutex<HashSet<PathBuf>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// Prend le bail d'écriture sur `chemin` (qui doit exister : il est
/// canonicalisé pour que `./base` et `/abs/base` désignent le même bail).
///
/// Rend le chemin canonique à conserver pour [`rendre`], ou un message si un
/// autre handle de ce processus tient déjà le bail.
pub(crate) fn prendre(chemin: &Path) -> Result<PathBuf, String> {
    let canonique = chemin
        .canonicalize()
        .map_err(|e| format!("bail d'écriture : chemin {} : {e}", chemin.display()))?;
    let mut baux = BAUX
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if baux.insert(canonique.clone()) {
        Ok(canonique)
    } else {
        Err(format!(
            "bail d'écriture déjà tenu dans ce processus pour {} : un seul obrain_open_write \
             par base ; fermez (obrain_close) ou libérez (obrain_free_database) le premier \
             handle, ou ouvrez le second en lecture seule (obrain_open)",
            canonique.display()
        ))
    }
}

/// Rend un bail pris par [`prendre`]. Sans effet s'il n'est plus tenu.
pub(crate) fn rendre(canonique: &Path) {
    let mut baux = BAUX
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    baux.remove(canonique);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn un_bail_par_chemin_canonique_puis_rendu() {
        let dir = tempfile::tempdir().expect("tempdir");
        let base = dir.path().join("base");
        std::fs::create_dir(&base).expect("mkdir");
        // Le même répertoire par deux écritures différentes du chemin.
        let relatif = base.join("..").join("base");

        let tenu = prendre(&base).expect("premier bail");
        let refus = prendre(&relatif).expect_err("second bail refusé");
        assert!(refus.contains("déjà tenu"), "{refus}");

        rendre(&tenu);
        let repris = prendre(&relatif).expect("bail repris après libération");
        rendre(&repris);
    }

    #[test]
    fn chemin_inexistant_refuse() {
        let dir = tempfile::tempdir().expect("tempdir");
        let erreur = prendre(&dir.path().join("absente")).expect_err("inexistant");
        assert!(erreur.contains("bail d'écriture"), "{erreur}");
    }
}
