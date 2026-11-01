//! La politique du nœud — ce qu'il accepte de faire, décidé par l'opérateur.
//!
//! # Pourquoi la politique vient de la LIGNE DE COMMANDE, pas du client
//!
//! Un nœud déployé sur une machine appartient à celui qui l'a installé. Si le
//! client pouvait élargir ses propres droits — demander à sortir de la racine,
//! ou exécuter sans que l'exécution ait été autorisée — la politique ne serait
//! pas une politique, mais une préférence. On suit ici la doctrine déjà tenue
//! par le seam `ctx.approval` du harness : **l'absence de décision vaut refus**.
//!
//! Les valeurs effectives sont donc fixées au démarrage, jamais négociées, et
//! `caps` les **rende** telles qu'elles sont — un client peut les lire, pas les
//! changer.
//!
//! # ANNONCER EST DÉCIDER — la capacité se MESURE, elle ne se déclare pas
//!
//! L'incident : `attache::capacites` annonçait `write` **inconditionnellement**,
//! alors que la politique confine le nœud à une racine. Sur une racine en lecture
//! seule (ou un montage `ro`), le nœud annonçait donc au moteur une capacité qu'il
//! refusait ensuite à l'exécution : le moteur proposait un geste, l'utilisateur le
//! demandait, l'échec arrivait après coup. C'est exactement ce que la doctrine
//! interdit — le même défaut que le commentaire d'origine dénonçait pour `exec`
//! (« afficher `exec` sur un nœud qui refuse d'exécuter ferait proposer à
//! l'utilisateur une action vouée au refus »), sans l'appliquer à `write`.
//!
//! Ce module porte donc les **verdicts** : pour chaque capacité, une mesure faite
//! ICI, sur CETTE racine, avec CETTE politique. Quand elle n'est pas exercable, le
//! motif est rendu — « je ne peux pas » sans dire pourquoi n'est pas une réponse,
//! c'est une panne déguisée.
//!
//! ## Ce qui est mesuré, et ce qui ne peut pas l'être
//!
//! La sonde d'écriture est une **écriture réelle** (création d'un répertoire, d'un
//! fichier, puis nettoyage), pas un test de permission : les bits d'un `stat` ne
//! disent rien d'un montage `ro`, d'une ACL, d'un attribut immuable ou d'un quota.
//! `outils::write` fait `create_dir_all(parent)` puis `write` — la sonde fait
//! exactement ces deux appels, donc elle échoue là où `write` échouerait.
//!
//! À l'inverse, la permission d'un **fichier précis** (absent, `0600` à un autre
//! utilisateur) n'est pas une capacité : c'est le résultat d'un appel, et il
//! restera légitimement un échec. La capacité mesurée est celle de la RACINE.

use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};

use anyhow::{bail, Result};
use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub struct Politique {
    /// Racine du nœud : tout chemin est résolu CONTRE elle, et rien n'en sort.
    pub racine: PathBuf,
    /// Exécution de commandes autorisée. Faux par défaut : un nœud qui n'a pas
    /// été autorisé à agir est un nœud qui observe.
    pub exec_autorise: bool,
    /// Plafond d'octets gardés par flux (stdout/stderr). Au-delà, la sortie est
    /// **drainée mais non conservée** : ne pas lire ferait bloquer l'enfant.
    pub plafond_octets: usize,
    /// Délai par défaut d'une commande.
    pub delai_ms: u64,
}

impl Default for Politique {
    fn default() -> Self {
        Self {
            racine: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
            exec_autorise: false,
            plafond_octets: 256 * 1024,
            delai_ms: 60_000,
        }
    }
}

impl Politique {
    /// Résout un chemin demandé par le client **à l'intérieur** de la racine.
    ///
    /// Le contrôle porte sur le chemin **canonisé**, pas sur la chaîne reçue :
    /// `..`, un lien symbolique ou un chemin absolu détourné doivent tous
    /// échouer de la même façon. Refuser sur la forme de la chaîne serait un
    /// filtre qu'un `..` suffit à contourner.
    pub fn resoudre(&self, demande: &str) -> Result<PathBuf> {
        // Un chemin absolu est accepté s'il est DÉJÀ sous la racine ; sinon la
        // requête tente une évasion et on la refuse sans l'exécuter.
        let brut = if Path::new(demande).is_absolute() {
            PathBuf::from(demande)
        } else {
            self.racine.join(demande)
        };

        // Le parent doit exister pour que `canonicalize` réussisse (le fichier
        // peut, lui, ne pas encore exister pour un `write`).
        let canonique = canonicaliser_souple(&brut)?;
        let racine = std::fs::canonicalize(&self.racine).unwrap_or_else(|_| self.racine.clone());
        if !canonique.starts_with(&racine) {
            bail!(
                "chemin refusé : « {demande} » sort de la racine du nœud ({}). \
                 C'est une évasion, pas une faute de frappe — elle est refusée, pas normalisée.",
                racine.display()
            );
        }
        Ok(canonique)
    }

