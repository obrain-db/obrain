// Client minimal des endpoints du serveur obrain (viz serve).
// Toutes les fonctions réseau acceptent un AbortSignal : dès qu'on change de
// configuration (store, niveau, zoom, requête…), on annule les fetchs en vol
// pour ne pas empiler d'agrégats lourds côté serveur.
export interface GraphNode { id: string; qn?: string; label?: string; file_path?: string;
  energy?: number; curvature?: number; scc_id?: number; community_id?: number;
  engram_id?: string; color_hint?: string; size_hint?: number; }
export interface GraphEdge { src: string; dst: string; type: string;
  weight?: number; ricci?: number; stale?: boolean; pheromone?: number; color_hint?: string; }
export interface JsonGraph { schema: string; stats: { node_count: number; edge_count: number;
  truncated: boolean }; nodes: GraphNode[]; edges: GraphEdge[]; }
export interface FunctionHit { id: number; qn: string; name: string;
  type?: string; file_path?: string }

const qs = (o: Record<string, string|number|undefined>) =>
  Object.entries(o).filter(([,v]) => v !== undefined && v !== "")
    .map(([k, v]) => `${encodeURIComponent(k)}=${encodeURIComponent(String(v))}`).join("&");


// ── FLUX UNIFIÉ : cache de session + single-flight (une seule requête en vol
// par URL). Les agrégats (signatures/overview/modules/info) sont immuables à
// court terme : on les garde 60 s et on déduplique les appels concurrents.
//
// ⚠️ À NE PAS UTILISER pour une requête ANNULABLE (voir `runQuery`) : le chemin
// caché et le single-flight IGNORENT le `signal` du demandeur et partagent UNE
// promesse. Une annulation (changement de store, de centre, rafale de clics)
// rejetterait alors la promesse pour TOUS les demandeurs, y compris ceux qui ne
// voulaient pas annuler. `runQuery` a donc son propre cache de RÉSULTATS.
const SESSION = new Map<string, { t: number; v: unknown }>();
const INFLIGHT = new Map<string, Promise<unknown>>();
/** Plafond du cache de session : un sous-graphe pèse jusqu'à 450 000 octets
 *  (mesuré : Linux e1000, hops=0, limit=20000), et le corpus servi en propose
 *  plusieurs — sans plafond, le cache retiendrait des mégaoctets par onglet. */
const SESSION_MAX = 64;
async function getCached<T>(url: string, ttlMs = 60_000, signal?: AbortSignal,
  /** Chargeur de remplacement : permet d'interposer un AUTRE transport (WS) sous
   *  le même cache, sans dupliquer la logique de session/single-flight. */
  chargeur?: (s?: AbortSignal) => Promise<T>): Promise<T> {
  const hit = SESSION.get(url);
  if (hit && Date.now() - hit.t < ttlMs) return hit.v as T;
  const busy = INFLIGHT.get(url) as Promise<T> | undefined;
  if (busy) return busy;
  const p = (chargeur ? chargeur(signal) : get<T>(url, signal)).then((v) => {
    SESSION.set(url, { t: Date.now(), v });
    while (SESSION.size > SESSION_MAX) {
      const plus_vieille = SESSION.keys().next().value as string | undefined;
      if (plus_vieille === undefined) break;
      SESSION.delete(plus_vieille);
    }
    return v;
  }).finally(() => INFLIGHT.delete(url));
  INFLIGHT.set(url, p as Promise<unknown>);
  return p;
}

async function get<T>(url: string, signal?: AbortSignal): Promise<T> {
  const r = await fetch(url, { signal });
  if (!r.ok) throw new Error((await r.text()).slice(0, 300));
  return r.json() as Promise<T>;
}
/** Vrai pour une annulation volontaire (AbortError) — à ignorer partout. */
export const isAbort = (e: unknown) => (e as any)?.name === "AbortError";
/** Une erreur d'annulation fabriquée (chemin caché) : même nom que celle de
 *  `fetch`, pour que tous les `catch` existants la reconnaissent. */
const abortError = () => Object.assign(new Error("requête annulée"), { name: "AbortError" });

