//! CACHE D'EXTRACTION (T27) — un fichier inchangé ne repasse pas par tree-sitter.
//!
//! ── POURQUOI ────────────────────────────────────────────────────────────────
//! Sur le noyau Linux, l'analyse tree-sitter seule coûte ≈ 230 s de CPU (989 Mo
//! de C, 190 M de nœuds syntaxiques) : le premier index en est borné. Une
//! RÉINDEXATION, elle, retrouve presque tous les fichiers identiques. Lire et
//! empreinter 1 Go coûte 1 à 2 s ; réanalyser, des minutes de CPU.
//!
//! ── CE QUI REND LE CACHE SÛR ────────────────────────────────────────────────
//! Une entrée n'est servie que si TROIS choses concordent :
//! * le chemin du fichier ;
//! * l'empreinte xxh3-128 de son CONTENU (pas sa date : un `git checkout` ne
//!   dit pas la vérité sur les dates) ;
//! * l'empreinte du BINAIRE qui a produit l'entrée. Toute recompilation de
//!   l'extracteur invalide donc tout le cache — sans numéro de version à
//!   penser à incrémenter, ce qu'on finit toujours par oublier.
//! Au moindre doute (fichier illisible, format inconnu, binaire différent), le
//! cache est ignoré : on réanalyse, jamais on ne sert du périmé.
//!
//! ── FORMAT ──────────────────────────────────────────────────────────────────
//! `MAGIC · version · empreinte binaire (16 o) · N × (longueur u64 · entrée)`,
//! chaque entrée en bincode indépendant : la désérialisation se fait en
//! parallèle.
//!
//! ── ACTIVATION EXPLICITE ─────────────────────────────────────────────────────
//! Le cache n'existe que si un dossier est désigné : `IndexOptions::cache_dir`,
//! sinon `$OBRAIN_INDEX_CACHE_DIR`. Pas d'emplacement par défaut : chaque test
//! qui indexe un dépôt temporaire y déposerait un fichier que rien ne ramasse.
//! Un fichier par racine de dépôt. `OBRAIN_INDEX_CACHE=0` le coupe même si un
//! dossier est désigné. Sur HOTE, la doctrine de stockage le range sous
//! `/CHEMIN/obrain/caches/extraction`.

use rayon::prelude::*;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::parse::FileExtraction;

const MAGIC: &[u8; 8] = b"OBRXCACH";
const VERSION: u32 = 1;

/// Empreinte d'un contenu.
#[must_use]
pub fn empreinte(octets: &[u8]) -> u128 {
    twox_hash::XxHash3_128::oneshot(octets)
}

/// Empreinte du binaire courant, calculée une fois par processus. `None` si
/// l'exécutable ne se relit pas — le cache est alors désactivé.
pub(crate) fn empreinte_binaire() -> Option<u128> {
    static E: OnceLock<Option<u128>> = OnceLock::new();
    *E.get_or_init(|| {
        let exe = std::env::current_exe().ok()?;
        let octets = std::fs::read(exe).ok()?;
        Some(empreinte(&octets))
    })
}

/// Le dossier du cache : celui des options, sinon celui de l'environnement,
/// sinon aucun (cache désactivé).
fn dossier(choisi: Option<&Path>) -> Option<PathBuf> {
    if std::env::var("OBRAIN_INDEX_CACHE").is_ok_and(|v| v == "0") {
        return None;
    }
    choisi.map(Path::to_path_buf).or_else(|| {
        std::env::var("OBRAIN_INDEX_CACHE_DIR")
            .ok()
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
    })
}

fn fichier_pour(racine: &Path, choisi: Option<&Path>) -> Option<PathBuf> {
    let d = dossier(choisi)?;
    let absolu = std::fs::canonicalize(racine).unwrap_or_else(|_| racine.to_path_buf());
    let cle = empreinte(absolu.to_string_lossy().as_bytes());
    Some(d.join(format!("{cle:032x}.bin")))
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Entree {
    chemin: String,
    empreinte: u128,
    extraction: FileExtraction,
}

#[derive(serde::Serialize)]
struct EntreeRef<'a> {
    chemin: &'a str,
    empreinte: u128,
    extraction: &'a FileExtraction,
}

/// Le cache chargé pour UNE indexation. Chaque entrée ne peut être prise
/// qu'une fois (elle est DÉPLACÉE vers l'indexation, pas copiée).
pub struct Cache {
    entrees: HashMap<String, Mutex<Option<(u128, FileExtraction)>>>,
}

