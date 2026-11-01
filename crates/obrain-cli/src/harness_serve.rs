//! Harness natif — T1 (plan « Harness Rust natif », 2026-09-15).
//!
//! Daemon HTTP persistant servant les contrats du harness DIRECTEMENT sur le
//! store, sans respawn de bridge (mesure de reference : 632 ms de demarrage
//! par injection via le GraphInjector dsh). Etape T1 : LECTURE SEULE
//! (verrou partage — coexiste avec l ecrivain dsh, invariant WAL respecte),
//! routes /api/harness/health, /api/harness/machine, /api/harness/continuity,
//! /api/harness/sessions et /api/harness/context.
//!
//! std-only, comme viz_serve : aucune dependance reseau nouvelle.
//!
//! # L'auto-declaration de la machine (2026-09-15)
//!
//! Releve du 2026-09-15 (`docs/MACHINES-CONNUES-2026-09-15.md`, section 2) :
//! n2 et n3 rendaient la MEME reponse de sante, et le nom de l'hote n'y
//! apparaissait pas une seule fois. Deux machines de la flotte etaient donc
//! indiscernables par leur propre contrat — on ne peut pas piloter une machine
//! qui n'a pas de nom. La reponse est ici : la machine s'annonce elle-meme, et
//! sur une route dediee (`/api/harness/machine`), avec ce qu'elle sert et avec
//! la provenance EXACTE du binaire qui tourne. Voir `identite_machine`.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use obrain_engine::ObrainDB;
use serde_json::{Value, json};