/**
 * Enveloppe un résultat (caché ou partagé) du `signal` du DEMANDEUR.
 * Une annulation ne rejette que l'attente de CE demandeur : la promesse
 * partagée (single-flight) continue pour les autres, et `onAbandon` permet de
 * décompter les attentes d'un lot planifié.
 */
function avecSignal<T>(p: Promise<T>, signal?: AbortSignal, onAbandon?: () => void): Promise<T> {
  if (!signal) return p;
  return new Promise<T>((res, rej) => {
    const abandon = () => { onAbandon?.(); rej(abortError()); };
    if (signal.aborted) return abandon();
    signal.addEventListener("abort", abandon, { once: true });
    p.then(
      (v) => { signal.removeEventListener("abort", abandon); res(v); },
      (e) => { signal.removeEventListener("abort", abandon); rej(e); },
    );
  });
}

export const fetchStores = (signal?: AbortSignal) => get<any>("/api/stores", signal);
export const fetchInfo = (store: string, signal?: AbortSignal) =>
  get<{ store: string; nodes_estimate: number }>("/api/info?store=" + encodeURIComponent(store), signal).catch(() => null);
/**
 * Sous-graphe d'un centre.
 *
 * DÉFAUT MESURÉ : cet appel était le SEUL des appels lourds sans aucun cache,
 * alors qu'une réponse pèse jusqu'à 450 000 octets (mesuré : `hops=0&limit=20000`
 * sur Linux e1000) et que le même centre est redemandé à chaque image par la
 * boucle de rendu de `StageContinu`. Il passe donc par le cache de session
 * (30 s) — le centre d'une exploration ne change pas plus vite que cela.
 *
 * ── RÉUNIFICATION DES LIGNÉES (2026-09-15) ──────────────────────────────────
 * Cet appel porte désormais les DEUX apports que deux lignées avaient
 * séparément : le cache de session (lignée du tronc) ET le transport WebSocket
 * en streaming avec repli HTTP (lignée servie, commit `bfad86ff`). Les deux
 * lignées avaient le même point de passage ; la réunification se fait ICI, sous
 * le cache — et non à sa place : un portage brut du client WS aurait supprimé
 * le cache de session sans que rien ne le dise.
 */

/** Le transport WS pousse le MÊME subgraph par chunks ; identité prouvée côté
 *  serveur. Au moindre échec on retombe sur HTTP+gzip, sans erreur visible. */
function wsUrl(path: string): string {
  const proto = location.protocol === "https:" ? "wss:" : "ws:";
  return `${proto}//${location.host}${path}`;
}
const WS_DISPO = typeof WebSocket !== "undefined";
const urlSubgraphHttp = (store: string, centers: string[], hops: number, limit: number, overlay: string) =>
  `/api/viz/subgraph?${qs({ store, center: centers.join(","), hops, limit, overlay })}`;

/**
 * Streame un subgraph par WebSocket et résout le `JsonGraph` COMPLET à `end`.
 * `onChunk` (optionnel) sert au rendu progressif ; les appelants actuels ne le
 * passent pas → pur échange de transport, rendu inchangé.
 */
export function streamSubgraph(
  store: string, centers: string[], hops: number, limit: number, overlay: string,
  signal?: AbortSignal, onChunk?: (g: JsonGraph) => void,
): Promise<JsonGraph> {
  const params = { store, center: centers.join(","), hops, limit, overlay };
  return new Promise<JsonGraph>((resolve, reject) => {
    let ws: WebSocket;
    try { ws = new WebSocket(wsUrl(`/api/viz/subgraph/stream?${qs(params)}`)); }
    catch (e) { reject(e); return; }
    const nodes: GraphNode[] = [];
    const edges: GraphEdge[] = [];
    let schema = "";
    let stats: JsonGraph["stats"] = { node_count: 0, edge_count: 0, truncated: false };
    let ended = false;
    const cleanup = () => {
      clearTimeout(timer);
      signal?.removeEventListener("abort", onAbort);
      try { ws.close(); } catch { /* déjà fermé */ }
    };
    // Sans première frame sous ce délai, on considère le WS indisponible.
    const timer = setTimeout(() => { if (!ended) { cleanup(); reject(new Error("ws timeout")); } }, 30_000);
    const onAbort = () => { cleanup(); reject(abortError()); };
    if (signal?.aborted) { onAbort(); return; }
    signal?.addEventListener("abort", onAbort, { once: true });
    ws.onmessage = (ev) => {
      let m: any;
      try { m = JSON.parse(ev.data as string); } catch { return; }
      switch (m.t) {
        case "begin": schema = m.schema; stats = m.stats; break;
        case "nodes": for (const n of m.items) nodes.push(n); onChunk?.({ schema, stats, nodes, edges }); break;
        case "edges": for (const e of m.items) edges.push(e); onChunk?.({ schema, stats, nodes, edges }); break;
        case "end": ended = true; cleanup(); resolve({ schema, stats, nodes, edges }); break;
        case "error": cleanup(); reject(new Error(m?.detail?.error || "ws error")); break;
        // live/delta/epoch/heartbeat/resnapshot/bye : réservés au mode `live=1`,
        // ignorés ici (instantané one-shot).
        default: break;
      }
    };
    ws.onerror = () => { if (!ended) { cleanup(); reject(new Error("ws error")); } };
    ws.onclose = () => { if (!ended) { cleanup(); reject(new Error("ws closed before end")); } };
  });
}

