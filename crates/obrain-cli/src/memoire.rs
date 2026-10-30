//! MÉMOIRE DU CHAT — ce que la conversation STRUCTURE, et comment ça survit.
//!
//! ## Le défaut que ce module corrige
//!
//! Jusqu'ici `demander_stream` n'écrivait **que** le journal d'activation : une
//! conversation ne produisait ni note, ni objectif, ni état. L'agent ne se
//! structurait pas et ne conservait rien — alors que les mécanismes existent
//! (`self/consolidate`, `SelfState` versionné, `proc:self.remember`).
//!
//! ## Pourquoi un JOURNAL, et pas une écriture directe
//!
//! La contrainte est explicite dans le substrat (`file.rs`) : une base ouverte
//! en lecture prend un `flock` **partagé**, une écriture exige le **exclusif**, et
//! le registre refuse la promotion (« already open read-only in this process —
//! cannot upgrade to an exclusive (write) lock »). Le serveur du dashboard tient
//! ses bases en lecture : il ne peut donc **pas** écrire dedans, et ce n'est pas
//! un défaut à contourner — c'est l'invariant mono-écrivain qui protège les
//! données.
//!
//! D'où deux temps, comme le tissage (`scripts/weave.sh` arrête, écrit, relance) :
//!
//! 1. **le chat JOURNALISE** — un fichier append-only, un par Self, écrit sans
//!    aucun verrou. Durable immédiatement : l'intention survit à un arrêt, à un
//!    crash, à un redémarrage. C'est la doctrine exacte du journal d'activation ;
//! 2. **une passe APPLIQUE** (`obrain memoire --appliquer`) — elle ouvre la base
//!    du Self en EXCLUSIF, écrit en `MERGE` idempotent, et **consomme** ce qu'elle
//!    a appliqué.
//!
//! ## Pourquoi c'est sûr
//!
//! - rien n'est écrit dans les grosses bases (runtime-dev, linux) : la cible est
//!   la base du Self, et **seulement** par `MERGE` ;
//! - appliquer deux fois ne duplique rien (les nœuds sont identifiés par `qn`) ;
//!   la seule écriture NON idempotente est le `SelfState`, qui est **versionné**
//!   par construction — et c'est précisément pourquoi le journal est *consommé* ;
//! - une entrée refusée reste dans le journal : on ne perd pas une intention
//!   parce qu'une requête a échoué.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Ce que la conversation veut structurellement retenir.
///
/// Volontairement pauvre : quatre gestes, chacun correspondant à une entité qui
/// EXISTE déjà dans la base du Self (`Note`, `Goal`, `Procedure`, `SelfState`).
/// Inventer de nouvelles formes ferait diverger la mémoire du modèle de celle
/// qui est déjà tissée.
#[derive(Clone, Debug, PartialEq)]
pub enum Intention {
    /// « retiens ceci » — une note, la plus petite unité de mémoire.
    Note { texte: String, sujet: String },
    /// « tel est l'objectif » — un but, rattaché au Self par `HAS_GOAL`.
    Objectif { nom: String, texte: String },
    /// « voici comment interroger » — une recette du référentiel (auto-structuration).
    Procedure { nom: String, description: String, corps: String },
    /// « voilà où j'en suis » — un état daté et VERSIONNÉ du Self.
    EtatSelf {
        epithet: String,
        objectif: String,
        prochaine_action: String,
        note: String,
    },
}

impl Intention {
    pub fn genre(&self) -> &'static str {
        match self {
            Intention::Note { .. } => "note",
            Intention::Objectif { .. } => "objectif",
            Intention::Procedure { .. } => "procedure",
            Intention::EtatSelf { .. } => "etat-self",
        }
    }

    /// Référence lisible de ce qui a été retenu (pour l'affichage et le rapport).
    pub fn reference(&self) -> String {
        match self {
            Intention::Note { texte, .. } => texte.chars().take(60).collect(),
            Intention::Objectif { nom, .. } => nom.clone(),
            Intention::Procedure { nom, .. } => nom.clone(),
            Intention::EtatSelf { objectif, .. } => objectif.chars().take(60).collect(),
        }
    }

    fn vers_json(&self) -> serde_json::Value {
        match self {
            Intention::Note { texte, sujet } => {
                serde_json::json!({ "kind": "note", "texte": texte, "sujet": sujet })
            }
            Intention::Objectif { nom, texte } => {
                serde_json::json!({ "kind": "objectif", "nom": nom, "texte": texte })
            }
            Intention::Procedure { nom, description, corps } => serde_json::json!({
                "kind": "procedure", "nom": nom, "description": description, "corps": corps
            }),
            Intention::EtatSelf { epithet, objectif, prochaine_action, note } => serde_json::json!({
                "kind": "etat-self", "epithet": epithet, "objectif": objectif,
                "prochaine_action": prochaine_action, "note": note
            }),
        }
    }

    fn depuis_json(v: &serde_json::Value) -> Option<Self> {
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        Some(match v.get("kind")?.as_str()? {
            "note" => Intention::Note { texte: s("texte"), sujet: s("sujet") },
            "objectif" => Intention::Objectif { nom: s("nom"), texte: s("texte") },
            "procedure" => Intention::Procedure {
                nom: s("nom"),
                description: s("description"),
                corps: s("corps"),
            },
            "etat-self" => Intention::EtatSelf {
                epithet: s("epithet"),
                objectif: s("objectif"),
                prochaine_action: s("prochaine_action"),
                note: s("note"),
            },
            _ => return None,
        })
    }
}

/// Une entrée du journal : l'intention, son contexte, sa date.
#[derive(Clone, Debug)]
pub struct Entree {
    pub t_ms: i64,
    pub self_qn: String,
    /// La base et le sujet d'où l'intention vient — une mémoire sans provenance
    /// n'est pas vérifiable.
    pub provenance: String,
    pub intention: Intention,
}

impl Entree {
    fn vers_json(&self) -> serde_json::Value {
        serde_json::json!({
            "t": self.t_ms,
            "self": self.self_qn,
            "provenance": self.provenance,
            "intention": self.intention.vers_json(),
        })
    }

    fn depuis_json(v: &serde_json::Value) -> Option<Self> {
        Some(Entree {
            t_ms: v.get("t").and_then(|x| x.as_i64()).unwrap_or(0),
            self_qn: v.get("self").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            provenance: v.get("provenance").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            intention: Intention::depuis_json(v.get("intention")?)?,
        })
    }
}

