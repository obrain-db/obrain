//! `sceller` — sceller et desceller un fichier, pour vérifier à la main.
//!
//! Cet exemple existe pour la **porte d'acceptation** (`scripts/acceptation-coffre.sh`).
//! Sa raison d'être est de rendre le critère vérifiable par un outil
//! **indépendant du harnais de test** : le script scelle un fichier, puis
//! cherche un littéral dedans avec `grep`. Un test Rust qui affirme « le
//! littéral n'apparaît pas » pourrait se tromper avec le code qu'il teste ;
//! `grep` sur le fichier produit, non.
//!
//! ```text
//! sceller sceller   <phrase> <espace> <clair> <scellé> <clé>
//! sceller desceller <phrase> <clé> <scellé> <clair>
//! sceller enveloppe <espace> <charge> <sortie>   # taille fixe, sans clé de sortie
//! ```

use std::process::ExitCode;

use obrain_coffre::cle::ParametresKdf;
use obrain_coffre::{ClasseTaille, CleEspace, Coffre, Enveloppe, ecrire_enveloppe, lire_enveloppe};

/// Le plancher : ce que la porte paie, et rien de moins.
const PARAMS: ParametresKdf = ParametresKdf {
    m_cost_kio: 8 * 1024,
    t_cost: 2,
    p_cost: 1,
};

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let Some((commande, reste)) = arguments.split_first() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };

    let resultat = match commande.as_str() {
        "sceller" => sceller(reste),
        "desceller" => desceller(reste),
        "enveloppe" => enveloppe(reste),
        _ => Err(format!("commande inconnue : {commande}\n{USAGE}")),
    };

    match resultat {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("refus : {message}");
            ExitCode::from(1)
        }
    }
}

const USAGE: &str = "usage :
  sceller sceller   <phrase> <espace> <clair> <scellé> <clé>
  sceller desceller <phrase> <clé> <scellé> <clair>
  sceller enveloppe <espace> <charge> <sortie>";

fn sceller(arguments: &[String]) -> Result<(), String> {
    let [phrase, espace, clair, sortie, chemin_cle] = arguments else {
        return Err(USAGE.to_owned());
    };

    let donnees = std::fs::read(clair).map_err(|e| format!("{clair} : {e}"))?;
    let cle = CleEspace::engendrer(espace).map_err(|e| e.to_string())?;
    let coffre = Coffre::neuf(&cle).map_err(|e| e.to_string())?;
    let scelle = coffre.sceller(&donnees).map_err(|e| e.to_string())?;
    let enveloppe = cle
        .envelopper_avec_phrase(phrase, PARAMS)
        .map_err(|e| e.to_string())?;

    std::fs::write(sortie, &scelle).map_err(|e| format!("{sortie} : {e}"))?;
    ecrire_enveloppe(std::path::Path::new(chemin_cle), &enveloppe).map_err(|e| e.to_string())?;

    let identifiant = Coffre::identifiant_lu(&scelle).map_err(|e| e.to_string())?;
    println!(
        "scellé : {} octets sur {} octets de clair, coffre {}",
        scelle.len(),
        donnees.len(),
        hex::encode(identifiant)
    );
    Ok(())
}

fn desceller(arguments: &[String]) -> Result<(), String> {
    let [phrase, chemin_cle, scelle, sortie] = arguments else {
        return Err(USAGE.to_owned());
    };

    let enveloppe = lire_enveloppe(std::path::Path::new(chemin_cle)).map_err(|e| e.to_string())?;
    let kek = enveloppe.deriver_kek(phrase).map_err(|e| e.to_string())?;
    let cle = CleEspace::desceller(&enveloppe, &kek).map_err(|e| e.to_string())?;
    let coffre = Coffre::neuf(&cle).map_err(|e| e.to_string())?;

    let octets = std::fs::read(scelle).map_err(|e| format!("{scelle} : {e}"))?;
    let clair = coffre.desceller(&octets).map_err(|e| e.to_string())?;
    std::fs::write(sortie, &clair).map_err(|e| format!("{sortie} : {e}"))?;
    println!("descellé : {} octets", clair.len());
    Ok(())
}

fn enveloppe(arguments: &[String]) -> Result<(), String> {
    let [espace, charge, sortie] = arguments else {
        return Err(USAGE.to_owned());
    };

    let cle = CleEspace::engendrer(espace).map_err(|e| e.to_string())?;
    let enveloppe = Enveloppe::neuf(&cle).map_err(|e| e.to_string())?;
    let scellee = enveloppe
        .sceller(charge.as_bytes())
        .map_err(|e| e.to_string())?;

    std::fs::write(sortie, &scellee).map_err(|e| format!("{sortie} : {e}"))?;
    let classe = ClasseTaille::lue(&scellee).map_err(|e| e.to_string())?;
    println!(
        "enveloppe : {} octets, classe {} (capacité {})",
        scellee.len(),
        classe.indice(),
        classe.capacite()
    );
    Ok(())
}
