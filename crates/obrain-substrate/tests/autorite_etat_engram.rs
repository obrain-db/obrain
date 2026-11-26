//! (#414, périmètre B — AUTORITÉ DU SUBSTRAT) L'ÉTAT D'UN ENGRAMME : ce qui est
//! durable, ce qui se rejoue, et ce qui ne doit jamais arriver.
//!
//! `Writer::set_engram_state` écrit le couple (membres, poids) d'un engramme
//! avec le WAL pour AUTORITÉ et les zones mmap pour DÉRIVÉ. Ces bancs mesurent
//! ce que cette phrase implique, et rien de moins :
//!
//!   1. un REFUS ne laisse aucune trace durable — ni record, ni mutation (y
//!      compris l'engramme 0, réservé : un record pour lui rendrait la base
//!      INOUVRABLE, puisque le rejeu le refuse) ;
//!   2. une panne DANS LA FENÊTRE (entre le record et les zones, ou entre les
//!      deux zones) rend après réouverture le couple COMPLET du nouvel état,
//!      jamais « membres neufs + poids anciens » ;
//!   3. l'état RECONSTRUIT égale l'état ÉCRIT, même les deux zones effacées :
//!      le journal suffit ;
//!   4. le rejeu est IDEMPOTENT (états absolus, rejouables deux fois de suite) ;
//!   5. l'ordre du journal est l'ordre d'application (le dernier état gagne) ;
//!   6. une suppression est un état comme un autre : durable, rejouable, et
//!      elle ne ressuscite pas.
//!
//! PHYSIQUE DE LA PANNE. Le point d'injection `panne::point_etat_engram` appelle
//! `abort()` : c'est un SIGKILL, pas une coupure de courant. Le noyau garde le
//! cache de pages, donc tout octet déjà écrit survit — c'est ce qui permet de
//! viser la FENÊTRE exacte entre deux écritures au lieu de la viser au hasard
//! avec un `kill -9` chronométré. La perte des écritures NON durables (ENOSPC,
//! coupure de courant) est un autre scénario, couvert par la troncature du WAL
//! dans `crash_safety.rs` et `recovery_synapse.rs` : il n'est pas mesuré ici.
//!
//! Run: cargo test -p obrain-substrate --test autorite_etat_engram

use std::path::{Path, PathBuf};
use std::time::Duration;

use obrain_substrate::replay::replay_from;
use obrain_substrate::wal::WalPayload;
use obrain_substrate::wal_io::{SyncMode, WalReader};
use obrain_substrate::zone;
use obrain_substrate::{SubstrateError, SubstrateFile, Writer};

/// Le couple tel que l'API le rend : membres, puis poids (`None` si la zone des
/// poids n'a jamais été initialisée — base legacy).
type Etat = Option<(Vec<u32>, Option<Vec<f32>>)>;

/// Records `EngramStateSet` du journal pour `engram_id`, DANS L'ORDRE ÉCRIT.
fn records_etat(wal: &Path, engram_id: u16) -> Vec<(Vec<u32>, Vec<u32>)> {
    WalReader::open(wal)
        .expect("relire le WAL")
        .iter_from(0)
        .filter_map(Result::ok)
        .filter_map(|(rec, _, _)| match rec.payload {
            WalPayload::EngramStateSet {
                engram_id: id,
                members,
                weights,
            } if id == engram_id => Some((members, weights)),
            _ => None,
        })
        .collect()
}

/// Trames du journal qui désignent l'engramme 0 (réservé), quel que soit leur
/// type : c'est la trace durable qu'un refus ne doit jamais laisser.
fn records_engramme_nul(wal: &Path) -> Vec<&'static str> {
    WalReader::open(wal)
        .expect("relire le WAL")
        .iter_from(0)
        .filter_map(Result::ok)
        .filter_map(|(rec, _, _)| match rec.payload {
            WalPayload::EngramMembersSet { engram_id: 0, .. } => Some("EngramMembersSet"),
            WalPayload::EngramWeightsSet { engram_id: 0, .. } => Some("EngramWeightsSet"),
            WalPayload::EngramStateSet { engram_id: 0, .. } => Some("EngramStateSet"),
            _ => None,
        })
        .collect()
}

