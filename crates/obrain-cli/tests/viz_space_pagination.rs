//! PAGINATION D'UN NIVEAU — `/api/viz/space`, et ses cas limites.
//!
//! Ce que cette route sert, et pourquoi elle se teste par le BINAIRE : la
//! pagination (`offset`) n'est pas une commodité, c'est ce qui a remplacé un
//! PLAFOND SILENCIEUX. Le front coupait à 24 une liste que le serveur servait
//! jusqu'à 200, si bien que 176 groupes étaient inatteignables par AUCUN geste et
//! que le panneau annonçait « 24 ensembles » sur un niveau qui en portait 200.
//!
//! Les cas limites testés ici, et ce qu'ils protègent :
//!   · `offset` ABSENT ou `0` → la PREMIÈRE page (c'est le cas par défaut : une
//!     page zéro vide rendrait le premier niveau de la navigation vide) ;
//!   · `offset` de la page suivante → **autre chose** que la première page, et
//!     c'est le test qui attrape le défaut le plus vicieux de cette route :
//!     l'OFFSET DOIT FAIRE PARTIE DE LA CLÉ DE CACHE. Sans lui, la page 2 se
//!     servait la page 1 depuis le cache — un mensonge silencieux, et le cache
//!     disque le rejouait même après un redémarrage ;
//!   · `offset` malformé, négatif, ou au-delà du plafond → borné, jamais une
//!     erreur, et l'`offset` RENDU dit ce qui a été appliqué ;
//!   · un store SANS nœud → page vide qui ne s'annonce PAS tronquée ;
//!   · une page PLEINE → `tronque: true`, seul aveu honnête de « il y en a
//!     d'autres » sans compter les valeurs distinctes.
//!
//! Le serveur est lancé sur un port libre, avec un store et un dossier de cache
//! TEMPORAIRES : aucun test ne touche un service ni un cache de l'utilisateur.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn port_libre() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Un GET HTTP/1.1 minimal : la réponse est rendue telle quelle, et le corps
/// seul est décodé par l'appelant. Pas de client HTTP en dépendance pour un test.
fn get(addr: &str, chemin: &str) -> (u16, String) {
    let mut s = TcpStream::connect(addr).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    write!(s, "GET {chemin} HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n").unwrap();
    let mut buf = String::new();
    s.read_to_string(&mut buf).unwrap();
    let statut: u16 = buf.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    let corps = buf.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    (statut, corps)
}

/// Un serveur `viz serve` possédé par le test : il meurt avec lui, même si le
/// test panique (sans quoi un port reste pris et la campagne suivante échoue
/// pour une raison qui n'a rien à voir avec le produit).
struct Serveur {
    enfant: Child,
    port: u16,
    _td: tempfile::TempDir,
}

impl Drop for Serveur {
    fn drop(&mut self) {
        let _ = self.enfant.kill();
        let _ = self.enfant.wait();
    }
}

