//! Identité de DOMAINE persistante — protocole DEUX FICHIERS (`domaine.id` +
//! `domaine.init`), second incrément natif (#334).
//!
//! Le `domaine_id` identifie une BASE — le répertoire de stockage — par
//! opposition à `writer_id` (un ÉCRIVAIN) et à `ReferenceTentative` (une
//! TENTATIVE). Il est tiré une fois, persisté dans `domaine.id` DANS le
//! répertoire de la base, et relu à chaque ouverture : le domaine suit les
//! FICHIERS (`rename` / `cp -a` le transportent), jamais le chemin. Deux bases
//! qui partagent `writer.id` (copie du fichier) restent donc distinguables par
//! leur `domaine_id` — à condition qu'un fork soit DÉCLARÉ (voir `forker`).
//!
//! # Le défaut que ce protocole ferme
//!
//! La première version ne persistait QUE `domaine.id`, et tranchait « base
//! jamais identifiée » vs « base identifiée puis perdue » par une heuristique
//! d'occupation (WAL / dictionnaire / pages). Cette heuristique confond
//! « base legacy VIDE » et « base identifiée VIDE ayant perdu son fichier » :
//! les deux ressemblent à « vide + pas de `domaine.id` », donc la seconde était
//! RÉ-IDENTIFIÉE en silence — une tentative passée devenait étrangère à sa
//! propre base. Un marqueur DURABLE et VERSIONNÉ (`domaine.init`) lève
//! l'ambiguïté sans jamais relire le contenu des données.
//!
//! # Protocole DEUX FICHIERS
//!
//! * `domaine.id`  — l'identité (u128, UUID v4), format `OBRD1\n` + 16 octets ;
//! * `domaine.init` — le MARQUEUR D'INITIALISATION : l'empreinte du `domaine_id`
//!   (8 premiers octets du u128, ordre big-endian), format `OBRI1\n` + 8 octets.
//!
//! `domaine.init` est écrit ATOMIQUEMENT APRÈS `domaine.id`, à la PREMIÈRE
//! matérialisation (`create` ou première ouverture d'un legacy) — et réécrit à
//! chaque fork. Il atteste « cette base A été initialisée avec TEL domaine »,
//! indépendamment de toute donnée : une base identifiée VIDE qui perd
//! `domaine.id` garde son `domaine.init`, donc la perte est DÉTECTÉE.
//!
//! # Formats (versionnés, SÉPARÉS de tout format persisté existant)
//!
//! ```text
//! domaine.id   : octets 0..6   b"OBRD1\n" (magic OBRD + version 1 + \n)
//!                octets 6..22  le domaine_id sur 16 octets (u128, big-endian
//!                              = UUID RFC 4122, bits version=4 / variant=0b10)
//!                longueur totale 22 octets.
//!
//! domaine.init : octets 0..6   b"OBRI1\n" (magic OBRI + version 1 + \n)
//!                octets 6..14  l'EMPREINTE : les 8 premiers octets (haut) du
//!                              domaine_id big-endian.
//!                longueur totale 14 octets.
//! ```
//!
//! Toute autre longueur, entête inconnue (version non reconnue) ou valeur NULLE
//! (identité 16 octets à zéro, ou empreinte 8 octets à zéro) est un refus
//! EXPLICITE (`InvalidData`) — jamais une régénération silencieuse, qui
//! masquerait une corruption ou un fork d'une version future.
//!
//! # Matrice de décision à l'ouverture
//!
//! | `domaine.id` | `domaine.init`       | verdict                                   |
//! |--------------|----------------------|-------------------------------------------|
//! | présent      | présent (concordant) | OUVERTURE NORMALE (identité relue)        |
//! | présent      | ABSENT               | legacy-domaine → MIGRER (écrire `init`),  |
//! |              |                      |   JAMAIS d'identité neuve                 |
//! | ABSENT       | présent              | PERTE d'identité → `InvalidData`, jamais   |
//! |              |                      |   de réattribution (même base VIDE)       |
//! | ABSENT       | ABSENT               | legacy jamais identifié → matérialiser    |
//! |              |                      |   identité neuve + `domaine.init`         |
//! | présent      | présent (DISCORDANT) | `InvalidData` (corruption / fork coupé)   |
//!
//! En LECTURE SEULE, aucun fichier n'est créé (contrat RO) : le cas
//! « legacy-domaine » rend l'identité lue SANS écrire `init` (la migration est
//! reportée au prochain écrivain), et le cas « jamais identifié » rend une
//! identité ÉPHÉMÈRE non persistée.
//!
//! # Durabilité et concurrence
//!
//! Chaque fichier est écrit par PUBLICATION ATOMIQUE : écriture de
//! `*.tmp` (`sync_all`, données + métadonnées), `rename` vers le final
//! (atomique POSIX), puis fsync du répertoire (durabilité du rename). Un lecteur
//! ne voit jamais un fichier partiel. La matérialisation, la migration et le
//! fork prennent le verrou de fichier `substrate.domaine.lock` (`flock`,
//! MULTI-PROCESSUS) : deux créateurs concurrents s'accordent sur UNE identité.
//! Le verrou est un fichier STABLE (jamais renommé), comme `substrate.dirlock`.
//!
//! Fenêtre d'interruption assumée : si le processus meurt ENTRE l'écriture de
//! `domaine.id` et celle de `domaine.init` (matérialisation) ou entre les deux
//! réécritures d'un fork, la réouverture voit « `domaine.id` présent +
//! `domaine.init` absent » (migration, pas de perte) pour la matérialisation, et
//! « empreinte discordante » (refus, réparation opérateur) pour le fork. Aucun
//! des deux ne produit de réattribution silencieuse.
//!
//! # Limite assumée (documentée)
//!
//! 1. Une copie intégrale NON déclarée (simple `cp -a`) partage le MÊME
//!    `domaine_id` — la distinguer exige `forker` (structurel : « déplacer » et
//!    « copier » produisent des répertoires byte-identiques, seule l'intention
//!    de l'opérateur les distingue — voir le RFC `contrat-identite-domaine.md`,
//!    §5).
//! 2. La suppression des DEUX fichiers (`domaine.id` ET `domaine.init`) rend la
//!    base INDISTINGUABLE d'un legacy jamais identifié : elle est alors
//!    matérialisée (ré-identifiée) — ACCEPTÉ. Seule la perte de `domaine.id`
//!    SEUL (marqueur restant) est refusée.