/// Efface les DEUX zones de l'état d'engramme : le cas « le dérivé a disparu,
/// le journal doit suffire ».
fn effacer_zones_engram(base: &Path) {
    std::fs::write(base.join(zone::ENGRAM_MEMBERS), Vec::<u8>::new()).expect("vider la zone membres");
    std::fs::write(base.join(zone::ENGRAM_WEIGHTS), Vec::<u8>::new()).expect("vider la zone poids");
}

/// Ce que les zones contiennent VRAIMENT, sans rejeu : le dérivé seul.
fn etat_brut(base: &Path, engram_id: u16) -> Etat {
    let sub = SubstrateFile::open(base).expect("ouvrir");
    let w = Writer::new(sub, SyncMode::Never).expect("writer");
    w.read_engram_state(engram_id).expect("lecture brute")
}

/// Ouvre, REJOUE le journal depuis 0, et rend l'état reconstruit.
fn etat_reconstruit(base: &Path, engram_id: u16) -> Etat {
    let sub = SubstrateFile::open(base).expect("ouvrir");
    let stats = replay_from(&sub, 0).expect("rejeu");
    assert_eq!(stats.decode_errors, 0, "aucune frame indécodable");
    let w = Writer::new(sub, SyncMode::Never).expect("writer");
    w.read_engram_state(engram_id).expect("lecture après rejeu")
}

// ===========================================================================
// 1. UN REFUS NE LAISSE AUCUNE TRACE DURABLE
// ===========================================================================

/// Membres et poids en désaccord : la mutation est refusée AVANT toute
/// écriture. Un refus qui journalise ou qui mute les zones serait un demi-état
/// créé par une opération qui n'a jamais eu lieu.
#[test]
fn un_refus_ne_laisse_aucune_trace_durable() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("kb");
    let sub = SubstrateFile::create(&base).unwrap();
    let wal = sub.wal_path();
    let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();

    // État de départ, DURABLE.
    w.set_engram_state(7, vec![10, 20, 30], vec![0.25, 0.5, 0.25])
        .unwrap();
    w.commit().unwrap();
    w.wal().fsync().unwrap();
    let avant = w.read_engram_state(7).unwrap();
    assert_eq!(
        avant,
        Some((vec![10, 20, 30], Some(vec![0.25, 0.5, 0.25]))),
        "état de départ"
    );
    let records_avant = records_etat(&wal, 7).len();

    // REFUS : deux membres, un seul poids.
    let err = w.set_engram_state(7, vec![1, 2], vec![0.9]).unwrap_err();
    assert!(
        matches!(err, SubstrateError::Internal(_)),
        "erreur de mutation attendue, obtenu {err:?}"
    );

    // (a) Rien au journal.
    assert_eq!(
        records_etat(&wal, 7).len(),
        records_avant,
        "un refus ne doit rien journaliser"
    );
    // (b) Rien dans les zones.
    assert_eq!(
        w.read_engram_state(7).unwrap(),
        avant,
        "un refus ne doit pas muter l'état"
    );
    w.wal().fsync().unwrap();
    drop(w);

    // (c) Et la réouverture rend le même état : le refus n'a rien laissé.
    assert_eq!(etat_reconstruit(&base, 7), avant);
}

