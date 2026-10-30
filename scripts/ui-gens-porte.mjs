#!/usr/bin/env node
// ── PORTE : LE PANNEAU DES GENS (comptes et résonance) ──────────────────────
//
// L'ancien panneau « Comptes » avait ses styles EN LIGNE et aucune règle pour
// téléphone : mesuré, il tombait sur le champ de saisie du chat et ses boutons
// faisaient 22 px de haut. Cette porte vérifie ce qui a été corrigé :
//
//   1. le bouton « Gens » est atteignable au doigt et ne recouvre AUCUN autre
//      contrôle ;
//   2. ouvert, le panneau tient dans l'écran (rien qui déborde) ;
//   3. ses contrôles font ≥ 44 px — y compris les actions par ligne ;
//   4. PENDANT LA CONVERSATION, il s'efface : la saisie du chat n'est jamais
//      recouverte (c'est le défaut rapporté, mesuré à 274×50 px) ;
//   5. la RÈGLE DU PARTAGE est affichée : ce qui devient lisible, et ce qui ne
//      sort jamais. Un consentement dont on ne connaît pas les limites n'en est
//      pas un — donc c'est un contrôle, pas une décoration.
//
// Elle mesure la GÉOMÉTRIE, pas la beauté : elle juge ce qui empêche d'agir.
// Usage : node scripts/ui-gens-porte.mjs [base]

import { spawn } from "node:child_process";
import { existsSync } from "node:fs";

