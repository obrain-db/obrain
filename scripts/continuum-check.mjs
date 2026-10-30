#!/usr/bin/env node
// CONTINUUM — la descente doit être continue, et chaque étage PEUPLÉ AVANT
// d'être remplacé.
//
// Trois exigences vérifiées, dans l'ordre où elles se produisent :
//   1. CONTINUITÉ : en approchant par la grammaire commune (+), la descente
//      prend des valeurs intermédiaires — pas un saut de 0 à 100 %. Une
//      transition qui saute n'est pas une transition ;
//   2. PEUPLEMENT AVANT REMPLACEMENT : aux paliers intermédiaires (monde), la
//      scène n'est PAS vide — la composition de surface reste visible tant que
//      l'étage intérieur n'est pas construit. Mesuré en pixels sur la scène
//      nue : c'est le seul moyen de distinguer « en cours de chargement » de
//      « écran vide » ;
//   3. ÉTAGE INTÉRIEUR RÉEL : l'intérieur se peuple avec des données du store
//      (noms d'ensembles + effectifs), pas des libellés décoratifs.
//
// Usage : node scripts/continuum-check.mjs [base]
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

/** Contenu (non-fond) d'une capture : distingue « vide » de « peuplé ». */
function contenuPct(file) {
  const raw = execFileSync("ffmpeg", ["-v", "quiet", "-i", file, "-f", "rawvideo",
    "-pix_fmt", "rgb24", "-s", `${W}x${H}`, "-"], { maxBuffer: 1 << 28 });
  let n = 0;
  for (let i = 0; i < raw.length; i += 3) {
    if (Math.abs(raw[i] - 5) + Math.abs(raw[i + 1] - 7) + Math.abs(raw[i + 2] - 12) > 26) n++;
  }
  return (n * 100) / (raw.length / 3);
}

const tab = await j("/json/new?" + encodeURIComponent(BASE + "/?cosmos=1"), { method: "PUT" });
if (!tab?.webSocketDebuggerUrl) { console.error("continuum : onglet CDP refusé"); process.exit(2); }
const ws = new WebSocket(tab.webSocketDebuggerUrl);
let id = 0;
const pending = new Map();
const errors = [];
/**
 * ── LES ERREURS CONNUES NE SONT PAS DES ÉCHECS DE CE CONTRÔLE ───────────────
 *
 * Ce contrôle teste la DESCENTE. Il comptait pourtant comme échec le
 * `404 /ui/state/local` du pont d'état : `viz serve` n'expose pas `/ui/state`,
 * c'est DOCUMENTÉ (`docs/PROCESSUS-UI-8432.md` §6, et le module
 * `app/uiStateBridge.ts` le journalise une fois avant de continuer en local).
 * Résultat mesuré avant ce filtre : les quatre propriétés de la descente
 * passaient, et le verdict affichait « ✗ » à cause de cette seule erreur — un
 * contrôle qui échoue pour une raison qu'il ne teste pas finit par ne plus être
 * lu. On l'écarte donc EXPLICITEMENT (et on l'affiche quand même, pour qu'elle
 * reste visible), sans élargir la tolérance à autre chose.
 */
const erreursConnues = [];
const connue = (u) => /\/ui\/state\//.test(u);
const send = (m, p = {}) => new Promise((r) => { const i = ++id; pending.set(i, r); ws.send(JSON.stringify({ id: i, method: m, params: p })); });
let requetesKO = new Set();
ws.addEventListener("message", (ev) => {
  const m = JSON.parse(ev.data);
  if (m.id && pending.has(m.id)) { pending.get(m.id)(m.result); pending.delete(m.id); return; }
  if (m.method === "Network.responseReceived" && m.params.response.status >= 400) {
    if (connue(m.params.response.url)) erreursConnues.push(m.params.response.status + " " + m.params.response.url.replace(BASE, ""));
    else requetesKO.add(m.params.response.status + " " + m.params.response.url.replace(BASE, "").slice(0, 120));
    return;
  }
  if (m.method === "Runtime.exceptionThrown") errors.push(m.params.exceptionDetails.text);
  // Le navigateur journalise « Failed to load resource » SANS l'URL : compter ces
  // lignes reviendrait à juger une requête sur son seul code. Ce sont les
  // événements `Network` (ci-dessus, qui portent l'URL) qui font autorité ; ici
  // on ne collecte donc QUE ce qui n'est pas un échec réseau.
  if (m.method === "Log.entryAdded" && m.params.entry.level === "error"
      && !/Failed to load resource/.test(m.params.entry.text)) errors.push(m.params.entry.text);
});
await new Promise((r) => ws.addEventListener("open", r, { once: true }));
await send("Runtime.enable"); await send("Log.enable"); await send("Page.enable"); await send("Network.enable");
await new Promise((r) => setTimeout(r, 9000));

