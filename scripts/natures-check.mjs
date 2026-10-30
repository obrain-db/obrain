#!/usr/bin/env node
/**
 * ── LA GARDE DES NATURES : AUCUN TYPE NE S'AFFICHE SANS ÊTRE DÉCLARÉ ────────
 *
 * Ce qu'elle affirme, et que rien d'autre dans le dépôt n'affirme :
 *
 *  1. TOUT TYPE RÉELLEMENT PRÉSENT dans les bases servies est déclaré dans
 *     `src/graph/natures.ts`. C'est la garantie qui manquait : un type publié
 *     demain par le serveur (un `Agent`, un `Trace`, un `Embedding`…) ne peut
 *     pas se glisser à l'écran en se faisant passer pour un type connu.
 *     Mesuré le 2026-09-14 : **27 types** sur les 7 bases servies.
 *     ⚠ On lit `/api/viz/overview` base par base, JAMAIS `/api/viz/signatures` :
 *     ce dernier est TRONQUÉ à 10 types par base (constaté : `Runtime dev` y
 *     montre 10 types sur les 14 qu'il a), et une garde qui s'y fierait
 *     laisserait passer 4 types par base en silence.
 *
 *  2. DEUX TYPES NE PARTAGENT JAMAIS UNE COULEUR, et deux couleurs restent à
 *     une distance perceptible. La palette d'origine échoue à ce contrôle :
 *     `#ffd166` et `#ffe066` sont à **15** l'une de l'autre — deux jaunes
 *     indistinguables pour dix types. Le seuil retenu ici est la distance
 *     minimale du jeu déclaré, mesurée à **33,8** ; la garde refuse donc toute
 *     nouvelle couleur qui rapprocherait deux types en dessous de **33**.
 *
 *  3. LA TABLE NE CONTREDIT PAS LES VUES. Deux listes de types vivent encore
 *     en dur (`COG` en `StageContinu.tsx`, le filtre `Function`/`Method` en
 *     `StoreOverview.tsx`). La table est une CONSOLIDATION, pas un second avis :
 *     si elle rangeait dans « code » ce que la vue appelle « cognition »,
 *     brancher la table changerait le comportement en silence. La garde compare
 *     donc les deux, et échoue à la moindre divergence.
 *
 *  4. LA DETTE EST COMPTÉE, PAS CACHÉE. Les listes de types encore écrites en
 *     dur dans les vues sont listées avec leur `fichier:ligne`. Avertissement
 *     par défaut (la dette peut exister le temps d'un chantier) ; `--strict` en
 *     fait un échec, pour un dépôt qui ne veut plus en accepter de nouvelles.
 *
 * Usage :
 *   node scripts/natures-check.mjs [base]         # http://192.168.1.103:8432
 *   node scripts/natures-check.mjs --strict
 */
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative } from "node:path";

const args = process.argv.slice(2);
const STRICT = args.includes("--strict");
const BASE = (args.find((a) => a.startsWith("http")) || "http://192.168.1.103:8432").replace(/\/$/, "");
const RACINE = new URL("..", import.meta.url).pathname.replace(/\/$/, "");
const FICHIER_TABLE = join(RACINE, "packages/obrain-dashboard/src/graph/natures.ts");
const SRC = join(RACINE, "packages/obrain-dashboard/src");

/** Seuil de séparation : 33 (le jeu déclaré descend à 33,8 — marge comprise). */
const SEUIL_DISTANCE = 33;
/** Les littéraux que les vues déclarent encore — la table doit s'y accorder. */
const ATTENDUS_COGNITION = ["Message", "ToolCall", "Session", "Step", "SelfState", "ToolResult"];
const ATTENDUS_CODE = ["Function", "Method"];

const echecs = [];
const avertissements = [];
const ok = (m) => console.log(`  OK    ${m}`);
const ko = (m) => { echecs.push(m); console.error(`  ÉCHEC ${m}`); };
const attention = (m) => { avertissements.push(m); console.log(`  ATTENTION ${m}`); };

