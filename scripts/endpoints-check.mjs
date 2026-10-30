#!/usr/bin/env node
// ── BALAYAGE COMPLET DES ENDPOINTS ──────────────────────────────────────────
//
// « Vérifie que tout est bon sur tous les endpoints. » Les instruments existants
// en couvraient une partie : `smoke-backend.sh` (agrégats chauds < 50 ms) et
// `perf-endpoints.mjs` (coûts de 10 routes). Dix routes n'étaient mesurées par
// aucun des deux — `/api/viz/subgraph`, `/api/viz/status`, `/api/info`,
// `/api/stores`, `/api/engine/ask`, `/api/engine/chat`, `/api/engine/memoire`,
// `/api/worlds/status`, `/__dev/version` et les statiques.
//
// Ce script les prend TOUTES, avec des paramètres VALIDES (un endpoint qui
// répond 400 faute de paramètre ne prouve rien), et rend un verdict par route :
//
//   OK       · 200, charge utile non vide, pas de champ `error`
//   VIDE     · 200 mais aucune donnée là où il en faut
//   ERREUR   · code != 200, ou `error` dans le corps
//   ABSENT   · 404 — la route n'existe pas dans CE binaire
//   LENT     · au-delà du seuil (le seuil est par route : certains sont
//              intrinsèquement lourds, d'autres doivent répondre en quelques ms)
//
// À froid PUIS à chaud : un cache chaud donne 0,01 s là où le premier appel en
// coûte 12. Publier un seul des deux serait mensonger.
//
// Usage : node scripts/endpoints-check.mjs [base] [store]
import { argv, exit } from "node:process";

const BASE = argv[2] || "http://192.168.1.103:8432";
const STORE = argv[3] || "/home/triviere/.local/share/obrain-harness/data/runtime-dev.obrain";
const QN = "dsh-runtime";

// ── L'IDENTITÉ, QUAND ELLE EST FOURNIE PAR L'ENVIRONNEMENT ──────────────────
//
// Deux routes — `/api/worlds` et `/api/worlds/status` — sont, par conception,
// jugées sur les espaces DE L'APPELANT et non sur la base visitée
// (`viz_serve.rs`, commit `45dd9212` : « les mondes sont une affaire de compte,
// pas de base visitée »). Sans identité, elles répondent 401 même quand l'IAM
// n'est pas armé, et c'est leur comportement CORRECT.
//
// Ce contrôle ne présentait rien : il comptait donc ces deux 401 comme des
// pannes, et la porte d'acceptation échouait pour une raison qui n'en est pas
// une. Même convention que `echelle-multi-niveaux-check.mjs` : si l'environnement
// fournit une identité, on la présente à TOUTES les requêtes ; sinon on le DIT,
// et les routes qui en exigent une sont comptées à part — jamais en silence.
const JETON = (process.env.OBRAIN_TOKEN || "").trim();
const COOKIE = (process.env.OBRAIN_COOKIE || "").trim();
const ENTETES_IDENTITE = JETON ? { authorization: `Bearer ${JETON}` } : COOKIE ? { cookie: COOKIE } : null;

/** Une requête chronométrée, avec un délai explicite. */
async function coup(url, { ms = 120_000, sse = false } = {}) {
  const t0 = performance.now();
  const ctl = new AbortController();
  const minuteur = setTimeout(() => ctl.abort(), ms);
  try {
    const r = await fetch(url, { signal: ctl.signal, headers: ENTETES_IDENTITE || undefined });
    let corps = "";
    if (sse) {
      // On lit le début du flux et on s'arrête au premier événement complet :
      // attendre la fin d'une conversation coûterait des minutes.
      const lecteur = r.body.getReader();
      const dec = new TextDecoder();
      while (corps.length < 4000) {
        const { value, done } = await lecteur.read();
        if (done) break;
        corps += dec.decode(value, { stream: true });
        if (/event: \w+\ndata: /.test(corps)) break;
      }
      try { await lecteur.cancel(); } catch { /* le flux se ferme seul */ }
    } else {
      corps = await r.text();
    }
    return { statut: r.status, corps, ms: performance.now() - t0, erreur: null };
  } catch (e) {
    return { statut: 0, corps: "", ms: performance.now() - t0, erreur: String(e).slice(0, 60) };
  } finally {
    clearTimeout(minuteur);
  }
}

