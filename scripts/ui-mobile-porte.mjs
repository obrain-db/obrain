#!/usr/bin/env node
// ── PORTE : L'INTERFACE EST-ELLE UTILISABLE AU DOIGT ? ──────────────────────
//
// « Ça doit être simple et compréhensible même sur mobile » ne se vérifie pas en
// relisant du SCSS : le CSS mobile EXISTAIT déjà quand la mesure a montré sept
// cibles sous le seuil tactile et huit chevauchements. Cette porte mesure la
// géométrie RÉELLEMENT rendue, sur les trois tailles de téléphone qui comptent,
// et échoue si l'une des quatre règles est violée :
//
//   1. rien ne DÉBORDE de l'écran (un contrôle coupé est un contrôle perdu) ;
//   2. tout contrôle atteignable au doigt fait au moins 44 px (Apple 44 pt,
//      Material 48 dp — 40 n'est ni l'un ni l'autre) ;
//   3. deux contrôles ne se CHEVAUCHENT pas (on vise l'un, on tape l'autre) ;
//   4. aucun texte n'est illisible (police < 11 px).
//
// Elle ne juge pas l'ESTHÉTIQUE — elle juge ce qui empêche d'agir. Le seuil de
// chevauchement toléré est de 6 px, parce que deux boîtes peuvent se toucher
// sans se gêner (bordures, ombres) ; au-delà, c'est un vrai recouvrement.
//
// Usage : node scripts/ui-mobile-porte.mjs [base] [dossier-rapports]
// Sortie : 0 si tout passe, 1 sinon. Les rapports JSON restent pour l'enquête.

import { spawn } from "node:child_process";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";

const BASE = process.argv[2] || "http://127.0.0.1:8462";
const SORTIE = process.argv[3] || "/tmp/ui-porte";
const CHROME = process.env.CHROME
  || "/home/triviere/.cache/ms-playwright/chromium-1234/chrome-linux64/chrome";
if (!existsSync(CHROME)) { console.log(`SKIP — chromium absent (${CHROME})`); process.exit(0); }
mkdirSync(SORTIE, { recursive: true });

// Le téléphone le plus courant, un Android plus étroit, et le plus petit encore
// en service : si ça tient à 320, ça tient partout.
const TAILLES = [[390, 844], [360, 640], [320, 568]];
const SEUIL_TACTILE = 44;
const TOLERANCE_CHEVAUCHEMENT = 6;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

let echecs = 0;
const ok = (m) => console.log(`  ✅ ${m}`);
const ko = (m) => { console.log(`  ❌ ${m}`); echecs++; };

