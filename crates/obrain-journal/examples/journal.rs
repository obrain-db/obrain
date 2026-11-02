//! Un journal sur le disque — **la démonstration, et le sujet de la porte**.
//!
//! Trois modes, chacun fait pour être appelé par
//! `scripts/acceptation-journal.sh` avec des outils qui ne savent rien de Rust :
//!
//! ```text
//! journal ecrire   <chemin> <identifiant> <cle-en-hex> <charge…>   # écrit
//! journal verifier <chemin> <cle-en-hex>                            # relit
//! journal tete     <chemin>                                        # tête seule
//! ```
//!
//! La clé est passée en hexadécimal sur la ligne de commande : c'est un
//! exemple, et une vraie surface la lirait ailleurs — mais elle est **exigée**,
//! jamais devinée, parce qu'un programme qui trouve sa clé tout seul est un
//! programme qui démarre avec celle d'un autre.
//!
//! `verifier` rend un code de sortie non nul dès que la chaîne ou une signature
//! ne tient plus : c'est ce qui permet à la porte de faire altérer le fichier
//! par `dd`, puis de constater le refus sans rien savoir du format.

use std::path::PathBuf;
use std::process::ExitCode;

use obrain_journal::{CleJournal, Journal, empreinte_de};

fn main() -> ExitCode {
    match executer() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("journal : {message}");
            ExitCode::FAILURE
        }
    }
}

fn executer() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("ecrire") => ecrire(&args[1..]),
        Some("verifier") => verifier(&args[1..]),
        Some("tete") => tete(&args[1..]),
        Some(autre) => Err(format!("mode inconnu : {autre}")),
        None => Err("usage : journal <ecrire|verifier|tete> …".to_owned()),
    }
}

fn ecrire(args: &[String]) -> Result<(), String> {
    let [chemin, identifiant, cle_hex, charges @ ..] = args else {
        return Err("usage : journal ecrire <chemin> <identifiant> <cle-hex> <charge…>".to_owned());
    };
    if charges.is_empty() {
        return Err(
            "au moins une charge est nécessaire : un journal vide ne démontre rien".to_owned(),
        );
    }

    let cle = CleJournal::depuis_octets(graine(cle_hex)?);
    let mut journal = Journal::neuf(identifiant).map_err(|e| e.to_string())?;

    let mut horodatage = 1_700_000_000u64;
    for charge in charges {
        journal
            .recevoir(&cle, empreinte_de(charge.as_bytes()), horodatage)
            .map_err(|e| e.to_string())?;
        horodatage += 1;
    }

    let chemin = PathBuf::from(chemin);
    std::fs::write(&chemin, journal.vers_octets())
        .map_err(|e| format!("écriture de {} : {e}", chemin.display()))?;

    // Ce que la porte lit : la tête, et de quoi vérifier l'arithmétique du
    // format sans rien connaître de lui.
    println!("TETE={}", hex(&journal.tete()));
    println!("ENTREES={}", journal.longueur());
    println!("OCTETS={}", journal.vers_octets().len());
    Ok(())
}

fn verifier(args: &[String]) -> Result<(), String> {
    let (chemin, cle_hex, tete_attendue) = match args {
        [chemin, cle_hex] => (chemin, cle_hex, None),
        [chemin, cle_hex, tete_hex] => (chemin, cle_hex, Some(graine(tete_hex)?)),
        _ => {
            return Err(
                "usage : journal verifier <chemin> <cle-hex> [tete-attendue-hex]".to_owned(),
            );
        }
    };
    let octets = std::fs::read(chemin).map_err(|e| format!("lecture de {chemin} : {e}"))?;

    // Le chargement refuse déjà un journal rompu : c'est le premier verrou.
    let journal = Journal::depuis_octets(&octets).map_err(|e| format!("journal refusé : {e}"))?;
    // Le second : les signatures, contre une clé venue d'ailleurs.
    journal
        .verifier_signatures(&CleJournal::depuis_octets(graine(cle_hex)?).publique())
        .map_err(|e| format!("signatures refusées : {e}"))?;
    // Le troisième, et il ne peut venir que du client : la tête qu'il a
    // conservée. **C'est le seul contrôle qui attrape la troncature** — un
    // journal amputé de sa queue est parfaitement cohérent avec lui-même.
    if let Some(tete) = tete_attendue {
        journal
            .verifier_jusqua(tete)
            .map_err(|e| format!("tête refusée : {e}"))?;
    }

    println!("TETE={}", hex(&journal.tete()));
    println!("ENTREES={}", journal.longueur());
    println!("OK");
    Ok(())
}

fn tete(args: &[String]) -> Result<(), String> {
    let [chemin] = args else {
        return Err("usage : journal tete <chemin>".to_owned());
    };
    let octets = std::fs::read(chemin).map_err(|e| format!("lecture de {chemin} : {e}"))?;
    let journal = Journal::depuis_octets(&octets).map_err(|e| format!("journal refusé : {e}"))?;
    println!("TETE={}", hex(&journal.tete()));
    Ok(())
}

/// Une graine de 32 octets depuis 64 caractères hexadécimaux.
fn graine(hexadecimal: &str) -> Result<[u8; 32], String> {
    if hexadecimal.len() != 64 {
        return Err(format!(
            "clé de {} caractères, 64 attendus (32 octets en hexadécimal)",
            hexadecimal.len()
        ));
    }
    let mut graine = [0u8; 32];
    for (i, octet) in graine.iter_mut().enumerate() {
        *octet = u8::from_str_radix(&hexadecimal[i * 2..i * 2 + 2], 16)
            .map_err(|e| format!("clé non hexadécimale : {e}"))?;
    }
    Ok(graine)
}

fn hex(octets: &[u8]) -> String {
    use core::fmt::Write as _;
    octets.iter().fold(String::new(), |mut rendu, o| {
        let _ = write!(rendu, "{o:02x}");
        rendu
    })
}
