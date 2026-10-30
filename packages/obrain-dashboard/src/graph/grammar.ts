import * as THREE from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";

/**
 * GRAMMAIRE COMMUNE DES VUES — Cosmos · Monde · Graphe (et morph).
 *
 * Constat qui a motivé ce module : les quatre vues apportaient chacune du
 * relief, mais parlaient quatre langues différentes.
 *
 * | vue          | ←/→ | ↑ | ↓ | ⌂ | zoom clavier | double-clic |
 * |--------------|-----|----|----|----|--------------|-------------|
 * | Cosmos       |  ✗  | ✗  | ✗  | ✗  |      ✗       |      ✗      |
 * | Atlas/morph  |  ✓  | ✓  | ✓  | ✓  |      ✗       |      ✗      |
 * | Graphe       |  ✗  | ✗  | ✗  | ✗  |      ✓       |      ✓      |
 * | Monde        |  ✗  | ✗  | ✗  | partiel |   ✓       |      ✓      |
 *
 * Ici vivent désormais : UNE grammaire de touches, UN profil de caméra, UNE
 * palette. Les vues gardent leur relief propre — ce qui s'unifie, c'est la
 * façon de s'y déplacer et de se lire.
 */

// ---------------------------------------------------------------------------
// Palette partagée : les trois plans ont le MÊME sens dans toutes les vues.
// ---------------------------------------------------------------------------

/** Nature d'un monde — le sens est identique partout. */
export type Plan = "connaissance" | "self" | "utilisateur";

export const PLAN_COLOR: Record<Plan, number> = {
  connaissance: 0x4aa8ff,
  self: 0xffd166,
  utilisateur: 0xb07aa1,
};

export const PLAN_CSS: Record<Plan, string> = {
  connaissance: "#4aa8ff",
  self: "#ffd166",
  utilisateur: "#b07aa1",
};

export const PLAN_ORDER: Plan[] = ["connaissance", "self", "utilisateur"];

/**
 * PALETTE DE TYPES — partagée par toutes les vues qui montrent la COMPOSITION
 * d'un monde (Atlas, Cosmos, Monde). Elle vivait en double dans StageContinu et
 * CosmosWeave : deux vues pouvaient donc colorer le même type différemment, ce
 * qui casse la lecture d'une échelle à l'autre.
 */
export const TYPE_PALETTE = [
  "#7fd1ff", "#21d4b4", "#ffd166", "#ff7a5c", "#ff6b9d",
  "#b07aa1", "#ffe066", "#3db4ff", "#5fd68a", "#f5d63d",
] as const;

/** Couleur d'un type selon sa position (même index ⇒ même couleur partout). */
export const typeColor = (i: number): string => TYPE_PALETTE[i % TYPE_PALETTE.length];

/** Couleurs transverses : ce qui relie, ce qui est faible, ce qui est visé. */
export const WEAVE_COLOR = 0x9ffff0;   // influx synaptique
export const KINSHIP_COLOR = 0x3a5f80; // parenté de structure (preuve faible)
export const ACCENT = 0xffe066;        // sélection / foyer
export const ACCENT_CSS = "#ffe066";

// ---------------------------------------------------------------------------
// Profil de caméra unifié
// ---------------------------------------------------------------------------

/**
 * Un seul profil de perspective pour toutes les vues : changer de vue ne doit
 * pas changer la sensation de profondeur. Les vues peuvent resserrer les
 * bornes de distance (échelles différentes), jamais la perspective.
 */
export const CAMERA = {
  fov: 60,
  near: 1,
  far: 12000,
  dampingFactor: 0.08,
  minDistance: 2,
  maxDistance: 5200,
  zoomSpeed: 5,
} as const;

/**
 * Applique le profil commun à un `OrbitControls` : perspective, amorti,
 * gestes tactiles, bornes. `overrides` ne touche QUE les distances.
 */
