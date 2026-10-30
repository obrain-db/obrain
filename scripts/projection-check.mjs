#!/usr/bin/env node
// VÉRIFICATION DE LA PROJECTION MULTI-AXES — en vraie interface, contre le
// service vivant. LECTURE SEULE : ce script n'émet que des GET de navigation et
// des clics d'interface (aucun POST, aucune écriture moteur).
//
// Ce qu'il prouve, dans l'ordre de la chaîne :
//   1. l'état vide du panneau DIT quelque chose (et se rouvre après la croix) ;
//   2. après une requête GQL, les axes existent et `.proj-row` compte les lignes ;
//   3. le décompte des groupes masqués est PRÉSENT et COHÉRENT (comparé au
//      nombre de groupes distincts relu du moteur, pas seulement au chiffre
//      affiché par le panneau) ;
//   4. un clic sur une ligne ancrable produit ≥ 1 requête `/api/viz/subgraph`
//      dans la face par défaut — c'est LE maillon qui était cassé : la face
//      par défaut (morph → `StageContinu`) ne lisait ni `centersAtom` ni
//      `requestTokenAtom`, donc l'ancrage n'émettait RIEN ;
//   5. aucun avertissement/erreur console React au chargement.
//
// Et deux contrôles de plus, parce que les affirmations correspondantes doivent
// être mesurées et pas crues :
//   6. CACHE — relancer la MÊME requête n'émet aucune nouvelle requête moteur ;
//   7. RAFALE — 6 clics rapides sur des lignes ancrables n'émettent pas 6
//      requêtes de sous-graphe (coalescence du report).
//
// Usage : node scripts/projection-check.mjs [BASE] [--store=<label>] [--keep]
//   BASE par défaut : http://192.168.1.103:8432
//   --keep : laisse le navigateur et les captures pour l'inspection
//
// Code de sortie : 0 si tous les contrôles passent, 1 sinon (lignes PASS/FAIL).

import { existsSync, writeFileSync } from "node:fs";
import { spawn } from "node:child_process";

const args = process.argv.slice(2);
const BASE = (args.find((a) => !a.startsWith("--")) || "http://192.168.1.103:8432").replace(/\/$/, "");
const STORE = (args.find((a) => a.startsWith("--store=")) || "--store=Linux e1000").slice(8);
const KEEP = args.includes("--keep");
const PORT = Number(process.env.CDP_PORT || 9253);
const CHROME = process.env.CHROME
  || "/home/triviere/.cache/ms-playwright/chromium-1234/chrome-linux64/chrome";

// 157 groupes distincts mesurés sur ce store (GET /api/query, 7 ms de moteur) :
// assez pour que le plafond de construction (64) morde, assez peu pour que le
// garde-fou de volume (20 000 lignes) ne s'arme pas.
const REQUETE = "MATCH (a)-[:CALLS]->(b) RETURN a.qualified_name AS qn, count(b) AS nbr ORDER BY nbr DESC LIMIT 400";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const faits = [];
const noter = (ok, quoi, detail = "") => {
  faits.push({ ok, quoi, detail });
  console.log(`${ok ? "  ok  " : " FAIL "} ${quoi}${detail ? `\n         ${detail}` : ""}`);
};
const info = (quoi) => console.log(`        ${quoi}`);

const dir = `/tmp/obrain-projection-${Date.now()}`;
/** Sortie PROPRE : le navigateur est tué AVANT `process.exit` (sinon le
 *  processus laissé en vie retient le shell appelant — constaté). */
const terminer = (code) => { try { chrome.kill("SIGKILL"); } catch { /* déjà mort */ } process.exit(code); };

if (!existsSync(CHROME)) {
  console.log(`SKIP — chromium headless absent (${CHROME})`);
  terminer(0);
}

const chrome = spawn(CHROME, [
  "--headless=new", "--no-sandbox", "--disable-dev-shm-usage",
  "--use-angle=swiftshader", "--enable-unsafe-swiftshader",
  `--remote-debugging-port=${PORT}`, `--user-data-dir=${dir}`, "--window-size=1440,900",
  "about:blank",
], { stdio: "ignore" });

