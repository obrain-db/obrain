//! REMOTE — le registre des clients connectés, et le canal qui les joint.
//!
//! ── POURQUOI CE MODULE EXISTE ────────────────────────────────────────────────
//! Le Self d'un compte doit pouvoir agir sur LA MACHINE de ce compte. Or cette
//! machine est presque toujours derrière un routeur : le moteur ne peut pas
//! l'appeler. C'est donc le client qui appelle — il ouvre une connexion
//! SORTANTE, la tient, et le moteur pousse ses ordres dedans.
//!
//! Trois décisions, dictées par ce qui existe plutôt que par le goût :
//!
//! 1. **Rien d'asynchrone.** `viz serve` est std-only, un fil par connexion.
//!    Une attente bornée par `Condvar` fait exactement le travail d'une boucle
//!    d'événements ici, sans rien ajouter au socle. Le verrou n'est JAMAIS tenu
//!    pendant une attente (`wait_timeout` le rend, c'est son contrat).
//!
//! 2. **L'identité vient de l'IAM, pas d'un identifiant de client.** Un
//!    `client_id` circule dans des URL et des journaux : le traiter comme un
//!    secret serait bâtir sur du sable. Ce qui autorise, c'est le compte porté
//!    par le jeton ; le registre refuse tout appel dont le compte ne possède
//!    pas le client. Deviner un identifiant ne donne donc rien.
//!
//! 3. **Le client garde le dernier mot.** Le moteur envoie une DEMANDE, pas un
//!    ordre exécutoire : la politique locale de `selfd` (racine confinée,
//!    exécution sur autorisation explicite) est seule à décider. Un refus
//!    revient comme un résultat motivé — jamais comme un silence.
//!
//! ── LE DÉFAUT QUE LA PERSISTANCE CORRIGE (2026-09-16) ───────────────────────
//! Jusqu'ici ce registre vivait ENTIÈREMENT en mémoire : un redémarrage du
//! moteur effaçait la flotte, SILENCIEUSEMENT. `docs/MACHINES-CONNUES-2026-09-15.md`
//! le dit sans détour — « on ne peut pas piloter une machine qui n'a pas de
//! nom », ni auditer **par machine**. Un registre qui oublie au premier
//! redémarrage ne fait pas de la machine un nœud de premier ordre : il en fait
//! un état de session.
//!
//! Le remède reprend le mécanisme que ce dépôt utilise déjà pour survivre à un
//! arrêt (`memoire.rs`, `activation.rs`) plutôt que d'en inventer un second :
//!
//! * **un JOURNAL append-only, un fichier par COMPTE**, sous le même cache que
//!   l'activation et la mémoire (`OBRAIN_VIZ_CACHE_DIR`, défaut
//!   `/tmp/obrain-viz-cache`, ici `…/machines/`). Aucun verrou n'est pris : ce
//!   fichier n'est pas la base d'un Self, c'est le nôtre, et nous en sommes le
//!   seul écrivain. Best-effort assumé — un registre qui ne peut pas s'écrire
//!   continue de servir la session en cours, mais il le DIT (voir
//!   [`Registre::journaliser`]) : le silence sur une écriture perdue est
//!   précisément le défaut qu'on répare ;
//! * **une relecture au démarrage** : le registre se reconstruit en rejouant les
//!   journaux. Le nom de fichier est le hachage du compte (un compte peut porter
//!   n'importe quel caractère, un nom de fichier non), donc — comme le journal
//!   de mémoire porte son `self` — c'est la LIGNE qui dit à qui appartient la
//!   machine. On n'inverse jamais le hachage, et le cloisonnement se vérifie en
//!   lisant le fichier ;
//! * **le compte n'est pas un filtre d'affichage, c'est une frontière** : la
//!   relecture ne mélange rien, et `clients_du_compte` re-filtre sur le compte
//!   porté par la ligne. Un compte ne voit jamais les machines d'un autre,
//!   avant comme après un redémarrage (prouvé par test).
//!
//! ── L'ANNONCE A DEUX MOITIÉS, ET LES DEUX ARRIVENT (2026-09-16) ─────────────
//! Un nœud annonce ce qu'il SAIT faire (`capacites`) **et ce qu'il REFUSE**,
//! motivé (`capacites_refusees`), plus ses limites (racine réelle, délai, octets
//! par flux, `exec` ouvert ou fermé). La seconde moitié était calculée par sa
//! politique, envoyée... et **jetée ici, en silence** : ce module ne retenait que
//! `capacites`. Le résultat est le défaut que la doctrine nomme mot pour mot :
//! « le modèle propose des gestes qu'il ne peut pas prendre, et l'échec arrive
//! après coup ».
//!
//! Ce qui arrive désormais est gardé ENTIER, journalisé avec le reste (donc
//! survivant au redémarrage : un fait daté, pas un état de session), et rendu à
//! l'appelant de `/api/remote/machines` et `/api/remote/clients`.
//!
//! **Et ce qui n'est pas arrivé est DIT.** Un nœud tout ouvert envoie
//! `capacites_refusees: []` : ce vide est une *bonne* réponse. Un champ ABSENT
//! n'en est pas une, et les deux ne se ressemblent plus — c'est [`Annonce`], et
//! le bloc `annonce` de [`Client::en_json`]. Une liste vide qui a la forme d'une
//! bonne réponse est exactement le mode de défaillance que la doctrine interdit.
//!
//! ── LE MOTEUR NE MASQUE PAS UNE CAPACITÉ ABSENTE ────────────────────────────
//! La conséquence de la moitié perdue se voyait à l'appel : le moteur poussait
//! l'ordre, la machine le refusait, et l'appelant apprenait **après coup** — au
//! bout de trente secondes — un refus que la machine avait annoncé d'avance, avec
//! son motif. Le repli générique (« le client n'a pas répondu en 30 s ») est
//! remplacé par un refus IMMÉDIAT et MOTIVÉ ([`refus_annonce`]) :
//!
//! * la machine a dit qu'elle refusait — son motif part tel quel, il est
//!   actionnable (« hors de la racine autorisée » se corrige, « erreur » non) ;
//! * la machine ne l'a pas annoncée du tout — on le dit, en nommant ce qu'elle
//!   annonce ;
//! * la machine n'a rien annoncé (`capacites` vide) — **on ne refuse rien** :
//!   le silence n'est pas une mesure, et refuser au nom d'une mesure qu'on n'a
//!   pas serait décider à la place du nœud. L'ordre part, le nœud tranche.
//! * l'outil n'appartient pas au vocabulaire des capacités (`chat`, `caps`,
//!   `run`) — on ne refuse rien non plus : le moteur ne connaît pas la table de
//!   routes du nœud, et un refus inventé pour un geste que la machine sait faire
//!   serait la faute symétrique.
//!
//! ── CONNUE N'EST PAS JOIGNABLE — « ANNONCER EST DÉCIDER » ────────────────────
//! Une machine relue depuis le journal n'a parlé qu'à un processus MORT. La
//! remettre dans la liste sans le dire ferait annoncer comme joignable une
//! machine dont on n'a aucun battement : exactement ce que la doctrine interdit.
//! D'où deux états, distincts et affichés :
//!
//! * **connue** (`vivant: false`) — le journal la porte, son identifiant et son
//!   historique sont retrouvés, mais personne n'a frappé à la porte de CE
//!   processus. On ne lui envoie pas d'ordre : `appeler` refuse, et le refus
//!   dit depuis combien de temps elle se tait ;
//! * **vivante** (`vivant: true`) — un battement a été vu récemment (une
//!   demande d'ordre ou un verdict de sa part, y compris une attente sans
//!   ordre : c'est le battement de cœur du canal). Une machine connue qui
//!   redemande du travail redevient vivante — c'est le seul geste qui prouve
//!   qu'elle est là, et il exige toujours le compte du jeton.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Duration;

/// Au-delà de ce silence, un client est considéré parti : il ne figure plus
/// dans la liste et on cesse de lui garder des ordres. C'est LARGEMENT plus que
/// la fenêtre de long-poll (25 s), pour qu'un client lent ou une reconnexion en
/// cours ne soit jamais pris pour un absent.
const SILENCE_MAX_MS: i64 = 90_000;

/// Cadence MINIMALE d'écriture des battements au journal.
///
/// Un client redemande du travail toutes les 25 s : journaliser chaque battement
/// ferait ~3 500 lignes par jour et par machine pour une information qui ne
/// change pas — un journal qui croît sans fin finit par ne plus être lu, ce qui
/// revient à ne pas l'avoir. Une minute est très en deçà de [`SILENCE_MAX_MS`] :
/// une présence écrite est donc toujours une présence FRAÎCHE.
const BATTEMENT_DURABLE_MS: i64 = 60_000;

/// Taille au-delà de laquelle le journal d'un compte est COMPACTÉ (voir
/// [`Registre::compacter_si_besoin`]).
const JOURNAL_MAX_OCTETS: u64 = 256 * 1024;

/// Une demande d'action adressée à un client.
#[derive(Clone, Debug)]
pub struct Ordre {
    /// Identifiant de CET appel — c'est lui qui apparie la réponse à la demande.
    pub appel_id: String,
    pub outil: String,
    pub arguments: serde_json::Value,
    pub emis_ms: i64,
}

impl Ordre {
    #[must_use]
    pub fn en_json(&self) -> serde_json::Value {
        serde_json::json!({
            "appel_id": self.appel_id,
            "outil": self.outil,
            "arguments": self.arguments,
            "emis_ms": self.emis_ms,
        })
    }
}

