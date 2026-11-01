---
title: Performance
description: Where the measured figures live, and how to keep an Obrain workload fast.
tags:
  - performance
  - benchmarks
  - optimization
---

# Performance

**Every figure has a command, a machine and a load, or it is not a figure.**
The measured numbers of this release live in one place:
[`docs/BENCHMARKS.md`](../BENCHMARKS.md) — the four reference benches
(`make bench-v001`), the hardware they ran on, the load at the time, and what
the README is allowed to claim because of them.

The previous version of this page carried tables of throughputs and latencies
(« ~2M ops/s », Ryzen 9 5900X, LDBC comparisons, Python examples) that no
bench of this repository produces and no Python binding of this release can
run. They were removed rather than kept as decoration.

## Reproducing the figures

```bash
make bench-v001                      # the four reference benches (Criterion, release profile)
scripts/bench-v001.sh save           # record a baseline named after the current commit
scripts/bench-v001.sh compare <sha>  # compare the current tree to that baseline (threshold 10 %)
```

`mesures.yml` (manual workflow) runs the same benches on a shared runner for
orientation only: a shared runner does not measure, so it carries no threshold
and is not a merge gate.

## Keeping a workload fast

These rules follow from what the benches show, not from a target.

- **Group writes in an explicit transaction.** On a database opened from a
  path, every auto-committed statement is one `fsync` (measured: ≈ 225
  durable commits/s on an Apple internal SSD, see the D-DUR section of
  `BENCHMARKS.md`). `BEGIN … COMMIT` pays that `fsync` once per batch. For
  imports, `open_substrate_with_mode(SyncMode::Group)` plus `flush_wal()` per
  lot is the same trade-off as SQLite's `PRAGMA synchronous=NORMAL`.
- **Index the properties you look up by.** `CREATE INDEX` on a property turns
  a label scan into a hash lookup; the `query_node_lookup_by_property` bench
  is the cost of the unindexed path.
- **Bound exploratory queries.** Variable-length paths (`-[:KNOWS*1..5]->`)
  and two-hop patterns fan out; add `LIMIT` while exploring.
- **Prefer the tiered vector scan for retrieval.** `tiered_scan` is the
  measured path for top-k over 10⁶ nodes; the p95 and the recall it reaches
  are not measured yet and are therefore not claimed.

If a figure you need is missing here, it is missing from the benches too:
add the bench first, then the number.
