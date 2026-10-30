#!/usr/bin/env node
// Vérifie que les DEUX VUES FORTES du dashboard s'affichent vraiment, et que le
// bundle servi est bien celui de la source.
//
// Pourquoi ce script existe (défaut mesuré le 2026-09-12). Deux pannes
// simultanées ont produit le même symptôme — « les vues Monde et Cosmos ne
// s'affichent plus » — et AUCUNE des deux ne produit d'erreur visible :
//
//  1. LES ROUTES QUI OUVRENT UNE BASE PENDAIENT. `cached_ro` gardait le verrou
//     du cache pendant tout le corps d'un `if let`, puis le redemandait : le
//     premier appel qui trouvait une base en cache s'auto-bloquait en TENANT le
//     verrou. Résultat : `/api/info`, `/api/viz/signatures`, `/api/viz/weave`
//     et `/api/worlds` ne rendaient plus un octet, pour toutes les bases, sans
//     erreur, sans CPU, pendant que `/api/viz/status` répondait en 1 ms. Les
//     vues se construisaient alors sans données : pas de canvas, écran vide.
//     Un `curl` sans borne de temps ne le voit pas — il attend. D'où le délai.
//
//  2. LE BUNDLE SERVI ÉTAIT PÉRIMÉ. Le veilleur de `viz serve` échouait à
//     chaque changement (« pnpm indisponible » : le service systemd n'avait pas
//     le PATH de nvm). La source était corrigée, l'écran montrait l'ancienne —
//     jusqu'à afficher un commentaire CSS sous forme de texte dans le canevas.
//     Une page peut donc être « servie » et fausse : on l'exige sans fuite.
//
// Ce que le script exige, pour chaque vue : un canvas avec un contexte WebGL
// réel, et AUCUN commentaire de source rendu comme du texte.
//
// Usage : node scripts/vues-check.mjs [base] [chrome-cdp]
import { readFileSync } from "node:fs";

const BASE = (process.argv[2] || "http://192.168.1.103:8432").replace(/\/$/, "");
const CDP = process.argv[3] || process.env.CDP_URL || "http://127.0.0.1:9248";
const DELAI_MS = Number(process.env.DELAI_MS || 12_000);
const ATTENTE_MS = Number(process.env.ATTENTE_MS || 9_000);

const echecs = [];
const ok = (m) => console.log(`  OK   ${m}`);
const ko = (m) => { echecs.push(m); console.error(`  ÉCHEC ${m}`); };

// ── 1. LES ROUTES DONT LES VUES VIVENT ────────────────────────────────────
// Toutes celles qui ouvrent une base : c'est le groupe qui pendait ensemble.
const ROUTES = [
  "/api/viz/status",
  "/api/stores",
  "/api/worlds",
  "/api/viz/signatures",
  "/api/viz/overview?store=Runtime%20dev%20(engrams)",
  "/api/viz/weave",
  "/api/info?store=Runtime%20dev%20(engrams)",
];

console.log(`\n── ROUTES (délai ${DELAI_MS} ms ; un dépassement = blocage, pas lenteur) ──`);
for (const r of ROUTES) {
  const t0 = Date.now();
  try {
    const rep = await fetch(BASE + r, { signal: AbortSignal.timeout(DELAI_MS) });
    const corps = await rep.text();
    const ms = Date.now() - t0;
    // Un serveur qui bloque ne renvoie rien : le cas « 200 mais 0 octet » est
    // donc un échec à part entière, pas un succès.
    if (!rep.ok) ko(`${r} → HTTP ${rep.status}`);
    else if (!corps.length) ko(`${r} → 200 mais corps vide`);
    else ok(`${r} → ${corps.length} o en ${ms} ms`);
  } catch (e) {
    const ms = Date.now() - t0;
    const motif = e?.name === "TimeoutError" ? `AUCUNE RÉPONSE en ${DELAI_MS} ms (route bloquée)` : String(e?.message || e);
    ko(`${r} → ${motif} (${ms} ms)`);
  }
}

