//! Atomic checkpoint — durably snapshot the substrate state and reset the WAL.
//!
//! A checkpoint is the mechanism that keeps the WAL bounded: every time we
//! checkpoint, all mutations up to that point are reflected on disk in the
//! zone mmaps and the WAL can safely start over from offset 0.
//!
//! ## Protocol
//!
//! 1. **Pause writes** — the caller holds an exclusive lock on the writer so
//!    no new WAL records can be appended during the checkpoint.
//! 2. **Flush zones** — `msync` every live zone mmap, then `fsync` each
//!    backing file. Zone bytes are now durable.
//! 3. **Meta rewrite (atomic)** — build a new `MetaHeader` with
//!    `last_checkpoint = now`, `last_wal_offset = 0`, and write it to
//!    `substrate.meta.tmp`, fsync, rename → `substrate.meta`.
//! 4. **Checkpoint marker** — append a `WalPayload::Checkpoint` record with
//!    `FLAG_CHECKPOINT` to the new WAL. This marker is the first frame of
//!    the post-checkpoint WAL; any replay will see it at offset 0.
//! 5. **WAL truncate** — the new WAL starts with only the checkpoint marker.
//!
//! ## Crash safety
//!
//! The rename in step 3 is atomic on POSIX: the new meta either exists
//! (checkpoint succeeded) or doesn't (the old meta + old WAL are intact and
//! will be replayed). Either way the on-disk state is consistent.

use crate::error::{SubstrateError, SubstrateResult};
use crate::file::{SubstrateFile, Zone};
use crate::meta::MetaHeader;
use crate::wal::{WalPayload, WalRecord};
use crate::wal_io::{ElectionCheckpoint, WalWriter};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use tracing::info;

/// Outcome of a checkpoint operation.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct CheckpointStats {
    /// WAL byte offset at the start of the checkpoint (will be 0 after).
    pub wal_offset_before: u64,
    /// The LSN stamped into the Checkpoint marker in the new WAL.
    pub at_lsn: u64,
    /// `last_checkpoint` timestamp written into the new meta.
    pub checkpoint_timestamp: i64,
}

/// ── QUI A CHECKPOINTÉ ? (Phase B, pièce 1) ──────────────────────────────────
///
/// Un checkpoint n'est pas une opération locale : il TRONQUE un fichier que
/// d'autres processus écrivent. Le rendre élu (un seul checkpointeur à la fois,
/// désigné par un `flock` sur `<wal>.chk`) dissocie deux issues qui n'ont rien à
/// voir :
///
/// * [`CheckpointOutcome::Fait`] — nous étions élus, le WAL a été tronqué ;
/// * [`CheckpointOutcome::NonElu`] — un pair détenait le fence. **Rien n'a été
///   fait, et ce n'est pas un échec** : le pair vient de faire le même travail.
///   Un appelant automatique (surveillance de taille) doit le traiter comme un
///   « rien à faire » et surtout PAS comme une panne — sans quoi chaque flush
///   d'un store multi-écrivains journaliserait un avertissement mensonger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckpointOutcome {
    /// Nous étions le checkpointeur élu : le WAL a été tronqué.
    Fait(CheckpointStats),
    /// Un autre processus détenait le fence : RIEN n'a été fait.
    NonElu {
        /// Chemin du fence détenu par le pair — pour que le refus soit
        /// actionnable (« qui » n'est pas nommable sans sonder `/proc`, mais
        /// « quoi » l'est).
        fence: PathBuf,
    },
}

impl CheckpointOutcome {
    /// Les statistiques, si le checkpoint a bien eu lieu.
    #[must_use]
    pub fn stats(&self) -> Option<CheckpointStats> {
        match self {
            Self::Fait(s) => Some(*s),
            Self::NonElu { .. } => None,
        }
    }

    /// Le checkpoint a-t-il été fait par nous ?
    #[must_use]
    pub fn fait(&self) -> bool {
        matches!(self, Self::Fait(_))
    }
}

/// Perform a full checkpoint, apres ELECTION.
///
/// ── L'ÉLECTION EST ICI, ET PAS CHEZ L'APPELANT ──────────────────────────────
/// Le fence est pris dans cette fonction, avant la PREMIÈRE E/S, et rendu à sa
/// sortie (y compris sur un chemin d'erreur : [`ElectionCheckpoint`] le rend dans
/// son `Drop`). Un appelant ne peut donc ni l'oublier, ni l'inverser, ni le
/// garder par accident. C'est la même discipline que l'ordre ancre-avant-
/// troncature, qui vit dans [`WalWriter::truncate`] pour la même raison.
pub fn checkpoint_elu(
    substrate: &mut SubstrateFile,
    wal: &WalWriter,
) -> SubstrateResult<CheckpointOutcome> {
    let fence = wal.path().to_path_buf();
    let Some(election) = ElectionCheckpoint::tenter(&fence) else {
        return Ok(CheckpointOutcome::NonElu {
            fence: crate::wal_io::chemin_fence_checkpoint(&fence),
        });
    };
    let stats = checkpoint_sous_fence(substrate, wal, &election)?;
    Ok(CheckpointOutcome::Fait(stats))
}

