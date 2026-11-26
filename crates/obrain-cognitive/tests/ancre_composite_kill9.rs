//! (2026-11-24) ATOMICITÉ DE L'ANCRE COMPOSITE SOUS SIGKILL — banc d'injection.
//!
//! `decay_clock::reinforce` écrit l'ancre composite `(E_ref, t_ref, t_hi, H)`
//! (UNE propriété atomique) puis le miroir de colonne (best-effort). Le point
//! d'injection `panne::point_entre_ancre_et_miroir` tue le processus (abort,
//! état d'un SIGKILL) entre les deux. Après réouverture, `read_anchor` rend une
//! ancre valide ou `None` (jamais une ancre déchirée), et `current_energy`
//! n'expose que 0,5 ou 0,75 — jamais 0,375 (énergie neuve + ancienne ancre) ni
//! 1,0 (ancienne énergie + nouvelle ancre), les deux combinaisons déchirées que
//! l'ancien code (colonne puis ancre séparées) exposait. Deux bancs distinguent
//! la PREMIÈRE création (jamais engagée : `None` admissible) de la MISE À JOUR
//! d'un état DURABLE (`None` inadmissible — perdre A serait un défaut).
//!
//! DISTINCTION DES PANNES. Le `abort()` simule un SIGKILL : le noyau préserve le
//! cache de pages, donc les octets déjà écrits (ancre en zone de propriétés,
//! colonne en zone nœuds) survivent. La perte des écritures NON DURABLES (panne
//! de courant) est un autre scénario, couvert par la troncature du WAL dans
//! `recovery_synapse.rs` — ici on mesure exactement la fenêtre ancre/miroir.
//!
//! Run: cargo test -p obrain-cognitive --test ancre_composite_kill9 \
//!        --features substrate -- --nocapture
#![cfg(feature = "substrate")]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use obrain_cognitive::decay_clock;
use obrain_cognitive::decay_law::{DecayLaw, unix_micros};
use obrain_common::types::NodeId;
use obrain_core::graph::GraphStoreMut;
use obrain_substrate::SubstrateStore;

const H: Duration = Duration::from_secs(3600);
const ENV_ENFANT: &str = "OBRAIN_TEST_ANCRE_COMPOSITE_ENFANT";

fn law() -> DecayLaw {
    DecayLaw::new(H)
}

/// Enfant : ouvre une base neuve, pose un état initial cohérent (énergie 0,5 +
/// ancre), marque `pret`, arme l'arrêt entre ancre et miroir, puis renforce
/// 0,25 → 0,75. Il meurt sur le point d'injection (abort), sans jamais écrire
/// le miroir.
#[test]
fn enfant_renforce_et_meurt_entre_ancre_et_miroir() {
    let Ok(dir) = std::env::var(ENV_ENFANT) else {
        return;
    };
    let dir = Path::new(&dir);
    let chemin = dir.join("kb");
    let sub = Arc::new(SubstrateStore::create(&chemin).expect("create"));
    let id = sub.create_node(&["n"]);
    assert_eq!(id, NodeId(1), "premier nœud d'une base neuve");
    let now = unix_micros();
    // État initial cohérent : énergie 0,5, ancre composite (0,5, now, H).
    decay_clock::persist_anchor_explicit(&sub, id, 0.5, now, H);
    sub.set_node_energy_f32(id, 0.5).expect("colonne initiale");
    // Marqueur posé APRÈS l'état initial : tout ce que le parent mesure ensuite
    // a bien eu lieu dans cet enfant.
    std::fs::write(dir.join("pret"), b"ok").expect("marqueur pret");

    // Arme l'arrêt entre l'ancre composite et le miroir de colonne.
    obrain_common::utils::panne::armer_arret_entre_ancre_et_miroir();
    // Renforce 0,25 → 0,75 : l'ancre (0,75) est écrite, puis abort AVANT le
    // miroir — la colonne reste à 0,5.
    let _ = decay_clock::reinforce(&sub, id, 0.25, &law(), 1.0, now);
    // Atteint seulement si l'injection n'est pas sur le chemin.
    std::process::exit(3);
}