use std::io::{self, Read};
use std::path::Path;

/// Nom du fichier qui porte le `domaine_id` DANS le répertoire de la base.
pub const FICHIER_DOMAINE_ID: &str = "domaine.id";

/// Nom du MARQUEUR D'INITIALISATION (`domaine.init`) : l'empreinte du
/// `domaine_id` qui a matérialisé la base. Sa présence atteste « cette base a
/// été identifiée », indépendamment de toute donnée — c'est lui qui refuse la
/// ré-identification d'une base identifiée VIDE ayant perdu `domaine.id`.
pub const FICHIER_DOMAINE_INIT: &str = "domaine.init";

/// Verrou de fichier du domaine : sérialise la matérialisation, la migration et
/// le fork entre processus. Fichier STABLE, jamais renommé (le `rename` de
/// `domaine.id`/`domaine.init` remplacerait l'inode et rendrait un `flock` posé
/// dessus sans objet — même raison que `substrate.dirlock`). Contenu sans
/// signification.
pub(crate) const FICHIER_DOMAINE_VERROU: &str = "substrate.domaine.lock";

/// Nom du fichier TEMPORAIRE de la publication atomique (`ecrire_atomique`).
const TMP_DOMAINE_ID: &str = "domaine.id.tmp";

/// Nom du fichier TEMPORAIRE de la publication atomique du marqueur.
const TMP_DOMAINE_INIT: &str = "domaine.init.tmp";

/// Entête du format v1 de `domaine.id` : magic `OBRD` + version `1` + `\n`.
const ENTETE: &[u8; 6] = b"OBRD1\n";

/// Longueur totale d'une entrée `domaine.id` valide : entête (6) + u128 (16).
const TAILLE: usize = 6 + 16;