/// CE QUE LA MACHINE A DIT D'ELLE-MÊME, EN ENTIER — les deux moitiés.
///
/// `manquante` est le champ qui compte autant que les autres : il nomme ce que le
/// corps d'attachement **ne portait pas**. C'est lui qui empêche une annonce
/// tronquée d'avoir la forme d'une annonce complète (voir l'en-tête du module).
#[derive(Clone, Debug, Default)]
pub struct Annonce {
    /// Ce qui est exercable, tel que la politique de la machine l'a MESURÉ.
    pub capacites: Vec<String>,
    /// Ce qu'elle refuse, avec le motif de sa politique locale — la moitié
    /// qu'on perdait, et sans laquelle un refus ne se corrige pas.
    pub capacites_refusees: Vec<serde_json::Value>,
    /// Racine réelle, délai par défaut, octets par flux, `exec` ouvert ou fermé,
    /// version du binaire. `None` = la machine n'a rien dit de ses limites : ce
    /// n'est pas « aucune limite », c'est une ignorance, et elle se dit.
    pub limites: Option<serde_json::Value>,
    /// Les moitiés du corps d'attachement qui ne sont PAS arrivées, NOMMÉES.
    /// Vide = annonce complète.
    pub manquante: Vec<String>,
}

impl Annonce {
    /// L'ancien chemin : la seule liste de capacités. Il ne *devient* pas
    /// complet — il est dit incomplet, parce qu'il l'est.
    pub fn depuis_capacites(capacites: Vec<String>) -> Self {
        Self {
            capacites,
            capacites_refusees: Vec::new(),
            limites: None,
            manquante: vec!["capacites_refusees".to_string(), "limites".to_string()],
        }
    }

    /// Le chemin complet : le corps d'attachement tel que la machine l'envoie.
    ///
    /// La complétude se mesure sur la PRÉSENCE des clés, jamais sur leur contenu :
    /// un nœud tout ouvert envoie `capacites_refusees: []`, et ce tableau vide est
    /// une réponse pleine (« je ne refuse rien »). Le confondre avec un champ
    /// absent ferait dire au registre l'inverse de ce que la machine a dit.
    pub fn depuis_json(v: &serde_json::Value) -> Self {
        let capacites = v
            .get("capacites")
            .and_then(|c| c.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        // Un refus annoncé est un OBJET (`nom`, `exercable`, `motif`). On ne garde
        // que les objets : un refus sans nom ne se corrige pas, et le stocker
        // donnerait une liste où l'on ne peut rien chercher.
        let capacites_refusees = v
            .get("capacites_refusees")
            .and_then(|c| c.as_array())
            .map(|a| a.iter().filter(|x| x.is_object()).cloned().collect())
            .unwrap_or_default();
        let limites = v.get("limites").filter(|l| l.is_object()).cloned();

        let mut manquante = Vec::new();
        if v.get("capacites").is_none() {
            manquante.push("capacites".to_string());
        }
        if v.get("capacites_refusees").is_none() {
            manquante.push("capacites_refusees".to_string());
        }
        if v.get("limites").is_none() {
            manquante.push("limites".to_string());
        }
        Self {
            capacites,
            capacites_refusees,
            limites,
            manquante,
        }
    }
}

/// Ce qu'un appelant peut remettre à [`attacher`] : le corps d'attachement
/// ENTIER (le chemin qui porte les refus et les limites), ou la seule liste de
/// capacités — l'ancien chemin, conservé pour ne casser aucun appelant.
///
/// Ce trait est ce qui rend le passage au corps entier **sans rupture** : un
/// appelant qui détient déjà la requête (`&Value`) remet ce qu'il a, un appelant
/// qui n'a que la liste continue de compiler. Ce qu'il remet est ensuite DIT
/// dans l'annonce (`Annonce::manquante`) : jamais présumé complet.
pub trait AnnonceEntrante {
    fn en_annonce(self) -> Annonce;
}

impl AnnonceEntrante for Vec<String> {
    fn en_annonce(self) -> Annonce {
        Annonce::depuis_capacites(self)
    }
}

impl AnnonceEntrante for &serde_json::Value {
    fn en_annonce(self) -> Annonce {
        Annonce::depuis_json(self)
    }
}

/// Une annonce DÉJÀ construite passe telle quelle : c'est ce qui permet à
/// l'entrée publique et au registre de partager la même signature sans convertir
/// deux fois.
impl AnnonceEntrante for Annonce {
    fn en_annonce(self) -> Annonce {
        self
    }
}

/// Un client attaché : une machine, un compte, et ce qu'il DIT savoir faire.
#[derive(Clone, Debug)]
pub struct Client {
    pub id: String,
    /// Le compte IAM propriétaire — la seule chose qui autorise.
    pub compte: String,
    pub machine: String,
    /// Les capacités ANNONCÉES par le client, c'est-à-dire ce que sa politique
    /// locale lui permet. Le moteur les affiche ; il ne s'en sert jamais pour
    /// autoriser (c'est au client de refuser, et il le fait).
    ///
    /// Tableau de chaînes, et il le RESTE : l'interface et la porte le lisent
    /// ainsi depuis le premier jour. Ce qui s'ajoute est à côté, jamais dedans.
    pub capacites: Vec<String>,
    /// Ce que la machine a ANNONCÉ REFUSER (`nom`, `exercable: false`, `motif`).
    /// Le motif est ce qui rend le refus actionnable : c'est lui qui évite au
    /// modèle de proposer un geste voué au refus, et à l'utilisateur de
    /// l'apprendre par un échec.
    pub capacites_refusees: Vec<serde_json::Value>,
    /// Les LIMITES annoncées : racine réelle, délai, octets par flux, `exec`
    /// ouvert ou fermé, version.
    pub limites: Option<serde_json::Value>,
    /// Les moitiés de l'annonce qui ne sont PAS arrivées, nommées. Vide = l'annonce
    /// est complète (voir [`Annonce`]). Une ligne de journal écrite avant que ce
    /// module ne sache garder ces moitiés n'en porte aucune trace : elle est donc
    /// relue comme INCOMPLÈTE, pas comme « rien à signaler ».
    pub annonce_manquante: Vec<String>,
    pub attache_ms: i64,
    pub derniere_vue_ms: i64,
    /// Cette machine a-t-elle parlé à CE processus ? `false` = connue du
    /// journal, mais pas joignable (voir l'en-tête du module). Ce champ n'est
    /// JAMAIS relu du journal : une présence écrite par un processus mort ne
    /// vaut pas une présence vivante.
    pub vivant: bool,
    /// Dernière valeur de présence ÉCRITE au journal. Locale au processus (elle
    /// n'est pas persistée) : elle ne sert qu'à ne pas réécrire la même chose
    /// toutes les 25 s.
    journalise_ms: i64,
}

impl Client {
    /// Un client complet, pour les TESTS d'autres modules — et seulement eux.
    ///
    /// `journalise_ms` est privé à dessein (c'est un détail du battement), mais la
    /// passe de la flotte (`crate::machines`) doit pouvoir éprouver son agrégation
    /// sans registre ni journal. Exposer un constructeur `#[cfg(test)]` vaut mieux
    /// que rendre le champ public : la garantie de l'encapsulation ne se desserre
    /// pas pour un test, et ce constructeur ne peut pas exister dans un binaire
    /// livré.
    #[cfg(test)]
    pub fn pour_test(
        id: &str,
        compte: &str,
        machine: &str,
        capacites: &[&str],
        attache_ms: i64,
        derniere_vue_ms: i64,
        vivant: bool,
    ) -> Self {
        Client {
            id: id.to_string(),
            compte: compte.to_string(),
            machine: machine.to_string(),
            capacites: capacites.iter().map(|s| s.to_string()).collect(),
            // Le constructeur de test ne prend qu'une liste de capacités : il
            // décrit donc une annonce PARTIELLE, et le dit — un fixture ne doit
            // pas se faire passer pour une annonce complète.
            capacites_refusees: Vec::new(),
            limites: None,
            annonce_manquante: vec!["capacites_refusees".to_string(), "limites".to_string()],
            attache_ms,
            derniere_vue_ms,
            vivant,
            journalise_ms: 0,
        }
    }

    /// L'annonce de cette machine est-elle arrivée ENTIÈRE ?
    ///
    /// Faux ne veut pas dire « cette machine ne refuse rien » : ça veut dire
    /// qu'on ne le SAIT PAS — le corps d'attachement ne portait pas la moitié qui
    /// le dit. Deux choses différentes, et les confondre est exactement le défaut
    /// qu'une liste vide raconte quand personne ne la dénonce.
    #[must_use]
    pub fn annonce_complete(&self) -> bool {
        self.annonce_manquante.is_empty()
    }

    #[must_use]
    pub fn en_json(&self) -> serde_json::Value {
        // Les NOMS des capacités refusées, à côté de `capacites` : la question
        // « que sait faire cette machine ? » a maintenant sa jumelle — « que ne
        // fait-elle pas ? » — et elle se lit aussi facilement. Le détail motivé
        // reste dans `capacites_refusees`.
        let refusees: Vec<&str> = self
            .capacites_refusees
            .iter()
            .filter_map(|r| r.get("nom").and_then(|n| n.as_str()))
            .collect();
        let annonce = serde_json::json!({
            "complete": self.annonce_complete(),
            "manquante": self.annonce_manquante,
            "reponse": self.phrase_annonce(&refusees),
        });
        serde_json::json!({
            "id": self.id,
            "machine": self.machine,
            "capacites": self.capacites,
            "capacites_refusees": self.capacites_refusees,
            "refusees": refusees,
            "limites": self.limites,
            "annonce": annonce,
            "attache_ms": self.attache_ms,
            "derniere_vue_ms": self.derniere_vue_ms,
            "silence_ms": crate::activation::now_ms() - self.derniere_vue_ms,
            "vivant": self.vivant,
        })
    }

    /// L'annonce en une phrase — ce qu'un lecteur (humain ou modèle) comprend sans
    /// dépouiller les tableaux, et qui DIT l'incomplétude quand il y en a une.
    fn phrase_annonce(&self, refusees: &[&str]) -> String {
        if !self.annonce_complete() {
            return format!(
                "annonce INCOMPLÈTE pour « {} » : le corps d'attachement ne portait pas {} — \
                 cette machine ne dit donc pas tout ce qu'elle refuse, et le moteur ne peut pas \
                 l'inventer à sa place",
                self.machine,
                self.annonce_manquante.join(", ")
            );
        }
        if refusees.is_empty() {
            format!(
                "« {} » annonce {} et ne refuse rien",
                self.machine,
                self.capacites.join(", ")
            )
        } else {
            format!(
                "« {} » annonce {} ; elle REFUSE {}",
                self.machine,
                self.capacites.join(", "),
                refusees.join(", ")
            )
        }
    }

