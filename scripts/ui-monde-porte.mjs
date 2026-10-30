#!/usr/bin/env node
// ── PORTE : LE PANNEAU « MONDE » (ajouter un dépôt) ─────────────────────────
//
// C'est LE geste par lequel un compte neuf obtient sa première base. Il vivait
// dans un panneau du canvas — sous la charge GPU de la scène ET sous son risque
// d'échec. Ce qui se vérifie ici n'est donc pas cosmétique :
//
//   1. le bouton « ➕ Monde » existe, fait ≥ 44 px dans les DEUX sens, tient dans
//      l'écran et ne recouvre AUCUN autre contrôle (la colonne de droite porte
//      déjà ⌂ et 💬 : c'est là que les collisions se produisent) ;
//   2. ouvert, le panneau tient dans l'écran et ses contrôles sont tactiles ;
//   3. le CHAMP D'ADRESSE accepte la saisie — sans lui, aucun dépôt ne s'ajoute ;
//   4. PENDANT LA CONVERSATION, le panneau s'efface (la saisie du chat n'est
//      jamais recouverte : c'est le défaut qui a ouvert ce chantier) ;
//   5. il est LIBÉRÉ DU CANVAS : il vit hors de `.stage`, donc l'échec de la
//      scène ne peut plus l'emporter.
//
// Usage : node scripts/ui-monde-porte.mjs [base]

import { spawn } from "node:child_process";
import { existsSync } from "node:fs";

const BASE = process.argv[2] || "http://127.0.0.1:8477";
const CHROME = process.env.CHROME
  || "/home/triviere/.cache/ms-playwright/chromium-1234/chrome-linux64/chrome";
if (!existsSync(CHROME)) { console.log(`SKIP — chromium absent (${CHROME})`); process.exit(0); }

const TAILLES = [[390, 844], [360, 640], [320, 568]];
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let echecs = 0;
const ok = (m) => console.log(`  ✅ ${m}`);
const ko = (m) => { console.log(`  ❌ ${m}`); echecs++; };

