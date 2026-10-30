//! LA PROPRIÉTÉ QUI GARDE L'ANNONCE : ce qui est annoncé s'exécute, ce qui est
//! refusé est dit.
//!
//! # L'incident que ces tests empêchent de revenir
//!
//! `attache::capacites` annonçait `write` **en dur**, sans regarder la racine.
//! Sur une racine en lecture seule, le moteur apprenait que la machine savait
//! écrire ; l'utilisateur ne découvrait le contraire qu'à l'échec, après coup, et
//! un échec qui arrive après coup ressemble à une panne. La doctrine du dépôt le
//! dit en une ligne : **annoncer est décider**.
//!
//! # Pourquoi ces tests lancent le BINAIRE, et pas une maquette
//!
//! La propriété porte sur ce que le nœud annonce ET sur ce qu'il fait : la
//! vérifier sur une structure interne ne prouverait rien du chemin réel, qui
//! passe par la ligne de commande, la politique construite au démarrage et
//! `protocole::executer_outil`. On lance donc `selfd` (`CARGO_BIN_EXE_selfd`,
//! le binaire que cargo vient de construire), sur des racines et des états
//! JETABLES sous `/tmp` : aucune base vivante, aucun socket réel, aucun état
//! réel n'est touché.
//!
//! # Ce que « refus de POLITIQUE » veut dire ici
//!
//! Deux échecs différents ne doivent pas être confondus :
//!
//! * « ce chemin sort de la racine », « exécution NON AUTORISÉE » — un refus de
//!   politique. Il ne doit JAMAIS arriver pour une capacité annoncée : c'est la
//!   faute qu'on interdit ;
//! * « No such file or directory » — un résultat d'appel, parfaitement légitime,
//!   qui laisse la capacité intacte.
//!
//! On ne peut les distinguer que par le MOTIF. C'est la raison d'être de
//! l'exigence « un refus doit être motivé » : sans motif, les deux se
//! ressemblent.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

/// Le binaire RÉEL du nœud, construit par cargo pour ce test.
fn selfd() -> &'static str {
    env!("CARGO_BIN_EXE_selfd")
}

/// Délai au-delà duquel un appel est considéré comme bloqué. Un test qui pend
/// n'apprend rien ; un test qui tue son enfant et le dit, si.
const DELAI: Duration = Duration::from_secs(30);

/// Un bac jetable : une racine, un état d'identité, sous `/tmp`.
struct Bac {
    base: PathBuf,
}

