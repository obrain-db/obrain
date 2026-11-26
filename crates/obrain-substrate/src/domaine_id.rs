//! Identité de DOMAINE persistante (`domaine.id`), premier incrément natif (#334).
//!
//! Le `domaine_id` identifie une BASE — le répertoire de stockage — par
//! opposition à `writer_id` (un ÉCRIVAIN) et à `ReferenceTentative` (une
//! TENTATIVE). Il est tiré une fois, persisté dans `domaine.id` DANS le
//! répertoire de la base, et relu à chaque ouverture : le domaine suit les
//! FICHIERS (`rename` / `cp -a` le transportent), jamais le chemin. Deux bases
//! qui partagent `writer.id` (copie du fichier) restent donc distinguables par
//! leur `domaine_id` — à condition qu'un fork soit DÉCLARÉ (voir `forker`).
//!
//! # Format du fichier (versionné, SÉPARÉ de tout format persisté existant)
//!
//! ```text
//! octets 0..6   : b"OBRD1\n" — magic `OBRD` + version `1` + `\n`
//! octets 6..22  : le domaine_id sur 16 octets (u128, ordre big-endian = UUID
//!                 RFC 4122 canonique, bits version=4 / variant=0b10 posés)
//! ```
//!
//! Longueur totale : 22 octets. Toute autre longueur, ou une entête inconnue
//! (version non reconnue), est un refus EXPLICITE (`InvalidData`) — jamais une
//! régénération silencieuse, qui masquerait une corruption ou un fork d'une
//! version future.
//!
//! # Sémantique
//!
//! * `create` (base neuve)            → génère un UUID v4 et l'écrit.
//! * `open` (base existante)          → lit `domaine.id` ; s'il est ABSENT
//!   (base legacy antérieure à ce chantier), matérielise une identité NEUVE —
//!   cas documenté « legacy sans identité », PAS une réinitialisation : un
//!   domaine existant a toujours son fichier.
//! * `open` (fichier corrompu)        → ERREUR explicite, pas de réinitialisation.
//! * `forker` (fork DÉCLARÉ)          → tire et persiste un NOUVEAU domaine_id.
//!
//! Limite assumée : une copie intégrale NON déclarée (simple `cp -a` du
//! répertoire) partage le MÊME `domaine_id` — le distinguer exige l'opération
//! explicite `forker`. C'est structurel : « déplacer » et « copier » produisent
//! des répertoires byte-identiques ; seule l'intention de l'opérateur les
//! distingue (voir le RFC `contrat-identite-domaine.md`, §5).

use std::io::{self, Read, Write};
use std::path::Path;

/// Nom du fichier qui porte le `domaine_id` DANS le répertoire de la base.
pub const FICHIER_DOMAINE_ID: &str = "domaine.id";

/// Entête du format v1 : magic `OBRD` + version `1` + séparateur `\n`.
const ENTETE: &[u8; 6] = b"OBRD1\n";

/// Longueur totale d'une entrée valide : entête (6 octets) + u128 (16 octets).
const TAILLE: usize = 6 + 16;

/// Nanosecondes depuis l'epoch, 0 si l'horloge est indisponible.
fn nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