for (const [L, H] of TAILLES) {
  const dir = `/tmp/obrain-porte-${L}-${Date.now()}`;
  const chrome = spawn(CHROME, [
    "--headless=new", "--no-sandbox", "--disable-dev-shm-usage",
    "--use-angle=swiftshader", "--enable-unsafe-swiftshader",
    "--remote-debugging-port=9253", `--user-data-dir=${dir}`,
    `--window-size=${L},${H}`, "about:blank",
  ], { stdio: "ignore" });
  let ws, id = 0;
  const pend = new Map();
  const console_ = [], exceptions = [];
  const send = (m, p = {}) => new Promise((r) => { const i = ++id; pend.set(i, r); ws.send(JSON.stringify({ id: i, method: m, params: p })); });
  try {
    await sleep(3500);
    const t = await (await fetch("http://127.0.0.1:9253/json")).json();
    ws = new WebSocket(t.find((x) => x.type === "page").webSocketDebuggerUrl);
    await new Promise((r) => ws.addEventListener("open", r, { once: true }));
    ws.addEventListener("message", (e) => {
      const m = JSON.parse(e.data);
      if (m.id && pend.has(m.id)) { pend.get(m.id)(m.result); pend.delete(m.id); return; }
      if (m.method === "Runtime.consoleAPICalled" && m.params.type === "error")
        console_.push((m.params.args || []).map((a) => a.value ?? "").join(" ").slice(0, 160));
      if (m.method === "Runtime.exceptionThrown") exceptions.push(m.params.exceptionDetails.text);
    });
    await send("Runtime.enable"); await send("Page.enable");
    await send("Emulation.setDeviceMetricsOverride", { width: L, height: H, deviceScaleFactor: 2, mobile: true });
    await send("Emulation.setTouchEmulationEnabled", { enabled: true, maxTouchPoints: 5 });
  // ── LE JETON, QUAND LE SERVEUR EST ARMÉ ─────────────────────────────────
  // Depuis que l'accueil redirige vers `/login` sans session (mesuré le
  // 2026-09-16), une porte qui ne présente pas d'identité mesure la PAGE DE
  // CONNEXION en croyant mesurer le dashboard. `OBRAIN_TOKEN` (rendu par
  // `scripts/banc-interface-authentifiee.sh`) pose le cookie de session dans le
  // navigateur avant toute navigation.
  const jeton = process.env.OBRAIN_TOKEN || "";
  if (jeton) {
    const verif = await send("Network.enable");
    await send("Network.setCookie", { name: "obrain_session", value: jeton, domain: new URL(BASE).hostname, path: "/" });
  }
    await send("Page.navigate", { url: BASE + "/" });
    await sleep(9000);
    const ev = async (e) => (await send("Runtime.evaluate", { expression: e, returnByValue: true, awaitPromise: true }))?.result?.value;

    const m = await ev(`(() => {
      const vis = e => { const s = getComputedStyle(e), r = e.getBoundingClientRect();
        return s.display!=='none' && s.visibility!=='hidden' && r.width>1 && r.height>1; };
      const ctl = [...document.querySelectorAll('button, a, input, select, textarea, [role=button], [tabindex]')].filter(vis);
      const r = e => e.getBoundingClientRect();
      const chev = [];
      for (let i=0;i<ctl.length;i++) for (let j=i+1;j<ctl.length;j++) {
        const a=r(ctl[i]), b=r(ctl[j]);
        const ox=Math.min(a.right,b.right)-Math.max(a.left,b.left);
        const oy=Math.min(a.bottom,b.bottom)-Math.max(a.top,b.top);
        if (ox>${TOLERANCE_CHEVAUCHEMENT} && oy>${TOLERANCE_CHEVAUCHEMENT}) chev.push({
          a:(ctl[i].textContent||'').trim().slice(0,24)||ctl[i].tagName,
          b:(ctl[j].textContent||'').trim().slice(0,24)||ctl[j].tagName, ox:Math.round(ox), oy:Math.round(oy)});
      }
      return {
        page: { l: document.documentElement.scrollWidth, h: document.documentElement.scrollHeight },
        vue: { l: innerWidth, h: innerHeight },
        n: ctl.length,
        tropPetits: ctl.filter(e => r(e).height < ${SEUIL_TACTILE} || r(e).width < ${SEUIL_TACTILE})
          .map(e => ({ t:(e.textContent||'').trim().slice(0,28)||e.getAttribute('aria-label')||e.tagName,
                       l:Math.round(r(e).width), h:Math.round(r(e).height) })),
        horsEcran: ctl.filter(e => r(e).right > innerWidth+1 || r(e).left < -1)
          .map(e => ({ t:(e.textContent||'').trim().slice(0,28), x:Math.round(r(e).x), l:Math.round(r(e).width) })),
        chev,
        illisible: [...document.querySelectorAll('*')].filter(vis).filter(e => {
          const fs=parseFloat(getComputedStyle(e).fontSize)||0;
          return e.children.length===0 && (e.textContent||'').trim().length>3 && fs>0 && fs<11; })
          .map(e => ({ t:e.textContent.trim().slice(0,36), px:parseFloat(getComputedStyle(e).fontSize) })).slice(0,8),
      };
    })()`);

    writeFileSync(`${SORTIE}/ecran-${L}x${H}.json`, JSON.stringify({ base: BASE, taille: `${L}x${H}`, ...m, console: console_, exceptions }, null, 2));
    console.log(`\n── ÉCRAN ${L}×${H} (${m.n} contrôles) ──`);
    m.page.l <= L + 1 ? ok(`rien ne déborde de l'écran (${m.page.l} ≤ ${L})`)
                      : ko(`la page déborde en largeur : ${m.page.l} px pour ${L}`);
    m.horsEcran.length === 0 ? ok("aucun contrôle hors de l'écran")
      : ko(`${m.horsEcran.length} contrôle(s) hors écran : ${m.horsEcran.map((c) => `« ${c.t} » x=${c.x} l=${c.l}`).join(", ")}`);
    m.tropPetits.length === 0 ? ok(`tous les contrôles font ≥ ${SEUIL_TACTILE} px`)
      : ko(`${m.tropPetits.length} contrôle(s) sous ${SEUIL_TACTILE} px : ${m.tropPetits.map((c) => `« ${c.t} » ${c.l}×${c.h}`).join(", ")}`);
    m.chev.length === 0 ? ok("aucun chevauchement de contrôles")
      : ko(`${m.chev.length} chevauchement(s) : ${m.chev.map((c) => `« ${c.a} »/« ${c.b} » ${c.ox}×${c.oy}`).join(", ")}`);
    m.illisible.length === 0 ? ok("aucun texte sous 11 px")
      : ko(`${m.illisible.length} texte(s) illisible(s) : ${m.illisible.map((t) => `« ${t.t} » ${t.px}px`).join(", ")}`);
    exceptions.length === 0 && console_.length === 0 ? ok("aucune erreur console")
      : ko(`${exceptions.length} exception(s), ${console_.length} erreur(s) console`);
  } catch (e) {
    ko(`mesure impossible en ${L}×${H} : ${e.message}`);
  } finally {
    try { chrome.kill("SIGKILL"); } catch { /* déjà mort */ }
  }
}

console.log(echecs === 0
  ? "\n✅ l'interface est manoeuvrable au doigt sur les trois tailles de téléphone"
  : `\n❌ ${echecs} contrôle(s) en échec`);
console.log(`rapports : ${SORTIE}`);
process.exit(echecs === 0 ? 0 : 1);