    /// La ligne d'attache : tout ce qu'il faut pour retrouver la machine après
    /// un redémarrage, et rien de plus. Aucun jeton, aucun secret — un journal
    /// est fait pour être lu.
    ///
    /// ── ON N'ÉCRIT QUE CE QUI A ÉTÉ REÇU ────────────────────────────────────
    /// Écrire `capacites_refusees: []` pour une annonce qui ne la portait pas
    /// ferait dire au journal « cette machine ne refuse rien » — un mensonge, et
    /// exactement celui qu'un tableau vide raconte si personne ne le dénonce.
    /// Les moitiés absentes sont donc ABSENTES de la ligne, et `annonce_manquante`
    /// les nomme.
    fn ligne_attache(&self) -> serde_json::Value {
        let mut l = serde_json::json!({
            "kind": "attache",
            "id": self.id,
            "compte": self.compte,
            "machine": self.machine,
            "capacites": self.capacites,
            "attache_ms": self.attache_ms,
            "vue_ms": self.derniere_vue_ms,
        });
        if !self
            .annonce_manquante
            .iter()
            .any(|m| m == "capacites_refusees")
        {
            l["capacites_refusees"] = serde_json::json!(self.capacites_refusees);
        }
        if let Some(lim) = &self.limites {
            l["limites"] = lim.clone();
        }
        if !self.annonce_manquante.is_empty() {
            l["annonce_manquante"] = serde_json::json!(self.annonce_manquante);
        }
        l
    }

    fn depuis_ligne(v: &serde_json::Value) -> Option<Self> {
        let s = |k: &str| {
            v.get(k)
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string()
        };
        let id = s("id");
        let compte = s("compte");
        // Une ligne sans identifiant ou sans compte ne désigne rien : un client
        // qui n'appartient à personne ne doit pas naître d'un journal corrompu.
        if id.is_empty() || compte.is_empty() {
            return None;
        }
        let capacites = v
            .get("capacites")
            .and_then(|c| c.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let capacites_refusees: Vec<serde_json::Value> = v
            .get("capacites_refusees")
            .and_then(|c| c.as_array())
            .map(|a| a.iter().filter(|x| x.is_object()).cloned().collect())
            .unwrap_or_default();
        let limites = v.get("limites").filter(|l| l.is_object()).cloned();
        // `annonce_manquante` est lu s'il est écrit. Sinon — ligne d'un journal
        // antérieur, qui n'a jamais su garder ces moitiés — on le DÉDUIT de
        // l'absence des clés : « je ne l'ai pas » et « il n'y en avait pas » ne
        // sont pas la même chose, et c'est la première qui est vraie ici.
        let annonce_manquante: Vec<String> =
            match v.get("annonce_manquante").and_then(|m| m.as_array()) {
                Some(a) => a
                    .iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect(),
                None => {
                    let mut m = Vec::new();
                    if v.get("capacites_refusees").is_none() {
                        m.push("capacites_refusees".to_string());
                    }
                    if limites.is_none() {
                        m.push("limites".to_string());
                    }
                    m
                }
            };
        let t = |k: &str| v.get(k).and_then(serde_json::Value::as_i64).unwrap_or(0);
        let derniere_vue_ms = t("vue_ms");
        Some(Client {
            id,
            compte,
            machine: s("machine"),
            capacites,
            capacites_refusees,
            limites,
            annonce_manquante,
            attache_ms: t("attache_ms"),
            derniere_vue_ms,
            vivant: false,
            journalise_ms: derniere_vue_ms,
        })
    }
}

/// La ligne d'un battement : elle ne porte que ce qui a changé.
fn ligne_battement(client_id: &str, vue_ms: i64) -> serde_json::Value {
    serde_json::json!({ "kind": "battement", "id": client_id, "vue_ms": vue_ms })
}

fn ligne_detache(client_id: &str) -> serde_json::Value {
    serde_json::json!({ "kind": "detache", "id": client_id })
}

#[derive(Default)]
struct Etat {
    /// TOUTES les machines connues — vivantes ou non. Une machine muette n'est
    /// plus effacée : elle redevient « connue mais pas joignable ».
    clients: HashMap<String, Client>,
    /// Les ordres en attente, par client. Seuls les clients VIVANTS en ont une.
    files: HashMap<String, VecDeque<Ordre>>,
    /// Les résultats déposés, par appel.
    resultats: HashMap<String, serde_json::Value>,
    compteur: u64,
}

struct Registre {
    etat: Mutex<Etat>,
    /// Un ordre est arrivé pour un client, ou un résultat pour un appel.
    signal: Condvar,
    /// Le répertoire des journaux, porté PAR le registre et non relu dans
    /// l'environnement à chaque écriture : deux registres — un par processus,
    /// un par test — ne doivent jamais pouvoir écrire dans le même fichier.
    dir: PathBuf,
}

fn registre() -> &'static Registre {
    static R: OnceLock<Registre> = OnceLock::new();
    R.get_or_init(|| Registre::charger(&dir()))
}

/// Le répertoire du registre (le même cache que l'activation et la mémoire).
fn dir() -> PathBuf {
    std::env::var_os("OBRAIN_VIZ_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/obrain-viz-cache"))
        .join("machines")
}

/// Un identifiant lisible et unique dans ce processus.
///
/// Volontairement PAS un secret : voir la décision 2 en tête de module — ce qui
/// autorise est le compte du jeton, jamais la connaissance d'un identifiant.
///
/// L'identifiant nomme une INCARNATION d'une machine, et le numéro de processus
/// en fait partie : sans lui, un registre relu qui se réannonce dans la même
/// milliseconde que sa vie antérieure engendrerait le même identifiant — et le
/// journal porterait un détachement puis une attache sous le MÊME nom, sans
/// qu'on puisse dire laquelle des deux incarnations parle.
fn identifiant(prefixe: &str, compteur: &mut u64) -> String {
    *compteur += 1;
    format!(
        "{prefixe}-{:x}-{:x}-{:x}",
        std::process::id(),
        crate::activation::now_ms(),
        compteur
    )
}

/// Un identifiant qui n'est porté par AUCUNE machine du registre : les
/// identifiants relus au démarrage sont réservés, et les réattribuer à une
/// machine neuve rendrait la seconde indétachable par la première.
fn identifiant_libre(etat: &mut Etat, prefixe: &str) -> String {
    loop {
        let mut compteur = etat.compteur;
        let id = identifiant(prefixe, &mut compteur);
        etat.compteur = compteur;
        if !etat.clients.contains_key(&id) {
            return id;
        }
    }
}

/// Éteint les clients dont on n'a plus de battement depuis [`SILENCE_MAX_MS`],
/// et jette les ordres qui leur étaient destinés (les garder ferait attendre un
/// appelant pour une machine qui ne reviendra pas).
///
/// « Éteindre » et non « oublier » : le registre est durable, donc une machine
/// qui se tait reste CONNUE (`vivant: false`) au lieu d'être effacée. Toucher
/// ce booléen est ce qui empêche `appeler` d'attendre une machine partie.
fn eteindre_les_muets(etat: &mut Etat) {
    let maintenant = crate::activation::now_ms();
    let partis: Vec<String> = etat
        .clients
        .iter()
        .filter(|(_, c)| c.vivant && maintenant - c.derniere_vue_ms > SILENCE_MAX_MS)
        .map(|(id, _)| id.clone())
        .collect();
    for id in partis {
        if let Some(c) = etat.clients.get_mut(&id) {
            c.vivant = false;
        }
        etat.files.remove(&id);
    }
}

/// Rejoue une ligne de journal dans l'état. Fonction PURE — c'est ce qui rend
/// la relecture testable sans processus ni horloge.
///
/// Dernière ligne gagnante, par machine : c'est le seul ordre possible pour un
/// journal append-only dont les lignes sont des ÉTATS (et non des compteurs à
/// agréger).
fn rejouer(clients: &mut HashMap<String, Client>, ligne: &str) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(ligne) else {
        return;
    };
    let Some(id) = v.get("id").and_then(|x| x.as_str()) else {
        return;
    };
    match v.get("kind").and_then(|x| x.as_str()) {
        Some("detache") => {
            clients.remove(id);
        }
        Some("attache") => {
            if let Some(c) = Client::depuis_ligne(&v) {
                clients.insert(id.to_string(), c);
            }
        }
        Some("battement") => {
            // Un battement que nulle attache ne nomme n'INVENTE pas une machine :
            // annoncer est décider, et le journal tronqué ne doit pas peupler la
            // flotte d'inconnues.
            let t = v
                .get("vue_ms")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0);
            if let Some(c) = clients.get_mut(id)
                && t > c.derniere_vue_ms
            {
                c.derniere_vue_ms = t;
                c.journalise_ms = t;
            }
        }
        _ => {}
    }
}

