//! Miroir durable : le support de persistance du substrat hors système de
//! fichiers (navigateur : OPFS, en worker ; tests : mémoire avec coupure simulée).
//!
//! SÉMANTIQUE, et c'est le contrat central : une écriture n'est DURABLE qu'après
//! [`Miroir::synchroniser`]. Ce qui précède la synchronisation est visible pour le
//! processus, mais disparaît à une coupure. C'est exactement le contrat de
//! `SyncAccessHandle.flush()` côté navigateur, et celui de `fsync` côté natif.
//!
//! Au-dessus du miroir, [`EspaceMiroir`] réalise [`EspaceFichiers`] : la
//! publication atomique (`renommer`) est reconstruite à partir de DEUX emplacements
//! et d'un numéro de génération, car le déplacement d'un fichier n'est pas atomique
//! sur OPFS. Une écriture interrompue ne peut jamais rendre l'état précédent
//! illisible : le lecteur prend l'emplacement valide de génération maximale.

use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::sync::Mutex;

use crate::es::EspaceFichiers;

/// Stockage durable de blobs nommés, à écritures tamponnées jusqu'à `synchroniser`.
pub trait Miroir: Send + Sync + std::fmt::Debug {
    /// Contenu VISIBLE du blob (durable ou tamponné), `None` s'il n'existe pas.
    fn lire(&self, nom: &str) -> io::Result<Option<Vec<u8>>>;
    /// Remplace le blob par `octets` (tamponné).
    fn ecrire(&self, nom: &str, octets: &[u8]) -> io::Result<()>;
    /// Ajoute `octets` à la fin du blob, en le créant si besoin (tamponné).
    fn ajouter(&self, nom: &str, octets: &[u8]) -> io::Result<()>;
    /// Tronque le blob à `longueur` (tamponné).
    fn tronquer(&self, nom: &str, longueur: u64) -> io::Result<()>;
    /// Supprime le blob (tamponné).
    fn supprimer(&self, nom: &str) -> io::Result<()>;
    /// Rend durables TOUTES les écritures tamponnées.
    fn synchroniser(&self) -> io::Result<()>;
}

/// Miroir en mémoire, qui simule la frontière de durabilité.
///
/// Deux états : `durable` (ce qui survit à une coupure) et `tampon` (écritures
/// faites depuis la dernière synchronisation). `lire` voit les deux ; une coupure
/// ([`MiroirMemoire::couper`]) ne garde que `durable`.
#[derive(Debug, Default)]
pub struct MiroirMemoire {
    etat: Mutex<EtatMiroir>,
}

#[derive(Debug, Default, Clone)]
struct EtatMiroir {
    durable: HashMap<String, Vec<u8>>,
    /// Les blobs tamponnés, `None` = supprimé.
    tampon: HashMap<String, Option<Vec<u8>>>,
}

impl MiroirMemoire {
    /// Simule une coupure de courant : tout ce qui n'a pas été synchronisé est perdu.
    pub fn couper(&self) {
        let mut e = self.etat.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        e.tampon.clear();
    }

    fn visible(e: &EtatMiroir, nom: &str) -> Option<Vec<u8>> {
        match e.tampon.get(nom) {
            Some(v) => v.clone(),
            None => e.durable.get(nom).cloned(),
        }
    }
}

impl Miroir for MiroirMemoire {
    fn lire(&self, nom: &str) -> io::Result<Option<Vec<u8>>> {
        let e = self.etat.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(Self::visible(&e, nom))
    }

    fn ecrire(&self, nom: &str, octets: &[u8]) -> io::Result<()> {
        let mut e = self.etat.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        e.tampon.insert(nom.to_string(), Some(octets.to_vec()));
        Ok(())
    }

    fn ajouter(&self, nom: &str, octets: &[u8]) -> io::Result<()> {
        let mut e = self.etat.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut courant = Self::visible(&e, nom).unwrap_or_default();
        courant.extend_from_slice(octets);
        e.tampon.insert(nom.to_string(), Some(courant));
        Ok(())
    }

