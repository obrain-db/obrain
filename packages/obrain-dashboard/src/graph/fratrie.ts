/**
 * ── LA FRATRIE : CE QUI ENTOURE UN ÉLÉMENT, DÉRIVÉ UNE SEULE FOIS ────────────
 *
 * DEMANDE : « qu'on voie bien les frères et les sœurs en cas de sélection d'un
 * élément, pour garantir l'affichage de l'immensité d'information. »
 *
 * ── CE QUI EXISTAIT, ET QUI ÉTAIT FAUX ─────────────────────────────────────
 *
 * La parenté se dérivait DÉJÀ, en DEUX exemplaires incompatibles, tous deux à
 * partir du `qualified_name` :
 *
 *   1. `CosmosWeave.tsx:2446-2451` (`gr.noeudsInfo[].type`) :
 *      `parts[parts.length-2]`, sans filtrer le marqueur `__file__` ;
 *   2. `CosmosWeave.tsx:662-666` (`etiquetteDimension(qn, "parent")`) :
 *      filtre `__file__` PUIS prend `src[src.length-2]`.
 *
 * Ces deux règles divergent sur la TOTALITÉ des qn portant `__file__` (les
 * fichiers indexés finissent par `…index.ts.__file__`) : la puce de filtre
 * annonçait « msg_zerocopy » pendant que `infos[i].type` valait « rst ». Comme
 * le filtre compare `infos[i].type === filtre`, cette puce ne pouvait
 * STRUCTURELLEMENT rien allumer.
 *
 * Ici : UNE fonction, `parentDe`, et l'index de parenté construit en UN parcours.
 *
 * ── LA RÈGLE, EN UNE PHRASE ────────────────────────────────────────────────
 *
 * Le parent d'un nœud est LE DOSSIER QUI LE PORTE. Quand la donnée réelle le
 * dit (`file_path`, présent sur les nœuds servis par `/api/viz/subgraph`), c'est
 * le dossier du chemin sur disque — une information, pas une déduction. Sinon,
 * on le dérive du `qualified_name` : on écarte le marqueur `__file__` (c'est un
 * marqueur de qn, pas un nom) et l'on prend le segment porteur.
 *
 * ── LE COÛT, MESURÉ ────────────────────────────────────────────────────────
 *
 * L'index est construit UNE fois par chargement, JAMAIS dans la boucle de rendu
 * (celle de `CosmosWeave` tourne à 60 Hz et parcourt déjà tous les nœuds et
 * toutes les arêtes). `construireIndex` rend son propre temps de construction
 * (`ms`) : un coût qu'on ne mesure pas est un coût qu'on ne connaît pas.
 */
import { atom } from "jotai";
import type { GraphNode } from "../lib/api";

// ---------------------------------------------------------------------------
// Découpage du qualified_name — une seule fois, pour tout le dépôt
// ---------------------------------------------------------------------------

/** Marqueur de nœud « fichier » dans un qualified_name — il n'est pas un nom. */
export const MARQUEUR_FICHIER = "__file__";

/** Longueur maximale d'une étiquette de groupe (au-delà, illisible à l'écran). */
export const LONGUEUR_ETIQUETTE = 22;

/** Segments d'un qn : les séparateurs réels sont le point et le slash. */
export function segmentsQn(qn: string): string[] {
  return String(qn ?? "").split(/[./]/).filter((x) => x.length > 0);
}

/**
 * Segments UTILES : ceux qui portent un nom. `__file__` est un marqueur, donc
 * écarté ; s'il ne restait que lui, on rend le découpage brut plutôt que rien
 * (un qn dégénéré ne doit pas produire d'étiquette vide).
 */
export function segmentsUtiles(qn: string): string[] {
  const brut = segmentsQn(qn);
  const utiles = brut.filter((x) => x !== MARQUEUR_FICHIER);
  return utiles.length ? utiles : brut;
}

/** Vrai si le qn désigne un FICHIER (il porte le marqueur). */
export const estFichier = (qn: string): boolean =>
  segmentsQn(qn).indexOf(MARQUEUR_FICHIER) >= 0;

/** Un segment qui n'est qu'un remplissage (`_`, `__`) ne dit rien : on le dit. */
const nettoyer = (s: string): string => {
  const t = (s || "").replace(/^_+$/, "?");
  return t.length ? t : "?";
};