/** WS d'abord, HTTP ensuite — le chemin HTTP reste le filet, jamais l'inverse. */
const subgraphParWsOuHttp = async (store: string, centers: string[], hops: number, limit: number,
  overlay: string, signal?: AbortSignal): Promise<JsonGraph> => {
  const parHttp = () => get<JsonGraph>(urlSubgraphHttp(store, centers, hops, limit, overlay), signal);
  if (!WS_DISPO) return parHttp();
  try {
    return await streamSubgraph(store, centers, hops, limit, overlay, signal);
  } catch (e) {
    if (isAbort(e)) throw e; // annulation volontaire : ne pas retomber
    return parHttp();        // repli HTTP+gzip transparent
  }
};

export const fetchSubgraph = (store: string, centers: string[], hops: number, limit: number, overlay: string, signal?: AbortSignal) =>
  getCached<JsonGraph>(
    urlSubgraphHttp(store, centers, hops, limit, overlay),
    30_000,
    signal,
    (s) => subgraphParWsOuHttp(store, centers, hops, limit, overlay, s),
  );

// ── RECHERCHE DE FONCTIONS : le seul appel par FRAPPE ────────────────────────
// DÉFAUTS MESURÉS : ni debounce ni cache. Un `q` neuf coûte 77 ms de moteur, mais
// 1 250 ms quand la construction de l'index de trigrammes tombe sur cet appel ;
// et il partait à CHAQUE frappe, pour deux composants (barre de commandes et
// recherche de centre) qui retapaient les mêmes préfixes.
//   · cache par `store|q` (60 s) : retaper un préfixe déjà vu est gratuit, et
//     les deux composants partagent le même résultat ;
//   · debounce 120 ms : coalesce une rafale (frappe rapide) en UN appel ;
//   · une attente abandonnée (AbortSignal du demandeur) ne tue PAS l'appel
//     partagé : elle se décompte, et le lot planifié est annulé si plus personne
//     n'attend — c'est ainsi qu'une rafale de frappes ne produit pas N appels.
const FN_CACHE = new Map<string, { t: number; v: { items: FunctionHit[] } }>();
const FN_TTL = 60_000;
const FN_DEBOUNCE_MS = 120;
interface LotFn { p: Promise<{ items: FunctionHit[] }>; timer: ReturnType<typeof setTimeout> | null; attente: number }
const FN_PEND = new Map<string, LotFn>();

