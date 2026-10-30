// Combinaison C1 « dépôt → analyse → réponse » : ingestion native + tri
// LDleiden + compteurs O(1) + fenêtre Z sur les hubs — mesurée bout en bout.
// Usage : node combo-pipeline.mjs <store-neuf> <db-cbm> [db-cbm...]
import { execFileSync } from 'node:child_process';
import { DatabaseSync } from 'node:sqlite';
import { writeFileSync, rmSync, existsSync } from 'node:fs';
import { createRequire } from 'node:module';
const require = createRequire('/tmp/');
const { ObrainDB } = require('/tmp/rtprobe.node');
const dest = process.argv[2];
const dbs = process.argv.slice(3);
const OB = '/home/triviere/projects/banc-obrain/target/release/obrain';
rmSync(dest, { recursive: true, force: true });
// 1) génération corpus binaire (un dépôt)
const labels = [], SRC = [], DST = [], TY = [], TYS = [];
const tymap = new Map();
let base = 0;
for (const name of dbs) {
  const s = new DatabaseSync(name, { readOnly: true });
  const ns = s.prepare('select id,label from nodes').all();
  const map = new Map();
  for (const r of ns) { labels.push(String(r.label)); base++; map.set(r.id, base); }
  const es = s.prepare('select source_id,target_id,type from edges').all();
  for (const e of es) { const t = String(e.type); if (!tymap.has(t)) { tymap.set(t, TYS.length); TYS.push(t); } SRC.push(map.get(e.source_id)); DST.push(map.get(e.target_id)); TY.push(tymap.get(t)); }
  s.close();
}
const chunks = [];
const w64 = (v) => { const b = Buffer.alloc(8); b.writeBigUInt64LE(BigInt(v)); chunks.push(b); };
const wstr = (t) => { const l = Buffer.alloc(2); l.writeUInt16LE(Buffer.byteLength(t)); chunks.push(l, Buffer.from(t, 'utf8')); };
w64(labels.length); w64(SRC.length); const nb = Buffer.alloc(4); nb.writeUInt32LE(TYS.length); chunks.push(nb);
for (const t of TYS) wstr(t);
for (const l of labels) wstr(l);
for (let i = 0; i < SRC.length; i++) { const b = Buffer.alloc(8); b.writeUInt32LE(SRC[i], 0); b.writeUInt32LE(DST[i], 4); const e = Buffer.alloc(2); e.writeUInt16LE(TY[i]); chunks.push(b, e); }
writeFileSync('/tmp/combo.bin', Buffer.concat(chunks));
// 2) ingestion native
const t0 = Date.now();
const imp = execFileSync(OB, ['native-import', dest, '/tmp/combo.bin'], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });
const tImp = (Date.now() - t0) / 1000;
// 3) tri LDleiden (analyse architecture)
const t1 = Date.now();
execFileSync(OB, ['topo', 'hilbert', dest], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });
const tTopo = (Date.now() - t1) / 1000;
// 4) réponses (compteurs O(1) + communauté top + hubs zruns)
const db = ObrainDB.openReadOnly(dest);
const t2 = Date.now();
let top = [0, 0];
for (let cid = 1; cid <= 60000; cid++) { const n = db.windowQuery2dSorted(cid, 0, 65535, 0, 4294967295).length; if (n > top[1]) top = [cid, n]; }
const hubs = db.windowQuery2dSorted(top[0], 0, 65535, 50, 4294967295).length;
const tResp = (Date.now() - t2) / 1000;
console.log(`C1 ${dbs.length > 1 ? dbs.length + ' dépôts' : dbs[0].split('/').pop()}`);
console.log(`  ingestion native : ${tImp.toFixed(3)} s | tri+communautés : ${tTopo.toFixed(3)} s | réponse (top comm + hubs) : ${tResp.toFixed(3)} s`);
console.log(`  TOTAL boucle : ${(tImp + tTopo + tResp).toFixed(3)} s — ${db.nodeCount()} nœuds / ${db.edgeCount()} arêtes | top communauté ${top[0]} (${top[1]}) hubs d≥50 : ${hubs}`);
db.close();
