#!/usr/bin/env node
// ── PORTE : PEUT-ON VRAIMENT CONVERSER ? ────────────────────────────────────
//
// « Je dois pouvoir avoir des conversations » — et le défaut rapporté était que
// la vue des comptes venait SE POSER sur le champ de saisie. Les deux se
// vérifient sur la géométrie, pas sur une impression :
//
//   1. le champ de saisie EXISTE, il est visible, et il fait au moins 44 px de
//      haut (sur téléphone, un champ plus court est un champ qu'on rate) ;
//   2. l'élément qui reçoit réellement le clic AU CENTRE du champ est le champ
//      lui-même — c'est la mesure qui compte : un panneau posé par-dessus laisse
//      le champ « visible » tout en le rendant inutilisable (défaut déjà mesuré
//      une fois dans ce dépôt, où un en-tête de panneau recevait le clic) ;
//   3. on peut Y ÉCRIRE : on tape un texte, on vérifie que la valeur est bien
//      dans le champ, et on envoie (Entrée) — le tour apparaît dans la
//      conversation ;
//   4. rien d'autre ne se trouve SUR le champ (aucun contrôle superposé) ;
//   5. la conversation est INDÉPENDANTE du canvas : les deux vivent dans des
//      conteneurs distincts, et retirer/masquer la scène ne l'emporte pas.
//
// Usage : node scripts/ui-chat-porte.mjs [base] [largeur] [hauteur]

import { spawn } from "node:child_process";
import { existsSync } from "node:fs";

const BASE = process.argv[2] || "http://127.0.0.1:8466";
const L = Number(process.argv[3] || 390);
const H = Number(process.argv[4] || 844);
const CHROME = process.env.CHROME
  || "/home/triviere/.cache/ms-playwright/chromium-1234/chrome-linux64/chrome";
if (!existsSync(CHROME)) { console.log(`SKIP — chromium absent (${CHROME})`); process.exit(0); }

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const chrome = spawn(CHROME, [
  "--headless=new", "--no-sandbox", "--disable-dev-shm-usage",
  "--use-angle=swiftshader", "--enable-unsafe-swiftshader",
  "--remote-debugging-port=9254", `--user-data-dir=/tmp/obrain-chat-${Date.now()}`,
  `--window-size=${L},${H}`, "about:blank",
], { stdio: "ignore" });

let echecs = 0;
const ok = (m) => console.log(`  ✅ ${m}`);
const ko = (m) => { console.log(`  ❌ ${m}`); echecs++; };
let ws, id = 0;
const pend = new Map();
const send = (m, p = {}) => new Promise((r) => { const i = ++id; pend.set(i, r); ws.send(JSON.stringify({ id: i, method: m, params: p })); });

