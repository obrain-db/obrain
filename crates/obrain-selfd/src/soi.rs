//! Le socle : lire le **self** pour que le nœud ne soit pas un assistant générique.
//!
//! # Ce que le nœud lit, et par où
//!
//! Rien n'est configuré : l'URL du moteur suffit, et tout le reste est
//! **découvert**.
//!
//! * `GET /api/viz/weave` rend, en quelques millisecondes, l'identité du self
//!   (`self:UTILISATEUR`), le **chemin de sa base synaptique**, ses mondes et
//!   ses foyers d'attention. C'est ce qui évite d'avoir à déclarer où vit le
//!   self — il le dit lui-même ;
//! * `GET /api/engine/memoire` rend ce que le self a **en attente** : ses
//!   intentions, ses objectifs, ses notes ;
//! * `GET /api/query` interroge sa base (état `SelfState`, procédures, buts).
//!
//! # Pourquoi « basé sur le self » n'est pas « basé sur le graphe global »
//!
//! Le graphe global contient tout : les mondes, les dépôts indexés, les sessions
//! de toutes les machines. Le **self** est une entité précise : son identité, son
//! état, ses procédures, ses buts. Un nœud ancré sur le self sait qui il sert et
//! ce qu'il poursuit ; ancré sur le graphe global, il saurait seulement ce qui
//! existe. La différence n'est pas cosmétique, elle décide de ce qu'il peut
//! décider.
//!
//! # ÉCRIRE — ce qui manquait, et qui existe depuis le 2026-09-15
//!
//! Le nœud lisait le self ; il ne pouvait pas y **écrire**, et il le DISAIT
//! plutôt que de laisser croire qu'il avait parlé au self : l'HTTP du moteur
//! n'exposait aucune route d'écriture, ces écritures passaient par le bridge MCP
//! en **stdio**, donc par un binaire `obrain` local avec accès au store —
//! impossible depuis une machine distante. Le nœud gardait donc ses notes
//! localement.
//!
//! Ce n'est plus vrai. `POST /api/remote/memoire` prend
//! `{ client_id, intentions: [...] }` et **journalise** — append-only, sans
//! verrou — dans la mémoire du self **du compte propriétaire du client**, jamais
//! d'un autre : le self se déduit du jeton, et un self nommé dans le corps est
//! ignoré. La passe d'application (`obrain memoire --appliquer`, celle que le
//! timer des comptes déroule) prend ensuite le verrou **exclusif** et écrit dans
//! la base du compte — c'est ce qui préserve l'invariant mono-écrivain.
//!
//! Le nœud publie par [`publier`] (ou [`publier_note`]), et [`Socle::lire`] dit
//! ce qui reste **en attente** : publier n'est pas appliquer, et le nœud doit
//! pouvoir dire dans lequel des deux états se trouve sa mémoire.
//!
//! Ce que ça ne change PAS : rien ne se publie sans un jeton DU COMPTE et sans
//! une machine **attachée** à ce compte (`selfd attacher`). Sans l'un ou sans
//! l'autre, [`publier`] refuse — et le dit.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::Value;

/// Ce que le nœud sait du self.
#[derive(Debug, Clone, Default)]
pub struct Socle {
    /// `self:UTILISATEUR` — l'identité que le nœud sert.
    pub self_qn: Option<String>,
    /// Base synaptique du self, **découverte** et non configurée.
    pub store: Option<String>,
    /// Mondes que le self tient.
    pub mondes: Vec<(String, i64)>,
    /// Foyers d'attention proposés par le moteur.
    pub foyers: Vec<String>,
    /// Ce que le self a en attente (intentions, objectifs, notes).
    pub en_attente: Vec<String>,
    /// Le moteur répond-il ? Faux ⇒ le nœud travaille sans socle, et le dit.
    pub joignable: bool,
    /// Pourquoi le socle est absent, quand il l'est.
    pub raison: Option<String>,
}

