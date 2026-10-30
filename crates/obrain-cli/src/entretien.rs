//! ENTRETIEN EN ATTENTE — les demandes de réparation **attachées en vol**.
//!
//! ## Le problème que ce module résout
//!
//! Une réparation exige une ouverture **exclusive**. Or les bases sont tenues :
//! par le service (lecture), par les ponts MCP du harnais (lecture et écriture),
//! parfois par un autre agent qui relance le même service sur le même port.
//!
//! Jusqu'ici, une base qu'on ne pouvait pas réparer était simplement **abandonnée**
//! à ce passage : le compte rendu disait « TENUE », et l'information **mourait
//! là**. Au redémarrage suivant, il fallait que quelqu'un se souvienne qu'elle
//! était à réparer — et personne ne s'en souvient.
//!
//! ## Le protocole
//!
//! Le même que la mémoire du chat (`crate::memoire`), pour la même raison : le
//! substrat impose l'invariant mono-écrivain, donc on **journalise l'intention**
//! quand on ne peut pas agir, et on **l'applique** quand on peut.
//!
//! 1. **Pendant que la base tourne** — un producteur constate un défaut et
//!    **attache une demande** : le store, le genre de réparation, la raison
//!    mesurée, l'horodatage. Aucun verrou n'est nécessaire : c'est un fichier
//!    append-only.
//! 2. **À la fenêtre suivante** (l'entretien de redémarrage) — les demandes en
//!    attente sont **rejouées** sur les bases libérées, et **accordées** (retirées)
//!    uniquement quand la réparation a réussi.
//!
//! Une demande refusée **reste** : elle sera retentée au redémarrage suivant, et
//! son ancienneté est visible — une réparation qui traîne depuis trois jours se
//! voit, au lieu de disparaître.
//!
//! ## Ce que ce n'est pas
//!
//! Ce n'est pas une file d'exécution : rien ne s'exécute en tâche de fond contre
//! une base tenue. C'est une **mémoire des réparations dues**, ce qui est la seule
//! chose honnêtement possible tant que l'invariant mono-écrivain tient.

use std::path::{Path, PathBuf};

/// Ce qu'il faut réparer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Genre {
    /// Colonnes de degrés (reconstruction en place).
    Degres,
    /// Enregistrements corrompus (`obrain repair --apply`).
    Enregistrements,
}

impl Genre {
    pub fn nom(&self) -> &'static str {
        match self {
            Genre::Degres => "degres",
            Genre::Enregistrements => "enregistrements",
        }
    }
    /// Variante publique : la CLI et les scripts lisent ce nom.
    pub fn depuis_publique(s: &str) -> Option<Self> {
        Self::depuis(s)
    }
    fn depuis(s: &str) -> Option<Self> {
        match s {
            "degres" => Some(Genre::Degres),
            "enregistrements" => Some(Genre::Enregistrements),
            _ => None,
        }
    }
}

/// Une demande de réparation attachée en vol.
#[derive(Clone, Debug)]
pub struct Demande {
    pub store: String,
    pub genre: Genre,
    /// La mesure qui justifie la demande — jamais une impression.
    pub raison: String,
    pub t_ms: i64,
    /// Qui l'a constatée (`entretien`, `hubs`, `atlas`…).
    pub source: String,
}

fn chemin() -> PathBuf {
    let dir = std::env::var_os("OBRAIN_VIZ_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/obrain-viz-cache"))
        .join("entretien");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("en-attente.jsonl")
}

fn vers_json(d: &Demande) -> String {
    serde_json::json!({
        "t": d.t_ms, "store": d.store, "genre": d.genre.nom(),
        "raison": d.raison, "source": d.source,
    })
    .to_string()
}

fn depuis_json(v: &serde_json::Value) -> Option<Demande> {
    Some(Demande {
        store: v.get("store")?.as_str()?.to_string(),
        genre: Genre::depuis(v.get("genre")?.as_str()?)?,
        raison: v.get("raison").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        t_ms: v.get("t").and_then(|x| x.as_i64()).unwrap_or(0),
        source: v.get("source").and_then(|x| x.as_str()).unwrap_or("").to_string(),
    })
}

/// ATTACHE une demande. **Idempotent** : attacher dix fois la même demande n'en
/// crée qu'une — sans quoi un défaut constaté à chaque requête remplirait le
/// journal et le rendrait illisible, exactement le défaut corrigé sur le journal
/// du serveur (6 039 lignes).
pub fn attacher(store: &Path, genre: Genre, raison: &str, source: &str) -> std::io::Result<()> {
    attacher_dans(&chemin(), store, genre, raison, source)
}

/// La couture testable : même chose, sur un journal DONNÉ.
///
/// Elle existe pour que les tests n'aient pas à toucher une variable
/// d'environnement — `set_var` est `unsafe` depuis Rust 2024 et ce crate
/// **interdit `unsafe`** (`#![deny(unsafe_code)]`). Un test qui polluerait la
/// file réelle de l'entretien serait par ailleurs pire qu'inutile : il y
/// laisserait une demande que le redémarrage tenterait de réparer.
pub fn attacher_dans(
    fichier: &Path,
    store: &Path,
    genre: Genre,
    raison: &str,
    source: &str,
) -> std::io::Result<()> {
    let store = store.to_string_lossy().to_string();
    // Déjà en attente pour ce couple (store, genre) ? On ne double pas. On
    // RAFRAÎCHIT la raison : la dernière mesure est la plus utile.
    let mut lignes: Vec<String> = en_attente_dans(fichier)
        .into_iter()
        .filter(|d| !(d.store == store && d.genre == genre))
        .map(|d| vers_json(&d))
        .collect();
    lignes.push(vers_json(&Demande {
        store,
        genre,
        raison: raison.to_string(),
        t_ms: crate::activation::now_ms(),
        source: source.to_string(),
    }));
    std::fs::write(fichier, lignes.join("\n") + "\n")
}

