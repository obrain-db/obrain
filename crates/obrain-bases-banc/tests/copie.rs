//! La copie refuse une base qu'un processus vivant tient ouverte : prise
//! pendant qu'il écrit (un checkpoint au milieu, par exemple), elle serait
//! incohérente.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};

use obrain_bases_banc::essayer_copie_de;
use obrain_substrate::SubstrateStore;

const ROLE: &str = "OBRAIN_TEST_ENFANT_TIENT_LA_BASE";
const BASE: &str = "OBRAIN_TEST_ENFANT_BASE_TENUE";

/// L'enfant : ouvre la base (en écriture ou en lecture seule) et la tient
/// jusqu'à la fermeture de son entrée standard.
#[test]
fn enfant_tient_la_base() {
    let Ok(role) = std::env::var(ROLE) else {
        return;
    };
    let chemin = std::env::var(BASE).expect(BASE);
    let store = match role.as_str() {
        "ecrivain" => SubstrateStore::open(&chemin).expect("ouverture en écriture"),
        "lecteur" => SubstrateStore::open_read_only(&chemin).expect("ouverture en lecture"),
        autre => panic!("rôle inconnu : {autre}"),
    };
    println!("TIENT");
    let mut ligne = String::new();
    while std::io::stdin().read_line(&mut ligne).is_ok_and(|n| n > 0) {}
    drop(store);
}

fn fabriquer(chemin: &Path) {
    let store = SubstrateStore::create(chemin).expect("création de la base jetable");
    store.flush().expect("flush");
}

#[test]
fn une_base_tenue_par_un_processus_vivant_n_est_pas_copiee() {
    for role in ["ecrivain", "lecteur"] {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("base");
        fabriquer(&base);
        assert!(
            essayer_copie_de(&base, "essai").is_ok(),
            "copie d'une base fermée"
        );

        let mut enfant = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "enfant_tient_la_base", "--nocapture"])
            .env(ROLE, role)
            .env(BASE, &base)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut sortie = BufReader::new(enfant.stdout.take().unwrap());
        let mut ligne = String::new();
        while !ligne.contains("TIENT") {
            ligne.clear();
            assert!(
                sortie.read_line(&mut ligne).unwrap() > 0,
                "l'enfant « {role} » n'a pas ouvert la base"
            );
        }

        let refus = essayer_copie_de(&base, "essai").unwrap_err();
        assert!(
            refus.contains("TENUE par un processus vivant")
                && refus.contains(&format!("pid {}", enfant.id())),
            "« {role} » : {refus}"
        );

        drop(enfant.stdin.take());
        assert!(enfant.wait().unwrap().success());
        assert!(
            essayer_copie_de(&base, "essai").is_ok(),
            "« {role} » : copie refusée après la fermeture"
        );
    }
}