export function searchFunctions(store: string, q: string, signal?: AbortSignal): Promise<{ items: FunctionHit[] }> {  const key = `${store}\u0001${q}`;
  const hit = FN_CACHE.get(key);
  if (hit && Date.now() - hit.t < FN_TTL) return avecSignal(Promise.resolve(hit.v), signal);

  let lot = FN_PEND.get(key);
  if (!lot) {
    const entry: LotFn = { p: null as unknown as Promise<{ items: FunctionHit[] }>, timer: null, attente: 0 };
    entry.p = new Promise<{ items: FunctionHit[] }>((res, rej) => {
      entry.timer = setTimeout(() => {
        entry.timer = null;
        if (entry.attente <= 0) { FN_PEND.delete(key); rej(abortError()); return; }
        // Sans `signal` : l'appel est PARTAGÉ, il ne doit pas mourir avec le
        // premier demandeur qui renonce.
        fetch(`/api/functions?${qs({ store, q, limit: 30 })}`)
          .then(async (r) => {
            if (!r.ok) throw new Error((await r.text()).slice(0, 300));
            const v = await r.json() as { items: FunctionHit[] };
            FN_CACHE.set(key, { t: Date.now(), v });
            FN_PEND.delete(key);
            res(v);
          })
          .catch((e) => { FN_PEND.delete(key); rej(e); });
      }, FN_DEBOUNCE_MS);
    });
    FN_PEND.set(key, entry);
    lot = entry;
  }
  const courant = lot;
  courant.attente++;
  return avecSignal(courant.p, signal, () => {
    courant.attente--;
    if (courant.attente <= 0 && courant.timer) {  // plus personne n'attend
      clearTimeout(courant.timer);
      courant.timer = null;
      FN_PEND.delete(key);
    }
  });
}
/**
 * ── UNE ANCRE PAR DÉFAUT EST UN NOM NU : LA RÉSOUDRE EN qn ──────────────────
 *
 * DÉFAUT MESURÉ LE 2026-09-15 sur le service 8432 : `App.defaultsFor` ancre les
 * stores non-runtime sur un **nom nu** (`e1000_probe`, `tcp_v4_rcv`,
 * `substitute_in_operator`). Or la lignée SERVIE résout un centre en STRICT —
 * id numérique ou `qualified_name` EXACT — et répond sinon
 * `400 {"error":"centre introuvable: e1000_probe"}` (décision NATIF-J5C, qui
 * évite de résoudre un centre absent sur un nœud arbitraire). Résultat : choisir
 * « Linux e1000 », « Linux complet » ou « Obrain (rust) » dans le dashboard
 * laissait la scène VIDE, sans rien dire. La lignée du tronc, elle, a un repli
 * par suffixe : le même code marchait sur un binaire, pas sur l'autre.
 *
 * Le remède est côté CLIENT — le serveur reste strict, et l'ancre devient
 * exacte : on résout le nom en qn par la recherche (`/api/functions`, dont les
 * paliers sont qn exact → nom → suffixe → contient). En cas d'échec on rend le
 * nom inchangé : un binaire au repli permissif le résoudra encore.
 */
export async function resoudreAncres(store: string, noms: string[]): Promise<string[]> {
  const out: string[] = [];
  for (const n of noms) {
    // Déjà un identifiant : un id numérique ou un qn (qui contient un point).
    if (/^\d+$/.test(n) || n.includes(".")) { out.push(n); continue; }
    try {
      const r = await searchFunctions(store, n);
      const exact = r.items.find((h) => h.name === n) || r.items[0];
      out.push(exact?.qn || n);
    } catch {
      out.push(n);
    }
  }
  return out;
}
export const fetchModules = (store: string, signal?: AbortSignal) =>
  get<{ modules: { name: string; count: number }[]; rank?: number }>(
    "/api/viz/modules?store=" + encodeURIComponent(store) + "&top=200", signal);
export const fetchSignatures = () =>
  getCached<{ stores: { label: string; path: string; nodes: number | null; edges: number | null; by_label: { label: string; count: number }[] }[] }>("/api/viz/signatures");
export const fetchOverview = (store: string, signal?: AbortSignal) =>
  get<any>("/api/viz/overview?store=" + encodeURIComponent(store), signal);

/**
 * REQUÊTE AU MOTEUR — avec un cache de RÉSULTATS (20 s), pas un single-flight.
 *
 * DÉFAUT MESURÉ : `runQuery` passait par `get` NU — ni cache, ni déduplication —
 * alors que c'est l'appel le plus cher du paquet et qu'il est déclenché par trois
 * chemins (bouton « Projeter », barre de commandes, `fetchLabelPairs`). La même
 * requête relancée (re-clic, retour sur un échantillon, remontage de composant)
 * repayait tout le moteur.
 *
 * POURQUOI PAS `getCached` : il partage une promesse par URL et n'y fait pas
 * entrer le `signal` — une annulation rejetterait la promesse PARTAGÉE, donc
 * l'annulation d'un demandeur ferait échouer les autres. Ici chaque demandeur
 * garde SA requête (et SON annulation) ; seul le RÉSULTAT déjà obtenu est
 * partagé, et seulement s'il n'est pas annulé.
 */