try {
  await sleep(3500);
  const t = await (await fetch("http://127.0.0.1:9254/json")).json();
  ws = new WebSocket(t.find((x) => x.type === "page").webSocketDebuggerUrl);
  await new Promise((r) => ws.addEventListener("open", r, { once: true }));
  ws.addEventListener("message", (e) => {
    const m = JSON.parse(e.data);
    if (m.id && pend.has(m.id)) { pend.get(m.id)(m.result); pend.delete(m.id); }
  });
  await send("Runtime.enable"); await send("Page.enable");
  await send("Emulation.setDeviceMetricsOverride", { width: L, height: H, deviceScaleFactor: 2, mobile: true });
  // ── LE JETON, QUAND LE SERVEUR EST ARMÉ ─────────────────────────────────
  // Depuis que l'accueil redirige vers `/login` sans session, une porte sans
  // identité mesure la PAGE DE CONNEXION en croyant mesurer le dashboard.
  // `OBRAIN_TOKEN` (rendu par `scripts/banc-interface-authentifiee.sh`) pose le
  // cookie de session avant toute navigation.
  const jeton = process.env.OBRAIN_TOKEN || "";
  if (jeton) {
    await send("Network.enable");
    await send("Network.setCookie", { name: "obrain_session", value: jeton, domain: new URL(BASE).hostname, path: "/" });
  }
  await send("Page.navigate", { url: BASE + "/" });
  await sleep(10000);
  const ev = async (e) => (await send("Runtime.evaluate", { expression: e, returnByValue: true, awaitPromise: true }))?.result?.value;

  console.log(`── CONVERSATION EN ${L}×${H} ──`);

  // ── ON OUVRE LA CONVERSATION COMME LE FERAIT L'UTILISATEUR ──────────────
  // Depuis que le chat est FERMÉ au chargement (l'interface s'ouvre sur ce
  // qu'elle sert à regarder, pas sur un panneau), ce contrôle commence par le
  // geste réel : cliquer « 💬 ». Le tester autrement mesurerait un état que
  // personne n'a demandé.
  const ouvertParClic = await ev(`(() => {
    const b = document.querySelector('.chat-flottant');
    if (!b) return false;
    b.click();
    return true;
  })()`);
  await sleep(2500);
  if (!ouvertParClic) ko("aucun bouton pour ouvrir la conversation (💬 absent)");
  else ok("la conversation s'ouvre par le bouton 💬");

  // ── 1. LE CHAMP EXISTE ET EST UTILISABLE ────────────────────────────────
  const champ = await ev(`(() => {
    const c = [...document.querySelectorAll('textarea, input[type=text]')]
      .filter(e => { const r = e.getBoundingClientRect(); return r.width > 1 && r.height > 1; });
    if (!c.length) return { trouve: false };
    const e = c[c.length - 1], r = e.getBoundingClientRect();
    const cx = r.left + r.width/2, cy = r.top + r.height/2;
    const dessus = document.elementFromPoint(cx, cy);
    return { trouve: true, tag: e.tagName.toLowerCase(),
      x: Math.round(r.x), y: Math.round(r.y), l: Math.round(r.width), h: Math.round(r.height),
      desactive: e.disabled || e.readOnly,
      placeholder: e.getAttribute('placeholder') || '',
      // QUI reçoit le clic au centre : le champ, ou un panneau posé dessus ?
      dessus: dessus ? (dessus.tagName.toLowerCase() + (dessus.className ? '.' + String(dessus.className).split(' ')[0] : '')) : 'aucun',
      dessusEstChamp: dessus === e || (dessus && e.contains(dessus)) };
  })()`);

  if (!champ?.trouve) ko("aucun champ de saisie visible : on ne peut pas converser");
  else {
    ok(`champ présent (${champ.l}×${champ.h} px, ${champ.tag})`);
    champ.h >= 44 ? ok(`hauteur ${champ.h} px ≥ 44 : atteignable au doigt`)
                  : ko(`champ de ${champ.h} px : sous le seuil tactile de 44 px`);
    !champ.desactive ? ok("champ actif (ni désactivé ni en lecture seule)")
                     : ko("le champ est désactivé ou en lecture seule");
    champ.dessusEstChamp ? ok(`c'est bien le champ qui reçoit le clic (dessus : ${champ.dessus})`)
      : ko(`le clic au centre tombe sur « ${champ.dessus} » : quelque chose recouvre la saisie`);
  }

  // ── 2. UN CONTRÔLE SUPERPOSÉ AU CHAMP ? ─────────────────────────────────
  const surLeChamp = await ev(`(() => {
    const c = [...document.querySelectorAll('textarea, input[type=text]')]
      .filter(e => { const r = e.getBoundingClientRect(); return r.width > 1 && r.height > 1; }).pop();
    if (!c) return [];
    const r = c.getBoundingClientRect();
    return [...document.querySelectorAll('button, a, [role=button]')].filter(b => {
      const q = b.getBoundingClientRect();
      const ox = Math.min(r.right,q.right) - Math.max(r.left,q.left);
      const oy = Math.min(r.bottom,q.bottom) - Math.max(r.top,q.top);
      return ox > 6 && oy > 6;
    }).map(b => ({ t: (b.textContent||'').trim().slice(0,28), cls: String(b.className||'').slice(0,24) }));
  })()`);
  surLeChamp.length === 0 ? ok("aucun contrôle ne recouvre le champ")
    : ko(`${surLeChamp.length} contrôle(s) SUR le champ : ${surLeChamp.map((b) => `« ${b.t} » (${b.cls})`).join(", ")}`);

  // ── 3. ON ÉCRIT VRAIMENT, ET ON ENVOIE ──────────────────────────────────
  const ecrit = await ev(`(() => {
    const e = [...document.querySelectorAll('textarea, input[type=text]')]
      .filter(x => { const r = x.getBoundingClientRect(); return r.width > 1 && r.height > 1; }).pop();
    if (!e) return null;
    e.focus();
    const proto = e.tagName === 'TEXTAREA' ? window.HTMLTextAreaElement.prototype : window.HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(proto, 'value').set.call(e, 'bonjour');
    e.dispatchEvent(new Event('input', { bubbles: true }));
    return { valeur: e.value, focus: document.activeElement === e };
  })()`);
  ecrit?.valeur === "bonjour" ? ok("on peut écrire dans le champ (« bonjour » posé)")
    : ko(`écriture impossible dans le champ (valeur = « ${ecrit?.valeur} »)`);
  ecrit?.focus ? ok("le champ prend le focus") : ko("le champ ne prend pas le focus");

  const envoye = await ev(`(() => {
    const e = [...document.querySelectorAll('textarea, input[type=text]')]
      .filter(x => { const r = x.getBoundingClientRect(); return r.width > 1 && r.height > 1; }).pop();
    if (!e) return null;
    e.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    return true;
  })()`);
  await sleep(2500);
  const apres = await ev(`document.body.innerText`);
  const tourApparu = /bonjour/i.test(apres || "");
  envoye && tourApparu ? ok("l'envoi fait apparaître le tour dans la conversation")
    : ko("le message envoyé n'apparaît pas dans la conversation");

  // ── 4bis. LES RÉGLAGES REPLIÉS SONT RAPPELABLES ────────────────────────
  // Mesuré : à 320 px, l'en-tête ne peut pas porter cinq éléments en restant
  // lisible (≥ 11 px) et tactile (≥ 44 px). On en retire donc deux — mais un
  // réglage retiré SANS RECOURS est un cul-de-sac. Ce contrôle exige que le
  // bouton de rappel existe, qu'il soit atteignable, et qu'il FONCTIONNE.
  const rappel = await ev(`(() => {
    const b = document.querySelector('.chat-reglages-bouton');
    if (!b) return { present: false };
    const r = b.getBoundingClientRect();
    if (r.width < 1 || r.height < 1) return { present: true, visible: false };
    const avant = [...document.querySelectorAll('.chat-reglage')].filter(e => e.getBoundingClientRect().width > 1).length;
    b.click();
    return { present: true, visible: true, l: Math.round(r.width), h: Math.round(r.height), avant };
  })()`);
  await sleep(600);
  const apresRepli = await ev(`[...document.querySelectorAll('.chat-reglage')].filter(e => e.getBoundingClientRect().width > 1).length`);
  if (!rappel?.present) ok("pas de réglages repliés à cette largeur (rien à rappeler)");
  else if (!rappel.visible) ko("le bouton de rappel des réglages existe mais n'est pas visible");
  else {
    ok(`bouton de rappel présent (${rappel.l}×${rappel.h})`);
    apresRepli > rappel.avant ? ok(`le rappel FAIT APPARAÎTRE les réglages (${rappel.avant} → ${apresRepli})`)
      : ko(`le rappel ne montre rien (${rappel.avant} → ${apresRepli}) : les réglages sont un cul-de-sac`);
  }

  // ── 4ter. LA CONVERSATION EST CELLE D'UN SELF, PAS D'UN NAVIGATEUR ──────
  // Plusieurs personnes peuvent ouvrir le même navigateur, et chacune a SON self
  // (le serveur le décide et l'annonce). Si l'historique se rangeait par base
  // seule, la conversation d'un compte resterait à l'écran du suivant — le
  // mélange même que tout ce chantier combat. On vérifie donc la CLÉ de
  // stockage, pas seulement que « ça marche ».
  // ── ON ATTEND QUE LA RÉPONSE ARRIVE VRAIMENT ────────────────────────────
  // Erreur de ma première version : je lisais le stockage 2 s après l'envoi. Or
  // le self est annoncé par le serveur dans l'événement `contexte`, qui vient
  // APRÈS la réflexion du modèle — mesuré plus long que 9 s avec un modèle
  // distant. Je concluais « rangée sans self » alors que le self n'avait pas
  // encore parlé. Un contrôle qui échantillonne trop tôt accuse le code de sa
  // propre impatience : on attend que le tour soit posé (ou 40 s), puis on lit.
  for (let i = 0; i < 20; i++) {
    const pret = await ev(`(() => {
      const cles = Object.keys(localStorage).filter(k => k.startsWith('obrain.chat'));
      return cles.some(k => /self/.test(k));
    })()`);
    if (pret) break;
    await sleep(2000);
  }
  const histo = await ev(`(() => {
    const cles = Object.keys(localStorage).filter(k => k.startsWith('obrain.chat'));
    return { cles, ancienne: cles.filter(k => k.startsWith('obrain.chat.v1.')) };
  })()`);
  if (!histo?.cles?.length) {
    ko("aucune conversation enregistrée : on ne peut pas vérifier à quel self elle appartient");
  } else if (histo.ancienne.length) {
    ko(`une clé de l'ancien format subsiste : ${histo.ancienne.join(", ")} — la conversation n'est pas rangée par self`);
  } else {
    // ── DEUX CAS À NE PAS CONFONDRE (erreur de ma première version) ─────────
    // Sans compte du tout (IAM désarmé sur ce serveur, ou aucune session), la
    // conversation NE PEUT PAS être rangée par personne : la clé provisoire est
    // le comportement juste, et la refuser ferait échouer le contrôle sur un
    // serveur de test. Ce qui serait un VRAI défaut, c'est une session
    // AUTHENTIFIÉE dont la conversation n'est rangée ni par compte ni par self —
    // c'est là que deux personnes se partageraient l'écran.
    const qui = await ev(`fetch('/api/auth/whoami').then(r => r.json()).then(d => !!(d && d.authentifie)).catch(() => false)`);
    const parPersonne = histo.cles.filter((k) => /self[:%]/.test(k) || k.includes("compte:"));
    if (parPersonne.length) ok(`conversation rangée par personne (${parPersonne[0].slice(0, 46)}…)`);
    else if (!qui) ok("aucun compte sur ce serveur : la clé provisoire est le bon comportement (rien à isoler)");
    else ko(`session AUTHENTIFIÉE mais conversation rangée sans compte ni self : ${histo.cles.join(", ")}`);
  }

  // ── 5. LA CONVERSATION EST INDÉPENDANTE DE LA SCÈNE ─────────────────────
  // Première formulation (fausse) : « le canvas ne contient pas le chat » en
  // remontant jusqu'à `.shell`. Or `.shell` contient TOUT — le test ne pouvait
  // que réussir ou accuser à tort. La question utile est : le chat est-il dans
  // le conteneur de la SCÈNE (`.stage`, celui qui porte le canvas et son rendu) ?
  // S'il est dehors, il vit sa vie : ni ses re-rendus, ni une scène cassée ne
  // l'emportent.
  const conteneurs = await ev(`(() => {
    const chat = document.querySelector('.chat-plein, .chat-panneau');
    const stage = document.querySelector('.stage');
    if (!chat) return { chat: false };
    if (!stage) return { chat: true, stage: false };
    return { chat: true, stage: true, dedans: stage.contains(chat),
             canvasDansStage: !!stage.querySelector('canvas') };
  })()`);
  if (!conteneurs?.stage) console.log("  (info) pas de scene montee ici (headless sans WebGL) : l'independance est structurelle — `CosmosWeave` n'importe plus le chat");
  else if (conteneurs.dedans) ko("le chat est TOUJOURS dans le conteneur du canvas");
  else ok("le chat vit hors du conteneur du canvas (independant de la scene)");
} catch (e) {
  ko(`test impossible : ${e.message}`);
} finally {
  try { chrome.kill("SIGKILL"); } catch { /* déjà mort */ }
}
console.log(echecs === 0 ? "\n✅ on peut converser, et la saisie n'est recouverte par rien"
  : `\n❌ ${echecs} contrôle(s) en échec`);
process.exit(echecs === 0 ? 0 : 1);