const court = (s: string): string =>
  s.length > LONGUEUR_ETIQUETTE ? s.slice(0, LONGUEUR_ETIQUETTE) : s;

/** Dernier segment d'un chemin sur disque, sans le dossier. */
function baseDeChemin(chemin?: string): string | undefined {
  if (!chemin) return undefined;
  const c = String(chemin).replace(/[/\\]+$/, "");
  const i = Math.max(c.lastIndexOf("/"), c.lastIndexOf("\\"));
  const base = i >= 0 ? c.slice(i + 1) : c;
  return base.length ? base : undefined;
}

/** Dernier dossier d'un chemin sur disque. */
function dossierDeChemin(chemin?: string): string | undefined {
  if (!chemin) return undefined;
  const c = String(chemin).replace(/[/\\]+$/, "");
  const i = Math.max(c.lastIndexOf("/"), c.lastIndexOf("\\"));
  if (i <= 0) return undefined;
  return baseDeChemin(c.slice(0, i));
}

// ---------------------------------------------------------------------------
// LA RÈGLE CANONIQUE DE PARENTÉ — une seule implémentation
// ---------------------------------------------------------------------------

/**
 * PARENT d'un nœud = le dossier qui le porte.
 *
 * `cheminFichier` (le `file_path` réel, quand la charge utile le porte) est
 * PRIVILÉGIÉ : c'est une donnée, pas une déduction. La dérivation par le qn
 * reste le repli — et c'est le SEUL endroit du dépôt où elle est écrite.
 *
 * Exemples réels (store « Obrain (rust) », `/api/viz/subgraph`) :
 *   `…crates.obrain_engine.benches.query_bench.rs.__file__`
 *       + `file_path` = `/…/crates/obrain-engine/benches/query_bench.rs`
 *       → « benches »                     (le dossier réel)
 *   `…crates.obrain_engine.benches.bench_node_lookup`
 *       → « benches »                     (segment porteur du qn, `__file__` absent)
 *   `…crates.obrain_engine`
 *       → « crates »
 */
export function parentDe(qn: string, cheminFichier?: string): string {
  const dossier = dossierDeChemin(cheminFichier);
  if (dossier) return court(nettoyer(dossier));
  const u = segmentsUtiles(qn);
  if (!u.length) return "?";
  const brut = u.length >= 2 ? u[u.length - 2] : u[0];
  return court(nettoyer(brut));
}

// ── LES MÊMES RÈGLES, SUR DES SEGMENTS DÉJÀ DÉCOUPÉS ────────────────────────
// `construireIndex` parcourt 200 000 nœuds : y appeler quatre fois
// `segmentsUtiles` faisait quatre découpes de chaîne par nœud. Ces variantes
// prennent le découpage en argument, ce qui ramène le coût à UNE découpe par
// nœud — mêmes règles, même résultats, moins de travail.
function parentDeSegments(u: readonly string[], cheminFichier?: string): string {
  const dossier = dossierDeChemin(cheminFichier);
  if (dossier) return court(nettoyer(dossier));
  if (!u.length) return "?";
  return court(nettoyer(u.length >= 2 ? u[u.length - 2] : u[0]));
}

function nomDeSegments(u: readonly string[]): string {
  return nettoyer(u.length ? u[u.length - 1] : "?");
}

function moduleDeSegments(u: readonly string[]): string {
  if (!u.length) return "?";
  const i = u.indexOf("projects");
  return court(nettoyer(i >= 0 && u.length > i + 1 ? u[i + 1] : u[0]));
}

function fichierDeSegments(u: readonly string[], cheminFichier?: string, marque: boolean = false): string | undefined {
  const base = baseDeChemin(cheminFichier);
  if (base) return base;
  if (u.length < 2 || !marque) return undefined;
  return `${u[u.length - 2]}.${u[u.length - 1]}`;
}

/**
 * FICHIER d'un nœud. Le `file_path` réel fait autorité ; à défaut, un qn
 * marqué `__file__` se termine par `<nom>.<ext>` — on réunit les deux derniers
 * segments utiles. Un nœud qui n'est pas un fichier n'a PAS de fichier : on
 * rend `undefined` plutôt qu'un nom de fonction déguisé en nom de fichier.
 */
export function fichierDe(qn: string, cheminFichier?: string): string | undefined {
  const base = baseDeChemin(cheminFichier);
  if (base) return base;
  const u = segmentsUtiles(qn);
  if (u.length < 2 || !estFichier(qn)) return undefined;
  return `${u[u.length - 2]}.${u[u.length - 1]}`;
}

