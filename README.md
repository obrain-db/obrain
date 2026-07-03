# Obrain

**A fast, embedded graph database engine in Rust** — memory-mapped storage that
*is* the on-disk format, crash-safe WAL, Cypher & GQL query languages, and
native vector + full-text retrieval. Open a multi-million-node graph in
milliseconds; run it in-process, as a CLI, or through language bindings.

[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

## Highlights

- **Instant open** — the `.obrain` format is directly memory-mappable: no
  deserialization, no rebuild. Measured cold opens: **~5 ms** on a 1.35M-node /
  1.9M-edge graph, **~17 ms** on an 8.1M-node corpus.
- **Crash-safe by construction** — write-ahead log with atomic dual-header
  checkpoints; torn-write and SIGKILL recovery covered by an adversarial test
  suite (50-seed torn-WAL replay, idempotent re-application).
- **O(1) analytics primitives** — live label/edge-type counters and per-node
  typed-degree columns: `count(n)` in microseconds, top-K connected nodes
  without a scan.
- **Query engines** — openCypher and ISO GQL front-ends over a shared
  vectorized executor (chunked columns, LIMIT/label pushdowns).
- **Retrieval built in** — HNSW vector index, tiered quantized cascade
  (1-bit → PQ → f32, p95 ≤ 1 ms on 10⁶ nodes), BM25 full text.
- **Bindings** — Python, Node.js, WASM, Go, C, C#; plus a standalone CLI
  (`obrain query/info/stats/backup/compact`).
- **Streaming importer** — `neo4j2obrain` migrates a live Neo4j (bolt) into an
  obrain substrate.

## Quickstart

```bash
# CLI
cargo install --path crates/obrain-cli
obrain init mygraph && obrain query mygraph "INSERT (:Person {name: 'Ada'})"
obrain query mygraph "MATCH (p:Person) RETURN p.name"
```

```rust
// Embedded
use obrain_engine::ObrainDB;

let db = ObrainDB::open("./mygraph")?;
let session = db.session();
session.execute("INSERT (:Person {name: 'Ada'})")?;
let rows = session.execute("MATCH (p:Person) RETURN p.name")?;
```

## Architecture

```
 crates/
 ├── obrain-common      shared types (NodeId, Value, PropertyMap…)
 ├── obrain-core        graph traits, vectorized execution operators
 ├── obrain-substrate   mmap-native storage: records, zones, WAL, tiers
 ├── obrain-engine      ObrainDB, sessions, planners, Cypher/GQL translators
 ├── obrain-adapters    parsers, plugins, graph algorithms (Leiden, PageRank…)
 ├── obrain-reactive    post-commit mutation event bus
 ├── obrain-iam         identity & access (WAMI integration)
 ├── obrain-cli         command-line interface
 └── bindings/          python · node · wasm · go · c · csharp
```

The engine also carries the storage primitives (weighted edges, energy
columns, activation counters) that power **obrain-cognitive** — a separate,
closed layer that adds adaptive memory, learned retrieval and an LLM-facing
runtime on top of this engine.

## License & lineage

Apache-2.0. Obrain began as a fork of
[GrafeoDB/grafeo](https://github.com/GrafeoDB/grafeo) and has since been
substantially rewritten (mmap-native substrate, WAL, tiered retrieval, GQL
front-end); original copyright notices are preserved.
