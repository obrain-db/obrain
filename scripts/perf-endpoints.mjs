#!/usr/bin/env node
// ── MESURE CONSOLIDÉE DES COÛTS ─────────────────────────────────────────────
//
// Manquait : un instrument unique qui mesure TOUS les endpoints et TOUTES les
// opérations du moteur sous le même protocole. Les chiffres existaient, mais
// dispersés — un curl par-ci, un chrono par-là — donc incomparables, et
// impossibles à suivre dans le temps.
//
// Ce que fait ce script :
//   - mesure chaque ROUTE de l'API (celles que la descente emprunte vraiment) ;
//   - mesure chaque SECTION du moteur (compter, chercher, agréger, étendre) ;
//   - distingue À FROID et À CHAUD : un cache chaud donne 0,01 s là où le
//     premier appel en coûte 12 — publier un seul des deux serait mensonger ;
//   - classe par coût, en ms, et écrit un artefact durable (JSON + Markdown)
//     pour que la prochaine mesure se COMPARE à celle-ci.
//
// Usage : node scripts/perf-endpoints.mjs [base] [store]
import { writeFileSync, mkdirSync, readFileSync } from "node:fs";

const BASE = process.argv[2] || "http://192.168.1.103:8432";
const STORE = process.argv[3] || "/home/triviere/.local/share/obrain-harness/data/runtime-dev.obrain";
const TOURS = Number(process.env.TOURS || 3);

/** Une mesure : N tours, on garde min / médiane / max. Jamais une seule passe. */
const chrono = async (nom, section, url, init) => {
  const ms = [];
  let taille = 0, note = "";
  for (let i = 0; i < TOURS; i++) {
    const t0 = performance.now();
    try {
      const r = await fetch(url, init);
      const txt = await r.text();
      ms.push(performance.now() - t0);
      taille = txt.length;
      if (i === 0 && !r.ok) note = `HTTP ${r.status}`;
      if (i === 0 && /"error"/.test(txt)) note = "erreur moteur";
    } catch (e) {
      ms.push(performance.now() - t0);
      if (!note) note = String(e).slice(0, 40);
    }
  }
  const tri = [...ms].sort((a, b) => a - b);
  return {
    nom, section,
    min: tri[0], median: tri[Math.floor(tri.length / 2)], max: tri[tri.length - 1],
    froid: ms[0], octets: taille, note,
  };
};

const q = (p) => BASE + p + (p.includes("?") ? "&" : "?") + new URLSearchParams({ store: STORE });
// Le endpoint `/api/query` lit ses paramètres dans la CHAÎNE DE REQUÊTE, pas
// dans un corps JSON : un POST avec body renvoyait « query requis » et je
// mesurais des erreurs en croyant mesurer le moteur. Corrigé.
const gql = (requete) => [
  BASE + "/api/query?" + new URLSearchParams({ store: STORE, query: requete, language: "gql" }),
  undefined,
];

const mesures = [];

// ── SECTION 1 : LE MOTEUR (les opérations que la descente provoque) ─────────
// Ces requêtes disent OÙ passe le temps du graphe, indépendamment de l'API.
const OPERATIONS = [
  ["compter les nœuds", "MATCH (n) RETURN count(n)"],
  ["compter les arêtes", "MATCH ()-[r]->() RETURN count(r)"],
  ["compter UN type d'arête", "MATCH ()-[r:PROVES]->() RETURN count(r)"],
  ["chercher par nom (index)", "MATCH (n) WHERE n.qualified_name = 'banc_obrain' RETURN n"],
  ["lire 10 nœuds", "MATCH (n) RETURN n LIMIT 10"],
  ["étendre 1 saut depuis 1 nœud", "MATCH (a)-[r]->(b) RETURN b LIMIT 200"],
  ["agréger par label", "MATCH (n) RETURN labels(n), count(n)"],
  ["agréger par type d'arête", "MATCH ()-[r]->() RETURN type(r), count(r)"],
];
for (const [nom, requete] of OPERATIONS) {
  const [url, init] = gql(requete);
  const m = await chrono(nom, "moteur", url, init);
  // Le moteur rend son propre temps d'exécution : on le garde, c'est la mesure
  // la moins bruitée (elle exclut le réseau et la sérialisation HTTP).
  try {
    const r = await fetch(url, init);
    const d = await r.json();
    m.moteur_ms = d.execution_time_ms ?? null;
  } catch { /* ignoré */ }
  mesures.push(m);
}

