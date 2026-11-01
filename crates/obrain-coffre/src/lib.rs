//! # obrain-coffre — la confidentialité du calcul, rendue structurelle
//!
//! Ce que l'engine ne peut pas lire, il ne peut pas l'écouter. Cette crate
//! existe pour que cette phrase soit une **propriété du code** et non une
//! politique : là où l'IAM rend un *verdict* (`Allow` / `Deny`), ici il n'y a
//! pas de verdict à rendre — il y a une clé, ou il n'y a pas de clé.
//!
//! > Une politique qui a tort ne le dit pas ; une clé absente ne peut pas avoir
//! > tort.
//!
//! ## Ce que la crate garantit, et à quel prix
//!
//! | Garantie | Tenue ici ? | Moyen |
//! |---|---|---|
//! | **G1 — contenu** : personne ne lit les octets, au repos comme en transit | **oui** | AES-256-GCM, clé par espace, en-tête et blocs authentifiés |
//! | **G2 — calcul** : personne ne voit *ce que* le calcul fait | **partiellement** | l'enveloppe ne dit que sa classe de taille ; le clair du calcul n'existe que chez qui détient la clé |
//! | **G3 — métadonnées** : personne ne déduit qui calcule quoi | **partiellement** | enveloppes à taille fixe et jetons à taille fixe ; le *moment*, la *fréquence* et le *volume* restent visibles |
//! | **G4 — intégrité** : personne ne substitue un résultat | **oui, sauf un cas nommé** | toute altération, troncature, réordonnancement et greffe entre coffres est détectée ; la substitution d'un **coffre entier** exige de dire lequel on attend ([`Coffre::desceller_avec_id`]) |
//! | **G5 — inférence** : aucun résultat ne révèle les données d'autrui | **non** | hors du périmètre de cette crate : ne jamais croiser deux espaces |
//!
//! ## Ce que la crate ne garantit pas — et il faut le dire
//!
//! * **Le clair pendant le calcul.** Si le clair est descellé sur la machine de
//!   l'opérateur, l'opérateur le lit. Aucune ligne de cette crate n'y change
//!   quoi que ce soit : pour tenir « même celui qui exécute l'engine », le
//!   déscellement doit avoir lieu **chez celui qui a le droit de lire** (calcul
//!   local), ou dans une enveloppe attestée par le silicium avec vérification
//!   du rapport d'attestation par le client (voir l'ADR du 2026-09-15).
//! * **Le canal.** Chiffrer le contenu sans chiffrer le transport ne protège
//!   rien : un observateur réseau lit les jetons et les résultats. Le TLS sur
//!   les surfaces d'entrée est une étape distincte et préalable.
//! * **Les canaux auxiliaires.** Les durées, les tailles de classe, les motifs
//!   d'accès mémoire et la fréquence des requêtes restent observables.
//! * **La perte de clé.** Il n'existe aucun recours : c'est le prix de
//!   l'absence de point de défaillance unique. Un utilisateur qui perd sa clé
//!   perd ses données, et doit le savoir **avant**.
//!
//! ## La hiérarchie de clés
//!
//! ```text
//! phrase / secret de l'utilisateur
//!        │  Argon2id (sel de 16 octets, paramètres plancher vérifiés)
//!        ▼
//!   clé maîtresse (KEK) ── ne quitte jamais celui qui la dérive
//!        │  AES-256-GCM, AAD liant l'empreinte de l'espace
//!        ▼
//!   enveloppe de clé (128 octets fixes, sur disque)
//!        │
//!        ▼
//!   clé d'espace (DEK, 32 octets) ── une par espace, jamais partagée
//!        │  HKDF-SHA256, séparation de domaines
//!        ├──▶ sous-clé « coffre »    → scellement de contenu
//!        ├──▶ sous-clé « enveloppe » → enveloppes à taille fixe
//!        └──▶ sous-clé « jeton »     → jetons de capacité
//! ```
//!
//! Aucune clé n'est écrite en clair : le disque ne porte que des enveloppes de
//! clé, illisibles sans la KEK.
//!
//! ## Ce que l'API rend impossible par construction
//!
//! * **Utiliser une clé sans la détenir** : [`Coffre`], [`Enveloppe`] et
//!   [`Jeton`] ne se construisent qu'à partir d'une [`CleEspace`]. Il n'existe
//!   aucune variante qui prenne un espace et rende du clair : le type de la
//!   clé *est* l'autorisation.
//! * **Réutiliser une clé pour un autre espace** : l'empreinte de l'espace
//!   entre dans l'AAD de chaque sceau. Copier une clé d'un espace à l'autre ne
//!   donne donc pas accès aux données de l'autre — le déscellement échoue.
//! * **Fabriquer une enveloppe dont le sel ne correspond pas à la clé** : le sel
//!   est tiré par [`CleEspace::envelopper_avec_phrase`] et écrit dans
//!   l'enveloppe ; il n'existe aucun chemin qui accepte les deux séparément. Ce
//!   défaut-là a existé, et c'est un test qui l'a trouvé — voir la documentation
//!   de `cle.rs`.
//! * **Greffer un bloc d'un coffre dans un autre** : chaque coffre porte un
//!   identifiant tiré au sort qui entre dans l'AAD de ses blocs. Ce défaut-là
//!   aussi a existé : deux coffres du même espace et de même forme avaient un
//!   AAD identique.
//! * **Confondre deux usages** : un jeton ne peut pas être présenté comme un
//!   bloc de coffre, ni l'inverse (sous-clés distinctes par HKDF).
//! * **Accepter une altération** : longueur, index de bloc, identité d'espace,
//!   identité de coffre, troncature et réordonnancement sont tous détectés par
//!   l'AEAD.
//!
//! ## Ce que les tests vérifient, critère par critère
//!
//! Les critères falsifiables de l'ADR sont exécutables :
//! `tests/porte_confidentialite.rs` établit qu'un `hexdump` du scellé ne rend
//! aucun littéral du clair, que retirer la clé rend les données illisibles
//! « même avec tous les droits », et que deux charges utiles différentes
//! produisent des enveloppes de longueur identique.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

pub mod aleatoire;
pub mod cle;
pub mod coffre;
pub mod depot;
pub mod enveloppe;
pub mod erreur;
pub mod jeton;

pub use cle::{
    CleEspace, CleMaitresse, EmpreinteEspace, EnveloppeCle, OrigineKek, ParametresKdf, Sel,
};
pub use coffre::{Coffre, IdCoffre};
pub use depot::{ecrire_enveloppe, lire_enveloppe};
pub use enveloppe::{ClasseTaille, Enveloppe};
pub use erreur::{CoffreError, Resultat};
pub use jeton::{Jeton, Portee};
