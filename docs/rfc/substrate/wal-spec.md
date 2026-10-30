# Substrate Storage — Write-Ahead Log Specification

**Status**: Draft
**Companion of**: `format-spec.md`
**Goal**: Durability, crash-safety, and replay for all mutations to the Substrate.

## 1. Design principles

1. **Everything goes through WAL first.** No mmap mutation is visible/durable before its WAL record is fsync'd.
2. **Binary, append-only, framed.** Each record is length-prefixed and CRC-protected. No seek.
3. **Replay is idempotent.** Records carry enough information to be re-applied after a crash without duplicating effects.
4. **Bounded recovery.** Checkpoints bound the tail of WAL that must be replayed at boot (target: < 100 ms for PO, < 1 s for megalaw).
5. **No locking on the hot read path.** Readers consult mmap directly; writers serialize through the WAL appender + mmap writer.

## 2. On-disk framing

Each record is:

```
offset  size  field
------  ----  -----
0       4     length       (u32 LE — payload length in bytes, excluding this header)
4       1     kind         (u8 — WalRecord discriminant, see §3)
5       1     flags        (u8 — bit 0 = "commit marker", 1 = "checkpoint boundary")
6       2     reserved     (u16 LE, zero)
8       8     lsn          (u64 LE — monotonic Log Sequence Number)
16      8     timestamp    (i64 LE — Unix micros)
24      4     crc32        (u32 LE — crc32 of bytes [0..24] XOR payload)
28      ...   payload      (length bytes, bincode-serialized `WalPayload` enum)
```

- Records are written with `O_APPEND` — **et c'est ce qui rend l'append atomique
  entre PROCESSUS** (avant le 2026-10-06 le fichier n'était PAS ouvert en ajout :
  deux processus écrivaient au même offset de descripteur et le second écrasait
  le premier, frame par frame, avec des CRC valides — 83,3 % de perte mesurée).
- **UNE SEULE écriture par record** (jamais `write_all`, qui BOUCLE et peut donc
  poser une frame HYBRIDE après un `EINTR`/`ENOSPC` partiel). Une écriture courte
  est ANNULÉE par troncature à la position de départ ; au-delà de
  `MAX_FRAME_BYTES` (1 Gio), la frame est refusée à l'encodage.
- `lsn = ancre + position` : le LSN est DÉRIVÉ de l'offset octet de la frame, et
  l'ancre est durable (side-car `<wal>.lsn`, 32 octets, avancée par le checkpoint
  élu). `lsn` reste la source de vérité pour la causalité, mais ce n'est plus un
  compteur : en multi-écrivain (régime `OAppend`) c'est une **borne inférieure**
  de la position. `timestamp` reste informatif.
- Un TROU de zéros SUIVI DE DONNÉES est NOMMÉ (`WalTrou`), jamais confondu avec
  une fin de fichier pré-allouée ni avec une frontière de crash.

## 3. Record kinds

```rust
#[repr(u8)]
pub enum WalKind {
    NodeInsert   = 0x01,
    NodeUpdate   = 0x02,
    NodeDelete   = 0x03,
    EdgeInsert   = 0x10,
    EdgeUpdate   = 0x11,
    EdgeDelete   = 0x12,
    PropSet      = 0x20,
    PropDelete   = 0x21,
    StringIntern = 0x30,  // append a new entry to StringHeap
    LabelIntern  = 0x31,  // extend label dictionary
    KeyIntern    = 0x32,  // extend property-key dictionary

    // Cognitive state mutations (high-frequency, small)
    EnergyDecay        = 0x40,  // batch decay all nodes (records a scale factor, not per-node)
    EnergyReinforce    = 0x41,
    SynapseReinforce   = 0x42,
    ScarUtilAffinitySet = 0x43,
    CentralityUpdate   = 0x44,

    // Topology maintenance
    CommunityAssign    = 0x50,  // batch: list of (node_id, community_id) pairs
    HilbertRepermute   = 0x51,  // batch: full permutation snapshot (idempotent)
    RicciUpdate        = 0x52,  // batch: list of (edge_id, ricci_u8)

    // Embedding tier updates
    Tier0Update        = 0x60,
    Tier1Update        = 0x61,
    Tier2Update        = 0x62,

    // Control
    Checkpoint         = 0xF0,  // marks a durable checkpoint of the mmap
    NoOp               = 0xFE,
    EndOfLog           = 0xFF,
}
```

