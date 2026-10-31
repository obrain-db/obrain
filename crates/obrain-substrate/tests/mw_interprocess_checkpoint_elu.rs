//! ── PHASE B — LES DEUX TESTS QUI EXIGENT LES SEAMS NEUFS DE LA PHASE ───────
//!
//! Ils vivent dans leur propre fichier pour une raison PRÉCISE : ce fichier ne
//! compile QUE contre le code d'après la phase (seam `forcer_pause_apres_ecriture`
//! et type `ElectionCheckpoint`). `mw_interprocess_phase_b.rs`, lui, n'utilise que
//! des API PUBLIQUES ANTÉRIEURES : c'est ce qui permet d'exécuter la preuve
//! ROUGE (source remise à l'état d'avant, tests inchangés) sans qu'un échec de
//! compilation ne vienne se substituer à l'échec de l'invariant.

use std::io::{BufRead as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use obrain_substrate::wal::{WalPayload, WalRecord};
use obrain_substrate::wal_io::{
    ANCRE_INITIALE, CoordinationAppend, SyncMode, WalReader, WalWriter,
};

fn role() -> Option<String> {
    std::env::var("MWB_ROLE").ok()
}

fn env_obligatoire(cle: &str) -> String {
    std::env::var(cle).unwrap_or_else(|_| panic!("{cle} manquant dans l'enfant"))
}

fn annoncer(ligne: &str) {
    println!("{ligne}");
    std::io::stdout().flush().ok();
}

fn attendre_fichier(p: &Path, quoi: &str) {
    let debut = Instant::now();
    while !p.exists() {
        assert!(
            debut.elapsed() < Duration::from_secs(30),
            "attente de {quoi} ({}) : délai dépassé",
            p.display()
        );
        std::thread::sleep(Duration::from_micros(200));
    }
}

struct Enfant {
    process: Child,
    lignes: std::io::Lines<std::io::BufReader<std::process::ChildStdout>>,
}

impl Enfant {
    fn lancer(test: &str, vars: &[(&str, String)]) -> Self {
        let moi = std::env::current_exe().expect("current_exe");
        let mut cmd = Command::new(moi);
        cmd.args(["--exact", test, "--nocapture"])
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        for (k, v) in vars {
            cmd.env(k, v);
        }
        let mut process = cmd.spawn().expect("lancer l'enfant");
        let sortie = process.stdout.take().expect("stdout de l'enfant");
        Self {
            process,
            lignes: std::io::BufReader::new(sortie).lines(),
        }
    }

    fn attendre_ligne(&mut self, prefixe: &str) -> String {
        loop {
            match self.lignes.next() {
                Some(Ok(l)) => {
                    if let Some(reste) = l.strip_prefix(prefixe) {
                        return reste.trim().to_string();
                    }
                }
                autre => panic!("enfant terminé sans « {prefixe} » ({autre:?})"),
            }
        }
    }

    fn terminer(mut self) {
        let statut = self.process.wait().expect("attendre l'enfant");
        assert!(statut.success(), "l'enfant a échoué : {statut:?}");
    }
}

/// Relit un WAL : `(clés PropSetInline, trou?, erreurs, lsn_min)`.
fn auditer_wal(wal: &Path) -> (Vec<String>, Option<(u64, u64)>, usize, u64) {
    let r = WalReader::open(wal).expect("ouvrir le WAL");
    let mut cles = Vec::new();
    let mut erreurs = 0usize;
    let mut min = u64::MAX;
    let mut it = r.iter_from(0);
    while let Some(item) = it.next() {
        match item {
            Ok((rec, _, _)) => {
                min = min.min(rec.lsn);
                if let WalPayload::PropSetInline { key, .. } = rec.payload {
                    cles.push(key);
                }
            }
            Err(_) => {
                erreurs += 1;
                break;
            }
        }
    }
    (cles, it.trou(), erreurs, min)
}

/// Enfant « écrivain avec fenêtre ouverte » : pose UNE frame dans le WAL, mais
/// s'endort ENTRE le `write` et la lecture de la taille qui suit (seam
/// `forcer_pause_apres_ecriture`). Pendant ce sommeil, le parent tronque le WAL —
/// l'entrelacement exact que la pièce 1 doit rendre récupérable.
fn enfant_temoin_fenetre(wal: &Path, drapeau: &Path) -> ! {
    let w = WalWriter::open(wal, SyncMode::Never, ANCRE_INITIALE).expect("ouvrir le WAL");
    w.coordination(CoordinationAppend::OAppend);
    // Fenêtre de 700 ms : le parent a le temps de tronquer APRÈS l'écriture et
    // AVANT l'observation. Aucune course réelle n'est assez large pour être
    // reproductible ; c'est précisément pourquoi le seam existe.
    w.forcer_pause_apres_ecriture(Duration::from_millis(700));
    let _ = std::fs::write(drapeau, b"go");
    // La frame porte une clé UNIQUE : le parent doit la retrouver, ou constater
    // qu'elle a disparu. C'est la perte, ou sa réparation, qui est mesurée.
    let rec = WalRecord {
        lsn: 0,
        timestamp: 0,
        flags: WalRecord::FLAG_COMMIT,
        payload: WalPayload::PropSetInline {
            node_id: 1,
            key: "phase-b-temoin".to_string(),
            value_bincode: vec![1, 2, 3, 4],
        },
    };
    let (pos, len) = w.append(rec).expect("append du témoin");
    annoncer(&format!(
        "APPEND pos={pos} len={len} troncatures={} reprises={} offset={}",
        w.troncatures_subies(),
        w.reprises_apres_troncature(),
        w.offset()
    ));
    std::process::exit(0);
}

// ===========================================================================
// T2 — UN CHECKPOINT D'UN PROCESSUS NE PERT PAS L'APPEND DE L'AUTRE (pièce 1)
// ===========================================================================
//
// ── LA FENÊTRE, RENDUE REPRODUCTIBLE ────────────────────────────────────────
// L'enfant écrit une frame, puis S'ENDORT (seam) entre le `write` et la lecture
// de la taille. Le parent tronque le WAL pendant ce sommeil — c'est exactement ce
// que fait un checkpoint élu entre l'avance d'ancre et la troncature.
//
// Avant la pièce 1 : la garde détectait bien que la taille avait diminué, mais
// elle se contentait de RECALER le curseur. La frame, elle, n'était jamais
// réécrite : `append` rendait `Ok`, et l'enregistrement n'existait plus nulle
// part. Après : la frame est EMPORTÉE (`taille_apres < fin`), l'ancre est relue,
// la position recalée, la frame RÉÉCRITE — et le témoin est retrouvé dans le WAL.
#[test]
fn t2_checkpoint_d_un_processus_ne_perd_pas_l_append_de_l_autre() {
    if let Some(r) = role() {
        let wal = PathBuf::from(env_obligatoire("MWB_WAL"));
        let drapeau = PathBuf::from(env_obligatoire("MWB_DRAPEAU"));
        match r.as_str() {
            "temoin" => enfant_temoin_fenetre(&wal, &drapeau),
            autre => panic!("rôle inconnu {autre}"),
        }
    }
    let td = tempfile::tempdir().unwrap();
    let wal = td.path().join("substrate.wal");
    let drapeau = td.path().join("enfant-en-fenetre");

    // Un WAL non vide : la troncature déplace donc réellement l'ancre, et une
    // frame emportée l'est par une troncature de taille NON NULLE.
    {
        let w = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).unwrap();
        for _ in 0..40u64 {
            w.append(WalRecord {
                lsn: 0,
                timestamp: 0,
                flags: WalRecord::FLAG_COMMIT,
                payload: WalPayload::NoOp,
            })
            .unwrap();
        }
    }

    let mut enfant = Enfant::lancer(
        "t2_checkpoint_d_un_processus_ne_perd_pas_l_append_de_l_autre",
        &[
            ("MWB_ROLE", "temoin".into()),
            ("MWB_WAL", wal.display().to_string()),
            ("MWB_DRAPEAU", drapeau.display().to_string()),
        ],
    );
    attendre_fichier(&drapeau, "l'enfant en fenêtre");
    // L'enfant a écrit la frame PUIS s'est endormi 700 ms. On laisse passer
    // ~150 ms pour être certain que le `write` a eu lieu, puis on tronque.
    std::thread::sleep(Duration::from_millis(150));

    // ── LE CHECKPOINT D'UN PAIR, RÉDUIT À SA PART WAL ──────────────────────
    // `truncate()` publie l'ancre AVANT de tronquer (ordre invariant de la
    // pièce 4 de l'axe append), puis on repose le marqueur et on scelle le
    // plancher : c'est exactement la séquence de `checkpoint::checkpoint`.
    let pair = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).unwrap();
    pair.coordination(CoordinationAppend::OAppend);
    let ancre_avant = pair.ancre();
    pair.truncate().expect("troncature du pair");
    let at_lsn = pair.peek_lsn();
    pair.append(WalRecord {
        lsn: 0,
        timestamp: 0,
        flags: WalRecord::FLAG_CHECKPOINT | WalRecord::FLAG_COMMIT,
        payload: WalPayload::Checkpoint { at_lsn },
    })
    .expect("marqueur de checkpoint");
    pair.sceller_segment().expect("sceller le segment");
    let ancre_apres = pair.ancre();
    assert!(
        ancre_apres > ancre_avant,
        "la troncature doit avoir avancé l'ancre (sinon le test ne prouve rien)"
    );

    let ligne = enfant.attendre_ligne("APPEND");
    enfant.terminer();
    // ── LA PREUVE POSITIVE QUE LE CHEMIN A SERVI ───────────────────────────
    // Un test qui ne ferait que constater « la frame est là » passerait aussi
    // sans la reprise (si le parent avait tronqué trop tard, la frame ne serait
    // jamais partie). Les deux compteurs de l'enfant disent que la fenêtre a bien
    // été ouverte ET que la reprise a bien tiré : `troncatures_subies >= 1`
    // (la frame a été EMPORTÉE) et `reprises_apres_troncature >= 1` (elle a été
    // réécrite). Sans ces deux-là, le test serait vert pour la mauvaise raison.
    let champs: std::collections::HashMap<&str, u64> = ligne
        .split_whitespace()
        .filter_map(|c| c.split_once('='))
        .filter_map(|(k, v)| v.parse::<u64>().ok().map(|v| (k, v)))
        .collect();
    assert!(
        champs.get("troncatures").copied().unwrap_or(0) >= 1,
        "l'enfant n'a SUBI aucune troncature : la fenêtre n'a pas été ouverte, \
         le test ne prouve rien (ligne = {ligne:?})"
    );
    assert!(
        champs.get("reprises").copied().unwrap_or(0) >= 1,
        "la frame a été emportée SANS être reprise (ligne = {ligne:?}) — c'est \
         exactement la perte que la pièce 1 devait fermer"
    );

    let (cles, trou, erreurs, lsn_min) = auditer_wal(&wal);
    assert!(
        trou.is_none(),
        "le WAL porte un TROU de zéros {:?} — c'est le défaut même que la phase \
         devait fermer (une annulation d'écriture courte qui ALLONGE le fichier, \
         ou une écriture à un offset qui n'existe plus)",
        trou
    );
    assert_eq!(erreurs, 0, "aucune frame illisible après le checkpoint");
    assert!(
        cles.iter().any(|k| k == "phase-b-temoin"),
        "LA FRAME DU TÉMOIN A DISPARU : `append` a rendu Ok ({ligne}), et \
         l'enregistrement n'existe plus dans le WAL ({} clés relues). C'est \
         exactement la perte que la pièce 1 devait rendre récupérable",
        cles.len()
    );
    assert!(
        lsn_min >= ancre_apres,
        "aucune frame ne peut porter un LSN ANTÉRIEUR à la base du segment \
         courant (lsn_min={lsn_min}, ancre={ancre_apres}) — un LSN plus bas \
         signifie que la frame a été estampillée avec l'ancre du segment \
         PRÉCÉDENT"
    );
}

