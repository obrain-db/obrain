#!/usr/bin/env node
// ── VITESSES DU 8432 — MESURE QUI DISTINGUE LE CALCUL DU CACHE ──────────────
//
// Le piège que ce script existe pour éviter : un cache chaud rend 2,7 ms là où
// le calcul en coûte 1 900. Publier un seul des deux serait mensonger. On
// mesure donc SYSTÉMATIQUEMENT deux fois la même famille de requête :
//
//   CLÉ NEUVE  · des paramètres JAMAIS demandés (jamais de cache possible) →
//                c'est le VRAI coût d'exécution : ce que paie la première
//                visite, ou une nouvelle question ;
//   CLÉ REJOUÉE· les mêmes paramètres tout de suite après → c'est le coût de
//                service, ce que paie la navigation ensuite.
//
// Les deux comptent : l'un dit si le moteur travaille trop, l'autre si la
// mémoïsation fait son office. Le rapport entre les deux dit où est le levier.
//
// Usage :
//   node scripts/vitesse-8432.mjs [base] [store] [--json /tmp/apres.json]
//   node scripts/vitesse-8432.mjs [base] [store] --compare /tmp/avant.json
//
// Le store par défaut est le chemin réel du preset « Runtime dev (engrams) ».
import { argv, exit } from "node:process";
import { writeFileSync, readFileSync } from "node:fs";

const args = argv.slice(2);
const flag = (n) => { const i = args.indexOf(n); return i < 0 ? null : args[i + 1]; };
const pos = args.filter((a, i) => !a.startsWith("--") && !(i > 0 && args[i - 1].startsWith("--")));
const BASE = pos[0] || "http://192.168.1.103:8432";
const STORE = pos[1] || "/home/triviere/.local/share/obrain-harness/data/runtime-dev.obrain";
const TOURS = Number(process.env.TOURS || 3);
const SORTIE = flag("--json");
const COMPARE = flag("--compare");

const enc = encodeURIComponent;
/** Un nonce frais : garantit que la clé de cache ne peut pas exister. */
const nonce = () => Math.random().toString(36).slice(2, 10);

async function chrono(nom, url, { method = "GET", corps = null, entetes = {} } = {}) {
  const ms = [];
  let octets = 0, code = 0, note = "";
  for (let i = 0; i < TOURS; i++) {
    const t0 = performance.now();
    try {
      const r = await fetch(url, { method, headers: entetes, body: corps });
      const t = await r.arrayBuffer();
      ms.push(performance.now() - t0);
      // ── LA TAILLE QUI COMPTE EST CELLE DU FIL ─────────────────────────────
      // `arrayBuffer().byteLength` rend la taille APRÈS décompression : `fetch`
      // dégonfle lui-même dès que `Content-Encoding` est présent. Mesurer la
      // compression avec ce chiffre donne « aucun gain » alors que le gain est
      // de 70 %. On lit donc le `Content-Length` ANNONCÉ, que le serveur pose
      // sur le corps réellement transmis (défaut corrigé le 2026-09-13, le
      // même que dans `scripts/protocole-check.mjs`).
      octets = Number(r.headers.get("content-length") ?? t.byteLength);
      code = r.status;
      if (i === 0 && !r.ok) note = `HTTP ${r.status}`;
    } catch (e) {
      ms.push(performance.now() - t0);
      if (!note) note = String(e).slice(0, 40);
    }
  }
  const tri = [...ms].sort((a, b) => a - b);
  return {
    nom,
    min: tri[0], median: tri[Math.floor(tri.length / 2)], max: tri[tri.length - 1],
    premier: ms[0], // la 1re passe est celle qui paie le calcul
    octets, code, note,
  };
}

const q = (chemin, o) =>
  BASE + chemin + "?" + new URLSearchParams({ store: STORE, ...o });

const mesures = [];
const pousse = (m) => { mesures.push(m); return m; };

