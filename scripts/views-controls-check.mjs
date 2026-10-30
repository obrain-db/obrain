#!/usr/bin/env node
// Vérifie que la GRAMMAIRE DE CONTRÔLES est réellement commune aux vues.
//
// Deux contrôles, pour deux parties de la demande :
//  1. CONTRÔLES — pour chaque vue, on envoie une touche de la grammaire
//     commune (←/→ parcourir) et on exige un ÉTAT OBSERVÉ dans le DOM
//     (« Monde visé » côté Cosmos, « Étage base » côté Atlas). Sans cette
//     exigence, une touche morte passerait inaperçue : elle ne produit
//     aucune erreur.
//  2. REPRÉSENTATION — chaque vue doit afficher le MÊME rappel de grammaire.
//
// Pourquoi pas un différentiel d'images : essayé, puis écarté. Dans les vues
// « morph », la scène bouge de 17 à 24 % EN PERMANENCE (sous-graphes qui se
// chargent et s'animent) ; « − » y donnait un gain de −0,22 %, indiscernable
// du fond animé. Le différentiel est conservé à titre informatif seulement.
//
// Usage : node scripts/views-controls-check.mjs [base] [chrome-cdp]
import { execFileSync } from "node:child_process";
import { writeFileSync, unlinkSync } from "node:fs";

const BASE = process.argv[2] || "http://192.168.1.103:8432/";
const CDP = process.argv[3] || "http://127.0.0.1:9248";
const W = 1440, H = 757;
const GEOM = `${W}x${H}`;

const j = async (p, init) => {
  const r = await fetch(CDP + p, init);
  const t = await r.text();
  try { return JSON.parse(t); } catch { return t; }
};

/**
 * Mesure d'une capture : le contenu se construit-il, se peuple-t-il, et le
 * sujet reste-t-il dans le cadre ? Trois seuils, volontairement bas : ils
 * attrapent une scène blanche ou vide, pas une question de goût.
 */
function measureScene(file) {
  const raw = execFileSync("ffmpeg", ["-v", "quiet", "-i", file, "-f", "rawvideo",
    "-pix_fmt", "rgb24", "-s", GEOM, "-"], { maxBuffer: 1 << 28 });
  let content = 0, bright = 0, sx = 0, sy = 0, sn = 0;
  for (let y = 0; y < H; y++) {
    for (let x = 0; x < W; x++) {
      const i = (y * W + x) * 3;
      const d = Math.abs(raw[i] - 5) + Math.abs(raw[i + 1] - 7) + Math.abs(raw[i + 2] - 12);
      if (d > 26) content++;
      const l = 0.2126 * raw[i] + 0.7152 * raw[i + 1] + 0.0722 * raw[i + 2];
      if (l > 150) { bright++; sx += x; sy += y; sn++; }
    }
  }
  const n = W * H;
  return {
    content: (content * 100) / n,
    bright: (bright * 100) / n,
    cx: sn ? sx / sn / W : 0.5,
    cy: sn ? sy / sn / H : 0.5,
  };
}

/** Différence de pixels entre deux PNG (informatif). */
function diffPct(a, b) {
  const dec = (f) => execFileSync("ffmpeg", ["-v", "quiet", "-i", f, "-f", "rawvideo",
    "-pix_fmt", "rgb24", "-s", GEOM, "-"], { maxBuffer: 1 << 28 });
  const A = dec(a), B = dec(b);
  let n = 0;
  for (let i = 0; i < A.length; i += 3) {
    if (Math.abs(A[i] - B[i]) + Math.abs(A[i + 1] - B[i + 1]) + Math.abs(A[i + 2] - B[i + 2]) > 24) n++;
  }
  return (n * 100) / (A.length / 3);
}