/// Compteur monotone : deux tirages dans le MÊME processus restent distincts.
static COMPTEUR: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Tire un `domaine_id` frais : 16 octets, version/variant UUID v4 (RFC 4122).
///
/// L'entropie combine la source aléatoire de `RandomState` (graine tirée par
/// l'OS au démarrage du processus), l'horloge, le pid et le compteur monotone —
/// mêlés deux fois pour 128 bits. Ce n'est pas un RNG cryptographique, et ce
/// n'est pas requis : un `domaine_id` est un séparateur de domaines, pas un
/// secret. Les bits de version (4) et de variant (0b10) sont posés pour que les
/// 16 octets soient reconnaissables comme UUID v4.
pub(crate) fn tirer() -> u128 {
    use std::hash::{BuildHasher, Hasher};
    let mut h1 = std::collections::hash_map::RandomState::new().build_hasher();
    h1.write_u32(std::process::id());
    h1.write_u64(nanos());
    h1.write_u64(COMPTEUR.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let a = h1.finish();
    let mut h2 = std::collections::hash_map::RandomState::new().build_hasher();
    h2.write_u64(a);
    h2.write_u64(nanos());
    let b = h2.finish();
    let mut octets = [0u8; 16];
    octets[..8].copy_from_slice(&a.to_le_bytes());
    octets[8..].copy_from_slice(&b.to_le_bytes());
    // RFC 4122 : version = 4 (nibble haut de l'octet 6), variant = 0b10
    // (2 bits hauts de l'octet 8), en ordre canonique big-endian.
    octets[6] = (octets[6] & 0x0f) | 0x40;
    octets[8] = (octets[8] & 0x3f) | 0x80;
    u128::from_be_bytes(octets)
}

/// Lit `domaine.id` dans `dir`.
///
/// * `Ok(Some(id))` — fichier présent et valide ;
/// * `Ok(None)` — fichier ABSENT (base legacy antérieure à ce chantier, ou base
///   en cours de création) ;
/// * `Err(InvalidData)` — fichier corrompu (longueur ≠ 22) ou entête inconnue
///   (version non reconnue) : refus EXPLICITE, jamais de régénération.
pub fn lire(dir: &Path) -> io::Result<Option<u128>> {
    let chemin = dir.join(FICHIER_DOMAINE_ID);
    match std::fs::File::open(&chemin) {
        Ok(mut f) => {
            let mut buf = Vec::with_capacity(TAILLE);
            f.read_to_end(&mut buf)?;
            if buf.len() != TAILLE {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "domaine.id : {} octets attendus (entête 6 + u128 16), {} lus",
                        TAILLE,
                        buf.len()
                    ),
                ));
            }
            if &buf[..ENTETE.len()] != ENTETE {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "domaine.id : entête inconnue (magic/version non reconnus) — refus, pas de régénération",
                ));
            }
            let octets: [u8; 16] = buf[ENTETE.len()..]
                .try_into()
                .expect("TAILLE garantit 16 octets après l'entête");
            Ok(Some(u128::from_be_bytes(octets)))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Écrit `domaine.id` en ÉCRASANT (fork déclaré). Écriture directe + fsync.
fn ecrire(dir: &Path, id: u128) -> io::Result<()> {
    let chemin = dir.join(FICHIER_DOMAINE_ID);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&chemin)?;
    f.write_all(ENTETE)?;
    f.write_all(&id.to_be_bytes())?;
    f.sync_all()?;
    Ok(())
}

/// Lit, et matérielise une identité NEUVE si le fichier est absent.
///
/// C'est le chemin `create` (base neuve) ET `open` sur une base legacy sans
/// `domaine.id` : dans les deux cas le fichier est absent, on tire une identité
/// neuve qu'on écrit. La distinction « legacy » est documentaire — un domaine
/// EXISTANT a toujours son fichier ; une absence = jamais identifié.
pub(crate) fn lire_ou_materieliser(dir: &Path) -> io::Result<u128> {
    match lire(dir)? {
        Some(id) => Ok(id),
        None => {
            let id = tirer();
            // `create_new` : ne jamais écraser un `domaine.id` créé entre-temps
            // par un pair — on relit alors sa valeur.
            let chemin = dir.join(FICHIER_DOMAINE_ID);
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&chemin)
            {
                Ok(mut f) => {
                    f.write_all(ENTETE)?;
                    f.write_all(&id.to_be_bytes())?;
                    f.sync_all()?;
                    Ok(id)
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => lire_ou_materieliser(dir),
                Err(e) => Err(e),
            }
        }
    }
}

/// Fork DÉCLARÉ : tire et persiste un NOUVEAU `domaine_id`, écrasant l'ancien.
///
/// C'est l'opération explicite qui distingue un fork d'une copie : une copie
/// intégrale non déclarée partage le même identifiant (limite assumée), un fork
/// déclaré le régénère.
pub fn forker(dir: &Path) -> io::Result<u128> {
    let id = tirer();
    ecrire(dir, id)?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ecriture_lecture_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let id = lire_ou_materieliser(dir.path()).unwrap();
        assert_eq!(lire(dir.path()).unwrap(), Some(id));
    }

    #[test]
    fn deux_domaines_distincts() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        assert_ne!(
            lire_ou_materieliser(a.path()).unwrap(),
            lire_ou_materieliser(b.path()).unwrap()
        );
    }

    #[test]
    fn corrompu_tronque_refuse() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FICHIER_DOMAINE_ID), b"OBRD1\n\x00\x01").unwrap();
        let e = lire(dir.path()).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn version_inconnue_refuse() {
        let dir = tempfile::tempdir().unwrap();
        let mut buf = b"OBRD9\n".to_vec();
        buf.extend_from_slice(&[0u8; 16]);
        std::fs::write(dir.path().join(FICHIER_DOMAINE_ID), buf).unwrap();
        let e = lire(dir.path()).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn forker_regenerer_un_domaine_distinct() {
        let dir = tempfile::tempdir().unwrap();
        let premier = lire_ou_materieliser(dir.path()).unwrap();
        let second = forker(dir.path()).unwrap();
        assert_ne!(premier, second);
        assert_eq!(lire(dir.path()).unwrap(), Some(second));
    }
}