impl Registre {
    /// Le registre reconstruit à partir d'un répertoire de journaux. C'est LE
    /// geste du démarrage : une machine connue redevient connue.
    fn charger(dir: &Path) -> Self {
        let mut etat = Etat::default();
        let mut rejouees = 0u64;
        if let Ok(lectures) = std::fs::read_dir(dir) {
            for e in lectures.flatten() {
                let chemin = e.path();
                // `.jsonl` seulement : un `.tmp` (compactage interrompu) n'est
                // pas un journal, et le lire à moitié serait pire que l'ignorer.
                if chemin.extension().and_then(|x| x.to_str()) != Some("jsonl") {
                    continue;
                }
                let Ok(txt) = std::fs::read_to_string(&chemin) else {
                    continue;
                };
                for ligne in txt.lines().filter(|l| !l.trim().is_empty()) {
                    rejouer(&mut etat.clients, ligne);
                    rejouees += 1;
                }
            }
        }
        // Le compteur ne repart pas de zéro : il passe au-delà de ce que le
        // journal portait. Sans ça, une machine qui se réannonce dans la même
        // milliseconde que sa vie antérieure — c'est le cas d'un redémarrage
        // simulé en test, et d'un `restart` bien plus rapide qu'un humain —
        // recevrait l'identifiant de la morte, et deux incarnations se
        // confondraient dans le journal.
        etat.compteur = rejouees;
        Registre {
            etat: Mutex::new(etat),
            signal: Condvar::new(),
            dir: dir.to_path_buf(),
        }
    }

    fn verrou(&self) -> std::sync::MutexGuard<'_, Etat> {
        self.etat
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// UN FICHIER PAR COMPTE. Le nom est le hachage du compte — un compte peut
    /// porter n'importe quel caractère, un nom de fichier non. C'est donc la
    /// LIGNE qui dit à qui appartient la machine (même choix que le journal de
    /// mémoire, qui porte son `self`), et on n'a jamais à inverser le hachage.
    fn journal(&self, compte: &str) -> PathBuf {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        compte.hash(&mut h);
        self.dir.join(format!("{:016x}.jsonl", h.finish()))
    }

    /// Écrit une ligne au journal du compte — **best-effort assumé**, comme le
    /// journal d'activation et celui de la mémoire : un disque plein ne doit pas
    /// couper le canal vers la machine de l'utilisateur.
    ///
    /// Mais il le DIT. Un registre qui n'arrive pas à écrire perd la flotte au
    /// prochain redémarrage ; l'avaler en silence reproduirait exactement le
    /// défaut que cette persistance corrige.
    fn journaliser(&self, compte: &str, ligne: &serde_json::Value) {
        if let Err(e) = self.ecrire(compte, ligne) {
            tracing::warn!(
                compte = %compte,
                erreur = %e,
                chemin = %self.journal(compte).display(),
                "registre des machines : écriture impossible — la machine reste connue de ce \
                 processus, mais elle ne survivra pas à un redémarrage"
            );
        }
    }

    fn ecrire(&self, compte: &str, ligne: &serde_json::Value) -> std::io::Result<()> {
        let chemin = self.journal(compte);
        if let Some(d) = chemin.parent() {
            std::fs::create_dir_all(d)?;
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&chemin)?;
        use std::io::Write;
        writeln!(f, "{ligne}")?;
        f.flush()
    }

    /// Le journal est append-only, il ne doit pas pour autant croître sans fin.
    /// Au-delà de [`JOURNAL_MAX_OCTETS`], il est réécrit en un ÉTAT : une ligne
    /// d'attache par machine encore connue de ce compte, avec sa dernière
    /// présence. Le fichier neuf prend la place de l'ancien par `rename` —
    /// atomique, donc un arrêt au mauvais moment laisse l'ancien journal intact
    /// plutôt qu'un fichier à moitié écrit.
    fn compacter_si_besoin(&self, compte: &str, clients: &[Client]) {
        let chemin = self.journal(compte);
        let taille = std::fs::metadata(&chemin).map(|m| m.len()).unwrap_or(0);
        if taille <= JOURNAL_MAX_OCTETS {
            return;
        }
        let tmp = chemin.with_extension("jsonl.tmp");
        let contenu: String = clients
            .iter()
            .map(|c| format!("{}\n", c.ligne_attache()))
            .collect();
        let ecriture = std::fs::write(&tmp, contenu).and_then(|()| std::fs::rename(&tmp, &chemin));
        match ecriture {
            Ok(()) => tracing::info!(
                compte = %compte,
                octets_avant = taille,
                machines = clients.len(),
                "registre des machines : journal compacté"
            ),
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                tracing::warn!(compte = %compte, erreur = %e,
                    "registre des machines : compactage impossible — le journal continue de croître");
            }
        }
    }

    /// Les machines de CE compte, vivantes d'abord.
    fn machines(&self, compte: &str) -> Vec<Client> {
        let mut etat = self.verrou();
        eteindre_les_muets(&mut etat);
        let mut v: Vec<Client> = etat
            .clients
            .values()
            .filter(|c| c.compte == compte)
            .cloned()
            .collect();
        // Une machine qui répond passe avant une machine connue mais muette :
        // c'est l'ordre dans lequel on adresse quelqu'un.
        v.sort_by(|a, b| {
            b.vivant
                .cmp(&a.vivant)
                .then_with(|| a.machine.cmp(&b.machine))
        });
        v
    }

    /// TOUTES les machines connues, tous comptes confondus.
    ///
    /// C'est la matière de la passe qui fait de la flotte un FAIT DU GRAPHE
    /// (`crate::machines`). Le registre connaissait déjà les machines et la route
    /// les disait ; mais une IA qui interrogeait le GRAPHE ne trouvait rien —
    /// c'était le dernier échec de la porte d'acceptation.
    ///
    /// Cette lecture ne décide d'AUCUN droit et n'en desserre aucun : la passe qui
    /// l'utilise écrit dans une base ce que le compte porté par chaque machine a
    /// réellement attaché. Le cloisonnement vit dans `clients_du_compte`, pas ici,
    /// et il reste la seule autorisation qui vaille en lecture HTTP.
    fn toutes(&self) -> Vec<Client> {
        let mut etat = self.verrou();
        eteindre_les_muets(&mut etat);
        let mut v: Vec<Client> = etat.clients.values().cloned().collect();
        // Ordre STABLE par nom : deux passes successives écrivent les mêmes
        // nœuds dans le même ordre, donc un journal de graphe reste comparable.
        v.sort_by(|a, b| a.machine.cmp(&b.machine).then_with(|| a.id.cmp(&b.id)));
        v
    }

    /// Un client s'annonce. Rend son identifiant.
    ///
    /// L'annonce est gardée ENTIÈRE : les deux moitiés, plus ce qui n'est pas
    /// arrivé (voir [`Annonce`]). C'est le point où la moitié « ce qu'elle ne fait
    /// pas » cessait d'exister.
    ///
    /// Prend la même chose que l'entrée publique (corps entier ou liste seule) :
    /// un appelant qui n'a qu'un morceau le remet, et ce morceau est DIT.
    fn attacher(&self, compte: &str, machine: &str, annonce: impl AnnonceEntrante) -> String {
        let annonce = annonce.en_annonce();
        let mut etat = self.verrou();
        eteindre_les_muets(&mut etat);
        // ── UNE MACHINE QUI SE RÉANNONCE REMPLACE SON INCARNATION MORTE ─────
        // Sans ça, chaque redémarrage de `selfd` ajouterait un fantôme : au
        // bout d'un mois, la liste d'un compte porterait trente « portable-alice
        // », dont vingt-neuf non joignables — et une liste où l'on ne
        // reconnaît plus sa machine ne sert à rien.
        //
        // On ne touche PAS aux incarnations VIVANTES du même nom : deux
        // connexions vivantes sont deux connexions réelles (deux `selfd` sur la
        // même machine), et les confondre serait décider à la place de
        // l'utilisateur.
        let remplacees: Vec<String> = etat
            .clients
            .values()
            .filter(|c| c.compte == compte && c.machine == machine && !c.vivant)
            .map(|c| c.id.clone())
            .collect();
        for ancien in &remplacees {
            etat.clients.remove(ancien);
            etat.files.remove(ancien);
            self.journaliser(compte, &ligne_detache(ancien));
        }
        let id = identifiant_libre(&mut etat, "cl");
        let maintenant = crate::activation::now_ms();
        let client = Client {
            id: id.clone(),
            compte: compte.to_string(),
            machine: machine.to_string(),
            capacites: annonce.capacites,
            capacites_refusees: annonce.capacites_refusees,
            limites: annonce.limites,
            annonce_manquante: annonce.manquante,
            attache_ms: maintenant,
            derniere_vue_ms: maintenant,
            vivant: true,
            journalise_ms: maintenant,
        };
        etat.clients.insert(id.clone(), client.clone());
        etat.files.insert(id.clone(), VecDeque::new());
        // Journaliser AVANT de rendre l'identifiant : l'appelant ne doit pas
        // repartir avec un identifiant que le prochain redémarrage ignorera.
        self.journaliser(compte, &client.ligne_attache());
        let du_compte: Vec<Client> = etat
            .clients
            .values()
            .filter(|c| c.compte == compte)
            .cloned()
            .collect();
        self.compacter_si_besoin(compte, &du_compte);
        id
    }

    /// Le client se détache proprement (sa machine s'éteint, l'utilisateur arrête).
    fn detacher(&self, compte: &str, client_id: &str) -> bool {
        let mut etat = self.verrou();
        match etat.clients.get(client_id) {
            Some(c) if c.compte == compte => {
                etat.clients.remove(client_id);
                etat.files.remove(client_id);
                self.journaliser(compte, &ligne_detache(client_id));
                true
            }
            _ => false,
        }
    }

    /// Un battement : la machine s'est rappelée à nous. Elle est vivante — même
    /// si le journal ne la connaissait que morte — et sa présence n'est écrite
    /// que si la valeur durable a bougé d'au moins [`BATTEMENT_DURABLE_MS`].
    ///
    /// L'identité est vérifiée AVANT toute mutation : un client qui n'est pas à
    /// ce compte ne change rien, pas même son horodatage.
    fn battre(&self, etat: &mut Etat, compte: &str, client_id: &str) -> Result<(), String> {
        let maintenant = crate::activation::now_ms();
        let a_ecrire = {
            let Some(c) = etat.clients.get_mut(client_id) else {
                return Err("client inconnu : il faut se rattacher".into());
            };
            if c.compte != compte {
                return Err("ce client est attaché à un autre compte".into());
            }
            c.vivant = true;
            c.derniere_vue_ms = maintenant;
            let a_ecrire = maintenant - c.journalise_ms >= BATTEMENT_DURABLE_MS;
            if a_ecrire {
                c.journalise_ms = maintenant;
            }
            a_ecrire
        };
        etat.files.entry(client_id.to_string()).or_default();
        if a_ecrire {
            self.journaliser(compte, &ligne_battement(client_id, maintenant));
        }
        Ok(())
    }

    /// Le prochain ordre pour ce client, en attendant AU PLUS `attente`.
    ///
    /// Rend `None` au délai : le client rappelle alors, et cette respiration sert
    /// aussi de battement de cœur (c'est elle qui tient `derniere_vue_ms` à jour).
    /// `Err` si le client est inconnu ou n'est pas à ce compte.
    fn prochain_ordre(
        &self,
        compte: &str,
        client_id: &str,
        attente: Duration,
    ) -> Result<Option<Ordre>, String> {
        {
            let mut etat = self.verrou();
            self.battre(&mut etat, compte, client_id)?;
            if let Some(o) = etat.files.get_mut(client_id).and_then(VecDeque::pop_front) {
                return Ok(Some(o));
            }
        }
        // Attente BORNÉE. `wait_timeout` rend le verrou pendant l'attente : les
        // autres fils continuent d'être servis.
        let (mut etat, _) = self
            .signal
            .wait_timeout_while(self.verrou(), attente, |e| {
                e.files.get(client_id).is_none_or(VecDeque::is_empty)
                    && e.clients.contains_key(client_id)
            })
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Le client a tenu la connexion : c'est une présence, même sans ordre.
        // (`Err` ici veut dire « détaché ou volé pendant l'attente » : on le
        // laisse filer, il n'y a plus d'ordre à lui rendre.)
        let _ = self.battre(&mut etat, compte, client_id);
        Ok(etat.files.get_mut(client_id).and_then(VecDeque::pop_front))
    }

    /// Le client rend son verdict pour un appel.
    fn deposer_resultat(
        &self,
        compte: &str,
        client_id: &str,
        appel_id: &str,
        resultat: serde_json::Value,
    ) -> Result<(), String> {
        let mut etat = self.verrou();
        self.battre(&mut etat, compte, client_id)?;
        etat.resultats.insert(appel_id.to_string(), resultat);
        drop(etat);
        self.signal.notify_all();
        Ok(())
    }

    /// Demande une action à un client, et attend son verdict au plus `delai`.
    ///
    /// C'est le seul point d'entrée « côté moteur » : il vérifie d'abord que le
    /// client APPARTIENT au compte appelant.
    fn appeler(
        &self,
        compte: &str,
        client_id: &str,
        outil: &str,
        arguments: serde_json::Value,
        delai: Duration,
    ) -> Result<serde_json::Value, String> {
        let appel_id;
        {
            let mut etat = self.verrou();
            eteindre_les_muets(&mut etat);
            match etat.clients.get(client_id) {
                Some(c) if c.compte != compte => return Err("ce client n'est pas à vous".into()),
                // REFUS MOTIVÉ, et rendu TOUT DE SUITE : attendre le délai
                // complet pour une machine dont on sait qu'elle est muette
                // ferait passer une absence pour une lenteur.
                Some(c) if !c.vivant => {
                    let silence = (crate::activation::now_ms() - c.derniere_vue_ms) / 1000;
                    return Err(format!(
                        "« {} » n'est pas joignable : aucun battement depuis {} s — elle \
                         redevient joignable en redemandant du travail",
                        c.machine, silence
                    ));
                }
                // ── CE QUE LA MACHINE A DIT D'ELLE-MÊME PASSE AVANT L'ORDRE ─
                // Le compte et la présence d'abord (ce sont des droits et des
                // faits), la politique ANNONCÉE ensuite : un ordre qui part pour
                // être refusé après coup est exactement le geste que la doctrine
                // interdit de proposer.
                Some(c) => {
                    if let Some(motif) = refus_annonce(c, outil) {
                        return Err(motif);
                    }
                }
                None => return Err("aucun client de ce nom n'est connecté".into()),
            }
            let mut compteur = etat.compteur;
            appel_id = identifiant("ap", &mut compteur);
            etat.compteur = compteur;
            let ordre = Ordre {
                appel_id: appel_id.clone(),
                outil: outil.to_string(),
                arguments,
                emis_ms: crate::activation::now_ms(),
            };
            etat.files
                .entry(client_id.to_string())
                .or_default()
                .push_back(ordre);
        }
        self.signal.notify_all();

        let etat = self.verrou();
        let (mut etat, expire) = self
            .signal
            .wait_timeout_while(etat, delai, |e| !e.resultats.contains_key(&appel_id))
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match etat.resultats.remove(&appel_id) {
            Some(v) => Ok(v),
            None if expire.timed_out() => {
                // On retire l'ordre s'il n'a pas encore été pris : sans ça, une
                // machine qui revient exécuterait une demande que son auteur a
                // cessé d'attendre — une action sans destinataire.
                if let Some(f) = etat.files.get_mut(client_id) {
                    f.retain(|o| o.appel_id != appel_id);
                }
                Err(format!(
                    "le client n'a pas répondu en {} s",
                    delai.as_secs()
                ))
            }
            None => Err("appel interrompu".into()),
        }
    }
}

