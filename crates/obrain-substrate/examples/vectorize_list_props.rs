//! # `vectorize_list_props` — offline `List(Float64)` → `Vector` rewrite of `substrate.props`
//!
//! Converts every `(key, value)` pair whose value is a homogeneous numeric
//! [`Value::List`] of at least `--min-dims` elements into a [`Value::Vector`],
//! then atomically re-persists the sidecar. Zones (`substrate.nodes`,
//! `.edges`, `.tier*`, WAL, dict, meta) are **not touched**.
//!
//! ## STATUS: NOT PRODUCTION-READY — see "Known regression" below.
//!
//! ## Why
//!
//! `SubstrateStore::load_properties` auto-routes `Value::Vector` payloads
//! out of the legacy sidecar and into dense mmap'd `substrate.veccol.*`
//! zones (T16.7 Step 1). That routing is keyed on the *variant*: an
//! embedding persisted as `Value::List(Float64)` — which is what an
//! agent writing through Cypher or JSON naturally produces — never
//! matches, so it stays in `substrate.props` and pays the full bincode
//! decode on every open. Measured on a 93 616-node agent base: 8042
//! such lists (dim 1024) account for ~99 MB of a 123 MB sidecar.
//!
//! This tool is the missing first half of that migration. The second
//! half already exists — `obrain-migrate --finalize` does the
//! open → `flush()` → drop cycle that writes the vec-column headers and
//! re-serialises the sidecar without the routed-out keys.
//!
//! ## What this does NOT fix
//!
//! It was written on the hypothesis that these lists drove that base's
//! 10.9 GB open-time RSS. **They do not.** Measured end to end: the
//! sidecar drops 123.7 MB → 24.5 MB and RSS moves by 1.3%. The actual
//! cause on that base was 12.52 GB of dead payload in append-only blob
//! columns — see the `compact_blob_columns` example, which recovers
//! 99.2% of it. Sidecar size is simply not what dominates open RSS here.
//!
//! ## Known regression — why this is not deployed
//!
//! A vec column is **dense**: it allocates `max_entity_id + 1` slots, so
//! a column holding 8042 embeddings spread over 94 734 node ids is 99%
//! legitimately zero. The query layer does not treat a zeroed slot as
//! absent, so after conversion:
//!
//! ```text
//! MATCH (n) WHERE n.embedding IS NOT NULL RETURN count(n)
//!   before: 8042      after: 94424
//! ```
//!
//! Every node claims to have an embedding. Until the read path filters
//! all-zero slots the way `VecColumnRegistry::read_nonzero` already does,
//! this conversion changes query semantics and must not be run on a base
//! anything reads from.
//!
//! ## Why this is NOT a fix inside `load_properties`
//!
//! Converting on the open path would make **every read a write**: the
//! store opens read-write, so merely inspecting a base would mutate it.
//! That is not hypothetical — it is how a production base lost its
//! embeddings twice during the T-series investigation. The conversion
//! belongs in an explicit, offline, backed-up pass. This one never opens
//! a `SubstrateStore` at all; it is a pure file → file transform.
//!
//! ## Conversion rule
//!
//! A `List` is converted only if **every** element is `Float64` or
//! `Int64` and the length is `>= --min-dims` (default 64). Any other
//! element type, or a shorter list, is left untouched — a 3-element
//! `List` of coordinates is not an embedding.
//!
//! Note the *dimension* consequence: `VecColumnRegistry::write` freezes
//! the spec on first write and **rejects** a later write to the same
//! `(key, EntityKind)` with a different `dim`, and `route_vector_write`
//! swallows that rejection into a `tracing::error!` — leaving the slot
//! silently zeroed. So this tool refuses to convert a key whose lengths
//! are heterogeneous unless `--allow-mixed-dims` is passed; run
//! `probe_prop_shape` first to see the length histogram.
//!
//! ## Safety
//!
//! * Writes atomically via `persist()` (tmp file + rename). A crash
//!   mid-write leaves either the old or the new file, never a torn one.
//! * A `.pre-vectorize` copy is created before rewriting, so a manual
//!   rollback is `mv substrate.props.pre-vectorize substrate.props`.
//! * Zones and WAL are untouched — reopening an unconverted base still
//!   works, just with the old RSS profile.
//! * `--dry-run` reports the full plan (per-key counts and length
//!   histogram) without touching anything.
//!
//! ## Usage
//!
//! ```text
//! # 1. Inspect first — never convert blind:
//! cargo run --release -p obrain-substrate --example probe_prop_shape -- \
//!     ~/.obrain/db/agent/system embedding
//!
//! # 2. Dry-run the conversion:
//! cargo run --release -p obrain-substrate --example vectorize_list_props -- \
//!     ~/.obrain/db/agent/system --keys embedding --dry-run
//!
//! # 3. Convert for real:
//! cargo run --release -p obrain-substrate --example vectorize_list_props -- \
//!     ~/.obrain/db/agent/system --keys embedding
//!
//! # 4. Route into vec_columns and purge the sidecar (writes the headers):
//! cargo run --release -p obrain-migrate -- --finalize ~/.obrain/db/agent/system
//! ```
//!
//! Without `--keys`, *every* qualifying key is converted. Flags are
//! position-independent after the path.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use obrain_common::types::Value;
use obrain_substrate::PropertiesSnapshotV1;

