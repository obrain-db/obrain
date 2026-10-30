// Projection multi-axes : transforme une réponse moteur (colonnes + lignes) en
// vue structurée par axes — un axe par colonne significative, « organisée
// structurellement » : les dimensions catégorielles deviennent des barres
// (groupes agrégés), les mesures numériques deviennent une distribution.
// C'est la couche qui répond au « voir ce qui répond à la demande » du canvas.
//
// ── DÉFAUTS MESURÉS DE LA VERSION PRÉCÉDENTE (2026-09-13, Node 22.19 / V8) ────
// Le banc `scripts/projection-check.mjs` et la sonde de coût rejouent ces cas :
//
//  1. `Math.min(...vals)` / `Math.max(...vals)` (anciennes lignes 53-54)
//     ÉTALAIENT le tableau en arguments. Mesuré : 125 258 lignes passaient,
//     150 000 levaient `RangeError: Maximum call stack size exceeded`, dans une
//     fonction appelée pendant le RENDU React — l'exception cassait tout
//     l'arbre (l'`Inspector` n'est enveloppé d'aucun `CanvasBoundary`).
//     `/api/query` n'a AUCUNE borne de lignes côté serveur et le corpus servi
//     fait 413 985 nœuds : le seuil était atteignable en une requête.
//     Correction : UNE passe qui porte min, max, somme et effectif à la fois.
//
//  2. TOUS les groupes étaient construits puis triés (`items` de 8 000 entrées
//     mesurées sur 8 000 lignes) pour n'en AFFICHER que 16 (`.slice(0, 16)`).
//     Le plafond de 16 était donc un plafond d'affichage appliqué APRÈS le coût.
//     Correction : construction bornée à TOP_GROUPES (64) par sélection partielle
//     (pas de tri complet), plus les compteurs EXACTS `total` / `groupCount` /
//     `hidden` — sans eux, borner ferait mentir les pourcentages.
//
//  3. Les bornes de buckets étaient arrondies au plus proche, donc les
//     intervalles se CHEVAUCHAIENT : sur [2, 3, 5] on lisait « 2–3 », « 3–4 »,
//     « 4–5 » (le 3 dans deux buckets). Correction : pas « rond » (1/2/5 × 10^k)
//     et libellés au nombre de décimales du pas.

export interface ProjectItem {
  label: string;
  /** Somme de la mesure du groupe (ou cardinal si aucune colonne-mesure). */
  value: number;
  /** Effectif : nombre de lignes du groupe. */
  cnt?: number;
}
export interface ProjectAxis {
  key: string;                 // nom de colonne (axe)
  kind: "cat" | "num";
  /** cat : TOP_GROUPES groupes au plus, triés desc. num : la distribution. */
  items: ProjectItem[];
  anchorable: boolean;         // les items sont des qualified_name → ancrables
  /**
   * Somme EXACTE de TOUS les groupes, y compris ceux qui ne sont pas construits.
   * Sans ce champ, borner `items` ferait mentir les pourcentages affichés.
   */
  total: number;
  /**
   * cat : nombre EXACT de groupes distincts. num : nombre de buckets construits
   * (le nombre exact de valeurs distinctes n'est PAS suivi : un `Set` de 413 985
   * valeurs coûterait une passe et de la mémoire pour une information dont seul
   * le « y a-t-il plus d'une valeur ? » est utilisé — le Set est donc borné).
   */
  groupCount: number;
  /** Groupes présents dans les données mais absents d'`items` (cat). */
  hidden: number;
  // num : distribution discrète en buckets
  min?: number; max?: number;
  sum?: number; mean?: number;
  values?: ProjectItem[];
}

const MEASURE_RE = /^(count|nbr|nb|n|total|weight|w|value|size|nbre|nombre)$/i;
const QN_RE = /^[a-zA-Z_][a-zA-Z0-9_]*(\.[a-zA-Z0-9_]+)+$/;

/** Plafond de CONSTRUCTION par axe (l'affichage en montre moins encore). */
export const TOP_GROUPES = 64;
export const MAX_BUCKETS = 8;

/**
 * Le seuil au-delà duquel la projection ne se lance plus toute seule.
 *
 * Mesuré sur le banc (`node scripts/projection-check.mjs --bench`) : à
 * 50 000 lignes la construction coûte déjà ~39 ms pour n'afficher que 64 barres ;
 * à 150 000 l'ancien code LEVAIT. 20 000 est le point où le coût (≈ 8 ms mesuré)
 * reste sous la durée d'une image de 60 Hz, avec une marge pour une machine
 * chargée : au-delà, l'utilisateur doit CHOISIR (LIMIT, ou projeter quand même).
 */
export const SEUIL_PROJECTION_ROWS = 20_000;

/** Décision de refus, calculée — les deux points d'entrée la partagent. */
export function refusProjection(rowCount: number): { rows: number } | null {
  return rowCount > SEUIL_PROJECTION_ROWS ? { rows: rowCount } : null;
}

