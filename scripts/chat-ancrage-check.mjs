#!/usr/bin/env node
// CHAT — L'ANCRAGE VÉRIFIÉ DANS LE NAVIGATEUR RÉEL.
//
// Défaut reproduit le 2026-09-11 : le chat annonçait « ancré sur this-rs/this »
// et lisait EN FAIT runtime-dev. La réponse citait `dsh-runtime`,
// `session-c254ac35…:msg:7`, `file:/tmp/codebase-memory-mcp/README.md` — les 24
// premiers nœuds de runtime-dev, alors que `this-rs/this` contient 1 500
// `Function` et 1 175 `Class`.
//
// Ce contrôle fait le GESTE de l'utilisateur, pas un appel d'API :
//   1. il ouvre le Cosmos ;
//   2. il parcourt les mondes au CLAVIER (←/→, la grammaire commune) jusqu'à
//      viser `this-rs/this` — un monde AJOUTÉ À CHAUD, donc absent de la base
//      synaptique : c'est précisément le cas qui échouait ;
//   3. il ouvre la conversation par le bouton flottant ;
//   4. il écrit la question et l'envoie ;
//   5. il lit ce que l'EN-TÊTE et le CONTEXTE affichent, et exige que le store
//      ouvert soit celui du sujet.
//
// Un contrôle qui piloterait l'endpoint directement ne testerait pas le défaut :
// la cause était dans la VUE (un libellé comparé à un qn, puis un repli sur le
// premier monde de la liste).
//
// Usage : node scripts/chat-ancrage-check.mjs [base] [limite-secondes]
import { mkdirSync, writeFileSync } from "node:fs";

const BASE = process.argv[2] || "http://192.168.1.103:8432";
const LIMITE = Number(process.argv[3] || 90);
const CDP = process.env.CDP_URL || "http://127.0.0.1:9248";
// Le monde visé : « this-rs/this (GitHub) », le cas même du défaut.
const MONDE = "this-rs/this";
const STORE_ATTENDU = "this-rs-this.obrain";

const j = async (p, init) => {
  const r = await fetch(CDP + p, init);
  const t = await r.text();
  try { return JSON.parse(t); } catch { return t; }
};

const sante = await (await fetch(`${BASE}/api/engine/sante`)).json();
if (!sante.disponible) {
  console.error(`chat : service LLM indisponible (${sante.url}) — ${sante.raison || ""}`);
  process.exit(2);
}
console.log(`service LLM : ${sante.modele}`);

const tab = await j("/json/new?" + encodeURIComponent(BASE + "/?cosmos=1"), { method: "PUT" });
if (!tab?.webSocketDebuggerUrl) { console.error("chat : onglet CDP refusé"); process.exit(2); }
const ws = new WebSocket(tab.webSocketDebuggerUrl);
let id = 0;
const pending = new Map();
const errors = [];
const send = (m, p = {}) => new Promise((r) => {
  const i = ++id; pending.set(i, r); ws.send(JSON.stringify({ id: i, method: m, params: p }));
});
ws.addEventListener("message", (ev) => {
  const m = JSON.parse(ev.data);
  if (m.id && pending.has(m.id)) { pending.get(m.id)(m.result); pending.delete(m.id); return; }
  if (m.method === "Runtime.exceptionThrown") errors.push(m.params.exceptionDetails.text);
});
await new Promise((r) => ws.addEventListener("open", r, { once: true }));
await send("Runtime.enable"); await send("Log.enable"); await send("Page.enable");
await new Promise((r) => setTimeout(r, 10000));

const txt = async () => (await send("Runtime.evaluate", {
  returnByValue: true, expression: "document.body.innerText",
}))?.result?.value || "";

const touche = async (k, code) => {
  await send("Input.dispatchKeyEvent", { type: "rawKeyDown", key: k, code });
  await send("Input.dispatchKeyEvent", { type: "keyUp", key: k, code });
  await new Promise((r) => setTimeout(r, 900));
};