const QUERY_CACHE = new Map<string, { t: number; v: any }>();
const QUERY_TTL_MS = 20_000;
/** Plafond : un résultat moteur peut porter 413 985 lignes (corpus runtime-dev). */
const QUERY_CACHE_MAX = 16;
export const runQuery = (store: string, query: string, lang: string, signal?: AbortSignal): Promise<any> => {
  const key = `${store}\u0001${lang}\u0001${query}`;
  const hit = QUERY_CACHE.get(key);
  if (hit && Date.now() - hit.t < QUERY_TTL_MS) {
    // Un demandeur déjà annulé ne consomme pas le cache : il doit voir son
    // annulation, comme s'il avait attendu le réseau.
    if (signal?.aborted) return Promise.reject(abortError());
    return Promise.resolve(hit.v);
  }
  return get<any>(`/api/query?${qs({ store, query, language: lang })}`, signal).then((v) => {
    QUERY_CACHE.set(key, { t: Date.now(), v });
    while (QUERY_CACHE.size > QUERY_CACHE_MAX) {
      const plus_vieille = QUERY_CACHE.keys().next().value as string | undefined;
      if (plus_vieille === undefined) break;
      QUERY_CACHE.delete(plus_vieille);
    }
    return v;
  });
};

// ── COSMOS : la toile de l'univers (base synaptique durable de l'agent) ──
// Voir docs/ADR-COSMOS-TOILE-2026-09-10.md. Le serveur relit la base
// synaptique persistée (produite par `obrain viz weave`) et y ajoute les
// FOYERS contextuels réels : le centre du cosmos n'est pas fixe.
export interface WeaveWorld {
  qn: string; slug?: string; label: string; path: string;
  nature: "connaissance" | "self" | "utilisateur";
  nodes: number; edges: number; mass: number; gravity: number; orbit: number;
  types?: { label: string; count: number }[];
  /** Signature de flux réelle du monde (marges) — ce que ce monde FAIT. */
  flows?: WeaveFlow[];
}
export interface WeaveFlow {
  src: string; rel: string; dst: string;
  src_count: number; dst_count: number; bound: number;
}
export interface WeaveFlowSample {
  src: string; rel: string; dst: string; count_a: number; count_b: number;
}
export interface WeaveFiber {
  a: string; b: string; weight: number;
  shared?: number; bridged?: number; reinforcement?: number;
  /** Flux de types réellement partagés (preuve P4, parenté de STRUCTURE). */
  flows?: number; flow_sample?: WeaveFlowSample[];
  dated?: number; first_seen?: number | null; last_spike?: number | null;
  sample?: string[];
}
export interface WeaveFocus {
  kind: "instant" | "conversation" | "agent" | "objectif" | "sujet" | "utilisateur" | string;
  qn: string; label: string; detail?: string; world?: string; ts?: number;
}
export interface Weave {
  schema: string; available?: boolean; reason?: string; source?: string;
  self?: { qn: string };
  worlds?: WeaveWorld[]; fibers?: WeaveFiber[];
  foci?: WeaveFocus[]; focus?: { requested?: string; default?: WeaveFocus };
  clock?: { t0: number; t1: number; sessions: number };
  densification?: { t0: number; t1: number; steps: { t: number; fibers: number; weight: number; ratio?: number }[] };
  counts?: Record<string, number>;
  persisted?: { store?: string; writes?: number };
}
/** Lit la toile de l'univers (base synaptique durable + foyers réels). */
export const fetchWeave = (focus?: string, signal?: AbortSignal, frais = false) => {
  // `qs` ne préfixe PAS par « ? » (l'appel d'origine l'ajoutait lui-même) : sans
  // ce « ? », l'URL devenait `/api/viz/weave_=1789…` → 404 → le Cosmos
  // remplaçait la scène par « Cosmos : not found ». Constaté à la capture.
  const q = qs({ focus: focus || "", _: frais ? Date.now() : undefined });
  return getCached<Weave>(`/api/viz/weave${q ? "?" + q : ""}`, frais ? 0 : 30_000, signal);
};

