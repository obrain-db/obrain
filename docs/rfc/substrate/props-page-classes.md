# Substrate Storage — Property Page Classes

**Status**: Draft
**Companion of**: `format-spec.md` (§5 PropertyPage, §12 Versioning)
**Addresses**: obrain-db/obrain#181
**Measurement**: `crates/obrain-substrate/examples/histogramme_props_181.rs` (PR #276)

## 1. Problem

`substrate.props.v2` allocates a 4 KiB page per node. On the Project Orchestrator
base (`~/.obrain/db/po`, 2 531 328 node slots) the zone is **9.01 GiB logical /
7.44 GiB allocated** for **169 548 265 bytes of property entries** — an
amplification of **47×** by entry bytes.

Measured distribution (same tool, 14 s, two sequential scans, no random I/O):

| pages per node | nodes | share |
|---|---|---|
| 1 | 1 937 563 | 99.90 % |
| 2 | 1 004 | 0.05 % |
| 7 | 990 | 0.05 % |
| 8–9 | 10 | < 0.01 % |

| bytes used (`free_offset` / 4 072) | pages | share |
|---|---|---|
| 0 – 255 | 1 925 502 | 98.92 % |
| 256 – 1023 | 605 | 0.03 % |
| 1024 – 2047 | 14 337 | 0.74 % |
| 3 968 – 4 072 (full) | 6 005 | 0.31 % |

Method check: 20 000 chains re-walked from `first_prop_off` following
`next_page` — 19 989 agree with the per-owner page count (99.94 %), 11 pages
enumerated but unreachable.

Separately, **411 027 pages are entirely zero, in a single contiguous run at
the end of the file** (indices 1951177..2362202, 1.57 GiB, 17.4 % of the logical
length). That is the ×1.5 growth margin, not freed pages: it costs logical
length (copies, backups, non-sparse filesystems) but no allocated blocks on
APFS.

**Conclusion the measurement forces**: the cost is granularity, not overflow.
An overflow fix (bigger pages) would be wrong. A growth-margin fix (`piste 2`)
is orthogonal and does not touch these 7.4 GiB.

## 2. Proposal A (recommended) — a small page class per owner

Introduce a **page class** for node-owned property pages:

```rust
pub enum PropPageClass { Small, Large }   // 256 B, 4096 B
```

- **File layout**: add `substrate.props.small.v3`, an array of 256-byte pages,
  same header semantics as today (a 256-byte page keeps the 24-byte header and
  gives 232 bytes of payload: enough for the 98.92 % of pages using ≤ 255 bytes
  once the entries are re-encoded, and for six short properties as PO writes
  them).
- **Chain locality**: a chain never mixes classes, because `next_page` is a page
  index inside one file. A chain that outgrows `Small` is **re-encoded into
  `Large`** and the small pages are released. Measured cost: 2 004 nodes out of
  1 939 568 (0.10 %) would ever pay this copy.
- **Class discovery**: the class is implied by the head page's magic. Two new
  magic values (`PROP_PAGE_MAGIC_NODE_SMALL`, `..._EDGE_SMALL`) keep
  `first_prop_off` a single number — no `NodeRecord` change, no new flag bit.
  `u48` stays the page index within the file the magic selects.
- **Promotion rule**: first append to an owner opens a `Small` page; when it
  fills, the chain moves to `Large` and never comes back (no hysteresis needed,
  measured overflow is 0.10 %).
- **Free list**: released small pages go on a per-file free list rather than
  growing the file, because churn on properties is expected (1 619 640
  tombstones are declared on PO).

**Expected effect on PO**: 1 925 502 pages × 256 B ≈ **0.46 GiB** where 7.35 GiB
is used today, plus 21 084 nodes on `Large` (≈ 86 MiB) and the tail of the
growth margin handled by Proposal C. Target: `props.v2` + `props.small.v3`
allocated **≤ 1.0 GiB**, from 7.44 GiB.

## 3. Proposal B (considered, rejected for now) — slotted pages shared by owners

Pack several small nodes into one 4 KiB page with a slot directory.

Rejected as the first step, not as a wrong idea:

- it makes a property page address a *pair* (page, slot), so `first_prop_off`
  no longer fits in the `u48` field as a plain page index — every reader of
  `NodeRecord` changes, including `props.idx.v2`, `props.eq.idx.v2` and
  `props.ord.idx.v2`;
- page compaction now moves entries of *other* owners, so a node update can
  invalidate another node's offsets;
- the measured distribution shows a 256-byte class already captures 98.92 % of
  pages, so most of the win is available without changing the addressing model.

It remains the right answer if the small class proves insufficient (e.g. many
nodes under 64 bytes).

## 4. Proposal C (independent, cheap) — truncate the growth margin at checkpoint

At `checkpoint()`/`close()`, `set_len` the zone to the highest written page
index + 1, keeping the ×1.5 growth during the session.

- Effect measured today: **1.57 GiB** of logical length (17.4 %) removed, no
  allocated-block change on APFS.
- Value: copies, `rsync`, container images, filesystems without holes, and any
  tool that trusts `st_size`.
- Risk: none for readers (the file grows again on the next write, with the same
  ×1.5 policy), but it must be done **after** the WAL is checkpointed and before
  the meta file records `property_page_count`, or a crash between the two
  leaves the meta claiming pages that no longer exist.

## 5. Non-goals

- Changing the entry encoding (§5 `PropertyEntry`). It is orthogonal and every
  measured byte here is page slack, not entry overhead.
- Reclaiming tombstones beyond the existing checkpoint compaction.
- Edge property pages: only 4 590 pages on PO. They follow the same mechanism
  for free, but they are not the target.

## 6. Impact

| area | change |
|---|---|
| `format-spec.md` | §5 gains the class table and the two magics; §1 file layout gains `substrate.props.small.v3` |
| `format_version` | bump; migration in `crates/obrain-substrate/src/migration.rs::table()` rewrites `props.v2` into the two classes, offline, under the exclusive lock |
| WAL | new page-allocation records per class; replay must be idempotent per class |
| readers | `PropsZone::walk_chain` follows the class of the head; `read_heap_view` unchanged |
| indexes | `props.idx.v2` / `eq.idx.v2` / `ord.idx.v2` store page ids — they must carry the class (one bit), or the class must be derivable from the id by a per-class range |
| open time | unchanged: lazy mmap, one more file |
| downgrade | a binary that does not know the class must refuse to open (magic check), never read a 256-byte page as a 4 KiB one |

## 7. Erratum found while measuring

`format-spec.md` §5 describes `PropertyPage` as `crc32` @8 … `next_page` @12?
It currently lists `free_offset` @16 and `crc32` @20. The **code**
(`crates/obrain-substrate/src/page.rs`, `PropertyPageHeader`) is
`magic` @0, `node_id` @4, `crc32` @8, `next_page` @12, `entry_count` @18,
`free_offset` @20, `tombstones` @22. The measurement walks `next_page` at
offset 12 and re-walks 20 000 chains with a 99.94 % agreement, which pins the
code layout and makes the document stale. §5 also claims "at most 4 pages in
practice", while the same measurement found chains of up to 9 pages (2 004
nodes above one page). Both lines are corrected in the same change as this RFC's
implementation, not before, so the correction is reviewable next to the code.