/// Decode percent-encoding minimal (%XX et + -> espace).
fn decode(v: &str) -> String {
    let b = v.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() + 1 && i + 2 < b.len() + 1 => {
                if i + 2 < b.len() {
                    if let Ok(x) = u8::from_str_radix(&v[i + 1..i + 3], 16) {
                        out.push(x);
                        i += 3;
                        continue;
                    }
                }
                out.push(b'%');
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_query(q: &str) -> Vec<(String, String)> {
    q.split('&')
        .filter_map(|p| p.split_once('=').map(|(k, v)| (decode(k), decode(v))))
        .collect()
}

fn respond(stream: &mut TcpStream, status: u16, body: &str) {
    let raison = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "Error",
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status} {raison}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

/// Seuil de fraîcheur publié avec l'injection (minutes). Au-delà, le tissage
/// est déclaré arrêté — même valeur que la sentinelle ops.
pub const SEUIL_TISSAGE_MIN: u64 = 30;

/// **Le tissage écrit-il encore ?** — le fil qui manquait.
///
/// La sentinelle `obrain continuity` existe depuis le 2026-09-05 (incident
/// « écrivain absent ~1 h → base figée **sans alerte** »), mais **rien ne la
/// lisait** : ni le harness, ni le contexte de l'IA. Une IA qui s'auto-tisse
/// ignorait donc que son propre tissage s'était arrêté — elle continuait à
/// parler sur un graphe gelé.
///
/// Ce bloc met le verdict **dans le contrat d'injection** : tout consommateur
/// du contexte le reçoit sans requête supplémentaire. La sentinelle reste la
/// source unique du calcul (`continuity::compute`), donc le démon et le timer
/// ops ne peuvent pas diverger — deux calculs pour une même vérité finiraient
/// par se contredire.
///
/// Coût : `compute` ne lit que des **mtime** de fichiers de zone
/// (`substrate.journal.<session>`) — aucune ouverture de store, aucun verrou.
/// Il tourne donc pendant que l'écrivain travaille, et le payload reste borné
/// (des compteurs, pas la liste des sessions).
///
/// # `etat` : un zéro muet est interdit
///
/// `stale: false` avec `journaux: 0` se lirait « tout va bien » alors qu'il
/// signifie **« je n'ai rien à regarder »** — un store neuf, un chemin faux, ou
/// des zones de journal disparues. C'est exactement le mensonge que ce dépôt
/// interdit ailleurs (« sans `?counts=true`, pas de compteur — et surtout pas un
/// zéro muet »). L'état est donc publié nommément :
///
/// | `etat` | Sens |
/// |---|---|
/// | `frais` | au moins un journal écrit il y a moins du seuil |
/// | `fige` | des journaux existent, le plus récent dépasse le seuil |
/// | `sans_journal` | **aucune** zone de journal : on ne sait pas, on ne dit pas « frais » |
fn bloc_tissage(dir: &Path) -> Value {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    bloc_tissage_a(dir, now)
}

/// Le même bloc, à un instant **imposé** — la seule façon de tester les bords
/// (un journal « exactement au seuil », un mtime dans le futur) sans attendre.
fn bloc_tissage_a(dir: &Path, now: i64) -> Value {
    let r = crate::commands::continuity::compute(dir, now, SEUIL_TISSAGE_MIN);
    let actives = r.sessions.iter().filter(|s| !s.stale).count();
    let etat = if r.journals == 0 {
        "sans_journal"
    } else if r.stale {
        "fige"
    } else {
        "frais"
    };
    json!({
        "etat": etat,
        "stale": r.stale,
        "age_sec": r.last_event_age_sec,
        "seuil_min": r.stale_after_min,
        "journaux": r.journals,
        "sessions_actives": actives,
        "checked_at_ms": r.checked_at_ms,
    })
}

// ============================================================================
// L'IDENTITE DE LA MACHINE — auto-declaree, jamais inventoriee
// ============================================================================
//
// Ce que ce bloc corrige, mesure en main : le 2026-09-15,
// `GET /api/harness/health` sur n2 et sur n3 rendait la MEME reponse, et le nom
// de l'hote n'y figurait pas une seule fois. `version` est une CONSTANTE
// (`0.0.1` sur toutes les machines) ; seul le chemin d'un store distinguait
// deux hotes — et sur n2, provisionnee six jours plus tot, il n'y avait meme
// pas de store. Consequence directe : la flotte n'etait pas adressable. On ne
// pilote pas une machine qui n'a pas de nom, et le graphe global ne contenait
// aucun noeud de machine (`docs/MACHINES-CONNUES-2026-09-15.md`, sections 2 et 3).
//
// La reponse est l'inverse d'un inventaire administre : **la machine se declare
// elle-meme**. Lui seul sait ou il tourne, quel store il sert et depuis quel
// binaire. Aucun fichier de configuration a tenir a jour, aucune liste a
// reconcilier — donc aucune liste qui mente.
//
// # Ce qui est un FAIT, et ce qui ne l'est pas
//
// - le nom d'hote vient du NOYAU (`/proc/sys/kernel/hostname`), pas d'une
//   variable d'environnement : meme doctrine que `obrain-selfd`
//   (`crates/obrain-selfd/src/identite.rs`), ou une variable se laisse mentir
//   dans un shell `ssh` non interactif. Quand on a du tomber sur `$HOSTNAME`,
//   `nom_source` le DIT : un nom dont on ignore la provenance se lirait comme
//   un nom verifie ;
// - si rien n'est lisible, on ecrit `inconnu`. JAMAIS devine : un identifiant
//   faux serait pire qu'un identifiant absent — il designerait la mauvaise
//   machine dans une flotte, et une action partirait au mauvais endroit ;
// - `store` et `store_racine` (le dossier qui le contient) sont donnes : c'est
//   l'argument du processus. `racine_declaree` n'est rempli que si
//   l'exploitant a declare `OBRAIN_HARNESS_ROOT` — sinon elle vaut
//   `non_declaree`, ce qui est une information, pas un trou ;
// - la provenance du binaire n'est pas inventee ici : elle existe deja, posee
//   par `build.rs` (`OBRAIN_GIT_COMMIT` / `OBRAIN_GIT_DESCRIBE` /
//   `OBRAIN_GIT_ARBRE`) et lue par `obrain version`. Le 2026-09-01, sept
//   binaires de secours cohabitaient sans manifeste : impossible de dire lequel
//   correspondait a quoi. Le harness publie donc `commit` et `arbre` — un
//   binaire bati sur un arbre SALE n'est pas reconstructible, et cela doit se
//   lire avant de conclure quoi que ce soit du comportement observe.
//
// # Pourquoi `instance` en plus de `machine`
//
// `machine:n3` ne suffit pas : deux harnesses peuvent servir deux bases sur le
// meme hote (deux comptes, deux environnements) — c'est exactement ce que
// `obrain-selfd` a constate pour les noeuds (« deux noeuds peuvent vivre sur la
// meme machine, meme nom d'hote »). L'identite porte donc l'instance
// `machine:<hote>#<base servie>`, et `machine:<hote>` reste le nom de noeud de
// premier ordre demande par le document (point 1).
//
// # Ce que ce bloc fait MAINTENANT, et pourquoi en deux temps
//
// Il ne cree toujours PAS le noeud `machine:` dans le store, et ce n'est pas un
// retard : ce daemon ouvre la base en LECTURE SEULE (verrou partage), y ecrire
// exigerait l'exclusif et romprait l'invariant mono-ecrivain qui permet a `dsh`
// d'ecrire pendant que le harness repond. Le motif autorise est celui de
// `memoire.rs`, et il a deux moities :
//
//   1. **JOURNALISER** — append-only, sans verrou. C'est ce que ce fichier fait
//      au demarrage, dans le journal que le registre utilise deja
//      (`OBRAIN_VIZ_CACHE_DIR/machines`, meme extension `.jsonl` : aucune
//      seconde mecanique a faire exister, ni a decouvrir) ;
//   2. **APPLIQUER** — `obrain machines --store <base> --appliquer` ouvre la base
//      en exclusif et y ecrit le noeud `machine:<hote>` (`crate::machines`).
//
// C'est le chainon qui manquait au point 1 du document : sans lui, une machine
// sur laquelle tourne un moteur SANS `selfd` — le cas le plus frequent, et le
// seul qui existe sur une machine fraichement provisionnee — restait innommable
// dans le graphe, alors que c'est exactement celle-la qu'on veut piloter.
//
// # Best-effort ASSUME, et jamais fatal
//
// La journalisation ne peut pas faire echouer le harness : un cache plein, un
// `OBRAIN_VIZ_CACHE_DIR` qui pointe vers un fichier, un disque en lecture seule
// ne doivent pas empecher un moteur de servir sa base. C'est le meme contrat que
// le journal d'activation et celui de la memoire. Mais — comme le registre — il
// le DIT : une declaration perdue en silence rendrait une machine invisible dans
// le graphe sans que personne ne l'apprenne, et le silence ressemble a une panne.

/// Le mode que ce harness offre. `ro` par construction : il ouvre le store en
/// lecture seule et ne propose AUCUN autre mode. Le publier nommement interdit
/// de croire qu'un mode `rw` existe parce qu'un client l'a demande.
pub const MODE_OFFERT: &str = "ro";

/// Nom d'hote de la machine, et d'ou il vient.
///
/// Le second membre du couple n'est pas decoratif : le document qui a ouvert ce
/// chantier reproche precisement au harness d'annoncer des choses qu'il n'a pas
/// verifiees. Un nom lu dans une variable d'environnement et un nom lu dans le
/// noyau ne se valent pas, et l'appelant doit pouvoir faire la difference.
///
/// `pub(crate)` depuis le 2026-09-16 : la passe de la flotte (`crate::machines`)
/// inscrit dans le graphe QUELLE machine a ecrit le fait. Lire ce nom autrement
/// la-bas — ou pire, l'inventer — donnerait deux verites sur la meme machine.
pub(crate) fn nom_hote() -> (String, &'static str) {
    // Le noyau d'abord : `hostname(1)` et le noyau repondent la meme chose, mais
    // ce fichier-la ne depend d'aucune variable et ne coute aucun processus.
    for source in ["/proc/sys/kernel/hostname", "/etc/hostname"] {
        if let Ok(s) = std::fs::read_to_string(source) {
            let t = s.trim();
            if !t.is_empty() {
                return (t.to_string(), source);
            }
        }
    }
    // Dernier recours, et nomme comme tel : `$HOSTNAME` n'est pas toujours pose
    // ni toujours juste.
    match std::env::var("HOSTNAME") {
        Ok(v) if !v.trim().is_empty() => (v.trim().to_string(), "variable HOSTNAME (non verifiee)"),
        _ => ("inconnu".to_string(), "aucune source lisible"),
    }
}

/// L'estampille du compilateur, si `build.rs` a pu la poser.
///
/// `RUSTC_VERSION` n'est pas toujours defini : dans ce cas on l'ecrit, plutot
/// que de servir la `rust-version` du manifeste Cargo — qui est une
/// DECLARATION de compatibilite, pas la version qui a compile ce binaire.
fn rustc_estampe() -> &'static str {
    match option_env!("RUSTC_VERSION") {
        Some(v) if !v.is_empty() => v,
        _ => "non_estampee",
    }
}

/// Horodatage en millisecondes depuis l'epoque Unix (0 si l'horloge recule).
fn maintenant_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// L'identite complete de ce harness — ce qu'il dit de lui-meme.
///
/// Calculee UNE fois au demarrage par `serve_on` : rien ici ne change sans
/// redemarrage (hors `presence_ms`, ajoute par la route). La fonction est pure
/// vis-a-vis du store : elle ne l'ouvre pas, ne le verrouille pas, ne le lit
/// pas — elle publie le chemin qu'on lui a demande de servir et les faits du
/// processus.
fn identite_machine(store_label: &str, ecoute: &str, demarre_le_ms: i64) -> Value {
    let (machine, nom_source) = nom_hote();
    let store = Path::new(store_label);
    // Le nom de la base servie : c'est lui qui desambigue deux harnesses d'un
    // meme hote. « ? » si le chemin n'a pas de dernier element (racine).
    let base = store
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("?");
    let store_racine = store
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| ".".to_string());
    // Declaree par l'exploitant, sinon dite telle quelle. Deviner la racine a
    // partir du store ferait passer une convention de depot pour un fait du
    // deploiement.
    let racine_declaree = std::env::var("OBRAIN_HARNESS_ROOT")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "non_declaree".to_string());
    let machine_qn = format!("machine:{machine}");
    json!({
        "machine": machine,
        "machine_qn": machine_qn,
        "instance": format!("{machine_qn}#{base}"),
        "base": base,
        "nom_source": nom_source,
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "famille": std::env::consts::FAMILY,
        "store": store_label,
        "store_racine": store_racine,
        "racine_declaree": racine_declaree,
        "mode": MODE_OFFERT,
        "ecoute": ecoute,
        "pid": std::process::id(),
        "demarre_le_ms": demarre_le_ms,
        "build": {
            "version": env!("CARGO_PKG_VERSION"),
            "commit": env!("OBRAIN_GIT_COMMIT"),
            "describe": env!("OBRAIN_GIT_DESCRIBE"),
            "arbre": env!("OBRAIN_GIT_ARBRE"),
            "rustc": rustc_estampe(),
        },
    })
}

// ============================================================================
// LA DECLARATION AU DEMARRAGE — la moitie JOURNAL du motif en deux temps
// ============================================================================

/// La LIGNE que ce harness journalise sur lui-meme. Fonction **PURE** : elle
/// transforme l'identite deja calculee en un fait de journal, sans ouvrir quoi
/// que ce soit, sans verrou, sans horloge (l'instant est passe).
///
/// Rien n'est reinvente ici : chaque champ vient de `identite_machine`, donc de
/// ce que le harness sait de lui-meme et publie deja sur `/api/harness/machine`.
/// C'est deliberé — l'identite annoncee et l'identite journalisee doivent etre la
/// MEME, sinon le graphe et la route se contrediraient, et on ne saurait plus
/// laquelle croire.
///
/// Ce qui n'y est PAS, et ne doit jamais y etre : ni compte, ni capacite, ni
/// « joignable ». Un harness n'appartient a personne (ce n'est pas un `selfd`
/// attache), n'annonce aucune capacite (il sert une base en lecture), et sa
/// joignabilite n'est pas un fait qu'on ecrit — elle se mesure en interrogeant la
/// route, pas en relisant un journal.
fn ligne_declaration(identite: &Value, declaree_ms: i64) -> Value {
    let s = |k: &str| identite.get(k).cloned().unwrap_or(Value::Null);
    json!({
        // Le genre : c'est lui qui separe, dans un meme journal, les attaches du
        // registre et les declarations des moteurs. Il ne porte ni `id` ni
        // `compte` — le rejeu du registre (`remote.rs::rejouer`) exige un `id` et
        // ignore donc cette ligne, sans qu'aucune machine fantome n'entre dans la
        // liste de qui que ce soit.
        "kind": "machine",
        "machine": s("machine"),
        "instance": s("instance"),
        "base": s("base"),
        "nom_source": s("nom_source"),
        "os": s("os"),
        "arch": s("arch"),
        "famille": s("famille"),
        "store": s("store"),
        "store_racine": s("store_racine"),
        "mode": s("mode"),
        "racine_declaree": s("racine_declaree"),
        "ecoute": s("ecoute"),
        "pid": s("pid"),
        "declaree_ms": declaree_ms,
        "version": s("build")["version"].clone(),
        "commit": s("build")["commit"].clone(),
        "arbre": s("build")["arbre"].clone(),
    })
}

