//! Ingestion native zone-direct (RFC §16.1) : lit un corpus binaire et
//! l'importe d'un bloc dans un store substrate NEUF via
//! `ObrainDB::bulk_import` (Writer::bulk_write, pas de WAL par op).
//!
//! Format binaire (little-endian) :
//!   u64 n, u64 m
//!   u32 nt, puis nt × (u16 len, bytes)      // noms de types d'arêtes
//!   n × (u16 len, bytes)                     // un label par nœud
//!   m × (u32 src, u32 dst, u16 type_index)

use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};

fn read_str(r: &mut impl Read, len_bits: usize) -> Result<String> {
    let mut lb = [0u8; 2];
    r.read_exact(&mut lb)?;
    let len = u16::from_le_bytes(lb) as usize;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

pub fn run(path: &Path, file: &Path) -> Result<()> {
    let bytes = std::fs::read(file).with_context(|| format!("lire {}", file.display()))?;
    let mut cur = &bytes[..];
    let read_u64 = |c: &mut &[u8]| -> u64 {
        let mut b = [0u8; 8];
        c.read_exact(&mut b).unwrap();
        u64::from_le_bytes(b)
    };
    let read_u32 = |c: &mut &[u8]| -> u32 {
        let mut b = [0u8; 4];
        c.read_exact(&mut b).unwrap();
        u32::from_le_bytes(b)
    };
    let n = read_u64(&mut cur) as usize;
    let m = read_u64(&mut cur) as usize;
    let nt = read_u32(&mut cur) as usize;
    let mut types: Vec<String> = Vec::with_capacity(nt);
    for _ in 0..nt {
        types.push(read_str(&mut cur, 16)?);
    }
    let mut labels: Vec<String> = Vec::with_capacity(n);
    for _ in 0..n {
        labels.push(read_str(&mut cur, 16)?);
    }
    let mut src = Vec::with_capacity(m);
    let mut dst = Vec::with_capacity(m);
    let mut edge_types = Vec::with_capacity(m);
    for _ in 0..m {
        src.push(read_u32(&mut cur));
        dst.push(read_u32(&mut cur));
        let mut b = [0u8; 2];
        cur.read_exact(&mut b)?;
        let ti = u16::from_le_bytes(b) as usize;
        edge_types.push(types[ti].clone());
    }
    drop(types);

    let t0 = std::time::Instant::now();
    let db = obrain_engine::ObrainDB::open(path)
        .with_context(|| format!("créer/ouvrir store {}", path.display()))?;
    let (nn, mm) = db
        .bulk_import(&labels, &src, &dst, &edge_types)
        .map_err(|e| anyhow::anyhow!("bulk_import: {e}"))?;
    db.checkpoint()
        .map_err(|e| anyhow::anyhow!("checkpoint: {e}"))?;
    let elapsed = t0.elapsed();
    eprintln!(
        "native-import: +{nn} nœuds +{mm} arêtes en {:.3} s",
        elapsed.as_secs_f64()
    );
    Ok(())
}
