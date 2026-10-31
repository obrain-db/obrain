//! # `probe_prop_shape` — exact shape of ONE property key in `substrate.props`
//!
//! `audit_props` answers "which keys are heavy?" but sizes unknown `Value`
//! variants through a `_ => 8` catch-all, so a `List` of 1024 `Float64`
//! reports as 12 bytes. That is precisely the case that matters when
//! diagnosing "why didn't my vectors get routed into `vec_columns`?":
//! `route_vector_write` only fires for `Value::Vector`, so embeddings
//! persisted as `Value::List(Float64)` stay in the legacy sidecar and
//! inflate open-time anon-RSS.
//!
//! This binary answers, for a single key:
//!   - which `Value` variant is actually stored,
//!   - the *distribution of lengths* (a heterogeneous `dim` is fatal:
//!     `VecColumnRegistry::write` rejects a second spec for the same
//!     `(key, EntityKind)` and `route_vector_write` swallows the error
//!     into a `tracing::error!`, leaving the slot silently zeroed),
//!   - the element types inside the container,
//!   - how many entries are all-zero (the registry's "absent" sentinel).
//!
//! Read-only and streaming, same as `audit_props`: one `PropEntry` is
//! decoded at a time through a `BufReader` and dropped, so peak anon
//! stays under ~10 MB regardless of sidecar size. **It never opens a
//! `SubstrateStore`**, which matters because the store's open path is
//! read-write — auditing through it would mutate what you are auditing.
//!
//! ## Usage
//! ```text
//! cargo run --release -p obrain-substrate --example probe_prop_shape -- \
//!     /home/me/.obrain/db/agent/system embedding
//! ```

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::PathBuf;

use bincode::config::{Configuration, Fixint, LittleEndian};
use obrain_common::types::Value;
use obrain_substrate::PROPS_FILENAME;
use serde::{Deserialize, Serialize};

type BoxErr = Box<dyn std::error::Error + Send + Sync>;

const MAGIC: u32 = 0x5052_4F50;
const FORMAT_VERSION: u32 = 1;

/// Mirror of `PropEntry` — decoded one at a time then dropped.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PropEntry {
    id: u64,
    props: Vec<(String, Value)>,
}

fn bincode_config() -> Configuration<LittleEndian, Fixint> {
    bincode::config::standard()
        .with_little_endian()
        .with_fixed_int_encoding()
}

/// Name of the `Value` variant, so the report says `List` vs `Vector`
/// rather than guessing from a byte count.
fn variant_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "Null",
        Value::Bool(_) => "Bool",
        Value::Int64(_) => "Int64",
        Value::Float64(_) => "Float64",
        Value::String(_) => "String",
        Value::Bytes(_) => "Bytes",
        Value::List(_) => "List",
        Value::Vector(_) => "Vector",
        _ => "other",
    }
}

#[derive(Default)]
struct Report {
    /// variant name -> occurrences
    variants: BTreeMap<&'static str, u64>,
    /// container length -> occurrences (the dim histogram)
    lengths: BTreeMap<usize, u64>,
    /// element variant inside a List -> occurrences
    elem_variants: BTreeMap<&'static str, u64>,
    /// entries whose numeric payload is entirely zero
    all_zero: u64,
    /// entries with at least one non-finite element (NaN / inf)
    non_finite: u64,
    total: u64,
    /// first observed payload, for eyeballing real values
    sample: Option<String>,
}

impl Report {
    fn observe(&mut self, v: &Value) {
        self.total += 1;
        *self.variants.entry(variant_name(v)).or_default() += 1;

        // Collect the numeric payload, whichever container holds it.
        let nums: Vec<f64> = match v {
            Value::List(items) => {
                *self.lengths.entry(items.len()).or_default() += 1;
                let mut out = Vec::with_capacity(items.len());
                for it in items.iter() {
                    *self.elem_variants.entry(variant_name(it)).or_default() += 1;
                    match it {
                        Value::Float64(f) => out.push(*f),
                        Value::Int64(i) => out.push(*i as f64),
                        // Non-numeric element: this is not a vector at all.
                        _ => {}
                    }
                }
                out
            }
            Value::Vector(vec) => {
                *self.lengths.entry(vec.len()).or_default() += 1;
                *self.elem_variants.entry("f32").or_default() += vec.len() as u64;
                vec.iter().map(|f| *f as f64).collect()
            }
            // Scalar or unexpected variant — nothing dimensional to report.
            _ => Vec::new(),
        };

        if !nums.is_empty() {
            if nums.iter().all(|f| *f == 0.0) {
                self.all_zero += 1;
            }
            if nums.iter().any(|f| !f.is_finite()) {
                self.non_finite += 1;
            }
            if self.sample.is_none() {
                let head: Vec<String> = nums.iter().take(5).map(|f| format!("{f:.6}")).collect();
                self.sample = Some(format!("[{}, ...]", head.join(", ")));
            }
        }
    }

