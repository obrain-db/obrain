//! La copie refuse tout ce qui la rendrait fausse : une base qu'un processus
//! vivant tient (un autre, ou CE processus), une base qui change ou s'ouvre
//! pendant la copie. Et le nettoyage ne supprime que les résidus dont le
//! propriétaire est mort.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use obrain_bases_banc::{Base, designer_pour_un_enfant, essais, essayer_copie_de};
use obrain_substrate::SubstrateStore;

const ROLE: &str = "OBRAIN_TEST_ENFANT_TIENT_LA_BASE";
const BASE: &str = "OBRAIN_TEST_ENFANT_BASE_TENUE";
const DELAI_MS: &str = "OBRAIN_TEST_ENFANT_DELAI_MS";

fn attendre_stdin() {
    let mut ligne = String::new();
    while std::io::stdin().read_line(&mut ligne).is_ok_and(|n| n > 0) {}
}

/// L'enfant, selon son rôle :
/// * `ecrivain` / `lecteur` : ouvre la base et la tient jusqu'à la fermeture
///   de son entrée standard ;
/// * `essai_ecrivain` : essaie d'ouvrir en écriture, dit `OUVERT` ou `REFUSE` ;
/// * `tient_un_residu` : tient le verrou de propriétaire d'un faux résidu ;
/// * `lecteur_deux_fois` : demande deux fois la base `po` en lecture, la
///   seconde après un signal sur l'entrée standard.
#[test]
fn enfant_tient_la_base() {
    let Ok(role) = std::env::var(ROLE) else {
        return;
    };
    let chemin = std::env::var(BASE).expect(BASE);
    if let Ok(ms) = std::env::var(DELAI_MS) {
        std::thread::sleep(Duration::from_millis(ms.parse().expect(DELAI_MS)));
    }
    match role.as_str() {
        "ecrivain" | "lecteur" => {
            let store = if role == "ecrivain" {
                SubstrateStore::open(&chemin).expect("ouverture en écriture")
            } else {
                SubstrateStore::open_read_only(&chemin).expect("ouverture en lecture")
            };
            println!("TIENT");
            attendre_stdin();
            drop(store);
        }
        "essai_ecrivain" => match SubstrateStore::open(&chemin) {
            Ok(store) => {
                println!("OUVERT");
                drop(store);
            }
            Err(e) => println!("REFUSE {e}"),
        },
        "tient_un_residu" => {
            let verrou = tenir_un_verrou(Path::new(&chemin));
            println!("TIENT");
            attendre_stdin();
            drop(verrou);
        }
        "lecteur_deux_fois" => {
            for _ in 0..2 {
                let resultat =
                    std::panic::catch_unwind(|| obrain_bases_banc::base_lecture(Base::Po));
                match resultat {
                    Ok(Some(_)) => println!("LECTURE OK"),
                    Ok(None) => println!("LECTURE SAUTEE"),
                    Err(e) => println!(
                        "LECTURE REFUSEE {}",
                        e.downcast_ref::<String>().cloned().unwrap_or_default()
                    ),
                }
                attendre_stdin_une_ligne();
            }
        }
        autre => panic!("rôle inconnu : {autre}"),
    }
}

fn attendre_stdin_une_ligne() {
    let mut ligne = String::new();
    let _ = std::io::stdin().read_line(&mut ligne);
}

#[cfg(unix)]
#[allow(unsafe_code)]
fn tenir_un_verrou(fichier: &Path) -> std::fs::File {
    use std::os::unix::io::AsRawFd;
    let f = std::fs::File::create(fichier).unwrap();
    // SAFETY: descripteur valide pendant l'appel.
    assert_eq!(unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) }, 0);
    f
}

#[cfg(not(unix))]
fn tenir_un_verrou(fichier: &Path) -> std::fs::File {
    std::fs::File::create(fichier).unwrap()
}

fn fabriquer(chemin: &Path) {
    let store = SubstrateStore::create(chemin).expect("création de la base jetable");
    store.flush().expect("flush");
}

/// Lance un enfant dans un rôle, avec `base` pour chemin, et attend sa ligne
/// `TIENT`. Son entrée standard, gardée, le libère quand on la ferme.
fn lancer(role: &str, base: &Path, delai_ms: Option<u64>) -> Child {
    let mut commande = Command::new(std::env::current_exe().unwrap());
    commande
        .args(["--exact", "enfant_tient_la_base", "--nocapture"])
        .env(ROLE, role)
        .env(BASE, base)
        .env_remove(DELAI_MS)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    if let Some(ms) = delai_ms {
        commande.env(DELAI_MS, ms.to_string());
    }
    commande.spawn().unwrap()
}