/**
 * Réécrit une requête pour la borner : « … LIMIT 8 » → « … LIMIT 20000 ».
 * Sans ce geste, « ajouter LIMIT » demanderait à l'utilisateur de retaper sa
 * requête — c'est-à-dire de renoncer.
 */
export function withLimit(query: string, n: number): string {
  const sans = query.trim().replace(/\s*;?\s*LIMIT\s+\d+\s*;?\s*$/i, "").trim();
  return `${sans} LIMIT ${n}`;
}

function isQn(v: unknown): boolean {
  return typeof v === "string" && QN_RE.test(v.trim()) &&
    (v.match(/\./g) || []).length >= 2;
}

/** Colonne « mesure » utilisée pour agréger (sinon on compte les lignes). */
function pickMeasure(columns: string[]): number {
  return columns.findIndex((c) => MEASURE_RE.test(c.trim()));
}

const finite = (v: unknown): number => {
  const n = typeof v === "number" ? v : parseFloat(String(v));
  return Number.isFinite(n) ? n : NaN;
};

/** Pas « rond » (1/2/5 × 10^k) : des bornes de buckets lisibles et disjointes. */
function pasRond(brut: number): number {
  if (!Number.isFinite(brut) || brut <= 0) return 0;
  const mag = Math.pow(10, Math.floor(Math.log10(brut)));
  const norm = brut / mag;
  const nice = norm <= 1 ? 1 : norm <= 2 ? 2 : norm <= 5 ? 5 : 10;
  return nice * mag;
}

/** Décimales nécessaires pour écrire un pas sans arrondi trompeur. */
function decOf(step: number): number {
  if (!Number.isFinite(step) || step <= 0) return 0;
  return Math.max(0, Math.min(6, -Math.floor(Math.log10(step))));
}

/**
 * Sélection partielle des TOP_GROUPES plus gros groupes d'une Map, sans trier
 * l'ensemble : on garde un tableau trié de 64 entrées au plus et on REJETTE en
 * O(1) tout ce qui est plus petit que le dernier — c'est ce qui remplace le tri
 * des 8 000 à 413 985 groupes.
 */
function topK(entries: [string, { v: number; cnt: number }][], k: number): ProjectItem[] {
  const top: ProjectItem[] = [];
  for (const [label, g] of entries) {
    const value = Math.round(g.v * 100) / 100;
    const item: ProjectItem = { label, value, cnt: g.cnt };
    if (top.length === k && value <= top[k - 1].value) continue;
    let lo = 0, hi = top.length;
    while (lo < hi) {                       // insertion par dichotomie (desc)
      const mid = (lo + hi) >> 1;
      if (top[mid].value >= value) lo = mid + 1; else hi = mid;
    }
    top.splice(lo, 0, item);
    if (top.length > k) top.pop();
  }
  return top;
}

/**
 * Projette une réponse moteur en axes multi-dimensionnels.
 * - Chaque colonne catégorielle (ou non-mesure) devient un axe avec barres
 *   agrégées par valeur (somme de la mesure si présente, sinon cardinal).
 * - La première colonne mesure (count/nbr/…) devient l'axe « volume ».
 * - Les colonnes dont les valeurs sont des qualified_name sont marquées
 *   `anchorable` pour proposer l'ancrage dans le graphe.
 *
 * Coût borné : UNE passe par colonne pour l'agrégat, une passe sur les groupes
 * pour la sélection partielle. Aucun `Math.min(...)` étalé, aucune liste
 * illimitée construite.
 */
