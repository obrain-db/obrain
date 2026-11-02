//! LA FLOTTE COMME FAIT DU GRAPHE — la machine comme nœud de premier ordre.
//!
//! # Le manque que ce module comble
//!
//! Relevé du 2026-09-15, §3 : « Le graphe global ne connaît aucune machine » —
//! les labels présents étaient `Message`, `ToolCall`, `ToolResult`, `Step`,
//! `Function`, `SelfState`, « mais pas la flotte ». Et §4 : « On ne peut pas
//! piloter une machine qui n'a pas de nom. » La porte d'acceptation en a fait son
//! DERNIER échec (`docs/ACCEPTATION-FLOTTE-2026-09-16.md`, manque 3) : le
//! registre connaît les machines, la route les dit — mais une IA qui interroge le
//! **graphe** ne trouvait rien.
//!
//! # Pourquoi une PASSE, et pas une écriture directe
//!
//! Parce que le moteur sert ses bases en **LECTURE** (flock partagé) : il ne peut
//! pas écrire dedans, et ce n'est pas un défaut à contourner — l'invariant
//! mono-écrivain protège les données. C'est le motif en deux temps de
//! `memoire.rs`, appliqué à la flotte :
//!
//!   1. le registre **JOURNALISE** ce qu'il apprend — append-only, aucun verrou.
//!      C'est déjà fait et déjà durable : voir `remote.rs` ;
//!   2. cette passe **APPLIQUE** : elle ouvre la base en EXCLUSIF, écrit en
//!      `MERGE` idempotent, et consomme ce que le journal portait.
//!
//! # Ce qui est écrit — et ce qui ne l'est PAS
//!
//! Un nœud `machine:<hôte>` par machine connue, portant des **FAITS DATÉS** : le
//! compte propriétaire, les capacités **ANNONCÉES**, la première attache, la
//! dernière présence, et quelle machine a écrit le fait.
//!
//! Il n'y a **aucune propriété « joignable »**, et c'est délibéré. Un booléen
//! écrit à l'application serait périmé une minute plus tard, et un état recopié
//! qui ment est exactement ce que la doctrine interdit (« annoncer est décider »).
//! `derniere_presence_ms` est un fait : c'est au LECTEUR de le comparer à l'heure
//! s'il veut savoir si la machine parle encore. Le registre, lui, garde `vivant`
//! en mémoire — là où l'information est fraîche.
//!
//! # Pourquoi DEUX propriétés d'identité
//!
//! `qn` **et** `qualified_name`, mesuré : le dépôt porte deux conventions. La
//! mémoire et le registre nomment leurs nœuds `qn` (`memoire.rs`, `remote.rs`) ;
//! le graphe de code et la surface MCP qui parle à une IA nomment
//! `qualified_name` (`obrain-indexer`, `mcp_server.rs`). Une machine écrite avec
//! une seule des deux serait INVISIBLE à l'autre lecteur — et l'objectif même de
//! ce module est qu'on la trouve. On écrit donc les deux, plutôt que de trancher
//! une incohérence du dépôt qui n'est pas le sujet de ce chantier.
//!
//! ── AUTO-DÉCLARATION : LE MOTEUR SEUL ENTRE ENFIN DANS LE GRAPHE (2026-09-16) ─
//!
//! Ce module n'écrivait que ce que le REGISTRE connaissait, c'est-à-dire les
//! machines sur lesquelles un `selfd` s'est attaché. Une machine qui fait tourner
//! un moteur SANS `selfd` — le cas le plus fréquent, et le seul qui existe sur une
//! machine fraîchement provisionnée (n2 dans le relevé du 2026-09-15 : binaire
//! présent, aucun store, « rien qui tourne ») — restait donc innommable dans le
//! graphe, alors que c'est précisément elle qu'on veut piloter.
//!
//! Le manque est comblé par la seule source qui puisse le combler : le harness
//! JOURNALISE sa propre identité au démarrage (`harness_serve.rs`, section
//! « l'identité de la machine »), dans le **même** journal que le registre — même
//! répertoire (`OBRAIN_VIZ_CACHE_DIR/machines`), même extension `.jsonl`, donc
//! aucune seconde mécanique à faire exister ni à découvrir. Une ligne de
//! déclaration ne porte ni `id` ni `compte` : un moteur seul n'a pas de compte, et
//! lui en prêter un serait décider à sa place. Le rejeu du registre l'ignore (il
//! exige un `id`) ; ce module en est le seul lecteur.
//!
//! Deux nœuds en sortent, et ils ne disent pas la même chose :
//!
//! * `machine:<hôte>` — le nœud de PREMIER ORDRE demandé par le document (point 1).
//!   Il porte ce que le registre sait de la machine (comptes, capacités
//!   annoncées) ET ce que les moteurs en ont déclaré (os, arch, mode offert,
//!   stores servis, instant de la dernière déclaration) ;
//! * `machine:<hôte>#<base>` — l'INSTANCE, telle que le harness la publie déjà sur
//!   `/api/harness/machine`. Elle existe parce qu'un hôte peut servir PLUSIEURS
//!   bases (deux comptes, deux environnements) : sans elle, deux moteurs d'un même
//!   hôte seraient indiscernables — c'est le fait mesuré qui a créé l'identité du
//!   harness, et c'est pourquoi ce nom-là porte le store servi.
//!
//! **Aucune capacité n'est inventée.** Une déclaration n'annonce aucune capacité :
//! le nœud d'une machine sans `selfd` porte `capacites_annoncees` VIDE, et ses
//! faits de rattachement (`compte`, `premier_attache_ms`, `derniere_presence_ms`,
//! `incarnations`) ne sont **pas écrits du tout** — un zéro écrit se lirait comme
//! une date, et une liste vide se lirait comme une liste. Réciproquement, une
//! déclaration de moteur n'EFFACE JAMAIS les capacités annoncées par un `selfd` :
//! ne rien annoncer n'est pas annoncer qu'il n'y a rien.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::memoire::lit;

