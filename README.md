[![CI](https://github.com/obrain-db/obrain/actions/workflows/ci.yml/badge.svg)](https://github.com/obrain-db/obrain/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![MSRV](https://img.shields.io/badge/MSRV-1.91.1-blue)](Cargo.toml)
[![Coverage](https://img.shields.io/endpoint?url=https%3A%2F%2Fobrain-db.github.io%2Fobrain%2Fcouverture.json)](https://obrain-db.github.io/obrain/)

**English** · [Français](README.fr.md)

# Obrain — an embedded graph database in Rust

Obrain is an **embedded** graph engine (a library, not a server): "topology-as-storage" on memory-mapped files, a native write-ahead log (WAL), transactions, the GQL language, graph algorithms exposed as `CALL` procedures, and vectors as a first-class type.

This README describes **what v0.0.1 does and proves**. Every code example below is run as-is by `crates/obrain/tests/readme_examples.rs`; every guarantee points to the test that locks it; every figure points to [docs/BENCHMARKS.md](docs/BENCHMARKS.md). What is not verified is in the [Roadmap](#roadmap), not here. Known limits are in [docs/LIMITES-v0.0.1.md](docs/LIMITES-v0.0.1.md).

## Quick start (Rust)

```bash
cargo add obrain
```

```rust
use obrain::ObrainDB;

fn main() {
    let db = ObrainDB::new_in_memory();
    let session = db.session();

    // Build a graph
    session.execute("INSERT (:Person {name: 'Alix', age: 30})").unwrap();
    session.execute("INSERT (:Person {name: 'Gus', age: 25})").unwrap();
    session.execute(
        "MATCH (a:Person {name: 'Alix'}), (b:Person {name: 'Gus'})
         INSERT (a)-[:KNOWS {since: 2020}]->(b)"
    ).unwrap();

    // Query it
    let result = session.execute(
        "MATCH (p:Person)-[:KNOWS]->(friend)
         RETURN p.name, friend.name"
    ).unwrap();
    for row in result.iter() {
        println!("{} knows {}", row[0], row[1]);
    }

    // Run graph algorithms
    let pr = session.execute("CALL obrain.pagerank()").unwrap();
    for row in pr.iter() {
        println!("Node {} — PageRank {:.4}", row[0], row[1]);
    }
}
```

Persistent database: `ObrainDB::open("path/to/db")` opens or creates a directory of `substrate.*` files; `db.close()` closes it cleanly. **`open` requires the `storage` feature** (`cargo add obrain --features storage`): the default `embedded` profile is in-memory only. (Test: `stockage_persistant_rouvrir_et_relire`, under `--features wal`.)

## Guarantees

| Guarantee | What it means | Test that locks it |
|---|---|---|
| **An acknowledged commit is durable** | When `commit()` (explicit, or implicit for each statement outside a transaction) returns `Ok`, the writes are in the WAL **and** acknowledged by the disk (`fsync` — `F_FULLFSYNC` on macOS). Three tests, three distinct coverages: **process crash (`kill -9`)** — every acknowledged commit is there after reopening (this test alone also passes without `fsync`: the page cache survives the process, measured); **`fsync` oracle** — after each acknowledged `commit()`, the durable WAL line (`wal_octets_durables`, the offset covered by a completed `fsync`) covers the commit's frame (red without `fsync`); **simulated cut (WAL truncated at the durable line)** — everything not `fsync`ed is removed from the WAL, exactly the acknowledged commits come back. A real power cut is not tested: the truncation only replays the cut for the WAL, not for the mmap zones. | `obrain-engine/tests/durabilite_commit_kill9.rs` (`un_commit_acquitte_survit_a_un_kill_9`, oracle in each child, `une_coupure_simulee_ne_garde_que_les_commits_durables`), `obrain-substrate/tests/wal_fsync_partage.rs` |
| **An uncommitted transaction leaves nothing** | A process that disappears with an open transaction publishes nothing on reopening: the WAL carries transaction boundaries, and replay discards what was not sealed. | `durabilite_commit_kill9.rs` (test 2) |
| **A refused commit leaves nothing** | A validation conflict (two sessions, same entity, the second committed after the first started) is refused, and nothing of the refused transaction remains — neither in memory nor on disk after reopening. | `obrain-engine/tests/echec_commit_rembobinage.rs` |
| **`ROLLBACK` only removes its own creations** | Two concurrent sessions: rolling back one never touches the other's commits. | `obrain-engine/tests/rollback_isolation.rs` |
| **Snapshot isolation** | An open transaction does not see a neighbouring commit made after its start. | `readme_examples.rs::mvcc_snapshot_isolation_…` |
| **Crash recovery** | A truncated or torn WAL is replayed up to the last valid boundary, never beyond; a run of zeros followed by data is reported by name, not mistaken for an end of file. | `obrain-substrate/tests/crash_safety.rs`, `wal_recovery_reelle.rs` |
| **On-disk format frozen (v3)** | A database written by v0.0.1 reopens with any v0.0.x; a database from a newer version is refused, never read as empty. | `obrain-engine/tests/format_fige_v3.rs`, fixture `tests/golden/v3-mini` |
| **Unknown WAL opcode ⇒ named refusal, never read as empty** | A `substrate.wal` holding a complete frame (valid CRC) of an opcode this binary does not know was written by a newer version: opening is refused with an error naming the opcode and the offset; the WAL is neither partly replayed, nor truncated, nor modified. A torn tail (incomplete frame) remains a crash boundary. | `obrain-substrate/tests/wal_opcode_inconnu.rs`, `obrain-engine/tests/format_fige_v3.rs` (tests `…opcode…`), fixture `tests/golden/v3-mini-opcode-futur` |
| **Read-only mode does not write (`open_read_only`)** | Opening a database read-only, querying it and closing it modifies **no byte of any file**, creates none, deletes none — whether the database was closed cleanly, left by a writer killed before its flush, or while a shared writer is alive. This is not a coding discipline: `substrate.meta`, the lock, the zones and the WAL are opened **without write rights**, and meta and zones are mapped privately (copy-on-write). The pending WAL is replayed **in memory**; degree columns with a stale CRC are recomputed in memory; a named graph also opens read-only. Any mutation is refused: `Transaction(ReadOnly)` for a query, `LectureSeule` for a substrate operation (flush, checkpoint, repair, WAL append), a panic carrying that message for infallible direct APIs (`create_node` returns an identifier, not a `Result`). A directory that is not a database **fails without a database being created in it**. Some cases are **refused** rather than served by writing (`LectureSeuleRefusee`, which says what to run): a database in an older format to migrate, an inherited sidecar holding vectors to migrate, a database without `substrate.dirlock`. **Lock**: shared — it excludes the exclusive writer (`open`) from another process, **not** a shared writer nor other readers; against a live shared writer, the reader sees the state at its opening, with no isolation beyond that (see LIMITS). One exception, at the operator's request: `OBRAIN_TRIGRAMMES_PERSISTER=1` publishes the trigram cache (separate files, atomic `rename`). | `obrain-substrate/tests/lecture_seule_contrat.rs` (fingerprint of all files before/after, one test per state; live writer in another process; database with write rights removed), `obrain-engine/tests/lecture_seule_contrat.rs` |
| **A single writer per database (`open`)** | The directory is locked (fcntl); a second process is refused, and the refusal names the holder (Linux, macOS). | `obrain-substrate/tests/verrou_apres_drop.rs` |
| **Restricted shared writing (`open_shared_write`)** | N processes create **nodes and edges without properties**: no loss, no slot collision, and each sees the others' writes on **reopening**. An inherited database (non-empty sidecar) is refused, named, without modification. Not guaranteed: properties written by several processes, concurrent transactions across processes, visibility without reopening (see LIMITS). | `obrain-engine/tests/ecriture_multi_processus.rs`, `obrain-substrate/tests/wal_chaine_cas.rs`, `obrain-substrate/tests/mw_interprocess_append.rs`, `obrain-substrate/tests/base_heritee_shared_write.rs` |

What v0.0.1 **does not guarantee**: multi-process writing beyond the restricted scope above (shared properties, concurrent transactions across processes, visibility without reopening), and everything listed in [docs/LIMITES-v0.0.1.md](docs/LIMITES-v0.0.1.md).

## Features (default features of the `obrain` crate)

- **Model**: labelled property graph (LPG). RDF and the other languages (Cypher, SPARQL, Gremlin, GraphQL, SQL/PGQ) exist behind features that are not enabled by default and are not covered by this README.
- **Language**: GQL (ISO/IEC 39075) — `INSERT`, `MATCH`, `SET`, `DELETE`, `RETURN`, aggregates, `CALL … YIELD`.
- **Transactions**: `begin_transaction` / `commit` / `rollback`, savepoints, auto-commit per statement, write-write conflict detection, snapshot isolation.
- **Storage**: in memory (`new_in_memory`, default) or persistent (`open`, feature `storage`), `substrate.*` format v3, WAL + checkpoint.
- **Vectors**: `Value::Vector` stored with the graph; retrieval in tiers L0/L1/L2 (128-bit binary → 512-bit Hamming → f16 cosine); BM25; hybrid search (feature `hybrid-search`, included in `embedded`) in Rust (`ObrainDB::hybrid_search`) and in GQL (`CALL obrain.hybrid_search`, see below).
- **Algorithms** (`CALL obrain.<name>(…)`), each run by the harness on a small graph:

| Family | Procedures |
|---|---|
| Centrality | `pagerank`, `personalized_pagerank({seed_nodes})`, `betweenness_centrality`, `closeness_centrality`, `degree_centrality`, `hits` |
| Communities | `louvain`, `leiden`, `label_propagation`, `stable_communities` |
| Shortest paths | `dijkstra(source)`, `astar(source, target)` (null heuristic without coordinates: exact, equal to Dijkstra), `bellman_ford(source)`, `sssp({source: 'name'})` (source designated by its `name` property), `floyd_warshall` |
| Traversal and structure | `bfs(source)`, `dfs(source)`, `connected_components`, `strongly_connected_components`, `topological_sort`, `bridges`, `articulation_points`, `kcore` (alias `k_core`), `subgraph.khop(center, k)`, `irreducible_kernel`, `subgraph_contraction({node_ids})` |
| Aggregation and similarity | `clustering_coefficient`, `triangle_count`, `similarity(a, b, metric)` and `similarity.topk(a, k, metric)` with `metric` ∈ `jaccard` (default), `overlap`, `cosine`, `adamic_adar`; shortcuts `jaccard(a, b)`, `cosine_similarity(a, b)`, `adamic_adar(a, b)` |
| Flow and spanning trees | `max_flow({source, sink})`, `min_cost_max_flow({source, sink})` (node identifiers), `kruskal`, `prim` |
| Spectral geometry | `spectral_embedding`, `heat_kernel_diffusion({seed_nodes})`, `hilbert_features` |

Syntax:

```sql
CALL obrain.pagerank()
CALL obrain.pagerank({damping: 0.85, max_iterations: 20})
CALL obrain.louvain() YIELD node_id, community_id
CALL obrain.pagerank() YIELD node_id AS id, score AS rank
```

### Hybrid search (feature `hybrid-search`)

`CALL obrain.hybrid_search` fuses BM25 (text index) and cosine (vector index) — by default with Reciprocal Rank Fusion. It needs a text index on `:label(text_property)` (`create_text_index`) and, for the vector part, a vector index on `:label(vector_property)` (`create_vector_index`); without an index, the error names the ones it expects. The query vector is a list of numbers or a parameter (`$vec`); without a vector, only BM25 contributes. RRF scores are those of the fusion (1-based rank, `rrf_k` = 60); `fusion: 'weighted'` combines the normalised scores with `weights`.

```sql
CALL obrain.hybrid_search('Doc', 'title', 'emb', 'Rust graph', [1.0, 0.0, 0.0], {k: 3}) YIELD node_id, score
CALL obrain.hybrid_search('Doc', 'title', 'emb', 'Rust graph') YIELD node_id, score
CALL obrain.hybrid_search('Doc', 'title', 'emb', 'Rust graph', [1.0, 0.0, 0.0], {k: 2}) YIELD node_id MATCH (n) WHERE id(n) = node_id RETURN n.title
CALL obrain.hybrid_search('Doc', 'title', 'emb', 'Rust graph', [1.0, 0.0, 0.0], {k: 3, fusion: 'weighted', weights: [0.5, 0.5]}) YIELD node_id, score
```

Options (a map as the last argument): `k` (default 10), `fusion` (`'rrf'` default, `'weighted'`), `rrf_k` (default 60), `weights`. Columns: `node_id`, `score`. The chaining `CALL … YIELD … MATCH … RETURN` is run by `readme_examples.rs` (module `recherche_hybride`).

## Cognitive layer (feature `cognitive`)

Obrain carries a cognitive layer — node energy with a half-life, Hebbian synapses, scars, a knowledge fabric (`risk_score`), propagated activation. From the `obrain` crate:

```bash
cargo add obrain --features cognitive       # engine: energy, synapses, UDFs, CALL obrain.cognitive.spread / distill
cargo add obrain --features cognitive-full  # + detect_gaps, synthesize, engrams, decision
```

**Opt-in.** Opening a database (`ObrainDB::open_substrate`, `new_in_memory`) starts **no** cognitive task: energy and synapses stay at zero, as without the feature. To have every mutation feed the layer, open the database in observed mode:

```rust
let db = obrain::ObrainDB::open_substrate_observed("./my_db")?; // or new_in_memory_observed()
```

Observed mode starts a dedicated tokio runtime (1 worker) if none is current, and a flush task; co-activation goes to `edges`. Warning: in observed mode, the layer's writes to the log (WAL) are asynchronous and not synchronised: do not use it for data that must survive a crash before the corresponding durability rule.

```sql
CALL obrain.cognitive.spread([0], {hops: 2})                 -- activation propagated from a node
CALL obrain.cognitive.distill({min_weight: 0.1})             -- strong synaptic connections
MATCH (n:Person) RETURN n.name, obrain.energy(id(n)) AS energy
MATCH (n) RETURN obrain.risk(id(n)) AS risk ORDER BY risk DESC LIMIT 10
MATCH (n:Person {name: 'Alix'}) RETURN obrain.synapses(id(n)) AS connections
```

These calls are run by `readme_examples.rs` under `--features cognitive` (module `cognitif`); that test checks that they return no error, not their values. Model documentation: `crates/obrain-cognitive`.

## Performance

The reproducible benchmarks, the exact command, the hardware and the figures are in [docs/BENCHMARKS.md](docs/BENCHMARKS.md). No figure is repeated here.

## Installation

```bash
cargo add obrain                                           # embedded profile: GQL + vectors/text/hybrid + algorithms + parallelism (in memory)
cargo add obrain --features storage                        # + persistent databases (ObrainDB::open, WAL, checkpoint)
cargo add obrain --no-default-features --features gql      # minimal
cargo add obrain --no-default-features --features full     # all languages + storage + RDF + CDC
```

MSRV: 1.91.1 (the workspace `rust-version`, checked in CI).

## Examples

`examples/rust/` (`cargo run -p obrain-examples --bin <name>`): `basic`, `transactions`, `parameterized`, `persistence`, `vector_search`, `algorithms`, `multi_language`, `social_network`, `knowledge_base`, `iot_stream`, `migrate-v2`, `bench-load` (the last two binary names contain a hyphen).

## Bindings

The binding code is in `crates/bindings/`: C (`c`), C# (`csharp`), Dart (`dart`), Go (`go`), Node.js (`node`), Python (`python`), with the LangChain (`python/integrations/langchain_obrain`) and LlamaIndex (`python/integrations/llama_index_obrain`) integrations. Each is built and tested by its own CI job ("Bindings C", "Bindings C#", "Bindings Dart", "Bindings Go", "Bindings Node", "Bindings Python", "Bindings LangChain/LlamaIndex"). None is published to its registry: the release chain (`.github/workflows/release.yml`) only publishes the Rust crates to crates.io. The `packages/obrain-cli-{npm,python}` packages are wrappers around the CLI, not bindings.

## Roadmap

What is **not** in v0.0.1:

- Publishing the bindings to their registries (PyPI, npm, NuGet, pub.dev, Go) and the WebAssembly package `crates/obrain-web` (trial package, `publish = false`, see its README).
- HTTP server, Bolt protocol, playground, MCP server: not implemented in this repository.
- LDBC comparison (`graph-bench`) re-run with a pinned version.

## Documentation

- [docs/LIMITES-v0.0.1.md](docs/LIMITES-v0.0.1.md) — known limits, each with its test.
- [docs/BENCHMARKS.md](docs/BENCHMARKS.md) — method and figures.
- [docs/rfc/substrate/format-spec.md](docs/rfc/substrate/format-spec.md) — on-disk format (§ 0: real state of v0.0.1).
- [docs/MIGRATION.md](docs/MIGRATION.md), [docs/architecture/overview.md](docs/architecture/overview.md).
- [CHANGELOG.md](CHANGELOG.md).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). The CI (`.github/workflows/ci.yml`) runs: format, clippy and rustdoc with no warnings (`-D warnings`), tests, code coverage, `--all-features` compilation (`-D warnings`), MSRV, `cargo deny`, checks of the publication manifests and `cargo package` of the first crate in the publication order, the benchmarks on fabricated databases, the cross-process shared WAL, the seven binding jobs, the browser jobs (wasm32, `obrain-web`, OPFS persistence under headless Chrome), and the integrity tests on macOS (job conditional on the repository variable `CI_MACOS`, otherwise "skipped").

**Code coverage.** The badge at the top of this README and the [coverage page](https://obrain-db.github.io/obrain/) (total and per-crate detail) are published by the CI on every push to `main`: no figure is copied by hand. The project target is 95 % of lines. The "Coverage" job enforces two checks, whose thresholds and method are commented in `ci.yml`: the new code of a PR must be 95 % covered, and a global floor (ratchet) that never decreases.

## Acknowledgements

Obrain is a fork of [GrafeoDB](https://github.com/GrafeoDB/grafeo) (Apache-2.0) — see [NOTICE](NOTICE). The execution engine draws on [DuckDB](https://duckdb.org/) (vectorised push-based execution, morsel-driven parallelism) and [Kuzu](https://github.com/kuzudb/kuzu) (CSR adjacency, factorised processing).

## License

Apache-2.0 — see [LICENSE](LICENSE) and [NOTICE](NOTICE).