/// Le cache où vivent le journal et le témoin (le même que l'activation).
fn racine() -> PathBuf {
    std::env::var_os("OBRAIN_VIZ_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/obrain-viz-cache"))
}

/// Répertoire du journal (le même cache que l'activation et les agrégats).
fn dir() -> PathBuf {
    racine().join("memoire")
}

/// ── LE TÉMOIN : CE QUI RESTE APRÈS QUE LE JOURNAL A ÉTÉ CONSOMMÉ ────────────
///
/// ## Le défaut que ce témoin corrige (mesuré le 2026-09-16)
///
/// Le journal est CONSOMMÉ par la passe : `appliquer` réécrit le fichier sans
/// les entrées appliquées, et le SUPPRIME quand plus rien n'attend. C'est
/// délibéré (`SelfState` est versionné, donc non idempotent : une intention ne
/// doit être appliquée qu'une fois), mais la conséquence n'avait pas été vue :
/// **une fois la mémoire appliquée, plus rien ne dit qu'elle l'a été.**
///
/// Le rapport du 2026-09-16 s'est donc appuyé sur une ABSENCE — « depuis le
/// 16/09 00:02:03 plus rien n'a été produit », déduit du `mtime` du répertoire
/// et d'un répertoire vide — et il a fallu six cents lignes de mesures pour
/// établir que cette absence voulait dire « en repos » et non « cassé ». C'est
/// exactement le mode de défaillance que la doctrine du dépôt interdit : une
/// garde qui tombe en silence ne garde rien, et un état normal qu'on ne peut pas
/// distinguer d'une panne n'est pas un état mesuré.
///
/// ## Ce qu'il est, et ce qu'il n'est pas
///
/// Un registre **append-only** de deux gestes, et de deux seulement :
///
///   · `journalise` — l'instant où une intention est entrée dans la mémoire
///     (c'est l'appel de l'outil `obrain_memoriser` / `obrain_structurer`, ou la
///     route d'un harness distant : le SEUL chemin qui produit de la mémoire) ;
///   · `applique` — l'instant où une passe a fait avancer une base de compte.
///
/// Il répond donc, sur une période quelconque, à la seule question du chantier :
/// « le self se développe-t-il, et depuis quand ne s'est-il rien passé ? »
/// — avec un NOMBRE et une DATE, là où il n'y avait qu'un fichier vide.
///
/// Il ne remplace rien : le journal reste la source de vérité de ce qui attend,
/// et le témoin n'est jamais relu pour décider quoi que ce soit. C'est un
/// instrument de mesure, pas un état — le confondre avec le journal ferait
/// rejouer des intentions.
///
/// ## Pourquoi AILLEURS que dans `dir()`
///
/// Le répertoire `memoire/` est PARCOURU pour découvrir ce qui attend : la passe
/// lit chaque `*.jsonl` de ce répertoire, et `lire_catalogue` en fait autant. Y
/// déposer un registre qui n'est pas un journal de self (ses lignes ne portent
/// pas d'`intention`) ferait compter un fichier de plus par la passe et
/// attribuerait à personne des entrées qui n'en sont pas — deux noms pour une
/// chose, l'écart exact que l'en-tête de `scripts/memoire-appliquer-comptes.sh`
/// raconte à propos de `self-…` contre `selfs/…`. D'où `memoire-temoins.jsonl`,
/// FRÈRE du répertoire et non son contenu (un test le fixe).
///
/// ## Pourquoi c'est sûr
///
/// Même contrat que le journal : **best-effort assumé**, aucun verrou (journal
/// append-only, on en est le seul écrivain), et un échec d'écriture ne fait
/// JAMAIS échouer une conversation — perdre un témoin est regrettable, refuser
/// une mémoire parce qu'on n'a pas pu la mesurer serait pire.
pub fn chemin_temoins() -> PathBuf {
    racine().join("memoire-temoins.jsonl")
}

/// Une ligne du témoin. Les champs sont volontairement plats et lisibles : le
/// lecteur est un script (`scripts/memoire-appliquer-comptes.sh`, en `python3`),
/// pas ce module — le témoin sert à MESURER, et une mesure qu'on ne peut pas
/// lire avec les outils du bord ne mesure rien.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Temoin {
    /// Millisecondes epoch, comme partout dans le dépôt (`activation::now_ms`).
    pub t: i64,
    /// Le self dont la mémoire a bougé (`self:<compte>`).
    #[serde(rename = "self")]
    pub self_qn: String,
    /// `journalise` ou `applique` — les deux seuls gestes qui font grandir.
    pub geste: String,
    /// Le genre de l'intention (`note`, `objectif`, `procedure`, `etat-self`),
    /// ou `passe` pour une application.
    pub genre: String,
    /// Ce qui a été retenu, ou le compte rendu de la passe.
    pub reference: String,
    /// La base écrite — vide pour une journalisation (rien n'est encore écrit).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub base: String,
}

/// Les deux gestes, nommés une fois : ce sont des CONSTANTES de contrat entre
/// l'écrivain (ici) et le lecteur (le script), pas des chaînes à retaper.
pub const GESTE_JOURNALISE: &str = "journalise";
pub const GESTE_APPLIQUE: &str = "applique";

impl Temoin {
    fn journalisation(e: &Entree) -> Self {
        Temoin {
            t: e.t_ms,
            self_qn: e.self_qn.clone(),
            geste: GESTE_JOURNALISE.to_string(),
            genre: e.intention.genre().to_string(),
            reference: e.intention.reference(),
            base: String::new(),
        }
    }

    fn application(t_ms: i64, self_qn: &str, rapport: &Rapport) -> Self {
        Temoin {
            t: t_ms,
            self_qn: self_qn.to_string(),
            geste: GESTE_APPLIQUE.to_string(),
            genre: "passe".to_string(),
            reference: format!(
                "{} appliquée(s), {} refusée(s)",
                rapport.appliquees,
                rapport.refusees.len()
            ),
            base: rapport.store.clone(),
        }
    }