    fn tronquer(&self, nom: &str, longueur: u64) -> io::Result<()> {
        let mut e = self.etat.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut courant = Self::visible(&e, nom).unwrap_or_default();
        courant.truncate(longueur as usize);
        e.tampon.insert(nom.to_string(), Some(courant));
        Ok(())
    }

    fn supprimer(&self, nom: &str) -> io::Result<()> {
        let mut e = self.etat.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        e.tampon.insert(nom.to_string(), None);
        Ok(())
    }

    fn synchroniser(&self) -> io::Result<()> {
        let mut e = self.etat.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let tampon = std::mem::take(&mut e.tampon);
        for (nom, valeur) in tampon {
            match valeur {
                Some(v) => {
                    e.durable.insert(nom, v);
                }
                None => {
                    e.durable.remove(&nom);
                }
            }
        }
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Publication atomique par deux emplacements
// ─────────────────────────────────────────────────────────────────────────────

/// En-tête d'un emplacement : `MAGIE(4) GEN(8) LONGUEUR(4) CRC(4)` puis la charge.
const MAGIE_EMPLACEMENT: [u8; 4] = *b"OBRE";
const TAILLE_EN_TETE: usize = 20;

fn encoder_emplacement(generation: u64, charge: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(TAILLE_EN_TETE + charge.len());
    v.extend_from_slice(&MAGIE_EMPLACEMENT);
    v.extend_from_slice(&generation.to_le_bytes());
    v.extend_from_slice(&(charge.len() as u32).to_le_bytes());
    v.extend_from_slice(&crc32fast::hash(charge).to_le_bytes());
    v.extend_from_slice(charge);
    v
}

/// Décode un emplacement ; `None` s'il est tronqué, de mauvaise magie, ou de CRC faux.
fn decoder_emplacement(octets: &[u8]) -> Option<(u64, Vec<u8>)> {
    if octets.len() < TAILLE_EN_TETE || octets[0..4] != MAGIE_EMPLACEMENT {
        return None;
    }
    let generation = u64::from_le_bytes(octets[4..12].try_into().ok()?);
    let longueur = u32::from_le_bytes(octets[12..16].try_into().ok()?) as usize;
    let crc = u32::from_le_bytes(octets[16..20].try_into().ok()?);
    let charge = octets.get(TAILLE_EN_TETE..TAILLE_EN_TETE + longueur)?;
    if crc32fast::hash(charge) != crc {
        return None;
    }
    Some((generation, charge.to_vec()))
}

/// Espace de fichiers réalisé au-dessus d'un [`Miroir`].
///
/// Un nom logique `x` vit dans les blobs `x.A` et `x.B` ; une publication écrit le
/// NOUVEL état dans l'emplacement qui n'est pas le plus récent, puis synchronise.
/// Une coupure pendant cette écriture laisse l'autre emplacement valide : l'état
/// précédent reste lisible, et jamais un mélange des deux.
#[derive(Debug)]
pub struct EspaceMiroir {
    miroir: std::sync::Arc<dyn Miroir>,
}

impl EspaceMiroir {
    #[must_use]
    pub fn nouveau(miroir: std::sync::Arc<dyn Miroir>) -> Self {
        Self { miroir }
    }

    /// Emplacement valide de génération maximale pour `nom`, s'il existe.
    fn meilleur(&self, nom: &str) -> io::Result<Option<(u64, u8, Vec<u8>)>> {
        let mut meilleur: Option<(u64, u8, Vec<u8>)> = None;
        for (suffixe, lettre) in [("A", 0u8), ("B", 1u8)] {
            if let Some(octets) = self.miroir.lire(&format!("{nom}.{suffixe}"))? {
                if let Some((generation, charge)) = decoder_emplacement(&octets) {
                    if meilleur.as_ref().is_none_or(|(g, _, _)| generation > *g) {
                        meilleur = Some((generation, lettre, charge));
                    }
                }
            }
        }
        Ok(meilleur)
    }

    /// Le nom temporaire d'une publication : un blob simple, sans emplacements.
    fn tmp(nom: &str) -> String {
        format!("{nom}.tmp")
    }
}

impl EspaceFichiers for EspaceMiroir {
    fn lire_tout(&self, chemin: &Path) -> io::Result<Vec<u8>> {
        let nom = nom_de(chemin);
        match self.meilleur(&nom)? {
            Some((_, _, charge)) => Ok(charge),
            None => Err(io::Error::new(io::ErrorKind::NotFound, nom)),
        }
    }

    fn ecrire_synchronise(&self, chemin: &Path, octets: &[u8]) -> io::Result<()> {
        // Le temporaire est un blob brut, rendu durable avant la publication.
        self.miroir.ecrire(&Self::tmp(&nom_de(chemin)), octets)?;
        self.miroir.synchroniser()
    }

    fn ecrire_tout(&self, chemin: &Path, octets: &[u8]) -> io::Result<()> {
        self.miroir.ecrire(&Self::tmp(&nom_de(chemin)), octets)
    }

    fn renommer(&self, de: &Path, vers: &Path) -> io::Result<()> {
        // La publication : le contenu du temporaire devient le NOUVEL emplacement
        // le plus récent. Une seule écriture + synchronisation la rend atomique.
        let nom_vers = nom_de(vers);
        let contenu = self
            .miroir
            .lire(&Self::tmp(&nom_de(de)))?
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, nom_de(de)))?;
        let (gen_actuelle, lettre_actuelle) = match self.meilleur(&nom_vers)? {
            Some((g, l, _)) => (g, l),
            None => (0, 1), // première publication : on écrit en A
        };
        let cible = if lettre_actuelle == 0 { "B" } else { "A" };
        let nouvel = encoder_emplacement(gen_actuelle + 1, &contenu);
        self.miroir.ecrire(&format!("{nom_vers}.{cible}"), &nouvel)?;
        self.miroir.supprimer(&Self::tmp(&nom_de(de)))?;
        self.miroir.synchroniser()
    }

