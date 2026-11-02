//! La configuration TLS — et le fait qu'elle soit **obligatoire**.
//!
//! [`ConfigTls`] n'a pas de constructeur qui réussisse sans certificat **et**
//! clé. Il n'existe pas de `ConfigTls::par_defaut()`, pas de `Option<ConfigTls>`
//! dans [`crate::EcouteTls`], pas de méthode `sans_tls()`. Un serveur qui veut
//! servir en clair ne peut donc pas le faire *par accident* : il doit écrire
//! `TcpListener::bind` à la main, et cela se voit dans une revue.
//!
//! C'est le seul moyen connu de rendre l'étape 0 tenable : un repli en clair
//! ajouté « pour le développement » est exactement ce qui se retrouve en
//! production deux ans plus tard.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustls::ServerConfig;
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

use crate::erreur::{ErreurTls, Resultat};

/// La configuration d'un serveur TLS.
///
/// Le `Debug` n'imprime **pas** la clé privée : il n'imprime que le nombre de
/// certificats de la chaîne et l'origine. Une clé privée qui se retrouve dans un
/// journal de démarrage est une clé privée perdue.
#[derive(Clone)]
pub struct ConfigTls {
    interieur: Arc<ServerConfig>,
    nb_certificats: usize,
    origine: Option<PathBuf>,
}

impl ConfigTls {
    /// Charge la chaîne de certificats et la clé privée depuis deux fichiers
    /// PEM.
    ///
    /// # Erreurs
    ///
    /// * [`ErreurTls::CertificatAbsent`] ou [`ErreurTls::CleAbsente`] si un
    ///   fichier est illisible — jamais un repli ;
    /// * [`ErreurTls::AucunCertificat`] / [`ErreurTls::AucuneCle`] si le PEM ne
    ///   porte pas ce qu'on y cherche ;
    /// * [`ErreurTls::CleEtCertificatIncoherents`] si la clé ne correspond pas
    ///   au certificat — refusé **au démarrage**.
    pub fn depuis_fichiers(chemin_certificat: &Path, chemin_cle: &Path) -> Resultat<Self> {
        let certificat = fs::read(chemin_certificat).map_err(|e| ErreurTls::CertificatAbsent {
            chemin: chemin_certificat.display().to_string(),
            detail: e.to_string(),
        })?;
        let cle = fs::read(chemin_cle).map_err(|e| ErreurTls::CleAbsente {
            chemin: chemin_cle.display().to_string(),
            detail: e.to_string(),
        })?;

        let mut config = Self::depuis_pem(&certificat, &cle)?;
        config.origine = Some(chemin_certificat.to_path_buf());
        Ok(config)
    }

    /// Comme [`ConfigTls::depuis_fichiers`], depuis des octets déjà en mémoire.
    ///
    /// # Erreurs
    ///
    /// Les mêmes, sans les erreurs de lecture de fichier.
    pub fn depuis_pem(certificat: &[u8], cle: &[u8]) -> Resultat<Self> {
        let chaine = lire_certificats(certificat)?;
        let nb_certificats = chaine.len();
        let cle_privee = lire_cle(cle)?;

        let interieur = ServerConfig::builder_with_provider(fournisseur())
            .with_safe_default_protocol_versions()
            .map_err(|e| ErreurTls::ConfigurationRefusee(e.to_string()))?
            .with_no_client_auth()
            .with_single_cert(chaine, cle_privee)
            .map_err(|e| ErreurTls::CleEtCertificatIncoherents(e.to_string()))?;

        Ok(Self {
            interieur: Arc::new(interieur),
            nb_certificats,
            origine: None,
        })
    }

    /// Le nombre de certificats de la chaîne présentée.
    #[must_use]
    pub fn nb_certificats(&self) -> usize {
        self.nb_certificats
    }

    /// Le chemin du certificat, si la configuration vient d'un fichier.
    #[must_use]
    pub fn origine(&self) -> Option<&Path> {
        self.origine.as_deref()
    }

    pub(crate) fn interieur(&self) -> Arc<ServerConfig> {
        Arc::clone(&self.interieur)
    }
}

impl core::fmt::Debug for ConfigTls {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ConfigTls")
            .field("certificats", &self.nb_certificats)
            .field("origine", &self.origine)
            .field("cle_privee", &"•••")
            .finish()
    }
}

