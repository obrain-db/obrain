//! Identité de DOMAINE persistante — protocole DEUX FICHIERS (`domaine.id` +
//! `domaine.init`) + JOURNAL DE FORK (`domaine.fork`), second incrément natif
//! (#334).
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
//! l'ambiguïté SANS jamais relire le contenu des données.
//!
//! # Protocole TROIS FICHIERS
//!
//! * `domaine.id`  — l'identité (u128, UUID v4), format `OBRD1\n` + 16 octets ;
//! * `domaine.init` — le MARQUEUR D'INITIALISATION : l'EMPREINTE COMPLÈTE du
//!   `domaine_id` sur 128 bits (les 16 octets, ordre big-endian), format
//!   `OBRI1\n` + 16 octets — jamais un préfixe 64 bits, qui ferait partager le
//!   même marqueur par deux domaines distincts ;
//! * `domaine.fork` — le JOURNAL de fork (transitoire) : posé AVANT de réécrire
//!   `domaine.id`/`domaine.init` lors d'un fork, retiré APRÈS. Sa présence
//!   atteste « un fork est en cours ou a été interrompu », ce qui rend la
//!   fenêtre d'interruption RÉCUPÉRABLE (voir matrice ci-dessous).
//!
//! `domaine.init` est écrit ATOMIQUEMENT APRÈS `domaine.id`, à la PREMIÈRE
//! matérialisation (`create` ou migration legacy explicite) — et réécrit à
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
//!                octets 6..22  l'EMPREINTE : l'identité COMPLÈTE sur 16 octets
//!                              (u128, big-endian).
//!                longueur totale 22 octets.
//!
//! domaine.fork : octets 0..6   b"OBRF1\n" (magic OBRF + version 1 + \n)
//!                octets 6..22  le NOUVEAU domaine_id vers lequel le fork
//!                              interrompu doit converger.
//!                longueur totale 22 octets.
//! ```
//!
//! Toute autre longueur, entête inconnue (version non reconnue) ou valeur NULLE
//! (identité/empreinte à zéro) est un refus EXPLICITE (`InvalidData`) — jamais
//! une régénération silencieuse, qui masquerait une corruption ou un fork d'une
//! version future.
//!
//! # Régimes d'ouverture
//!
//! Trois régimes, trois traitements du cas « les DEUX fichiers absents » :
//!
//! * **Création** (`create`, migration legacy explicite `migrer_legacy`) — la
//!   base est VIDE ou l'opérateur demande EXPLICITEMENT l'identification : on
//!   MATÉRIALISE une identité neuve persistée ;
//! * **Réouverture** (`open` en écriture) — la base EXISTE et son historique
//!   est INCONNU : deux fichiers absents ne prouvent PAS qu'elle n'a jamais été
//!   identifiée (les deux ont pu être supprimés). On ne réattribue donc PAS
//!   d'identité persistée : identité ÉPHÉMÈRE (jamais refusée, jamais écrite),
//!   jusqu'à ce qu'un `migrer_legacy` explicite la matérialise ;
//! * **Lecture seule** — jamais d'écriture : identité relue ou ÉPHÉMÈRE.
//!
//! # Matrice de décision à l'ouverture
//!
//! | `domaine.id` | `domaine.init`         | Création            | Réouverture | Lecture seule   |
//! |--------------|------------------------|---------------------|-------------|-----------------|
//! | présent      | présent (concordant)   | LUE                 | LUE         | LUE             |
//! | présent      | ABSENT                 | MIGRER (écrire init)| MIGRER      | LUE (sans écrire) |
//! | ABSENT       | présent                | PERTE → refus       | PERTE → refus | PERTE → refus  |
//! | ABSENT       | ABSENT                 | MATÉRIALISER        | ÉPHÉMÈRE    | ÉPHÉMÈRE        |
//! | présent      | présent (DISCORDANT)   | refus               | refus       | refus           |
//!
//! En LECTURE SEULE, aucun fichier n'est créé (contrat RO) : « legacy-domaine »
//! rend l'identité lue SANS écrire `init`, et « jamais identifié » rend une
//! identité ÉPHÉMÈRE non persistée.
//!
//! # Durabilité et concurrence
//!
//! Chaque fichier est écrit par PUBLICATION ATOMIQUE : écriture de `*.tmp`
//! (`sync_all`, données + métadonnées), `rename` vers le final (atomique POSIX),
//! puis fsync du répertoire (durabilité du rename). Un lecteur ne voit jamais un
//! fichier partiel. La matérialisation, la migration et le fork prennent le
//! verrou de fichier `substrate.domaine.lock` (`flock`, MULTI-PROCESSUS) : deux
//! créateurs concurrents s'accordent sur UNE identité. Le verrou est un fichier
//! STABLE (jamais renommé), comme `substrate.dirlock`.
//!
//! # Matrice d'interruption (récupérable et vérifiable)
//!
//! Deux `rename` atomiques ne forment pas une transaction commune ; chaque
//! fenêtre est donc soit RÉCUPÉRABLE automatiquement, soit VÉRIFIABLE (refus
//! explicite avec diagnostic) — jamais une réattribution silencieuse.
//!
//! **Matérialisation** (les deux fichiers absents → écrire id puis init) :
//!
//! | Crash                    | État relu            | Verdict                              |
//! |--------------------------|----------------------|--------------------------------------|
//! | avant `domaine.id`       | absent + absent      | re-matérialisation (idempotent)      |
//! | entre id et init         | présent + absent     | MIGRER : écrire init (récupéré)      |
//! | après init               | présent + présent    | LUE (terminé)                        |
//!
//! **Migration** (`domaine.id` présent sans marqueur → écrire init) :
//!
//! | Crash                    | État relu            | Verdict                              |
//! |--------------------------|----------------------|--------------------------------------|
//! | avant init               | présent + absent     | MIGRER à nouveau                     |
//! | après init               | présent + présent    | LUE                                  |
//!
//! **Fork** (journal → id → init → retrait du journal) :
//!
//! | Crash                     | État relu                     | Verdict                                   |
//! |---------------------------|-------------------------------|-------------------------------------------|
//! | avant journal             | ancien id + ancien init       | rien à récupérer (fork pas commencé)      |
//! | entre journal et id       | journal + ancien id           | RÉCUPÉRÉ : écrire id+init neufs           |
//! | entre id et init          | journal + id neuf + init ancien | RÉCUPÉRÉ : écrire init neuf             |
//! | après init, avant retrait | journal + id neuf + init neuf | RÉCUPÉRÉ : retirer le journal            |
//! | après retrait             | id neuf + init neuf           | LUE (terminé)                             |
//!
//! Un `domaine.fork` corrompu (longueur, magic ou valeur nulle) est un refus
//! EXPLICITE, jamais un effacement silencieux.
//!
//! # Limite assumée (documentée)
//!
//! 1. Une copie intégrale NON déclarée (simple `cp -a`) partage le MÊME
//!    `domaine_id` — la distinguer exige `forker` (structurel : « déplacer » et
//!    « copier » produisent des répertoires byte-identiques, seule l'intention
//!    de l'opérateur les distingue — voir le RFC `contrat-identite-domaine.md`,
//!    §5).
//! 2. La suppression des DEUX fichiers (`domaine.id` ET `domaine.init`) rend la
//!    base INDISTINGUABLE d'un legacy jamais identifié. En RÉOUVERTURE, on ne
//!    la ré-identifie donc PLUS automatiquement (identité éphémère) : une
//!    identité persistée exige `migrer_legacy`, l'opération explicite. Seule la
//!    perte de `domaine.id` SEUL (marqueur restant) est refusée.

