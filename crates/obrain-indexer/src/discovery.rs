//! Découverte des fichiers d'un dépôt (RFC M6 §2.3) — et ce qu'on refuse d'indexer.
//!
//! # Pourquoi la liste d'exclusions est un sujet, et pas un détail
//!
//! Mesuré sur `runtime-dev`, les douze premiers nœuds du graphe par degré
//! sortant étaient **onze artéfacts** : des sorties de build minifiées
//! (`aletheia/dist/static/assets/_app-…`, 28 368 et 23 792 arêtes) et un fichier
//! de verrouillage (`deepseek-harness/pnpm-lock.yaml`, 17 585 arêtes `DEFINES`).
//! **94 % des arêtes des douze premiers hubs.**
//!
//! Sur un dépôt indexé sans ces artéfacts (un monde de code), **zéro** hub sur
//! huit est un artéfact. Même moteur, même résolution : seule la politique de
//! parcours différait. Un hub qui dit « ce fichier est énorme » ne dit rien de
//! l'architecture — il dit comment on a indexé.
//!
//! D'où deux exigences tenues ici :
//!
//! 1. **Chaque exclusion porte sa raison.** Un `continue` silencieux rend la
//!    politique invisible : on ne peut ni la compter, ni la contester, ni mesurer
//!    son effet. [`Exclusion`] et [`DiscoveryResult::par_raison`] existent pour
//!    cela ;
//! 2. **La politique est un choix, réversible.** `exclure_artefacts` (défaut
//!    vrai) se désactive — c'est ce qui permet de **mesurer** avant/après sur le
//!    même dépôt, au lieu d'affirmer que c'est mieux.
//!
//! Ce qui n'est **jamais** inclus, quelle que soit l'option : le versionnage
//! (`.git`…) et les fichiers au-delà du budget d'octets.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Pourquoi un chemin a été écarté de l'indexation.
///
/// Chaque variante correspond à une **famille de décision**, pour qu'un total par
/// raison veuille dire quelque chose : « 3 200 sorties de build » est
/// exploitable, « 3 201 exclus » ne l'est pas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Raison {
    /// `.git`, `.hg`… — l'histoire du dépôt, jamais du code source.
    Versionnage,
    /// `node_modules`, `vendor`, `third_party`… — du code, mais pas le nôtre.
    Dependances,
    /// `dist`, `build`, `.vercel`… — un **état** de construction, pas une intention.
    SortieDeBuild,
    /// `.venv`, `__pycache__`, `.cargo`… — de l'outillage, pas un projet.
    Environnement,
    /// `pnpm-lock.yaml`, `Cargo.lock`… — une résolution de dépendances, pas du code.
    Verrouillage,
    /// `*.min.js`… — lisible par un parseur, illisible par un humain, et plein
    /// d'identifiants courts qui fabriquent des collisions de noms.
    Minifie,
    /// Au-delà du budget d'octets (RFC M6 §2.2).
    TropGros,
}

impl Raison {
    /// Un **artéfact** décrit un état — d'une construction, d'une résolution de
    /// dépendances, d'un environnement — et non une intention de code.
    ///
    /// La distinction sert à `exclure_artefacts` : le versionnage et le budget
    /// d'octets ne sont pas des artéfacts, ils sont hors sujet par nature.
    pub fn est_artefact(self) -> bool {
        matches!(
            self,
            Raison::Dependances
                | Raison::SortieDeBuild
                | Raison::Environnement
                | Raison::Verrouillage
                | Raison::Minifie
        )
    }

    /// Libellé court, pour un rapport lisible.
    pub fn libelle(self) -> &'static str {
        match self {
            Raison::Versionnage => "versionnage",
            Raison::Dependances => "dépendances",
            Raison::SortieDeBuild => "sortie de build",
            Raison::Environnement => "environnement",
            Raison::Verrouillage => "verrouillage",
            Raison::Minifie => "minifié",
            Raison::TropGros => "trop gros",
        }
    }
}