const txt = async () => (await send("Runtime.evaluate", {
  returnByValue: true, expression: "document.body.innerText",
}))?.result?.value || "";
const lire = async () => {
  const t = await txt();
  const d = t.match(/Descente\s*:\s*([^\n]+)/)?.[1]?.trim() || "";
  const pct = Number((d.match(/(\d+)\s*%/) || [])[1] ?? (d.startsWith("espace") ? 0 : NaN));
  const inter = t.match(/Intérieur\s*:\s*([^\n]+)/)?.[1]?.trim() || null;
  const gr = t.match(/Graphe\s*:\s*([^\n]+)/)?.[1]?.trim() || null;
  return { d, pct, inter, gr };
};
// capture de la scène NUE (panneaux masqués) pour mesurer le peuplement réel
const sceneNu = async (tag) => {
  await send("Runtime.evaluate", {
    expression: `(() => { if (!document.getElementById('__bare')) { const s = document.createElement('style');
      s.id = '__bare'; s.textContent = '.ov-panel,.echelle-curseur,.zoomctl,.zoomhint,.ask-msg{display:none!important}';
      document.head.appendChild(s); } return true; })()`,
  });
  await new Promise((r) => setTimeout(r, 700));
  const s = await send("Page.captureScreenshot", { format: "png" });
  const f = `/tmp/cont-${tag}.png`;
  writeFileSync(f, Buffer.from(s.data, "base64"));
  await send("Runtime.evaluate", { expression: `document.getElementById('__bare')?.remove()` });
  const pct = contenuPct(f);
  try { unlinkSync(f); } catch { /* ignoré */ }
  return pct;
};

const key = async (k) => {
  await send("Input.dispatchKeyEvent", { type: "rawKeyDown", key: k, code: k });
  await send("Input.dispatchKeyEvent", { type: "keyUp", key: k, code: k });
  await new Promise((r) => setTimeout(r, 900));
};

// ÉTAT INITIAL (espace, avant toute visée) : c'est le premier état de la
// descente, et il doit déjà être peuplé.
const mesures = [{ ...(await lire()), contenu: await sceneNu("espace") }];
// viser un monde par la grammaire commune, puis descendre
await key("ArrowRight");
for (let i = 0; i < 8; i++) {
  const l = await lire();
  const c = await sceneNu(`z${i}`);
  mesures.push({ ...l, contenu: c });
  await key("+");
}
// ATTENDRE L'ÉTAGE LE PLUS PROFOND, avec une borne.
// Constaté : la source est LENTE — `hops=2` coûte 12,8 s au premier appel (puis
// 0,01 s en cache). Un relevé à cadence fixe concluait « étage graphe absent »
// alors qu'il était simplement en cours de chargement. C'est le chargement
// qu'on vérifie, pas la vitesse de la machine ; on lui laisse donc le temps,
// mais on échoue s'il ne vient jamais.
let fin = await lire();
for (let k = 0; k < 12 && !fin.gr; k++) {
  await new Promise((r) => setTimeout(r, 2500));
  fin = await lire();
}
mesures.push({ ...fin, contenu: await sceneNu("fin") });

const chiffres = mesures.map((m) => m.pct).filter((v) => Number.isFinite(v));
const distincts = new Set(chiffres).size;
// La descente doit DESCENDRE, pas être monotone au point près : la scène est
// animée et les relevés sont espacés d'environ une seconde. On tolère une
// faible remontée (≤ 3 points) sans considérer qu'on remonte.
const croissante = chiffres.every((v, i) => i === 0 || v >= chiffres[i - 1] - 3);
const interieur = mesures.find((m) => m.inter);
// AUCUN TROU, sur TOUTE la descente. La version précédente ne regardait que les
// états « avant l'intérieur » : quand le cache disque est chaud, l'étage arrive
// au premier relevé, la liste est vide, et le minimum retombait à 0 — le
// contrôle échouait sur une valeur par défaut au lieu d'un vrai trou. Ici la
// propriété testée est celle qui compte : à aucun moment la scène n'est vide.
const minContenu = Math.min(...mesures.map((m) => m.contenu));