impl Serveur {
    /// Lance un serveur sur un store déjà peuplé, cache DISQUE isolé.
    fn sur(store: &Path) -> Serveur {
        let td = tempfile::tempdir().unwrap();
        let port = port_libre();
        let enfant = Command::new(env!("CARGO_BIN_EXE_obrain"))
            .arg("viz")
            .arg("serve")
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string())
            .arg("--store")
            .arg(store)
            .arg("--enrich-ricci")
            .arg("0")
            .env("OBRAIN_VIZ_CACHE_DIR", td.path())
            // ── L'ENFANT N'HÉRITE PAS DE L'IAM DE LA MACHINE ───────────────
            // Défaut mesuré le 2026-09-16 : sur une machine où `OBRAIN_IAM` et
            // `OBRAIN_IAM_STORE` sont armés (le poste de développement), le
            // serveur lancé ici refuse de servir — et les six cas de ce fichier
            // échouaient sur « le serveur n'écoute pas », sans qu'aucune ligne du
            // produit ait changé. L'isolement porte sur l'ENFANT (un processus
            // distinct) : il ne touche pas à l'environnement du test, donc il
            // n'introduit aucune course.
            .env_remove("OBRAIN_IAM")
            .env_remove("OBRAIN_IAM_STORE")
            .env_remove("OBRAIN_IAM_ACCOUNT")
            .env_remove("OBRAIN_IAM_TOKEN")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn viz serve");
        let s = Serveur { enfant, port, _td: td };
        let debut = Instant::now();
        while debut.elapsed() < Duration::from_secs(30) {
            if let Ok(mut c) = TcpStream::connect(format!("127.0.0.1:{}", s.port)) {
                let _ = write!(c, "GET /api/info HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n");
                let mut b = String::new();
                if c.read_to_string(&mut b).is_ok() && b.contains("200") {
                    return s;
                }
            }
            std::thread::sleep(Duration::from_millis(150));
        }
        panic!("le serveur n'écoute pas sur {}", s.port);
    }

    fn space(&self, store: &Path, offset: Option<&str>, rank: usize) -> serde_json::Value {
        let mut q = format!(
            "/api/viz/space?store={}&rank={rank}&links=0",
            store.display()
        );
        if let Some(o) = offset {
            q.push_str(&format!("&offset={o}"));
        }
        let (statut, corps) = get(&format!("127.0.0.1:{}", self.port), &q);
        assert_eq!(statut, 200, "la route doit répondre 200 (offset={offset:?}) : {corps}");
        serde_json::from_str(&corps).expect("corps JSON")
    }

    /// La CHAUFFE, et il faut savoir pourquoi elle existe.
    ///
    /// Défaut CONSTATÉ le 2026-09-16, reproduit à l'identique sur le binaire de
    /// service ET sur celui du tronc consolidé — donc ANTÉRIEUR à cette
    /// consolidation. Mesuré sur un processus neuf, quatre appels successifs :
    ///
    ///     offset=1    (1er appel du processus)  →   0 ensemble   ← FAUX (200 attendus)
    ///     offset=201  (2e appel, autre texte)   →  49 ensembles  ← juste
    ///     offset=0    (3e appel, texte neuf)    → 200 ensembles  ← juste
    ///     offset=1    (relu, MÊME texte)        →   0 ensemble   ← le faux est EN CACHE
    ///
    /// Autrement dit : **la première requête d'un processus qui emprunte le
    /// chemin `CONTAINS` rend une page vide, quel que soit l'offset**, et le
    /// résultat faux est ensuite mis en CACHE sous le texte de la requête
    /// (mémoire ET disque). Le prédicat racine de cette route est justement
    /// `qualified_name CONTAINS '.'` (`in_prefix`, préfixe vide) : c'est donc le
    /// PREMIER NIVEAU que l'utilisateur ouvre qui reste vide.
    ///
    /// Le détail qui a coûté une mesure fausse : chauffer avec le texte qu'on
    /// veut mesurer ne chauffe RIEN — on empoisonne le cache de ce texte avec la
    /// réponse vide. La chauffe doit donc consommer le défaut sur un texte qu'on
    /// ne mesurera jamais, et c'est ce que fait cette fonction.
    ///
    /// Ce n'est pas une excuse : c'est ce qui permet aux tests ci-dessous de
    /// mesurer la PAGINATION au lieu de mesurer ce défaut-là. Le défaut lui-même
    /// est mesuré par `offset_absent_rend_la_premiere_page`, sans chauffe.
    fn chauffer(&self, store: &Path) {
        let _ = self.space(store, Some("999999"), 1); // écrêté à 40 000 : page vide, jamais mesurée
    }
}

/// Peuple un store neuf de `n` nœuds `x.<préfixe><i>`, en UNE requête.
///
/// Un groupe par nœud, et des noms ORDONNÉS : c'est ce qui rend la pagination
/// vérifiable. Les effectifs sont tous égaux (1), donc l'ordre entre eux est
/// l'ordre de parcours — mesuré stable sur ce moteur (deux processus, quatre
/// exécutions, même découpage), et c'est ce que les tests vérifient.
fn peupler(store: &Path, n: usize, prefixe: &str) {
    let init = Command::new(env!("CARGO_BIN_EXE_obrain"))
        .arg("init")
        .arg(store)
        .output()
        .expect("init");
    assert!(init.status.success(), "init : {}", String::from_utf8_lossy(&init.stderr));
    let motifs: Vec<String> = (0..n)
        .map(|i| format!("(:G {{qualified_name: 'x.{prefixe}{i:04}'}})"))
        .collect();
    let q = format!("INSERT {}", motifs.join(", "));
    let o = Command::new(env!("CARGO_BIN_EXE_obrain"))
        .arg("query")
        .arg(store)
        .arg(&q)
        .output()
        .expect("query");
    assert!(o.status.success(), "peuplement : {}", String::from_utf8_lossy(&o.stderr));
}

