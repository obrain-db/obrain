//! LE VERROU EXCLUSIF EST RENDU **SYNCHRONEMENT** AU DROP — invariant gardé.
//!
//! Deux tests de `simplex_durability` tombent sous charge sur un refus de verrou
//! exclusif, alors qu'ils viennent de fermer le store quelques lignes plus haut :
//!
//! ```text
//! substrate /tmp/.tmp…/kb is already open by another process (exclusive lock on
//! …/substrate.meta refused: Resource temporarily unavailable)
//! ```
//!
//! Hypothèse : le `flock` est porté par une *description de fichier* que le
//! **mmap** des zones maintient en vie ; le démontage des mappings peut donc
//! retarder la libération, et un `open` immédiat la rencontre.
//!
//! ── CE QUE L'INVARIANT EXIGE, ET POURQUOI IL EST UTILE ─────────────────────
//! « Fermer puis rouvrir » est un geste NORMAL (les tests, les outils, une
//! réouverture après migration) : si le `flock` survivait au drop, ce geste
//! deviendrait un pari. L'invariant : après le drop du dernier porteur, une
//! ouverture immédiate doit réussir **sans attendre**.
//!
//! ── CE QUI A ÉTÉ MESURÉ (2026-09-17) ────────────────────────────────────────
//! Deux tests de `simplex_durability` sont tombés, en suite COMPLÈTE seulement,
//! sur un refus de verrou quelques lignes après avoir fermé le store. Recherche
//! de reproduction :
//!
//! * seul : 6/6 vert, 0/60 ouvertures immédiates refusées, pire attente 72 µs ;
//! * 4 instances en parallèle (4 × 60 itérations) : **0 refus**, pire attente
//!   810 µs — le démontage des zones se paie sous charge, mais il reste
//!   SYNCHRONE ;
//! * 4 instances de `simplex_durability` en parallèle : 6/6 chacune.
//!
//! Le flake reste **inexpliqué** et rare (deux occurrences sur six passes
//! complètes) ; il est nommé dans `docs/GARANTIES-NON-REGRESSION.md` §4. Ce que
//! cette sonde garantit, elle, c'est que la libération n'est pas *différée* :
//! une régression du genre « le mmap retient la description, donc le `flock` »
//! ferait tomber l'assertion ci-dessous du premier coup.

use obrain_substrate::wal_io::SyncMode;
use obrain_substrate::{SubstrateFile, Writer, simplex_flags, simplex_kind};

/// Combien de descripteurs du processus pointent encore sur ce store ?
///
/// Lecture de `/proc/self/fd` : c'est l'observation directe du mécanisme
/// (mapping qui retient la description, cache qui garde un fichier ouvert…),
/// pas une déduction.
fn descripteurs_du_store(chemin: &std::path::Path) -> usize {
    let Ok(entrees) = std::fs::read_dir("/proc/self/fd") else {
        return 0;
    };
    let prefixe = chemin.to_string_lossy().to_string();
    let mut n = 0;
    for e in entrees.flatten() {
        if let Ok(cible) = std::fs::read_link(e.path()) {
            let c = cible.to_string_lossy().to_string();
            if c.starts_with(&prefixe) || c.contains(&format!("{} (deleted)", prefixe)) {
                n += 1;
            }
        }
    }
    n
}

/// ── UN REFUS DE VERROU NOMME SON DÉTENTEUR ──────────────────────────────────
///
/// Deux exécutions d'une suite complète sont tombées sur un refus de verrou
/// (`simplex_durability`) sans qu'on puisse dire QUI tenait le `flock`. Un refus
/// qu'on ne peut pas instruire coûte une enquête à chaque occurrence : le message
/// nomme donc le détenteur, lu dans `/proc` au moment du refus.
///
/// Le refus n'existe QU'ENTRE PROCESSUS (dans un même processus, le verrou est
/// partagé — c'est la conception) : le test lance donc un ENFANT qui tient le
/// verrou, puis tente d'ouvrir la même base.
#[test]
fn un_refus_de_verrou_nomme_le_detenteur() {
    // Corps de l'enfant : tenir le verrou et attendre.
    if std::env::var("SONDE_TIENT_LE_VERROU").is_ok() {
        let chemin = std::env::var("SONDE_CHEMIN").expect("SONDE_CHEMIN");
        let _verrou = SubstrateFile::create(&chemin).expect("l'enfant prend le verrou");
        println!("PRET");
        use std::io::Write as _;
        std::io::stdout().flush().ok();
        std::thread::sleep(std::time::Duration::from_secs(8));
        return; // garde vivante jusqu'ici
    }

    let td = tempfile::tempdir().expect("tempdir");
    let chemin = td.path().join("kb");
    let moi = std::env::current_exe().expect("current_exe");
    let mut enfant = std::process::Command::new(moi)
        .args([
            "--exact",
            "un_refus_de_verrou_nomme_le_detenteur",
            "--nocapture",
        ])
        .env("SONDE_TIENT_LE_VERROU", "1")
        .env("SONDE_CHEMIN", &chemin)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("lancer l'enfant");

    // Attendre que l'enfant TIENNE le verrou (il l'annonce).
    {
        use std::io::{BufRead as _, BufReader};
        let sortie = enfant.stdout.take().expect("stdout");
        let mut lignes = BufReader::new(sortie).lines();
        let mut pret = false;
        for _ in 0..40 {
            match lignes.next() {
                Some(Ok(l)) if l.contains("PRET") => {
                    pret = true;
                    break;
                }
                Some(Ok(_)) => continue,
                _ => break,
            }
        }
        assert!(pret, "l'enfant devait annoncer qu'il tient le verrou");
    }

    let refus = SubstrateFile::open(&chemin).expect_err("un AUTRE processus tient le verrou");
    let message = format!("{refus}");
    let pid_enfant = enfant.id();
    let _ = enfant.kill();
    let _ = enfant.wait();

    assert!(
        message.contains(&format!("pid {pid_enfant}")),
        "le refus doit NOMMER le détenteur (pid {pid_enfant} attendu) : {message}"
    );
}