/// Le VOCABULAIRE des capacités qu'un nœud annonce — les cinq noms que la porte
/// d'acceptation et l'interface emploient déjà.
///
/// Il sert à UNE chose : savoir qu'un nom absent de l'annonce est une capacité
/// que la machine a mesurée et n'a pas ouverte. C'est le vocabulaire des
/// CAPACITÉS, pas la table de routes du nœud : `chat`, `caps`, `initialize` et
/// `run` en sont dehors, et le moteur n'a aucun moyen de savoir s'ils existent —
/// il ne les refuse donc jamais au nom d'une ignorance (voir [`refus_annonce`]).
const VOCABULAIRE_CAPACITES: [&str; 5] = ["read", "list", "write", "status", "exec"];

/// Pourquoi le moteur refuse CETTE demande **tout de suite**, ou `None` s'il n'a
/// rien à en dire et que l'ordre doit partir.
///
/// ── CE QUE CE REFUS RÉPARE ──────────────────────────────────────────────────
/// Le repli générique (« le client n'a pas répondu en 30 s ») faisait porter à la
/// machine un refus qu'elle avait ANNONCÉ d'avance, avec son motif, et faisait
/// attendre trente secondes pour une réponse qu'on avait déjà. Le moteur ne
/// masque plus une capacité absente : il la nomme, avec ce que la machine a dit.
fn refus_annonce(c: &Client, outil: &str) -> Option<String> {
    // 1. La machine a dit qu'elle REFUSAIT : son motif part tel quel. C'est le
    //    seul refus qui soit entièrement fondé — il vient du nœud, pas de nous.
    if let Some(r) = c
        .capacites_refusees
        .iter()
        .find(|r| r.get("nom").and_then(|n| n.as_str()) == Some(outil))
    {
        let motif = r
            .get("motif")
            .and_then(|m| m.as_str())
            .unwrap_or("refus sans motif");
        return Some(format!(
            "« {} » a ANNONCÉ qu'elle refusait « {outil} » : {motif}",
            c.machine
        ));
    }
    // 2. Ni exercable, ni refusé nommément, mais une capacité quand même : la
    //    politique de la machine l'a mesurée et ne l'a pas ouverte. On ne
    //    fabrique pas son motif — on dit ce qu'on sait, et on nomme ce qu'elle
    //    annonce, pour que la demande se corrige.
    //
    //    `capacites` VIDE fait exception, et c'est délibéré : une machine qui
    //    n'annonce rien n'a pas mesuré « rien » — elle s'est tue. Refuser à sa
    //    place serait décider au nom d'une mesure qui n'existe pas.
    if VOCABULAIRE_CAPACITES.contains(&outil)
        && !c.capacites.is_empty()
        && !c.capacites.iter().any(|x| x == outil)
    {
        let suite = if c
            .annonce_manquante
            .iter()
            .any(|m| m == "capacites_refusees")
        {
            " ; le motif de ce refus n'est PAS arrivé avec son annonce, ce moteur ne peut donc \
             pas le donner — rechargez le nœud, ou demandez-lui `status`"
        } else {
            ""
        };
        return Some(format!(
            "« {} » n'annonce PAS « {outil} » : elle annonce {}{suite}",
            c.machine,
            c.capacites.join(", ")
        ));
    }
    None
}

/// Un client s'annonce. Rend son identifiant.
///
/// Accepte le corps d'attachement entier (`&Value` — le chemin qui porte les
/// refus et les limites) ou la seule liste de capacités (l'ancien chemin). Ce
/// qu'il n'a pas reçu est DIT dans l'annonce, jamais présumé.
pub fn attacher(compte: &str, machine: &str, annonce: impl AnnonceEntrante) -> String {
    registre().attacher(compte, machine, annonce.en_annonce())
}

/// Le client se détache proprement (sa machine s'éteint, l'utilisateur arrête).
pub fn detacher(compte: &str, client_id: &str) -> bool {
    registre().detacher(compte, client_id)
}

/// Les clients d'un compte — et d'aucun autre.
///
/// Rend aussi les machines CONNUES mais non vivantes (`vivant: false`), parce
/// qu'un redémarrage ne doit pas effacer la flotte de l'écran : c'est à
/// l'appelant de ne pas les annoncer comme joignables, et le champ `vivant` est
/// là pour ça.
pub fn clients_du_compte(compte: &str) -> Vec<Client> {
    registre().machines(compte)
}

/// TOUTES les machines connues, tous comptes confondus — voir `Registre::toutes`.
///
/// Réservée à la PASSE qui écrit la flotte dans une base (`crate::machines`), et
/// jamais à une route HTTP : une route qui l'utiliserait sans vérifier le compte
/// rendrait à chacun la flotte de tout le monde, ce que la décision du
/// 2026-09-16 interdit (voir `docs/ACCEPTATION-FLOTTE-2026-09-16.md` §9.2).
pub fn toutes_les_machines() -> Vec<Client> {
    registre().toutes()
}

