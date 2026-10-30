// LATENCES DES REQUÊTES D'OUTILS — l'exigence : « tout doit être instantané »
//
// Ce que ce script mesure, et pourquoi : l'utilisateur a refusé de voir, dans la
// trace d'outils du chat (port 8432), des lignes comme « 128 ms », « 200 ms »,
// « 2,6 s ». « C'est une base pour l'inférence » : une lecture doit être
// instantanée, pas seulement rapide.
//
// Il interroge le VRAI serveur (`/api/query`), sur le VRAI store, et rapporte le
// temps que le MOTEUR a passé (`execution_time_ms` — pas le temps réseau).
//
// Usage :
//   node scripts/requetes-latence-check.mjs                 (store « Runtime dev (engrams) »)
//   node scripts/requetes-latence-check.mjs --store NOM --budget-ms 50
//
// Sortie : une ligne par requête (mesuré / budget), puis PASS ou FAIL.

import { ouvrirSession } from "./session-iam.mjs";

const args = process.argv.slice(2);
const lire = (nom, defaut) => {
  const i = args.indexOf(nom);
  return i >= 0 && args[i + 1] ? args[i + 1] : defaut;
};
const BASE = lire("--base", "http://192.168.1.103:8432");
const STORE = lire("--store", "Runtime dev (engrams)");
/** Budget par défaut : « instantané » au sens de l'utilisateur — 50 ms. */
const BUDGET = Number(lire("--budget-ms", "50"));
/**
 * ── POURQUOI PLUSIEURS ESSAIS, ET UNE MÉDIANE ─────────────────────────────
 * Une mesure unique ne veut rien dire sur une machine chargée : le MÊME
 * `count(n)` avec préfixe a été mesuré 104 ms puis 713 ms à une heure
 * d'intervalle, et un agrégat 60 ms puis 745 ms. C'est ce bruit qui a fait
 * garder puis jeter une micro-optimisation (voir l'historique du chantier) : on
 * croyait comparer deux code, on comparait deux moments. Avec `--repetitions N`,
 * chaque cas est mesuré N fois et c'est la MÉDIANE qui décide — le seuil porte
 * sur elle, et l'étendue est affichée pour qu'on sache à quel point le chiffre
 * est solide.
 */
const REPETITIONS = Math.max(1, Number(lire("--repetitions", "1")));

// ── POURQUOI UNE CLÉ DISTINCTE À CHAQUE MESURE ─────────────────────────────
// `/api/query` garde un CACHE DURABLE par `(store, requête)` : rejouer la même
// requête rend le `execution_time_ms` du PREMIER calcul, à la décimale près — on
// croit mesurer, on relit. (Piège rencontré : un préfixe « mesuré » 3 027,3 ms
// trois fois de suite, alors que la valeur datait d'une autre session.) Le cache
// distingue les espaces de fin, et le moteur les ignore : on ajoute donc un
// espace de plus à chaque mesure, ce qui force un calcul RÉEL sans changer la
// requête.
// Le nonce doit être UNIQUE PAR EXÉCUTION, sinon deux passes du banc relisent les
// mêmes clés et rendent les mêmes chiffres (piège vérifié : 60,8 / 60,5 / 75,0
// identiques d'une exécution à l'autre). On part de l'horloge.
let nonce = 1 + (Date.now() % 1000);
const mesurer = async (titre, query, budget = BUDGET) => {
  const u = new URL("/api/query", BASE);
  u.searchParams.set("store", STORE);
  u.searchParams.set("query", query + " ".repeat(nonce++));
  const t0 = Date.now();
  let corps;
  try {
    corps = await (
      await fetch(u, { headers: session.enTetes(), signal: AbortSignal.timeout(120_000) })
    ).json();
  } catch (e) {
    return { titre, ok: false, ms: null, lignes: null, note: `erreur : ${e.message}` };
  }
  const ms = corps.execution_time_ms ?? null;
  const lignes = Array.isArray(corps.rows) ? corps.rows.length : null;
  const ok = typeof ms === "number" && ms <= budget;
  return { titre, ok, ms, lignes, note: ok ? "" : `> budget ${budget} ms`, aller: Date.now() - t0 };
};

