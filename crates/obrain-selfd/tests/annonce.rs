//! L'ANNONCE, DANS LES DEUX SENS — et ce que le moteur en a gardé.
//!
//! # Ce que ce fichier ajoute à `capacites.rs`
//!
//! `capacites.rs` mesure que l'annonce d'un nœud est vraie, sur des
//! configurations qu'il CHOISIT (`write` sur une racine 0555, `exec` sans
//! `--allow-exec`). Ce fichier-ci prend l'annonce TELLE QU'ELLE PART SUR LE FIL
//! et exige que le nœud s'y tienne **dans les deux sens** :
//!
//!   (a) ce qui est ANNONCÉ s'exécute — et ce qui n'est PAS annoncé échoue
//!       vraiment. Le second sens manquait : sans lui, un nœud qui n'annonce
//!       qu'une capacité sur cinq passerait le test en étant muet sur le reste ;
//!   (b) un refus est ANNONCÉ COMME REFUSÉ, avec son motif, et ce motif est
//!       celui qui s'applique réellement — pas seulement « absent de la liste »,
//!       ce qui laisserait croire que le reste est une question de choix ;
//!   (c) les LIMITES annoncées sont celles qui coupent vraiment : comparées à un
//!       refus réel (un chemin hors racine), à un plafond réel (une sortie
//!       tronquée) et à un délai réel (une commande tuée à l'échéance) ;
//!   (d) le nœud DÉNONCE un moteur qui n'a pas gardé sa moitié « ce qu'elle ne
//!       fait pas » — la propriété du manque 4, mesurée par une vraie réponse
//!       HTTP, sans dépendre de l'implémentation du moteur.
//!
//! # Pourquoi le binaire, et pourquoi un FAUX moteur
//!
//! Le binaire, parce que la propriété porte sur ce que le nœud annonce ET sur ce
//! qu'il fait : une structure interne ne prouverait rien du chemin réel (ligne de
//! commande, politique construite au démarrage, `protocole::executer_outil`).
//! Un faux moteur, parce que ce qui est mesuré est ce qui PART — et le nœud, sur
//! un jeton refusé, sort proprement au lieu de boucler.
//!
//! Tout vit sous `/tmp` (racines, états, ports éphémères) : aucune base vivante,
//! aucun service, aucun port de l'utilisateur n'est touché.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
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

/// Délai au-delà duquel on considère que le nœud est bloqué. Un test qui pend
/// n'apprend rien ; un test qui tue son enfant et le dit, si.
const DELAI: Duration = Duration::from_secs(30);

/// Le vocabulaire des capacités qu'un nœud annonce — les cinq noms que la porte
/// d'acceptation et l'interface emploient. C'est écrit ici pour pouvoir exiger
/// le sens NÉGATIF : tout nom absent de l'annonce doit échouer pour de bon.
const VOCABULAIRE: [&str; 5] = ["read", "list", "write", "status", "exec"];

// ─────────────────────────────────────────────────────────────────────────────
// Un bac jetable, et de quoi lancer le nœud
// ─────────────────────────────────────────────────────────────────────────────

struct Bac {
    base: PathBuf,
}

