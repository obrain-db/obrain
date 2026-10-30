// VÉRIFICATION EN VRAIE INTERFACE — la coupure ne doit plus JAMAIS s'afficher
//
//   « flux interrompu avant la fin — ce qui précède est incomplet »
//
// Ce que ce test ajoute à `chat-reprise-check.mjs` (qui vérifie le protocole) :
// il pilote le VRAI dashboard dans un VRAI navigateur, coupe le service EN PLEIN
// FLUX, et regarde ce que l'utilisateur verrait. C'est la seule façon de vérifier
// la reprise automatique de la vue (minuteur, réouverture, texte déjà affiché) —
// un test de protocole ne la voit pas.
//
// Ce qu'il exige :
//   1. la phrase interdite n'apparaît JAMAIS dans le DOM ;
//   2. la reprise SE DIT (« reprise automatique ») ;
//   3. la réponse CONTINUE : le texte final est plus long qu'au moment de la
//      coupure, et le tour se termine.
//
// Usage : node scripts/chat-reprise-ui-check.mjs [BASE]
//   BASE par défaut : http://192.168.1.103:8432  (nécessite un Chromium headless)
//
// ATTENTION : ce test TUE `obrain viz serve` (comme un déploiement). Le service
// est relancé par systemd (`Restart=always`, `RestartSec=2`).

import { existsSync } from "node:fs";
import { spawn, execFileSync } from "node:child_process";

const BASE = process.argv[2] || "http://192.168.1.103:8432";
const CHROME = process.env.CHROME
  || "/home/triviere/.cache/ms-playwright/chromium-1234/chrome-linux64/chrome";
if (!existsSync(CHROME)) {
  console.log(`SKIP — chromium headless absent (${CHROME})`);
  process.exit(0);
}

const QUESTION = "Compte de 1 à 40, en chiffres, séparés par des virgules.";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const faits = [];
const noter = (ok, quoi) => { faits.push({ ok, quoi }); console.log(`${ok ? "  ok  " : " FAIL "} ${quoi}`); };

const dir = `/tmp/obrain-chat-ui-${Date.now()}`;
const chrome = spawn(CHROME, [
  "--headless=new", "--no-sandbox", "--disable-dev-shm-usage",
  "--use-angle=swiftshader", "--enable-unsafe-swiftshader",
  "--remote-debugging-port=9251", `--user-data-dir=${dir}`, "--window-size=1440,900",
  "about:blank",
], { stdio: "ignore" });

