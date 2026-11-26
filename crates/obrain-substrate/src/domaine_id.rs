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
//! Longueur totale : 22 octets. Toute autre longueur, une entête inconnue
//! (version non reconnue) ou une valeur NULLE (16 octets à zéro) est un refus
//! EXPLICITE (`InvalidData`) — jamais une régénération silencieuse, qui
//! masquerait une corruption ou un fork d'une version future.
//!
//! # Sémantique
//!
//! * `create` (base neuve)       → génère un UUID v4 et l'écrit (atomiquement).
//! * `open` (base existante)     → lit `domaine.id` :
//!   * PRÉSENT et valide          → relu tel quel ;
//!   * ABSENT et base VIDE         → matérielise une identité NEUVE (base legacy
//!     antérieure à ce chantier, jamais identifiée) ;
//!   * ABSENT et base AVEC DONNÉES → ERREUR explicite (fichier perdu), JAMAIS
//!     une réattribution silencieuse : « jamais identifiée » et « identifiée
//!     puis perdue » ne se distinguent plus, alors on refuse. Le discriminant
//!     est fourni par l'appelant (`store::base_porte_des_donnees`), qui relit
//!     les marqueurs EXISTANTS (taille du WAL, `next_node_id` du dictionnaire,
//!     compteur de pages PropsZone) — aucun format persisté nouveau.
//!   * CORROMPU (tronqué, entête inconnue, valeur nulle) → ERREUR explicite.
//! * `forker` (fork DÉCLARÉ)     → tire et persiste un NOUVEAU domaine_id.
//!
//! # Durabilité et concurrence
//!
//! `domaine.id` est écrit par PUBLICATION ATOMIQUE : écriture de
//! `domaine.id.tmp` (`sync_all`, données + métadonnées), `rename` vers
//! `domaine.id` (atomique POSIX), puis fsync du répertoire (durabilité du
//! rename). Un lecteur ne voit jamais un fichier partiel — il voit l'ancien
//! fichier, le nouveau complet, ou rien. La matérialisation et le fork prennent
//! le verrou de fichier `substrate.domaine.lock` (`flock`) : deux créateurs
//! concurrents s'accordent sur UNE identité, deux forks concurrents se
//! sérialisent. Le verrou est un fichier STABLE (jamais renommé), comme
//! `substrate.dirlock`.
//!
//! Limite assumée : une copie intégrale NON déclarée (simple `cp -a` du
//! répertoire) partage le MÊME `domaine_id` — le distinguer exige l'opération
//! explicite `forker`. C'est structurel : « déplacer » et « copier » produisent
//! des répertoires byte-identiques ; seule l'intention de l'opérateur les
//! distingue (voir le RFC `contrat-identite-domaine.md`, §5).

use std::io::{self, Read};
use std::path::Path;

/// Nom du fichier qui porte le `domaine_id` DANS le répertoire de la base.
pub const FICHIER_DOMAINE_ID: &str = "domaine.id";

/// Verrou de fichier du domaine : sérialise la matérialisation et le fork entre
/// processus. Fichier STABLE, jamais renommé (le `rename` de `domaine.id`
/// remplacerait l'inode et rendrait un `flock` posé dessus sans objet — même
/// raison que `substrate.dirlock`). Contenu sans signification.
pub(crate) const FICHIER_DOMAINE_VERROU: &str = "substrate.domaine.lock";

/// Nom du fichier TEMPORAIRE de la publication atomique (`ecrire_atomique`).
const TMP_DOMAINE_ID: &str = "domaine.id.tmp";

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
/// * `Ok(Some(id))` — fichier présent et valide (id non nul) ;
/// * `Ok(None)` — fichier ABSENT (base legacy antérieure à ce chantier, ou base
///   en cours de création) ;
/// * `Err(InvalidData)` — fichier corrompu (longueur ≠ 22), entête inconnue
///   (version non reconnue) ou valeur NULLE (16 octets à zéro) : refus
///   EXPLICITE, jamais de régénération.
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
            let id = u128::from_be_bytes(octets);
            if id == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "domaine.id : identité nulle (16 octets à zéro) — corruption, refus, pas de régénération",
                ));
            }
            Ok(Some(id))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// L'erreur « fichier d'identité perdu » : `domaine.id` absent sur une base qui
/// porte des données. Refus EXPLICITE — matérialiser une identité neuve ferait
/// d'une tentative passée une étrangère à sa propre base.
fn erreur_fichier_perdu() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "domaine.id absent sur une base qui porte des données : fichier d'identité \
         perdu — refus, pas de réattribution silencieuse",
    )
}