/**
 * MODULE (projet porteur) d'un nœud : le segment qui suit `projects` quand le
 * qn est absolu, sinon le premier segment. Même règle que l'axe « module » de
 * la vue Cosmos — elle vivait dans `etiquetteDimension`, elle est ici.
 */
export function moduleDe(qn: string): string {
  const u = segmentsUtiles(qn);
  if (!u.length) return "?";
  const i = u.indexOf("projects");
  const brut = i >= 0 && u.length > i + 1 ? u[i + 1] : u[0];
  return court(nettoyer(brut));
}

/** NOM court d'un nœud : le dernier segment utile de son qn. */
export function nomDe(qn: string): string {
  const u = segmentsUtiles(qn);
  return nettoyer(u.length ? u[u.length - 1] : String(qn ?? "?"));
}

/** STRATE d'un nœud : la profondeur de son nom, `__file__` écarté. */
export function strateDe(qn: string): number {
  return Math.min(segmentsUtiles(qn).length, 9);
}

// ---------------------------------------------------------------------------
// L'INDEX DE PARENTÉ — un parcours, un objet
// ---------------------------------------------------------------------------

/** Ce qu'un nœud doit porter pour entrer dans l'index (sous-ensemble d'api.ts). */
export interface NoeudParente {
  id: string;
  qn?: string;
  file_path?: string;
  community_id?: number | null;
  scc_id?: number | null;
}

/**
 * L'index. Toutes les relations sont des `Map` de listes : un index de parenté
 * qui ne rendrait que des compteurs ne permettrait pas de NOMMER les frères —
 * or c'est exactement ce qui manquait.
 */
export interface IndexFratrie {
  /** Nombre de nœuds indexés. */
  readonly n: number;
  /** Temps de construction RÉELLEMENT mesuré (ms). */
  readonly ms: number;
  /** qn → clé de nœud (pour retrouver un nœud par son qualified_name). */
  readonly cle: ReadonlyMap<string, string>;
  /** clé → qn. */
  readonly qn: ReadonlyMap<string, string>;
  /** clé → nom court affichable. */
  readonly nom: ReadonlyMap<string, string>;
  /** clé → dossier porteur (voir `parentDe`). */
  readonly parent: ReadonlyMap<string, string>;
  /** clé → fichier porteur, quand le nœud EST un fichier. */
  readonly fichier: ReadonlyMap<string, string>;
  /** clé → module (projet) porteur. */
  readonly module: ReadonlyMap<string, string>;
  /** Dossier porteur → ses nœuds (la FRATRIE). */
  readonly fratrieParParent: ReadonlyMap<string, readonly string[]>;
  /** Communauté Leiden → ses nœuds (les SŒURS). */
  readonly soeursParCommunaute: ReadonlyMap<number, readonly string[]>;
  /** Fichier → ses nœuds (les CO-FICHIERS). */
  readonly parFichier: ReadonlyMap<string, readonly string[]>;
  /** Nom court → les HOMONYMES (deux `index.ts` de dossiers différents). */
  readonly parLabel: ReadonlyMap<string, readonly string[]>;
  /** Module → ses nœuds (les CO-MODULES). */
  readonly parModule: ReadonlyMap<string, readonly string[]>;
  /** Composante fortement connexe → ses nœuds (les membres du même cycle). */
  readonly parScc: ReadonlyMap<number, readonly string[]>;
}

/** Ajoute `v` à la liste de `m[k]`, en créant la liste au besoin. */
function empiler<T>(m: Map<T, string[]>, k: T, v: string): void {
  const l = m.get(k);
  if (l) l.push(v);
  else m.set(k, [v]);
}

/**
 * Construit l'index en UN parcours O(n).
 *
 * Aucune requête, aucune lecture de fichier : l'index est LOCAL. C'est la
 * promesse « sans ralentissement » — et `scripts/fratrie-check.mjs` la vérifie
 * en comparant les entrées `performance` avant et après la construction.
 *
 * `options.chrono = false` évite l'appel à `performance.now()` dans un contexte
 * où l'horloge n'existe pas (tests hors navigateur).
 */