/// Un chemin écarté, **et pourquoi**.
#[derive(Debug, Clone)]
pub struct Exclusion {
    pub chemin: PathBuf,
    pub raison: Raison,
}

/// Résultat de la découverte d'un dépôt.
#[derive(Debug, Clone)]
pub struct DiscoveryResult {
    /// Fichiers sources découverts (chemins absolus ou relatifs au root).
    pub files: Vec<PathBuf>,
    /// Fichiers et répertoires écartés, chacun avec sa raison.
    pub excluded: Vec<Exclusion>,
}

impl DiscoveryResult {
    /// Combien d'exclusions par raison — c'est ce qui rend la politique
    /// **comptable**, donc contestable.
    pub fn par_raison(&self) -> BTreeMap<&'static str, usize> {
        let mut m = BTreeMap::new();
        for e in &self.excluded {
            *m.entry(e.raison.libelle()).or_insert(0) += 1;
        }
        m
    }

    /// Nombre d'exclusions qui sont des artéfacts.
    pub fn nb_artefacts(&self) -> usize {
        self.excluded
            .iter()
            .filter(|e| e.raison.est_artefact())
            .count()
    }
}

/// Options de découverte.
#[derive(Debug, Clone)]
pub struct DiscoveryOptions {
    /// Budget maximal par fichier en octets (défaut 5 Mo — budget C
    /// `CBM_EXTRACT_BUDGET`, RFC M6 §2.2).
    pub max_file_bytes: u64,
    /// Activer le suivi du .cbmignore.
    pub honor_cbmignore: bool,
    /// Écarter les artéfacts : dépendances, sorties de build, environnements,
    /// fichiers de verrouillage, fichiers minifiés.
    ///
    /// **Vrai par défaut**, et ce défaut est le résultat d'une mesure : sur
    /// `runtime-dev`, les artéfacts portaient 94 % des arêtes des douze premiers
    /// hubs. Le passer à faux est légitime — pour indexer une dépendance
    /// vendorisée qu'on étudie, ou pour mesurer l'écart — et c'est justement
    /// pourquoi c'est une option et non une constante.
    pub exclure_artefacts: bool,
}

impl Default for DiscoveryOptions {
    fn default() -> Self {
        Self {
            max_file_bytes: 5 * 1024 * 1024,
            honor_cbmignore: true,
            exclure_artefacts: true,
        }
    }
}

/// Répertoires d'histoire du dépôt — **jamais** indexés.
const REPERTOIRES_VERSIONNAGE: &[&str] = &[".git", ".hg", ".svn", ".bzr"];

/// Répertoires de dépendances : du code, mais pas celui du projet.
const REPERTOIRES_DEPENDANCES: &[&str] = &[
    "node_modules",
    "bower_components",
    "vendor",
    "third_party",
    "jspm_packages",
];

/// Sorties de construction : un **état**, pas une intention.
const REPERTOIRES_BUILD: &[&str] = &[
    "dist",
    "build",
    "out",
    ".next",
    ".nuxt",
    ".output",
    ".svelte-kit",
    ".vercel",
    ".netlify",
    "target",
    "coverage",
    "htmlcov",
];

/// Outillage et environnements : jamais un projet.
const REPERTOIRES_ENVIRONNEMENT: &[&str] = &[
    ".venv",
    "venv",
    ".tox",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    "__pycache__",
    ".cargo",
    ".gradle",
    ".idea",
    ".vscode",
    ".cache",
];

/// Fichiers de verrouillage : une résolution de dépendances, pas du code.
const FICHIERS_VERROUILLAGE: &[&str] = &[
    "pnpm-lock.yaml",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "bun.lockb",
    "Cargo.lock",
    "poetry.lock",
    "Pipfile.lock",
    "uv.lock",
    "composer.lock",
    "Gemfile.lock",
    "go.sum",
    "flake.lock",
];