/**
 * Chaque vue doit DIRE où la navigation l'a menée ; on compare donc une
 * SIGNATURE avant/après la touche, et non une apparition de texte.
 *
 * Pourquoi : les vues Atlas arrivent avec un foyer DÉJÀ posé (la base active),
 * donc « Étage base » est présent avant même la touche — exiger son apparition
 * faisait échouer un contrôle alors que la touche agissait (gain mesuré
 * +10,3 %) . Comparez ce qui EST nommé, pas ce qui apparaît.
 */
const VIEWS = [
  { name: "Cosmos", btn: "🌌 Cosmos", key: "ArrowRight",
    sig: (t) => (t.match(/Monde visé\s*:\s*([^\n—]+)/) || [])[1] || "",
    what: "monde visé" },
  { name: "Monde", btn: "🧠 Monde", key: "ArrowRight",
    sig: (t) => (t.match(/Étage base\s*:\s*(\S+)/) || [])[1] || "",
    what: "base au foyer" },
  { name: "Graphe", btn: "🔺 Graphe", key: "ArrowRight",
    sig: (t) => (t.match(/Étage base\s*:\s*(\S+)/) || [])[1] || "",
    what: "base au foyer" },
];

const results = [];

for (const v of VIEWS) {
  const tab = await j("/json/new?" + encodeURIComponent(BASE), { method: "PUT" });
  if (!tab?.webSocketDebuggerUrl) {
    results.push({ name: v.name, cdpErr: "onglet CDP refusé" });
    continue;
  }
  const ws = new WebSocket(tab.webSocketDebuggerUrl);
  let id = 0;
  const pending = new Map();
  const errors = [];
  const send = (method, params = {}) =>
    new Promise((res) => { const i = ++id; pending.set(i, res); ws.send(JSON.stringify({ id: i, method, params })); });
  ws.addEventListener("message", (ev) => {
    const m = JSON.parse(ev.data);
    if (m.id && pending.has(m.id)) { pending.get(m.id)(m.result); pending.delete(m.id); return; }
    if (m.method === "Runtime.exceptionThrown") errors.push(m.params.exceptionDetails.text);
    if (m.method === "Log.entryAdded" && m.params.entry.level === "error") errors.push(m.params.entry.text);
  });
  await new Promise((r) => ws.addEventListener("open", r, { once: true }));
  await send("Runtime.enable"); await send("Log.enable"); await send("Page.enable");
  await new Promise((r) => setTimeout(r, 7000)); // chargement + première scène

  // bascule sur la vue par son bouton (le vrai geste utilisateur)
  const clicked = await send("Runtime.evaluate", {
    returnByValue: true,
    expression: `(() => { const b = [...document.querySelectorAll('button')]
      .find(x => (x.textContent || '').includes(${JSON.stringify(v.btn)}));
      if (!b) return false; b.click(); return true; })()`,
  });
  await new Promise((r) => setTimeout(r, 4000));

  const dom = async () => (await send("Runtime.evaluate", {
    returnByValue: true, expression: "document.body.innerText",
  }))?.result?.value || "";

  const before = await dom();

  // ── SCÈNE NUE (panneaux masqués) : se construit-elle, se peuple-t-elle ? ──
  await send("Runtime.evaluate", {
    expression: `(() => { const s = document.createElement('style'); s.id = '__bare';
      s.textContent = '.ov-panel,.echelle-curseur,.chip,.zoomctl,.zoomhint,.qb-lbl,.ask-msg{display:none!important}';
      document.head.appendChild(s); return true; })()`,
  });
  await new Promise((r) => setTimeout(r, 900));
  const bareFile = `/tmp/vc-${v.name}-scene.png`;
  const shotBare = await send("Page.captureScreenshot", { format: "png" });
  writeFileSync(bareFile, Buffer.from(shotBare.data, "base64"));
  let scene = null, sceneErr = null;
  try { scene = measureScene(bareFile); } catch (e) { sceneErr = e?.message?.slice(0, 60); }
  await send("Runtime.evaluate", {
    expression: `document.getElementById('__bare')?.remove()`,
  });
  await new Promise((r) => setTimeout(r, 500));

  // différentiel informatif : fond animé, puis effet de la touche
  const A = `/tmp/ctl-${v.name}-a.png`, B = `/tmp/ctl-${v.name}-b.png`;
  const C = `/tmp/ctl-${v.name}-c.png`, D = `/tmp/ctl-${v.name}-d.png`;
  let base = 0, pct = 0, err = null;
  try {
    const s0 = await send("Page.captureScreenshot", { format: "png" });
    writeFileSync(C, Buffer.from(s0.data, "base64"));
    await new Promise((r) => setTimeout(r, 1400));
    const s1 = await send("Page.captureScreenshot", { format: "png" });
    writeFileSync(D, Buffer.from(s1.data, "base64"));
    base = diffPct(C, D);
    const sA = await send("Page.captureScreenshot", { format: "png" });
    writeFileSync(A, Buffer.from(sA.data, "base64"));
  } catch (e) { err = e?.message?.slice(0, 60); }

  await send("Input.dispatchKeyEvent", { type: "rawKeyDown", key: v.key, code: v.key });
  await send("Input.dispatchKeyEvent", { type: "keyUp", key: v.key, code: v.key });
  await new Promise((r) => setTimeout(r, 1600));

  try {
    const sB = await send("Page.captureScreenshot", { format: "png" });
    writeFileSync(B, Buffer.from(sB.data, "base64"));
    pct = diffPct(A, B);
  } catch (e) { err = err || e?.message?.slice(0, 60); }

  // ÉTAT OBSERVÉ : la touche a-t-elle changé ce que la vue NOMME ?
  const after = await dom();
  const sig0 = v.sig(before), sig1 = v.sig(after);
  const acted = !!sig1 && sig1 !== sig0;
  const hint = after.includes("parcourir") && after.includes("densifier");
  // LÉGENDE UNIQUE : mêmes noms de plans, et ils sont bien nommés « plans ».
  const legend = after.includes("Trois plans") &&
    after.includes("connaissance") && after.includes("self") && after.includes("utilisateur");

  results.push({
    name: v.name, clicked: clicked?.result?.value, key: v.key, what: v.what,
    acted, sig0, sig1, hint, legend, diff: pct, base, errors: errors.length, cdpErr: err,
    scene, sceneErr,
  });

  await j("/json/close/" + tab.id);
  ws.close();
  for (const f of [A, B, C, D, `/tmp/vc-${v.name}-scene.png`]) { try { unlinkSync(f); } catch { /* ignoré */ } }
}