export function construireIndex(
  noeuds: readonly NoeudParente[],
  options: { chrono?: boolean } = {},
): IndexFratrie {
  const chrono = options.chrono !== false && typeof performance !== "undefined";
  const t0 = chrono ? performance.now() : 0;

  const cle = new Map<string, string>();
  const qn = new Map<string, string>();
  const nom = new Map<string, string>();
  const parent = new Map<string, string>();
  const fichier = new Map<string, string>();
  const module = new Map<string, string>();
  const fratrieParParent = new Map<string, string[]>();
  const soeursParCommunaute = new Map<number, string[]>();
  const parFichier = new Map<string, string[]>();
  const parLabel = new Map<string, string[]>();
  const parModule = new Map<string, string[]>();
  const parScc = new Map<number, string[]>();

  // Un nœud sans identifiant n'est pas indexable : on l'écarte au lieu de le
  // compter sous une clé inventée (ce qui créerait une fratrie fantôme).
  for (let i = 0; i < noeuds.length; i++) {
    const nd = noeuds[i];
    if (!nd || nd.id === undefined || nd.id === null || nd.id === "") continue;
    const k = String(nd.id);
    const q = String(nd.qn || nd.id);
    // UNE SEULE DÉCOUPE PAR NŒUD : tout se dérive de `u`.
    const brut = segmentsQn(q);
    let marque = false;
    const utiles: string[] = [];
    for (let s = 0; s < brut.length; s++) {
      if (brut[s] === MARQUEUR_FICHIER) marque = true;
      else utiles.push(brut[s]);
    }
    const u = utiles.length ? utiles : brut;
    const p = parentDeSegments(u, nd.file_path);
    cle.set(q, k);
    qn.set(k, q);
    const nm = nomDeSegments(u);
    nom.set(k, nm);
    parent.set(k, p);
    const f = fichierDeSegments(u, nd.file_path, marque);
    if (f) fichier.set(k, f);
    const m = moduleDeSegments(u);
    module.set(k, m);
    empiler(fratrieParParent, p, k);
    if (nd.community_id !== undefined && nd.community_id !== null) {
      empiler(soeursParCommunaute, Number(nd.community_id), k);
    }
    if (f) empiler(parFichier, f, k);
    empiler(parLabel, nm, k);
    empiler(parModule, m, k);
    if (nd.scc_id !== undefined && nd.scc_id !== null) {
      empiler(parScc, Number(nd.scc_id), k);
    }
  }

  return {
    n: qn.size,
    ms: chrono ? +(performance.now() - t0).toFixed(3) : 0,
    cle, qn, nom, parent, fichier, module,
    fratrieParParent, soeursParCommunaute, parFichier, parLabel, parModule, parScc,
  };
}

/** Index vide — ce qu'une vue rend quand elle n'a AUCUNE donnée à indexer. */
export const INDEX_VIDE: IndexFratrie = construireIndex([], { chrono: false });

// ---------------------------------------------------------------------------
// LA FRATRIE D'UN ÉLÉMENT — six relations, toutes fondées sur des données
// ---------------------------------------------------------------------------

export type GenreFratrie =
  | "freres"      // même dossier porteur
  | "co-fichiers" // même fichier (file_path réel)
  | "soeurs"      // même communauté (Leiden, community_id)
  | "co-modules"  // même module / projet
  | "cycle"       // même composante fortement connexe (scc_id)
  | "homonymes";  // même nom court

export interface GroupeFratrie {
  genre: GenreFratrie;
  /** Libellé humain de la relation. */
  titre: string;
  /** Ce qui définit le groupe (« benches », « communauté 3 »…) — la VRAIE clé. */
  cle: string;
  /** Effectif RÉEL de la relation, soi EXCLU. `membres.length` peut le dépasser. */
  total: number;
  /** Clés de nœuds, triées par nom. Jamais tronqué en silence. */
  membres: string[];
  /** Pourquoi le groupe est vide, quand il l'est — jamais une devinette. */
  raison?: string;
}

export interface Fratrie {
  /** Clé du nœud dans l'index. */
  cle: string;
  qn: string;
  nom: string;
  parent: string;
  fichier?: string;
  module: string;
  communaute?: number;
  scc?: number;
  /** Nombre de nœuds indexés — le dénominateur de tout ce qui suit. */
  indexes: number;
  groupes: GroupeFratrie[];
}

const sansSoi = (l: readonly string[] | undefined, moi: string): string[] =>
  l ? l.filter((x) => x !== moi) : [];