/// Les demandes en attente, la plus ancienne d'abord.
pub fn en_attente() -> Vec<Demande> {
    en_attente_dans(&chemin())
}

/// La couture testable de [`en_attente`].
pub fn en_attente_dans(fichier: &Path) -> Vec<Demande> {
    let Ok(txt) = std::fs::read_to_string(fichier) else {
        return Vec::new();
    };
    let mut v: Vec<Demande> = txt
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter_map(|j| depuis_json(&j))
        .collect();
    v.sort_by_key(|d| d.t_ms);
    v
}

/// ACCORDE une demande : la réparation a réussi, on la retire.
pub fn accorder(store: &Path, genre: Genre) {
    accorder_dans(&chemin(), store, genre)
}

/// La couture testable de [`accorder`].
pub fn accorder_dans(fichier: &Path, store: &Path, genre: Genre) {
    let store = store.to_string_lossy().to_string();
    let restantes: Vec<String> = en_attente_dans(fichier)
        .into_iter()
        .filter(|d| !(d.store == store && d.genre == genre))
        .map(|d| vers_json(&d))
        .collect();
    if restantes.is_empty() {
        let _ = std::fs::remove_file(fichier);
    } else {
        let _ = std::fs::write(fichier, restantes.join("\n") + "\n");
    }
}

/// L'âge de la plus ancienne demande, en secondes (`None` si la file est vide).
pub fn age_max_secondes() -> Option<i64> {
    let plus_vieille = en_attente().first()?.t_ms;
    let age = crate::activation::now_ms() - plus_vieille;
    Some((age / 1000).max(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_genre_fait_un_aller_retour() {
        for g in [Genre::Degres, Genre::Enregistrements] {
            assert_eq!(Genre::depuis(g.nom()), Some(g));
        }
        assert_eq!(Genre::depuis("inconnu"), None);
    }

    #[test]
    fn une_demande_fait_un_aller_retour_json() {
        let d = Demande {
            store: "/tmp/x.obrain".into(),
            genre: Genre::Degres,
            raison: "somme incohérente".into(),
            t_ms: 42,
            source: "entretien".into(),
        };
        let r = depuis_json(&serde_json::from_str(&v(&d)).unwrap()).unwrap();
        assert_eq!(r.store, d.store);
        assert_eq!(r.genre, d.genre);
        assert_eq!(r.raison, d.raison);
        assert_eq!(r.source, d.source);
    }

    fn v(d: &Demande) -> String {
        vers_json(d)
    }

    /// Attacher DIX fois la même demande n'en crée qu'UNE. Sans cela, un défaut
    /// constaté à chaque requête remplirait le journal et le rendrait illisible
    /// — le défaut des 6 039 lignes, transposé.
    #[test]
    fn attacher_est_idempotent() {
        // La couture : un journal DÉDIÉ, jamais la file réelle (un test qui y
        // laisserait une demande ferait tenter une réparation pour rien au
        // prochain redémarrage).
        let td = std::env::temp_dir().join("entretien-test-file.jsonl");
        let _ = std::fs::remove_file(&td);
        let s = Path::new("/tmp/idempotence-test.obrain");
        for i in 0..10 {
            attacher_dans(&td, s, Genre::Degres, &format!("raison {i}"), "test").unwrap();
        }
        let att = en_attente_dans(&td);
        assert_eq!(att.len(), 1, "dix attachements, une demande : {att:?}");
        assert_eq!(att[0].raison, "raison 9", "la DERNIÈRE mesure est gardée");
        accorder_dans(&td, s, Genre::Degres);
        assert!(en_attente_dans(&td).is_empty(), "accorder retire la demande");
        let _ = std::fs::remove_file(&td);
    }

    /// Deux GENRES pour la même base sont DEUX demandes : réparer les colonnes
    /// n'est pas réparer les enregistrements, et accorder l'une ne doit pas
    /// acquitter l'autre.
    #[test]
    fn les_deux_genres_sont_distincts() {
        let td = std::env::temp_dir().join("entretien-test-genres.jsonl");
        let _ = std::fs::remove_file(&td);
        let s = Path::new("/tmp/genres-test.obrain");
        attacher_dans(&td, s, Genre::Degres, "d", "test").unwrap();
        attacher_dans(&td, s, Genre::Enregistrements, "e", "test").unwrap();
        assert_eq!(en_attente_dans(&td).len(), 2);
        accorder_dans(&td, s, Genre::Degres);
        let reste = en_attente_dans(&td);
        assert_eq!(reste.len(), 1, "accorder les degrés ne doit pas acquitter le reste");
        assert_eq!(reste[0].genre, Genre::Enregistrements);
        let _ = std::fs::remove_file(&td);
    }
}