/// Entête du format v1 de `domaine.init` : magic `OBRI` + version `1` + `\n`.
const ENTETE_INIT: &[u8; 6] = b"OBRI1\n";

/// Longueur totale d'une entrée `domaine.init` valide : entête (6) + empreinte (8).
const TAILLE_INIT: usize = 6 + 8;

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

/// L'EMPREINTE d'un `domaine_id` : ses 8 premiers octets (les 8 de poids fort,
/// big-endian). Les bits de version UUID (nibble haut de l'octet 6) y sont, donc
/// une empreinte valide n'est jamais nulle.
fn empreinte(id: u128) -> [u8; 8] {
    let mut emp = [0u8; 8];
    emp.copy_from_slice(&id.to_be_bytes()[..8]);
    emp
}

/// Lit `domaine.id` dans `dir`.
///
/// * `Ok(Some(id))` — fichier présent et valide (id non nul) ;
/// * `Ok(None)` — fichier ABSENT (base jamais identifiée, ou en cours de
///   création) ;
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

/// Lit `domaine.init` dans `dir`.
///
/// * `Ok(Some(empreinte))` — marqueur présent et valide (empreinte non nulle) ;
/// * `Ok(None)` — marqueur ABSENT (base identifiée avant le marqueur, ou jamais
///   identifiée) ;
/// * `Err(InvalidData)` — marqueur corrompu (longueur ≠ 14), entête inconnue
///   (version non reconnue) ou empreinte NULLE (8 octets à zéro) : refus
///   EXPLICITE, jamais de régénération.
fn lire_init(dir: &Path) -> io::Result<Option<[u8; 8]>> {
    let chemin = dir.join(FICHIER_DOMAINE_INIT);
    match std::fs::File::open(&chemin) {
        Ok(mut f) => {
            let mut buf = Vec::with_capacity(TAILLE_INIT);
            f.read_to_end(&mut buf)?;
            if buf.len() != TAILLE_INIT {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "domaine.init : {} octets attendus (entête 6 + empreinte 8), {} lus",
                        TAILLE_INIT,
                        buf.len()
                    ),
                ));
            }
            if &buf[..ENTETE_INIT.len()] != ENTETE_INIT {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "domaine.init : entête inconnue (magic/version non reconnus) — refus, pas de régénération",
                ));
            }
            let emp: [u8; 8] = buf[ENTETE_INIT.len()..]
                .try_into()
                .expect("TAILLE_INIT garantit 8 octets après l'entête");
            if emp == [0u8; 8] {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "domaine.init : empreinte nulle (8 octets à zéro) — corruption, refus, pas de régénération",
                ));
            }
            Ok(Some(emp))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// L'erreur « identité de domaine perdue » : `domaine.id` ABSENT alors que le
/// MARQUEUR `domaine.init` est PRÉSENT. Refus EXPLICITE — matérialiser une
/// identité neuve ferait d'une tentative passée une étrangère à sa propre base.
/// Ce refus vaut même pour une base VIDE : le marqueur atteste l'initialisation.
fn erreur_identite_perdue() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "domaine.id ABSENT mais domaine.init PRÉSENT : identité de domaine perdue — \
         refus, jamais de réattribution silencieuse (même une base VIDE)",
    )
}

/// L'erreur « empreinte discordante » : `domaine.init` ne porte pas l'empreinte
/// du `domaine.id` présent — corruption, ou fork interrompu entre les deux
/// écritures. Refus EXPLICITE, jamais de réattribution silencieuse.
fn erreur_empreinte_discordante() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "domaine.init : empreinte discordante avec domaine.id — corruption ou fork \
         interrompu, refus, pas de réattribution silencieuse",
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