/// Suffixes de fichiers minifiés : lisibles par un parseur, pas par un humain —
/// et pleins d'identifiants courts.
const SUFFIXES_MINIFIES: &[&str] = &[
    ".min.js",
    ".min.mjs",
    ".min.cjs",
    ".min.css",
    ".bundle.js",
    ".chunk.js",
];

/// Rend la raison si le nom est écarté, sinon `None`.
fn classer(nom: &str, est_repertoire: bool, exclure_artefacts: bool) -> Option<Raison> {
    // Hors sujet par nature : jamais inclus, quelle que soit l'option.
    if est_repertoire && REPERTOIRES_VERSIONNAGE.contains(&nom) {
        return Some(Raison::Versionnage);
    }
    if !exclure_artefacts {
        return None;
    }
    if est_repertoire {
        if REPERTOIRES_DEPENDANCES.contains(&nom) {
            return Some(Raison::Dependances);
        }
        if REPERTOIRES_BUILD.contains(&nom) {
            return Some(Raison::SortieDeBuild);
        }
        if REPERTOIRES_ENVIRONNEMENT.contains(&nom) {
            return Some(Raison::Environnement);
        }
    } else {
        let bas = nom.to_ascii_lowercase();
        if FICHIERS_VERROUILLAGE
            .iter()
            .any(|l| l.eq_ignore_ascii_case(&bas))
        {
            return Some(Raison::Verrouillage);
        }
        if SUFFIXES_MINIFIES.iter().any(|s| bas.ends_with(s)) {
            return Some(Raison::Minifie);
        }
    }
    None
}

