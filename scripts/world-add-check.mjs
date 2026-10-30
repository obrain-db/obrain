#!/usr/bin/env node
// AJOUTER UN MONDE — le geste complet depuis l'interface.
//
// Ce contrôle fait ce que ferait l'utilisateur, et rien d'autre : il ouvre le
// Cosmos, tape une adresse GitHub, clique « Ajouter », attend, et exige que le
// monde APPARAISSE — dans le message d'abord, puis dans la scène.
//
// C'est la seule façon de vérifier la demande (« je devrais pouvoir ajouter un
// repo depuis github sur mon interface pour voir son monde apparaître ») : un
// test qui appellerait l'API directement vérifierait le serveur, pas le geste.
//
// Usage : node scripts/world-add-check.mjs [base] [url-github] [limite-secondes]
const BASE = process.argv[2] || "http://192.168.1.103:8432";
const REPO = process.argv[3] || "https://github.com/octocat/Spoon-Knife";
const LIMITE = Number(process.argv[4] || 120);
const CDP = process.env.CDP_URL || "http://127.0.0.1:9248";

const j = async (p, init) => {
  const r = await fetch(CDP + p, init);
  const t = await r.text();
  try { return JSON.parse(t); } catch { return t; }
};

// Le monde est-il visible par le Cosmos AVANT l'ajout ?
//
// On vérifie la PRÉSENCE du monde, pas l'augmentation du NOMBRE : ré-ajouter un
// dépôt déjà présent rafraîchit son clone et le réindexe — c'est légitime, et
// le compte ne bouge pas. Un contrôle qui exige « +1 » échoue donc à tort au
// deuxième passage (constaté).
const nom = REPO.split("/").pop().replace(/\.git$/, "");
const avant = await (await fetch(`${BASE}/api/viz/signatures`)).json();
const present = (j) => (j.stores || []).some((s) => (s.label || "").includes(nom));
const etaitPresent = present(avant);

const tab = await j("/json/new?" + encodeURIComponent(BASE + "/?cosmos=1"), { method: "PUT" });
if (!tab?.webSocketDebuggerUrl) { console.error("monde : onglet CDP refusé"); process.exit(2); }
const ws = new WebSocket(tab.webSocketDebuggerUrl);
let id = 0;
const pending = new Map();
const errors = [];
const send = (m, p = {}) => new Promise((r) => { const i = ++id; pending.set(i, r); ws.send(JSON.stringify({ id: i, method: m, params: p })); });
ws.addEventListener("message", (ev) => {
  const m = JSON.parse(ev.data);
  if (m.id && pending.has(m.id)) { pending.get(m.id)(m.result); pending.delete(m.id); return; }
  if (m.method === "Runtime.exceptionThrown") errors.push(m.params.exceptionDetails.text);
});
await new Promise((r) => ws.addEventListener("open", r, { once: true }));
await send("Runtime.enable"); await send("Log.enable"); await send("Page.enable");
await new Promise((r) => setTimeout(r, 9000));

const txt = async () => (await send("Runtime.evaluate", {
  returnByValue: true, expression: "document.body.innerText",
}))?.result?.value || "";

const zone = (await txt()).includes("Ajouter un monde");

// saisir l'adresse puis cliquer (le vrai geste)
await send("Runtime.evaluate", {
  expression: `(() => { const i = document.querySelector('input[aria-label="Adresse du dépôt GitHub"]');
    if (!i) return false; i.focus();
    const set = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
    set.call(i, ${JSON.stringify(REPO)});
    i.dispatchEvent(new Event('input', { bubbles: true })); return true; })()`,
});
await new Promise((r) => setTimeout(r, 400));
await send("Runtime.evaluate", {
  expression: `[...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'Ajouter')?.click()`,
});

