import { atom } from "jotai";
import type { JsonGraph } from "../lib/api";

export interface StorePreset { label: string; path: string; monde_ajoute?: boolean }
export interface StoresInfo {
  default: string;
  /** Les bases que l'appelant peut OUVRIR — le serveur les filtre selon ses
   *  droits (`/api/stores`). Une base absente d'ici est une base refusée. */
  presets: StorePreset[];
  /** Le compte connecté, quand l'IAM est armé. Son absence veut dire « pas de
   *  réponse du serveur », pas « aucun droit » : c'est ce témoin qui empêche
   *  d'afficher « aucune base » pendant le chargement. */
  compte?: string | null;
  /** Les espaces accordés à ce compte. */
  espaces?: string[];
  /** Les bases qui EXISTENT mais ne sont pas ouvertes à ce compte. */
  refuses?: string[] | null;
  /** Quoi faire quand il n'y a rien à lire. */
  aide?: string;
  /** Le défaut EFFECTIF pour cet appelant : le premier preset qu'il peut lire,
   *  jamais le défaut global du serveur (qui lui serait refusé). */
  path_default?: string | null;
}

export const storesAtom = atom<StoresInfo>({ default: "", presets: [] });
export const activeStoreAtom = atom<string>("");       // chemin actif
export const centersAtom = atom<string[]>([]);          // choix multiples
export const hopsAtom = atom<number>(1);                // 0 = arbre complet
export const overlayAtom = atom<string>("");            // ricci|roles|energy|…
export const limitAtom = atom<number>(160);
export const layersAtom = atom<Record<string, boolean>>({
  edges: true, tri: true, k4: true, hyper: true,
  contour: true, spectral: true, synapse: true, labels: true, rotate: false,
});
export const requestTokenAtom = atom<number>(0);        // incrément = (re)charger
export interface Anchor { centers: string[]; hops: number; label: string }
export const historyAtom = atom<Anchor[]>([]);            // fil d'Ariane multi-échelle
export const lastGraphAtom = atom<JsonGraph | null>(null);
export const statsAtom = atom<string>("—");
export const selectedNodeAtom = atom<string | null>(null);
export const morphModeAtom = atom<boolean>(true); // même substrat, 3 projections
export const organicModeAtom = atom<boolean>(false); // vue « Organique » : halos, synapses vives
// `cosmosModeAtom` et `mondeModeAtom` ne sont PLUS des autorités : deux booléens
// pour une seule face, c'était un état impossible à tenir (cosmos ET monde vrais,
// ou aucun des deux). Ils sont désormais DÉRIVÉS de `faceAtom` — les composants
// qui les lisaient continuent de fonctionner, et plus personne ne peut les
// écrire : une seconde autorité ne peut pas naître par accident.
export const suggestionsAtom = atom<string[]>([]);   // candidats proposés en cas d'ambiguïté

// ── console « questions au moteur » ──
export interface QueryResult { columns: string[]; rows: unknown[][]; error?: string; execution_time_ms?: number }
export const queryTextAtom = atom("MATCH (n) RETURN labels(n)[0] AS type, count(n) AS nbr ORDER BY nbr DESC LIMIT 8");
export const queryLangAtom = atom("gql");
export const queryResultAtom = atom<QueryResult | null>(null);
export const queryBusyAtom = atom(false);

// ── UX « canvas-first » (2026-09-10) : la représentation reste maîtresse,
//    les réglages vivent dans un inspecteur escamotable + barre de commandes ──
export const inspectorOpenAtom = atom<boolean>(false); // panneau config droit (⌘K / ⚙)
export const projectionVisibleAtom = atom<boolean>(true); // rendre la projection du résultat
/**
 * LE GARDE-FOU DE VOLUME DE LA PROJECTION (2026-09-13).
 *
 * Défaut mesuré : une requête sans borne (le serveur n'en pose aucune :
 * `/api/query` sérialise `r.rows` tel quel, et le corpus servi fait 413 985
 * nœuds) faisait construire des axes complets — 8 000 groupes mesurés sur
 * 8 000 lignes — pour n'en afficher que 16, et levait `RangeError` vers
 * 125 000 lignes. La projection ne doit donc pas se lancer TOUTE SEULE
 * au-delà du seuil (`SEUIL_PROJECTION_ROWS`, voir lib/projection.ts) : ce
 * qu'elle refuse est un fait partagé par les deux points d'entrée (le bouton
 * « Projeter » de l'Inspector et la barre de commandes d'`App`), donc il vit
 * dans un atom et non dans l'état local d'un composant.
 *
 * `null` = aucune raison de refuser (soit peu de lignes, soit l'utilisateur a
 * répondu « projeter quand même », ce qui remet cet atom à `null`).
 */
