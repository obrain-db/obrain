#!/usr/bin/env node
// MOBILE — « les menus doivent pouvoir être rétractables ».
//
// On émule un téléphone (390×844, tactile), on charge la vue, et on mesure la
// place que les panneaux prennent RÉELLEMENT, dans les deux sens.
//
// Deux pièges rencontrés en écrivant ce contrôle, tous deux notés ici parce
// qu'ils invalideraient un test naïf :
//   1. le repli est MÉMORISÉ (localStorage) : un test qui ne part pas d'un état
//      vierge mesure l'état laissé par l'exécution précédente ;
//   2. le défaut mobile est REPLIÉ — un test qui suppose les panneaux ouverts
//      ne replie rien et conclut à l'échec à tort.
//
// Usage : node scripts/mobile-panels-check.mjs [base] [largeur] [hauteur]
const BASE = process.argv[2] || "http://192.168.1.103:8432";
const MW = Number(process.argv[3] || 390);
const MH = Number(process.argv[4] || 844);
const CDP = process.env.CDP_URL || "http://127.0.0.1:9248";

const j = async (p, init) => {
  const r = await fetch(CDP + p, init);
  const t = await r.text();
  try { return JSON.parse(t); } catch { return t; }
};

const tab = await j("/json/new?" + encodeURIComponent(BASE + "/?cosmos=1"), { method: "PUT" });
if (!tab?.webSocketDebuggerUrl) { console.error("mobile : onglet CDP refusé"); process.exit(2); }
const ws = new WebSocket(tab.webSocketDebuggerUrl);
let id = 0;
const pending = new Map();
const errors = [];
const send = (m, p = {}) => new Promise((r) => { const i = ++id; pending.set(i, r); ws.send(JSON.stringify({ id: i, method: m, params: p })); });
ws.addEventListener("message", (ev) => {
  const m = JSON.parse(ev.data);
  if (m.id && pending.has(m.id)) { pending.get(m.id)(m.result); pending.delete(m.id); return; }
  if (m.method === "Runtime.exceptionThrown") errors.push(m.params.exceptionDetails.text);
  if (m.method === "Log.entryAdded" && m.params.entry.level === "error") errors.push(m.params.entry.text);
});
await new Promise((r) => ws.addEventListener("open", r, { once: true }));
await send("Runtime.enable"); await send("Log.enable"); await send("Page.enable");
await send("Emulation.setDeviceMetricsOverride", {
  width: MW, height: MH, deviceScaleFactor: 2, mobile: true,
});
await send("Emulation.setTouchEmulationEnabled", { enabled: true, maxTouchPoints: 5 });
await send("Page.navigate", { url: BASE + "/?cosmos=1" });
await new Promise((r) => setTimeout(r, 4000));

// ÉTAT VIERGE : on efface la mémoire de repli AVANT de mesurer, sinon on mesure
// ce que l'exécution précédente a laissé.
await send("Runtime.evaluate", {
  expression: `(() => { for (const k of Object.keys(localStorage)) {
    if (k.indexOf('obrain.panel.') === 0) localStorage.removeItem(k); } return true; })()`,
});
await send("Page.reload");
await new Promise((r) => setTimeout(r, 9000));

/** Occupation des panneaux + atteignabilité des boutons de repli. */
const mesure = async () => (await send("Runtime.evaluate", {
  returnByValue: true,
  expression: `(() => {
    const vp = innerWidth * innerHeight;
    const visibles = [...document.querySelectorAll('.ov-panel')].filter(p => {
      const r = p.getBoundingClientRect();
      return r.width > 2 && r.height > 2 && getComputedStyle(p).display !== 'none';
    });
    let aire = 0;
    for (const p of visibles) {
      const r = p.getBoundingClientRect();
      aire += Math.max(0, Math.min(r.right, innerWidth) - Math.max(r.left, 0)) *
              Math.max(0, Math.min(r.bottom, innerHeight) - Math.max(r.top, 0));
    }
    const replis = [...document.querySelectorAll('.ov-panel-repli')].filter(b => {
      const r = b.getBoundingClientRect();
      return r.width >= 24 && r.height >= 24 && getComputedStyle(b).display !== 'none';
    });
    return {
      vw: innerWidth, vh: innerHeight,
      panneaux: visibles.length,
      replis: replis.length,
      airePct: Math.round(aire * 100 / vp),
    };
  })()`,
}))?.result?.value || {};

const boutons = `[...document.querySelectorAll('.ov-panel-repli')]`;
const cliquer = async (etat) => (await send("Runtime.evaluate", {
  returnByValue: true,
  expression: `(() => { let n = 0; for (const b of ${boutons}) {
    if (b.getAttribute('aria-expanded') === '${etat}') { b.click(); n++; } } return n; })()`,
}))?.result?.value || 0;

const replie = await mesure();                        // état par défaut (mobile)
const nDeplies = await cliquer("false");
await new Promise((r) => setTimeout(r, 900));
const deplie = await mesure();
const nReplies = await cliquer("true");
await new Promise((r) => setTimeout(r, 900));
const revenu = await mesure();

const atteignable = replie.replis >= 1;
const deplierCoute = deplie.airePct > replie.airePct + 5;
const replierRend = revenu.airePct < deplie.airePct - 5;
const retourInitial = Math.abs(revenu.airePct - replie.airePct) <= 8;
const respire = replie.airePct <= 40;
const ok = atteignable && deplierCoute && replierRend && retourInitial && respire && !errors.length;

console.log(`MOBILE ${MW}×${MH} — les menus doivent être rétractables\n`);
console.log(`  panneaux visibles : ${replie.panneaux} · boutons de repli atteignables (≥ 24 px) : ${replie.replis}`);
console.log(`  replié au départ (défaut mobile) : ${replie.airePct} % de l'écran`);
console.log(`  déplié : ${deplie.airePct} % (${nDeplies} déplié(s))`);
console.log(`  re-replié : ${revenu.airePct} % (${nReplies} replié(s))`);
console.log(`  erreurs console : ${errors.length}`);
console.log(
  `\n  bouton atteignable : ${atteignable ? "✓" : "✗"} · déplier COÛTE de la place : ${deplierCoute ? "✓" : "✗"} · ` +
  `replier la REND : ${replierRend ? "✓" : "✗"} · retour à l'état initial : ${retourInitial ? "✓" : "✗"} · ` +
  `canvas majoritaire replié (≤ 40 %) : ${respire ? "✓" : "✗"}`,
);
console.log(ok
  ? "\n✓ les menus sont rétractables sur mobile, et repliés par défaut"
  : "\n✗ les menus ne sont pas rétractables sur mobile");

await j("/json/close/" + tab.id);
ws.close();
process.exit(ok ? 0 : 1);