// attendre la fin (clone + indexation + enrichissement)
let t = "";
const t0 = Date.now();
let etapes = [];
while ((Date.now() - t0) / 1000 < LIMITE) {
  await new Promise((r) => setTimeout(r, 3000));
  t = await txt();
  // ── L'ENRICHISSEMENT FAIT PARTIE DES ÉTAPES OBSERVÉES ────────────────────
  // Défaut mesuré le 2026-09-15 : l'étape n'existait pas côté serveur, donc ce
  // contrôle ne pouvait pas la voir — il validait un monde BRUT en annonçant
  // « monde ajouté ». Une étape qu'on ne nomme pas ici est une étape dont
  // l'absence ne se remarque pas.
  const e = (t.match(/(clone|indexation|enrichissement)\s+—\s+([^\n]+)/) || [])[2];
  if (e && etapes[etapes.length - 1] !== e) etapes.push(e);
  // ⚠ Le marqueur d'erreur doit être SPÉCIFIQUE au formulaire : chercher
  // « n'est pas » suffisait à sortir de la boucle dès le 1er sondage, car le
  // panneau Cosmos contient « Le centre n'est pas fixe ». Le contrôle
  // concluait à l'échec alors que le monde venait d'être ajouté.
  if (t.includes("ajouté :") || /monde :[^\n]*accepté|git clone a échoué|indexation :/.test(t)) break;
}

// le monde est-il apparu côté serveur ?
const apres = await (await fetch(`${BASE}/api/viz/signatures`)).json();
const nApres = (apres.stores || []).length;
const mAjout = t.match(/ajouté\s*:\s*(\d+) nœuds,\s*(\d+) arêtes en ([\d.]+) s/);
const message = t.includes("ajouté :");
const apparu = present(apres);
const erreurVisible = t.match(/monde :[^\n]{0,120}|git clone a échoué[^\n]{0,120}/)?.[0] || null;

// ── LE MONDE EST-IL ARRIVÉ ENRICHI ? ────────────────────────────────────────
// C'est la question que ce contrôle ne posait pas — et c'est celle qui compte
// pour l'utilisateur (« le même monde que les presets »). On la lit sur le
// rapport affiché : courbure mesurée, et AUCUN échec d'enrichissement nommé.
// Un rapport partiel doit faire ÉCHOUER le contrôle : c'est précisément le
// mensonge par omission qu'on corrige, et il reviendrait par le rapport si on
// se contentait de chercher « enrichi ».
const enrichi = /enrichi en [\d.]+ s/.test(t);
const courbure = /courbure : \d+ arête\(s\)/.test(t);
const partiel = /enrichissement PARTIEL/.test(t);
const sansRapport = /aucun compte rendu/.test(t);
const enrichOk = enrichi && courbure && !partiel && !sansRapport;

const ok = zone && message && apparu && enrichOk && !errors.length;
console.log("AJOUTER UN MONDE DEPUIS L'INTERFACE\n");
console.log(`  zone « Ajouter un monde » présente : ${zone ? "oui" : "NON"}`);
console.log(`  étapes traversées : ${etapes.join(" → ") || "(aucune, trop rapide)"}`);
console.log(`  message final : ${message ? `oui — ${mAjout ? `${mAjout[1]} nœuds, ${mAjout[2]} arêtes en ${mAjout[3]} s` : "ok"}` : "AUCUN"}`);
console.log(`  enrichissement : ${enrichi ? "rapporté" : "AUCUN RAPPORT"}${courbure ? " · courbure mesurée" : " · COURBURE ABSENTE"}${partiel ? " · PARTIEL" : ""}`);
console.log(`  monde « ${nom} » visible par le Cosmos : ${etaitPresent ? "déjà présent avant" : "absent avant"} → ${apparu ? "PRÉSENT ✓" : "ABSENT ✗"} (${nApres} mondes au total)`);
console.log(`  erreurs console : ${errors.length}${erreurVisible ? ` · ${erreurVisible}` : ""}`);
console.log(ok
  ? "\n✓ un dépôt GitHub s'ajoute depuis l'interface, enrichi, et son monde apparaît"
  : "\n✗ l'ajout de monde depuis l'interface n'aboutit pas — ou il arrive SANS son relief");

await j("/json/close/" + tab.id);
ws.close();
process.exit(ok ? 0 : 1);
