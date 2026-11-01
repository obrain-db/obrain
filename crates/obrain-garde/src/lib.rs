//! ── LA GARDE D'UNE GÉNÉRATION ───────────────────────────────────────────────
//!
//! Ce crate répond à une question mesurée le 2026-09-16 : **l'engine sait-il
//! s'arrêter, PARTOUT ?** La réponse était non. Le coupe-circuit de répétition,
//! le budget par question et l'arrêt sur client parti vivaient dans
//! `obrain-cli/src/engine.rs` — un seul des quatre endroits du dépôt qui parlent
//! à un modèle. Les deux boucles de `obrain-selfd` (le micro-harness qui AGIT sur
//! la machine) n'avaient aucun des trois, et `obrain-httpmin` — le client SSE
//! qu'elles utilisent — avait un rappel qui rendait `()`, donc un appelant
//! **incapable d'interrompre la lecture**.
//!
//! Une garantie qui vit dans un appelant n'est pas une garantie : c'est une
//! habitude. Ce crate porte donc la garantie, une fois, et les boucles la
//! traversent.
//!
//! # Ce qu'il porte, et ce qu'il ne porte pas
//!
//! * [`garde::Garde`] — l'horloge (budget), le drapeau du client parti, la prose
//!   accumulée, et le motif d'arrêt une fois décidé. **Aucune E/S** : la garde ne
//!   lit pas le flux, elle juge ce que la boucle lui donne à voir.
//! * [`repetition`] — la détection d'un motif dégénéré (bloc ou ligne répétés).
//!   Prudente par construction : un détecteur qui crie au loup couperait des
//!   réponses JUSTES, ce qui serait pire que le défaut.
//! * [`garde::index_appel_valide`] — le refus d'un index d'appel d'outil
//!   aberrant. Défaut mesuré ailleurs dans le dépôt : `while appels.len() <= i`
//!   avec un index venu du flux distant allouait un milliard de structures
//!   (~70 Go) et faisait avorter le processus. Le correctif existait dans une
//!   seule boucle sur trois.
//!
//! La garde ne juge **pas** la qualité d'une réponse, et elle n'invente **aucun**
//! chiffre : tout ce qu'elle dit vient de ce qui a été émis, et l'arrêt se dit.

pub mod garde;
pub mod repetition;

pub use garde::{Garde, Motif, Reglages, Veille, index_appel_valide};
pub use repetition::{Repetition, boucle};