impl Bac {
    fn neuf(nom: &str) -> Self {
        static COMPTEUR: AtomicU64 = AtomicU64::new(0);
        let n = COMPTEUR.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir().join(format!("selfd-cap-{}-{nom}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("racine")).expect("création de la racine du bac");
        std::fs::create_dir_all(base.join("etat")).expect("création de l'état du bac");
        Self { base }
    }

    fn racine(&self) -> PathBuf {
        self.base.join("racine")
    }

    fn etat(&self) -> PathBuf {
        self.base.join("etat")
    }

    /// Rend la racine NON inscriptible. Rend `false` si le processus peut quand
    /// même y écrire (root, CAP_DAC_OVERRIDE) : le test ne peut alors rien
    /// prouver, et il le dit au lieu de passer en silence.
    fn rendre_racine_non_inscriptible(&self) -> bool {
        let racine = self.racine();
        std::fs::set_permissions(&racine, std::fs::Permissions::from_mode(0o555))
            .expect("chmod 0555 sur la racine du bac");
        let temoin = racine.join(".temoin-ecriture");
        if std::fs::write(&temoin, b"x").is_ok() {
            let _ = std::fs::remove_file(&temoin);
            return false;
        }
        true
    }
}

impl Drop for Bac {
    fn drop(&mut self) {
        // Une racine rendue non inscriptible doit redevenir supprimable : sans
        // cela, /tmp se remplirait de bacs orphelins à chaque exécution.
        let _ = std::fs::set_permissions(
            self.racine(),
            std::fs::Permissions::from_mode(0o755),
        );
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

/// Ce qu'un appel a rendu.
struct Sortie {
    ok: bool,
    stdout: String,
    stderr: String,
}

impl Sortie {
    fn json(&self) -> Value {
        assert!(self.ok, "l'appel a échoué : {}", self.stderr.trim());
        serde_json::from_str(&self.stdout)
            .unwrap_or_else(|e| panic!("stdout non-JSON ({e}) : {}", self.stdout))
    }

    /// Le motif rendu. En mode `--call`, un refus remonte comme une ERREUR (code
    /// de sortie non nul), jamais comme une réponse vide — c'est déjà la moitié
    /// de la propriété : le silence ressemblerait à une panne.
    fn motif(&self) -> String {
        self.stderr.trim().to_string()
    }
}

/// Lance un enfant en ATTENDANT sa fin, bornée.
fn lancer(mut commande: Command) -> Sortie {
    let mut enfant = commande
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("lancement de selfd");
    let debut = Instant::now();
    loop {
        match enfant.try_wait().expect("attente de l'enfant") {
            Some(_) => break,
            None if debut.elapsed() > DELAI => {
                let _ = enfant.kill();
                let _ = enfant.wait();
                panic!("selfd n'a pas rendu la main en {} s", DELAI.as_secs());
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }
    let sortie = enfant.wait_with_output().expect("sortie de selfd");
    Sortie {
        ok: sortie.status.success(),
        stdout: String::from_utf8_lossy(&sortie.stdout).to_string(),
        stderr: String::from_utf8_lossy(&sortie.stderr).to_string(),
    }
}

/// Un appel au nœud, sur le bac donné.
fn appel(bac: &Bac, methode: &str, params: &str, allow_exec: bool, extra: &[&str]) -> Sortie {
    let mut c = Command::new(selfd());
    c.arg("--call").arg(methode);
    if !params.is_empty() {
        c.arg(params);
    }
    c.arg("--root").arg(bac.racine());
    c.arg("--state-dir").arg(bac.etat());
    c.arg("--json");
    if allow_exec {
        c.arg("--allow-exec");
    }
    for a in extra {
        c.arg(a);
    }
    lancer(c)
}

/// L'appel qui ANNONCE : `caps`.
fn caps(bac: &Bac, allow_exec: bool) -> Value {
    appel(bac, "caps", "", allow_exec, &[]).json()
}

fn annoncees(caps: &Value) -> Vec<String> {
    caps["capacites"]
        .as_array()
        .unwrap_or_else(|| panic!("« capacites » doit être un tableau de chaînes : {caps}"))
        .iter()
        .map(|v| v.as_str().expect("capacité non textuelle").to_string())
        .collect()
}

/// Le motif du refus d'une capacité, s'il est annoncé.
fn motif_refus(caps: &Value, nom: &str) -> Option<String> {
    caps["capacites_refusees"]
        .as_array()?
        .iter()
        .find(|r| r["nom"].as_str() == Some(nom))
        .and_then(|r| r["motif"].as_str())
        .map(str::to_string)
}

/// Un refus de POLITIQUE, reconnaissable à son motif (voir l'en-tête).
fn est_refus_de_politique(motif: &str) -> bool {
    motif.contains("sort de la racine")
        || motif.contains("chemin refusé")
        || motif.contains("NON AUTORISÉE")
}

/// Exerce UNE capacité annoncée. Panique si le nœud la refuse pour un motif de
/// politique — c'est exactement la faute interdite : annoncée puis refusée.
fn exercer(nom: &str, bac: &Bac, allow_exec: bool) {
    let sortie = match nom {
        "write" => {
            let s = appel(
                bac,
                "write",
                r#"{"path":"ecrit-par-le-noeud.txt","content":"bonjour"}"#,
                allow_exec,
                &[],
            );
            let v = s.json();
            assert_eq!(v["bytes"].as_u64(), Some(7), "write annoncé doit écrire : {v}");
            let relu = std::fs::read_to_string(bac.racine().join("ecrit-par-le-noeud.txt"))
                .expect("le fichier écrit doit exister dans la racine");
            assert_eq!(relu, "bonjour");
            return;
        }
        "read" => {
            // La cible est posée par le test, pas par la capacité en cours
            // d'examen : `read` doit tenir tout seul.
            std::fs::write(bac.racine().join("a-lire.txt"), b"contenu").expect("pose du fichier");
            appel(bac, "read", r#"{"path":"a-lire.txt"}"#, allow_exec, &[])
        }
        "list" => {
            std::fs::write(bac.racine().join("vu-par-list.txt"), b"x").ok();
            appel(bac, "list", "", allow_exec, &[])
        }
        "status" => appel(bac, "status", "", allow_exec, &[]),
        "exec" => appel(
            bac,
            "exec",
            r#"{"cmd":"/bin/sh","args":["-c","exit 0"]}"#,
            allow_exec,
            &[],
        ),
        autre => panic!(
            "capacité annoncée que ce test ne sait pas exercer : « {autre} ». \
             Une capacité nouvelle doit être exercée ici, sinon l'annonce n'est pas vérifiée."
        ),
    };
    assert!(
        sortie.ok,
        "« {nom} » est ANNONCÉE mais l'appel échoue : {}",
        sortie.motif()
    );
    assert!(
        !est_refus_de_politique(&sortie.motif()),
        "« {nom} » est ANNONCÉE et refusée par la POLITIQUE : {}",
        sortie.motif()
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// (a) Racine inscriptible + exec autorisé : annoncé == exercable
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn a_racine_inscriptible_exec_ouvert_annonce_est_exercable() {
    let bac = Bac::neuf("inscriptible");
    let c = caps(&bac, true);

    let a = annoncees(&c);
    assert_eq!(
        a,
        vec!["read", "list", "write", "status", "exec"],
        "tout est ouvert ici : l'ensemble annoncé doit être complet et dans l'ordre stable"
    );
    assert!(
        c["capacites_refusees"].as_array().map(Vec::is_empty).unwrap_or(false),
        "aucune capacité ne doit être refusée quand rien ne l'est : {}",
        c["capacites_refusees"]
    );

    // AUCUNE capacité annoncée n'est refusée — la propriété, dans ce sens-là.
    for nom in &a {
        exercer(nom, &bac, true);
    }

    // Les limites sont celles de la MACHINE, pas des constantes décoratives.
    let reelle = std::fs::canonicalize(bac.racine()).expect("canonicalisation de la racine");
    assert_eq!(
        c["limites"]["racine_reelle"].as_str(),
        Some(reelle.display().to_string().as_str()),
        "la racine annoncée doit être la racine RÉELLE (celle qui confine)"
    );
    assert_eq!(c["limites"]["exec"].as_str(), Some("ouvert"));
    assert_eq!(c["limites"]["delai_ms_par_defaut"].as_u64(), Some(60_000));
    assert_eq!(c["limites"]["plafond_octets_par_flux"].as_u64(), Some(262_144));
    assert_eq!(c["limites"]["version"].as_str(), Some(env!("CARGO_PKG_VERSION")));
}

// ─────────────────────────────────────────────────────────────────────────────
// (b) Racine NON inscriptible : write n'est PAS annoncé
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn b_racine_non_inscriptible_write_nest_pas_annonce() {
    let bac = Bac::neuf("lecture-seule");
    if !bac.rendre_racine_non_inscriptible() {
        eprintln!(
            "IGNORÉ : ce processus peut écrire dans une racine 0555 (root ou CAP_DAC_OVERRIDE) \
             — la propriété n'est pas mesurable ici."
        );
        return;
    }

    let c = caps(&bac, true);
    let a = annoncees(&c);
    assert!(
        !a.contains(&"write".to_string()),
        "la racine est en LECTURE SEULE : write ne doit pas être annoncé, or {a:?}"
    );
    // …et les capacités qui, elles, restent exercables doivent rester annoncées :
    // ne rien annoncer serait une autre façon de mentir.
    assert!(a.contains(&"read".to_string()), "{a:?}");
    assert!(a.contains(&"list".to_string()), "{a:?}");
    assert!(a.contains(&"exec".to_string()), "{a:?}");

    // Le refus est DIT, avec un motif — pas seulement absent de la liste.
    let motif = motif_refus(&c, "write").expect("write doit être annoncé comme refusé, motivé");
    assert!(!motif.trim().is_empty(), "un refus sans motif ne se corrige pas");
    assert!(
        motif.contains("write"),
        "le motif doit nommer ce qui est refusé : {motif}"
    );

    // Et l'écriture est VRAIMENT refusée : annoncer un refus qu'on ne tient pas
    // serait la faute symétrique.
    let essai = appel(
        &bac,
        "write",
        r#"{"path":"tentative.txt","content":"x"}"#,
        true,
        &[],
    );
    assert!(!essai.ok, "write est refusé : l'appel doit échouer");
    assert!(!bac.racine().join("tentative.txt").exists());

    // ── LA VÉRITÉ NE DÉPEND PAS DE LA MÉTHODE APPELÉE ───────────────────────
    // `status` porte le même bloc `capabilities` que `caps` (il vient du même
    // `outils::identite`). L'incident mesuré : le bloc hérité y disait encore
    // `fsWrite: true` sur cette racine 0555, pendant que `capacites` disait le
    // contraire — deux vérités dans une réponse.
    let s = appel(&bac, "status", "", true, &[]).json();
    assert_eq!(
        s["capabilities"]["fsWrite"].as_bool(),
        Some(false),
        "status ne doit pas annoncer l'écriture sur une racine en lecture seule : {}",
        s["capabilities"]
    );
    assert_eq!(s["capabilities"]["exec"].as_bool(), Some(true), "{}", s["capabilities"]);
    assert!(
        s["politique"]["capacites_refusees"]
            .as_array()
            .expect("status porte aussi les refus")
            .iter()
            .any(|r| r["nom"].as_str() == Some("write")),
        "status doit porter le refus de write, motivé : {}",
        s["politique"]["capacites_refusees"]
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// (c) Sans --allow-exec : exec n'est PAS annoncé ET l'appel est REFUSÉ, motivé
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn c_sans_allow_exec_exec_est_absent_et_refuse_avec_motif() {
    let bac = Bac::neuf("sans-exec");
    let c = caps(&bac, false);

    // Moitié 1 : l'annonce.
    let a = annoncees(&c);
    assert!(!a.contains(&"exec".to_string()), "sans --allow-exec : {a:?}");
    let motif = motif_refus(&c, "exec").expect("exec doit être annoncé comme refusé, motivé");
    assert!(
        motif.contains("--allow-exec"),
        "le motif doit dire COMMENT ouvrir la capacité : {motif}"
    );

    // Moitié 2 : l'appel, refusé par la POLITIQUE (pas par une panne).
    let essai = appel(&bac, "exec", r#"{"cmd":"/bin/sh","args":["-c","exit 0"]}"#, false, &[]);
    assert!(!essai.ok, "sans --allow-exec, exec doit être refusé");
    assert!(
        est_refus_de_politique(&essai.motif()),
        "le refus doit être un refus de politique, reconnaissable : {}",
        essai.motif()
    );
    assert!(
        essai.motif().contains("--allow-exec"),
        "l'utilisateur doit apprendre comment l'autoriser : {}",
        essai.motif()
    );

    // Le reste du nœud n'est pas sacrifié : la racine inscriptible reste annoncée.
    assert!(a.contains(&"write".to_string()), "{a:?}");
}

// ─────────────────────────────────────────────────────────────────────────────
// (d) La cohérence inverse, et ce qui la rend mesurable : distinguer les motifs
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn d_capacite_annoncee_jamais_refusee_et_echec_legitime_distingue() {
    // On exerce CE QUE LE NŒUD ANNONCE, pas une liste écrite dans le test : si
    // une capacité s'ajoute demain sans être exercable, ce test la rattrape.
    for (exec_autorise, nom) in [(false, "sans-exec"), (true, "avec-exec")] {
        let bac = Bac::neuf(nom);
        let c = caps(&bac, exec_autorise);
        for capacite in annoncees(&c) {
            exercer(&capacite, &bac, exec_autorise);
        }
    }

    let bac = Bac::neuf("motifs");
    // Un échec LÉGITIME : la capacité `read` est réelle, le fichier n'existe pas.
    let absent = appel(&bac, "read", r#"{"path":"absent.txt"}"#, true, &[]);
    assert!(!absent.ok, "un fichier absent doit échouer");
    assert!(
        !est_refus_de_politique(&absent.motif()),
        "un fichier absent n'est PAS un refus de politique : {}",
        absent.motif()
    );
    assert!(
        absent.motif().contains("absent.txt"),
        "le motif doit nommer la cible, sinon il ne se corrige pas : {}",
        absent.motif()
    );

    // Le pendant : un vrai refus de politique est bien reconnu — sans quoi
    // l'assertion ci-dessus ne prouverait rien (elle passerait sur un texte quelconque).
    let evasion = appel(&bac, "read", r#"{"path":"../../etc/passwd"}"#, true, &[]);
    assert!(!evasion.ok, "sortir de la racine doit être refusé");
    assert!(
        est_refus_de_politique(&evasion.motif()),
        "une évasion doit être un refus de politique, reconnaissable : {}",
        evasion.motif()
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Les LIMITES annoncées sont celles qui s'appliquent — mesuré, pas déclaré
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn e_les_limites_annoncees_sont_celles_qui_s_appliquent() {
    let bac = Bac::neuf("limites");
    let c = appel(
        &bac,
        "caps",
        "",
        false,
        &["--max-output", "4096", "--timeout-ms", "1234"],
    )
    .json();

    // Ce que le nœud annonce…
    assert_eq!(
        c["limites"]["plafond_octets_par_flux"].as_u64(),
        Some(4096),
        "les limites annoncées doivent être celles du démarrage, pas des constantes"
    );
    assert_eq!(c["limites"]["delai_ms_par_defaut"].as_u64(), Some(1234));

    // …est ce qui s'applique : 9000 octets lus, 4096 conservés, et le nœud le dit.
    // Le même `--max-output` est redonné à l'appel : en mode `--call`, la
    // politique est construite à chaque invocation — c'est donc le même nœud
    // (mêmes limites) qui doit annoncer ET appliquer.
    let gros = "a".repeat(9000);
    std::fs::write(bac.racine().join("gros.txt"), &gros).expect("pose du gros fichier");
    let v = appel(
        &bac,
        "read",
        r#"{"path":"gros.txt"}"#,
        false,
        &["--max-output", "4096"],
    )
    .json();
    assert_eq!(v["bytes"].as_u64(), Some(9000), "l'appel doit dire la taille réelle");
    assert_eq!(v["tronque"].as_bool(), Some(true));
    assert_eq!(
        v["content"].as_str().map(str::len),
        Some(4096),
        "le plafond annoncé doit être celui qui coupe réellement la sortie"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// L'ANNONCE AU MOTEUR : ce que le nœud déclare en s'attachant
// ─────────────────────────────────────────────────────────────────────────────

/// Attache le nœud à un faux moteur qui garde le corps de la requête, et rend ce
/// corps.
///
/// Le faux moteur répond `401` : sur un jeton refusé, la boucle d'attachement
/// s'arrête net (voir `attache::boucle`) — pas de boucle infinie, donc pas de test
/// qui pend. Ce qui nous intéresse est ce qui est PARTI, pas ce qui revient.
fn annonce_au_moteur(bac: &Bac, allow_exec: bool) -> Value {
    let ecoute = TcpListener::bind("127.0.0.1:0").expect("écoute sur un port libre");
    let port = ecoute.local_addr().expect("adresse locale").port();
    let capte: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));

    let garde = Arc::clone(&capte);
    std::thread::spawn(move || {
        let Ok((flux, _)) = ecoute.accept() else {
            return;
        };
        let mut lecteur = BufReader::new(flux.try_clone().expect("clonage du flux"));
        let mut longueur = 0usize;
        loop {
            let mut ligne = String::new();
            if lecteur.read_line(&mut ligne).unwrap_or(0) == 0 || ligne == "\r\n" {
                break;
            }
            let bas = ligne.to_ascii_lowercase();
            if let Some(v) = bas.strip_prefix("content-length:") {
                longueur = v.trim().parse().unwrap_or(0);
            }
        }
        let mut corps = vec![0u8; longueur];
        let _ = lecteur.read_exact(&mut corps);
        *garde.lock().expect("verrou du corps capté") =
            String::from_utf8_lossy(&corps).to_string();

        let mut flux = flux;
        let _ = flux.write_all(
            b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
        );
        let _ = flux.flush();
    });

    let mut c = Command::new(selfd());
    c.arg("attacher")
        .arg("--moteur")
        .arg(format!("http://127.0.0.1:{port}"))
        .arg("--jeton-moteur")
        .arg("jeton-de-test")
        .arg("--machine")
        .arg("machine-du-test")
        .arg("--root")
        .arg(bac.racine())
        .arg("--state-dir")
        .arg(bac.etat());
    if allow_exec {
        c.arg("--allow-exec");
    }
    let sortie = lancer(c);
    assert!(
        !sortie.ok,
        "un jeton refusé doit faire sortir le nœud en erreur, pas en silence"
    );

    // Le corps est capté par le fil du faux moteur : on lui laisse un instant,
    // mais l'enfant n'est sorti qu'après avoir reçu la réponse, donc il est là.
    let debut = Instant::now();
    loop {
        let corps = capte.lock().expect("verrou du corps capté").clone();
        if !corps.is_empty() {
            return serde_json::from_str(&corps)
                .unwrap_or_else(|e| panic!("corps d'attachement non-JSON ({e}) : {corps}"));
        }
        assert!(
            debut.elapsed() < Duration::from_secs(5),
            "aucun corps d'attachement capté"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn f_lannonce_au_moteur_porte_les_capacites_verifiees_et_les_limites() {
    let bac = Bac::neuf("annonce-ouverte");
    let corps = annonce_au_moteur(&bac, false);

    assert_eq!(corps["machine"].as_str(), Some("machine-du-test"));
    let a: Vec<String> = corps["capacites"]
        .as_array()
        .expect("« capacites » doit rester un tableau de chaînes (le moteur le lit ainsi)")
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect();
    assert!(a.contains(&"write".to_string()), "racine inscriptible : {a:?}");
    assert!(
        !a.contains(&"exec".to_string()),
        "sans --allow-exec, exec ne doit pas partir dans l'annonce : {a:?}"
    );

    // Le refus part AVEC l'annonce : le moteur n'a pas à le déduire.
    let refus = corps["capacites_refusees"]
        .as_array()
        .expect("« capacites_refusees » doit accompagner l'annonce");
    assert!(
        refus.iter().any(|r| r["nom"].as_str() == Some("exec")
            && r["motif"].as_str().unwrap_or_default().contains("--allow-exec")),
        "exec doit être annoncé comme refusé ET motivé : {refus:?}"
    );

    // Les limites : racine réelle, délai, octets par flux, version du binaire.
    let reelle = std::fs::canonicalize(bac.racine()).expect("canonicalisation");
    assert_eq!(
        corps["limites"]["racine_reelle"].as_str(),
        Some(reelle.display().to_string().as_str())
    );
    assert_eq!(corps["limites"]["delai_ms_par_defaut"].as_u64(), Some(60_000));
    assert_eq!(corps["limites"]["plafond_octets_par_flux"].as_u64(), Some(262_144));
    assert_eq!(corps["limites"]["version"].as_str(), Some(env!("CARGO_PKG_VERSION")));
}

#[test]
fn g_lannonce_au_moteur_ne_promet_pas_write_sur_une_racine_en_lecture_seule() {
    let bac = Bac::neuf("annonce-lecture-seule");
    if !bac.rendre_racine_non_inscriptible() {
        eprintln!(
            "IGNORÉ : ce processus peut écrire dans une racine 0555 (root ou CAP_DAC_OVERRIDE) \
             — l'annonce ne serait pas mesurable ici."
        );
        return;
    }
    let corps = annonce_au_moteur(&bac, true);
    let a: Vec<String> = corps["capacites"]
        .as_array()
        .expect("« capacites » doit rester un tableau de chaînes")
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        !a.contains(&"write".to_string()),
        "l'annonce ne doit pas promettre write sur une racine en lecture seule : {a:?}"
    );
    assert!(a.contains(&"read".to_string()), "{a:?}");
    // Le refus est dit, avec son motif : le moteur sait POURQUOI.
    let refus = corps["capacites_refusees"]
        .as_array()
        .expect("les refus doivent accompagner l'annonce");
    assert!(
        refus.iter().any(|r| r["nom"].as_str() == Some("write")
            && !r["motif"].as_str().unwrap_or_default().is_empty()),
        "write doit être refusé AVEC un motif : {refus:?}"
    );
}