type BoxErr = Box<dyn std::error::Error + Send + Sync>;

/// Below this length a numeric list is a tuple, not an embedding.
const DEFAULT_MIN_DIMS: usize = 64;

fn resolve_substrate_dir(base: &std::path::Path) -> PathBuf {
    let nested = base.join("substrate.obrain");
    if nested.join("substrate.meta").exists() {
        return nested;
    }
    base.to_path_buf()
}

/// Returns the `Vector` equivalent of a homogeneous numeric list, or
/// `None` if the value isn't one (wrong variant, too short, or mixed
/// element types).
fn as_vector(v: &Value, min_dims: usize) -> Option<Value> {
    let items = match v {
        Value::List(items) if items.len() >= min_dims => items,
        _ => return None,
    };
    let mut out: Vec<f32> = Vec::with_capacity(items.len());
    for it in items.iter() {
        match it {
            Value::Float64(f) => out.push(*f as f32),
            Value::Int64(i) => out.push(*i as f32),
            // One non-numeric element disqualifies the whole list.
            _ => return None,
        }
    }
    Some(Value::Vector(Arc::from(out.into_boxed_slice())))
}

#[derive(Default)]
struct KeyPlan {
    /// length -> occurrences, so a heterogeneous key is visible before
    /// it silently zeroes slots in the vec column.
    lengths: BTreeMap<usize, u64>,
    count: u64,
}