/// Le corps du checkpoint, pour un appelant qui a DÉJÀ gagné l'élection.
///
/// ── POURQUOI CETTE PORTE EXISTE SÉPARÉMENT DE `checkpoint_elu` ──────────────
/// `SubstrateStore::checkpoint_elu` doit prendre le fence AVANT son `flush()`
/// (sinon chaque processus du store paierait le msync de toutes les zones pour
/// qu'un seul soit utile) ; il ne peut donc pas appeler `checkpoint_elu`, qui
/// reprendrait le fence. Cette porte rend le corps réutilisable sans jamais
/// laisser un appelant exécuter un checkpoint NON élu : elle exige un
/// [`ElectionCheckpoint`] en argument, c'est-à-dire une preuve d'élection qui ne
/// se fabrique qu'en gagnant le `flock`.
pub fn checkpoint_sous_election(
    substrate: &mut SubstrateFile,
    wal: &WalWriter,
    election: &ElectionCheckpoint,
) -> SubstrateResult<CheckpointStats> {
    checkpoint_sous_fence(substrate, wal, election)
}

/// Perform a full checkpoint.
///
/// The caller owns `substrate` (typically wrapped in an `Arc<Mutex<..>>`) and
/// `wal` and MUST ensure no concurrent writes are in flight for the duration
/// of this call. The WAL is truncated and reseeded with a Checkpoint marker.
///
/// Cette porte ÉCHOUE si un pair détient le fence ([`SubstrateError::
/// CheckpointNonElu`]) : un appelant qui demande explicitement un checkpoint veut
/// savoir qu'il n'a pas eu lieu, plutôt que de croire à un succès qui a tronqué
/// le segment d'un autre.
pub fn checkpoint(
    substrate: &mut SubstrateFile,
    wal: &WalWriter,
) -> SubstrateResult<CheckpointStats> {
    match checkpoint_elu(substrate, wal)? {
        CheckpointOutcome::Fait(s) => Ok(s),
        CheckpointOutcome::NonElu { fence } => Err(SubstrateError::CheckpointNonElu {
            fence: fence.display().to_string(),
        }),
    }
}