    /// La racine **réelle** : le chemin canonisé, celui qui confine vraiment.
    ///
    /// Elle peut différer de `racine` quand l'opérateur a déclaré un lien
    /// symbolique : c'est ce chemin-là qui apparaît dans les refus, et le taire
    /// ferait chercher longtemps pourquoi « `/srv/data/x` sort de la racine
    /// `/srv/data` ».
    pub fn racine_reelle(&self) -> PathBuf {
        std::fs::canonicalize(&self.racine).unwrap_or_else(|_| self.racine.clone())
    }

    /// Les verdicts, **mesurés maintenant**, sur cette racine et cette politique.
    ///
    /// L'ordre est stable (`read`, `list`, `write`, `status`, `exec`) : un client
    /// qui affiche la liste ne doit pas la voir se réordonner sous ses yeux.
    pub fn verdicts(&self) -> Vec<Verdict> {
        vec![
            self.verdict_lecture(),
            self.verdict_liste(),
            self.verdict_ecriture(),
            Verdict::ouvert("status"),
            self.verdict_exec(),
        ]
    }

    /// Les verdicts **retenus** — la sonde d'écriture n'est pas refaite à chaque
    /// lecture.
    ///
    /// Un tableau de bord interroge `status` en boucle ; refaire la sonde à chaque
    /// fois créerait et supprimerait un fichier dans la racine de l'opérateur
    /// toutes les secondes. Mémoriser par racine canonique (un processus = un
    /// nœud = une racine) évite ce bruit sans mentir sur ce qui a été mesuré :
    /// c'est la même mesure, simplement pas répétée. L'annonce au moteur, elle,
    /// repasse par `verdicts()` — c'est le moment où la vérité compte, et il est
    /// rare.
    pub fn verdicts_retenus(&self) -> Vec<Verdict> {
        static CACHE: OnceLock<Mutex<Option<(PathBuf, Vec<Verdict>)>>> = OnceLock::new();
        let cache = CACHE.get_or_init(|| Mutex::new(None));
        let cle = self.racine_reelle();
        {
            let garde = cache.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some((k, v)) = garde.as_ref() {
                if *k == cle {
                    return v.clone();
                }
            }
        }
        let mesures = self.verdicts();
        let mut garde = cache.lock().unwrap_or_else(PoisonError::into_inner);
        *garde = Some((cle, mesures.clone()));
        mesures
    }

    /// Les capacités **exercables**, et rien d'autre — la forme attendue par
    /// l'annonce au moteur (tableau de chaînes, jamais un objet).
    pub fn capacites(&self) -> Vec<String> {
        self.verdicts()
            .iter()
            .filter(|v| v.exercable)
            .map(|v| v.nom.to_string())
            .collect()
    }

    /// Les capacités refusées, avec leur motif — la moitié qu'on oublie.
    ///
    /// Une liste qui ne dit que ce qu'elle sait faire laisse croire que le reste
    /// est une question de choix. Ici, le refus est nommé et motivé.
    pub fn capacites_refusees(&self) -> Vec<Value> {
        self.verdicts()
            .iter()
            .filter(|v| !v.exercable)
            .map(Verdict::en_json)
            .collect()
    }

