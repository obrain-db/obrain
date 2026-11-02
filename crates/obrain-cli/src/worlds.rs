//! MONDES — ajouter un dépôt GitHub et le voir apparaître comme une base.
//!
//! Demande : « je devrais pouvoir ajouter un repo depuis github sur mon
//! interface pour voir son monde apparaître ». Le dépôt devient un **monde** au
//! sens du Cosmos : une base obrain indexée, avec sa masse, sa signature de
//! types et sa place dans la toile.
//!
//! Trois décisions, dictées par ce qui existe déjà plutôt que par le goût :
//!
//! 1. **On RÉUTILISE l'indexeur du serveur MCP** (`index_repository`), qui écrit
//!    déjà nœuds et arêtes en bulk dans un store. Le réécrire aurait créé deux
//!    chemins d'indexation divergents — le meilleur moyen d'avoir un monde qui
//!    ne ressemble pas aux autres.
//! 2. **Le travail se fait dans un THREAD** : un clone + une indexation durent
//!    de quelques secondes à plusieurs minutes. La requête HTTP rend la main
//!    immédiatement avec un identifiant de travail, et le dashboard interroge
//!    l'avancement. Un monde ne se crée pas en tenant une connexion ouverte.
//! 3. **La provenance est enregistrée** : d'où vient le dépôt, quand, combien de
//!    nœuds — pour que l'utilisateur sache ce qu'il regarde, et pour que
//!    l'ajout soit reproductible.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result, bail};

/// Racine de travail : clones et stores des mondes ajoutés.
fn racine() -> PathBuf {
    std::env::var_os("OBRAIN_WORLDS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/obrain-worlds"))
}

/// Ce qu'un monde ajouté à chaud doit à son auteur.
///
/// ── POURQUOI LE PROPRIÉTAIRE EST UNE DONNÉE, PAS UN DÉTAIL ──────────────────
/// Un monde ajouté apparaît chez TOUT LE MONDE : il entre dans `enregistres()`,
/// donc dans la toile et dans les presets. Quand une seule personne ajoutait des
/// mondes, ça allait de soi. Dès qu'elles sont plusieurs, « qui a ajouté ça »
/// devient la question à laquelle il faut pouvoir répondre — pour attribuer, pour
/// expliquer un monde inconnu, et pour un jour borner la visibilité. On
/// l'enregistre donc au moment de l'ajout, à côté de la provenance du dépôt
/// (qui, lui, dit d'où vient le CODE ; celui-ci dit d'où vient le MONDE).
#[derive(Clone, Debug)]
pub struct Monde {
    pub label: String,
    pub store: PathBuf,
    /// L'identité de l'ajoutant (`utilisateur@compte`), telle que l'IAM la rend.
    pub proprietaire: Option<String>,
    pub ajoute_ms: Option<i64>,
}

/// État d'un travail d'ajout de monde.
///
/// ── L'ENRICHISSEMENT EST RAPPORTÉ, PAS SUPPOSÉ ──────────────────────────────
///
/// Défaut mesuré le 2026-09-15 : l'ajout d'un monde s'arrêtait à l'indexation.
/// La base arrivait donc **brute** — ni courbure, ni registres de degrés, ni
/// classement de hubs, ni index de texte — et l'écran annonçait « monde ajouté :
/// N nœuds ». C'était vrai, et c'était le mensonge par omission : un monde sans
/// ses signaux n'a pas le relief des autres, et la comparaison entre mondes que
/// tout le reste du produit promet n'a plus de sens. Un utilisateur qui ajoute
/// un dépôt doit obtenir **le même monde** que les presets, pas un squelette.
///
/// Les compteurs ci-dessous existent pour que l'écran dise ce qui a été
/// enrichi, combien d'arêtes portent une courbure, et ce qui a ÉCHOUÉ — un
/// enrichissement partiel silencieux serait le même défaut, déplacé.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Travail {
    pub id: String,
    pub url: String,
    pub label: String,
    /// `clone` | `indexation` | `enrichissement` | `fini` | `erreur`
    pub etat: String,
    pub etape: String,
    pub store: Option<String>,
    pub depot: Option<String>,
    pub debut_ms: i64,
    pub fin_ms: Option<i64>,
    pub erreur: Option<String>,
    pub noeuds: Option<i64>,
    pub aretes: Option<i64>,
    /// Durée de la seule passe d'enrichissement, en millisecondes.
    pub enrich_ms: Option<u64>,
    /// Arêtes VISITÉES par la courbure, et celles dont la valeur a bougé.
    pub aretes_visitees: Option<u64>,
    pub aretes_curvature: Option<u64>,
    /// Courbure maximale |κ| observée : le relief le plus intense du monde.
    pub courbure_max: Option<f64>,
    /// Hubs retenus par le classement (enregistré sur disque), et son verdict.
    pub hubs: Option<usize>,
    pub hubs_refus: Option<String>,
    /// Propriétés textuelles dont l'index de trigrammes a été construit.
    pub trigrammes: Option<Vec<String>>,
    /// Ce qui a échoué, nommé. Vide = enrichissement complet.
    pub enrich_erreurs: Vec<String>,
}