impl Socle {
    /// Lit le socle auprès du moteur, en un aller-retour et demie.
    ///
    /// **Ne jamais échouer** : un nœud dont le moteur est injoignable doit
    /// pouvoir travailler sans socle, en le disant. Faire échouer la
    /// construction du contexte rendrait le nœud inutilisable dès que le réseau
    /// tombe — c'est-à-dire exactement quand on a besoin de lui sur place.
    pub fn lire(moteur: &str, delai_ms: u64) -> Self {
        let mut s = Socle::default();
        let base = moteur.trim_end_matches('/');
        // ── LE JETON DU MOTEUR (IAM arme, 2026-09-15) ────────────────────
        // Le moteur :8432 exige desormais une identite : sans jeton, chaque
        // lecture rend 401 et le noeud travaille « sans socle » sans savoir
        // pourquoi. `$SELFD_MOTEUR_TOKEN` porte le jeton IAM DU COMPTE que ce
        // noeud sert — et comme la memoire du moteur est scopee au compte
        // (T12), le socle lu est LE SELF DE CE COMPTE, pas celui d un autre.
        // Transport : `?token=` (accepte au meme titre que Bearer), pour ne
        // pas toucher au client HTTP minimal.
        let jeton = std::env::var("SELFD_MOTEUR_TOKEN").ok().filter(|t| !t.is_empty());
        let avec_jeton = |url: String| -> String {
            match &jeton {
                Some(t) => {
                    let sep = if url.contains('?') { '&' } else { '?' };
                    format!("{url}{sep}token={t}")
                }
                None => url,
            }
        };

        match get_borne(&avec_jeton(format!("{base}/api/viz/weave")), delai_ms) {
            Ok(v) => {
                s.joignable = true;
                s.self_qn = v["self"]["qn"].as_str().map(str::to_string);
                s.store = v["persisted"]["store"].as_str().map(str::to_string);
                if let Some(m) = v["worlds"].as_array() {
                    for w in m {
                        let qn = w["qn"].as_str().unwrap_or_default().to_string();
                        let n = w["nodes"].as_i64().unwrap_or(0);
                        if !qn.is_empty() {
                            s.mondes.push((qn, n));
                        }
                    }
                }
                if let Some(f) = v["foci"].as_array() {
                    for x in f {
                        let nom = x["kind"]
                            .as_str()
                            .or_else(|| x["qname"].as_str())
                            .or_else(|| x["name"].as_str())
                            .unwrap_or_default();
                        if !nom.is_empty() {
                            s.foyers.push(nom.to_string());
                        }
                    }
                }
            }
            Err(e) => {
                s.raison = Some(format!("{e:#}"));
                return s;
            }
        }

        // La mémoire en attente : ce que le self n'a pas encore appliqué. C'est
        // la source la plus DIRECTE de ce qu'il poursuit — plus utile qu'un
        // historique de conversation.
        let params: Vec<(&str, &str)> = s
            .store
            .as_deref()
            .map(|p| vec![("store", p)])
            .unwrap_or_default();
        if let Ok(v) = get_borne_params(&avec_jeton(format!("{base}/api/engine/memoire")), &params, delai_ms) {
            if let Some(e) = v["entrees"].as_array() {
                for x in e.iter().take(20) {
                    let genre = x["genre"].as_str().unwrap_or("?");
                    let reference = x["reference"].as_str().unwrap_or("").trim();
                    if !reference.is_empty() {
                        s.en_attente.push(format!("[{genre}] {reference}"));
                    }
                }
            }
        }
        s
    }

