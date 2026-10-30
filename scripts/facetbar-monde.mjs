#!/usr/bin/env node
// ── LA FRATRIE SE MONTRE-T-ELLE DANS L'ÉTAT PAR DÉFAUT ? ────────────────────
// État par défaut mesuré : face « Monde », « ⟲ morph » ACTIF, scène StageContinu.
// Avant le branchement : `.facetbar` ABSENTE quoi qu'on fasse (App exigeait
// `!monde`, et Graph3D n'est monté que « morph » inactif). Après : un clic sur un
// ENSEMBLE de la scène publie son graphe et son centre, et la barre doit se monter.
//
// Cette sonde ne pose AUCUN état à la main : elle clique de vraies coordonnées du
// canvas et lit le DOM.
//
// Usage : node scripts/facetbar-monde.mjs [url] [cdp]  (défaut : instance isolée 8433)
import { argv, exit } from "node:process";
const URL_PAGE = argv[2] || "http://127.0.0.1:8433/";
const CDP = argv[3] || "http://127.0.0.1:9423";
const dormir = (ms) => new Promise((r) => setTimeout(r, ms));

const j = async (p, init) => { const r = await fetch(CDP + p, init); const t = await r.text(); try { return JSON.parse(t); } catch { return t; } };
let cible = await j("/json/new?" + encodeURIComponent(URL_PAGE), { method: "PUT" });
if (typeof cible === "string" || !cible?.webSocketDebuggerUrl) {
  const l = await j("/json/list"); cible = Array.isArray(l) ? l.find((t) => t.type === "page") : null;
}
if (!cible?.webSocketDebuggerUrl) { console.log("ÉCHEC  aucune cible CDP"); exit(2); }

const ws = new WebSocket(cible.webSocketDebuggerUrl);
const att = new Map(); let id = 0;
const envoyer = (m, p = {}) => new Promise((res) => { const n = ++id; att.set(n, res); ws.send(JSON.stringify({ id: n, method: m, params: p })); });
ws.addEventListener("message", (e) => { const m = JSON.parse(e.data); if (m.id && att.has(m.id)) { att.get(m.id)(m.result); att.delete(m.id); } });
await new Promise((r) => ws.addEventListener("open", r));
await envoyer("Runtime.enable"); await envoyer("Page.enable");
const ev = async (x) => { const r = await envoyer("Runtime.evaluate", { expression: x, returnByValue: true, awaitPromise: true }); return r?.result?.value; };

let ko = 0;
const dit = (ok, nom, d) => { if (!ok) ko++; console.log(`${ok ? "PASS" : "FAIL"}  ${nom.padEnd(46)} ${d}`); };

// 1. Charger jusqu'à ce que l'application soit montée (le veilleur reconstruit).
let monte = false;
for (let e = 0; e < 5 && !monte; e++) {
  if (e === 0) await envoyer("Page.navigate", { url: URL_PAGE });
  else { try { await envoyer("Page.reload", { ignoreCache: true }); } catch {} }
  for (let i = 0; i < 10 && !monte; i++) { await dormir(3000); monte = (await ev(`(document.querySelector('#root')?.children.length ?? 0) > 0`)) === true; }
}
dit(monte, "l'application est montée", monte ? "oui" : "page blanche (reconstructions)");
if (!monte) exit(1);

// 2. L'état par défaut, tel qu'il est.
const etat = await ev(`JSON.stringify({
  faces: [...document.querySelectorAll('button')].filter(b=>/Cosmos|Monde|Graphe/.test(b.textContent)).map(b=>b.className),
  morph: (() => { const m = [...document.querySelectorAll('button')].find(b=>/morph/i.test(b.textContent)); return m ? m.className : null; })(),
  facetbar: !!document.querySelector('.facetbar'), canvas: document.querySelectorAll('canvas').length })`);
console.log("  état par défaut : " + etat);
const e0 = JSON.parse(etat || "{}");
dit(e0.facetbar === false, "au chargement, aucune barre (rien n'est encore désigné)", `.facetbar=${e0.facetbar}`);
dit(e0.canvas > 0, "la scène du Monde est montée", `canvas=${e0.canvas}`);

