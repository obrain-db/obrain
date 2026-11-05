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
        let mut e = self
            .etat
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        let e = self
            .etat
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(Self::visible(&e, nom))
    }

    fn ecrire(&self, nom: &str, octets: &[u8]) -> io::Result<()> {
        let mut e = self
            .etat
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        e.tampon.insert(nom.to_string(), Some(octets.to_vec()));
        Ok(())
    }

    fn ajouter(&self, nom: &str, octets: &[u8]) -> io::Result<()> {
        let mut e = self
            .etat
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut courant = Self::visible(&e, nom).unwrap_or_default();
        courant.extend_from_slice(octets);
        e.tampon.insert(nom.to_string(), Some(courant));
        Ok(())
    }

    fn tronquer(&self, nom: &str, longueur: u64) -> io::Result<()> {
        let mut e = self
            .etat
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut courant = Self::visible(&e, nom).unwrap_or_default();
        courant.truncate(longueur as usize);
        e.tampon.insert(nom.to_string(), Some(courant));
        Ok(())
    }

    fn supprimer(&self, nom: &str) -> io::Result<()> {
        let mut e = self
            .etat
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        e.tampon.insert(nom.to_string(), None);
        Ok(())
    }

    fn synchroniser(&self) -> io::Result<()> {
        let mut e = self
            .etat
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
            if let Some(octets) = self.miroir.lire(&format!("{nom}.{suffixe}"))?
                && let Some((generation, charge)) = decoder_emplacement(&octets)
                && meilleur.as_ref().is_none_or(|(g, _, _)| generation > *g)
            {
                meilleur = Some((generation, lettre, charge));
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
        self.miroir
            .ecrire(&format!("{nom_vers}.{cible}"), &nouvel)?;
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
        self.meilleur(&nom_de(chemin))
            .map(|m| m.is_some())
            .unwrap_or(false)
    }

    fn ouvrir_journal(&self, _chemin: &Path) -> io::Result<Box<dyn crate::es::FichierJournal>> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "journal sur miroir : étape P2",
        ))
    }

    fn ouvrir_donnees(
        &self,
        _chemin: &Path,
        _ecriture: bool,
    ) -> io::Result<Box<dyn crate::es::FichierDonnees>> {
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
        assert_eq!(
            m.lire("a").unwrap().as_deref(),
            Some(&b"tampon"[..]),
            "visible du processus"
        );
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
        e.ecrire_synchronise(Path::new("substrate.meta.tmp"), b"v1")
            .unwrap();
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
        assert_eq!(
            e.lire_tout(p).unwrap(),
            b"ancien",
            "l'ancien état reste intact"
        );
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

// ─────────────────────────────────────────────────────────────────────────────
// Journal de blobs : un miroir réel, sur UN fichier (P2)
// ─────────────────────────────────────────────────────────────────────────────

/// En-tête d'un enregistrement : `MAGIE(4) TAG(1) LONG_NOM(2) LONG_VAL(4)`, puis
/// le nom, la valeur, et un CRC32 sur tout ce qui précède.
const MAGIE_ENREG: [u8; 4] = *b"OBRL";
const TAG_PUT: u8 = 1;
const TAG_APPEND: u8 = 2;
const TAG_TRUNC: u8 = 3;
const TAG_DEL: u8 = 4;

/// Un enregistrement décodé.
#[derive(Debug, PartialEq, Eq)]
enum Enreg {
    Put(String, Vec<u8>),
    Append(String, Vec<u8>),
    Trunc(String, u64),
    Del(String),
}

fn encoder_enreg(e: &Enreg) -> Vec<u8> {
    let (tag, nom, val): (u8, &str, Vec<u8>) = match e {
        Enreg::Put(n, v) => (TAG_PUT, n, v.clone()),
        Enreg::Append(n, v) => (TAG_APPEND, n, v.clone()),
        Enreg::Trunc(n, l) => (TAG_TRUNC, n, l.to_le_bytes().to_vec()),
        Enreg::Del(n) => (TAG_DEL, n, Vec::new()),
    };
    let mut out = Vec::with_capacity(11 + nom.len() + val.len() + 4);
    out.extend_from_slice(&MAGIE_ENREG);
    out.push(tag);
    out.extend_from_slice(&(nom.len() as u16).to_le_bytes());
    out.extend_from_slice(&(val.len() as u32).to_le_bytes());
    out.extend_from_slice(nom.as_bytes());
    out.extend_from_slice(&val);
    let crc = crc32fast::hash(&out);
    out.extend_from_slice(&crc.to_le_bytes());
    out
}

/// Décode le premier enregistrement de `octets`. `Ok((enreg, longueur))`, ou `Err`
/// si l'enregistrement est absent, tronqué ou corrompu : c'est la fin valide du journal.
fn decoder_enreg(octets: &[u8]) -> Result<(Enreg, usize), ()> {
    if octets.len() < 11 || octets[0..4] != MAGIE_ENREG {
        return Err(());
    }
    let tag = octets[4];
    let lnom = u16::from_le_bytes([octets[5], octets[6]]) as usize;
    let lval = u32::from_le_bytes([octets[7], octets[8], octets[9], octets[10]]) as usize;
    let total = 11 + lnom + lval + 4;
    if octets.len() < total {
        return Err(());
    }
    let crc = u32::from_le_bytes(octets[total - 4..total].try_into().map_err(|_| ())?);
    if crc32fast::hash(&octets[..total - 4]) != crc {
        return Err(());
    }
    let nom = std::str::from_utf8(&octets[11..11 + lnom])
        .map_err(|_| ())?
        .to_string();
    let val = octets[11 + lnom..11 + lnom + lval].to_vec();
    let enreg = match tag {
        TAG_PUT => Enreg::Put(nom, val),
        TAG_APPEND => Enreg::Append(nom, val),
        TAG_TRUNC => {
            let arr: [u8; 8] = val.as_slice().try_into().map_err(|_| ())?;
            Enreg::Trunc(nom, u64::from_le_bytes(arr))
        }
        TAG_DEL => Enreg::Del(nom),
        _ => return Err(()),
    };
    Ok((enreg, total))
}

/// Miroir réel, sur un journal append-only : chaque opération est un enregistrement
/// à CRC, l'état courant se reconstruit en rejouant le journal à l'ouverture.
///
/// Reprise : à l'ouverture, on rejoue les enregistrements valides ; le premier
/// déchiré ou corrompu marque la FIN du journal, et tout ce qui le suit est
/// tronqué. Une coupure en plein enregistrement ne peut donc jamais produire un état
/// mélangé.
#[derive(Debug)]
pub struct MiroirJournal {
    fichier: std::sync::Mutex<Box<dyn crate::es::FichierJournal>>,
    etat: Mutex<HashMap<String, Vec<u8>>>,
}

impl MiroirJournal {
    /// Ouvre le journal et rejoue ses enregistrements valides.
    pub fn ouvrir(fichier: Box<dyn crate::es::FichierJournal>) -> io::Result<Self> {
        let mut fichier = fichier;
        let taille = fichier.taille()? as usize;
        let mut octets = vec![0u8; taille];
        if taille > 0 {
            fichier.lire_a(0, &mut octets)?;
        }
        let mut etat: HashMap<String, Vec<u8>> = HashMap::new();
        let mut pos = 0usize;
        while pos < octets.len() {
            match decoder_enreg(&octets[pos..]) {
                Ok((enreg, longueur)) => {
                    appliquer(&mut etat, enreg);
                    pos += longueur;
                }
                Err(()) => break,
            }
        }
        if pos < taille {
            // Fin valide dépassée : on abandonne le reste (coupure en cours d'écriture).
            fichier.tronquer(pos as u64)?;
            fichier.synchroniser(true)?;
        }
        Ok(Self {
            fichier: std::sync::Mutex::new(fichier),
            etat: Mutex::new(etat),
        })
    }

    fn ajouter_enreg(&self, e: Enreg) -> io::Result<()> {
        let octets = encoder_enreg(&e);
        let mut f = self
            .fichier
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f.ajouter(&octets)?;
        drop(f);
        let mut etat = self
            .etat
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        appliquer(&mut etat, e);
        Ok(())
    }
}

fn appliquer(etat: &mut HashMap<String, Vec<u8>>, e: Enreg) {
    match e {
        Enreg::Put(n, v) => {
            etat.insert(n, v);
        }
        Enreg::Append(n, v) => etat.entry(n).or_default().extend_from_slice(&v),
        Enreg::Trunc(n, l) => {
            if let Some(b) = etat.get_mut(&n) {
                b.truncate(l as usize);
            }
        }
        Enreg::Del(n) => {
            etat.remove(&n);
        }
    }
}

impl Miroir for MiroirJournal {
    fn lire(&self, nom: &str) -> io::Result<Option<Vec<u8>>> {
        let etat = self
            .etat
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(etat.get(nom).cloned())
    }

    fn ecrire(&self, nom: &str, octets: &[u8]) -> io::Result<()> {
        self.ajouter_enreg(Enreg::Put(nom.to_string(), octets.to_vec()))
    }

    fn ajouter(&self, nom: &str, octets: &[u8]) -> io::Result<()> {
        self.ajouter_enreg(Enreg::Append(nom.to_string(), octets.to_vec()))
    }

    fn tronquer(&self, nom: &str, longueur: u64) -> io::Result<()> {
        self.ajouter_enreg(Enreg::Trunc(nom.to_string(), longueur))
    }

    fn supprimer(&self, nom: &str) -> io::Result<()> {
        self.ajouter_enreg(Enreg::Del(nom.to_string()))
    }

    fn synchroniser(&self) -> io::Result<()> {
        let mut f = self
            .fichier
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f.synchroniser(true)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Persistance du store sur un miroir (P1.2)
// ─────────────────────────────────────────────────────────────────────────────

/// Nom logique de la dernière image du store sur le miroir.
const IMAGE_STORE: &str = "store.image";

impl crate::store::SubstrateStore {
    /// Ouvre le store depuis le miroir : restaure la DERNIÈRE image publiée, ou
    /// rend un store vide si aucune n'a jamais été publiée.
    ///
    /// Une image présente mais illisible est une ERREUR, jamais un store vide
    /// silencieux : un store vide à la place d'une base perdrait les données sans
    /// rien signaler.
    pub fn ouvrir_sur_miroir(
        miroir: std::sync::Arc<dyn Miroir>,
    ) -> crate::error::SubstrateResult<Self> {
        let espace = EspaceMiroir::nouveau(miroir);
        let chemin = Path::new(IMAGE_STORE);
        if espace.existe(chemin) {
            let image = espace.lire_tout(chemin)?;
            Self::depuis_instantane(&image)
        } else {
            Self::open_memory()
        }
    }

    /// Publie l'état courant dans le miroir. Durable quand cet appel rend `Ok` :
    /// l'image précédente reste lisible à toute coupure pendant la publication.
    pub fn persister_sur_miroir(
        &self,
        miroir: std::sync::Arc<dyn Miroir>,
    ) -> crate::error::SubstrateResult<()> {
        let espace = EspaceMiroir::nouveau(miroir);
        let image = self.instantane()?;
        let tmp = Path::new("store.image.tmp");
        espace.ecrire_synchronise(tmp, &image)?;
        espace.renommer(tmp, Path::new(IMAGE_STORE))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests_store {
    use super::*;
    use obrain_common::types::Value;
    use obrain_core::graph::{GraphStore as _, GraphStoreMut as _};
    use std::sync::Arc;

    fn terrain(store: &crate::store::SubstrateStore, n: usize) {
        for i in 0..n {
            let id = store.create_node(&["Personne"]);
            store.set_node_property(id, "nom", Value::String(format!("p{i}").into()));
        }
    }

    #[test]
    fn un_miroir_vide_donne_un_store_vide() {
        let m: Arc<dyn Miroir> = Arc::new(MiroirMemoire::default());
        let s = crate::store::SubstrateStore::ouvrir_sur_miroir(m).expect("ouverture");
        assert_eq!(s.node_count(), 0);
    }

    #[test]
    fn un_etat_publie_survit_a_la_coupure_et_se_relit() {
        let m = Arc::new(MiroirMemoire::default());
        {
            let s = crate::store::SubstrateStore::open_memory().unwrap();
            terrain(&s, 5);
            s.persister_sur_miroir(m.clone()).expect("publication");
        }
        m.couper();
        let s = crate::store::SubstrateStore::ouvrir_sur_miroir(m.clone()).expect("réouverture");
        assert_eq!(s.node_count(), 5, "les 5 nœuds publiés sont revenus");
    }

    #[test]
    fn ce_qui_n_est_pas_publie_est_perdu_a_la_coupure() {
        let m = Arc::new(MiroirMemoire::default());
        let s = crate::store::SubstrateStore::open_memory().unwrap();
        terrain(&s, 3);
        s.persister_sur_miroir(m.clone()).unwrap();
        terrain(&s, 4); // 4 nœuds de plus, JAMAIS publiés
        drop(s);
        m.couper();
        let r = crate::store::SubstrateStore::ouvrir_sur_miroir(m.clone()).unwrap();
        assert_eq!(r.node_count(), 3, "seul l'état publié survit");
    }

    #[test]
    fn une_publication_ulterieure_remplace_la_precedente() {
        let m = Arc::new(MiroirMemoire::default());
        let s = crate::store::SubstrateStore::open_memory().unwrap();
        terrain(&s, 2);
        s.persister_sur_miroir(m.clone()).unwrap();
        terrain(&s, 3);
        s.persister_sur_miroir(m.clone()).unwrap();
        m.couper();
        let r = crate::store::SubstrateStore::ouvrir_sur_miroir(m.clone()).unwrap();
        assert_eq!(r.node_count(), 5);
    }
}

#[cfg(test)]
mod tests_journal {
    use super::*;
    use crate::es::espace;
    use obrain_common::types::Value;
    use obrain_core::graph::{GraphStore as _, GraphStoreMut as _};
    use std::sync::Arc;

    fn ouvrir(chemin: &Path) -> MiroirJournal {
        let j = espace().ouvrir_journal(chemin).expect("journal");
        MiroirJournal::ouvrir(j).expect("reprise du journal")
    }

    fn tmp(nom: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join(nom);
        (d, p)
    }

    #[test]
    fn toutes_les_operations_se_rejouent_a_la_reouverture() {
        let (_d, p) = tmp("journal.bin");
        {
            let m = ouvrir(&p);
            m.ecrire("a", b"abc").unwrap();
            m.ajouter("a", b"def").unwrap();
            m.ecrire("b", b"xyz").unwrap();
            m.tronquer("a", 4).unwrap();
            m.supprimer("b").unwrap();
            m.ecrire("c", b"gone").unwrap();
            m.supprimer("c").unwrap();
            m.synchroniser().unwrap();
        }
        let m = ouvrir(&p);
        assert_eq!(m.lire("a").unwrap().as_deref(), Some(&b"abcd"[..]));
        assert_eq!(m.lire("b").unwrap(), None);
        assert_eq!(m.lire("c").unwrap(), None);
    }

    #[test]
    fn un_enregistrement_dechire_en_fin_est_abandonne_et_le_fichier_est_tronque() {
        let (_d, p) = tmp("journal.bin");
        let taille_saine;
        {
            let m = ouvrir(&p);
            m.ecrire("a", b"valide").unwrap();
            m.synchroniser().unwrap();
            taille_saine = std::fs::metadata(&p).unwrap().len();
        }
        // Coupure en plein enregistrement : quelques octets, pas un enregistrement entier.
        {
            use std::io::Write as _;
            let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
            f.write_all(&encoder_enreg(&Enreg::Put("b".into(), b"incomplet".to_vec()))[..9])
                .unwrap();
        }
        let m = ouvrir(&p);
        assert_eq!(m.lire("a").unwrap().as_deref(), Some(&b"valide"[..]));
        assert_eq!(m.lire("b").unwrap(), None);
        assert_eq!(
            std::fs::metadata(&p).unwrap().len(),
            taille_saine,
            "reprise : fin tronquée"
        );
    }

    #[test]
    fn la_partie_non_synchronisee_disparait_a_la_coupure() {
        let (_d, p) = tmp("journal.bin");
        let taille_synchro;
        {
            let m = ouvrir(&p);
            m.ecrire("a", b"synchro").unwrap();
            m.synchroniser().unwrap();
            taille_synchro = std::fs::metadata(&p).unwrap().len();
            m.ecrire("a", b"perdu").unwrap();
            m.ecrire("b", b"perdu").unwrap();
        }
        // La coupure n'a conservé que ce qui était synchronisé.
        std::fs::OpenOptions::new()
            .write(true)
            .open(&p)
            .unwrap()
            .set_len(taille_synchro)
            .unwrap();
        let m = ouvrir(&p);
        assert_eq!(m.lire("a").unwrap().as_deref(), Some(&b"synchro"[..]));
        assert_eq!(m.lire("b").unwrap(), None);
    }

    #[test]
    fn une_corruption_au_milieu_arrete_la_reprise_a_la_fin_valide() {
        let (_d, p) = tmp("journal.bin");
        let fin_premier;
        {
            let m = ouvrir(&p);
            m.ecrire("a", b"un").unwrap();
            fin_premier = std::fs::metadata(&p).unwrap().len() as usize;
            m.ecrire("a", b"deux").unwrap();
            m.ecrire("a", b"trois").unwrap();
            m.synchroniser().unwrap();
        }
        // Un octet flippé dans le DEUXIÈME enregistrement : seul le premier est valide.
        let mut octets = std::fs::read(&p).unwrap();
        octets[fin_premier + 15] ^= 0xFF;
        std::fs::write(&p, &octets).unwrap();
        let m = ouvrir(&p);
        assert_eq!(m.lire("a").unwrap().as_deref(), Some(&b"un"[..]));
    }

    #[test]
    fn le_store_complet_survit_a_une_reouverture_du_fichier() {
        let (_d, p) = tmp("journal.bin");
        {
            let s = crate::store::SubstrateStore::open_memory().unwrap();
            for i in 0..6 {
                let id = s.create_node(&["Personne"]);
                s.set_node_property(id, "nom", Value::String(format!("p{i}").into()));
            }
            let m: Arc<dyn Miroir> = Arc::new(ouvrir(&p));
            s.persister_sur_miroir(m).unwrap();
        }
        let m: Arc<dyn Miroir> = Arc::new(ouvrir(&p));
        let s = crate::store::SubstrateStore::ouvrir_sur_miroir(m).expect("store restauré");
        assert_eq!(
            s.node_count(),
            6,
            "les 6 nœuds sont revenus depuis le fichier"
        );
    }
}

#[cfg(target_arch = "wasm32")]
impl crate::store::SubstrateStore {
    /// Ouvre un store PERSISTANT dans le navigateur : le journal OPFS `nom`, rejoué,
    /// puis le store restauré depuis sa dernière image publiée.
    ///
    /// Rend le miroir avec le store : c'est lui que [`persister_sur_miroir`] doit
    /// recevoir pour publier. Le store ne persiste rien tant qu'on ne publie pas.
    ///
    /// [`persister_sur_miroir`]: crate::store::SubstrateStore::persister_sur_miroir
    pub async fn ouvrir_navigateur(
        nom: &str,
    ) -> crate::error::SubstrateResult<(Self, std::sync::Arc<dyn Miroir>)> {
        let fichier = crate::opfs::JournalOpfs::ouvrir(nom).await?;
        let miroir: std::sync::Arc<dyn Miroir> =
            std::sync::Arc::new(MiroirJournal::ouvrir(Box::new(fichier))?);
        let store = Self::ouvrir_sur_miroir(miroir.clone())?;
        Ok((store, miroir))
    }
}
