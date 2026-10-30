#!/usr/bin/env node
// Capture de la vue Cosmos via le Chrome LOCAL (CDP sur 127.0.0.1:9248).
//
// L'outil navigateur de l'agent refuse les IP privées, mais le Chrome de la
// machine, lui, n'a pas cette limite : ce script ouvre un onglet, laisse la
// scène se construire, collecte les erreurs (console + exceptions + WebGL),
// puis écrit une capture PNG — la vérification visuelle que l'agent ne peut
// pas produire autrement.
//
// Usage : node scripts/cosmos-capture.mjs [url] [sortie.png]
import { writeFileSync } from "node:fs";

const TARGET = process.argv[2] || "http://192.168.1.103:8432/?cosmos=1";
const OUT = process.argv[3] || "/tmp/cosmos.png";
const CDP = process.env.CDP_URL || "http://127.0.0.1:9248";
const WAIT_MS = Number(process.env.CAPTURE_WAIT_MS || 9000);

const j = async (p, init) => {
  const r = await fetch(CDP + p, init);
  const t = await r.text();
  try { return JSON.parse(t); } catch { return t; }
};

const tab = await j("/json/new?" + encodeURIComponent(TARGET), { method: "PUT" });
if (!tab?.webSocketDebuggerUrl) {
  console.error("capture : impossible d'ouvrir un onglet CDP —", JSON.stringify(tab).slice(0, 200));
  process.exit(2);
}

const ws = new WebSocket(tab.webSocketDebuggerUrl);
let id = 0;
const pending = new Map();
const logs = [];
const errors = [];
const send = (method, params = {}) =>
  new Promise((res) => { const i = ++id; pending.set(i, res); ws.send(JSON.stringify({ id: i, method, params })); });

ws.addEventListener("message", (ev) => {
  const m = JSON.parse(ev.data);
  if (m.id && pending.has(m.id)) { pending.get(m.id)(m.result); pending.delete(m.id); return; }
  if (m.method === "Runtime.exceptionThrown") {
    const d = m.params.exceptionDetails;
    errors.push(`EXCEPTION ${d.text} ${d.exception?.description?.split("\n")[0] || ""}`.trim());
  }
  if (m.method === "Log.entryAdded") {
    const e = m.params.entry;
    const line = `[${e.level}] ${e.text}${e.url ? " @ " + e.url.split("/").pop() : ""}`;
    (e.level === "error" ? errors : logs).push(line);
  }
  if (m.method === "Runtime.consoleAPICalled") {
    const txt = (m.params.args || []).map((a) => a.value ?? a.description ?? a.type).join(" ");
    (m.params.type === "error" ? errors : logs).push(`[console.${m.params.type}] ${txt}`);
  }
});

await new Promise((r) => ws.addEventListener("open", r, { once: true }));
await send("Runtime.enable");
await send("Log.enable");
await send("Page.enable");
await send("Page.navigate", { url: TARGET });
await new Promise((r) => setTimeout(r, WAIT_MS));

// État réel de la scène : le canvas existe-t-il, a-t-il une taille, et
// WebGL a-t-il bien rendu quelque chose ?
const probe = await send("Runtime.evaluate", {
  returnByValue: true,
  expression: `(() => {
    const cv = document.querySelector('.overview canvas');
    const gl = cv && (cv.getContext('webgl2') || cv.getContext('webgl'));
    const txt = document.body.innerText || '';
    return {
      canvas: !!cv,
      w: cv ? cv.width : 0, h: cv ? cv.height : 0,
      gl: !!gl,
      glError: gl ? gl.getError() : 'aucun contexte',
      fournisseur: gl ? (() => { const e = gl.getExtension('WEBGL_debug_renderer_info');
        return e ? gl.getParameter(e.UNMASKED_RENDERER_WEBGL) : 'masqué'; })() : '',
      panneau: txt.includes("la toile de l'univers"),
      foyers: (txt.match(/Foyer/g) || []).length,
      tisser: txt.includes('tisser'),
      ancres: (txt.match(/(\\d[\\d\\s]*)\\s+ancres tissées/) || [])[1] || '',
      fibres: (txt.match(/(\\d+)\\s+fibres/) || [])[1] || '',
      texte: txt.slice(0, 700),
    };
  })()`,
});
const p = probe?.result?.value || {};
console.log("sonde :", JSON.stringify(p, null, 1));
console.log("erreurs :", errors.length ? errors.slice(0, 8) : "aucune");
if (logs.length) console.log("journaux :", logs.slice(0, 6));