/** Verdict d'une mesure : ce qui est MESURÉ, pas ce qui est supposé. */
function verdict(res, { seuilMs, videSi = null, sse = false, erreurAttendue = null, exigeIdentite = false }) {
  if (res.erreur) return { etat: "ERREUR", note: res.erreur };
  if (res.statut === 404) return { etat: "ABSENT", note: "404" };
  if (res.statut === 0) return { etat: "ERREUR", note: "aucune réponse" };
  // ── CE QU'ON NE PEUT PAS MESURER SE DIT, IL NE SE TAIT PAS ──────────────
  // Une route qui exige une identité, interrogée sans identité, ne dit RIEN de
  // son état : son 401 est le comportement attendu. La compter en panne était un
  // faux positif ; la compter en succès serait un faux négatif — elle devient
  // donc un SKIP explicite, compté à part dans le verdict.
  if (res.statut === 401 && exigeIdentite && !ENTETES_IDENTITE) {
    return { etat: "SKIP", note: "exige une identité, aucune n'est présentée" };
  }
  if (res.statut !== 200) return { etat: "ERREUR", note: `HTTP ${res.statut}` };
  if (sse) {
    if (!/^event: \w+/m.test(res.corps)) return { etat: "VIDE", note: "aucun événement SSE" };
    if (/(^|\n)data: .*"error"/m.test(res.corps)) {
      const m = res.corps.match(/"error":"([^"]{0,70})/);
      return { etat: "ERREUR", note: m ? m[1] : "erreur dans le flux" };
    }
    return { etat: "OK", note: (res.corps.match(/^event: (\w+)/m) || [])[1] || "flux" };
  }
  // ── L'HEURISTIQUE « error » NE VAUT QUE POUR DU JSON ────────────────────
  // Constaté : un bundle JS contient forcément la chaîne `"error"` (gestion
  // d'erreurs, clés de messages) — la vérifier sur un actif statique produisait
  // un faux positif sur les DEUX serveurs à la fois, ce qui aurait envoyé
  // chercher une panne inexistante.
  const estJson = /^\s*[\[{]/.test(res.corps);
  if (estJson && /"error"/.test(res.corps)) {
    const m = res.corps.match(/"error"\s*:\s*"([^"]{0,70})/);
    const msg = m ? m[1] : "champ error";
    // ── TOUTE ERREUR N'EST PAS UN DÉFAUT ────────────────────────────────────
    // Sur une route dont le métier est de VALIDER un paramètre, répondre « cette
    // valeur est inconnue » est le comportement CORRECT. La compter comme une
    // panne était un faux positif de ce script (constaté sur
    // `/api/worlds/status` interrogé avec un identifiant bidon).
    if (erreurAttendue && erreurAttendue.test(msg)) {
      return { etat: "OK", note: "paramètre invalide refusé comme attendu" };
    }
    return { etat: "ERREUR", note: msg };
  }
  if (videSi && videSi(res.corps)) return { etat: "VIDE", note: "" };
  if (res.ms > seuilMs) return { etat: "LENT", note: `> ${seuilMs} ms` };
  return { etat: "OK", note: "" };
}

const u = (chemin, params = {}) =>
  BASE + chemin + "?" + new URLSearchParams({ store: STORE, ...params });

// ── Les routes : TOUTES celles que le serveur déclare ───────────────────────
const ROUTES = [
  { nom: "racine (app)", url: () => BASE + "/", seuil: 3000 },
  { nom: "app/ (bundle)", url: () => BASE + "/app/", seuil: 3000, videSi: (c) => !/<div id="root"/.test(c) },

  { nom: "sitemap.xml", url: () => BASE + "/sitemap.xml", seuil: 3000, videSi: (c) => !c.includes("<urlset") },
  { nom: "__dev/version", url: () => BASE + "/__dev/version", seuil: 3000 },
  { nom: "api/stores", url: () => BASE + "/api/stores", seuil: 1000, videSi: (c) => !/"presets"/.test(c) },
  { nom: "api/info", url: () => u("/api/info"), seuil: 3000 },
  { nom: "api/viz/status", url: () => BASE + "/api/viz/status", seuil: 3000 },
  { nom: "api/viz/signatures", url: () => BASE + "/api/viz/signatures", seuil: 30_000 },
  { nom: "api/viz/overview", url: () => u("/api/viz/overview"), seuil: 5000 },
  { nom: "api/viz/modules", url: () => u("/api/viz/modules", { top: 200 }), seuil: 25_000 },
  { nom: "api/viz/arcs", url: () => u("/api/viz/arcs", { top: 48, limit: 320 }), seuil: 25_000 },
  { nom: "api/viz/space", url: () => u("/api/viz/space", { prefix: "deepseek_harness", rank: 4, top: 60, links: 0 }), seuil: 25_000 },
  { nom: "api/viz/subgraph", url: () => u("/api/viz/subgraph", { center: QN, hops: 1, limit: 120 }), seuil: 30_000, videSi: (c) => !/"nodes"/.test(c) },
  { nom: "api/viz/weave", url: () => BASE + "/api/viz/weave", seuil: 30_000, videSi: (c) => !/"worlds"/.test(c) },
  { nom: "api/viz/activation", url: () => BASE + "/api/viz/activation", seuil: 3000 },
  { nom: "api/functions", url: () => u("/api/functions", { q: "main", limit: 10 }), seuil: 10_000 },
  { nom: "api/query", url: () => u("/api/query", { query: "MATCH (n) RETURN labels(n)[0] AS t, count(n) AS w ORDER BY w DESC LIMIT 5", language: "gql" }), seuil: 10_000, videSi: (c) => !/"rows"/.test(c) },
  { nom: "api/worlds", url: () => BASE + "/api/worlds", seuil: 20_000, videSi: (c) => !/"mondes"/.test(c), exigeIdentite: true },
  { nom: "api/worlds/status", url: () => BASE + "/api/worlds/status?job=inexistant", seuil: 3000, erreurAttendue: /travail inconnu/, exigeIdentite: true },
  { nom: "viewer2d", url: () => BASE + "/viewer2d", seuil: 5000, videSi: (c) => c.length < 200 },
  { nom: "api/engine/sante", url: () => BASE + "/api/engine/sante", seuil: 10_000 },
  { nom: "api/engine/memoire", url: () => BASE + "/api/engine/memoire", seuil: 5000 },
  // LES RÉPARATIONS DUES : le troisième temps du protocole d'entretien doit être
  // balayé comme les autres — un endpoint hors inventaire est un angle mort.
  { nom: "api/entretien", url: () => BASE + "/api/entretien", seuil: 5000, videSi: (c) => !/"en_attente"/.test(c) },
  { nom: "api/engine/ask", url: () => u("/api/engine/ask", { qn: QN, q: "une phrase", max_tokens: 24 }), seuil: 300_000 },
  { nom: "api/engine/chat (SSE)", url: () => u("/api/engine/chat", { qn: "world:runtime-dev-engrams", q: "test", max_tokens: 32 }), seuil: 300_000, sse: true },
];