use std::io::{self, Read};
use std::path::Path;

/// Nom du fichier qui porte le `domaine_id` DANS le répertoire de la base.
pub const FICHIER_DOMAINE_ID: &str = "domaine.id";

/// Nom du MARQUEUR D'INITIALISATION (`domaine.init`) : l'empreinte COMPLÈTE
/// (128 bits) du `domaine_id` qui a matérialisé la base. Sa présence atteste
/// « cette base a été identifiée », indépendamment de toute donnée — c'est lui
/// qui refuse la ré-identification d'une base identifiée VIDE ayant perdu
/// `domaine.id`.
pub const FICHIER_DOMAINE_INIT: &str = "domaine.init";

/// JOURNAL de fork (`domaine.fork`) : posé AVANT de réécrire l'identité lors
/// d'un fork, retiré APRÈS. Transitoire, comme le verrou — jamais transporté
/// dans un instantané. Sa présence rend un fork interrompu RÉCUPÉRABLE.
pub const FICHIER_DOMAINE_FORK: &str = "domaine.fork";

/// Verrou de fichier du domaine : sérialise la matérialisation, la migration,
/// le fork et la récupération de fork entre processus. Fichier STABLE, jamais
/// renommé (le `rename` de `domaine.id`/`domaine.init` remplacerait l'inode et
/// rendrait un `flock` posé dessus sans objet — même raison que
/// `substrate.dirlock`). Contenu sans signification.
pub(crate) const FICHIER_DOMAINE_VERROU: &str = "substrate.domaine.lock";