export const projectionRefusAtom = atom<{ rows: number } | null>(null);
// `commandModeAtom` a été RETIRÉ (2026-09-13) : écrit par `App` en deux endroits
// (« ask » par défaut, « query » après une requête GQL) et lu par PERSONNE —
// vérifié par recherche exhaustive. Il prétendait décrire un mode de la barre de
// commandes qui n'existe pas dans l'affichage. Même règle que `loadingMsAtom` et
// `lastResolvedAtom` ci-dessous : un atom sans lecteur est une invitation à
// écrire dedans par erreur.
// `loadingMsAtom` et `lastResolvedAtom` ont été RETIRÉS : aucun lecteur ni
// écrivain dans tout le paquet (vérifié par recherche exhaustive). Un atom que
// personne ne lit est une invitation à écrire dedans par erreur.

// ── LE SUJET DE LA PIÈCE (2026-09-10) ────────────────────────────────────────
// « Ce sont toutes les faces d'une même pièce. » Les vues partageaient déjà la
// grammaire, le relief et le chrome — mais chacune gardait SON état : on
// pouvait viser un monde dans le Cosmos et arriver dans le Graphe sur autre
// chose. Ce sujet est l'état unique de la pièce : chaque face le LIT, et
// l'écrit quand l'utilisateur agit. Changer de face ne change donc plus ce
// qu'on regarde.
export interface Subject {
  /** Base obrain concernée (une face peut en changer). */
  store: string;
  /** Entité visée : `qualified_name` d'un nœud, ou `world:<slug>`. */
  qn: string;
  /** Nom lisible, affiché par toutes les faces. */
  label: string;
  /** Par quelle face le sujet a été posé (traçabilité, jamais une priorité). */
  via: "cosmos" | "atlas" | "monde" | "graphe" | "";
}
// ── LE CHAT S'OUVRE PAR UN ÉTAT PARTAGÉ, PAS PAR UNE PROPRIÉTÉ ─────────────
// Le bouton « conversation plein écran » vit dans un panneau du canvas, mais le
// chat, lui, est monté par `App`. Sans cet atome, le canvas devrait connaître le
// chat pour l'ouvrir — c'est-à-dire le contenir de nouveau, et recreuser le
// défaut qu'on vient de corriger (`ChatPleinEcran` monté DANS la scène WebGL).
// L'état d'ouverture appartient donc à l'application, et la scène se contente de
// le DEMANDER.
// ── FERMÉE AU CHARGEMENT (corrigé le 2026-09-15) ─────────────────────────────
// Défaut mesuré : `atom(true)` ouvrait la conversation À L'ARRIVÉE. Conséquence
// observée par l'utilisateur — et rapportée ainsi : « la vue des comptes arrive
// sur le champ de saisie » — le graphe n'était jamais visible au chargement, et
// ce qui restait dessous (barre d'onglets, panneau des gens) se retrouvait
// derrière une conversation qu'on n'avait pas demandée. Une interface ne
// s'ouvre pas sur un panneau modal : elle s'ouvre sur ce qu'elle sert à
// regarder. Le chat s'ouvre quand on le demande (bouton 💬, ou le panneau du
// Cosmos qui le demande via cet atome).
//
// ── IL NE S'OUVRE PAS PAR-DESSUS LE CANEVAS (rapatrié le 2026-09-16) ────────
//
// Le défaut tenait au DÉFAUT DE VALEUR : ouvert au chargement, `.chat-plein`
// occupe 1094×696 en z-index 60 et REÇOIT TOUS LES CLICS au centre de la vue —
// `elementFromPoint(720,420)` rendait `DIV.chat-corps`. Mesuré sur le build
// servi : la molette ne descendait plus (0 % → 0 %), le bouton « ⤓ descendre »
// était recouvert (le clic partait au chat), la barre d'échelle était derrière,
// et le contrôle de navigation tombait sur 5 exigences au lieu de 0.
//
// Le principe de cette vue est « le canevas d'abord » : la conversation reste à
// UN CLIC (bouton flottant 💬 et bouton du panneau), elle ne s'installe
// simplement plus devant la navigation au chargement.
//
// La correction existait dans la lignée `union/nav-et-rendu`, sous la forme d'un
// `useState(false)` DANS le composant. Elle est portée ici sur l'ATOME, qui est
// devenu la seule autorité depuis que l'état a quitté `CosmosWeave` : reprendre
// le composant tel quel aurait recréé une seconde déclaration d'état, et la
// fusion aurait rendu la main au défaut qu'`union/nav-et-rendu` avait corrigé.
//
// ── FUSION DES DEUX LIGNÉES : deux corrections indépendantes du même défaut ──
// Les deux côtés ont conclu la MÊME valeur (`false`) par deux mesures
// DIFFÉRENTES et toutes deux valables : côté canonique, la conversation
// s'ouvrait devant le graphe à l'arrivée ; côté multi-écrivain, elle
// interceptait les clics au centre du canevas (`elementFromPoint`). Les deux
// constats sont conservés ci-dessus — ils décrivent le même défaut sous deux
// angles, et la valeur finale est celle qu'ils demandent tous les deux.
export const chatOuvertAtom = atom(false);

