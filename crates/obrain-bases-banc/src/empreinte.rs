//! Empreinte d'une arborescence : la preuve que rien n'a bougé.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// État observable d'une entrée : taille, date de modification, condensé du
/// contenu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmpreinteFichier {
    /// Taille en octets (`0` pour un répertoire).
    pub taille: u64,
    /// Date de dernière modification. Pour un répertoire, elle bouge dès
    /// qu'une entrée y est créée, supprimée ou renommée : c'est elle qui
    /// trahit un fichier créé PUIS supprimé (verrou, temporaire), que la
    /// seule liste des chemins ne verrait pas.
    pub modifie: SystemTime,
    /// Condensé FNV-1a 64 bits du contenu (`0` pour un répertoire).
    pub condense: u64,
}

/// Empreinte d'une arborescence : chaque chemin relatif (la racine comprise,
/// sous le chemin vide) et son état. Deux empreintes égales prouvent que rien
/// n'a été créé, supprimé, renommé ni modifié — même transitoirement.
pub type Empreinte = BTreeMap<PathBuf, EmpreinteFichier>;

/// Calcule l'[`Empreinte`] de `racine` (récursive).
///
/// # Errors
///
/// Toute erreur d'entrée-sortie rencontrée pendant le parcours.
pub fn empreinte(racine: &Path) -> io::Result<Empreinte> {
    let mut etat = Empreinte::new();
    etat.insert(
        PathBuf::new(),
        EmpreinteFichier {
            taille: 0,
            modifie: fs::metadata(racine)?.modified()?,
            condense: 0,
        },
    );
    empreinte_dans(racine, racine, &mut etat)?;
    Ok(etat)
}

fn empreinte_dans(racine: &Path, dossier: &Path, etat: &mut Empreinte) -> io::Result<()> {
    for entree in fs::read_dir(dossier)? {
        let chemin = entree?.path();
        let relatif = chemin
            .strip_prefix(racine)
            .expect("chemin sous la racine")
            .to_path_buf();
        let meta = fs::metadata(&chemin)?;
        if meta.is_dir() {
            etat.insert(
                relatif,
                EmpreinteFichier {
                    taille: 0,
                    modifie: meta.modified()?,
                    condense: 0,
                },
            );
            empreinte_dans(racine, &chemin, etat)?;
        } else {
            etat.insert(
                relatif,
                EmpreinteFichier {
                    taille: meta.len(),
                    modifie: meta.modified()?,
                    condense: condense_fichier(&chemin)?,
                },
            );
        }
    }
    Ok(())
}

fn condense_fichier(chemin: &Path) -> io::Result<u64> {
    let mut fichier = fs::File::open(chemin)?;
    let mut tampon = vec![0u8; 1 << 16];
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    loop {
        let lus = fichier.read(&mut tampon)?;
        if lus == 0 {
            return Ok(h);
        }
        for octet in &tampon[..lus] {
            h ^= u64::from(*octet);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}