/// Ce qu'un compte a attaché, agrégé par NOM de machine.
///
/// L'agrégation est par nom et non par client : le journal porte des
/// INCARNATIONS (une par processus qui s'est attaché), alors que le graphe doit
/// porter des MACHINES. Plusieurs incarnations du même nom ne font qu'une
/// machine ; c'est la plus récente qui dit ses capacités, parce que c'est la
/// seule dont on sache qu'elles sont encore les siennes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FaitMachine {
    pub machine: String,
    /// Les comptes qui ont attaché une machine de ce nom — le plus souvent un
    /// seul. Plusieurs comptes sur un même nom d'hôte est un cas réel (deux
    /// personnes, une machine partagée) et il est CONSERVÉ, pas écrasé.
    pub comptes: Vec<String>,
    /// Capacités annoncées par l'incarnation la plus récente.
    pub capacites: Vec<String>,
    /// Première attache connue — **0 n'est pas une date** : quand `comptes` est
    /// vide, aucun `selfd` ne s'est jamais attaché ici, et `appliquer` n'écrit
    /// alors PAS ce fait (un zéro écrit se lirait comme le 1er janvier 1970).
    pub premier_attache_ms: i64,
    /// Dernière présence vue par le registre — même convention que ci-dessus.
    pub derniere_presence_ms: i64,
    /// Combien d'incarnations distinctes le journal porte pour ce nom.
    pub incarnations: usize,
    /// Ce que des MOTEURS ont déclaré d'eux-mêmes sur cet hôte, du plus ancien au
    /// plus récent — vide quand aucun harness n'a journalisé son identité ici
    /// (flotte purement `selfd`).
    ///
    /// C'est un fait d'une AUTRE nature que les précédents : il ne vient pas du
    /// registre, et il ne porte ni compte ni capacité. Le garder séparé interdit
    /// de confondre « une machine sur laquelle quelqu'un s'est attaché » et « une
    /// machine sur laquelle un moteur tourne » — les mélanger ferait hériter un
    /// moteur seul des capacités d'un autre, exactement ce que « annoncer est
    /// décider » interdit.
    pub declarations: Vec<Declaration>,
}

/// Une déclaration qu'un harness a faite **de lui-même** au démarrage.
///
/// ── POURQUOI CE N'EST PAS UN `Client` ────────────────────────────────────────
/// Un `Client` (`remote.rs`) est un `selfd` ATTACHÉ à un compte : il porte un
/// compte, des capacités annoncées, et un canal par lequel le moteur l'appelle.
/// Un harness qui tourne seul n'a rien de tout cela. Le ranger dans un `Client`
/// lui prêterait un compte qu'il n'a pas et le ferait apparaître dans la liste
/// d'un utilisateur à qui la machine n'appartient pas — c'est la décision du
/// 2026-09-16 (chacun ne voit que SES machines) qui l'interdit. Ce fait-là
/// n'appartient qu'au harness, et le journal est ce qui les réunit sans les
/// confondre.
///
/// **Tout ce qui est ici est mesurable par le harness**, et rien d'autre : son
/// nom d'hôte lu du noyau (et la source de lecture, qui n'est pas décorative),
/// sa plateforme, le store qu'on lui a demandé de servir, le mode qu'il offre, le
/// binaire qui tourne. Aucune capacité, aucun droit, aucune joignabilité.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Declaration {
    /// Le nom d'hôte — `inconnu` si le harness n'a rien pu lire (jamais deviné).
    pub machine: String,
    /// `machine:<hôte>#<base servie>`, **telle que le harness la publie** : on ne
    /// la recalcule pas ici, on la journalise (deux calculs pour une même vérité
    /// finissent par se contredire).
    pub instance: String,
    /// Le nom de la base servie (dernier élément du chemin du store).
    pub base: String,
    /// D'où vient le nom d'hôte : `/proc/sys/kernel/hostname` n'est pas `$HOSTNAME`.
    pub nom_source: String,
    pub os: String,
    pub arch: String,
    /// Le store RÉELLEMENT servi — l'argument du processus.
    pub store: String,
    pub store_racine: String,
    /// Le mode offert (`ro` aujourd'hui : le harness ouvre en lecture seule).
    pub mode: String,
    /// `OBRAIN_HARNESS_ROOT` si l'exploitant l'a déclarée, sinon `non_declaree`.
    pub racine_declaree: String,
    pub ecoute: String,
    pub pid: i64,
    /// L'instant de la déclaration : c'est la dernière présence que le graphe
    /// puisse affirmer d'un moteur. Datée, donc comparable à l'heure de lecture —
    /// c'est au LECTEUR de décider si elle est fraîche (même règle que
    /// `derniere_presence_ms`).
    pub declaree_ms: i64,
    pub version: String,
    pub commit: String,
    pub arbre: String,
}

impl Declaration {
    /// Le vrai discriminant de cette ligne parmi celles du registre : le genre.
    /// C'est lui qui fait qu'un même répertoire peut porter deux natures de
    /// faits sans qu'on ait à les démêler par le nom du fichier.
    pub const GENRE: &'static str = "machine";

