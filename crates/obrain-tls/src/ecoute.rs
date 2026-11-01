//! L'écoute chiffrée — et la session qu'elle rend.
//!
//! [`FluxTls`] implémente [`std::io::Read`] et [`std::io::Write`]. C'est le
//! point de l'étape 0 : la boucle HTTP/1.1 écrite à la main dans
//! `obrain-cli/src/viz_serve.rs` prend un lecteur-écrivain générique ; lui
//! donner un `FluxTls` au lieu d'un `TcpStream` suffit, et **rien dans la boucle
//! ne change**. Le chiffrement cesse alors d'être un chantier pour devenir une
//! ligne.
//!
//! ## Deux garde-fous qui n'existent pas par défaut
//!
//! * **Le délai de poignée de main.** Un pair qui connecte et se tait
//!   immobiliserait un fil par connexion : mille connexions muettes, mille fils
//!   bloqués, zéro octet envoyé. Le délai n'existe que pendant la poignée de
//!   main ; une session établie n'est jamais coupée par lui.
//! * **Aucun repli.** [`EcouteTls::lier`] exige une [`ConfigTls`]. Il n'existe
//!   pas de variante « si le TLS échoue, continuer en clair », et cette absence
//!   est vérifiable par lecture : la signature ne le permet pas.

use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::time::Duration;

use rustls::{ProtocolVersion, ServerConnection, StreamOwned, SupportedCipherSuite};

use crate::config::ConfigTls;
use crate::erreur::{ErreurTls, Resultat};

/// Délai de poignée de main par défaut.
///
/// Dix secondes : largement au-delà d'un aller-retour réseau normal, très en
/// dessous de ce qu'il faut à un attaquant pour saturer les fils d'un serveur.
pub const DELAI_DE_POIGNEE_DE_MAIN: Duration = Duration::from_secs(10);

/// Délai d'écriture de l'alerte de fermeture, à la destruction d'une session.
///
/// Court à dessein : une fermeture ne doit jamais immobiliser un fil parce
/// qu'un pair a cessé de lire.
pub const DELAI_DE_FERMETURE: Duration = Duration::from_millis(500);

/// Une écoute TCP qui **ne sert que du chiffré**.
pub struct EcouteTls {
    config: ConfigTls,
    ecoute: TcpListener,
    delai: Duration,
}

impl EcouteTls {
    /// Lie une adresse et prépare l'écoute chiffrée.
    ///
    /// # Erreurs
    ///
    /// [`ErreurTls::Ecoute`] si l'adresse ne peut pas être liée.
    pub fn lier(adresse: impl ToSocketAddrs, config: ConfigTls) -> Resultat<Self> {
        let ecoute = TcpListener::bind(adresse).map_err(|e| ErreurTls::Ecoute(e.to_string()))?;
        Ok(Self {
            config,
            ecoute,
            delai: DELAI_DE_POIGNEE_DE_MAIN,
        })
    }

    /// Change le délai de poignée de main.
    #[must_use]
    pub fn avec_delai_de_poignee_de_main(mut self, delai: Duration) -> Self {
        self.delai = delai;
        self
    }

    /// L'adresse réellement liée — utile quand le port demandé était `0`.
    ///
    /// # Erreurs
    ///
    /// [`ErreurTls::Ecoute`] si le système ne rend pas l'adresse.
    pub fn adresse_locale(&self) -> Resultat<SocketAddr> {
        self.ecoute
            .local_addr()
            .map_err(|e| ErreurTls::Ecoute(e.to_string()))
    }

    /// La configuration employée. Elle ne contient aucune clé lisible : voir le
    /// `Debug` de [`ConfigTls`].
    #[must_use]
    pub fn config(&self) -> &ConfigTls {
        &self.config
    }