    /// LES DEUX MOITIÉS DE L'ANNONCE, MESURÉES EN UNE FOIS.
    ///
    /// ── POURQUOI PAS `capacites()` PUIS `capacites_refusees()` ──────────────
    /// Parce que ce sont DEUX mesures : chacune refait la sonde d'écriture, à un
    /// instant différent. Entre les deux, la racine peut changer d'état (un
    /// montage remonté, un `chmod`, un quota atteint) — et l'annonce porterait
    /// alors `write` dans les capacités **et** `write` refusé dans les refus.
    /// Deux vérités dans une réponse, c'est une de trop : c'est la faute que ce
    /// module combat déjà pour le bloc `capabilities` hérité d'`outils.rs`.
    /// Une seule mesure, deux projections — donc un ensemble et son complément,
    /// qui ne peuvent pas se contredire.
    pub fn annonce(&self) -> (Vec<String>, Vec<Value>) {
        let verdicts = self.verdicts();
        let ouvertes = verdicts
            .iter()
            .filter(|v| v.exercable)
            .map(|v| v.nom.to_string())
            .collect();
        let refusees = verdicts.iter().filter(|v| !v.exercable).map(Verdict::en_json).collect();
        (ouvertes, refusees)
    }

    /// Les LIMITES du nœud : ce qu'il accepte, et jusqu'où.
    ///
    /// Racine autorisée (déclarée **et** réelle), délai par défaut, octets gardés
    /// par flux, état de l'exécution, version du binaire. Sans elles, un moteur ne
    /// peut pas savoir ce que la machine accepte : il devine, et il devine mal.
    pub fn limites(&self) -> Value {
        json!({
            "racine": self.racine.display().to_string(),
            "racine_reelle": self.racine_reelle().display().to_string(),
            "delai_ms_par_defaut": self.delai_ms,
            "plafond_octets_par_flux": self.plafond_octets,
            "exec": if self.exec_autorise { "ouvert" } else { "ferme" },
            "version": env!("CARGO_PKG_VERSION"),
        })
    }

    /// La sonde d'écriture : une **écriture réelle** dans la racine, nettoyée.
    ///
    /// Elle reproduit les deux appels d'`outils::write` (`create_dir_all` du
    /// parent, puis `write`) pour que son échec soit le même que le sien. Le
    /// nettoyage est tenté même après un échec partiel : une sonde qui laisse un
    /// répertoire derrière elle serait un dégât, pas une mesure.
    pub fn sonde_ecriture(&self) -> Result<(), String> {
        let racine = self.racine_reelle();
        if !racine.is_dir() {
            return Err(format!(
                "écriture impossible : la racine « {} » n'est pas un répertoire atteignable",
                racine.display()
            ));
        }
        // Un nom unique : deux nœuds sur la même racine, ou deux sondes du même
        // processus, ne doivent pas se marcher dessus — sinon la seconde
        // conclurait « non inscriptible » à cause de la première.
        static COMPTEUR: AtomicU64 = AtomicU64::new(0);
        let n = COMPTEUR.fetch_add(1, Ordering::Relaxed);
        let dossier = racine.join(format!(".selfd-sonde-{}-{n}", std::process::id()));
        let fichier = dossier.join("ecriture");

        let resultat = (|| -> std::io::Result<()> {
            std::fs::create_dir(&dossier)?;
            std::fs::write(&fichier, b"sonde")?;
            Ok(())
        })();
        let _ = std::fs::remove_file(&fichier);
        let _ = std::fs::remove_dir(&dossier);

        resultat.map_err(|e| {
            format!(
                "écriture impossible à la racine « {} » ({e}) : write n'est pas exercable ici, \
                 il n'est donc pas annoncé — l'annoncer ferait proposer un geste voué au refus",
                racine.display()
            )
        })
    }

    /// Rend l'état de la politique tel qu'un client peut le lire.
    ///
    /// Les quatre clés d'origine sont INCHANGÉES (forme et sens) : des clients les
    /// lisent déjà. Ce qui s'ajoute est additif — les capacités mesurées, les
    /// refus motivés, et le bloc `limites`.
    pub fn en_json(&self) -> Value {
        let verdicts = self.verdicts_retenus();
        let ouvertes: Vec<&str> = verdicts
            .iter()
            .filter(|v| v.exercable)
            .map(|v| v.nom)
            .collect();
        let refusees: Vec<Value> = verdicts
            .iter()
            .filter(|v| !v.exercable)
            .map(Verdict::en_json)
            .collect();
        json!({
            "racine": self.racine.display().to_string(),
            "exec_autorise": self.exec_autorise,
            "plafond_octets": self.plafond_octets,
            "delai_ms_par_defaut": self.delai_ms,
            "capacites_verifiees": ouvertes,
            "capacites_refusees": refusees,
            "limites": self.limites(),
        })
    }