export function orbitFor(
  camera: THREE.PerspectiveCamera,
  dom: HTMLElement,
  overrides: { minDistance?: number; maxDistance?: number; zoomSpeed?: number } = {},
): OrbitControls {
  const ctrl = new OrbitControls(camera, dom);
  ctrl.enableDamping = true;
  ctrl.dampingFactor = CAMERA.dampingFactor;
  // ── VITESSE DE MOLETTE : LE RÉGLAGE QUI DÉCIDE SI ON PEUT TRAVERSER ──────
  // three.js applique `0,95^zoomSpeed` par cran. À la valeur par défaut (1,0),
  // chaque cran ne change la distance que de 5 % : comme la descente couvre un
  // rapport de 56× (14 rayons → 0,25 rayon), il fallait **78 crans de molette**
  // pour aller de l'espace au dedans. L'utilisateur zoomait sans jamais changer
  // d'échelle — régression constatée à la molette réelle (10 crans → 18 %).
  // Ce n'est pas la descente qui était en cause, c'est le pas du geste.
  ctrl.zoomSpeed = overrides.zoomSpeed ?? CAMERA.zoomSpeed;
  ctrl.minDistance = overrides.minDistance ?? CAMERA.minDistance;
  ctrl.maxDistance = overrides.maxDistance ?? CAMERA.maxDistance;
  return ctrl;
}

/** Distance de zoom la plus proche de la cible, bornée par le contrôle. */
export function zoomBy(camera: THREE.PerspectiveCamera, ctrl: OrbitControls, k: number): void {
  const dir = camera.position.clone().sub(ctrl.target);
  const len = Math.min(ctrl.maxDistance, Math.max(ctrl.minDistance, dir.length() * k));
  camera.position.copy(ctrl.target).add(dir.setLength(len));
  camera.lookAt(ctrl.target);
  ctrl.update();
}

// ---------------------------------------------------------------------------
// Relief partagé : fond, brouillard, rendu
// ---------------------------------------------------------------------------

/**
 * Le RELIEF était lui aussi éclaté — quatre atmosphères pour quatre vues :
 *
 * | vue    | fond      | brouillard    | pixelRatio |
 * |--------|-----------|---------------|------------|
 * | Cosmos | `05070c`  | 2600 → 5400   | 1,5        |
 * | Atlas  | `070a0f`  | 950 → 2600    | 1,5        |
 * | Graphe | `0c1016`  | aucun         | 1,6        |
 * | Monde  | `05070d`  | aucun         | 2          |
 *
 * Changer de vue changeait donc l'atmosphère entière. Un seul fond, un seul
 * plafond de densité de pixels, et un brouillard **exprimé en multiples de la
 * distance de cadrage** de chaque vue — ce qui le rend identique en proportion,
 * donc lisible pareil à toutes les échelles.
 */
export const RELIEF = {
  /** Fond unique de toutes les vues. */
  background: 0x05070c,
  /** Plafond commun de densité de pixels (au-delà, on paie sans voir). */
  pixelRatioCap: 1.6,
  /**
   * Le brouillard commence À 1,5× LA DISTANCE DE CADRAGE : ce qui est dans le
   * cadre n'est jamais voilé, seul le fond profond s'efface — c'est le principe
   * qui a fait corriger le Cosmos, où l'ancienne plage (1400→3400) noyait des
   * objets situés à 1500-2200 du point de vue.
   */
  fogNearFactor: 1.5,
  fogFarFactor: 3.8,
} as const;

/**
 * Applique le fond et le brouillard communs. `framingDistance` = distance
 * typique de la caméra au centre de la scène (la « mise au point » de la vue).
 */
export function applyRelief(scene: THREE.Scene, framingDistance: number): THREE.Fog {
  scene.background = new THREE.Color(RELIEF.background);
  const fog = new THREE.Fog(
    RELIEF.background,
    framingDistance * RELIEF.fogNearFactor,
    framingDistance * RELIEF.fogFarFactor,
  );
  scene.fog = fog;
  return fog;
}