/// L'engramme 0 est RÉSERVÉ (null engram). Le refus existe déjà dans
/// `EngramZone::set_members_raw`, mais il tombe APRÈS l'ajout du record au
/// journal : le record `EngramStateSet { engram_id: 0 }` reste durable, et le
/// rejeu le refuse à son tour — la base devient INOUVRABLE pour une mutation
/// qui n'a jamais eu lieu. L'autorité du substrat interdit exactement ça : une
/// opération refusée ne laisse aucune trace durable.
#[test]
fn un_engramme_nul_refuse_ne_bloque_pas_la_reouverture() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("kb");
    let sub = SubstrateFile::create(&base).unwrap();
    let wal = sub.wal_path();
    let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();

    w.set_engram_state(5, vec![1, 2], vec![0.5, 0.5]).unwrap();
    w.commit().unwrap();
    w.wal().fsync().unwrap();

    // LES TROIS PORTES de l'état d'engramme refusent l'identifiant nul.
    let refus = [
        ("set_engram_state", w.set_engram_state(0, vec![3], vec![0.75]).err()),
        ("set_engram_members", w.set_engram_members(0, vec![3]).err()),
        ("set_engram_weights", w.set_engram_weights(0, vec![0.75]).err()),
    ];
    for (porte, err) in refus {
        let err = err.unwrap_or_else(|| panic!("{porte} doit refuser l'engramme 0"));
        assert!(
            matches!(err, SubstrateError::WalBadFrame(_)),
            "{porte} : refus attendu pour l'engramme 0, obtenu {err:?}"
        );
    }
    w.wal().fsync().unwrap();

    assert!(
        records_engramme_nul(&wal).is_empty(),
        "un refus ne doit pas journaliser : un record pour l'engramme 0 rend la base inouvrable (obtenu {:?})",
        records_engramme_nul(&wal)
    );
    drop(w);

    // La base se rouvre, et l'état légitime est intact.
    let sub2 = SubstrateFile::open(&base).unwrap();
    let stats = replay_from(&sub2, 0).expect("le rejeu d'un journal sain ne doit pas refuser");
    assert_eq!(stats.decode_errors, 0);
    let w2 = Writer::new(sub2, SyncMode::Never).unwrap();
    assert_eq!(
        w2.read_engram_state(5).unwrap(),
        Some((vec![1, 2], Some(vec![0.5, 0.5]))),
        "l'état d'un autre engramme survit au refus"
    );
}

/// La CONSÉQUENCE, mesurée : un journal qui PORTE un état d'engramme 0 fait
/// REFUSER le rejeu (`set_members_raw` rejette l'identifiant nul). C'est ce qui
/// rend l'invariant du banc précédent nécessaire : sans lui, une mutation
/// refusée laisse la base inouvrable. Le record est forgé ici à la main,
/// exactement comme l'ancien chemin en produisait un avant de refuser.
#[test]
fn un_journal_portant_l_engramme_nul_fait_refuser_le_rejeu() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("kb");
    let sub = SubstrateFile::create(&base).unwrap();
    let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();
    w.set_engram_state(5, vec![1], vec![0.5]).unwrap();
    w.wal()
        .append(obrain_substrate::wal::WalRecord {
            lsn: 0,
            timestamp: 0,
            flags: 0,
            payload: WalPayload::EngramStateSet {
                engram_id: 0,
                members: vec![3],
                weights: vec![0.75f32.to_bits()],
            },
        })
        .unwrap();
    w.wal().fsync().unwrap();
    drop(w);

    let sub2 = SubstrateFile::open(&base).unwrap();
    match replay_from(&sub2, 0) {
        Err(SubstrateError::WalBadFrame(m)) => {
            assert!(m.contains("reserved"), "refus nommé, message : {m}");
        }
        Err(autre) => panic!("refus attendu (WalBadFrame), obtenu {autre:?}"),
        Ok(_) => panic!("le rejeu devait REFUSER un état d'engramme 0 — une base inouvrable"),
    }
}

// ===========================================================================
// 2. ÉTAT ÉCRIT == ÉTAT RECONSTRUIT, ET LE REJEU EST IDEMPOTENT
// ===========================================================================