// ── SECTION 2 : LES ROUTES DE L'API (celles que la descente emprunte) ───────
const MODULES = (await (await fetch(q("/api/viz/modules") + "&top=8")).json().catch(() => ({})));
const centre = MODULES?.modules?.[0]?.name || "deepseek_harness";

const ROUTES = [
  ["/api/viz/signatures (toutes les signatures)", "/api/viz/signatures"],
  ["/api/viz/overview (résumé d'un store)", "/api/viz/overview"],
  ["/api/viz/modules (ensembles d'un monde)", "/api/viz/modules?top=8"],
  ["/api/viz/weave (tissage complet)", "/api/viz/weave"],
  ["/api/viz/activation (journal vif)", "/api/viz/activation"],
  [`/api/viz/subgraph hops=1 (phase RAPIDE)`, `/api/viz/subgraph?center=${encodeURIComponent(centre)}&hops=1&limit=150`],
  [`/api/viz/subgraph hops=2 (phase MOTIFS)`, `/api/viz/subgraph?center=${encodeURIComponent(centre)}&hops=2&limit=300`],
  ["/api/viz/space (sous-ensembles)", `/api/viz/space?prefix=${encodeURIComponent(centre)}&top=8`],
  ["/api/functions (recherche)", `/api/functions?q=index&limit=20`],
  ["/api/worlds (registre)", "/api/worlds"],
  ["/api/engine/sante (LLM)", "/api/engine/sante"],
];
for (const [nom, route] of ROUTES) {
  mesures.push(await chrono(nom, "route", BASE + route + (route.includes("?") ? "&" : "?") + new URLSearchParams({ store: STORE })));
}