for (const [L, H] of TAILLES) {
  const chrome = spawn(CHROME, [
    "--headless=new", "--no-sandbox", "--disable-dev-shm-usage",
    "--use-angle=swiftshader", "--enable-unsafe-swiftshader",
    "--remote-debugging-port=9264", `--user-data-dir=/tmp/monde-${L}-${Date.now()}`,
    `--window-size=${L},${H}`, "about:blank",
  ], { stdio: "ignore" });
  let ws, id = 0;
  const pend = new Map();
  const send = (m, p = {}) => new Promise((r) => { const i = ++id; pend.set(i, r); ws.send(JSON.stringify({ id: i, method: m, params: p })); });
  try {
    await sleep(3500);
    const t = await (await fetch("http://127.0.0.1:9264/json")).json();
    ws = new WebSocket(t.find((x) => x.type === "page").webSocketDebuggerUrl);
    await new Promise((r) => ws.addEventListener("open", r, { once: true }));
    ws.addEventListener("message", (e) => {
      const m = JSON.parse(e.data);
      if (m.id && pend.has(m.id)) { pend.get(m.id)(m.result); pend.delete(m.id); }
    });
    await send("Runtime.enable"); await send("Page.enable");
    await send("Emulation.setDeviceMetricsOverride", { width: L, height: H, deviceScaleFactor: 2, mobile: true });
    await send("Emulation.setTouchEmulationEnabled", { enabled: true, maxTouchPoints: 5 });
  // ── LE JETON, QUAND LE SERVEUR EST ARMÉ ─────────────────────────────────
  // Depuis que l'accueil redirige vers `/login` sans session (mesuré le
  // 2026-09-16), une porte qui ne présente pas d'identité mesure la PAGE DE
  // CONNEXION en croyant mesurer le dashboard. `OBRAIN_TOKEN` (rendu par
  // `scripts/banc-interface-authentifiee.sh`) pose le cookie de session dans le
  // navigateur avant toute navigation.
  const jeton = process.env.OBRAIN_TOKEN || "";
  if (jeton) {
    const verif = await send("Network.enable");
    await send("Network.setCookie", { name: "obrain_session", value: jeton, domain: new URL(BASE).hostname, path: "/" });
  }
    await send("Page.navigate", { url: BASE + "/" });
    await sleep(9000);
    const ev = async (e) => (await send("Runtime.evaluate", { expression: e, returnByValue: true, awaitPromise: true }))?.result?.value;

    console.log(`\n── MONDE EN ${L}×${H} ──`);

    // ── 1. LE BOUTON ────────────────────────────────────────────────────────
    const bouton = await ev(`(() => {
      const b = document.querySelector('.monde-bouton');
      if (!b) return { present: false };
      const r = b.getBoundingClientRect();
      const autres = [...document.querySelectorAll('button, a, input, select, [role=button]')]
        .filter(e => { const q = e.getBoundingClientRect(); return q.width > 1 && q.height > 1 && e !== b; });
      const chev = autres.filter(e => { const q = e.getBoundingClientRect();
        const ox = Math.min(r.right,q.right) - Math.max(r.left,q.left);
        const oy = Math.min(r.bottom,q.bottom) - Math.max(r.top,q.top);
        return ox > 6 && oy > 6; }).map(e => (e.textContent||'').trim().slice(0,16));
      return { present: true, l: Math.round(r.width), h: Math.round(r.height),
               dedans: r.left >= -1 && r.right <= innerWidth + 1,
               chev };
    })()`);
    if (!bouton?.present) ko("le bouton « ➕ Monde » est absent");
    else {
      bouton.l >= 44 && bouton.h >= 44 ? ok(`bouton ${bouton.l}×${bouton.h} px (≥ 44 dans les deux sens)`)
        : ko(`bouton ${bouton.l}×${bouton.h} px : sous 44 dans une dimension`);
      bouton.dedans ? ok("le bouton tient dans l'écran") : ko("le bouton sort de l'écran");
      bouton.chev.length === 0 ? ok("il ne recouvre aucun autre contrôle")
        : ko(`il recouvre : ${bouton.chev.join(", ")}`);
    }

    // ── 2/3. OUVERT : GÉOMÉTRIE, CIBLES, SAISIE ─────────────────────────────
    await ev(`document.querySelector('.monde-bouton')?.click()`);
    await sleep(1500);
    const ouvert = await ev(`(() => {
      const p = document.querySelector('.monde-panneau');
      if (!p) return { ouvert: false };
      const r = p.getBoundingClientRect();
      const ctl = [...p.querySelectorAll('button, input, select, summary')]
        .filter(e => { const q = e.getBoundingClientRect(); return q.width > 1 && q.height > 1; });
      const champ = p.querySelector('input[aria-label="Adresse du dépôt GitHub"], input');
      return {
        ouvert: true,
        l: Math.round(r.width), h: Math.round(r.height),
        dansEcran: r.left >= -1 && r.right <= innerWidth + 1,
        n: ctl.length,
        tropPetits: ctl.filter(e => { const q = e.getBoundingClientRect(); return q.height < 44; })
          .map(e => ({ t: (e.textContent||'').trim().slice(0,20) || e.tagName,
                       h: Math.round(e.getBoundingClientRect().height) })),
        champ: champ ? { l: Math.round(champ.getBoundingClientRect().width),
                         h: Math.round(champ.getBoundingClientRect().height) } : null,
        texte: (p.innerText || '').slice(0, 220),
      };
    })()`);
    if (!ouvert?.ouvert) ko("le panneau ne s'ouvre pas au clic");
    else {
      ok(`panneau ouvert (${ouvert.l}×${ouvert.h} px, ${ouvert.n} contrôles)`);
      ouvert.dansEcran ? ok("le panneau tient dans l'écran") : ko("le panneau déborde");
      ouvert.tropPetits.length === 0 ? ok("tous ses contrôles font ≥ 44 px de haut")
        : ko(`trop petits : ${ouvert.tropPetits.map((c) => `« ${c.t} » ${c.h}px`).join(", ")}`);
      if (!ouvert.champ) ko("aucun champ d'adresse : impossible d'ajouter un dépôt");
      else {
        ouvert.champ.h >= 44 ? ok(`champ d'adresse ${ouvert.champ.l}×${ouvert.champ.h} px (≥ 44)`)
          : ko(`champ d'adresse de ${ouvert.champ.h} px : sous 44`);
        // La saisie doit VRAIMENT fonctionner : c'est tout l'objet du panneau.
        const saisi = await ev(`(() => {
          const e = document.querySelector('.monde-panneau input');
          if (!e) return null;
          e.focus();
          const proto = window.HTMLInputElement.prototype;
          Object.getOwnPropertyDescriptor(proto, 'value').set.call(e, 'https://github.com/a/b');
          e.dispatchEvent(new Event('input', { bubbles: true }));
          return e.value;
        })()`);
        saisi === "https://github.com/a/b" ? ok("on peut écrire l'adresse du dépôt")
          : ko(`saisie impossible (valeur = « ${saisi} »)`);
      }
    }

    // ── 4. PENDANT LA CONVERSATION, LE PANNEAU S'EFFACE ─────────────────────
    await ev(`document.querySelector('.monde-bouton')?.click()`);
    await sleep(400);
    await ev(`document.querySelector('.chat-flottant')?.click()`);
    await sleep(2500);
    const pendant = await ev(`(() => {
      const vis = e => { if (!e) return false; const s = getComputedStyle(e), r = e.getBoundingClientRect();
        return s.display !== 'none' && s.visibility !== 'hidden' && r.width > 1 && r.height > 1; };
      const champ = [...document.querySelectorAll('textarea')].pop();
      let recouvre = false;
      if (vis(champ)) { const r = champ.getBoundingClientRect();
        const dessus = document.elementFromPoint(r.left + r.width/2, r.top + r.height/2);
        recouvre = !(dessus === champ || champ.contains(dessus)); }
      return { bouton: vis(document.querySelector('.monde-bouton')),
               panneau: vis(document.querySelector('.monde-panneau')),
               recouvre };
    })()`);
    pendant.bouton || pendant.panneau ? ko("le panneau « Monde » reste visible pendant la conversation")
      : ok("le panneau s'efface pendant la conversation");
    pendant.recouvre ? ko("la saisie du chat est recouverte") : ok("la saisie du chat n'est recouverte par rien");

    // ── 5. LIBÉRÉ DU CANVAS ─────────────────────────────────────────────────
    const horsCanvas = await ev(`(() => {
      const p = document.querySelector('.monde-panneau') || document.querySelector('.monde-bouton');
      const stage = document.querySelector('.stage');
      if (!p) return { p: false };
      if (!stage) return { p: true, dedans: false, note: 'pas de scène montée' };
      return { p: true, dedans: stage.contains(p) };
    })()`);
    horsCanvas?.dedans ? ko("le panneau est DANS le conteneur de la scène")
      : ok("le panneau vit hors de la scène (indépendant du canvas)");
  } catch (e) {
    ko(`mesure impossible en ${L}×${H} : ${e.message}`);
  } finally {
    try { chrome.kill("SIGKILL"); } catch { /* déjà mort */ }
  }
}

console.log(echecs === 0
  ? "\n✅ l'ajout de monde est utilisable au doigt, et libéré du canvas"
  : `\n❌ ${echecs} contrôle(s) en échec`);
process.exit(echecs === 0 ? 0 : 1);