/// Tue l'enfant au point d'injection, rouvre, et vérifie la cohérence.
#[test]
fn l_ancre_composite_reste_coherente_sous_sigkill() {
    if std::env::var(ENV_ENFANT).is_ok() {
        return;
    }
    let td = tempfile::tempdir().expect("tempdir");
    let mut enfant = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "enfant_renforce_et_meurt_entre_ancre_et_miroir",
            "--exact",
            "--nocapture",
        ])
        .env(ENV_ENFANT, td.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn enfant");

    // Attendre que l'état initial soit posé.
    let mut tours = 0u32;
    while !td.path().join("pret").exists() {
        tours += 1;
        assert!(tours < 1200, "l'enfant n'a jamais posé `pret`");
        std::thread::sleep(Duration::from_millis(50));
    }
    let statut = enfant.wait().expect("wait enfant");
    assert!(
        !statut.success() && statut.code() != Some(3),
        "l'enfant devait mourir sur le point d'injection : {statut:?}"
    );

    // Réouverture.
    let sub = Arc::new(SubstrateStore::open(td.path().join("kb")).expect("open"));
    let id = NodeId(1);

    // (a) L'ancre est valide ou absente — jamais malformée.
    let ancre = decay_clock::read_anchor(&sub, id);
    match ancre {
        Some(a) => {
            assert!(
                a.e_ref.is_finite() && a.e_ref > 0.0,
                "ancre à e_ref aberrante"
            );
            assert!(
                (a.e_ref - 0.5).abs() < 1e-9 || (a.e_ref - 0.75).abs() < 1e-9,
                "l'ancre ne doit porter que l'état avant (0,5) ou après (0,75), pas {}",
                a.e_ref
            );
        }
        None => {}
    }

    // (b) L'énergie exposée est cohérente : ≈0,5 ou ≈0,75, jamais 0,375 ni 1,0.
    // La décroissance naturelle (2^(-Δt/H)) abaisse la valeur de quelques
    // millièmes entre l'ancre de l'enfant et la lecture du parent : on accepte
    // une bande [0,45, 0,76] qui contient 0,5 et 0,75 décrus, et exclut
    // nettement 0,375 et 1,0.
    let energie = decay_clock::current_energy(&sub, id, &law()).expect("énergie lisible");
    assert!(
        energie > 0.45 && energie <= 0.76,
        "get_energy doit rendre ≈0,5 ou ≈0,75, jamais une combinaison déchirée (rendu {})",
        energie
    );
    assert!(
        (energie - 0.375).abs() > 0.05,
        "0,375 est la combinaison déchirée (énergie neuve + ancienne ancre), rendu {energie}"
    );
    assert!(
        (energie - 1.0).abs() > 0.15,
        "1,0 est la combinaison déchirée (ancienne énergie + nouvelle ancre), rendu {energie}"
    );
}

/// Enfant : ouvre une base neuve, pose un état initial DURABLE A (énergie 0,5
/// plus ancre composite, puis `flush()`), vérifie que A se relit, marque `pret`,
/// arme l'arrêt entre ancre et miroir, puis renforce 0,25 → 0,75. Il meurt sur
/// le point d'injection (abort), sans jamais écrire le miroir. Ce qui le
/// distingue de [`enfant_renforce_et_meurt_entre_ancre_et_miroir`] : A est ICI
/// durable, donc le parent peut exiger qu'il ne soit JAMAIS perdu.
#[test]
fn enfant_renforce_apres_ancre_durable_et_meurt() {
    let Ok(dir) = std::env::var(ENV_ENFANT) else {
        return;
    };
    let dir = Path::new(&dir);
    let chemin = dir.join("kb");
    let sub = Arc::new(SubstrateStore::create(&chemin).expect("create"));
    let id = sub.create_node(&["n"]);
    assert_eq!(id, NodeId(1), "premier nœud d'une base neuve");
    let now = unix_micros();
    // État initial cohérent A : énergie 0,5, ancre composite (0,5, now, H).
    decay_clock::persist_anchor_explicit(&sub, id, 0.5, now, H);
    sub.set_node_energy_f32(id, 0.5).expect("colonne initiale");
    // A devient DURABLE : le flush msync les zones (dont la propriété d'ancre)
    // et commit le WAL. La récupération ne peut plus le perdre.
    sub.flush().expect("flush de l'état initial A");
    // Vérifie la relecture de A AVANT la mutation — c'est le préalable du test.
    let a = decay_clock::read_anchor(&sub, id).expect("A doit se relire avant la mutation");
    assert!(
        (a.e_ref - 0.5).abs() < 1e-9,
        "l'ancre durable de départ est A=0,5 (relu {})",
        a.e_ref
    );
    // Marqueur posé APRÈS l'état durable : tout ce que le parent mesure ensuite
    // a bien eu lieu dans cet enfant.
    std::fs::write(dir.join("pret"), b"ok").expect("marqueur pret");

    // Arme l'arrêt entre l'ancre composite et le miroir de colonne.
    obrain_common::utils::panne::armer_arret_entre_ancre_et_miroir();
    // Mutation A → B : l'ancre (0,75) est écrite, puis abort AVANT le miroir.
    let _ = decay_clock::reinforce(&sub, id, 0.25, &law(), 1.0, now);
    // Atteint seulement si l'injection n'est pas sur le chemin.
    std::process::exit(3);
}