// ── L'IDENTITÉ : le service est ARMÉ, donc une session est exigée ───────────
const session = await ouvrirSession(BASE, {
  utilisateur: lire("--user", null),
  motDePasse: lire("--password", null),
  identifiants: lire("--identifiants", undefined),
});
console.log(`  …    ${session.note}`);

// ── Les requêtes du RÉEL : celles que le modèle a écrites dans le chat, plus les
// sondes qui séparent les chemins (index d'égalité, index de label, balayage).
const CAS = [
  ["égalité sur qualified_name (index)", "MATCH (n:File) WHERE n.qualified_name = 'file:/home/triviere/projects/obrain/projets/graph-storage/00-SYNTHESE.md' RETURN n LIMIT 1"],
  ["compte par label (compteur O(1))", "MATCH (n:Class) RETURN count(n)"],
  ["scan de label", "MATCH (n:Class) RETURN n.qualified_name LIMIT 30"],
  ["préfixe DANS un label", "MATCH (n:Class) WHERE n.qualified_name STARTS WITH 'home.triviere.projects.banc_obrain' RETURN n.qualified_name LIMIT 30"],
  ["préfixe sur le graphe entier", "MATCH (n) WHERE n.qualified_name STARTS WITH 'home.triviere.projects.banc_obrain.crates.' RETURN n.qualified_name, labels(n) LIMIT 30"],
  ["préfixe + label en prédicat (défaut corrigé)", "MATCH (n) WHERE n.qualified_name STARTS WITH 'home.triviere.projects.banc_obrain' AND labels(n) CONTAINS 'Class' RETURN n.qualified_name LIMIT 40"],
  ["sous-chaîne sur le graphe entier", "MATCH (n) WHERE n.qualified_name CONTAINS 'obrain' RETURN n.qualified_name, labels(n) LIMIT 40"],
  ["agrégation split + count", "MATCH (n) WHERE n.qualified_name STARTS WITH 'home.triviere.projects.banc_obrain.crates.' RETURN split(n.qualified_name, '.')[3] AS crate, count(*) AS n LIMIT 30"],
];

console.log(`── latences — ${BASE} · store « ${STORE} » · budget ${BUDGET} ms · ${REPETITIONS} essai(s) par cas`);
const resultats = [];
const mediane = (v) => {
  const s = [...v].sort((a, b) => a - b);
  const m = Math.floor(s.length / 2);
  return s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2;
};
for (const [titre, query] of CAS) {
  const essais = [];
  let dernier = null;
  for (let i = 0; i < REPETITIONS; i++) {
    dernier = await mesurer(titre, query);
    if (typeof dernier.ms === "number") essais.push(dernier.ms);
  }
  const med = essais.length ? mediane(essais) : null;
  const r = { ...dernier, ms: med, essais };
  r.ok = typeof med === "number" && med <= BUDGET;
  resultats.push(r);
  const ms = med === null ? "  —  " : `${med.toFixed(1)} ms`.padStart(9);
  const etendue = essais.length > 1
    ? ` [${Math.min(...essais).toFixed(0)}–${Math.max(...essais).toFixed(0)}]`
    : "";
  console.log(`  ${r.ok ? "ok  " : "FAIL"} ${ms}${etendue.padEnd(14)} ${r.lignes === null ? "" : `${r.lignes} lig.`.padEnd(8)} ${titre}`);
  // La requête de label doit aussi rendre des LIGNES : un faux négatif coûte plus
  // cher qu'une lenteur (il se lit comme une réponse).
  if (titre.includes("défaut corrigé") && r.lignes === 0) {
    console.log("        ATTENTION : 0 ligne — `labels(n) CONTAINS 'Class'` doit rendre des nœuds (sémantique corrigée)");
  }
}

const echecs = resultats.filter((r) => !r.ok);
if (echecs.length) {
  console.log("── les plus lentes :");
  for (const r of [...resultats].filter((x) => x.ms !== null).sort((a, b) => b.ms - a.ms).slice(0, 5)) {
    console.log(`        ${(r.ms).toFixed(1).padStart(9)} ms  ${r.titre}`);
  }
}
console.log(`── ${echecs.length === 0 ? "PASS" : `FAIL (${echecs.length}/${resultats.length})`} — latences d'outils` +
  (REPETITIONS > 1 ? " (médianes)" : " — relancez avec --repetitions 5 pour une médiane"));
process.exit(echecs.length === 0 ? 0 : 1);