const shot = await send("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
if (shot?.data) {
  writeFileSync(OUT, Buffer.from(shot.data, "base64"));
  console.log("capture :", OUT);
} else {
  console.error("capture : échec de Page.captureScreenshot");
}

// Seconde capture, PANNEAUX MASQUÉS : seule façon de mesurer le CONTENU 3D
// seul. Sans cela, le chrome d'interface (barre du haut, panneaux latéraux)
// contamine toute mesure de cadrage ou de luminance par région — constaté :
// une boîte englobante « qui touche le bord bas » n'était que du texte de
// panneau, pas une planète coupée.
const bare = OUT.replace(/\.png$/, "-scene.png");
await send("Runtime.evaluate", {
  expression: `(() => { const s = document.createElement('style');
    s.id = '__bare';
    s.textContent = '.ov-panel,.echelle-curseur,.chip,.zoomctl,.ask-msg{display:none!important}';
    document.head.appendChild(s); return true; })()`,
});
await new Promise((r) => setTimeout(r, 900));
const shot2 = await send("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
if (shot2?.data) {
  writeFileSync(bare, Buffer.from(shot2.data, "base64"));
  console.log("capture scène nue :", bare);
}
await j("/json/close/" + tab.id);
ws.close();

// ── MESURE DE LA SCÈNE NUE ────────────────────────────────────────────────
// Une scène blanche ou quasi vide ne produit AUCUNE erreur JS : elle passe
// tous les autres contrôles. On mesure donc les pixels (via ffmpeg, présent
// sur la machine ; sans lui on l'annonce au lieu de faire semblant).
let pixelFail = false;
if (shot2?.data) {
  try {
    const { execFileSync } = await import("node:child_process");
    const W = 1440, H = 757;
    const raw = execFileSync("ffmpeg", ["-v", "quiet", "-i", bare, "-f", "rawvideo", "-pix_fmt", "rgb24", "-"],
      { maxBuffer: 1 << 28 });
    let content = 0, bright = 0, sx = 0, sy = 0, sn = 0;
    for (let y = 0; y < H; y++) {
      for (let x = 0; x < W; x++) {
        const i = (y * W + x) * 3;
        const dr = Math.abs(raw[i] - 5) + Math.abs(raw[i + 1] - 7) + Math.abs(raw[i + 2] - 12);
        if (dr > 26) content++;
        const l = 0.2126 * raw[i] + 0.7152 * raw[i + 1] + 0.0722 * raw[i + 2];
        if (l > 150) { bright++; sx += x; sy += y; sn++; }
      }
    }
    const pct = (content * 100) / (W * H);
    const bpct = (bright * 100) / (W * H);
    const cx = sn ? sx / sn : 0, cy = sn ? sy / sn : 0;
    const cxb = Math.abs(cx - W / 2) / W, cyb = Math.abs(cy - H / 2) / H;
    console.log(`scène nue : contenu ${pct.toFixed(1)} % · lumineux ${bpct.toFixed(2)} % · ` +
      `centre de lumière (${cx.toFixed(0)},${cy.toFixed(0)}) soit ${(cxb * 100).toFixed(1)} %/${(cyb * 100).toFixed(1)} % d'écart`);
    if (pct < 6) { console.error(`ÉCHEC : scène quasi vide (${pct.toFixed(1)} % de contenu)`); pixelFail = true; }
    if (bpct < 0.5) { console.error(`ÉCHEC : presque rien de lumineux (${bpct.toFixed(2)} %) — étoile absente ?`); pixelFail = true; }
    if (cxb > 0.2 || cyb > 0.25) {
      console.error(`ÉCHEC : le point lumineux principal n'est pas centré (foyer hors cadre ?)`);
      pixelFail = true;
    }
    console.log("mesure de scène :", pixelFail ? "ÉCHEC" : "OK");
  } catch (e) {
    console.log("mesure de scène : ignorée (ffmpeg indisponible) —", e?.message?.slice(0, 60));
  }
}

process.exit(errors.length || !p.canvas || pixelFail ? 1 : 0);