/// Le corps du checkpoint, exécuté SOUS le fence d'élection.
fn checkpoint_sous_fence(
    substrate: &mut SubstrateFile,
    wal: &WalWriter,
    _election: &ElectionCheckpoint,
) -> SubstrateResult<CheckpointStats> {
    let before = wal.offset();

    // ---- (1) Flush zones. Open each zone briefly, msync+fsync. -------------
    for zone in [
        Zone::Nodes,
        Zone::Edges,
        Zone::Props,
        Zone::Strings,
        Zone::Tier0,
        Zone::Tier1,
        Zone::Tier2,
        Zone::Hilbert,
        Zone::Community,
        Zone::Zorder,
        Zone::SimplexDir,
        Zone::SimplexIncidence,
    ] {
        let zf = substrate.open_zone(zone)?;
        // An empty zone has no mmap; skip silently.
        if !zf.is_empty() {
            zf.msync()?;
            zf.fsync()?;
        }
    }
    // Side-cars simpliciaux nommés (T4) — même discipline que les zones.
    for name in [
        crate::simplex_zone::SIMPLEX_HEAP_FILENAME,
        crate::simplex_incidence::SIMPLEX_POSTINGS_FILENAME,
    ] {
        let zf = substrate.open_named_zone(name)?;
        if !zf.is_empty() {
            zf.msync()?;
            zf.fsync()?;
        }
    }

    // ---- (2) Build the new meta header and atomically write it. ------------
    let mut header = substrate.meta_header();
    let now = unix_seconds();
    header.last_checkpoint = now;
    header.last_wal_offset = 0;
    atomically_write_meta(substrate.path(), &header)?;
    // Reopen the mmap-backed meta with the new bytes.
    substrate.write_meta_header(&header)?;

    // ---- (3) Truncate the WAL and write the checkpoint marker. ------------
    // ── L'ORDRE EST L'INVARIANT (pièce 4, 2026-10-06) ───────────────────────
    // `truncate()` publie d'abord l'ANCRE durable, qui avance de la taille du
    // segment abandonné, PUIS tronque. L'ordre n'est pas ici, il est DANS
    // `WalWriter::truncate` : un appelant ne peut donc pas l'inverser ni
    // l'oublier. Tronquer avant d'avancer l'ancre ferait repartir les positions
    // à 0 avec l'ancienne base, et les LSN du nouveau segment RECOUVRERAIENT
    // ceux du précédent — deux enregistrements distincts porteraient le même
    // LSN, et `at_lsn` ci-dessous mentirait.
    let ancienne_ancre = wal.ancre();
    let taille_segment = wal.offset();
    wal.truncate()?;
    // ── SOUS GROUP COMMIT, LE SEGMENT ABANDONNÉ EST PLUS GRAND QUE `offset()` ──
    // `truncate` VIDE d'abord le tampon du group commit (sinon l'ancre avancerait
    // d'une taille qui ne compte pas les frames en attente, et leurs LSN seraient
    // RÉÉMIS dans le segment suivant — le doublon que l'ancre existe pour
    // empêcher). Le segment abandonné fait donc `taille_segment + octets en
    // attente`, et l'égalité stricte ci-dessus ne tient plus dès qu'un tampon
    // n'est pas vide au moment du checkpoint.
    //
    // L'INVARIANT QUI COMPTE est conservé : l'ancre avance d'AU MOINS la taille
    // visible, et ne recule JAMAIS (aucun LSN n'est réémis, `at_lsn` ne désigne
    // jamais une époque révolue). C'est ce sens-là que ce garde protège — le
    // chiffre exact du delta, lui, dépend de ce que le WAL avait en attente.
    debug_assert!(
        wal.ancre() >= ancienne_ancre + taille_segment,
        "l'ancre a RECULÉ au checkpoint : {} < {} + {} — des LSN vont être réémis",
        wal.ancre(),
        ancienne_ancre,
        taille_segment
    );
    let at_lsn = wal.peek_lsn();
    let marker = WalRecord {
        lsn: 0, // assigned by append
        timestamp: unix_micros(),
        flags: WalRecord::FLAG_CHECKPOINT | WalRecord::FLAG_COMMIT,
        payload: WalPayload::Checkpoint { at_lsn },
    };
    wal.append(marker)?;
    wal.fsync()?;
    // ── SCELLER LE PLANCHER DU SEGMENT ──────────────────────────────────────
    // Une fois le marqueur posé, la taille courante devient le PLANCHER du
    // nouveau segment : à la réouverture, un fichier plus court que ce plancher
    // est une troncature ÉTRANGÈRE, et elle est nommée comme telle
    // (`Reconciliation::TroncatureEtrangere`). Sans ce plancher, une troncature
    // à zéro serait indiscernable d'un WAL neuf.
    wal.sceller_segment()?;

    // ---- (4) Drop a checkpoint marker file next to the WAL. ---------------
    // Useful for external tooling (obrain-migrate, audit) to see the last
    // checkpoint without reading the WAL.
    let marker_path = substrate.checkpoint_path();
    let body = format!("{now}\n{at_lsn}\n");
    fs::write(&marker_path, body)?;

    info!(
        wal_before = before,
        at_lsn,
        timestamp = now,
        "checkpoint completed"
    );
    Ok(CheckpointStats {
        wal_offset_before: before,
        at_lsn,
        checkpoint_timestamp: now,
    })
}

/// Write the meta header to `<dir>/substrate.meta.tmp`, fsync it, then rename
/// to `<dir>/substrate.meta`. The rename is atomic on POSIX.
fn atomically_write_meta(dir: &Path, header: &MetaHeader) -> SubstrateResult<()> {
    let final_path = dir.join(crate::file::zone::META);
    let tmp_path = dir.join(format!("{}.tmp", crate::file::zone::META));

    let current_len = fs::metadata(&final_path)?.len() as usize;
    let mut buf = vec![0u8; current_len.max(crate::meta::META_FILE_SIZE)];
    buf[..core::mem::size_of::<MetaHeader>()].copy_from_slice(bytemuck::bytes_of(header));

    {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp_path)?;
        f.write_all(&buf)?;
        f.sync_all()?;
    }
    fs::rename(&tmp_path, &final_path)?;
    // Fsync the directory so the rename is durable.
    #[cfg(unix)]
    {
        let d = fs::File::open(dir)?;
        d.sync_all()?;
    }
    Ok(())
}

// Les deux horloges du checkpoint suivent la même règle que
// `meta::now_unix_seconds` : sur wasm32 il n'existe AUCUNE source de temps
// (`SystemTime::now()` y panique), et zéro est la réponse honnête parce que
// les deux valeurs alimentent des champs DOCUMENTAIRES — `last_checkpoint`
// dans l'en-tête meta, `timestamp` d'un `WalRecord` (dont l'ordre réel vient
// du `lsn`, cf. `wal.rs`), et le texte du fichier marqueur lu par les outils
// externes. Aucune reprise ne trie sur ces valeurs.
fn unix_seconds() -> i64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
    #[cfg(target_arch = "wasm32")]
    {
        0
    }
}