/// Trois engrammes, dont un supprimé : les deux zones effacées, le journal seul
/// doit rendre EXACTEMENT ce qui avait été écrit — et une SECONDE passe de rejeu
/// ne doit rien changer (les records portent des états ABSOLUS, pas des deltas).
#[test]
fn l_etat_reconstruit_egale_l_etat_ecrit_et_se_rejoue_deux_fois() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("kb");
    let sub = SubstrateFile::create(&base).unwrap();
    let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();

    w.set_engram_state(11, vec![1, 2, 3], vec![0.1, 0.2, 0.7])
        .unwrap();
    w.set_engram_state(12, vec![42], vec![1.0]).unwrap();
    w.set_engram_state(13, vec![7, 8], vec![0.5, 0.5]).unwrap();
    // Suppression : un état comme un autre (([], [])).
    w.set_engram_state(13, Vec::new(), Vec::new()).unwrap();
    w.commit().unwrap();
    w.wal().fsync().unwrap();

    let ecrit: Vec<Etat> = [11u16, 12, 13]
        .iter()
        .map(|&id| w.read_engram_state(id).unwrap())
        .collect();
    assert_eq!(
        ecrit,
        vec![
            Some((vec![1, 2, 3], Some(vec![0.1, 0.2, 0.7]))),
            Some((vec![42], Some(vec![1.0]))),
            None,
        ],
        "état écrit"
    );

    // Le dérivé disparaît ; seul le journal reste.
    w.msync_zones().unwrap();
    w.wal().fsync().unwrap();
    drop(w);
    effacer_zones_engram(&base);

    let sub2 = SubstrateFile::open(&base).unwrap();
    let stats = replay_from(&sub2, 0).unwrap();
    assert_eq!(stats.decode_errors, 0);
    let w2 = Writer::new(sub2, SyncMode::Never).unwrap();
    let reconstruit: Vec<Etat> = [11u16, 12, 13]
        .iter()
        .map(|&id| w2.read_engram_state(id).unwrap())
        .collect();
    assert_eq!(reconstruit, ecrit, "l'état RECONSTRUIT égale l'état ÉCRIT");

    // IDEMPOTENCE : une seconde passe de rejeu sur la même base.
    {
        let sub_arc = w2.substrate();
        let garde = sub_arc.lock();
        let stats2 = replay_from(&garde, 0).expect("second rejeu");
        assert_eq!(stats2.decode_errors, 0);
    }
    let rejoue_deux_fois: Vec<Etat> = [11u16, 12, 13]
        .iter()
        .map(|&id| w2.read_engram_state(id).unwrap())
        .collect();
    assert_eq!(
        rejoue_deux_fois, ecrit,
        "rejouer deux fois ne change rien : les records sont des états absolus"
    );
}

// ===========================================================================
// 3. L'ORDRE DU JOURNAL EST L'ORDRE D'APPLICATION
// ===========================================================================

/// Les records de types DIFFÉRENTS s'appliquent dans l'ordre du journal : un
/// `EngramWeightsSet` écrit APRÈS un `EngramStateSet` a le dernier mot sur les
/// poids. Le premier record est en plus un `EngramWeightsSet` seul, ce qui
/// ouvre la zone des poids avant la zone des membres : les deux ouvertures
/// paresseuses du rejeu sont donc prises dans un ordre différent.
#[test]
fn l_ordre_du_journal_est_l_ordre_d_application() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("kb");
    let sub = SubstrateFile::create(&base).unwrap();
    let wal = sub.wal_path();
    let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();

    // 1. Des POIDS seuls pour 20 (ouvre la zone des poids la première au rejeu).
    w.set_engram_weights(20, vec![0.1, 0.2]).unwrap();
    // 2. L'état COMPLET de 9.
    w.set_engram_state(9, vec![3, 4], vec![0.3, 0.7]).unwrap();
    // 3. Des poids NEUFS pour 9 : le DERNIER mot.
    w.set_engram_weights(9, vec![0.9, 0.1]).unwrap();
    w.commit().unwrap();
    w.wal().fsync().unwrap();

    let ecrit_9 = w.read_engram_state(9).unwrap();
    let ecrit_20 = w.read_engram_state(20).unwrap();
    assert_eq!(
        ecrit_9,
        Some((vec![3, 4], Some(vec![0.9, 0.1]))),
        "les poids du 3e record écrasent ceux du 2e"
    );

    // Le journal porte UN record d'état pour 9, et il est bien celui du 2e
    // appel (poids 0,3 / 0,7) — les poids neufs sont dans un autre record.
    let etats_9 = records_etat(&wal, 9);
    assert_eq!(etats_9.len(), 1, "un seul EngramStateSet pour 9");
    assert_eq!(
        etats_9[0],
        (vec![3u32, 4], vec![0.3f32.to_bits(), 0.7f32.to_bits()]),
        "le record d'état porte les poids du moment, pas ceux d'après"
    );

    w.msync_zones().unwrap();
    w.wal().fsync().unwrap();
    drop(w);
    effacer_zones_engram(&base);

    let sub2 = SubstrateFile::open(&base).unwrap();
    replay_from(&sub2, 0).unwrap();
    let w2 = Writer::new(sub2, SyncMode::Never).unwrap();
    assert_eq!(
        w2.read_engram_state(9).unwrap(),
        ecrit_9,
        "l'ordre du journal reconstruit le même état final"
    );
    assert_eq!(w2.read_engram_state(20).unwrap(), ecrit_20);
}