/// Le prochain ordre pour ce client, en attendant AU PLUS `attente`.
pub fn prochain_ordre(
    compte: &str,
    client_id: &str,
    attente: Duration,
) -> Result<Option<Ordre>, String> {
    registre().prochain_ordre(compte, client_id, attente)
}

/// Le client rend son verdict pour un appel.
pub fn deposer_resultat(
    compte: &str,
    client_id: &str,
    appel_id: &str,
    resultat: serde_json::Value,
) -> Result<(), String> {
    registre().deposer_resultat(compte, client_id, appel_id, resultat)
}

/// Demande une action à un client, et attend son verdict au plus `delai`.
pub fn appeler(
    compte: &str,
    client_id: &str,
    outil: &str,
    arguments: serde_json::Value,
    delai: Duration,
) -> Result<serde_json::Value, String> {
    registre().appeler(compte, client_id, outil, arguments, delai)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// Un registre ISOLÉ sur son propre répertoire.
    ///
    /// Les tests de ce module écrivaient autrefois dans le registre global, donc
    /// dans le cache partagé : c'était sans conséquence tant que rien n'était
    /// durable. Depuis que le registre RELIT ce qu'il a écrit, deux exécutions
    /// successives des mêmes tests se verraient l'une l'autre — un test qui
    /// dépend de l'histoire de la machine n'est plus un test.
    fn registre_isole(td: &tempfile::TempDir) -> Registre {
        Registre::charger(td.path())
    }

    /// La relecture depuis le même répertoire : c'est la simulation du
    /// redémarrage du moteur, sans processus ni horloge.
    fn relire(td: &tempfile::TempDir) -> Registre {
        Registre::charger(td.path())
    }

    #[test]
    fn un_compte_ne_voit_jamais_le_client_d_un_autre() {
        let td = tempfile::tempdir().unwrap();
        let r = registre_isole(&td);
        let a = r.attacher("alice", "portable-alice", vec!["read".into()]);
        let _b = r.attacher("bob", "tour-bob", vec!["read".into()]);

        let vus: Vec<String> = r.machines("alice").into_iter().map(|c| c.machine).collect();
        assert_eq!(vus, vec!["portable-alice"], "alice ne voit que sa machine");

        // Et même en CONNAISSANT l'identifiant, bob ne peut pas s'en servir.
        let refus = r.appeler(
            "bob",
            &a,
            "read",
            serde_json::json!({}),
            Duration::from_millis(50),
        );
        assert!(
            refus.is_err(),
            "connaître un identifiant ne donne aucun droit"
        );
        assert!(refus.unwrap_err().contains("pas à vous"));

        assert!(
            r.prochain_ordre("bob", &a, Duration::from_millis(10))
                .is_err()
        );
        assert!(
            !r.detacher("bob", &a),
            "bob ne détache pas le client d'alice"
        );
        assert!(r.detacher("alice", &a));
    }

    #[test]
    fn un_appel_trouve_son_client_et_revient() {
        let td = tempfile::tempdir().unwrap();
        let r = std::sync::Arc::new(registre_isole(&td));
        let id = r.attacher("carol", "poste-carol", vec!["write".into()]);
        let (rc, idc) = (std::sync::Arc::clone(&r), id.clone());
        let fil = std::thread::spawn(move || {
            let o = rc
                .prochain_ordre("carol", &idc, Duration::from_secs(5))
                .expect("client connu")
                .expect("un ordre doit arriver");
            assert_eq!(o.outil, "write");
            rc.deposer_resultat("carol", &idc, &o.appel_id, serde_json::json!({"ok": true}))
                .expect("dépôt");
        });
        let v = r
            .appeler(
                "carol",
                &id,
                "write",
                serde_json::json!({"chemin": "note.txt"}),
                Duration::from_secs(5),
            )
            .expect("le verdict revient");
        assert_eq!(v["ok"], serde_json::json!(true));
        fil.join().unwrap();
        r.detacher("carol", &id);
    }

    #[test]
    fn sans_reponse_l_appel_rend_la_main_et_ne_laisse_pas_d_ordre_orphelin() {
        let td = tempfile::tempdir().unwrap();
        let r = registre_isole(&td);
        let id = r.attacher("dan", "poste-dan", vec![]);
        let debut = Instant::now();
        let res = r.appeler(
            "dan",
            &id,
            "exec",
            serde_json::json!({}),
            Duration::from_millis(300),
        );
        assert!(res.is_err(), "sans client à l'écoute, l'appel expire");
        assert!(
            debut.elapsed() >= Duration::from_millis(250),
            "il a bien attendu"
        );
        assert!(
            debut.elapsed() < Duration::from_secs(3),
            "et pas indéfiniment"
        );
        // L'ordre abandonné ne doit plus attendre personne.
        let reste = r
            .prochain_ordre("dan", &id, Duration::from_millis(50))
            .expect("client connu");
        assert!(
            reste.is_none(),
            "un appel expiré ne laisse pas d'ordre derrière lui"
        );
        r.detacher("dan", &id);
    }

    #[test]
    fn l_attente_sans_ordre_rend_none_et_sert_de_battement() {
        let td = tempfile::tempdir().unwrap();
        let r = registre_isole(&td);
        let id = r.attacher("erin", "poste-erin", vec![]);
        let avant = r.machines("erin")[0].derniere_vue_ms;
        std::thread::sleep(Duration::from_millis(20));
        let res = r
            .prochain_ordre("erin", &id, Duration::from_millis(100))
            .expect("client connu");
        assert!(
            res.is_none(),
            "aucun ordre : on rend None, le client rappellera"
        );
        let apres = r.machines("erin")[0].derniere_vue_ms;
        assert!(apres > avant, "l'attente elle-même vaut battement de cœur");
        r.detacher("erin", &id);
    }

    /// Ce test affirmait « un client muet est OUBLIÉ ». C'était vrai tant que le
    /// registre ne durait pas plus que le processus ; ça ne l'est plus. La
    /// vérité d'aujourd'hui, et ce qu'il faut protéger, c'est qu'un muet n'est
    /// plus ANNONCÉ comme joignable — sans être effacé pour autant.
    #[test]
    fn un_client_muet_reste_connu_mais_n_est_plus_joignable() {
        let td = tempfile::tempdir().unwrap();
        let r = registre_isole(&td);
        let id = r.attacher("frank", "poste-frank", vec![]);
        {
            let mut etat = r.verrou();
            etat.clients.get_mut(&id).unwrap().derniere_vue_ms =
                crate::activation::now_ms() - SILENCE_MAX_MS - 1;
        }
        let vu = r.machines("frank");
        assert_eq!(vu.len(), 1, "la machine reste CONNUE (le journal la porte)");
        assert!(!vu[0].vivant, "mais elle n'est pas annoncée comme vivante");

        let debut = Instant::now();
        let refus = r.appeler(
            "frank",
            &id,
            "read",
            serde_json::json!({}),
            Duration::from_millis(500),
        );
        assert!(refus.is_err(), "et on n'attend pas une machine partie");
        assert!(
            refus.unwrap_err().contains("pas joignable"),
            "le refus dit POURQUOI, au lieu d'un délai qui ressemble à une panne"
        );
        assert!(
            debut.elapsed() < Duration::from_millis(100),
            "il refuse tout de suite"
        );
    }

    #[test]
    fn un_journal_relu_retrouve_les_machines_et_leur_proprietaire() {
        let td = tempfile::tempdir().unwrap();
        let id_alice;
        let attache_ms;
        {
            let r = registre_isole(&td);
            id_alice = r.attacher(
                "alice",
                "portable-alice",
                vec!["read".into(), "exec".into()],
            );
            r.attacher("bob", "tour-bob", vec!["read".into()]);
            attache_ms = r.machines("alice")[0].attache_ms;
            // Le journal existe, et il nomme la machine : c'est la preuve que
            // « attacher » ÉCRIT quelque chose quelque part.
            let chemin = r.journal("alice");
            let txt = std::fs::read_to_string(&chemin).expect("journal écrit");
            assert!(
                txt.contains("portable-alice"),
                "le journal nomme la machine : {txt}"
            );
            assert!(txt.contains("alice"), "et son compte propriétaire");
        }

        // « REDÉMARRAGE » : un registre neuf, le même répertoire.
        let r2 = relire(&td);
        let alice = r2.machines("alice");
        assert_eq!(alice.len(), 1, "la machine d'alice est retrouvée");
        assert_eq!(alice[0].machine, "portable-alice");
        assert_eq!(
            alice[0].id, id_alice,
            "avec le MÊME identifiant : rien n'est réattribué"
        );
        assert_eq!(
            alice[0].attache_ms, attache_ms,
            "et sa première attache est datée"
        );
        assert_eq!(
            alice[0].capacites,
            vec!["read".to_string(), "exec".to_string()]
        );
        assert!(
            !alice[0].vivant,
            "relue ne veut pas dire vivante : personne n'a battu depuis ce processus"
        );
        assert_eq!(
            r2.machines("bob").len(),
            1,
            "et le compte voisin n'a rien perdu au passage"
        );
    }

    #[test]
    fn le_cloisonnement_survit_a_la_relecture_du_registre() {
        let td = tempfile::tempdir().unwrap();
        let (id_alice, id_bob);
        {
            let r = registre_isole(&td);
            id_alice = r.attacher("alice", "portable-alice", vec![]);
            id_bob = r.attacher("bob", "tour-bob", vec![]);
        }
        let r = relire(&td);

        let vus_alice: Vec<String> = r.machines("alice").into_iter().map(|c| c.machine).collect();
        let vus_bob: Vec<String> = r.machines("bob").into_iter().map(|c| c.machine).collect();
        assert_eq!(
            vus_alice,
            vec!["portable-alice"],
            "alice ne relit que la sienne"
        );
        assert_eq!(vus_bob, vec!["tour-bob"], "bob ne relit que la sienne");

        // Et un identifiant relu n'ouvre rien à un autre compte.
        let refus = r.appeler(
            "bob",
            &id_alice,
            "read",
            serde_json::json!({}),
            Duration::from_millis(50),
        );
        assert!(refus.is_err());
        assert!(refus.unwrap_err().contains("pas à vous"));
        // Deux machines, deux journaux : la frontière est aussi sur le disque.
        assert_ne!(
            r.journal("alice"),
            r.journal("bob"),
            "un fichier par compte"
        );
        assert!(
            r.appeler(
                "bob",
                &id_bob,
                "read",
                serde_json::json!({}),
                Duration::from_millis(50)
            )
            .is_err(),
            "et bob ne peut pas davantage attendre la sienne, qui ne bat pas"
        );
    }

    #[test]
    fn une_machine_sans_battement_recent_n_est_pas_annoncee_vivante() {
        let td = tempfile::tempdir().unwrap();
        // Un journal d'hier : la machine s'est annoncée il y a une heure.
        let vieux = crate::activation::now_ms() - 3_600_000;
        let ligne = serde_json::json!({
            "kind": "attache", "id": "cl-vielli-1", "compte": "alice",
            "machine": "portable-alice", "capacites": ["read"],
            "attache_ms": vieux, "vue_ms": vieux,
        });
        std::fs::write(td.path().join("journal.jsonl"), format!("{ligne}\n")).unwrap();

        let r = relire(&td);
        let vus = r.machines("alice");
        assert_eq!(vus.len(), 1, "la machine est connue");
        assert!(!vus[0].vivant, "mais elle n'est PAS annoncée vivante");
        assert!(
            vus[0].en_json()["silence_ms"].as_i64().unwrap() > SILENCE_MAX_MS,
            "et son silence est dit en clair : {}",
            vus[0].en_json()
        );
        let refus = r.appeler(
            "alice",
            "cl-vielli-1",
            "read",
            serde_json::json!({}),
            Duration::from_secs(5),
        );
        assert!(refus.is_err(), "on ne lui envoie pas d'ordre");
        assert!(refus.unwrap_err().contains("pas joignable"));
    }

    #[test]
    fn une_machine_connue_qui_redemande_du_travail_redevient_vivante() {
        let td = tempfile::tempdir().unwrap();
        let id;
        {
            let r = registre_isole(&td);
            id = r.attacher("alice", "portable-alice", vec![]);
        }
        // Nouveau processus : la machine est connue, pas vivante.
        let r = relire(&td);
        assert!(!r.machines("alice")[0].vivant);
        // Elle redemande du travail : c'est le seul geste qui prouve qu'elle est là.
        let rien = r
            .prochain_ordre("alice", &id, Duration::from_millis(20))
            .expect("connue");
        assert!(rien.is_none(), "aucun ordre en attente");
        assert!(
            r.machines("alice")[0].vivant,
            "elle redevient vivante en se rappelant à nous"
        );
    }

    #[test]
    fn un_battement_orphelin_n_invente_pas_une_machine() {
        let td = tempfile::tempdir().unwrap();
        // Aucune attache : seulement des battements (journal tronqué, compactage
        // interrompu). « Annoncer est décider » : on n'en fait pas des machines.
        let l = serde_json::json!({"kind": "battement", "id": "cl-fantome-1", "vue_ms": 1});
        std::fs::write(td.path().join("journal.jsonl"), format!("{l}\n")).unwrap();
        let r = relire(&td);
        assert!(r.machines("alice").is_empty());
        assert!(r.machines("").is_empty());
    }

    #[test]
    fn le_detachement_est_durable() {
        let td = tempfile::tempdir().unwrap();
        let id;
        {
            let r = registre_isole(&td);
            id = r.attacher("alice", "portable-alice", vec![]);
            assert!(r.detacher("alice", &id));
        }
        let r = relire(&td);
        assert!(
            r.machines("alice").is_empty(),
            "une machine détachée ne ressuscite pas"
        );
    }

    #[test]
    fn une_machine_qui_se_reannonce_ne_laisse_pas_de_fantome() {
        let td = tempfile::tempdir().unwrap();
        let premier;
        {
            let r = registre_isole(&td);
            premier = r.attacher("alice", "portable-alice", vec!["read".into()]);
        }
        // Le moteur redémarre, puis `selfd` se réattache sous le MÊME nom.
        let r = relire(&td);
        assert_eq!(r.machines("alice").len(), 1, "elle est connue, non vivante");
        let second = r.attacher(
            "alice",
            "portable-alice",
            vec!["read".into(), "exec".into()],
        );

        let vus = r.machines("alice");
        assert_eq!(vus.len(), 1, "un fantôme ne s'ajoute pas : {vus:?}");
        assert_eq!(vus[0].id, second);
        assert!(vus[0].vivant, "et c'est bien la nouvelle incarnation");
        assert_eq!(
            vus[0].capacites,
            vec!["read".to_string(), "exec".to_string()]
        );
        assert_ne!(
            vus[0].id, premier,
            "un nouvel identifiant, l'ancien est mort"
        );

        // Le fantôme est retiré DURABLEMENT, pas seulement de l'affichage.
        let r3 = relire(&td);
        assert_eq!(
            r3.machines("alice").len(),
            1,
            "et il ne revient pas du journal"
        );
        assert_eq!(r3.machines("alice")[0].id, second);
    }

    #[test]
    fn deux_connexions_vivantes_du_meme_nom_ne_sont_pas_fondues() {
        let td = tempfile::tempdir().unwrap();
        let r = registre_isole(&td);
        let a = r.attacher("alice", "portable-alice", vec![]);
        let b = r.attacher("alice", "portable-alice", vec![]);
        assert_ne!(a, b);
        assert_eq!(
            r.machines("alice").len(),
            2,
            "deux `selfd` vivants sur la même machine sont deux connexions réelles — \
             les fondre serait décider à la place de l'utilisateur"
        );
    }

    #[test]
    fn le_journal_ne_croit_pas_sans_fin() {
        let td = tempfile::tempdir().unwrap();
        // Un journal d'un compte, gonflé de battements au-delà du seuil.
        let t = crate::activation::now_ms();
        let attache = serde_json::json!({
            "kind": "attache", "id": "cl-long-1", "compte": "alice",
            "machine": "portable-alice", "capacites": [],
            "attache_ms": t, "vue_ms": t,
        });
        let mut txt = format!("{attache}\n");
        for i in 0..6000 {
            let b = serde_json::json!({"kind": "battement", "id": "cl-long-1", "vue_ms": t + i});
            txt.push_str(&format!("{b}\n"));
        }
        // Le chemin RÉEL du journal de ce compte (nom haché) : c'est lui que le
        // compactage surveille, pas un fichier posé à côté.
        let chemin = Registre::charger(td.path()).journal("alice");
        std::fs::write(&chemin, txt).unwrap();
        let avant = std::fs::metadata(&chemin).unwrap().len();
        assert!(
            avant > JOURNAL_MAX_OCTETS,
            "le journal de départ dépasse le seuil : {avant}"
        );

        // Une écriture quelconque déclenche le compactage.
        let r = relire(&td);
        r.attacher("alice", "deuxieme-machine", vec![]);
        let apres = std::fs::metadata(&chemin).unwrap().len();
        assert!(
            apres < JOURNAL_MAX_OCTETS,
            "le journal est retombé : {avant} → {apres}"
        );

        // Et rien n'a été perdu au passage.
        let r2 = relire(&td);
        let machines: Vec<String> = r2
            .machines("alice")
            .into_iter()
            .map(|c| c.machine)
            .collect();
        assert_eq!(
            machines,
            vec!["deuxieme-machine".to_string(), "portable-alice".to_string()],
            "les deux machines survivent au compactage"
        );
        assert_eq!(
            r2.machines("alice")
                .iter()
                .find(|c| c.machine == "portable-alice")
                .unwrap()
                .id,
            "cl-long-1",
            "et la machine compactée garde son identifiant"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // L'ANNONCE COMPLÈTE : les deux moitiés survivent au transport ET au
    // redémarrage. Sans ces tests, la moitié « ce qu'elle ne fait pas » peut
    // disparaître en silence — c'est précisément l'incident qu'on répare.
    // ─────────────────────────────────────────────────────────────────────────

    /// Le corps d'attachement tel que `selfd` l'envoie : les deux moitiés.
    fn corps_annonce() -> serde_json::Value {
        serde_json::json!({
            "machine": "portable-alice",
            "capacites": ["read", "list", "status"],
            "capacites_refusees": [
                {"nom": "write", "exercable": false,
                 "motif": "écriture impossible à la racine « /srv/data » (Permission denied)"},
                {"nom": "exec", "exercable": false,
                 "motif": "exécution NON AUTORISÉE : ce nœud a été démarré sans --allow-exec"},
            ],
            "limites": {
                "racine": "/srv/data",
                "racine_reelle": "/srv/data",
                "delai_ms_par_defaut": 60_000,
                "plafond_octets_par_flux": 262_144,
                "exec": "ferme",
                "version": "0.1.0",
            },
        })
    }

    #[test]
    fn l_annonce_complete_traverse_le_registre_et_survit_au_redemarrage() {
        let td = tempfile::tempdir().unwrap();
        let id;
        {
            let r = registre_isole(&td);
            id = r.attacher("alice", "portable-alice", &corps_annonce());
            let c = &r.machines("alice")[0];
            assert_eq!(
                c.capacites,
                vec!["read".to_string(), "list".into(), "status".into()]
            );
            assert_eq!(
                c.capacites_refusees.len(),
                2,
                "les refus motivés doivent ARRIVER au moteur, pas être jetés : {:?}",
                c.capacites_refusees
            );
            assert!(
                c.capacites_refusees.iter().any(|x| x["nom"] == "write"
                    && x["motif"]
                        .as_str()
                        .unwrap_or_default()
                        .contains("Permission denied")),
                "et avec leur MOTIF — c'est lui qui rend l'échec actionnable"
            );
            assert_eq!(
                c.limites.as_ref().and_then(|l| l["exec"].as_str()),
                Some("ferme"),
                "les limites annoncées doivent arriver telles quelles"
            );
            assert!(
                c.annonce_manquante.is_empty(),
                "le corps portait les deux moitiés"
            );
        }

        // « REDÉMARRAGE » : un registre neuf sur le même répertoire. C'est LA
        // propriété du manque 4 — un redémarrage ne doit pas perdre la moitié
        // « ce qu'elle ne fait pas ».
        let r2 = relire(&td);
        let c = &r2.machines("alice")[0];
        assert_eq!(c.id, id);
        assert_eq!(
            c.capacites,
            vec!["read".to_string(), "list".into(), "status".into()]
        );
        assert_eq!(
            c.capacites_refusees.len(),
            2,
            "les refus SURVIVENT au redémarrage : {:?}",
            c.capacites_refusees
        );
        assert!(
            c.capacites_refusees.iter().any(|x| x["nom"] == "exec"
                && x["motif"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("--allow-exec")),
            "et le motif avec eux : {:?}",
            c.capacites_refusees
        );
        assert_eq!(
            c.limites.as_ref().and_then(|l| l["racine_reelle"].as_str()),
            Some("/srv/data"),
            "les limites aussi : racine réelle, délai, octets, exec, version"
        );
        assert_eq!(
            c.limites
                .as_ref()
                .and_then(|l| l["delai_ms_par_defaut"].as_u64()),
            Some(60_000)
        );
        assert!(
            c.annonce_manquante.is_empty(),
            "et la relecture ne la déclare pas tronquée : {:?}",
            c.annonce_manquante
        );

        // Ce qu'un client de `/api/remote/machines` reçoit, dans la forme qui
        // sert aux deux moitiés de la question.
        let j = c.en_json();
        assert_eq!(
            j["capacites"],
            serde_json::json!(["read", "list", "status"])
        );
        assert_eq!(j["refusees"], serde_json::json!(["write", "exec"]));
        assert_eq!(j["annonce"]["complete"], serde_json::json!(true));
        assert!(
            j["annonce"]["reponse"]
                .as_str()
                .unwrap_or_default()
                .contains("REFUSE"),
            "la réponse doit dire ce qu'elle ne fait pas, en clair : {}",
            j["annonce"]["reponse"]
        );
    }

    #[test]
    fn une_annonce_sans_les_refus_est_dite_incomplete_et_pas_vide() {
        let td = tempfile::tempdir().unwrap();
        let r = registre_isole(&td);
        // L'ancien chemin : la seule liste de capacités. C'est l'état dans lequel
        // le moteur recevait autrefois TOUTES les annonces.
        r.attacher(
            "alice",
            "portable-alice",
            vec!["read".into(), "list".into()],
        );

        let j = r.machines("alice")[0].en_json();
        assert_eq!(
            j["annonce"]["complete"],
            serde_json::json!(false),
            "un corps sans refus n'est PAS une annonce complète : {j}"
        );
        assert_eq!(
            j["annonce"]["manquante"],
            serde_json::json!(["capacites_refusees", "limites"]),
            "ce qui manque est NOMMÉ : {j}"
        );
        assert!(
            j["annonce"]["reponse"]
                .as_str()
                .unwrap_or_default()
                .contains("INCOMPLÈTE"),
            "et dit en clair, pour que « rien de refusé » ne se confonde pas avec « on ne sait pas » : {}",
            j["annonce"]["reponse"]
        );

        // Le pendant, et c'est lui qui rend le test ci-dessus signifiant : un
        // nœud qui envoie `capacites_refusees: []` a une annonce COMPLÈTE. Le
        // vide est une réponse ; l'absence de champ n'en est pas une.
        let td2 = tempfile::tempdir().unwrap();
        let r2 = registre_isole(&td2);
        r2.attacher(
            "bob",
            "tour-bob",
            &serde_json::json!({
                "capacites": ["read", "list", "write", "status", "exec"],
                "capacites_refusees": [],
                "limites": {"racine": "/srv", "exec": "ouvert"},
            }),
        );
        let j2 = r2.machines("bob")[0].en_json();
        assert_eq!(j2["annonce"]["complete"], serde_json::json!(true), "{j2}");
        assert!(
            j2["annonce"]["reponse"]
                .as_str()
                .unwrap_or_default()
                .contains("ne refuse rien"),
            "et la réponse le dit sans détour : {}",
            j2["annonce"]["reponse"]
        );
    }

    #[test]
    fn une_capacite_refusee_est_refusee_tout_de_suite_et_avec_son_motif() {
        let td = tempfile::tempdir().unwrap();
        let r = registre_isole(&td);
        let id = r.attacher("alice", "portable-alice", &corps_annonce());

        // `write` est ANNONCÉ comme refusé : le moteur ne pousse pas l'ordre pour
        // le voir échouer trente secondes plus tard.
        let debut = Instant::now();
        let refus = r.appeler(
            "alice",
            &id,
            "write",
            serde_json::json!({}),
            Duration::from_secs(30),
        );
        let e = refus.expect_err("une capacité annoncée refusée ne part pas en ordre");
        assert!(
            debut.elapsed() < Duration::from_millis(500),
            "le refus est IMMÉDIAT, pas un délai déguisé : {:?}",
            debut.elapsed()
        );
        assert!(e.contains("ANNONCÉ qu'elle refusait"), "{e}");
        assert!(
            e.contains("Permission denied"),
            "le motif de la MACHINE doit remonter tel quel — c'est lui qui se corrige : {e}"
        );

        // Le pendant : une capacité annoncée part bel et bien. Sans lui, le test
        // ci-dessus passerait sur un moteur qui refuse tout.
        let td2 = tempfile::tempdir().unwrap();
        let r2 = std::sync::Arc::new(registre_isole(&td2));
        let id2 = r2.attacher("alice", "portable-alice", &corps_annonce());
        let (rp, idp) = (std::sync::Arc::clone(&r2), id2.clone());
        let fil = std::thread::spawn(move || {
            let o = rp
                .prochain_ordre("alice", &idp, Duration::from_secs(5))
                .expect("client connu")
                .expect("un ordre doit arriver pour une capacité ANNONCÉE");
            assert_eq!(o.outil, "read");
            rp.deposer_resultat("alice", &idp, &o.appel_id, serde_json::json!({"ok": true}))
                .expect("dépôt");
        });
        let v = r2
            .appeler(
                "alice",
                &id2,
                "read",
                serde_json::json!({}),
                Duration::from_secs(5),
            )
            .expect("read est annoncée : l'ordre part et le verdict revient");
        assert_eq!(v["ok"], serde_json::json!(true));
        fil.join().unwrap();
    }

    #[test]
    fn une_capacite_jamais_annoncee_ne_part_pas_en_coupe_non_plus() {
        let td = tempfile::tempdir().unwrap();
        let r = registre_isole(&td);
        // Une annonce PARTIELLE (sans les refus) : c'est exactement l'état dans
        // lequel le moteur ne savait rien du refus de `write`.
        let id = r.attacher(
            "alice",
            "portable-alice",
            vec!["read".into(), "list".into()],
        );

        let debut = Instant::now();
        let refus = r.appeler(
            "alice",
            &id,
            "write",
            serde_json::json!({}),
            Duration::from_secs(30),
        );
        let e = refus.expect_err("write n'est pas annoncée");
        assert!(
            debut.elapsed() < Duration::from_millis(500),
            "{:?}",
            debut.elapsed()
        );
        assert!(e.contains("n'annonce PAS « write »"), "{e}");
        assert!(
            e.contains("read, list"),
            "elle nomme ce qu'elle annonce : {e}"
        );
        assert!(
            e.contains("n'est PAS arrivé avec son annonce"),
            "et elle dit que le MOTIF ne lui est pas parvenu, au lieu de le fabriquer : {e}"
        );

        // ── CE QU'UN SILENCE N'AUTORISE PAS ─────────────────────────────────
        // Une machine qui n'annonce RIEN n'a pas mesuré « rien » : elle s'est tue.
        // Aucun refus n'est fabriqué à sa place — l'ordre part, et c'est le délai
        // qui tranche, comme avant.
        let id_muet = r.attacher("alice", "poste-muet", vec![]);
        let debut = Instant::now();
        let res = r.appeler(
            "alice",
            &id_muet,
            "write",
            serde_json::json!({}),
            Duration::from_millis(300),
        );
        assert!(res.is_err(), "personne n'écoute : l'appel expire");
        assert!(
            debut.elapsed() >= Duration::from_millis(250),
            "l'ordre est bien PARTI (aucun refus inventé sur une annonce vide) : {:?}",
            debut.elapsed()
        );
    }

    #[test]
    fn un_outil_hors_du_vocabulaire_n_est_jamais_refuse_au_nom_d_une_ignorance() {
        let td = tempfile::tempdir().unwrap();
        let r = registre_isole(&td);
        // Le nœud sert aussi `chat`, `caps`, `initialize`, `run` — des routes que
        // sa POLITIQUE ne mesure pas, donc absentes de son annonce. Le moteur ne
        // connaît pas cette table de routes : il ne doit pas la refuser.
        let id = r.attacher("alice", "portable-alice", &corps_annonce());
        let debut = Instant::now();
        let res = r.appeler(
            "alice",
            &id,
            "chat",
            serde_json::json!({}),
            Duration::from_millis(300),
        );
        let e = res.expect_err("personne n'écoute");
        assert!(
            !e.contains("n'annonce PAS"),
            "un refus inventé pour un geste que la machine sait faire serait la faute \
             symétrique : {e}"
        );
        assert!(e.contains("pas répondu"), "l'ordre a bien été poussé : {e}");
        assert!(
            debut.elapsed() >= Duration::from_millis(250),
            "{:?}",
            debut.elapsed()
        );
    }
}