static TRAVAUX: OnceLock<Mutex<HashMap<String, Travail>>> = OnceLock::new();
static MONDES: OnceLock<Mutex<Vec<Monde>>> = OnceLock::new();
/// L'instant (secondes epoch) du registre tel qu'on l'a lu pour la dernière
/// fois. Sert à voir qu'une AUTRE main (un autre processus, un opérateur) a
/// écrit le fichier depuis — et à refusionner au lieu de régner sur du vieux.
static REGISTRE_MTIME: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn mtime_registre() -> u64 {
    std::fs::metadata(registre())
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// FUSIONNE le fichier de registre dans la mémoire : toute entrée du disque
/// dont le store est inconnu de la mémoire y entre ; celles que la mémoire
/// connaît gardent leur état en mémoire (elle est plus à jour sur ce qu'elle a
/// elle-même touché). Réciproque de la persistance : sans cette fusion, deux
/// processus qui partagent le fichier s'effacent mutuellement — perte MESURÉE
/// le 2026-09-15 : 4 mondes posés dans le fichier, effacés par le premier
/// `POST /api/worlds` venu d'un processus à la mémoire plus vieille.
fn fusionner_du_disque(g: &mut Vec<Monde>) {
    let Ok(txt) = std::fs::read_to_string(registre()) else {
        return;
    };
    let Ok(v) = serde_json::from_str::<Vec<serde_json::Value>>(&txt) else {
        return;
    };
    for e in v {
        let (Some(l), Some(p)) = (
            e.get("label").and_then(|x| x.as_str()),
            e.get("store").and_then(|x| x.as_str()),
        ) else {
            continue;
        };
        let path = PathBuf::from(p);
        if est_store(&path) && !g.iter().any(|m| m.store == path) {
            g.push(Monde {
                label: l.to_string(),
                store: path,
                proprietaire: e
                    .get("proprietaire")
                    .and_then(|x| x.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .map(str::to_string),
                ajoute_ms: e.get("ajoute_ms").and_then(serde_json::Value::as_i64),
            });
        }
    }
}

fn travaux() -> &'static Mutex<HashMap<String, Travail>> {
    TRAVAUX.get_or_init(|| Mutex::new(HashMap::new()))
}
fn mondes() -> &'static Mutex<Vec<Monde>> {
    MONDES.get_or_init(|| Mutex::new(Vec::new()))
}

/// Fichier de registre : sans lui, un monde ajouté disparaîtrait de la liste au
/// redémarrage du service — alors que son store, lui, reste sur le disque. Un
/// monde qui s'évapore au reboot n'est pas un monde.
fn registre() -> PathBuf {
    racine().join("mondes.json")
}

/// Mondes ajoutés à chaud, dans l'ordre d'ajout (relus du disque au 1er appel).
///
/// ── LA RELECTURE EST TOLÉRANTE AUX ANCIENS REGISTRES ────────────────────────
/// Un `mondes.json` écrit avant l'ajout du propriétaire n'a pas ce champ : on le
/// relit avec `proprietaire: None` plutôt que de jeter l'entrée. Un monde sans
/// auteur reste un monde — le refuser parce qu'une métadonnée manque ferait
/// disparaître des bases bien réelles du disque, ce qui est pire que de ne pas
/// savoir qui les a ajoutées.
pub fn enregistres() -> Vec<Monde> {
    let Ok(mut g) = mondes().lock() else {
        return Vec::new();
    };
    // Le fichier a-t-il bougé depuis notre dernière lecture ? Si oui, une autre
    // main a écrit : on FUSIONNE au lieu d'ignorer. Le premier appel (mémoire
    // vide, mtime mémorisé à 0) passe par le même chemin.
    let mt = mtime_registre();
    let connu = REGISTRE_MTIME.swap(mt, std::sync::atomic::Ordering::SeqCst);
    if g.is_empty() || connu != mt {
        fusionner_du_disque(&mut g);
    }
    g.clone()
}