// ===========================================================================
// T5 — UN SEUL CHECKPOINTEUR À LA FOIS (pièce 1, élection)
// ===========================================================================
//
// Le fence `<wal>.chk` est un `flock` exclusif non bloquant : EXACTEMENT un
// détenteur. Deux tentatives concurrentes ne peuvent pas toutes deux réussir, et
// c'est ce qui empêche deux processus d'avancer chacun l'ancre de SA mesure.
#[test]
fn t5_un_seul_checkpointeur_a_la_fois() {
    use obrain_substrate::wal_io::{ElectionCheckpoint, chemin_fence_checkpoint};
    let td = tempfile::tempdir().unwrap();
    let wal = td.path().join("substrate.wal");
    let _ = WalWriter::open(&wal, SyncMode::Never, ANCRE_INITIALE).unwrap();

    let a = ElectionCheckpoint::tenter(&wal).expect("le premier doit être élu");
    assert!(a.elu());
    let b = ElectionCheckpoint::tenter(&wal);
    assert!(
        b.is_none(),
        "DEUX CHECKPOINTEURS ONT ÉTÉ ÉLUS EN MÊME TEMPS : rien n'empêcherait \
         alors deux avances d'ancre concurrentes, donc deux LSN recouverts"
    );
    assert_eq!(a.chemin(), chemin_fence_checkpoint(&wal).as_path());

    // Le fence est rendu AVEC l'objet : le suivant est élu.
    drop(a);
    let c = ElectionCheckpoint::tenter(&wal);
    assert!(
        c.is_some(),
        "le fence n'est pas rendu à la mort de son détenteur : la base ne \
         pourrait plus JAMAIS être checkpointée"
    );
}