// ===========================================================================
// 4. UNE SUPPRESSION EST UN ÉTAT DURABLE
// ===========================================================================

/// `set_engram_state(id, [], [])` vide les DEUX zones (pas une seule) et se
/// rejoue : une suppression effacée du dérivé ne ressuscite pas l'état d'avant.
#[test]
fn une_suppression_est_un_etat_durable_qui_ne_ressuscite_pas() {
    let td = tempfile::tempdir().unwrap();
    let base = td.path().join("kb");
    let sub = SubstrateFile::create(&base).unwrap();
    let wal = sub.wal_path();
    let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();

    w.set_engram_state(30, vec![5, 6], vec![0.5, 0.5]).unwrap();
    w.set_engram_state(30, Vec::new(), Vec::new()).unwrap();
    w.commit().unwrap();
    w.wal().fsync().unwrap();

    assert_eq!(w.engram_members(30).unwrap(), None, "membres vidés");
    assert_eq!(
        w.engram_weights(30).unwrap(),
        None,
        "poids vidés — les DEUX zones, pas une"
    );
    assert_eq!(w.read_engram_state(30).unwrap(), None);

    let records = records_etat(&wal, 30);
    assert_eq!(
        records.len(),
        2,
        "deux records : l'état, puis le vide — dans cet ordre"
    );
    assert_eq!(records[1], (Vec::new(), Vec::new()), "le dernier dit vide");

    w.msync_zones().unwrap();
    w.wal().fsync().unwrap();
    drop(w);
    effacer_zones_engram(&base);
    assert_eq!(
        etat_reconstruit(&base, 30),
        None,
        "une suppression rejouée reste une suppression"
    );
}

// ===========================================================================
// 5. LE COUPLE EST INDIVISIBLE SOUS PANNE DANS LA FENÊTRE
// ===========================================================================

const ENV_ENFANT: &str = "OBRAIN_TEST_ETAT_ENGRAM_ENFANT";
const ENV_RANG: &str = "OBRAIN_TEST_ETAT_ENGRAM_RANG";

/// Enfant : écrit l'état A (durable), pose `pret`, arme l'arrêt au point
/// `rang`, puis demande l'état B — et meurt dans la fenêtre.
#[test]
fn enfant_ecrit_l_etat_puis_meurt_dans_la_fenetre() {
    let Ok(dir) = std::env::var(ENV_ENFANT) else {
        return;
    };
    let rang: u64 = std::env::var(ENV_RANG)
        .expect("rang transmis par le parent")
        .parse()
        .expect("rang entier");
    let dir = PathBuf::from(dir);
    let base = dir.join("kb");

    let sub = SubstrateFile::create(&base).expect("create");
    let w = Writer::new(sub, SyncMode::EveryCommit).expect("writer");
    // État A, DURABLE : zones msync + journal fsync. La panne ne peut pas le
    // perdre, donc son retour est admissible ; ce qui ne l'est pas, c'est un
    // MÉLANGE de A et de B.
    w.set_engram_state(9, vec![1, 2, 3], vec![0.1, 0.2, 0.3])
        .expect("état A");
    w.commit().expect("commit A");
    w.wal().fsync().expect("fsync A");
    w.msync_zones().expect("msync A");
    assert_eq!(
        w.read_engram_state(9).unwrap(),
        Some((vec![1, 2, 3], Some(vec![0.1, 0.2, 0.3]))),
        "A relu avant la mutation"
    );
    std::fs::write(dir.join("pret"), b"ok").expect("marqueur pret");

    obrain_common::utils::panne::armer_arret_etat_engram(rang);
    let _ = w.set_engram_state(9, vec![7, 8], vec![0.75, 0.25]);
    // Atteint seulement si l'injection n'est pas sur le chemin.
    std::process::exit(3);
}