/// Écrit `domaine.id` par PUBLICATION ATOMIQUE : `domaine.id.tmp` (écriture +
/// `sync_all`), `rename` vers `domaine.id` (atomique POSIX), fsync du
/// répertoire. Un lecteur concurrent ne voit jamais un fichier partiel.
fn ecrire_atomique(dir: &Path, id: u128) -> io::Result<()> {
    let final_chemin = dir.join(FICHIER_DOMAINE_ID);
    let tmp_chemin = dir.join(TMP_DOMAINE_ID);
    let mut buf = Vec::with_capacity(TAILLE);
    buf.extend_from_slice(ENTETE);
    buf.extend_from_slice(&id.to_be_bytes());
    let es = crate::es::espace();
    es.ecrire_synchronise(&tmp_chemin, &buf)?;
    es.renommer(&tmp_chemin, &final_chemin)?;
    es.synchroniser_repertoire(dir)?;
    Ok(())
}

/// Prend le verrou EXCLUSIF du domaine (fichier stable `substrate.domaine.lock`).
/// Le verrou est tenu tant que l'objet rendu vit — la section critique vit donc
/// dans la portée du résultat.
fn verrouiller_domaine(dir: &Path) -> io::Result<Box<dyn crate::es::FichierVerrou>> {
    let verrou = crate::es::espace().ouvrir_verrou(&dir.join(FICHIER_DOMAINE_VERROU))?;
    verrou.exclusif()?;
    Ok(verrou)
}

/// Lit, et matérielise une identité NEUVE si le fichier est absent ET que la
/// base est vide.
///
/// `base_a_des_donnees` (lazy) est le DISCRIMINANT de l'appelant : il n'est
/// évalué que si `domaine.id` est absent, et rend `true` si la base porte une
/// donnée persistée (voir `store::base_porte_des_donnees`).
///
/// * fichier présent et valide → relu ;
/// * absent + base VIDE         → identité neuve, écrite atomiquement ;
/// * absent + base AVEC DONNÉES → ERREUR (fichier perdu), jamais une
///   réattribution silencieuse.
pub(crate) fn lire_ou_materieliser(
    dir: &Path,
    base_a_des_donnees: impl FnOnce() -> bool,
) -> io::Result<u128> {
    match lire(dir)? {
        Some(id) => Ok(id),
        None => {
            if base_a_des_donnees() {
                return Err(erreur_fichier_perdu());
            }
            // Verrou : deux créateurs concurrents (même répertoire, plusieurs
            // processus en écriture partagée) s'accordent sur UNE identité.
            let _verrou = verrouiller_domaine(dir)?;
            // Relecture SOUS verrou : un pair a pu matérialiser entre-temps.
            if let Some(id) = lire(dir)? {
                return Ok(id);
            }
            let id = tirer();
            ecrire_atomique(dir, id)?;
            Ok(id)
        }
    }
}

/// Lecture SANS écriture (contrat lecture seule) : rend l'identité si présente ;
/// si ABSENTE, identité ÉPHÉMÈRE sur base vide, ERREUR sur base avec données.
pub(crate) fn lire_ou_ephemere(
    dir: &Path,
    base_a_des_donnees: impl FnOnce() -> bool,
) -> io::Result<u128> {
    match lire(dir)? {
        Some(id) => Ok(id),
        None => {
            if base_a_des_donnees() {
                Err(erreur_fichier_perdu())
            } else {
                // Éphémère : jamais persisté (un lecteur n'écrit rien).
                Ok(tirer())
            }
        }
    }
}