    /// Attend une connexion et mène la poignée de main, avec le délai courant.
    ///
    /// # Erreurs
    ///
    /// * [`ErreurTls::PoigneeDeMainSansReponse`] si le pair se tait ;
    /// * [`ErreurTls::PoigneeDeMainRefusee`] si la poignée de main échoue
    ///   (pair qui parle en clair, client qui refuse le certificat, version
    ///   refusée). Dans les deux cas **aucun octet applicatif n'est servi**.
    pub fn accepter(&self) -> Resultat<FluxTls> {
        self.accepter_avec_delai(self.delai)
    }

    /// Comme [`EcouteTls::accepter`], avec un délai explicite.
    ///
    /// # Erreurs
    ///
    /// Les mêmes que [`EcouteTls::accepter`].
    pub fn accepter_avec_delai(&self, delai: Duration) -> Resultat<FluxTls> {
        let (mut socket, _pair) = self
            .ecoute
            .accept()
            .map_err(|e| ErreurTls::Ecoute(e.to_string()))?;

        // Le délai vaut pour la poignée de main, et pour elle seule.
        socket
            .set_read_timeout(Some(delai))
            .map_err(|e| ErreurTls::Ecoute(e.to_string()))?;
        socket
            .set_write_timeout(Some(delai))
            .map_err(|e| ErreurTls::Ecoute(e.to_string()))?;

        let mut connexion = ServerConnection::new(self.config.interieur())
            .map_err(|e| ErreurTls::ConfigurationRefusee(e.to_string()))?;

        // Poignée de main explicite : une erreur ici est une erreur *de
        // poignée de main*, nommée, et non une erreur d'entrée/sortie au milieu
        // du traitement d'une requête — c'est la différence entre « ce pair
        // n'est pas un client TLS » et « le client a disparu ».
        connexion.complete_io(&mut socket).map_err(|e| {
            if matches!(
                e.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
            ) {
                ErreurTls::PoigneeDeMainSansReponse(delai)
            } else {
                ErreurTls::PoigneeDeMainRefusee(e.to_string())
            }
        })?;

        // Session établie : plus de délai. Une session ouverte est légitime et
        // peut le rester.
        socket
            .set_read_timeout(None)
            .map_err(|e| ErreurTls::EntreeSortie(e.to_string()))?;
        socket
            .set_write_timeout(None)
            .map_err(|e| ErreurTls::EntreeSortie(e.to_string()))?;

        Ok(FluxTls {
            flux: StreamOwned::new(connexion, socket),
        })
    }
}

impl core::fmt::Debug for EcouteTls {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EcouteTls")
            .field("adresse", &self.ecoute.local_addr().ok())
            .field("config", &self.config)
            .field("delai", &self.delai)
            .finish()
    }
}

/// Une session TLS établie, lisible et scriptible comme un `TcpStream`.
///
/// Tout ce qui sort d'ici est chiffré : il n'existe aucun moyen d'obtenir le
/// `TcpStream` sous-jacent en écriture, donc aucun moyen d'écrire en clair par
/// mégarde. C'est une propriété de type, pas une convention.
pub struct FluxTls {
    flux: StreamOwned<ServerConnection, TcpStream>,
}

impl FluxTls {
    /// La version de protocole négociée.
    #[must_use]
    pub fn protocole(&self) -> Option<ProtocolVersion> {
        self.flux.conn.protocol_version()
    }

    /// La suite de chiffrement négociée.
    #[must_use]
    pub fn suite_de_chiffrement(&self) -> Option<SupportedCipherSuite> {
        self.flux.conn.negotiated_cipher_suite()
    }

    /// L'adresse du pair.
    #[must_use]
    pub fn pair_distante(&self) -> Option<SocketAddr> {
        self.flux.sock.peer_addr().ok()
    }

    /// Le nom de serveur demandé par le client (SNI), s'il en a donné un.
    ///
    /// Une seule configuration est servie, quel que soit le nom : cette crate
    /// ne fait pas de sélection par nom d'hôte, et le dit plutôt que de le
    /// laisser croire.
    #[must_use]
    pub fn nom_demande(&self) -> Option<String> {
        self.flux
            .conn
            .server_name()
            .map(std::string::ToString::to_string)
    }

