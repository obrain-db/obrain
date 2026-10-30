// Génère le corpus binaire pour `obrain native-import` (RFC §16.1).
// Usage : node code-corpus-to-binary.mjs <out.bin> <cbm-db...>
// Format : u64 n, u64 m | u32 nt, nt×(u16 len+bytes) | n×(u16 len+bytes) |
//          m×(u32 src, u32 dst, u16 type_index)  — LE little-endian.
import { DatabaseSync } from 'node:sqlite';
import { writeFileSync } from 'node:fs';
const out = process.argv[2];
const dbs = process.argv.slice(3);
const labels = [], SRC = [], DST = [], TYI = [], TYS = [];
const tymap = new Map();
const tIdx = (t) => { if (!tymap.has(t)) { tymap.set(t, TYS.length); TYS.push(t); } return tymap.get(t); };
let base = 0;
for (const name of dbs) {
  const s = new DatabaseSync(name, { readOnly: true });
  const ns = s.prepare('select id,label from nodes').all();
  const map = new Map();
  for (const r of ns) { labels.push(String(r.label)); base++; map.set(r.id, base); }
  const es = s.prepare('select source_id,target_id,type from edges').all();
  for (const e of es) { SRC.push(map.get(e.source_id)); DST.push(map.get(e.target_id)); TYI.push(tIdx(e.type)); }
  s.close();
}
const chunks = [];
const w64 = (v) => { const b = Buffer.alloc(8); b.writeBigUInt64LE(BigInt(v)); chunks.push(b); };
const w32 = (v) => { const b = Buffer.alloc(4); b.writeUInt32LE(v); chunks.push(b); };
const wstr = (s) => { const l = Buffer.alloc(2); l.writeUInt16LE(Buffer.byteLength(s)); chunks.push(l, Buffer.from(s, 'utf8')); };
w64(labels.length); w64(SRC.length); w32(TYS.length);
for (const t of TYS) wstr(t);
for (const lab of labels) wstr(lab);
for (let i = 0; i < SRC.length; i++) {
  const b = Buffer.alloc(8); b.writeUInt32LE(SRC[i], 0); b.writeUInt32LE(DST[i], 4); chunks.push(b);
  const e = Buffer.alloc(2); e.writeUInt16LE(TYI[i]); chunks.push(e);
}
writeFileSync(out, Buffer.concat(chunks));
console.log(`corpus écrit : ${labels.length} nœuds / ${SRC.length} arêtes / ${TYS.length} types → ${out}`);