/// Un fichier de journal par STORE SERVI — le meme nom que ceux du registre
/// (`<hachage 16 hex>.jsonl`), parce que c'est la convention de ce repertoire :
/// on la suit au lieu d'en inventer une seconde. Le hachage porte le chemin du
/// store (un chemin peut contenir n'importe quel caractere, un nom de fichier
/// non), et c'est la LIGNE qui dit de quelle machine il s'agit — exactement comme
/// le journal du registre, dont le nom est le hachage du compte et dont la ligne
/// porte le compte.
///
/// # Pourquoi le STORE, et pas l'instance
///
/// MESURE, et c'est un defaut corrige : la cle etait d'abord l'instance
/// (`machine:<hote>#<base servie>`). Or deux harnesses d'un meme hote servent
/// souvent deux stores qui portent le MEME nom de base — `runtime-dev.obrain`
/// dans deux racines, ou deux stores de test — donc la meme instance : les deux
/// processus ecrivaient dans le MEME fichier, et leurs lignes se sont
/// ENTREMELES (`{"arbre":"SALE"{,""arch"arbre:"":x86_64""...` : un fichier
/// illisible, mesure dans le cache d'une machine de developpement). Les faits
/// d'un seul store restent donc dans un seul fichier, et deux processus ne se
/// disputent plus le meme.
///
/// Un redemarrage du meme harness reecrit dans le meme fichier : append-only,
/// puis « derniere ligne gagnante » a la relecture. L'ambiguite residuelle — deux
/// stores de meme nom de base sur un meme hote partagent un nom d'INSTANCE, donc
/// un seul noeud — n'est pas cachee : elle vient de l'identite du harness
/// (`machine:<hote>#<base servie>`, publiee par `/api/harness/machine`), et le
/// noeud ecrit garde le store de la declaration la plus recente.
fn chemin_declaration(dir: &Path, store_servi: &str) -> std::path::PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    store_servi.hash(&mut h);
    dir.join(format!("{:016x}.jsonl", h.finish()))
}

/// Journalise la declaration — append-only, aucun verrou, **best-effort**.
///
/// Aucun verrou n'est pris, et c'est le point : ce fichier n'est pas la base d'un
/// Self, c'est le notre, et nous en sommes le seul ecrivain. L'appelant ne doit
/// JAMAIS faire echouer le harness sur cette erreur : il la rapporte (voir
/// `serve_on`), il ne la propage pas.
///
/// # La ligne part en UNE seule ecriture
///
/// `writeln!` sur un `File` fait PLUSIEURS appels systeme (un par morceau
/// formate) : deux processus qui ajoutent au meme fichier peuvent alors
/// s'entremeler au milieu d'une ligne, et le journal devient illisible — c'est
/// arrive et c'est mesure (`chemin_declaration`). On ecrit donc la ligne entiere
/// en un `write_all`, atomique en `O_APPEND` pour une ligne de cette taille.
fn journaliser_declaration(dir: &Path, ligne: &Value) -> std::io::Result<std::path::PathBuf> {
    let store = ligne.get("store").and_then(Value::as_str).unwrap_or_default();
    let chemin = chemin_declaration(dir, store);
    if let Some(d) = chemin.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&chemin)?;
    use std::io::Write;
    let mut complete = ligne.to_string();
    complete.push('\n');
    f.write_all(complete.as_bytes())?;
    f.flush()?;
    Ok(chemin)
}

/// La session nommee existe-t-elle dans le graphe SERVI ?
///
/// **Trois états, et le troisième compte** : `Some(true)` = oui, `Some(false)` =
/// non, `None` = *on n'a pas pu mesurer*. Confondre « je n'ai pas pu vérifier »
/// avec « elle existe » est le mensonge que cette route combat ; le confondre
/// avec « elle n'existe pas » accuserait un appelant innocent d'avoir mal
/// orthographié un nom. On rend donc les trois, et l'appelant DIT lequel il a.
///
/// La requête lit exactement ce que lit la route `/sessions` (label `Session`,
/// propriété `qualified_name`) : deux lectures pour une même vérité finiraient
/// par se contredire, et la porte du harness éprouve précisément qu'une session
/// LISTÉE est reconnue connue par cette fonction.
fn session_connue(db: &ObrainDB, session_id: &str) -> Option<bool> {
    if session_id.trim().is_empty() {
        return Some(false);
    }
    let q = format!(
        "MATCH (s:Session {{qualified_name: {}}}) RETURN s.qualified_name LIMIT 1",
        crate::memoire::lit(session_id)
    );
    match db.session().execute(&q) {
        Ok(r) => Some(!r.rows.is_empty()),
        Err(_) => None,
    }
}