export interface ModuleArc { src: string; dst: string; weight: number }
/** Arcs CALLS inter-modules agrégés par le moteur (poids = nb d'arêtes). */
export const fetchModuleArcs = (store: string, top = 48, limit = 300, signal?: AbortSignal) =>
  get<{ store: string; arcs: ModuleArc[] }>(`/api/viz/arcs?${qs({ store, top, limit })}`, signal)
    .then((j) => j.arcs || []);
/**
 * Sous-espace d'un ensemble (dimension « projets ») : ensembles enfants d'un
 * préfixe de qn + arcs CALLS entre eux. `rank` = index du token enfant
 * (rootRank exposé par /api/viz/modules, +1 par niveau de descente).
 *
 * ── `offset` : LA PAGE SUIVANTE DU MÊME NIVEAU (2026-09-15) ──────────────────
 * La route sert au plus 200 ensembles par page. Sans `offset`, la seule façon de
 * voir les suivants était de DESCENDRE d'un cran — donc de changer d'échelle
 * pour une question (« montre-moi ce qui manque ICI ») qui n'en change pas.
 * `tronque` dit que la page est pleine, donc qu'il y en a d'autres : c'est le
 * seul aveu honnête possible sans une seconde requête de comptage.
 */
export interface EspacePage {
  ensembles: { name: string; count: number }[];
  arcs?: ModuleArc[];
  rank: number;
  /** Le rang de départ de CETTE page (0 pour la première). */
  offset?: number;
  /** La page est PLEINE : le serveur a coupé, la suite est chez lui. */
  tronque?: boolean;
}
export const fetchSpacePage = (
  store: string,
  o: { prefix: string; rank: number; top?: number; offset?: number; links?: boolean; signal?: AbortSignal },
) =>
  get<EspacePage>(
    `/api/viz/space?${qs({
      store, prefix: o.prefix, rank: o.rank,
      top: o.top, offset: o.offset, links: (o.links ?? true) ? 1 : 0,
    })}`,
    o.signal,
  );
/** Même route, forme historique (positionnelle) — une seule implémentation. */
export const fetchSpace = (store: string, prefix: string, rank: number, top = 60, signal?: AbortSignal, links = true) =>
  fetchSpacePage(store, { prefix, rank, top, links, signal });
/**
 * Agrégation label→label (toutes arêtes) : pondérations fiables pour des
 * ensembles « familles » quand les arcs inter-modules n'existent pas. Le
 * moteur peut émettre des lignes dupliquées pour une même paire → fusion.
 *
 * ⚠️ NOTE (2026-09-10, mesuré) — la requête ci-dessous repose sur
 * `labels(a)[0]` ET `labels(b)[0]` dans un même GROUP BY, forme dont il est
 * établi qu'elle est FAUSSE : le label de la cible est recopié dans celui de
 * la source. Vérifié sur trois stores — `Turn-[HAS_STEP]->Step` devient
 * `Step,Step` ; `Message-[PROVES]->Task` devient `Task,Task` ; avec les
 * formes ancrées `(a:Message)-[r:PROVES]->(b)` la lecture redevient correcte.
 * Conséquence directe ici : `same[s]` capte presque tout et `cross` est
 * quasi vide (seules les vraies boucles apparaissent). La forme fiable est
 * celle utilisée par `obrain viz weave` (marges sur `labels(a)[0]` SEUL) ;
 * cette fonction reste donc à reprendre — voir
 * docs/ADR-COSMOS-TOILE-2026-09-10.md § « Ce que le moteur ne sait pas encore
 * lire ». Aucune donnée ci-dessous n'est fiable pour le cross-label tant que
 * ce n'est pas corrigé.
 */