// ── 1. LIRE LA TABLE, ET REFUSER DE DEVINER ───────────────────────────────
// Contrat de forme : une ligne par type, `Nom: { famille, couleur, libelle }`.
// Le nombre de lignes lues doit ÉGALER le nombre de couleurs déclarées : une
// dérive de format doit échouer bruyamment, pas passer pour une table valide.
console.log(`\n── LA TABLE (${relative(RACINE, FICHIER_TABLE)}) ──`);
let table = new Map();
try {
  const src = readFileSync(FICHIER_TABLE, "utf8");
  const bloc = src.slice(src.indexOf("export const NATURES"));
  const fin = bloc.indexOf("\n};");
  const corps = fin > 0 ? bloc.slice(0, fin) : bloc;
  const ligne = /^\s*([A-Za-z_][A-Za-z0-9_]*):\s*\{\s*famille:\s*"(cognition|code)",\s*couleur:\s*"(#[0-9a-fA-F]{6})",\s*libelle:\s*"([^"]*)",?\s*\},?\s*$/gm;
  for (const m of corps.matchAll(ligne)) table.set(m[1], { famille: m[2], couleur: m[3].toLowerCase(), libelle: m[4] });
  const couleursVues = (corps.match(/couleur:/g) || []).length;
  if (table.size !== couleursVues) ko(`table illisible en partie : ${table.size} types lus pour ${couleursVues} couleurs déclarées (format d'une ligne modifié ?)`);
  else ok(`${table.size} types déclarés, tous lus`);
} catch (e) {
  ko(`table illisible : ${e.message}`);
}
const neutre = (() => {
  try {
    const src = readFileSync(FICHIER_TABLE, "utf8");
    const m = src.match(/NATURE_INCONNUE[^=]*=\s*\{[^}]*couleur:\s*"(#[0-9a-fA-F]{6})"/);
    return m ? m[1].toLowerCase() : null;
  } catch { return null; }
})();
if (!neutre) ko("couleur neutre (NATURE_INCONNUE) introuvable — un type inconnu serait peint comme un type connu");
else if ([...table.values()].some((n) => n.couleur === neutre)) ko(`la couleur neutre ${neutre} est aussi celle d'un type déclaré : l'inconnu se lirait comme du connu`);
else ok(`repli neutre ${neutre}, distinct de tous les types déclarés`);

// ── 2. DEUX TYPES, DEUX COULEURS ──────────────────────────────────────────
console.log("\n── SÉPARATION DES COULEURS ──");
const rgb = (h) => [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16));
const distance = (a, b) => Math.hypot(...rgb(a).map((x, i) => x - rgb(b)[i]));
const entrees = [...table.entries()];
const parCouleur = new Map();
for (const [nom, n] of entrees) {
  if (parCouleur.has(n.couleur)) ko(`${nom} et ${parCouleur.get(n.couleur)} partagent la couleur ${n.couleur}`);
  parCouleur.set(n.couleur, nom);
}
let proche = null;
for (let i = 0; i < entrees.length; i++)
  for (let j = i + 1; j < entrees.length; j++) {
    const d = distance(entrees[i][1].couleur, entrees[j][1].couleur);
    if (!proche || d < proche.d) proche = { d, a: entrees[i][0], b: entrees[j][0] };
  }
if (proche) {
  const d = proche.d.toFixed(1);
  if (proche.d < SEUIL_DISTANCE) ko(`les deux types les plus proches sont à ${d} (< ${SEUIL_DISTANCE}) : ${proche.a} / ${proche.b}`);
  else ok(`le couple le plus proche est à ${d} (seuil ${SEUIL_DISTANCE}) : ${proche.a} / ${proche.b}`);
}