export const subjectAtom = atom<Subject>({ store: "", qn: "", label: "", via: "" });

/**
 * LE SEUIL DE DÉPLOIEMENT DES BRANCHES (θ) — une grandeur, un atom.
 *
 * Défaut mesuré : θ existait en DEUX `useState` indépendants — `theta` dans
 * l'Atlas (`StageContinu`) et `thetaBranch` + `thetaMode` dans le Monde
 * (`StoreOverview`) — avec les mêmes valeurs `[0, 0.01, 0.05, 0.2]` et le même
 * sens. Régler le seuil dans une face ne se retrouvait donc pas dans l'autre, et
 * rien ne le signalait : c'est la désynchronisation la plus discrète du paquet,
 * parce que les deux valeurs restent PLAUSIBLES séparément.
 *
 * `mode` dit sur quoi porte le seuil : la part d'items d'une branche, ou le
 * poids de ses liens (degré). Les deux faces partagent désormais les deux.
 */
export interface Seuil {
  part: number;
  mode: "part" | "poids";
}
export const seuilAtom = atom<Seuil>({ part: 0, mode: "part" });

// ═══════════════════════════════════════════════════════════════════════════════
// L'ANCRAGE : UNE SEULE PORTE POUR « MONTRE-MOI CECI » (2026-09-11)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Défaut mesuré dans l'état précédent : viser un centre demandait d'écrire TROIS
// atoms (`centersAtom`, `historyAtom`, `requestTokenAtom`) et de les garder
// cohérents À LA MAIN, à neuf endroits. Deux conséquences, toutes deux
// observées :
//
//   · le fil d'ancrage affiché (`historyAtom`) et le centre RÉELLEMENT chargé
//     (`centersAtom`) divergaient — `FacetBar` et `CenterSearch` posaient l'un
//     sans l'autre, donc le fil montrait un chemin que la vue n'avait pas suivi ;
//   · un oubli de `bump()` ne rechargeait RIEN, sans erreur : la vue restait
//     figée et rien ne le disait.
//
// Une seule porte supprime les deux : écrire un centre, c'est désormais écrire
// ICI, et le rechargement comme le fil en DÉCOULENT. C'est la différence entre
// « synchronisé » et « calculé ».
export interface Ancrage {
  centers: string[];
  hops?: number;
  label?: string;
  /** `true` (défaut) : le fil s'allonge. `false` : on ne fait que recharger. */
  empiler?: boolean;
}
/**
 * Recharge le graphe du périmètre courant SANS toucher au fil — le geste « ↻ »,
 * et tout ce qui change une couche d'affichage (overlay, strates, organic).
 */
export const rechargerAtom = atom(null, (get, set) => {
  void get;
  set(requestTokenAtom, (x) => x + 1);
});