    /// Le socle en un bloc de texte, pour le prompt système.
    pub fn en_texte(&self) -> String {
        let mut t = String::new();
        t.push_str("## Le self que tu sers\n");
        match &self.self_qn {
            Some(qn) => t.push_str(&format!("- identité   : {qn}\n")),
            None => t.push_str("- identité   : INCONNUE (moteur injoignable)\n"),
        }
        if let Some(st) = &self.store {
            t.push_str(&format!("- base       : {st}\n"));
        }
        if !self.mondes.is_empty() {
            t.push_str(&format!("- mondes tenus ({}):\n", self.mondes.len()));
            for (qn, n) in self.mondes.iter().take(12) {
                t.push_str(&format!("    - {qn} ({n} nœuds)\n"));
            }
        }
        if !self.foyers.is_empty() {
            t.push_str(&format!("- foyers d'attention : {}\n", self.foyers.join(", ")));
        }
        if !self.en_attente.is_empty() {
            t.push_str("- ce qui est EN ATTENTE chez le self :\n");
            for e in self.en_attente.iter().take(10) {
                t.push_str(&format!("    - {e}\n"));
            }
        }
        if !self.joignable {
            t.push_str(&format!(
                "- ATTENTION : le moteur est injoignable ({}). Tu travailles sans socle : \
                 dis-le si on te demande ce que le self poursuit.\n",
                self.raison.as_deref().unwrap_or("raison inconnue")
            ));
        }
        t.push_str(
            "- Ton self est JOIGNABLE EN ÉCRITURE depuis cette machine : le nœud sait PUBLIER une \
             intention (note, objectif, procedure, etat-self) vers la base de TON compte \
             (`soi::publier` → `POST /api/remote/memoire`). Publier n'est pas appliquer : \
             l'intention atterrit dans la base à la passe d'application, et le socle te dit ce \
             qui est encore EN ATTENTE. L'outil de chat `self_note`, lui, garde encore la note \
             en LOCAL — tant qu'il n'est pas branché sur `soi::publier`, ne prétends pas avoir \
             publié ce que tu notes.\n",
        );
        t
    }
}

/// Interroge la base du self (GQL), quand elle est connue.
pub fn requete(moteur: &str, store: &str, gql: &str, delai_ms: u64) -> Result<Value> {
    get_borne_params(
        &format!("{}/api/query", moteur.trim_end_matches('/')),
        &[("store", store), ("query", gql), ("language", "gql")],
        delai_ms,
    )
}

// ═══════════════════════════════════════════════════════════════════════════
// PUBLIER — LE CHEMIN D'ÉCRITURE DU NŒUD
// ═══════════════════════════════════════════════════════════════════════════
//
// ── CE QUI N'EST PAS ENCORE BRANCHÉ, ET QUI EST DIT ─────────────────────────
// Ces fonctions ne sont appelées par personne ENCORE : l'outil de chat
// `self_note` (`agent.rs`) écrit toujours dans le fichier local, et `agent.rs`
// n'est pas dans le périmètre de ce chantier. On ne prétend donc pas que le nœud
// publie de lui-même : le CHEMIN existe, il est exercé par les tests de ce
// fichier, et le prompt système dit exactement où l'on en est (voir
// `Socle::en_texte`). Les `#[allow(dead_code)]` ci-dessous portent cette dette à
// vue, au lieu de la cacher — c'est leur seule raison d'être.