// ── 3. TOUT TYPE PRÉSENT EST DÉCLARÉ ──────────────────────────────────────
console.log(`\n── LES TYPES RÉELLEMENT PRÉSENTS (${BASE}) ──`);
const presentes = new Map();
try {
  const stores = await (await fetch(`${BASE}/api/stores`, { signal: AbortSignal.timeout(15_000) })).json();
  const labels = (stores.presets || []).map((p) => p.label);
  let basesLues = 0;
  for (const label of labels) {
    const r = await fetch(`${BASE}/api/viz/overview?store=${encodeURIComponent(label)}`, { signal: AbortSignal.timeout(60_000) });
    if (!r.ok) { attention(`base « ${label} » : HTTP ${r.status} — non vérifiée`); continue; }
    const d = await r.json();
    basesLues++;
    for (const b of d.by_label || []) presentes.set(b.label, (presentes.get(b.label) || 0) + b.count);
  }
  if (!basesLues) ko("aucune base lue : la garantie n'a pas été vérifiée");
  const inconnus = [...presentes.entries()].filter(([t]) => !table.has(t));
  ok(`${basesLues} bases lues · ${presentes.size} types distincts présents · ${table.size} déclarés`);
  if (inconnus.length) {
    ko(`${inconnus.length} type(s) présent(s) mais NON déclaré(s) — ils s'afficheraient en neutre sans que personne ne sache pourquoi :`);
    for (const [t, n] of inconnus.sort((a, b) => b[1] - a[1])) console.error(`         ${String(n).padStart(8)}  ${t}`);
  } else ok("chaque type présent a une déclaration");
  const nonVus = [...table.keys()].filter((t) => !presentes.has(t));
  if (nonVus.length) console.log(`  (déclarés mais absents de ces bases : ${nonVus.join(", ")} — une déclaration en avance n'est pas une faute)`);
} catch (e) {
  ko(`bases illisibles (${e.message}) — la garantie n'a pas été vérifiée`);
}

// ── 4. LA TABLE CONTREDIT-ELLE LES VUES ? ─────────────────────────────────
console.log("\n── ACCORD AVEC CE QUE LES VUES DÉCLARENT ENCORE ──");
for (const t of ATTENDUS_COGNITION) {
  const n = table.get(t);
  if (!n) ko(`${t} attendu en cognition (liste COG d'une vue) mais absent de la table`);
  else if (n.famille !== "cognition") ko(`${t} est rangé « ${n.famille} » alors qu'une vue le traite comme cognition`);
}
for (const t of ATTENDUS_CODE) {
  const n = table.get(t);
  if (!n) ko(`${t} attendu en code (filtre d'une vue) mais absent de la table`);
  else if (n.famille !== "code") ko(`${t} est rangé « ${n.famille} » alors qu'une vue le traite comme du code`);
}
if (!echecs.length) ok("la table s'accorde avec les listes encore en dur (consolidation, pas second avis)");

// ── 5. LA DETTE : LES LISTES DE TYPES ENCORE EN DUR ───────────────────────
console.log("\n── LISTES DE TYPES ENCORE ÉCRITES EN DUR DANS LES VUES ──");
const fichiers = [];
const parcourir = (d) => {
  for (const e of readdirSync(d)) {
    const p = join(d, e);
    if (statSync(p).isDirectory()) parcourir(p);
    else if (/\.tsx?$/.test(p) && !p.endsWith("natures.ts")) fichiers.push(p);
  }
};
try { parcourir(SRC); } catch (e) { ko(`lecture de src : ${e.message}`); }
const declares = [...table.keys()];
const trouvees = [];
let lectures = 0;
for (const f of fichiers) {
  const lignes = readFileSync(f, "utf8").split("\n");
  lignes.forEach((l, i) => {
    if (/\bnatureDe\b|\bestCognition\b|\bestCode\b|\bcouleurDuType\b/.test(l)) lectures++;
    const vus = declares.filter((t) => new RegExp(`["']${t}["']`).test(l));
    if (vus.length >= 2) trouvees.push({ f: relative(RACINE, f), l: i + 1, vus });
  });
}
for (const t of trouvees) console.log(`  ${t.f}:${t.l}  ${t.vus.length} types en dur — ${t.vus.slice(0, 6).join(", ")}${t.vus.length > 6 ? "…" : ""}`);
if (trouvees.length) {
  const msg = `${trouvees.length} ligne(s) rangent des types en dur au lieu de lire la table`;
  if (STRICT) ko(msg + " (mode --strict)"); else attention(msg + " — à résorber ; `--strict` en fait un échec");
} else ok("aucune liste de types en dur : toute vue qui parle d'un type lit la table");
console.log(`  (la table est lue ${lectures} fois dans src/ — 0 signifie que le branchement des vues reste à faire)`);

// ── VERDICT ──────────────────────────────────────────────────────────────
console.log("");
if (echecs.length) { console.error(`${echecs.length} échec(s) : ${echecs.length ? "la garantie n'est PAS tenue." : ""}`); process.exit(1); }
console.log(`Natures : garantie tenue${avertissements.length ? ` (${avertissements.length} avertissement(s) : dette comptée)` : ""}.`);
process.exit(0);
