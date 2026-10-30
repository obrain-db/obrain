#!/usr/bin/env node
// ── LIRE L'ÉCRAN SANS LE VOIR : UNE CARTE MESURÉE DE CE QUI EST AFFICHÉ ──────
//
// Pourquoi ce script, et pas une capture d'écran : une image ne se LIT pas
// toujours (le modèle qui l'examine peut ne pas accepter d'images), et surtout
// elle ne se COMPARE pas — on ne sait pas dire si un écran s'est dégradé. Ici on
// rend une carte mesurée : chaque élément visible avec sa boîte, sa police, son
// texte. C'est la même information qu'un œil, en chiffres, donc vérifiable.
//
// Ce qu'il sert à décider : « simple et compréhensible sur mobile » se joue sur
// quatre choses qu'on peut mesurer sans les voir —
//   · ce qui DÉBORDE (coupé à droite = perdu) ;
//   · ce qui est TROP PETIT pour un doigt (seuil tactile 44 px) ;
//   · ce qui se CHEVAUCHE (deux contrôles au même endroit : on tape l'autre) ;
//   · ce qui est illisible (police < 11 px, contraste faible) ;
//   · et ce que l'écran DIT : les libellés réellement rendus, dans l'ordre
//     vertical — c'est le récit que l'utilisateur reçoit.
//
// Usage : node scripts/ui-lire-ecran.mjs [base] [largeur] [hauteur] [sortie]

import { spawn } from "node:child_process";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";

const BASE = process.argv[2] || "http://127.0.0.1:8462";
const L = Number(process.argv[3] || 390);
const H = Number(process.argv[4] || 844);
const SORTIE = process.argv[5] || "/tmp/ui-lecture";
const CHROME = process.env.CHROME
  || "/home/triviere/.cache/ms-playwright/chromium-1234/chrome-linux64/chrome";
if (!existsSync(CHROME)) { console.log(`SKIP — chromium absent (${CHROME})`); process.exit(0); }
mkdirSync(SORTIE, { recursive: true });

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const dir = `/tmp/obrain-lecture-${Date.now()}`;
const chrome = spawn(CHROME, [
  "--headless=new", "--no-sandbox", "--disable-dev-shm-usage",
  "--use-angle=swiftshader", "--enable-unsafe-swiftshader",
  "--remote-debugging-port=9251", `--user-data-dir=${dir}`,
  `--window-size=${L},${H}`, "about:blank",
], { stdio: "ignore" });

let ws, id = 0;
const pending = new Map();
const console_ = [], exceptions = [];
const send = (method, params = {}) => new Promise((res) => {
  const i = ++id; pending.set(i, res); ws.send(JSON.stringify({ id: i, method, params }));
});

