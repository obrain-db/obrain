#!/usr/bin/env node
// (RFC zorder-superposition §12) Importe les index de code cbm (SQLite, un
// fichier par projet) vers un store substrate unique. Voir en-tête usage :
// node scripts/code-index-to-substrate.mjs <dest.obrain> <src.db>...
// Import de TOUS les index de code cbm vers un store substrate unique.
import { DatabaseSync } from 'node:sqlite';
import { createRequire } from 'node:module';
import { rmSync, existsSync } from 'node:fs';
const require = createRequire('/tmp/');
const { ObrainDB } = require('/tmp/rtprobe.node');
const dest = process.argv[2];
const dbs = process.argv.slice(3);
const fresh = !existsSync(dest + '/substrate.meta');
if (!fresh) console.log('store existant — append');
const db = fresh ? ObrainDB.create(dest) : ObrainDB.open(dest);
let tN = 0, tE = 0, skip = 0;
for (const src of dbs) {
  const key = src.split('/').pop();
  const s = new DatabaseSync(src, { readOnly: true });
  const ids = new Map();
  const t0 = Date.now();
  const nodes = s.prepare('select id,label,name,qualified_name,file_path,start_line,end_line from nodes').all();
  for (const r of nodes) {
    const NO_PROPS = process.env.NO_PROPS === '1';
    const props = NO_PROPS ? { qualified_name: r.qualified_name ?? null } : { name: r.name ?? null, qualified_name: r.qualified_name ?? null, file_path: r.file_path ?? null, start_line: r.start_line ?? 0, end_line: r.end_line ?? 0 };
    const lab = [String(r.label || 'Node')];
    ids.set(key + '|' + r.id, NO_PROPS ? db.createNodeNoWal(lab) : db.createNode(lab, props).id);
  }
  const edges = s.prepare('select source_id,target_id,type from edges').all();
  const BATCH = 8192;
  let buf = [];
  const flush = () => {
    if (!buf.length) return;
    const src = buf.map((x) => x[0]), dst = buf.map((x) => x[1]), ty = buf.map((x) => x[2]);
    db.createEdgesBatchFast(src, dst, ty);
    buf = [];
  };
  for (const e of edges) {
    const a = ids.get(key + '|' + e.source_id), b = ids.get(key + '|' + e.target_id);
    if (a === undefined || b === undefined) { skip++; continue; }
    buf.push([a, b, String(e.type)]);
    if (buf.length >= BATCH) flush();
  }
  flush();
  tN += nodes.length; tE += edges.length;
  console.log(`${key}: +${nodes.length} nœuds +${edges.length} arêtes (${((Date.now()-t0)/1000).toFixed(1)}s)`);
  s.close();
}
if (typeof db.checkpoint === 'function') db.checkpoint();
db.close();
console.log(`DONE total +${tN} nœuds +${tE} arêtes (skipped ${skip})`);

// Journal d'import (RFC diff-incremental-variantes) : captation RUST après
// import — manifest + diff + préconstruction des résumés de compare. Glue
// uniquement (logique dans la CLI obrain) ; non fatal si la CLI manque.
if (dbs.length === 1) {
  // Le binding tient encore le store tant que node vit : la CLI (journal-import)
  // est lancée DÉTACHÉE — elle s'exécute après la sortie de node et capture le
  // journal + préconstruit les résumés de compare (logique 100 % Rust).
  const { spawn } = await import('child_process');
  const prefix = String(dbs[0]).split(/[\\/]/).pop().replace(/\.db$/i, '');
  const obrain = process.env.OBRAIN_BIN || 'target/release/obrain';
  const child = spawn(obrain, ['journal-import', String(dest), '--prefix', prefix], {
    stdio: 'ignore',
    detached: true,
  });
  child.unref();
  console.log(`journal-import lancé (async) : ${dest} (${prefix})`);
}
