---
title: Crate Structure
description: The crates that make up Obrain.
tags:
  - architecture
  - crates
---

# Crate Structure

Obrain is organized into core library crates and language binding crates.

## Dependency Graph

Relu dans les manifestes (`crates/*/Cargo.toml`, 2026-11-01) :

```mermaid
graph TD
    COMMON[obrain-common]
    CORE[obrain-core] --> COMMON
    SUBSTRATE[obrain-substrate] --> COMMON
    SUBSTRATE --> CORE
    REACTIVE[obrain-reactive] --> COMMON
    REACTIVE --> CORE
    ADAPTERS[obrain-adapters] --> COMMON
    ADAPTERS --> CORE
    ADAPTERS --> SUBSTRATE
    COGNITIVE[obrain-cognitive] --> COMMON
    COGNITIVE --> CORE
    COGNITIVE --> REACTIVE
    COGNITIVE -.->|optionnel| ADAPTERS
    COGNITIVE -.->|optionnel| SUBSTRATE
    ENGINE[obrain-engine] --> COMMON
    ENGINE --> CORE
    ENGINE --> ADAPTERS
    ENGINE --> SUBSTRATE
    ENGINE --> REACTIVE
    ENGINE --> COGNITIVE
    OBRAIN[obrain] --> ENGINE
    OBRAIN --> CORE
    OBRAIN --> COMMON
    OBRAIN --> ADAPTERS
```

Ordre de publication (chaque crate après ses dépendances) : common → core → substrate → reactive → adapters → cognitive → engine → obrain.

## obrain

Top-level facade crate that re-exports the public API.

| Module | Purpose |
|--------|---------|
| `lib.rs` | Re-exports from obrain-engine |

```rust
use obrain::ObrainDB;

let db = ObrainDB::new_in_memory();
```

## obrain-common

Foundation types and utilities.

| Module | Purpose |
|--------|---------|
| `types/` | NodeId, EdgeId, Value, LogicalType |
| `memory/` | Arena allocator, memory pools |
| `utils/` | Hashing, error types |

```rust
use obrain_common::types::{NodeId, Value};
use obrain_common::memory::Arena;
```

## obrain-core

Core data structures, graph-store traits and execution engine. Since the
T17 substrate cutover, `obrain-core` owns the abstract traits
(`GraphStore`, `GraphStoreMut`) and the query-execution pipeline. The
concrete on-disk backend lives in [obrain-substrate](#obrain-substrate).

| Module | Purpose |
|--------|---------|
| `graph/traits.rs` | `GraphStore` / `GraphStoreMut` traits (canonical API) |
| `graph/lpg/` | Legacy LPG in-memory backend (scheduled for removal in T17 W4) |
| `index/` | Hash, B-tree, adjacency indexes |
| `execution/` | DataChunk, operators, pipelines |

```rust
use obrain_substrate::SubstrateStore;
use obrain_core::graph::traits::GraphStoreMut;
use obrain_core::index::HashIndex;
use obrain_core::execution::DataChunk;

let store = SubstrateStore::open_tempfile().unwrap();
```

## obrain-substrate

Canonical on-disk backend for the obrain graph database — a single mmap'd
file that *is* the topology (records `#[repr(C)]` bytemuck::Pod, inline
index-free adjacency, WAL-native). Implements `GraphStore` /
`GraphStoreMut` as a drop-in replacement for the legacy `LpgStore`.

| Module | Purpose |
|--------|---------|
| `store.rs` | `SubstrateStore` — open / mmap / WAL entry point |
| `record.rs` | Fixed-size `NodeRecord` (32 B) / `EdgeRecord` (36 B post-T17f) |
| `props_zone.rs` | 4 KiB `PropertyPage` mmap-backed property chains |
| `wal/` | Crash-safe WAL (NodeInsert / EdgeInsert / PropSet / Delete / Checkpoint) |
| `tiers/` | Retrieval tiers L0 (128-bit) / L1 (512-bit) / L2 (f16 384-dim) |

```rust
use obrain_substrate::SubstrateStore;

let store = SubstrateStore::open("/path/to/db.obrain").unwrap();
```

## obrain-adapters

External interfaces and adapters.

| Module | Purpose |
|--------|---------|
| `query/gql/` | GQL parser (lexer, parser, AST) |
| `query/cypher/` | Cypher compatibility layer |
| `storage/` | Storage backends (memory, WAL) |
| `plugins/` | Plugin system |

```rust
use obrain_adapters::query::gql::Parser;
use obrain_adapters::storage::wal::WalManager;
```

## obrain-engine

Database facade and coordination.

| Module | Purpose |
|--------|---------|
| `database.rs` | ObrainDB struct, lifecycle |
| `session.rs` | Session management |
| `query/` | Query processor, planner, optimizer |
| `transaction/` | Transaction manager, MVCC |

```rust
use obrain_engine::{ObrainDB, Session, Config};
```

## Ce qui n'est pas dans ce dépôt

Les bindings (Python, Node.js, C, C#, Dart, WebAssembly) et le CLI ne font pas
partie de ce dépôt en v0.0.1 : le CLI vit dans `obrain-db/obrain-cli`, et aucun
binding n'est publié. La couche cognitive est aussi extraite, à l'identique,
dans `obrain-db/obrain-cognitive`.

## Crate Guidelines

1. **No cyclic dependencies** - Strict layering
2. **Public API minimization** - Only expose what's needed
3. **Feature flags** - Optional functionality gated by features
4. **Documentation** - All public items documented
