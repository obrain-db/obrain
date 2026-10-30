// Compteurs O(1) — protocole canonique (process dédié, aucun autre appel napi
// préalable, N=20000) : reproduit les annonces 0,20 µs / 0,14 µs.
import { createRequire } from 'node:module';
const require = createRequire('/tmp/');
const { ObrainDB } = require('/tmp/rtprobe.node');
const db = ObrainDB.openReadOnly('/home/triviere/projects/banc-obrain/target/bench/real/code-all3.obrain');
const N = 20000;
for (let i = 0; i < 5; i++) db.edgeCountByType('CALLS');
let t0 = process.hrtime.bigint();
let v = 0;
for (let i = 0; i < N; i++) v = db.edgeCountByType('CALLS');
console.log('edgeCountByType(CALLS) = ' + v + '  O(1) ' + (Number(process.hrtime.bigint() - t0) / 1000 / N).toFixed(2) + ' µs (annoncé 0,20)');
for (let i = 0; i < 5; i++) db.nodeCountByLabel('File');
t0 = process.hrtime.bigint();
for (let i = 0; i < N; i++) v = db.nodeCountByLabel('File');
console.log('nodeCountByLabel(File) = ' + v + '  O(1) ' + (Number(process.hrtime.bigint() - t0) / 1000 / N).toFixed(2) + ' µs (annoncé 0,14)');
db.close();