/// Tue l'enfant au point d'injection, rouvre, et vérifie que la récupération
/// d'une MUTATION sur un état durable retrouve A complet OU B complet — jamais
/// `None` (perdre A serait un défaut), jamais une ancre déchirée, jamais un
/// hybride (0,375 / 1,0). Contraste avec
/// [`l_ancre_composite_reste_coherente_sous_sigkill`] : là, la PREMIÈRE
/// création n'était pas durable et `None` restait admissible ; ici, l'état
/// durable A rend `None` inadmissible.
#[test]
fn l_ancre_durable_n_est_jamais_perdue_sous_sigkill() {
    if std::env::var(ENV_ENFANT).is_ok() {
        return;
    }
    let td = tempfile::tempdir().expect("tempdir");
    let mut enfant = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "enfant_renforce_apres_ancre_durable_et_meurt",
            "--exact",
            "--nocapture",
        ])
        .env(ENV_ENFANT, td.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn enfant");

    // Attendre que l'état durable A soit posé.
    let mut tours = 0u32;
    while !td.path().join("pret").exists() {
        tours += 1;
        assert!(tours < 1200, "l'enfant n'a jamais posé `pret`");
        std::thread::sleep(Duration::from_millis(50));
    }
    let statut = enfant.wait().expect("wait enfant");
    assert!(
        !statut.success() && statut.code() != Some(3),
        "l'enfant devait mourir sur le point d'injection : {statut:?}"
    );

    // Réouverture.
    let sub = Arc::new(SubstrateStore::open(td.path().join("kb")).expect("open"));
    let id = NodeId(1);

    // (a) L'ancre n'est JAMAIS perdue : A était durable, la récupération rend
    // donc A (0,5) ou B (0,75). `None` est ici un défaut (A durable perdu),
    // et un e_ref hors {0,5 ; 0,75} est une ancre déchirée (0,375 / 1,0).
    let a = decay_clock::read_anchor(&sub, id)
        .expect("A était durable : la récupération ne doit jamais rendre None");
    assert!(
        (a.e_ref - 0.5).abs() < 1e-9 || (a.e_ref - 0.75).abs() < 1e-9,
        "l'ancre doit être A (0,5) ou B (0,75), pas {}",
        a.e_ref
    );
    assert!(
        (a.e_ref - 0.375).abs() > 1e-9 && (a.e_ref - 1.0).abs() > 1e-9,
        "aucun état hybride déchiré (0,375 / 1,0) : relu {}",
        a.e_ref
    );

    // (b) L'énergie exposée est cohérente : ≈0,5 ou ≈0,75, jamais 0,375 ni 1,0.
    let energie = decay_clock::current_energy(&sub, id, &law()).expect("énergie lisible");
    assert!(
        energie > 0.45 && energie <= 0.76,
        "get_energy doit rendre ≈0,5 ou ≈0,75 (rendu {})",
        energie
    );
    assert!(
        (energie - 0.375).abs() > 0.05,
        "0,375 est la combinaison déchirée (rendu {energie})"
    );
    assert!(
        (energie - 1.0).abs() > 0.15,
        "1,0 est la combinaison déchirée (rendu {energie})"
    );
}