// ── 2) viser le monde AU CLAVIER ─────────────────────────────────────────────
let sujet = "";
for (let i = 0; i < 16; i++) {
  const t = await txt();
  sujet = t.match(/Sujet\s*:\s*([^\n—]+)/)?.[1]?.trim() || "";
  if (sujet.includes(MONDE)) break;
  await touche("ArrowRight", "ArrowRight");
}
sujet = (await txt()).match(/Sujet\s*:\s*([^\n—]+)/)?.[1]?.trim() || sujet;
const mondeVise = sujet.includes(MONDE);
console.log(`  sujet visé au clavier : ${sujet || "(aucun)"}`);

// ── 3) ouvrir la conversation par le bouton flottant ─────────────────────────
const ouvert = await send("Runtime.evaluate", {
  returnByValue: true,
  expression: `(() => { const b = document.querySelector('.chat-flottant');
    if (!b) return false; b.click(); return true; })()`,
});
await new Promise((r) => setTimeout(r, 800));
const chatOuvert = (await txt()).includes("Chat — l'engine sur le graphe");

// ── 4) écrire la question et l'envoyer ───────────────────────────────────────
await send("Runtime.evaluate", {
  expression: `(() => { const i = document.querySelector('.chat-pied textarea');
    if (!i) return false; i.focus();
    const set = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value').set;
    set.call(i, "Parle moi de ce projet");
    i.dispatchEvent(new Event('input', { bubbles: true }));
    i.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    return true; })()`,
});

// ── 5) lire l'ancrage RÉELLEMENT affiché ─────────────────────────────────────
// L'événement `contexte` arrive AVANT l'appel au modèle : l'ancrage est visible
// en quelques secondes, la réponse peut prendre une minute. On n'attend donc que
// lui — c'est lui qui porte le défaut corrigé.
let t = "";
const t0 = Date.now();
while ((Date.now() - t0) / 1000 < LIMITE) {
  await new Promise((r) => setTimeout(r, 2000));
  t = await txt();
  if (t.includes("store :")) break;
}
const enTete = t.split("\n").find((l) => l.includes("ancré sur")) || "";
const ancrage = t.split("\n").find((l) => l.includes("store :")) || "";
const partLue = t.split("\n").find((l) => /\/\s*\d+\s*nœuds/.test(l)) || "";

// Le défaut : l'en-tête nomme le sujet, le contexte lit runtime-dev.
const bonStore = ancrage.includes(STORE_ATTENDU);
const mauvaisStore = ancrage.includes("runtime-dev");
const ancrageVisible = ancrage.length > 0;
const partHonnete = /\d+\s*\/\s*\d+\s*nœuds/.test(partLue);

console.log(`  conversation ouverte : ${chatOuvert ? "oui" : "non"} (bouton flottant : ${ouvert?.result?.value ? "cliqué" : "ABSENT"})`);
console.log(`  en-tête   : ${enTete.trim() || "(aucun)"}`);
console.log(`  ancrage   : ${ancrage.trim() || "(aucun)"}`);
console.log(`  part lue  : ${partLue.trim() || "(aucune)"}`);
console.log(`  erreurs console : ${errors.length}`);

const ok = mondeVise && chatOuvert && ancrageVisible && bonStore && !mauvaisStore && partHonnete && !errors.length;
console.log(ok
  ? `\n✓ le chat lit le store du SUJET (${STORE_ATTENDU}) et annonce la part réellement lue`
  : `\n✗ l'ancrage du chat ne correspond pas au sujet visé`);

// ── preuve visuelle ──────────────────────────────────────────────────────────
try {
  const shot = await send("Page.captureScreenshot", { format: "png" });
  if (shot?.data) {
    mkdirSync("docs/perf", { recursive: true });
    const p = "docs/perf/chat-ancrage.png";
    writeFileSync(p, Buffer.from(shot.data, "base64"));
    console.log(`  capture : ${p}`);
  }
} catch { /* la capture n'est pas le contrôle */ }

await j("/json/close/" + tab.id);
ws.close();
process.exit(ok ? 0 : 1);