const trier = (idx: IndexFratrie, l: string[]): string[] =>
  l.slice().sort((a, b) =>
    (idx.nom.get(a) || a).localeCompare(idx.nom.get(b) || b, "fr"));

/**
 * Les six relations d'un nœud, calculées depuis l'index — donc sans réseau et
 * sans parcours du graphe : une lecture de `Map`.
 *
 * Un groupe sans membre reste PRÉSENT avec sa `raison` : une facette qui
 * disparaît ne dit pas si la relation est absente des données ou si elle est
 * vide. C'est la règle du dépôt — « une facette vide s'affiche désactivée avec
 * sa raison, jamais alimentée par une devinette ».
 */
export function fratrieDe(index: IndexFratrie, cleNoeud: string): Fratrie {
  const k = cleNoeud;
  const q = index.qn.get(k) ?? String(cleNoeud);
  const p = index.parent.get(k) ?? parentDe(q);
  const f = index.fichier.get(k);
  const m = index.module.get(k) ?? moduleDe(q);
  const comm = index.soeursParCommunaute.size
    ? trouverEntier(index.soeursParCommunaute, k)
    : undefined;
  const scc = index.parScc.size ? trouverEntier(index.parScc, k) : undefined;

  const freres = trier(index, sansSoi(index.fratrieParParent.get(p), k));
  const coFichiers = f ? trier(index, sansSoi(index.parFichier.get(f), k)) : [];
  const soeurs = comm !== undefined
    ? trier(index, sansSoi(index.soeursParCommunaute.get(comm), k)) : [];
  const coModules = trier(index, sansSoi(index.parModule.get(m), k));
  const cycle = scc !== undefined ? trier(index, sansSoi(index.parScc.get(scc), k)) : [];
  const homonymes = trier(index, sansSoi(index.parLabel.get(index.nom.get(k) ?? ""), k));

  const groupes: GroupeFratrie[] = [
    { genre: "freres", titre: "Frères — même dossier porteur", cle: p,
      total: freres.length, membres: freres,
      raison: freres.length ? undefined : `aucun autre nœud chargé sous « ${p} »` },
    { genre: "co-fichiers", titre: "Co-fichiers — même fichier", cle: f ?? "?",
      total: coFichiers.length, membres: coFichiers,
      raison: f
        ? (coFichiers.length ? undefined : `seul nœud chargé de « ${f} »`)
        : "nœud sans fichier (aucun file_path et qn non marqué __file__)" },
    { genre: "soeurs", titre: "Sœurs — même communauté", cle: comm !== undefined ? String(comm) : "?",
      total: soeurs.length, membres: soeurs,
      raison: comm !== undefined
        ? (soeurs.length ? undefined : `seule de la communauté ${comm} dans ce chargement`)
        : "aucune communauté renseignée (le moteur n'en a pas fourni)" },
    { genre: "co-modules", titre: "Co-modules — même module", cle: m,
      total: coModules.length, membres: coModules,
      raison: coModules.length ? undefined : `seul nœud chargé du module « ${m} »` },
    { genre: "cycle", titre: "Cycle — même composante fortement connexe",
      cle: scc !== undefined ? String(scc) : "?",
      total: cycle.length, membres: cycle,
      raison: scc !== undefined
        ? (cycle.length ? undefined : `seul membre chargé de la composante ${scc}`)
        : "aucun scc_id fourni par le moteur pour ce chargement" },
    { genre: "homonymes", titre: "Homonymes — même nom",
      cle: index.nom.get(k) ?? "?",
      total: homonymes.length, membres: homonymes,
      raison: homonymes.length ? undefined : "aucun homonyme chargé" },
  ];

  return {
    cle: k, qn: q, nom: index.nom.get(k) ?? nomDe(q), parent: p, fichier: f, module: m,
    communaute: comm, scc,
    indexes: index.n,
    groupes,
  };
}

/** Retrouve la clé numérique (communauté, scc) d'un nœud — sans faux positif. */
function trouverEntier(m: ReadonlyMap<number, readonly string[]>, k: string): number | undefined {
  for (const [entier, liste] of m) {
    if (liste.indexOf(k) >= 0) return entier;
  }
  return undefined;
}

/** Le groupe d'un genre donné, ou `undefined` si l'index ne le porte pas. */
export const groupeDe = (fr: Fratrie, g: GenreFratrie): GroupeFratrie | undefined =>
  fr.groupes.find((x) => x.genre === g);