**Payload** is `bincode` (v2.0, configured with little-endian, fixed-int encoding) of a `WalPayload` enum matching the kinds. For batch kinds, the payload carries `Vec<T>`.

## 4. Write path (hot)

```
Writer thread                    Reader threads
-------------                    --------------
1. acquire AppenderMutex         (lock-free — mmap reads)
2. assemble WalRecord
3. write(wal_fd, record_bytes)
4. fsync(wal_fd)                  (≤ 1× per batch; grouped via "commit marker")
5. apply in-memory update         (mutate mmap pages or cognitive state)
6. release AppenderMutex
```

**Batching**: mutations in the same logical transaction are stamped with DISTINCT
`lsn` (each record carries its own position) and share a trailing record with
`flags.bit(0) = 1` ("commit marker"). `WalWriter::append_batch` pose jusqu'à N
records en UN SEUL `write` (group commit) : un lot est, sur le disque,
indiscernable de N appends successifs. Readers never observe partial transactions because the mmap update in step 5 happens **after** the fsync of step 4.

**Group-commit**: writers may coalesce up to `wal_group_commit_ms` (default 2 ms) of concurrent mutations into a single fsync. Measured latency target: p99 < 5 ms for single-node update on NVMe.

**Cognitive high-frequency writes**: `EnergyReinforce`, `SynapseReinforce`, `ScarUtilAffinitySet` bypass fsync and rely on **periodic coalesced fsync** every `wal_cognitive_flush_ms` (default 200 ms). Rationale: cognitive state is inherently lossy (decay/reinforcement converges); losing the last 200 ms on crash is acceptable. This is opt-in per-mutation via a `durability: Durability::Relaxed` flag.

## 5. Replay

At boot:

1. Open `substrate.meta`; read `last_wal_offset` and `last_checkpoint`.
2. Open `substrate.wal` with `O_RDONLY`.
3. Seek to `last_wal_offset`.
4. For each record:
   a. Verify `crc32`. On mismatch ⇒ truncate WAL at this offset (tail corruption, common on SIGKILL mid-write) and abort replay.
   b. Decode `WalPayload`.
   c. Apply to mmap + in-memory indexes (same path as the hot writer, minus the WAL append).
   d. Track highest `lsn` seen.
5. After replay: write a new `Checkpoint` record, update `substrate.meta.last_wal_offset`, fsync, and rename atomically.

**Invariant**: replay must be idempotent. Record kinds are designed so that re-applying a record that was already durably persisted in mmap (but the checkpoint marker wasn't flushed) is a no-op:
- `NodeInsert` carries the assigned `node_id`; if the slot is already populated with the same payload, skip.
- `EnergyReinforce` is a write, not an increment — idempotent by construction.
- `SynapseReinforce` carries the absolute new weight, not the delta.

## 6. Checkpointing

A checkpoint is a durable snapshot of the in-memory + mmap state such that everything before `checkpoint.lsn` can be truncated from WAL.

**Procedure**:
1. Acquire CheckpointMutex (blocks new WAL appends briefly).
2. `msync(mmap, MS_SYNC)` on all data files (`nodes`, `edges`, `props`, `strings`, `tier0/1/2`, `hilbert`, `community`).
3. Write a fresh `substrate.meta.tmp` with updated `last_wal_offset` and `last_checkpoint`.
4. `fsync(meta.tmp)`; `rename(meta.tmp, meta)`; `fsync(parent_dir)`.
5. **Publier l'ANCRE durable** (`<wal>.lsn` : `ancre += taille du segment`, par
   tmp+rename) — AVANT la troncature, sinon les LSN du nouveau segment
   RECOUVRERAIENT ceux du précédent. L'ordre est dans `WalWriter::truncate()`,
   pas chez l'appelant.
