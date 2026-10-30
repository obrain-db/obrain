// Vérification reproductible des performances annoncées (RFC §14/§16, 2026-09-04).
// Usage : node verify-engine.mjs   (stores sous target/bench/real/)
import { createRequire } from 'node:module';
import { existsSync } from 'node:fs';
const require = createRequire('/tmp/');
const { ObrainDB } = require('/tmp/rtprobe.node');
const R = '/home/triviere/projects/banc-obrain/target/bench/real/';
const out = [];
const log = (s) => { out.push(s); console.log(s); };
async function gqlCount(db, q) { const r = await db.execute(q, null); const row = r.get(0) ?? {}; for (const v of Object.values(row)) if (typeof v === 'number') return v; return NaN; }
async function bench(fn, n = 30) { await fn(); const t0 = process.hrtime.bigint(); for (let i = 0; i < n; i++) await fn(); return Number(process.hrtime.bigint() - t0) / 1000 / n; }
if (!existsSync(R + 'code-all3.obrain')) { log('stores absents — lancer l’import natif d’abord'); process.exit(1); }
log('=== VÉRIFICATION REQUÊTES & PERFORMANCES — ' + new Date().toISOString() + ' ===');
const t = ObrainDB.openReadOnly(R + 'code-all3.obrain'); // topologie (445 k / 1,67 M)
log('store code-all3 : ' + t.nodeCount() + ' nœuds / ' + t.edgeCount() + ' arêtes');
// NOTE : compteurs O(1) mesurés dans verify-o1.mjs (process dédié, protocole canonique).
// Syntaxes GQL exactes
const q1 = 'MATCH (n) RETURN count(n) AS n';
const q2 = 'MATCH (n:File) RETURN count(n) AS n';
const q3 = 'MATCH (n:Function) RETURN count(n) AS n';
const q4 = 'MATCH ()-[r:CALLS]->() RETURN count(r) AS n';
log('GQL count(n)            = ' + await gqlCount(t, q1) + '   syntaxe : ' + q1);
log('GQL count(n:File)       = ' + await gqlCount(t, q2) + '   syntaxe : ' + q2);
log('GQL count(n:Function)   = ' + await gqlCount(t, q3) + '   syntaxe : ' + q3);
log('GQL count(CALLS) scan   = ' + await gqlCount(t, q4) + '   syntaxe : ' + q4);
log('latence count(n)        : ' + (await bench(() => t.execute(q1, null), 40)).toFixed(1) + ' µs (annoncé ~54)');
log('latence count(File)     : ' + (await bench(() => t.execute(q2, null), 40)).toFixed(1) + ' µs (annoncé ~39)');
log('latence count(CALLS)    : ' + (await bench(() => t.execute(q4, null), 10)).toFixed(0) + ' µs (annoncé ~20 000)');
// Fenêtres 2D exact vs sorted (égalité + latences) sur la plus grosse communauté
let best = [0, 0];
for (let cid = 1; cid <= 60000; cid++) { const n = t.windowQuery2dSorted(cid, 0, 65535, 0, 4294967295).length; if (n > best[1]) best = [cid, n]; }
const [cid] = best;
log('communauté testée : ' + best[0] + ' (' + best[1] + ' nœuds)');
const we = [0, 65535, 0, 4294967295];
const wh = [0, 65535, 50, 4294967295];
const eq = (a, b) => a.length === b.length && a.every((v, i) => v === b[i]);
const rE = t.windowQuery2d(cid, ...we); const rS = t.windowQuery2dSorted(cid, ...we);
const rEh = t.windowQuery2d(cid, ...wh); const rSh = t.windowQuery2dSorted(cid, ...wh);
log('window large exact==sorted : ' + eq(rE, rS) + ' (' + rE.length + ') ; hubs d≥50 exact==sorted : ' + eq(rEh, rSh));
const te = await bench(() => t.windowQuery2d(cid, ...wh), 100);
const ts = await bench(() => t.windowQuery2dSorted(cid, ...wh), 100);
log('fenêtre hubs d≥50 : scan ' + te.toFixed(1) + ' µs | zruns ' + ts.toFixed(1) + ' µs (égalité vérifiée)');
t.close();
// Lookup propriété (store props indexé précédemment)
if (existsSync(R + 'code-all.obrain')) {
  const p = ObrainDB.openReadOnly(R + 'code-all.obrain');
  const q5 = "MATCH (n:Method) WHERE n.qualified_name = 'obrain-engine.crates.obrain-substrate.src.hilbert.zorder_key_2d' RETURN n.id AS id";
  const lk = await bench(() => p.execute(q5, null), 30);
  log('lookup qualified_name (index) : ' + lk.toFixed(1) + ' µs (annoncé ~65)   syntaxe : ' + q5);
  p.close();
}
log('=== FIN VÉRIFICATION ===');