/// Fork DÉCLARÉ : tire et persiste un NOUVEAU `domaine_id`, écrasant l'ancien.
///
/// C'est l'opération explicite qui distingue un fork d'une copie : une copie
/// intégrale non déclarée partage le même identifiant (limite assumée), un fork
/// déclaré le régénère.
///
/// Le verrou de fichier sérialise les forks concurrents (plusieurs processus) :
/// le dernier renommage atomique gagne, et aucun lecteur ne voit un fichier
/// partiel. Limite : deux forks séquentiels produisent deux identités
/// DIFFÉRENTES (c'est le sens du fork) — le verrou empêche la corruption de
/// l'écriture, pas la divergence voulue.
pub fn forker(dir: &Path) -> io::Result<u128> {
    let _verrou = verrouiller_domaine(dir)?;
    let id = tirer();
    ecrire_atomique(dir, id)?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Discriminant « toujours vide » pour les tests qui ne portent pas de données.
    fn sans_donnees() -> bool {
        false
    }

    #[test]
    fn ecriture_lecture_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let id = lire_ou_materieliser(dir.path(), sans_donnees).unwrap();
        assert_eq!(lire(dir.path()).unwrap(), Some(id));
    }

    #[test]
    fn deux_domaines_distincts() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        assert_ne!(
            lire_ou_materieliser(a.path(), sans_donnees).unwrap(),
            lire_ou_materieliser(b.path(), sans_donnees).unwrap()
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
    fn valeur_nulle_refuse() {
        let dir = tempfile::tempdir().unwrap();
        let mut buf = ENTETE.to_vec();
        buf.extend_from_slice(&[0u8; 16]);
        std::fs::write(dir.path().join(FICHIER_DOMAINE_ID), buf).unwrap();
        let e = lire(dir.path()).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn forker_regenerer_un_domaine_distinct() {
        let dir = tempfile::tempdir().unwrap();
        let premier = lire_ou_materieliser(dir.path(), sans_donnees).unwrap();
        let second = forker(dir.path()).unwrap();
        assert_ne!(premier, second);
        assert_eq!(lire(dir.path()).unwrap(), Some(second));
    }

    /// Base AVEC données + `domaine.id` absent → refus, pas de matérialisation.
    #[test]
    fn base_avec_donnees_sans_identite_refuse() {
        let dir = tempfile::tempdir().unwrap();
        let e = lire_ou_materieliser(dir.path(), || true).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
        // Rien n'a été écrit : pas de réattribution silencieuse.
        assert!(!dir.path().join(FICHIER_DOMAINE_ID).exists());
    }

    /// Lecture seule : base vide sans identité → identité ÉPHÉMÈRE, rien d'écrit.
    #[test]
    fn lecture_ephemere_sur_base_vide_n_ecrit_rien() {
        let dir = tempfile::tempdir().unwrap();
        let id = lire_ou_ephemere(dir.path(), sans_donnees).unwrap();
        assert_ne!(id, 0);
        assert!(!dir.path().join(FICHIER_DOMAINE_ID).exists());
    }

    /// Lecture seule : base avec données sans identité → refus.
    #[test]
    fn lecture_ephemere_sur_base_avec_donnees_refuse() {
        let dir = tempfile::tempdir().unwrap();
        let e = lire_ou_ephemere(dir.path(), || true).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
    }

    /// L'écriture atomique ne laisse pas de fichier partiel lisible.
    #[test]
    fn ecriture_atomique_renomme_sans_partiel() {
        let dir = tempfile::tempdir().unwrap();
        let id = tirer();
        ecrire_atomique(dir.path(), id).unwrap();
        assert_eq!(lire(dir.path()).unwrap(), Some(id));
        // Le temporaire a disparu (renommé), le final est complet.
        assert!(!dir.path().join(TMP_DOMAINE_ID).exists());
    }

    /// Création CONCURRENTE (deux threads, même répertoire) : le verrou de
    /// fichier les sérialise — ils s'accordent sur UNE seule identité.
    #[test]
    fn creation_concurrente_un_identifiant_unique() {
        use std::sync::Arc;
        let dir = Arc::new(tempfile::tempdir().unwrap());
        let mut mains = Vec::new();
        for _ in 0..8 {
            let d = Arc::clone(&dir);
            mains.push(std::thread::spawn(move || {
                lire_ou_materieliser(d.path(), sans_donnees).unwrap()
            }));
        }
        let identifiants: Vec<u128> = mains
            .into_iter()
            .map(|m| m.join().expect("thread paniqué"))
            .collect();
        let premier = identifiants[0];
        assert!(
            identifiants.iter().all(|&i| i == premier),
            "tous les créateurs concurrents doivent lire la MÊME identité, obtenu {identifiants:?}"
        );
    }

    /// Interruption APRÈS écriture du temporaire (crash avant le rename) : le
    /// `.tmp` orphelin est écrasé à la prochaine matérialisation, sans erreur.
    #[test]
    fn tmp_orphelin_repris_par_la_materialisation() {
        let dir = tempfile::tempdir().unwrap();
        // Simule un crash entre l'écriture du .tmp et le rename.
        std::fs::write(dir.path().join(TMP_DOMAINE_ID), b"partiel orphelin").unwrap();
        let id = lire_ou_materieliser(dir.path(), sans_donnees).unwrap();
        assert_eq!(lire(dir.path()).unwrap(), Some(id));
        // Le temporaire a été consommé par le rename.
        assert!(!dir.path().join(TMP_DOMAINE_ID).exists());
    }

    /// Un `.tmp` orphelin laissé après un `forker` interrompu est de même repris.
    #[test]
    fn fork_apres_tmp_orphelin_est_sain() {
        let dir = tempfile::tempdir().unwrap();
        let premier = lire_ou_materieliser(dir.path(), sans_donnees).unwrap();
        std::fs::write(dir.path().join(TMP_DOMAINE_ID), b"partiel").unwrap();
        let second = forker(dir.path()).unwrap();
        assert_ne!(premier, second);
        assert_eq!(lire(dir.path()).unwrap(), Some(second));
        assert!(!dir.path().join(TMP_DOMAINE_ID).exists());
    }
}
