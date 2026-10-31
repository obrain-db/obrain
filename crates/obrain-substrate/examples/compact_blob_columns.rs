//! # `compact_blob_columns` — reclaim dead space in `substrate.blobcol.*`
//!
//! Blob columns are an **append-only** arena: rewriting a property appends
//! a fresh payload and simply repoints the slot, leaving the previous bytes
//! stranded forever. There is no compaction anywhere in the crate, so a
//! base whose large text properties are rewritten in a loop grows without
//! bound.
//!
//! Measured on a production 93 616-node agent base (2026-07-28):
//!
//! ```text
//!   col        dat_bytes       live_bytes     dead   slots used
//!  0015      12_423_016_448        3_411_291  100.0%     88/101372
//!  0011         111_857_664       81_427_226   27.2%  27994/134396
//!  0000          49_713_152          233_120   99.5%    750/121084
//!  ...
//!  TOTAL         12.62 GB           0.10 GB    99.2%
//! ```
//!
//! 12.52 GB of the 12.62 GB on disk was dead. One column — 88 live values
//! totalling 3.4 MB — accounted for 12.42 GB of it.
//!
//! ## What this does
//!
//! For each `(idx, dat)` pair: walk the slot directory, copy only the live
//! payloads (`len > 0`) into a fresh arena in slot order, rewrite each
//! slot's `arena_offset`, recompute both CRC32s, and swap the files in.
//!
//! `n_slots` is **preserved**, so slot indices (which are entity ids) keep
//! their meaning and absent slots stay absent. Reads are unaffected: a
//! slot's `(offset, len)` is the only thing that moves.
//!
//! ## Safety
//!
//! * Never opens a `SubstrateStore` — pure file → file, so it cannot
//!   mutate anything through the store's read-write open path.
//! * The originals are **renamed** to `.pre-compact` (free, no copy), so
//!   rollback is a rename back and needs no extra disk space.
//! * New files are written to `.tmp`, fsynced, and only then swapped in.
//! * Verifies the source `arena_crc32` before touching a column unless
//!   `--no-verify`; a mismatch skips that column (or aborts with
//!   `--strict`).
//! * `--dry-run` reports the full plan and writes nothing.
//!
//! Run with the service offline.
//!
//! ## Usage
//!
//! ```text
//! cargo run --release -p obrain-substrate --example compact_blob_columns -- \
//!     ~/.obrain/db/agent/system --dry-run
//!
//! cargo run --release -p obrain-substrate --example compact_blob_columns -- \
//!     ~/.obrain/db/agent/system
//!
//! # once the base is verified good:
//! rm ~/.obrain/db/agent/system/*.pre-compact
//! ```

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

type BoxErr = Box<dyn std::error::Error + Send + Sync>;

const BLOB_MAGIC: u32 = u32::from_le_bytes(*b"BLB1");
const BLOB_VERSION: u32 = 1;
const HEADER_SIZE: usize = 64;
const SLOT_STRIDE: usize = 16;
/// Copy buffer for moving one payload from the old arena to the new one.
const COPY_CHUNK: usize = 1 << 20;

#[derive(Debug, Clone, Copy)]
struct Header {
    prop_key_id: u16,
    entity_kind: u8,
    n_slots: u32,
    arena_bytes: u64,
    idx_crc32: u32,
    arena_crc32: u32,
}

impl Header {
    fn parse(b: &[u8]) -> Result<Self, BoxErr> {
        if b.len() < HEADER_SIZE {
            return Err("idx shorter than header".into());
        }
        let u32at = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        let magic = u32at(0);
        if magic != BLOB_MAGIC {
            return Err(format!("bad magic {magic:#x}").into());
        }
        let version = u32at(4);
        if version != BLOB_VERSION {
            return Err(format!("unsupported version {version}").into());
        }
        Ok(Self {
            prop_key_id: u16::from_le_bytes(b[8..10].try_into().unwrap()),
            entity_kind: b[10],
            n_slots: u32at(12),
            arena_bytes: u64::from_le_bytes(b[16..24].try_into().unwrap()),
            idx_crc32: u32at(24),
            arena_crc32: u32at(28),
        })
    }