/// Le registre des mondes, en triplets `(label, store, propriétaire)`.
///
/// Rend le registre LISIBLE par d'autres modules (le catalogue des selfs, par
/// exemple) sans leur donner accès à la structure interne ni au verrou : le
/// registre porte déjà « qui a apporté quoi », et c'est une donnée qu'on veut
/// pouvoir croiser ailleurs sans la recopier.
#[must_use]
pub fn registre_des_mondes() -> Vec<(String, String, Option<String>)> {
    enregistres()
        .into_iter()
        .map(|m| {
            (
                m.label,
                m.store.to_string_lossy().to_string(),
                m.proprietaire,
            )
        })
        .collect()
}

/// Le propriétaire d'un monde ajouté à chaud, s'il est connu.
#[must_use]
pub fn proprietaire_de(path: &Path) -> Option<String> {
    enregistres()
        .into_iter()
        .find(|m| m.store == path)
        .and_then(|m| m.proprietaire)
}

/// Enregistre un monde pour qu'il apparaisse partout où les presets apparaissent.
///
/// `proprietaire` = l'identité de l'ajoutant (`utilisateur@compte`), quand le
/// serveur en a une. On l'écrit même si le monde existait déjà sans auteur :
/// c'est la seule occasion de renseigner une provenance manquante.
fn enregistrer(label: String, store: PathBuf, proprietaire: Option<String>) {
    {
        let Ok(mut g) = mondes().lock() else { return };
        if let Some(m) = g.iter_mut().find(|m| m.store == store) {
            if m.proprietaire.is_none() {
                m.proprietaire = proprietaire;
            }
        } else {
            g.push(Monde {
                label,
                store,
                proprietaire,
                ajoute_ms: Some(crate::activation::now_ms()),
            });
        }
    }
    // Persiste le registre (best-effort : l'échec d'écriture ne défait pas le
    // monde). FUSION D'ABORD : le fichier peut porter des mondes que d'autres y
    // ont mis depuis notre chargement — écrire notre seule mémoire les
    // effacerait.
    if let Ok(mut g) = mondes().lock() {
        fusionner_du_disque(&mut g);
        if let Some(d) = registre().parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let v: Vec<serde_json::Value> = g
            .iter()
            .map(|m| {
                serde_json::json!({
                    "label": m.label,
                    "store": m.store.to_string_lossy(),
                    "proprietaire": m.proprietaire,
                    "ajoute_ms": m.ajoute_ms,
                })
            })
            .collect();
        let _ = std::fs::write(
            registre(),
            serde_json::to_string_pretty(&v).unwrap_or_default(),
        );
        REGISTRE_MTIME.store(mtime_registre(), std::sync::atomic::Ordering::SeqCst);
    }
}

/// État d'un travail, s'il existe.
pub fn etat(id: &str) -> Option<Travail> {
    travaux().lock().ok()?.get(id).cloned()
}

/// Tous les travaux, du plus récent au plus ancien.
pub fn tous() -> Vec<Travail> {
    let mut v: Vec<Travail> = travaux()
        .lock()
        .map(|g| g.values().cloned().collect())
        .unwrap_or_default();
    v.sort_by(|a, b| b.debut_ms.cmp(&a.debut_ms));
    v
}

/// Normalise une URL GitHub et en extrait `(propriétaire, dépôt)`.
///
/// On n'accepte QUE github.com : une URL arbitraire serait une exécution de
/// `git clone` sur une cible non validée, depuis une interface web.
fn analyser(url: &str) -> Result<(String, String, String)> {
    let u = url.trim().trim_end_matches('/');
    let sans_schema = u
        .strip_prefix("https://")
        .or_else(|| u.strip_prefix("http://"))
        .or_else(|| u.strip_prefix("git@"))
        .unwrap_or(u);
    let reste = sans_schema
        .strip_prefix("github.com/")
        .or_else(|| sans_schema.strip_prefix("github.com:"))
        .with_context(|| {
            format!(
                "monde : seul github.com est accepté (reçu « {url} ») — \
                     copiez l'adresse du dépôt depuis GitHub"
            )
        })?;
    let mut parts = reste.split('/');
    let proprietaire = parts.next().unwrap_or("").trim();
    let depot = parts.next().unwrap_or("").trim().trim_end_matches(".git");
    if proprietaire.is_empty() || depot.is_empty() {
        bail!("monde : URL incomplète — attendu https://github.com/<propriétaire>/<dépôt>");
    }
    let propre = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    if !propre(proprietaire) || !propre(depot) {
        bail!("monde : nom de dépôt invalide (« {proprietaire}/{depot} »)");
    }
    Ok((
        proprietaire.to_string(),
        depot.to_string(),
        format!("https://github.com/{proprietaire}/{depot}.git"),
    ))
}

