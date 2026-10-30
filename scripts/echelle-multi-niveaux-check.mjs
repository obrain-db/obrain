#!/usr/bin/env node
// ── ÉCHELLE MULTI-NIVEAUX — LES CINQ EXIGENCES DU CANEVAS, MESURÉES ─────────
//
// Ce contrôle existe parce que les défauts qu'il couvre étaient tous INVISIBLES
// au code relu, et tous mesurables en trois minutes sur la page servie :
//
//  1. QUATRE CRANS D'ÉCHELLE — les quatre boutons du sélecteur doivent produire
//     quatre ÉCHELLES distinctes et MONOTONES de la même scène : espace, monde,
//     groupe, signaux. Défaut mesuré avant ce correctif :
//     « 🧠 Monde » et « 🔺 Graphe » laissaient `t = 0` et la distance caméra à
//     1141 → 1131 px (l'orbite au repos), le bouton actif restant « 🌌 Cosmos ».
//     Cause : `niveau` figé dans la fermeture de l'effet de scène.
//  2. CONTRÔLE DE DESCENTE VISIBLE — le seul bouton de descente à la souris
//     vivait en bas du panneau qui défile (contenu 1251 px pour 588 px visibles
//     en 757 px de haut) : mesuré à y = 805, `elementFromPoint` répondait « HORS
//     ÉCRAN » et le clic ne produisait rien.
//  3. ON PEUT TOUJOURS DESCENDRE — chaque geste annoncé doit augmenter l'échelle
//     depuis l'espace, en particulier `↓` (« ↓ entrer »), qui appelait un
//     changement de base et laissait `t` à 12 % avant comme après.
//  4. LES GROUPES S'AUTO-SEGMENTENT — l'étage intérieur d'un monde porte
//     plusieurs ensembles RÉELS (effectifs) et l'axe de groupement est offert.
//  5. FLUIDITÉ — la descente prend des valeurs intermédiaires, sans saut.
//  6. LE QUATRIÈME CRAN EXISTE VRAIMENT — l'espace des signaux (courbure ·
//     énergie · degré) doit s'exécuter, avec ses DEUX origines de disposition.
//  7. LE POLYMORPHISME — le même geste doit traverser d'AUTRES relations que la
//     descendance (communauté Leiden, contour ∂, parenté, extension, strate).
//
// Usage : node scripts/echelle-multi-niveaux-check.mjs [base]
// Env   : CDP_URL (défaut http://127.0.0.1:9248)
import { writeFileSync } from "node:fs";

const BASE = process.argv[2] || "http://192.168.1.103:8432";
const CDP = process.env.CDP_URL || "http://127.0.0.1:9248";
const W = 1440, H = 757;          // la fenêtre des autres contrôles de ce dépôt
const ATTENTE_MS = Number(process.env.ATTENTE_MS || 12000);

const j = async (p, init) => {
  const r = await fetch(CDP + p, init);
  const t = await r.text();
  try { return JSON.parse(t); } catch { return t; }
};

const tab = await j("/json/new?" + encodeURIComponent(BASE + "/?cosmos=1&echelle=" + Date.now()), { method: "PUT" });
if (!tab?.webSocketDebuggerUrl) {
  console.error("échelle : onglet CDP refusé — le chrome de la machine est-il sur " + CDP + " ?");
  process.exit(2);
}
const ws = new WebSocket(tab.webSocketDebuggerUrl);
let id = 0;
const pending = new Map();
const erreurs = [];
const httpKO = [];
/** Les appels de NIVEAU observés (`/api/viz/space`) : sert à prouver qu'un
 *  « densifier » en mémoire n'en émet AUCUN. */
const appelsSpace = [];
/**
 * ── UNE SESSION CDP MORTE DOIT SE DIRE, PAS SE FIGER ────────────────────────
 *
 * Constaté en lançant ce contrôle plusieurs fois de suite : le chrome de mesure
 * finit par mourir (rendu logiciel `swiftshader` + plusieurs scènes WebGL
 * lourdes chargées d'affilée). Le script s'arrêtait alors sur un
 * « unsettled top-level await » — un message qui ne dit ni ce qui s'est passé ni
 * quoi faire. On détecte donc la fermeture de la session et on REFUSE d'attendre
 * une réponse qui ne viendra jamais : l'erreur nomme la cause et l'issue.
 */
