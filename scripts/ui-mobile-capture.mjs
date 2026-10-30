#!/usr/bin/env node
// ── CAPTURE MOBILE DE L'INTERFACE ───────────────────────────────────────────
//
// À quoi ça sert, et pourquoi ce n'est pas un gadget : « ça doit être simple et
// compréhensible même sur mobile » ne se vérifie pas en relisant du SCSS. Les
// règles média existent déjà (`@media (pointer: coarse)`, `max-width: 620px`) —
// et c'est justement le piège : du CSS présent ne prouve pas un écran lisible.
// Un contrôle qui mesure des pixels ne peut pas dire si un bouton est
// COMPRÉHENSIBLE ; une image, oui.
//
// Ce script ouvre la page à une taille de téléphone, capture les états qui
// comptent, et RELÈVE ce que le CSS ne dit pas :
//   · les erreurs console (une frontière d'erreur casse la vue entière) ;
//   · les débordements horizontaux (le défaut mobile le plus courant) ;
//   · la taille des cibles tactiles réellement rendues, mesurée sur les
//     éléments INTERACTIFS — pas la valeur déclarée dans une règle ;
//   · le texte tronqué ou illisible (police sous 11 px).
//
// Usage :
//   node scripts/ui-mobile-capture.mjs [base] [sortie]
//   BASE=http://127.0.0.1:8432 (une session doit être ouverte dans le profil)
//
// Il écrit un rapport JSON + les PNG dans le dossier de sortie, pour qu'on
// puisse REGARDER l'écran au lieu de l'imaginer.

import { spawn } from "node:child_process";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";

const BASE = process.argv[2] || "http://192.168.1.103:8432";
const SORTIE = process.argv[3] || "/tmp/ui-mobile";
const CHROME = process.env.CHROME
  || "/home/triviere/.cache/ms-playwright/chromium-1234/chrome-linux64/chrome";
if (!existsSync(CHROME)) {
  console.log(`SKIP — chromium headless absent (${CHROME})`);
  process.exit(0);
}
mkdirSync(SORTIE, { recursive: true });

// Un iPhone récent en portrait, en pixels CSS : c'est la fenêtre la plus
// contrainte qu'un utilisateur réel aura. 390×844 est le cas courant.
const L = 390, H = 844;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const dir = `/tmp/obrain-ui-mobile-${Date.now()}`;
const chrome = spawn(CHROME, [
  "--headless=new", "--no-sandbox", "--disable-dev-shm-usage",
  "--use-angle=swiftshader", "--enable-unsafe-swiftshader",
  "--remote-debugging-port=9250", `--user-data-dir=${dir}`,
  `--window-size=${L},${H}`, "about:blank",
], { stdio: "ignore" });

let ws, id = 0;
const pending = new Map();
const evenements = { console: [], erreurs: [] };
const send = (method, params = {}) => new Promise((res) => {
  const i = ++id;
  pending.set(i, res);
  ws.send(JSON.stringify({ id: i, method, params }));
});