#[test]
fn le_verrou_est_rendu_immediatement_apres_le_drop() {
    let iterations = 60usize;
    let td_dernier: std::sync::Mutex<std::path::PathBuf> =
        std::sync::Mutex::new(std::path::PathBuf::new());
    let mut echecs_immediats = 0usize;
    let mut pire_attente_us = 0u128;

    for i in 0..iterations {
        let td = tempfile::tempdir().expect("tempdir");
        let chemin = td.path().join("kb");
        *td_dernier.lock().unwrap_or_else(|e| e.into_inner()) = chemin.clone();
        {
            let sub = SubstrateFile::create(&chemin).expect("create");
            let w = Writer::new(sub, SyncMode::EveryCommit).expect("writer");
            // Une écriture RÉELLE (des simplexes) : c'est ce qui fait vivre les
            // zones mmap et les zones nommées du store.
            for i in 0..8u32 {
                let base = i * 10;
                let _ = w.upsert_simplex(
                    simplex_kind::TRIANGLE,
                    0,
                    if i % 3 == 0 { simplex_flags::CYCLIC } else { 0 },
                    &[base + 2, base, base + 1],
                    &[],
                );
            }
        }
        // Drop fait. On retente l'ouverture en mesurant l'attente.
        let debut = web_time::Instant::now();
        let mut essais = 0u32;
        loop {
            match SubstrateFile::open(&chemin) {
                Ok(_sub) => {
                    let attente = debut.elapsed().as_micros();
                    if attente > pire_attente_us {
                        pire_attente_us = attente;
                    }
                    if essais > 0 {
                        eprintln!(
                            "  itération {i} : ouvert après {essais} refus (attente {attente} µs)"
                        );
                    }
                    break;
                }
                Err(_) => {
                    essais += 1;
                    if essais == 1 {
                        echecs_immediats += 1;
                    }
                    if essais > 200 {
                        panic!("itération {i} : verrou toujours tenu après 200 essais");
                    }
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            }
        }
    }
    eprintln!(
        "verrou après drop : {echecs_immediats}/{iterations} ouvertures immédiates refusées, \
         pire attente {pire_attente_us} µs"
    );
    // ── LA SONDE REGARDE AUSSI LES DESCRIPTEURS ────────────────────────────
    // Le `flock` vit sur une *description de fichier* : il ne part que quand
    // TOUS les descripteurs de cette description sont fermés. Un mapping mmap
    // garde une référence à la description — donc un store dont le `Drop` ne
    // démonte pas ses zones laisserait le verrou en place, et toute
    // réouverture immédiate échouerait. C'est la cause soupçonnée du flake de
    // `simplex_durability` ; ce comptage dit si elle est réelle.
    let chemin = td_dernier.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let restants = descripteurs_du_store(&chemin);
    eprintln!("descripteurs encore ouverts sur le store après drop : {restants}");
    assert_eq!(
        restants, 0,
        "après le drop du dernier porteur, plus AUCUN descripteur ne doit rester \
         sur le store (sinon le flock survit, et la réouverture immédiate échoue)"
    );
    assert_eq!(
        echecs_immediats, 0,
        "le verrou exclusif doit être rendu SYNCHRONEMENT au drop : \
         {echecs_immediats} ouvertures immédiates refusées sur {iterations}"
    );
}