const BASE = process.argv[2] || "http://127.0.0.1:8472";
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
    "--remote-debugging-port=9261", `--user-data-dir=/tmp/gens-${L}-${Date.now()}`,
    `--window-size=${L},${H}`, "about:blank",
  ], { stdio: "ignore" });
  let ws, id = 0;
  const pend = new Map();
  const send = (m, p = {}) => new Promise((r) => { const i = ++id; pend.set(i, r); ws.send(JSON.stringify({ id: i, method: m, params: p })); });
  try {
    await sleep(3500);
    const t = await (await fetch("http://127.0.0.1:9261/json")).json();
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

    console.log(`\n── GENS EN ${L}×${H} ──`);
    // ÉTAT PAR DÉFAUT : la conversation est FERMÉE (l'interface s'ouvre sur ce
    // qu'elle sert à regarder). On s'en assure, sans quoi on mesurerait un état
    // que l'utilisateur n'a pas demandé.
    await ev(`(() => { const b = document.querySelector('.chat-tete button'); if (b && /✕/.test(b.textContent||'')) b.click(); })()`);
    await sleep(1200);

    // ── 1. LE BOUTON ────────────────────────────────────────────────────────
    const bouton = await ev(`(() => {
      const b = document.querySelector('.gens-bouton');
      if (!b) return { present: false };
      const r = b.getBoundingClientRect();
      const ctl = [...document.querySelectorAll('button, a, input, select, [role=button]')]
        .filter(e => { const q = e.getBoundingClientRect(); return q.width > 1 && q.height > 1 && e !== b; });
      const chev = ctl.filter(e => { const q = e.getBoundingClientRect();
        const ox = Math.min(r.right,q.right) - Math.max(r.left,q.left);
        const oy = Math.min(r.bottom,q.bottom) - Math.max(r.top,q.top);
        return ox > 6 && oy > 6; }).map(e => (e.textContent||'').trim().slice(0,20));
      return { present: true, l: Math.round(r.width), h: Math.round(r.height),
               dedans: r.left >= 0 && r.right <= innerWidth && r.bottom <= innerHeight, chev };
    })()`);
    if (!bouton?.present) ko("le bouton « Gens » est absent");
    else {
      bouton.h >= 44 ? ok(`bouton « Gens » ${bouton.l}×${bouton.h} px (≥ 44)`)
                     : ko(`bouton « Gens » ${bouton.l}×${bouton.h} px : sous 44`);
      bouton.dedans ? ok("le bouton tient dans l'écran") : ko("le bouton sort de l'écran");
      bouton.chev.length === 0 ? ok("il ne recouvre aucun autre contrôle")
        : ko(`il recouvre ${bouton.chev.length} contrôle(s) : ${bouton.chev.join(", ")}`);
    }

    // ── 2/3. OUVERT : GÉOMÉTRIE ET CIBLES ───────────────────────────────────
    await ev(`document.querySelector('.gens-bouton')?.click()`);
    await sleep(1500);
    const ouvert = await ev(`(() => {
      const p = document.querySelector('.gens-panneau');
      if (!p) return { ouvert: false };
      const r = p.getBoundingClientRect();
      const ctl = [...p.querySelectorAll('button, input, select')]
        .filter(e => { const q = e.getBoundingClientRect(); return q.width > 1 && q.height > 1; });
      return {
        ouvert: true,
        boite: { x: Math.round(r.x), y: Math.round(r.y), l: Math.round(r.width), h: Math.round(r.height) },
        dansEcran: r.left >= -1 && r.right <= innerWidth + 1,
        scrollInterne: p.scrollHeight > p.clientHeight + 2,
        n: ctl.length,
        tropPetits: ctl.filter(e => { const q = e.getBoundingClientRect(); return q.height < 44 || q.width < 44; })
          .map(e => ({ t: (e.textContent||'').trim().slice(0,22) || e.tagName,
                       l: Math.round(e.getBoundingClientRect().width), h: Math.round(e.getBoundingClientRect().height) })),
        texte: (p.innerText || '').slice(0, 400),
      };
    })()`);
    if (!ouvert?.ouvert) ko("le panneau ne s'ouvre pas au clic");
    else {
      ok(`panneau ouvert (${ouvert.boite.l}×${ouvert.boite.h} px, ${ouvert.n} contrôles)`);
      ouvert.dansEcran ? ok("le panneau tient dans l'écran") : ko(`le panneau déborde (x=${ouvert.boite.x} l=${ouvert.boite.l})`);
      ouvert.tropPetits.length === 0 ? ok("tous ses contrôles font ≥ 44 px")
        : ko(`${ouvert.tropPetits.length} contrôle(s) sous 44 px : ${ouvert.tropPetits.map((c) => `« ${c.t} » ${c.l}×${c.h}`).join(", ")}`);
      // ── 5. LA RÈGLE DU PARTAGE EST DITE ───────────────────────────────────
      const ditJamais = /jamais/i.test(ouvert.texte);
      const ditNoms = /noms?/i.test(ouvert.texte);
      ditJamais && ditNoms ? ok("la règle du partage est affichée (ce qui sort, ce qui ne sort jamais)")
        : ko(`la règle du partage n'est pas dite (jamais=${ditJamais}, noms=${ditNoms})`);
    }

    // ── 3bis. LA FICHE D'UN SELF QU'ON LIT S'OUVRE ──────────────────────────
    // Un self qu'on lit n'était qu'un NOM suivi de compteurs : on savait qu'il
    // partageait quelque chose, sans jamais pouvoir le lire. Or c'est tout
    // l'objet de la résonance. On exige donc que la ligne soit une CIBLE et
    // qu'elle DÉPLIE une fiche — sinon la fonctionnalité n'existe qu'en théorie.
    const lignes = await ev(`(() => {
      const l = [...document.querySelectorAll('.gens-ligne')];
      return l.map(e => { const r = e.getBoundingClientRect();
        return { t: (e.textContent||'').trim().slice(0,24), l: Math.round(r.width), h: Math.round(r.height) }; });
    })()`);
    if (!lignes?.length) {
      console.log("  (info) aucun self dans « Je lis » sur ce serveur : dépliage non mesurable");
    } else {
      const petite = lignes.filter((x) => x.h < 44 || x.l < 44);
      petite.length === 0 ? ok(`ligne de self cliquable et tactile (${lignes[0].l}×${lignes[0].h} px)`)
        : ko(`ligne de self trop petite : ${petite.map((x) => `« ${x.t} » ${x.l}×${x.h}`).join(", ")}`);
      await ev(`document.querySelector('.gens-ligne')?.click()`);
      await sleep(800);
      const fiche = await ev(`(() => {
        const f = document.querySelector('.gens-fiche');
        if (!f) return { ouverte: false };
        const r = f.getBoundingClientRect();
        return { ouverte: true, l: Math.round(r.width), h: Math.round(r.height),
                 texte: (f.innerText || '').slice(0, 400) };
      })()`);
      if (!fiche?.ouverte) ko("la ligne ne déplie AUCUNE fiche : on ne peut pas lire les facettes");
      else {
        ok(`la fiche s'ouvre (${fiche.l}×${fiche.h} px)`);
        /jamais lisible/i.test(fiche.texte)
          ? ok("la fiche dit ce qui n'est JAMAIS lisible (au moment où on regarde)")
          : ko("la fiche ne dit pas ses limites : un partage partiel se lirait comme total");
      }
      await ev(`document.querySelector('.gens-ligne.on')?.click()`);
      await sleep(500);
    }

    // ── 4. PENDANT LA CONVERSATION, LE PANNEAU S'EFFACE ─────────────────────
    await ev(`document.querySelector('.gens-bouton')?.click()`);   // refermer
    await sleep(400);
    await ev(`(() => { const b = document.querySelector('.chat-flottant'); if (b) b.click(); })()`);
    await sleep(2500);
    const pendantChat = await ev(`(() => {
      const g = document.querySelector('.gens-bouton');
      const p = document.querySelector('.gens-panneau');
      const champ = [...document.querySelectorAll('textarea')].pop();
      const vis = e => { if (!e) return false; const s = getComputedStyle(e), r = e.getBoundingClientRect();
        return s.display !== 'none' && s.visibility !== 'hidden' && r.width > 1 && r.height > 1; };
      let recouvre = false;
      if (champ && vis(champ)) {
        const r = champ.getBoundingClientRect();
        const dessus = document.elementFromPoint(r.left + r.width/2, r.top + r.height/2);
        recouvre = !(dessus === champ || champ.contains(dessus));
      }
      return { boutonVisible: vis(g), panneauVisible: vis(p), champVisible: vis(champ), recouvre };
    })()`);
    if (!pendantChat?.champVisible) console.log("  (info) pas de conversation ouverte à cette largeur : contrôle sans objet");
    else {
      pendantChat.boutonVisible || pendantChat.panneauVisible
        ? ko("le panneau « Gens » reste visible pendant la conversation")
        : ok("le panneau s'efface pendant la conversation");
      pendantChat.recouvre ? ko("la saisie du chat est recouverte")
        : ok("la saisie du chat n'est recouverte par rien");
    }
  } catch (e) {
    ko(`mesure impossible en ${L}×${H} : ${e.message}`);
  } finally {
    try { chrome.kill("SIGKILL"); } catch { /* déjà mort */ }
  }
}

console.log(echecs === 0
  ? "\n✅ le panneau des gens est utilisable au doigt, et il ne recouvre rien"
  : `\n❌ ${echecs} contrôle(s) en échec`);
process.exit(echecs === 0 ? 0 : 1);