/// Découvre les fichiers indexables sous `root`.
///
/// Liste récursive, exclusions **comptées et motivées** (voir [`classer`]), budget
/// par fichier, ordre déterministe pour des indexations reproductibles.
///
/// `honor_cbmignore` reste déclaré et **n'est pas encore appliqué** : le suivi de
/// `.cbmignore`/`.gitignore` (RFC M6 §5) viendra après. Une exclusion qu'on lit
/// sans l'appliquer serait pire que pas d'exclusion du tout, parce qu'on la
/// croirait en vigueur — c'est écrit ici pour que personne ne s'y trompe.
pub fn discover(root: &Path, opts: &DiscoveryOptions) -> DiscoveryResult {
    let mut files = Vec::new();
    let mut excluded = Vec::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let nom = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            let is_file = entry.file_type().map(|t| t.is_file()).unwrap_or(false);

            if is_dir {
                match classer(&nom, true, opts.exclure_artefacts) {
                    Some(raison) => excluded.push(Exclusion {
                        chemin: path,
                        raison,
                    }),
                    None => stack.push(path),
                }
            } else if is_file {
                match classer(&nom, false, opts.exclure_artefacts) {
                    Some(raison) => excluded.push(Exclusion {
                        chemin: path,
                        raison,
                    }),
                    None => {
                        let trop_gros = entry
                            .metadata()
                            .map(|m| m.len() > opts.max_file_bytes)
                            .unwrap_or(false);
                        if trop_gros {
                            excluded.push(Exclusion {
                                chemin: path,
                                raison: Raison::TropGros,
                            });
                        } else {
                            files.push(path);
                        }
                    }
                }
            }
        }
    }
    // Ordre déterministe pour des indexations reproductibles.
    files.sort();
    excluded.sort_by(|a, b| a.chemin.cmp(&b.chemin));
    DiscoveryResult { files, excluded }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn decouvre_les_fichiers_sources() {
        let td = tempfile::tempdir().unwrap();
        fs::create_dir_all(td.path().join("src")).unwrap();
        fs::write(td.path().join("src/main.rs"), "fn main() {}").unwrap();
        fs::write(td.path().join("Cargo.toml"), "[package]").unwrap();
        fs::create_dir_all(td.path().join(".git")).unwrap();
        fs::write(td.path().join(".git/config"), "x").unwrap();

        let r = discover(td.path(), &DiscoveryOptions::default());
        assert!(r.files.iter().any(|f| f.ends_with("src/main.rs")));
        assert!(r.files.iter().any(|f| f.ends_with("Cargo.toml")));
        assert!(!r.files.iter().any(|f| f.to_string_lossy().contains(".git")));
        // L'exclusion est COMPTÉE, pas silencieuse.
        assert!(r.excluded.iter().any(|e| e.raison == Raison::Versionnage));
    }

    #[test]
    fn budget_taille_respecte() {
        let td = tempfile::tempdir().unwrap();
        fs::write(td.path().join("big.rs"), vec![0u8; 10 * 1024 * 1024]).unwrap();
        fs::write(td.path().join("small.rs"), "fn f() {}".as_bytes()).unwrap();
        let r = discover(td.path(), &DiscoveryOptions::default());
        assert!(r.files.iter().any(|f| f.ends_with("small.rs")));
        assert!(
            r.excluded
                .iter()
                .any(|e| e.chemin.ends_with("big.rs") && e.raison == Raison::TropGros)
        );
    }

    #[test]
    fn les_artefacts_mesures_sont_ecartes_et_motives() {
        let td = tempfile::tempdir().unwrap();
        let p = td.path();
        // Exactement ce qui a été MESURÉ comme contaminant le graphe :
        fs::create_dir_all(p.join("dist/static/assets")).unwrap();
        fs::write(
            p.join("dist/static/assets/_app-B22zkXm.js"),
            "function t(){}",
        )
        .unwrap();
        fs::create_dir_all(p.join("web/git/vendor")).unwrap();
        fs::write(p.join("web/git/vendor/three.module.js"), "class A{}").unwrap();
        fs::create_dir_all(p.join("node_modules/x")).unwrap();
        fs::write(p.join("node_modules/x/i.js"), "x").unwrap();
        fs::write(p.join("pnpm-lock.yaml"), "lockfileVersion: 9").unwrap();
        fs::write(p.join("app.min.js"), "var a=1").unwrap();
        // …et ce qui doit RESTER :
        fs::create_dir_all(p.join("src")).unwrap();
        fs::write(p.join("src/main.rs"), "fn main(){}").unwrap();

        let r = discover(p, &DiscoveryOptions::default());
        assert!(
            r.files.iter().any(|f| f.ends_with("src/main.rs")),
            "le source doit rester"
        );
        assert!(!r.files.iter().any(|f| f.to_string_lossy().contains("dist")));
        assert!(
            !r.files
                .iter()
                .any(|f| f.to_string_lossy().contains("vendor"))
        );
        assert!(
            !r.files
                .iter()
                .any(|f| f.to_string_lossy().contains("node_modules"))
        );
        assert!(!r.files.iter().any(|f| f.ends_with("pnpm-lock.yaml")));

        let compte = r.par_raison();
        assert!(compte.contains_key("sortie de build"), "{compte:?}");
        assert!(compte.contains_key("dépendances"), "{compte:?}");
        assert!(compte.contains_key("verrouillage"), "{compte:?}");
        assert!(compte.contains_key("minifié"), "{compte:?}");
        assert!(r.nb_artefacts() >= 5);
    }

    #[test]
    fn la_politique_est_reversible_pour_pouvoir_mesurer() {
        let td = tempfile::tempdir().unwrap();
        let p = td.path();
        fs::create_dir_all(p.join("vendor")).unwrap();
        fs::write(p.join("vendor/lib.js"), "function v(){}").unwrap();
        fs::create_dir_all(p.join("src")).unwrap();
        fs::write(p.join("src/main.rs"), "fn main(){}").unwrap();

        let avec = discover(p, &DiscoveryOptions::default());
        assert!(
            !avec
                .files
                .iter()
                .any(|f| f.to_string_lossy().contains("vendor"))
        );

        let sans = discover(
            p,
            &DiscoveryOptions {
                exclure_artefacts: false,
                ..Default::default()
            },
        );
        assert!(
            sans.files
                .iter()
                .any(|f| f.to_string_lossy().contains("vendor")),
            "sans la politique, le vendoris revient — c'est ce qui permet le avant/après"
        );
    }
}