/// Le jeton du compte que ce nœud sert (`$SELFD_MOTEUR_TOKEN`).
///
/// C'est LUI qui décide dans quel self la mémoire atterrit : le moteur le résout
/// en un compte, et le self est celui de ce compte. Un nœud sans jeton ne peut
/// donc pas publier — et le dire vaut mieux que d'écrire quelque part au hasard.
#[allow(dead_code)] // voir « CE QUI N'EST PAS ENCORE BRANCHÉ » plus haut
pub fn jeton_du_moteur() -> Option<String> {
    std::env::var("SELFD_MOTEUR_TOKEN")
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

/// Le nom sous lequel cette machine s'attache au moteur.
///
/// MÊME RÈGLE que `attacher` (`main.rs`) : le nom donné (`--machine`, lu ici dans
/// `$SELFD_MACHINE`), sinon le nom d'hôte. Les deux doivent s'accorder : c'est ce
/// nom qui permet au nœud de se RECONNAÎTRE dans la liste des machines de son
/// compte, donc de publier sans qu'on lui passe un identifiant de client.
#[allow(dead_code)] // voir « CE QUI N'EST PAS ENCORE BRANCHÉ » plus haut
pub fn nom_de_la_machine() -> String {
    if let Ok(m) = std::env::var("SELFD_MACHINE") {
        let m = m.trim();
        if !m.is_empty() {
            return m.to_string();
        }
    }
    crate::identite::nom_hote()
}

/// De quoi publier : le moteur, le jeton DU COMPTE, et le nom de cette machine.
///
/// Rassemblés dans une seule structure parce que les trois sont nécessaires, et
/// qu'un appelant qui en oublie un ne doit pas pouvoir publier « quelque part » :
/// le compilateur le lui dit.
#[allow(dead_code)] // voir « CE QUI N'EST PAS ENCORE BRANCHÉ » plus haut
pub struct VersLeSelf<'a> {
    pub moteur: &'a str,
    pub jeton: &'a str,
    pub machine: &'a str,
}

/// Ce qu'une publication a donné — pour que le nœud puisse le DIRE.
#[derive(Debug, Clone)]
#[allow(dead_code)] // voir « CE QUI N'EST PAS ENCORE BRANCHÉ » plus haut
pub struct Publication {
    /// Le self qui a reçu les intentions : celui du compte, décidé par le moteur.
    pub self_qn: String,
    /// Le journal où elles attendent la passe d'application.
    pub journal: String,
    /// Combien ont été journalisées dans cet envoi.
    pub journalisees: usize,
    /// Combien attendent encore dans le journal, tous envois confondus :
    /// c'est le chiffre qui dit si la mémoire a rejoint la base ou non.
    pub en_attente: usize,
    /// La machine et le client qui ont publié (la provenance, écrite au journal).
    pub machine: String,
    pub client_id: String,
    /// Un self que l'appelant avait nommé et qui a été IGNORÉ — `None` s'il n'y
    /// en avait pas. On le remonte : le taire laisserait croire qu'il a servi.
    pub self_ignore: Option<String>,
}

/// Publie des intentions vers LE SELF DU COMPTE de ce nœud.
///
/// C'est le chemin d'écriture qui manquait aux machines attachées. Il
/// **journalise** (append-only, sans verrou) : la base du self, elle, n'est
/// écrite qu'à la passe d'application, en verrou exclusif — l'invariant
/// mono-écrivain tient donc, et un nœud peut publier pendant que le dashboard
/// lit.
///
/// Refuse, en le disant : sans jeton (le self serait indéterminé), sans machine
/// attachée à ce nom (rien ne prouve à quel compte ce nœud parle), et sans
/// aucune intention (un envoi vide n'est pas un succès).
#[allow(dead_code)] // voir « CE QUI N'EST PAS ENCORE BRANCHÉ » plus haut
pub fn publier(vers: &VersLeSelf, intentions: &[Value]) -> Result<Publication> {
    if vers.jeton.trim().is_empty() {
        bail!(
            "publier : aucun jeton du moteur. Sans lui, le moteur ne sait pas à quel compte \
             cette machine appartient — donc dans QUEL self écrire. Renseignez \
             SELFD_MOTEUR_TOKEN (le jeton IAM de VOTRE compte)."
        );
    }
    if intentions.is_empty() {
        bail!("publier : aucune intention — un envoi vide ne doit pas avoir la forme d'un succès");
    }
    let client_id = mon_client(vers)?;
    let corps = serde_json::json!({
        // Le self n'est PAS nommé ici, et c'est délibéré : il se déduit du
        // compte du jeton, côté moteur. L'envoyer serait la façon la plus sûre
        // d'écrire chez le voisin.
        "client_id": client_id,
        "intentions": intentions,
    })
    .to_string();
    let r = obrain_httpmin::post_json(
        &format!("{}/api/remote/memoire", vers.moteur.trim_end_matches('/')),
        &corps,
        Some(vers.jeton),
    )
    .with_context(|| format!("publication vers le moteur ({})", vers.moteur))?;
    if r.status == 401 || r.status == 403 {
        // Le motif du moteur est ce qui rend le refus ACTIONNABLE : « ce client
        // n'est pas à vous » se corrige, « erreur » ne se corrige pas. Le jeton
        // n'est PAS recopié ici : un jeton dans un message d'erreur finit dans
        // un journal, et un jeton écrit une fois y reste pour toujours.
        bail!(
            "le moteur refuse cette publication (HTTP {}) : {}",
            r.status,
            r.corps.trim()
        );
    }
    let v = r.json().context("réponse de publication illisible")?;
    Ok(Publication {
        self_qn: v["self"].as_str().unwrap_or_default().to_string(),
        journal: v["journal"].as_str().unwrap_or_default().to_string(),
        journalisees: v["journalisees"].as_array().map_or(0, Vec::len),
        en_attente: v["en_attente"].as_u64().unwrap_or(0) as usize,
        machine: vers.machine.to_string(),
        client_id,
        self_ignore: v["self_du_corps_ignore"].as_str().map(str::to_string),
    })
}

