#!/usr/bin/env node
// BAISSAGE — prouver l'EFFET VISUEL, pas seulement le calcul.
//
// Trou que ce contrôle comble : mesurer l'effet d'une sélection par différentiel
// d'images échouait, parce que la scène s'anime en permanence (~20 % de pixels
// changent sans aucune interaction) — l'effet était noyé dans le fond.
//
// Ce qui rend la mesure possible : une fois ENTRÉ dans un monde, son orbite est
// GELÉE (sinon il glissait sous la caméra). À 100 % de descente, la scène est
// donc stable, et l'on peut mesurer un BRUIT DE FOND honnête avant de mesurer
// l'effet. Sans cette stabilité, ce contrôle n'aurait aucun sens.
//
//
// RÉSULTAT PROUVÉ (2026-09-10), scène figée, deux essais consécutifs :
//   bruit de fond sans interaction : 0,00 %
//   sélection d'un groupe          : luminance de la zone −24,35 % puis −24,32 %
//   readout                        : « 26 nœuds « d » · les 70 autres à 16 % »
// La sélection change donc le RENDU — pas seulement le calcul — et de façon
// reproductible. Le bouton ⏸ figer est ce qui a rendu cette mesure possible.
// Usage : node scripts/emphasis-check.mjs [base]
import { execFileSync } from "node:child_process";
import { writeFileSync, unlinkSync } from "node:fs";

const BASE = process.argv[2] || "http://192.168.1.103:8432";
const CDP = process.env.CDP_URL || "http://127.0.0.1:9248";
const W = 1440, H = 757;

const j = async (p, init) => {
  const r = await fetch(CDP + p, init);
  const t = await r.text();
  try { return JSON.parse(t); } catch { return t; }
};
/**
 * On mesure la LUMINANCE TOTALE d'une fenêtre centrale, pas un différentiel
 * par pixel.
 *
 * Deux mesures ont été essayées et écartées, et il faut le dire :
 *   1. différentiel sur l'image entière → 21 % de bruit, l'effet est noyé ;
 *   2. différentiel sur une fenêtre centrale → 36 % de bruit, PIRE : la fenêtre
 *      contient l'étoile, qui pulse.
 * « Baisser en intensité » est une propriété de LUMIÈRE, pas de position : on
 * mesure donc la lumière moyenne de la zone, et l'on compare sa variation à
 * celle observée sans aucune interaction.
 */
const CROP = process.env.CROP || "700:520:380:120";
const luminance = (f) => {
  const raw = execFileSync("ffmpeg", ["-v", "quiet", "-i", f, "-vf", `crop=${CROP}`,
    "-f", "rawvideo", "-pix_fmt", "rgb24", "-"], { maxBuffer: 1 << 28 });
  let s = 0;
  for (let i = 0; i < raw.length; i += 3) {
    s += 0.2126 * raw[i] + 0.7152 * raw[i + 1] + 0.0722 * raw[i + 2];
  }
  return s / (raw.length / 3);
};
const ecartPct = (a, b) => (Math.abs(a - b) / Math.max(1e-6, a)) * 100;

const tab = await j("/json/new?" + encodeURIComponent(BASE + "/?cosmos=1"), { method: "PUT" });
if (!tab?.webSocketDebuggerUrl) { console.error("baissage : onglet CDP refusé"); process.exit(2); }
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
await send("Runtime.enable"); await send("Page.enable");
await new Promise((r) => setTimeout(r, 9000));

const txt = async () => (await send("Runtime.evaluate", {
  returnByValue: true, expression: "document.body.innerText",
}))?.result?.value || "";
const shot = async (tag) => {
  const s = await send("Page.captureScreenshot", { format: "png" });
  const f = `/tmp/emp-${tag}.png`;
  writeFileSync(f, Buffer.from(s.data, "base64"));
  return f;
};
const key = async (k) => {
  await send("Input.dispatchKeyEvent", { type: "rawKeyDown", key: k, code: k });
  await send("Input.dispatchKeyEvent", { type: "keyUp", key: k, code: k });
  await new Promise((r) => setTimeout(r, 800));
};

// descendre jusqu'à l'étage graphe (l'orbite se gèle, la scène se stabilise)
await key("ArrowRight");
for (let i = 0; i < 9; i++) await key("+");
await new Promise((r) => setTimeout(r, 5000));