/**
 * Le TOTAL d'une relation, tel qu'il doit être ANNONCÉ : c'est le nombre de
 * membres réellement connus de l'index, jamais le nombre d'éléments affichés.
 * « Si tu tronques à N, écris le total » — ce total vient d'ici.
 */
export const totalDe = (fr: Fratrie, g: GenreFratrie): number =>
  groupeDe(fr, g)?.total ?? 0;

/** Somme des effectifs de toutes les relations — la « part visible » du nœud. */
export function porteeTotale(fr: Fratrie): number {
  const vues = new Set<string>();
  for (const g of fr.groupes) for (const m of g.membres) vues.add(m);
  return vues.size;
}

/**
 * LE NŒUD LE PLUS ENTOURÉ DE L'INDEX — dernier recours, TOUJOURS NOMMÉ.
 *
 * Cas réel mesuré (Cosmos, store « Runtime dev ») : le graphe est chargé autour
 * du groupe « deepseek_harness », qui est un PRÉFIXE de qn et non un nœud — le
 * sous-graphe revient donc sans lui, et la fratrie n'avait rien à montrer.
 * Plutôt que de rester muet, on montre l'élément du graphe chargé qui a le plus
 * de frères, et l'on DIT que c'est ce choix-là : c'est un nœud réel, choisi par
 * un critère énoncé — jamais une parenté inventée.
 *
 * Le parcours est celui des groupes déjà construits : O(nombre de groupes).
 */
export function noeudLePlusEntoure(index: IndexFratrie): { cle: string; total: number } | null {
  let meilleur: { cle: string; total: number } | null = null;
  for (const membres of index.fratrieParParent.values()) {
    if (membres.length < 2) continue;
    if (!meilleur || membres.length > meilleur.total + 1) {
      meilleur = { cle: membres[0], total: membres.length - 1 };
    }
  }
  return meilleur;
}

// ---------------------------------------------------------------------------
// L'ATOME DE CIBLE — ce que la sélection perd, la fratrie le garde
// ---------------------------------------------------------------------------

/**
 * ── LE PIÈGE CONNU, TRAITÉ ICI ──────────────────────────────────────────────
 *
 * `ancrerAtom` (`src/app/store.ts:144`) écrit `selectedNodeAtom = null` : c'est
 * VOULU (on ne sélectionne plus l'ancien centre), mais cela fermait la fratrie
 * à l'instant précis où l'on venait de cliquer un frère — donc impossible
 * d'aller de frère en frère, qui est le geste même de la demande.
 *
 * Cette cible-ci n'est PAS la sélection : c'est « la fratrie ouverte ». Elle
 * survit à l'ancrage, et la fratrie se rouvre sur le nouveau nœud dès qu'il est
 * chargé. L'atome vit ici (et non dans `src/app/store.ts`, qui appartient à un
 * autre chantier) : une autorité, un fichier propriétaire.
 */
export const fratrieCibleAtom = atom<string | null>(null);

// ---------------------------------------------------------------------------
// INSTRUMENT — la règle est vérifiable depuis le navigateur
// ---------------------------------------------------------------------------

/**
 * `window.__fratrie` : la MÊME porte que `window.__pickSweep` et
 * `window.__etages` (voir `CosmosWeave.tsx`). Sans elle, un contrôle ne
 * pourrait vérifier la règle de parenté qu'en la RÉÉCRIVANT dans le script de
 * test — c'est-à-dire en mesurant sa copie, pas le produit.
 */
export interface InstrumentFratrie {
  parentDe: typeof parentDe;
  fichierDe: typeof fichierDe;
  moduleDe: typeof moduleDe;
  segmentsUtiles: typeof segmentsUtiles;
  construireIndex: typeof construireIndex;
  fratrieDe: typeof fratrieDe;
  version: string;
}

if (typeof window !== "undefined") {
  (window as unknown as Record<string, unknown>).__fratrie = {
    parentDe, fichierDe, moduleDe, segmentsUtiles, construireIndex, fratrieDe,
    version: "fratrie/1",
  } satisfies InstrumentFratrie;
}

/** Convertit une liste de nœuds du client (`GraphNode`) en nœuds indexables. */
export const aIndexer = (noeuds: readonly GraphNode[]): NoeudParente[] =>
  noeuds.map((n) => ({
    id: String(n.id),
    qn: n.qn, file_path: n.file_path,
    community_id: n.community_id, scc_id: n.scc_id,
  }));