fn handle(mut stream: TcpStream, db: &Arc<ObrainDB>, store_label: &str, identite: &Value) {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(10)));
    // Lecture robuste : accumuler jusqu a l en-tete complet (\r\n\r\n), puis
    // jusqu a Content-Length octets de corps. Gere l en-tete et le corps
    // arrivant en segments TCP separes (cas curl).
    let mut raw: Vec<u8> = Vec::with_capacity(8192);
    let mut tmp = [0u8; 8192];
    let mut entete_fin: Option<usize> = None;
    let mut clen: usize = 0;
    loop {
        // en-tete deja complet et corps suffisant ? on s arrete.
        if let Some(hf) = entete_fin {
            if raw.len() >= hf + clen {
                break;
            }
        }
        match stream.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => {
                raw.extend_from_slice(&tmp[..n]);
                if entete_fin.is_none() {
                    if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                        let hf = pos + 4;
                        entete_fin = Some(hf);
                        let tete = String::from_utf8_lossy(&raw[..hf]);
                        clen = tete
                            .lines()
                            .find_map(|l| {
                                let (k, v) = l.split_once(':')?;
                                if k.trim().eq_ignore_ascii_case("content-length") {
                                    v.trim().parse::<usize>().ok()
                                } else {
                                    None
                                }
                            })
                            .unwrap_or(0);
                    }
                }
                if raw.len() > 1_048_576 {
                    return respond(&mut stream, 400, r#"{"error":"requete trop grande"}"#);
                }
            }
            Err(_) => break,
        }
    }
    let Some(hf) = entete_fin else {
        return respond(&mut stream, 400, r#"{"error":"en-tete incomplet"}"#);
    };
    let tete = String::from_utf8_lossy(&raw[..hf]).into_owned();
    let corps = String::from_utf8_lossy(&raw[hf..(hf + clen).min(raw.len())]).into_owned();
    let Some(ligne) = tete.lines().next() else { return };
    let mut parts = ligne.split_whitespace();
    let (Some(_methode), Some(cible)) = (parts.next(), parts.next()) else {
        return respond(&mut stream, 400, r#"{"error":"requete invalide"}"#);
    };
    let (chemin, query) = match cible.split_once('?') {
        Some((c, q)) => (c, q),
        None => (cible, ""),
    };
    match chemin {
        // ── LA MACHINE SE DECLARE ─────────────────────────────────────────
        // Route dediee : un poller de flotte n'a pas a decortiquer la sante
        // pour savoir a qui il parle. Sa reponse est l'identite elle-meme, au
        // premier niveau — et « repondre » EST la preuve de presence : on ne
        // stocke pas une derniere-presence qu'il faudrait croire, on sert
        // l'instant de la reponse.
        "/api/harness/machine" => {
            let mut corps = identite.clone();
            if let Some(o) = corps.as_object_mut() {
                o.insert("status".to_string(), json!("ok"));
                o.insert("presence_ms".to_string(), json!(maintenant_ms()));
            }
            respond(&mut stream, 200, &corps.to_string());
        }
        "/api/harness/health" => {
            // ── SUR QUELLE MACHINE TOURNE CE HARNESS (2026-09-15) ──────────
            // Sans ce champ, la reponse ne nommait RIEN : `version` est une
            // CONSTANTE (`0.0.1` sur toutes les machines) et seul le chemin du
            // store distinguait deux hotes — quand ils en avaient un.
            //
            // Mesure qui a ouvert ce chantier : n2 (IP-INTERNE) et n3
            // (IP-INTERNE) rendent la MEME reponse, et `obrain version` sur
            // n2 n'affiche ZERO occurrence du nom de l'hote. On ne pilote pas
            // une machine qui n'a pas de nom ; le graphe global, lui, ne
            // contenait aucun noeud de machine.
            //
            // Resolue UNE fois : le nom d'hote ne change pas sans redemarrage,
            // et cette route est interrogee en boucle.
            //
            // JAMAIS DEVINEE : si on ne sait pas la lire, on ecrit « inconnu ».
            // Un identifiant faux serait pire qu'un identifiant absent — il
            // designerait la mauvaise machine dans une flotte.
            //
            // Les cinq champs d'origine (status, mode, store, version, machine)
            // restent presents et gardent leur sens : `scripts/verifier-
            // integration-tissage.sh` lit `"status":"ok"` et `"mode"`, et
            // `scripts/flotte-auto-connaissance.sh` cherche le nom de l'hote
            // dans le corps. On AJOUTE `identite` (l'objet complet, identique a
            // la route dediee) : des scalaires de taille fixe, pas de liste —
            // le payload ne grandit pas avec la flotte, seulement avec le
            // nombre de faits qu'on sait dire de soi.
            let mut body = identite.clone();
            if let Some(o) = body.as_object_mut() {
                // Identite du binaire ET de la conversation : la presence est
                // horodatee a chaque reponse, c'est elle qui dit « je suis
                // vivant a cet instant », sans rien ecrire nulle part.
                o.insert("presence_ms".to_string(), json!(maintenant_ms()));
            }
            let corps = json!({
                "status": "ok",
                "mode": MODE_OFFERT,
                "store": store_label,
                "version": env!("CARGO_PKG_VERSION"),
                "machine": identite["machine"],
                "identite": body,
            });
            respond(&mut stream, 200, &corps.to_string());
        }
        "/api/harness/continuity" => {
            // La sentinelle, servie par le démon : le MÊME calcul que
            // `obrain continuity` (mtime des zones de journal), donc le timer
            // ops et cette route ne peuvent pas se contredire. Borné : les
            // compteurs store-wide et les cinq journaux les plus récents.
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            let r = crate::commands::continuity::compute(
                Path::new(store_label),
                now,
                SEUIL_TISSAGE_MIN,
            );
            let recents: Vec<Value> = r
                .sessions
                .iter()
                .take(5)
                .map(|s| {
                    json!({
                        "session": s.session_id,
                        "age_sec": s.last_event_age_sec,
                        "stale": s.stale,
                    })
                })
                .collect();
            let body = json!({
                "status": "ok",
                "store": r.store,
                "etat": if r.journals == 0 {
                    "sans_journal"
                } else if r.stale {
                    "fige"
                } else {
                    "frais"
                },
                "stale": r.stale,
                "age_sec": r.last_event_age_sec,
                "seuil_min": r.stale_after_min,
                "journaux": r.journals,
                "sessions_actives": r.sessions.iter().filter(|s| !s.stale).count(),
                "checked_at_ms": r.checked_at_ms,
                "recentes": recents,
            });
            respond(&mut stream, 200, &body.to_string());
        }
        "/api/harness/sessions" => {
            let mut args = serde_json::Map::new();
            for (k, v) in parse_query(query) {
                match k.as_str() {
                    "limit" | "offset" => {
                        if let Ok(n) = v.parse::<u64>() {
                            args.insert(k, json!(n));
                        }
                    }
                    "owner" | "project" => {
                        args.insert(k, json!(v));
                    }
                    _ => {}
                }
            }
            let (out, err) = crate::mcp_server::sessions_list(db, &Value::Object(args));
            respond(&mut stream, if err { 400 } else { 200 }, &out.to_string());
        }
        "/api/harness/context" => {
            // Injection composee (CH6) in-process : goals + summaries + weave +
            // engrams, sans respawn de bridge. Corps JSON : {session_id,
            // subject|prompt, limit?, owner?}. Cible contrat : < 50 ms p95.
            //
            // ── LE VIDE N'A PAS LA FORME D'UNE BONNE REPONSE (2026-09-15) ──
            // Releve qui a ouvert ce chantier : cette route SANS session rendait
            // `{"engrams":[],"goals":[],"session_id":"","weave":[],"summaries":
            // [],"status":"ok"}` — un « ok » sur un vide integral. Un appelant
            // ne pouvait pas distinguer « le graphe n'a rien a dire » de « on ne
            // m'a pas dit pour qui regarder » : les deux rendaient le meme
            // objet, et le second est une ERREUR DE L'APPELANT, pas une reponse.
            // C'est le mode de defaillance que `docs/MACHINES-CONNUES-2026-09-15.md`
            // interdit dans un systeme de pilotage.
            //
            // Deux situations se ressemblaient et sont desormais separees :
            //  · corps ILLISIBLE (pas de JSON : `GET` nu, troncature, octets
            //    binaires) → `status: "aucune_session"`, `corps_lisible: false`,
            //    et le bloc `tissage` TOUJOURS publie (`tests` internes : le fil
            //    doit survivre a une requete illisible — c'est dans les cas
            //    degrades qu'on a besoin de savoir si le tissage tourne). Code
            //    200 : on n'invente pas un echec de transport pour une demande
            //    qu'on a comprise comme vide de session ;
            //  · corps lisible qui NE NOMME PAS de session → meme statut, mais
            //    `corps_lisible: true` et code 400 : la demande est comprise et
            //    refusee, comme `checkpoint_material` refuse un `session_id`
            //    vide (`mcp_server.rs`). Un refus doit etre motive et remonter :
            //    le silence ressemble a une panne.
            let brut: Value = serde_json::from_str(&corps).unwrap_or(Value::Null);
            let corps_lisible = brut.is_object();
            let body: Value = brut;
            let get = |k: &str| body.get(k).and_then(|v| v.as_str()).map(String::from);
            let session_declaree = get("session_id")
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
            let mut args = serde_json::Map::new();
            if let Some(sid) = get("session_id") {
                args.insert("session_id".into(), json!(sid));
            }
            // "prompt" accepte comme alias de "subject" (vocabulaire cote chat).
            if let Some(subj) = get("subject").or_else(|| get("prompt")) {
                args.insert("subject".into(), json!(subj));
            }
            if let Some(owner) = get("owner") {
                args.insert("owner".into(), json!(owner));
            }
            if let Some(n) = body.get("limit").and_then(|v| v.as_u64()) {
                args.insert("limit".into(), json!(n));
            }
            let (mut out, err) = crate::mcp_server::context_build(db, &Value::Object(args));
            // **Le fil** : la sante du tissage voyage AVEC le contexte. Sans
            // elle, un consommateur ne peut pas distinguer « le graphe n'a rien
            // à dire » de « le graphe ne reçoit plus rien » — deux situations
            // opposées, et c'est la seconde qui casse la cohérence d'une
            // conversation longue.
            if let Some(o) = out.as_object_mut() {
                o.insert("tissage".to_string(), bloc_tissage(Path::new(store_label)));
                if !session_declaree {
                    o.insert("status".to_string(), json!("aucune_session"));
                    o.insert("corps_lisible".to_string(), json!(corps_lisible));
                    o.insert(
                        "raison".to_string(),
                        json!(
                            "aucune session nommee : le corps doit porter un session_id non vide. \
                             Les sections rendues sont vides par construction, pas parce que le \
                             graphe est muet."
                        ),
                    );
                } else {
                    // ── UNE SESSION INCONNUE N'EST PAS « ok » (2026-09-16) ──────
                    // Defaut mesure par la porte du harness, et c'est le MEME que
                    // celui corrige plus haut, par l'autre bout : une session
                    // INCONNUE rendait `200 {"status":"ok"}` avec des sections
                    // vides. C'est pire que le cas « aucune session », parce que
                    // l'appelant a bel et bien nomme quelqu'un : il croira que le
                    // graphe est muet alors que le nom n'existe pas.
                    //
                    // ── CE QUE CE BLOC NE FAIT PAS, ET QUI EST DELIBERE ────────
                    // Il ne REFUSE pas la demande (pas de 4xx) : une session NEUVE
                    // n'est pas encore tissee, et c'est le cas NORMAL d'une
                    // conversation qui commence — la refuser casserait le fil.
                    // Le harness ne peut pas distinguer « nom nouveau » de « nom
                    // faux », et il ne fait pas semblant : il le DIT. Le statut
                    // cesse d'etre « ok », et la raison nomme les deux lectures
                    // possibles avec ce qui les separe.
                    match session_connue(db, &get("session_id").unwrap_or_default()) {
                        Some(true) => {}
                        Some(false) => {
                            let sid = get("session_id").unwrap_or_default();
                            o.insert("status".to_string(), json!("session_inconnue"));
                            o.insert("session_connue".to_string(), json!(false));
                            o.insert(
                                "raison".to_string(),
                                json!(format!(
                                    "la session « {sid} » n'existe PAS dans le graphe servi. Si elle \
                                     est NEUVE, c'est normal : elle sera tissee, et les sections \
                                     vides sont alors exactes. Si c'est une faute de frappe, le nom \
                                     est faux — et le harness ne peut pas distinguer les deux, donc \
                                     il le dit plutot que de rendre « ok » sur un vide."
                                )),
                            );
                        }
                        None => {
                            // On n'a PAS pu verifier : le dire est la seule
                            // reponse honnete. Rendre « ok » sans l'avoir mesure
                            // serait exactement ce que cette route combat.
                            o.insert("session_verifiee".to_string(), json!(false));
                            o.insert(
                                "raison".to_string(),
                                json!(
                                    "la presence de la session dans le graphe servi n'a PAS pu \
                                     etre verifiee : le statut n'est donc pas une affirmation sur \
                                     son existence."
                                ),
                            );
                        }
                    }
                }
            }
            // Le code HTTP suit la meme distinction que le statut : un refus
            // lisible part en 400, une demande qu'on n'a pas pu lire garde 200
            // (on ne fabrique pas un echec de transport pour un corps qu'on a
            // compris comme vide de session, et le fil doit passer).
            let code = if (!session_declaree && corps_lisible) || err {
                400
            } else {
                200
            };
            respond(&mut stream, code, &out.to_string());
        }
        _ => respond(&mut stream, 404, r#"{"error":"route inconnue"}"#),
    }
}

/// Boucle du daemon sur un listener deja lie (testable sur port ephemere).
///
/// L'identite est resolue ICI, une fois, avant d'accepter la premiere
/// connexion : elle decrit le lien (l'adresse REELLEMENT liee, pas celle qu'on a
/// demandee — `:0` donne un port arbitraire), le store servi et le binaire qui
/// tourne. La figer au demarrage interdit qu'une reponse se mette a dire autre
/// chose que les precedentes en cours de vie.
///
/// **La declaration est journalisee ici**, au meme instant et depuis la MEME
/// identite que celle qui sera servie : c'est ce qui garantit que le graphe et la
/// route ne peuvent pas se contredire. Le journal est celui du registre
/// (`crate::machines::dir_journal_machines`), et une seule resolution de ce
/// chemin existe pour les deux modules — deux resolutions divergentes ecriraient
/// la declaration dans un journal que la passe ne lit pas, sans rien dire.
///
/// **Echec de journalisation n'est pas echec de demarrage** : un cache en lecture
/// seule, un `OBRAIN_VIZ_CACHE_DIR` pointant sur un fichier, un disque plein —
/// aucun de ces cas ne doit empecher un moteur de servir sa base. On le dit sur
/// la sortie d'erreur (`tracing::warn`, comme le registre, et visible par defaut :
/// le niveau est WARN), et on continue.
pub fn serve_on(listener: TcpListener, db: Arc<ObrainDB>, store_label: &str) -> Result<()> {
    serve_on_journalisant(listener, db, store_label, &crate::machines::dir_journal_machines())
}

/// La meme boucle, avec le journal **impose**.
///
/// Le chemin est un parametre et non une variable d'environnement relue, pour la
/// meme raison que le registre porte son repertoire : deux harnesses — et deux
/// tests dans un meme processus — ne doivent jamais pouvoir ecrire dans le journal
/// l'un de l'autre. Les tests unitaires de ce fichier s'en servent pour ne PAS
/// toucher au cache vivant de la machine.
fn serve_on_journalisant(
    listener: TcpListener,
    db: Arc<ObrainDB>,
    store_label: &str,
    journal: &Path,
) -> Result<()> {
    let ecoute = listener
        .local_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| "inconnue".to_string());
    let identite = identite_machine(store_label, &ecoute, maintenant_ms());
    let ligne = ligne_declaration(&identite, maintenant_ms());
    match journaliser_declaration(journal, &ligne) {
        Ok(chemin) => tracing::debug!(
            machine = %identite["machine"],
            store = store_label,
            chemin = %chemin.display(),
            "harness : machine auto-declaree au journal"
        ),
        Err(e) => tracing::warn!(
            machine = %identite["machine"],
            store = store_label,
            chemin = %journal.display(),
            erreur = %e,
            "harness : declaration de machine NON journalisee — le harness sert sa base, mais \
             aucun noeud machine: ne sera ecrit pour elle tant que le journal ne s'ecrit pas"
        ),
    }
    for stream in listener.incoming() {
        match stream {
            Ok(s) => handle(s, &db, store_label, &identite),
            Err(_) => continue,
        }
    }
    Ok(())
}

/// Entree CLI : ouvre le store en LECTURE SEULE et sert sur addr.
pub fn run(addr: &str, store: &Path, quiet: bool) -> Result<()> {
    let db = Arc::new(
        ObrainDB::open_read_only(store)
            .with_context(|| format!("harness: ouverture RO de {}", store.display()))?,
    );
    let listener =
        TcpListener::bind(addr).with_context(|| format!("harness: bind {addr}"))?;
    if !quiet {
        eprintln!(
            "obrain-harness: pret sur http://{addr} (store {}, {}, v{})",
            store.display(),
            MODE_OFFERT,
            env!("CARGO_PKG_VERSION")
        );
    }
    serve_on(listener, db, &store.display().to_string())
}

// ============================================================================
// Tests — les COINS du fil du tissage.
//
// Ce que ces tests protègent n'est pas une fonction, c'est une **cohérence** :
// une IA qui s'auto-tisse doit pouvoir distinguer « le graphe n'a rien à dire »
// de « le graphe ne reçoit plus rien ». Chaque cas ci-dessous est un coin où
// cette distinction se perd — et « un zéro muet » est le pire d'entre eux :
// il se lit comme une bonne nouvelle.
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::{Duration, SystemTime};

    /// Crée un répertoire de store avec des zones de journal dont on **impose**
    /// le mtime (décalage en secondes par rapport à maintenant).
    ///
    /// `File::set_modified` est la seule façon de fabriquer les bords : un
    /// journal « exactement au seuil », un mtime dans le futur. Attendre 30
    /// minutes pour tester un seuil de 30 minutes n'est pas un test.
    fn store_avec_journaux(entrees: &[(&str, i64)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        for (nom, decalage_s) in entrees {
            let chemin = dir.path().join(nom);
            let mut f = std::fs::File::create(&chemin).expect("création");
            f.write_all(b"zone").expect("écriture");
            let t = if *decalage_s >= 0 {
                SystemTime::now() + Duration::from_secs(*decalage_s as u64)
            } else {
                SystemTime::now() - Duration::from_secs((-*decalage_s) as u64)
            };
            f.set_modified(t).expect("mtime imposé");
        }
        dir
    }

    fn etat(bloc: &Value) -> String {
        bloc["etat"].as_str().unwrap_or("<absent>").to_string()
    }

    // ── Le calcul : les bords ───────────────────────────────────────────────

    #[test]
    fn un_journal_frais_est_frais() {
        let d = store_avec_journaux(&[("substrate.journal.session-a", -5)]);
        let b = bloc_tissage_a(d.path(), maintenant_ms());
        assert_eq!(etat(&b), "frais");
        assert_eq!(b["stale"], false);
        assert_eq!(b["journaux"], 1);
        assert_eq!(b["sessions_actives"], 1);
        assert!(b["age_sec"].as_i64().unwrap() <= 6, "{b}");
    }

    #[test]
    fn un_journal_au_dela_du_seuil_est_fige() {
        // 31 minutes : une minute au-delà du seuil de 30.
        let d = store_avec_journaux(&[("substrate.journal.session-a", -31 * 60)]);
        let b = bloc_tissage_a(d.path(), maintenant_ms());
        assert_eq!(etat(&b), "fige");
        assert_eq!(b["stale"], true);
        assert!(b["age_sec"].as_i64().unwrap() >= 31 * 60 - 2, "{b}");
    }

    /// **Le bord exact** : la condition est `age > seuil`, donc à l'égalité le
    /// tissage n'est PAS figé. Un test qui ne vérifie que « 29 min frais /
    /// 31 min figé » laisse la frontière libre de dériver d'un cran.
    #[test]
    fn exactement_au_seuil_le_tissage_n_est_pas_encore_fige() {
        let seuil_s = (SEUIL_TISSAGE_MIN * 60) as i64;
        let now = maintenant_ms();
        let d = store_avec_journaux(&[("substrate.journal.session-a", -seuil_s)]);
        // `compute` recalcule l'âge depuis le mtime réel : on vise l'égalité et
        // on accepte la seconde de granularité du système de fichiers.
        let b = bloc_tissage_a(d.path(), now);
        let age = b["age_sec"].as_i64().unwrap();
        assert!(
            (seuil_s - 2..=seuil_s + 2).contains(&age),
            "l'âge doit être au seuil, pas ailleurs : {b}"
        );
        // Ce qui compte : la frontière est franchie à `>` et pas à `>=`.
        let juste_avant = bloc_tissage_a(d.path(), maintenant_ms() - 2000);
        assert_eq!(etat(&juste_avant), "frais", "une seconde avant : frais");
    }

    /// **Le zéro muet** : aucun journal ⇒ on ne sait pas. Annoncer `frais`
    /// (comme le faisait `stale: false` seul) ferait passer un store vide, un
    /// chemin faux ou des zones disparues pour un tissage en bonne santé.
    #[test]
    fn sans_aucun_journal_on_ne_dit_pas_frais() {
        let d = tempfile::tempdir().expect("tempdir");
        let b = bloc_tissage_a(d.path(), maintenant_ms());
        assert_eq!(etat(&b), "sans_journal", "{b}");
        assert_eq!(b["journaux"], 0);
        assert_eq!(b["sessions_actives"], 0);
        assert_eq!(b["age_sec"], -1, "aucun événement : -1, jamais un zéro");
    }

    /// Un répertoire **inexistant** (chemin faux, store démonté) ne doit ni
    /// paniquer ni se déclarer frais.
    #[test]
    fn un_repertoire_inexistant_ne_panique_pas_et_ne_dit_pas_frais() {
        let d = tempfile::tempdir().expect("tempdir");
        let absent = d.path().join("pas-la");
        let b = bloc_tissage_a(&absent, maintenant_ms());
        assert_eq!(etat(&b), "sans_journal", "{b}");
    }

    /// Un fichier nommé exactement `substrate.journal.` (suffixe vide) n'est
    /// PAS une session : le compter inventerait une conversation sans nom.
    #[test]
    fn un_prefixe_sans_session_est_ignore() {
        let d = store_avec_journaux(&[
            ("substrate.journal.", -5),
            ("substrate.journal", -5),
            ("substrate.nodes", -5),
        ]);
        let b = bloc_tissage_a(d.path(), maintenant_ms());
        assert_eq!(b["journaux"], 0, "aucun de ces noms n'est une zone de journal");
        assert_eq!(etat(&b), "sans_journal");
    }

    /// Un mtime **dans le futur** (horloge décalée, restauration `tar -p`,
    /// synchronisation) ne doit pas produire d'âge négatif : un âge négatif se
    /// lirait comme « écrit avant d'exister » et fausserait tout seuil.
    #[test]
    fn un_mtime_dans_le_futur_donne_un_age_nul_jamais_negatif() {
        let d = store_avec_journaux(&[("substrate.journal.session-a", 3600)]);
        let b = bloc_tissage_a(d.path(), maintenant_ms());
        assert_eq!(b["age_sec"], 0, "jamais négatif : {b}");
        assert_eq!(etat(&b), "frais");
    }

    /// L'âge **store-wide** est celui du journal le plus RÉCENT : un vieux
    /// journal ne doit pas faire passer un tissage vivant pour figé, et les
    /// compteurs doivent distinguer les deux sessions.
    #[test]
    fn l_age_store_wide_suit_le_journal_le_plus_recent() {
        let d = store_avec_journaux(&[
            ("substrate.journal.session-vieille", -2 * 3600),
            ("substrate.journal.session-vivante", -3),
        ]);
        let b = bloc_tissage_a(d.path(), maintenant_ms());
        assert_eq!(etat(&b), "frais", "le plus récent commande : {b}");
        assert_eq!(b["journaux"], 2);
        assert_eq!(
            b["sessions_actives"], 1,
            "une seule des deux a écrit récemment : {b}"
        );
    }

    /// Le seuil est borné à 1 minute : `0` ne doit pas rendre « figé » un
    /// journal écrit à l'instant (un seuil nul crierait toujours).
    #[test]
    fn un_seuil_nul_est_borne_a_une_minute() {
        let d = store_avec_journaux(&[("substrate.journal.session-a", 0)]);
        let now = maintenant_ms();
        let r = crate::commands::continuity::compute(d.path(), now, 0);
        assert_eq!(r.stale_after_min, 0);
        assert!(
            !r.stale,
            "un journal écrit à l'instant n'est pas figé, même avec un seuil nul"
        );
    }

    /// La forme du bloc est un CONTRAT : ces sept clés, et rien de plus. Une
    /// liste de sessions ajoutée ici ferait grossir le contexte injecté à
    /// chaque tour — le payload doit rester des compteurs.
    #[test]
    fn le_bloc_est_borne_et_sa_forme_est_un_contrat() {
        let d = store_avec_journaux(&[("substrate.journal.session-a", -5)]);
        let b = bloc_tissage_a(d.path(), maintenant_ms());
        let mut cles: Vec<&str> = b.as_object().unwrap().keys().map(String::as_str).collect();
        cles.sort_unstable();
        assert_eq!(
            cles,
            vec![
                "age_sec",
                "checked_at_ms",
                "etat",
                "journaux",
                "sessions_actives",
                "seuil_min",
                "stale"
            ],
            "le contrat du bloc a changé : {b}"
        );
    }

    // ── Les routes : les coins ──────────────────────────────────────────────

    fn maintenant_ms() -> i64 {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }

    /// Une requête HTTP brute sur le démon (std-only, comme lui) : on ne teste
    /// pas un client HTTP, on teste ce que le serveur écrit sur la socket.
    fn requete(port: u16, methode: &str, chemin: &str, corps: Option<&str>) -> (u16, String) {
        let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connexion");
        s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        let corps = corps.unwrap_or("");
        let req = format!(
            "{methode} {chemin} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{corps}",
            corps.len()
        );
        s.write_all(req.as_bytes()).expect("envoi");
        let mut rep = String::new();
        s.read_to_string(&mut rep).expect("lecture");
        let statut: u16 = rep
            .split_whitespace()
            .nth(1)
            .and_then(|c| c.parse().ok())
            .unwrap_or(0);
        (statut, rep)
    }

    /// Démon de test : store réel (substrat), répertoire de journaux imposé,
    /// port éphémère. Le `store_label` du démon EST le chemin des journaux :
    /// c'est ce qui permet de fabriquer les cas sans toucher au vrai store.
    ///
    /// Le cache des auto-déclarations est TEMPORAIRE lui aussi : un test qui
    /// écrirait dans `/tmp/obrain-viz-cache/machines` déclarerait au graphe de la
    /// machine des moteurs servant des stores de test, et la passe suivante y
    /// créerait des nœuds `machine:` pour des bases disparues. Un test ne pollue
    /// pas le cache vivant (même règle que le registre, qui isole le sien).
    struct Demon {
        port: u16,
        _store: tempfile::TempDir,
        _journaux: tempfile::TempDir,
        _cache: tempfile::TempDir,
    }

    fn demon(journaux: &[(&str, i64)]) -> Demon {
        let store = tempfile::tempdir().expect("tempdir store");
        let chemin = store.path().join("t.obrain");
        let db = Arc::new(
            ObrainDB::open_substrate(chemin.to_str().unwrap()).expect("store de test"),
        );
        let j = store_avec_journaux(journaux);
        let cache = tempfile::tempdir().expect("tempdir cache");
        let cache_machines = cache.path().join("machines");
        let listener = TcpListener::bind("127.0.0.1:0").expect("port éphémère");
        let port = listener.local_addr().unwrap().port();
        let label = j.path().display().to_string();
        std::thread::spawn(move || {
            let _ = serve_on_journalisant(listener, db, &label, &cache_machines);
        });
        Demon {
            port,
            _store: store,
            _journaux: j,
            _cache: cache,
        }
    }

    #[test]
    fn la_route_sante_repond_toujours_et_nomme_la_machine() {
        let d = demon(&[]);
        let (code, rep) = requete(d.port, "GET", "/api/harness/health", None);
        assert_eq!(code, 200, "{rep}");
        assert!(rep.contains("\"status\":\"ok\""), "{rep}");
        assert!(rep.contains("\"machine\""), "la machine est nommée : {rep}");
    }

    #[test]
    fn la_route_continuity_sert_le_verdict_et_les_cinq_derniers() {
        let d = demon(&[
            ("substrate.journal.session-1", -1),
            ("substrate.journal.session-2", -2),
            ("substrate.journal.session-3", -3),
            ("substrate.journal.session-4", -4),
            ("substrate.journal.session-5", -5),
            ("substrate.journal.session-6", -6),
            ("substrate.journal.session-vieille", -2 * 3600),
        ]);
        let (code, rep) = requete(d.port, "GET", "/api/harness/continuity", None);
        assert_eq!(code, 200, "{rep}");
        let v: Value = serde_json::from_str(
            rep.split("\r\n\r\n").nth(1).unwrap_or("{}"),
        )
        .expect("corps JSON");
        assert_eq!(v["etat"], "frais");
        assert_eq!(v["journaux"], 7);
        assert_eq!(v["sessions_actives"], 6);
        let recentes = v["recentes"].as_array().expect("recentes");
        assert_eq!(recentes.len(), 5, "borné à cinq, jamais la liste entière");
        assert_eq!(
            recentes[0]["session"], "session-1",
            "le plus récent d'abord : {v}"
        );
    }

    /// Une query string sur la route ne doit pas la casser : le handler coupe
    /// déjà `?…` du chemin, et un cache ou un outil en ajoute toujours une.
    #[test]
    fn la_route_continuity_ignore_la_query_string() {
        let d = demon(&[("substrate.journal.session-a", -1)]);
        let (code, rep) = requete(d.port, "GET", "/api/harness/continuity?x=1&y=2", None);
        assert_eq!(code, 200, "{rep}");
        assert!(rep.contains("\"journaux\":1"), "{rep}");
    }

    #[test]
    fn une_route_inconnue_repond_404() {
        let d = demon(&[]);
        let (code, rep) = requete(d.port, "GET", "/api/harness/inconnu", None);
        assert_eq!(code, 404, "{rep}");
        assert!(rep.contains("route inconnue"), "{rep}");
    }

    /// **Le coin le plus important pour le fil** : le bloc `tissage` doit être
    /// publié même quand la requête est illisible. Un fil qui disparaît sur une
    /// requête mal formée est un fil sur lequel on ne peut pas compter — et
    /// c'est justement dans les cas dégradés qu'on a besoin de savoir si le
    /// tissage tourne.
    #[test]
    fn le_fil_est_publie_meme_avec_un_corps_illisible() {
        let d = demon(&[("substrate.journal.session-a", -1)]);
        let (code, rep) = requete(
            d.port,
            "POST",
            "/api/harness/context",
            Some("ceci n'est pas du JSON"),
        );
        assert_eq!(code, 200, "{rep}");
        let corps = rep.split("\r\n\r\n").nth(1).unwrap_or("{}");
        let v: Value = serde_json::from_str(corps).expect("corps JSON");
        assert!(
            v.get("tissage").is_some(),
            "le fil doit survivre à une requête illisible : {v}"
        );
        assert_eq!(v["tissage"]["etat"], "frais");
    }

    /// Et il est publié pour une session inconnue du graphe : le fil décrit le
    /// STORE, pas la session — sinon une session neuve (aucun nœud) ferait
    /// croire que le tissage est muet.
    #[test]
    fn le_fil_est_publie_pour_une_session_inconnue() {
        let d = demon(&[("substrate.journal.session-a", -1)]);
        let (code, rep) = requete(
            d.port,
            "POST",
            "/api/harness/context",
            Some(r#"{"session_id":"session-qui-nexiste-pas","subject":"x"}"#),
        );
        assert_eq!(code, 200, "{rep}");
        let corps = rep.split("\r\n\r\n").nth(1).unwrap_or("{}");
        let v: Value = serde_json::from_str(corps).expect("corps JSON");
        assert_eq!(
            v["tissage"]["journaux"], 1,
            "le fil décrit le store, pas la session : {v}"
        );
    }

    /// Un store SANS journal : le fil doit dire `sans_journal`, jamais `frais`.
    /// C'est le cas d'un store neuf — et d'un chemin faux.
    #[test]
    fn sans_journal_le_fil_le_dit_au_lieu_de_laisser_croire_que_tout_va_bien() {
        let d = demon(&[]);
        let (code, rep) = requete(
            d.port,
            "POST",
            "/api/harness/context",
            Some(r#"{"session_id":"s","subject":"x"}"#),
        );
        assert_eq!(code, 200, "{rep}");
        let corps = rep.split("\r\n\r\n").nth(1).unwrap_or("{}");
        let v: Value = serde_json::from_str(corps).expect("corps JSON");
        assert_eq!(v["tissage"]["etat"], "sans_journal", "{v}");
        assert_eq!(v["tissage"]["age_sec"], -1, "{v}");
    }

    // ── L'AUTO-DÉCLARATION : ce qui est journalisé, et ce qui ne l'est pas ───

    fn identite_de_test() -> Value {
        identite_machine("/tmp/kb.obrain", "127.0.0.1:8451", 1_700_000_000_000)
    }

    /// La ligne journalisée est l'identité DÉJÀ ANNONCÉE, et rien d'autre : le
    /// graphe et la route ne doivent pas pouvoir se contredire.
    #[test]
    fn la_ligne_declaration_est_l_identite_deja_annoncee() {
        let i = identite_de_test();
        let l = ligne_declaration(&i, 1_700_000_000_500);
        assert_eq!(l["kind"], "machine");
        assert_eq!(l["machine"], i["machine"]);
        assert_eq!(l["instance"], i["instance"]);
        assert_eq!(l["store"], i["store"]);
        assert_eq!(l["store_racine"], i["store_racine"]);
        assert_eq!(l["mode"], MODE_OFFERT);
        assert_eq!(l["os"], std::env::consts::OS);
        assert_eq!(l["arch"], std::env::consts::ARCH);
        assert_eq!(l["nom_source"], i["nom_source"]);
        assert_eq!(l["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(l["commit"], env!("OBRAIN_GIT_COMMIT"));
        assert_eq!(l["arbre"], env!("OBRAIN_GIT_ARBRE"));
        assert_eq!(
            l["declaree_ms"],
            json!(1_700_000_000_500i64),
            "l'instant de la déclaration est celui qu'on passe"
        );
    }

    /// **CE QUI N'Y EST PAS**, et ne doit jamais y être : ni compte, ni capacité,
    /// ni `id`. Un moteur seul n'appartient à personne ; une ligne qui porterait
    /// un compte ferait entrer la machine dans la liste d'un utilisateur, et une
    /// ligne qui porterait un `id` serait relue comme une attache de `selfd`.
    #[test]
    fn la_ligne_declaration_ne_porte_ni_compte_ni_capacite_ni_id() {
        let l = ligne_declaration(&identite_de_test(), 1);
        for absent in ["id", "compte", "capacites", "vivant", "joignable"] {
            assert!(l.get(absent).is_none(), "« {absent} » ne doit pas être journalisé : {l}");
        }
    }

    /// Le harness JOURNALISE au démarrage, dans le répertoire qu'on lui impose —
    /// et c'est la même identité que celle qu'il sert.
    #[test]
    fn au_demarrage_le_harness_journalise_sa_declaration() {
        let d = demon(&[]);
        let (code, rep) = requete(d.port, "GET", "/api/harness/machine", None);
        assert_eq!(code, 200, "{rep}");
        let annonce: Value = serde_json::from_str(rep.split("\r\n\r\n").nth(1).unwrap_or("{}")).unwrap();

        let cache = d._cache.path().join("machines");
        let fichiers: Vec<std::path::PathBuf> = std::fs::read_dir(&cache)
            .expect("le cache imposé doit porter le journal")
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("jsonl"))
            .collect();
        assert_eq!(fichiers.len(), 1, "un fichier par store servi : {fichiers:?}");
        let txt = std::fs::read_to_string(&fichiers[0]).expect("journal lisible");
        let lignes: Vec<&str> = txt.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(lignes.len(), 1, "une déclaration, une ligne : {txt}");
        let l: Value = serde_json::from_str(lignes[0]).expect("ligne JSON");
        assert_eq!(l["kind"], "machine");
        assert_eq!(l["store"], annonce["store"], "le journal dit ce que la route annonce");
        assert_eq!(l["instance"], annonce["instance"]);
        assert_eq!(l["mode"], "ro", "le mode offert est journalisé, pas supposé");
        assert!(l["declaree_ms"].as_i64().unwrap_or(0) > 0, "{l}");
    }

    /// **DEUX ÉCRIVAINS CONCURRENTS NE DOIVENT PAS ENTREMÊLER LEURS LIGNES.**
    ///
    /// Défaut MESURÉ, pas supposé : la clé du journal était l'instance, donc deux
    /// harnesses servant deux stores du même nom de base écrivaient dans le MÊME
    /// fichier — et comme `writeln!` fait plusieurs appels système, leurs lignes
    /// se sont entremêlées dans le cache d'une machine réelle :
    ///
    /// ```text
    /// {"arbre":"SALE"{,""arch"arbre:"":x86_64""SALE,"",base""arch:"":session-alpha.obrain""...
    /// ```
    ///
    /// Un journal qu'on ne peut plus relire ne déclare plus rien. Ce test tient le
    /// contrat à la place : chaque ligne est un JSON COMPLET, quel que soit le
    /// nombre d'écrivains, et aucune écriture n'est perdue.
    #[test]
    fn des_ecrivains_concurrents_ne_saccagent_pas_le_journal() {
        let td = tempfile::tempdir().expect("tempdir");
        let dir = td.path().join("machines");
        let identite = identite_de_test();
        let (fils, lignes_par_fil) = (8usize, 40usize);
        let ecrivains: Vec<std::thread::JoinHandle<()>> = (0..fils)
            .map(|i| {
                let dir = dir.clone();
                let l = ligne_declaration(&identite, 1_700_000_000_000 + i as i64);
                std::thread::spawn(move || {
                    for _ in 0..lignes_par_fil {
                        journaliser_declaration(&dir, &l).expect("journal");
                    }
                })
            })
            .collect();
        for e in ecrivains {
            e.join().expect("fil d'écriture");
        }

        let fichiers: Vec<std::path::PathBuf> =
            std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).collect();
        assert_eq!(fichiers.len(), 1, "un seul store servi, donc un seul fichier : {fichiers:?}");
        let txt = std::fs::read_to_string(&fichiers[0]).expect("journal lisible");
        let lignes: Vec<&str> = txt.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(
            lignes.len(),
            fils * lignes_par_fil,
            "aucune écriture perdue (le journal porte {} lignes)",
            lignes.len()
        );
        for l in lignes {
            serde_json::from_str::<Value>(l)
                .unwrap_or_else(|e| panic!("ligne entremêlée ({e}) : {l}"));
        }
    }

    /// Deux stores du même NOM DE BASE sur le même hôte : deux FICHIERS — c'est ce
    /// qui empêche deux processus de se disputer le même journal, et donc de
    /// s'entremêler. Leurs déclarations portent pourtant la même INSTANCE :
    /// l'identité du harness ne porte que le nom de base (`machine:<hôte>#<base>`),
    /// et cette ambiguïté n'est pas cachée — elle est dite, et la relecture de la
    /// passe en garde la plus récente.
    #[test]
    fn deux_stores_de_meme_nom_de_base_ecrivent_deux_fichiers() {
        let td = tempfile::tempdir().expect("tempdir");
        let dir = td.path().join("machines");
        let i1 = identite_machine("/data/a/kb.obrain", "127.0.0.1:8401", 1);
        let i2 = identite_machine("/data/b/kb.obrain", "127.0.0.1:8402", 2);
        assert_eq!(i1["instance"], i2["instance"], "même nom de base, même instance");
        assert_ne!(i1["store"], i2["store"]);
        journaliser_declaration(&dir, &ligne_declaration(&i1, 10)).expect("journal A");
        journaliser_declaration(&dir, &ligne_declaration(&i2, 20)).expect("journal B");

        let fichiers: Vec<std::path::PathBuf> =
            std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).collect();
        assert_eq!(
            fichiers.len(),
            2,
            "deux stores servis : deux fichiers, donc deux écrivains qui ne se rencontrent pas"
        );
    }

    /// **Une journalisation impossible ne fait PAS échouer le harness.** Un cache    /// qui ne peut pas être créé (ici : un fichier à la place d'un répertoire) est
    /// un cas réel — disque plein, cache en lecture seule, variable mal posée. Le
    /// moteur doit servir sa base quand même : c'est le même contrat que le
    /// journal d'activation et celui de la mémoire, et un test qui ne le vérifie
    /// pas laisserait passer une régression qui coupe le service pour une écriture
    /// de confort.
    #[test]
    fn une_journalisation_impossible_ne_fait_pas_echouer_le_harness() {
        let store = tempfile::tempdir().expect("tempdir store");
        let chemin = store.path().join("t.obrain");
        let db = Arc::new(ObrainDB::open_substrate(chemin.to_str().unwrap()).expect("store"));
        let j = store_avec_journaux(&[]);
        let cache = tempfile::tempdir().expect("tempdir cache");
        // Un FICHIER là où le journal voudrait un répertoire : `create_dir_all`
        // échoue, et aucun octet ne peut être écrit.
        let obstacle = cache.path().join("obstacle");
        std::fs::write(&obstacle, b"je ne suis pas un repertoire").unwrap();
        let journal = obstacle.join("machines");

        let listener = TcpListener::bind("127.0.0.1:0").expect("port éphémère");
        let port = listener.local_addr().unwrap().port();
        let label = j.path().display().to_string();
        std::thread::spawn(move || {
            let _ = serve_on_journalisant(listener, db, &label, &journal);
        });

        let (code, rep) = requete(port, "GET", "/api/harness/machine", None);
        assert_eq!(code, 200, "le harness doit servir MALGRÉ la journalisation perdue : {rep}");
        assert!(rep.contains("\"status\":\"ok\""), "{rep}");
    }
}