// ── CONTINUITÉ : « toutes les faces d'une même pièce » ─────────────────────
// On vise un monde dans le Cosmos, on bascule sur Monde, et on exige que le
// SUJET affiché soit le même. Quatre vues qui gardent quatre mémoires ne
// seraient que quatre peaux alignées ; ce test vérifie que c'est une pièce.
let continuity = null;
{
  const subj = (t) => (t.match(/Sujet\s*:\s*([^\n—]+)/) || [])[1]?.trim() || "";
  const tab = await j("/json/new?" + encodeURIComponent(BASE), { method: "PUT" });
  if (tab?.webSocketDebuggerUrl) {
    const ws = new WebSocket(tab.webSocketDebuggerUrl);
    let id = 0; const pending = new Map();
    const send = (m, p = {}) => new Promise((r) => { const i = ++id; pending.set(i, r); ws.send(JSON.stringify({ id: i, method: m, params: p })); });
    ws.addEventListener("message", (ev) => {
      const m = JSON.parse(ev.data);
      if (m.id && pending.has(m.id)) { pending.get(m.id)(m.result); pending.delete(m.id); }
    });
    await new Promise((r) => ws.addEventListener("open", r, { once: true }));
    await send("Page.enable"); await send("Runtime.enable");
    await new Promise((r) => setTimeout(r, 8000));
    const txt = async () => (await send("Runtime.evaluate", { returnByValue: true, expression: "document.body.innerText" }))?.result?.value || "";
    const clickView = async (label) => {
      await send("Runtime.evaluate", { expression: `[...document.querySelectorAll('button')].find(b=>(b.textContent||'').includes(${JSON.stringify(label)}))?.click()` });
      await new Promise((r) => setTimeout(r, 3500));
    };
    await clickView("🌌 Cosmos");
    const vide = subj(await txt());
    await send("Input.dispatchKeyEvent", { type: "rawKeyDown", key: "ArrowRight", code: "ArrowRight" });
    await send("Input.dispatchKeyEvent", { type: "keyUp", key: "ArrowRight", code: "ArrowRight" });
    await new Promise((r) => setTimeout(r, 1800));
    const dansCosmos = subj(await txt());
    await clickView("🧠 Monde");
    const dansMonde = subj(await txt());
    continuity = { vide, dansCosmos, dansMonde, ok: !!dansCosmos && dansCosmos === dansMonde };
    await j("/json/close/" + tab.id); ws.close();
  }
}