    /// Serialise back into the frozen 64 B layout. `_pad0` and the
    /// 32 B `_reserved` tail stay zero, matching `BlobColumnHeader::new`.
    fn to_bytes(self) -> [u8; HEADER_SIZE] {
        let mut out = [0u8; HEADER_SIZE];
        out[0..4].copy_from_slice(&BLOB_MAGIC.to_le_bytes());
        out[4..8].copy_from_slice(&BLOB_VERSION.to_le_bytes());
        out[8..10].copy_from_slice(&self.prop_key_id.to_le_bytes());
        out[10] = self.entity_kind;
        out[12..16].copy_from_slice(&self.n_slots.to_le_bytes());
        out[16..24].copy_from_slice(&self.arena_bytes.to_le_bytes());
        out[24..28].copy_from_slice(&self.idx_crc32.to_le_bytes());
        out[28..32].copy_from_slice(&self.arena_crc32.to_le_bytes());
        out
    }
}

/// One `(arena_offset, len)` slot. `len == 0` means absent.
#[derive(Clone, Copy)]
struct Slot {
    off: u64,
    len: u32,
}

fn read_slots(idx: &[u8], n_slots: u32) -> Vec<Slot> {
    (0..n_slots as usize)
        .map(|i| {
            let b = HEADER_SIZE + i * SLOT_STRIDE;
            Slot {
                off: u64::from_le_bytes(idx[b..b + 8].try_into().unwrap()),
                len: u32::from_le_bytes(idx[b + 8..b + 12].try_into().unwrap()),
            }
        })
        .collect()
}

/// CRC32 over `dat[0..len]`, streamed so a 12 GB arena costs one buffer.
fn crc_of_file_prefix(path: &Path, len: u64) -> Result<u32, BoxErr> {
    let mut f = File::open(path)?;
    let mut h = crc32fast::Hasher::new();
    let mut buf = vec![0u8; COPY_CHUNK];
    let mut left = len;
    while left > 0 {
        let want = COPY_CHUNK.min(left as usize);
        f.read_exact(&mut buf[..want])?;
        h.update(&buf[..want]);
        left -= want as u64;
    }
    Ok(h.finalize())
}

fn human(n: u64) -> String {
    const U: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", U[i])
}

struct Plan {
    idx_path: PathBuf,
    dat_path: PathBuf,
    header: Header,
    slots: Vec<Slot>,
    live_bytes: u64,
    live_slots: usize,
    dat_size: u64,
}

fn scan(dir: &Path) -> Result<Vec<Plan>, BoxErr> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let p = entry?.path();
        let name = match p.file_name().and_then(|s| s.to_str()) {
            Some(n) if n.starts_with("substrate.blobcol.") && n.ends_with(".idx") => n.to_string(),
            _ => continue,
        };
        let dat_path = p.with_extension("dat");
        if !dat_path.exists() {
            eprintln!("compact: {name}: no matching .dat, skipping");
            continue;
        }
        let idx_bytes = std::fs::read(&p)?;
        let header = match Header::parse(&idx_bytes) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("compact: {name}: unreadable header ({e}), skipping");
                continue;
            }
        };
        let need = HEADER_SIZE + header.n_slots as usize * SLOT_STRIDE;
        if idx_bytes.len() < need {
            eprintln!(
                "compact: {name}: idx truncated ({} < {need}), skipping",
                idx_bytes.len()
            );
            continue;
        }
        let slots = read_slots(&idx_bytes, header.n_slots);
        let live_bytes: u64 = slots.iter().map(|s| s.len as u64).sum();
        let live_slots = slots.iter().filter(|s| s.len > 0).count();
        out.push(Plan {
            idx_path: p,
            dat_path: dat_path.clone(),
            header,
            slots,
            live_bytes,
            live_slots,
            dat_size: std::fs::metadata(&dat_path)?.len(),
        });
    }
    out.sort_by_key(|pl| std::cmp::Reverse(pl.dat_size));
    Ok(out)
}