/// Lance l'ajout d'un monde. Rend IMMÉDIATEMENT un identifiant de travail.
///
/// `auteur` = l'identité de l'ajoutant (`utilisateur@compte`), quand le serveur
/// en a une. Elle part dans le registre : c'est ce qui permet de répondre à
/// « qui a ajouté ce monde » une fois qu'ils sont plusieurs.
pub fn lancer(
    url: String,
    label_demande: Option<String>,
    auteur: Option<String>,
) -> Result<String> {
    let (proprietaire, depot, clone_url) = analyser(&url)?;
    let id = format!("monde-{}-{}", depot, crate::activation::now_ms());
    let label = label_demande
        .filter(|l| !l.trim().is_empty())
        .unwrap_or_else(|| format!("{proprietaire}/{depot} (GitHub)"));
    let t = Travail {
        id: id.clone(),
        url: clone_url.clone(),
        label: label.clone(),
        etat: "clone".into(),
        etape: "en file".into(),
        store: None,
        depot: None,
        debut_ms: crate::activation::now_ms(),
        fin_ms: None,
        erreur: None,
        noeuds: None,
        aretes: None,
        enrich_ms: None,
        aretes_visitees: None,
        aretes_curvature: None,
        courbure_max: None,
        hubs: None,
        hubs_refus: None,
        trigrammes: None,
        enrich_erreurs: Vec::new(),
    };
    travaux().lock().map(|mut g| g.insert(id.clone(), t)).ok();

    let id_travail = id.clone();
    std::thread::spawn(move || {
        let id = id_travail.as_str();
        let maj = |id: &str, f: &dyn Fn(&mut Travail)| {
            if let Ok(mut g) = travaux().lock()
                && let Some(t) = g.get_mut(id)
            {
                f(t);
            }
        };
        let r = travailler(&id, &clone_url, &label, &proprietaire, &depot, &maj, auteur);
        maj(&id, &|t| match &r {
            Ok((store, noeuds, aretes, e)) => {
                t.etat = "fini".into();
                t.etape = if e.erreurs.is_empty() {
                    "monde prêt, enrichi".into()
                } else {
                    format!(
                        "monde prêt — enrichissement PARTIEL : {}",
                        e.erreurs.join(" · ")
                    )
                };
                t.store = Some(store.to_string_lossy().to_string());
                t.noeuds = Some(*noeuds);
                t.aretes = Some(*aretes);
                t.enrich_ms = Some(e.ms);
                t.aretes_visitees = Some(e.aretes_visitees);
                t.aretes_curvature = Some(e.aretes_curvature);
                t.courbure_max = Some(e.courbure_max);
                t.hubs = Some(e.hubs);
                t.hubs_refus = e.hubs_refus.clone();
                t.trigrammes = Some(e.trigrammes.clone());
                t.enrich_erreurs = e.erreurs.clone();
                t.fin_ms = Some(crate::activation::now_ms());
            }
            Err(e) => {
                t.etat = "erreur".into();
                t.erreur = Some(format!("{e:#}").chars().take(600).collect());
                t.fin_ms = Some(crate::activation::now_ms());
            }
        });
    });
    Ok(id)
}

/// Ce qu'une passe d'enrichissement a réellement produit.
///
/// Chaque champ est MESURÉ, jamais supposé : c'est ce qui permet à l'écran de
/// distinguer « enrichi » de « on a appelé la fonction ».
#[derive(Clone, Debug, Default)]
struct Enrichissement {
    ms: u64,
    aretes_visitees: u64,
    aretes_curvature: u64,
    courbure_max: f64,
    hubs: usize,
    hubs_refus: Option<String>,
    trigrammes: Vec<String>,
    erreurs: Vec<String>,
}

/// Les propriétés textuelles dont on construit l'index de trigrammes AVANT la
/// première question de l'utilisateur.
///
/// La construction de l'index est adaptative : il se construit à la DEUXIÈME
/// exécution d'un prédicat de sous-chaîne. Sans cette passe, la première
/// question qui cherche un nom dans un monde neuf paie donc la construction
/// entière — c'est le défaut que `prechauffer_les_bases` corrige au démarrage,
/// et qu'un monde ajouté à chaud ne recevait jamais (il n'était pas encore là
/// quand la passe est passée).
const PROPS_TRIGRAMMES: &[&str] = &["qualified_name", "name", "file_path"];
const AIGUILLE_TRIGRAMMES: &str = "_obrain_prechauffage_trigrammes_";

