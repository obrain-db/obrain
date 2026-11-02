//! `obrain resonance` — le GESTE de partage, dit à l'utilisateur.
//!
//! La règle vit dans `crate::resonance` (ce qui se partage, ce qui ne sort
//! jamais, et comment un lien se referme) ; ce module ne fait que la déclencher
//! et la RENDRE LISIBLE. C'est la même séparation que pour les mondes : le
//! raisonnement dans un module testable, la présentation dans une commande.
//!
//! ── UN GESTE QUI NE SE DIT PAS NE SE VÉRIFIE PAS ────────────────────────────
//! Poser un lien est une décision sur des données personnelles. La commande
//! annonce donc, à chaque fois, EXACTEMENT ce qui devient lisible et ce qui ne
//! le devient pas — et non un « ok » qui laisserait croire à un partage total.

use anyhow::Result;

use crate::OutputFormat;
use crate::output;

/// Exécute le geste demandé (poser ou refermer un lien).
pub fn run(
    publieur: &str,
    lecteur: &str,
    delier: bool,
    format: OutputFormat,
    quiet: bool,
) -> Result<()> {
    // Ce qui est partagé, et ce qui ne l'est pas — écrit UNE fois, et affiché
    // dans les deux sens.
    let partage = "noms d'objectifs et de procédures · mondes apportés · \
                   volume de mémoire et dernière activité";
    let jamais = "texte des notes · état intime (etat-self)";

    if delier {
        let retire = crate::resonance::delier(publieur, lecteur)?;
        if !quiet {
            if retire {
                eprintln!(
                    "résonance : lien refermé — « {lecteur} » ne voit plus les facettes de « {publieur} »"
                );
            } else {
                eprintln!(
                    "résonance : aucun lien à refermer entre « {publieur} » et « {lecteur} »"
                );
            }
        }
        output::print_result_table(
            &[
                "geste".into(),
                "publieur".into(),
                "lecteur".into(),
                "change".into(),
            ],
            &[vec![
                "delier".into(),
                publieur.into(),
                lecteur.into(),
                retire.to_string(),
            ]],
            format.into(),
            quiet,
        );
        return Ok(());
    }

    let pose = crate::resonance::lier(publieur, lecteur)?;
    if !quiet {
        if pose {
            eprintln!(
                "résonance : lien établi — « {lecteur} » peut lire les facettes publiques de « {publieur} »"
            );
        } else {
            eprintln!("résonance : le lien existait déjà — il n'est ni dupliqué ni redaté");
        }
        // LE CONTRAT EST DIT À CHAQUE FOIS : c'est la seule façon qu'un partage
        // reste une décision informée, et non un effet qu'on découvre après.
        eprintln!("  partagé   : {partage}");
        eprintln!("  jamais    : {jamais}");
    }
    output::print_result_table(
        &[
            "geste".into(),
            "publieur".into(),
            "lecteur".into(),
            "change".into(),
            "partage".into(),
            "jamais".into(),
        ],
        &[vec![
            "lier".into(),
            publieur.into(),
            lecteur.into(),
            pose.to_string(),
            partage.into(),
            jamais.into(),
        ]],
        format.into(),
        quiet,
    );
    Ok(())
}