    fn supprimer(&self, chemin: &Path) -> io::Result<()> {
        let nom = nom_de(chemin);
        self.miroir.supprimer(&Self::tmp(&nom))?;
        self.miroir.supprimer(&format!("{nom}.A"))?;
        self.miroir.supprimer(&format!("{nom}.B"))?;
        self.miroir.synchroniser()
    }

    fn synchroniser_repertoire(&self, _repertoire: &Path) -> io::Result<()> {
        self.miroir.synchroniser()
    }

    fn existe(&self, chemin: &Path) -> bool {
        self.meilleur(&nom_de(chemin)).map(|m| m.is_some()).unwrap_or(false)
    }

    fn ouvrir_journal(&self, _chemin: &Path) -> io::Result<Box<dyn crate::es::FichierJournal>> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "journal sur miroir : étape P2",
        ))
    }

    fn ouvrir_donnees(&self, _chemin: &Path, _ecriture: bool) -> io::Result<Box<dyn crate::es::FichierDonnees>> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "réécriture en place sur miroir : hors périmètre (migration)",
        ))
    }

    fn ouvrir_verrou(&self, _chemin: &Path) -> io::Result<Box<dyn crate::es::FichierVerrou>> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "verrou de fichier sur miroir : étape P2",
        ))
    }
}

