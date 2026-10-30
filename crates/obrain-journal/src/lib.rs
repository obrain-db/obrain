//! # obrain-journal — l'histoire qu'un opérateur ne peut pas réécrire
//!
//! **Étape 4 de l'ADR `docs/ADR-2026-09-15-GARANTIE-CONFIDENTIALITE-CALCUL.md`.**
//!
//! ## Le constat qui justifie cette crate
//!
//! L'ADR, §4.1, réserve n° 1 — **CONSTATÉ** : `crates/obrain-iam/src/model.rs`
//! décrit ses entrées d'audit comme *« immutable audit trail entries »*. Or il
//! n'y a **ni chaînage par hachage, ni signature**, et le magasin appartient à
//! l'opérateur. Il peut donc supprimer ou réécrire un événement sans que rien ne
//! le signale — *« ce qui disqualifie l'audit comme garantie »*.
//!
//! > « Immuable » sans chaînage ni signature est un souhait.
//!
//! Cette crate remplace le souhait par une propriété.
//!
//! ## Les trois forces, et chacune ne fait qu'une chose
//!
//! | Contrôle | Ce qu'il attrape | Ce qu'il n'attrape pas |
//! |---|---|---|
//! | **la chaîne** ([`Journal::verifier`]) | toute entrée retirée, réordonnée, ou modifiée — et il **dit à quelle séquence** | une histoire reconstruite de bout en bout |
//! | **la signature** ([`Journal::verifier_signatures`]) | une entrée fabriquée par un autre, ou altérée après coup | une histoire reconstruite **et resignée** par qui détient la clé |
//! | **le reçu du client** ([`Journal::accorde_avec`]) | **la reconstruction resignée** : le reçu détenu ne s'accorde plus | rien, si le client n'a pas gardé de reçu |
//!
//! La troisième est la seule qui tienne contre l'opérateur, et c'est pour cela
//! qu'elle existe : les deux premières ne le gênent pas, puisqu'il détient la
//! clé. **La garantie ne vient pas du journal, elle vient de la rencontre entre
//! le journal et ce que le client garde.**
//!
//! ## Ce que cette crate ne prouve pas — et qu'il faut dire avant de promettre
//!
//! * **La complétude.** Un journal vide est parfaitement valide. Un opérateur
//!   qui n'inscrit rien n'est pris par rien ; ce qui le prend, c'est un client
//!   qui **attend** un reçu par calcul, et [`sequences_manquantes`] est là pour
//!   rendre ce trou visible.
//! * **L'honnêteté du calcul.** Le journal prouve qu'une entrée existe à une
//!   place, pas que le calcul annoncé a été exécuté. Cette moitié demande la
//!   mesure de code attestée (la seconde moitié de l'étape 4), pas un journal.
//! * **La fraîcheur de la clé publique.** Une clé publique livrée *avec* le
//!   journal ne prouve rien : elle serait réécrite avec lui. Elle doit être
//!   connue du client **avant**, par un autre canal.
//! * **L'horodatage.** Il est **déclaré** par l'opérateur. La chaîne prouve
//!   l'ordre, pas la date.
//! * **La confidentialité.** Le journal ne conserve que des empreintes, donc il
//!   ne dit rien du contenu — c'est une propriété du format, pas une preuve sur
//!   ce que l'appelant y met.
//!
//! ## L'usage, en trois lignes
//!
//! ```ignore
//! let cle = CleJournal::engendre()?;                 // l'opérateur signe
//! let publique = cle.publique();                     // ... et publie ceci AVANT
//! let mut journal = Journal::neuf("espace-alpha")?;
//!
//! // Chaque calcul rend un reçu ; le client l'emporte.
//! let recu = journal.recevoir(&cle, empreinte_de(requete), horodatage)?;
//!
//! // Plus tard, le client confronte ce qu'il garde à ce qu'on lui montre.
//! journal.verifier_signatures(&publique)?;
//! journal.accorde_avec(&recu)?;                      // ← le contrôle qui compte
//! ```
//!
//! L'étape 5 de l'ADR — le croisement de deux espaces — n'est pas traitée ici :
//! elle demande deux parties qui ne se font pas confiance, pas un journal.

#![forbid(unsafe_code)]

pub mod cle;
pub mod entree;
pub mod erreur;
pub mod journal;

pub use cle::CleJournal;
pub use entree::{Entree, TAILLE_EMPREINTE, TAILLE_SIGNATURE, empreinte_de, genese};
pub use erreur::{ErreurJournal, Resultat};
pub use journal::{IDENTIFIANT_MAXIMUM, Journal, sequences_manquantes};