fn attendre(enfant: &mut Child, mot: &str) -> String {
    let mut sortie = BufReader::new(enfant.stdout.as_mut().unwrap());
    let mut ligne = String::new();
    loop {
        ligne.clear();
        assert!(
            sortie.read_line(&mut ligne).unwrap() > 0,
            "l'enfant s'est arrêté sans dire « {mot} »"
        );
        if ligne.contains(mot) {
            return ligne;
        }
    }
}

fn liberer(mut enfant: Child) {
    drop(enfant.stdin.take());
    assert!(enfant.wait().unwrap().success());
}

/// Un autre processus peut-il ouvrir la base en écriture, maintenant ?
fn un_autre_ecrivain_peut_ouvrir(base: &Path) -> bool {
    let mut essai = lancer("essai_ecrivain", base, None);
    drop(essai.stdin.take());
    let mut sortie = String::new();
    essai
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut sortie)
        .unwrap();
    assert!(essai.wait().unwrap().success());
    sortie.contains("OUVERT")
}

#[test]
fn une_base_tenue_par_un_autre_processus_n_est_pas_copiee() {
    for role in ["ecrivain", "lecteur"] {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("base");
        fabriquer(&base);
        assert!(
            essayer_copie_de(&base, "essai").is_ok(),
            "copie d'une base fermée"
        );

        let mut enfant = lancer(role, &base, None);
        attendre(&mut enfant, "TIENT");

        let refus = essayer_copie_de(&base, "essai").unwrap_err();
        assert!(
            refus.contains("TENUE par un processus vivant")
                && refus.contains(&format!("pid {}", enfant.id())),
            "« {role} » : {refus}"
        );

        liberer(enfant);
        assert!(
            essayer_copie_de(&base, "essai").is_ok(),
            "« {role} » : copie refusée après la fermeture"
        );
    }
}

/// Le processus appelant tient lui-même la base : la copie est refusée en le
/// nommant, et — c'est le point — l'interrogation du verrou N'A PAS lâché le
/// verrou du moteur : un second écrivain reste refusé.
#[cfg(unix)]
#[test]
fn une_base_tenue_par_ce_processus_n_est_pas_copiee_et_reste_tenue() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("base");
    fabriquer(&base);

    let store = SubstrateStore::open(&base).expect("ouverture en écriture ici");
    assert!(
        !un_autre_ecrivain_peut_ouvrir(&base),
        "prémisse : la base ouverte ici doit être refusée à un autre écrivain"
    );

    let refus = essayer_copie_de(&base, "essai").unwrap_err();
    assert!(
        refus.contains("TENUE par un processus vivant")
            && refus.contains("CE processus")
            && refus.contains(&format!("pid {}", std::process::id())),
        "{refus}"
    );

    // Le verrou du moteur est intact : interroger n'a rien lâché.
    assert!(
        !un_autre_ecrivain_peut_ouvrir(&base),
        "l'interrogation du verrou a LÂCHÉ le verrou du moteur : un second écrivain a ouvert"
    );

    drop(store);
    assert!(un_autre_ecrivain_peut_ouvrir(&base));
    assert!(essayer_copie_de(&base, "essai").is_ok());
}

/// Une base ouverte PENDANT la copie (entre la copie et ses contrôles) fait
/// abandonner la copie.
#[test]
fn une_base_ouverte_pendant_la_copie_est_abandonnee() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("base");
    fabriquer(&base);

    // L'enfant ouvre la base 300 ms après son lancement ; la copie, elle,
    // marque une pause de 1,5 s avant ses contrôles.
    let mut enfant = lancer("ecrivain", &base, Some(300));
    essais::pause_apres_copie(1500);
    let resultat = essayer_copie_de(&base, "essai");
    essais::pause_apres_copie(0);
    attendre(&mut enfant, "TIENT");
    liberer(enfant);

    let refus = resultat.expect_err("la copie aurait dû être abandonnée");
    assert!(
        refus.contains("OUVERTE pendant la copie")
            || refus.contains("TENUE par un processus vivant"),
        "{refus}"
    );
}