    // ── LES RÉGLAGES DE SESSION, ET POURQUOI ILS SONT ICI ───────────────────
    //
    // `viz_serve.rs` pose trois réglages sur chaque connexion : un délai de
    // lecture, un délai d'écriture, et `nodelay`. Le délai de lecture n'est pas
    // un confort : c'est la protection contre un client qui annonce une requête
    // et ne l'envoie jamais (slowloris) — sans lui, chaque connexion paresseuse
    // immobilise un fil.
    //
    // Si `FluxTls` ne les exposait pas, migrer vers le chiffré **perdrait cette
    // protection en silence** : la boucle resterait identique en apparence, et
    // le serveur deviendrait attaquable d'une façon qu'il ne l'était pas. Ces
    // trois méthodes existent pour que l'adoption du TLS soit neutre — ni gain,
    // ni perte, sur tout ce qui n'est pas le chiffrement.

    /// Pose un délai de lecture, ou le retire avec `None`.
    ///
    /// # Erreurs
    ///
    /// [`ErreurTls::EntreeSortie`] si le système refuse le réglage.
    pub fn set_read_timeout(&self, delai: Option<Duration>) -> Resultat<()> {
        self.flux
            .sock
            .set_read_timeout(delai)
            .map_err(|e| ErreurTls::EntreeSortie(e.to_string()))
    }

    /// Pose un délai d'écriture, ou le retire avec `None`.
    ///
    /// # Erreurs
    ///
    /// [`ErreurTls::EntreeSortie`] si le système refuse le réglage.
    pub fn set_write_timeout(&self, delai: Option<Duration>) -> Resultat<()> {
        self.flux
            .sock
            .set_write_timeout(delai)
            .map_err(|e| ErreurTls::EntreeSortie(e.to_string()))
    }

    /// Active ou désactive `TCP_NODELAY`.
    ///
    /// # Erreurs
    ///
    /// [`ErreurTls::EntreeSortie`] si le système refuse le réglage.
    pub fn set_nodelay(&self, actif: bool) -> Resultat<()> {
        self.flux
            .sock
            .set_nodelay(actif)
            .map_err(|e| ErreurTls::EntreeSortie(e.to_string()))
    }
}

impl Read for FluxTls {
    fn read(&mut self, tampon: &mut [u8]) -> io::Result<usize> {
        self.flux.read(tampon)
    }
}

impl Write for FluxTls {
    fn write(&mut self, tampon: &[u8]) -> io::Result<usize> {
        self.flux.write(tampon)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flux.flush()
    }
}

/// Fermer proprement : envoyer `close_notify` avant de rendre le socket.
///
/// Sans cela, la session se termine par un EOF sec — et rustls, côté client,
/// refuse un EOF sec (`peer closed connection without sending TLS
/// close_notify`), **à raison** : c'est ainsi qu'on détecte une attaque par
/// troncature, où un intermédiaire coupe le flux pour amputer la réponse.
///
/// Un serveur qui ferme mal fait donc apparaître une erreur de sécurité chez
/// tous ses clients, et leur apprend au passage à ignorer ce signal. Envoyer
/// l'alerte est la moindre des choses.
///
/// Le délai d'écriture est borné ici, et seulement ici : une fermeture ne doit
/// jamais immobiliser un fil parce qu'un pair ne lit plus.
impl Drop for FluxTls {
    fn drop(&mut self) {
        let _ = self
            .flux
            .sock
            .set_write_timeout(Some(DELAI_DE_FERMETURE));
        self.flux.conn.send_close_notify();
        let _ = self.flux.flush();
    }
}

impl core::fmt::Debug for FluxTls {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FluxTls")
            .field("pair", &self.pair_distante())
            .field("protocole", &self.protocole())
            .field("suite", &self.suite_de_chiffrement().map(|s| format!("{:?}", s.suite())))
            .finish()
    }
}