fn noms(page: &serde_json::Value) -> Vec<String> {
    page["ensembles"]
        .as_array()
        .map(|v| {
            v.iter()
                .filter_map(|e| e["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn la_page_suivante_n_est_ni_la_premiere_ni_la_meme() {
    // 250 groupes : la première page est PLEINE (200), la deuxième en porte 50.
    let td = tempfile::tempdir().unwrap();
    let store = td.path().join("kb-pagination");
    peupler(&store, 250, "g");
    let srv = Serveur::sur(&store);
    srv.chauffer(&store);

    let p0 = srv.space(&store, Some("0"), 1);
    let p2 = srv.space(&store, Some("200"), 1);
    let (n0, n2) = (noms(&p0), noms(&p2));

    assert_eq!(n0.len(), 200, "la première page porte exactement le plafond du serveur");
    assert_eq!(n2.len(), 50, "la deuxième page porte ce qui reste");
    assert_eq!(p0["offset"].as_u64(), Some(0));
    assert_eq!(p2["offset"].as_u64(), Some(200));

    // LE CŒUR DU TEST : les deux pages doivent être DISJOINTES. Si l'offset
    // n'entre pas dans la clé de cache, la page 2 est servie par la page 1 — et
    // le défaut ne se voit pas à l'œil nu, puisque la réponse est bien formée.
    let communs: Vec<&String> = n0.iter().filter(|x| n2.contains(x)).collect();
    assert!(communs.is_empty(), "les deux pages se recouvrent : {communs:?}");
    let mut toutes = n0.clone();
    toutes.extend(n2.clone());
    toutes.sort();
    toutes.dedup();
    assert_eq!(toutes.len(), 250, "les deux pages couvrent les 250 groupes, sans doublon");
}

#[test]
fn les_bornes_du_dernier_element_et_de_l_apres_dernier() {
    // Les deux bornes qui décident si un niveau est épuisé : le DERNIER élément
    // (offset = total − 1) et celui d'APRÈS le dernier (offset = total). Se
    // tromper d'un cran fait disparaître un groupe, ou fait proposer une page
    // qui n'existe pas.
    let td = tempfile::tempdir().unwrap();
    let store = td.path().join("kb-bornes");
    peupler(&store, 30, "g");
    let srv = Serveur::sur(&store);
    srv.chauffer(&store);

    let dernier = srv.space(&store, Some("29"), 1);
    assert_eq!(noms(&dernier).len(), 1, "offset = total − 1 rend le dernier groupe");
    assert_eq!(dernier["tronque"].as_bool(), Some(false));

    let apres = srv.space(&store, Some("30"), 1);
    assert!(noms(&apres).is_empty(), "offset = total ne rend rien : le niveau est épuisé");
    assert_eq!(apres["tronque"].as_bool(), Some(false), "une page vide n'est pas pleine");

    let premier = srv.space(&store, Some("0"), 1);
    assert_eq!(noms(&premier).len(), 30, "le store porte 30 groupes, tous sur une page");
    assert_eq!(premier["tronque"].as_bool(), Some(false), "30 < 200 : le serveur n'a rien coupé");
}

#[test]
fn la_page_pleine_s_annonce_tronquee_et_la_derniere_non() {
    let td = tempfile::tempdir().unwrap();
    let store = td.path().join("kb-tronque");
    peupler(&store, 250, "g");
    let srv = Serveur::sur(&store);
    srv.chauffer(&store);

    // Une page pleine (200 lignes rendues) est le SEUL signal honnête de « il y a
    // une suite » : le serveur ne compte pas les valeurs distinctes, ce qui
    // coûterait un second parcours complet pour un compteur.
    let pleine = srv.space(&store, Some("1"), 1);
    assert_eq!(noms(&pleine).len(), 200);
    assert_eq!(pleine["tronque"].as_bool(), Some(true), "une page pleine s'annonce tronquée");

    // La dernière page ne l'est pas : au-delà, il n'y a rien.
    let derniere = srv.space(&store, Some("200"), 1);
    assert_eq!(noms(&derniere).len(), 50);
    assert_eq!(derniere["tronque"].as_bool(), Some(false));
}

#[test]
fn offset_malforme_ou_hors_bornes_est_borne_et_le_dit() {
    let td = tempfile::tempdir().unwrap();
    let store = td.path().join("kb-offset");
    peupler(&store, 30, "g");
    let srv = Serveur::sur(&store);
    srv.chauffer(&store);

    // `usize::from_str` refuse « abc » et « -5 » : la route retombe sur 0, elle
    // ne renvoie pas d'erreur pour un paramètre qu'un client a mal écrit. Et
    // l'`offset` RENDU est celui qui a été APPLIQUÉ, jamais celui demandé.
    for (demande, attendu) in [("abc", 0u64), ("-5", 0), ("1.5", 0), ("", 0)] {
        let p = srv.space(&store, Some(demande), 1);
        assert_eq!(
            p["offset"].as_u64(),
            Some(attendu),
            "offset={demande:?} doit être ramené à {attendu}"
        );
    }

    // Au-delà du plafond de pagination (40 000), l'offset est ÉCRÊTÉ : une page
    // très profonde ne sert personne et ferait payer un SKIP coûteux.
    let loin = srv.space(&store, Some("999999"), 1);
    assert_eq!(loin["offset"].as_u64(), Some(40_000));
    assert!(noms(&loin).is_empty(), "au-delà de ce que le store porte, la page est vide");
    assert_eq!(loin["tronque"].as_bool(), Some(false));
}

#[test]
fn un_store_sans_noeud_rend_une_page_vide_qui_ne_se_dit_pas_tronquee() {
    // Le cas limite du vide : une page vide n'est PAS une page pleine. Confondre
    // les deux ferait proposer « densifier » à l'infini sur un monde vide.
    let td = tempfile::tempdir().unwrap();
    let store = td.path().join("kb-vide");
    assert!(Command::new(env!("CARGO_BIN_EXE_obrain")).arg("init").arg(&store).output().unwrap().status.success());
    let srv = Serveur::sur(&store);

    let p = srv.space(&store, None, 1);
    assert!(noms(&p).is_empty());
    assert_eq!(p["tronque"].as_bool(), Some(false), "vide n'est pas « il y a une suite »");
    assert_eq!(p["offset"].as_u64(), Some(0));
}

/// LE CAS PAR DÉFAUT DE LA ROUTE — ET LE TEST QUI A FAIT CORRIGER UN DÉFAUT RÉEL.
///
/// `offset` absent ou nul est ce que la navigation demande en arrivant : la
/// première page, celle qui doit porter le premier lot d'ensembles. Elle rendait
/// une page VIDE, parce que dans un processus neuf la première exécution d'un
/// filtre `CONTAINS` rendait zéro ligne — or le prédicat racine de cette route est
/// `qualified_name CONTAINS '.'`. Le résultat faux était ensuite mis en CACHE sous
/// le texte de la requête, mémoire ET disque.
///
/// La cause a été établie dans `obrain-substrate` : sur une base ouverte en
/// LECTURE SEULE (le mode de `viz serve`), le chemin qui suit la construction de
/// l'index de trigrammes faisait `candidats(...).unwrap_or_default()` — un `None`
/// (« l'index n'a rien à dire ») y devenait un vecteur vide, c'est-à-dire « aucune
/// valeur ne correspond », affirmé sans preuve. Corrigé le 2026-09-16 : le silence
/// rend désormais la main au balayage.
///
/// Ce test n'est plus ignoré : il est le gardien de cette correction, vu d'en
/// haut — par le HTTP, sans chauffe, exactement comme la navigation appelle.
///
/// Reproduit à l'identique sur `banc-obrain-tronc` (binaire de service) et sur le
/// tronc consolidé : le défaut est ANTÉRIEUR à cette consolidation.
#[test]
fn offset_absent_rend_la_premiere_page() {
    let td = tempfile::tempdir().unwrap();
    let store = td.path().join("kb-premiere-page");
    peupler(&store, 250, "g");
    let srv = Serveur::sur(&store);

    // SANS chauffe : c'est exactement l'appel que fait la navigation en arrivant.
    let p = srv.space(&store, None, 1);
    let n = noms(&p);
    assert_eq!(n.len(), 200, "la première page doit être pleine, pas vide (offset={})", p["offset"]);
    assert_eq!(p["tronque"].as_bool(), Some(true));
}