try {
  await sleep(3500);
  const cibles = await (await fetch("http://127.0.0.1:9250/json")).json();
  const page = cibles.find((x) => x.type === "page");
  if (!page) throw new Error("aucune page CDP");
  ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((r) => ws.addEventListener("open", r, { once: true }));
  ws.addEventListener("message", (ev) => {
    const m = JSON.parse(ev.data);
    if (m.id && pending.has(m.id)) { pending.get(m.id)(m.result); pending.delete(m.id); return; }
    if (m.method === "Runtime.consoleAPICalled" && m.params.type === "error") {
      evenements.console.push((m.params.args || []).map((a) => a.value ?? a.description ?? "?").join(" ").slice(0, 300));
    }
    if (m.method === "Runtime.exceptionThrown") {
      evenements.erreurs.push(m.params.exceptionDetails.text);
    }
  });
  await send("Runtime.enable");
  await send("Page.enable");
  // Le viewport est ce qui décide du rendu, indépendamment de la fenêtre.
  await send("Emulation.setDeviceMetricsOverride", {
    width: L, height: H, deviceScaleFactor: 2, mobile: true,
  });
  await send("Emulation.setTouchEmulationEnabled", { enabled: true, maxTouchPoints: 5 });

  const evaluer = async (expression) => (await send("Runtime.evaluate", {
    expression, returnByValue: true, awaitPromise: true,
  }))?.result?.value;

  await send("Page.navigate", { url: BASE + "/?cosmos=1" });
  await sleep(9000);

  // ── CE QUE LE CSS NE DIT PAS : LA GÉOMÉTRIE RÉELLE ──────────────────────
  const mesure = await evaluer(`(() => {
    const interactifs = [...document.querySelectorAll('button, a, input, select, [role=button]')]
      .filter(e => { const r = e.getBoundingClientRect(); return r.width > 0 && r.height > 0; })
      .map(e => { const r = e.getBoundingClientRect();
        return { tag: e.tagName.toLowerCase(), texte: (e.textContent||e.getAttribute('aria-label')||'').trim().slice(0,40),
                 w: Math.round(r.width), h: Math.round(r.height) }; });
    const petits = [...document.querySelectorAll('*')].filter(e => {
      const r = e.getBoundingClientRect();
      if (r.width === 0 || r.height === 0) return false;
      const fs = parseFloat(getComputedStyle(e).fontSize) || 0;
      return (e.children.length === 0) && (e.textContent||'').trim().length > 3 && fs > 0 && fs < 11;
    }).slice(0, 12).map(e => ({ texte: e.textContent.trim().slice(0, 40), px: parseFloat(getComputedStyle(e).fontSize) }));
    const debord = [...document.querySelectorAll('*')].filter(e => e.getBoundingClientRect().right > window.innerWidth + 1)
      .slice(0, 12).map(e => ({ tag: e.tagName.toLowerCase(), cls: (e.className||'').toString().slice(0,50),
                 droite: Math.round(e.getBoundingClientRect().right) }));
    return {
      viewport: { l: window.innerWidth, h: window.innerHeight },
      scrollLargeur: document.documentElement.scrollWidth,
      debordementHorizontal: document.documentElement.scrollWidth > window.innerWidth + 1,
      cibles: interactifs,
      ciblesTropPetites: interactifs.filter(c => c.h < 44 || c.w < 44),
      texteTropPetit: petits,
      debordements: debord,
      titre: document.title,
    };
  })()`);

  // ── LES CAPTURES : LES ÉTATS QUI COMPTENT ───────────────────────────────
  const capturer = async (nom) => {
    const r = await send("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
    if (r?.data) writeFileSync(`${SORTIE}/${nom}.png`, Buffer.from(r.data, "base64"));
  };
  await capturer("01-accueil-mobile");

  // Le panneau de chat : c'est là que l'utilisateur lit les réponses.
  const ouvert = await evaluer(`(() => {
    const b = [...document.querySelectorAll('button')].find(x => /chat|question|demander/i.test(x.textContent||''));
    if (b) { b.click(); return true; } return false;
  })()`);
  await sleep(2500);
  await capturer("02-apres-clic-chat");

  // Les commandes du cosmos (là où vivent les chips).
  await evaluer(`(() => { const b = [...document.querySelectorAll('button')].find(x => /cosmos|monde|vue/i.test(x.textContent||'')); if (b) b.click(); })()`);
  await sleep(2500);
  await capturer("03-commandes");

  const rapport = {
    base: BASE,
    viewport: `${L}x${H}`,
    chat_ouvert_par_clic: !!ouvert,
    ...mesure,
    erreursConsole: evenements.console,
    exceptions: evenements.erreurs,
    captures: ["01-accueil-mobile", "02-apres-clic-chat", "03-commandes"],
  };
  writeFileSync(`${SORTIE}/rapport.json`, JSON.stringify(rapport, null, 2));

  // ── LE VERDICT, DIT EN CLAIR ────────────────────────────────────────────
  const n = mesure?.ciblesTropPetites?.length || 0;
  console.log(`INTERFACE EN ${L}x${H} — ${mesure?.titre || "?"}`);
  console.log(`  débordement horizontal : ${mesure?.debordementHorizontal ? `OUI (${mesure.scrollLargeur}px pour ${L})` : "non"}`);
  console.log(`  cibles interactives : ${mesure?.cibles?.length || 0} · sous 44 px : ${n}`);
  if (n) for (const c of mesure.ciblesTropPetites.slice(0, 8))
    console.log(`      ${c.tag} « ${c.texte} » ${c.w}×${c.h}`);
  console.log(`  texte sous 11 px : ${mesure?.texteTropPetit?.length || 0}`);
  if (mesure?.debordements?.length) for (const d of mesure.debordements.slice(0, 6))
    console.log(`      déborde : ${d.tag}.${d.cls} → ${d.droite}px`);
  console.log(`  erreurs console : ${evenements.console.length} · exceptions : ${evenements.erreurs.length}`);
  for (const e of evenements.console.slice(0, 5)) console.log(`      console: ${e}`);
  console.log(`\ncaptures et rapport : ${SORTIE}`);
} catch (e) {
  console.error("capture impossible :", e.message);
} finally {
  try { chrome.kill("SIGKILL"); } catch { /* déjà mort */ }
}