    /// La ligne, **fonction pure** : elle se prouve sans store ni variable
    /// d'environnement (ce module ne peut pas poser `OBRAIN_VIZ_CACHE_DIR` dans
    /// un test — `std::env::set_var` exige `unsafe`, et le crate le refuse).
    pub fn ligne(&self) -> Result<String> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Écrit une ligne de témoin. **Best-effort** : voir la doctrine plus haut —
/// l'appelant ne doit jamais faire échouer son geste pour un témoin manquant.
fn temoigner(t: &Temoin) {
    let Ok(ligne) = t.ligne() else { return };
    let chemin = chemin_temoins();
    if let Some(d) = chemin.parent()
        && std::fs::create_dir_all(d).is_err()
    {
        return;
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&chemin) {
        use std::io::Write;
        let _ = writeln!(f, "{ligne}");
        let _ = f.flush();
    }
}

/// ── QUI EXISTE : LE CATALOGUE DES SELFS RÉSIDENTS ───────────────────────────
///
/// Défaut comblé le 2026-09-15, en ouvrant le chantier de la résonance : une
/// fois qu'il y a plusieurs selfs, RIEN ne dit lesquels existent. Le catalogue
/// n'est pas un registre à tenir — ce serait une seconde vérité à garder
/// d'accord avec la première — mais une LECTURE de ce qui est déjà là :
///
///   · les **journaux** de mémoire (`<cache>/memoire/*.jsonl`) : chacun porte le
///     `self` de ses entrées, donc on n'a pas à inverser le hachage du nom de
///     fichier pour savoir à qui il appartient ;
///   · le **registre des mondes** : il porte le propriétaire de chaque monde
///     ajouté, donc « ce self a apporté ce monde » est déjà écrit ;
///   · la **base synaptique**, quand elle est lisible : elle dit quels selfs ont
///     un nœud `Self` (donc une mémoire déjà appliquée dans le graphe).
///
/// Ce que ça ne fait PAS, volontairement : ça n'ouvre aucune base de données et
/// ça n'écrit rien. Un catalogue qui RELIERAIT les selfs serait déjà la
/// résonance — et la résonance doit être un geste, pas l'effet de bord d'un
/// affichage.
#[derive(Clone, Debug, serde::Serialize)]
pub struct SelfResident {
    /// Le `qn` du self (`self:<utilisateur>`).
    pub self_qn: String,
    /// Le journal qui porte sa mémoire, s'il en a un.
    pub journal: Option<String>,
    /// Le nombre d'intentions enregistrées, et la plus récente.
    pub entrees: usize,
    pub derniere_ms: Option<i64>,
    /// Un nœud `Self` de ce nom existe-t-il dans la base synaptique lue ?
    /// `false` ne veut pas dire « pas de self » : la mémoire attend au journal.
    pub noeud_dans_synapse: bool,
    /// Les mondes que ce self a ajoutés (déjà portés par le registre).
    pub mondes: Vec<String>,
}

/// Le catalogue, et ce qu'il a pu lire.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct CatalogueSelfs {
    pub selfs: Vec<SelfResident>,
    pub entrees_total: usize,
    /// `true` quand la base synaptique n'a PAS pu être lue : `mondes` et
    /// `noeud_dans_synapse` sont alors des LACUNES, pas des négations — et le
    /// dire est la seule façon d'éviter qu'on lise un vide comme un fait.
    pub synapse_illisible: bool,
}

/// Délègue à [`lire_catalogue`] avec la vraie base synaptique et le vrai journal.
pub fn catalogue(synapse: Option<&std::path::Path>) -> CatalogueSelfs {
    lire_catalogue(synapse, &dir(), &crate::worlds::registre_des_mondes())
}

/// Le catalogue, construit sur des chemins EXPLICITES (donc vérifiable en test).
pub fn lire_catalogue(
    synapse: Option<&std::path::Path>,
    journal_dir: &std::path::Path,
    registre: &[(String, String, Option<String>)],
) -> CatalogueSelfs {
    use std::collections::HashMap;
    let mut par_self: HashMap<String, (usize, i64, Option<String>)> = HashMap::new();
    let mut total = 0usize;

    // ── 1. LES JOURNAUX ────────────────────────────────────────────────────
    if let Ok(lectures) = std::fs::read_dir(journal_dir) {
        for e in lectures.flatten() {
            let chemin = e.path();
            if chemin.extension().and_then(|x| x.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(txt) = std::fs::read_to_string(&chemin) else {
                continue;
            };
            for ligne in txt.lines().filter(|l| !l.trim().is_empty()) {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(ligne) else {
                    continue;
                };
                let self_qn =
                    v.get("self").and_then(|x| x.as_str()).unwrap_or("").to_string();
                if self_qn.is_empty() {
                    continue;
                }
                let t = v.get("t").and_then(|x| x.as_i64()).unwrap_or(0);
                let fiche = par_self
                    .entry(self_qn)
                    .or_insert((0, 0, Some(chemin.to_string_lossy().to_string())));
                fiche.0 += 1;
                fiche.1 = fiche.1.max(t);
                total += 1;
            }
        }
    }

    // ── 2. LES MONDES APPORTÉS (registre des mondes) ───────────────────────
    // Un self qui n'a encore rien écrit dans son journal mais qui a ajouté un
    // monde EXISTE : le catalogue doit le montrer, sinon il mentirait par
    // omission sur le seul geste qui l'a fait connaître.
    let mut mondes_par_self: HashMap<String, Vec<String>> = HashMap::new();
    for (label, _store, proprietaire) in registre {
        let Some(p) = proprietaire.as_deref().map(str::trim).filter(|p| !p.is_empty()) else {
            continue;
        };
        // Le registre porte `utilisateur@compte` ; le self est nommé par
        // l'utilisateur — MÊME règle que `ancrage::self_qn_pour`, appliquée UNE
        // fois de plus ici parce que les deux écritures sont indépendantes.
        let utilisateur = p.split('@').next().unwrap_or(p).trim();
        if utilisateur.is_empty() {
            continue;
        }
        let cle = format!("self:{utilisateur}");
        mondes_par_self.entry(cle.clone()).or_default().push(label.clone());
        par_self.entry(cle).or_insert((0, 0, None));
    }

    // ── 3. LES NŒUDS `Self` DE LA SYNAPTIQUE ───────────────────────────────
    // ── UN CHEMIN QUI N'EXISTE PAS N'EST PAS UNE BASE À CRÉER ──────────────
    // Défaut trouvé par le test de ce module : `cached_ro` OUVRE une base, et
    // l'ouvrir sur un chemin absent la CRÉE. Un catalogue en lecture seule
    // fabriquait donc une base vide — et, parce que l'ouverture réussissait,
    // il annonçait `synapse_illisible: false` : la lacune se lisait comme un
    // fait (« aucun nœud Self »). On exige donc que le chemin EXISTE avant de
    // l'ouvrir, et son absence est DITE.
    let mut noeuds: Vec<String> = Vec::new();
    let mut synapse_illisible = true;
    if let Some(p) = synapse
        && p.exists()
        && let Some(db) = crate::viz_serve::cached_ro_si_present(p)
            .or_else(|| crate::viz_serve::cached_ro(p).ok())
        && let Ok(r) = db.session().execute("MATCH (s:Self) RETURN s.qn")
    {
        for row in &r.rows {
            if let Some(obrain_common::types::Value::String(s)) = row.first() {
                noeuds.push(s.to_string());
            }
        }
        synapse_illisible = false;
    }
    for n in &noeuds {
        par_self.entry(n.clone()).or_insert((0, 0, None));
    }

    let mut selfs: Vec<SelfResident> = par_self
        .into_iter()
        .map(|(self_qn, (entrees, derniere_ms, journal))| SelfResident {
            noeud_dans_synapse: noeuds.iter().any(|n| *n == self_qn),
            mondes: mondes_par_self.remove(&self_qn).unwrap_or_default(),
            self_qn,
            journal,
            entrees,
            derniere_ms: (derniere_ms > 0).then_some(derniere_ms),
        })
        .collect();
    // Du plus récemment actif au plus ancien : c'est l'ordre dans lequel on
    // cherche quelqu'un. Les selfs sans journal passent après (date absente).
    selfs.sort_by(|a, b| {
        b.derniere_ms.cmp(&a.derniere_ms).then_with(|| a.self_qn.cmp(&b.self_qn))
    });
    CatalogueSelfs { selfs, entrees_total: total, synapse_illisible }
}

/// Le journal est indexé par le **Self**, pas par le store : c'est SA mémoire, et
/// la même quelle que soit la base qu'on regardait au moment où elle s'est formée.
pub fn journal(self_qn: &str) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    self_qn.hash(&mut h);
    dir().join(format!("{:016x}.jsonl", h.finish()))
}