    fn verdict_lecture(&self) -> Verdict {
        let racine = self.racine_reelle();
        match std::fs::metadata(&racine) {
            Err(e) => {
                return Verdict::refuse(
                    "read",
                    format!("racine « {} » non atteignable ({e})", racine.display()),
                );
            }
            Ok(m) if !m.is_dir() => {
                return Verdict::refuse(
                    "read",
                    format!("racine « {} » n'est pas un répertoire", racine.display()),
                );
            }
            Ok(_) => {}
        }
        // Ouvrir la racine est la mesure la plus proche de « je peux lire ce qui
        // est dessous » sans désigner un fichier : la permission d'un fichier
        // précis reste une affaire d'appel, pas de capacité.
        match std::fs::File::open(&racine) {
            Ok(_) => Verdict::ouvert("read"),
            Err(e) => Verdict::refuse(
                "read",
                format!("ouverture impossible sous « {} » ({e})", racine.display()),
            ),
        }
    }

    fn verdict_liste(&self) -> Verdict {
        let racine = self.racine_reelle();
        match std::fs::read_dir(&racine) {
            Ok(mut flux) => {
                // On tire UNE entrée : `opendir` peut réussir et `readdir` échouer.
                // Annoncer `list` sur un répertoire qui ne rend aucune entrée
                // serait la même faute, un cran plus bas.
                let _ = flux.next();
                Verdict::ouvert("list")
            }
            Err(e) => Verdict::refuse(
                "list",
                format!("listing impossible sous « {} » ({e})", racine.display()),
            ),
        }
    }

    fn verdict_ecriture(&self) -> Verdict {
        match self.sonde_ecriture() {
            Ok(()) => Verdict::ouvert("write"),
            Err(motif) => Verdict::refuse("write", motif),
        }
    }

    fn verdict_exec(&self) -> Verdict {
        if self.exec_autorise {
            Verdict::ouvert("exec")
        } else {
            Verdict::refuse(
                "exec",
                "exécution NON AUTORISÉE : ce nœud a été démarré sans --allow-exec. \
                 Un nœud qui n'a pas été autorisé à agir observe ; il n'agit pas."
                    .to_string(),
            )
        }
    }
}

/// Le verdict d'UNE capacité sur cette machine : exercable, ou refusée et
/// POURQUOI.
///
/// `motif` est toujours renseigné quand `exercable` est faux : un refus sans
/// motif ne se corrige pas — il ressemble à une panne, et on attend.
#[derive(Debug, Clone)]
pub struct Verdict {
    /// Le nom annoncé au client (« read », « list », « write », « status », « exec »).
    pub nom: &'static str,
    /// Vrai seulement si la capacité a été exercée pour de bon pendant la mesure.
    pub exercable: bool,
    /// Le motif du refus — `None` si et seulement si `exercable`.
    pub motif: Option<String>,
}

impl Verdict {
    /// Une capacité mesurée comme exercable.
    pub fn ouvert(nom: &'static str) -> Self {
        Self { nom, exercable: true, motif: None }
    }

    /// Une capacité refusée, avec le motif qui la rend actionnable.
    pub fn refuse(nom: &'static str, motif: String) -> Self {
        Self { nom, exercable: false, motif: Some(motif) }
    }

    /// La forme lisible par un client.
    pub fn en_json(&self) -> Value {
        json!({
            "nom": self.nom,
            "exercable": self.exercable,
            "motif": self.motif,
        })
    }
}

/// Canonicalise un chemin dont le dernier segment peut ne pas exister encore.
fn canonicaliser_souple(chemin: &Path) -> Result<PathBuf> {
    if let Ok(c) = std::fs::canonicalize(chemin) {
        return Ok(c);
    }
    let parent = chemin.parent().unwrap_or(Path::new("/"));
    let nom = chemin
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    // On remonte jusqu'à un ancêtre qui existe, puis on redescend : cela résout
    // les liens symboliques RÉELS sans exiger que la cible existe.
    let base = canonicaliser_souple(parent)?;
    if nom.is_empty() {
        return Ok(base);
    }
    let mut out = base;
    for c in Path::new(&nom).components() {
        match c {
            Component::Normal(s) => out.push(s),
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            _ => {}
        }
    }
    Ok(out)
}
