//! Storage backends - how your data gets persisted.
//!
//! | Backend | Speed | Durability | Use when |
//! | ------- | ----- | ---------- | -------- |
//! | `wal` (feature-gated) | Fast | Survives crashes | Production workloads |
//!
//! The WAL (Write-Ahead Log) writes changes to disk before applying them,
//! so you can recover after crashes without losing committed transactions.
//! The WAL module requires filesystem I/O and is gated behind the `wal` feature.
//!
//! > **Note (T17 Wave A)**: the single-file `.obrain` format (`file` module,
//! > former `obrain-file` feature) was retired with the substrate cutover;
//! > its sources remain on disk only pending final deletion.
//!
//! > **Note (T17 W4.p4)**: the historical in-memory `MemoryBackend` (a thin
//! > `Arc<LpgStore>` wrapper) was removed as part of the substrate cutover.
//! > For in-memory / test usage, construct a `SubstrateStore` directly via
//! > `SubstrateStore::open_tempfile()` from the `obrain-substrate` crate.

#[cfg(feature = "wal")]
pub mod wal;

#[cfg(feature = "wal")]
pub use wal::WalManager;
