//! Le dépôt de clés — le seul objet de cette crate qui touche le disque.
//!
//! ## Deux refus, et leur raison
//!
//! 1. **Un fichier de clé lisible par d'autres est refusé**, pas utilisé. Une
//!    clé que le voisin peut lire n'est pas une clé, et continuer donnerait à
//!    l'utilisateur l'illusion d'une garantie tenue. Le remède est nommé dans
//!    l'erreur : le mode du fichier.
//! 2. **Un lien symbolique est refusé.** Lire une clé à travers un lien, c'est
//!    laisser un tiers choisir *quelle* clé est lue. On exige un fichier
//!    ordinaire.
//!
//! ## Ce que le dépôt ne fait pas
//!
//! Il n'écrit **aucune clé en clair** : il ne pose que des [`EnveloppeCle`],
//! illisibles sans la clé maîtresse. Et il n'y a **aucune porte de
//! récupération** : un fichier perdu est perdu. C'est le prix de l'absence de
//! point de défaillance unique, et l'utilisateur doit le savoir avant.

use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::Path;

use crate::cle::{EnveloppeCle, TAILLE_ENVELOPPE_CLE};
use crate::erreur::{CoffreError, Resultat};

/// Écrit une enveloppe de clé, avec les droits les plus stricts possibles.
///
/// Le mode est posé à **0600** (lecture et écriture par le propriétaire seul),
/// y compris si le fichier existait déjà avec des droits plus larges : un
/// fichier réécrit ne doit pas garder les permissions d'avant.
///
/// # Erreurs
///
/// [`CoffreError::Depot`] si l'écriture échoue.
pub fn ecrire_enveloppe(chemin: &Path, enveloppe: &EnveloppeCle) -> Resultat<()> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let chemin_affiche = chemin.display().to_string();
    let mut fichier = options
        .open(chemin)
        .map_err(|e| CoffreError::Depot(format!("{chemin_affiche} : {e}")))?;

    // Le mode de `open` ne s'applique qu'à la création : on le repose, pour le
    // cas où le fichier existait déjà.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let droits = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(chemin, droits)
            .map_err(|e| CoffreError::Depot(format!("{chemin_affiche} : {e}")))?;
    }

    fichier
        .write_all(enveloppe.octets())
        .and_then(|()| fichier.sync_all())
        .map_err(|e| CoffreError::Depot(format!("{chemin_affiche} : {e}")))?;
    Ok(())
}