let ws;
try {
  await sleep(3500);
  const cibles = await (await fetch("http://127.0.0.1:9251/json")).json();
  const page = cibles.find((x) => x.type === "page");
  if (!page) throw new Error("pas de page CDP");
  ws = new WebSocket(page.webSocketDebuggerUrl);
  let idc = 0; const pend = new Map();
  ws.onmessage = (ev) => {
    const m = JSON.parse(ev.data);
    if (m.id && pend.has(m.id)) { const p = pend.get(m.id); pend.delete(m.id);
      m.error ? p.rej(new Error(m.error.message)) : p.res(m.result); }
  };
  await new Promise((r) => { ws.onopen = r; });
  const send = (method, params = {}) => new Promise((res, rej) => {
    const id = ++idc; pend.set(id, { res, rej }); ws.send(JSON.stringify({ id, method, params }));
  });
  const ev = async (expr) => {
    const r = await send("Runtime.evaluate", { expression: expr, returnByValue: true, awaitPromise: true });
    if (r.exceptionDetails) return "EXC:" + (r.exceptionDetails.exception?.description || "");
    return r.result?.value;
  };
  const texteAssistant = () => ev(
    `[...document.querySelectorAll('.chat-lui')].map(e=>e.innerText).join('\\n')`,
  );
  const textePage = () => ev(`document.body.innerText`);

  await send("Runtime.enable");
  console.log(`── reprise en vraie interface — ${BASE}`);
  await send("Page.navigate", { url: `${BASE}/` });
  await sleep(9000);

  // ── OUVRIR LE CHAT (il vit dans la vue COSMOS) ────────────────────────────
  await ev(`(() => {
    const cosmos = [...document.querySelectorAll('button')].find(b => /Cosmos/i.test(b.textContent||''));
    cosmos?.click();
    return 1;
  })()`);
  await sleep(7000);
  const ouvert = await ev(`(() => {
    const b = document.querySelector('.chat-flottant')
      || [...document.querySelectorAll('button')].find(x => /chat/i.test((x.textContent||'') + (x.title||'') + String(x.className)));
    if (!b) return 'aucun-contrôle';
    b.click();
    return 'cliqué';
  })()`);
  await sleep(2500);
  const champ = await ev(`!!document.querySelector('textarea')`);
  noter(ouvert === "cliqué" && champ === true, `chat ouvert (${ouvert}, champ de saisie : ${champ})`);
  if (champ !== true) throw new Error("le chat ne s'est pas ouvert — test interrompu");

  // ── POSER LA QUESTION ─────────────────────────────────────────────────────
  await ev(`(() => {
    const t = document.querySelector('textarea');
    const set = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value').set;
    set.call(t, ${JSON.stringify(QUESTION)});
    t.dispatchEvent(new Event('input', { bubbles: true }));
    return 1;
  })()`);
  await sleep(400);
  await ev(`(() => {
    const b = [...document.querySelectorAll('button')].find(x => /envoyer/i.test(x.textContent||''));
    b.click(); return 1;
  })()`);

  // ── ATTENDRE LE PREMIER TEXTE, PUIS COUPER EN PLEIN FLUX ──────────────────
  let avant = "";
  for (let i = 0; i < 60; i++) {
    await sleep(2000);
    const t = await texteAssistant();
    const m = /(?:^|\n)(\d[\d, ]{20,})/m.exec(t || "");
    if (m) { avant = m[1]; break; }
  }
  noter(avant.length > 20, `réponse en cours de flux : ${avant.length} caractères de chiffres`);
  if (avant.length <= 20) throw new Error("aucun flux de réponse observé — test interrompu");

  let tues = 0;
  try {
    const pids = execFileSync("pgrep", ["-f", "obrain vi[z] serve"], { encoding: "utf8" })
      .split("\n").map((s) => s.trim()).filter(Boolean);
    for (const pid of pids) { try { process.kill(Number(pid), "SIGKILL"); tues++; } catch { /* mort */ } }
  } catch { /* rien à tuer */ }
  noter(tues > 0, `service TUÉ en pleine réponse (${tues} processus) — comme un déploiement`);

  // ── CE QUE L'UTILISATEUR VOIT ENSUITE ─────────────────────────────────────
  let suite = "";
  let notice = false;
  for (let i = 0; i < 90; i++) {
    await sleep(2000);
    const vue = await textePage();
    if (typeof vue === "string" && /flux interrompu avant la fin/i.test(vue)) {
      noter(false, "LA PHRASE INTERDITE EST APPARUE À L'ÉCRAN");
      break;
    }
    if (typeof vue === "string" && /reprise automatique/i.test(vue)) notice = true;
    const t = await texteAssistant();
    const m = /(?:^|\n)(\d[\d, ]{20,})/m.exec(t || "");
    if (m) suite = m[1];
    if (suite.length > avant.length + 5) break;
  }
  const pageFinale = await textePage();
  noter(!/flux interrompu avant la fin/i.test(pageFinale || ""), "la phrase « flux interrompu avant la fin » n'est jamais apparue");
  noter(notice, "la reprise automatique s'est ANNONCÉE à l'écran");
  noter(suite.length > avant.length, `la réponse a CONTINUÉ après la coupure (${avant.length} → ${suite.length} caractères)`);
  console.log(`        avant coupure : ${JSON.stringify(avant.slice(-30))}`);
  console.log(`        après reprise : ${JSON.stringify(suite.slice(-30))}`);

  const echecs = faits.filter((f) => !f.ok);
  console.log(`── ${echecs.length === 0 ? "PASS" : `FAIL (${echecs.length})`} — reprise en vraie interface`);
  process.exit(echecs.length === 0 ? 0 : 1);
} catch (e) {
  console.log(`  FAIL ${e.message}`);
  process.exit(1);
} finally {
  try { ws?.close(); } catch { /* déjà fermé */ }
  chrome.kill("SIGKILL");
}