// ── 1. TRANSPORT : ce qui part sur le fil, compressé ou non ─────────────────
// `identity` d'abord : c'est le coût SERVEUR, sans le bruit de la compression.
// Puis `gzip` : c'est ce que paie réellement le navigateur.
const sectionTransport = async () => {
  // Le centre n'est pas choisi au hasard : on prend le nœud de plus fort degré
  // sortant du store, donc le voisinage le PLUS GROS réellement servi. Mesurer
  // la compression sur un sous-graphe de 791 octets ne prouverait rien — c'est
  // le cas d'un nœud isolé, pas celui que le dashboard charge en plongeant.
  let qn = "dsh-runtime";
  try {
    const r = await fetch(q("/api/query", {
      language: "gql",
      query: "MATCH (a)-[r]->(b) RETURN a.qualified_name AS q, count(r) AS c ORDER BY c DESC LIMIT 1",
    }));
    const j = await r.json();
    const trouve = j?.rows?.[0]?.[0];
    if (typeof trouve === "string" && trouve) qn = trouve;
    else pousse({ nom: "centre : découverte du plus fort degré", min: 0, median: 0, max: 0,
      premier: 0, octets: 0, code: r.status, note: "repli sur dsh-runtime" });
  } catch (e) {
    pousse({ nom: "centre : découverte impossible", min: 0, median: 0, max: 0, premier: 0,
      octets: 0, code: 0, note: String(e).slice(0, 40) });
  }
  console.log(`# centre mesuré : ${qn}\n`);
  const urlSub = q("/api/viz/subgraph", { center: qn, hops: 2, limit: 349 });
  const id = await chrono("subgraph hops=1 (identity)", urlSub, { entetes: { "Accept-Encoding": "identity" } });
  const gz = await chrono("subgraph hops=1 (gzip)", urlSub, { entetes: { "Accept-Encoding": "gzip" } });
  pousse(id);
  pousse({ ...gz, gain_pct: id.octets ? (1 - gz.octets / id.octets) * 100 : 0 });
  const urlWeave = q("/api/viz/weave", {});
  const wi = await chrono("weave (identity)", urlWeave, { entetes: { "Accept-Encoding": "identity" } });
  const wg = await chrono("weave (gzip)", urlWeave, { entetes: { "Accept-Encoding": "gzip" } });
  pousse(wi);
  pousse({ ...wg, gain_pct: wi.octets ? (1 - wg.octets / wi.octets) * 100 : 0 });
  const urlModules = q("/api/viz/modules", { top: 200 });
  const mi = await chrono("modules (identity)", urlModules, { entetes: { "Accept-Encoding": "identity" } });
  const mg = await chrono("modules (gzip)", urlModules, { entetes: { "Accept-Encoding": "gzip" } });
  pousse(mi);
  pousse({ ...mg, gain_pct: mi.octets ? (1 - mg.octets / mi.octets) * 100 : 0 });
};

