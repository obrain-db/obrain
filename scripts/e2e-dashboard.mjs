// E2E versionné du dashboard (Monde) — vérifie l'état « raccord » de la base
// graph côté interface, sur le serveur live. Usage :
//   node scripts/e2e-dashboard.mjs [BASE]     (nécessite un Chromium headless :
//   $CHROME ou le cache playwright par défaut)
import { existsSync } from "node:fs";
import { spawn } from "node:child_process";

const BASE = process.argv[2] || "http://192.168.1.103:8432";
const CHROME = process.env.CHROME
  || "/home/triviere/.cache/ms-playwright/chromium-1234/chrome-linux64/chrome";
if (!existsSync(CHROME)) { console.log("SKIP — chromium headless absent ($CHROME)"); process.exit(0); }

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const dir = `/tmp/obrain-e2e-${Date.now()}`;
const chrome = spawn(CHROME, [
  "--headless=new", "--no-sandbox", "--disable-dev-shm-usage",
  "--use-angle=swiftshader", "--enable-unsafe-swiftshader",
  "--remote-debugging-port=9248", `--user-data-dir=${dir}`, "--window-size=1440,900", "about:blank",
], { stdio: "ignore" });

let ws;
try {
  await sleep(3500);
  const targets = await (await fetch("http://127.0.0.1:9248/json")).json();
  const page = targets.find((x) => x.type === "page");
  if (!page) throw new Error("pas de page");
  ws = new WebSocket(page.webSocketDebuggerUrl);
  let idc = 0; const pend = new Map(); const errs = [];
  ws.onmessage = (ev) => {
    const m = JSON.parse(ev.data);
    if (m.id && pend.has(m.id)) { const p = pend.get(m.id); pend.delete(m.id);
      m.error ? p.rej(new Error(m.error.message)) : p.res(m.result); }
    else if (m.method === "Runtime.exceptionThrown") errs.push("exc:" + (m.params.exceptionDetails?.text || ""));
    else if (m.method === "Log.entryAdded" && m.params.entry.level === "error") errs.push("log:" + m.params.entry.text.slice(0, 120));
  };
  await new Promise((r) => { ws.onopen = r; });
  const send = (method, params = {}) => new Promise((res, rej) => {
    const id = ++idc; pend.set(id, { res, rej }); ws.send(JSON.stringify({ id, method, params }));
  });
  const ev = async (e) => {
    const r = await send("Runtime.evaluate", { expression: e, returnByValue: true, awaitPromise: true });
    if (r.exceptionDetails) return "EXC:" + (r.exceptionDetails.exception?.description || "");
    return r.result?.value;
  };
  await send("Runtime.enable"); await send("Log.enable");
  const out = {};
  await send("Page.navigate", { url: `${BASE}/` }); await sleep(7500);
  out.racine_h2 = await ev(`document.querySelector('h2')?.innerText||'(absent)'`);
  out.canvas = await ev(`!!document.querySelector('.overview canvas')`);
  out.labels = await ev(`document.querySelectorAll('.monde-label').length`);
  await send("Page.navigate", { url: `${BASE}/?dive=deepseek_harness` }); await sleep(7000);
  out.dive_h2 = await ev(`document.querySelector('h2')?.innerText||'(absent)'`);
  out.map_btn = await ev(`[...document.querySelectorAll('.zoomctl button')].some(b=>b.textContent==='🗺')`);
  const net0 = await ev(`performance.getEntriesByType('resource').filter(r=>r.name.includes('/api/')).length`);
  await ev(`(() => { const b=[...document.querySelectorAll('.zoomctl button')].find(x=>x.textContent==='🗺'); b?.click(); return 1; })()`);
  await sleep(900);
  out.map = await ev(`!!document.querySelector('.monde-map')`);
  await sleep(2000);
  out.map_net0 = (await ev(`performance.getEntriesByType('resource').filter(r=>r.name.includes('/api/')).length`)) === net0;
  // parcours par double-clic → entrée d'historique → retour navigateur
  await ev(`(() => { const n=[...document.querySelectorAll('.monde-label:not(.ghost) .ml-name')].find(e=>e.textContent==='packages');
    if(!n) return 0; const r=n.parentElement.getBoundingClientRect(); const o={clientX:r.left+r.width/2,clientY:r.top-8,bubbles:true};
    const c=document.querySelector('.overview canvas'); c.dispatchEvent(new MouseEvent('click',o)); c.dispatchEvent(new MouseEvent('dblclick',o)); return 1; })()`);
  await sleep(3500);
  out.h2_packages = await ev(`document.querySelector('h2')?.innerText||'(absent)'`);
  await ev(`window.history.back(); 1`);
  await sleep(2500);
  out.h2_back = await ev(`document.querySelector('h2')?.innerText||'(absent)'`);
  await ev(`window.dispatchEvent(new KeyboardEvent('keydown',{key:'Backspace',bubbles:true}));1`);
  await sleep(2500);
  out.retour_h2 = await ev(`document.querySelector('h2')?.innerText||'(absent)'`);
  // traversée multi-store (Linux, agrégats chauds/disque)
  await send("Page.navigate", { url: `${BASE}/?store=Linux%20complet%20(v6.6)&dive=drivers` }); await sleep(8000);
  out.linux_h2 = await ev(`document.querySelector('h2')?.innerText||'(absent)'`);
  out.errors = errs;
  console.log(JSON.stringify(out, null, 1));

  const ok = out.racine_h2 && out.canvas && out.labels > 0
    && out.dive_h2 && out.dive_h2.includes("Sous-espace")
    && out.map_btn && out.map
    && out.h2_packages && out.h2_packages.includes("packages")
    && out.h2_back && out.h2_back.includes("deepseek_harness")
    && out.retour_h2 === "Tous les projets"
    && out.linux_h2 && out.linux_h2.includes("drivers")
    && out.map_net0
    && out.errors.length === 0;
  console.log(ok ? "E2E OK" : "E2E ÉCHEC");
  process.exit(ok ? 0 : 1);
} finally {
  try { ws?.close(); } catch {}
  chrome.kill("SIGKILL");
}