/// Nom du fichier TEMPORAIRE de la publication atomique (`ecrire_atomique`).
const TMP_DOMAINE_ID: &str = "domaine.id.tmp";

/// Nom du fichier TEMPORAIRE de la publication atomique du marqueur.
const TMP_DOMAINE_INIT: &str = "domaine.init.tmp";

/// Nom du fichier TEMPORAIRE de la publication atomique du journal de fork.
const TMP_DOMAINE_FORK: &str = "domaine.fork.tmp";

/// Entête du format v1 de `domaine.id` : magic `OBRD` + version `1` + `\n`.
const ENTETE: &[u8; 6] = b"OBRD1\n";

/// Longueur totale d'une entrée `domaine.id` valide : entête (6) + u128 (16).
const TAILLE: usize = 6 + 16;

/// Entête du format v1 de `domaine.init` : magic `OBRI` + version `1` + `\n`.
const ENTETE_INIT: &[u8; 6] = b"OBRI1\n";

/// Longueur totale d'une entrée `domaine.init` valide : entête (6) + empreinte (16).
const TAILLE_INIT: usize = 6 + 16;

/// Entête du format v1 de `domaine.fork` : magic `OBRF` + version `1` + `\n`.
const ENTETE_FORK: &[u8; 6] = b"OBRF1\n";

/// Longueur totale d'une entrée `domaine.fork` valide : entête (6) + u128 (16).
const TAILLE_FORK: usize = 6 + 16;

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