/// Écrit `domaine.init` par PUBLICATION ATOMIQUE : `domaine.init.tmp` (écriture
/// + `sync_all`), `rename` vers `domaine.init`, fsync du répertoire. Même
/// séquence de durabilité que `ecrire_atomique`.
fn ecrire_init_atomique(dir: &Path, emp: [u8; 8]) -> io::Result<()> {
    let final_chemin = dir.join(FICHIER_DOMAINE_INIT);
    let tmp_chemin = dir.join(TMP_DOMAINE_INIT);
    let mut buf = Vec::with_capacity(TAILLE_INIT);
    buf.extend_from_slice(ENTETE_INIT);
    buf.extend_from_slice(&emp);
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

/// Action d'écriture décidée par la matrice, exécutée SOUS verrou (écrivain).
enum Ecriture {
    /// Cas « legacy-domaine » : `domaine.id` présent, `domaine.init` absent.
    /// Écrire l'empreinte du domaine EXISTANT — jamais une identité neuve.
    Migrer(u128),
    /// Cas « legacy jamais identifié » : les deux fichiers absents. Tirer une
    /// identité neuve, écrire `domaine.id` puis `domaine.init`.
    Materialiser,
}

/// Verdict de la matrice de décision pour un état (domaine.id, domaine.init).
enum Decision {
    /// Identité résolue sans écriture (ouverture normale, ou lecture seule).
    Lue(u128),
    /// Lecture seule sur base jamais identifiée : identité ÉPHÉMÈRE, rien d'écrit.
    Ephemere,
    /// Une écriture est requise (écrivain seulement).
    Ecrire(Ecriture),
    /// Refus explicite (`InvalidData`), jamais de réattribution silencieuse.
    Refus(io::Error),
}

/// La matrice de décision de l'identité de domaine (protocole DEUX FICHIERS).
///
/// `peut_ecrire` sépare l'écrivain (matérialise / migre) du lecteur en lecture
/// seule (ne crée AUCUN fichier : contrat RO) ; le lecteur rend alors l'identité
/// lue, ou une identité éphémère sur une base jamais identifiée.
fn decider(id: Option<u128>, init: Option<[u8; 8]>, peut_ecrire: bool) -> Decision {
    match (id, init) {
        // (1) domaine.id + domaine.init, empreinte concordante → OUVERTURE NORMALE.
        (Some(id), Some(emp)) => {
            if emp == empreinte(id) {
                Decision::Lue(id)
            } else {
                Decision::Refus(erreur_empreinte_discordante())
            }
        }
        // (2) domaine.id présent + init absent → identifiée AVANT le marqueur
        // (legacy-domaine) : migrer (écrire init) côté écrivain, relire côté RO.
        (Some(id), None) => {
            if peut_ecrire {
                Decision::Ecrire(Ecriture::Migrer(id))
            } else {
                Decision::Lue(id)
            }
        }
        // (3) domaine.id ABSENT + domaine.init PRÉSENT → PERTE d'identité : refus
        // EXPLICITE, même si la base est VIDE (le marqueur atteste l'initialisation).
        (None, Some(_)) => Decision::Refus(erreur_identite_perdue()),
        // (4) les deux absents → legacy jamais identifiée : matérieliser (écrivain)
        // ou identité éphémère (lecteur seule).
        (None, None) => {
            if peut_ecrire {
                Decision::Ecrire(Ecriture::Materialiser)
            } else {
                Decision::Ephemere
            }
        }
    }
}

/// Exécute une écriture décidée par la matrice (le verrou est DÉJÀ tenu).
fn executer(dir: &Path, ecriture: Ecriture) -> io::Result<u128> {
    match ecriture {
        Ecriture::Migrer(id) => {
            ecrire_init_atomique(dir, empreinte(id))?;
            Ok(id)
        }
        Ecriture::Materialiser => {
            let id = tirer();
            ecrire_atomique(dir, id)?;
            ecrire_init_atomique(dir, empreinte(id))?;
            Ok(id)
        }
    }
}

/// Résout l'identité de domaine selon la matrice, en écrivant si nécessaire.
///
/// `peut_ecrire` : `true` pour un écrivain (matérialisation/migration sous
/// verrou), `false` pour un lecteur en lecture seule (jamais d'écriture).
fn resoudre(dir: &Path, peut_ecrire: bool) -> io::Result<u128> {
    match decider(lire(dir)?, lire_init(dir)?, peut_ecrire) {
        Decision::Lue(id) => Ok(id),
        Decision::Ephemere => Ok(tirer()),
        Decision::Refus(e) => Err(e),
        Decision::Ecrire(_) => {
            // Verrou : deux écrivains concurrents (même répertoire, plusieurs
            // processus en écriture partagée) s'accordent sur UNE identité.
            let _verrou = verrouiller_domaine(dir)?;
            // Relecture SOUS verrou : un pair a pu matérialiser/migrer entre-temps.
            // L'action recalculée sur l'état re-lu fait foi.
            match decider(lire(dir)?, lire_init(dir)?, peut_ecrire) {
                Decision::Lue(id) => Ok(id),
                Decision::Ephemere => Ok(tirer()),
                Decision::Refus(e) => Err(e),
                Decision::Ecrire(action) => executer(dir, action),
            }
        }
    }
}

/// Lit, et matérielise une identité NEUVE si les DEUX fichiers sont absents
/// (base legacy jamais identifiée) ; migre `domaine.init` si `domaine.id` est
/// présent sans marqueur (base identifiée avant le marqueur).
///
/// * identité valide (marqueur concordant ou absent) → relue ;
/// * `domaine.id` présent + `domaine.init` absent → migration (écrire
///   `domaine.init`), PAS de nouvelle identité ;
/// * `domaine.id` absent + `domaine.init` présent → ERREUR (identité perdue),
///   même si la base est VIDE — jamais une réattribution silencieuse ;
/// * les deux absents → matérieliser une identité neuve + `domaine.init`.
pub(crate) fn lire_ou_materieliser(dir: &Path) -> io::Result<u128> {
    resoudre(dir, true)
}

/// Lecture SANS écriture (contrat lecture seule) : rend l'identité si présente
/// (marqueur concordant, ou `domaine.id` sans marqueur — sans migrer) ; si les
/// DEUX fichiers sont absents, identité ÉPHÉMÈRE (jamais persistée) ; si
/// `domaine.init` est présent SANS `domaine.id`, ERREUR (identité perdue).
pub(crate) fn lire_ou_ephemere(dir: &Path) -> io::Result<u128> {
    resoudre(dir, false)
}

/// Fork DÉCLARÉ : tire et persiste un NOUVEAU `domaine_id` ET un nouveau
/// marqueur `domaine.init` (empreinte du nouveau domaine), écrasant l'ancien.
///
/// C'est l'opération explicite qui distingue un fork d'une copie : une copie
/// intégrale non déclarée partage le même identifiant (limite assumée), un fork
/// déclaré le régénère.
///
/// Le verrou de fichier sérialise les forks concurrents (plusieurs processus) :
/// le dernier renommage atomique gagne, et aucun lecteur ne voit un fichier
/// partiel. Limite : deux forks séquentiels produisent deux identités
/// DIFFÉRENTES (c'est le sens du fork) — le verrou empêche la corruption de
/// l'écriture, pas la divergence voulue. Un fork interrompu ENTRE les deux
/// écritures laisse `domaine.id` et `domaine.init` discordants : la réouverture
/// REFUSE (réparation opérateur), jamais de réattribution silencieuse.
pub fn forker(dir: &Path) -> io::Result<u128> {
    let _verrou = verrouiller_domaine(dir)?;
    let id = tirer();
    ecrire_atomique(dir, id)?;
    ecrire_init_atomique(dir, empreinte(id))?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Écrit un `domaine.id` valide (sans marqueur) — simule une base legacy-domaine.
    fn ecrire_id_seul(dir: &Path, id: u128) {
        let mut buf = ENTETE.to_vec();
        buf.extend_from_slice(&id.to_be_bytes());
        std::fs::write(dir.join(FICHIER_DOMAINE_ID), buf).unwrap();
    }

    #[test]
    fn creation_ecrit_id_et_init_concordants() {
        let dir = tempfile::tempdir().unwrap();
        let id = lire_ou_materieliser(dir.path()).unwrap();
        assert_eq!(lire(dir.path()).unwrap(), Some(id));
        assert_eq!(lire_init(dir.path()).unwrap(), Some(empreinte(id)));
    }

    #[test]
    fn ouverture_normale_relit_le_meme() {
        let dir = tempfile::tempdir().unwrap();
        let premier = lire_ou_materieliser(dir.path()).unwrap();
        let second = lire_ou_materieliser(dir.path()).unwrap();
        assert_eq!(premier, second);
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
    fn valeur_nulle_refuse() {
        let dir = tempfile::tempdir().unwrap();
        let mut buf = ENTETE.to_vec();
        buf.extend_from_slice(&[0u8; 16]);
        std::fs::write(dir.path().join(FICHIER_DOMAINE_ID), buf).unwrap();
        let e = lire(dir.path()).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn init_tronque_refuse() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FICHIER_DOMAINE_INIT), b"OBRI1\n\x00\x01").unwrap();
        let e = lire_init(dir.path()).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn init_version_inconnue_refuse() {
        let dir = tempfile::tempdir().unwrap();
        let mut buf = b"OBRI9\n".to_vec();
        buf.extend_from_slice(&[0u8; 8]);
        std::fs::write(dir.path().join(FICHIER_DOMAINE_INIT), buf).unwrap();
        let e = lire_init(dir.path()).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn init_empreinte_nulle_refuse() {
        let dir = tempfile::tempdir().unwrap();
        let mut buf = ENTETE_INIT.to_vec();
        buf.extend_from_slice(&[0u8; 8]);
        std::fs::write(dir.path().join(FICHIER_DOMAINE_INIT), buf).unwrap();
        let e = lire_init(dir.path()).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
    }

    /// Empreinte DISCORDANTE : `domaine.id` présent + `domaine.init` présent
    /// mais qui ne porte pas l'empreinte du domaine → refus à la résolution.
    #[test]
    fn empreinte_discordante_refuse() {
        let dir = tempfile::tempdir().unwrap();
        let id = lire_ou_materieliser(dir.path()).unwrap();
        // Remplace le marqueur par une empreinte DISCORDANTE (un octet retourné).
        let mut emp_fausse = empreinte(id);
        emp_fausse[0] ^= 0xff;
        let mut buf = ENTETE_INIT.to_vec();
        buf.extend_from_slice(&emp_fausse);
        std::fs::write(dir.path().join(FICHIER_DOMAINE_INIT), buf).unwrap();
        let e = lire_ou_materieliser(dir.path()).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
    }

    /// PERTE : `domaine.id` supprimé, marqueur restant → refus, et rien n'est
    /// réécrit (l'identité perdue n'est jamais réattribuée).
    #[test]
    fn perte_domaine_id_refuse() {
        let dir = tempfile::tempdir().unwrap();
        let id = lire_ou_materieliser(dir.path()).unwrap();
        std::fs::remove_file(dir.path().join(FICHIER_DOMAINE_ID)).unwrap();
        let e = lire_ou_materieliser(dir.path()).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
        assert!(!dir.path().join(FICHIER_DOMAINE_ID).exists());
        // Le marqueur, lui, est intact : l'empreinte perdue y reste lisible.
        assert_eq!(lire_init(dir.path()).unwrap(), Some(empreinte(id)));
    }

    /// Le CAS CRITIQUE : une base identifiée VIDE (aucune donnée) qui perd
    /// `domaine.id` est REFUSÉE — le marqueur atteste l'initialisation, là où
    /// l'ancienne heuristique d'occupation l'aurait ré-identifiée en silence.
    #[test]
    fn base_identifiee_vide_perte_refuse() {
        let dir = tempfile::tempdir().unwrap();
        lire_ou_materieliser(dir.path()).unwrap();
        std::fs::remove_file(dir.path().join(FICHIER_DOMAINE_ID)).unwrap();
        let e = lire_ou_materieliser(dir.path()).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
        assert!(!dir.path().join(FICHIER_DOMAINE_ID).exists());
    }

    /// MIGRATION legacy-domaine : `domaine.id` présent sans marqueur → la
    /// résolution écrit `domaine.init` (empreinte du domaine EXISTANT) et rend
    /// la MÊME identité — jamais une identité neuve.
    #[test]
    fn migration_legacy_domaine_ecrit_init() {
        let dir = tempfile::tempdir().unwrap();
        let id = tirer();
        ecrire_id_seul(dir.path(), id);
        assert_eq!(lire_init(dir.path()).unwrap(), None);
        let relu = lire_ou_materieliser(dir.path()).unwrap();
        assert_eq!(relu, id, "la migration ne change pas l'identité existante");
        assert_eq!(lire_init(dir.path()).unwrap(), Some(empreinte(id)));
    }

    /// Lecture seule : base jamais identifiée → identité ÉPHÉMÈRE, rien d'écrit.
    #[test]
    fn lecture_ephemere_sur_base_vide_n_ecrit_rien() {
        let dir = tempfile::tempdir().unwrap();
        let id = lire_ou_ephemere(dir.path()).unwrap();
        assert_ne!(id, 0);
        assert!(!dir.path().join(FICHIER_DOMAINE_ID).exists());
        assert!(!dir.path().join(FICHIER_DOMAINE_INIT).exists());
    }

    /// Lecture seule : base identifiée (marqueur concordant) → identité relue,
    /// rien d'écrit.
    #[test]
    fn lecture_ephemere_base_identifiee_lit_sans_ecrire() {
        let dir = tempfile::tempdir().unwrap();
        let id = lire_ou_materieliser(dir.path()).unwrap();
        let relu = lire_ou_ephemere(dir.path()).unwrap();
        assert_eq!(relu, id);
    }

    /// Lecture seule : base identifiée sans marqueur (legacy-domaine) → identité
    /// relue SANS migrer (contrat RO : aucun fichier créé).
    #[test]
    fn lecture_ephemere_legacy_domaine_lit_sans_migrer() {
        let dir = tempfile::tempdir().unwrap();
        let id = tirer();
        ecrire_id_seul(dir.path(), id);
        let relu = lire_ou_ephemere(dir.path()).unwrap();
        assert_eq!(relu, id);
        assert!(!dir.path().join(FICHIER_DOMAINE_INIT).exists());
    }

    /// Lecture seule : `domaine.init` présent SANS `domaine.id` → refus (perte).
    #[test]
    fn lecture_ephemere_perte_refuse() {
        let dir = tempfile::tempdir().unwrap();
        lire_ou_materieliser(dir.path()).unwrap();
        std::fs::remove_file(dir.path().join(FICHIER_DOMAINE_ID)).unwrap();
        let e = lire_ou_ephemere(dir.path()).unwrap_err();
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

    /// Fork : le marqueur est RÉÉCRIT à l'empreinte du NOUVEAU domaine (sinon la
    /// réouverture verrait une empreinte discordante et refuserait).
    #[test]
    fn forker_met_a_jour_le_marqueur() {
        let dir = tempfile::tempdir().unwrap();
        lire_ou_materieliser(dir.path()).unwrap();
        let nouveau = forker(dir.path()).unwrap();
        assert_eq!(lire_init(dir.path()).unwrap(), Some(empreinte(nouveau)));
        // La réouverture résout le nouveau domaine sans discordance.
        assert_eq!(lire_ou_materieliser(dir.path()).unwrap(), nouveau);
    }

    /// L'écriture atomique ne laisse pas de fichier partiel lisible.
    #[test]
    fn ecriture_atomique_renomme_sans_partiel() {
        let dir = tempfile::tempdir().unwrap();
        let id = tirer();
        ecrire_atomique(dir.path(), id).unwrap();
        assert_eq!(lire(dir.path()).unwrap(), Some(id));
        assert!(!dir.path().join(TMP_DOMAINE_ID).exists());
    }

    /// L'écriture atomique du marqueur ne laisse pas de fichier partiel.
    #[test]
    fn ecriture_init_atomique_renomme_sans_partiel() {
        let dir = tempfile::tempdir().unwrap();
        let id = tirer();
        ecrire_init_atomique(dir.path(), empreinte(id)).unwrap();
        assert_eq!(lire_init(dir.path()).unwrap(), Some(empreinte(id)));
        assert!(!dir.path().join(TMP_DOMAINE_INIT).exists());
    }

    /// Création CONCURRENTE (deux threads, même répertoire) : le verrou de
    /// fichier les sérialise — ils s'accordent sur UNE seule identité ET un seul
    /// marqueur concordant.
    #[test]
    fn creation_concurrente_un_identifiant_unique() {
        use std::sync::Arc;
        let dir = Arc::new(tempfile::tempdir().unwrap());
        let mut mains = Vec::new();
        for _ in 0..8 {
            let d = Arc::clone(&dir);
            mains.push(std::thread::spawn(move || {
                lire_ou_materieliser(d.path()).unwrap()
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
        assert_eq!(lire_init(dir.path()).unwrap(), Some(empreinte(premier)));
    }

    /// Migration CONCURRENTE (plusieurs threads) : ils s'accordent sur l'identité
    /// existante et n'écrivent qu'UN marqueur concordant.
    #[test]
    fn migration_concurrente_un_seul_marqueur() {
        use std::sync::Arc;
        let dir = Arc::new(tempfile::tempdir().unwrap());
        let id = tirer();
        ecrire_id_seul(dir.path(), id);
        let mut mains = Vec::new();
        for _ in 0..8 {
            let d = Arc::clone(&dir);
            mains.push(std::thread::spawn(move || {
                lire_ou_materieliser(d.path()).unwrap()
            }));
        }
        let identifiants: Vec<u128> = mains
            .into_iter()
            .map(|m| m.join().expect("thread paniqué"))
            .collect();
        assert!(identifiants.iter().all(|&i| i == id));
        assert_eq!(lire_init(dir.path()).unwrap(), Some(empreinte(id)));
    }

    /// Interruption APRÈS écriture du temporaire (crash avant le rename) : le
    /// `.tmp` orphelin est écrasé à la prochaine matérialisation, sans erreur.
    #[test]
    fn tmp_orphelin_repris_par_la_materialisation() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(TMP_DOMAINE_ID), b"partiel orphelin").unwrap();
        let id = lire_ou_materieliser(dir.path()).unwrap();
        assert_eq!(lire(dir.path()).unwrap(), Some(id));
        assert!(!dir.path().join(TMP_DOMAINE_ID).exists());
    }

    /// Interruption entre `domaine.id` et `domaine.init` (crash après le premier,
    /// avant le marqueur) : la réouverture voit « id présent + init absent » =
    /// migration — pas de perte.
    #[test]
    fn crash_entre_id_et_init_est_une_migration() {
        let dir = tempfile::tempdir().unwrap();
        // Simule l'état d'un processus mort après `ecrire_atomique` mais avant
        // `ecrire_init_atomique`.
        let id = tirer();
        ecrire_atomique(dir.path(), id).unwrap();
        assert_eq!(lire_init(dir.path()).unwrap(), None);
        let relu = lire_ou_materieliser(dir.path()).unwrap();
        assert_eq!(relu, id, "crash avant le marqueur = migration, pas de perte");
        assert_eq!(lire_init(dir.path()).unwrap(), Some(empreinte(id)));
    }

    /// Un `.tmp` de marqueur orphelin est repris par la migration/matérialisation.
    #[test]
    fn init_tmp_orphelin_repris() {
        let dir = tempfile::tempdir().unwrap();
        let id = tirer();
        ecrire_id_seul(dir.path(), id);
        std::fs::write(dir.path().join(TMP_DOMAINE_INIT), b"partiel").unwrap();
        assert_eq!(lire_ou_materieliser(dir.path()).unwrap(), id);
        assert_eq!(lire_init(dir.path()).unwrap(), Some(empreinte(id)));
        assert!(!dir.path().join(TMP_DOMAINE_INIT).exists());
    }

    /// Un `.tmp` orphelin laissé après un `forker` interrompu est de même repris.
    #[test]
    fn fork_apres_tmp_orphelin_est_sain() {
        let dir = tempfile::tempdir().unwrap();
        let premier = lire_ou_materieliser(dir.path()).unwrap();
        std::fs::write(dir.path().join(TMP_DOMAINE_ID), b"partiel").unwrap();
        let second = forker(dir.path()).unwrap();
        assert_ne!(premier, second);
        assert_eq!(lire(dir.path()).unwrap(), Some(second));
        assert!(!dir.path().join(TMP_DOMAINE_ID).exists());
    }
}