/// Enregistre une intention. **Best-effort assumé** : ce journal ne doit jamais
/// faire échouer une conversation (même contrat que le journal d'activation).
pub fn enregistrer(self_qn: &str, provenance: &str, intention: Intention) -> Result<Entry> {
    let chemin = journal(self_qn);
    if let Some(d) = chemin.parent() {
        std::fs::create_dir_all(d)?;
    }
    let entree = Entree {
        t_ms: crate::activation::now_ms(),
        self_qn: self_qn.to_string(),
        provenance: provenance.to_string(),
        intention,
    };
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&chemin)
        .with_context(|| format!("mémoire : ouverture du journal {}", chemin.display()))?;
    use std::io::Write;
    writeln!(f, "{}", entree.vers_json())?;
    f.flush()?;
    // LE TÉMOIN, APRÈS le journal et jamais avant : le témoin mesure ce qui a
    // été ÉCRIT. Témoigner d'abord ferait compter une intention que le journal
    // n'a pas reçue — un instrument qui mesure l'intention au lieu du fait.
    temoigner(&Temoin::journalisation(&entree));
    Ok(Entry {
        t_ms: entree.t_ms,
        genre: entree.intention.genre(),
        reference: entree.intention.reference(),
    })
}

/// Ce qu'un enregistrement a donné — pour l'affichage comme pour le rapport.
#[derive(Clone, Debug)]
pub struct Entry {
    pub t_ms: i64,
    pub genre: &'static str,
    pub reference: String,
}

/// Lit les intentions EN ATTENTE.
pub fn en_attente(self_qn: &str) -> Vec<Entree> {
    let Ok(txt) = std::fs::read_to_string(journal(self_qn)) else {
        return Vec::new();
    };
    txt.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter_map(|v| Entree::depuis_json(&v))
        .collect()
}