// ── 2. LA SOURCE ET LE BUNDLE SERVI DISENT-ILS LA MÊME CHOSE ? ────────────
console.log("\n── BUNDLE SERVI ──");
const j = async (p, init) => { const r = await fetch(CDP + p, init); const t = await r.text(); try { return JSON.parse(t); } catch { return t; } };
let onglet = null, ws = null;
try {
  const page = await (await fetch(BASE + "/", { signal: AbortSignal.timeout(5000) })).text();
  const bundle = (page.match(/\/app\/assets\/(index-[A-Za-z0-9_-]+\.js)/) || [])[1];
  if (!bundle) ko("la page servie ne désigne aucun bundle /app/assets/index-*.js");
  else {
    const src = await (await fetch(`${BASE}/app/assets/${bundle}`, { signal: AbortSignal.timeout(10_000) })).text();
    ok(`bundle ${bundle} servi (${src.length} o)`);
    // Le défaut exact constaté : un commentaire de source dans le JSX, donc
    // rendu comme du texte dans le canevas. On cherche la SIGNATURE, pas une
    // phrase précise : tout `/* … */` recopié tel quel dans le bundle.
    const fuite = /"\/\* ──[^"]{0,200}/.test(src) || /children:\[[^\]]*"\/\*/.test(src);
    if (fuite) ko("un commentaire de source est recopié dans le bundle (il s'affichera en texte)");
    else ok("aucun commentaire de source recopié dans le bundle");
  }
} catch (e) {
  ko(`lecture du bundle : ${e?.message || e}`);
}

// ── 3. LES DEUX VUES, DANS UN VRAI NAVIGATEUR ─────────────────────────────
if (!(await j("/json/version")).webSocketDebuggerUrl) {
  ko(`aucun Chrome CDP sur ${CDP} — les vues ne sont PAS vérifiées (démarrez-en un)`);
} else {
  for (const [vue, url] of [["Monde", `${BASE}/`], ["Cosmos", `${BASE}/?cosmos=1`]]) {
    console.log(`\n── VUE ${vue} (${url}) ──`);
    const tab = await j("/json/new?" + encodeURIComponent(url), { method: "PUT" });
    const sock = new WebSocket(tab.webSocketDebuggerUrl);
    let id = 0; const attente = new Map();
    const send = (method, params = {}) => new Promise((res) => { const i = ++id; attente.set(i, res); sock.send(JSON.stringify({ id: i, method, params })); });
    sock.addEventListener("message", (ev) => { const m = JSON.parse(ev.data); if (m.id && attente.has(m.id)) { attente.get(m.id)(m.result); attente.delete(m.id); } });
    await new Promise((r) => sock.addEventListener("open", r, { once: true }));
    await send("Runtime.enable");
    await send("Page.navigate", { url });
    await new Promise((r) => setTimeout(r, ATTENTE_MS));
    const p = (await send("Runtime.evaluate", {
      returnByValue: true,
      expression: `(() => {
        const cv = document.querySelector('.overview canvas, .stage canvas');
        const gl = cv && (cv.getContext('webgl2') || cv.getContext('webgl'));
        const txt = document.body.innerText || '';
        return { canvas: !!cv, w: cv ? cv.width : 0, h: cv ? cv.height : 0,
                 glError: gl ? gl.getError() : 'sans contexte WebGL',
                 fuite: /\\/\\*[^]*?\\*\\//.test(txt) && txt.indexOf('/*') < 400,
                 extrait: txt.replace(/\\s+/g, ' ').slice(0, 120) };
      })()`,
    }))?.result?.value || {};
    if (!p.canvas) ko(`${vue} : aucun canvas — la vue est vide (données absentes ou scène en panne)`);
    else if (p.glError !== 0) ko(`${vue} : canvas ${p.w}×${p.h} mais contexte WebGL en erreur (${p.glError})`);
    else ok(`${vue} : canvas ${p.w}×${p.h}, contexte WebGL sain`);
    if (p.fuite) ko(`${vue} : un commentaire de source est AFFICHÉ dans la vue — « ${p.extrait}… »`);
    else ok(`${vue} : aucun commentaire affiché (début : « ${p.extrait}… »)`);
    await j("/json/close/" + tab.id);
    sock.close();
  }
}

console.log(echecs.length ? `\n${echecs.length} ÉCHEC(S) : les vues ne sont pas fiables.\n` : "\nVues Monde et Cosmos : OK.\n");
process.exit(echecs.length ? 1 : 0);