/**
 * GRAMMAIRE D'EMPHASE — combien plus gros est ce qui compte.
 *
 * Les tailles absolues ne peuvent pas être communes : chaque vue a son échelle
 * (un point vaut 1,7 unité dans l'Atlas et 11 dans le Cosmos). Ce qui peut et
 * doit l'être, c'est le RAPPORT : dans toutes les vues, un élément au repos,
 * survolé, déployé ou sélectionné doit se distinguer de la même façon. Mesuré
 * avant unification : l'Atlas utilisait 1,7 / — / 3,1 / 3,4 (rapports 1 /
 * 1,82 / 2,0) et le Cosmos n'avait aucune emphase du tout.
 */
export const EMPHASIS = {
  idle: 1,
  hover: 1.25,
  /** Élément déployé / au foyer. */
  active: 1.8,
  /** Élément explicitement sélectionné. */
  selected: 2,
} as const;

/** Applique la grammaire d'emphase à une taille de base propre à la vue. */
export function emphasis(base: number, level: keyof typeof EMPHASIS): number {
  return base * EMPHASIS[level];
}

/**
 * LOD partagé : nombre d'éléments étiquetés selon la distance caméra. Les vues
 * qui ont un budget d'étiquettes doivent le réduire de la même façon en
 * s'éloignant, sinon le même éloignement allège une vue et pas l'autre.
 */
export const LOD = {
  /** Paliers de proximité (levier du « slide cursor ») → budget d'étiquettes. */
  budgets: [36, 90, 170, 260],
  /** Un survol ou une sélection garde toujours son étiquette. */
  keepSelected: true,
} as const;

/** Rend un moteur WebGL avec les réglages communs (antialias, densité bornée). */
export function makeRenderer(w: number, h: number): THREE.WebGLRenderer {
  const r = new THREE.WebGLRenderer({
    antialias: true,
    powerPreference: "high-performance",
  });
  r.setPixelRatio(Math.min(globalThis.devicePixelRatio || 1, RELIEF.pixelRatioCap));
  r.setSize(w, h);
  return r;
}

// ---------------------------------------------------------------------------
// Grammaire de touches : UNE seule implémentation
// ---------------------------------------------------------------------------

/**
 * Actions qu'une vue peut exposer. Toutes sont optionnelles : une vue qui
 * n'a pas d'« étage supérieur » n'expose simplement pas `up` — et la touche
 * n'est alors pas consommée (pas de touche morte silencieuse qui avale
 * l'événement sans rien faire).
 */
export interface ViewNav {
  /** Entrer dans le plus gros SOUS-GROUPE de l'étage courant (`Page ↓`). */
  entrer?: () => void;
  /** Remonter d'un groupe dans la hiérarchie (`Page ↑` / Retour arrière). */
  remonter?: () => void;
  /** `→` — élément suivant à l'étage courant. */
  next?: () => void;
  /** `←` — élément précédent. */
  prev?: () => void;
  /** `↑` — remonter d'un étage (dé-zoomer, sortir, tout voir). */
  up?: () => void;
  /** `↓` ou `Entrée` — entrer dans l'élément visé. */
  down?: () => void;
  /** `⌂` ou `r` — vue d'ensemble / racine. */
  home?: () => void;
  /** `+` — densifier (se rapprocher, montrer plus de détail). */
  zoomIn?: () => void;
  /** `-` — alléger (s'éloigner). */
  zoomOut?: () => void;
  /**
   * ── `d` — DENSIFIER LE NIVEAU : LA PAGE SUIVANTE, PAS UN CRAN DE PLUS ─────
   *
   * Deux gestes répondent à « davantage », et ils ne font pas la même chose
   * (`densite.ts`) : `+` rapproche la caméra (donc, à terme, DESCEND d'un cran),
   * `d` montre plus de matière DU MÊME NIVEAU — ce que l'appel a rendu et qui
   * n'était pas dessiné. Sans ce second geste, un niveau qui portait 200
   * éléments n'en montrait que 24, et la seule façon de voir les autres était de
   * changer d'échelle : c'est-à-dire de ne pas les voir là où ils vivent.
   *
   * OPTIONNEL comme les autres : une vue sans niveaux paginés ne l'expose pas,
   * et la touche n'est alors pas consommée (pas de touche morte silencieuse).
   */
  densifier?: () => void;
  /** `Échap` — désélectionner / fermer la couche courante. */
  escape?: () => void;
  /** `h` ou `?` — aide. */
  help?: () => void;
}