/// Une base MODIFIÉE pendant la copie (un fichier qui change de taille) fait
/// abandonner la copie.
#[test]
fn une_base_modifiee_pendant_la_copie_est_abandonnee() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("base");
    fabriquer(&base);
    let cible = base.join("substrate.wal");

    let modificateur = {
        let cible = cible.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&cible)
                .unwrap();
            f.write_all(b"un enregistrement de plus").unwrap();
        })
    };
    essais::pause_apres_copie(1500);
    let resultat = essayer_copie_de(&base, "essai");
    essais::pause_apres_copie(0);
    modificateur.join().unwrap();

    let refus = resultat.expect_err("la copie aurait dû être abandonnée");
    assert!(
        refus.contains("a CHANGÉ pendant la copie") && refus.contains("substrate.wal"),
        "{refus}"
    );
}

/// Un refus de copie de lecture est mémorisé pour tout le binaire : la
/// seconde demande refuse encore, même une fois la base libérée, sans
/// retenter.
#[test]
fn un_refus_de_copie_de_lecture_est_memorise() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("po");
    fabriquer(&base);
    let mut detenteur = lancer("ecrivain", &base, None);
    attendre(&mut detenteur, "TIENT");

    let mut commande = Command::new(std::env::current_exe().unwrap());
    commande
        .args(["--exact", "enfant_tient_la_base", "--nocapture"])
        .env(ROLE, "lecteur_deux_fois")
        .env(BASE, &base)
        .env_remove(DELAI_MS)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    designer_pour_un_enfant(&mut commande, &[(Base::Po, &base)]);
    let mut lecteur = commande.spawn().unwrap();

    let premiere = attendre(&mut lecteur, "LECTURE");
    assert!(
        premiere.contains("REFUSEE") && premiere.contains("TENUE"),
        "{premiere}"
    );
    // La base est libérée : une nouvelle copie serait possible…
    liberer(detenteur);
    writeln!(lecteur.stdin.as_mut().unwrap(), "suite").unwrap();
    let seconde = attendre(&mut lecteur, "LECTURE");
    // … mais le refus est mémorisé.
    assert!(
        seconde.contains("REFUSEE") && seconde.contains("refus mémorisé"),
        "{seconde}"
    );
    drop(lecteur.stdin.take());
    let mut stderr = String::new();
    lecteur
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(lecteur.wait().unwrap().success());
    assert_eq!(
        stderr.matches("] : copie de ").count(),
        0,
        "une copie a été prise :\n{stderr}"
    );
}

/// Le balayage ne supprime qu'un résidu dont le propriétaire est MORT : son
/// verrou se laisse prendre. Un résidu dont le propriétaire vit est gardé.
#[cfg(unix)]
#[test]
fn le_balayage_ne_supprime_que_les_residus_sans_proprietaire_vivant() {
    let unique = format!("{}-{}", std::process::id(), line!());
    let vivant = std::env::temp_dir().join(format!("obrain-banc-99999-vivant-{unique}"));
    let mort = std::env::temp_dir().join(format!("obrain-banc-99998-mort-{unique}"));
    let garde = std::env::temp_dir().join(format!("obrain-banc-gardee-99997-{unique}"));
    for d in [&vivant, &mort, &garde] {
        std::fs::create_dir_all(d.join("base")).unwrap();
        std::fs::write(d.join(".proprietaire"), b"").unwrap();
    }
    let mut proprietaire = lancer("tient_un_residu", &vivant.join(".proprietaire"), None);
    attendre(&mut proprietaire, "TIENT");

    let supprimes = essais::balayer_les_residus_maintenant();
    assert!(supprimes.contains(&mort), "{supprimes:?}");
    assert!(
        vivant.is_dir(),
        "le résidu d'un propriétaire VIVANT a été supprimé"
    );
    assert!(
        garde.is_dir(),
        "une copie conservée à la demande a été balayée"
    );
    assert!(!mort.exists());

    liberer(proprietaire);
    let supprimes = essais::balayer_les_residus_maintenant();
    assert!(supprimes.contains(&vivant), "{supprimes:?}");
    std::fs::remove_dir_all(&garde).unwrap();
}

/// Une base sans `substrate.dirlock` (jamais ouverte par un moteur qui le
/// pose) : aucun détenteur ne peut être cherché, on le dit, et la copie passe.
#[test]
fn une_base_sans_dirlock_est_copiee_en_le_disant() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("base");
    fabriquer(&base);
    std::fs::remove_file(base.join("substrate.dirlock")).unwrap();
    let copie = essayer_copie_de(&base, "essai").unwrap();
    assert!(copie.chemin().join("substrate.meta").is_file());
    let _ = PathBuf::new();
}