    /// Parse UNE ligne du journal. Fonction **PURE** — c'est elle qui s'éprouve
    /// sans fichier, sans horloge et sans variable d'environnement.
    ///
    /// Une ligne qui n'est pas une déclaration de machine rend `None` : le même
    /// répertoire porte les journaux du registre (attaches, battements), et les
    /// confondre ferait naître une machine d'un battement.
    ///
    /// Une déclaration sans nom d'hôte ou sans instance ne désigne RIEN : elle
    /// est écartée plutôt que complétée par des valeurs par défaut. Inventer le
    /// nom de la machine serait le pire des défauts — une action partirait au
    /// mauvais endroit.
    pub fn depuis_ligne(ligne: &str) -> Option<Self> {
        let v: serde_json::Value = serde_json::from_str(ligne).ok()?;
        if v.get("kind").and_then(|k| k.as_str()) != Some(Self::GENRE) {
            return None;
        }
        let s = |k: &str| {
            v.get(k)
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string()
        };
        let machine = s("machine");
        let instance = s("instance");
        if machine.is_empty() || instance.is_empty() {
            return None;
        }
        // La base vient de l'instance si la ligne est ancienne : c'est le seul
        // endroit où elle est sûre, et la recalculer depuis le store donnerait un
        // nom pour un chemin qui n'en a pas.
        let base = {
            let b = s("base");
            if b.is_empty() {
                instance
                    .split_once('#')
                    .map(|(_, x)| x.to_string())
                    .unwrap_or_default()
            } else {
                b
            }
        };
        Some(Self {
            machine,
            instance,
            base,
            nom_source: s("nom_source"),
            os: s("os"),
            arch: s("arch"),
            store: s("store"),
            store_racine: s("store_racine"),
            mode: s("mode"),
            racine_declaree: s("racine_declaree"),
            ecoute: s("ecoute"),
            pid: v
                .get("pid")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0),
            declaree_ms: v
                .get("declaree_ms")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0),
            version: s("version"),
            commit: s("commit"),
            arbre: s("arbre"),
        })
    }
}

/// Les déclarations d'un flot de lignes, dans l'ordre du journal. Fonction PURE :
/// c'est le contrat de lecture, éprouvé sans répertoire.
pub fn declarations_depuis<'a>(lignes: impl IntoIterator<Item = &'a str>) -> Vec<Declaration> {
    lignes
        .into_iter()
        .filter_map(Declaration::depuis_ligne)
        .collect()
}

impl FaitMachine {
    /// La déclaration la plus RÉCENTE de cet hôte — celle qui fait autorité pour
    /// ce qui se réannonce (mode offert, os, arch, store), comme la plus récente
    /// des incarnations fait autorité pour les capacités. `declarations` est
    /// trié par la passe de lecture et par l'agrégation, donc « la dernière » est
    /// bien la plus récente.
    pub fn declaration_recente(&self) -> Option<&Declaration> {
        self.declarations.last()
    }

    /// Les stores servis sur cet hôte, distincts et triés : un hôte peut en
    /// servir plusieurs (deux comptes, deux environnements), et le graphe doit
    /// pouvoir le dire sans en perdre un.
    pub fn stores_servis(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .declarations
            .iter()
            .map(|d| d.store.clone())
            .filter(|s| !s.is_empty())
            .collect();
        v.sort();
        v.dedup();
        v
    }
}

/// Le répertoire du journal commun — celui du registre (`remote.rs`) **et** celui
/// des auto-déclarations.
///
/// La convention est réécrite ici plutôt qu'importée parce que `remote.rs` garde
/// la sienne privée : ce qui compte est qu'il n'y en ait qu'UNE pour les deux
/// lecteurs de ce module (la passe) et l'écrivain (`harness_serve.rs`). Deux
/// résolutions divergentes écriraient la déclaration dans un journal que la passe
/// ne lit pas — le défaut le plus coûteux de ce genre de mécanisme, parce qu'il
/// est silencieux.
pub fn dir_journal_machines() -> PathBuf {
    std::env::var_os("OBRAIN_VIZ_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/obrain-viz-cache"))
        .join("machines")
}