/**
 * Vise un centre : périmètre, fil et rechargement dans la MÊME écriture.
 * `null` = on vide le périmètre (le fil repart de zéro).
 */
export const ancrerAtom = atom(null, (get, set, vise: Ancrage | null) => {
  if (!vise) {
    set(centersAtom, []);
    set(historyAtom, []);
    set(selectedNodeAtom, null);
    set(requestTokenAtom, (x) => x + 1);
    return;
  }
  set(centersAtom, vise.centers);
  if (vise.hops !== undefined) set(hopsAtom, vise.hops);
  const label = vise.label ?? vise.centers[0] ?? "";
  if (vise.empiler !== false) {
    set(historyAtom, (h) => [...h, { centers: vise.centers, hops: vise.hops ?? get(hopsAtom), label }]);
  } else if (vise.centers.length) {
    // Remplacement du dernier pas plutôt qu'un pas de plus : on corrige la
    // destination sans allonger un chemin qui n'a pas été parcouru.
    set(historyAtom, (h) => (h.length
      ? [...h.slice(0, -1), { centers: vise.centers, hops: vise.hops ?? get(hopsAtom), label }]
      : [{ centers: vise.centers, hops: vise.hops ?? get(hopsAtom), label }]));
  }
  set(selectedNodeAtom, null);
  set(suggestionsAtom, []);
  set(requestTokenAtom, (x) => x + 1);
});

// ── RAFRAÎCHISSEMENT DE LA TOILE ─────────────────────────────────────────────
// Ajouter un monde doit le faire APPARAÎTRE. La toile est un artefact persisté
// et mise en cache 30 s : sans jeton, l'utilisateur ajouterait un dépôt et ne
// verrait rien avant une demi-minute. Incrémenter ce jeton force la relecture.
export const weaveTokenAtom = atom<number>(0);

// ═══════════════════════════════════════════════════════════════════════════════
// L'ÉTAT D'INTERFACE : UNE GRANDEUR, UN ATOM (2026-09-11)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Défaut mesuré avant cette refonte : trois « niveaux » coexistaient pour une
// seule réalité — `niveauCosmos` (demande) et `niveauAffiche` (rapport) vivaient
// dans les `useState` de `App`, pendant que la scène gardait `niveauScene` de son
// côté. Le fil des poupées lisait l'un, le sélecteur de vues l'autre : deux
// affichages pouvaient donc nommer deux étages différents au même instant, et
// rien ne pouvait le détecter. De même, la face courante était portée par
// QUATRE booléens (`cosmosModeAtom`, `mondeModeAtom`, `morphModeAtom`, plus le
// mode organique) dont la priorité n'existait que dans un ternaire d'`App` :
// écrire deux booléens dans le mauvais ordre produisait un état impossible.
//
// Règle d'ici : *une grandeur, un atom*, et tout ce qui s'affiche en DÉCOULE
// (atoms dérivés) au lieu d'être recopié. Un composant ne peut plus montrer
// autre chose que ce que l'état dit, parce qu'il n'a pas d'autre source.

/**
 * LA FACE COURANTE — autorité unique.
 *
 * `cosmos` (toutes les bases déployées), `monde` (organisation des
 * connaissances), `graphe` (formes : K3/K4/∂/synapses). Les anciens booléens
 * sont DÉRIVÉS de cet atom (plus bas) : ils restent lisibles par les composants
 * qui les consommaient, mais ils ne peuvent plus diverger entre eux.
 */
export type Face = "cosmos" | "monde" | "graphe";
export const faceAtom = atom<Face>("graphe");

/**
 * ── L'AXE DE NAVIGATION, DÉCISION PARTAGÉE (2026-09-15) ──────────────────────
 *
 * « Descendre » n'est qu'un axe. La même matière se découpe aussi par communauté
 * du moteur (Leiden), par contour ∂ (courbure), par parenté, par extension, par
 * strate — et ces découpages sont des relations RÉELLES de la donnée, pas des
 * variantes cosmétiques.
 *
 * L'axe vivait en état LOCAL de `CosmosWeave`, donc hors du document d'état : il
 * ne pouvait être ni partagé, ni rejoué, ni lu par une autre face. Il devient un
 * atome d'AUTORITÉ, au même titre que la face et l'échelle — c'est la même
 * nature de décision : « ce que je regarde, et selon quelle relation ».
 *
 * Les six valeurs sont celles que `etiquetteDimension` sait calculer ; le jour où
 * un type d'entité arrive du moteur, il s'ajoute ici.
 */