/// Le fournisseur cryptographique employé.
///
/// `ring`, choisi pour la légèreté du build (ni `cmake` ni `nasm`). **Ce que
/// cela coûte**, et qu'il faut savoir : `ring` ne propose pas encore d'échange
/// de clés post-quantique, là où `aws-lc-rs` offre `X25519MLKEM768`. Passer à
/// `aws-lc-rs` se fait en changeant la feature du manifeste, sans toucher à une
/// ligne de cette crate — la contrepartie est un build qui réclame une
/// chaîne de compilation C.
#[must_use]
pub fn fournisseur() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn lire_certificats(pem: &[u8]) -> Resultat<Vec<CertificateDer<'static>>> {
    let mut lecteur = std::io::BufReader::new(pem);
    let collecte: core::result::Result<Vec<_>, _> = rustls_pemfile::certs(&mut lecteur).collect();
    let chaine = collecte.map_err(|e| ErreurTls::PemIllisible {
        quoi: "chaîne de certificats",
        detail: e.to_string(),
    })?;
    if chaine.is_empty() {
        return Err(ErreurTls::AucunCertificat);
    }
    Ok(chaine)
}

fn lire_cle(pem: &[u8]) -> Resultat<PrivateKeyDer<'static>> {
    let mut lecteur = std::io::BufReader::new(pem);
    rustls_pemfile::private_key(&mut lecteur)
        .map_err(|e| ErreurTls::PemIllisible {
            quoi: "clé privée",
            detail: e.to_string(),
        })?
        .ok_or(ErreurTls::AucuneCle)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const CERTIFICAT: &[u8] = include_bytes!("../tests/donnees/cert-de-test.pem");
    const CLE: &[u8] = include_bytes!("../tests/donnees/cle-de-test.pem");
    const AUTRE_CLE: &[u8] = include_bytes!("../tests/donnees/autre-cle-de-test.pem");

    #[test]
    fn une_configuration_valide_se_construit() {
        let config = ConfigTls::depuis_pem(CERTIFICAT, CLE).unwrap();
        assert_eq!(config.nb_certificats(), 1);
        assert!(config.origine().is_none());
    }

    /// Le `Debug` ne doit jamais rendre la clé privée : un journal de démarrage
    /// n'est pas un coffre.
    #[test]
    fn le_debug_ne_rend_pas_la_cle() {
        let config = ConfigTls::depuis_pem(CERTIFICAT, CLE).unwrap();
        let rendu = format!("{config:?}");
        assert!(rendu.contains("•••"), "{rendu}");
        assert!(!rendu.contains("PRIVATE"), "{rendu}");
        assert!(!rendu.contains("BEGIN"), "{rendu}");
    }

    #[test]
    fn une_cle_qui_ne_correspond_pas_est_refusee_au_demarrage() {
        match ConfigTls::depuis_pem(CERTIFICAT, AUTRE_CLE) {
            Err(ErreurTls::CleEtCertificatIncoherents(_)) => {}
            autre => panic!("attendu CleEtCertificatIncoherents, obtenu {autre:?}"),
        }
    }

    #[test]
    fn un_pem_sans_certificat_ni_cle_est_refuse() {
        assert!(matches!(
            ConfigTls::depuis_pem(b"", CLE),
            Err(ErreurTls::AucunCertificat)
        ));
        assert!(matches!(
            ConfigTls::depuis_pem(CERTIFICAT, b""),
            Err(ErreurTls::PemIllisible { .. } | ErreurTls::AucuneCle)
        ));
        assert!(matches!(
            ConfigTls::depuis_pem(CLE, CLE),
            Err(ErreurTls::AucunCertificat)
        ));
    }

    /// Une erreur de configuration est classée « avant toute écoute » : c'est ce
    /// qui autorise une surface à dire « je ne suis pas démarré » plutôt que de
    /// laisser croire qu'elle sert quelque chose.
    #[test]
    fn une_erreur_de_configuration_precede_toute_ecoute() {
        let e = ConfigTls::depuis_pem(CERTIFICAT, AUTRE_CLE).unwrap_err();
        assert!(e.avant_toute_ecoute());
        let e =
            ConfigTls::depuis_fichiers(Path::new("/inexistant.pem"), Path::new("/inexistant.pem"))
                .unwrap_err();
        assert!(e.avant_toute_ecoute());
        assert!(matches!(e, ErreurTls::CertificatAbsent { .. }));
    }
}