/// Relit le journal et rend les déclarations d'auto-déclaration qu'il porte.
///
/// Même geste que la relecture du registre (`Registre::charger`) : tous les
/// `.jsonl` du répertoire, une ligne à la fois. Un `.tmp` (écriture interrompue)
/// n'est pas un journal — le lire à moitié serait pire que l'ignorer. Un
/// répertoire absent (jamais rien écrit, ou cache effacé) n'est pas une erreur :
/// il n'y a simplement rien à déclarer, et cela se dit par une liste vide.
pub fn lire_declarations(dir: &Path) -> Vec<Declaration> {
    let mut toutes: Vec<Declaration> = Vec::new();
    if let Ok(lectures) = std::fs::read_dir(dir) {
        for e in lectures.flatten() {
            let chemin = e.path();
            if chemin.extension().and_then(|x| x.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(txt) = std::fs::read_to_string(&chemin) else {
                continue;
            };
            for ligne in txt.lines().filter(|l| !l.trim().is_empty()) {
                if let Some(d) = Declaration::depuis_ligne(ligne) {
                    toutes.push(d);
                }
            }
        }
    }
    // DERNIÈRE LIGNE GAGNANTE, par instance — la règle du registre, et pour la
    // même raison : un journal append-only porte des ÉTATS successifs. Un harness
    // redémarré cinquante fois ne doit pas faire cinquante machines, il doit faire
    // une machine dont la déclaration est la plus récente. Le tri par instance
    // puis par instant rend la relecture indépendante de l'ordre des fichiers,
    // donc deux passes successives écrivent le même graphe.
    toutes.sort_by(|a, b| {
        a.instance
            .cmp(&b.instance)
            .then(a.declaree_ms.cmp(&b.declaree_ms))
    });
    let mut par_instance: BTreeMap<String, Declaration> = BTreeMap::new();
    for d in toutes {
        par_instance.insert(d.instance.clone(), d);
    }
    par_instance.into_values().collect()
}

/// Agrège des clients du registre en machines — **fonction PURE**, éprouvable
/// sans base ni variable d'environnement.
pub fn faits_depuis(clients: &[crate::remote::Client]) -> Vec<FaitMachine> {
    let mut par_machine: BTreeMap<String, FaitMachine> = BTreeMap::new();
    for c in clients {
        let e = par_machine
            .entry(c.machine.clone())
            .or_insert_with(|| FaitMachine {
                machine: c.machine.clone(),
                comptes: Vec::new(),
                capacites: c.capacites.clone(),
                premier_attache_ms: c.attache_ms,
                derniere_presence_ms: c.derniere_vue_ms,
                incarnations: 0,
                declarations: Vec::new(),
            });
        if !e.comptes.contains(&c.compte) {
            e.comptes.push(c.compte.clone());
        }
        e.incarnations += 1;
        // La plus RÉCENTE fait autorité pour les capacités : une machine qui
        // s'est réannoncée sans `--allow-exec` ne doit pas rester décrite par
        // l'incarnation d'avant, qui l'autorisait.
        if c.derniere_vue_ms > e.derniere_presence_ms {
            e.capacites = c.capacites.clone();
        }
        e.derniere_presence_ms = e.derniere_presence_ms.max(c.derniere_vue_ms);
        e.premier_attache_ms = e.premier_attache_ms.min(c.attache_ms);
    }
    for e in par_machine.values_mut() {
        e.comptes.sort();
        e.capacites.sort();
    }
    par_machine.into_values().collect()
}

/// **La flotte entière** : ce que le registre sait (rattachements, capacités
/// annoncées) ET ce que les moteurs ont déclaré d'eux-mêmes.
///
/// L'agrégation reste par NOM D'HÔTE — c'est la clé du nœud de premier ordre, et
/// deux sources qui compteraient les machines différemment rendraient le graphe
/// incohérent avec lui-même. Ce qui s'ajoute est une AUTRE nature de fait sur le
/// même nœud, jamais un remplacement :
///
/// * les capacités ne viennent QUE des `selfd` attachés. Une déclaration de
///   moteur n'en porte aucune, et n'efface donc pas celles qu'un `selfd` a
///   annoncées : le harness ne sait pas ce que la politique locale d'un `selfd`
///   autorise, et l'écrire à sa place serait inventer un droit ;
/// * une machine qu'AUCUN `selfd` n'a jamais attachée obtient un fait dont les
///   champs de rattachement restent à zéro — et `appliquer` ne les écrira pas.
///   C'est la machine « moteur seul », celle que le point 1 du document réclame.
pub fn faits_avec_declarations(
    clients: &[crate::remote::Client],
    declarations: &[Declaration],
) -> Vec<FaitMachine> {
    let mut par_machine: BTreeMap<String, FaitMachine> = faits_depuis(clients)
        .into_iter()
        .map(|f| (f.machine.clone(), f))
        .collect();
    for d in declarations {
        let e = par_machine
            .entry(d.machine.clone())
            .or_insert_with(|| FaitMachine {
                machine: d.machine.clone(),
                comptes: Vec::new(),
                // AUCUNE capacité : un moteur seul n'en annonce pas. C'est le point
                // délicat du chantier — « annoncer est décider » — et une liste vide
                // ici est un fait, pas un trou.
                capacites: Vec::new(),
                premier_attache_ms: 0,
                derniere_presence_ms: 0,
                incarnations: 0,
                declarations: Vec::new(),
            });
        e.declarations.push(d.clone());
    }
    for e in par_machine.values_mut() {
        e.declarations.sort_by(|a, b| {
            a.declaree_ms
                .cmp(&b.declaree_ms)
                .then(a.instance.cmp(&b.instance))
        });
    }
    par_machine.into_values().collect()
}

/// Ce qu'une passe a écrit.
#[derive(Debug, Default)]
pub struct Rapport {
    pub store: String,
    /// Nœuds écrits dans la base : les machines, PLUS les instances déclarées.
    pub machines: usize,
    /// Nœuds d'INSTANCE écrits (`machine:<hôte>#<base>`), compris dans
    /// `machines`. Publié à part parce que c'est la réponse à « combien de
    /// moteurs se sont déclarés », qui n'est pas « combien de machines ».
    pub instances: usize,
    /// Ce que le journal portait, avant écriture (0 = rien à appliquer).
    pub connues: usize,
    /// Déclarations d'auto-déclaration lues au journal — celles qui font entrer
    /// un moteur seul dans le graphe, sans `selfd` et sans capacité.
    pub declarations: usize,
    /// La machine qui a écrit le fait (nom d'hôte du moteur, lu du noyau).
    pub faits_par: String,
    /// Refus, nommés et motivés — jamais perdus en silence.
    pub refusees: Vec<(String, String)>,
}

/// Écrit la flotte dans une base — ouverture **EXCLUSIVE**.
///
/// Échoue si un lecteur tient la base : c'est voulu (voir l'en-tête de
/// `memoire.rs`), et l'appelant doit alors arrêter le service le temps de la
/// passe. `MERGE` par `qn` rend l'opération **idempotente** : appliquer deux
/// fois ne duplique rien, et une machine disparue du registre garde son nœud —
/// on n'efface pas un fait passé parce qu'il n'est plus d'actualité.
///
/// # `compte_filtre` : pourquoi la passe sait se restreindre
///
/// `None` écrit TOUTE la flotte (c'est le geste du propriétaire du serveur, et
/// celui de la porte d'acceptation). `Some(compte)` n'écrit que les machines de
/// ce compte — et c'est ce qui rend la passe **utilisable par un timer** :
///
/// un timer qui viserait la base SERVIE échouerait à chaque passage, parce que le
/// moteur la tient en lecture (verrou partagé) et que l'écriture exige
/// l'exclusif. Les bases `selfs/<compte>.obrain`, elles, ne sont tenues par aucun
/// service : un timer peut donc y écrire. Chaque compte apprend ainsi QUELLES
/// MACHINES SONT LES SIENNES, dans sa propre base — ce qui est le sens de la
/// décision du 2026-09-16 : la flotte est une affaire de compte, pas une vue
/// d'ensemble sur tout le monde.
///
/// # Une passe CLOISONNÉE n'écrit aucune auto-déclaration
///
/// Un moteur seul n'a pas de compte : rien ne dit à qui il appartient. L'écrire
/// dans la base d'un compte lui prêterait cette machine — et une liste ne doit
/// jamais contenir une machine dont on n'a pas vérifié le droit (« annoncer est
/// décider »). Les déclarations ne sont donc lues QUE par la passe non filtrée,
/// celle qui écrit dans le store servi et que déclenche le propriétaire du
/// serveur. Le cloisonnement est ainsi un fait du CODE, pas une convention.
pub fn appliquer(store: &Path, compte_filtre: Option<&str>) -> Result<Rapport> {
    let clients = crate::remote::toutes_les_machines();
    let declarations = match compte_filtre {
        Some(_) => Vec::new(),
        None => lire_declarations(&dir_journal_machines()),
    };
    let faits: Vec<FaitMachine> = match compte_filtre {
        None => faits_avec_declarations(&clients, &declarations),
        Some(compte) => faits_depuis(&clients)
            .into_iter()
            .filter(|f| f.comptes.iter().any(|x| x == compte))
            .collect(),
    };
    let (faits_par, _source) = crate::harness_serve::nom_hote();
    let mut rapport = Rapport {
        store: store.to_string_lossy().to_string(),
        connues: faits.len(),
        declarations: declarations.len(),
        faits_par,
        ..Default::default()
    };
    if faits.is_empty() {
        // Rien à appliquer n'est pas une erreur, et surtout on n'ouvre PAS la
        // base : une passe qui n'a rien à dire ne doit pas disputer le verrou
        // exclusif au service qui la sert en lecture.
        return Ok(rapport);
    }
    let db = obrain_engine::ObrainDB::open_substrate(store).with_context(|| {
        format!(
            "flotte : ouverture EXCLUSIVE de {} — si le dashboard tourne, il tient la base \
             en lecture : arrêtez-le le temps de la passe (même contrainte que \
             `obrain memoire --appliquer`)",
            store.display()
        )
    })?;
    let run = |gql: &str| -> Result<(), String> {
        db.session()
            .execute(gql)
            .map(|_| ())
            .map_err(|e| e.to_string())
    };
    let applique_ms = crate::activation::now_ms();
    for f in &faits {
        // ── (1) LE NŒUD DE PREMIER ORDRE : `machine:<hôte>` ────────────────
        // Le document (point 1) nomme la machine, pas le processus : c'est ce
        // nœud-là qu'une question de flotte doit trouver. Il réunit deux natures
        // de faits, sous des noms distincts pour qu'aucune ne se lise comme
        // l'autre.
        let qn = format!("machine:{}", f.machine);
        let mut sets: Vec<String> = vec![
            format!("m.qualified_name = {}", lit(&qn)),
            format!("m.machine = {}", lit(&f.machine)),
            // Le vide est ÉCRIT, et il est vrai : « aucune capacité annoncée »
            // est un fait vérifié par cette passe, pas un zéro muet. Ce qui n'est
            // PAS écrit, ce sont les faits de rattachement, qui n'existent pas.
            format!("m.comptes_connus = {}", lit(&f.comptes.join(","))),
            format!("m.capacites_annoncees = {}", lit(&f.capacites.join(","))),
            format!("m.faits_par = {}", lit(&rapport.faits_par)),
            format!("m.applique_ms = {applique_ms}"),
        ];
        if let Some(compte) = f.comptes.first() {
            sets.push(format!("m.compte = {}", lit(compte)));
        }
        if !f.comptes.is_empty() {
            sets.push(format!("m.premier_attache_ms = {}", f.premier_attache_ms));
            sets.push(format!(
                "m.derniere_presence_ms = {}",
                f.derniere_presence_ms
            ));
            sets.push(format!("m.incarnations = {}", f.incarnations));
        }
        if let Some(d) = f.declaration_recente() {
            sets.push(format!("m.os = {}", lit(&d.os)));
            sets.push(format!("m.arch = {}", lit(&d.arch)));
            sets.push(format!("m.nom_source = {}", lit(&d.nom_source)));
            sets.push(format!("m.mode_offert = {}", lit(&d.mode)));
            // Singulier = la dernière déclaration (même règle d'autorité que les
            // capacités) ; pluriel = la liste complète, parce qu'un hôte peut
            // servir plusieurs bases et qu'en perdre une serait perdre une
            // machine de la flotte.
            sets.push(format!("m.store_servi = {}", lit(&d.store)));
            sets.push(format!(
                "m.stores_servis = {}",
                lit(&f.stores_servis().join(","))
            ));
            sets.push(format!("m.instances_declarees = {}", f.declarations.len()));
            sets.push(format!("m.declaration_ms = {}", d.declaree_ms));
            sets.push(format!("m.instance_recente = {}", lit(&d.instance)));
            sets.push(format!("m.racine_declaree = {}", lit(&d.racine_declaree)));
        }
        let gql = format!(
            "MERGE (m:Machine {{qn: {}}}) SET {} RETURN m",
            lit(&qn),
            sets.join(", ")
        );
        match run(&gql) {
            Ok(()) => rapport.machines += 1,
            Err(e) => rapport.refusees.push((qn, e)),
        }
        // ── (2) LE NŒUD D'INSTANCE : `machine:<hôte>#<base servie>` ────────
        // Un hôte peut servir PLUSIEURS bases (deux comptes, deux
        // environnements) : sans ce nœud, deux moteurs du même hôte seraient
        // indiscernables, et c'est le fait mesuré qui a créé l'identité du
        // harness. Son nom porte le store servi — la réponse à « quelle machine
        // sert quoi » se lit donc dans la question elle-même.
        //
        // Tout y vient de la déclaration : rien n'est recalculé, donc rien ne
        // peut diverger de ce que le harness publie sur `/api/harness/machine`.
        // `capacites_annoncees` y est VIDE parce que c'est la vérité : un harness
        // n'annonce aucune capacité, il sert une base en lecture.
        for d in &f.declarations {
            let iqn = d.instance.clone();
            let gql = format!(
                "MERGE (m:Machine {{qn: {}}}) SET m.qualified_name = {}, m.machine = {}, \
                 m.instance = {}, m.store_servi = {}, m.store_racine = {}, m.base = {}, \
                 m.mode_offert = {}, m.os = {}, m.arch = {}, m.nom_source = {}, \
                 m.racine_declaree = {}, m.ecoute = {}, m.pid = {}, m.declaree_ms = {}, \
                 m.version = {}, m.binaire_commit = {}, m.binaire_arbre = {}, \
                 m.comptes_connus = '', m.capacites_annoncees = '', \
                 m.faits_par = {}, m.applique_ms = {} RETURN m",
                lit(&iqn),
                lit(&iqn),
                lit(&d.machine),
                lit(&d.instance),
                lit(&d.store),
                lit(&d.store_racine),
                lit(&d.base),
                lit(&d.mode),
                lit(&d.os),
                lit(&d.arch),
                lit(&d.nom_source),
                lit(&d.racine_declaree),
                lit(&d.ecoute),
                d.pid,
                d.declaree_ms,
                lit(&d.version),
                lit(&d.commit),
                lit(&d.arbre),
                lit(&rapport.faits_par),
                applique_ms
            );
            match run(&gql) {
                Ok(()) => {
                    rapport.machines += 1;
                    rapport.instances += 1;
                }
                Err(e) => rapport.refusees.push((iqn, e)),
            }
        }
    }
    let _ = db.flush_wal();
    Ok(rapport)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::Client;

    fn client(
        id: &str,
        compte: &str,
        machine: &str,
        caps: &[&str],
        attache: i64,
        vue: i64,
    ) -> Client {
        Client::pour_test(id, compte, machine, caps, attache, vue, true)
    }

    /// Deux incarnations du MÊME nom ne font qu'UNE machine — sinon le graphe
    /// porterait un nœud par redémarrage, et « la machine » se perdrait dans ses
    /// propres traces.
    #[test]
    fn deux_incarnations_du_meme_nom_font_une_machine() {
        let cs = vec![
            client("a-1", "alice", "n3", &["read", "list"], 100, 200),
            client("a-2", "alice", "n3", &["read", "list"], 300, 400),
        ];
        let f = faits_depuis(&cs);
        assert_eq!(f.len(), 1, "un nom d'hôte, un nœud");
        assert_eq!(
            f[0].incarnations, 2,
            "les deux incarnations restent comptées"
        );
        assert_eq!(
            f[0].premier_attache_ms, 100,
            "la PREMIÈRE attache est le minimum"
        );
        assert_eq!(
            f[0].derniere_presence_ms, 400,
            "la dernière présence est le maximum"
        );
    }

    /// ── LA PLUS RÉCENTE FAIT AUTORITÉ POUR LES CAPACITÉS ────────────────────
    ///
    /// Une machine relancée SANS `--allow-exec` ne doit pas rester décrite par
    /// l'incarnation d'avant, qui l'autorisait : le graphe dirait alors qu'elle
    /// sait exécuter, et l'échec arriverait après coup — le défaut exact que
    /// « annoncer est décider » interdit.
    #[test]
    fn les_capacites_sont_celles_de_l_incarnation_la_plus_recente() {
        let cs = vec![
            client("vieux", "alice", "n3", &["read", "exec"], 100, 200),
            client("neuf", "alice", "n3", &["read", "list"], 300, 400),
        ];
        let f = faits_depuis(&cs);
        assert_eq!(f.len(), 1);
        assert_eq!(
            f[0].capacites,
            vec!["list".to_string(), "read".to_string()],
            "les capacités retenues sont celles de la PLUS RÉCENTE, pas la réunion des deux"
        );
        assert!(
            !f[0].capacites.iter().any(|c| c == "exec"),
            "l'ancienne autorisation ne doit pas survivre à la nouvelle incarnation"
        );
    }

    /// Deux comptes sur un même nom d'hôte : c'est un cas réel (une machine
    /// partagée), et il est CONSERVÉ — le graphe ne doit pas effacer l'un parce
    /// que l'autre s'est annoncé.
    #[test]
    fn deux_comptes_sur_le_meme_hote_sont_tous_deux_conserves() {
        let cs = vec![
            client("c-a", "alice", "poste-commun", &["read"], 10, 20),
            client("c-b", "bob", "poste-commun", &["read"], 30, 40),
        ];
        let f = faits_depuis(&cs);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].comptes, vec!["alice".to_string(), "bob".to_string()]);
    }

    /// L'ordre est STABLE : deux passes successives écrivent les mêmes nœuds dans
    /// le même ordre, donc un journal de graphe reste comparable.
    #[test]
    fn l_ordre_est_stable_quel_que_soit_l_ordre_d_entree() {
        let cs = vec![
            client("i", "alice", "zulu", &["read"], 1, 1),
            client("j", "alice", "alpha", &["read"], 1, 1),
        ];
        let mut inverses = cs.clone();
        inverses.reverse();
        let noms = |v: &[FaitMachine]| v.iter().map(|f| f.machine.clone()).collect::<Vec<_>>();
        assert_eq!(noms(&faits_depuis(&cs)), noms(&faits_depuis(&inverses)));
        assert_eq!(
            noms(&faits_depuis(&cs)),
            vec!["alpha".to_string(), "zulu".to_string()]
        );
    }

    /// Un registre VIDE ne produit aucun fait — et la passe n'ouvrira donc pas la
    /// base. Le dire est le sujet : « je ne connais aucune machine » est un fait.
    #[test]
    fn un_registre_vide_ne_produit_aucun_fait() {
        assert!(faits_depuis(&[]).is_empty());
    }

    // ── L'AUTO-DÉCLARATION : la ligne, la relecture, l'agrégation ───────────

    /// Une ligne telle que le harness l'écrit (`harness_serve::ligne_declaration`).
    fn ligne(machine: &str, base: &str, store: &str, declaree_ms: i64) -> String {
        serde_json::json!({
            "kind": "machine",
            "machine": machine,
            "instance": format!("machine:{machine}#{base}"),
            "base": base,
            "nom_source": "/proc/sys/kernel/hostname",
            "os": "linux",
            "arch": "x86_64",
            "famille": "unix",
            "store": store,
            "store_racine": "/data",
            "mode": "ro",
            "racine_declaree": "non_declaree",
            "ecoute": "127.0.0.1:8451",
            "pid": 4242,
            "declaree_ms": declaree_ms,
            "version": "0.0.1",
            "commit": "abc",
            "arbre": "propre",
        })
        .to_string()
    }

    /// Le flot de lignes d'un journal, tel que `lire_declarations` le parcourt.
    fn decls(l: &[String]) -> Vec<Declaration> {
        declarations_depuis(l.iter().map(String::as_str))
    }

    /// **LE GENRE SÉPARE DEUX NATURES DE FAITS DANS UN MÊME JOURNAL.** Les
    /// attaches du registre vivent dans le même répertoire : les lire comme des
    /// déclarations ferait naître une machine d'un battement.
    #[test]
    fn une_ligne_du_registre_n_est_pas_une_declaration() {
        let attache = r#"{"kind":"attache","id":"cl-1","compte":"alice","machine":"n3","capacites":["read"],"attache_ms":1,"vue_ms":2}"#;
        // Un battement ne porte même pas de machine : le lire « au mieux » avec
        // des valeurs par défaut inventerait une machine sans nom.
        let battement = r#"{"kind":"battement","id":"cl-1","vue_ms":3}"#;
        assert!(Declaration::depuis_ligne(attache).is_none());
        assert!(Declaration::depuis_ligne(battement).is_none());
        assert!(declarations_depuis([attache, battement]).is_empty());
    }

    /// Une déclaration qui ne NOMME pas sa machine ne désigne rien : elle est
    /// écartée, jamais complétée par des valeurs par défaut. Un identifiant faux
    /// désignerait la mauvaise machine dans une flotte — pire qu'une absence.
    #[test]
    fn une_declaration_sans_machine_est_ecartee() {
        let sans_machine = r#"{"kind":"machine","machine":"","instance":"machine:x"}"#;
        let sans_instance = r#"{"kind":"machine","machine":"n3"}"#;
        assert!(Declaration::depuis_ligne(sans_machine).is_none());
        assert!(Declaration::depuis_ligne(sans_instance).is_none());
        assert!(Declaration::depuis_ligne("").is_none());
        assert!(Declaration::depuis_ligne("pas du JSON").is_none());
    }

    /// **AUCUNE CAPACITÉ N'EST INVENTÉE.** Un moteur seul — aucun `selfd` ne s'est
    /// jamais attaché — entre dans le graphe avec une liste de capacités VIDE.
    /// C'est le point délicat du chantier : une machine qu'on annonce sans savoir
    /// ce qu'elle sait faire doit le DIRE, pas hériter d'un catalogue.
    #[test]
    fn un_moteur_seul_entre_sans_capacite() {
        let fs = faits_avec_declarations(
            &[],
            &decls(&[ligne("n2", "kb.obrain", "/data/kb.obrain", 10)]),
        );
        assert_eq!(fs.len(), 1, "un hôte déclaré est une machine");
        assert!(
            fs[0].capacites.is_empty(),
            "aucune capacité annoncée : {:?}",
            fs[0].capacites
        );
        assert!(
            fs[0].comptes.is_empty(),
            "et aucun compte : un moteur n'appartient à personne"
        );
        assert_eq!(
            fs[0].incarnations, 0,
            "aucune incarnation : rien ne s'est attaché"
        );
        assert_eq!(
            fs[0].premier_attache_ms, 0,
            "aucun fait d'attache — la passe ne l'écrira pas"
        );
        assert_eq!(fs[0].stores_servis(), vec!["/data/kb.obrain".to_string()]);
    }

    /// Une déclaration de moteur n'EFFACE PAS les capacités qu'un `selfd` a
    /// annoncées : ne rien annoncer n'est pas annoncer qu'il n'y a rien. Le
    /// harness ne connaît pas la politique locale du `selfd` — écrire « aucune
    /// capacité » à sa place retirerait un droit vérifié.
    #[test]
    fn une_declaration_n_efface_pas_les_capacites_annoncees_par_un_selfd() {
        let cs = vec![client("cl-1", "alice", "n3", &["read", "exec"], 100, 200)];
        let fs = faits_avec_declarations(
            &cs,
            &decls(&[ligne(
                "n3",
                "runtime-dev.obrain",
                "/data/runtime-dev.obrain",
                300,
            )]),
        );
        assert_eq!(
            fs.len(),
            1,
            "deux sources, un seul nœud : l'agrégation reste par nom d'hôte"
        );
        assert_eq!(
            fs[0].capacites,
            vec!["exec".to_string(), "read".to_string()]
        );
        assert_eq!(fs[0].comptes, vec!["alice".to_string()]);
        assert_eq!(
            fs[0].derniere_presence_ms, 200,
            "le fait d'attache reste intact"
        );
        assert_eq!(
            fs[0].declaration_recente().unwrap().store,
            "/data/runtime-dev.obrain"
        );
    }

    /// Deux instances d'un MÊME hôte : l'agrégation reste par nom d'hôte (un
    /// nœud de premier ordre), mais les DEUX stores servis sont conservés — en
    /// perdre un serait perdre une machine de la flotte.
    #[test]
    fn deux_moteurs_du_meme_hote_donnent_un_noeud_et_deux_stores() {
        let fs = faits_avec_declarations(
            &[],
            &decls(&[
                ligne("n3", "a.obrain", "/data/a.obrain", 10),
                ligne("n3", "b.obrain", "/data/b.obrain", 20),
            ]),
        );
        assert_eq!(fs.len(), 1);
        assert_eq!(
            fs[0].declarations.len(),
            2,
            "les deux instances sont conservées"
        );
        assert_eq!(
            fs[0].stores_servis(),
            vec!["/data/a.obrain".to_string(), "/data/b.obrain".to_string()]
        );
        assert_eq!(
            fs[0].declaration_recente().unwrap().store,
            "/data/b.obrain",
            "la plus récente fait autorité pour ce qui se réannonce"
        );
    }

    /// La relecture : un fichier par STORE servi (`harness_serve.rs`), append-only,
    /// DERNIÈRE LIGNE GAGNANTE par INSTANCE. Un harness redémarré cinquante fois
    /// fait UNE machine, décrite par sa déclaration la plus récente — sinon la
    /// flotte porterait un nœud par redémarrage.
    #[test]
    fn un_harness_redemarre_reste_une_machine_decrite_par_sa_derniere_declaration() {
        let td = tempfile::tempdir().expect("tempdir");
        let kb = ligne("n3", "kb.obrain", "/data/kb.obrain", 10);
        let kb_neuf = ligne("n3", "kb.obrain", "/data/kb.obrain", 99);
        let autre = ligne("n3", "autre.obrain", "/data/autre.obrain", 50);
        std::fs::write(td.path().join("a.jsonl"), format!("{kb}\n{kb_neuf}\n")).expect("journal");
        std::fs::write(td.path().join("b.jsonl"), format!("{autre}\n")).expect("journal");
        // Un `.tmp` n'est PAS un journal (écriture interrompue) : le lire à moitié
        // serait pire que l'ignorer — même règle que le registre.
        std::fs::write(
            td.path().join("c.jsonl.tmp"),
            format!("{}\n", ligne("n9", "x", "/x", 1)),
        )
        .expect("tmp");

        let ds = lire_declarations(td.path());
        assert_eq!(ds.len(), 2, "deux instances, pas trois : {ds:?}");
        let kb = ds.iter().find(|d| d.base == "kb.obrain").expect("kb");
        assert_eq!(kb.declaree_ms, 99, "la dernière ligne fait autorité");
    }

    /// Un répertoire absent n'est pas une erreur : « rien ne s'est déclaré » est
    /// un fait, et il se dit par une liste vide — jamais par une panique.
    #[test]
    fn un_repertoire_de_journal_absent_ne_panique_pas() {
        let td = tempfile::tempdir().expect("tempdir");
        assert!(lire_declarations(&td.path().join("pas-la")).is_empty());
    }
}
