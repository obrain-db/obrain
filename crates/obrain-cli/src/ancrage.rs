//! ANCRAGE — **quel store un sujet désigne-t-il réellement ?**
//!
//! Défaut mesuré le 2026-09-11, reproduit et chiffré : depuis le Cosmos, le chat
//! annonçait « ancré sur `world:this-rs-this--github` » et lisait **en fait**
//! `runtime-dev`. La réponse citait `dsh-runtime`, `session-c254ac35…:msg:7`,
//! `:step:1:1` — les 21 premiers nœuds d'UNE session de runtime-dev, alors que
//! `this-rs/this` contient 1 500 `Function` et 1 175 `Class`.
//!
//! Deux causes cumulées, toutes deux côté vue :
//!
//!   1. le client comparait `focus.world` — un **libellé** (« Runtime dev
//!      (engrams) ») — au `qn` des mondes (`world:runtime-dev-engrams`) : jamais
//!      égal, donc la résolution échouait **à tous les coups** ;
//!   2. le repli était `worlds[0].path` : le chat était donc câblé en dur sur le
//!      **premier monde de la liste**, quel que soit le sujet visé.
//!
//! Une vue ne doit pas interpréter ce qu'elle ne peut pas vérifier. La résolution
//! vit donc ici, et elle s'appuie sur **la base synaptique de l'agent** — la même
//! source de vérité que le Cosmos : `World { qn, label, path }`. Ce qui a été
//! résolu est ensuite **renvoyé à l'écran** (`ancrage` dans l'événement
//! `contexte`) : l'interface ne peut plus afficher autre chose que ce qui a été lu.
//!
//! Ordre de résolution, du plus sûr au plus tolérant :
//!
//!   1. la base synaptique (`World.path`) — c'est l'autorité ;
//!   2. le repli par `slug` sur les mondes connus du serveur — il couvre le cas
//!      d'une base synaptique absente ou pas encore tissée pour ce monde ;
//!   3. aucun ancrage : on le DIT, on ne retombe pas silencieusement sur un store
//!      arbitraire. Un ancrage inventé produit une réponse confiante et fausse ;
//!      c'est exactement le défaut qu'on corrige.

use std::path::{Path, PathBuf};

use obrain_common::types::Value as OV;

use crate::viz_serve::{cached_ro, cached_ro_si_present};

/// Un monde résolu : de quoi ouvrir la bonne base ET le dire à l'écran.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Monde {
    /// `world:<slug>` — l'identité du monde dans la toile.
    pub qn: String,
    /// Nom lisible (« this-rs/this (GitHub) »).
    pub label: String,
    /// Le store réellement ouvrable.
    pub path: PathBuf,
    /// Comment l'ancrage a été trouvé : `synapse` (autorité) ou `slug` (repli).
    /// Exposé parce qu'un repli n'a pas la même valeur de preuve qu'une lecture.
    pub source: &'static str,
}

/// Le handle de la base synaptique : **peek d'abord** (le préchauffage l'a déjà
/// ouverte), ouverture ensuite.
///
/// Le peek seul ne suffit pas : une version intermédiaire s'en contentait, et le
/// self retombait alors sur `self:agent` — c'est-à-dire que la mémoire du chat
/// se serait ancrée sur un AUTRE Self que celui du tissage. Une dégradation
/// silencieuse de l'identité est pire qu'une ouverture un peu lente.
fn lire_synapse(p: &Path) -> Option<std::sync::Arc<obrain_engine::ObrainDB>> {
    cached_ro_si_present(p).or_else(|| cached_ro(p).ok())
}

/// Extrait une chaîne d'une valeur du moteur (tolérant aux variantes).
fn as_str(v: Option<&OV>) -> Option<String> {
    match v? {
        OV::String(s) => Some(s.to_string()),
        OV::Int64(x) => Some(x.to_string()),
        OV::Float64(x) => Some(x.to_string()),
        _ => None,
    }
}

/// Vrai si le sujet désigne un MONDE de la toile plutôt qu'un nœud d'un store.
pub fn est_un_monde(qn: &str) -> bool {
    qn.starts_with("world:")
}

// ── L'ATLAS — la carte des mondes et de leurs synapses ───────────────────────
//
// Pourquoi l'engine en a besoin : le chat ne connaissait qu'UN store. Or
// l'agent est tissé à douze mondes, et **son propre référentiel d'interrogation
// ne vit pas dans le store qu'on regarde** — mesuré : les 29 recettes
// `proc:ref*` sont dans `self-demo.obrain`, et runtime-dev / runtime-prod en ont
// ZÉRO. Sans cette carte, un outil ne peut ni nommer les mondes, ni atteindre le
// référentiel, ni traverser une synapse.
//
// Elle est lue dans la base synaptique (l'autorité du Cosmos), où le tissage
// persiste `World {qn,label,path,nature,nodes,edges}` et
// `(a:World)-[:SYNAPSE]->(b:World)` avec les preuves de la fibre.