/// Rewrite one column's arena keeping only live payloads.
fn compact_one(pl: &Plan, verify: bool, strict: bool) -> Result<bool, BoxErr> {
    let name = pl
        .idx_path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();

    // Bounds are checked against the **file**, not against the header's
    // `arena_bytes`.
    //
    // `write_slot` appends the payload (step 1) before publishing the slot
    // entry (step 2), and only `sync()` refreshes `arena_bytes` in the
    // header. A process killed between the last write and the next sync
    // therefore leaves a perfectly valid slot directory in front of a
    // stale header — which is exactly the state an OOM-killed service
    // leaves behind, and it describes most of the columns on the base this
    // tool was written for. Treating `arena_bytes` as authoritative would
    // skip precisely the columns that need compacting most.
    //
    // The slot directory is the authority for what is live. Copying the
    // bytes each slot points at reproduces, byte for byte, what a read
    // returns from this base today — so compaction cannot degrade a slot
    // that was already questionable, it only relocates it.
    let mut beyond_recorded = 0usize;
    let mut max_end = 0u64;
    for (i, s) in pl.slots.iter().enumerate() {
        if s.len == 0 {
            continue;
        }
        let end = s.off.saturating_add(s.len as u64);
        max_end = max_end.max(end);
        if end > pl.dat_size {
            let msg = format!(
                "{name}: slot {i} range {}..{end} escapes the dat file \
                 (dat_size={})",
                s.off, pl.dat_size
            );
            if strict {
                return Err(msg.into());
            }
            eprintln!("compact: {msg} — skipping column");
            return Ok(false);
        }
        if end > pl.header.arena_bytes {
            beyond_recorded += 1;
        }
    }
    if beyond_recorded > 0 {
        eprintln!(
            "compact: {name}: {beyond_recorded} slot(s) live past the header's \
             arena_bytes ({} < {max_end}) — stale header from an unclean \
             shutdown; trusting the slot directory",
            pl.header.arena_bytes
        );
    }

    // Only meaningful when the header actually describes the whole live
    // region; a stale `arena_bytes` makes its CRC describe a prefix, so
    // comparing against it would fail by construction.
    let verify = verify && beyond_recorded == 0;
    if verify {
        let actual = crc_of_file_prefix(&pl.dat_path, pl.header.arena_bytes)?;
        if actual != pl.header.arena_crc32 {
            let msg = format!(
                "{name}: source arena CRC mismatch (header={:#010x}, actual={:#010x})",
                pl.header.arena_crc32, actual
            );
            if strict {
                return Err(msg.into());
            }
            eprintln!("compact: {msg} — skipping column");
            return Ok(false);
        }
    }

    let dat_tmp = pl.dat_path.with_extension("dat.tmp");
    let idx_tmp = pl.idx_path.with_extension("idx.tmp");

    // ---- Write the new arena, live payloads only, in slot order. -------
    let mut src = File::open(&pl.dat_path)?;
    let mut new_slots: Vec<Slot> = Vec::with_capacity(pl.slots.len());
    let mut arena_crc = crc32fast::Hasher::new();
    let mut cursor: u64 = 0;
    {
        let mut dst = BufWriter::with_capacity(COPY_CHUNK, File::create(&dat_tmp)?);
        let mut buf = vec![0u8; COPY_CHUNK];
        for s in &pl.slots {
            if s.len == 0 {
                // Absent stays absent — offset 0 is never dereferenced
                // because `len == 0` is the unambiguous absent marker.
                new_slots.push(Slot { off: 0, len: 0 });
                continue;
            }
            src.seek(SeekFrom::Start(s.off))?;
            let mut left = s.len as usize;
            while left > 0 {
                let want = COPY_CHUNK.min(left);
                src.read_exact(&mut buf[..want])?;
                dst.write_all(&buf[..want])?;
                arena_crc.update(&buf[..want]);
                left -= want;
            }
            new_slots.push(Slot {
                off: cursor,
                len: s.len,
            });
            cursor += s.len as u64;
        }
        dst.flush()?;
        dst.into_inner()?.sync_all()?;
    }

    // ---- Write the new idx: header + rewritten slot directory. ---------
    {
        let mut slot_region = Vec::with_capacity(new_slots.len() * SLOT_STRIDE);
        for s in &new_slots {
            slot_region.extend_from_slice(&s.off.to_le_bytes());
            slot_region.extend_from_slice(&s.len.to_le_bytes());
            slot_region.extend_from_slice(&0u32.to_le_bytes()); // _pad
        }
        let mut idx_crc = crc32fast::Hasher::new();
        idx_crc.update(&slot_region);

        let mut h = pl.header;
        h.arena_bytes = cursor;
        h.arena_crc32 = arena_crc.finalize();
        h.idx_crc32 = idx_crc.finalize();

        let mut f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&idx_tmp)?;
        f.write_all(&h.to_bytes())?;
        f.write_all(&slot_region)?;
        f.sync_all()?;
    }

    // ---- Swap in. Originals are renamed, never copied. -----------------
    // A crash between the two rename pairs leaves `.pre-compact` present
    // alongside a `.tmp`; both are intact files, so recovery is manual
    // but lossless.
    std::fs::rename(&pl.dat_path, pl.dat_path.with_extension("dat.pre-compact"))?;
    std::fs::rename(&pl.idx_path, pl.idx_path.with_extension("idx.pre-compact"))?;
    std::fs::rename(&dat_tmp, &pl.dat_path)?;
    std::fs::rename(&idx_tmp, &pl.idx_path)?;

    eprintln!(
        "compact: {name}: {} -> {} ({} live in {} slots)",
        human(pl.dat_size),
        human(cursor),
        human(pl.live_bytes),
        pl.live_slots,
    );
    Ok(true)
}