fn main() -> Result<(), BoxErr> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        return Err("usage: vectorize_list_props <substrate-dir> [--dry-run] \
                    [--keys k1,k2,...] [--min-dims N] [--allow-mixed-dims]"
            .into());
    }
    let base: PathBuf = PathBuf::from(args.remove(0));

    let mut dry_run = false;
    let mut allow_mixed = false;
    let mut min_dims = DEFAULT_MIN_DIMS;
    let mut only_keys: Option<Vec<String>> = None;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--dry-run" => dry_run = true,
            "--allow-mixed-dims" => allow_mixed = true,
            "--min-dims" => {
                let n = iter
                    .next()
                    .ok_or_else(|| -> BoxErr { "--min-dims requires a value".into() })?;
                min_dims = n
                    .parse::<usize>()
                    .map_err(|e| -> BoxErr { format!("--min-dims: not a number ({e})").into() })?;
            }
            "--keys" => {
                let csv = iter.next().ok_or_else(|| -> BoxErr {
                    "--keys requires a comma-separated value".into()
                })?;
                only_keys = Some(
                    csv.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect(),
                );
            }
            other => return Err(format!("unknown arg: {other}").into()),
        }
    }

    let dir = resolve_substrate_dir(&base);
    let props_path = dir.join("substrate.props");
    if !props_path.exists() {
        return Err(format!("no substrate.props at {}", props_path.display()).into());
    }

    let key_wanted = |k: &str| match &only_keys {
        Some(list) => list.iter().any(|s| s.as_str() == k),
        None => true,
    };

    let size_before = std::fs::metadata(&props_path)?.len();
    eprintln!(
        "vectorize: opening {} ({:.2} GiB)",
        props_path.display(),
        size_before as f64 / (1024.0 * 1024.0 * 1024.0)
    );

    // One full decode — peak anon tracks the file size. Acceptable for a
    // one-shot offline pass; the streaming variant lives in
    // `probe_prop_shape` for the read-only case.
    let t_load = web_time::Instant::now();
    let mut snap = PropertiesSnapshotV1::load(&props_path)?;
    eprintln!(
        "vectorize: loaded snapshot in {:.2}s ({} node entries, {} edge entries)",
        t_load.elapsed().as_secs_f64(),
        snap.nodes.len(),
        snap.edges.len()
    );

    // ---- Pass 1: plan (no mutation) ----
    let mut plan: BTreeMap<String, KeyPlan> = BTreeMap::new();
    {
        let mut survey = |props: &Vec<(String, Value)>| {
            for (k, v) in props {
                if !key_wanted(k) {
                    continue;
                }
                if let Value::List(items) = v
                    && as_vector(v, min_dims).is_some()
                {
                    let e = plan.entry(k.clone()).or_default();
                    e.count += 1;
                    *e.lengths.entry(items.len()).or_default() += 1;
                }
            }
        };
        for e in &snap.nodes {
            survey(&e.props);
        }
        for e in &snap.edges {
            survey(&e.props);
        }
    }

    if plan.is_empty() {
        eprintln!("vectorize: no qualifying numeric List >= {min_dims} elements — nothing to do");
        return Ok(());
    }

    eprintln!("vectorize: plan (min-dims={min_dims})");
    let mut mixed: Vec<&str> = Vec::new();
    for (k, p) in &plan {
        let dims: Vec<String> = p
            .lengths
            .iter()
            .map(|(len, n)| format!("dim={len}×{n}"))
            .collect();
        eprintln!("  {k:<24} {} values   {}", p.count, dims.join("  "));
        if p.lengths.len() > 1 {
            mixed.push(k.as_str());
        }
    }

    if !mixed.is_empty() && !allow_mixed {
        return Err(format!(
            "heterogeneous dimensions on key(s) {mixed:?}. VecColumnRegistry freezes the \
             spec on first write and rejects the rest into a swallowed tracing::error!, \
             leaving those slots silently zeroed. Re-run with --allow-mixed-dims only if \
             you accept losing every value whose dim differs from the first one written."
        )
        .into());
    }

    if dry_run {
        eprintln!("vectorize: --dry-run specified, not rewriting the file");
        return Ok(());
    }

    // ---- Pass 2: convert ----
    let mut converted = 0u64;
    {
        let mut apply = |props: &mut Vec<(String, Value)>| {
            for (k, v) in props.iter_mut() {
                if !key_wanted(k) {
                    continue;
                }
                if let Some(vec) = as_vector(v, min_dims) {
                    *v = vec;
                    converted += 1;
                }
            }
        };
        for e in snap.nodes.iter_mut() {
            apply(&mut e.props);
        }
        for e in snap.edges.iter_mut() {
            apply(&mut e.props);
        }
    }
    eprintln!("vectorize: converted {converted} values to Value::Vector");

    // Backup before rewriting. Intra-volume copies promote to clonefile
    // on APFS; elsewhere it's a real but bounded one-off cost.
    let backup = props_path.with_extension("props.pre-vectorize");
    if !backup.exists() {
        std::fs::copy(&props_path, &backup)?;
        eprintln!(
            "vectorize: backup written to {} ({} B)",
            backup.display(),
            std::fs::metadata(&backup)?.len()
        );
    } else {
        eprintln!(
            "vectorize: backup already exists at {}, keeping existing",
            backup.display()
        );
    }

    // Atomic rewrite: `persist` uses tmp + rename.
    let t_write = web_time::Instant::now();
    snap.persist(&props_path)?;
    let size_after = std::fs::metadata(&props_path)?.len();
    eprintln!(
        "vectorize: rewrote {} in {:.2}s ({:.2} MiB -> {:.2} MiB)",
        props_path.display(),
        t_write.elapsed().as_secs_f64(),
        size_before as f64 / (1024.0 * 1024.0),
        size_after as f64 / (1024.0 * 1024.0),
    );
    eprintln!(
        "vectorize: NEXT STEP — the vectors are still in the sidecar. Run\n  \
         obrain-migrate --finalize {}\n\
         to route them into substrate.veccol.* and write the column headers.",
        base.display()
    );
    Ok(())
}