// 3. Cliquer une GRILLE de coordonnées réelles et guetter la barre.
const essai = await ev(`(async () => {
  const dormir = (ms) => new Promise(r => setTimeout(r, ms));
  const canvas = document.querySelector('canvas');
  if (!canvas) return { err: 'pas de canvas' };
  const b = canvas.getBoundingClientRect();
  const lire = () => {
    const fb = document.querySelector('.facetbar');
    if (!fb) return null;
    // TOUT le texte de la barre, plus le bandeau d'effectifs : la première
    // version ne lisait que 200 caractères et jugeait « pas de fratrie » sur un
    // en-tête de nœud — une assertion trop étroite, pas un défaut du produit.
    const tot = document.querySelector('.facetbar-totaux');
    return { barre: (fb.innerText || '').replace(/\\s+/g, ' ').slice(0, 700),
             totaux: tot ? (tot.innerText || '').replace(/\\s+/g, ' ') : null };
  };
  const clics = [];
  for (const fy of [0.38, 0.44, 0.50, 0.56, 0.62, 0.30, 0.68]) {
    for (const fx of [0.50, 0.35, 0.65, 0.20, 0.80, 0.28, 0.72]) {
      const x = Math.round(b.width * fx), y = Math.round(b.height * fy);
      const o = { clientX: b.left + x, clientY: b.top + y, bubbles: true, cancelable: true, view: window };
      canvas.dispatchEvent(new MouseEvent('mousemove', o));
      canvas.dispatchEvent(new MouseEvent('mousedown', o));
      canvas.dispatchEvent(new MouseEvent('mouseup', o));
      canvas.dispatchEvent(new MouseEvent('click', o));
      await dormir(2200);   // laisser l'index de parenté se construire (différé après peinture)
      const t = lire();
      clics.push({ x, y });
      if (t) return { clique: { x, y }, texte: t, essais: clics.length };
    }
  }
  // ── ENSUITE LA GRAMMAIRE CLAVIER (si les clics n'ont rien donné), QUI EST DÉTERMINISTE ────────────────────
  // Cliquer une scène 3D à coordonnées FIXES est une loterie : deux passes
  // identiques ont donné « montée au 3ᵉ clic » puis « jamais montée en 49 clics »,
  // parce que la scène se réagence selon les données chargées. La scène publie sa
  // grammaire (« ←/→ parcourir ») : on essaie d'abord des touches, qui traversent
  // les ensembles dans un ordre défini, puis on retombe sur la grille de clics.
  for (let k = 0; k < 14; k++) {
    const touche = k % 2 ? 'ArrowLeft' : 'ArrowRight';
    for (const cible of [document, window]) {
      cible.dispatchEvent(new KeyboardEvent('keydown', { key: touche, code: touche, bubbles: true }));
      cible.dispatchEvent(new KeyboardEvent('keyup', { key: touche, code: touche, bubbles: true }));
    }
    await dormir(1600);
    const t = lire();
    if (t) return { parTouche: touche, essais: k + 1, texte: t };
  }
  return { clique: null, essais: clics.length, texte: lire() };
})()`);
console.log("  essai   : " + JSON.stringify(essai).slice(0, 420));
dit(!!(essai?.clique || essai?.parTouche), "désigner un ensemble FAIT MONTER la barre de facettes",
  essai?.parTouche ? `par la touche ${essai.parTouche} (${essai.essais} appui(s))`
    : (essai?.clique ? `par clic en (${essai.clique.x}, ${essai.clique.y})` : `jamais montée (${essai?.essais ?? 0} essais)`));
const txt = (essai?.texte?.barre || "") + " " + (essai?.texte?.totaux || "");
dit(/fr[èe]res/i.test(txt), "la barre DIT la fratrie (effectifs)", (essai?.texte?.totaux || txt).slice(0, 170) || "rien");
console.log(`\n${ko === 0 ? "MODE PAR DÉFAUT : LA FRATRIE SE MONTRE" : `MODE PAR DÉFAUT : ${ko} ÉCHEC(S)`}`);
try { await envoyer("Page.close"); } catch {}
ws.close();
exit(ko === 0 ? 0 : 1);