// ── LES ACTIFS RÉELLEMENT RÉFÉRENCÉS PAR LA PAGE ────────────────────────────
// Vérifier « la page répond » ne dit pas que le BUNDLE se sert : une page qui
// pointe un fichier absent donne une application blanche et un 200. On lit donc
// les URL dans le HTML, et on les demande.
try {
  const html = await (await fetch(BASE + "/")).text();
  for (const m of html.matchAll(/["']([^"']*\.(?:js|css))["']/g)) {
    const chemin = m[1].startsWith("/") ? m[1] : "/app/" + m[1].replace(/^\.\//, "");
    if (chemin.includes("assets/")) {
      ROUTES.push({
        nom: `actif ${chemin.split(".").pop()}`,
        url: () => BASE + chemin,
        seuil: 8000,
        statique: true,
        videSi: (c) => c.length < 500,
      });
    }
  }
} catch { /* la page est déjà contrôlée plus bas */ }

// `RAPIDE=1` saute les deux routes qui appellent le modèle (une à deux minutes
// chacune, deux fois) : utile pour vérifier le reste pendant qu'on itère.
const RAPIDE = process.env.RAPIDE === "1";
const routes = RAPIDE
  ? ROUTES.filter((r) => !/engine\/(ask|chat)/.test(r.nom))
  : ROUTES;

console.log(`\nBALAYAGE DES ENDPOINTS — ${routes.length} routes${RAPIDE ? " (mode rapide, hors modèle)" : ""}\n${BASE}\nstore : ${STORE.split("/").pop()}\n`);
if (ENTETES_IDENTITE) {
  console.log(`identité présentée : ${JETON ? "OBRAIN_TOKEN" : "OBRAIN_COOKIE"}\n`);
} else {
  console.log("identité présentée : AUCUNE — les routes qui en exigent une seront SKIP, pas jugées.\n");
}

// Précondition : le moteur est-il là ? Sans lui, deux routes ne peuvent pas
// répondre, et l'annoncer évite d'accuser le serveur.
let moteur = "(inconnu)";
try {
  const r = await fetch(BASE + "/api/engine/sante");
  const j = await r.json();
  moteur = j.disponible ? `disponible (${j.modele || "?"})` : `INDISPONIBLE — ${j.raison || ""}`;
} catch { moteur = "sante injoignable"; }
console.log(`moteur LLM : ${moteur}\n`);

const lignes = [];
let souci = 0;
let sautes = 0;
for (const route of routes) {
  const a = await coup(route.url(), { sse: route.sse, ms: Math.min(route.seuil * 2, 320_000) });
  const b = await coup(route.url(), { sse: route.sse, ms: Math.min(route.seuil * 2, 320_000) });
  const v = verdict(b, {
    seuilMs: route.seuil,
    videSi: route.videSi,
    sse: route.sse,
    erreurAttendue: route.erreurAttendue,
    exigeIdentite: route.exigeIdentite,
  });
  if (v.etat === "SKIP") sautes++;
  else if (v.etat !== "OK") souci++;
  const froid = a.erreur ? "—" : `${Math.round(a.ms)} ms`;
  const chaud = b.erreur ? "—" : `${Math.round(b.ms)} ms`;
  lignes.push({ route: route.nom, etat: v.etat, note: v.note, froid, chaud });
  const marque = v.etat === "OK" ? "ok " : v.etat === "SKIP" ? "-- " : "!! ";
  console.log(
    `${marque}${route.nom.padEnd(26)} ${v.etat.padEnd(7)} froid ${froid.padStart(8)} · chaud ${chaud.padStart(8)}${v.note ? "  · " + v.note : ""}`,
  );
}

const mesures = routes.length - sautes;
console.log(
  `\n${mesures - souci}/${mesures} endpoint(s) mesurés OK${souci ? ` — ${souci} à regarder` : ""}` +
    (sautes ? ` · ${sautes} NON mesuré(s) (identité requise, aucune présentée)` : ""),
);
exit(souci ? 1 : 0);