fn unix_micros() -> i64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_micros() as i64)
            .unwrap_or(0)
    }
    #[cfg(target_arch = "wasm32")]
    {
        0
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file::SubstrateFile;
    use crate::record::{NodeRecord, PackedScarUtilAff, U48, f32_to_q1_15};
    use crate::wal_io::{SyncMode, WalReader};
    use crate::writer::Writer;
    use tempfile::tempdir;

    fn n(i: u32) -> NodeRecord {
        NodeRecord {
            label_bitset: i as u64 + 1,
            first_edge_off: U48::default(),
            first_prop_off: U48::default(),
            community_id: 0,
            energy: f32_to_q1_15(0.5),
            scar_util_affinity: PackedScarUtilAff::new(0, 0, 0, false).pack(),
            centrality_cached: 0,
            flags: 0,
            // (jalon 2) appartenance : fixture publique.
            tenant_id: 0,
            _reserve: 0,
        }
    }

    #[test]
    fn checkpoint_resets_wal() {
        let dir = tempdir().unwrap();
        let sub_path = dir.path().join("kb");
        let sub = SubstrateFile::create(&sub_path).unwrap();
        let wal_path = sub.wal_path();
        let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();
        for i in 0..5u32 {
            w.write_node(i, n(i)).unwrap();
        }
        w.commit().unwrap();
        let before = w.wal().offset();
        assert!(before > 0);

        // Checkpoint needs an exclusive &mut SubstrateFile — drop the writer's
        // ownership and re-open the substrate directly.
        let wal_writer = w.wal();
        drop(w);
        let mut sub2 = SubstrateFile::open(&sub_path).unwrap();
        let stats = checkpoint(&mut sub2, &wal_writer).unwrap();
        assert_eq!(stats.wal_offset_before, before);

        // The new WAL should contain exactly one record: the checkpoint marker.
        let r = WalReader::open(&wal_path).unwrap();
        let items: Vec<_> = r.iter_from(0).collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(items.len(), 1);
        assert!(items[0].0.is_checkpoint());
        matches!(items[0].0.payload, WalPayload::Checkpoint { .. });

        // Meta reflects the checkpoint.
        let h = sub2.meta_header();
        assert!(h.last_checkpoint > 0);
        assert_eq!(h.last_wal_offset, 0);
    }

    #[test]
    fn checkpoint_persists_zone_data() {
        let dir = tempdir().unwrap();
        let sub_path = dir.path().join("kb");
        let sub = SubstrateFile::create(&sub_path).unwrap();
        let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();
        for i in 0..20u32 {
            w.write_node(i, n(i)).unwrap();
        }
        w.commit().unwrap();
        let wal_writer = w.wal();
        drop(w);
        let mut sub2 = SubstrateFile::open(&sub_path).unwrap();
        checkpoint(&mut sub2, &wal_writer).unwrap();

        // Reopen, no replay needed — checkpoint flushed everything.
        drop(sub2);
        let sub3 = SubstrateFile::open(&sub_path).unwrap();
        let nz = sub3.open_zone(Zone::Nodes).unwrap();
        let slice: &[NodeRecord] = bytemuck::cast_slice(&nz.as_slice()[..20 * NodeRecord::SIZE]);
        for i in 0..20u32 {
            assert_eq!(slice[i as usize].label_bitset, i as u64 + 1);
        }
    }

    #[test]
    fn checkpoint_marker_file_written() {
        let dir = tempdir().unwrap();
        let sub_path = dir.path().join("kb");
        let sub = SubstrateFile::create(&sub_path).unwrap();
        let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();
        w.commit().unwrap();
        let wal_writer = w.wal();
        drop(w);
        let mut sub2 = SubstrateFile::open(&sub_path).unwrap();
        let marker = sub2.checkpoint_path();
        checkpoint(&mut sub2, &wal_writer).unwrap();
        assert!(marker.exists());
        let body = std::fs::read_to_string(&marker).unwrap();
        assert!(body.lines().count() >= 2);
    }

    #[test]
    fn replay_after_checkpoint_sees_only_marker() {
        let dir = tempdir().unwrap();
        let sub_path = dir.path().join("kb");
        {
            let sub = SubstrateFile::create(&sub_path).unwrap();
            let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();
            for i in 0..10u32 {
                w.write_node(i, n(i)).unwrap();
            }
            w.commit().unwrap();
            let wal_writer = w.wal();
            drop(w);
            let mut sub2 = SubstrateFile::open(&sub_path).unwrap();
            checkpoint(&mut sub2, &wal_writer).unwrap();
        }

        // Reopen + replay: should find exactly 1 applied record (the marker).
        let sub = SubstrateFile::open(&sub_path).unwrap();
        let stats = crate::replay::replay_from(&sub, 0).unwrap();
        assert_eq!(stats.applied, 1);
        assert_eq!(stats.decode_errors, 0);
    }
}