let bad = 0;
console.log("GRAMMAIRE DE CONTRÔLES — une touche doit AGIR, et la vue doit le DIRE\n");
for (const r of results) {
  // « se construit, se peuple, reste dans son cadre »
  const sc = r.scene;
  const populated = !!sc && sc.content > 4 && sc.bright > 0.3;
  const inFrame = !!sc && sc.cx > 0.2 && sc.cx < 0.8 && sc.cy > 0.15 && sc.cy < 0.85;
  const ok = r.acted && r.hint && r.legend && !r.errors && r.clicked && populated && inFrame;
  if (!ok) bad++;
  const gain = (r.diff ?? 0) - (r.base ?? 0);
  console.log(
    `${ok ? "✓" : "✗"} ${String(r.name).padEnd(7)} « ${String(r.key).padEnd(10)} » ` +
    `${r.what} : ${r.sig0 ? `« ${String(r.sig0).slice(0, 18)} » → ` : ""}« ${String(r.sig1).slice(0, 18)} » ` +
    `${r.acted ? "✓" : "INCHANGÉ ✗"} · rappel ${r.hint ? "affiché" : "ABSENT"} · ` +
    `légende ${r.legend ? "unifiée" : "ABSENTE"} · ` +
    `erreurs ${r.errors} · (info pixels : effet ${(r.diff ?? 0).toFixed(1)} % / fond ${(r.base ?? 0).toFixed(1)} % / gain ${gain.toFixed(1)} %)` +
    (r.clicked === false ? " · VUE NON TROUVÉE" : "") + (r.cdpErr ? ` · ${r.cdpErr}` : ""),
  );
  console.log(
    `   scène nue : contenu ${sc ? sc.content.toFixed(1) + " %" : "?"} · lumineux ` +
    `${sc ? sc.bright.toFixed(2) + " %" : "?"} · sujet à (${sc ? (sc.cx * 100).toFixed(0) : "?"} %, ` +
    `${sc ? (sc.cy * 100).toFixed(0) : "?"} %) → ${populated ? "peuplée" : "VIDE"} / ` +
    `${inFrame ? "dans le cadre" : "HORS CADRE"}` + (r.sceneErr ? ` · ${r.sceneErr}` : ""),
  );
}
if (continuity) {
  if (!continuity.ok) bad++;
  console.log(
    `\n${continuity.ok ? "✓" : "✗"} PIÈCE UNIQUE : sujet ${continuity.dansCosmos ? `« ${continuity.dansCosmos} »` : "(aucun)"} ` +
    `dans le Cosmos → ${continuity.dansMonde ? `« ${continuity.dansMonde} »` : "(aucun)"} dans le Monde ` +
    `${continuity.ok ? "— la même pièce" : "— CONTINUITÉ ROMPUE"}`,
  );
}
console.log(`\n${bad ? `ÉCHEC : ${bad} contrôle(s) non conforme(s)` : "OK : les 3 vues partagent grammaire, chrome et sujet"}`);
process.exit(bad ? 1 : 0);