/// ── L'ENRICHISSEMENT D'UN MONDE NEUF ────────────────────────────────────────
///
/// Quatre passes, dans l'ordre où elles se rendent utiles les unes aux autres :
///
/// 1. **Courbure de Ricci** (`refresh_all_ricci`) — le relief. C'est elle qui
///    distingue un pont d'une autoroute, et que `node_curvature` relit. Sans
///    elle, la carte affiche un monde plat et la comparaison entre mondes est
///    vide de sens.
/// 2. **Registres de degrés** — lus une fois pour qu'ils se MATÉRIALISENT
///    (lazy-init du substrat). Le classement de hubs en dépend directement.
/// 3. **Hubs** — calculés et PERSISTÉS, pour que le chat les relise en
///    millisecondes au lieu de refaire un parcours de 14,1 s.
/// 4. **Trigrammes** — l'index de texte construit à froid, pour que la première
///    question ne le paie pas.
///
/// ── CE QUI ÉCHOUE EST NOMMÉ, ET NE JETTE PAS LE MONDE ───────────────────────
/// L'indexation a réussi : le monde existe et se lit. Un enrichissement partiel
/// est donc un monde DÉGRADÉ, pas un échec — on le rend, et on le dit. Jeter le
/// travail déjà fait parce qu'une passe a manqué serait la faute inverse de
/// celle qu'on corrige (un monde utile perdu pour un défaut cosmétique).
fn enrichir(
    db: &obrain_engine::ObrainDB,
    store: &Path,
    maj: &dyn Fn(&str, &dyn Fn(&mut Travail)),
    id: &str,
) -> Enrichissement {
    let t0 = std::time::Instant::now();
    let mut e = Enrichissement::default();

    // ── 1. LA COURBURE ────────────────────────────────────────────────────
    maj(id, &|t| {
        t.etat = "enrichissement".into();
        t.etape = "courbure de Ricci sur toutes les arêtes vivantes".into();
    });
    match db.substrate_handle() {
        Some(sub) => match obrain_substrate::geometric::refresh_all_ricci(&sub) {
            Ok(st) => {
                e.aretes_visitees = st.edges_visited;
                e.aretes_curvature = st.edges_updated;
                e.courbure_max = f64::from(st.max_abs_curvature);
                // ── 2. LES REGISTRES DE DEGRÉS SE MATÉRIALISENT ICI ───────────
                // Une lecture les reconstruit s'ils manquent : c'est la seule
                // raison d'appeler une fonction dont on ne lit pas le résultat.
                let _ = sub.out_degree(obrain_common::types::NodeId(1));
                let _ = sub.in_degree(obrain_common::types::NodeId(1));
            }
            Err(err) => e.erreurs.push(format!("courbure : {err}")),
        },
        None => e
            .erreurs
            .push("courbure : store sans substrat (signaux inaccessibles)".into()),
    }

    // ── 3. LES HUBS ───────────────────────────────────────────────────────
    maj(id, &|t| {
        t.etape = "classement des hubs (registres de degrés)".into()
    });
    let classement = crate::hubs::calculer(db, store, crate::hubs::HUBS_PAR_DEFAUT);
    if let Some(refus) = classement.refus.clone() {
        // On PERSISTE le refus : un lecteur doit savoir que le classement
        // n'existe pas, et pourquoi — sinon il croira qu'aucun nœud n'est
        // connecté. C'est le mode de panne que `hubs.rs` documente.
        let _ = crate::hubs::persister(store, &classement);
        e.hubs_refus = Some(refus);
    } else {
        e.hubs = classement.hubs.len();
        let _ = crate::hubs::persister(store, &classement);
    }

    // ── 4. LES TRIGRAMMES ─────────────────────────────────────────────────
    maj(id, &|t| t.etape = "index de texte (trigrammes)".into());
    let presentes: std::collections::HashSet<String> = match db.schema() {
        obrain_engine::SchemaInfo::Lpg(s) => s.property_keys.iter().cloned().collect(),
        _ => std::collections::HashSet::new(),
    };
    for prop in PROPS_TRIGRAMMES {
        // Une propriété ABSENTE ferait balayer tout le store pour rien : on ne
        // sonde que ce que la base déclare — même règle que le préchauffage.
        if !presentes.contains(*prop) {
            continue;
        }
        let sonde =
            format!("MATCH (n) WHERE n.{prop} CONTAINS '{AIGUILLE_TRIGRAMMES}' RETURN 1 LIMIT 1");
        // DEUX exécutions : la garde adaptative construit à la seconde.
        for _ in 0..2 {
            let _ = db.session().execute(&sonde);
        }
        e.trigrammes.push((*prop).to_string());
    }

    // ── LA DURABILITÉ ─────────────────────────────────────────────────────
    // Un enrichissement non flushé serait reperdu au prochain chargement : le
    // monde afficherait son relief, puis l'oublierait. C'est exactement la
    // signature « husk » que le journal du substrat signale au replay.
    if let Err(err) = db.flush_wal() {
        e.erreurs.push(format!("flush du WAL : {err}"));
    }

    e.ms = t0.elapsed().as_millis() as u64;
    e
}