/// L'EMPREINTE d'un `domaine_id` : l'identité COMPLÈTE sur 128 bits (les 16
/// octets, ordre big-endian). Les bits de version UUID (nibble haut de l'octet
/// 6) y sont, donc une empreinte valide n'est jamais nulle. 128 bits, et non le
/// préfixe 64 bits : un préfixe tronqué ferait partager le même marqueur par
/// deux domaines distincts — le marqueur ne prouverait plus rien.
fn empreinte(id: u128) -> [u8; 16] {
    id.to_be_bytes()
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
/// * `Err(InvalidData)` — marqueur corrompu (longueur ≠ 22), entête inconnue
///   (version non reconnue) ou empreinte NULLE (16 octets à zéro) : refus
///   EXPLICITE, jamais de régénération.
fn lire_init(dir: &Path) -> io::Result<Option<[u8; 16]>> {
    let chemin = dir.join(FICHIER_DOMAINE_INIT);
    match std::fs::File::open(&chemin) {
        Ok(mut f) => {
            let mut buf = Vec::with_capacity(TAILLE_INIT);
            f.read_to_end(&mut buf)?;
            if buf.len() != TAILLE_INIT {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "domaine.init : {} octets attendus (entête 6 + empreinte 16), {} lus",
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
            let emp: [u8; 16] = buf[ENTETE_INIT.len()..]
                .try_into()
                .expect("TAILLE_INIT garantit 16 octets après l'entête");
            if emp == [0u8; 16] {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "domaine.init : empreinte nulle (16 octets à zéro) — corruption, refus, pas de régénération",
                ));
            }
            Ok(Some(emp))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Lit le journal de fork `domaine.fork` dans `dir`.
///
/// * `Ok(Some(nouveau))` — journal présent et valide (identité non nulle) ;
/// * `Ok(None)` — journal ABSENT (pas de fork interrompu) ;
/// * `Err(InvalidData)` — journal corrompu (longueur ≠ 22, entête inconnue ou
///   valeur nulle) : refus EXPLICITE, jamais d'effacement silencieux.
fn lire_journal_fork(dir: &Path) -> io::Result<Option<u128>> {
    let chemin = dir.join(FICHIER_DOMAINE_FORK);
    match std::fs::File::open(&chemin) {
        Ok(mut f) => {
            let mut buf = Vec::with_capacity(TAILLE_FORK);
            f.read_to_end(&mut buf)?;
            if buf.len() != TAILLE_FORK {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "domaine.fork : {} octets attendus (entête 6 + u128 16), {} lus",
                        TAILLE_FORK,
                        buf.len()
                    ),
                ));
            }
            if &buf[..ENTETE_FORK.len()] != ENTETE_FORK {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "domaine.fork : entête inconnue (magic/version non reconnus) — refus, pas d'effacement",
                ));
            }
            let octets: [u8; 16] = buf[ENTETE_FORK.len()..]
                .try_into()
                .expect("TAILLE_FORK garantit 16 octets après l'entête");
            let id = u128::from_be_bytes(octets);
            if id == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "domaine.fork : identité nulle (16 octets à zéro) — corruption, refus, pas d'effacement",
                ));
            }
            Ok(Some(id))
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
fn ecrire_init_atomique(dir: &Path, emp: [u8; 16]) -> io::Result<()> {
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

/// Écrit le journal de fork `domaine.fork` par publication atomique (même
/// séquence de durabilité que `ecrire_atomique`). Le journal DOIT être durable
/// AVANT la première réécriture de `domaine.id`/`domaine.init`, sinon un crash
/// entre le journal et l'identité ne serait pas récupérable.
fn ecrire_journal_fork(dir: &Path, nouveau: u128) -> io::Result<()> {
    let final_chemin = dir.join(FICHIER_DOMAINE_FORK);
    let tmp_chemin = dir.join(TMP_DOMAINE_FORK);
    let mut buf = Vec::with_capacity(TAILLE_FORK);
    buf.extend_from_slice(ENTETE_FORK);
    buf.extend_from_slice(&nouveau.to_be_bytes());
    let es = crate::es::espace();
    es.ecrire_synchronise(&tmp_chemin, &buf)?;
    es.renommer(&tmp_chemin, &final_chemin)?;
    es.synchroniser_repertoire(dir)?;
    Ok(())
}

/// Retire le journal de fork `domaine.fork` et rend le retrait durable.
fn effacer_journal_fork(dir: &Path) -> io::Result<()> {
    let es = crate::es::espace();
    match es.supprimer(&dir.join(FICHIER_DOMAINE_FORK)) {
        Ok(()) => es.synchroniser_repertoire(dir),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
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
    /// Cas « jamais identifié » en CRÉATION explicite : les deux fichiers
    /// absents. Tirer une identité neuve, écrire `domaine.id` puis
    /// `domaine.init`.
    Materialiser,
}

/// Verdict de la matrice de décision pour un état (domaine.id, domaine.init).
enum Decision {
    /// Identité résolue sans écriture (ouverture normale, ou lecture seule).
    Lue(u128),
    /// Identité ÉPHÉMÈRE (jamais persistée) : lecture seule, ou réouverture
    /// d'une base jamais identifiée dont l'historique est inconnu.
    Ephemere,
    /// Une écriture est requise (écrivain seulement).
    Ecrire(Ecriture),
    /// Refus explicite (`InvalidData`), jamais de réattribution silencieuse.
    Refus(io::Error),
}

/// Régime d'ouverture du domaine : détermine le traitement du cas « les deux
/// fichiers absents » et celui de la migration.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Regime {
    /// Création EXPLICITE (`create`) ou migration legacy EXPLICITE
    /// (`migrer_legacy`) : matérialise une identité persistée sur « absent +
    /// absent ».
    Creation,
    /// Réouverture en écriture (`open`) : sur « absent + absent », identité
    /// ÉPHÉMÈRE (jamais refusée, jamais persistée) — l'historique est inconnu,
    /// la matérialisation exige `migrer_legacy`.
    Reouverture,
    /// Lecture seule : aucune écriture, identité ÉPHÉMÈRE sur « absent +
    /// absent ».
    LectureSeule,
}

impl Regime {
    /// Ce régime écrit-il ? (tout sauf la lecture seule).
    fn ecrit(self) -> bool {
        self != Regime::LectureSeule
    }
}

/// La matrice de décision de l'identité de domaine (protocole TROIS FICHIERS).
///
/// `regime` sépare la création explicite (matérialise), la réouverture en
/// écriture (éphémère sur « absent + absent », migre sur « présent + absent »)
/// et le lecteur en lecture seule (éphémère, ne migre pas — contrat RO).
fn decider(id: Option<u128>, init: Option<[u8; 16]>, regime: Regime) -> Decision {
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
            if regime.ecrit() {
                Decision::Ecrire(Ecriture::Migrer(id))
            } else {
                Decision::Lue(id)
            }
        }
        // (3) domaine.id ABSENT + domaine.init PRÉSENT → PERTE d'identité : refus
        // EXPLICITE, même si la base est VIDE (le marqueur atteste l'initialisation).
        (None, Some(_)) => Decision::Refus(erreur_identite_perdue()),
        // (4) les deux absents : seule la CRÉATION explicite matérialise. La
        // réouverture et la lecture seule rendent une identité ÉPHÉMÈRE (jamais
        // refusée, jamais persistée) — deux fichiers absents ne prouvent PAS que
        // la base n'a jamais été identifiée.
        (None, None) => match regime {
            Regime::Creation => Decision::Ecrire(Ecriture::Materialiser),
            Regime::Reouverture | Regime::LectureSeule => Decision::Ephemere,
        },
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

/// Récupère un fork interrompu (journal `domaine.fork` présent). LE VERROU DU
/// DOMAINE EST DÉJÀ TENU par l'appelant. Idempotent : force `domaine.id` et
/// `domaine.init` à converger vers l'identité portée par le journal, puis
/// retire le journal. Un journal corrompu est un refus explicite (remonté tel
/// quel par `lire_journal_fork`).
fn recuperer_fork_interrompu(dir: &Path) -> io::Result<()> {
    let nouveau = match lire_journal_fork(dir)? {
        None => return Ok(()),
        Some(nouveau) => nouveau,
    };
    ecrire_atomique(dir, nouveau)?;
    ecrire_init_atomique(dir, empreinte(nouveau))?;
    effacer_journal_fork(dir)?;
    Ok(())
}

/// Résout l'identité de domaine selon la matrice, en écrivant si nécessaire.
fn resoudre(dir: &Path, regime: Regime) -> io::Result<u128> {
    // Récupération d'un fork interrompu : l'ÉCRIVAIN seulement (le lecteur ne
    // peut pas écrire), et seulement s'il y a un journal à récupérer.
    if regime.ecrit() && dir.join(FICHIER_DOMAINE_FORK).exists() {
        let _verrou = verrouiller_domaine(dir)?;
        recuperer_fork_interrompu(dir)?;
    }
    match decider(lire(dir)?, lire_init(dir)?, regime) {
        Decision::Lue(id) => Ok(id),
        Decision::Ephemere => Ok(tirer()),
        Decision::Refus(e) => Err(e),
        Decision::Ecrire(_) => {
            // Verrou : deux écrivains concurrents (même répertoire, plusieurs
            // processus en écriture partagée) s'accordent sur UNE identité.
            let _verrou = verrouiller_domaine(dir)?;
            // Relecture SOUS verrou : un pair a pu matérialiser/migrer/forker
            // entre-temps. L'action recalculée sur l'état re-lu fait foi.
            match decider(lire(dir)?, lire_init(dir)?, regime) {
                Decision::Lue(id) => Ok(id),
                Decision::Ephemere => Ok(tirer()),
                Decision::Refus(e) => Err(e),
                Decision::Ecrire(action) => executer(dir, action),
            }
        }
    }
}

/// Matérialise (ou lit) l'identité de domaine selon le régime CRÉATION :
/// identité neuve persistée sur « les deux fichiers absents ». C'est l'opération
/// de `create`.
///
/// * identité valide (marqueur concordant ou absent) → relue (migration pour
///   « présent + absent ») ;
/// * `domaine.id` absent + `domaine.init` présent → ERREUR (identité perdue) ;
/// * les deux absents → matérialiser une identité neuve + `domaine.init`.
pub(crate) fn lire_ou_materieliser(dir: &Path) -> io::Result<u128> {
    resoudre(dir, Regime::Creation)
}

/// Migration legacy EXPLICITE : même matrice que la création (matérialise une
/// identité persistée sur « absent + absent », migre le marqueur sur « présent
/// + absent »), mais posée comme l'opération EXPLICITE qui lève l'ambiguïté
/// d'une base existante non vide à l'historique inconnu. C'est l'opération que
/// le consommateur appelle pour établir une identité stable sur une base legacy
/// ouverte en réouverture (qui, elle, n'a reçu qu'une identité éphémère).
pub fn migrer_legacy(dir: &Path) -> io::Result<u128> {
    resoudre(dir, Regime::Creation)
}

/// Résout l'identité à la RÉOUVERTURE en écriture (`open`).
///
/// * identité valide → relue ; « présent + absent » → migration (écrire le
///   marqueur), jamais de nouvelle identité ;
/// * `domaine.id` absent + `domaine.init` présent → ERREUR (identité perdue) ;
/// * les deux absents → identité ÉPHÉMÈRE (jamais persistée) : l'historique est
///   inconnu, la matérialisation exige `migrer_legacy`.
pub(crate) fn lire_ou_reouvrir(dir: &Path) -> io::Result<u128> {
    resoudre(dir, Regime::Reouverture)
}

/// Lecture SANS écriture (contrat lecture seule) : rend l'identité si présente
/// (marqueur concordant, ou `domaine.id` sans marqueur — sans migrer) ; si les
/// DEUX fichiers sont absents, identité ÉPHÉMÈRE (jamais persistée) ; si
/// `domaine.init` est présent SANS `domaine.id`, ERREUR (identité perdue).
pub(crate) fn lire_ou_ephemere(dir: &Path) -> io::Result<u128> {
    resoudre(dir, Regime::LectureSeule)
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
/// l'écriture, pas la divergence voulue.
///
/// Le JOURNAL `domaine.fork` rend le fork RÉCUPÉRABLE : un fork interrompu
/// entre les deux écritures (ou avant elles) est complété à la prochaine
/// réouverture en écriture, au lieu de laisser `domaine.id` et `domaine.init`
/// discordants (refus opérateur). Voir la matrice d'interruption en tête de
/// module.
pub fn forker(dir: &Path) -> io::Result<u128> {
    let _verrou = verrouiller_domaine(dir)?;
    // Un fork interrompu antérieur : le récupérer d'abord (idempotent), puis
    // procéder au fork demandé.
    recuperer_fork_interrompu(dir)?;
    let id = tirer();
    ecrire_journal_fork(dir, id)?;
    ecrire_atomique(dir, id)?;
    ecrire_init_atomique(dir, empreinte(id))?;
    effacer_journal_fork(dir)?;
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
        buf.extend_from_slice(&[0u8; 16]);
        std::fs::write(dir.path().join(FICHIER_DOMAINE_INIT), buf).unwrap();
        let e = lire_init(dir.path()).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn init_empreinte_nulle_refuse() {
        let dir = tempfile::tempdir().unwrap();
        let mut buf = ENTETE_INIT.to_vec();
        buf.extend_from_slice(&[0u8; 16]);
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

    /// RÉOUVERTURE : une base dont les DEUX fichiers d'identité sont absents ne
    /// reçoit PAS d'identité persistée — identité ÉPHÉMÈRE, rien d'écrit. Deux
    /// fichiers absents ne prouvent pas que la base n'a jamais été identifiée :
    /// on ne la ré-identifie donc pas en silence.
    #[test]
    fn reouverture_sans_fichiers_est_ephemere() {
        let dir = tempfile::tempdir().unwrap();
        let id = lire_ou_reouvrir(dir.path()).unwrap();
        assert_ne!(id, 0);
        assert!(!dir.path().join(FICHIER_DOMAINE_ID).exists());
        assert!(!dir.path().join(FICHIER_DOMAINE_INIT).exists());
    }

    /// MIGRATION legacy explicite : matérialise une identité persistée sur
    /// « absent + absent ».
    #[test]
    fn migrer_legacy_materialise_une_identite_persistee() {
        let dir = tempfile::tempdir().unwrap();
        let id = migrer_legacy(dir.path()).unwrap();
        assert_eq!(lire(dir.path()).unwrap(), Some(id));
        assert_eq!(lire_init(dir.path()).unwrap(), Some(empreinte(id)));
    }

    /// Réouverture (éphémère) PUIS migration explicite : l'identité devient
    /// stable et persistée ; les réouvertures suivantes la relisent.
    #[test]
    fn reouverture_puis_migration_rend_l_identite_stable() {
        let dir = tempfile::tempdir().unwrap();
        let _ephemere = lire_ou_reouvrir(dir.path()).unwrap();
        assert!(!dir.path().join(FICHIER_DOMAINE_ID).exists());
        let stable = migrer_legacy(dir.path()).unwrap();
        assert_ne!(stable, 0);
        assert!(dir.path().join(FICHIER_DOMAINE_ID).exists());
        assert_eq!(lire_ou_reouvrir(dir.path()).unwrap(), stable);
        assert_eq!(lire_ou_materieliser(dir.path()).unwrap(), stable);
    }

    /// Fork interrompu ENTRE le journal et `domaine.id` (id/init restés anciens) :
    /// la réouverture en écriture RÉCUPÈRE (écrit id+init neufs, retire le
    /// journal), au lieu de refuser.
    #[test]
    fn fork_interrompu_avant_id_est_recupere_par_reouverture() {
        let dir = tempfile::tempdir().unwrap();
        let ancien = lire_ou_materieliser(dir.path()).unwrap();
        let nouveau = tirer();
        assert_ne!(nouveau, ancien);
        ecrire_journal_fork(dir.path(), nouveau).unwrap();
        assert_eq!(lire(dir.path()).unwrap(), Some(ancien));
        let relu = lire_ou_reouvrir(dir.path()).unwrap();
        assert_eq!(relu, nouveau, "la réouverture doit récupérer le fork");
        assert_eq!(lire(dir.path()).unwrap(), Some(nouveau));
        assert_eq!(lire_init(dir.path()).unwrap(), Some(empreinte(nouveau)));
        assert!(!dir.path().join(FICHIER_DOMAINE_FORK).exists());
    }

    /// Fork interrompu ENTRE `domaine.id` et `domaine.init` (id neuf, init
    /// ancien) : la réouverture récupère en réécrivant le marqueur.
    #[test]
    fn fork_interrompu_entre_id_et_init_est_recupere() {
        let dir = tempfile::tempdir().unwrap();
        let _ancien = lire_ou_materieliser(dir.path()).unwrap();
        let nouveau = tirer();
        ecrire_journal_fork(dir.path(), nouveau).unwrap();
        ecrire_atomique(dir.path(), nouveau).unwrap(); // id neuf, init ancien
        let relu = lire_ou_reouvrir(dir.path()).unwrap();
        assert_eq!(relu, nouveau);
        assert_eq!(lire_init(dir.path()).unwrap(), Some(empreinte(nouveau)));
        assert!(!dir.path().join(FICHIER_DOMAINE_FORK).exists());
    }

    /// Fork interrompu APRÈS init mais AVANT le retrait du journal : la
    /// réouverture ne fait que retirer le journal (idempotent).
    #[test]
    fn fork_interrompu_apres_init_retire_le_journal() {
        let dir = tempfile::tempdir().unwrap();
        lire_ou_materieliser(dir.path()).unwrap();
        let nouveau = tirer();
        ecrire_journal_fork(dir.path(), nouveau).unwrap();
        ecrire_atomique(dir.path(), nouveau).unwrap();
        ecrire_init_atomique(dir.path(), empreinte(nouveau)).unwrap();
        let relu = lire_ou_reouvrir(dir.path()).unwrap();
        assert_eq!(relu, nouveau);
        assert!(!dir.path().join(FICHIER_DOMAINE_FORK).exists());
    }

    /// Journal de fork corrompu (entête inconnue) : refus explicite, jamais
    /// d'effacement silencieux.
    #[test]
    fn journal_fork_version_inconnue_refuse() {
        let dir = tempfile::tempdir().unwrap();
        lire_ou_materieliser(dir.path()).unwrap();
        let mut buf = b"OBRF9\n".to_vec();
        buf.extend_from_slice(&[0u8; 16]);
        std::fs::write(dir.path().join(FICHIER_DOMAINE_FORK), buf).unwrap();
        let e = lire_ou_reouvrir(dir.path()).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
        assert!(dir.path().join(FICHIER_DOMAINE_FORK).exists());
    }

    /// Journal de fork à identité nulle : refus explicite.
    #[test]
    fn journal_fork_nul_refuse() {
        let dir = tempfile::tempdir().unwrap();
        lire_ou_materieliser(dir.path()).unwrap();
        let mut buf = ENTETE_FORK.to_vec();
        buf.extend_from_slice(&[0u8; 16]);
        std::fs::write(dir.path().join(FICHIER_DOMAINE_FORK), buf).unwrap();
        let e = lire_ou_reouvrir(dir.path()).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
    }
}