/// Un monde, tel que les outils le présentent au modèle.
#[derive(Clone, Debug)]
pub struct MondeAtlas {
    pub qn: String,
    pub label: String,
    pub path: PathBuf,
    /// `connaissance` · `self` · `utilisateur`.
    pub nature: String,
    /// `None` quand le monde n'est pas tissé (ajouté à chaud) : sa volumétrie
    /// n'a pas été calculée. On le dit plutôt que d'afficher un zéro trompeur.
    pub nodes: Option<i64>,
    pub edges: Option<i64>,
    /// LA VOLUMÉTRIE EST-ELLE VRAIE ? `Some(false)` = le compteur annonce des
    /// nœuds mais **aucun enregistrement ne se lit** — le store a perdu ses
    /// zones de données et seuls les compteurs du manifeste ont survécu.
    ///
    /// Défaut mesuré le 2026-09-12 : `linux-full3` annonçait 1 553 071 nœuds et
    /// 4 779 754 arêtes, `kv-linux-driversnet` 310 547 et 769 685 — et
    /// `MATCH (n) RETURN n LIMIT 1` ne rendait **rien** dans les deux. Le Cosmos
    /// les dessinait pourtant comme des mondes massifs (masse 1,000 et 0,887),
    /// et le chat les présentait comme des graphes pleins.
    ///
    /// Une sonde de LECTURE tranche (O(1)) — jamais le compteur, qui est
    /// précisément ce qui ment. `None` = non sondé (store pas encore ouvert).
    pub lisible: Option<bool>,
}

/// Une synapse entre deux mondes, avec les PREUVES qui la fondent.
///
/// Les preuves ne sont jamais confondues : `shared` (même `qualified_name`),
/// `bridged` (sessions réellement exécutées dans ce territoire), `flows`
/// (parenté de structure — la preuve la plus faible), `dated` (synapses datées).
#[derive(Clone, Debug)]
pub struct SynapseAtlas {
    pub a: String,
    pub b: String,
    pub poids: i64,
    pub shared: i64,
    pub bridged: i64,
    pub flows: i64,
    pub dated: i64,
    /// Échantillon des ancres réellement partagées : **les clés de jointure**.
    /// C'est ce qui permet de continuer une interrogation dans l'autre monde au
    /// lieu de le nommer seulement.
    pub cles: Vec<String>,
}

/// La carte complète : les mondes et les fibres qui les relient.
#[derive(Clone, Debug, Default)]
pub struct Atlas {
    pub mondes: Vec<MondeAtlas>,
    pub synapses: Vec<SynapseAtlas>,
    /// LA BASE SYNAPTIQUE A-T-ELLE ÉTÉ LUE ?
    ///
    /// Sans elle, l'atlas ne connaît que les mondes déclarés au serveur : **aucune
    /// volumétrie, aucune fibre, aucun lien**. Défaut constaté : dans cet état, le
    /// rendu disait « NON TISSÉ (volumétrie non calculée) » pour TOUS les mondes —
    /// une phrase qui se lit comme un fait (« ce monde n'est pas tissé ») alors
    /// qu'elle décrit un échec de LECTURE. Un lecteur (ici le modèle) en concluait
    /// que la fiabilité n'était pas exposée, au lieu de comprendre que l'atlas
    /// était amputé.
    pub base_synaptique_lue: bool,
}

impl Atlas {
    /// Retrouve un monde par son qn, son libellé, son slug ou un fragment —
    /// le modèle n'a pas à connaître la forme exacte des identifiants.
    pub fn trouver(&self, demande: &str) -> Option<&MondeAtlas> {
        let d = demande.trim();
        if d.is_empty() {
            return None;
        }
        let cle = cle_canonique(d.trim_start_matches("world:"));
        self.mondes
            .iter()
            .find(|m| m.qn == d || m.label == d)
            .or_else(|| {
                self.mondes
                    .iter()
                    .find(|m| cle_canonique(m.qn.trim_start_matches("world:")) == cle)
            })
            .or_else(|| {
                self.mondes
                    .iter()
                    .find(|m| cle_canonique(&m.label) == cle)
            })
            .or_else(|| {
                // Fragment : « self » doit trouver « Self-demo (engrams) ».
                self.mondes
                    .iter()
                    .find(|m| cle_canonique(&m.label).contains(&cle) && cle.len() >= 3)
            })
    }

