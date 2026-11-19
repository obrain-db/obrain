//! Pré-vol : vérifie que la limite de descripteurs de fichiers suffit à la suite.
//!
//! ── POURQUOI (obrain#238) ─────────────────────────────────────────────────
//!
//! Les tests de `obrain-engine` tournent en parallèle et chacun ouvre plusieurs
//! zones mmap. Avec la limite par défaut de macOS (`ulimit -n 256`), 53 tests
//! concurrents l'épuisent : des ouvertures de zone échouent, et la requête
//! concernée rend **zéro ligne** au lieu d'une — sans erreur.
//!
//! Mesuré le 19/11/2026 sur le binaire `match_cible_deja_liee` :
//!
//! | `ulimit -n` | exécutions en échec |
//! |---|---|
//! | 256 (défaut macOS) | **5 sur 10** |
//! | 4096 | **0 sur 10** |
//!
//! Le symptôme est trompeur : il tombe sur un test différent à chaque exécution,
//! jamais le même couple, et disparaît avec `--test-threads=1`. On cherche une
//! course dans le code alors que c'est une limite d'environnement.
//!
//! Ce test ne corrige rien : il refuse de laisser la suite tourner dans des
//! conditions qui produiront un flake, et dit quoi faire.
#![cfg(unix)]

/// En dessous, la suite épuise les descripteurs : mesuré 5 échecs sur 10 à 256.
/// Le runner de CI est à 1024, d'où ce plancher : il doit passer.
const LIMITE_PLANCHER: u64 = 1024;

/// À partir d'ici, aucune exécution en échec n'a été mesurée. Entre les deux,
/// on avertit sans faire échouer : la CI tourne à 1024 et doit rester verte.
const LIMITE_CONFORT: u64 = 4096;

fn limite_actuelle() -> Option<u64> {
    let sortie = std::process::Command::new("sh")
        .arg("-c")
        .arg("ulimit -n")
        .output()
        .ok()?;
    String::from_utf8_lossy(&sortie.stdout).trim().parse().ok()
}

#[test]
fn la_limite_de_descripteurs_suffit_pour_la_suite() {
    let Some(limite) = limite_actuelle() else {
        // `sh` absent ou sortie illisible : on ne fait pas échouer la suite pour
        // un pré-vol qu'on ne sait pas lire.
        eprintln!("pré-vol descripteurs : limite illisible, vérification ignorée");
        return;
    };
    if (LIMITE_PLANCHER..LIMITE_CONFORT).contains(&limite) {
        eprintln!(
            "pré-vol descriptors : ulimit -n = {limite}. Aucun échec n'a été mesuré \
             à {LIMITE_CONFORT} ou au-delà ; en dessous, un flake reste possible. \
             Pour être tranquille :  ulimit -n {LIMITE_CONFORT} && cargo test"
        );
    }
    assert!(
        limite >= LIMITE_PLANCHER,
        "ulimit -n = {limite}, en dessous de {LIMITE_PLANCHER}.\n\
         Les tests parallèles ouvrent plusieurs zones mmap chacun : sous ce seuil, \
         des ouvertures échouent et des requêtes rendent zéro ligne au lieu d'une, \
         sur un test différent à chaque exécution (obrain#238).\n\
         Relancer avec :  ulimit -n 4096 && cargo test"
    );
}