    fn print(&self, key: &str, scope: &str) {
        println!();
        println!("=== {scope} key `{key}` ===");
        if self.total == 0 {
            println!("(absent)");
            return;
        }
        println!("occurrences           : {}", self.total);

        println!("variants              :");
        for (name, n) in &self.variants {
            println!("  {name:<10} {n}");
        }

        println!("lengths (dim)         : {} distinct", self.lengths.len());
        for (len, n) in &self.lengths {
            println!("  dim={len:<6} {n}");
        }
        // The single most actionable line in the whole report.
        if self.lengths.len() > 1 {
            println!("  !! HETEROGENEOUS — VecColumnRegistry::write would reject all");
            println!("     but the first spec; those slots stay silently zeroed.");
        }

        if !self.elem_variants.is_empty() {
            println!("element variants      :");
            for (name, n) in &self.elem_variants {
                println!("  {name:<10} {n}");
            }
        }

        println!("all-zero payloads     : {}", self.all_zero);
        println!("non-finite payloads   : {}", self.non_finite);
        if let Some(ref s) = self.sample {
            println!("first payload         : {s}");
        }
    }
}

fn resolve_props_path(base: &std::path::Path) -> PathBuf {
    let nested = base.join("substrate.obrain").join(PROPS_FILENAME);
    if nested.exists() {
        return nested;
    }
    base.join(PROPS_FILENAME)
}

fn main() -> Result<(), BoxErr> {
    let mut args = std::env::args().skip(1);
    let base: PathBuf = args
        .next()
        .ok_or_else(|| -> BoxErr { "usage: probe_prop_shape <substrate-dir> <key>".into() })?
        .into();
    let key = args
        .next()
        .ok_or_else(|| -> BoxErr { "usage: probe_prop_shape <substrate-dir> <key>".into() })?;

    let path = resolve_props_path(&base);
    if !path.exists() {
        return Err(format!("no substrate.props at {}", path.display()).into());
    }

    let size = std::fs::metadata(&path)?.len();
    eprintln!(
        "probe_prop_shape: {} ({:.2} GiB), key=`{}`",
        path.display(),
        size as f64 / (1024.0 * 1024.0 * 1024.0),
        key
    );

    let mut f = File::open(&path)?;
    let mut hdr = [0u8; 4];
    f.read_exact(&mut hdr)?;
    let magic = u32::from_le_bytes(hdr);
    if magic != MAGIC {
        return Err(format!("bad magic {magic:#x}").into());
    }
    f.read_exact(&mut hdr)?;
    let version = u32::from_le_bytes(hdr);
    if version != FORMAT_VERSION {
        return Err(format!("unsupported version {version}").into());
    }

    let mut reader = BufReader::with_capacity(1 << 20, f);
    let cfg = bincode_config();
    let mut len_buf = [0u8; 8];

    // ---- Nodes ----
    reader.read_exact(&mut len_buf)?;
    let nodes_len = u64::from_le_bytes(len_buf);
    let mut nodes = Report::default();
    // Node ids carrying the key, so the caller can compare max_id against
    // the dense vec-column file size: 64 + (max_id + 1) * dim * 4.
    let mut min_id = u64::MAX;
    let mut max_id = 0u64;

    for i in 0..nodes_len {
        let entry: PropEntry = bincode::serde::decode_from_std_read(&mut reader, cfg)
            .map_err(|e| -> BoxErr { format!("node decode at {i}: {e}").into() })?;
        for (k, v) in &entry.props {
            if k == &key {
                nodes.observe(v);
                min_id = min_id.min(entry.id);
                max_id = max_id.max(entry.id);
            }
        }
    }

    // ---- Edges ----
    reader.read_exact(&mut len_buf)?;
    let edges_len = u64::from_le_bytes(len_buf);
    let mut edges = Report::default();
    for i in 0..edges_len {
        let entry: PropEntry = bincode::serde::decode_from_std_read(&mut reader, cfg)
            .map_err(|e| -> BoxErr { format!("edge decode at {i}: {e}").into() })?;
        for (k, v) in &entry.props {
            if k == &key {
                edges.observe(v);
            }
        }
    }

    nodes.print(&key, "node");
    if nodes.total > 0 {
        println!("node id range         : {min_id} ..= {max_id}");
        if let Some((dim, _)) = nodes.lengths.iter().next_back() {
            let projected = 64 + (max_id + 1) * (*dim as u64) * 4;
            println!(
                "dense veccol would be : {} bytes ({:.1} MiB) for dim={}",
                projected,
                projected as f64 / (1024.0 * 1024.0),
                dim
            );
        }
    }
    edges.print(&key, "edge");

    Ok(())
}