// ── 2. LES POSTES LOURDS, CLÉ NEUVE PUIS CLÉ REJOUÉE ────────────────────────
// Les clés sont tirées au hasard : un `top` déjà demandé rendrait un cache
// chaud, et on publierait 2,7 ms en l'appelant « le coût de arcs ».
const sectionPostes = async () => {
  {
    // ── `arcs` : LA CLÉ NEUVE N'EST PLUS UN CALCUL, ET C'EST LE CORRECTIF ───
    // Avant le 2026-09-13, le cache était indexé par `(store, top, limit)` : un
    // `top` jamais demandé repayait TOUT l'agrégat (mesuré 1 849-6 667 ms pour
    // 3 413 octets). L'agrégat est calculé à `engine_limit` fixe puis tronqué
    // côté serveur, donc `top` ne change pas la réponse — la clé ne le porte
    // plus, et un `top` neuf coûte désormais le régime servi (~3 ms).
    // La légende de la mesure change donc : « clé neuve » ne veut plus dire
    // « calcul » ici, et il faut le dire au lieu de publier 3 ms comme un gain
    // de calcul. La CORRECTION de ce cache se vérifie séparément, en comparant
    // les réponses de `top=16`, `top=48` et `top=220` : mêmes arcs, même ordre.
    const top = 200 + Math.floor(Math.random() * 200); // jamais demandé
    const url = q("/api/viz/arcs", { top, limit: 300 });
    const froid = await chrono(`arcs (top neuf, cache par store)`, url);
    const chaud = await chrono(`arcs (rejouée)`, url);
    pousse({ ...froid, cle: top });
    pousse(chaud);
  }
  {
    // ── `space` : la mesure vise l'OUVERTURE DE BASE, pas la requête ────────
    // Le correctif (2026-09-13) remplace un `open_read_only` EN DIRECT par
    // `cached_ro` : c'est l'ouverture complète d'un store de 11 Go qui
    // disparaît, et elle ne se voit QUE sur un préfixe neuf, à froid.
    const prefixe = `tmp.vitesse${nonce()}`; // préfixe inexistant : un vrai froid
    const url = q("/api/viz/space", { prefix: prefixe, rank: 2, top: 60, links: 1 });
    const froid = await chrono(`space (clé neuve)`, url);
    const chaud = await chrono(`space (rejouée)`, url);
    pousse(froid);
    pousse(chaud);
  }
  {
    const terme = `zz${nonce()}`; // aucun résultat attendu : mesure la descente
    const url = q("/api/functions", { q: terme, limit: 30 });
    const froid = await chrono(`functions (clé neuve)`, url);
    const chaud = await chrono(`functions (rejouée)`, url);
    pousse(froid);
    pousse(chaud);
  }
  {
    // ── LE NONCE DOIT ÊTRE UNIQUE PAR EXÉCUTION DU SCRIPT ───────────────────
    // Constat du 2026-09-13, et il m'a eu : un nonce d'ESPACES DE FIN fixe
    // (« LIMIT 20 » + 3 espaces) force bien une exécution réelle… la première
    // fois. Relancer le script avec le MÊME texte retrouve l'entrée de cache
    // disque de la passe précédente : l'agrégation par type d'arête est alors
    // revenue à 1,2 ms après avoir coûté 1 079 ms, et j'ai failli conclure que
    // la clé était normalisée. Le vrai coupable était le script, pas le cache.
    // Le nonce retenu ajoute une COLONNE CONSTANTE tirée à chaque exécution :
    // le texte de requête est neuf à coup sûr, et le travail du moteur est
    // inchangé (un littéral par ligne, une agrégation identique).
    const gql = `MATCH (n) RETURN count(n) AS n, '${nonce()}' AS k`;
    const url = q("/api/query", { query: gql, language: "gql" });
    const froid = await chrono("moteur : count (clé neuve)", url);
    const chaud = await chrono("moteur : count (rejouée)", url);
    pousse(froid);
    pousse(chaud);
  }
  {
    // Même protocole pour l'agrégat par type d'arête : payé à CHAQUE exécution.
    // Deux nonces distincts : deux exécutions réelles et comparables.
    const base = "MATCH ()-[r]->() RETURN type(r) AS t, count(r) AS c, ";
    const suite = " ORDER BY c DESC LIMIT 20";
    const a = await chrono("moteur : types d'arête (1)", q("/api/query", { query: base + `'${nonce()}' AS k` + suite, language: "gql" }));
    const b = await chrono("moteur : types d'arête (2)", q("/api/query", { query: base + `'${nonce()}' AS k` + suite, language: "gql" }));
    pousse(a);
    pousse(b);
  }
};

await sectionTransport();
await sectionPostes();

// ── RAPPORT ─────────────────────────────────────────────────────────────────
const precedent = COMPARE ? JSON.parse(readFileSync(COMPARE, "utf8")) : null;
const ancien = precedent
  ? Object.fromEntries(precedent.mesures.map((m) => [m.nom, m])) : null;

const f = (n) => (n == null ? "—" : n.toFixed(1));
console.log(`\nbase  : ${BASE}\nstore : ${STORE}\ntours : ${TOURS} (min/médiane/max)\n`);
console.log("  mesure                                   1er     min      med      max    octets   avant→après");
console.log("  " + "─".repeat(108));
for (const m of mesures) {
  const av = ancien?.[m.nom];
  const delta = av && av.premier ? `  ${f(av.premier)} → ${f(m.premier)} ms (1er)` : "";
  console.log(
    "  " + m.nom.padEnd(40).slice(0, 40) +
    f(m.premier).padStart(7) +
    f(m.min).padStart(8) + f(m.median).padStart(9) + f(m.max).padStart(9) +
    String(m.octets ?? "").padStart(9) + "  " + delta + (m.note ? `  [${m.note}]` : "")
  );
}
console.log(`\n  « 1er » = la passe qui paie le calcul (clé neuve) ; « min » = le régime servi par la mémoïsation.`);
console.log(`  Un « 1er » proche du « min » sur une clé NEUVE veut dire soit un calcul devenu court,`);
console.log(`  soit un nonce qui ne force plus rien — vérifier que les deux passes froides diffèrent.`);
if (SORTIE) {
  writeFileSync(SORTIE, JSON.stringify({ horodatage: new Date().toISOString(), base: BASE, store: STORE, tours: TOURS, mesures }, null, 2));
  console.log(`\n  artefact : ${SORTIE}`);
}
exit(mesures.some((m) => m.code && m.code >= 400) ? 1 : 0);
