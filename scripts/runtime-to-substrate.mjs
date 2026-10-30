// runtime cbm → substrate (props COMPLÈTES + texte depuis runtime_payloads).
// Usage : node runtime-to-substrate.mjs <out.obrain> <cbm-runtime.db>
import { DatabaseSync } from 'node:sqlite';
import { rmSync } from 'node:fs';
import { createRequire } from 'node:module';
const require = createRequire('/tmp/');
const { ObrainDB } = require('/tmp/rtprobe.node');
const out = process.argv[2];
const dbPath = process.argv[3];
rmSync(out, { recursive: true, force: true });
const db = ObrainDB.create(out);
const s = new DatabaseSync(dbPath, { readOnly: true });
const hasPayloads = s.prepare("select name from sqlite_master where type='table' and name='runtime_payloads'").get();
const pl = hasPayloads ? s.prepare('select qn,payload from runtime_payloads') : null;
const payloads = new Map();
if (pl) for (const r of pl.all()) payloads.set(r.qn, r.payload);
const scalar = (o) => o !== null && typeof o !== 'object';
function textOf(qn, props, label) {
  if (typeof props.text === 'string' && props.text) return props.text;
  const p = payloads.get(qn);
  if (p == null) return undefined;
  if (typeof p === 'string') {
    const t = p.trim();
    if (t.startsWith('{') || t.startsWith('[')) { try { const j = JSON.parse(t); if (j && typeof j === 'object') { if (typeof j.text === 'string') return j.text; if (typeof j.content === 'string') return j.content; } } catch {} }
    return p;
  }
  return undefined;
}
const t0 = Date.now();
const nodes = s.prepare('select id,label,qualified_name,properties from nodes').all();
const nid = new Map();
const sessNodes = new Map(); // name -> nodeId
const msgs = []; // {sess, id}
for (const r of nodes) {
  const props = {};
  try { Object.assign(props, JSON.parse(r.properties)); } catch { /* props absentes */ }
  const qn = r.qualified_name ?? props.qualified_name ?? null;
  const clean = {};
  for (const [k, v] of Object.entries(props)) if (scalar(v)) clean[k] = v;
  if (qn != null) clean.qualified_name = qn;
  if (r.label === 'Message') { const t = textOf(qn, props, r.label); if (t !== undefined) clean.text = t; }
  const created = db.createNode([r.label || 'Node'], clean);
  nid.set(r.id, created.id);
  if (r.label === 'Session' && qn) sessNodes.set(qn, created.id);
  else if (r.label === 'Message' && qn) msgs.push({ sess: qn.split(':msg:')[0], id: created.id });
}
console.log(`nodes: ${nodes.length} (${((Date.now() - t0) / 1000).toFixed(1)}s)`);
const edges = s.prepare('select source_id,target_id,type from edges').all();
const t1 = Date.now();
let ok = 0, skip = 0;
for (const e of edges) {
  const a = nid.get(e.source_id), b = nid.get(e.target_id);
  if (a === undefined || b === undefined) { skip++; continue; }
  db.createEdge(a, b, String(e.type), {});
  ok++;
}
console.log(`edges: ${ok} (${skip} sautées) (${((Date.now() - t1) / 1000).toFixed(1)}s) | total ${((Date.now() - t0) / 1000).toFixed(1)}s`);
// IN_SESSION (structure native de lecture des messages) : session → message.
let ins = 0, miss = 0;
for (const m of msgs) {
  const sId = sessNodes.get(m.sess);
  if (sId === undefined) { miss++; continue; }
  db.createEdge(sId, m.id, 'IN_SESSION', {});
  ins++;
}
console.log(`IN_SESSION: +${ins} arêtes (${miss} messages sans session)`);
if (typeof db.checkpoint === 'function') db.checkpoint();
db.close();
console.log('DONE runtime → substrate');