/// Lit une enveloppe de clé.
///
/// # Erreurs
///
/// * [`CoffreError::CleAbsente`] si le fichier n'existe pas — une
///   **impossibilité**, pas un refus : il n'y a pas de clé, donc rien à
///   déchiffrer, et aucun droit ne le change ;
/// * [`CoffreError::PermissionsTropLarges`] si le fichier est lisible au-delà du
///   propriétaire ;
/// * [`CoffreError::Depot`] si le chemin est un lien symbolique, ou si la
///   lecture échoue ;
/// * [`CoffreError::Format`] si le contenu n'est pas une enveloppe valide.
pub fn lire_enveloppe(chemin: &Path) -> Resultat<EnveloppeCle> {
    let chemin_affiche = chemin.display().to_string();

    let metadonnees = std::fs::symlink_metadata(chemin).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            CoffreError::CleAbsente(format!("{chemin_affiche} n'existe pas"))
        } else {
            CoffreError::Depot(format!("{chemin_affiche} : {e}"))
        }
    })?;

    if metadonnees.file_type().is_symlink() {
        return Err(CoffreError::Depot(format!(
            "{chemin_affiche} est un lien symbolique : la clé lue serait choisie par un tiers"
        )));
    }
    if !metadonnees.is_file() {
        return Err(CoffreError::Depot(format!(
            "{chemin_affiche} n'est pas un fichier ordinaire"
        )));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = metadonnees.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            return Err(CoffreError::PermissionsTropLarges {
                chemin: chemin_affiche,
                mode,
            });
        }
    }

    let mut fichier = std::fs::File::open(chemin)
        .map_err(|e| CoffreError::Depot(format!("{chemin_affiche} : {e}")))?;
    let mut octets = Vec::with_capacity(TAILLE_ENVELOPPE_CLE);
    fichier
        .read_to_end(&mut octets)
        .map_err(|e| CoffreError::Depot(format!("{chemin_affiche} : {e}")))?;

    EnveloppeCle::depuis_octets(&octets)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::cle::{CleEspace, ParametresKdf};

    const PARAMS: ParametresKdf = ParametresKdf {
        m_cost_kio: 8 * 1024,
        t_cost: 2,
        p_cost: 1,
    };

    fn enveloppe() -> EnveloppeCle {
        CleEspace::engendrer("e1")
            .unwrap()
            .envelopper_avec_phrase("phrase", PARAMS)
            .unwrap()
    }

    /// Le tour complet : écrire, relire, desceller.
    #[test]
    fn ecriture_et_lecture() {
        let dossier = tempfile::tempdir().unwrap();
        let chemin = dossier.path().join("e1.cle");
        let posee = enveloppe();

        ecrire_enveloppe(&chemin, &posee).unwrap();
        let relue = lire_enveloppe(&chemin).unwrap();
        assert_eq!(relue, posee);

        let kek = relue.deriver_kek("phrase").unwrap();
        assert!(CleEspace::desceller(&relue, &kek).is_ok());
    }

    /// Le fichier est posé en **0600** : personne d'autre ne peut le lire.
    #[cfg(unix)]
    #[test]
    fn fichier_en_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dossier = tempfile::tempdir().unwrap();
        let chemin = dossier.path().join("e1.cle");
        ecrire_enveloppe(&chemin, &enveloppe()).unwrap();

        let mode = std::fs::metadata(&chemin).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "mode constaté : {mode:o}");
    }

    /// Le mode est reposé même si le fichier existait avec des droits larges.
    #[cfg(unix)]
    #[test]
    fn droits_larges_corriges_a_la_reecriture() {
        use std::os::unix::fs::PermissionsExt;
        let dossier = tempfile::tempdir().unwrap();
        let chemin = dossier.path().join("e1.cle");
        std::fs::write(&chemin, b"peu importe").unwrap();
        std::fs::set_permissions(&chemin, std::fs::Permissions::from_mode(0o644)).unwrap();

        ecrire_enveloppe(&chemin, &enveloppe()).unwrap();
        let mode = std::fs::metadata(&chemin).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    /// **Un fichier de clé lisible par d'autres est refusé**, avec son mode.
    #[cfg(unix)]
    #[test]
    fn fichier_trop_permissif_refuse() {
        use std::os::unix::fs::PermissionsExt;
        let dossier = tempfile::tempdir().unwrap();
        let chemin = dossier.path().join("e1.cle");
        ecrire_enveloppe(&chemin, &enveloppe()).unwrap();
        std::fs::set_permissions(&chemin, std::fs::Permissions::from_mode(0o644)).unwrap();

        match lire_enveloppe(&chemin) {
            Err(CoffreError::PermissionsTropLarges { mode, .. }) => assert_eq!(mode, 0o644),
            autre => panic!("attendu PermissionsTropLarges, reçu {autre:?}"),
        }
    }

    /// **Un lien symbolique est refusé** : la clé lue ne doit pas être choisie
    /// par un tiers.
    #[cfg(unix)]
    #[test]
    fn lien_symbolique_refuse() {
        let dossier = tempfile::tempdir().unwrap();
        let vraie = dossier.path().join("vraie.cle");
        let lien = dossier.path().join("lien.cle");
        ecrire_enveloppe(&vraie, &enveloppe()).unwrap();
        std::os::unix::fs::symlink(&vraie, &lien).unwrap();

        assert!(matches!(lire_enveloppe(&lien), Err(CoffreError::Depot(_))));
        assert!(lire_enveloppe(&vraie).is_ok(), "le fichier ordinaire, lui, passe");
    }

    /// **Un fichier absent est une impossibilité, pas un refus.** La distinction
    /// est portée par le type d'erreur, pas par un message.
    #[test]
    fn fichier_absent_est_une_impossibilite() {
        let dossier = tempfile::tempdir().unwrap();
        let absent = dossier.path().join("jamais-ecrit.cle");

        match lire_enveloppe(&absent) {
            Err(erreur @ CoffreError::CleAbsente(_)) => {
                assert!(erreur.est_une_impossibilite());
            }
            autre => panic!("attendu CleAbsente, reçu {autre:?}"),
        }
    }

    /// Un contenu qui n'est pas une enveloppe est refusé.
    #[test]
    fn contenu_invalide_refuse() {
        let dossier = tempfile::tempdir().unwrap();
        let chemin = dossier.path().join("faux.cle");
        ecrire_enveloppe(&chemin, &enveloppe()).unwrap();

        for mauvais in [vec![0u8; 10], vec![0u8; TAILLE_ENVELOPPE_CLE]] {
            ecrire_enveloppe(&chemin, &enveloppe()).unwrap();
            std::fs::write(&chemin, &mauvais).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&chemin, std::fs::Permissions::from_mode(0o600)).unwrap();
            }
            assert!(matches!(
                lire_enveloppe(&chemin),
                Err(CoffreError::Format(_))
            ));
        }
    }
}