impl Cache {
    /// Charge le cache d'un dépôt, ou `None` s'il n'y en a pas d'utilisable.
    #[must_use]
    pub fn charger(racine: &Path, choisi: Option<&Path>) -> Option<Self> {
        let fichier = fichier_pour(racine, choisi)?;
        let binaire = empreinte_binaire()?;
        let octets = std::fs::read(fichier).ok()?;
        if octets.len() < 28 || &octets[..8] != MAGIC {
            return None;
        }
        if u32::from_le_bytes(octets[8..12].try_into().ok()?) != VERSION
            || u128::from_le_bytes(octets[12..28].try_into().ok()?) != binaire
        {
            return None; // autre binaire, autre extracteur : on réanalyse
        }
        let mut tranches: Vec<&[u8]> = Vec::new();
        let mut i = 28usize;
        while i < octets.len() {
            let n = u64::from_le_bytes(octets.get(i..i + 8)?.try_into().ok()?) as usize;
            i += 8;
            tranches.push(octets.get(i..i + n)?);
            i += n;
        }
        let entrees: Option<Vec<Entree>> = tranches
            .par_iter()
            .map(|t| {
                bincode::serde::decode_from_slice::<Entree, _>(t, bincode::config::standard())
                    .ok()
                    .map(|(e, _)| e)
            })
            .collect();
        let entrees = entrees?; // une seule entrée illisible : on ignore tout
        Some(Self {
            entrees: entrees
                .into_iter()
                .map(|e| (e.chemin, Mutex::new(Some((e.empreinte, e.extraction)))))
                .collect(),
        })
    }

    /// L'extraction en cache pour ce fichier, si son contenu n'a pas changé.
    #[must_use]
    pub fn prendre(&self, chemin: &str, empreinte_contenu: u128) -> Option<FileExtraction> {
        let case = self.entrees.get(chemin)?;
        let mut garde = case.lock().ok()?;
        match garde.take() {
            Some((e, ex)) if e == empreinte_contenu => Some(ex),
            autre => {
                *garde = autre; // contenu changé : on laisse l'entrée, on réanalyse
                None
            }
        }
    }
}

/// Écrit le cache d'un dépôt : `(chemin, empreinte, extraction)` de CHAQUE
/// fichier indexé — les fichiers disparus en sortent d'eux-mêmes. Écriture
/// atomique (fichier temporaire puis renommage). Les erreurs sont rendues à
/// l'appelant, qui peut les ignorer : un cache absent ne fausse rien.
pub fn ecrire<'a, I>(racine: &Path, choisi: Option<&Path>, entrees: I) -> std::io::Result<()>
where
    I: IntoParallelIterator<Item = (&'a str, u128, &'a FileExtraction)>,
{
    let (Some(cible), Some(binaire)) = (fichier_pour(racine, choisi), empreinte_binaire()) else {
        return Ok(());
    };
    let blocs: Vec<Vec<u8>> = entrees
        .into_par_iter()
        .map(|(chemin, empreinte, extraction)| {
            bincode::serde::encode_to_vec(
                EntreeRef {
                    chemin,
                    empreinte,
                    extraction,
                },
                bincode::config::standard(),
            )
            .unwrap_or_default()
        })
        .collect();
    if let Some(d) = cible.parent() {
        std::fs::create_dir_all(d)?;
    }
    let temp = cible.with_extension(format!("tmp-{}", std::process::id()));
    {
        let mut f = std::io::BufWriter::with_capacity(8 << 20, std::fs::File::create(&temp)?);
        f.write_all(MAGIC)?;
        f.write_all(&VERSION.to_le_bytes())?;
        f.write_all(&binaire.to_le_bytes())?;
        for b in &blocs {
            if b.is_empty() {
                continue; // encodage impossible : l'entrée manquera, sans plus
            }
            f.write_all(&(b.len() as u64).to_le_bytes())?;
            f.write_all(b)?;
        }
        f.flush()?;
    }
    std::fs::rename(&temp, &cible)
}

/// (T27) Empreinte d'un DÉPÔT tel que l'indexeur le verrait : ensemble des
/// fichiers indexés (chemin + empreinte de contenu, triés), binaire de
/// l'extracteur, et contexte git (le graphe porte un nœud de branche et
/// l'historique de co-évolution, qui bougent avec la tête même à contenu égal).
/// Deux empreintes égales ⇒ le graphe produit serait le même : une
/// réindexation n'a rien à faire.
pub(crate) fn combiner(fichiers: &mut [(&str, u128)], git: &crate::git_ctx::GitContext) -> u128 {
    fichiers.sort_unstable_by(|a, b| a.0.cmp(b.0));
    let mut tampon: Vec<u8> = Vec::with_capacity(fichiers.len() * 80 + 256);
    tampon.extend_from_slice(b"obrain-empreinte-depot/1\0");
    tampon.extend_from_slice(&empreinte_binaire().unwrap_or(0).to_le_bytes());
    tampon.extend_from_slice(format!("{git:?}").as_bytes());
    for (chemin, e) in fichiers.iter() {
        tampon.extend_from_slice(chemin.as_bytes());
        tampon.push(0);
        tampon.extend_from_slice(&e.to_le_bytes());
    }
    empreinte(&tampon)
}