fn main() -> Result<(), BoxErr> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        return Err("usage: compact_blob_columns <substrate-dir> [--dry-run] \
                    [--no-verify] [--strict]"
            .into());
    }
    let base = PathBuf::from(args.remove(0));
    let mut dry_run = false;
    let mut verify = true;
    let mut strict = false;
    for a in args {
        match a.as_str() {
            "--dry-run" => dry_run = true,
            "--no-verify" => verify = false,
            "--strict" => strict = true,
            other => return Err(format!("unknown arg: {other}").into()),
        }
    }

    let dir = {
        let nested = base.join("substrate.obrain");
        if nested.join("substrate.meta").exists() {
            nested
        } else {
            base.clone()
        }
    };

    let plans = scan(&dir)?;
    if plans.is_empty() {
        eprintln!("compact: no blob columns under {}", dir.display());
        return Ok(());
    }

    println!("=== compact_blob_columns: {} ===", dir.display());
    println!(
        "{:<10} {:>12} {:>12} {:>8} {:>16}",
        "col", "on_disk", "live", "dead", "slots_used"
    );
    let mut tot_disk = 0u64;
    let mut tot_live = 0u64;
    for pl in &plans {
        let dead = if pl.dat_size > 0 {
            100.0 * (1.0 - pl.live_bytes as f64 / pl.dat_size as f64)
        } else {
            0.0
        };
        println!(
            "{:<10} {:>12} {:>12} {:>7.1}% {:>7}/{}",
            format!("{:04x}", pl.header.prop_key_id),
            human(pl.dat_size),
            human(pl.live_bytes),
            dead,
            pl.live_slots,
            pl.header.n_slots,
        );
        tot_disk += pl.dat_size;
        tot_live += pl.live_bytes;
    }
    println!();
    println!("total on disk : {}", human(tot_disk));
    println!("total live    : {}", human(tot_live));
    println!(
        "reclaimable   : {} ({:.1}%)",
        human(tot_disk.saturating_sub(tot_live)),
        if tot_disk > 0 {
            100.0 * (1.0 - tot_live as f64 / tot_disk as f64)
        } else {
            0.0
        }
    );

    if dry_run {
        println!();
        println!("--dry-run specified, nothing written");
        return Ok(());
    }

    println!();
    let mut done = 0usize;
    for pl in &plans {
        if compact_one(pl, verify, strict)? {
            done += 1;
        }
    }
    eprintln!(
        "compact: {done}/{} columns compacted. Originals kept as \
         *.pre-compact — delete them once the base is verified good.",
        plans.len()
    );
    Ok(())
}