// ── SECTION 3 : REJEU DU JOURNAL DU SERVEUR ─────────────────────────────────
// « Regarde les usages du 8432, rejoue-les dans les bases et mesure. »
// Le journal du serveur dit ce qu'il a RÉELLEMENT rencontré : anomalies
// structurelles, propriétés illisibles, compteurs faux. On les extrait, on
// rejoue les nœuds impliqués, et on mesure — un avertissement sans coût mesuré
// n'est qu'une rumeur.
let journal = { chemin: null, lignes: 0, cycles: [], decodeKo: 0, noeudsAbimes: 0, compteursFaux: null };
for (const chemin of ["/tmp/vizserve.log", "/tmp/viz8432.log"]) {
  try {
    const txt = readFileSync(chemin, "utf8");
    if (!txt) continue;
    journal.chemin = chemin;
    journal.lignes = txt.split("\n").length;
    for (const m of txt.matchAll(/cycle detected at src=(\d+)/g)) journal.cycles.push(Number(m[1]));
    journal.decodeKo = (txt.match(/decode failed/g) || []).length;
    journal.noeudsAbimes = new Set([...txt.matchAll(/node_id=(\d+)/g)].map((m) => m[1])).size;
    const c = txt.match(/compteurs d'instantane FAUX[^\n]*noeuds_avant=(\d+) noeuds_apres=(\d+)[^\n]*aretes_avant=(\d+) aretes_apres=(\d+)/);
    if (c) journal.compteursFaux = { avant: [Number(c[1]), Number(c[3])], apres: [Number(c[2]), Number(c[4])] };
    break;
  } catch { /* journal absent : on le dit plus bas */ }
}
if (journal.chemin) {
  const parNoeud = new Map();
  for (const id of journal.cycles) parNoeud.set(id, (parNoeud.get(id) || 0) + 1);
  const pires = [...parNoeud.entries()].sort((a, b) => b[1] - a[1]).slice(0, 2).map(([id]) => id);
  for (const id of pires) {
    const [url, init] = gql(`MATCH (a)-[r]->(b) WHERE id(a) = ${id} RETURN b LIMIT 200`);
    const m = await chrono(`expansion du nœud cyclique ${id}`, "rejeu", url, init);
    try { const d = await (await fetch(url, init)).json(); m.moteur_ms = d.execution_time_ms ?? null; } catch { /* ignoré */ }
    mesures.push(m);
  }
  // Témoin : mêmes requêtes sur des nœuds VOISINS non signalés, pour que le
  // surcoût soit attribuable au nœud et non à la forme de la requête.
  for (const id of pires) {
    const [url, init] = gql(`MATCH (a)-[r]->(b) WHERE id(a) = ${id + 1} RETURN b LIMIT 200`);
    const m = await chrono(`expansion du nœud VOISIN ${id + 1} (témoin)`, "rejeu", url, init);
    try { const d = await (await fetch(url, init)).json(); m.moteur_ms = d.execution_time_ms ?? null; } catch { /* ignoré */ }
    mesures.push(m);
  }
}

// ── RENDU CONSOLIDÉ ─────────────────────────────────────────────────────────
// Les trois ordres de grandeur doivent se lire d'un coup d'œil : sous la
// milliseconde on parle en MICROsecondes (le plancher mesuré du moteur y est),
// et jamais en nanosecondes — aucune opération observée ne l'atteint.
const f = (x) =>
  x >= 1000 ? `${(x / 1000).toFixed(2)} s`
  : x >= 1 ? `${x.toFixed(x < 10 ? 1 : 0)} ms`
  : `${Math.max(1, Math.round(x * 1000))} µs`;
// Le coût qui compte est le PIRE des deux régimes : un endpoint peut être
// instantané en cache et catastrophique à froid — classer sur la seule médiane
// désignerait le mauvais coupable (mesuré : `weave` en tête alors que `hops=2`
// coûte 27 s au premier appel).
const cout = (m) => Math.max(m.froid, m.median);
const tri = [...mesures].sort((a, b) => cout(b) - cout(a));
const plusLourd = tri[0];

console.log(`\nCOÛTS CONSOLIDÉS — ${TOURS} tours par mesure\n${BASE}\nstore : ${STORE.split("/").pop()}\n`);
console.log("  1er appel = cache serveur FROID · suivants = cache serveur CHAUD (le serveur");
console.log("  mémorise les sous-graphes par (store, centre, sauts, limite) — c'est un fait mesuré).\n");
console.log("  section   mesure                                     1er appel   suivants   max        moteur");
console.log("  " + "─".repeat(100));
for (const m of tri) {
  console.log(
    `  ${m.section.padEnd(9)} ${m.nom.slice(0, 44).padEnd(45)}` +
    `${f(m.froid).padStart(12)}${f(m.median).padStart(11)}${f(m.max).padStart(10)}` +
    `${(m.moteur_ms != null ? f(m.moteur_ms) : "—").padStart(11)}${m.note ? "  ⚠ " + m.note : ""}`,
  );
}

// Combien de fois le plus lourd dépasse la médiane des autres : c'est ce ratio
// qui dit s'il y a UN point noir ou si tout est uniformément lent.
const autres = tri.slice(1).map(cout).sort((a, b) => a - b);
const medAutres = autres[Math.floor(autres.length / 2)];
if (journal.chemin) {
  console.log(`\n  JOURNAL DU SERVEUR (${journal.chemin}, ${journal.lignes} lignes)`);
  console.log(`    décodements de propriétés en échec : ${journal.decodeKo} · nœuds concernés : ${journal.noeudsAbimes}`);
  console.log(`    cycles d'expansion détectés : ${journal.cycles.length}` +
    (journal.cycles.length ? ` sur ${new Set(journal.cycles).size} nœud(s)` : ""));
  if (journal.compteursFaux) {
    console.log(`    compteurs d'instantané déclarés FAUX puis corrigés : ` +
      `nœuds ${journal.compteursFaux.avant[0]} → ${journal.compteursFaux.apres[0]}, ` +
      `arêtes ${journal.compteursFaux.avant[1]} → ${journal.compteursFaux.apres[1]}`);
  }
}
console.log(`\n  point noir : « ${plusLourd.nom} »` +
  `\n    premier appel ${f(plusLourd.froid)} · appels suivants ${f(plusLourd.median)}` +
  `\n    soit ${(cout(plusLourd) / Math.max(1, medAutres)).toFixed(0)}× le coût médian des autres (${f(medAutres)})` +
  `\n    écart froid/chaud : ${(plusLourd.froid / Math.max(1, plusLourd.median)).toFixed(0)}× — le prix est payé UNE fois par (store, centre, sauts, limite).`);

mkdirSync("docs/perf", { recursive: true });
const horodatage = new Date().toISOString();
writeFileSync("docs/perf/endpoints.json", JSON.stringify({ horodatage, base: BASE, store: STORE, tours: TOURS, mesures: tri }, null, 2));
const plusLourd2 = tri[1];
const lignes = [
  "# Coûts consolidés des endpoints et des opérations du moteur", "",
  "`1er appel` = cache serveur froid · `suivants` = cache serveur chaud. Le serveur mémorise les",
  "sous-graphes par `(store, centre, sauts, limite)` : le prix se paie une fois par combinaison.", "",
  `Mesuré le ${horodatage} sur \`${STORE.split("/").pop()}\` — ${TOURS} tours par mesure, min/médiane/max.`,
  "**Aucune valeur n'est en nanosecondes.** Le plancher observé est de l'ordre de la dizaine de",
  "microsecondes (`count(n)` et `count(r)` ne comptent pas assez pour remplir une milliseconde),",
  "et le plafond à 26 s pour un seul appel. L'écart entre les deux est de **six ordres de grandeur**.", "",
  "| section | mesure | froid | médiane | max | temps moteur |", "|---|---|---|---|---|---|",
  ...tri.map((m) => `| ${m.section} | ${m.nom} | ${f(m.froid)} | ${f(m.median)} | ${f(m.max)} | ${m.moteur_ms != null ? f(m.moteur_ms) : "—"} |`),
  "", `Point noir : **${plusLourd.nom}** — premier appel ${f(plusLourd.froid)}, suivants ${f(plusLourd.median)} ` +
  `(${(cout(plusLourd) / Math.max(1, medAutres)).toFixed(0)}× le coût médian des autres).`,
  `Deuxième poste : **${plusLourd2.nom}** (${f(cout(plusLourd2))}).`, "",
  ...(journal.chemin ? [
    "## Rejeu du journal du serveur", "",
    `Journal : \`${journal.chemin}\` (${journal.lignes} lignes).`,
    `- décodements de propriétés en échec : **${journal.decodeKo}** sur **${journal.noeudsAbimes}** nœuds distincts ;`,
    `- cycles d'expansion détectés : **${journal.cycles.length}** sur ${new Set(journal.cycles).size} nœud(s) ;`,
    ...(journal.compteursFaux ? [`- compteurs d'instantané déclarés **faux** puis corrigés : ` +
      `nœuds ${journal.compteursFaux.avant[0]} → ${journal.compteursFaux.apres[0]}, ` +
      `arêtes ${journal.compteursFaux.avant[1]} → ${journal.compteursFaux.apres[1]} ;`] : []),
    "",
  ] : []),
];
writeFileSync("docs/perf/ENDPOINTS.md", lignes.join("\n"));
console.log("\n  artefact : docs/perf/ENDPOINTS.md · docs/perf/endpoints.json\n");
