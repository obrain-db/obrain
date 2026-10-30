#!/usr/bin/env node
// ACTIVATION EN DIRECT — « le graph devrait s'activer en temps réel dès
// consultation de l'engine ».
//
// Ce contrôle reproduit la chaîne réelle : on ouvre le dashboard, PUIS on fait
// consulter l'engine par un AUTRE processus (un appel HTTP au serveur viz, comme
// le ferait le serveur MCP ou un LLM), et on exige que le dashboard le dise —
// sans rechargement de page.
//
// C'est la seule façon de vérifier la propriété demandée : un test qui
// déclencherait la consultation DEPUIS la page mesurerait la page, pas le lien
// entre l'engine et l'affichage. Le journal d'activation étant un fichier
// partagé (voir crates/obrain-cli/src/activation.rs), un processus séparé peut
// légitimement l'alimenter.
//
// Usage : node scripts/activation-live-check.mjs [base] [store]
import { execFileSync } from "node:child_process";
import { writeFileSync, unlinkSync } from "node:fs";

const BASE = process.argv[2] || "http://192.168.1.103:8432";
const STORE = process.argv[3] ||
  "/home/triviere/.local/share/obrain-harness/data/runtime-dev.obrain";
const CDP = process.env.CDP_URL || "http://127.0.0.1:9248";
const W = 1440, H = 757;

const j = async (p, init) => {
  const r = await fetch(CDP + p, init);
  const t = await r.text();
  try { return JSON.parse(t); } catch { return t; }
};

// `?cosmos=1` : la face Cosmos est celle qui affiche l'énergie PAR MONDE.
const tab = await j("/json/new?" + encodeURIComponent(BASE + "/?cosmos=1"), { method: "PUT" });
if (!tab?.webSocketDebuggerUrl) {
  console.error("activation : onglet CDP refusé");
  process.exit(2);
}
const ws = new WebSocket(tab.webSocketDebuggerUrl);
let id = 0;
const pending = new Map();
const errors = [];
const send = (m, p = {}) => new Promise((r) => { const i = ++id; pending.set(i, r); ws.send(JSON.stringify({ id: i, method: m, params: p })); });
ws.addEventListener("message", (ev) => {
  const m = JSON.parse(ev.data);
  if (m.id && pending.has(m.id)) { pending.get(m.id)(m.result); pending.delete(m.id); return; }
  if (m.method === "Runtime.exceptionThrown") errors.push(m.params.exceptionDetails.text);
});
await new Promise((r) => ws.addEventListener("open", r, { once: true }));
await send("Runtime.enable"); await send("Log.enable"); await send("Page.enable");
await new Promise((r) => setTimeout(r, 9000)); // chargement + scène + 1er sondage

const txt = async () => (await send("Runtime.evaluate", {
  returnByValue: true, expression: "document.body.innerText",
}))?.result?.value || "";

const ligneEngine = (t) => (t.match(/Engine\s*:\s*([^\n]+)/) || [])[1]?.trim() || "";
const avant = ligneEngine(await txt());

// On attend que le journal retombe (les consultations précédentes s'éteignent).
await new Promise((r) => setTimeout(r, 3000));

const shot = async (f) => {
  const s = await send("Page.captureScreenshot", { format: "png" });
  writeFileSync(f, Buffer.from(s.data, "base64"));
  return f;
};
const A = await shot("/tmp/act-a.png");

// ── CONSULTATION DEPUIS UN AUTRE PROCESSUS (curl = un vrai client engine) ──
const qs = new URLSearchParams({
  store: STORE,
  center: "self:agent:state:v72",
  hops: "1",
  limit: "30",
});
const t0 = Date.now();
const r = await fetch(`${BASE}/api/viz/subgraph?${qs}`);
const msReponse = Date.now() - t0;

await new Promise((r2) => setTimeout(r2, 3000)); // ≥ 2 sondages (1,2 s)
const apres = ligneEngine(await txt());
const B = await shot("/tmp/act-b.png");

// Le monde consulté doit aussi s'allumer : mesure du changement de pixels.
let pct = 0;
try {
  const dec = (f) => execFileSync("ffmpeg", ["-v", "quiet", "-i", f, "-f", "rawvideo",
    "-pix_fmt", "rgb24", "-s", `${W}x${H}`, "-"], { maxBuffer: 1 << 28 });
  const X = dec(A), Y = dec(B);
  let n = 0;
  for (let i = 0; i < X.length; i += 3) {
    if (Math.abs(X[i] - Y[i]) + Math.abs(X[i + 1] - Y[i + 1]) + Math.abs(X[i + 2] - Y[i + 2]) > 24) n++;
  }
  pct = (n * 100) / (X.length / 3);
} catch { /* ffmpeg absent : mesure informative seulement */ }

const ok = !!apres && apres !== avant && r.ok && !errors.length;
console.log("ACTIVATION EN DIRECT — l'engine consulte, le dashboard doit le dire\n");
console.log(`  consultation externe : HTTP ${r.status} en ${msReponse} ms`);
console.log(`  avant : ${avant || "(rien)"}`);
console.log(`  après : ${apres || "(rien)"}`);
console.log(`  pixels modifiés (allumage du monde) : ${pct.toFixed(2)} %`);
console.log(`  erreurs console : ${errors.length}`);
console.log(`\n${ok ? "✓ le graphe s'active en direct dès consultation de l'engine" : "✗ l'activation n'est pas remontée à l'affichage"}`);

await j("/json/close/" + tab.id);
ws.close();
for (const f of [A, B]) { try { unlinkSync(f); } catch { /* ignoré */ } }
process.exit(ok ? 0 : 1);