/// Échappe une chaîne pour une interpolation GQL entre quotes simples.
///
/// `pub(crate)` depuis le 2026-09-16 : la passe de la flotte (`crate::machines`)
/// écrit elle aussi en GQL, et deux règles d'échappement dans un même binaire
/// finiraient par diverger — c'est exactement le genre d'écart qui ne se voit
/// qu'en production, sur un nom de machine contenant une apostrophe.
pub(crate) fn lit(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        match c {
            '\'' => out.push_str("\\'"),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// Un identifiant court et stable, dérivé du texte : deux applications de la
/// MÊME intention tombent sur le même nœud (`MERGE` sur `qn`).
fn cle(texte: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    texte.trim().hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Rapport d'une passe d'application.
#[derive(Clone, Debug, Default)]
pub struct Rapport {
    /// Le Self concerné.
    pub self_qn: String,
    /// La base écrite.
    pub store: String,
    pub appliquees: usize,
    /// Entrées refusées, gardées dans le journal — jamais perdues en silence.
    pub refusees: Vec<(String, String)>,
    /// Version du `SelfState` créé, si un état a été consolidé.
    pub version_self: Option<i64>,
}

/// Ce qu'une passe réécrit dans le journal, et pourquoi — **fonction PURE**.
///
/// Elle est extraite pour être éprouvable sans store et sans variable
/// d'environnement : ce qu'elle décide est ce qui distingue une mémoire qui
/// s'applique d'une mémoire qui se rejoue indéfiniment.
///
/// # Ce qu'elle rend
///
/// Les entrées en ÉCHEC (sous forme canonique, pour qu'elles repartent à la
/// passe suivante) suivies de toute ligne du journal qui n'appartenait PAS à
/// l'instantané de la passe.
///
/// # Défaut 1 (mesuré le 2026-09-11) — la consommation était par GENRE
///
/// Le filtre retenait toute entrée dont le genre figurait parmi les refus, donc
/// après un échec partiel des entrées DÉJÀ APPLIQUÉES étaient réécrites et
/// rejouées. Ce n'est pas anodin : `SelfState` est VERSIONNÉ exprès, et sa
/// justification écrite est « le mode consommé protège les gestes non
/// idempotents ». On garde donc les seules entrées en échec, **par indice**.
///
/// # Défaut 2 (mesuré le 2026-09-16) — la consommation comparait du TEXTE
///
/// `connues` était un ensemble des ré-sérialisations JSON, et la comparaison
/// portait sur la ligne BRUTE. Une ligne qui dit exactement la même chose mais
/// dans un AUTRE ORDRE DE CLÉS — ou écrite par une version antérieure du code,
/// avant qu'un champ ne bouge — n'était donc jamais reconnue comme appliquée.
/// Elle restait dans le journal, et la passe suivante la rejouait. Sur
/// `etat-self`, non idempotent, cela veut dire **une version de plus toutes les
/// deux minutes, indéfiniment** : mesuré sur un store temporaire, `v1` puis `v2`
/// pour une seule intention, avec le journal toujours présent après la passe.
///
/// On compare donc la **VALEUR** (`serde_json::Value`, insensible à l'ordre des
/// clés). Une ligne reconnue est CONSOMMÉE, et le journal se normalise au
/// passage puisque les échecs repartent, eux, sous forme canonique.
///
/// # Ce qui n'est JAMAIS jeté
///
/// Une ligne illisible est GARDÉE telle quelle. On ne sait pas ce qu'elle dit ;
/// la perdre serait exactement la « mémoire perdue en silence » qu'on refuse.
fn journal_a_reecrire(
    entrees: &[Entree],
    echecs: &std::collections::HashSet<usize>,
    courant: &str,
) -> Vec<String> {
    let restantes: Vec<String> = entrees
        .iter()
        .enumerate()
        .filter(|(i, _)| echecs.contains(i))
        .map(|(_, e)| e.vers_json().to_string())
        .collect();
    let connues: Vec<serde_json::Value> = entrees.iter().map(|e| e.vers_json()).collect();
    // ── ON NE TRONQUE PAS CE QUI A ÉTÉ JOURNALISÉ PENDANT LA PASSE ────────
    // `std::fs::write` TRONQUE. Or le chat journalise pendant qu'une passe
    // `--appliquer` travaille : ces intentions-là n'existent pas dans notre
    // instantané (`entrees`), et les réécrire les effaçait — une écriture
    // durable perdue sans un mot, contre la promesse « on ne perd pas une
    // intention ». D'où la relecture du journal juste avant d'écrire.
    let mut a_ecrire = restantes;
    for ligne in courant.lines() {
        let l = ligne.trim();
        if l.is_empty() || a_ecrire.iter().any(|x| x == l) {
            continue;
        }
        let reconnue = serde_json::from_str::<serde_json::Value>(l)
            .ok()
            .is_some_and(|v| connues.contains(&v));
        if reconnue {
            continue;
        }
        a_ecrire.push(l.to_string());
    }
    a_ecrire
}

/// APPLIQUE le journal dans la base du Self — ouverture **exclusive**.
///
/// `store` doit être la base où vit le Self (`self-demo`). L'appel échoue si un
/// lecteur la tient : c'est voulu (voir l'en-tête), et l'appelant doit alors
/// arrêter le service le temps de la passe — `scripts/memoire.sh` le fait.
pub fn appliquer(store: &Path, self_qn: &str) -> Result<Rapport> {
    let entrees = en_attente(self_qn);
    // Les entrées RÉELLEMENT refusées, par indice : une entrée APPLIQUÉE ne doit
    // jamais être rejouée, même si une autre entrée du même genre a échoué.
    let mut echecs: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut rapport = Rapport {
        self_qn: self_qn.to_string(),
        store: store.to_string_lossy().to_string(),
        ..Default::default()
    };
    if entrees.is_empty() {
        return Ok(rapport);
    }
    let db = obrain_engine::ObrainDB::open_substrate(store).with_context(|| {
        format!(
            "mémoire : ouverture EXCLUSIVE de {} — si le dashboard tourne, il tient la base \
             en lecture : utilisez `scripts/memoire.sh` (arrêt, application, relance)",
            store.display()
        )
    })?;
    // ── LA RAISON EST LE MESSAGE DU MOTEUR, PAS UNE DESCRIPTION ───────────
    // Défaut mesuré (2026-09-11) : `run` rendait un BOOLÉEN, donc l'erreur était
    // jetée et le rapport publiait sous la clé « raison » une simple description
    // de l'entrée (« note note:ab12… ») — le document promettait « le rapport dit
    // laquelle, ET POURQUOI ». Un échec qu'on ne peut pas expliquer ne se répare
    // pas : on transporte donc le message exact du moteur.
    let run = |gql: &str| -> Result<(), String> {
        db.session().execute(gql).map(|_| ()).map_err(|e| e.to_string())
    };

    // Le Self est la racine de tout ce qu'on écrit : s'il manque, on le crée
    // (MERGE — idempotent), jamais on ne l'écrase.
    let _ = run(&format!(
        "MERGE (s:Self {{qn: {}}}) SET s.updated_ms = {} RETURN s",
        lit(self_qn),
        crate::activation::now_ms()
    ));

    // La version se lit DANS la base, pas dans le journal : c'est la seule façon
    // qu'elle reste monotone même si plusieurs passes s'enchaînent.
    let mut version = db
        .session()
        .execute(&format!(
            "MATCH (s:Self {{qn: {}}})-[:HAS_STATE]->(st:SelfState) RETURN st.version \
             ORDER BY st.version DESC LIMIT 1",
            lit(self_qn)
        ))
        .ok()
        .and_then(|r| r.rows.first().and_then(|row| row.first().cloned()))
        .and_then(|v| match v {
            obrain_common::types::Value::Int64(x) => Some(x),
            obrain_common::types::Value::Float64(x) => Some(x as i64),
            _ => None,
        })
        .unwrap_or(0);

    for (i, e) in entrees.iter().enumerate() {
        let t = e.t_ms;
        let (ok, refus) = match &e.intention {
            Intention::Note { texte, sujet } => {
                let qn = format!("note:{}:{}", cle(texte), crate::viz_weave::slug(sujet));
                let ok = run(&format!(
                    "MERGE (n:Note {{qn: {}}}) SET n.text = {}, n.sujet = {}, \
                     n.provenance = {}, n.created_ms = {} RETURN n",
                    lit(&qn),
                    lit(texte),
                    lit(sujet),
                    lit(&e.provenance),
                    t
                ));
                let _ = run(&format!(
                    "MATCH (s:Self {{qn: {}}}), (n:Note {{qn: {}}}) MERGE (s)-[:HAS_NOTE]->(n) RETURN n",
                    lit(self_qn),
                    lit(&qn)
                ));
                (ok, format!("note {qn}"))
            }
            Intention::Objectif { nom, texte } => {
                let qn = format!("goal:{}", crate::viz_weave::slug(nom));
                let ok = run(&format!(
                    "MERGE (g:Goal {{qn: {}}}) SET g.name = {}, g.text = {}, \
                     g.provenance = {}, g.created_ms = {} RETURN g",
                    lit(&qn),
                    lit(nom),
                    lit(texte),
                    lit(&e.provenance),
                    t
                ));
                let _ = run(&format!(
                    "MATCH (s:Self {{qn: {}}}), (g:Goal {{qn: {}}}) MERGE (s)-[:HAS_GOAL]->(g) RETURN g",
                    lit(self_qn),
                    lit(&qn)
                ));
                (ok, format!("objectif {qn}"))
            }
            Intention::Procedure { nom, description, corps } => {
                let qn = format!("proc:{}", nom.trim_start_matches("proc:"));
                let ok = run(&format!(
                    "MERGE (p:Procedure {{qn: {}}}) SET p.name = {}, p.description = {}, \
                     p.body = {}, p.cls = 'read', p.origin = 'chat', p.provenance = {}, \
                     p.created_ms = {} RETURN p",
                    lit(&qn),
                    lit(nom),
                    lit(description),
                    lit(corps),
                    lit(&e.provenance),
                    t
                ));
                let _ = run(&format!(
                    "MATCH (s:Self {{qn: {}}}), (p:Procedure {{qn: {}}}) MERGE (s)-[:MASTERS]->(p) RETURN p",
                    lit(self_qn),
                    lit(&qn)
                ));
                (ok, format!("procedure {qn}"))
            }
            Intention::EtatSelf { epithet, objectif, prochaine_action, note } => {
                // Le SEUL geste non idempotent — et c'est voulu : un état est un
                // instantané DATÉ, pas une valeur écrasée. Le journal est consommé
                // pour qu'il ne soit créé qu'une fois.
                version += 1;
                let qn = format!("selfstate:{}:v{}", crate::viz_weave::slug(self_qn), version);
                let ok = run(&format!(
                    "MERGE (st:SelfState {{qn: {}}}) SET st.version = {version}, \
                     st.epithet = {}, st.objective = {}, st.next_action = {}, st.note = {}, \
                     st.status = 'journaled', st.created_at = {}, st.provenance = {} RETURN st",
                    lit(&qn),
                    lit(epithet),
                    lit(objectif),
                    lit(prochaine_action),
                    lit(note),
                    t,
                    lit(&e.provenance)
                ));
                let _ = run(&format!(
                    "MATCH (s:Self {{qn: {}}}), (st:SelfState {{qn: {}}}) \
                     MERGE (s)-[:HAS_STATE]->(st) RETURN st",
                    lit(self_qn),
                    lit(&qn)
                ));
                if ok.is_ok() {
                    rapport.version_self = Some(version);
                }
                (ok, format!("etat-self v{version}"))
            }
        };
        match ok {
            Ok(()) => rapport.appliquees += 1,
            Err(erreur) => {
                // Une entrée refusée RESTE : on ne perd pas une intention parce
                // qu'une requête a échoué. Le rapport dit laquelle, ET POURQUOI —
                // la raison est le message du moteur, pas la description de
                // l'entrée (défaut mesuré : le rapport publiait « note note:ab12… »
                // sous la clé « raison », une description à la place d'une cause).
                echecs.insert(i);
                rapport
                    .refusees
                    .push((e.intention.genre().to_string(), format!("{refus} : {erreur}")));
            }
        }
    }

    // Le journal est CONSOMMÉ ENTRÉE PAR ENTRÉE et par VALEUR — jamais par
    // genre, et jamais par texte : `journal_a_reecrire` porte la doctrine et les
    // deux défauts mesurés (le genre le 2026-09-11, le texte le 2026-09-16).
    // Extraite, elle se prouve sans store ni variable d'environnement.
    let chemin = journal(self_qn);
    // Une lecture qui échoue (journal absent) n'est pas une erreur : il n'y a
    // simplement rien à conserver, et les échecs éventuels repartent quand même.
    let courant = std::fs::read_to_string(&chemin).unwrap_or_default();
    let a_ecrire = journal_a_reecrire(&entrees, &echecs, &courant);
    if a_ecrire.is_empty() {
        let _ = std::fs::remove_file(&chemin);
    } else {
        let _ = std::fs::write(&chemin, a_ecrire.join("\n") + "\n");
    }
    let _ = db.flush_wal();
    // ── LE TÉMOIN DE LA PASSE — ET SEULEMENT QUAND ELLE A FAIT QUELQUE CHOSE ─
    // Une passe qui ne trouve rien ne témoigne pas : elle le DIT déjà, chaque
    // fois, dans le journal du service (la ligne que `memoire-appliquer-comptes.sh`
    // imprime). Un témoin qui compterait le repos toutes les deux minutes
    // produirait 720 lignes par jour pour noyer le seul fait qu'il existe pour
    // rendre visible. Il ne témoigne donc que d'une AVANCE, ou d'un refus —
    // jamais d'une absence, qui n'est pas un fait de mémoire.
    if rapport.appliquees > 0 || !rapport.refusees.is_empty() {
        temoigner(&Temoin::application(crate::activation::now_ms(), self_qn, &rapport));
    }
    Ok(rapport)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intention_fait_un_aller_retour_json() {
        let i = Intention::Procedure {
            nom: "code.hot".into(),
            description: "chemins critiques".into(),
            corps: "MATCH (f) RETURN f".into(),
        };
        let j = i.vers_json();
        assert_eq!(Intention::depuis_json(&j), Some(i));
        assert_eq!(Intention::depuis_json(&serde_json::json!({"kind": "inconnu"})), None);
    }

    #[test]
    fn entree_fait_un_aller_retour_json() {
        let e = Entree {
            t_ms: 42,
            self_qn: "self:test".into(),
            provenance: "world:x".into(),
            intention: Intention::Note { texte: "hello".into(), sujet: "s".into() },
        };
        let r = Entree::depuis_json(&e.vers_json()).expect("relu");
        assert_eq!(r.t_ms, 42);
        assert_eq!(r.self_qn, "self:test");
        assert_eq!(r.provenance, "world:x");
        assert_eq!(r.intention, e.intention);
    }

    /// ── LE JOURNAL SE CONSOMME PAR VALEUR, PAS PAR TEXTE ───────────────────
    ///
    /// Le geste `etat-self` est VERSIONNÉ exprès : le rejouer, c'est ajouter une
    /// version de l'état du self. Une ligne du journal qui dit la même chose dans
    /// un autre ordre de clés était donc rejouée à chaque passe — soit un
    /// `SelfState` de plus toutes les deux minutes, indéfiniment, dans la base du
    /// compte. C'est le défaut que ce test fixe.
    #[test]
    fn une_ligne_non_canonique_est_consommee_et_non_rejouee() {
        let e = Entree {
            t_ms: 7,
            self_qn: "self:t".into(),
            provenance: "p".into(),
            intention: Intention::EtatSelf {
                epithet: "e".into(),
                objectif: "o".into(),
                prochaine_action: "a".into(),
                note: "n".into(),
            },
        };
        // LA MÊME CHOSE, dans un autre ordre de clés — comme une ligne écrite
        // par une version antérieure du code, avant qu'un champ ne bouge.
        let non_canonique = concat!(
            "{\"self\":\"self:t\",\"provenance\":\"p\",\"t\":7,",
            "\"intention\":{\"note\":\"n\",\"prochaine_action\":\"a\",",
            "\"objectif\":\"o\",\"kind\":\"etat-self\",\"epithet\":\"e\"}}"
        );
        // GARDE DE NON-VACUITÉ : si la ligne était canonique, le test ne
        // prouverait rien du tout — il passerait pour la mauvaise raison.
        assert_ne!(
            non_canonique,
            e.vers_json().to_string(),
            "la ligne de ce test doit être NON canonique"
        );

        let reste = journal_a_reecrire(&[e], &std::collections::HashSet::new(), non_canonique);
        assert!(
            reste.is_empty(),
            "l'intention a été appliquée : la ligne doit être CONSOMMÉE, pas \
             laissée pour un rejeu — reste = {reste:?}"
        );
    }

    /// Ce qu'une passe ne doit JAMAIS perdre : une intention journalisée par le
    /// chat PENDANT la passe. Elle n'est pas dans l'instantané, donc elle n'a pas
    /// été appliquée — l'effacer serait une écriture durable perdue sans un mot.
    #[test]
    fn une_intention_journalisee_pendant_la_passe_survit() {
        let traitee = Entree {
            t_ms: 7,
            self_qn: "self:t".into(),
            provenance: "p".into(),
            intention: Intention::Note { texte: "déjà vue".into(), sujet: "s".into() },
        };
        let pendant = concat!(
            "{\"t\":99,\"self\":\"self:t\",\"provenance\":\"p\",",
            "\"intention\":{\"kind\":\"note\",\"texte\":\"neuve\",\"sujet\":\"s\"}}"
        );
        let reste = journal_a_reecrire(&[traitee], &std::collections::HashSet::new(), pendant);
        assert_eq!(
            reste,
            vec![pendant.to_string()],
            "une intention arrivée pendant la passe doit être conservée telle quelle"
        );
    }

    /// Une entrée en ÉCHEC reste — une seule fois. Ni perdue (elle doit repartir
    /// à la passe suivante), ni dupliquée (même si le journal en porte une copie
    /// non canonique, qui est reconnue par valeur et non recopiée).
    #[test]
    fn un_echec_reste_une_fois_ni_perdu_ni_duplique() {
        let e = Entree {
            t_ms: 7,
            self_qn: "self:t".into(),
            provenance: "p".into(),
            intention: Intention::Note { texte: "à reprendre".into(), sujet: "s".into() },
        };
        let mut echecs = std::collections::HashSet::new();
        echecs.insert(0usize);

        // Journal portant la copie canonique de l'entrée en échec.
        let un = journal_a_reecrire(&[e.clone()], &echecs, &e.vers_json().to_string());
        assert_eq!(un.len(), 1, "un échec reste UNE fois : reste = {un:?}");

        // Journal portant la même entrée sous une forme NON canonique : elle est
        // reconnue par valeur, donc pas ajoutée en plus de la canonique.
        let non_canonique = "{\"intention\":{\"sujet\":\"s\",\"texte\":\"à reprendre\",\
                             \"kind\":\"note\"},\"t\":7,\"self\":\"self:t\",\"provenance\":\"p\"}";
        let deux = journal_a_reecrire(&[e], &echecs, non_canonique);
        assert_eq!(
            deux.len(),
            1,
            "l'échec ne doit pas être DUPLIQUÉ quand sa copie disque est non \
             canonique — reste = {deux:?}"
        );
    }

    /// Une ligne qu'on ne sait pas lire est GARDÉE. On ne sait pas ce qu'elle
    /// dit ; la jeter serait la mémoire perdue en silence qu'on refuse.
    #[test]
    fn une_ligne_illisible_n_est_jamais_jetee() {
        let illisible = "ceci n'est pas du json — mais c'est peut-être une intention";
        let reste = journal_a_reecrire(&[], &std::collections::HashSet::new(), illisible);
        assert_eq!(reste, vec![illisible.to_string()]);
    }

    /// ── LE TÉMOIN SE LIT, ET IL NE SE CONFOND PAS AVEC LE JOURNAL ──────────
    ///
    /// Deux propriétés, et chacune fixe un défaut mesuré :
    ///
    ///  · **il se relit en JSON** avec les clés du contrat (`self`, `geste`,
    ///    `genre`, `reference`) : le lecteur est un `python3` dans la passe. Un
    ///    témoin illisible serait un témoin qu'on ne lit pas, c'est-à-dire un
    ///    silence de plus ;
    ///  · **il ne vit PAS dans `dir()`** — le répertoire que la passe et le
    ///    catalogue PARCOURENT pour découvrir ce qui attend. Un `.jsonl` de plus
    ///    là-dedans serait compté comme un journal de self et attribuerait à
    ///    personne des entrées qui n'en sont pas.
    #[test]
    fn le_temoin_se_lit_et_ne_vit_pas_dans_le_repertoire_du_journal() {
        let j = journal("self:t");
        let t = chemin_temoins();
        assert_ne!(
            t.parent(),
            Some(j.parent().expect("le journal a un répertoire")),
            "le témoin ne doit pas être dans le répertoire PARCOURU ({}) : il y \
             serait compté comme un journal de self — témoin = {}",
            j.display(),
            t.display()
        );
        assert_eq!(t.file_name().and_then(|x| x.to_str()), Some("memoire-temoins.jsonl"));

        let e = Entree {
            t_ms: 1234,
            self_qn: "self:alice".into(),
            provenance: "monde · /tmp/w.obrain".into(),
            intention: Intention::Note { texte: "un loup gris".into(), sujet: "s".into() },
        };
        let ligne = Temoin::journalisation(&e).ligne().expect("sérialisation");
        let v: serde_json::Value = serde_json::from_str(&ligne).expect("le témoin est du JSON");
        assert_eq!(v["self"], "self:alice");
        assert_eq!(v["geste"], GESTE_JOURNALISE);
        assert_eq!(v["genre"], "note");
        assert_eq!(v["t"], 1234);
        assert!(
            v.get("base").is_none(),
            "une journalisation n'écrit aucune base : le champ doit être ABSENT, \
             pas vide — sinon le lecteur ne peut pas distinguer « aucune base » \
             de « une base au chemin vide »"
        );

        let r = Rapport {
            self_qn: "self:alice".into(),
            store: "/tmp/selfs/alice.obrain".into(),
            appliquees: 2,
            refusees: vec![],
            version_self: None,
        };
        let a: serde_json::Value =
            serde_json::from_str(&Temoin::application(9, "self:alice", &r).ligne().unwrap())
                .expect("le témoin d'une passe est du JSON");
        assert_eq!(a["geste"], GESTE_APPLIQUE);
        assert_eq!(a["base"], "/tmp/selfs/alice.obrain");
        assert_eq!(a["reference"], "2 appliquée(s), 0 refusée(s)");
    }

    /// ── LA PREUVE DE BOUT EN BOUT N'EST PAS ICI, ET C'EST DÉLIBÉRÉ ─────────
    ///
    /// La reproduire en test unitaire demanderait de poser
    /// `OBRAIN_VIZ_CACHE_DIR` dans le processus : `std::env::set_var` exige un
    /// bloc `unsafe` en édition 2024, et ce crate porte `#![deny(unsafe_code)]`.
    /// On ne desserre pas cette garantie pour un test. La preuve de bout en bout
    /// (une intention non canonique, deux passes, UNE seule version d'état) est
    /// donc faite sur le BINAIRE RÉEL, avec un cache isolé par l'environnement du
    /// processus lancé — ce que les quatre tests purs ci-dessus ne peuvent pas
    /// faire, et ce qu'ils n'ont pas besoin de faire : ils fixent la décision,
    /// la démonstration fixe le comportement observable.
    ///
    /// La clé d'un nœud est dérivée du TEXTE : deux applications de la même note
    /// tombent sur le même `qn`, donc `MERGE` ne duplique pas. C'est ce qui rend
    /// une double application inoffensive (hors `SelfState`, versionné exprès).
    #[test]
    fn cle_stable_pour_le_meme_texte() {
        assert_eq!(cle("un texte"), cle("un texte"));
        assert_ne!(cle("un texte"), cle("un autre"));
    }

    /// Le journal est indexé par le Self : deux Selfs ne partagent pas leur
    /// mémoire, et le même Self garde la sienne quel que soit le store regardé.
    #[test]
    fn journal_par_self() {
        assert_ne!(journal("self:a"), journal("self:b"));
        assert_eq!(journal("self:a"), journal("self:a"));
        assert!(journal("self:a").to_string_lossy().ends_with(".jsonl"));
    }

    #[test]
    fn echappement_gql() {
        assert_eq!(lit("a'b"), "'a\\'b'");
        assert_eq!(lit("l1\nl2"), "'l1\\nl2'");
    }

    /// ── LE CATALOGUE LIT, IL NE DEVINE PAS ──────────────────────────────────
    ///
    /// Trois propriétés qui décident de son honnêteté :
    ///   · un self dont le journal porte des entrées APPARAÎT, avec son compte ;
    ///   · un self qui n'a qu'un MONDE (aucune entrée) apparaît AUSSI — sinon on
    ///     cacherait le seul geste qui l'a fait connaître ;
    ///   · l'ordre met le plus récemment actif devant, et une synaptique
    ///     illisible est DITE, pas confondue avec « aucun self ».
    #[test]
    fn catalogue_lit_journaux_mondes_et_ordre() {
        let base = std::env::temp_dir().join(format!("obrain-cat-{}", std::process::id()));
        let jdir = base.join("memoire");
        std::fs::create_dir_all(&jdir).expect("répertoire de journaux");
        // Un journal pour alice : deux entrées, la seconde plus récente.
        let ja = jdir.join("alice.jsonl");
        std::fs::write(
            &ja,
            "{\"t\":10,\"self\":\"self:alice\",\"provenance\":\"p\",\"intention\":{\"genre\":\"note\"}}\n\
             {\"t\":50,\"self\":\"self:alice\",\"provenance\":\"p\",\"intention\":{\"genre\":\"note\"}}\n",
        )
        .expect("journal alice");
        // Un fichier qui n'est PAS un journal : il ne doit pas être lu.
        std::fs::write(jdir.join("lisez-moi.txt"), "pas du jsonl").expect("fichier parasite");

        // Le registre : bob n'a AUCUNE entrée, mais il a apporté un monde.
        let registre = vec![(
            "sindresorhus/is-npm (GitHub)".to_string(),
            "/tmp/is-npm.obrain".to_string(),
            Some("bob@obrain".to_string()),
        )];
        // Synapse illisible (chemin inexistant) : c'est le cas à vérifier.
        let cat = lire_catalogue(Some(&base.join("synapse-absente.obrain")), &jdir, &registre);

        assert!(cat.synapse_illisible, "une synaptique illisible doit être DITE");
        let alice = cat
            .selfs
            .iter()
            .find(|s| s.self_qn == "self:alice")
            .expect("alice doit apparaître (elle a un journal)");
        assert_eq!(alice.entrees, 2, "les deux entrées doivent être comptées");
        assert_eq!(alice.derniere_ms, Some(50), "la date la plus récente est retenue");
        assert_eq!(alice.journal.as_deref(), Some(ja.to_string_lossy().as_ref()));
        let bob = cat
            .selfs
            .iter()
            .find(|s| s.self_qn == "self:bob")
            .expect("bob doit apparaître (il a apporté un monde)");
        assert_eq!(bob.entrees, 0);
        assert_eq!(bob.mondes.len(), 1, "son monde lui est attribué");
        assert!(!bob.noeud_dans_synapse, "synaptique illisible → aucun nœud affirmé");
        assert_eq!(cat.entrees_total, 2);
        assert_eq!(
            cat.selfs.first().map(|s| s.self_qn.as_str()),
            Some("self:alice"),
            "le plus récemment actif vient en premier"
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}