export type AxeNavigation = "module" | "parent" | "extension" | "communauté" | "couche" | "∂";
export const axeNavigationAtom = atom<AxeNavigation>("module");

/** La face, en booléens dérivés — lecture seule, jamais une seconde autorité. */
export const estCosmosAtom = atom((get) => get(faceAtom) === "cosmos");
export const estMondeAtom = atom((get) => get(faceAtom) === "monde");
/** Noms historiques, conservés pour les lecteurs existants (dérivés, en lecture). */
export const cosmosModeAtom = estCosmosAtom;
export const mondeModeAtom = estMondeAtom;

/**
 * LE NIVEAU DANS LE COSMOS — la DEMANDE et le RAPPORT, séparés à dessein.
 *
 * `App` documentait déjà pourquoi ils ne doivent pas fusionner : un seul état
 * servant à la fois de demande et d'affichage produisait un asservissement qui
 * RAMENAIT la caméra en arrière (le rapport redevenait une demande, donc un vol
 * se relançait). Ils restent donc deux atoms — mais deux atoms PARTAGÉS : la
 * scène écrit le rapport, les boutons écrivent la demande, et le fil des poupées
 * comme le sélecteur de vues lisent la MÊME vérité.
 *
 * ── ET IL Y A QUATRE CRANS, PAS TROIS (2026-09-15) ──────────────────────────
 * `0 espace · 1 monde · 2 groupe · 3 signaux`. Le quatrième est le FOND de la
 * descente : la matière s'y réorganise selon ses signaux (courbure · énergie ·
 * degré) au lieu de sa disposition de graphe. Le mécanisme existait déjà dans
 * `CosmosWeave` (le curseur `tS`, `posSignaux`) mais AUCUN contrôle n'y menait —
 * et il ne s'exécutait pas non plus, faute d'une origine de disposition conservée
 * (`posGraphe` jamais assignée).
 *
 * On étend CE type plutôt que d'ajouter un cinquième atom : c'est la même
 * échelle, donc la même vérité. Un `niveauSignaux` séparé pourrait diverger du
 * fil des poupées et du sélecteur de vues, qui lisent tous les deux celui-ci.
 */
export type NiveauCosmos = 0 | 1 | 2 | 3;
export const niveauDemandeAtom = atom<NiveauCosmos>(0);
export const niveauAfficheAtom = atom<NiveauCosmos>(0);

// ── L'ÉCRAN, EN UNE SEULE GRANDEUR ───────────────────────────────────────────
// Tout ce qui s'adapte au matériel (surface des menus, taille des cibles
// tactiles, présence d'un clavier) doit se dériver d'ICI. Trois media queries
// éparpillées dans les feuilles de style ne peuvent pas se contredire, mais
// trois conditions JavaScript écrites à trois endroits, si.
export type Ecran = "telephone" | "tablette" | "bureau";
export const ecranAtom = atom<Ecran>("bureau");

/** La classe d'écran, en booléens dérivés (lecture seule). */
export const estTelephoneAtom = atom((get) => get(ecranAtom) === "telephone");
export const estBureauAtom = atom((get) => get(ecranAtom) === "bureau");

// ── LA STRUCTURE DE RÉDUCTION DU CHROME ──────────────────────────────────────
/**
 * Les éléments d'interface posés SUR le canvas, et leur priorité.
 *
 * L'utilisateur a raison sur le fond : « tout consomme de la place sur le
 * Canvas ». La réponse n'est pas de masquer à la main dans chaque composant
 * (c'est ainsi qu'on obtient des affichages qui se contredisent) mais de
 * DÉCLARER la priorité une fois, ici, et de laisser chaque affichage en
 * DÉCOULER. Les quatre niveaux :
 *
 *   1 · navigation   — ne disparaît jamais : sans elle, la vue est un cul-de-sac
 *   2 · essentiel    — masqué seulement si l'on RÉDUIT (geste explicite)
 *   3 · confort      — bureau et tablette ; sur téléphone, il vole le canvas
 *   4 · décoratif    — jamais sur téléphone
 */