let cdpMorte = null;
const send = (m, p = {}) => new Promise((résoudre, rejeter) => {
  if (cdpMorte) { rejeter(new Error(cdpMorte)); return; }
  const i = ++id;
  pending.set(i, résoudre);
  try { ws.send(JSON.stringify({ id: i, method: m, params: p })); }
  catch (e) { pending.delete(i); rejeter(e); }
});
ws.addEventListener("close", () => {
  cdpMorte = "session CDP fermée en cours de route — le chrome de mesure (" + CDP +
    ") a probablement été tué. Relancer un chrome neuf, puis ce contrôle.";
  for (const r of pending.values()) r(undefined);
  pending.clear();
});
ws.addEventListener("message", (ev) => {
  const m = JSON.parse(ev.data);
  if (m.id && pending.has(m.id)) { pending.get(m.id)(m.result); pending.delete(m.id); return; }
  if (m.method === "Runtime.exceptionThrown") erreurs.push("EXCEPTION " + (m.params.exceptionDetails.text || ""));
  /**
   * ── LES `console.error` COMPTENT, ET ILS M'ONT ÉCHAPPÉ ─────────────────────
   * MESURÉ : une vue tombée dans une FRONTIÈRE D'ERREUR React (#300, un hook
   * déclaré après un retour anticipé) produisait une page vide — pas de canevas,
   * pas de barre, pas de mesure — et ce contrôle la déclarait « 23 exigences non
   * tenues » sans jamais dire POURQUOI : React signale l'exception par
   * `console.error`, que je ne collectais pas (`Runtime.exceptionThrown`, lui, ne
   * voit pas ce qu'une frontière attrape).
   * Un contrôle qui compte les échecs sans nommer la cause ne sert qu'à moitié :
   * on collecte donc aussi les `console.error`.
   */
  if (m.method === "Runtime.consoleAPICalled" && m.params.type === "error") {
    const t = (m.params.args || []).map((x) => x.value ?? x.description ?? x.type).join(" ").slice(0, 300);
    if (/React error|Cannot read|is not a function|undefined is not/i.test(t)) erreurs.push("CONSOLE " + t);
  }
  if (m.method === "Log.entryAdded" && m.params.entry.level === "error") erreurs.push("console " + m.params.entry.text);
  // ── LES REQUÊTES DE NIVEAU, COMPTÉES : « densifier » doit être GRATUIT ────
  // La loi de `densite.ts` affirme que densifier un niveau est un REDESSIN (la
  // matière est déjà reçue) tant que la page du serveur n'est pas pleine. Cette
  // affirmation est mesurable : si un `/api/viz/space` part au moment du geste,
  // alors ce n'était pas un redessin et la phrase du panneau mentait.
  if (m.method === "Network.requestWillBeSent" && /\/api\/viz\/space/.test(m.params.request.url)) {
    appelsSpace.push(m.params.request.url.replace(/^https?:\/\/[^/]+/, ""));
  }
  if (m.method === "Network.responseReceived" && m.params.response.status >= 400) {
    // 400/404 ATTENDUS et traités (voir §« Ce qui est connu » plus bas) : le
    // pont d'état `/ui/state/*` n'existe pas sur `viz serve`, et l'entrée par un
    // nom de FAMILLE est désormais résolue AVANT l'appel.
    const u = m.params.response.url;
    if (!/\/ui\/state\//.test(u)) httpKO.push(m.params.response.status + " " + u.replace(BASE, ""));
  }
});
await new Promise((r) => ws.addEventListener("open", r, { once: true }));
await send("Runtime.enable"); await send("Log.enable"); await send("Page.enable"); await send("Network.enable");
/**
 * ── LE SERVEUR SERT DERRIÈRE UN IAM (2026-09-15) ─────────────────────────────
 * `obrain viz serve` protège désormais `/api/*` : sans identité, il répond
 * `401 {"error":"aucune identité présentée (en-tête Authorization: Bearer
 * <jeton>…)"}` et `/` renvoie `303 → /login`. Un contrôle qui ne présente rien
 * mesurerait une page vide et conclurait « le canevas ne marche plus » — un
 * verdict FAUX sur une application qui va très bien.
 *
 * On ne devine donc ni mot de passe ni jeton : s'il est fourni par
 * l'environnement (`OBRAIN_TOKEN`), on le présente à TOUTES les requêtes de
 * l'onglet ; sinon on le DIT au lieu de mesurer dans le vide.
 */
const JETON = process.env.OBRAIN_TOKEN || "";
const COOKIE = process.env.OBRAIN_COOKIE || "";
if (JETON || COOKIE) {
  const headers = JETON ? { authorization: `Bearer ${JETON}` } : { cookie: COOKIE };
  await send("Network.setExtraHTTPHeaders", { headers });
  console.log(`  (identité présentée : ${JETON ? "OBRAIN_TOKEN" : "OBRAIN_COOKIE"})\n`);
}
await send("Page.navigate", { url: BASE + "/?cosmos=1&echelle=" + Date.now() });
const dormir = (ms) => new Promise((r) => setTimeout(r, ms));
await dormir(ATTENTE_MS);

await send("Emulation.setDeviceMetricsOverride", { width: W, height: H, deviceScaleFactor: 1, mobile: false });
await dormir(1500);

const ev = async (expr) => (await send("Runtime.evaluate", { returnByValue: true, expression: expr, awaitPromise: true }))?.result?.value;
/** La page est-elle l'écran de connexion (IAM) ? On le DIT, on ne mesure pas. */
const estLogin = async () => {
  const t = (await (async () => (await send("Runtime.evaluate", { returnByValue: true, expression: "document.body.innerText" }))?.result?.value)()) || "";
  return /Connexion à obrain|connexion requise/i.test(t);
};
const txt = async () => (await ev("document.body.innerText")) || "";

/** Ce que la scène DIT d'elle-même : mot d'échelle, pourcentage, distance. */
const echelle = async () => {
  const t = await txt();
  const ligne = (t.match(/Descente\s*:\s*([^\n]+)/) || [])[1] || "";
  const pct = Number((ligne.match(/(\d+)\s*%/) || [])[1]);
  const inter = (t.match(/Intérieur\s*:\s*([^\n]+)/) || [])[1]?.trim() || null;
  const gr = (t.match(/Graphe\s*:\s*([^\n]+)/) || [])[1]?.trim() || null;
  const e = JSON.parse((await ev("JSON.stringify(window.__etages||null)")) || "null") || {};
  const monde = e.monde || {};
  /**
   * Le LIBELLÉ du cran allumé, et lui seul : depuis que le curseur porte la
   * donnée (`éc-donnees`), l'`innerText` du cran fait DEUX lignes — le libellé
   * puis « 450 662 nœuds · 1 349 794 arêtes ». Comparer le tout rendait les
   * verdicts illisibles et fragiles. On ne lit que la première ligne.
   */
  const vue = await ev(`(()=>{const b=document.querySelector('.ec-cran.on'); return b? (b.innerText||'').split('\\n')[0].trim() : '';})()`);
  return { ligne: ligne.trim(), pct: Number.isFinite(pct) ? pct : 0, inter, gr, camDist: monde.camDist, vue };
};

const clicSel = async (sel) => {
  const box = await ev(`(() => { const e=document.querySelector(${JSON.stringify(sel)}); if(!e) return null;
    const r=e.getBoundingClientRect(); return JSON.stringify({x:Math.round(r.x+r.width/2),y:Math.round(r.y+r.height/2)}); })()`);
  if (!box) return false;
  const { x, y } = JSON.parse(box);
  await send("Input.dispatchMouseEvent", { type: "mousePressed", x, y, button: "left", clickCount: 1 });
  await send("Input.dispatchMouseEvent", { type: "mouseReleased", x, y, button: "left", clickCount: 1 });
  return true;
};
const clicTexte = async (label) => {
  const box = await ev(`(() => { const b=[...document.querySelectorAll('button')].find(x=>x.innerText.trim().includes(${JSON.stringify(label)})); if(!b) return null;
    const r=b.getBoundingClientRect(); return JSON.stringify({x:Math.round(r.x+r.width/2),y:Math.round(r.y+r.height/2)}); })()`);
  if (!box) return false;
  const { x, y } = JSON.parse(box);
  await send("Input.dispatchMouseEvent", { type: "mousePressed", x, y, button: "left", clickCount: 1 });
  await send("Input.dispatchMouseEvent", { type: "mouseReleased", x, y, button: "left", clickCount: 1 });
  return true;
};
const VK = { ArrowDown: 40, ArrowUp: 38, ArrowLeft: 37, ArrowRight: 39, Enter: 13, Home: 36, Equal: 187, KeyD: 68 };
const touche = async (k, code) => {
  const cd = code || k;
  await send("Input.dispatchKeyEvent", { type: "keyDown", key: k, code: cd, windowsVirtualKeyCode: VK[cd] });
  await send("Input.dispatchKeyEvent", { type: "keyUp", key: k, code: cd, windowsVirtualKeyCode: VK[cd] });
  await dormir(800);
};
const molette = async (dy) => {
  await send("Input.dispatchMouseEvent", { type: "mouseWheel", x: 720, y: 420, deltaX: 0, deltaY: dy });
  await dormir(500);
};
const retourEspace = async () => { await touche("Home"); await dormir(2600); };

const lignes = [];
const dit = (s) => { console.log(s); lignes.push(s); };
let KO = 0;
const porte = (ok, nom, detail) => {
  dit(`  ${ok ? "✓" : "✗"} ${nom}${detail ? " — " + detail : ""}`);
  if (!ok) KO++;
  return ok;
};

dit("ÉCHELLE MULTI-NIVEAUX — le canevas servi, mesuré\n");
if (await estLogin()) {
  dit("✗ LA PAGE DEMANDE UNE CONNEXION — ce contrôle ne mesure rien sans identité.");
  dit("  Le serveur sert derrière un IAM (`/api/auth/login`). Deux issues :");
  dit("    · ouvrir la session dans un navigateur, puis relancer ce contrôle ;");
  dit("    · ou fournir une identité, sans jamais la coller dans un fichier :");
  dit("        curl -s -X POST --data 'username=<compte>&password=<mdp>' -c /tmp/obrain-cookie.txt \\");
  dit("             http://192.168.1.103:8432/api/auth/login");
  dit("        OBRAIN_COOKIE=\"$(grep -o 'obrain[^\t]*$' /tmp/obrain-cookie.txt)\" node scripts/echelle-multi-niveaux-check.mjs");
  dit("      (ou, avec un jeton : OBRAIN_TOKEN=<jeton> node scripts/echelle-multi-niveaux-check.mjs)");
  process.exit(2);
}
dit("Source        : " + BASE);
const bundle = (await ev(`(document.querySelector('script[src*="/app/assets/"]')||{}).src || ''`)).split("/").pop();
dit("Bundle servi : " + bundle + "\n");

// ── 1. PLUSIEURS VUES : trois échelles DISTINCTES de la même scène ──────────
dit("── 1. PLUSIEURS VUES — trois échelles distinctes de la même scène");
/**
 * ── LE CONTRÔLE N'ATTEND PAS UN DÉLAI : IL ATTEND L'EFFET ──────────────────
 *
 * MESURÉ (2026-09-15, 20:5x) : avec des attentes FIXES (3,2 à 5 s), ce bloc a
 * échoué deux fois de suite — « 0 % / 7 % / 8 % / 8 % », bouton actif resté
 * « 🌌 Cosmos » — alors que le produit allait bien : un essai ciblé, sur le MÊME
 * bundle, changeait de cran et descendait le Monde normalement. La cause est le
 * temps de CHARGEMENT de la scène, qui dépend de l'état du chrome (plusieurs
 * scènes WebGL lourdes se succèdent dans cet onglet) : un relevé sec conclut sur
 * l'instant où l'on est passé, pas sur l'état. C'est la leçon déjà écrite dans ce
 * fichier pour le §4 et pour le chemin du §8 ; on l'applique ici aussi.
 */
const attendreVue = async (motifAttendu, max = 14) => {
  let dernier = await echelle();
  for (let i = 0; i < max; i++) {
    if (motifAttendu.test(String(dernier.vue))) return dernier;
    await dormir(2500);
    dernier = await echelle();
  }
  return dernier;   // on rend la DERNIÈRE mesure : le verdict la jugera, sans mentir
};
/**
 * ── UN CRAN N'EST ATTEINT QUE QUAND LA CAMÉRA S'ARRÊTE ─────────────────────
 *
 * MESURÉ (2026-09-16) : après avoir rendu la main dès que le VOYANT changeait,
 * ce bloc a relevé `camDist` 536 / 519 / 359 pour les crans monde / groupe /
 * signaux — alors que la caméra, une fois posée, est à ~69. Le voyant change dès
 * que le palier bascule, c'est-à-dire PENDANT le vol (0,42 s de course) : on
 * mesurait une transition, pas un état. On attend donc AUSSI que la distance se
 * stabilise (deux relevés à moins de 2 % l'un de l'autre), borné comme le reste.
 */
const attendreArret = async (motifAttendu, max = 24) => {
  let dernier = await echelle();
  let precedent = null;
  for (let i = 0; i < max; i++) {
    const vu = motifAttendu.test(String(dernier.vue));
    const d = Number(dernier.camDist);
    const stable = precedent !== null && Number.isFinite(d) && Number.isFinite(precedent)
      && Math.abs(d - precedent) < Math.max(1, Math.abs(precedent) * 0.02);
    if (vu && stable) return dernier;
    precedent = d;
    await dormir(2000);
    dernier = await echelle();
  }
  return dernier;
};
const choisirVue = async (n, motif) => {
  await clicSel(`.ec-cran:nth-child(${n})`);
  await dormir(2500);
  return attendreArret(motif);
};
const vCosmos = await choisirVue(1, /Espace|Cosmos/);
const vMonde = await choisirVue(2, /Monde/);
const vGraphe = await choisirVue(3, /Groupe|Graphe/);
const vSignaux = await choisirVue(4, /Signaux/);
const vRetour = await choisirVue(1, /Espace|Cosmos/);
for (const [nom, v] of [["🌌 Cosmos", vCosmos], ["🧠 Monde ", vMonde], ["🔺 Groupe", vGraphe], ["🧬 Signaux", vSignaux], ["🌌 retour", vRetour]]) {
  dit(`  ${nom}  ${String(v.ligne).padEnd(28)} camDist=${String(v.camDist).padStart(8)}  bouton actif=${v.vue}`);
}
const distinctes = new Set([vCosmos.pct, vMonde.pct, vGraphe.pct, vSignaux.pct]).size;
// Le cran « graphe » est BORNÉ plus près que le cran « monde » (voir la borne
// `dMondeN * 0.7` dans la demande de niveau) : la descente doit donc être
// monotone, et c'est ce qu'on exige ici plutôt qu'un simple « ça bouge ».
const descend = vMonde.camDist < vCosmos.camDist * 0.9
  && vGraphe.camDist < vMonde.camDist * 0.9
  && vSignaux.camDist < vGraphe.camDist * 0.99;
porte(distinctes === 4, "QUATRE échelles distinctes",
  `${vCosmos.pct} % / ${vMonde.pct} % / ${vGraphe.pct} % / ${vSignaux.pct} %`);
porte(descend, "les quatre crans DESCENDENT et sont monotones",
  `camDist ${vCosmos.camDist} → ${vMonde.camDist} → ${vGraphe.camDist} → ${vSignaux.camDist}`);
porte(/Monde/.test(vMonde.vue) && /Groupe/.test(vGraphe.vue) && /Signaux/.test(vSignaux.vue) && /Espace|Cosmos/.test(vRetour.vue),
  "le bouton actif suit ce que la scène MONTRE",
  `Monde→« ${vMonde.vue} », Groupe→« ${vGraphe.vue} », Signaux→« ${vSignaux.vue} », retour→« ${vRetour.vue} »`);
// Le mot du panneau et le voyant doivent dire la MÊME chose (autorité unique).
porte(String(vSignaux.ligne).includes("signaux"),
  "le panneau nomme le quatrième cran « signaux »", String(vSignaux.ligne));

// ── 2. LE CONTRÔLE DE DESCENTE EST-IL ATTEIGNABLE ? ─────────────────────────
dit("\n── 2. LE CONTRÔLE DE DESCENTE EST VISIBLE ET ATTEIGNABLE (fenêtre 1440×757)");
const geo = JSON.parse(await ev(`(() => {
  const b=[...document.querySelectorAll('button')].find(x=>/descendre/.test(x.innerText));
  if(!b) return JSON.stringify({trouve:false});
  const r=b.getBoundingClientRect(); const cx=Math.round(r.x+r.width/2), cy=Math.round(r.y+r.height/2);
  const top=document.elementFromPoint(cx,cy);
  return JSON.stringify({trouve:true, texte:b.innerText.trim(), rect:[Math.round(r.x),Math.round(r.y),Math.round(r.width),Math.round(r.height)],
    bas:Math.round(r.bottom), hauteurVue:window.innerHeight, barre:!!b.closest('.echelle-bar'),
    recoit: top ? top.tagName + '.' + String(top.className||'').split(' ')[0] : 'HORS ÉCRAN',
    estLeBouton: !!top && (top===b || b.contains(top))});
})()`));
dit("  " + JSON.stringify(geo));
porte(geo.trouve && geo.bas <= geo.hauteurVue && geo.estLeBouton,
  "le bouton de descente est dans la fenêtre et reçoit le clic",
  geo.trouve ? `bas=${geo.bas} / vue=${geo.hauteurVue}, reçoit=${geo.recoit}` : "bouton introuvable");

// ── 3. ON PEUT TOUJOURS DESCENDRE ──────────────────────────────────────────
dit("\n── 3. ON PEUT TOUJOURS DESCENDRE — chaque geste annoncé, depuis l'espace");
const gestes = [
  ["molette ↑ ×4", async () => { for (let i = 0; i < 4; i++) await molette(-240); }],
  ["touche + ×4", async () => { for (let i = 0; i < 4; i++) await touche("+", "Equal"); }],
  ["↓ (entrer)", async () => { await touche("ArrowDown"); await dormir(3000); }],
  ["⤓ descendre", async () => { await clicTexte("descendre"); await dormir(3500); }],
];
const culs = [];
for (const [nom, geste] of gestes) {
  await retourEspace();
  const a = await echelle();
  await geste();
  const b = await echelle();
  const progresse = b.pct > a.pct + 2 || (!!b.gr && !a.gr) || b.camDist < a.camDist * 0.9;
  if (!progresse) culs.push(nom);
  dit(`  ${nom.padEnd(14)} ${String(a.pct).padStart(4)} % → ${String(b.pct).padStart(4)} %  (camDist ${a.camDist} → ${b.camDist})  ${progresse ? "✓ descend" : "✗ AUCUN MOUVEMENT"}`);
}
porte(culs.length === 0, "aucun cul-de-sac : tous les gestes de descente descendent",
  culs.length ? "en échec : " + culs.join(" ; ") : `${gestes.length}/${gestes.length}`);

// ── 3 bis. « DESCENDRE » NE RECULE JAMAIS ──────────────────────────────────
// MESURÉ avant ce correctif : à l'étage le plus profond, clic « ⤓ descendre »
// faisait `dedans — 85 %` → `monde — 63 %`. Le bouton reculait, parce que sa
// destination (une distance calculée sur le rayon du groupe) pouvait tomber plus
// loin que là où l'on était déjà. On impose donc la MONOTONIE du geste : répété,
// il ne doit jamais faire baisser l'échelle.
dit("\n── 3 bis. LE GESTE « DESCENDRE » NE RECULE JAMAIS");
await retourEspace();
const suite = [];
for (let i = 0; i < 4; i++) {
  const avant = await echelle();
  await clicTexte("descendre"); await dormir(3500);
  const apres = await echelle();
  suite.push({ avant: avant.pct, apres: apres.pct });
  dit(`  clic ${i + 1} : ${String(avant.pct).padStart(4)} % → ${String(apres.pct).padStart(4)} %  ` +
      `camDist ${avant.camDist} → ${apres.camDist}`);
}
const reculs = suite.filter((s) => s.apres < s.avant - 2);
porte(reculs.length === 0, "aucun clic ne fait RECULER l'échelle",
  reculs.length ? JSON.stringify(reculs) : "4 clics, descente monotone");

// ── 4. LES GROUPES S'AUTO-SEGMENTENT ───────────────────────────────────────
dit("\n── 4. LES GROUPES S'AUTO-SEGMENTENT (étage intérieur d'un monde)");
await retourEspace();
// On descend et on ATTEND l'étage graphe, avec une borne. Le premier appel du
// sous-graphe paie la résolution du nom de famille (voir § CONNU) : un relevé à
// cadence fixe conclurait « pas de graphe » alors qu'il est en cours.
let seg = await echelle();
for (let i = 0; i < 16 && !(seg.inter && seg.gr); i++) {
  await molette(-260);
  await dormir(1200);
  seg = await echelle();
}
for (let i = 0; i < 16 && !seg.gr; i++) { await dormir(2500); seg = await echelle(); }
dit("  Intérieur : " + (seg.inter || "—"));
dit("  Graphe    : " + (seg.gr || "—"));
const nEns = Number((String(seg.inter || "").match(/^(\d+)/) || [])[1] || 0);
porte(nEns > 1, "l'étage intérieur porte plus d'un ensemble", `${nEns} ensembles`);
const axes = await ev(`JSON.stringify([...document.querySelectorAll('button')].map(b=>b.innerText.trim()).filter(t=>["module","parent","extension","communauté","couche","∂"].includes(t)))`);
const arrAxes = (() => { try { return JSON.parse(axes || "[]"); } catch { return []; } })();
dit("  axes de groupement offerts : " + (arrAxes.join(" · ") || "aucun"));
porte(arrAxes.length >= 3, "l'axe de groupement est offert (donc re-segmentable)", arrAxes.length + " axes");
porte(/\d+\s*nœuds/.test(String(seg.gr || "")), "l'étage graphe se peuple avec des données réelles", String(seg.gr || "—").slice(0, 60));

// ── 6. LE FOND DE L'ÉCHELLE : L'ESPACE DES SIGNAUX ─────────────────────────
// Le mécanisme existait dans la scène (curseur `tS`, `posSignaux`) et ne
// s'exécutait JAMAIS : `gr.posGraphe` — l'origine de la disposition — était lue
// par la migration et n'était jamais assignée, donc la condition qui garde le
// mélange était toujours fausse. Un étage entier, décrit sur vingt lignes, mort
// à une ligne manquante. On exige ici : le cran 4 atteint, `tS` qui monte, et
// les DEUX origines présentes.
dit("\n── 6. LE QUATRIÈME CRAN : L'ESPACE DES SIGNAUX (courbure · énergie · degré)");
await retourEspace();
await clicSel(".ec-cran:nth-child(4)");
await dormir(6000);
let sig = null;
for (let i = 0; i < 14; i++) {
  sig = JSON.parse((await ev("JSON.stringify(window.__etages||null)")) || "null");
  const s = sig?.monde?.signaux;
  if (s && s.tS > 0.5 && s.origineGraphe && s.origineSignaux) break;
  await dormir(2500);
}
const s4 = sig?.monde?.signaux;
dit("  mesure interne : " + JSON.stringify(s4));
const palier4 = await ev(`(document.body.innerText.match(/Descente\\s*:\\s*([^\\n]+)/)||[])[1]||''`);
dit("  panneau : " + String(palier4).trim());
porte(!!s4 && s4.origineGraphe && s4.origineSignaux,
  "la migration a ses DEUX origines (disposition du graphe ET espace des signaux)",
  s4 ? `origineGraphe=${s4.origineGraphe} origineSignaux=${s4.origineSignaux}` : "mesure absente");
porte(!!s4 && s4.tS > 0.5, "le mélange vers les signaux est RÉELLEMENT à l'œuvre", s4 ? `tS=${s4.tS}` : "—");
porte(String(palier4).includes("signaux"), "le panneau nomme ce cran « signaux »", String(palier4).trim());

// ── 7. LE POLYMORPHISME : L'AXE DE NAVIGATION ──────────────────────────────
// « Descendre » n'est qu'un axe. Le même geste doit pouvoir traverser la
// COMMUNAUTÉ du moteur (Leiden) ou le CONTOUR ∂ (courbure), qui sont d'autres
// relations réelles — pas des variantes cosmétiques. On mesure que l'axe change
// réellement le découpage (nombre de groupes, plus gros groupe) et que l'axe est
// inscrit dans l'État d'échelle, pas seulement dans une couleur.
dit("\n── 7. LE POLYMORPHISME : L'AXE DE NAVIGATION");
const axesBrut = await ev(`JSON.stringify([...document.querySelectorAll('.echelle-bar .eb-axe button')].map(b=>b.innerText.trim()))`);
let axesOfferts = [];
try { axesOfferts = JSON.parse(axesBrut || "[]"); } catch { axesOfferts = []; }
dit("  axes offerts dans la barre : " + (axesOfferts.join(" · ") || "aucun"));
porte(axesOfferts.length >= 5, "l'axe de navigation est offert à part entière (≥ 5 axes)",
  axesOfferts.length + " axes");
const parAxe = [];
for (const nom of axesOfferts) {
  const ok = await ev(`(() => { const b=[...document.querySelectorAll('.echelle-bar .eb-axe button')]
    .find(x=>x.innerText.trim() === ${JSON.stringify(nom)}); if(!b) return false; b.click(); return true; })()`);
  if (!ok) continue;
  await dormir(2200);
  const e = JSON.parse((await ev("JSON.stringify(window.__etages||null)")) || "null");
  // Le texte de la barre est lu AU MOMENT de cet axe : c'est la seule façon de
  // vérifier qu'un axe non segmentant est ANNONCÉ (le lire une fois à la fin
  // testait le dernier axe, pas celui qu'on prétendait juger).
  const barre = await ev(`document.querySelector('.echelle-bar').innerText.replace(/\\n/g,' ')`);
  parAxe.push({ nom, annonce: /non segmenté|1 groupe/.test(String(barre)), ...(e?.monde?.axe || {}) });
}
for (const a of parAxe) {
  dit(`  axe « ${String(a.nom).padEnd(11)} » → ${String(a.groupes).padStart(3)} groupe(s), plus gros = ${String(a.plus ?? "—").slice(0, 26)} (${a.n ?? 0} nœuds)${a.segmente ? "" : "  [NON SEGMENTÉ]"}`);
}
const distincts = new Set(parAxe.map((a) => `${a.groupes}|${a.plus}`)).size;
const segmenteurs = parAxe.filter((a) => a.groupes > 1).length;
porte(distincts >= 3, "les axes produisent des découpages DIFFÉRENTS (polymorphisme réel)",
  `${distincts} découpages distincts sur ${parAxe.length} axes`);
porte(segmenteurs >= 2, "au moins deux axes segmentent réellement le graphe",
  `${segmenteurs} axes segmentent`);
// Un axe qui ne distingue RIEN doit le DIRE (doctrine du fichier : pas d'axe muet).
const muets = parAxe.filter((a) => a.groupes === 1);
const annonces = muets.filter((a) => a.annonce);
porte(muets.length === 0 || annonces.length === muets.length,
  "un axe qui ne distingue rien est ANNONCÉ au lieu de rester muet",
  muets.length ? `${annonces.length}/${muets.length} axe(s) muet(s) annoncé(s)` : "aucun axe muet");

// ── 8. LA DESCENTE HIÉRARCHIQUE : SOUS-GROUPES, CLAVIER, CHEMIN, GRAPHE ────
// « Ce sont des groupes et des sous-groupes extrêmement connectés. » Mesuré sur
// ce store : 3 racines, 200 enfants sous `deepseek_harness`, 60 petits-enfants
// sous `…packages`. Ce contrôle exige donc que la descente EXISTE, qu'elle
// fonctionne au CLAVIER comme au clic, que le chemin permette de revenir, et
// que le GRAPHE suive le niveau (sinon la navigation ne suit qu'à moitié).
dit("\n── 8. LA DESCENTE HIÉRARCHIQUE (sous-groupes · clavier · chemin · graphe)");
const prefixe = async () => {
  const e = JSON.parse((await ev("JSON.stringify(window.__etages||null)")) || "null") || {};
  return e?.organisation?.prefixe ?? null;
};
const interieurLigne = async () => String((await ev("document.body.innerText")).match(/Intérieur[^\n]*/)?.[0] || "");
const grapheLigne = async () => String((await ev("document.body.innerText")).match(/Graphe\s*:\s*([^\n]+)/)?.[1] || "");
const VK_PAGE = { PageDown: 34, PageUp: 33 };
const page = async (k) => {
  await send("Input.dispatchKeyEvent", { type: "keyDown", key: k, code: k, windowsVirtualKeyCode: VK_PAGE[k] });
  await send("Input.dispatchKeyEvent", { type: "keyUp", key: k, code: k, windowsVirtualKeyCode: VK_PAGE[k] });
  await dormir(8000);
};
await send("Page.navigate", { url: BASE + "/?cosmos=1&hier=" + Date.now() });
await dormir(ATTENTE_MS);
await clicSel(".ec-cran:nth-child(2)");
/**
 * ── ON ATTEND QUE L'ÉTAGE SOIT PUBLIÉ, ON NE L'ESPÈRE PAS ──────────────────
 * MESURÉ : avec 8 s fixes, ce bloc a rendu `prefixe=null` puis `""` à chaque
 * pas — cinq exigences déclarées non tenues — alors que la descente fonctionnait
 * (vérifié à part, même bundle). `null` signifie « rien de publié encore » : ce
 * n'est pas « ne descend pas ». On attend donc la PREMIÈRE publication, bornée.
 */
for (let i = 0; i < 16; i++) {
  const p = await prefixe();
  if (p !== null && p !== undefined) break;
  await dormir(3000);
}
const niv = [];
const noter = async (tag) => {
  const p = await prefixe();
  const i = await interieurLigne();
  const g = await grapheLigne();
  niv.push({ tag, p, i, g });
  dit(`  ${tag.padEnd(20)} prefixe=${JSON.stringify(p)}`);
  dit(`  ${"".padEnd(20)} ${i.slice(0, 78)}`);
  dit(`  ${"".padEnd(20)} graphe : ${g.slice(0, 62)}`);
  return { p, i, g };
};
const racine = await noter("racines");
await page("PageDown");
const n1 = await noter("Page ↓");
await page("PageDown");
const n2 = await noter("Page ↓");
await page("PageUp");
const n3 = await noter("Page ↑");
porte(n1.p && n1.p !== racine.p, "Page ↓ ENTRE dans le plus gros sous-groupe",
  `${JSON.stringify(racine.p)} → ${JSON.stringify(n1.p)}`);
porte(n2.p && n2.p.split(".").length > (n1.p || "").split(".").length,
  "on peut entrer PLUSIEURS niveaux (sous-groupes de sous-groupes)",
  `${JSON.stringify(n1.p)} → ${JSON.stringify(n2.p)}`);
porte(!!n2.i && /\d+\s*ensembles/.test(n2.i) && n2.i !== racine.i,
  "chaque niveau montre SES groupes (comptés par le serveur)", n2.i.slice(0, 60));
porte(n3.p === n1.p, "Page ↑ REMONTE d'un niveau", `${JSON.stringify(n2.p)} → ${JSON.stringify(n3.p)}`);
// La ligne peut commencer par « Intérieur » SUIVI d'une précision (axe, niveau) :
// on cherche le nombre d'ensembles LÀ OÙ IL EST, pas en début de ligne. Un
// contrôle qui exige un format que le produit est libre d'enrichir mesure sa
// propre rigidité.
const nbNiv = (s) => Number((String(s).match(/(\d+)\s*ensembles/) || [])[1] || 0);
porte(nbNiv(n2.i) > 1, "le niveau descendu porte plus d'un groupe", n2.i.slice(0, 40));
// Le GRAPHE doit suivre le niveau : même geste, autre graphe, puis retour.
const gRacine = racine.g, gProf = n2.g;
porte(!!gProf && gProf !== gRacine, "le GRAPHE change avec le niveau",
  `« ${gRacine.slice(0, 24)} » → « ${gProf.slice(0, 24)} »`);
// LE CHEMIN APPARAÎT QUAND L'ÉTAGE EST RE-SEGMENTÉ : un relevé sec tombe
// pendant la reconstruction (l'étage n'a alors qu'un groupe, et le bloc est
// masqué). On l'attend, borné — même leçon que la cadence de fluidité :
// un instrument qui conclut sur un instant mesure sa propre impatience.
let cheminTxt = "ABSENT";
for (let i = 0; i < 12 && cheminTxt === "ABSENT"; i++) {
  cheminTxt = String(await ev(`(()=>{const s=[...document.querySelectorAll('.qb-lbl')].find(x=>/chemin/.test(x.textContent||""));return s? s.innerText.replace(/\s+/g," ").slice(0,120):"ABSENT";})()`));
  if (cheminTxt === "ABSENT") await dormir(3000);
}
dit("  chemin affiché : " + cheminTxt);
porte(cheminTxt !== "ABSENT", "le chemin est AFFICHÉ, maillon par maillon", cheminTxt.slice(0, 70));
// Le geste de retour à la racine, par le maillon.
const retourRacines = await ev(`(()=>{const b=[...document.querySelectorAll('button.chip')].find(x=>/racines/.test(x.innerText.trim()));
 if(!b) return "absent"; b.click(); return "ok";})()`);
await dormir(8000);
const apres = await noter("⌂ racines");
porte(apres.p === "" && retourRacines === "ok", "un maillon RAMÈNE directement au niveau voulu",
  `clic « ⌂ racines » → prefixe=${JSON.stringify(apres.p)}`);

// ── 9. DENSIFIER OU APPROFONDIR : DEUX GESTES, PAS UN ─────────────────────
// DÉFAUT MESURÉ avant ce correctif : un niveau portait 200 ensembles servis par
// `/api/viz/space`, n'en DESSINAIT que 24, et lisait `total` DANS la liste déjà
// coupée — donc le panneau annonçait « 24 sur 24 » et les 176 autres n'étaient
// atteignables par aucun geste. Le seul geste de « plus » était `⤵`, qui DESCEND
// d'un cran : il ne répond pas à « montre-moi ce qui manque ICI ».
//
// Ce qu'on exige ici est la DISTINCTION, mesurée sur l'état publié :
//   · densifier augmente ce qui est DESSINÉ, et NE TOUCHE PAS le préfixe ni le
//     rang — et, si la page du serveur n'est pas pleine, n'émet AUCUN appel ;
//   · approfondir change le préfixe et le rang (l'échelle descend).
// Deux gestes qui produiraient le même effet mesurable seraient un seul geste.
dit("\n── 9. DENSIFIER (même niveau, page suivante) OU APPROFONDIR (un cran)");
await send("Page.navigate", { url: BASE + "/?cosmos=1&dens=" + Date.now() });
await dormir(ATTENTE_MS);
await clicSel(".ec-cran:nth-child(2)");
await dormir(8000);
const org = async () => {
  const e = JSON.parse((await ev("JSON.stringify(window.__etages||null)")) || "null") || {};
  return e.organisation || {};
};
// On descend jusqu'à un niveau qui ANNONCE plus qu'il ne dessine : c'est la
// définition d'un niveau densifiable, et c'est ce que l'ancien code rendait
// impossible (le total était coupé avant d'être lu).
let avant = await org();
for (let i = 0; i < 4 && !(Number(avant.tous) > Number(avant.montres)); i++) {
  await page("PageDown");
  avant = await org();
}
dit(`  niveau courant : préfixe=${JSON.stringify(avant.prefixe)} rang=${avant.rang} ` +
    `dessinés=${avant.montres} reçus=${avant.tous} geste=${avant.geste}`);
porte(Number(avant.tous) > Number(avant.montres),
  "le niveau ANNONCE plus qu'il ne dessine (le plafond silencieux a disparu)",
  `${avant.montres} dessinés sur ${avant.tous} reçus`);

// 1) DENSIFIER — la touche `d` (et le geste du panneau), sans changer d'échelle.
const espaceAvant = appelsSpace.length;
await touche("d", "KeyD");
await dormir(6000);
let apresDens = await org();
// Le redessin peut se re-segmenter : on attend la mesure, bornée — un relevé sec
// conclurait sur un instant de reconstruction (leçon déjà apprise au §8).
for (let i = 0; i < 8 && !(Number(apresDens.montres) > Number(avant.montres)); i++) {
  await dormir(2500);
  apresDens = await org();
}
const appelsPendant = appelsSpace.length - espaceAvant;
dit(`  après « d densifier » : préfixe=${JSON.stringify(apresDens.prefixe)} rang=${apresDens.rang} ` +
    `dessinés=${apresDens.montres} reçus=${apresDens.tous} · appels /api/viz/space émis : ${appelsPendant}`);
porte(Number(apresDens.montres) > Number(avant.montres),
  "densifier DESSINE plus (le geste fait ce qu'il annonce)",
  `${avant.montres} → ${apresDens.montres}`);
porte(apresDens.prefixe === avant.prefixe && Number(apresDens.rang) === Number(avant.rang),
  "densifier NE CHANGE PAS de niveau (préfixe et rang identiques)",
  `préfixe ${JSON.stringify(avant.prefixe)} → ${JSON.stringify(apresDens.prefixe)}, rang ${avant.rang} → ${apresDens.rang}`);
// La matière était DÉJÀ reçue : densifier ne doit rien demander au serveur tant
// que sa page n'est pas pleine. Si un appel part ici, la phrase du panneau
// (« aucun appel : la matière est déjà là ») est fausse.
const plein = Number(avant.tous) >= 200;
porte(plein || appelsPendant === 0,
  "densifier un niveau déjà reçu n'émet AUCUN appel",
  plein ? `page serveur pleine (${avant.tous}) : l'appel est légitime` : `${appelsPendant} appel(s)`);

// 2) APPROFONDIR — le geste de descente, mesuré sur le MÊME niveau.
const avantDesc = await org();
await page("PageDown");
const apresDesc = await org();
dit(`  après « Page ↓ approfondir » : préfixe=${JSON.stringify(apresDesc.prefixe)} rang=${apresDesc.rang}`);
const bougeLeNiveau = apresDesc.prefixe !== avantDesc.prefixe
  || Number(apresDesc.rang) > Number(avantDesc.rang);
porte(bougeLeNiveau, "approfondir CHANGE de niveau (préfixe allongé ou rang +1)",
  `${JSON.stringify(avantDesc.prefixe)} (rang ${avantDesc.rang}) → ${JSON.stringify(apresDesc.prefixe)} (rang ${apresDesc.rang})`);
// Le geste de densification doit être OFFERT là où il y a de la matière cachée,
// et DISPARAÎTRE quand tout est montré : un geste qui ne rapporte rien est un
// geste décoratif.
const offre = await ev(`(()=>{const b=[...document.querySelectorAll('.echelle-bar button, .ov-panel button')]
  .find(x=>/densifier/.test(x.innerText||"")); return b ? b.innerText.trim() : "ABSENT";})()`);
dit("  geste offert à l'écran : " + offre);
porte(offre === "ABSENT" || /densifier/.test(offre),
  "le geste de densification est ANNONCÉ (ou absent quand le niveau est entier)", String(offre));

// ── 5. FLUIDITÉ ────────────────────────────────────────────────────────────
// CADENCE : elle fait partie de l'instrument. La scène publie son curseur toutes
// les 200 ms ; un relevé toutes les 500 ms pendant qu'un cran de molette passe
// toutes les 500 ms peut donc CHEVAUCHER deux crans et n'en montrer qu'un —
// mesuré : une trace « 34 → 34 → 63 » qui fait croire à un saut de 29 points
// alors que la caméra, elle, n'a jamais sauté. C'est l'instrument qui mentait,
// pas la scène : relevé toutes les 120 ms et un cran toutes les 900 ms, la même
// descente donne 5 → 20 → 34 → 48 → 63 → 77 → 91 → 100, sans un seul saut.
dit("\n── 5. FLUIDITÉ — la descente prend des valeurs intermédiaires, sans saut");
await send("Page.navigate", { url: BASE + "/?cosmos=1&fluide=" + Date.now() });
await dormir(ATTENTE_MS);
const trace = [];
let dernierPct = (await echelle()).pct;
trace.push(dernierPct);
for (let n = 0; n < 10; n++) {
  await send("Input.dispatchMouseEvent", { type: "mouseWheel", x: 720, y: 420, deltaX: 0, deltaY: -200 });
  for (let k = 0; k < 7; k++) {
    await dormir(120);
    const v = (await echelle()).pct;
    if (v !== dernierPct) { trace.push(v); dernierPct = v; }
  }
}
const sauts = [];
for (let i = 1; i < trace.length; i++) if (Math.abs(trace[i] - trace[i - 1]) >= 20) sauts.push({ i, d: trace[i] - trace[i - 1] });
const paliers = new Set(trace).size;
dit("  trace : " + trace.join(" → "));
dit(`  paliers distincts : ${paliers} · sauts ≥ 20 pts : ${sauts.length}` + (sauts.length ? " " + JSON.stringify(sauts.slice(0, 8)) : ""));
porte(paliers >= 5, "la descente est un CONTINU, pas une commutation", paliers + " paliers");
porte(sauts.length === 0, "aucun saut brutal (≥ 20 points) entre deux crans");

// ── Ce qui est connu, et n'est pas une régression de ce correctif ───────────
dit("\n── CONNU (hors périmètre)");
dit("  · 404 sur /ui/state/* : le pont d'état n'existe pas sur `viz serve`, et le");
dit("    dashboard continue avec son état local (comportement voulu, journalisé).");
dit("  · Le 400 sur /api/viz/subgraph a DISPARU : l'entrée par un nom de FAMILLE est");
dit("    résolue AVANT l'appel (`/api/functions`), au lieu d'être tentée puis ratée.");
dit(`\n  erreurs console inattendues : ${erreurs.length}` + (erreurs.length ? " — " + erreurs.slice(0, 5).join(" | ") : ""));
dit(`  réponses HTTP inattendues : ${httpKO.length}` + (httpKO.length ? " — " + httpKO.slice(0, 5).join(" | ") : ""));

dit(KO === 0
  ? "\n✓ TOUT EST TENU — quatre crans d'échelle, descente continue et sans recul,"
    + "\n  groupes auto-segmentés, étage des signaux RÉEL, et six axes de navigation polymorphes"
  : `\n✗ ${KO} exigence(s) non tenue(s)`);
writeFileSync("/tmp/echelle-multi-niveaux-rapport.txt", lignes.join("\n"));
await j("/json/close/" + tab.id);
ws.close();
process.exit(KO === 0 ? 0 : 1);