## 8. Acceptance metrics

Measured with the same tool, on the same base, before and after:

| metric | today | target |
|---|---|---|
| `props.v2` + `props.small.v3` allocated | 7.44 GiB | ≤ 1.0 GiB |
| node pages above one page | 2 004 (0.10 %) | ≤ 2 004 (no regression) |
| open time, read-only | 2.1 ms (PR #270) | unchanged |
| `props` read latency | baseline from `banc_lecture_seule` | within noise |
| histogram agreement in the re-walk check | 99.94 % | ≥ 99.9 % |

## 9. Open questions

1. **Small page size**: 256 B is chosen from the measured `free_offset`
   distribution. 128 B would fit 0 % more pages on PO but would overflow on the
   first `id`+`path` pair of a `File` node; 512 B would waste half of each page.
   Is there a workload where 256 B is wrong?
2. **Promotion policy**: promote on first overflow only (proposed), or promote
   a node with `> N` properties pre-emptively to avoid the copy? Measured
   overflow is rare enough that pre-emption seems unjustified.
3. **Free list persistence**: in the file header, or rebuilt at open by scanning
   for zeroed pages (14 s on a 9 GiB zone — too slow at open, so persistent).
4. **Proposal C ordering**: is truncation before or after the meta update? The
   RFC proposes after the WAL checkpoint and before the meta write, with the
   meta write recording the truncated page count.
