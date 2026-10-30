#!/usr/bin/env node
// ENGINE — un LLM qui raisonne sur le graphe, à partir de ce que l'utilisateur
// vise, et le graphe qui s'allume pendant qu'il réfléchit.
//
// Vérifie la chaîne complète, dans l'ordre où elle se produit :
//   1. le SUJET est posé par la grammaire commune (←/→ dans le Cosmos) ;
//   2. la question part de ce sujet, sans ressaisie (point 3 de l'objectif) ;
//   3. PENDANT la réflexion du modèle, le monde visé est allumé (point 4) ;
//   4. la réponse s'affiche avec son contexte réel et ses latences réelles.
//
// Ce contrôle est LENT par nature : le modèle local met 20 à 75 s. On le borne,
// et on échoue franchement si rien n'arrive plutôt que d'attendre sans fin.
//
// Usage : node scripts/engine-ask-check.mjs [base] [limite-secondes]
const BASE = process.argv[2] || "http://192.168.1.103:8432";
const LIMITE = Number(process.argv[3] || 150);
const CDP = process.env.CDP_URL || "http://127.0.0.1:9248";

const j = async (p, init) => {
  const r = await fetch(CDP + p, init);
  const t = await r.text();
  try { return JSON.parse(t); } catch { return t; }
};

// 0) le service LLM répond-il ? (sinon, inutile d'aller plus loin)
const sante = await (await fetch(`${BASE}/api/engine/sante`)).json();
if (!sante.disponible) {
  console.error(`engine : service LLM indisponible (${sante.url}) — ${sante.raison || ""}`);
  process.exit(2);
}
console.log(`service LLM : ${sante.modele}`);

const tab = await j("/json/new?" + encodeURIComponent(BASE + "/?cosmos=1"), { method: "PUT" });
if (!tab?.webSocketDebuggerUrl) { console.error("engine : onglet CDP refusé"); process.exit(2); }
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
await new Promise((r) => setTimeout(r, 9000));

const txt = async () => (await send("Runtime.evaluate", {
  returnByValue: true, expression: "document.body.innerText",
}))?.result?.value || "";

// 1) poser le sujet par la grammaire commune
for (const k of ["ArrowRight"]) {
  await send("Input.dispatchKeyEvent", { type: "rawKeyDown", key: k, code: k });
  await send("Input.dispatchKeyEvent", { type: "keyUp", key: k, code: k });
}
await new Promise((r) => setTimeout(r, 1500));
const sujet = (await txt()).match(/Sujet\s*:\s*([^\n—]+)/)?.[1]?.trim() || "";
const zone = (await txt()).includes("un LLM qui raisonne sur le graphe");

// 2) écrire la question et demander (sans ressaisir le contexte)
await send("Runtime.evaluate", {
  expression: `(() => { const i = document.querySelector('input[aria-label="Question à l\\'engine"]');
    if (!i) return false; i.focus();
    const set = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
    set.call(i, "En une phrase : que contient ce sujet ?");
    i.dispatchEvent(new Event('input', { bubbles: true })); return true; })()`,
});
await new Promise((r) => setTimeout(r, 400));
await send("Runtime.evaluate", {
  expression: `[...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'Demander')?.click()`,
});

// 3) PENDANT la réflexion, le monde visé doit être allumé
await new Promise((r) => setTimeout(r, 6000));
const act = await (await fetch(`${BASE}/api/viz/activation?window=60000`)).json();
const allumes = (act.worlds || []).filter((w) => w.peak > 0);
const pendant = allumes.map((w) => `${w.store.split("/").pop()}=${w.peak}`).join(", ");

// 4) attendre la réponse, borné
let t = "";
const t0 = Date.now();
while ((Date.now() - t0) / 1000 < LIMITE) {
  await new Promise((r) => setTimeout(r, 4000));
  t = await txt();
  if (/nœuds \+ \d+ relations lus en/.test(t)) break;
}
const m = t.match(/(\d+) nœuds \+ (\d+) relations lus en (\d+) ms · réponse en ([\d.]+) s/);

const ok = !!sujet && zone && allumes.length > 0 && !!m && !errors.length;
console.log(`\n  sujet posé par la grammaire : ${sujet || "(aucun)"}`);
console.log(`  zone engine présente : ${zone ? "oui" : "non"}`);
console.log(`  PENDANT la réflexion, mondes allumés : ${pendant || "AUCUN"}`);
console.log(`  réponse : ${m ? `${m[1]} nœuds + ${m[2]} relations, moteur ${m[3]} ms, LLM ${m[4]} s` : "AUCUNE"}`);
console.log(`  erreurs console : ${errors.length}`);
console.log(ok
  ? "\n✓ l'engine raisonne sur le sujet visé, et le graphe s'allume pendant qu'il réfléchit"
  : "\n✗ la chaîne engine → graphe → affichage n'est pas complète");

await j("/json/close/" + tab.id);
ws.close();
process.exit(ok ? 0 : 1);
