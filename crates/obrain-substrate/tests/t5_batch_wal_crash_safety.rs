//! T5 batch-WAL crash-safety (plan b30650e2): validates that `SyncMode::Group`
//! combined with explicit `Writer::fsync_wal()` preserves the core invariant:
//! state reconstructed by replay always matches every record that was durable
//! (explicit `fsync_wal()` called) before the crash. Records written after
//! the last `fsync_wal()` MAY be lost (that is the whole point of batching —
//! bounded loss window in exchange for throughput), but must never corrupt
//! the WAL or produce a partially-applied record.
//!
//! Mirrors the method in crash_safety.rs (drop-without-msync / torn WAL) —
//! same two fault domains, but exercising the NEW opt-in `Group` mode
//! instead of the default `EveryCommit`.

use obrain_substrate::wal_io::SyncMode;
use obrain_substrate::{NodeRecord, SubstrateFile, Writer, replay_from};

fn sample_node(i: u32) -> NodeRecord {
    NodeRecord {
        label_bitset: 1u64 << (i % 64),
        ..Default::default()
    }
}

/// Group mode, no explicit fsync at all: writes are appended but never
/// synced. A hard crash right after should lose everything not yet flushed
/// by the OS — but replay must not panic or corrupt state; it should either
/// see all of it (if the OS happened to flush) or none of it cleanly.
#[test]
fn group_mode_without_fsync_replay_is_clean() {
    let td = tempfile::tempdir().unwrap();
    let sub_path = td.path().join("kb");

    {
        let sub = SubstrateFile::create(&sub_path).unwrap();
        let w = Writer::new(sub, SyncMode::Group).unwrap();
        for i in 0..500u32 {
            w.write_node(i, sample_node(i)).unwrap();
        }
        w.commit().unwrap();
        // No fsync_wal() call — everything after the last real durability
        // point (store creation) is "in flight" from the crash's point of
        // view. Drop without syncing.
    }

    // Replay must not panic/corrupt regardless of what actually hit disk.
    let sub = SubstrateFile::open(&sub_path).unwrap();
    let stats = replay_from(&sub, 0).unwrap();
    assert_eq!(stats.decode_errors, 0, "replay must not see corruption");
}

/// Group mode WITH periodic fsync_wal(): everything up to (and including)
/// the last fsync_wal() call must survive a simulated crash (WAL truncated
/// right after that point — the strongest adversarial case: as if the OS
/// had flushed nothing beyond the fsync'd byte offset).
#[test]
fn group_mode_periodic_fsync_bounds_the_loss_window() {
    let td = tempfile::tempdir().unwrap();
    let sub_path = td.path().join("kb");

    let wal_path = {
        let sub = SubstrateFile::create(&sub_path).unwrap();
        let wal_path = sub.wal_path();
        let w = Writer::new(sub, SyncMode::Group).unwrap();

        // First batch: 300 nodes, then an explicit fsync_wal() — this is
        // the batch-import pattern (T5): write a chunk, sync once, repeat.
        for i in 0..300u32 {
            w.write_node(i, sample_node(i)).unwrap();
        }
        w.commit().unwrap();
        w.fsync_wal().unwrap();
        // The commit record's own byte offset is a safe truncation point:
        // everything up to and including it was appended before fsync_wal().
        let offset_after_first_batch = std::fs::metadata(&wal_path).unwrap().len();

        // Second batch: 200 more nodes, NO fsync — simulates the crash
        // landing mid-batch, before the next scheduled fsync_wal().
        for i in 300..500u32 {
            w.write_node(i, sample_node(i)).unwrap();
        }
        w.commit().unwrap();

        (wal_path, offset_after_first_batch)
    };
    let (wal_path, synced_offset) = wal_path;

    // Adversarial simulation: truncate the WAL at the last fsync'd offset,
    // as if the OS had not flushed a single byte of the unsynced second
    // batch (worst case for Group mode — real disks usually do better).
    let f = std::fs::OpenOptions::new()
        .write(true)
        .open(&wal_path)
        .unwrap();
    f.set_len(synced_offset).unwrap();
    drop(f);

    let sub = SubstrateFile::open(&sub_path).unwrap();
    let stats = replay_from(&sub, 0).unwrap();
    assert_eq!(
        stats.decode_errors, 0,
        "truncation at an fsync'd boundary must replay cleanly, no corruption"
    );

    // The first 300 nodes (fsync'd) MUST be reconstructed.
    let nz = sub.open_zone(obrain_substrate::file::Zone::Nodes).unwrap();
    let slice: &[NodeRecord] = bytemuck::cast_slice(&nz.as_slice()[..300 * NodeRecord::SIZE]);
    for i in 0..300u32 {
        assert_eq!(
            slice[i as usize].label_bitset,
            1u64 << (i % 64),
            "fsync'd node {i} must survive — this is the durability contract"
        );
    }
    // Nodes 300..500 (unsynced second batch) are NOT guaranteed — the test
    // only asserts they don't corrupt anything; whether they replayed or
    // not depends on exactly where the truncation landed relative to
    // record boundaries, which is precisely the documented tradeoff.
}