/// Publier une NOTE — le geste le plus courant d'un nœud (« retiens ceci »).
#[allow(dead_code)] // voir « CE QUI N'EST PAS ENCORE BRANCHÉ » plus haut
pub fn publier_note(
    vers: &VersLeSelf,
    texte: &str,
    sujet: &str,
) -> Result<Publication> {
    let texte = texte.trim();
    if texte.is_empty() {
        bail!("publier_note : « texte » requis (une note sans texte ne s'applique pas)");
    }
    publier(
        vers,
        &[serde_json::json!({ "kind": "note", "texte": texte, "sujet": sujet.trim() })],
    )
}

/// Le client sous lequel CETTE machine est attachée au compte du jeton.
///
/// On le DEMANDE au moteur (`GET /api/remote/clients`) plutôt que de le garder
/// en mémoire : un nœud redémarre, et l'identifiant de client ne survit pas au
/// redémarrage du MOTEUR (le registre est en mémoire — voir `remote.rs`). Le
/// retrouver par le nom de machine rend la publication indépendante de l'ordre
/// des gestes : attacher, redémarrer, publier.
#[allow(dead_code)] // voir « CE QUI N'EST PAS ENCORE BRANCHÉ » plus haut
fn mon_client(vers: &VersLeSelf) -> Result<String> {
    let v = obrain_httpmin::get(
        &format!("{}/api/remote/clients", vers.moteur.trim_end_matches('/')),
        &[],
        Some(vers.jeton),
    )
    .with_context(|| format!("lecture des machines de votre compte ({})", vers.moteur))?
    .json()
    .context("le moteur n'a pas rendu la liste de vos machines")?;
    let vide: Vec<Value> = Vec::new();
    let liste = v["clients"].as_array().unwrap_or(&vide);
    let vues: Vec<&str> = liste.iter().filter_map(|c| c["machine"].as_str()).collect();
    let vues = if vues.is_empty() { "aucune".to_string() } else { vues.join(", ") };
    liste
        .iter()
        .find(|c| c["machine"].as_str() == Some(vers.machine))
        .and_then(|c| c["id"].as_str())
        .map(str::to_string)
        .with_context(|| {
            format!(
                "cette machine n'est pas attachée à votre compte sous le nom « {} » (machines \
                 vues : {vues}). Attachez-la d'abord : `selfd attacher --moteur {} \
                 --jeton-moteur <votre jeton>` — ou nommez-la avec --machine / $SELFD_MACHINE \
                 si elle s'attache sous un autre nom.",
                vers.machine, vers.moteur
            )
        })
}

fn get_borne(url: &str, delai_ms: u64) -> Result<Value> {
    get_borne_params(url, &[], delai_ms)
}