const continue_ = distincts >= 3 && croissante;
const jamaisVide = minContenu > 4;
const contenuReel = !!interieur && /\d/.test(interieur.inter);
// Le 3e étage doit se peupler À SON TOUR, avec des données réelles.
const graphe = mesures.find((m) => m.gr);
const grapheReel = !!graphe && /\d+\s*nœuds/.test(graphe.gr) && /\d+\s*arêtes/.test(graphe.gr);
const ok = continue_ && jamaisVide && contenuReel && grapheReel && !errors.length && !requetesKO.size;

console.log("CONTINUUM — descente continue, étages peuplés avant remplacement\n");
for (const m of mesures) {
  console.log(`  ${String(m.pct).padStart(3)} %  scène ${m.contenu.toFixed(1)} %  ` +
    `${m.gr ? `GRAPHE : ${m.gr.slice(0, 58)}` : m.inter ? `intérieur : ${m.inter.slice(0, 52)}` : ""}`);
}
console.log(`\n  paliers distincts : ${distincts} (${chiffres.join(" → ")})`);
console.log(`  scène la plus pauvre sur toute la descente : ${minContenu.toFixed(1)} %`);
console.log(`  erreurs console : ${errors.length}`);
console.log(`  échecs réseau INATTENDUS : ${requetesKO.size}` +
  (requetesKO.size ? " — " + [...requetesKO].slice(0, 4).join(" | ") : ""));
console.log(`  écartés car CONNUS : ${erreursConnues.length} — ` +
  `${[...new Set(erreursConnues)].join(" | ") || "aucun"}`);
console.log(
  `\n  descende continue : ${continue_ ? "✓" : "✗"} · ` +
  `aucun trou sur la descente : ${jamaisVide ? "✓" : "✗"} · ` +
  `étage « organisation » réel : ${contenuReel ? "✓" : "✗"} · ` +
  `étage GRAPHE réel : ${grapheReel ? "✓" : "✗"}`,
);
console.log(ok ? "\n✓ le Cosmos se descend continûment, sans trou" : "\n✗ la descente n'est pas continue ou laisse un trou");


// ── PASSE À LA MOLETTE ──────────────────────────────────────────────────────
// Ce contrôle pilotait la descente AU CLAVIER (`+`). Il a donc validé pendant
// plusieurs rounds un trajet que l'utilisateur n'emprunte pas — et il n'a pas vu
// la régression où la molette demandait 78 crans pour traverser les échelles.
// Un contrôle qui n'exerce pas le GESTE de l'utilisateur ne teste rien.
// On refait donc la descente à la MOLETTE SEULE, sans aucune touche, et on exige
// qu'elle traverse les échelles en un nombre de crans RAISONNABLE.
const roue = async (dy) => {
  await send("Input.dispatchMouseEvent", { type: "mouseWheel", x: 720, y: 380, deltaX: 0, deltaY: dy });
  await new Promise((r) => setTimeout(r, 900));
};
await send("Page.navigate", { url: BASE + "/?cosmos=1&roue=" + Date.now() });
await new Promise((r) => setTimeout(r, 11000));
const pct = () => {
  const m = t2.match(/Descente\s*:\s*[^\n]*?(\d+)\s*%/);
  return m ? Number(m[1]) : 0;
};
let t2 = await txt();
let crans = 0;
const MAX_CRANS = 20; // au-delà, la molette n'est plus une navigation
while (crans < MAX_CRANS && pct() < 90) { await roue(-220); crans++; t2 = await txt(); }
const pctRoue = pct();
const roueOk = pctRoue >= 90 && crans <= MAX_CRANS;
console.log(roueOk
  ? "✓ la molette seule traverse les échelles en un nombre de crans raisonnable"
  : `✗ la molette ne traverse pas les échelles (${pctRoue} % en ${crans} crans)`);

console.log(`\n  MOLETTE SEULE (aucune touche) : ${pctRoue} % en ${crans} crans` +
  ` (plafond raisonnable : ${MAX_CRANS}) → ${roueOk ? "✓" : "✗"}`);

await j("/json/close/" + tab.id);
ws.close();
process.exit(ok && roueOk ? 0 : 1);
