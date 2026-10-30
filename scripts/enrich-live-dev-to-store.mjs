// Copie les props LIVE de runtime-dev vers runtime-full-v2 (par qn) pour les
// sessions listées par dev (titre, env, createdAt, last_imported_seq, …).
import { createRequire } from 'node:module';
const require = createRequire('/tmp/');
const { ObrainDB } = require('/tmp/rtprobe.node');
const dev = ObrainDB.openReadOnly('/home/triviere/.local/share/obrain-harness/data/runtime-dev.obrain');
const v2 = ObrainDB.open('/home/triviere/.local/share/obrain-harness/data/runtime-full-v2.obrain'); // RW
const KEYS = ['title', 'name', 'env', 'env_source', 'source', 'source_path', 'cwd', 'agentPreset', 'createdAt', 'last_imported_seq'];
async function loadProps(db, qn) {
  const res = await db.execute(`MATCH (n:Session) WHERE n.qualified_name = '${qn}' RETURN n LIMIT 1`, null);
  if (!res.length) return null;
  const n = res.get(0)['n'] ?? {};
  const out = {};
  for (const k of KEYS) if (n[k] !== undefined) out[k] = n[k];
  return { id: n['_id'], props: out };
}
(async () => {
  // sessions dev avec leurs props (payload sessions_list contient properties JSON)
  const list = await dev.execute('MATCH (n:Session) RETURN n LIMIT 4000', null); // 196 nœuds
  let copied = 0, updated = 0, miss = 0;
  for (let i = 0; i < list.length; i++) {
    const n = list.get(i)['n'] ?? {};
    const qn = n.qualified_name ?? n.name;
    if (!qn) continue;
    const src = await loadProps(dev, qn);
    if (!src) continue;
    const dst = await loadProps(v2, qn);
    if (!dst) { miss++; continue; }
    for (const [k, v] of Object.entries(src.props)) {
      if (dst.props[k] === undefined || dst.props[k] !== v) {
        const vt = typeof v === 'number' ? 'n' : 'b';
        v2.setNodeProperty(dst.id, k, vt === 'n' ? 'n' : (typeof v === 'boolean' ? 'b' : 's'), String(v));
        copied++;
      }
    }
    updated++;
  }
  v2.checkpoint(); v2.close(); dev.close();
  console.log(`enrichi: ${updated} sessions, ${copied} props copiées (${miss} sessions introuvables en v2)`);
})();