export async function fetchLabelPairs(store: string, signal?: AbortSignal): Promise<{ same: Record<string, number>; cross: ModuleArc[] }> {
  const r = await runQuery(
    store,
    "MATCH (a)-[r]->(b) RETURN labels(a)[0] AS s, labels(b)[0] AS t, count(r) AS w ORDER BY w DESC LIMIT 900",
    "gql",
    signal,
  );
  const same: Record<string, number> = {};
  const acc = new Map<string, number>();
  for (const row of r.rows || []) {
    const [s, t, w] = row as [string, string, number];
    const ww = typeof w === "number" ? w : Number(w);
    if (s === t) same[s] = (same[s] || 0) + ww;
    else {
      const k = `${s}\u0001${t}`;
      acc.set(k, (acc.get(k) || 0) + ww);
    }
  }
  const cross = [...acc.entries()]
    .map(([k, weight]) => { const [src, dst] = k.split("\u0001"); return { src, dst, weight }; })
    .filter((a) => a.src && a.dst && a.weight >= 20)
    .sort((a, b) => b.weight - a.weight);
  return { same, cross };
}

// ── ENTRETIEN DÛ : les réparations attachées en vol ─────────────────────────
// Le protocole a trois temps : le producteur attache (pendant que la base
// tourne), le consommateur applique et accorde (au redémarrage), et l'interface
// le MONTRE. Une réparation due depuis trois jours doit se voir.
export interface ReparationDue {
  store: string; store_court: string; genre: string;
  raison: string; source: string; t: number; age_s: number;
}
export interface Entretien {
  en_attente: number; age_max_s: number | null;
  demandes: ReparationDue[]; appliquer: string;
}
export const fetchEntretien = (signal?: AbortSignal) =>
  get<Entretien>("/api/entretien", signal);

// ── ACTIVATION EN DIRECT : ce que l'engine vient de consulter ────────────────
// Le moteur obrain (serveur viz ou serveur MCP) inscrit chaque consultation
// dans un journal partagé ; ce endpoint le relit avec son énergie décrue.
// L'activation n'est donc pas décorative : elle reflète des lectures réelles.
export interface ActivationHit {
  store: string; qn: string; kind: string; t: number; energy: number;
}
export interface Activation {
  now: number; half_life_ms: number;
  /** Énergie par monde (pic) : c'est ce qui allume une planète. */
  worlds: { store: string; peak: number; nodes: number }[];
  hits: ActivationHit[];
}
/**
 * Sans `store`, agrège TOUS les mondes (le Cosmos les montre tous).
 *
 * TTL 900 ms — et non 0 comme avant. DÉFAUT MESURÉ : un TTL de 0 désactivait de
 * fait `getCached` (le test `Date.now() - t < 0` est toujours faux) tout en
 * gardant le single-flight : le cache était un mensonge et chaque consommateur
 * repayait l'appel. 900 ms est choisi SOUS l'intervalle de sondage de
 * `useActivation` (1 200 ms) : chaque tour de sondage lit donc frais (le
 * graphique reste vivant), mais deux composants montés en même temps (Cosmos et
 * Atlas) partagent une seule réponse au lieu de deux.
 */
export const fetchActivation = (store?: string, signal?: AbortSignal) =>
  getCached<Activation>(`/api/viz/activation${store ? "?" + qs({ store }) : ""}`, 900, signal);

// ── TIMELINE D'UNE SESSION — le fil d'exécution réel (prompts, appels d'outils,
// résultats, tokens) tel que le graphe l'a enregistré, ordonné par `seq`. ─────
export interface TimelineEvent {
  seq: number;
  kind: "message" | "appel" | "resultat";
  role?: string;
  text?: string;
  name?: string;
  args?: string;
  callId?: string;
  turn?: number;
  step?: number;
  tokens?: { in: number; out: number; cache: number };
}
export interface Timeline {
  session: string;
  compte: string | null;
  evenements: TimelineEvent[];
}
/** Le fil d'une session, fusionné (messages + appels + résultats) et trié. */
export const fetchTimeline = (session: string, store?: string, signal?: AbortSignal) =>
  get<Timeline>(`/api/viz/timeline?${qs({ session, store })}`, signal);

// ── ENGINE : un LLM qui raisonne sur le graphe ───────────────────────────────
// Le contexte n'est pas saisi : il vient du SUJET partagé (ce que l'utilisateur
// vise). Le serveur extrait le voisinage RÉEL du sujet, l'inscrit au journal
// d'activation (le graphe s'allume) puis interroge un LLM local.
export interface EngineAnswer {
  question: string; reponse: string; modele: string; sujet: string;
  depuis_raisonnement: boolean;
  noeuds_contexte: number; relations_contexte: number;
  ms_moteur: number; ms_llm: number; contexte: string;
  error?: string;
}
/** Le service LLM local répond-il ? (diagnostic, dit POURQUOI sinon) */
export const fetchEngineHealth = (signal?: AbortSignal) =>
  getCached<{ disponible: boolean; url: string; modele?: string; raison?: string }>(
    "/api/engine/sante", 10_000, signal,
  );