let ws;
const erreurs = [];     // console.error / warnings React / exceptions
const journaux = [];    // le reste (informatif)
const ressources = [];  // ressources réseau en échec (nommées, non bloquantes)
try {
  await sleep(3500);
  const cibles = await (await fetch(`http://127.0.0.1:${PORT}/json`)).json();
  const page = cibles.find((x) => x.type === "page");
  if (!page) throw new Error("pas de page CDP");
  ws = new WebSocket(page.webSocketDebuggerUrl);
  let idc = 0; const pend = new Map();
  ws.onmessage = (ev) => {
    const m = JSON.parse(ev.data);
    if (m.id && pend.has(m.id)) { const p = pend.get(m.id); pend.delete(m.id);
      m.error ? p.rej(new Error(m.error.message)) : p.res(m.result); return; }
    if (m.method === "Runtime.exceptionThrown") {
      erreurs.push("exception : " + (m.params.exceptionDetails?.exception?.description
        || m.params.exceptionDetails?.text || "?"));
    }
    if (m.method === "Runtime.consoleAPICalled") {
      const t = (m.params.args || []).map((a) => a.value ?? a.description ?? "").join(" ");
      if (m.params.type === "error" || (/Warning:/i.test(t) || /React/i.test(t))) erreurs.push(`${m.params.type} : ${t.slice(0, 200)}`);
      else journaux.push(`${m.params.type} : ${t.slice(0, 120)}`);
    }
    if (m.method === "Log.entryAdded" && m.params.entry.level === "error") {
      // Une ressource qui ne charge pas n'est PAS un avertissement React : elle
      // est comptée à part et NOMMÉE (sans son URL, un 404 reste invisible).
      const url = m.params.entry.url || "";
      if (/^https?:/.test(url)) ressources.push(`${m.params.entry.text.slice(0, 60)} — ${url.slice(0, 120)}`);
      else erreurs.push("log : " + m.params.entry.text.slice(0, 200));
    }
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
  /** Attend qu'une expression rende vrai (le service est vivant, il peut être lent). */
  const attendre = async (expr, ms = 15000) => {
    const t0 = Date.now();
    while (Date.now() - t0 < ms) {
      if (await ev(expr)) return true;
      await sleep(250);
    }
    return false;
  };
  const clic = (sel) => ev(`(() => { const e = document.querySelector(${JSON.stringify(sel)});
    if (!e) return false; e.click(); return true; })()`);
  const clicTexte = (txt) => ev(`(() => { const b = [...document.querySelectorAll('button')]
    .find(x => (x.textContent||'').includes(${JSON.stringify(txt)}));
    if (!b) return false; b.click(); return true; })()`);
  const net = () => ev(`window.__net || []`);
  const netCount = async (motif) => (await net()).filter((r) => r.url.includes(motif)).length;

  await send("Page.enable"); await send("Runtime.enable"); await send("Log.enable");
  // ── TRAÇAGE RÉSEAU, POSÉ AVANT LE CHARGEMENT (sinon les appels de démarrage
  //    échappent au comptage). Purement observationnel : `fetch` est enveloppé,
  //    jamais remplacé.
  //
  // ⚠ `/__dev/version` est NEUTRALISÉ : le service 8432 est en `OBRAIN_DEV=1` et
  // les autres sessions de travail modifient `src/` en continu — chaque rebuild
  // déclenche le `location.reload()` du client (voir l'index.html servi). Sans
  // cela, la page se rechargeait AU MILIEU d'une mesure et l'état d'interface
  // (panneau ouvert, requête saisie) repartait de zéro : constaté trois fois.
  // On ne modifie QUE cette sonde de développement ; l'application est intacte.
  //
  // ⚠ ANGLE MORT CORRIGÉ LE 2026-09-15 : cette sonde n'enveloppait que
  // `window.fetch`. Depuis la réunification, le sous-graphe part par
  // **WebSocket** (`/api/viz/subgraph/stream`, repli HTTP) : les contrôles 4 et
  // 4bis annonçaient donc « 0 requête(s) » alors que la requête était bien
  // émise — un angle mort de l'instrument, pas une absence de requête. Le
  // constructeur `WebSocket` est désormais enveloppé lui aussi (jamais
  // remplacé : les constantes `OPEN`/`CLOSED` sont recopiées).
  await send("Page.addScriptToEvaluateOnNewDocument", { source: `
    (() => {
      window.__net = []; const of = window.fetch;
      window.fetch = function (input, init) {
        let url = ""; try { url = typeof input === "string" ? input : (input && input.url) || String(input); } catch (e) {}
        window.__net.push({ url, method: (init && init.method) || "GET", t: Date.now() });
        if (url.includes("/__dev/version")) {
          return Promise.resolve(new Response('{"version":0}', { headers: { "content-type": "application/json" } }));
        }
        return of.apply(this, arguments);
      };
      const OW = window.WebSocket;
      if (OW) {
        function WS(url, protos) {
          try { window.__net.push({ url: String(url), method: "WS", t: Date.now() }); } catch (e) {}
          return protos === undefined ? new OW(url) : new OW(url, protos);
        }
        WS.prototype = OW.prototype;
        for (const k of ["CONNECTING", "OPEN", "CLOSING", "CLOSED"]) { try { WS[k] = OW[k]; } catch (e) {} }
        window.WebSocket = WS;
      }
      window.__t0 = performance.timeOrigin;
    })();` });

  console.log(`── projection multi-axes — ${BASE} · store « ${STORE} »`);
  await send("Page.navigate", { url: `${BASE}/?store=${encodeURIComponent(STORE)}` });
  await sleep(9000);

  // ── STABILITÉ : le service 8432 a `OBRAIN_DEV=1` et reconstruit le dashboard
  //    dès qu'un fichier de `src/` change (d'autres sessions travaillent dans le
  //    même dépôt) ; un rechargement au milieu du test remettrait l'état d'interface
  //    à zéro. On attend donc une fenêtre sans rechargement avant de mesurer.
  const waitStable = async () => {
    let o = await ev("window.__t0");
    for (let i = 0; i < 24; i++) {
      await sleep(1500);
      const t = await ev("window.__t0");
      if (t === o) return o;
      o = t;
      await sleep(3000);
    }
    return o;
  };
  let origine = await waitStable();

  // ── GARDE DE DÉMARRAGE : L'APPLICATION EST-ELLE LÀ ? ─────────────────────
  // Les autres sessions de travail modifient `src/` : le service reconstruit
  // alors le dashboard, et un chargement qui tombe pile dans la fenêtre de
  // réécriture de `dist/` peut servir un bundle incomplet. Sans cette garde,
  // TOUT le reste du test échoue pour une raison étrangère au code testé
  // (constaté une fois : 11 contrôles en échec, aucune erreur React).
  const appPrete = async () => await ev("!!document.querySelector('.lab-btn') || !!document.querySelector('.cmdbar')");
  for (let i = 0; i < 6 && !(await appPrete()); i++) { await send("Page.reload"); await sleep(6000); }
  const chargee = await appPrete();

  /** L'état d'interface survit-il ? Sinon on le rétablit (le test ne doit pas
   *  échouer parce qu'une autre session a rechargé la page). */
  const assurerPanneau = async () => {
    if (!(await ev("!!document.querySelector('.q-editor')"))) {
      await clic(".lab-btn"); await attendre("!!document.querySelector('.q-editor')", 8000);
    }
    if (!(await ev("!!document.querySelector('.proj')"))) {
      await clic(".proj-open"); await attendre("!!document.querySelector('.proj')", 8000);
    }
  };
  const poserRequete = () => ev(`(() => {
    const t = document.querySelector('.q-editor');
    if (!t) return false;
    const set = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value').set;
    set.call(t, ${JSON.stringify(REQUETE)});
    t.dispatchEvent(new Event('input', { bubbles: true }));
    return true; })()`);
  const charge = {
    overview: await ev("!!document.querySelector('.overview')"),
    canvas: await ev("!!document.querySelector('canvas')"),
    face: await ev(`(() => { const b = [...document.querySelectorAll('.ec-cran')].find(x => x.classList.contains('on'));
      return b ? b.textContent.trim() : ''; })()`),
    store: await ev(`(() => { const s = document.querySelector('.cmd-store select'); return s ? s.value : ''; })()`),
  };
  info(`face au chargement : « ${charge.face} » · substrat (StageContinu) : ${charge.overview ? "monté" : "ABSENT"} · canvas : ${charge.canvas ? "oui" : "non"}`);
  info(`store actif : ${charge.store}`);

  for (const [quoi, ok] of [["application chargée (barre de commandes / bouton Réglages)", chargee]]) {
    noter(ok, `0. ${quoi}`, ok ? "garde de démarrage franchie" : "l'application ne s'est pas montée");
  }

  // ── 5. CONSOLE AU CHARGEMENT (avant toute interaction) ────────────────────
  await sleep(500);
  noter(erreurs.length === 0, "5. aucun avertissement/erreur console React au chargement",
    erreurs.length ? erreurs.slice(0, 4).join(" | ") : `${journaux.length} message(s) informatif(s), 0 avertissement React`);
  if (ressources.length) info(`ressources en échec (hors React, informatif) : ${ressources.slice(0, 3).join(" | ")}`);

  // Le maillon cassé visait la face PAR DÉFAUT (« morph » → StageContinu).
  noter(charge.overview === true, "face par défaut = celle qui monte StageContinu (le maillon à réparer)",
    charge.overview ? "« .overview » présent : l'ancrage doit donc passer par StageContinu" : "PAS de .overview : le test 4 ne porterait pas sur le bon composant");

  // ── 1. ÉTAT VIDE ─────────────────────────────────────────────────────────
  const ouvert = await clic(".lab-btn");
  const pret = await attendre("!!document.querySelector('.proj')", 8000);
  const videAvant = (await ev("document.querySelector('.proj-empty')?.textContent || ''")) || "";
  noter(ouvert === true && pret && videAvant.trim().length > 40,
    "1a. l'état vide du panneau DIT quelque chose (avant toute requête)",
    `« ${videAvant.trim().slice(0, 110)}… »`);

  // …et il se ROUVRE après la croix : `projectionVisibleAtom` avait un écrivain
  // et aucun lecteur d'interface, donc la seule sortie était de relancer une requête.
  await clic(".proj .ctl-x");
  await sleep(400);
  const ferme = await ev("!!document.querySelector('.proj')");
  await clic(".proj-open");
  await sleep(500);
  const rouvert = await ev("!!document.querySelector('.proj')");
  noter(ferme === false && rouvert === true,
    "1b. la projection fermée se ROUVRE par un contrôle d'interface (⌗ projection → .proj de retour)",
    `après la croix : ${ferme ? "encore là (?)" : "fermée"} · après ⌗ : ${rouvert ? "rouverte" : "TOUJOURS FERMÉE"}`);

  // 1c. EN CHROME RÉDUIT, la projection reste atteignable.
  // Défaut mesuré : l'`Inspector` (et donc la projection) était monté sous
  // `montre("reglages")`, donc masqué par le geste « ⤡ réduire » — et le contrôle
  // ⌗ n'existait pas : la projection devenait inatteignable.
  await clicTexte("⤡ réduire");
  await sleep(500);
  const inspectorReduit = await ev("!!document.querySelector('.inspector')");
  await clic(".proj .ctl-x");
  await sleep(400);
  const fermeReduit = await ev("!!document.querySelector('.proj')");
  await clic(".proj-open");
  await sleep(700);
  const rouvertReduit = await ev("!!document.querySelector('.proj')");
  noter(fermeReduit === false && rouvertReduit === true,
    "1c. en chrome RÉDUIT, ⌗ projection rouvre le panneau (l'inspecteur est monté dès qu'on le demande)",
    `inspecteur après « réduire » : ${inspectorReduit ? "encore monté" : "démonté"} · après ⌗ : projection ${rouvertReduit ? "ROUVERTE" : "absente"}`);
  await clicTexte("⤢ afficher");
  await sleep(600);
  await assurerPanneau();

  // ── 2. REQUÊTE GQL → AXES + LIGNES ───────────────────────────────────────
  await assurerPanneau();
  const saisi = await poserRequete();
  await sleep(300);
  const q1 = await netCount("/api/query");
  await clicTexte("▶ Projeter");
  const pose = await attendre("document.querySelectorAll('.proj-ax').length > 0", 20000);
  await sleep(600);
  const axes = (await ev("[...document.querySelectorAll('.proj-ax')].map(b => b.textContent.trim())")) || [];
  const lignes = await ev("document.querySelectorAll('.proj-row').length");
  const meta = (await ev("document.querySelector('.proj-meta')?.textContent || ''")) || "";
  noter(saisi === true && pose === true && axes.length > 0 && lignes > 0,
    "2. après une requête GQL : axes présents et `.proj-row` non vide",
    `saisie=${saisi} · axes = [${axes.join(", ")}] · .proj-row = ${lignes} · méta « ${meta.trim()} » · requêtes /api/query émises : ${(await netCount("/api/query")) - q1}`);

  // ── 6. CACHE DES REQUÊTES, mesuré DANS la fenêtre du TTL (20 s) ───────────
  // (Mesuré après coup, un contrôle tardif ne testerait plus le cache mais
  // l'expiration : la première version de ce script s'y est trompée.)
  const avantCache = await netCount("/api/query");
  await clicTexte("▶ Projeter");
  await sleep(1200);
  const apresCache = await netCount("/api/query");
  noter(apresCache === avantCache,
    "6. relancer la MÊME requête ne repaie pas le moteur (cache par store|lang|requête)",
    `/api/query : ${avantCache} → ${apresCache} (delta ${apresCache - avantCache}) sur 2 clics identiques à ~1 s d'intervalle`);

  // ── 3. DÉCOMPTE DES GROUPES MASQUÉS, COHÉRENT ────────────────────────────
  const tell = ((await ev("document.querySelector('.proj-tell')?.textContent || ''")) || "").replace(/\s+/g, " ").trim();
  const m = /(\d+) groupes · (\d+) plus gros affichés(?: · (\d+) masqués)?/.exec(tell);
  // Vérification CROISÉE : le nombre de groupes est relu du MOTEUR (GET, lecture
  // seule), pas du panneau — sinon on ne comparerait que le panneau à lui-même.
  let groupesMoteur = null;
  try {
    // Le chemin du store vient de l'INTERFACE (le sélecteur), pas d'une constante :
    // c'est bien la base que la page interroge.
    const url = `${BASE}/api/query?store=${encodeURIComponent(charge.store || "")}`
      + `&language=gql&query=${encodeURIComponent(REQUETE)}`;
    const j = await (await fetch(url)).json();
    groupesMoteur = new Set((j.rows || []).map((r) => String(r[0]))).size;
  } catch { /* le croisement est informatif s'il échoue */ }
  const N = m ? Number(m[1]) : null, K = m ? Number(m[2]) : null, M = m ? Number(m[3] || 0) : null;
  const coherent = !!m && N !== null && K !== null
    && N - K === M                       // le reste est exactement le non-montré
    && K === lignes                      // ce qui est dit affiché EST affiché
    && K <= 64                           // plafond de construction
    && (groupesMoteur === null || N === groupesMoteur);
  noter(coherent, "3. le décompte des groupes masqués est présent ET cohérent",
    `panneau « ${tell} » → N=${N} K=${K} M=${M} · lignes DOM=${lignes} · groupes distincts relus du moteur=${groupesMoteur}`);

  // ── A. L'AXE MESURE : des intervalles DISJOINTS ──────────────────────────
  // Défaut mesuré de l'ancienne version : bornes arrondies au plus proche, donc
  // « 2–3 » et « 3–4 » — le 3 semblait dans deux buckets.
  await ev("document.querySelectorAll('.proj-ax')[1]?.click()");
  await sleep(500);
  const buckets = (await ev("[...document.querySelectorAll('.proj-row .proj-row-lbl')].map(e => e.textContent.trim())")) || [];
  const stats = ((await ev("document.querySelector('.proj-stats')?.textContent || ''")) || "").replace(/\s+/g, " ").trim();
  const disjoints = buckets.length > 1 && buckets.every((l) => /^\[[^\]]+\[?$|^\[[^\]]+\]$/.test(l));
  noter(disjoints && /moyenne/.test(stats), "A. l'axe mesure rend des intervalles bornés ([a, b[ / [a, b]) et ses statistiques",
    `${buckets.length} intervalle(s) : ${buckets.slice(0, 4).join(" ")} · « ${stats.slice(0, 90)} »`);
  await ev("document.querySelectorAll('.proj-ax')[0]?.click()");
  await sleep(400);

  // ── BONUS B : « tout voir » ne doit pas geler ────────────────────────────
  // SÉLECTEUR PRÉCIS : « tout voir » apparaît AUSSI dans StageContinu (« ⌂ tout voir »,
  // qui ne fait que reculer la caméra) et ce bouton-là est plus haut dans le DOM —
  // un clic par texte cliquait la caméra au lieu du panneau (constaté).
  const homonymes = await ev(`[...document.querySelectorAll('button')].filter(b => (b.textContent||'').includes('tout voir')).length`);
  const t0tout = Date.now();
  const clicTout = await clic("button.proj-plus");
  const listePrete = await attendre("!!document.querySelector('.proj-list')", 15000);
  const msTout = Date.now() - t0tout;
  const domRows = await ev("document.querySelectorAll('.proj-list .proj-row').length");
  const scrollH = await ev("(() => { const l = document.querySelector('.proj-list'); return l ? l.scrollHeight : 0; })()");
  noter(listePrete === true && domRows > 0 && domRows <= 60 && scrollH > 0,
    "B. « tout voir » matérialise la traîne en liste FENÊTRÉE (pas N nœuds DOM)",
    `clic=${clicTout} (${homonymes} bouton(s) « tout voir » dans la page) · groupes=${N} · lignes DOM posées=${domRows} · hauteur défilante=${scrollH}px · ${msTout} ms après le clic`);

  // le filtre local doit atteindre un libellé de la TRAÎNE (rang > 64)
  const qnTraine = await ev(`(() => {
    const t = document.querySelector('.proj-filtre input'); if (!t) return '';
    return t.getAttribute('placeholder') || ''; })()`);
  info(`filtre présent (placeholder : « ${qnTraine} »)`);

  // ── 4. LE MAILLON : CLIC SUR UNE LIGNE ANCRABLE → /api/viz/subgraph ──────
  await clic("button.proj-plus-info .textbtn");
  await sleep(400);
  const label = await ev("document.querySelector('button.proj-row')?.getAttribute('title') || ''");
  const nbLignesAvant = await ev("document.querySelectorAll('button.proj-row').length");
  const avant = await netCount("/api/viz/subgraph");
  const clique = await clic(".proj-row");
  const venu = await attendre(`(window.__net||[]).filter(r => r.url.includes('/api/viz/subgraph')).length > ${avant}`, 15000);
  await sleep(500);
  const apres = await netCount("/api/viz/subgraph");
  const urls = (await net()).filter((r) => r.url.includes("/api/viz/subgraph")).slice(-3).map((r) => r.url);
  const centre = (urls[urls.length - 1] || "").match(/[?&]center=([^&]*)/)?.[1] || "";
  const detail = ((await ev("document.querySelector('.proj-detail')?.textContent || ''")) || "").replace(/\s+/g, " ").trim();
  noter(clique === true && venu && apres > avant && decodeURIComponent(centre) === label,
    "4. un clic sur une ligne ancrable ÉMET une requête /api/viz/subgraph (face par défaut)",
    `lignes cliquables=${nbLignesAvant} · ligne « ${label} » → ${apres - avant} requête(s) · center=${decodeURIComponent(centre)} · détail « ${detail.slice(0, 90)} »`);
  if (urls.length) info(`dernière URL : ${urls[urls.length - 1].slice(0, 160)}`);

  // ── 4bis. RAFALE : 6 clics rapides ≠ 6 requêtes ──────────────────────────
  const avantRafale = await netCount("/api/viz/subgraph");
  for (let k = 1; k <= 6; k++) {
    await ev(`(() => { const r = document.querySelectorAll('.proj-row')[${k}]; if (r) r.click(); return !!r; })()`);
    await sleep(70);
  }
  await sleep(2500);
  const parRafale = (await netCount("/api/viz/subgraph")) - avantRafale;
  noter(parRafale >= 1 && parRafale < 6,
    "4bis. une RAFALE de 6 clics n'émet pas une rafale de requêtes (coalescence)",
    `${parRafale} requête(s) de sous-graphe pour 6 clics rapprochés`);

  // ═══════════════════════════════════════════════════════════════════════════
  // PHASE 2 — LE GARDE-FOU DE VOLUME, sur un store de 413 985 nœuds.
  // Défaut mesuré : la projection se lançait sur N'IMPORTE QUELLE réponse (le
  // serveur ne borne pas /api/query) ; à 50 000 lignes la construction coûtait
  // ~25 ms pour 64 barres, et l'ancien Math.min(...vals) levait une RangeError
  // vers 125 000 lignes — dans le rendu, donc en cassant tout l'arbre React.
  // La requête retenue coûte 117 ms de moteur et rend 25 000 lignes (mesuré) :
  // au-dessus du seuil (20 000), sans écraser le service.
  {
    const REQUETE_LOURDE = "MATCH (n) RETURN n.community_id AS c LIMIT 25000";
    /** Pose une requête dans l'éditeur et la lance — en rétablissant le panneau
     *  si un rechargement (rebuild d'une autre session) l'a refermé. */
    const lancer = async (texte, cible) => {
      for (let essai = 0; essai < 3; essai++) {
        await assurerPanneau();
        const ok = await ev(`(() => { const t = document.querySelector('.q-editor');
          if (!t) return false;
          const set = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value').set;
          set.call(t, ${JSON.stringify(texte)});
          t.dispatchEvent(new Event('input', { bubbles: true }));
          return true; })()`);
        if (ok) { await sleep(300); await clicTexte("▶ Projeter"); }
        if (await attendre(cible, 30000)) return true;
        await sleep(1000);
      }
      return false;
    };
    await send("Page.navigate", { url: `${BASE}/?store=${encodeURIComponent("Runtime dev (engrams)")}` });
    await sleep(9000);
    origine = await waitStable();   // la navigation change `__t0` : on repart d'ici
    const refuse = await lancer(REQUETE_LOURDE, "!!document.querySelector('.proj-refus')");
    const txt = ((await ev("document.querySelector('.proj-refus')?.textContent || ''")) || "").replace(/\s+/g, " ").trim();
    const lignesRefus = await ev("document.querySelectorAll('.proj-row').length");
    const boutons = (await ev("[...document.querySelectorAll('.proj-refus button')].map(b => b.textContent.trim())")) || [];
    noter(refuse && /25[\s ]?000/.test(txt) && lignesRefus === 0 && boutons.length === 2,
      "7. au-delà du seuil : la projection NE se lance PAS d'office — elle DIT les lignes et propose les deux issues",
      `« ${txt.slice(0, 130)} » · boutons = [${boutons.join(" | ")}] · .proj-row = ${lignesRefus} (aucun axe construit)`);

    // « projeter quand même » : l'utilisateur force, la projection s'exécute.
    const avantForce = await ev("!!document.querySelector('.proj-refus')");
    await clicTexte("projeter quand même");
    const force = await attendre("!document.querySelector('.proj-refus')", 20000);
    noter(refuse === true && avantForce === true && force === true,
      "7bis. « projeter quand même » lève le refus (le choix reste à l'utilisateur)",
      `refus présent avant le clic : ${avantForce} · levé après : ${force}`);

    // « ajouter LIMIT 20000 » : la requête est RÉÉCRITE (sinon l'utilisateur
    // devrait la retaper — c'est-à-dire renoncer).
    const rearme = await lancer(REQUETE_LOURDE, "!!document.querySelector('.proj-refus')");
    await clicTexte("ajouter LIMIT");
    await sleep(2500);
    const texte = (await ev("document.querySelector('.q-editor')?.value || ''")) || "";
    const refusApres = await ev("!!document.querySelector('.proj-refus')");
    noter(rearme === true && /LIMIT 20000\s*$/i.test(texte.trim()) && refusApres === false,
      "7ter. « ajouter LIMIT 20000 » RÉÉCRIT la requête et la relance (le refus tombe)",
      `requête : « ${texte.trim().slice(-40)} » · refus restant : ${refusApres}`);
  }

  const origineApres = await ev("window.__t0");
  if (origineApres !== origine) info("⚠ la page a été rechargée pendant le test (rebuild du watcher dev) — les mesures réseau restent valides");

  if (!KEEP) {
    try {
      const shot = await send("Page.captureScreenshot", { format: "png" });
      writeFileSync("/tmp/projection-check.png", Buffer.from(shot.data, "base64"));
    } catch { /* sans capture */ }
  }

  const echecs = faits.filter((f) => !f.ok);
  console.log(`\n── ${echecs.length === 0 ? "PASS" : `FAIL (${echecs.length}/${faits.length})`} — projection multi-axes`);
  terminer(echecs.length === 0 ? 0 : 1);
} catch (e) {
  console.log(`  FAIL ${e.message}`);
  terminer(1);
} finally {
  try { ws?.close(); } catch { /* déjà fermé */ }
  if (!KEEP) chrome.kill("SIGKILL");
}