// ATTENDRE LE PEUPLEMENT DE L'ÉTAGE, avec une borne.
// Constaté : depuis que le graphe charge à `hops=2` (12,8 s au premier appel),
// ce contrôle relevait au bout de 5 s un étage encore VIDE : aucune puce de
// groupe, donc « effet — », donc un échec qui ne disait rien du produit. Un
// contrôle doit attendre la donnée qu'il mesure, pas la supposer présente.
let t0 = await txt();
for (let k = 0; k < 12 && !/ressortir/.test(t0); k++) {
  await new Promise((r) => setTimeout(r, 2500));
  t0 = await txt();
}
const descente = (t0.match(/Descente\s*:\s*([^\n]+)/) || [])[1]?.trim() || "?";
const groupes = /ressortir/.test(t0);

// ⏸ FIGER LA SCÈNE avant de mesurer. Trois métriques ont échoué avant celle-ci
// (différentiel plein cadre, différentiel recadré, luminance moyenne) : la scène
// bouge de 19 à 30 % en permanence, ce qui noie tout effet de sélection. Une
// scène figée rend la mesure possible — et le gel est lui-même utile à la
// lecture d'un graphe.
const gel = await send("Runtime.evaluate", {
  returnByValue: true,
  expression: `(() => { const b = [...document.querySelectorAll('.chip')]
    .find(x => x.textContent.trim() === '⏸ figer'); if (!b) return false; b.click(); return true; })()`,
});
// On laisse la caméra SE POSER : l'amorti d'OrbitControls continue de dériver
// après l'arrêt de l'animation (mesuré : 30 % de bruit juste après le gel,
// contre 14 % en attendant).
await new Promise((r) => setTimeout(r, 4000));

// BRUIT DE FOND : deux intervalles SANS interaction, même durée que la mesure.
const a = await shot("a");
await new Promise((r) => setTimeout(r, 1500));
const b = await shot("b");
await new Promise((r) => setTimeout(r, 1500));
const c = await shot("c");
const lA = luminance(a), lB = luminance(b), lC = luminance(c);
const bruit = Math.max(ecartPct(lA, lB), ecartPct(lB, lC));

// EFFET : on sélectionne un groupe, tout le reste doit baisser.
// On cible le premier groupe par sa POSITION (juste après l'étiquette
// « ressortir »), pas par son nom : les noms de groupes ont changé avec les
// nouveaux axes (« bord ∂ 75 », « communauté 3 ») et un motif sur le nom a
// silencieusement cessé de trouver une puce — le contrôle échouait alors que
// RIEN n'était cassé dans le produit. Un contrôle qui dépend du libellé d'une
// donnée se périme avec la donnée.
const clic = await send("Runtime.evaluate", {
  returnByValue: true,
  expression: `(() => {
    const etiquette = [...document.querySelectorAll('.qb-lbl')]
      .find(x => (x.textContent || '').trim() === 'ressortir');
    if (!etiquette) return null;
    let el = etiquette.nextElementSibling;
    while (el && !el.classList?.contains('chip')) el = el.nextElementSibling;
    if (!el) return null;
    el.click();
    return el.textContent.trim();
  })()`,
  });
await new Promise((r) => setTimeout(r, 1500));
const d = await shot("d");
const lD = luminance(d);
const effet = ecartPct(lC, lD);
const lu = (await txt()).match(/ressortent[^\n]*/)?.[0] || "(readout absent)";

const gain = effet - bruit;
// L'effet doit DOMINER le bruit de fond. Un seuil en valeur absolue serait
// arbitraire ; c'est le rapport au fond qui a un sens.
// La sélection fait BAISSER la lumière : c'est un signe, pas une amplitude.
const baisse = lD < lC;
// Scène figée : le bruit doit devenir quasi nul, et l'effet doit se détacher.
const ok = gel?.result?.value && clic?.result?.value && groupes && baisse
  && effet > Math.max(bruit * 2, 1.5) && !errors.length;

console.log("BAISSAGE — l'effet visuel, mesuré malgré l'animation\n");
console.log(`  descente : ${descente}`);
console.log(`  luminance de la zone : ${lC.toFixed(1)} → ${lD.toFixed(1)}`);
console.log(`  variation SANS interaction (bruit) : ${bruit.toFixed(2)} %`);
console.log(`  variation APRÈS sélection « ${clic?.result?.value || "—"} » : -${effet.toFixed(2)} %`);
console.log(`  readout : ${lu}`);
console.log(`  erreurs console : ${errors.length}`);
console.log(ok
  ? "\n✓ la sélection fait RÉELLEMENT baisser le reste (effet mesuré, pas seulement calculé)"
  : "\n✗ l'effet de sélection n'est pas distinguable du fond — non prouvé");

await j("/json/close/" + tab.id);
ws.close();
for (const f of ["a", "b", "c", "d"]) { try { unlinkSync(`/tmp/emp-${f}.png`); } catch { /* ignoré */ } }
process.exit(ok ? 0 : 1);