/// Nom logique d'un chemin : son nom de fichier, sans répertoire.
fn nom_de(chemin: &Path) -> String {
    chemin
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| chemin.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn espace() -> (Arc<MiroirMemoire>, EspaceMiroir) {
        let m = Arc::new(MiroirMemoire::default());
        let e = EspaceMiroir::nouveau(m.clone());
        (m, e)
    }

    #[test]
    fn une_ecriture_non_synchronisee_disparait_a_la_coupure() {
        let m = MiroirMemoire::default();
        m.ecrire("a", b"tampon").unwrap();
        assert_eq!(m.lire("a").unwrap().as_deref(), Some(&b"tampon"[..]), "visible du processus");
        m.couper();
        assert_eq!(m.lire("a").unwrap(), None, "non durable : perdu");
    }

    #[test]
    fn une_ecriture_synchronisee_survit_a_la_coupure() {
        let m = MiroirMemoire::default();
        m.ecrire("a", b"durable").unwrap();
        m.synchroniser().unwrap();
        m.couper();
        assert_eq!(m.lire("a").unwrap().as_deref(), Some(&b"durable"[..]));
    }

    #[test]
    fn publication_puis_relecture() {
        let (_m, e) = espace();
        let p = Path::new("substrate.meta");
        e.ecrire_synchronise(&Path::new("substrate.meta.tmp"), b"v1").unwrap();
        e.renommer(Path::new("substrate.meta.tmp"), p).unwrap();
        assert_eq!(e.lire_tout(p).unwrap(), b"v1");
        assert!(e.existe(p));
    }

    #[test]
    fn la_generation_la_plus_recente_gagne() {
        let (_m, e) = espace();
        let p = Path::new("x");
        for v in [&b"un"[..], b"deux", b"trois"] {
            e.ecrire_synchronise(Path::new("x.tmp"), v).unwrap();
            e.renommer(Path::new("x.tmp"), p).unwrap();
        }
        assert_eq!(e.lire_tout(p).unwrap(), b"trois");
    }

    #[test]
    fn coupure_avant_publication_garde_l_etat_precedent() {
        let (m, e) = espace();
        let p = Path::new("x");
        e.ecrire_synchronise(Path::new("x.tmp"), b"ancien").unwrap();
        e.renommer(Path::new("x.tmp"), p).unwrap();
        // Nouveau contenu tamponné, jamais publié, puis coupure.
        m.ecrire("x.tmp", b"nouveau").unwrap();
        m.couper();
        assert_eq!(e.lire_tout(p).unwrap(), b"ancien", "l'ancien état reste intact");
    }

    #[test]
    fn coupure_pendant_la_publication_ne_melange_pas() {
        let (m, e) = espace();
        let p = Path::new("x");
        e.ecrire_synchronise(Path::new("x.tmp"), b"ancien").unwrap();
        e.renommer(Path::new("x.tmp"), p).unwrap();
        // La nouvelle version est écrite dans l'emplacement libre, mais la coupure
        // survient avant la synchronisation : l'ancien emplacement reste le seul valide.
        let generation = 2u64;
        let nouvel = encoder_emplacement(generation, b"nouveau");
        m.ecrire("x.B", &nouvel).unwrap();
        m.couper();
        assert_eq!(e.lire_tout(p).unwrap(), b"ancien");
    }

    #[test]
    fn emplacement_dechire_est_ignore() {
        let (m, e) = espace();
        let p = Path::new("x");
        e.ecrire_synchronise(Path::new("x.tmp"), b"bon").unwrap();
        e.renommer(Path::new("x.tmp"), p).unwrap();
        // Un emplacement plus récent mais tronqué (coupure en plein écrit) : ignoré.
        let mut tronque = encoder_emplacement(9, b"mauvais");
        tronque.truncate(TAILLE_EN_TETE + 2);
        m.ecrire("x.B", &tronque).unwrap();
        m.synchroniser().unwrap();
        assert_eq!(e.lire_tout(p).unwrap(), b"bon");
    }

    #[test]
    fn crc_faux_est_ignore() {
        let (m, e) = espace();
        let p = Path::new("x");
        e.ecrire_synchronise(Path::new("x.tmp"), b"bon").unwrap();
        e.renommer(Path::new("x.tmp"), p).unwrap();
        let mut corrompu = encoder_emplacement(9, b"charge");
        let n = corrompu.len();
        corrompu[n - 1] ^= 0xFF;
        m.ecrire("x.B", &corrompu).unwrap();
        m.synchroniser().unwrap();
        assert_eq!(e.lire_tout(p).unwrap(), b"bon");
    }

    #[test]
    fn lire_un_nom_absent_donne_notfound() {
        let (_m, e) = espace();
        let err = e.lire_tout(Path::new("absent")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(!e.existe(Path::new("absent")));
    }
}