/// Le travail lui-même : clone, puis indexation, puis enrichissement, puis
/// enregistrement.
#[allow(clippy::type_complexity)]
fn travailler(
    id: &str,
    clone_url: &str,
    label: &str,
    proprietaire: &str,
    depot: &str,
    maj: &dyn Fn(&str, &dyn Fn(&mut Travail)),
    // L'identité de l'AJOUTANT — à ne pas confondre avec `proprietaire`, qui est
    // le compte GitHub dont vient le dépôt. Deux « propriétaires » différents :
    // celui du code, et celui du monde.
    auteur: Option<String>,
) -> Result<(PathBuf, i64, i64, Enrichissement)> {
    let racine = racine();
    let src_dir = racine.join("src");
    let depot_dir = src_dir.join(format!("{proprietaire}-{depot}"));
    std::fs::create_dir_all(&src_dir).context("monde : création du répertoire de travail")?;

    // ── 1. CLONE ──────────────────────────────────────────────────────────
    maj(id, &|t| {
        t.etat = "clone".into();
        t.etape = "git clone --depth 1".into();
    });
    if depot_dir.join(".git").is_dir() {
        // Déjà cloné : on rafraîchit plutôt que de retélécharger tout le dépôt.
        let out = Command::new("git")
            .args([
                "-C",
                &depot_dir.to_string_lossy(),
                "pull",
                "--ff-only",
                "--depth",
                "1",
            ])
            .output()
            .context("monde : git pull")?;
        if !out.status.success() {
            // Un échec de rafraîchissement n'empêche pas d'indexer l'existant :
            // on le signale, on ne jette pas le travail déjà fait.
            maj(id, &|t| {
                t.etape = format!(
                    "git pull a échoué ({}), indexation de la copie existante",
                    String::from_utf8_lossy(&out.stderr)
                        .lines()
                        .next()
                        .unwrap_or("")
                        .to_string()
                );
            });
        }
    } else {
        let out = Command::new("git")
            .args([
                "clone",
                "--depth",
                "1",
                "--single-branch",
                clone_url,
                &depot_dir.to_string_lossy(),
            ])
            .output()
            .context("monde : git clone (git est-il installé ?)")?;
        if !out.status.success() {
            bail!(
                "git clone a échoué : {}",
                String::from_utf8_lossy(&out.stderr)
                    .lines()
                    .take(3)
                    .collect::<Vec<_>>()
                    .join(" / ")
            );
        }
    }
    maj(id, &|t| {
        t.depot = Some(depot_dir.to_string_lossy().to_string())
    });

    // ── 2. STORE + INDEXATION ─────────────────────────────────────────────
    maj(id, &|t| {
        t.etat = "indexation".into();
        t.etape = "création du store et indexation du dépôt".into();
    });
    let store = racine.join(format!("{proprietaire}-{depot}.obrain"));
    let db = obrain_engine::ObrainDB::open_substrate(&store)
        .with_context(|| format!("monde : création du store {}", store.display()))?;
    // On réutilise l'indexeur du serveur MCP : un seul chemin d'indexation dans
    // tout le projet, donc des mondes comparables entre eux.
    let (rapport, _erreur) = crate::mcp_server::index_repository(
        &db,
        &serde_json::json!({ "path": depot_dir.to_string_lossy() }),
    );
    if rapport.get("status").and_then(|s| s.as_str()) == Some("error") {
        bail!(
            "indexation : {}",
            rapport
                .get("error")
                .and_then(|e| e.as_str())
                .unwrap_or("échec sans message")
        );
    }
    db.flush_wal().context("monde : flush du store")?;

    let compte = |gql: &str| -> i64 {
        db.session()
            .execute(gql)
            .ok()
            .and_then(|r| r.rows.into_iter().next())
            .and_then(|row| row.into_iter().next())
            .and_then(|v| match v {
                obrain_common::types::Value::Int64(x) => Some(x),
                obrain_common::types::Value::Float64(x) => Some(x as i64),
                _ => None,
            })
            .unwrap_or(0)
    };
    let noeuds = compte("MATCH (n) RETURN count(n)");
    let aretes = compte("MATCH ()-[r]->() RETURN count(r)");
    if noeuds == 0 {
        bail!("indexation : aucun nœud produit — le dépôt n'a peut-être aucun fichier reconnu");
    }

    // ── 3. L'ENRICHISSEMENT — LE MONDE NAÎT AVEC SON RELIEF ────────────────
    // (voir `enrichir` : quatre passes, mesurées, et ce qui échoue est nommé)
    let enrichissement = enrichir(&db, &store, maj, id);

    // ── 4. LE MONDE APPARAÎT ──────────────────────────────────────────────
    enregistrer(label.to_string(), store.clone(), auteur);
    Ok((store, noeuds, aretes, enrichissement))
}