export type Cle = "navigation" | "commandes" | "reglages" | "fil" | "zoom"
  | "zoom_avance" | "carte" | "aide"
  | "facettes" | "chat" | "statut" | "ancres" | "legende";
export const PRIORITE: Record<Cle, 1 | 2 | 3 | 4> = {
  navigation: 1,
  commandes: 2,
  reglages: 2,
  fil: 2,
  // Le zoom ESSENTIEL (+ − ⌂) reste en niveau 2 ; les réglages fins passent en
  // niveau 3 : sur téléphone, ils volent 7 × 44 px de hauteur, mesuré — la
  // colonne complète fait 59 % de la hauteur d'un écran de 844 px.
  zoom: 2,
  zoom_avance: 3,
  carte: 3,
  aide: 4,
  // (2026-09-15) Les FACETTES volent 52 px de bas d ecran : confort, pas
  // essentiel — sur telephone elles cedent le canvas. Le CHAT, lui, est LA
  // vue d entree du produit : niveau 1, il ne disparait jamais.
  facettes: 3,
  chat: 1,
  statut: 3,
  ancres: 3,
  legende: 4,
};
/** Décisions explicites de l'utilisateur : elles priment sur la priorité. */
export const chromeAtom = atom<Partial<Record<Cle, boolean>>>({});
/** Le geste « réduire le chrome » : masque tout ce qui n'est pas navigation. */
export const chromeReduitAtom = atom(false);

/**
 * LA RÈGLE D'AFFICHAGE, CALCULÉE — jamais recopiée.
 *
 * Un composant demande `montre("commandes")` ; la réponse dépend de l'écran, du
 * geste de réduction et des décisions explicites, dans cet ordre. Aucun
 * composant ne détient cette logique, donc aucun ne peut la contredire.
 */
export const montreAtom = atom((get) => {
  const ecran = get(ecranAtom);
  const reduit = get(chromeReduitAtom);
  const choix = get(chromeAtom);
  return (cle: Cle): boolean => {
    const explicite = choix[cle];
    if (explicite !== undefined) return explicite;
    const p = PRIORITE[cle];
    if (p === 1) return true;
    if (reduit) return false;
    if (p === 4) return ecran !== "telephone";
    if (p === 3) return ecran !== "telephone";
    return true;
  };
});

/**
 * ── CE QUI RECOUVRE LE CANVAS, EN PIXELS, TRANSMIS À LA SCÈNE ───────────────
 *
 * « Tout consomme de la place sur le Canvas. » Le grief n'est pas seulement que
 * le chrome soit gros : c'est que la SCÈNE n'en sait rien. Elle se centre dans
 * la fenêtre entière, donc sous les panneaux — un monde cadré « au centre »
 * atterrit derrière la barre de commandes, et l'utilisateur recadre à la main.
 *
 * Cette empreinte est DÉRIVÉE de la même règle d'affichage que le chrome
 * lui-même : les deux ne peuvent pas diverger, et une scène peut donc se
 * cadonner dans l'espace réellement visible en lisant un seul atom.
 */
export interface Inset { haut: number; bas: number; droite: number }
export const canvasInsetAtom = atom<Inset>((get) => {
  const montre = get(montreAtom);
  const ecran = get(ecranAtom);
  const petit = ecran === "telephone";
  let haut = 0;
  let bas = 0;
  let droite = 0;
  if (montre("navigation")) haut += petit ? 96 : 56;   // sélecteur de vues + fil
  if (montre("commandes")) haut += petit ? 44 : 40;    // barre de commandes
  if (montre("fil")) haut += petit ? 34 : 30;          // fil des poupées
  if (montre("statut")) bas += 34;
  if (montre("facettes")) bas += 52;
  if (montre("zoom")) droite += petit ? 56 : 48;
  if (montre("reglages")) droite += petit ? 56 : 0;
  return { haut, bas, droite };
});

/** Nombre d'éléments masqués par la réduction — pour le dire à l'écran. */
export const chromeMasqueAtom = atom((get) => {
  const montre = get(montreAtom);
  return (Object.keys(PRIORITE) as Cle[]).filter((c) => !montre(c)).length;
});