export function projectAxes(columns: string[], rows: unknown[][]): ProjectAxis[] {
  if (!columns.length || !rows.length) return [];
  const measureIdx = pickMeasure(columns);
  const axes: ProjectAxis[] = [];

  // 1) axe « volume » (mesure) → distribution numérique.
  //    min / max / somme / effectif / distincts dans la MÊME boucle : c'est la
  //    correction du `Math.min(...vals)` qui levait sur 150 000 lignes.
  if (measureIdx >= 0) {
    let min = Infinity, max = -Infinity, sum = 0, n = 0;
    const uniq = new Set<number>();          // borné : on n'a besoin que de >8
    for (const r of rows) {
      const v = finite(r[measureIdx]);
      if (Number.isNaN(v)) continue;
      if (v < min) min = v;
      if (v > max) max = v;
      sum += v;
      n++;
      if (uniq.size <= MAX_BUCKETS) uniq.add(v);
    }
    if (n) {
      const nb = Math.min(MAX_BUCKETS, uniq.size);
      const values: ProjectItem[] = [];
      if (nb > 1 && max > min) {
        const step = pasRond((max - min) / nb) || (max - min) / nb;
        const nB = Math.max(1, Math.min(nb, Math.ceil((max - min) / step - 1e-9)));
        const dec = decOf(step);
        const counts = new Array<number>(nB).fill(0);
        for (const r of rows) {
          const v = finite(r[measureIdx]);
          if (Number.isNaN(v)) continue;
          const b = Math.max(0, Math.min(nB - 1, Math.floor((v - min) / step)));
          counts[b]++;
        }
        for (let b = 0; b < nB; b++) {
          if (!counts[b]) continue;
          const lo = min + b * step, hi = min + (b + 1) * step;
          // Crochets et non tiret : les intervalles sont DISJOINTS et le dernier
          // est fermé. « 2–3 », « 3–4 » se lisait comme un chevauchement (le 3
          // semblait dans deux buckets) — c'était le cas dans l'ancienne version,
          // où les bornes étaient arrondies au plus proche.
          const lab = `[${lo.toFixed(dec)}, ${hi.toFixed(dec)}${b === nB - 1 ? "]" : "["}`;
          values.push({ label: lab, value: counts[b], cnt: counts[b] });
        }
      } else {
        values.push({ label: String(max), value: n, cnt: n });
      }
      axes.push({
        key: columns[measureIdx], kind: "num", items: values, anchorable: false,
        // Ici le « total » est l'effectif des valeurs finies : la somme des
        // buckets lui est ÉGALE (chaque valeur tombe dans exactement un bucket).
        total: n, groupCount: values.length, hidden: 0,
        min, max, sum, mean: sum / n, values,
      });
    }
  }

  // 2) dimensions catégorielles (non-mesure) → barres de groupes
  columns.forEach((col, i) => {
    if (i === measureIdx) return;
    // Test « colonne numérique ? » avec SORTIE ANTICIPÉE : l'ancienne version
    // matérialisait `rows.map(...)` pour CHAQUE colonne (colonnes × lignes
    // allocations) avant de décider ; une colonne catégorielle sort maintenant
    // à la première valeur non numérique.
    let isNumCol = true;
    for (const r of rows) {
      const v = r[i];
      if (!(typeof v === "number" || (v != null && !isNaN(finite(v))))) { isNumCol = false; break; }
    }
    if (isNumCol) return; // colonne numérique non-mesure : ignorée comme dimension

    const groups = new Map<string, { v: number; cnt: number }>();
    let cents = 0;                           // somme exacte en centièmes (entiers)
    for (const r of rows) {
      const cell = r[i];
      const key = cell == null ? "(vide)" : String(cell);
      let g = groups.get(key);
      if (!g) { g = { v: 0, cnt: 0 }; groups.set(key, g); }
      g.cnt += 1;
      const m = measureIdx >= 0 ? finite(r[measureIdx]) : 1;
      if (Number.isFinite(m)) { g.v += m; cents += Math.round(m * 100); }
    }
    if (!groups.size) return;
    const items = topK([...groups.entries()], TOP_GROUPES);
    // `anchorable` est évalué sur les groupes CONSTRUITS (64) : c'est un indice
    // honnête — un axe de qualified_name l'est par ses plus gros groupes — et
    // cela évite 413 985 tests de regex.
    const anchorable = items.some((it) => isQn(it.label));
    axes.push({
      key: col, kind: "cat", items, anchorable,
      total: cents / 100, groupCount: groups.size,
      hidden: Math.max(0, groups.size - items.length),
    });
  });

  // tri : dimensions d'abord, volume en dernier
  return axes.sort((a, b) => (a.kind === "cat" ? 0 : 1) - (b.kind === "cat" ? 0 : 1));
}

/**
 * TOUS les groupes d'un axe, sans plafond — pour le geste explicite « tout voir »
 * (et pour que le filtre texte puisse atteindre la traîne, hors du top 64).
 *
 * COÛT MESURÉ (banc, 300 000 lignes, colonne à 997 groupes) : ~57 ms de
 * construction + tri. Ce n'est donc PAS appelé au rendu : seulement au clic, et
 * la liste est rendue par fenêtre (`ListeFenetree`) — jamais 300 000 nœuds DOM.
 */
export function axisItemsFull(columns: string[], rows: unknown[][], key: string): ProjectItem[] {
  const i = columns.indexOf(key);
  if (i < 0 || !rows.length || i === pickMeasure(columns)) return [];
  const groups = new Map<string, { v: number; cnt: number }>();
  const measureIdx = pickMeasure(columns);
  for (const r of rows) {
    const cell = r[i];
    const k = cell == null ? "(vide)" : String(cell);
    let g = groups.get(k);
    if (!g) { g = { v: 0, cnt: 0 }; groups.set(k, g); }
    g.cnt += 1;
    const m = measureIdx >= 0 ? finite(r[measureIdx]) : 1;
    if (Number.isFinite(m)) g.v += m;
  }
  return [...groups.entries()]
    .map(([label, g]) => ({ label, value: Math.round(g.v * 100) / 100, cnt: g.cnt }))
    .sort((a, b) => b.value - a.value || a.label.localeCompare(b.label));
}

/**
 * Total affichable d'un axe (pour l'échelle des barres et les pourcentages).
 *
 * Rend la somme EXACTE de tous les groupes — `total` quand il est connu (même
 * si `items` est borné à 64), sinon la somme des items. C'est ce qui garantit
 * qu'un pourcentage affiché reste vrai alors que la liste est tronquée.
 */
export function axisTotal(a: ProjectAxis): number {
  if (typeof a.total === "number" && Number.isFinite(a.total)) return a.total;
  return a.items.reduce((s, x) => s + Math.max(0, x.value), 0);
}