/// Un chemin est-il un store obrain déjà ouvert/valide ?
pub fn est_store(p: &Path) -> bool {
    p.join("substrate.meta").is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ── LE TEST QUI DÉCIDE : UN MONDE AJOUTÉ ARRIVE-T-IL ENRICHI ? ──────────
    ///
    /// C'est le défaut mesuré qu'on corrige, et il ne se voit ni en relisant le
    /// code ni en compilant : il se voit en ajoutant un dépôt et en REGARDANT ce
    /// que la base contient. Le test fait donc l'ajout réel — clone, indexation,
    /// enrichissement — et vérifie que le relief est là : courbure persistée sur
    /// les arêtes, index de texte construit, monde attribué à son auteur.
    ///
    /// Il touche au RÉSEAU (github.com). C'est délibéré : un test hors ligne
    /// validerait notre propre simulacre, pas la chaîne qui sert l'utilisateur.
    ///
    /// ── LE DÉPÔT DOIT AVOIR UN VRAI GRAPHE ──────────────────────────────────
    /// Défaut mesuré au premier essai : `octocat/Hello-World` (le plus petit
    /// dépôt qui existe) ne produit que **2 nœuds et 1 arête** — un README et
    /// sa branche. La courbure n'a alors rien à mesurer, et le test échouait en
    /// accusant l'enrichissement, qui n'y était pour rien. Un test de relief a
    /// besoin d'un graphe qui ait du relief : `is-npm` est petit (quelques
    /// fichiers) mais contient de VRAIS appels entre fonctions.
    #[test]
    #[ignore = "touche au réseau (clone github.com) — lancer avec --ignored"]
    fn un_monde_ajoute_arrive_enrichi() {
        // ── LA RACINE VIENT DE L'ENVIRONNEMENT, PAS DU TEST ─────────────────
        // La crate interdit `unsafe` (`#![deny(unsafe_code)]`), et `set_var` en
        // est un depuis Rust 2024 : le test ne peut donc pas s'isoler lui-même.
        // On exige la variable, on ne la pose pas — et on ÉCHOUE en disant quoi
        // faire, plutôt que d'écrire dans `/tmp/obrain-worlds` (la racine réelle
        // des mondes servis : un test ne doit pas y déposer un monde).
        let Some(racine) = std::env::var_os("OBRAIN_WORLDS_DIR").map(PathBuf::from) else {
            panic!(
                "OBRAIN_WORLDS_DIR est requis pour isoler ce test — par exemple :\n  \
                 OBRAIN_WORLDS_DIR=/tmp/obrain-monde-test cargo test --release -p obrain-cli \
                 --features ai,iam --bin obrain worlds:: -- --ignored --nocapture"
            );
        };
        let _ = std::fs::remove_dir_all(&racine);

        let id = lancer(
            "https://github.com/sindresorhus/is-npm".into(),
            Some("sindresorhus/is-npm (test)".into()),
            Some("test@obrain".into()),
        )
        .expect("l'ajout doit démarrer");

        // Un clone + une indexation + un enrichissement durent des secondes.
        let echeance = std::time::Instant::now() + std::time::Duration::from_secs(300);
        let mut t = etat(&id).expect("le travail doit être suivi");
        while t.etat != "fini" && t.etat != "erreur" && std::time::Instant::now() < echeance {
            std::thread::sleep(std::time::Duration::from_millis(500));
            t = etat(&id).expect("le travail doit rester suivi");
        }

        assert_eq!(t.etat, "fini", "l'ajout a échoué : {:?}", t.erreur);
        // LE COMPTE RENDU EST AFFICHÉ : quand le test échoue, on veut savoir
        // AVEC QUOI il a échoué (2 nœuds ? aucune arête ? quel graphe ?) sans
        // avoir à relancer une campagne pour le découvrir.
        eprintln!(
            "monde : {} nœuds, {} arêtes · enrichissement {} ms · courbure visitée {} / maj {} \
             (max|κ| {:.4}) · hubs {} {:?} · index {:?} · erreurs {:?}",
            t.noeuds.unwrap_or(0),
            t.aretes.unwrap_or(0),
            t.enrich_ms.unwrap_or(0),
            t.aretes_visitees.unwrap_or(0),
            t.aretes_curvature.unwrap_or(0),
            t.courbure_max.unwrap_or(0.0),
            t.hubs.unwrap_or(0),
            t.hubs_refus.clone().unwrap_or_default(),
            t.trigrammes.clone().unwrap_or_default(),
            t.enrich_erreurs,
        );
        assert_eq!(
            t.enrich_erreurs,
            Vec::<String>::new(),
            "enrichissement partiel : {:?}",
            t.enrich_erreurs
        );
        assert!(
            t.enrich_ms.is_some_and(|ms| ms > 0),
            "aucune passe d'enrichissement n'a été mesurée"
        );
        assert!(
            t.aretes_visitees.unwrap_or(0) > 0,
            "la courbure n'a visité aucune arête"
        );
        assert!(
            t.trigrammes.as_ref().is_some_and(|v| !v.is_empty()),
            "aucun index de texte construit"
        );
        // ── LE RELIEF N'A DE SENS QUE SUR UN GRAPHE QUI EN A ────────────────
        // Un dépôt qui ne produit qu'un nœud et une arête ne peut pas porter de
        // courbure : exiger alors un classement de hubs ferait échouer le test
        // sur la pauvreté du corpus, pas sur un défaut du code. On exige donc le
        // graphe minimal AVANT d'exiger le relief — et on le DIT si le corpus
        // choisi est trop pauvre, car ce serait alors le test qu'il faut changer.
        assert!(
            t.aretes.unwrap_or(0) >= 3,
            "le corpus de test est trop pauvre ({} arête(s)) pour porter un relief : \
             choisir un dépôt avec de vrais appels entre fonctions",
            t.aretes.unwrap_or(0)
        );
        assert!(
            t.hubs_refus.is_none(),
            "classement de hubs refusé : {:?}",
            t.hubs_refus
        );

        // ── LE MONDE EST ATTRIBUÉ, ET SON STORE PORTE VRAIMENT LE RELIEF ────
        // Le rapport du travail pourrait mentir (compteurs mal remplis) : on
        // relit donc la BASE elle-même, qui ne ment pas.
        let store = PathBuf::from(t.store.clone().expect("le store doit être nommé"));
        assert_eq!(proprietaire_de(&store).as_deref(), Some("test@obrain"));
        // ── LA COURBURE SE LIT DANS L'ENREGISTREMENT D'ARÊTE, PAS EN GQL ────
        // Défaut mesuré en écrivant ce test : `MATCH ()-[r]->() WHERE
        // r.ricci_f32 IS NOT NULL` rend **0** alors que toutes les arêtes
        // portent leur courbure. La propriété vit dans l'`EdgeRecord` du
        // substrat (`ricci_u8`), que le moteur GQL n'expose pas comme propriété
        // d'arête. Ce n'est donc pas le test qui « ne voyait pas » le relief —
        // c'est la surface d'interrogation qui ne le sert pas, et une
        // vérification écrite sur cette surface aurait conclu « le monde est
        // arrivé brut » sur un monde parfaitement enrichi. On lit donc la
        // source : le substrat, celui-là même que l'enrichissement a écrit.
        let sub = obrain_substrate::SubstrateStore::open(&store).expect("ouverture substrate");
        let mut courbure = 0usize;
        let mut vivantes = 0usize;
        sub.for_each_live_edge(|_slot, rec| {
            vivantes += 1;
            // `ricci_f32` est encodé sur un octet : la valeur NEUTRE (0,0 → 127)
            // est ce qu'un enregistrement JAMAIS enrichi porte. Compter les
            // valeurs non neutres, c'est compter les arêtes que la passe a
            // réellement touchées — pas celles qu'elle a survolées.
            if (rec.ricci_f32()).abs() > f32::EPSILON {
                courbure += 1;
            }
        })
        .expect("parcours des arêtes vivantes");
        assert!(
            courbure > 0,
            "aucune arête ne porte de courbure ({courbure}/{vivantes}) : \
             le monde est arrivé brut"
        );
        let _ = std::fs::remove_dir_all(&racine);
    }
}