/** Interroge l'engine sur le sujet visé. Peut être lent (modèle local). */
export const askEngine = (store: string, qn: string, q: string, maxTokens = 400, signal?: AbortSignal) =>
  get<EngineAnswer>(`/api/engine/ask?${qs({ store, qn, q, max_tokens: maxTokens })}`, signal);

// ── MONDES : ajouter un dépôt GitHub comme nouveau monde ─────────────────────
// L'ajout est une MUTATION → POST (un GET qui modifie l'état serait exécuté par
// un simple préchargement). Le clone et l'indexation durent de quelques
// secondes à plusieurs minutes : le serveur rend un identifiant de travail, on
// suit l'avancement.
export interface WorldJob {
  id: string; url: string; label: string;
  /** `enrichissement` est une étape À PART ENTIÈRE : le monde existe après
   *  l'indexation, mais il n'a pas encore son relief (courbure, hubs,
   *  trigrammes). La confondre avec « fini » était le mensonge corrigé le
   *  2026-09-15 : l'écran annonçait « monde ajouté » sur une base brute. */
  etat: "clone" | "indexation" | "enrichissement" | "fini" | "erreur";
  etape: string; store?: string | null; depot?: string | null;
  debut_ms: number; fin_ms?: number | null;
  erreur?: string | null; noeuds?: number | null; aretes?: number | null;
  /** Durée de la seule passe d'enrichissement, en millisecondes. */
  enrich_ms?: number | null;
  aretes_visitees?: number | null;
  aretes_curvature?: number | null;
  courbure_max?: number | null;
  hubs?: number | null;
  hubs_refus?: string | null;
  trigrammes?: string[] | null;
  /** Ce qui a échoué pendant l'enrichissement, nommé. Vide = complet. */
  enrich_erreurs?: string[];
}
export interface WorldsList {
  racine: string;
  mondes: {
    label: string; path: string; nodes: number; edges: number; ajoute_a_chaud: boolean;
    /** L'identité qui a ajouté ce monde (`utilisateur@compte`), si connue. */
    proprietaire?: string | null;
  }[];
  travaux: WorldJob[];
}
export const fetchWorlds = (signal?: AbortSignal) =>
  get<WorldsList>("/api/worlds", signal);
export async function addWorld(url: string, label?: string, signal?: AbortSignal): Promise<{ job?: string; error?: string }> {
  const r = await fetch("/api/worlds", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ url, label: label || "" }),
    signal,
  });
  return r.json();
}
export const worldJob = (job: string, signal?: AbortSignal) =>
  get<WorldJob>(`/api/worlds/status?${qs({ job })}`, signal);

/* ── LES RAMIFICATIONS D'UN SUJET (lignée du tronc, réunifiée ici) ───────────
 *
 * `api.ts` est le point de passage que DEUX lignées ont modifié : celle du tronc
 * (cache de session, ramifications) et celle servie (transport WebSocket avec
 * repli HTTP, pagination de niveau). La réunification se fait dans CE fichier et
 * non à sa place : un portage brut d'un côté aurait supprimé l'apport de l'autre
 * sans que rien ne le dise.
 *
 * `SubjectLine.tsx` vient du tronc et appelle `fetchRamifications` : sans ce
 * bloc, la réunification produirait un tableau de bord qui ne compile pas.
 */
export interface Ramifications {
  centres: string[];
  labels: string[];
  sortantes: { type: string; nb: number }[];
  entrantes: { type: string; nb: number }[];
  a_construire: {
    sortantes: { type: string; indice_store: number }[];
    entrantes: { type: string; indice_store: number }[];
  };
}
export const fetchRamifications = (store: string, centers: string[], signal?: AbortSignal) =>
  getCached<Ramifications>(
    `/api/viz/ramifications?${qs({ store, center: centers.join(",") })}`,
    10_000,
    signal,
  );