6. Append a `Checkpoint` WalRecord, puis `sceller_segment()` (relève le plancher
   du side-car : un fichier plus court que son plancher est une troncature
   ÉTRANGÈRE, et elle est nommée à l'ouverture suivante).
6. Truncate `substrate.wal` to the Checkpoint record boundary.
7. Release CheckpointMutex.

**Frequency**:
- Periodic: every `checkpoint_interval_secs` (default 300 s).
- Pressure-based: when `wal_size_bytes > wal_max_size_bytes` (default 512 MiB).
- On graceful shutdown (`Drop`): always.

**Not** on SIGKILL — that's what replay is for.

## 7. Durability modes

| Mode | Fsync policy | Use case |
|------|--------------|----------|
| `Durability::Sync`      | fsync per commit | default for schema mutations, decisions, RFC acceptance |
| `Durability::Batched`   | group-commit, fsync every ≤ 2 ms | default for property updates and edge inserts |
| `Durability::Relaxed`   | fsync every 200 ms | default for cognitive reinforcement |
| `Durability::InMemory`  | no WAL at all | tests, ephemeral scratch graphs |

Per-call override via `SubstrateStore::with_durability(...)`.

## 8. Error handling

- **CRC mismatch on replay** → truncate at offset, log a `corruption_detected` event, continue.
- **Short tail write** (EOF mid-record) → same as CRC mismatch.
- **Fsync error** during checkpoint → retry up to 3 times with 100 ms backoff, then PANIC with diagnostic (disk full or hardware failure).
- **Unknown `WalKind`** during replay → abort; indicates version mismatch. Use `obrain-migrate` to upgrade.

## 9. Concurrency model

- **Single appender thread** owns `wal_fd` and the mmap write path (serialized through `AppenderMutex`).
- **Multiple reader threads** consult mmap directly, lock-free. They may observe torn cognitive-state fields in the rare case of a racy read during write, but any such read is bounded to a single `u16` and is never used for a decision — only for display.
- For transactional reads (required by GraphStore trait), a reader takes a `Shared` snapshot handle that pins the mmap pages via `madvise(MADV_WILLNEED)` and records the current `lsn` — subsequent writes do not invalidate the snapshot (mmap is copy-on-write at the OS level for writes, but we use MAP_SHARED so we must document that snapshot isolation is **best-effort**, not serializable).

## 10. Test plan

See T3 (Foundation WAL) and T4 (crash-safety):
- [ ] Property test: random sequence of mutations + random crash + replay ⇒ graph state equals state at last committed LSN.
- [ ] SIGKILL test: spawn child, run workload, kill -9, re-open, verify invariants (no orphan edges, no duplicated nodes, idempotent).
- [ ] Fuzz tail corruption: truncate WAL at every byte offset in the last record, verify recovery.
- [x] **Axe APPEND (2026-10-06)** — ces cases sont désormais couvertes par des
  tests réels, y compris INTER-PROCESSUS (ce que la spécification ne prévoyait
  pas) : `tests/mw_interprocess_preuve.rs` (T2 : 6 processus, zéro perte, EOF
  exact — **rouge avant : 83,3 % de perte**), `tests/mw_interprocess_append.rs`
  (T2/T2b/T3/T4/T7 + débit à N écrivains), `tests/wal_append_unite.rs`
  (réconciliation d'ouverture, trou nommé, borne de taille, ENOSPC),
  `tests/wal_ecriture_courte.rs` (quota disque réel), `tests/wal_latence_append.rs`.
- [x] Benchmark group-commit : `benches/wal_append.rs` (chiffres datés dans
  `benches/BANCS.md`).

## 11. Open questions

- Should we support **multiple WAL segments** (log rotation) instead of a single truncating file? Needed if we want to ship WAL for replication (Phase 3+).
- Should `Durability::Relaxed` use a dedicated fsync thread rather than piggybacking on the maintenance tick?
- Is `bincode` the right payload format, or should we use a hand-rolled `#[repr(C)]` layout for the hot cognitive kinds to skip allocation on replay?