    /// Les fibres d'un monde, les plus fortes d'abord.
    pub fn fibres(&self, qn: &str) -> Vec<&SynapseAtlas> {
        let mut v: Vec<&SynapseAtlas> = self
            .synapses
            .iter()
            .filter(|s| s.a == qn || s.b == qn)
            .collect();
        v.sort_by(|x, y| y.poids.cmp(&x.poids));
        v
    }
}

/// Lit la carte des mondes et de leurs synapses dans la base synaptique.
///
/// `mondes` = les mondes connus du serveur (presets + ajoutés à chaud). Ceux qui
/// n'y figurent pas encore dans la base synaptique — donc pas tissés — sont
/// ajoutés avec `nodes: None` : « non tissé » est une information, pas un vide.
pub fn atlas(synapse: Option<&Path>, mondes: &[(String, PathBuf)]) -> Atlas {
    let mut a = Atlas::default();
    let mut lus: Vec<String> = Vec::new();
    if let Some(p) = synapse
        && let Some(db) = lire_synapse(p)
    {
        a.base_synaptique_lue = true;
        if let Ok(r) = db.session().execute(
            "MATCH (w:World) RETURN w.qn, w.label, w.path, w.nature, w.nodes, w.edges",
        ) {
            for row in &r.rows {
                let (Some(qn), Some(label), Some(path)) =
                    (as_str(row.first()), as_str(row.get(1)), as_str(row.get(2)))
                else {
                    continue;
                };
                let path = PathBuf::from(path);
                if !path.exists() {
                    continue;
                }
                lus.push(path.to_string_lossy().to_string());
                a.mondes.push(MondeAtlas {
                    qn,
                    label,
                    path,
                    nature: as_str(row.get(3)).unwrap_or_default(),
                    nodes: row.get(4).map(as_i64),
                    edges: row.get(5).map(as_i64),
                    lisible: None,
                });
            }
        }
        if let Ok(r) = db.session().execute(
            "MATCH (a:World)-[r:SYNAPSE]->(b:World) RETURN a.qn, b.qn, r.weight, \
             r.shared, r.bridged, r.flows, r.dated, r.sample",
        ) {
            for row in &r.rows {
                let (Some(x), Some(y)) = (as_str(row.first()), as_str(row.get(1))) else {
                    continue;
                };
                let cles = as_str(row.get(7))
                    .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
                    .unwrap_or_default();
                a.synapses.push(SynapseAtlas {
                    a: x,
                    b: y,
                    poids: row.get(2).map(as_i64).unwrap_or(0),
                    shared: row.get(3).map(as_i64).unwrap_or(0),
                    bridged: row.get(4).map(as_i64).unwrap_or(0),
                    flows: row.get(5).map(as_i64).unwrap_or(0),
                    dated: row.get(6).map(as_i64).unwrap_or(0),
                    cles,
                });
            }
        }
    }
    // ── LA VOLUMÉTRIE DIT-ELLE VRAI ? ───────────────────────────────────────
    // On sonde les mondes dont le store est DÉJÀ ouvert (le préchauffage s'en
    // charge) : aucun store n'est ouvert ici, l'atlas reste gratuit et ne peut
    // pas bloquer. Un monde dont le compteur annonce des nœuds et dont la sonde
    // ne lit rien est marqué — les lecteurs (outil `obrain_mondes`, chat)
    // cesseront de le présenter comme plein.
    for m in &mut a.mondes {
        let annonce = m.nodes.unwrap_or(0) > 0 || m.edges.unwrap_or(0) > 0;
        if !annonce {
            continue;
        }
        if let Some(db) = crate::viz_serve::cached_ro_si_present(&m.path) {
            let lisible = db
                .session()
                .execute("MATCH (n) RETURN n LIMIT 1")
                .map(|r| !r.rows.is_empty())
                .unwrap_or(false);
            m.lisible = Some(lisible);
        }
    }

    // Les mondes connus du serveur mais PAS dans la base synaptique : non tissés.
    for (label, path) in mondes {
        let p = path.to_string_lossy().to_string();
        if lus.contains(&p) || !path.exists() {
            continue;
        }
        a.mondes.push(MondeAtlas {
            qn: format!("world:{}", crate::viz_weave::slug(label)),
            label: label.clone(),
            path: path.clone(),
            nature: "connaissance".to_string(),
            nodes: None,
            edges: None,
            lisible: None,
        });
    }
    a
}

