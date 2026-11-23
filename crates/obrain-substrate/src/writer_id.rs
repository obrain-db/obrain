//! Identité d'écrivain stable (`writer_id`), pour #334.
//!
//! Le `writer_id` identifie UN ÉCRIVAIN — un démon, un import, un banc —
//! pas un store ni un processus : il est tiré une fois, persisté dans
//! `writer.id` dans le répertoire d'état de cet écrivain, et relu à ses
//! démarrages suivants. Il ne remplace pas l'identité de transaction MP-8
//! (`identite_de_processus`, 31 bits, par ouverture) : il sert à attribuer
//! une écriture à son auteur de façon stable, condition d'une règle de
//! convergence (HLC + tie-break).
//!
//! PIÈGE : le répertoire est celui de l'ÉCRIVAIN, jamais celui du store. En
//! écriture partagée, deux processus ouvrent le MÊME répertoire de store ;
//! s'ils y lisaient `writer.id`, ils auraient le même identifiant et l'auteur
//! d'une écriture serait indécidable. C'est pourquoi `SubstrateStore` ne
//! déduit pas de `writer_id` de son chemin : l'appelant le fournit.
//!
//! Le fichier n'est pas un champ de `substrate.meta` : cet en-tête est de
//! taille figée (96 octets), y ajouter un champ imposerait une migration de
//! format — et l'identité d'un écrivain n'appartient de toute façon pas au
//! store.

use std::io::{self, Read, Write};
use std::path::Path;

/// Nom du fichier qui porte le `writer_id` dans le répertoire d'état de
/// l'écrivain.
pub const FICHIER_WRITER_ID: &str = "writer.id";

/// Valeurs qu'un `writer_id` ne doit jamais prendre : elles sont réservées par
/// le mécanisme de transactions (MP-8, `SYSTEM=1`, `INVALID=u64::MAX`).
fn valeur_reservee(id: u64) -> bool {
    id == 0 || id == 1 || id == u64::MAX
}

/// Tire un `writer_id` frais, hors valeurs réservées.
fn tirer() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    loop {
        // Graine aléatoire par `RandomState`, mêlée à l'horloge et au pid.
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u32(std::process::id());
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        h.write_u128(nanos);
        let id = h.finish();
        if !valeur_reservee(id) {
            return id;
        }
    }
}

/// Renvoie le `writer_id` de l'écrivain dont le répertoire d'état est `dir`,
/// en le créant au premier appel. Les appels suivants sur le même répertoire
/// rendent la même valeur : même répertoire = même écrivain, par définition.
///
/// `dir` est le répertoire de l'ÉCRIVAIN, pas celui du store (voir le piège
/// en tête de module). La création écrit `writer.id` dans `dir`.
///
/// # Errors
///
/// Remonte les erreurs d'E/S, et une `InvalidData` si le fichier existant ne
/// fait pas 8 octets ou porte une valeur réservée.
pub fn writer_id(dir: &Path) -> io::Result<u64> {
    let chemin = dir.join(FICHIER_WRITER_ID);
    match std::fs::File::open(&chemin) {
        Ok(mut f) => {
            let mut buf = Vec::with_capacity(8);
            f.read_to_end(&mut buf)?;
            let octets: [u8; 8] = buf.as_slice().try_into().map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("writer.id : 8 octets attendus, {} lus", buf.len()),
                )
            })?;
            let id = u64::from_le_bytes(octets);
            if valeur_reservee(id) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("writer.id : valeur réservée {id}"),
                ));
            }
            Ok(id)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let id = tirer();
            // `create_new` : si un autre écrivain l'a créé entre-temps, on ne
            // l'écrase pas, et on relit sa valeur.
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&chemin)
            {
                Ok(mut f) => {
                    f.write_all(&id.to_le_bytes())?;
                    f.sync_all()?;
                    Ok(id)
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => writer_id(dir),
                Err(e) => Err(e),
            }
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Critère d'acceptation de l'étape 1 de #334 : deux démarrages du MÊME
    /// écrivain (même répertoire d'état) rendent le même `writer_id`.
    #[test]
    fn meme_repertoire_meme_writer_id() {
        let dir = tempfile::tempdir().unwrap();
        let premier = writer_id(dir.path()).unwrap();
        let second = writer_id(dir.path()).unwrap();
        assert_eq!(premier, second);
        assert!(!valeur_reservee(premier));
    }

    /// Deux écrivains distincts (deux répertoires d'état) reçoivent deux
    /// identités distinctes.
    #[test]
    fn repertoires_distincts_writer_ids_distincts() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        assert_ne!(writer_id(a.path()).unwrap(), writer_id(b.path()).unwrap());
    }

    /// Le scénario de #334 : deux écrivains sur le MÊME store, chacun avec
    /// son répertoire d'état. Ils restent distinguables. Le même test écrit
    /// avec le répertoire du store à la place rendrait un seul identifiant
    /// pour les deux — c'est le piège nommé en tête de module.
    #[test]
    fn deux_ecrivains_sur_le_meme_store_restent_distincts() {
        let store = tempfile::tempdir().unwrap();
        let etat_a = tempfile::tempdir().unwrap();
        let etat_b = tempfile::tempdir().unwrap();
        let a = writer_id(etat_a.path()).unwrap();
        let b = writer_id(etat_b.path()).unwrap();
        assert_ne!(
            a, b,
            "deux écrivains du même store doivent rester distincts"
        );
        // Le répertoire du store ne porte aucun writer.id : il n'en a pas.
        assert!(!store.path().join(FICHIER_WRITER_ID).exists());
    }

    /// Un fichier corrompu (mauvaise taille) est refusé, pas réinterprété.
    #[test]
    fn fichier_corrompu_refuse() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FICHIER_WRITER_ID), b"trop court").unwrap();
        let err = writer_id(dir.path()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    /// Une valeur réservée stockée sur disque est refusée.
    #[test]
    fn valeur_reservee_refusee() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FICHIER_WRITER_ID), 1u64.to_le_bytes()).unwrap();
        let err = writer_id(dir.path()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }
}