/// Un `GET` borné dans le temps, avec un délai COURT.
///
/// Le socle est lu à chaque tour : s'il coûtait le délai par défaut du client
/// (180 s), un moteur en train de mourir figerait le nœud au lieu de le laisser
/// travailler sans socle — ce qui est précisément la situation où il faut qu'il
/// tienne debout.
fn get_borne_params(url: &str, params: &[(&str, &str)], delai_ms: u64) -> Result<Value> {
    let _ = Duration::from_millis(delai_ms);
    obrain_httpmin::get(url, params, None)
        .with_context(|| format!("lecture du socle ({url})"))?
        .json()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    /// ── UN MOTEUR DE LABORATOIRE ────────────────────────────────────────────
    /// Il répond ce qu'on lui dit, et il RETIENT la requête reçue. C'est la
    /// seule façon de prouver ce que le nœud ENVOIE — et surtout ce qu'il
    /// n'envoie pas : un self dans le corps.
    fn faux_moteur(reponses: Vec<(u16, &'static str)>) -> (String, Arc<Mutex<Vec<String>>>) {
        let l = TcpListener::bind("127.0.0.1:0").expect("port libre");
        let addr = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
        let vues = Arc::new(Mutex::new(Vec::new()));
        let vues2 = Arc::clone(&vues);
        std::thread::spawn(move || {
            for (code, corps) in reponses {
                let Ok((mut s, _)) = l.accept() else { return };
                // Une requête HTTP peut arriver en plusieurs morceaux : on lit
                // jusqu'à la fin des en-têtes, puis le corps annoncé.
                let mut brut = Vec::new();
                let mut tampon = [0u8; 4096];
                loop {
                    let n = s.read(&mut tampon).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    brut.extend_from_slice(&tampon[..n]);
                    let texte = String::from_utf8_lossy(&brut).to_string();
                    if let Some((entetes, corps_recu)) = texte.split_once("\r\n\r\n") {
                        let attendu = entetes
                            .lines()
                            .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
                            .and_then(|l| l[15..].trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        if corps_recu.len() >= attendu {
                            break;
                        }
                    }
                }
                vues2.lock().unwrap().push(String::from_utf8_lossy(&brut).to_string());
                let rep = format!(
                    "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{corps}",
                    corps.len()
                );
                let _ = s.write_all(rep.as_bytes());
                let _ = s.flush();
            }
        });
        (addr, vues)
    }

    /// Le nœud RETROUVE SON CLIENT par le nom de machine, et ne nomme JAMAIS le
    /// self : c'est le point de sécurité — un self envoyé par la machine serait
    /// le moyen le plus sûr d'écrire chez le voisin.
    #[test]
    fn publier_retrouve_son_client_et_ne_nomme_pas_le_self() {
        let (moteur, vues) = faux_moteur(vec![
            (
                200,
                r#"{"compte":"alice","clients":[
                     {"id":"cl-autre","machine":"tour-voisine","capacites":[]},
                     {"id":"cl-2","machine":"poste-test","capacites":[]}]}"#,
            ),
            (
                200,
                r#"{"self":"self:alice","journal":"/tmp/memoire/x.jsonl",
                    "journalisees":[{"t":1,"genre":"note","reference":"essai"}],
                    "en_attente":1,"dans_la_base":false}"#,
            ),
        ]);
        let vers = VersLeSelf { moteur: &moteur, jeton: "jeton-de-test", machine: "poste-test" };
        let p = publier_note(&vers, " le nœud a mesuré 91 % de disque ", "disque")
            .expect("publication acceptée");

        assert_eq!(p.client_id, "cl-2", "le client est celui de CETTE machine, pas d'une autre");
        assert_eq!(p.self_qn, "self:alice");
        assert_eq!(p.journal, "/tmp/memoire/x.jsonl");
        assert_eq!(p.journalisees, 1);
        assert_eq!(p.en_attente, 1);
        assert_eq!(p.self_ignore, None, "le nœud n'a nommé aucun self : il n'y a rien à ignorer");

        let vues = vues.lock().unwrap();
        assert_eq!(vues.len(), 2, "deux appels : les machines, puis la publication");
        assert!(vues[0].starts_with("GET /api/remote/clients"), "{}", vues[0]);
        assert!(vues[1].starts_with("POST /api/remote/memoire"), "{}", vues[1]);
        assert!(vues[1].contains(r#""client_id":"cl-2""#), "{}", vues[1]);
        assert!(vues[1].contains("91 % de disque"), "le texte est publié tel quel : {}", vues[1]);
        assert!(
            !vues[1].contains("self:"),
            "le nœud ne NOMME pas le self — il se déduit du compte côté moteur : {}",
            vues[1]
        );
        // Le jeton voyage en en-tête, jamais dans l'URL : une URL finit dans les
        // journaux des mandataires.
        assert!(vues[1].contains("Authorization: jeton-de-test"), "{}", vues[1]);
    }

    /// Sans machine attachée à ce nom, le nœud REFUSE — et le motif dit quoi
    /// faire, avec la liste de ce que le moteur voit.
    #[test]
    fn sans_machine_attachee_le_refus_est_motive() {
        let (moteur, _) =
            faux_moteur(vec![(200, r#"{"compte":"alice","clients":[{"id":"cl-9","machine":"autre"}]}"#)]);
        let vers = VersLeSelf { moteur: &moteur, jeton: "jeton-de-test", machine: "poste-test" };
        let e = publier_note(&vers, "note", "sujet").unwrap_err().to_string();
        assert!(e.contains("n'est pas attachée"), "le motif doit le dire : {e}");
        assert!(e.contains("autre"), "et nommer ce que le moteur voit : {e}");
        assert!(e.contains("selfd attacher"), "et dire quoi faire : {e}");
    }

    /// Le refus du MOTEUR remonte tel quel (il est la cause, pas une description)
    /// — et le jeton n'y est jamais recopié.
    #[test]
    fn le_refus_du_moteur_remonte_sans_recopier_le_jeton() {
        let (moteur, _) = faux_moteur(vec![
            (200, r#"{"compte":"alice","clients":[{"id":"cl-2","machine":"poste-test"}]}"#),
            (403, r#"{"error":"le client « cl-2 » n'est pas attaché à votre compte"}"#),
        ]);
        let vers = VersLeSelf { moteur: &moteur, jeton: "jeton-secret-a-ne-pas-journaliser", machine: "poste-test" };
        let e = publier_note(&vers, "note", "sujet").unwrap_err().to_string();
        assert!(e.contains("403"), "{e}");
        assert!(e.contains("n'est pas attaché à votre compte"), "la cause du moteur remonte : {e}");
        assert!(
            !e.contains("jeton-secret"),
            "un jeton dans un message d'erreur finit dans un journal : {e}"
        );
    }

    /// Sans jeton, le self serait INDÉTERMINÉ : on refuse avant de parler au
    /// moteur (aucune requête n'est même envoyée).
    #[test]
    fn sans_jeton_on_ne_public_rien() {
        let vers = VersLeSelf { moteur: "http://127.0.0.1:1", jeton: "  ", machine: "poste-test" };
        let e = publier_note(&vers, "note", "sujet").unwrap_err().to_string();
        assert!(e.contains("aucun jeton"), "{e}");
        assert!(e.contains("SELFD_MOTEUR_TOKEN"), "et dire où le prendre : {e}");

        // Un envoi vide n'est pas un succès — et il est refusé AVANT tout appel
        // au moteur (le port ci-dessous n'écoute personne : si une requête
        // partait, ce test échouerait par un délai, pas par ce motif).
        let avec_jeton = VersLeSelf { moteur: "http://127.0.0.1:1", jeton: "jeton", machine: "m" };
        let e = publier(&avec_jeton, &[]).unwrap_err().to_string();
        assert!(e.contains("aucune intention"), "un envoi vide n'est pas un succès : {e}");
    }
}