impl Bac {
    fn neuf(nom: &str) -> Self {
        static COMPTEUR: AtomicU64 = AtomicU64::new(0);
        let n = COMPTEUR.fetch_add(1, Ordering::Relaxed);
        let base =
            std::env::temp_dir().join(format!("selfd-annonce-{}-{nom}-{n}", std::process::id()));
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
    /// même y écrire (root, CAP_DAC_OVERRIDE) : la propriété n'est alors pas
    /// mesurable, et le test le DIT au lieu de passer en silence.
    fn rendre_non_inscriptible(&self) -> bool {
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
        let _ = std::fs::set_permissions(self.racine(), std::fs::Permissions::from_mode(0o755));
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

    /// Le motif du refus. En mode `--call`, un refus remonte en ERREUR (code de
    /// sortie non nul), jamais en réponse vide : c'est déjà la moitié de la
    /// propriété — le silence ressemblerait à une panne.
    ///
    /// Le canal d'ERREUR seulement, et jamais `stdout` : le payload de `status`
    /// contient légitimement les motifs de refus (`politique.capacites_refusees`),
    /// et y chercher un marqueur de refus ferait passer un succès pour un refus.
    /// Constaté en écrivant ces tests : « status est ANNONCÉE et refusée par la
    /// POLITIQUE : { } » — le motif trouvé venait de l'annonce, pas de la réponse.
    fn motif(&self) -> String {
        self.stderr.trim().to_string()
    }
}

/// Un refus de POLITIQUE, reconnaissable à son motif. La distinction est
/// essentielle : « Permission denied » sur un fichier précis est un résultat
/// d'appel légitime, « ce chemin sort de la racine » est un refus de politique.
fn est_refus_de_politique(motif: &str) -> bool {
    motif.contains("sort de la racine")
        || motif.contains("chemin refusé")
        || motif.contains("NON AUTORISÉE")
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

/// Lance un enfant jusqu'à ce que son stderr porte `motif`, puis le tue. Rend
/// tout le stderr capté.
///
/// Nécessaire pour les scénarios où le nœud s'attache AVEC SUCCÈS : la boucle ne
/// rend alors jamais la main d'elle-même (c'est voulu — une machine qui se
/// déconnecte à la première contrariété ne serait pas une machine attachée).
fn lancer_jusqua(mut commande: Command, motif: &str) -> String {
    let mut enfant = commande
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("lancement de selfd");
    let erreur = enfant.stderr.take().expect("stderr pipé");
    let tampon: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let garde = Arc::clone(&tampon);
    let fil = std::thread::spawn(move || {
        for ligne in BufReader::new(erreur).lines().map_while(|l| l.ok()) {
            let mut t = garde
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            t.push_str(&ligne);
            t.push('\n');
        }
    });

    let debut = Instant::now();
    let mut sorti = false;
    loop {
        let vu = {
            let t = tampon
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            t.contains(motif)
        };
        if vu {
            break;
        }
        if enfant.try_wait().map(|s| s.is_some()).unwrap_or(false) {
            sorti = true;
            break;
        }
        if debut.elapsed() > DELAI {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = enfant.kill();
    let _ = enfant.wait();
    let _ = fil.join();

    let texte = tampon
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert!(
        texte.contains(motif),
        "le nœud n'a jamais dit « {motif} » (processus {} ; {:.1} s écoulées) :\n{texte}",
        if sorti { "sorti" } else { "tué" },
        debut.elapsed().as_secs_f32()
    );
    texte
}

/// Un appel au nœud, sur le bac donné, en mode `--call`.
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

/// Exerce UNE capacité. Ne juge pas : c'est l'appelant qui sait ce qu'il attend.
fn essayer(nom: &str, bac: &Bac, allow_exec: bool, extra: &[&str]) -> Sortie {
    match nom {
        "write" => appel(
            bac,
            "write",
            r#"{"path":"ecrit-par-l-annonce.txt","content":"bonjour"}"#,
            allow_exec,
            extra,
        ),
        "read" => {
            std::fs::write(bac.racine().join("a-lire.txt"), b"contenu").expect("pose du fichier");
            appel(bac, "read", r#"{"path":"a-lire.txt"}"#, allow_exec, extra)
        }
        "list" => {
            std::fs::write(bac.racine().join("vu-par-list.txt"), b"x").ok();
            appel(bac, "list", "", allow_exec, extra)
        }
        "status" => appel(bac, "status", "", allow_exec, extra),
        "exec" => appel(
            bac,
            "exec",
            r#"{"cmd":"/bin/sh","args":["-c","exit 0"]}"#,
            allow_exec,
            extra,
        ),
        autre => panic!(
            "capacité annoncée que ce test ne sait pas exercer : « {autre} ». Une capacité \
             nouvelle doit être exercée ici, sinon l'annonce n'est pas vérifiée."
        ),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Le faux moteur
// ─────────────────────────────────────────────────────────────────────────────

/// Lit une requête HTTP minimale et rend (chemin, corps).
fn lire_requete(flux: &mut TcpStream) -> Option<(String, String)> {
    let mut lecteur = BufReader::new(flux.try_clone().ok()?);
    let mut premiere = String::new();
    if lecteur.read_line(&mut premiere).ok()? == 0 {
        return None;
    }
    let chemin = premiere
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_string();
    let mut longueur = 0usize;
    loop {
        let mut ligne = String::new();
        if lecteur.read_line(&mut ligne).ok()? == 0 || ligne.trim().is_empty() {
            break;
        }
        let bas = ligne.to_ascii_lowercase();
        if let Some(v) = bas.strip_prefix("content-length:") {
            longueur = v.trim().parse().unwrap_or(0);
        }
    }
    let mut corps = vec![0u8; longueur];
    if longueur > 0 {
        let _ = lecteur.read_exact(&mut corps);
    }
    Some((chemin, String::from_utf8_lossy(&corps).to_string()))
}

fn repondre(flux: &mut TcpStream, code: u16, corps: &str) {
    let r = format!(
        "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{corps}",
        corps.len()
    );
    let _ = flux.write_all(r.as_bytes());
    let _ = flux.flush();
}

/// Un faux moteur qui GARDE le corps d'attachement et REFUSE le jeton (401).
///
/// Sur un jeton refusé, `attache::boucle` s'arrête net : pas de boucle infinie,
/// donc pas de test qui pend. Ce qui nous intéresse est ce qui est PARTI.
fn annonce_partie(bac: &Bac, allow_exec: bool, extra: &[&str], machine: &str) -> Value {
    let ecoute = TcpListener::bind("127.0.0.1:0").expect("écoute sur un port libre");
    let port = ecoute.local_addr().expect("adresse locale").port();
    let capte: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));

    let garde = Arc::clone(&capte);
    std::thread::spawn(move || {
        let Ok((mut flux, _)) = ecoute.accept() else {
            return;
        };
        if let Some((_, corps)) = lire_requete(&mut flux) {
            *garde
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = corps;
        }
        repondre(&mut flux, 401, "{}");
    });

    let mut c = Command::new(selfd());
    c.arg("attacher")
        .arg("--moteur")
        .arg(format!("http://127.0.0.1:{port}"))
        .arg("--jeton-moteur")
        .arg("jeton-de-test")
        .arg("--machine")
        .arg(machine)
        .arg("--root")
        .arg(bac.racine())
        .arg("--state-dir")
        .arg(bac.etat());
    if allow_exec {
        c.arg("--allow-exec");
    }
    for a in extra {
        c.arg(a);
    }
    let sortie = lancer(c);
    assert!(
        !sortie.ok,
        "un jeton refusé doit faire sortir le nœud en erreur, pas en silence"
    );

    let debut = Instant::now();
    loop {
        let corps = capte
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
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

/// Un faux moteur qui ACCEPTE l'attachement, puis rend sur
/// `GET /api/remote/clients` ce que le test décide. Rend le port.
fn moteur_qui_accepte(clients: Value) -> u16 {
    let ecoute = TcpListener::bind("127.0.0.1:0").expect("écoute sur un port libre");
    let port = ecoute.local_addr().expect("adresse locale").port();
    std::thread::spawn(move || {
        for flux in ecoute.incoming() {
            let Ok(mut flux) = flux else { continue };
            let clients = clients.clone();
            std::thread::spawn(move || {
                let Some((chemin, _)) = lire_requete(&mut flux) else {
                    return;
                };
                if chemin.starts_with("/api/remote/attacher") {
                    repondre(
                        &mut flux,
                        200,
                        r#"{"client_id":"cl-faux-1","compte":"admin","attente_max_s":25}"#,
                    );
                } else if chemin.starts_with("/api/remote/clients") {
                    let corps =
                        serde_json::json!({"compte": "admin", "clients": clients}).to_string();
                    repondre(&mut flux, 200, &corps);
                } else if chemin.starts_with("/api/remote/ordres") {
                    // 204 : « rien pour toi, rappelle ». Le rythme NORMAL du canal.
                    repondre(&mut flux, 204, "");
                } else {
                    repondre(&mut flux, 404, "{}");
                }
            });
        }
    });
    port
}

/// Attache le nœud à un faux moteur qui accepte, et rend son stderr.
fn attacher_et_lire(bac: &Bac, clients: Value, motif_attendu: &str) -> String {
    let port = moteur_qui_accepte(clients);
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
        .arg(bac.etat())
        .arg("--allow-exec");
    lancer_jusqua(c, motif_attendu)
}

/// Le nom de la machine tel que le moteur le rendrait, et l'identifiant rendu à
/// l'attachement (`cl-faux-1` dans `moteur_qui_accepte`).
fn client_du_moteur(annonce: Option<Value>) -> Value {
    let mut c = serde_json::json!({
        "id": "cl-faux-1",
        "machine": "machine-du-test",
        "capacites": ["read", "list", "status"],
        "vivant": true,
    });
    if let Some(a) = annonce {
        c["annonce"] = a;
    }
    serde_json::json!([c])
}

// ─────────────────────────────────────────────────────────────────────────────
// (a) Ce qui est annoncé s'exécute — ET ce qui n'est pas annoncé échoue
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn a_l_annonce_est_exactement_ce_qui_s_execute_dans_les_deux_sens() {
    for (allow_exec, nom) in [(false, "sans-exec"), (true, "avec-exec")] {
        let bac = Bac::neuf(&format!("a-{nom}"));
        // L'annonce TELLE QU'ELLE PART, captée sur le fil.
        let corps = annonce_partie(&bac, allow_exec, &[], "machine-du-test");
        let ouvertes: Vec<String> = corps["capacites"]
            .as_array()
            .unwrap_or_else(|| panic!("« capacites » doit être un tableau de chaînes : {corps}"))
            .iter()
            .map(|v| v.as_str().expect("capacité non textuelle").to_string())
            .collect();
        assert!(
            !ouvertes.is_empty(),
            "un nœud qui n'annonce RIEN n'annonce pas : {corps}"
        );

        // Sens 1 : ce qui est annoncé s'exécute vraiment.
        for nom in &ouvertes {
            let s = essayer(nom, &bac, allow_exec, &[]);
            assert!(
                s.ok,
                "« {nom} » est ANNONCÉE mais l'appel échoue : {}",
                s.motif()
            );
            // Le canal d'erreur est vide sur un succès : un refus de politique
            // y laisserait son motif.
            assert!(
                !est_refus_de_politique(&s.motif()),
                "« {nom} » est ANNONCÉE et refusée par la POLITIQUE : {}",
                s.motif()
            );
        }

        // Sens 2, celui qui manquait : ce qui N'EST PAS annoncé échoue pour de
        // bon. Sans lui, un nœud muet sur la moitié de ses capacités passerait.
        for nom in VOCABULAIRE {
            if ouvertes.iter().any(|o| o == nom) {
                continue;
            }
            let s = essayer(nom, &bac, allow_exec, &[]);
            assert!(
                !s.ok,
                "« {nom} » n'est PAS annoncée, et pourtant l'appel RÉUSSIT : l'annonce \
                 sous-déclare, donc le moteur croit la machine plus pauvre qu'elle n'est"
            );
        }
    }
}

#[test]
fn a_bis_un_changement_de_politique_change_l_annonce_et_l_usage_ensemble() {
    // Le même bac, deux politiques : ce qui change dans l'annonce doit changer
    // dans le même sens à l'usage. C'est ce qui distingue « mesuré » de
    // « recopié » — un catalogue figé ne suivrait pas.
    let bac = Bac::neuf("a-bis");
    let sans = annonce_partie(&bac, false, &[], "m1");
    let avec = annonce_partie(&bac, true, &[], "m2");
    assert!(
        !sans["capacites"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "exec"),
        "sans --allow-exec, exec ne doit pas être annoncé : {}",
        sans["capacites"]
    );
    assert!(
        avec["capacites"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "exec"),
        "avec --allow-exec, exec doit l'être : {}",
        avec["capacites"]
    );
    // Et l'usage suit dans le même sens : la capacité vient d'être OUVERTE, donc
    // elle s'exécute réellement.
    let s = essayer("exec", &bac, true, &[]);
    assert!(
        s.ok,
        "exec annoncée sous --allow-exec doit s'exécuter : {}",
        s.motif()
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// (b) Un refus est ANNONCÉ COMME REFUSÉ, motivé, et ce motif s'applique
// ─────────────────────────────────────────────────────────────────────────────

/// Le refus annoncé pour `nom`, ou panique en disant ce que l'annonce portait.
fn refus_annonce(corps: &Value, nom: &str) -> Value {
    let refus = corps["capacites_refusees"]
        .as_array()
        .unwrap_or_else(|| panic!("« capacites_refusees » doit accompagner l'annonce : {corps}"));
    let trouve = refus
        .iter()
        .find(|r| r["nom"].as_str() == Some(nom))
        .unwrap_or_else(|| {
            panic!(
                "« {nom} » n'est pas ANNONCÉ COMME REFUSÉ — absent de la liste n'est pas la \
                 même chose que refusé : {refus:?}"
            )
        })
        .clone();
    assert_eq!(trouve["exercable"].as_bool(), Some(false), "{trouve}");
    let motif = trouve["motif"].as_str().unwrap_or_default().to_string();
    assert!(
        !motif.trim().is_empty(),
        "un refus sans motif ne se corrige pas : {trouve}"
    );
    trouve
}

#[test]
fn b_write_refuse_par_la_racine_est_annonce_motive_et_le_reste() {
    let bac = Bac::neuf("b-lecture-seule");
    if !bac.rendre_non_inscriptible() {
        eprintln!(
            "IGNORÉ : ce processus peut écrire dans une racine 0555 (root ou CAP_DAC_OVERRIDE) \
             — la propriété n'est pas mesurable ici."
        );
        return;
    }
    let corps = annonce_partie(&bac, true, &[], "machine-ro");
    let annonces: Vec<&str> = corps["capacites"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c.as_str())
        .collect();

    // Les deux moitiés ne se contredisent pas : ce qui est refusé n'est pas
    // aussi annoncé.
    assert!(
        !annonces.contains(&"write"),
        "write est refusé et ne doit donc pas figurer dans les capacités exercables : {annonces:?}"
    );
    let refus = refus_annonce(&corps, "write");

    // ── ET LE REFUS EST TENU ────────────────────────────────────────────────
    let s = essayer("write", &bac, true, &[]);
    assert!(
        !s.ok,
        "write est annoncé refusé : l'appel doit échouer, or il réussit"
    );
    assert!(
        !bac.racine().join("ecrit-par-l-annonce.txt").exists(),
        "et rien ne doit avoir été écrit"
    );

    // ── LE MOTIF ANNONCÉ NOMME LA MÊME CHOSE QUE LE REFUS RÉEL ──────────────
    // C'est ce qui rend l'annonce ACTIONNABLE : le motif annoncé dit la racine,
    // et le refus réel la nomme aussi. Deux textes au sens identique.
    let reelle = std::fs::canonicalize(bac.racine()).expect("canonicalisation de la racine");
    let motif_annonce = refus["motif"].as_str().unwrap_or_default();
    assert!(
        motif_annonce.contains(&reelle.display().to_string()),
        "le motif annoncé doit nommer la racine qui refuse : « {motif_annonce} » vs {}",
        reelle.display()
    );
    assert!(
        s.motif().contains(&reelle.display().to_string()),
        "le refus réel doit nommer la même racine : {}",
        s.motif()
    );
}

#[test]
fn b_bis_exec_ferme_est_annonce_avec_le_geste_qui_l_ouvre() {
    let bac = Bac::neuf("b-exec-ferme");
    let corps = annonce_partie(&bac, false, &[], "machine-sans-exec");

    assert!(
        !corps["capacites"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "exec"),
        "sans --allow-exec, exec ne doit pas être dans les capacités : {}",
        corps["capacites"]
    );
    let refus = refus_annonce(&corps, "exec");
    let motif_annonce = refus["motif"].as_str().unwrap_or_default();
    assert!(
        motif_annonce.contains("--allow-exec"),
        "le motif annoncé doit dire COMMENT ouvrir la capacité, sinon il ne se corrige pas : \
         « {motif_annonce} »"
    );

    // Le refus réel porte le MÊME marqueur : le motif annoncé est bien celui qui
    // s'applique, et non un texte décoratif.
    let s = essayer("exec", &bac, false, &[]);
    assert!(!s.ok, "exec doit être refusé sans --allow-exec");
    assert!(
        est_refus_de_politique(&s.motif()),
        "et par la POLITIQUE : {}",
        s.motif()
    );
    assert!(
        s.motif().contains("--allow-exec"),
        "le refus réel doit porter le même marqueur que l'annonce : {}",
        s.motif()
    );

    // Et la moitié ouverte n'est pas sacrifiée au passage.
    assert!(s.motif().len() > 0);
    let w = essayer("write", &bac, false, &[]);
    assert!(
        w.ok,
        "write reste ouvert sur une racine inscriptible : {}",
        w.motif()
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// (c) Les LIMITES annoncées sont celles qui s'appliquent
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn c_les_limites_annoncees_sont_celles_qui_coupent_vraiment() {
    let bac = Bac::neuf("c-limites");
    let limites_flags: [&str; 4] = ["--max-output", "4096", "--timeout-ms", "800"];
    let corps = annonce_partie(&bac, true, &limites_flags, "machine-limitee");
    let lim = &corps["limites"];

    // 1. La racine annoncée est la racine RÉELLE (celle qui confine).
    let reelle = std::fs::canonicalize(bac.racine()).expect("canonicalisation");
    assert_eq!(
        lim["racine_reelle"].as_str(),
        Some(reelle.display().to_string().as_str())
    );
    assert_eq!(lim["delai_ms_par_defaut"].as_u64(), Some(800), "{lim}");
    assert_eq!(lim["plafond_octets_par_flux"].as_u64(), Some(4096), "{lim}");
    assert_eq!(lim["exec"].as_str(), Some("ouvert"));
    assert_eq!(lim["version"].as_str(), Some(env!("CARGO_PKG_VERSION")));

    // ── 2. UN REFUS RÉEL : un chemin hors racine ────────────────────────────
    // La limite de confinement est annoncée par la racine réelle, et c'est bien
    // ELLE qui refuse — le motif nomme la racine annoncée.
    let evasion = appel(
        &bac,
        "read",
        r#"{"path":"../../etc/passwd"}"#,
        true,
        &limites_flags,
    );
    assert!(!evasion.ok, "sortir de la racine doit être refusé");
    assert!(
        est_refus_de_politique(&evasion.motif()),
        "{}",
        evasion.motif()
    );
    assert!(
        evasion.motif().contains(&reelle.display().to_string()),
        "le refus doit nommer la racine ANNONCÉE : {} vs {}",
        evasion.motif(),
        reelle.display()
    );

    // ── 3. UN PLAFOND RÉEL : la sortie tronquée au plafond annoncé ──────────
    let gros = "a".repeat(9000);
    std::fs::write(bac.racine().join("gros.txt"), &gros).expect("pose du gros fichier");
    let v = appel(&bac, "read", r#"{"path":"gros.txt"}"#, true, &limites_flags).json();
    assert_eq!(
        v["bytes"].as_u64(),
        Some(9000),
        "l'appel dit la taille réelle : {v}"
    );
    assert_eq!(v["tronque"].as_bool(), Some(true), "{v}");
    assert_eq!(
        v["content"].as_str().map(str::len),
        Some(4096),
        "le plafond ANNONCÉ est celui qui coupe réellement : {v}"
    );

    // ── 4. UN DÉLAI RÉEL : la commande tuée à l'échéance annoncée ───────────
    // `exec sleep 30` et non `sh -c "sleep 30"` : avec `exec`, le shell SE
    // REMPLACE par `sleep`, donc le processus tué est celui qui tient les tuyaux.
    // Mesuré, et c'est tout l'objet de `c_bis` ci-dessous : avec un PETIT-FILS
    // qui hérite des tuyaux, le délai tue bien l'enfant mais l'appel attend
    // encore la fermeture des tuyaux.
    let debit = Instant::now();
    let e = appel(
        &bac,
        "exec",
        r#"{"cmd":"/bin/sh","args":["-c","exec sleep 30"]}"#,
        true,
        &limites_flags,
    )
    .json();
    let ecoule = debit.elapsed();
    assert_eq!(
        e["timeout"].as_bool(),
        Some(true),
        "le délai ANNONCÉ (800 ms) doit tuer la commande, pas attendre 30 s : {e}"
    );
    assert!(
        ecoule >= Duration::from_millis(700) && ecoule < Duration::from_secs(10),
        "et il doit tuer à l'échéance annoncée, ni avant ni bien après : {ecoule:?}"
    );
    assert_eq!(
        e["exitCode"],
        Value::Null,
        "un enfant tué n'a pas de code de sortie : {e}"
    );
    assert!(
        e["durationMs"].as_u64().unwrap_or(u64::MAX) < 5_000,
        "et la DURÉE rapportée doit être celle de l'échéance, pas celle du programme : {e}"
    );
}

/// ── CONSTAT, PAS PROPRIÉTÉ : ce que la limite annoncée NE borne PAS ─────────
///
/// Le nœud annonce `delai_ms_par_defaut`. Mesuré : ce délai borne **l'enfant
/// lancé**, pas **l'appel**. Quand la commande engendre un petit-fils qui hérite
/// des tuyaux (`sh -c "sleep 5"`), le nœud tue bien le shell à l'échéance
/// (`timeout: true`) mais reste ensuite bloqué sur la lecture des tuyaux jusqu'à
/// ce que le petit-fils les ferme — la mesure du 2026-09-16 donne `durationMs:
/// 5001` et 5,0 s d'horloge pour un délai annoncé de 800 ms.
///
/// Ce test décrit donc une LIMITE CONSTATÉE, et il le dit dans son nom : il
/// TOMBERA le jour où l'appel sera réellement borné par le délai. Ce jour-là, il
/// faudra le RETOURNER (exiger `durationMs < 5000`), pas le supprimer — sans
/// quoi la limite annoncée resterait une promesse que personne ne vérifie.
///
/// La correction est dans `outils::exec` (lecture des tuyaux bornée par le même
/// délai), pas dans ce fichier : elle est signalée en écart.
#[test]
fn c_bis_constat_la_limite_annoncee_borne_l_enfant_et_pas_l_appel() {
    let bac = Bac::neuf("c-bis-petit-fils");
    let limites_flags: [&str; 4] = ["--max-output", "4096", "--timeout-ms", "800"];
    let debit = Instant::now();
    let e = appel(
        &bac,
        "exec",
        r#"{"cmd":"/bin/sh","args":["-c","sleep 3"]}"#,
        true,
        &limites_flags,
    )
    .json();
    let ecoule = debit.elapsed();

    assert_eq!(
        e["timeout"].as_bool(),
        Some(true),
        "l'enfant EST tué à l'échéance annoncée — ça, la limite le tient : {e}"
    );
    assert!(
        ecoule >= Duration::from_secs(2),
        "CONSTAT : l'appel survit au délai annoncé tant qu'un petit-fils garde les \
         tuyaux. S'il est retombé sous les 2 s, la correction a eu lieu : retourner \
         ce test. Mesuré : {ecoule:?}, {e}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// (d) Le nœud DÉNONCE un moteur qui n'a pas gardé sa moitié « ce qu'elle ne
//     fait pas » — mesuré sur de vraies réponses HTTP
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn d_le_noeud_denonce_un_moteur_qui_avale_ses_refus() {
    let bac = Bac::neuf("d-moteur-muet");
    // Ce moteur-là est antérieur : il accepte la machine et ne DIT RIEN de ce
    // qu'il en a gardé. C'est exactement l'état du moteur en service le
    // 2026-09-16 — celui qui ne retenait que `capacites`.
    let texte = attacher_et_lire(
        &bac,
        client_du_moteur(None),
        "ce moteur ne dit pas ce qu'il a gardé",
    );
    assert!(
        texte.contains("perdue en route"),
        "le nœud doit dire la CONSÉQUENCE, pas seulement le fait : {texte}"
    );
    assert!(
        texte.contains("refus mesuré"),
        "et dire COMBIEN de refus risquent d'être perdus : {texte}"
    );
}

#[test]
fn d_bis_un_moteur_qui_garde_la_moitie_est_dit_incomplet_et_laquelle() {
    let bac = Bac::neuf("d-moteur-partiel");
    // Ce moteur-ci connaît l'annonce, mais il n'a pas gardé les refus — c'est
    // précisément le manque 4, vu depuis la machine.
    let texte = attacher_et_lire(
        &bac,
        client_du_moteur(Some(serde_json::json!({
            "complete": false,
            "manquante": ["capacites_refusees"],
        }))),
        "le moteur a gardé l'annonce INCOMPLÈTE",
    );
    assert!(
        texte.contains("capacites_refusees"),
        "le nœud doit NOMMER ce qui manque, sinon l'opérateur ne peut rien corriger : {texte}"
    );
    assert!(
        texte.contains("échec arrivera après coup"),
        "et dire ce que ça coûte à l'utilisateur : {texte}"
    );
}

#[test]
fn d_ter_un_moteur_qui_garde_tout_le_dit_et_le_noeud_le_rapporte() {
    let bac = Bac::neuf("d-moteur-complet");
    // Le pendant, sans lequel les deux tests ci-dessus passeraient sur un nœud
    // qui râle tout le temps.
    let texte = attacher_et_lire(
        &bac,
        client_du_moteur(Some(serde_json::json!({
            "complete": true,
            "manquante": [],
        }))),
        "annonce gardée en entier",
    );
    assert!(
        texte.contains("capacité(s) exercable(s)") && texte.contains("refus motivé(s)"),
        "le compte des DEUX moitiés doit être rapporté : {texte}"
    );
}
