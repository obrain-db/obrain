//! # obrain-tls — le transport chiffré des surfaces d'entrée
//!
//! **Étape 0 de l'ADR `docs/ADR-2026-09-15-GARANTIE-CONFIDENTIALITE-CALCUL.md`.**
//!
//! ## Le constat qui justifie cette crate
//!
//! **CONSTATÉ** : il n'existe **aucun TLS dans tout le workspace Obrain**. Un
//! `grep` sur `rustls`, `native-tls` et `openssl` dans l'ensemble des
//! `Cargo.toml` ne rend rien, et `crates/obrain-cli/src/viz_serve.rs` est un
//! HTTP/1.1 écrit à la main sur `std::net::TcpListener`. Conséquence directe :
//! **un `tcpdump` sur le LAN rend les jetons porteurs, les requêtes et les
//! résultats.** Tant que c'est vrai, tout ce qu'on édifie au-dessus — clés par
//! espace, enveloppes scellées, jetons de capacité — est lisible par le premier
//! venu sur le réseau, sans même patcher le moteur.
//!
//! C'est aussi le trou **le moins cher à fermer** : celui qui protège
//! immédiatement, contre tout le monde, sans rien décider d'autre.
//!
//! ## Le critère de vérification, et il est falsifiable
//!
//! Un `tcpdump` d'une requête `/api/query` ne doit montrer **ni** le jeton, **ni**
//! la requête, **ni** le résultat. `tests/porte_tls.rs` l'exécute sans
//! `tcpdump` : un relais enregistreur s'intercale entre le client et le serveur
//! et conserve **tous** les octets qui passent, dans les deux sens.
//!
//! Et parce qu'un test de capture qui ne capturerait rien passerait pour de
//! mauvaises raisons, **un témoin** vérifie que le même montage, appliqué à un
//! serveur en clair, **voit** le jeton. Le test a donc la puissance de détecter
//! ce qu'il cherche ; sans ce témoin, il ne prouverait rien.
//!
//! ## Ce que la crate garantit
//!
//! | Garantie | Moyen |
//! |---|---|
//! | Le contenu ne traverse pas le réseau en clair | rustls, TLS 1.3 négocié par défaut |
//! | **Aucun repli en clair, par construction** | [`EcouteTls::lier`] exige une [`ConfigTls`] ; il n'existe pas de variante « continuer sans TLS » |
//! | Une erreur de configuration survient **avant** toute écoute | [`ErreurTls::avant_toute_ecoute`] |
//! | Une clé qui ne correspond pas au certificat est refusée au démarrage | vérifié par rustls, testé |
//! | Un pair muet ne bloque pas un fil indéfiniment | délai de poignée de main, puis renvoi |
//! | La clé privée ne fuit pas dans les journaux | `Debug` de [`ConfigTls`] qui n'imprime jamais le matériel de clé |
//! | Le flux est utilisable **sans changer la boucle HTTP** | [`FluxTls`] implémente `Read + Write` |
//!
//! ## Ce que la crate ne fait pas — et qu'il faut dire
//!
//! * **Pas d'authentification du client** (pas de mTLS) : `with_no_client_auth()`.
//!   Un client n'est pas authentifié par son certificat ; l'authentification
//!   reste le jeton porteur de l'IAM, qui circule désormais **dans** le tunnel.
//!   Un jeton volé reste un jeton volé : le TLS protège le trajet, pas le porteur.
//! * **Pas de sélection par nom d'hôte** (un seul certificat, quel que soit le
//!   SNI reçu).
//! * **Pas de rotation, pas d'ACME, pas de révocation.** Le certificat est un
//!   fichier, rechargé au démarrage et pas à chaud : renouveler demande de
//!   relancer le service. C'est une limite réelle pour un déploiement, pas un
//!   détail.
//! * **Pas de post-quantique.** Le fournisseur est `ring`, sans `X25519MLKEM768`
//!   (voir [`fournisseur`]). Passer à `aws-lc-rs` est une ligne de manifeste.
//! * **Pas de protection contre un observateur qui mesure.** Les tailles, les
//!   durées et la fréquence des messages restent visibles, TLS ou non : c'est la
//!   fuite de métadonnées (G3), que cette crate ne traite pas.
//!
//! ## Le contrat d'intégration
//!
//! La boucle de `viz_serve.rs` lit et écrit sur un `TcpStream`. [`FluxTls`]
//! implémente les mêmes traits, donc **le corps de la boucle ne change pas** :
//!
//! ```ignore
//! // avant
//! let (flux, pair) = ecoute.accept()?;
//! // après
//! let flux = ecoute_tls.accepter()?;      // FluxTls : Read + Write
//! let pair = flux.pair_distante();
//! ```
//!
//! La seule décision à prendre est **où** placer la configuration : un drapeau
//! d'environnement qui bascule entre `TcpListener` et [`EcouteTls`] recrée
//! exactement le repli en clair que cette crate existe pour empêcher. Le choix
//! cohérent est de n'avoir **qu'un** chemin : celui-ci.

#![forbid(unsafe_code)]

pub mod config;
pub mod ecoute;
pub mod erreur;

pub use config::{ConfigTls, fournisseur};
pub use ecoute::{DELAI_DE_POIGNEE_DE_MAIN, EcouteTls, FluxTls};
pub use erreur::{ErreurTls, Resultat};