/** Libellé unique de l'aide clavier, affiché par toutes les vues. */
export const VIEW_KEYS_HINT =
  "←/→ parcourir · ↑ remonter · ↓ entrer · ⌂ tout voir · +/− densifier · d page du niveau · Échap fermer · h aide";

/**
 * Lie la grammaire commune. Renvoie la fonction de détachement.
 *
 * Règles respectées par toutes les vues :
 *  - aucune touche n'est consommée pendant une saisie (INPUT/TEXTAREA/select) ;
 *  - aucune touche n'est consommée si un modificateur est enfoncé (⌘K, ⌃, ⌥) ;
 *  - `preventDefault` seulement quand l'action existe réellement.
 */
export function bindViewKeys(nav: ViewNav): () => void {
  const onKey = (e: KeyboardEvent) => {
    const t = e.target as HTMLElement | null;
    const tag = t?.tagName;
    if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT" || t?.isContentEditable) return;
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    const fire = (fn?: () => void) => {
      if (!fn) return false;
      e.preventDefault();
      fn();
      return true;
    };
    const k = e.key;
    if (k === "ArrowRight") return void fire(nav.next);
    if (k === "ArrowLeft") return void fire(nav.prev);
    // ── MARCHER DANS UNE HIÉRARCHIE, PAS SEULEMENT ENTRE DES PAGES ──────────
    // Les touches de cran (↑/↓/⌂) déplacent dans une ÉCHELLE. Entrer dans un
    // sous-groupe et remonter d'un groupe sont d'autres gestes : `Page ↓` entre
    // (le plus gros sous-groupe), `Page ↑` et `Retour arrière` remontent d'un
    // niveau. Elles sont OPTIONNELLES : une vue qui ne les fournit pas ne change
    // pas de comportement, et une vue qui les fournit suit la même grammaire que
    // le reste de l'application (`StoreOverview` remonte déjà avec Retour arrière).
    if (k === "PageDown") return void fire(nav.entrer);
    if (k === "PageUp" || k === "Backspace") return void fire(nav.remonter);
    if (k === "ArrowUp") return void fire(nav.up);
    if (k === "ArrowDown" || k === "Enter") return void fire(nav.down);
    if (k === "Home") return void fire(nav.home);
    if (k === "+" || k === "=") return void fire(nav.zoomIn);
    if (k === "-" || k === "_") return void fire(nav.zoomOut);
    // `d` : DENSIFIER LE NIVEAU (la page suivante), et non descendre d'un cran.
    // Placée AVANT `Escape`/`r`/`h` comme les autres : toute action déclarée est
    // consommée, aucune ne l'est si la vue ne l'expose pas.
    if (k === "d" || k === "D") return void fire(nav.densifier);
    if (k === "Escape") return void fire(nav.escape);
    const low = k.toLowerCase();
    if (low === "r") return void fire(nav.home);
    if (low === "h" || k === "?") return void fire(nav.help);
  };
  window.addEventListener("keydown", onKey);
  return () => window.removeEventListener("keydown", onKey);
}

/** Applique le profil unifié à une caméra de vue. */
export function makeCamera(w: number, h: number): THREE.PerspectiveCamera {
  return new THREE.PerspectiveCamera(CAMERA.fov, w / h, CAMERA.near, CAMERA.far);
}

/** Vrai pour un libellé qui n'est qu'un identifiant technique (GUID/hash). */
export const isRawId = (s: string) =>
  !!s && /^[0-9a-fA-F\-_]+$/.test(s);