/// Tue l'enfant aux DEUX points de la fenêtre, puis vérifie trois choses :
/// (a) le dérivé seul est bien ce qu'on croit (A intact, ou le MÉLANGE membres
///     neufs + poids anciens — c'est la preuve que la fenêtre est réelle et que
///     le banc n'est pas vide) ;
/// (b) le rejeu rend le couple COMPLET de B, jamais un mélange ;
/// (c) le journal ne porte que les deux états demandés (A puis B).
#[test]
fn le_couple_membres_poids_survit_a_une_panne_dans_la_fenetre() {
    if std::env::var(ENV_ENFANT).is_ok() {
        return;
    }
    let a = (vec![1u32, 2, 3], Some(vec![0.1f32, 0.2, 0.3]));
    let b = (vec![7u32, 8], Some(vec![0.75f32, 0.25]));
    let melange = (vec![7u32, 8], Some(vec![0.1f32, 0.2, 0.3]));

    for (rang, apres_le_point, attendu_brut) in [
        (
            0u64,
            "le record est journalisé, aucune zone touchée",
            a.clone(),
        ),
        (
            1u64,
            "les membres sont écrits, les poids ne le sont pas",
            melange.clone(),
        ),
    ] {
        let td = tempfile::tempdir().expect("tempdir");
        let base = td.path().join("kb");
        let mut enfant = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "enfant_ecrit_l_etat_puis_meurt_dans_la_fenetre",
                "--exact",
                "--nocapture",
            ])
            .env(ENV_ENFANT, td.path())
            .env(ENV_RANG, rang.to_string())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn enfant");

        let mut tours = 0u32;
        while !td.path().join("pret").exists() {
            tours += 1;
            assert!(tours < 1200, "l'enfant n'a jamais posé `pret`");
            std::thread::sleep(Duration::from_millis(50));
        }
        let statut = enfant.wait().expect("wait enfant");
        assert!(
            !statut.success() && statut.code() != Some(3),
            "l'enfant devait mourir au point {rang} ({apres_le_point}) : {statut:?}"
        );

        // (a) Le dérivé seul : la fenêtre est RÉELLE.
        assert_eq!(
            etat_brut(&base, 9),
            Some(attendu_brut),
            "point {rang} ({apres_le_point}) : état des zones avant rejeu"
        );

        // (b) Le journal fait autorité : le couple COMPLET de B.
        let apres = etat_reconstruit(&base, 9);
        assert_eq!(
            apres,
            Some(b.clone()),
            "point {rang} ({apres_le_point}) : le rejeu doit rendre le couple complet du nouvel état"
        );
        assert_ne!(
            apres,
            Some(melange.clone()),
            "point {rang} : jamais « membres neufs + poids anciens »"
        );

        // (c) Le journal ne porte que ce qui a été demandé, dans l'ordre.
        let records = records_etat(&base.join("substrate.wal"), 9);
        assert_eq!(
            records.len(),
            2,
            "point {rang} : l'état A puis l'état B, rien d'autre (obtenu {records:?})"
        );
        assert_eq!(
            records[1],
            (
                vec![7u32, 8],
                vec![0.75f32.to_bits(), 0.25f32.to_bits()]
            ),
            "point {rang} : le second record est l'état B complet"
        );
    }
}