try {
  await sleep(3500);
  const cibles = await (await fetch("http://127.0.0.1:9251/json")).json();
  const page = cibles.find((x) => x.type === "page");
  ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((r) => ws.addEventListener("open", r, { once: true }));
  ws.addEventListener("message", (ev) => {
    const m = JSON.parse(ev.data);
    if (m.id && pending.has(m.id)) { pending.get(m.id)(m.result); pending.delete(m.id); return; }
    if (m.method === "Runtime.consoleAPICalled" && m.params.type === "error")
      console_.push((m.params.args || []).map((a) => a.value ?? "").join(" ").slice(0, 200));
    if (m.method === "Runtime.exceptionThrown") exceptions.push(m.params.exceptionDetails.text);
  });
  await send("Runtime.enable"); await send("Page.enable");
  await send("Emulation.setDeviceMetricsOverride", { width: L, height: H, deviceScaleFactor: 2, mobile: true });
  await send("Emulation.setTouchEmulationEnabled", { enabled: true, maxTouchPoints: 5 });
  const ev = async (e) => (await send("Runtime.evaluate", { expression: e, returnByValue: true, awaitPromise: true }))?.result?.value;

  await send("Page.navigate", { url: BASE + "/" });
  await sleep(9000);

  // ── LA CARTE : CE QUI EST VISIBLE, DANS L'ORDRE VERTICAL ────────────────
  const ecran = await ev(`(() => {
    const vis = e => { const s = getComputedStyle(e), r = e.getBoundingClientRect();
      return s.display !== 'none' && s.visibility !== 'hidden' && s.opacity !== '0' && r.width > 1 && r.height > 1; };
    const tous = [...document.querySelectorAll('*')].filter(vis);
    const feuilles = tous.filter(e => ![...e.children].some(vis));
    const boite = e => { const r = e.getBoundingClientRect();
      return { x: Math.round(r.x), y: Math.round(r.y), l: Math.round(r.width), h: Math.round(r.height) }; };
    const interactifs = tous.filter(e => e.matches('button, a, input, select, textarea, [role=button], [tabindex]'));
    // Deux contrôles qui se chevauchent : on tape l'un en visant l'autre.
    const chevauchent = [];
    for (let i = 0; i < interactifs.length; i++) for (let j = i+1; j < interactifs.length; j++) {
      const a = interactifs[i].getBoundingClientRect(), b = interactifs[j].getBoundingClientRect();
      const ox = Math.min(a.right,b.right) - Math.max(a.left,b.left);
      const oy = Math.min(a.bottom,b.bottom) - Math.max(a.top,b.top);
      if (ox > 6 && oy > 6) chevauchent.push({
        a: (interactifs[i].textContent||'').trim().slice(0,24) || interactifs[i].tagName,
        b: (interactifs[j].textContent||'').trim().slice(0,24) || interactifs[j].tagName,
        ox: Math.round(ox), oy: Math.round(oy) });
    }
    return {
      viewport: { l: innerWidth, h: innerHeight },
      page: { l: document.documentElement.scrollWidth, h: document.documentElement.scrollHeight },
      defilementVertical: document.documentElement.scrollHeight > innerHeight + 4,
      defilementHorizontal: document.documentElement.scrollWidth > innerWidth + 1,
      // Le récit de l'écran : ce que l'utilisateur lit, de haut en bas.
      recit: feuilles
        .filter(e => (e.textContent||'').trim().length > 0)
        .map(e => ({ t: e.textContent.trim().slice(0, 70), y: Math.round(e.getBoundingClientRect().y),
                     px: parseFloat(getComputedStyle(e).fontSize) }))
        .sort((a,b) => a.y - b.y).slice(0, 45),
      interactifs: interactifs.map(e => ({ ...boite(e),
        t: (e.textContent||'').trim().slice(0,32) || e.getAttribute('aria-label') || e.tagName.toLowerCase(),
        px: parseFloat(getComputedStyle(e).fontSize) })),
      tropPetits: interactifs.filter(e => { const r = e.getBoundingClientRect(); return r.height < 44 || r.width < 44; })
        .map(e => ({ ...boite(e), t: (e.textContent||'').trim().slice(0,32) || e.getAttribute('aria-label') || e.tagName.toLowerCase() })),
      horsEcran: interactifs.filter(e => { const r = e.getBoundingClientRect();
        return r.right > innerWidth + 1 || r.left < -1; })
        .map(e => ({ ...boite(e), t: (e.textContent||'').trim().slice(0,32) })),
      chevauchent,
      tropPetit: feuilles.filter(e => { const fs = parseFloat(getComputedStyle(e).fontSize) || 0;
        return (e.textContent||'').trim().length > 3 && fs > 0 && fs < 11; })
        .map(e => ({ t: e.textContent.trim().slice(0,40), px: parseFloat(getComputedStyle(e).fontSize) })).slice(0, 10),
    };
  })()`);

  const rapport = { base: BASE, taille: `${L}x${H}`, chat: null, ...ecran, console: console_, exceptions };
  writeFileSync(`${SORTIE}/ecran-${L}x${H}.json`, JSON.stringify(rapport, null, 2));

  // ── LE RENDU LISIBLE : CE QUE L'ÉCRAN DIT, DE HAUT EN BAS ───────────────
  console.log(`ÉCRAN ${L}x${H} — page ${ecran?.page?.l}x${ecran?.page?.h}px`);
  console.log(`  débordement horizontal : ${ecran?.defilementHorizontal ? "OUI" : "non"} · vertical : ${ecran?.defilementVertical ? "oui" : "non"}`);
  console.log(`  contrôles : ${ecran?.interactifs?.length || 0} · sous 44 px : ${ecran?.tropPetits?.length || 0} · hors écran : ${ecran?.horsEcran?.length || 0} · chevauchements : ${ecran?.chevauchent?.length || 0}`);
  if (ecran?.tropPetits?.length) for (const c of ecran.tropPetits.slice(0, 10))
    console.log(`      petit  ${c.l}×${c.h}  « ${c.t} »`);
  if (ecran?.horsEcran?.length) for (const c of ecran.horsEcran.slice(0, 6))
    console.log(`      DEHORS x=${c.x} l=${c.l}  « ${c.t} »`);
  for (const c of (ecran?.chevauchent || []).slice(0, 6))
    console.log(`      CHEVAUCHE « ${c.a} » / « ${c.b} » (${c.ox}×${c.oy}px)`);
  if (ecran?.tropPetit?.length) for (const t of ecran.tropPetit.slice(0, 5))
    console.log(`      illisible ${t.px}px « ${t.t} »`);
  console.log(`  erreurs console : ${console_.length} · exceptions : ${exceptions.length}`);
  for (const e of console_.slice(0, 5)) console.log(`      ${e}`);
  console.log("\n── CE QUE L'ÉCRAN DIT (haut → bas) ──");
  for (const l of (ecran?.recit || [])) console.log(`  y=${String(l.y).padStart(4)} ${String(l.px).padStart(4)}px  ${l.t}`);
  console.log(`\nrapport : ${SORTIE}/ecran-${L}x${H}.json`);
} catch (e) {
  console.error("lecture impossible :", e.message);
} finally {
  try { chrome.kill("SIGKILL"); } catch { /* déjà mort */ }
}