/// Entier tolérant d'une valeur du moteur (les variantes d'encodage varient
/// d'un store à l'autre : on accepte l'entier, le flottant et la chaîne).
fn as_i64(v: &OV) -> i64 {
    match v {
        OV::Int64(x) => *x,
        OV::Float64(x) => *x as i64,
        OV::String(s) => s.parse().unwrap_or(0),
        _ => 0,
    }
}

/// Clé canonique d'un slug : minuscules, alphanumériques seulement.
///
/// Elle existe parce que le dépôt contenait **deux règles de slug divergentes** :
/// `viz_weave::slug` (qui ÉCRASE les tirets consécutifs) et une règle en ligne
/// dans `api_viz_weave` (qui ne les écrase pas). Pour le même libellé
/// « this-rs/this (GitHub) », la première produit `this-rs-this-github` et la
/// seconde `this-rs-this--github` : **deux qn pour un seul monde**. Constaté à la
/// mesure — le Cosmos envoyait `world:this-rs-this--github`, et la résolution
/// échouait parce qu'elle cherchait la forme de l'autre règle.
///
/// Comparer des clés canoniques rend la résolution **insensible** à cette
/// divergence : un monde qui existe se résout, quelle que soit la règle qui l'a
/// nommé. C'est ce qui rend le correctif robuste aux qn déjà écrits dans la base
/// synaptique comme aux qn que la vue affiche aujourd'hui.
fn cle_canonique(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Résout `world:<slug>` en monde réellement ouvrable.
///
/// `synapse` = base synaptique de l'agent (`--synapse-store`). `mondes` = les
/// mondes connus du serveur (presets + ajoutés à chaud), utilisés en repli.
pub fn monde_du_sujet(
    synapse: Option<&Path>,
    qn: &str,
    mondes: &[(String, PathBuf)],
) -> Option<Monde> {
    if !est_un_monde(qn) {
        return None;
    }
    let cle = cle_canonique(qn.trim_start_matches("world:"));

    // 1 · L'AUTORITÉ : la base synaptique porte le chemin que le Cosmos affiche.
    //     On lit TOUS les mondes d'un coup (6 à 12 lignes, ~2 ms) : cela permet
    //     d'accepter aussi bien le qn exact que sa forme canonique, donc de
    //     résoudre un monde tissé quel que soit le slug que la vue a envoyé.
    if let Some(p) = synapse
        && let Some(db) = lire_synapse(p)
        && let Ok(r) = db
            .session()
            .execute("MATCH (w:World) RETURN w.qn, w.label, w.path")
    {
        let mut canonique: Option<Monde> = None;
        for row in &r.rows {
            let (Some(ws_qn), Some(label), Some(path)) =
                (as_str(row.first()), as_str(row.get(1)), as_str(row.get(2)))
            else {
                continue;
            };
            let path = PathBuf::from(path);
            if !path.exists() {
                continue;
            }
            if ws_qn == qn {
                return Some(Monde { qn: ws_qn, label, path, source: "synapse" });
            }
            if canonique.is_none() && cle_canonique(ws_qn.trim_start_matches("world:")) == cle {
                canonique = Some(Monde {
                    qn: ws_qn.clone(),
                    label,
                    path,
                    source: "synapse-slug",
                });
            }
        }
        if canonique.is_some() {
            return canonique;
        }
    }

    // 2 · LE REPLI : un monde **ajouté à chaud n'est pas tissé**, donc absent de
    //     la base synaptique — et c'est justement le cas où l'utilisateur vient
    //     de cliquer dessus. On le retrouve par la clé canonique de son libellé,
    //     puis par le nom de son store (dernier recours, pour un libellé renommé).
    for (label, path) in mondes {
        if cle_canonique(label) == cle && path.exists() {
            return Some(Monde {
                qn: qn.to_string(),
                label: label.clone(),
                path: path.clone(),
                source: "slug",
            });
        }
    }
    let par_fichier = mondes.iter().find(|(_, path)| {
        path.file_stem()
            .and_then(|s| s.to_str())
            .map(cle_canonique)
            .map(|c| c == cle || c.contains(&cle) || cle.contains(&c))
            .unwrap_or(false)
            && path.exists()
    });
    par_fichier.map(|(label, path)| Monde {
        qn: qn.to_string(),
        label: label.clone(),
        path: path.clone(),
        source: "fichier",
    })
}

/// Le Self de l'agent : lu dans la base synaptique **si elle est déjà ouverte**,
/// sinon la variable d'environnement, sinon le défaut historique.
///
/// On ne l'OUVRE pas ici : l'ouverture peut bloquer quand un autre processus
/// tient la base en écriture, et le self n'est pas une raison de figer une
/// conversation (voir `viz_serve::cached_ro_si_present`).
///
/// Un seul Self pour un seul agent : cette fonction existe parce que l'identité
/// divergeait — la toile et `self-demo` portent `self:triviere-dsh`, l'ancien
/// chemin MCP retombait sur `self:agent`, et la documentation citait
/// `self:agent:state:v72`. Trois identités, c'est trois mémoires séparées.
pub fn self_qn(synapse: Option<&Path>) -> String {
    if let Some(p) = synapse
        && let Some(db) = lire_synapse(p)
        && let Ok(r) = db
            .session()
            .execute("MATCH (s:Self) RETURN s.qn LIMIT 1")
        && let Some(v) = r.rows.first().and_then(|row| as_str(row.first()))
        && !v.is_empty()
    {
        return v;
    }
    std::env::var("OBRAIN_SELF_QN").unwrap_or_else(|_| "self:agent".to_string())
}

/// ── LE SELF D'UNE IDENTITÉ — « chacun le sien » ─────────────────────────────
///
/// Défaut mesuré le 2026-09-15 : `self_qn` rend LE Self de l'instance
/// (`MATCH (s:Self) RETURN s.qn LIMIT 1`). Tant qu'une seule personne parlait au
/// serveur, c'était juste. Dès qu'il y en a plusieurs, ce `LIMIT 1` attribue à
/// tout le monde la mémoire d'une seule — et un ami qui ouvre son compte voit
/// les souvenirs de quelqu'un d'autre, ce qui est pire que de n'en avoir aucun.
///
/// ── CE QUI PORTE LA MÉMOIRE EST LE JOURNAL, PAS LE NŒUD ─────────────────────
/// Première version de cette fonction : elle ne rendait `self:<utilisateur>` que
/// si un nœud `Self` portait déjà ce nom. C'était trop prudent, et ça manquait
/// la cible : **la mémoire d'une conversation vit dans le journal du self**
/// (`crate::memoire::journal(self_qn)`, un fichier par self), que le nœud existe
/// ou non. Exiger le nœud revenait donc à laisser chaque ami écrire dans le
/// journal de l'instance — exactement ce qu'on veut empêcher — jusqu'à ce que
/// quelqu'un ait provisionné un nœud par un geste hors bande.
///
/// Le nœud `Self`, lui, est créé là où il sert : à l'APPLICATION de la mémoire
/// dans le graphe (`memoire::appliquer`), qui est le seul moment où les
/// souvenirs rejoignent la base. Entre-temps ils ne sont pas orphelins : ils
/// sont dans le journal de LEUR self, lisible par `/api/engine/memoire`.
///
/// ── DEUX PERSONNES, UN MÊME NOM D'UTILISATEUR (corrigé le 2026-09-16) ───────
/// Le commentaire de la première version disait déjà « deux personnes peuvent
/// porter le même nom d'utilisateur sur deux comptes, et leurs mémoires ne
/// doivent pas se rejoindre ». Le code faisait le contraire : il coupait
/// l'identité au `@` et rendait `self:<utilisateur>` pour `alice@compteA` COMME
/// pour `alice@compteB`. Deux journaux, donc ? Non : un seul — `memoire::journal`
/// est une fonction du nom du self — et une seule base à l'application
/// (`selfs/<utilisateur>.obrain`, voir `scripts/memoire-appliquer-comptes.sh`).
/// La propriété était donc affirmée, jamais établie. Elle l'est maintenant.
///
/// Ce qui décide du nom, et pourquoi :
///
///   · `utilisateur@compte` → **`self:<utilisateur>@<compte>`** dès que le compte
///     n'est pas un compte historique. Le nom porte les DEUX parties, et la
///     coupe est INJECTIVE : la grammaire des noms d'`obrain-iam`
///     (`admin.rs::valider_nom` : minuscules, chiffres, `.`, `_`, `-`) interdit
///     le `@` dans chacune des deux parties, donc un seul découpage est possible
///     et deux comptes ne peuvent plus converger sur le même self ;
///
///   · `<utilisateur>` (nom nu, sans compte) → **`self:<utilisateur>`**, forme
///     INCHANGÉE : c'est le nom que passent les appelants qui n'ont pas de compte
///     sous la main (`POST /api/remote/memoire`, gelé ici : c'est le chemin en
///     service), et un nom nu n'a jamais porté de compte — on n'en invente pas ;
///
///   · `utilisateur@<compte historique>` → **`self:<utilisateur>`**, la forme
///     historique. C'est l'ARBITRAGE de compatibilité, et il est mesuré : sur la
///     ligne déployée le 2026-09-16, `OBRAIN_IAM_ACCOUNT=obrain` et les bases
///     `selfs/{achaptal,admin,triviere}.obrain` portent du contenu RÉEL sous
///     `Self {qn: 'self:<utilisateur>'}`. Renommer ces selves pour « faire plus
///     propre » orphelinerait cette mémoire : le journal est indexé par le nom,
///     et la base appliquée s'appelle `selfs/<nom>.obrain`. Un nom ne se change
///     pas sous les pieds de gens qui l'utilisent déjà.
///
/// ── CE QUE CETTE FONCTION NE PEUT PAS PROUVER SEULE, ET QUI EST ÉCRIT ───────
/// Deux appelants de `viz_serve` retirent le `@` AVANT d'appeler
/// (`api_engine_memoire`, et `api_engine_chat` qui recâble `self:<u>`) : sur une
/// instance dont le compte n'est PAS historique, ils nomment un self nu pendant
/// que la résonance nomme le self qualifié. Tant que ces appels ne passent pas
/// l'identité entière, la séparation n'est donc établie que pour les appelants
/// qui passent la forme documentée (`Garde::identite`). Le corriger demande
/// d'éditer `viz_serve.rs`, hors de ce chantier.
///
/// Rend `(qn, portee)` où `portee` dit ce que la base portait :
///   · `"compte"`  — un Self propre à cette identité existe déjà dans la base ;
///   · `"journal"` — l'identité a SON self et SON journal, mais la base n'a pas
///                   encore de nœud `Self` : il naîtra à l'application ;
///   · `"instance"`— aucune identité : le Self de l'instance (comportement
///                   historique, inchangé).
#[must_use]
pub fn self_qn_pour(synapse: Option<&Path>, identite: Option<&str>) -> (String, &'static str) {
    let universel = self_qn(synapse);
    let Some(identite) = identite.map(str::trim).filter(|i| !i.is_empty()) else {
        return (universel, "instance");
    };
    let (utilisateur, compte) = decouper_identite(identite);
    if utilisateur.is_empty() {
        return (universel, "instance");
    }
    let candidat = nom_du_self(utilisateur, compte);
    let existe = synapse
        .and_then(lire_synapse)
        .and_then(|db| {
            db.session()
                .execute(&format!(
                    "MATCH (s:Self) WHERE s.qn = '{candidat}' RETURN s.qn LIMIT 1"
                ))
                .ok()
        })
        .and_then(|r| r.rows.first().and_then(|row| as_str(row.first())))
        .is_some_and(|v| !v.is_empty());
    if existe {
        (candidat, "compte")
    } else {
        // LE SELF EST À L'IDENTITÉ MÊME SANS NŒUD : son journal l'attend, et le
        // nœud naîtra à l'application de la mémoire. `portee` le dit pour que
        // l'écran puisse nommer ce qui n'est pas encore dans le graphe.
        (candidat, "journal")
    }
}

/// Les comptes sous lesquels des gens ont DÉJÀ une mémoire nommée `self:<nom>`.
///
/// ── POURQUOI UNE LISTE, ET POURQUOI ELLE EST EN DUR ─────────────────────────
/// Ce n'est pas une préférence, c'est une MESURE : sur la ligne déployée le
/// 2026-09-16, `OBRAIN_IAM_ACCOUNT=obrain`, et trois personnes ont du contenu
/// réel dans `selfs/{achaptal,admin,triviere}.obrain` sous
/// `Self {qn: 'self:<utilisateur>'}`. Le journal (`memoire::journal`) et la base
/// appliquée (`selfs/<nom>.obrain`) sont tous deux indexés par ce nom : changer
/// le nom d'un self, c'est perdre la mémoire de quelqu'un qui ne l'a pas
/// demandé. Ces comptes gardent donc le nom NU.
///
/// C'est le POINT DE MIGRATION, et il est volontairement visible : y ajouter un
/// compte, c'est accepter que ses utilisateurs conservent le nom nu (et donc
/// qu'ils se rejoignent avec les utilisateurs de même nom des autres comptes de
/// la liste) ; l'en retirer, c'est couper en deux le journal et la base de ses
/// utilisateurs — un geste qui exige une migration de noms, pas une édition.
const COMPTES_A_SELF_HISTORIQUE: &[&str] = &["obrain"];

/// Découpe `utilisateur@compte` en ses deux parties (le compte peut être vide).
///
/// `Garde::identite` rend le `Display` d'`Identity` : `utilisateur@compte`. La
/// coupe est SÛRE — et c'est ce qui rend le nom du self injectif — parce que la
/// grammaire des noms d'`obrain-iam` interdit le `@` dans l'utilisateur comme
/// dans le compte : il n'y a donc qu'un seul découpage possible.
fn decouper_identite(identite: &str) -> (&str, &str) {
    match identite.split_once('@') {
        Some((utilisateur, compte)) => (utilisateur.trim(), compte.trim()),
        // Pas d'arobase : c'est un nom NU (le compte n'est pas dans la chaîne).
        None => (identite.trim(), ""),
    }
}

/// Le nom du self d'une personne — la règle, en un endroit.
fn nom_du_self(utilisateur: &str, compte: &str) -> String {
    if compte.is_empty() || COMPTES_A_SELF_HISTORIQUE.contains(&compte) {
        format!("self:{utilisateur}")
    } else {
        format!("self:{utilisateur}@{compte}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_prefix_detected() {
        assert!(est_un_monde("world:linux-complet-v6-6"));
        assert!(!est_un_monde("proc:self.boot"));
        assert!(!est_un_monde(""));
    }

    #[test]
    fn cle_canonique_ignore_la_ponctuation() {
        assert_eq!(cle_canonique("this-rs-this-github"), "thisrsthisgithub");
        assert_eq!(cle_canonique("this-rs-this--github"), "thisrsthisgithub");
        assert_eq!(cle_canonique("Linux complet (v6.6)"), "linuxcompletv66");
    }

    /// LE CAS DU DÉFAUT MESURÉ. Le Cosmos envoyait `world:this-rs-this--github`
    /// (règle de slug sans écrasement) alors que le libellé donne
    /// `this-rs-this-github` (règle du tissage) : la résolution échouait, et le
    /// chat retombait sur `worlds[0]` — runtime-dev. Les deux formes doivent
    /// désormais résoudre vers le MÊME store.
    #[test]
    fn les_deux_formes_de_slug_resolvent_le_meme_monde() {
        let tmp = std::env::temp_dir().join("ancrage-slug-store.obrain");
        std::fs::write(&tmp, b"x").unwrap();
        let mondes = vec![("this-rs/this (GitHub)".to_string(), tmp.clone())];

        for qn in ["world:this-rs-this--github", "world:this-rs-this-github"] {
            let m = monde_du_sujet(None, qn, &mondes)
                .unwrap_or_else(|| panic!("{qn} doit résoudre"));
            assert_eq!(m.path, tmp, "{qn}");
            assert_eq!(m.label, "this-rs/this (GitHub)", "{qn}");
        }
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn non_world_subject_has_no_monde() {
        assert_eq!(monde_du_sujet(None, "proc:self.boot", &[]), None);
    }

    /// Un monde inconnu ne doit PAS retomber sur un store arbitraire : l'absence
    /// d'ancrage est une information, un ancrage inventé est un mensonge. C'est
    /// exactement le défaut corrigé (`worlds[0]` en repli).
    #[test]
    fn monde_inconnu_ne_retombe_pas_sur_un_autre() {
        let mondes = vec![
            ("Runtime dev (engrams)".to_string(), PathBuf::from("/inexistant")),
            ("Linux complet (v6.6)".to_string(), PathBuf::from("/inexistant2")),
        ];
        assert_eq!(monde_du_sujet(None, "world:monde-inexistant", &mondes), None);
    }

    /// Un monde connu mais dont le store a disparu du disque ne résout pas non
    /// plus : mieux vaut dire « je ne peux pas ouvrir » que servir un autre monde.
    #[test]
    fn store_absent_du_disque_ne_resout_pas() {
        let mondes = vec![("this-rs/this (GitHub)".to_string(), PathBuf::from("/n-existe-pas"))];
        assert_eq!(monde_du_sujet(None, "world:this-rs-this--github", &mondes), None);
    }

    #[test]
    fn self_qn_repli_environnement() {
        // Sans base synaptique : la variable d'environnement, sinon le défaut.
        let attendu = std::env::var("OBRAIN_SELF_QN").unwrap_or_else(|_| "self:agent".to_string());
        assert_eq!(self_qn(None), attendu);
    }

    /// ── CHACUN SON SELF : LA PROPRIÉTÉ QUI COMPTE ───────────────────────────
    ///
    /// Sans identité, on garde le self de l'instance (rien ne change pour
    /// l'agent seul). Avec une identité, on obtient le self de LA PERSONNE — et
    /// deux personnes ne le partagent pas, quel que soit leur compte.
    ///
    /// ── CE QUE LA VERSION PRÉCÉDENTE AFFIRMAIT, ET QUI ÉTAIT FAUX ───────────
    /// Elle asserait, sous le commentaire « le compte ne fait pas partie du nom :
    /// c'est la PERSONNE qui a un self » :
    ///     assert_eq!(self_qn_pour(None, Some("alice@autre-compte")).0, "self:alice");
    /// — c'est-à-dire exactement le défaut que le commentaire de la fonction
    /// interdisait depuis le premier jour (« deux personnes peuvent porter le
    /// même nom d'utilisateur sur deux comptes, et leurs mémoires ne doivent pas
    /// se rejoindre »), et que `memoire::journal` traduisait en UN SEUL fichier
    /// pour les deux. Le test n'est pas renommé pour passer : son assertion est
    /// corrigée, et la propriété complète est figée juste en dessous.
    #[test]
    fn self_par_identite_est_nomme_par_l_utilisateur() {
        let universel = self_qn(None);
        // Aucune identité → self de l'instance, portée annoncée.
        assert_eq!(self_qn_pour(None, None), (universel.clone(), "instance"));
        assert_eq!(self_qn_pour(None, Some("   ")), (universel.clone(), "instance"));
        // Une identité du compte HISTORIQUE → la forme historique : c'est le nom
        // sous lequel la ligne déployée porte déjà sa mémoire (`selfs/*.obrain`),
        // il ne change pas. Sans base synaptique lisible, la portée dit que le
        // nœud n'existe pas encore : les souvenirs attendent au journal.
        let (qn, portee) = self_qn_pour(None, Some("alice@obrain"));
        assert_eq!(qn, "self:alice");
        assert_eq!(portee, "journal");
        // Le nom NU (sans compte) est inchangé : c'est le chemin en service
        // (`POST /api/remote/memoire`), on ne lui invente pas un compte.
        assert_eq!(self_qn_pour(None, Some("alice")), ("self:alice".to_string(), "journal"));
        // Deux personnes, deux selfs — la séparation est la raison d'être.
        let (a, _) = self_qn_pour(None, Some("alice@obrain"));
        let (b, _) = self_qn_pour(None, Some("bob@obrain"));
        assert_ne!(a, b, "deux personnes du même compte ne partagent jamais un self");
    }

    /// ── DEUX COMPTES, UN MÊME NOM D'UTILISATEUR : DEUX SELF ─────────────────
    ///
    /// Le défaut latent : `alice@compteA` et `alice@compteB` rendaient le MÊME
    /// candidat `self:alice`, donc le même journal et la même base à
    /// l'application. Le nom du self porte désormais les deux parties, et la
    /// coupe est injective parce que le `@` est interdit dans chacune.
    #[test]
    fn deux_comptes_de_meme_utilisateur_ne_partagent_pas_de_self() {
        let (a, pa) = self_qn_pour(None, Some("alice@compteA"));
        let (b, pb) = self_qn_pour(None, Some("alice@compteB"));
        assert_eq!(a, "self:alice@compteA");
        assert_eq!(b, "self:alice@compteB");
        assert_ne!(a, b, "deux comptes ne doivent JAMAIS converger sur le même self");
        assert_eq!(pa, "journal");
        assert_eq!(pb, "journal");
        // Le nom se relit : on retrouve les deux parties, sans ambiguïté — c'est
        // l'injectivité, vue de l'autre bout.
        assert_eq!(decouper_identite(a.trim_start_matches("self:")), ("alice", "compteA"));
        assert_eq!(decouper_identite(b.trim_start_matches("self:")), ("alice", "compteB"));
        // La grammaire d'`obrain-iam` n'accepte que des minuscules : la forme
        // réelle d'un compte est plutôt `compte-a`. Les deux doivent séparer.
        assert_eq!(self_qn_pour(None, Some("alice@compte-a")).0, "self:alice@compte-a");
        assert_ne!(self_qn_pour(None, Some("alice@compte-a")).0, a);
        // Un même utilisateur sur trois comptes : trois selfs, deux à deux distincts.
        let (c, _) = self_qn_pour(None, Some("alice@compteC"));
        assert_ne!(a, c);
        assert_ne!(b, c);
        // Le compte historique ne prend PAS le nom qualifié : sa mémoire existe
        // déjà sous le nom nu (c'est l'arbitrage de compatibilité, mesuré).
        assert_eq!(self_qn_pour(None, Some("alice@obrain")).0, "self:alice");
        // Et la stabilité : le même appel, cent fois, rend le même nom (aucun
        // état, aucune horloge, aucun aléa dans cette décision).
        for _ in 0..100 {
            assert_eq!(self_qn_pour(None, Some("alice@compteA")).0, a);
        }
    }
}
