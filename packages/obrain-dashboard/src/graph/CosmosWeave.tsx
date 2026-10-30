import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import { useAtom, useSetAtom } from "jotai";
import { useAtomValue } from "jotai";
import { subjectAtom, chatOuvertAtom, weaveTokenAtom, montreAtom, axeNavigationAtom, type NiveauCosmos, type AxeNavigation } from "../app/store";
import { choisir, type Candidat } from "./selection";
import { enfantsDe, pageServeur, rayons, directions, estFeuille, tokenValide, FRACTALE, rangRacine } from "./fractale";
import { PAGE, decisionDensite, pageSuivante, phraseDecision, type Decision } from "./densite";
import {
  BANDE_DEDANS, BANDE_MONDE, CRAN_T, PALIERS, PALIER_DEDANS_SIGNAUX, PALIER_SIGNAUX,
  distancePourPalier, distancePourT, palierDe, viserCran, type Palier,
} from "./echelle";
import * as THREE from "three";
import { ACCENT, applyRelief, bindViewKeys, EMPHASIS, emphasis, makeCamera, makeRenderer, orbitFor, typeColor, VIEW_KEYS_HINT, zoomBy } from "./grammar";
import { fetchModuleArcs, fetchModules, fetchWeave, type Weave, type WeaveFocus, type WeaveWorld } from "../lib/api";
import { PLAN_COLOR, PLAN_CSS } from "./grammar";
import { LevelCartouche, PlanLegend, ViewPanel } from "./ViewPanel";
import SubjectLine from "./SubjectLine";
import { useActivation } from "./useActivation";
import type { Activation } from "../lib/api";
import StageContinu from "../components/StageContinu";
// LA PARENTÉ : une seule règle dans tout le dépôt (voir ./fratrie.ts).
// Ces deux appels étaient divergents : `gr.noeudsInfo[].type` prenait
// l'avant-dernier segment BRUT (donc « rst » sur un fichier indexé) pendant que
// l'étiquette de filtre filtrait `__file__` (donc « msg_zerocopy »). Comme le
// filtre compare `infos[i].type === filtre`, la puce ne pouvait rien allumer.
import { fichierDe, moduleDe, parentDe, strateDe, aIndexer, construireIndex, fratrieDe,
  noeudLePlusEntoure, type IndexFratrie, type Fratrie } from "./fratrie";
import type { GraphNode as GraphNodePayload } from "../lib/api";

/**
 * COSMOS — un système solaire, et la toile de l'univers qui se tisse.
 *
 * Trois principes (voir docs/ADR-COSMOS-TOILE-2026-09-10.md) :
 *  1. **Le centre n'est pas fixe.** Le foyer (l'étoile) dépend du contexte :
 *     l'instant, l'agent, la conversation, l'objectif, l'utilisateur, le sujet.
 *     Changer de foyer, c'est changer ce qui attire.
 *  2. **Le sol s'affaisse à la masse.** La nappe d'espace-temps n'est pas un
 *     plan fixe : chaque monde y creuse un puits ∝ sa masse réelle (nœuds).
 *  3. **La toile se densifie.** Chaque fibre entre deux mondes est une synapse
 *     fondée sur une preuve réelle (entité partagée, ou session réellement
 *     exécutée dans ce territoire) et DATÉE : le curseur de temps rejoue la
 *     densification sur l'horloge réelle de l'agent.
 */


/**
 * SHADERS PAR TYPE D'ENTITÉ.
 *
 * Les points de three.js sont des CARRÉS : à l'échelle d'un monde, l'amas se lit
 * comme une grille, pas comme une matière. Un shader par type d'entité donne à
 * chaque nature sa façon d'occuper la lumière :
 *   - `MONDE` : particules rondes, additionnelles, qui se fondent quand elles
 *     se chevauchent — c'est ce qui fait « nuage » plutôt que « semis ».
 * Les types suivants (ancre, arête, fibre, signal) viendront s'ajouter ici,
 * chacun avec sa loi d'opacité et de taille.
 */
/**
 * ── LA MATIÈRE DES GROUPES ──────────────────────────────────────────────────
 *
 * Les mondes avaient un shader soigné (disque doux, cœur lumineux, halo,
 * additif) ; les GROUPES, eux, utilisaient un `PointsMaterial` nu — qui dessine
 * des CARRÉS SANS DÉGRADÉ. D'où le « c'est grossier » : le seul étage sans
 * matière était précisément la vue groupe.
 *
 * On lui donne la MÊME grammaire que les mondes — c'est ce qui rend l'ensemble
 * logique : un seul langage visuel d'une échelle à l'autre — en y ajoutant ce
 * que la densité permet : DEUX COURONNES, un cœur net et un halo large. Un
 * disque unique donne une tache plate ; deux rayons donnent la profondeur d'un
 * point d'étoile.
 *
 * `uOpacite` est présent : le fondu croisé de l'étage intérieur, qui ne pouvait
 * agir que sur un `PointsMaterial`, retrouve ici son chemin prévu.
 */
/**
 * ── RAMPE LISSÉE : LA TRANSITION DOIT ÊTRE VIABLE, PAS SEULEMENT FONCTIONNELLE ──
 *
 * Les ouvertures étaient LINÉAIRES (`(x - a) / b` borné) : la dérivée saute de 0
 * à plein régime au seuil, puis retombe à 0 d'un coup. Mesuré sur la descente :
 * QUATRE ruptures de plus de 15 points de contenu, dont une de +50 puis -46. Une
 * transition qui « saute » n'est pas une transition, c'est un remplacement.
 *
 * `smoothstep` (3x² - 2x³) a une dérivée NULLE aux deux extrémités : l'étage
 * apparaît et disparaît donc sans à-coup. Et comme la rampe est pilotée par la
 * DISTANCE, elle est SYMÉTRIQUE — la sortie se lisse du même coup, ce qui règle
 * aussi « je peux quitter la vue trop brusquement ».
 */
const lisser = (x: number): number => {
  const t = Math.max(0, Math.min(1, x));
  return t * t * (3 - 2 * t);
};

/**
 * ── LA LOI D'ÉCHELLE DES POUPÉES : DES RAPPORTS, PAS DES TAILLES ────────────
 *
 * « Coupler les échelles à la taille de l'ensemble. » Une scène à niveaux
 * imbriqués ne se règle pas par des tailles absolues (un groupe de 15 000 nœuds
 * dans un monde de 1 500 000 n'a pas de taille « en unités ») mais par des
 * RAPPORTS : chaque niveau tient sa taille de sa part du tout, et la variation
 * d'un niveau au suivant est UN nombre, déclaré ici.
 *
 *   - `PART_MAX_GROUPE` : la plus lourde poupée occupe cette part du rayon de
 *     son monde. Les autres suivent la même loi au prorata de leur VOLUME
 *     (`∛` de leur part), donc les proportions sont exactes à toute échelle.
 *   - `PLANCHER_GROUPE` : seul écart assumé — un groupe doit rester désignable.
 *     Mesuré, ce qu'il coûte : sur `runtime-dev`, le groupe `tsx` (10 nœuds sur
 *     127 215) devrait faire 0,015 du rayon du monde et en fait 0,050 — il est
 *     donc rendu 3,4 fois plus gros que sa part. C'est le prix d'un groupe qu'on
 *     peut encore viser au doigt, et il est BORNÉ : il ne touche que les groupes
 *     dont la part est sous 0,03 %.
 *   - `CADRAGE_POUPEE` : distance de cadrage, en rayons de la poupée visée.
 *     Une seule constante pour le clic, l'encart et le bouton « Graphe » : la
 *     variation d'échelle est ainsi la MÊME partout, au lieu de trois multiples
 *     réglés séparément (4,2 · 3,4 · 1,3) qui divergeaient en silence.
 */
const PART_MAX_GROUPE = 0.34;
const PLANCHER_GROUPE = 0.05;
const CADRAGE_POUPEE = 4.2;
/** Cadrage d'un MONDE : marge sur son rayon (la sphère tient dans le champ). */
const CADRAGE_MONDE = 1.3;

/**
 * ── L'ÉCHELLE A UNE SEULE DÉFINITION, ET ELLE VIT ICI ───────────────────────
 *
 * Le curseur de descente `t` (0 = espace, 1 = au cœur) est calculé dans la
 * boucle à partir de la distance caméra ↔ monde, sur une échelle
 * LOGARITHMIQUE : `t = ln(loin/d) / ln(loin/pres)`, avec `loin = 14 R` et
 * `pres = 0,25 R`. C'est lui que le panneau affiche, lui qui ouvre les étages.
 *
 * Deux autres endroits parlaient pourtant de la même échelle SANS le dire :
 *   - les BANDES affichées (« espace / monde / dedans »), à 0,34 et 0,72 ;
 *   - les CRANS des boutons de vue, exprimés en RAYONS (1,3 puis 0,55).
 *
 * MESURÉ, et c'est ce qui a rendu la faute visible : le cran « 🔺 Graphe »
 * (0,55 R) tombait à `t` = 0,63 — donc DANS la bande « monde » — pendant que le
 * bouton allumé restait « 🧠 Monde ». Un cran et son libellé se contredisaient,
 * parce que l'un était une distance et l'autre une valeur de curseur.
 *
 * On ne garde donc qu'UNE autorité : les bandes, ci-dessous, et des CRANS
 * exprimés en `t`. La distance d'un cran s'en DÉDUIT par l'inverse de la
 * formule — elle ne peut plus tomber dans la bande d'à côté.
 */
/**
 * ── LA LOI D'ÉCHELLE VIT DANS `echelle.ts` ──────────────────────────────────
 *
 * `BANDE_MONDE`, `BANDE_DEDANS`, `CRAN_T`, `PALIER_*`, `PALIERS`, `distancePourT`,
 * `palierDe`, `distancePourPalier` ont été SORTIS de ce fichier : elles n'étaient
 * testables nulle part, et c'est pourtant elles qui ont échoué quand un cran
 * visait un monde pendant que le palier en mesurait un autre. Elles sont
 * désormais dans un module pur, avec `echelle.test.mjs` — dont l'invariant
 * « un cran qui vise sa bande la retrouve » (`palierDe(distancePourPalier(n,R),R) === n`).
 * Le composant ne fait plus que l'appeler.
 */

const SHADER_GROUPE = {
  vertex: `
    uniform float uTaille;
    void main() {
      vec4 mv = modelViewMatrix * vec4(position, 1.0);
      // Même atténuation que les mondes : le détail suit l'échelle.
      gl_PointSize = uTaille * (420.0 / max(1.0, -mv.z));
      gl_Position = projectionMatrix * mv;
    }`,
  fragment: `
    uniform vec3 uCouleur;
    uniform float uOpacite;
    void main() {
      vec2 c = gl_PointCoord - vec2(0.5);
      float d = length(c);
      if (d > 0.5) discard;              // le carré disparaît : reste la particule
      float coeur = smoothstep(0.18, 0.0, d);
      float halo = smoothstep(0.5, 0.10, d);
      float a = clamp(halo * 0.55 + coeur * 0.85, 0.0, 1.0);
      // Le cœur s'éclaircit vers le blanc : c'est ce qui fait « grain » plutôt
      // que « pastille », et la famille de couleur reste lisible.
      vec3 col = uCouleur * (0.55 + 1.15 * coeur) + vec3(0.10, 0.11, 0.13) * coeur;
      gl_FragColor = vec4(col, a * uOpacite);
    }`,
};

function materiauGroupe(couleur: THREE.Color | string | number, opacite = 1) {
  return new THREE.ShaderMaterial({
    uniforms: {
      uCouleur: { value: new THREE.Color(couleur) },
      uOpacite: { value: opacite },
      uTaille: { value: 2.6 },
    },
    vertexShader: SHADER_GROUPE.vertex,
    fragmentShader: SHADER_GROUPE.fragment,
    transparent: true,
    depthWrite: false,
    // Même fusion que les mondes : un seul langage visuel d'une échelle à l'autre.
    blending: THREE.AdditiveBlending,
  });
}

const SHADER_MONDE = {
  vertex: `
    attribute vec3 aColor;
    attribute float aTaille;
    varying vec3 vColor;
    varying float vTaille;
    void main() {
      vColor = aColor;
      vTaille = aTaille;
      vec4 mv = modelViewMatrix * vec4(position, 1.0);
      // Taille en unités-monde, mais atténuée : le détail suit l'échelle.
      gl_PointSize = aTaille * (420.0 / max(1.0, -mv.z));
      gl_Position = projectionMatrix * mv;
    }`,
  fragment: `
    varying vec3 vColor;
    varying float vTaille;
    uniform float uOpacite;
    void main() {
      // Disque doux : le carré devient une particule.
      vec2 c = gl_PointCoord - vec2(0.5);
      float d = length(c);
      if (d > 0.5) discard;
      float a = smoothstep(0.5, 0.08, d);
      gl_FragColor = vec4(vColor * (0.55 + 0.9 * a), a * uOpacite);
    }`,
};

/** Matériau de particules conforme à la loi du type « monde ». */
/**
 * FIBRE — un filament de tissu, au lieu d'un trait.
 *
 * Le rendu précédent traçait des segments DROITS à opacité 0,25-0,75 : chaque
 * synapse se lisait comme une barre pleine, et le tissu n'apparaissait pas —
 * on voyait des traits, pas une densité. Ici :
 *
 *  - la fibre est COURBE (quadratique bowée perpendiculairement), avec une
 *    amplitude et un signe tirés d'un hachage de (a,b) : déterministe, donc
 *    stable entre rebuilds, mais organique et jamais deux fois la même ;
 *  - elle est FINEMENT bruitée le long de son tracé (jitter perpendiculaire
 *    décroissant vers les extrémités) : ça casse la rectitude sans casser
 *    l'ancrage sur les nœuds ;
 *  - l'opacité est TRÈS basse et le mélange ADDITIF : c'est la SUPERPOSITION
 *    qui fait la lumière. Un croisement dense s'éclaircit tout seul, ce qui
 *    rend la densité de tissage visible au lieu de l'écraser ;
 *  - `brins` parallèles pour les arcs lourds : un faisceau, pas un tuyau —
 *    le poids se lit à l'épaisseur du FAISCEAU, pas à celle du trait ;
 *  - le bow est orienté par la direction radiale (centre → extérieur), donc le
 *    tissu s'ouvre vers l'extérieur : expansion + direction.
 */
function fibre(
  p1: THREE.Vector3,
  p2: THREE.Vector3,
  poids: number,
  poidsMax: number,
  teinte = 0x4aa8ff,
): THREE.Group {
  const g = new THREE.Group();
  const w = Math.max(0, Math.min(1, poids / Math.max(1, poidsMax)));
  const n = 10; // échantillons le long de la fibre
  // Hachage stable de la paire : même arc => même courbe à chaque rebuild.
  const h = (() => {
    let x = 2166136261 >>> 0;
    for (const v of [p1.x, p1.y, p1.z, p2.x, p2.y, p2.z]) {
      x ^= Math.round(v * 997) >>> 0; x = Math.imul(x, 16777619) >>> 0;
    }
    return x;
  })();
  const signe = (h & 1) ? 1 : -1;
  const bow = (0.035 + 0.075 * ((h >>> 3) % 100) / 100) * signe; // 3,5-11 % de la longueur
  const dir = new THREE.Vector3().subVectors(p2, p1);
  const L = dir.length() || 1e-6;
  const perp = new THREE.Vector3().crossVectors(dir, new THREE.Vector3(0, 1, 0));
  if (perp.lengthSq() < 1e-9) perp.set(1, 0, 0); else perp.normalize();
  // Direction radiale au milieu de la fibre : donne l'EXPANSION.
  const milieu = new THREE.Vector3().addVectors(p1, p2).multiplyScalar(0.5);
  const radial = milieu.clone(); if (radial.lengthSq() > 1e-9) radial.normalize();
  const ouverture = 0.5 + 0.5 * Math.abs(radial.dot(perp)); // 0,5..1

  const brins = w > 0.55 ? 3 : w > 0.25 ? 2 : 1;
  const opacite = 0.030 + 0.055 * w; // accumulation, jamais un trait plein
  for (let b = 0; b < brins; b++) {
    const ecart = (b - (brins - 1) / 2) * (0.008 + 0.020 * w) * L;
    const pts: THREE.Vector3[] = [];
    for (let i = 0; i <= n; i++) {
      const t = i / n;
      const pt = new THREE.Vector3().lerpVectors(p1, p2, t);
      // bow quadratique (nul aux extrémités, max au milieu)
      pt.addScaledVector(perp, bow * L * ouverture * 4 * t * (1 - t));
      // jitter organique, amorti aux ancrages
      const j = (((h >>> (b + 1)) % 1000) / 1000 - 0.5) * 0.006 * L * Math.sin(Math.PI * t);
      pt.addScaledVector(perp, ecart + j);
      pts.push(pt);
    }
    // ── LE FIL A UNE MATIÈRE, PAS SEULEMENT UNE COULEUR ─────────────────────
    // Il était d'une couleur PLATE d'un bout à l'autre : un trait. On met un
    // DÉGRADÉ le long du brin — vif aux deux ancrages, estompé au milieu — ce qui
    // le fait lire comme un FILAMENT. Ce n'est pas décoratif : la fibre relie deux
    // mondes, elle leur appartient par ses extrémités ; le milieu n'est que le
    // trajet. La forme le dit, la matière le dit aussi.
    const cols = new Float32Array((n + 1) * 3);
    const base = new THREE.Color(teinte);
    for (let i = 0; i <= n; i++) {
      const t = i / n;
      const k = 0.45 + 0.55 * Math.abs(2 * t - 1); // 1,00 aux ancrages · 0,45 au milieu
      cols[i * 3] = base.r * k;
      cols[i * 3 + 1] = base.g * k;
      cols[i * 3 + 2] = base.b * k;
    }
    const geoBrins = new THREE.BufferGeometry().setFromPoints(pts);
    geoBrins.setAttribute("color", new THREE.BufferAttribute(cols, 3));
    const ligne = new THREE.Line(
      geoBrins,
      new THREE.LineBasicMaterial({
        color: teinte,
        vertexColors: true,
        transparent: true,
        opacity: opacite,
        blending: THREE.AdditiveBlending,
        depthWrite: false,
      }),
    );
    g.add(ligne);
  }
  return g;
}


function materiauMonde(opacite = 0.96) {
  return new THREE.ShaderMaterial({
    uniforms: {
      uOpacite: { value: opacite },
    },
    vertexShader: SHADER_MONDE.vertex,
    fragmentShader: SHADER_MONDE.fragment,
    transparent: true,
    depthWrite: false,
    blending: THREE.AdditiveBlending,
    vertexColors: false,
  });
}


/**
 * Type « NŒUD » (le graphe réel) : loi DIFFÉRENTE de celle du monde.
 * Un monde est une matière diffuse (disque large, très additif, grains variés) ;
 * un nœud est une entité précise qu'on doit pouvoir viser et distinguer de ses
 * voisines : disque plus petit, bord plus net, halo contenu. Deux types, deux
 * shaders — c'est ce qui permet de les lire ensemble sans les confondre.
 */
const SHADER_NOEUD = {
  vertex: `
    attribute vec3 aColor;
    attribute float aTaille;
    attribute float aChaleur;
    attribute float aEmphase;
    varying vec3 vColor;
    varying float vChaleur;
    varying float vEmphase;
    void main() {
      vColor = aColor;
      vChaleur = aChaleur;
      vEmphase = aEmphase;
      vec4 mv = modelViewMatrix * vec4(position, 1.0);
      gl_PointSize = aTaille * (300.0 / max(1.0, -mv.z));
      gl_Position = projectionMatrix * mv;
    }`,
  fragment: `
    varying vec3 vColor;
    varying float vChaleur;
    varying float vEmphase;
    uniform float uOpacite;
    void main() {
      vec2 c = gl_PointCoord - vec2(0.5);
      float d = length(c);
      if (d > 0.5) discard;
      // Bord net + petit cœur chaud : la chaleur vient de l'énergie réelle
      // (overlay du moteur), pas d'un dégradé décoratif.
      float bord = smoothstep(0.5, 0.34, d);
      float coeur = smoothstep(0.24, 0.0, d);
      // EMPHASE : ce qui ressort reste plein et garde sa couleur ; le reste
      // baisse d'intensité SANS disparaître — on doit toujours lire l'ensemble
      // dont l'élément se détache.
      vec3 col = (vColor + vec3(0.9, 0.55, 0.15) * coeur * vChaleur) * (0.35 + 0.65 * vEmphase);
      gl_FragColor = vec4(col, bord * uOpacite * (0.16 + 0.84 * vEmphase));
    }`,
};

function materiauNoeud(opacite = 0.95) {
  return new THREE.ShaderMaterial({
    uniforms: { uOpacite: { value: opacite } },
    vertexShader: SHADER_NOEUD.vertex,
    fragmentShader: SHADER_NOEUD.fragment,
    transparent: true,
    depthWrite: false,
    blending: THREE.NormalBlending,
  });
}

const PLAN_LABEL: Record<string, string> = {
  connaissance: "connaissance",
  self: "self",
  utilisateur: "utilisateur",
};

/** Hash stable → inclinaison / phase d'orbite (jamais aléatoire au rechargement). */
function hash(s: string): number {
  let h = 2166136261;
  for (let i = 0; i < s.length; i++) h = (h ^ s.charCodeAt(i)) * 16777619;
  return (h >>> 0) / 4294967295;
}

interface Layout {
  /** qn du monde → rayon orbital, inclinaison, phase, vitesse, taille. */
  orbits: Map<string, { r: number; inc: number; phase: number; w: number; size: number }>;
  starQn: string | null;
}

/**
 * ── LES DIMENSIONS PILOTENT LA DESCENTE, ELLES NE CHANGENT PAS DE CANEVAS ────
 *
 * `App.tsx` route « Cosmos » vers ce composant et « Monde »/« Graphe » vers
 * `StageContinu` : le voyage d'échelle traverse donc une FRONTIÈRE DE CANEVAS
 * entre le Cosmos et le Monde — c'est le défaut signalé.
 *
 * Or les trois dimensions existent DÉJÀ ici : le cosmos (les mondes), le monde
 * (son intérieur, ouvert par proximité) et le graphe (ses nœuds réels). On ne
 * remplace donc rien : les boutons DEMANDENT un niveau, et cette scène descend
 * — dans le même canevas, avec ses propres transitions.
 */
export default function CosmosWeave({ onOpen, activePath, niveau = 0, onNiveau }:
  { onOpen?: (path: string, label: string) => void; activePath?: string;
    niveau?: NiveauCosmos; onNiveau?: (n: NiveauCosmos) => void }) {
  const host = useRef<HTMLDivElement>(null);
  const setSubject = useSetAtom(subjectAtom);
  // Le chat est monté par `App` : ici on le DEMANDE, on ne le contient pas.
  const setChatOuvert = useSetAtom(chatOuvertAtom);
  // ACTIVATION EN DIRECT : ce que l'engine vient de consulter (journal partagé).
  const activation = useActivation();
  const [weave, setWeave] = useState<Weave | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [focusIdx, setFocusIdx] = useState(0);
  /** 0..1 — le curseur de densification (horloge réelle rejouée). */
  const [density, setDensity] = useState(1);
  /** Monde visé par la navigation clavier (grammaire commune). */
  const [focusQn, setFocusQn] = useState<string | null>(null);
  /**
   * DESCENTE CONTINUE — « comme Google Maps, de l'espace à une rue ».
   * `descente` va de 0 (espace : toutes les boules) à 1 (dedans : la boule
   * ouverte). Ce n'est pas un sélecteur de vue : c'est la DISTANCE de caméra,
   * lissée, et tout le rendu s'en déduit continûment.
   */
  const [descente, setDescente] = useState(0);
  /**
   * LE PALIER (0 espace · 1 monde · 2 groupe · 3 signaux).
   *
   * `descente` est un CURSEUR continu qui sature à 1 sous 0,25 R : il ne peut pas
   * dire si l'on est « dans le groupe » ou « au cœur, dans l'espace des signaux ».
   * Le palier est donc publié à part, calculé sur la distance par `palierDe` — et
   * il est la seule source du mot affiché, du voyant des boutons et du rapport de
   * niveau. On ne publie QUE ses changements (un `setState` par image relancerait
   * un rendu React à 60 Hz).
   */
  const [palier, setPalier] = useState<0 | 1 | 2 | 3>(0);
  const palierRef = useRef<0 | 1 | 2 | 3>(0);
  const chargementEnVolRef = useRef(false);
  /** Nombre de rapports de niveau émis, et le dernier — pour situer une panne
   *  entre la SCÈNE (qui rapporte) et l'APP (qui allume le voyant). */
  const rapportsRef = useRef<{ n: number; dernier: NiveauCosmos }>({ n: 0, dernier: 0 });
  /** Timeline des chargements d'étage : ce qui est prêt, ce qui charge. */
  const [timeline, setTimeline] = useState("");
  /** La conversation PLEIN ÉCRAN (mobile : tout l'écran, refermable). */
  // (2026-09-15, chat-first) Le chat est LA vue d entree du 8432 : la
  // conversation s ouvre au chargement, le graphe est la vue qu on decouvre
  // en la refermant. Demande produit explicite — « une vraie vue de chat,
  // le graphe est une vue en plus ».
  // ── LE STORE DE LA CONVERSATION ─────────────────────────────────────────
  // Défaut mesuré (2026-09-11), reproduit : le chat annonçait « ancré sur
  // world:this-rs-this--github » et lisait **runtime-dev**. Deux causes
  // cumulées :
  //   1. `focus.default.world` porte un LIBELLÉ (« Runtime dev (engrams) ») et
  //      était comparé au `qn` des mondes (`world:runtime-dev-engrams`) : jamais
  //      égal, donc la résolution échouait à TOUS les coups ;
  //   2. le repli `worlds[0].path` câblait la conversation sur le PREMIER monde.
  //
  // On transmet donc ce qu'on sait vraiment : le store du SUJET partagé (posé
  // par le Cosmos lui-même, `wd.path`). Et pour un sujet `world:`, c'est le
  // serveur qui fait autorité : il résout le monde dans la base synaptique et
  // RENVOIE ce qu'il a réellement ouvert — l'en-tête du chat affiche cela, plus
  // une supposition de la vue.
  const [sujet] = useAtom(subjectAtom);


  /** ⏸ Animation figée : on lit au lieu de regarder passer. */
  const [fige, setFige] = useState(false);
  const figeRef = useRef(false);
  const nowFigeRef = useRef(0);
  figeRef.current = fige;
  /** Ce que l'étage intérieur contient pour le monde visé (réel, pas décoratif). */
  /**
   * `n` = ce qui est DESSINÉ, `total` = ce que la base CONTIENT.
   *
   * Les deux étaient confondus : la liste des modules était plafonnée à 8 AVANT
   * qu'on en prenne la longueur, donc le panneau annonçait « 8 ensembles » alors
   * que la base en avait 23 — et rien ne le disait. Un plafond silencieux fait
   * croire que la donnée n'existe pas : c'est pire qu'une donnée manquante.
   */
  /**
   * CE QUI EST VISÉ, À L'ÉTAGE COURANT.
   *
   * La sélection existait déjà dans la logique (`ensembleChoisi` oriente le
   * chargement du graphe, et `chargerGraphe` recentre sur un nœud cliqué) mais
   * elle était INVISIBLE : rien ne disait ce qu'on avait visé. Naviguer en
   * choisissant des éléments sans voir son choix, c'est naviguer à l'aveugle —
   * et on ne sait pas si le pas suivant partira de là.
   *
   * `vise` porte la sélection à l'écran ; le marquage dans la scène est fait par
   * `marquerEnsemble` (taille) et par l'emphase des nœuds.
   */
  const [vise, setVise] = useState<{ genre: "ensemble" | "noeud" | "arete"; nom: string; detail: string } | null>(null);
  /** Lisible depuis la boucle (construite une fois) : voir l'export `cible`. */
  const viseRef = useRef<typeof vise>(null);
  viseRef.current = vise;
  const [interieur, setInterieur] = useState<{
    qn: string; n: number; total: number;
    /**
     * ── LA DÉCISION DU NIVEAU : DENSIFIER OU APPROFONDIR ────────────────────
     *
     * `n` (dessiné) et `total` (reçu) ne suffisaient pas à choisir : il fallait
     * la LOI (`densite.ts`). Elle est calculée à la construction de l'étage et
     * publiée ici — la barre d'échelle et le panneau lisent donc la MÊME
     * décision que les gestes, au lieu de la réinventer à l'affichage.
     */
    decision?: Decision;
    /** La page dessinée (`densite.ts`), pour que « ＋ densifier » sache où l'on en est. */
    page?: number;
    /** Le serveur a coupé à son plafond : densifier devient un appel. */
    plafondAtteint?: boolean;
    /**
     * D'OÙ VIENNENT CES GROUPES. `familles` = l'agrégat serveur des chemins
     * (`/api/viz/modules`), dont les effectifs portent sur le MONDE ENTIER ;
     * `axe` = les groupes d'un axe de RELATION (communauté Leiden, contour ∂),
     * calculés depuis le graphe chargé — leurs effectifs portent donc sur
     * l'ÉCHANTILLON du graphe, pas sur le monde. Sans ce champ, le panneau
     * annoncerait « 420 nœuds » là où le monde en a 75 209 : deux nombres qui ne
     * parlent pas de la même chose sous le même mot.
     */
    source?: "racines" | "sous-groupes" | "familles" | "axe" | "suivants";
    axe?: string;
    /** Le CHEMIN dans la hiérarchie du monde (`fractale.ts`) et son rang. */
    prefixe?: string;
    rang?: number;
    /** Ce niveau n'a plus de sous-groupes exploitables : on le DIT. */
    feuille?: boolean;
    /** La chaîne des CLASSES entrées (l'autre coordonnée du niveau). */
    classes?: { axe: string; valeur: string }[];
    /** Les trois plus gros — conservé : un lecteur l'utilise en ligne. */
    top: { name: string; count: number }[];
    /**
     * ── LES FRÈRES, NOMMÉS ────────────────────────────────────────────────
     * « Les frères restent visibles en retrait » (voir la boucle : les autres
     * groupes du même monde sont gardés à `ouvI * 0,26`) — mais AUCUN n'était
     * nommé, aucun effectif n'était affiché, et aucun n'était cliquable : on
     * voyait qu'il y avait des voisins, jamais lesquels. `top` n'en portait que
     * TROIS (`.slice(0, 3)`), coupure muette. Ici, la liste COMPLÈTE des
     * ensembles réellement dessinés, avec leur effectif réel — c'est la même
     * source que les boules de la scène (`modules`, sans plafond), donc une
     * puce par boule, et un clic la ramène dans le champ via `cadrerGroupeRef`.
     */
    boules: { name: string; count: number }[];
  } | null>(null);
  /** Ce que l'étage graphe contient pour le monde visé. */
  /** `total` = la taille RÉELLE du monde : un sous-graphe de 300 nœuds sur
   *  372 750 doit le DIRE, sinon l'échelle graphe se fait passer pour la donnée
   *  entière. */
  const [graphe, setGraphe] = useState<{ qn: string; centre: string; noeuds: number; aretes: number; total?: number; types: string[]; motif: string; degrade?: boolean } | null>(null);
  /**
   * DIMENSION DE GROUPEMENT, commutable.
   *
   * La demande portait sur « layer, type, chaîne…, quelle que soit la
   * conception ». Le TYPE d'entité n'est exposé par AUCUN champ du sous-graphe
   * (mesuré : `label` est le libellé d'affichage, `color_hint`/`size_hint` sont
   * nuls sans overlay) — il faudrait l'ajouter côté moteur. En attendant, on
   * offre les dimensions réellement dérivables du `qualified_name` : le module
   * porteur, le dossier parent, l'extension. Le jour où le type arrive, il
   * s'ajoute ici comme une dimension de plus.
   */
  // « CHANGER D'ANGLE DE VUE » : descendre n'est qu'un axe. On peut aussi
  // regarder une COMMUNAUTÉ au lieu d'une descendance, ou une COUCHE de type.
  // Ces axes existent déjà dans les données (`community_id`) — ils étaient
  // simplement absents de la navigation.
  /** L'axe est une DÉCISION PARTAGÉE (voir `axeNavigationAtom`) ; `Dimension`
   *  reste le nom local du même type, pour ne pas renommer tout le fichier. */
  type Dimension = AxeNavigation;
  /**
   * ── LES AXES, DÉCLARÉS UNE FOIS ─────────────────────────────────────────────
   *
   * Chaque axe est une RELATION RÉELLE de la donnée, pas une variante cosmétique :
   *   · `module` · `parent` · `extension` · `couche` se lisent dans le NOM
   *     (`qualified_name`, `file_path`) ;
   *   · `communauté` vient du MOTEUR (`community_id`, détection Leiden) ;
   *   · `∂` vient de la COURBURE (Ricci) : bords contre intérieur.
   *
   * Les deux derniers ne sont pas des découpages du nom : ils traversent les
   * dossiers, et c'est précisément ce qui rend la navigation polymorphe — le même
   * geste n'y mène pas au même endroit.
   */
  const AXES: { cle: Dimension; court: string; quoi: string }[] = [
    { cle: "module", court: "projet", quoi: "L'axe par DÉSCENDANCE : le projet porteur du nom" },
    { cle: "parent", court: "parent", quoi: "L'axe par parenté : le dossier ou l'espace de noms porteur" },
    { cle: "extension", court: "extension", quoi: "L'axe par extension de fichier" },
    { cle: "communauté", court: "communauté", quoi: "L'axe par COMMUNAUTÉ du moteur (Leiden) — traverse les dossiers" },
    { cle: "couche", court: "strate", quoi: "L'axe par strate du nom (projet › paquet › dossier › fichier)" },
    { cle: "∂", court: "bords ∂", quoi: "L'axe par CONTOUR structurel : nœuds de courbure négative (bords) contre intérieur" },
  ];
  const [dimension, setDimension] = useAtom(axeNavigationAtom);
  const [qnsGraphe, setQnsGraphe] = useState<string[]>([]);
  /** AXE DU TEMPS : l'histoire réelle d'un monde (sessions datées). */
  const [temps, setTemps] = useState<{
    qn: string; sessions: number; t0: number; t1: number;
    tranches: { d: number; n: number }[];
  } | null>(null);
  const [axeTemps, setAxeTemps] = useState(false);
  const axeTempsRef = useRef(false);
  axeTempsRef.current = axeTemps;
  /** Types présents dans le graphe courant (sélection de GROUPE). */
  const [typesGraphe, setTypesGraphe] = useState<{ type: string; n: number }[]>([]);
  const [filtreGraphe, setFiltreGraphe] = useState<string | null>(null);
  const filtreRef = useRef<string | null>(null);
  filtreRef.current = filtreGraphe;
  /**
   * ── L'AXE DE NAVIGATION, LU PAR LA BOUCLE ─────────────────────────────────
   * `dimension` est l'axe : selon lui, l'étage courant se découpe par nom, par
   * parenté, par extension, par COMMUNAUTÉ du moteur, par strate ou par courbure
   * (bords ∂ / intérieur). Le geste de descente s'y adosse, donc la boucle doit
   * voir l'axe COURANT — d'où la référence (même raison que `niveau`).
   */
  const dimensionRef = useRef<Dimension>("module");
  dimensionRef.current = dimension;
  /** Ce que portent réellement les vecteurs de signal du graphe courant. */
  const [signaux, setSignaux] = useState<{
    qn: string; n: number; degeneré: boolean;
    courbure: { min: number; max: number; distinctes: number };
    energie: { min: number; max: number; distinctes: number };
    degre: { min: number; max: number; distinctes: number };
  } | null>(null);
  const descenteRef = useRef(0);
  const dernierMajRef = useRef(0);
  /**
   * CADENCE DE LA SPHÈRE ENGLOBANTE.
   *
   * `computeBoundingSphere()` parcourt TOUS les sommets de la nappe (~8 000 à
   * 88 mailles) et était appelé à CHAQUE image — soit un parcours complet par
   * frame, juste après l'upload du tampon de positions. La sphère ne sert qu'au
   * culling : elle doit rester CONSERVATRICE, pas exacte à l'image près. La
   * recalculer deux fois par seconde suffit, et la nappe est de toute façon
   * toujours dans le champ.
   */
  const sphereMajRef = useRef(0);
  descenteRef.current = descente;
  /**
   * ── LA DEMANDE DE NIVEAU DOIT VOYAGER PAR RÉFÉRENCE, PAS PAR LA FERMETURE ──
   *
   * `niveau` est lu DANS la boucle de rendu, or le grand effet de scène a pour
   * dépendances `[weave, layout, onOpen, activePath]` — `niveau` n'y figure pas.
   * La boucle gardait donc la valeur du rendu dans lequel l'effet avait été
   * monté, et `niveau !== niveauAppliqueRef.current` restait faux POUR TOUJOURS.
   *
   * MESURÉ sur la page servie (8432), en cliquant les trois boutons de vue :
   * « 🧠 Monde » puis « 🔺 Graphe » laissaient tous deux `t = 0`, la distance
   * caméra à 1141 → 1131 px (l'orbite au repos, pas un vol), et le bouton actif
   * sur « 🌌 Cosmos ». Les trois vues étaient donc indiscernables — et comme
   * c'est la scène qui rapporte le niveau réel (`onNiveau`), l'indicateur ne
   * pouvait pas non plus se corriger : la boucle de rétroaction était ouverte.
   *
   * On n'ajoute PAS `niveau` aux dépendances de l'effet : le relancer reconstruit
   * la scène entière (les intérieurs retombent à `pret = false` — déjà mesuré et
   * déjà commenté dans `App`). La demande passe donc par une référence, ce qui est
   * exactement ce à quoi `niveauDemandeRef` avait été déclarée — et elle ne
   * l'était nulle part : c'était du code mort, la trace d'un correctif commencé.
   */
  /** Demande de niveau venue des boutons de dimension (0 cosmos, 1 monde, 2 graphe). */
  const niveauDemandeRef = useRef<NiveauCosmos>(0);
  niveauDemandeRef.current = niveau;
  const [playing, setPlaying] = useState(false);
  const densityRef = useRef(1);
  const playingRef = useRef(false);
  const weaveRef = useRef<Weave | null>(null);
  const focusQnRef = useRef<string | null>(null);
  focusQnRef.current = focusQn;
  /** « plonger » : descendre plusieurs niveaux d'un geste. */
  const zoomProfondRef = useRef<(() => void) | null>(null);
  /**
   * CHOISIR UNE CATÉGORIE DOIT DÉPLACER LA VUE.
   *
   * Constat mesuré : cliquer une puce de foyer (`Boot + auto-struct`, `Self`,
   * `Maintenant`, les objectifs) ne changeait le contenu de scène que de
   * moins de 2 points — c'est-à-dire RIEN de visible. La raison est dans le
   * câblage : le clic ne faisait que `setFocusIdx`, dont le seul effet réel est
   * de recalculer les RAYONS D'ORBITE (`orbits.set(wd.qn, { r, … })`) — quelques
   * pour cent. D'où le ressenti « je ne peux pas me déplacer dans les
   * catégories » : c'était exact, le clic ne déplaçait rien.
   *
   * Or un foyer PORTE de quoi agir : `WeaveFocus { kind, qn, label, world }`.
   * On vise donc le monde du foyer — le même chemin que le clic sur un monde
   * (`gotoWorld`), celui qui marche déjà.
   */
  const viserFocusRef = useRef<((f: WeaveFocus) => void) | null>(null);
  /**
   * TA SÉLECTION PRIME SUR LA CIBLE AUTOMATIQUE.
   *
   * Le vol vers un groupe posait bien la cible (`flyTo(…, P)`) — mais la boucle
   * l'ÉCRASAIT à l'image suivante avec la cible qu'elle avait choisie toute
   * seule. Conséquence : impossible de garder un groupe en vue en traversant les
   * échelles, alors que c'est exactement ce qu'on demande à une navigation
   * multi-échelle (voir un objet, le suivre, entrer dedans).
   * Quand tu désignes quelque chose — un monde, un groupe — la caméra le suit,
   * et continue de le suivre au palier suivant.
   */
  const cibleUtilisateurRef = useRef<{ pos: THREE.Vector3; nom: string } | null>(null);
  /**
   * ── FILTRE + HYSTÉRÉSIS : LE REMÈDE À L'OSCILLATION ────────────────────────
   *
   * MESURÉ à grain fin : la descente ne « sautait » pas, elle OSCILLAIT —
   * 48 % → 100 % → 60 % → 99 % → 57 % → 52 % → 100 % → 50 %, soit 20 sauts de
   * plus de 15 points sur 39 pas. C'est cela, « pas viable ».
   *
   * La cause est un ASSERVISSEMENT INSTABLE : la rampe dépend de la distance à
   * une cible que la logique d'étage re-choisit à chaque image. L'étage s'ouvre,
   * la cible change, la rampe se referme, la cible revient — et ainsi de suite.
   * Aucun réglage de seuil ne peut corriger une boucle : il faut casser la boucle.
   *
   *  1. FILTRE du premier ordre sur la grandeur pilotante : elle ne saute plus.
   *  2. HYSTÉRÉSIS : on OUVRE à un seuil, on ne REFERME qu'à un seuil nettement
   *     plus bas. Une fois ouvert, l'étage ne peut donc plus battre.
   */
  const lissageRef = useRef(new Map<string, { v: number; ouvert: boolean }>());
  /**
   * ── POINT DE MESURE INTERNE ────────────────────────────────────────────────
   *
   * J'ai mesuré des PIXELS pendant des heures : ce seul indicateur confond « un
   * étage apparaît » et « la caméra traverse un champ d'objets ». Impossible,
   * avec ça, de dire si une transition est douce — on devine, et j'ai deviné.
   *
   * On expose donc l'ÉTAT RÉEL des étages, image par image : ouverture de chacun,
   * grandeur pilotante, distance, cible. La douceur devient une COURBE qu'on lit
   * et qu'on juge, au lieu d'une impression qu'on défend.
   */
  const etagesRef = useRef<Record<string, unknown>>({});
  const dernierNiveauRef = useRef<NiveauCosmos>(0);
  /** Dernier niveau effectivement APPLIQUÉ par la boucle (évite de rejouer la descente). */
  /**
   * Dernier cran RÉELLEMENT appliqué par la boucle.
   *
   * J'ai tenté d'y poser `-1` (« la caméra n'est plus à une position de cran »)
   * après un recentrage sur un nœud, pour que le clic de cran suivant re-vole.
   * MESURÉ, et c'était PIRE : la boucle comparait alors `niv` (la demande
   * courante, ici 0 = cosmos) à `-1`, concluait à une demande neuve, et RENVOLAIT
   * À L'ESPACE — l'échelle passait de 100 % à 0 % sur un clic de « descendre ».
   * Un sentinelle qui déclenche la ré-application d'une demande n'est pas une
   * sentinelle : c'est une demande. On ne marque donc que ce qui a été appliqué.
   */
  const niveauAppliqueRef = useRef<number>(-1);   // -1 = rien d'appliqué encore
  /**
   * LE RETOUR VERS LE COSMOS.
   *
   * Traversée mesurée (trace CDP) : la descente marchait (Cosmos → Monde →
   * Intérieur → Graphe), mais la REMONTÉE ne marchait QUE par le clavier
   * (`Home` / `↑` → `overview`, qui fait un `flyTo` vers `HOME_POS`).
   * Le clic sur « 🌌 Cosmos » et la touche Échap, eux, se contentaient de
   * remettre l'état à zéro (`setFocusQn(null)`) SANS déplacer la caméra :
   * contenu de scène 100 % avant, 100 % après — on restait enfermé dans le
   * monde, alors que c'est le geste le plus naturel pour remonter.
   *
   * On expose donc la voie qui marche, et les deux commandes l'appellent.
   */
  const retourRef = useRef<(() => void) | null>(null);
  /**
   * ── LE FIL DES POUPÉES : CE QU'ON MONTRE, ET OÙ L'ON EST ──────────────────
   *
   * « Il faut voir les niveaux ENSEMBLE » : le cosmos, le monde, le groupe, ses
   * nœuds. La scène, elle, est une DESCENTE — par construction, elle montre un
   * étage à la fois (un étage est défini par la proximité). Le fil qui manquait
   * n'est donc pas dans la 3D mais à l'écran : un repère permanent qui nomme
   * chaque poupée et marque celle où l'on se trouve. Il vit hors des panneaux
   * repliables (`reperes`), donc il reste visible sur téléphone aussi.
   *
   * `niveauScene` est le niveau RÉELLEMENT mesuré par la scène (`ouvI`/`ouvG`),
   * pas la dimension demandée : l'encart ne doit pas mentir sur ce qu'on voit.
   */
  const [niveauScene, setNiveauScene] = useState<NiveauCosmos>(0);
  const niveauSceneRef = useRef<NiveauCosmos>(0);
  /** Cadrage d'un groupe, exposé à l'encart « poupées » (posé par la boucle). */
  const cadrerGroupeRef = useRef<((qnMonde: string, nom: string) => void) | null>(null);
  /** Recentrage du graphe sur un NŒUD (voir `chargerNoeudRef` plus bas). */
  const chargerNoeudRef = useRef<((qnMonde: string, qnNoeud: string) => void) | null>(null);
  /** Entrer dans un groupe de l'AXE (cadrer + mettre en valeur), exposé au panneau. */
  const entrerAxeRef = useRef<((qnMonde: string, type: string) => boolean) | null>(null);
  /**
   * ── ENTRER DANS UN SOUS-GROUPE / REMONTER D'UN GROUPE ─────────────────────
   *
   * Descendre dans un sous-groupe n'était possible par AUCUN geste : l'étage
   * intérieur ne montrait que les familles racines. Ce sont pourtant des arbres
   * profonds (3 racines → 200 enfants → 60 petits-enfants sur ce store), et
   * « plus de capacité de navigation » veut dire exactement cela : pouvoir entrer,
   * puis entrer encore, et remonter d'un cran sans perdre le fil.
   */
  const entrerGroupeRef = useRef<((qnMonde: string, nom: string, source?: "token" | "classe") => void) | null>(null);
  const remonterGroupeRef = useRef<((qnMonde: string) => void) | null>(null);
  /** Aller DIRECTEMENT à un niveau du chemin (clic sur un maillon du fil). */
  const allerPrefixeRef = useRef<((qnMonde: string, prefixe: string) => void) | null>(null);
  /** Revenir avant la n-ième classe entrée (clic sur un maillon de l'axe). */
  const allerClassesRef = useRef<((qnMonde: string, n: number) => void) | null>(null);
  /** Entrer dans le plus GROS sous-groupe du niveau courant (`Page ↓`). */
  const entrerPlusGrosRef = useRef<((qnMonde: string) => boolean) | null>(null);
  /**
   * ── DENSIFIER LE NIVEAU COURANT — LE GESTE QUI MANQUAIT ────────────────────
   *
   * `⤵` DESCEND d'un cran (préfixe + token, rang + 1) : c'est approfondir, et
   * c'était le seul geste. Densifier est l'AUTRE question — « montre-moi ce qui
   * manque ICI » — et elle se répond en dessinant la page suivante de la matière
   * DÉJÀ reçue : aucun appel, aucune échelle franchie. Rend `false` quand la loi
   * ne propose rien (`densite.ts`), pour que l'appelant le DISE au lieu de ne
   * rien faire.
   */
  const densifierNiveauRef = useRef<((qnMonde: string) => boolean) | null>(null);
  /** Revenir à une ENTRÉE du chemin de graphe (mêmes centres qu'alors). */
  const revenirEntreeRef = useRef<((qnMonde: string, i: number) => void) | null>(null);
  /**
   * ── LA FRATRIE DU GRAPHE CHARGÉ, CONSTRUITE UNE SEULE FOIS ────────────────
   *
   * Coût MESURÉ de l'index de parenté sur des formes de qn réelles :
   * 160 nœuds → 0,4 ms ; 8 000 → 8,9 ms ; 200 000 → 134 ms. Il est donc
   * construit UNE fois par chargement de graphe, À LA DEMANDE (quand on
   * sélectionne un nœud), dans un `setTimeout(0)` — donc APRÈS la peinture de
   * l'image qui vient d'afficher le graphe — et JAMAIS dans la boucle de rendu
   * (celle-ci tourne à 60 Hz et parcourt déjà tous les nœuds et toutes les
   * arêtes). `noeudsGrapheRef` porte la matière première, `indexNoeudRef` le
   * résultat, et `fratrieNoeud` l'expose au rendu.
   */
  const noeudsGrapheRef = useRef<{ qn: string; centre: string; noeuds: GraphNodePayload[] } | null>(null);
  const indexNoeudRef = useRef<{ cle: string; index: IndexFratrie; ms: number } | null>(null);
  const [fratrieNoeud, setFratrieNoeud] = useState<{
    cle: string; index: IndexFratrie; ms: number;
  } | null>(null);
  /**
   * L'ENSEMBLE QUE L'UTILISATEUR A CHOISI D'OUVRIR.
   *
   * Avant, c'était `it.boules[0]` — le plus gros — pris d'office : on ne
   * pouvait pas choisir sa déclinaison, seulement subir la plus peuplée. On
   * retient ici le choix explicite, avec le monde auquel il appartient pour
   * qu'il ne « fuie » pas vers un autre monde en changeant de cible.
   */
  const ensembleChoisi = useRef<{ qn: string; nom: string } | null>(null);
  /**
   * ── LE CHEMIN PARCOURU LE LONG DE L'AXE ────────────────────────────────────
   *
   * La distance n'est pas la seule profondeur. Une fois au dernier cran, on
   * descend dans le groupe que l'AXE désigne, par son nœud le plus important, et
   * le moteur rend le voisinage de ce nœud : un nouveau graphe, qui se re-découpe
   * avec LE MÊME axe. Ce chemin est donc une profondeur à part entière — et
   * jusqu'ici elle était INVISIBLE : rien ne disait où l'on était entré, ni
   * combien de fois, alors que chaque pas change bel et bien le graphe affiché.
   *
   * On garde la suite des entrées (axe · groupe · nœud). Elle ne se réinitialise
   * qu'au CHANGEMENT D'ÉCHELLE (ou au retour à l'espace) : un chemin qui
   * s'oublierait en route ferait croire à une descente sans profondeur, et un
   * chemin qui survivrait à un changement d'échelle mentirait sur l'endroit d'où
   * l'on vient.
   */
  const cheminAxeRef = useRef<{ axe: string; groupe: string; noeud: string }[]>([]);
  /**
   * LA DESCENTE EN COURS LE LONG DE L'AXE — et elle ne s'ajoutera au chemin QUE
   * SI ELLE ABOUTIT.
   *
   * MESURÉ : le geste peut être refusé (un chargement du même mode est déjà en
   * vol) ou rendre exactement le même voisinage. Ajouter la poupée au moment du
   * GESTE faisait donc grandir le fil sans que le graphe change : une profondeur
   * annoncée et non produite. Le fil dit maintenant ce qui s'est réellement
   * passé — il ne s'allonge que sur l'arrivée d'un graphe différent.
   */
  const descentePendanteRef = useRef<{ axe: string; groupe: string; noeud: string; avant: string } | null>(null);
  const [cheminAxe, setCheminAxe] = useState<{ axe: string; groupe: string; noeud: string }[]>([]);
  /** Le chemin des entrées de GRAPHE (voir `Graphe.entrees`), pour l'afficher. */
  const [cheminGraphe, setCheminGraphe] = useState<{ nom: string; centres?: string[] }[]>([]);
  /** Un chargement de graphe est en vol (publié par la boucle, toutes les 200 ms). */
  const [chargementEnVol, setChargementEnVol] = useState(false);
  focusQnRef.current = focusQn;
  // Énergie par chemin de store, lue par la boucle de rendu (allumage des mondes).
  const energyRef = useRef<Map<string, number>>(new Map());
  const activationRef = useRef<Activation | null>(null);
  activationRef.current = activation;
  energyRef.current = new Map((activation?.worlds || []).map((w) => [w.store, w.peak]));
  densityRef.current = density;
  playingRef.current = playing;
  weaveRef.current = weave;

  /** Étiquette d'un nœud selon la dimension choisie (dérivée du qn réel). */
  /** Communauté réelle d'un nœud (du moteur), quand il en porte une. */
  const [communautes, setCommunautes] = useState<Map<string, number>>(new Map());
  /**
   * AXE ∂ — le CONTOUR STRUCTUREL : les nœuds de courbure négative sont les
   * bords du graphe (frontières entre régions), les autres sont l'intérieur.
   * C'est une lecture réelle quand le store porte une courbure ; sur les autres,
   * l'axe disparaît (une seule catégorie) au lieu de mentir.
   */
  const [courbures, setCourbures] = useState<Map<string, number>>(new Map());
  /**
   * ── LES AXES SE LISENT PAR RÉFÉRENCE, PAS PAR LA FERMETURE ──────────────────
   *
   * MÊME PIÈGE QUE `niveau`, ET MESURÉ DE LA MÊME FAÇON : `etiquetteDimension`
   * est appelée DEPUIS LA BOUCLE DE RENDU, qui est capturée par l'effet de scène
   * au moment de son montage. En lisant `communautes` et `courbures` (des états
   * React), elle y voyait donc les cartes du PREMIER rendu — c'est-à-dire des
   * cartes VIDES, puisque le graphe n'était pas encore chargé. Les axes
   * « communauté » et « ∂ » auraient répondu « sans communauté » / « sans
   * courbure » pour tous les nœuds, dans la boucle, alors que le panneau les
   * affichait correctement : deux réponses pour la même question.
   *
   * `fichiersRef` avait déjà été posée pour cette raison exacte. On aligne les
   * deux autres : UNE implémentation (`etiquetteDimension`), et des références.
   */
  const communautesRef = useRef(communautes);
  communautesRef.current = communautes;
  const courburesRef = useRef(courbures);
  courburesRef.current = courbures;
  /**
   * LE CHEMIN DE FICHIER RÉEL DE CHAQUE NŒUD DU GRAPHE CHARGÉ.
   *
   * La charge utile le porte (`/api/viz/subgraph` → `nodes[].file_path`) et
   * personne ne le lisait : la parenté se déduisait donc du seul qn. Or le qn
   * d'un fichier indexé est un chemin POINTÉ dont le nom contient lui-même des
   * points (`…benches.query_bench.rs.__file__`) : aucune découpe du qn ne peut
   * distinguer le dossier du nom de fichier quand le nom en contient.
   * `parentDe` privilégie donc la donnée, et retombe sur le qn quand elle manque.
   */
  const fichiersRef = useRef<Map<string, string>>(new Map());
  /**
   * LES BOULES RÉELLEMENT DESSINÉES, lisibles DEPUIS LA BOUCLE.
   *
   * La liste vit dans l'état React, mais la boucle de rendu a été construite
   * une fois (`useEffect` aux dépendances `[weave, layout, onOpen, activePath]`)
   * : elle lirait un état périmé. Le ref, lui, est toujours à jour — c'est lui
   * qui permet à la boucle d'EXPORTER ce qu'elle montre (combien de frères
   * nommés pour combien de boules), au lieu de le supposer.
   */
  const interieurBoulesRef = useRef<{ name: string; count: number }[]>([]);
  const etiquetteDimension = (qn: string, dim: Dimension): string => {
    if (dim === "communauté") {
      const c = communautesRef.current.get(qn);
      return c === undefined ? "sans communauté" : `communauté ${c}`;
    }
    if (dim === "∂") {
      const c = courburesRef.current.get(qn);
      if (c === undefined) return "sans courbure";
      return c < 0 ? "bord ∂" : "intérieur";
    }
    if (dim === "couche") {
      // « couche » = la strate du nom : le nombre de segments sépare le projet
      // de la feuille (projet › paquet › dossier › fichier).
      return `strate ${strateDe(qn)}`;
    }
    // ── UNE SEULE RÈGLE DE PARENTÉ (voir ./fratrie.ts) ─────────────────────
    // `module` et `parent` étaient recalculés ici, en désaccord avec
    // `gr.noeudsInfo[].type` (voir plus bas). Ils viennent maintenant du même
    // module, donc les deux ne peuvent plus diverger.
    const chemin = fichiersRef.current.get(qn);
    if (dim === "module") return moduleDe(qn);
    if (dim === "parent") return parentDe(qn, chemin);
    // « extension » = dernier segment utile ; pour un nœud FICHIER c'est bien
    // l'extension (`…rs.__file__` → « rs »). Pour un nœud qui n'est pas un
    // fichier, ce segment est son nom — comportement historique conservé.
    const f = fichierDe(qn, chemin);
    if (f) {
      const i = f.lastIndexOf(".");
      if (i > 0) return f.slice(i + 1);
    }
    const parts = qn.split(/[./]/).filter(Boolean);
    const utiles = parts.filter((x) => x !== "__file__");
    const src = utiles.length ? utiles : parts;
    const t = (src[src.length - 1] || "?").replace(/^_+$/, "?");
    return t.length > 22 ? t.slice(0, 22) : t;
  };

  const [weaveToken] = useAtom(weaveTokenAtom);
  useEffect(() => {
    let live = true;
    // `frais` dès qu'un monde a été ajouté : sans cela la toile resterait en
    // cache 30 s et le monde ajouté n'apparaîtrait pas.
    fetchWeave(undefined, undefined, weaveToken > 0)
      .then((w) => { if (live) setWeave(w); })
      .catch((e) => { if (live) setErr(String(e?.message || e).slice(0, 200)); });
    return () => { live = false; };
  }, [weaveToken]);

  const foci: WeaveFocus[] = useMemo(() => weave?.foci || [], [weave]);
  const focus = foci[focusIdx] || null;
  /** Le choix de dimension par l'utilisateur est respecté une fois exprimé. */
  const dimensionChoisie = useRef(false);
  const grouper = (qns: string[], dim: Dimension) => {
    const compte = new Map<string, number>();
    for (const qn of qns) {
      const t = etiquetteDimension(qn, dim);
      compte.set(t, (compte.get(t) ?? 0) + 1);
    }
    return [...compte.entries()].map(([type, n]) => ({ type, n })).sort((a, b) => b.n - a.n);
  };
  useEffect(() => {
    if (!qnsGraphe.length) { setTypesGraphe([]); return; }
    // DIMENSION PAR DÉFAUT CHOISIE SUR MESURE. Constaté : « module » donnait UN
    // seul groupe sur ce graphe (tout un projet), donc AUCUNE puce de sélection
    // n'apparaissait — l'utilisateur arrivait dans un étage où il ne pouvait
    // rien faire ressortir. On prend donc, par défaut, la dimension qui
    // distingue le plus de groupes ; dès que l'utilisateur choisit, on ne
    // touche plus à son choix.
    if (!dimensionChoisie.current) {
      let meilleure: Dimension = dimension;
      let max = 0;
      for (const d of ["module", "parent", "extension", "communauté", "couche", "∂"] as const) {
        const n = grouper(qnsGraphe, d).length;
        if (n > max) { max = n; meilleure = d; }
      }
      if (meilleure !== dimension) { setDimension(meilleure); return; }
    }
    setTypesGraphe(
      grouper(qnsGraphe, dimension).sort((a, b) =>
        a.type === "sans communauté" ? 1 : b.type === "sans communauté" ? -1 : b.n - a.n
      ),
    );
    setFiltreGraphe(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [qnsGraphe, dimension, communautes, courbures]);

  /**
   * ── LA FRATRIE : CONSTRUITE UNE FOIS PAR GRAPHE, HORS DE L'IMAGE ──────────
   *
   * ── POURQUOI LE DÉCLENCHEUR N'EST PAS « un nœud est visé » ────────────────
   * Ma première version exigeait `vise.genre === "noeud"` : la fratrie
   * n'apparaissait donc qu'après avoir touché un POINT de quelques pixels dans
   * la nuée de graphe. Mesuré : cette condition rendait le panneau presque
   * inatteignable — et le geste naturel (cliquer une BOULE, ce qui charge le
   * graphe du groupe) ne l'ouvrait pas. Le déclencheur est donc le GRAPHE
   * lui-même : dès qu'un graphe est affichable, son centre est un élément que
   * l'utilisateur a désigné par un clic sur une boule.
   *
   * ── LE COÛT ──────────────────────────────────────────────────────────────
   * Une construction par graphe, jamais par image (garde `cle`), différée de
   * 120 ms APRÈS l'affichage du graphe : la peinture et les premières images de
   * la descente ne la paient pas. Coût mesuré de `construireIndex` (formes de
   * qn réelles, navigateur) : 160 nœuds → 0,6 à 10 ms ; 8 000 → 20 à 155 ms ;
   * 200 000 → 0,5 à 1,5 s sur une machine chargée (134 ms au repos, mesuré par
   * l'audit). L'index est LOCAL : aucune requête n'est émise.
   */
  useEffect(() => {
    const g = noeudsGrapheRef.current;
    if (!g || !g.noeuds.length) return;
    const cle = `${g.qn}|${g.centre}|${g.noeuds.length}`;
    if (indexNoeudRef.current?.cle === cle) return;
    let annule = false;
    const t = window.setTimeout(() => {
      if (annule) return;
      const index = construireIndex(aIndexer(g.noeuds));
      indexNoeudRef.current = { cle, index, ms: index.ms };
      setFratrieNoeud({ cle, index, ms: index.ms });
    }, 120);
    return () => { annule = true; window.clearTimeout(t); };
    // `graphe` est remplacé à chaque chargement : une construction par graphe.
  }, [graphe, vise]);

  // ── DISPOSITION : le foyer est l'étoile, les mondes orbitent ───────────
  const layout: Layout | null = useMemo(() => {
    const ws = weave?.worlds || [];
    if (!ws.length) return null;
    const fibers = weave?.fibers || [];
    // Si le foyer est un monde, l'étoile EST ce monde : les autres s'ordonnent
    // selon le POIDS RÉEL de leur fibre avec lui (attraction, pas décoration).
    const starWorld = focus?.world
      ? ws.find((w) => w.label === focus.world) || null
      : null;
    const starQn = starWorld?.qn || null;
    const weightTo = (qn: string) => {
      let w = 0;
      for (const f of fibers) {
        if (f.a === starQn && f.b === qn) w += f.weight;
        else if (f.b === starQn && f.a === qn) w += f.weight;
      }
      return w;
    };
    const maxW = Math.max(1, ...fibers.map((f) => f.weight));
    const orbits = new Map<string, { r: number; inc: number; phase: number; w: number; size: number }>();
    ws.forEach((wd, i) => {
      const h = hash(wd.slug || wd.qn);
      let attraction: number;
      if (starQn) {
        // Attirance réelle vers l'étoile : une fibre lourde rapproche.
        attraction = weightTo(wd.qn) / maxW;
      } else {
        // Sans foyer-monde : la gravité globale du monde (liens réels).
        attraction = wd.gravity ?? 0;
      }
      const r = 200 + (1 - Math.min(1, attraction)) * 370;
      // Orbite képlérienne : plus on est loin, plus on tourne lentement.
      const w0 = 0.00016 * Math.pow(200 / r, 1.5);
      orbits.set(wd.qn, {
        r,
        inc: (h - 0.5) * 0.55,
        phase: h * Math.PI * 2 + i * 0.9,
        w: w0,
        size: 15 + 46 * (wd.mass ?? 0.3),
      });
    });
    return { orbits, starQn };
  }, [weave, focus]);

  // ── SCÈNE ──────────────────────────────────────────────────────────────
  useEffect(() => {
    const el = host.current;
    const wv = weave;
    const worlds = wv?.worlds || [];
    if (!el || !wv || !worlds.length || !layout) return;

    const scene = new THREE.Scene();
    scene.background = new THREE.Color(0x05070c);
    // Brouillard repoussé : mesuré à la capture, l'ancienne plage (1400→3400)
    // noyait les mondes (distants de ~1500-2200 du point de vue) dans le fond.
    scene.fog = new THREE.Fog(0x05070c, 2600, 5400);
    const W = el.clientWidth || 800, H = el.clientHeight || 600;
    const camera = makeCamera(W, H);
    // Distance de cadrage : le système fait ~1300 unités de diamètre. À 1600
    // unités de recul, une sphère de 20 unités ne faisait que ~10 px et les
    // points des fibres tombaient SOUS LE PIXEL (sizeAttenuation = taille en
    // unités-monde). On se rapproche donc, et les tailles sont relevées.
    camera.position.set(0, 520, 1180);
    // Relief COMMUN : fond + brouillard proportionnel à la distance de cadrage.
    applyRelief(scene, camera.position.length());
    const renderer = makeRenderer(W, H);
    renderer.domElement.style.touchAction = "none";
    el.appendChild(renderer.domElement);
    // Profil de caméra COMMUN aux vues (perspective, amorti, bornes) :
    // changer de vue ne doit plus changer la sensation de profondeur.
    // minDistance descendu : on doit pouvoir entrer DANS un amas de nœuds, sinon
    // l'étage des signaux est inatteignable (mesuré : la borne de 90 bloquait
    // la caméra à 2,2 rayons de monde, exactement le seuil du dernier étage).
    // minDistance à 2 : la cellule se visite à 0,25 rayon d'un monde, et un
    // petit monde (rayon 20) demanderait 5 — une borne plus haute que cela
    // interdirait d'entrer, exactement le défaut qu'on corrige.
    const ctrl = orbitFor(camera, renderer.domElement, {
      minDistance: 2, maxDistance: 4200,
      // La descente couvre 56× de distance : il faut que quelques crans de
      // molette suffisent à la traverser, sinon on zoome sans changer d'échelle.
      zoomSpeed: 5,
    });
    ctrl.target.set(0, 0, 0);

    // ── L'ÉLÉMENT CENTRAL EST RETIRÉ ──────────────────────────────────────
    // Constat, vérifié dans le code : il n'apportait AUCUNE fonction.
    //   - la `PointLight` n'éclairait rien : toutes les matières de cette scène
    //     sont non éclairées (`MeshBasicMaterial` et mes propres shaders), donc
    //     elle coûtait un upload d'uniforme par image pour zéro pixel ;
    //   - la sphère et le halo ne servaient qu'à décorer le centre ;
    //   - et surtout, occuper le centre gêne la navigation : c'est là qu'on
    //     regarde quand on descend, et un objet opaque y masque ce qu'on vient
    //     voir.
    // Le FOYER garde tout son sens (il détermine les orbites et les gravités),
    // mais il n'a plus d'objet à l'écran. Un réglage permet de le faire
    // réapparaître pour qui veut situer le centre.
    const starQn = layout.starQn;
    const starWorld = worlds.find((w) => w.qn === starQn) || null;
    const starCol = new THREE.Color(starWorld ? PLAN_COLOR[starWorld.nature] || 0xffd166 : 0xfff0b8);
    const star = new THREE.Mesh(
      new THREE.SphereGeometry(26, 16, 12),
      new THREE.MeshBasicMaterial({ color: starCol, transparent: true, opacity: 0 }),
    );
    star.visible = false;
    scene.add(star);
    const halo = new THREE.Sprite(new THREE.SpriteMaterial({
      color: starCol, transparent: true, opacity: 0, blending: THREE.AdditiveBlending, depthWrite: false,
      visible: false,
    }));
    halo.scale.set(230, 230, 1);
    halo.visible = false;
    scene.add(halo);

    // ── la NAPPE : le sol s'affaisse à la masse ──
    // ── LA NAPPE EST UN TISSU, PAS UNE GRILLE ──
    //
    // Ce qui se voyait à l'écran, c'était la GRILLE : une plane 46x46 rendue en
    // wireframe à opacité 0,78 — donc une suite de cellules rectangulaires bleu
    // ciel, alignées. Aucune synapse là-dedans : les mailles elles-mêmes.
    //
    // Traitement de tissu, dans cet ordre :
    //  1. PLUS FIN : 46 -> 104 mailles par côté (le fil se resserre) ;
    //  2. PLUS ORGANIQUE : chaque sommet dérive latéralement d'une fraction de
    //     maille, tirée d'un hachage déterministe -> les cellules deviennent des
    //     quadrilatères irréguliers, la trame alignée disparaît, et la
    //     déformation de masse hérite de cette base irrégulière ;
    //  3. DENSITÉ AU LIEU DE TRAIT : opacité 0,78 -> 0,11 en mélange ADDITIF.
    //     Ce n'est plus le fil qui fait la lumière mais la SUPERPOSITION : les
    //     zones denses (sous les modules, là où le sol s'affaisse) s'éclairent
    //     d'elles-mêmes — c'est la densité de tissage qui devient visible.
    // 88 mailles : en ADDITIF, chaque segment ajoute de la lumiere. Passer de 46
    // a 104 avait DOUBLE le nombre de lignes (~21 800) et donc la saturation :
    // « plus fin » ne veut PAS dire « plus de lignes » quand le melange est
    // additif. On resserre le fil sans empiler un mur.
    const SEG = 88, SPAN = 2600;
    // ── LA TOILE ENGLOBE AU LIEU DE PORTER ──
    //
    // Elle était un SOL : plane, 300 unités sous les mondes, qui s'affaisse sous
    // leur masse. On n'en lisait donc qu'une face, en plongée — et sa déformation
    // ne racontait qu'une moitié de l'information.
    //
    // On la courbe en COUPE : la périphérie remonte, franchit les mondes, et la
    // nappe devient une membrane qui les ENVELOPPE. Les puits se lisent alors de
    // l'intérieur comme de l'extérieur, et le relief thermodynamique se déploie
    // sur toute la surface au lieu d'un seul plan.
    //
    // Réglage : 620 place le bord au-dessus des mondes. Plus haut = plus
    // enveloppant (risque d'occulter la caméra) ; plus bas = retour au sol.
    const LIFT = 620;
    const sheetGeo = new THREE.PlaneGeometry(SPAN, SPAN, SEG, SEG);
    sheetGeo.rotateX(-Math.PI / 2);
    {
      const pos = sheetGeo.attributes.position as THREE.BufferAttribute;
      const maille = SPAN / SEG;
      let seed = 0x9e3779b9;
      for (let i = 0; i < pos.count; i++) {
        seed = Math.imul(seed ^ i, 16777619) >>> 0;
        const rx = ((seed % 1000) / 1000 - 0.5) * maille * 0.85;
        const rz = (((seed >>> 11) % 1000) / 1000 - 0.5) * maille * 0.85;
        pos.setX(i, pos.getX(i) + rx);
        pos.setZ(i, pos.getZ(i) + rz);
      }
      pos.needsUpdate = true;
    }
    const sheetBase = Float32Array.from(sheetGeo.attributes.position.array);
    const sheetCols = new Float32Array(sheetGeo.attributes.position.count * 3);
    sheetGeo.setAttribute("color", new THREE.BufferAttribute(sheetCols, 3));
    // ── LE VOILE APPARTIENT AU LOIN, PAS AUX PIEDS ──────────────────────────
    // Constat (utilisateur) : « le voile en dessous est trop visible ». C'était
    // cette nappe — une grille filaire de SPAN × SPAN posée à y = −300, en
    // MeshBasicMaterial d'opacité fixe. Elle se lisait donc comme un FILET tendu
    // sous la scène, à toute distance, y compris sous les pieds, et elle
    // participait au « c'est grossier » : mesuré, la scène nue occupait 41,5 % de
    // l'écran.
    //
    // C'est un contraste de sol : il doit se lire comme un HORIZON, pas comme un
    // tapis. Un shader l'éteint donc de près — le voile n'apparaît qu'au-delà de
    // la distance où il signifie quelque chose (le relief lointain), et le sol
    // immédiat redevient du vide, ce qu'il est.
    const sheet = new THREE.Mesh(sheetGeo, new THREE.ShaderMaterial({
      uniforms: {
        // LA VALEUR DÉCLARÉE, ENFIN RESPECTÉE. La matière annonçait 0,075 mais
        // les trois sites d'animation écrasaient l'opacité à 0,78 — DIX FOIS plus
        // — dès la première image. Le 0,075 était un mensonge du code, et c'est
        // lui qui rendait le voile « trop visible ». On part de l'intention.
        uOpacite: { value: 0.09 },
        uProche: { value: 900.0 },   // en deçà : la nappe s'éteint
      },
      vertexShader: `
        // NE PAS déclarer l attribut de couleur ici : avec vertexColors a true,
        // three.js injecte DEJA "attribute vec3 color;". Le redeclarer produisait
        // une DOUBLE DECLARATION — mesure : glError 1282 (GL_INVALID_OPERATION)
        // et des erreurs en console.
        varying vec3 vCouleur;
        varying float vDist;
        void main() {
          vCouleur = color;
          vec4 mv = modelViewMatrix * vec4(position, 1.0);
          vDist = -mv.z;
          gl_Position = projectionMatrix * mv;
        }`,
      fragmentShader: `
        uniform float uOpacite;
        uniform float uProche;
        varying vec3 vCouleur;
        varying float vDist;
        void main() {
          // Fondu de proximité : 0 sous les pieds, plein à uProche.
          float proche = smoothstep(uProche * 0.25, uProche, vDist);
          float a = proche * uOpacite;
          if (a < 0.0015) discard;   // sous les pieds : rien, pas un filet
          gl_FragColor = vec4(vCouleur, a);
        }`,
      transparent: true,
      vertexColors: true,
      blending: THREE.AdditiveBlending,
      depthWrite: false,
    }));
    sheet.position.y = -300;
    scene.add(sheet);
    const SHEET_Y = -300;

    // ── ÉTAGE INTÉRIEUR : l'organisation d'une base, DANS la boule ───────
    // « L'approche révèle progressivement les étages. » Chaque monde porte donc
    // un groupe intérieur vide, peuplé À L'AVANCE (dès qu'on s'approche, pas
    // quand on est arrivé) : c'est ce qui garantit qu'un étage est peuplé AVANT
    // d'être montré. Si le chargement échoue, la boule reste — jamais de vide.
    interface Interieur {
      grp: THREE.Group;
      charge: boolean;   // chargement lancé
      pret: boolean;     // contenu réellement construit
      /**
       * OÙ CHAQUE GROUPE SE TROUVE, ET DE QUELLE TAILLE IL EST.
       *
       * Les nuées étaient des positions SANS dimension : on pouvait les cliquer
       * (raycast) mais pas dire « ce groupe occupe CETTE région de la boule ».
       * Or c'est exactement ce qu'il faut pour l'emboîtement : sans centre ni
       * rayon, le graphe d'un groupe ne peut pas s'ouvrir *dans* son groupe —
       * il s'ouvre à l'échelle du monde entier (`R = taille × 1,15`), et le
       * groupe cesse alors d'être représenté comme un groupe.
       */
      boules: {
        name: string; count: number; pts: THREE.Points;
        /** Centre du groupe, dans l'espace LOCAL de la boule du monde. */
        centre: THREE.Vector3;
        /** Rayon de la région occupée par le groupe (même espace). */
        rayon: number;
      }[];
      /** L'enveloppe du groupe VISÉ : la deuxième poupée, celle qu'on ouvre. */
      enveloppe?: THREE.LineSegments;
      /**
       * ── LE CHEMIN PARCOURU DANS LA HIÉRARCHIE ─────────────────────────────
       *
       * L'organisation d'un monde n'est pas UNE liste : c'est un ARBRE. Le
       * serveur le sait mieux que personne — `/api/viz/space` agrège
       * `split(qn,'.')[rang]` sous un PRÉFIXE, et la descente est donc un
       * CHEMIN de tokens (`fractale.ts` : `prefixe(profondeur) = prefixe(parent)
       * + "." + nom`, `rang(profondeur) = rangRacine + profondeur`).
       * L'étage intérieur ne s'arrêtait pourtant qu'aux familles RACINES (6 sur
       * ce store) : les sous-groupes — 200 sous `deepseek_harness`, 60 sous
       * `…packages` — n'étaient atteignables par AUCUN geste.
       * `prefixe` et `rang` portent donc la position courante dans cet arbre, et
       * c'est elle qui décide ce que l'étage montre.
       */
      prefixe: string;
      rang: number;
      /**
       * ── LA PAGE DE CE NIVEAU, ET TOUT CE QUE L'APPEL A RENDU ──────────────
       *
       * DÉFAUT MESURÉ (2026-09-15) : l'organisation était coupée à 24 groupes
       * AVANT qu'on lise sa longueur (`slice(0, FRACTALE.groupesMax)`), alors
       * que `/api/viz/space` en sert jusqu'à 200. Le panneau annonçait donc
       * « Intérieur : 24 ensembles » — son `total` valait 24 lui aussi — et les
       * 176 autres n'étaient atteignables par AUCUN geste : le seul geste
       * existant, `⤵`, descend d'un CRAN, ce qui n'est pas la même question.
       *
       * `tous` = la matière reçue (le niveau), `page` = ce qu'on en dessine.
       * Densifier devient alors un REDESSIN (aucun appel), et `total > montres`
       * est la preuve visible qu'un plafond n'a pas mangé la donnée.
       */
      tous: { name: string; count: number }[];
      page: number;
      /** Le serveur a coupé à son propre plafond : la page suivante est chez lui. */
      plafondAtteint: boolean;
      /**
       * ── CE QUE LE SKIP DU SERVEUR DOIT COMPTER : DES LIGNES BRUTES ────────
       * `tous` est FILTRÉ (tokens valides, effectifs > 1) ; `SKIP` du serveur, lui,
       * porte sur des lignes BRUTES. Densifier au-delà du plafond avec
       * `offset = tous.length` ramènerait donc des lignes déjà connues — mesuré sur
       * `runtime-dev` : 200 lignes brutes, un nom vide de 8 438 nœuds écarté, soit
       * 50 lignes de recouvrement par page. On tient donc le compte BRUT à part.
       */
      offsetBrut: number;
      /**
       * ── LE BUDGET DE CENTRES DU NIVEAU (densifier un ÉCHANTILLON) ─────────
       * Quand les groupes d'un niveau viennent du GRAPHE et non de l'agrégat, le
       * « plus » n'est ni un `offset` ni un cran : c'est un échantillon plus large,
       * et le seul levier mesuré est le nombre de CENTRES (le budget `limit` est
       * déjà à son maximum utile, et `hops=3` coûte 10× pour zéro nœud de plus).
       * Il double à chaque densification — donc le geste fait ce qu'il annonce.
       */
      centresGraphe: number;
      /**
       * Combien de pages serveur d'affilée n'ont apporté AUCUN groupe d'effectif
       * > 1 (voir `densifierNiveauRef`) : c'est ce qui borne l'offre au lieu de
       * laisser un geste tourner à vide.
       */
      pagesVides?: number;
      /**
       * ── LA SOURCE DU DÉCOUPAGE, RETENUE ───────────────────────────────────
       * Les groupes d'un niveau viennent SOIT de l'agrégat du serveur, SOIT des
       * classes du graphe chargé (axe de relation, token suivant). Recalculer ce
       * choix à chaque redessin le faisait CHANGER sous le geste — mesuré par le
       * contrôle : « 24 sur 56 » devenait « 8 sur 8 » au même préfixe. On le
       * retient donc, et un redessin redessine le même découpage.
       * `arcs` : les fibres du niveau, gardées pour qu'un redessin n'ait rien à
       * demander au serveur.
       */
      sourceAxe: { name: string; count: number }[] | null;
      arcs: { src: string; dst: string; weight: number }[];
      /** Le niveau est-il une FEUILLE (plus de sous-groupes exploitables) ? */
      feuille?: boolean;
      /**
       * L'ENTRÉE DU MONDE (celle du niveau racine) — retenue pour pouvoir y
       * REVENIR : en remontant à la racine, le graphe doit reprendre l'entrée du
       * monde, pas rester sur le sous-groupe qu'on vient de quitter.
       */
      entreeRacine?: string;
      /**
       * ── LA CHAÎNE DES CLASSES (l'autre coordonnée) ────────────────────────
       * Le CHEMIN dit où l'on est dans l'arbre ; les CLASSES disent comment on a
       * découpé ce qu'on y voit (communauté, contour ∂, parent…). Un niveau est
       * l'INTERSECTION des deux. Sans cela, choisir un axe REMPLAÇAIT le niveau —
       * il découpait TOUT le graphe — et l'on perdait sa place : l'organisation
       * n'était donc pas « à tous les niveaux », elle n'existait qu'au premier.
       */
      classes: { axe: string; valeur: string }[];
    }
    const interieurs = new Map<string, Interieur>();
    /** La paroi de chaque monde : elle se ferme quand on entre. */
    const cages = new Map<string, THREE.Mesh>();
    /** Étage GRAPHE : le graphe RÉEL du plus gros ensemble d'un monde. */
    interface Graphe {
      grp: THREE.Group; charge: boolean; pret: boolean;
      noeuds: number; aretes: number; types: string[]; motif: string;
      /**
       * ── LE BUDGET DE CENTRES QUI A PRODUIT CE GRAPHE ───────────────────────
       * Gardé pour que le garde « même centre » puisse distinguer un simple
       * redemander d'une DENSIFICATION : le même niveau redemandé avec plus de
       * centres est un échantillon plus large, pas un doublon (voir `densifie`).
       */
      centresUtilises?: number;
      /** L'ENSEMBLE dont ce graphe est le contenu — l'autorité de l'emboîtement. */
      ensemble?: string;
      /** Où est ce groupe (espace local de la boule) et de quel facteur réduire. */
      centreGroupe?: THREE.Vector3;
      echelle?: number;
      /** Position dans la boule (graphe) et position en ESPACE DES SIGNAUX. */
      posGraphe?: Float32Array;
      posSignaux?: Float32Array;
      /** Paires d'indices des arêtes : elles doivent suivre les nœuds. */
      paires?: [number, number][];
      /** qn et TYPE de chaque nœud : sert à sélectionner par groupe. */
      noeudsInfo?: { qn: string; type: string }[];
      /**
       * Les Points des NŒUDS, gardés pour le raycast. Sans référence, un nœud
       * affiché n'était pas désignable : seule la boule du monde l'était, donc
       * l'étage graphe restait une image qu'on regarde, pas une carte qu'on
       * parcourt. `noeudsInfo` donne le qn depuis l'index touché.
       */
      pts?: THREE.Points;
      /** Pour QUEL centre le qn ci-dessus a été résolu (voir `dejaResolu`). */
      entreePour?: string;
      /** Vrai quand l'entrée n'a rien donné d'exploitable (dit à l'utilisateur). */
      degrade?: boolean;
      /**
       * LES ARÊTES, gardées pour le raycast. Elles portent le SENS du graphe —
       * type de relation, poids, courbure de Ricci — et n'étaient pourtant pas
       * désignables : on pouvait voir une relation sans jamais l'interroger.
       * `paires` (déjà présent) donne les deux index de nœuds du segment touché,
       * et `noeudsInfo` leurs qn : la résolution d'un clic est directe.
       */
      lignes?: THREE.LineSegments;
      /** Phase COMPLÈTE (motifs) chargée ? */
      chargeComplet?: boolean;
      pretComplet?: boolean;
      /**
       * ── LE CHEMIN DES ENTRÉES DE GRAPHE ───────────────────────────────────
       *
       * Un clic sur un nœud RECENTRE le graphe sur son voisinage : c'est déjà une
       * descente, et on peut la répéter. Mais rien ne gardait le trajet : après
       * cinq clics, on ne savait plus d'où l'on venait, ni comment revenir — il
       * fallait cliquer « en arrière » à l'aveugle en espérant retomber juste.
       * Chaque entrée réussie s'ajoute donc au chemin, et chaque maillon y ramène
       * (mêmes `centres` qu'à l'époque : la reprise est exacte, pas approchée).
       */
      entrees?: { nom: string; centres?: string[] }[];
      /**
       * LE qualified_name RÉELLEMENT RÉSOLU pour l'ensemble visé.
       *
       * Le nom d'ensemble rendu par `/api/viz/modules` est une FAMILLE
       * (`deepseek_harness`), pas un nœud : le sous-graphe la refuse (400). La
       * résolution par recherche donne le vrai qn — et elle doit être payée UNE
       * fois pour les DEUX phases (rapide puis motifs), sinon la seconde refait
       * l'appel voué au refus et l'étage graphe reste vide deux fois.
       */
      entree?: string;
      /** AXE DU TEMPS : groupe des périodes réelles du monde. */
      chargeTemps?: boolean;
      pretTemps?: boolean;
      grpTemps?: THREE.Group;
      /** Ce que portent réellement les vecteurs de signal de ce graphe. */
      sig?: { courbure: { min: number; max: number; distinctes: number };
              energie: { min: number; max: number; distinctes: number };
              degre: { min: number; max: number; distinctes: number };
              degeneré: boolean };
    }
    const graphes = new Map<string, Graphe>();

    // ── MONDES ──
    const worldNodes = new Map<string, { pos: THREE.Vector3; mesh: THREE.Mesh; label: THREE.Sprite; r: number }>();
    for (const wd of worlds) {
      const o = layout.orbits.get(wd.qn)!;
      const col = new THREE.Color(PLAN_COLOR[wd.nature] || 0x8fa6bd);
      // BOULES DE PARTICULES (comme la vue Monde) : un monde n'est plus une
      // sphère pleine mais un NUAGE de points colorés par sa composition
      // réelle. C'est la primitive partagée des étages « toile » et « monde » —
      // sans elle, passer d'un monde à l'autre changeait de nature visuelle et
      // la navigation continue n'avait rien à quoi s'accrocher.
      const mesh = new THREE.Mesh(
        new THREE.SphereGeometry(o.size * 0.34, 20, 14),
        new THREE.MeshBasicMaterial({ color: col, transparent: true, opacity: 0.9 }),
      );
      const nuage = (() => {
        const types = wd.types || [];
        const total = Math.max(1, types.reduce((a, b) => a + b.count, 0));
        const n = Math.min(4200, Math.max(320, Math.round(Math.sqrt(wd.nodes || 100) * 22)));
        const pos2 = new Float32Array(n * 3);
        const col2 = new Float32Array(n * 3);
        // Répartition PROPORTIONNELLE à la part réelle de chaque type : chaque
        // particule tombe dans la bande radiale de son type. (La version
        // précédente calculait les indices bande par bande et se recouvrait :
        // les dernières bandes écrasaient les premières.)
        const parts = types.slice(0, 8).map((t) => Math.max(0.0001, t.count / total));
        const cumul: number[] = [];
        parts.reduce((a, p2) => { const v = a + p2; cumul.push(v); return v; }, 0);
        const sommeCumul = cumul[cumul.length - 1] || 1;
        for (let i = 0; i < n; i++) {
          const u = i / n;
          let bi = cumul.findIndex((c) => u <= c / sommeCumul);
          if (bi < 0) bi = cumul.length - 1;
          const r0 = o.size * 0.34 + (o.size * 0.66) * (bi === 0 ? 0 : cumul[bi - 1] / sommeCumul);
          const r1 = o.size * 0.34 + (o.size * 0.66) * (cumul[bi] / sommeCumul);
          const c = new THREE.Color(typeColor(bi));
          const a2 = Math.random() * Math.PI * 2;
          const ph = Math.acos(2 * Math.random() - 1);
          const rr = r0 + Math.random() * Math.max(0.5, r1 - r0);
          pos2[i * 3] = rr * Math.sin(ph) * Math.cos(a2);
          pos2[i * 3 + 1] = rr * Math.cos(ph);
          pos2[i * 3 + 2] = rr * Math.sin(ph) * Math.sin(a2);
          col2[i * 3] = c.r; col2[i * 3 + 1] = c.g; col2[i * 3 + 2] = c.b;
        }
        const g2 = new THREE.BufferGeometry();
        g2.setAttribute("position", new THREE.BufferAttribute(pos2, 3));
        g2.setAttribute("aColor", new THREE.BufferAttribute(col2, 3));
        // Variation de taille par particule : un nuage uniforme se lit comme un
        // semis régulier ; quelques grains plus gros suffisent à faire matière.
        const tailles = new Float32Array(n);
        for (let k = 0; k < n; k++) tailles[k] = 5 + Math.random() * 5;
        g2.setAttribute("aTaille", new THREE.BufferAttribute(tailles, 1));
        return new THREE.Points(g2, materiauMonde());
      })();
      mesh.add(nuage);
      // Silhouette : une cage filaire très discrète donne le VOLUME de la boule
      // (sans elle, un nuage de points ne se lit pas comme un monde).
      // Elle sert aussi de PAROI quand on entre : une cellule a des bords.
      const cage = new THREE.Mesh(
        new THREE.SphereGeometry(o.size, 18, 12),
        new THREE.MeshBasicMaterial({
          color: col, wireframe: true, transparent: true, opacity: 0.1,
        }),
      );
      cages.set(wd.qn, cage);
      mesh.add(cage);
      // Groupe intérieur (vide au départ, peuplé à l'approche).
      const inter = new THREE.Group();
      inter.visible = false;
      mesh.add(inter);
      interieurs.set(wd.qn, {
        grp: inter, charge: false, pret: false, boules: [], prefixe: "", rang: 0, classes: [],
        // ── LA PAGE DU NIVEAU (voir `densite.ts`) ────────────────────────────
        // `tous` garde TOUT ce que l'appel a rendu, `page` dit ce qu'on en
        // dessine. Sans cette mémoire, densifier devrait redemander au serveur
        // ce qu'on a déjà reçu — et le total resterait inconnu.
        tous: [], page: FRACTALE.pasPage, plafondAtteint: false, offsetBrut: 0,
        centresGraphe: PAGE.centresNiveau,
        sourceAxe: null, arcs: [],
      });
      const grGrp = new THREE.Group();
      grGrp.visible = false;
      mesh.add(grGrp);
      graphes.set(wd.qn, { grp: grGrp, charge: false, pret: false, noeuds: 0, aretes: 0, types: [], motif: "" });
      mesh.userData = { path: wd.path, label: wd.label, qn: wd.qn };
      scene.add(mesh);
      // anneau de composition réelle (types dominants)
      const types = wd.types || [];
      const tot = Math.max(1, types.reduce((a, b) => a + b.count, 0));
      let acc = -Math.PI / 2;
      const ring = new THREE.Group();
      types.slice(0, 8).forEach((t, ti) => {
        const a1 = acc + (t.count / tot) * Math.PI * 2;
        const pts: THREE.Vector3[] = [];
        for (let k = 0; k <= 8; k++) {
          const a = acc + (a1 - acc) * (k / 8);
          pts.push(new THREE.Vector3(Math.cos(a) * (o.size + 5), 0, Math.sin(a) * (o.size + 5)));
        }
        acc = a1;
        const seg = new THREE.Line(
          new THREE.BufferGeometry().setFromPoints(pts),
          new THREE.LineBasicMaterial({ color: new THREE.Color().setHSL((ti * 0.13) % 1, 0.6, 0.6), transparent: true, opacity: 0.85 }),
        );
        ring.add(seg);
      });
      mesh.add(ring);
      // étiquette
      const cv = document.createElement("canvas"); cv.width = 640; cv.height = 80;
      const g = cv.getContext("2d")!;
      g.font = "600 34px system-ui, sans-serif"; g.fillStyle = "#eaf2fa";
      g.fillText(wd.label.replace(/ \(.*\)/, "").slice(0, 26), 8, 50);
      const sp = new THREE.Sprite(new THREE.SpriteMaterial({
        map: new THREE.CanvasTexture(cv), transparent: true, depthWrite: false,
      }));
      sp.scale.set(300, 38, 1);
      scene.add(sp);
      // orbite
      const on = layout.orbits.get(wd.qn)!;
      const orbPts: THREE.Vector3[] = [];
      for (let k = 0; k <= 96; k++) {
        const a = (k / 96) * Math.PI * 2;
        orbPts.push(new THREE.Vector3(Math.cos(a) * on.r, 0, Math.sin(a) * on.r));
      }
      const orbit = new THREE.Line(
        new THREE.BufferGeometry().setFromPoints(orbPts),
        new THREE.LineBasicMaterial({ color: col, transparent: true, opacity: 0.32 }),
      );
      // inclinaison : chaque monde a SON plan orbital (relief réel, pas plat)
      orbit.rotation.z = on.inc;
      scene.add(orbit);
      worldNodes.set(wd.qn, { pos: new THREE.Vector3(), mesh, label: sp, r: on.r });
    }

    // ── LA TOILE : fibres entre mondes, densifiées sur l'horloge réelle ──
    interface Fib {
      curve: THREE.QuadraticBezierCurve3;
      weights: Float32Array;   // position de chaque synapse le long de la fibre
      points: THREE.Points;
      jitter: Float32Array;
      a: string; b: string; total: number;
    }
    const fibers: Fib[] = [];
    /** Brins de flux : les trajectoires réelles entre mondes. */
    const lignesFlux: THREE.Line[] = [];
    const kinshipLines: { line: THREE.Line; a: string; b: string }[] = [];
    const maxW = Math.max(1, ...(wv.fibers || []).map((f) => f.weight));
    for (const f of wv.fibers || []) {
      const wa = worldNodes.get(f.a), wb = worldNodes.get(f.b);
      if (!wa || !wb) continue;
      // PARENTÉ DE STRUCTURE (preuve P4 seule) : les deux mondes portent
      // réellement les mêmes flux de types, mais ils ne partagent ni entité
      // (P1) ni vécu (P2). Preuve plus FAIBLE → trait fin, discontinu, froid,
      // jamais fondu dans la toile synaptique : une ressemblance de forme ne
      // doit pas se lire comme une connexion vécue.
      if (f.weight === 0 && (f.flows || 0) > 0) {
        const A0 = wa.mesh.position.clone(), B0 = wb.mesh.position.clone();
        const mid0 = A0.clone().add(B0).multiplyScalar(0.5);
        const curve0 = new THREE.QuadraticBezierCurve3(A0, mid0.clone(), B0);
        const line = new THREE.Line(
          new THREE.BufferGeometry().setFromPoints(curve0.getPoints(64)),
          new THREE.LineDashedMaterial({
            color: 0x3a5f80, transparent: true, opacity: 0.55,
            dashSize: 16, gapSize: 13,
          }),
        );
        line.computeLineDistances();
        scene.add(line);
        kinshipLines.push({ line, a: f.a, b: f.b });
        continue;
      }
      const n = Math.max(2, Math.min(520, f.weight));
      const weights = new Float32Array(n);
      const jitter = new Float32Array(n * 3);
      const h = hash(f.a + f.b);
      for (let i = 0; i < n; i++) {
        // Répartition : les synapses partagées puis les ponts de vécu.
        weights[i] = (i + 0.5) / n;
        const spread = 6 + 40 * Math.pow(i / n, 0.6);
        jitter[i * 3] = (hash(f.a + i) - 0.5) * spread;
        jitter[i * 3 + 1] = (hash(f.b + i) - 0.5) * spread;
        jitter[i * 3 + 2] = (hash(f.a + f.b + i) - 0.5) * spread;
      }
      // ── LES FLUX SONT DES TRAJECTOIRES RÉELLES, DONC TRACÉES ────────────
      // Chaque flux est une relation partagée entre les deux mondes, avec son
      // compte réel (`{src, rel, dst, count}`) — mesuré jusqu'à 428 569
      // occurrences pour `Function -CALLS-> Function`. Ils n'étaient nulle part
      // sur la carte : le panneau en affichait le NOMBRE. Une trajectoire réelle
      // qui n'est pas tracée n'existe pas pour l'œil, et c'est exactement ce qui
      // faisait dire « je ne vois pas assez de tissage ».
      // Chaque flux devient un brin distinct, décalé perpendiculairement au
      // tronc de la fibre : on lit alors une FAISCEAU de trajectoires, avec
      // l'épaisseur proportionnelle au logarithme du compte réel (les comptes
      // vont de quelques unités à plusieurs centaines de milliers : sans
      // logarithme, un seul flux écraserait tous les autres).
      {
        const fs = f.flow_sample || [];
        const maxCount = Math.max(1, ...fs.map((x) => Math.max(x.count_a || 0, x.count_b || 0)));
        fs.forEach((fl, i) => {
          const cnt = Math.max(fl.count_a || 0, fl.count_b || 0);
          if (cnt <= 0) return;
          const t = i / Math.max(1, fs.length - 1);
          const dec = (t - 0.5) * 26; // écartement perpendiculaire du brin
          const A2 = wa.mesh.position.clone();
          const B2 = wb.mesh.position.clone();
          const milieu = A2.clone().add(B2).multiplyScalar(0.5);
          const perp = new THREE.Vector3().subVectors(B2, A2).normalize()
            .cross(new THREE.Vector3(0, 1, 0)).normalize().multiplyScalar(dec * 3);
          const c2 = new THREE.QuadraticBezierCurve3(
            A2.add(perp), milieu.add(perp.clone().multiplyScalar(1.7)), B2.add(perp),
          );
          const epaisseur = 0.25 + 0.75 * (Math.log10(1 + cnt) / Math.log10(1 + maxCount));
          const ligne = new THREE.Line(
            new THREE.BufferGeometry().setFromPoints(c2.getPoints(48)),
            new THREE.LineBasicMaterial({
              color: new THREE.Color().setHSL(0.55 - 0.12 * h, 0.85, 0.55 + 0.2 * epaisseur),
              transparent: true, opacity: 0.18 + 0.5 * epaisseur,
            }),
          );
          scene.add(ligne);
          lignesFlux.push(ligne);
        });
      }

      const posArr = new Float32Array(n * 3);
      const geo = new THREE.BufferGeometry();
      geo.setAttribute("position", new THREE.BufferAttribute(posArr, 3));
      const col = new THREE.Color().setHSL(0.55 - 0.12 * h, 0.75, 0.62);
      const pts = new THREE.Points(geo, new THREE.PointsMaterial({
        color: col, size: 11 + 13 * (f.weight / maxW), sizeAttenuation: true,
        transparent: true, opacity: 0.95,
      }));
      scene.add(pts);
      const curve = new THREE.QuadraticBezierCurve3(
        new THREE.Vector3(), new THREE.Vector3(), new THREE.Vector3(),
      );
      fibers.push({ curve, weights, points: pts, jitter, a: f.a, b: f.b, total: f.weight });
    }

    /**
     * MARQUAGE D'UN ENSEMBLE. On agrandit son nuage, on ne change PAS sa couleur :
     * la couleur dit une famille organisationnelle RÉELLE (le premier segment du
     * nom) — la repeindre pour signaler une sélection effacerait une information
     * pour en ajouter une. La taille, elle, est uniforme (2.6) : l'agrandir ne
     * détruit rien et se lit immédiatement.
     */
    let marquePrecedente: { pts: THREE.Points; taille: number } | null = null;
    const marquerEnsemble = (qn: string, nom: string) => {
      if (marquePrecedente) {
        const mp = marquePrecedente.pts.material as THREE.PointsMaterial;
        if (mp) mp.size = marquePrecedente.taille;
        marquePrecedente = null;
      }
      const b = interieurs.get(qn)?.boules.find((x) => x.name === nom);
      if (!b) return;
      const m = b.pts.material as THREE.PointsMaterial;
      if (!m) return;
      marquePrecedente = { pts: b.pts, taille: m.size };
      m.size = m.size * 2.4;
      m.needsUpdate = true;
    };

    // ── sélection / survol ──
    const ray = new THREE.Raycaster();
    const mouse = new THREE.Vector2();
    const pickAt = (ev: PointerEvent | MouseEvent) => {
      const b = el.getBoundingClientRect();
      mouse.set(((ev.clientX - b.left) / b.width) * 2 - 1, -((ev.clientY - b.top) / b.height) * 2 + 1);
      ray.setFromCamera(mouse, camera);
      // ON VISE AUSSI L'ETAGE INTERIEUR. Le raycast ne considerait que les
      // boules de mondes : une fois entre, plus rien n'etait cliquable — donc
      // aucun moyen de choisir la declinaison du dessous.
      const qnFocal = focusQnRef.current;
      const itFocal = qnFocal ? interieurs.get(qnFocal) : undefined;
      const ciblesEnsembles = itFocal?.pret ? itFocal.boules.map((b) => b.pts) : [];
      // ══════════════ SÉLECTION PAR CANDIDATS, EN PIXELS ══════════════════
      //
      // AVANT : on laissait three.js choisir, avec des seuils en UNITÉS-MONDE
      // (`Points.threshold = 7`). MESURÉ par balayage d'écran (390×844, pas de
      // 56 px) : **92 % des cellules ne sélectionnaient RIEN**, et les 8 % qui
      // répondaient rendaient TOUTES un monde — jamais un groupe, jamais un
      // nœud. Cause chiffrée par le module : le seuil fixe valait 36,4 px à
      // 100 unités mais 0,9 px à 4 000 — les mondes, énormes, passaient
      // toujours ; tout le reste, petit, tombait sous le seuil.
      //
      // MAINTENANT : on construit les candidats des deux étages qui étaient
      // morts (mondes et groupes) avec leurs VRAIES grandeurs — ancre en
      // coordonnées monde, rayon réel, opacité RÉELLEMENT appliquée — et c'est
      // le module qui classe, en pixels, avec priorité d'étage. Les nœuds et
      // les arêtes gardent le raycast (résolu au premier candidat résoluble),
      // qui reste juste : ce sont des points, sans rayon apparent.
      const cands: Candidat[] = [];
      const etageCourant = niveauSceneRef.current;
      worldNodes.forEach((wn, qn) => {
        const o = layout.orbits.get(qn);
        if (!o) return;
        const cage = cages.get(qn);
        const opacite = (cage?.material as THREE.MeshBasicMaterial | undefined)?.opacity ?? 0.1;
        cands.push({
          genre: "monde", qn, nom: "",
          ancre: { x: wn.mesh.position.x, y: wn.mesh.position.y, z: wn.mesh.position.z },
          rayon: o.size,
          opacite, etages: [0, 1],
          focal: qn === qnFocal,
        });
      });
      if (itFocal?.pret) {
        for (const b of itFocal.boules) {
          const P = b.pts.localToWorld(b.centre.clone());
          const mat = b.pts.material as THREE.Material & {
            opacity?: number; uniforms?: { uOpacite?: { value: number } };
          };
          const opacite = mat?.uniforms?.uOpacite?.value ?? mat?.opacity ?? 1;
          cands.push({
            genre: "ensemble", qn: qnFocal!, nom: b.name,
            ancre: { x: P.x, y: P.y, z: P.z },
            rayon: b.rayon * (itFocal.grp.scale.x || 1),
            opacite, etages: [1, 2],
            focal: b.name === ensembleChoisi.current?.nom,
          });
        }
      }
      const camModule = {
        viewProjection: new THREE.Matrix4().multiplyMatrices(
          camera.projectionMatrix, camera.matrixWorldInverse,
        ).toArray(),
        largeur: el.clientWidth, hauteur: el.clientHeight,
      } as unknown as Parameters<typeof choisir>[1];
      const meilleur = cands.length
        ? choisir(cands, camModule, {
            x: ((ev.clientX - b.left) / Math.max(1, b.width)) * el.clientWidth,
            y: ((ev.clientY - b.top) / Math.max(1, b.height)) * el.clientHeight,
          }, etageCourant)
        : null;
      if (meilleur) {
        const c = meilleur.candidat;
        // Le compte n'est pas dans l'interface de sélection (ce n'est pas un
        // critère de choix) : on le relit sur la boule, où il vit.
        const compte = c.genre === "ensemble"
          ? (itFocal?.boules.find((x) => x.name === c.nom)?.count ?? 0)
          : 0;
        return {
          genre: c.genre as "monde" | "ensemble", qn: c.qn, nom: c.nom,
          count: compte, nom2: undefined, path: "", label: "",
        };
      }

      // LES NŒUDS DU GRAPHE SONT DESIGNALBLES AUSSI. Un `Points` regroupe tous
      // les nœuds : le raycast rend l'INDEX du point touché, et `noeudsInfo`
      // (même ordre) donne son qn. Le seuil est indispensable — sans lui, viser
      // un point de quelques pixels est pratiquement impossible.
      ray.params.Points.threshold = 7;
      // Les segments se touchent d'autant moins qu'ils sont fins : sans seuil,
      // viser une arête est illusoire.
      ray.params.Line.threshold = 4;
      const grFocal = qnFocal ? graphes.get(qnFocal) : undefined;
      const ciblesGraphe = grFocal?.pret && grFocal.pts ? [grFocal.pts] : [];
      // LES ARÊTES SONT DES CIBLES ELLES AUSSI.
      const ciblesAretes = grFocal?.pret && grFocal.lignes ? [grFocal.lignes] : [];
      // ── `recursive = true` : SANS CELA, AUCUN MONDE N'EST CLIQUABLE ──────
      //
      // Constat mesuré (balayage de l'écran, curseur relevé tous les 55 px) :
      // ZÉRO position cliquable au niveau Cosmos. La cause est ici : on testait
      // `w.mesh` avec `recursive = false`, alors que la géométrie RÉELLE du
      // monde est un ENFANT de ce conteneur — la cage (`mesh.add(cage)`,
      // SphereGeometry) et la nuée (`mesh.add(nuage)`). Le conteneur lui-même
      // n'a pas de géométrie : le raycast ne pouvait donc rien toucher, et
      // « les groupes ne sont pas cliquables » était exact.
      const hits = ray.intersectObjects(
        [...ciblesAretes, ...ciblesGraphe, ...ciblesEnsembles, ...[...worldNodes.values()].map((w) => w.mesh)],
        true,
      );
      // ── ON CHERCHE LE PREMIER CANDIDAT **RÉSOLUBLE**, PAS LE PREMIER TOUCHÉ ──
      //
      // Deux défauts mesurables dans l'ancienne version :
      //   · elle ne consultait que `hits[0]`. Si la géométrie la plus proche
      //     était un point non résolu (`noeudsInfo` manquant) ou un segment aux
      //     extrémités inconnues, la fonction rendait `undefined` — un clic sur
      //     un objet PARFAITEMENT désignable ne faisait donc RIEN, alors que
      //     `hits[1]` était peut-être exactement ce que l'œil visait ;
      //   · à défaut de toute identité, elle rendait un `monde` avec `qn: ""`.
      //     Les appelants font `worlds.find(w => w.qn === "")` → `undefined` →
      //     aucun effet, en silence. Un hit qui ne résout rien doit être `None`,
      //     pas un objet vide qui a l'air d'un objet.
      //
      // On parcourt donc la liste (déjà triée par distance par three.js) et on
      // rend la PREMIÈRE entrée qui porte une identité exploitable.
      const resoudre = (obj: THREE.Object3D, index: number | undefined) => {
        let cible: THREE.Object3D | null = obj;
        let ud = cible.userData as
          | { genre?: string; qn?: string; nom?: string; count?: number; path?: string; label?: string }
          | undefined;
        while (cible && (!ud || (!ud.genre && !ud.qn))) {
          cible = cible.parent;
          ud = cible?.userData as typeof ud;
        }
        if (ud?.genre === "ensemble" && ud.qn && ud.nom) {
          return { genre: "ensemble" as const, qn: ud.qn, nom: ud.nom, count: ud.count ?? 0, nom2: undefined };
        }
        if (ud?.genre === "arete" && ud.qn) {
          const g2 = graphes.get(ud.qn);
          const k = index ?? -1;
          const paire = k >= 0 ? g2?.paires?.[Math.floor(k / 2)] : undefined;
          const i1 = paire?.[0], i2 = paire?.[1];
          const q1 = i1 != null ? g2?.noeudsInfo?.[i1]?.qn : undefined;
          const q2 = i2 != null ? g2?.noeudsInfo?.[i2]?.qn : undefined;
          if (q1 && q2) {
            return { genre: "arete" as const, qn: ud.qn, nom: q1, nom2: q2, count: 0 };
          }
          return undefined; // segment touché mais extrémités non résolues : on ne devine pas
        }
        if (ud?.genre === "noeud" && ud.qn) {
          const g = graphes.get(ud.qn);
          const info = index != null && index >= 0 ? g?.noeudsInfo?.[index] : undefined;
          if (info?.qn) {
            return { genre: "noeud" as const, qn: ud.qn, nom: info.qn, count: 0, nom2: undefined };
          }
          return undefined; // point touché mais nœud non résolu : on ne devine pas
        }
        if (ud?.qn) {
          return {
            genre: "monde" as const, qn: ud.qn, nom: "", count: 0, nom2: undefined,
            path: ud.path ?? "", label: ud.label ?? "",
          };
        }
        return undefined;
      };

      for (const h of hits) {
        const r = resoudre(h.object, h.index ?? undefined);
        if (r) return r;
      }
      return undefined;
    };
    const onMove = (ev: PointerEvent) => {
      renderer.domElement.style.cursor = pickAt(ev) ? "pointer" : "grab";
    };
    /**
     * ── BALAYAGE DE SÉLECTION : LA SEULE FAÇON DE CHIFFRER CET ALGORITHME ────
     *
     * Le fichier raconte qu'un balayage de l'écran (curseur relevé tous les
     * 55 px) avait trouvé « ZÉRO position cliquable » au niveau Cosmos — c'est
     * ainsi qu'a été trouvé le défaut de `recursive`. Cet instrument n'existait
     * pourtant nulle part : la mesure avait été faite à la main, une fois.
     *
     * On l'expose donc : `window.__pickSweep()` parcourt une grille d'écran et
     * rend, par cellule, ce que le clic aurait choisi. C'est ce qui permet de
     * comparer une refonte de la sélection À L'ANCIENNE, sans yeux et sans
     * navigateur — et de dire « avant : 41 % des cellules ne donnaient rien,
     * dont 100 % de la zone des groupes » plutôt que « ça a l'air mieux ».
     */
    (window as unknown as Record<string, unknown>).__pickSweep = (pas = 48) => {
      const b = renderer.domElement.getBoundingClientRect();
      const compte: Record<string, number> = {};
      const cells: { x: number; y: number; genre: string; qn: string; nom: string }[] = [];
      let vides = 0;
      for (let y = 0; y < b.height; y += pas) {
        for (let x = 0; x < b.width; x += pas) {
          const ev = { clientX: b.left + x, clientY: b.top + y } as MouseEvent;
          const r = pickAt(ev);
          if (!r) {
            vides += 1;
            continue;
          }
          compte[r.genre] = (compte[r.genre] ?? 0) + 1;
          // On ne garde que les cellules qui changent de cible : c'est la carte
          // des ZONES désignables, pas une liste de pixels.
          const prec = cells[cells.length - 1];
          if (!prec || prec.genre !== r.genre || prec.qn !== r.qn || prec.nom !== r.nom) {
            cells.push({ x, y, genre: r.genre, qn: r.qn, nom: r.nom });
          }
        }
      }
      const total = Math.ceil(b.height / pas) * Math.ceil(b.width / pas);
      return {
        ecran: { w: Math.round(b.width), h: Math.round(b.height), pas },
        cellules: total,
        vides,
        part_vide: +(100 * vides / Math.max(1, total)).toFixed(1),
        par_genre: compte,
        zones: cells,
      };
    };
    // ── NAVIGUER PAR LES DONNÉES ─────────────────────────────────────────
    // « Je dois pouvoir passer d'un écran à l'autre directement via mes
    // interactions avec les données du graphe. » Le clic ne change donc plus de
    // vue : il DESCEND d'un étage dans la même scène.
    //   clic sur un monde            → on le vise (étage « monde »)
    //   clic sur le monde déjà visé  → on entre dedans (étage « dedans »)
    //   double-clic                  → ouvre la vue dédiée (échappatoire)
    const onClick = (ev: MouseEvent) => {
      const hit = pickAt(ev);
      if (!hit) return;
      // UN ENSEMBLE CLIQUÉ DEVIENT LA DÉCLINAISON QU'ON OUVRE. On charge son
      // graphe en mode « rapide » (donc tout de suite visible) ; le mode
      // « complet » reste déclenché par la timeline, mais SUR LE MÊME ensemble
      // grâce à `ensembleChoisi`. Le choix survit au séquenceur.
      if (hit.genre === "ensemble") {
        const wd = worlds.find((w) => w.qn === hit.qn);
        if (wd) {
          ensembleChoisi.current = { qn: hit.qn, nom: hit.nom };
          marquerEnsemble(hit.qn, hit.nom); // le choix devient VISIBLE
          setVise({ genre: "ensemble", nom: hit.nom, detail: `${hit.count} nœuds` });
          // ── ON CADRE LE GROUPE, ON NE SAUTE PAS DEDANS ──────────────────
          // Le clic déclenchait AUSSI le chargement du graphe, qui remplit la
          // boule entière : l'étage groupe était donc remplacé à l'instant où on
          // venait de le choisir — on ne le voyait JAMAIS comme un groupe.
          // Or le graphe est l'étage d'APRÈS, et la timeline sait déjà quand il
          // doit apparaître (seuil de descente). `ensembleChoisi` retient le
          // choix : la timeline chargera le graphe DU GROUPE VISÉ, quand la
          // descente y arrivera. Le clic ne fait donc plus qu'une chose : il
          // cadre ce qu'on vient de désigner.
          void wd;
          // ── ALLER À SA VUE, PAS RESSORTIR DU MONDE ──────────────────────
          // C'était le défaut : `gotoWorld` re-volait vers la distance de VISÉE
          // du monde (~8 rayons). Cliquer un groupe DEDANS te rejetait donc
          // DEHORS — d'où « le clic sur un groupe ne produit rien ». Il produisait
          // l'inverse : il reculait.
          // ON CADRE LE GROUPE DE L'EXTÉRIEUR — LE PLUS LARGE POSSIBLE SANS
          // REFERMER L'ÉTAGE (voir `cadrerGroupe`, une seule implémentation pour
          // le clic ET pour l'encart « poupées »).
          cadrerGroupe(hit.qn, hit.nom);
        }
        return;
      }
      // UN NŒUD CLIQUÉ DEVIENT LE NOUVEAU CENTRE : on recharge son voisinage
      // (mêmes sauts), donc chaque clic descend d'un cran dans la déclinaison.
      // UNE ARÊTE CLIQUÉE : on la NOMME (source → destination) et on oriente le
      // pas suivant vers sa SOURCE — c'est de là que part la relation.
      if (hit.genre === "arete") {
        const wd = worlds.find((w) => w.qn === hit.qn);
        setVise({ genre: "arete", nom: hit.nom, detail: `→ ${hit.nom2 ?? "?"} (source = prochaine étape)` });
        if (wd) chargerGraphe(wd, hit.nom, "rapide");
        return;
      }
      if (hit.genre === "noeud") {
        const wd = worlds.find((w) => w.qn === hit.qn);
        if (wd) {
          // Le recentrage est visible, mais il faut aussi SAVOIR sur quoi on a
          // recentré : c'est ce qui permet d'orienter le pas suivant.
          setVise({ genre: "noeud", nom: hit.nom, detail: "nouveau centre du graphe" });
          chargerGraphe(wd, hit.nom, "rapide");
        }
        return;
      }
      const dejaVise = focusQnRef.current === hit.qn;
      if (!dejaVise) {
        const i = worlds.findIndex((w) => w.qn === hit.qn);
        if (i >= 0) gotoWorld(i);
        return;
      }
      // Déjà visé : on resserre. Le pas dépend de l'endroit où l'on en est,
      // pour que chaque clic se voie (un pas constant paraît inerte à 95 %).
      zoomBy(camera, ctrl, descenteRef.current > 0.85 ? 0.8 : 0.55);
    };
    const onDblClick = (ev: MouseEvent) => {
      const hit = pickAt(ev);
      if (!hit) return;
      // Sur un ensemble, le double-clic n'a pas d'ecran dedie : on ouvre sa
      // declinaison, comme le simple clic (sinon `path`/`label` n'existent pas).
      if (hit.genre === "ensemble") {
        const wd = worlds.find((w) => w.qn === hit.qn);
        if (wd) {
          ensembleChoisi.current = { qn: hit.qn, nom: hit.nom };
          chargerGraphe(wd, hit.nom, "rapide");
        }
        return;
      }
      // UN NŒUD CLIQUÉ DEVIENT LE NOUVEAU CENTRE : on recharge son voisinage
      // (mêmes sauts), donc chaque clic descend d'un cran dans la déclinaison.
      // UNE ARÊTE CLIQUÉE : on la NOMME (source → destination) et on oriente le
      // pas suivant vers sa SOURCE — c'est de là que part la relation.
      if (hit.genre === "arete") {
        const wd = worlds.find((w) => w.qn === hit.qn);
        setVise({ genre: "arete", nom: hit.nom, detail: `→ ${hit.nom2 ?? "?"} (source = prochaine étape)` });
        if (wd) chargerGraphe(wd, hit.nom, "rapide");
        return;
      }
      if (hit.genre === "noeud") {
        const wd = worlds.find((w) => w.qn === hit.qn);
        if (wd) chargerGraphe(wd, hit.nom, "rapide");
        return;
      }
      onOpen?.(hit.path, hit.label);
    };
    // ── MOLETTE (desktop) ET PINCEMENT (mobile) ────────────────────────────
    //
    // La caméra se pilotait au CLIC et au CLAVIER (`bindViewKeys` :
    // `zoomIn`/`zoomOut` appellent `zoomBy(camera, ctrl, 0.75 | 1.33)`) — mais
    // pas au geste naturel. Sur desktop on cherche la molette ; sur mobile, le
    // pincement est même la SEULE façon d'ajuster la distance, puisque le
    // viewport est en `user-scalable=no` (le pincement du navigateur est
    // désactivé).
    //
    // Aucune logique de caméra n'est ajoutée : les deux gestes appellent
    // EXACTEMENT la fonction que les boutons appellent déjà. Il n'y a donc rien
    // de nouveau à valider au-delà du geste lui-même.
    /**
     * ── LE GESTE DOIT POUVOIR ENTRER DANS LE MONDE ──────────────────────────
     *
     * `zoomBy` BORNE la distance : `Math.max(ctrl.minDistance, …)`, avec
     * `minDistance = 2`. Une fois la caméra à cette butée, un zoom vers
     * l'intérieur **ne fait donc plus RIEN** — et les étages intérieurs sont à
     * ~0,7 RAYON du monde, souvent sous ces 2 unités.
     *
     * Les voies qui marchaient contournent la borne : le clic (`gotoWorld` →
     * `flyTo` à 8 rayons, puis resserrement) et « plonger » (`flyTo` à 0,7
     * rayon). Mais sur MOBILE le pincement est le SEUL geste disponible, et il
     * passait par `zoomBy` : on restait donc dehors, sans aucun moyen de
     * descendre. C'est le défaut constaté (« aucun zoom ne permet de descendre »).
     *
     * Le geste s'enchaîne donc sur la voie qui marche : dès qu'il sature, il
     * PLONGE. Répété, il déplie les étages un par un — la fractale s'ouvre.
     */
    const zoomerVers = (dedans: boolean) => {
      if (dedans && camera.position.distanceTo(ctrl.target) <= ctrl.minDistance * 1.05) {
        zoomProfondRef.current?.();
        return;
      }
      zoomBy(camera, ctrl, dedans ? 0.89 : 1.12);
    };
    /**
     * ── SE DÉPLACER DANS TOUTES LES DIRECTIONS ──────────────────────────────
     *
     * Le déplacement latéral était CONFISQUÉ : OrbitControls garde ses défauts
     * (molette = zoom, clic droit = pan), mais c'est MON gestionnaire de molette
     * qui reçoit le geste à deux doigts d'un trackpad et appelle
     * `preventDefault()`. Sur un portable, le pan au trackpad partait donc dans
     * le zoom : on pouvait entrer et sortir, jamais longer.
     *
     * `panBy` déplace la caméra ET sa cible ensemble, dans le plan de l'écran :
     * c'est un glissement, pas une rotation ni un changement de distance.
     */
    const panBy = (dxPx: number, dyPx: number) => {
      const dist = camera.position.distanceTo(ctrl.target);
      const hauteur = Math.max(1, renderer.domElement.clientHeight);
      const echelle = (2 * Math.tan(((camera.fov * Math.PI) / 180) / 2) * dist) / hauteur;
      _zNormale.setFromMatrixColumn(camera.matrix, 0); // droite (objet reutilise)
      _zVise.setFromMatrixColumn(camera.matrix, 1);    // haut
      const delta = _zNormale.multiplyScalar(-dxPx * echelle).add(_zVise.multiplyScalar(dyPx * echelle));
      camera.position.add(delta);
      ctrl.target.add(delta);
      ctrl.update();
    };
    /**
     * ── LE ZOOM VISE LE POINT TOUCHÉ, PAS LE CENTRE ─────────────────────────
     *
     * Constat : « le zoom est mal positionné ». Il rapprochait la caméra de sa
     * CIBLE : on pouvait donc viser un élément sur le bord de l'écran et voir la
     * vue s'enfoncer ailleurs. Le geste naturel — et le seul qui reste cohérent
     * d'une échelle à l'autre — est d'ancrer le zoom sur CE QU'ON MONTRE DU DOIGT
     * ou du curseur : le point visé reste alors immobile sous le doigt, et tout
     * le reste s'organise autour de lui.
     */
    // (2026-09-15, zoom fluide) OBJETS DE TRAVAIL REUTILISES : un trackpad
    // envoie des centaines de ticks/seconde, et chaque tick allouait ~7
    // objets THREE (Vector2, Raycaster, Plane, Vector3 x3, clone) — des
    // milliers d allocations/seconde, du travail pour le GC, et des pauses
    // perceptibles PENDANT le geste. Les memes objets servent a chaque tick.
    const _zCible = new THREE.Vector2();
    const _zRay = new THREE.Raycaster();
    const _zNormale = new THREE.Vector3();
    const _zPlan = new THREE.Plane();
    const _zVise = new THREE.Vector3();
    const _zDir = new THREE.Vector3();
    const zoomerAncre = (dedans: boolean, clientX: number, clientY: number) => {
      const r = renderer.domElement.getBoundingClientRect();
      _zCible.set(
        ((clientX - r.left) / Math.max(1, r.width)) * 2 - 1,
        -((clientY - r.top) / Math.max(1, r.height)) * 2 + 1,
      );
      _zRay.setFromCamera(_zCible, camera);
      // Point du monde visé, sur le plan de la cible (perpendiculaire à la vue).
      camera.getWorldDirection(_zNormale);
      _zPlan.setFromNormalAndCoplanarPoint(_zNormale, ctrl.target);
      const vise = _zRay.ray.intersectPlane(_zPlan, _zVise);
      if (!vise) {
        zoomerVers(dedans);
        return;
      }
      // ── LA FORMULE EXACTE, PAS UNE COMPENSATION AU JUGÉ ────────────────────
      // Ma première version déplaçait la cible « un peu » puis rapprochait, avec
      // une correction inventée : MESURÉ faux — zoomer à droite déplaçait la vue
      // à GAUCHE. L'ancrage exact est pourtant simple : on met TOUT LE SYSTÈME
      // (cible ET caméra) à l'échelle autour du point visé. Ce point est alors un
      // point fixe de la transformation : il reste donc exactement sous le doigt,
      // et tout s'organise autour de lui — c'est vrai par construction, pas par
      // réglage.
      // ── LE PAS DE ZOOM ÉTAIT TROP GROS : LA TRANSITION N'AVAIT PAS LE TEMPS D'EXISTER ──
      //
      // Mesuré aux étages eux-mêmes (état interne, pas pixels) : la rampe
      // d'ouverture de l'intérieur est franchie ENTIÈREMENT ENTRE DEUX PAS de
      // zoom — la grandeur pilotante passe de 0,150 à 0,642 d'un coup, alors que
      // la rampe ouvre entre 0,12 et 0,37. Elle finit d'ailleurs à 11,4, trente
      // fois au-delà du seuil d'ouverture.
      //
      // Avec un facteur de 0,86 par cran, quarante crans multiplient la distance
      // par 0,002 : la caméra SAUTE. Aucun lissage de rampe ne peut se voir quand
      // la caméra saute — il faut d'abord que le zoom soit fin.
      //
      // À 0,94 par cran, la même course demande huit fois plus de gestes, et la
      // composition du monde s'ouvre alors sur PLUSIEURS crans : c'est la
      // condition pour qu'on la VOIE se détailler par proximité.
      const k = dedans ? 0.94 : 1.065;
      ctrl.target.sub(vise).multiplyScalar(k).add(vise);
      camera.position.sub(vise).multiplyScalar(k).add(vise);
      // Mêmes bornes que `zoomBy` : la caméra ne doit pas traverser sa cible.
      _zDir.copy(camera.position).sub(ctrl.target);
      const len = Math.min(ctrl.maxDistance, Math.max(ctrl.minDistance, _zDir.length()));
      camera.position.copy(ctrl.target).add(_zDir.setLength(len));
      camera.lookAt(ctrl.target);
      ctrl.update();
    };
    const onWheel = (ev: WheelEvent) => {
      ev.preventDefault(); // sinon la page défile pendant qu'on zoome
      // Un trackpad envoie du `deltaX` quand on fait défiler LATÉRALEMENT, et
      // Maj+molette est la convention du déplacement : les deux glissent au lieu
      // de zoomer. Sans ça, le pan au trackpad était purement et simplement
      // avalé par le zoom.
      if (ev.shiftKey || Math.abs(ev.deltaX) > Math.abs(ev.deltaY)) {
        panBy(ev.deltaX !== 0 ? -ev.deltaX : -ev.deltaY, 0);
        return;
      }
      zoomerAncre(ev.deltaY <= 0, ev.clientX, ev.clientY);
    };
    let pinceDist = 0;
    let pinceMilieu: { x: number; y: number } | null = null;
    const ecart = (t: TouchList) =>
      Math.hypot(t[0].clientX - t[1].clientX, t[0].clientY - t[1].clientY);
    const milieu = (t: TouchList) => ({
      x: (t[0].clientX + t[1].clientX) / 2,
      y: (t[0].clientY + t[1].clientY) / 2,
    });
    const onTouchStart = (ev: TouchEvent) => {
      pinceDist = ev.touches.length === 2 ? ecart(ev.touches) : 0;
      pinceMilieu = ev.touches.length === 2 ? milieu(ev.touches) : null;
    };
    const onTouchMove = (ev: TouchEvent) => {
      if (ev.touches.length !== 2 || !pinceDist) return;
      ev.preventDefault();
      const d = ecart(ev.touches);
      // Écarter les doigts (d augmente) => facteur précédent/courant < 1 => zoom
      // avant, même convention que `zoomBy(…, 0.75)`.
      const facteur = Math.min(1.3, Math.max(0.77, pinceDist / Math.max(1, d)));
      pinceDist = d;
      // DEUX DOIGTS QUI SE DÉPLACENT ENSEMBLE = DÉPLACEMENT. C'est le geste
      // standard, et il manquait : sur mobile, on ne pouvait qu'entrer et
      // sortir, jamais longer un monde. On sépare les deux intentions par ce
      // qu'elles changent — l'ÉCART (zoom) ou le MILIEU (pan).
      const m = milieu(ev.touches);
      if (pinceMilieu) {
        const dx = m.x - pinceMilieu.x;
        const dy = m.y - pinceMilieu.y;
        if (Math.abs(dx) > 0.5 || Math.abs(dy) > 0.5) panBy(dx, dy);
      }
      pinceMilieu = m;
      if (Math.abs(1 - facteur) > 0.002) {
        // Écarter les doigts à la butée basse = entrer dans le monde (voir
        // `zoomerVers`) : sans ça, le pincement s'arrêtait à la surface.
        // Le pincement s'ancre entre les deux doigts, comme la molette au curseur.
        zoomerAncre(facteur < 1, m.x, m.y);
      }
    };
    const onTouchEnd = () => {
      pinceDist = 0;
      pinceMilieu = null;
    };
    renderer.domElement.addEventListener("dblclick", onDblClick);
    renderer.domElement.addEventListener("pointermove", onMove);
    renderer.domElement.addEventListener("click", onClick);
    renderer.domElement.addEventListener("wheel", onWheel, { passive: false });
    renderer.domElement.addEventListener("touchstart", onTouchStart, { passive: true });
    renderer.domElement.addEventListener("touchmove", onTouchMove, { passive: false });
    renderer.domElement.addEventListener("touchend", onTouchEnd, { passive: true });

    /**
     * Peuple l'étage INTÉRIEUR d'un monde : son organisation réelle (modules,
     * avec leurs effectifs) et, quand le moteur en expose, les liens réels
     * entre modules. Mêmes primitives que l'étage espace (nuages de particules)
     * pour que la descente ne change pas de nature visuelle.
     */
    /**
     * ── TIMELINE D'ÉVÉNEMENTS DE NAVIGATION ────────────────────────────────
     *
     * Les chargements partaient chacun sur son seuil, EN PARALLÈLE : quatre
     * requêtes lourdes pouvaient partir ensemble, saturer le moteur, et le
     * dernier étage arrivait alors qu'on le regardait déjà. Chaque seuil était
     * une devinette sur le temps que mettrait le voisin.
     *
     * La descente devient une SUITE D'ÉVÉNEMENTS exécutés UN PAR UN, dans
     * l'ordre où on en aura besoin — une timeline qu'on déroule :
     *   - chaque événement a un seuil de descente, un état, une durée mesurée ;
     *   - un seul événement lourd à la fois : le temps de chargement devient
     *     prévisible et raisonnable au lieu de s'additionner ;
     *   - un étage n'est montré que si son chargement est TERMINÉ, ce qui rend
     *     la transition parfaite : on ne voit jamais un étage à moitié peuplé.
     */
    interface Evenement {
      id: string; seuil: number; label: string;
      etat: "attente" | "charge" | "pret" | "echec";
      ms?: number; lance?: number;
    }
    let evenements: Evenement[] = [];
    let evenementEnCours = false;
    let dernierMondeTimeline: string | null = null;
    /** L'axe avec lequel l'étage intérieur a été construit (voir la re-segmentation). */
    let sourceConstruite: "familles" | "axe" | "" = "";
    /** Combien de fois l'étage intérieur a été reconstruit (changement d'axe). */
    let reconstructionsInterieur = 0;
    let timelinePubliee = "";
    /**
     * Sortie de build : ce n'est pas un lieu où l'on veut ENTRER.
     *
     * Constaté en cliquant des nœuds à l'étage graphe : les nœuds visés étaient
     * `…deepseek_harness.vendor.hmr.…` — du code VENDORISÉ. On traversait un
     * monde pour arriver dans ses dépendances.
     *
     * Déclarée ICI, et non plus dans `avancerTimeline` : le bouton « 🔺 Graphe »
     * désigne lui aussi un ensemble d'entrée (voir la demande de niveau), et deux
     * listes d'artefacts qui divergeraient feraient entrer le bouton et le
     * séquenceur à deux endroits différents.
     */
    const estArtefact = (nom: string) =>
      ["dist", "build", "target", "node_modules", "vendor", "out"].includes(nom);
    const resetTimeline = () => {
      /**
       * ── L'ORDRE DES CHARGEMENTS DÉPEND DE L'AXE ────────────────────────────
       *
       * Pour un axe du NOM, `/api/viz/modules` suffit : l'organisation se charge
       * d'abord, le graphe ensuite. Pour un axe de RELATION (communauté Leiden,
       * contour ∂), c'est l'INVERSE : les groupes de l'intérieur sont calculés
       * depuis le graphe, donc le graphe doit être là AVANT. Inverser les deux
       * ── POURQUOI PAS UN SIMPLE RÉORDONNANCEMENT ──────────────────────────
       * J'ai d'abord mis le graphe AVANT l'organisation pour un axe de relation.
       * MESURÉ : blocage MUTUEL — l'événement « graphe » est gardé par
       * `it?.pret && it.boules[0]` (il lui faut une entrée, qui vient des
       * ensembles), et l'organisation attendait le graphe. Rien ne se chargeait.
       * L'ordre d'origine est donc CONSERVÉ, et la re-segmentation devient un
       * TROISIÈME événement, qui vient après le graphe : l'organisation par
       * familles peuple d'abord (jamais d'écran vide), puis l'axe la remplace.
       */
      const parRelation = sourceGroupes() === "axe";
      evenements = [
        // SEUILS CHOISIS SUR MESURE, pas à l'intuition. La timeline a montré que
        // le graphe coûte 26 432 ms (hops=2 sur un gros store) : c'est le poste
        // dominant, et il doit donc partir LE PLUS TÔT possible pour que
        // l'attente se recouvre avec la descente au lieu de s'y ajouter. Les
        // deux premiers étages sont légers (12 ms chacun) : ils partent tôt sans
        // rien coûter.
        { id: "interieur", seuil: 0.10, label: "organisation", etat: "attente" },
        { id: "temps", seuil: 0.20, label: "histoire", etat: "attente" },
        { id: "graphe", seuil: 0.30, label: "graphe", etat: "attente" },
        // L'ORGANISATION RE-SEGMENTÉE PAR L'AXE, dès que le graphe est là : c'est
        // ce qui fait changer les GROUPES du cran « monde » selon la relation
        // choisie (communauté Leiden, contour ∂) — les familles de chemins ont
        // servi à peupler, elles cèdent la place.
        ...(parRelation
          ? [{ id: "interieurAxe", seuil: 0.40, label: "organisation (par l'axe)", etat: "attente" as const }]
          : []),
        { id: "motifs", seuil: 0.62, label: "motifs K3/K4", etat: "attente" },
      ];
    };
    /** Déroule la timeline : UN événement lourd à la fois, dans l'ordre. */
    const avancerTimeline = (
      t: number, wd: WeaveWorld | undefined, it: Interieur | undefined, gr: Graphe | undefined,
    ) => {
      if (evenementEnCours || !wd) return;
      // L'ensemble vise par l'utilisateur prime sur `boules[0]` : le choix
      // explicite doit survivre au sequenceur, sinon on entre dans un ensemble
      // et on se retrouve avec le graphe d'un autre.
      /**
       * L'ENTRÉE DU GRAPHE NE DOIT PAS ÊTRE UN ARTEFACT DE BUILD.
       *
       * Constaté en cliquant des nœuds à l'étage graphe : les nœuds visés
       * étaient `…deepseek_harness.vendor.hmr.…`, `…vendor.cordis.…` — du code
       * VENDORISÉ. La raison : l'entrée par défaut était `boules[0]`, le plus
       * gros ensemble, sans regarder ce qu'il contient. On traversait donc un
       * monde pour arriver… dans ses dépendances.
       *
       * On préfère donc le plus gros ensemble dont le nom n'est PAS une sortie
       * de build — mêmes noms conventionnels que le filtre de l'agrégat `arcs`,
       * pour que les deux étages racontent la même chose. Si tous les ensembles
       * sont des artefacts, on reprend le plus gros : mieux vaut entrer quelque
       * part que nulle part.
       *
       * (`estArtefact` a été REMONTÉE au niveau de l'effet : le bouton « 🔺
       * Graphe » désigne lui aussi un ensemble d'entrée, et deux listes qui
       * divergeraient feraient entrer le bouton et le séquenceur à deux endroits
       * différents.)
       */
      const centreVise = (it: { boules: { name: string }[] }, wd: { qn: string }) => {
        const c = ensembleChoisi.current;
        if (c && c.qn === wd.qn) return c.nom;
        const propre = it.boules.find((x) => !estArtefact(x.name));
        return (propre ?? it.boules[0]).name;
      };
      const ev = evenements.find((e) => e.etat === "attente" && t >= e.seuil);
      if (!ev) return;
      const lancer = (action: () => unknown) => {
        ev.etat = "charge";
        ev.lance = performance.now();
        evenementEnCours = true;
        Promise.resolve()
          .then(action)
          .catch(() => { ev.etat = "echec"; })
          .finally(() => {
            if (ev.etat === "charge") ev.etat = "pret";
            ev.ms = Math.round(performance.now() - (ev.lance || performance.now()));
            evenementEnCours = false;
          });
      };
      if (ev.id === "interieur") lancer(() => chargerInterieur(wd));
      else if (ev.id === "interieurAxe" && it?.pret && gr?.pret) {
        // Re-segmentation : on vide l'étage intérieur et on le reconstruit depuis
        // les groupes de l'axe. Le GRAPHE n'est pas rechargé (il vient d'être
        // chargé, et ses gardes portent désormais sur le centre).
        lancer(() => {
          it.grp.clear();
          it.boules = [];
          it.enveloppe = undefined;
          it.charge = false;
          it.pret = false;
          return chargerInterieur(wd);
        });
      }
      else if (ev.id === "temps") lancer(() => chargerTemps(wd));
      else if (ev.id === "graphe" && it?.pret && it.boules[0]) {
        const entreeMonde = centreVise(it, wd);
        if (!it.prefixe) it.entreeRacine = entreeMonde;
        lancer(() => chargerGraphe(wd, entreeMonde, "rapide"));
      } else if (ev.id === "motifs" && it?.pret && it.boules[0]) {
        // Les motifs n'ont de sens qu'APRÈS le graphe rapide : ils le remplacent.
        lancer(() => chargerGraphe(wd, centreVise(it, wd), "complet"));
      }
    };

    /**
     * Budget TOTAL de particules pour l'intérieur d'un monde. Il ne sert pas à
     * « faire joli » : il est RÉPARTI AU PRORATA des effectifs réels, ce qui
     * préserve les rapports entre ensembles. Un budget plus grand ne change pas
     * les proportions, seulement la finesse.
     */
    const BUDGET_PARTICULES = 9000;

    /**
     * Couleur d'une FAMILLE organisationnelle réelle, déduite du nom : le
     * premier segment porteur du chemin. Deux ensembles du même dossier
     * partagent donc la même teinte, et deux dossiers différents se distinguent
     * — c'est une information, pas une décoration. La teinte est stable dans le
     * temps (même nom → même couleur au rechargement).
     */
    const couleurFamille = (nom: string) => {
      const seg = nom.split(/[./]/).filter(Boolean);
      const famille = (seg[0] || nom).toLowerCase();
      let h = 0;
      for (const c of famille) h = (h * 31 + c.charCodeAt(0)) >>> 0;
      // Teinte tirée du nom, mais SATURATION et LUMINOSITÉ fixes : la lisibilité
      // ne dépend pas du hasard, seule la teinte distingue les familles.
      return new THREE.Color().setHSL((h % 360) / 360, 0.55, 0.58);
    };

    /**
     * ── LES GROUPES DE L'AXE — UNE SEULE IMPLÉMENTATION ──────────────────────
     *
     * Le même découpage sert à trois choses, et il ne doit exister qu'ici :
     *   1. la MESURE publiée (`__etages.monde.axe`) ;
     *   2. la DESCENTE (`plonger` entre dans le plus gros groupe) ;
     *   3. la SEGMENTATION DU CRAN « MONDE » : pour un axe qui n'est pas un
     *      découpage du NOM (communauté du moteur, contour ∂), l'intérieur d'un
     *      monde se compose des groupes de l'axe au lieu des familles de chemins.
     *
     * `null` a un sens précis : l'axe ne distingue RIEN (un seul groupe). On ne
     * prétend alors pas segmenter — c'est la doctrine déjà tenue pour les axes
     * vides ailleurs dans ce fichier.
     */
    /**
     * ── LES NŒUDS DU NIVEAU COURANT — chemin ∩ classes ───────────────────────
     *
     * Une seule fonction pour les deux coordonnées : le CHEMIN (tokens, la loi de
     * `fractale.ts`) et les CLASSES (l'axe choisi, appliqué tour à tour). Tout ce
     * qui a besoin des nœuds d'un niveau — le découpage par l'axe, la résolution
     * d'un groupe en centres, la mesure — passe par ici. Deux implémentations
     * divergeraient : le panneau annoncerait un ensemble que le graphe ne
     * chargerait pas.
     */
    const noeudsCourants = (
      qnMonde: string, prefixe: string, classes: { axe: string; valeur: string }[],
      max = 0,
    ): string[] => {
      const tokens = prefixe.split(".").filter(Boolean);
      const infos = graphes.get(qnMonde)?.noeudsInfo;
      if (!infos?.length) return [];
      const out: string[] = [];
      for (const x of infos) {
        if (tokens.length) {
          const parts = String(x.qn).split(/[.:]/).filter(Boolean);
          let k = 0;
          for (const seg of parts) {
            if (seg === tokens[k]) { k++; if (k === tokens.length) break; }
          }
          if (k !== tokens.length) continue;
        }
        let ok = true;
        for (const c of classes) {
          if (etiquetteDimension(x.qn, c.axe as Dimension) !== c.valeur) { ok = false; break; }
        }
        if (ok) out.push(x.qn);
        if (max && out.length >= max) break;
      }
      return out;
    };
    /**
     * LE DÉCOUPAGE PAR L'AXE, AU NIVEAU COURANT — et non sur tout le graphe.
     * C'est ce qui rend l'organisation disponible à TOUS les étages : au niveau
     * `deepseek_harness.packages`, l'axe ∂ coupe LES NŒUDS DE CE NIVEAU, pas ceux
     * de l'univers entier.
     */
    /**
     * ── LE TOKEN SUIVANT, DANS L'ENSEMBLE RÉDUIT ─────────────────────────────
     *
     * Quand l'axe ne distingue PLUS rien au niveau courant (une seule classe), il
     * ne faut PAS retomber en silence sur les enfants du préfixe : le fil dirait
     * « ∩ ∂=intérieur » et l'étage montrerait autre chose — deux autorités qui se
     * contredisent, exactement ce que ce fichier refuse.
     *
     * On continue donc par l'AUTRE coordonnée : le token de la profondeur
     * suivante, calculé sur les nœuds QUI RESTENT (chemin ∩ classes). C'est la même
     * loi que la descente fractale (le token du rang suivant), appliquée à
     * l'ensemble que les classes ont réduit — les deux coordonnées descendent
     * ensemble, à tous les niveaux.
     */
    const suivantsDuNiveau = (noeuds: string[], tokens: string[]): Map<string, string[]> | null => {
      const par = new Map<string, string[]>();
      for (const q of noeuds) {
        const parts = String(q).split(/[.:]/).filter(Boolean);
        let k = 0, pos = -1;
        for (let i = 0; i < parts.length; i++) {
          if (parts[i] === tokens[k]) { k++; if (k === tokens.length) { pos = i; break; } }
        }
        const suiv = pos >= 0 ? parts[pos + 1] : parts[0];
        if (!suiv || !tokenValide(suiv)) continue;
        const arr = par.get(suiv);
        if (arr) arr.push(q); else par.set(suiv, [q]);
      }
      return par.size < 2 ? null : par;
    };
    const groupesAxe = (qn: string, noeuds: string[]): Map<string, string[]> | null => {
      if (!noeuds.length) return null;
      const parType = new Map<string, string[]>();
      for (const q of noeuds) {
        const t = etiquetteDimension(q, dimensionRef.current);
        const arr = parType.get(t);
        if (arr) arr.push(q); else parType.set(t, [q]);
      }
      return parType.size < 2 ? null : parType;
    };
    /** L'axe se lit-il dans le NOM (donc sans le graphe) ? */
    const axeEstNom = () => ["module", "parent", "extension", "couche"].includes(dimensionRef.current);
    /**
     * ── D'OÙ VIENNENT LES GROUPES DE L'ÉTAGE INTÉRIEUR — UNE SEULE RÈGLE ────
     * `familles` : l'agrégat serveur des chemins, qui porte sur le MONDE entier.
     * `axe`      : les groupes d'une relation (communauté Leiden, contour ∂),
     *              calculés depuis le graphe — donc sur l'ÉCHANTILLON du graphe.
     * La règle exige un axe de relation EXPLICITEMENT choisi : un axe auto-choisi
     * ne doit pas changer ce que l'étage montre (mesuré : « 1 ensemble — intel »
     * au lieu de 6). Cette fonction est la seule autorité : l'événement de
     * re-segmentation et la reconstruction l'interrogent toutes deux.
     */
    const sourceGroupes = (): "familles" | "axe" =>
      (!axeEstNom() && dimensionChoisie.current) ? "axe" : "familles";

    /**
     * ── PUBLIER L'ÉTAT D'ORGANISATION SANS ÉCRASER CELUI QU'ON VIENT D'Y METTRE
     *
     * L'étage publie la POSITION (préfixe, rang, source) au lancement du
     * chargement et la MESURE (total, montres, geste) à la fin : deux moments,
     * un seul objet. Une affectation directe ferait disparaître l'un ou
     * l'autre — et le contrôle, qui lit les deux ENSEMBLE pour exiger que
     * densifier ne bouge pas la position, mesurerait `undefined`.
     */
    const publierOrganisation = (champs: Record<string, unknown>) => {
      const e = etagesRef.current as Record<string, unknown>;
      const avant = (e.organisation as Record<string, unknown> | undefined) ?? {};
      e.organisation = { ...avant, ...champs };
    };

    /**
     * ── LA COUVERTURE DE L'ÉCHANTILLON — CE QUI DIT « IL Y A PLUS » ─────────
     *
     * Un niveau découpé par l'axe (ou par le token suivant) l'est sur les nœuds
     * du GRAPHE chargé, pas sur le monde — et le graphe ne dit PAS qu'il a coupé
     * (mesuré : 350 nœuds rendus, `truncated` absent, pour un niveau qui en porte
     * 52 979). La seule grandeur honnête est donc le RAPPORT entre ce qui est lu
     * et ce que le monde contient : c'est lui qui arme la règle 3 bis de
     * `densite.ts` (« densifier = élargir l'échantillon ») au lieu de laisser le
     * geste tourner à vide sur un niveau qu'il croit entier.
     */
    const couvertureGraphe = (qnMonde: string): number => {
      const wd = worlds.find((w) => w.qn === qnMonde);
      const lus = graphes.get(qnMonde)?.noeuds ?? 0;
      return wd && wd.nodes > 0 ? Math.min(1, lus / wd.nodes) : 1;
    };

    /** Le budget de centres à demander pour l'échantillon d'un niveau. */
    const centresDuNiveau = (qnMonde: string) =>
      interieurs.get(qnMonde)?.centresGraphe ?? PAGE.centresNiveau;

    /**
     * ── `redessin` : DENSIFIER NE REDEMANDE RIEN ────────────────────────────
     *
     * MESURÉ par le contrôle (2026-09-15, §9) : `d` (densifier) rappelait cette
     * fonction SANS le dire, donc elle RÉÉMETTAIT `/api/viz/space` — et, pire,
     * elle rechoisissait la SOURCE du découpage (agrégat du serveur ou
     * échantillon du graphe) selon l'état du moment : observé « 24 sur 56 » →
     * « 8 sur 8 » au même préfixe et au même rang. Deux défauts d'un coup :
     * la phrase « la matière est déjà là » était fausse, et le geste semblait
     * changer de niveau alors qu'il ne le changeait pas.
     *
     * `redessin = true` court-circuite les DEUX : la source est `it.tous` (la
     * matière gardée) et les fibres sont `it.arcs` (celles d'avant). Un
     * redessin ne fait donc AUCUNE requête, et ne peut pas changer de source.
     */
    const chargerInterieur = (wd: WeaveWorld, redessin = false) => {
      const it = interieurs.get(wd.qn);
      if (!it || it.charge) return;
      it.charge = true;
      const o = layout.orbits.get(wd.qn);
      const taille = o?.size ?? 30;
      // ── LA SOURCE DES GROUPES DÉPEND DE L'AXE ──────────────────────────────
      // Pour un axe du NOM, `/api/viz/modules` fait autorité (les familles de
      // chemins du monde entier). Pour un axe de RELATION (communauté Leiden,
      // contour ∂), il n'existe pas d'agrégat serveur : les groupes sont ceux du
      // GRAPHE chargé, avec ses attributs réels (`community_id`, `curvature`).
      // On ne fabrique donc rien — on change de source, et on le DIRA au panneau
      // (les effectifs sont ceux de l'échantillon du graphe, pas du monde).
      /**
       * ── LA RE-SEGMENTATION EST UN GESTE, PAS UN EFFET AUTOMATIQUE ─────────
       *
       * MESURÉ (et trouvé par le contrôle, pas par relecture) : l'axe est
       * AUTO-choisi — le fichier prend la dimension qui distingue le plus de
       * groupes — et comme l'axe pilote maintenant la SOURCE des groupes de
       * l'étage intérieur, ce choix automatique pouvait le re-segmenter tout seul.
       * Observé : « Intérieur : 1 ensembles — intel (1378) » là où le monde en a 6.
       * Une décision d'affichage prise par la machine ne doit pas changer ce que
       * l'étage MONTRE.
       *
       * La segmentation par l'axe n'a donc lieu que si l'UTILISATEUR a désigné un
       * axe de relation (`communauté`, `∂`) : c'est alors un geste, et il se voit.
       */
      const noeudsNiveau = (it.prefixe || it.classes.length)
        ? noeudsCourants(wd.qn, it.prefixe, it.classes) : null;
      const parAxe = sourceGroupes() === "axe" && noeudsNiveau
        ? groupesAxe(wd.qn, noeudsNiveau)
        : null;
      // L'axe a saturé (une seule classe) : on continue par le token suivant de
      // l'ensemble RÉDUIT, au lieu de retomber sur les enfants du préfixe.
      const parSuivant = !parAxe && noeudsNiveau
        ? suivantsDuNiveau(noeudsNiveau, it.prefixe.split(".").filter(Boolean))
        : null;
      /**
       * ── LA DESCENTE FRACTALE : LES SOUS-GROUPES DU PRÉFIXE COURANT ─────────
       *
       * « Ce sont des groupes et des sous-groupes extrêmement connectés. » Exact,
       * et mesuré sur ce store : 3 racines, 200 enfants sous `deepseek_harness`,
       * 60 petits-enfants sous `…packages` — une hiérarchie profonde que l'étage
       * intérieur ne montrait PAS : il s'arrêtait aux familles racines, et aucun
       * geste ne menait plus bas.
       *
       * La loi existait déjà, écrite et mesurée dans `fractale.ts` ; elle n'était
       * simplement pas branchée ici. On l'utilise telle quelle :
       *   · `enfantsDe(store, prefixe, rang)` — enfants d'un préfixe, agrégés par
       *     le SERVEUR sur tout le store (pas sur l'échantillon du graphe), avec
       *     le filtre des tokens valides et des effectifs > 1 ;
       *   · `rayons(groupes, R)` — racine CUBIQUE de la part, pour que le VOLUME
       *     dise l'effectif réel (le seul canal qui se lise dans une scène 3D) ;
       *   · `directions(n)` — disposition déterministe (angle d'or), donc un
       *     groupe se retrouve « là où il était » quand on revient.
       */
      const pEnfants: Promise<{ tous: { name: string; count: number }[]; rang: number; plafondAtteint: boolean; bruts: number } | null> =
        redessin ? Promise.resolve(null)
          : parAxe ? Promise.resolve(null)
          : it.prefixe
            // `it.page` : on demande TOUT ce que le niveau contient (le plafond de
            // l'appel), et c'est le DESSIN qui prend une page. L'ancienne forme
            // coupait ici, avant que quiconque lise la longueur — d'où un
            // « 24 sur 24 » qui ne pouvait pas dire la vérité.
            ? enfantsDe(wd.path, it.prefixe, it.rang, it.page)
              .then((r) => ({
                tous: r.tous.map((g) => ({ name: g.nom, count: g.count })),
                rang: r.rang,
                plafondAtteint: r.plafondAtteint,
                bruts: r.bruts,
              }))
            : fetchModules(wd.path).then((m) => {
              const bruts = m.modules || [];
              return {
                tous: bruts.filter((x) => x.count > 1),
                rang: typeof m.rank === "number" && m.rank > 0 ? m.rank : 3,
                // Le plafond se mesure sur ce que le SERVEUR a rendu, avant nos
                // filtres : un groupe d'un seul nœud écarté ne dit rien du plafond.
                plafondAtteint: bruts.length >= PAGE.plafondAppel,
                // `/api/viz/modules` N'A PAS d'`offset` : le cran « racines » ne peut
                // donc pas paginer chez le serveur (voir le reste ouvert du §13 du
                // PROCESSUS). On tient le compte quand même, pour que l'aveu soit
                // mesurable plutôt que supposé.
                bruts: bruts.length,
              };
            });
      // Le rang du niveau MONTRE : au niveau racine il vient de l'agrégat, plus
      // bas il avance d'un token (`rang` + 1). Il est retenu sur l'étage pour que
      // la descente suivante sache où elle en est.
      const fEnfants = { tous: [] as { name: string; count: number }[], rang: it.rang, plafondAtteint: false, bruts: 0 };
      // La POSITION est publiée tout de suite (un geste doit pouvoir dire où il
      // mène avant la fin du chargement) ; la MESURE de densité l'est à la fin
      // (voir `publierOrganisation` dans le `.then`), parce qu'avant, `tous` et
      // `montres` décrivent encore le niveau PRÉCÉDENT — un chiffre périmé est
      // pire qu'un chiffre absent.
      publierOrganisation({
        axe: dimensionRef.current,
        source: parAxe ? "axe" : parSuivant ? "suivants" : it.prefixe ? "sous-groupes" : "racines",
        prefixe: it.prefixe,
        rang: it.rang,
        // `page` est la page DEMANDÉE : elle est connue avant le chargement, et
        // c'est elle que le geste « densifier » fait avancer.
        page: it.page,
        // ── MESURABLE : LA DÉCISION ELLE-MÊME ────────────────────────────────
        // Le contrôle lit ici `geste` / `montres` / `total` : la loi n'est pas
        // une intention dans un commentaire, c'est un état publié.
        decision: decisionDensite({
          total: it.tous.length, montres: it.page,
          feuille: it.feuille, plafondAtteint: it.plafondAtteint,
        }).geste,
      });
      const depuisAxe = parAxe ?? parSuivant;
      const sourceAxe: { name: string; count: number }[] | null = redessin
        ? it.sourceAxe
        : depuisAxe
          ? [...depuisAxe.entries()]
            .map(([name, qns]) => ({ name, count: qns.length }))
            .sort((a, b) => b.count - a.count)
          : null;
      // La SOURCE du découpage est RETENUE sur l'étage : un redessin doit
      // redessiner le MÊME découpage, pas en recalculer un autre (mesuré : le
      // recalcul faisait passer « 24 sur 56 » à « 8 sur 8 »).
      if (!redessin) it.sourceAxe = sourceAxe;
      // On REND la promesse : la timeline doit savoir quand le chargement est
      // réellement TERMINÉ, pas quand il est lancé. Sans cela, elle annonçait
      // « prêt 3 ms » pour une requête de plusieurs secondes — une timeline qui
      // ment sur ses durées ne sert à rien.
      return Promise.all([
        redessin || sourceAxe
          ? Promise.resolve({ modules: sourceAxe ?? it.tous })
          : fetchModules(wd.path).catch(() => ({ modules: [] as { name: string; count: number }[] })),
        // Les liens entre modules n'ont de sens que pour les familles de chemins :
        // un arc « src → dst » ne relie pas deux communautés.
        redessin
          ? Promise.resolve(it.arcs)
          : sourceAxe
            ? Promise.resolve([] as { src: string; dst: string; weight: number }[])
            : fetchModuleArcs(wd.path, 16, 60).catch(() => [] as { src: string; dst: string; weight: number }[]),
      ]).then(async ([mods, arcs]) => {
        // Les sous-groupes du PRÉFIXE courant — la source de la descente fractale.
        const enf = await pEnfants.catch(() => null);
        if (enf && !parAxe) {
          fEnfants.tous = enf.tous.filter((g) => tokenValide(g.name));
          fEnfants.rang = enf.rang;
          fEnfants.plafondAtteint = enf.plafondAtteint;
          fEnfants.bruts = enf.bruts;
          it.rang = enf.rang;
        }
        // ── LE PLAFOND ÉTAIT DEVENU UNE PAGE, SANS QUE RIEN NE LE DISE ───────
        // MESURÉ : le commentaire ci-dessus annonçait « plus de plafond : en
        // montrer 200 ne coûte pas plus que d'en montrer 8 », mais la source
        // était COUPÉE À 24 en amont (`slice(0, FRACTALE.groupesMax)`) — donc
        // l'intention n'était pas tenue par le code, et le panneau affichait
        // « 24 ensembles » sur un niveau qui en portait 200.
        //
        // Désormais : `tousModules` EST le niveau (tout ce que l'appel a rendu),
        // et `modules` en est la PAGE. Le reste n'est pas perdu : il est gardé
        // sur l'étage (`it.tous`), prêt à être dessiné par « ＋ densifier » sans
        // redemander au serveur. C'est la loi de `densite.ts` : tant qu'il reste
        // de la matière non montrée, on densifie ; on n'approfondit qu'ensuite.
        // En REDESSIN, la source est la matière DÉJÀ gardée : ni le serveur, ni
        // l'échantillon du graphe ne sont réinterrogés — c'est ce qui rend
        // l'affirmation « aucun appel, même niveau » vraie au lieu de plausible.
        const groupesSource = redessin
          ? it.tous
          : (sourceAxe ?? (fEnfants.tous.length ? fEnfants.tous : (mods.modules || [])));
        const tousModules = groupesSource.filter((m) => m.count > 1);
        const modules = tousModules.slice(0, it.page);
        it.tous = tousModules;
        // Les fibres suivent : gardées pour un redessin, renouvelées sinon.
        if (!redessin) it.arcs = arcs;
        // ── LE VERDICT DU SERVEUR SURVIT AU REDESSIN ────────────────────────
        // MESURÉ (2026-09-15, instance isolée) : `plafondAtteint` valait `true` sur
        // un niveau à page pleine (75 groupes servis), puis devenait `false` APRÈS
        // une simple densification EN MÉMOIRE — donc la page 2 du serveur n'était
        // plus jamais demandée, et la règle 3 restait lettre morte là où elle était
        // armée. La cause : un redessin n'interroge personne (`pEnfants` est
        // `null`), donc `fEnfants.plafondAtteint` retombait à sa valeur initiale
        // `false`, et on écrasait un verdict qu'on venait de recevoir.
        // Un redessin ne redemande rien : il ne peut donc RIEN apprendre sur ce que
        // le serveur a coupé. On le RETIENT, comme `tous`, `arcs` et `sourceAxe`.
        if (!redessin) it.plafondAtteint = sourceAxe ? false : fEnfants.plafondAtteint;
        // ── CE QUE LA PAGE SUIVANTE DEVRA SAUTER : DES LIGNES BRUTES ────────
        // Un RE-DESSIN ne touche pas au compte (on ne redemande rien) ; un
        // chargement de niveau le repart de ce que le serveur vient de rendre.
        // Le compter sur `it.tous.length` (filtré) ferait recouvrir les pages.
        if (!redessin) it.offsetBrut = sourceAxe ? 0 : fEnfants.bruts;
        if (!modules.length) return; // rien de réel à montrer : la boule reste
        // ── EST-CE UNE FEUILLE ? ────────────────────────────────────────────
        // La loi de `fractale.ts` : moins de deux enfants, ou des effectifs sous
        // le seuil, et il n'y a plus rien à organiser. On le RETIENT pour le DIRE
        // (le panneau n'offre alors plus d'entrer) au lieu de laisser cliquer
        // dans le vide.
        // La question porte sur le NIVEAU, pas sur la page : un niveau qui porte
        // 200 groupes n'est pas une feuille parce qu'on n'en dessine que 24.
        it.feuille = estFeuille(tousModules.map((m) => ({ nom: m.name, count: m.count })), tousModules.reduce((a, x) => a + x.count, 0));
        const maxC = Math.max(...modules.map((m) => m.count));
        const totalNoeudsMonde = Math.max(1, modules.reduce((a, x) => a + x.count, 0));
        // ── DEUX LOIS DE TAILLE, UNE SEULE PROPRIÉTÉ ────────────────────────
        // Cet étage calcule déjà son rayon par la racine cubique de la part
        // (juste en dessous) : le VOLUME dit l'effectif réel, comme `rayons()`
        // dans `fractale.ts`. Les deux divergent pourtant sur les BORNES
        // (`PART_MAX_GROUPE` 0,34 / `PLANCHER_GROUPE` 0,05 ici, 0,55 / 0,08
        // là-bas) — deux jeux de réglages pour la même loi, ce qui est un cran
        // de trop. On garde ceux-ci (mesurés dans cette vue) et on le DIT, plutôt
        // que d'importer l'autre et de changer en silence la taille des groupes.
        const positions = new Map<string, THREE.Vector3>();
        modules.forEach((m, i) => {
          // Direction déterministe (jamais aléatoire au rechargement).
          const a = i * 2.399, b = Math.sin(i * 1.618) * 0.7;
          const dir = new THREE.Vector3(Math.cos(a), b, Math.sin(a)).normalize();
          const centre = dir.multiplyScalar(taille * (0.55 + 0.35 * (m.count / maxC)));
          positions.set(m.name, centre);
          // ── UN GROUPE EST UNE NUÉE, PAS UN POINT ────────────────────────
          // Diagnostic mesuré : l'intérieur n'était qu'une COQUILLE CREUSE de six
          // amas — au fond du zoom, la composition tombait à 18,9 % de contenu
          // contre 81,2 % à l'étage « monde ». On ne voyait donc plus la
          // décomposition en groupes, alors qu'elle existe dans les données.
          // Chaque ensemble se décompose désormais en SOUS-AMAS répartis autour
          // de sa direction : le groupe occupe une région au lieu d'un point, et
          // il y a de la matière dans toutes les directions où l'on regarde.
          // ── LA LOI D'ÉCHELLE : LE VOLUME DIT LA PART RÉELLE ─────────────
          // « Coupler les échelles à la taille de l'ensemble. » Avant :
          //     rayon = taille × (0,12 + 0,22 × count/maxC)
          // Deux défauts de sens, mesurables :
          //   1. la plage est FIXE (0,12 → 0,34) : un groupe 100 fois plus lourd
          //      qu'un autre n'a que 2,8 fois son rayon — le rapport réel est
          //      écrasé, et la scène ment sur les proportions ;
          //   2. elle ne dépend PAS de la taille globale : le même groupe occupe
          //      la même part d'un monde de 3 000 nœuds et d'un monde de 1,5 M.
          // Ici le rayon suit la racine CUBIQUE de la part DU MONDE
          // (`count / totalNoeudsMonde`, pas `count / maxC`) : le VOLUME (le
          // seul canal qui se lise dans une scène 3D) est alors EXACTEMENT
          // proportionnel au nombre de nœuds, et la loi DÉPEND de la taille de
          // l'ensemble — 15 000 nœuds dans un monde de 1,5 M (part 1 %) rendent
          // un rayon 4,5 fois plus petit que 15 000 nœuds dans un monde de
          // 17 000 (part 88 %). Normaliser par `maxC` était la forme déguisée
          // de l'ancienne faute : deux mondes de mêmes PROPORTIONS mais de
          // tailles différentes auraient rendu exactement les mêmes rayons.
          // `PART_MAX_GROUPE` borne la poupée dominante, le plancher est le
          // SEUL autre écart assumé (un groupe doit rester désignable au clic).
          const partVolume = Math.cbrt(m.count / totalNoeudsMonde);
          const rayon = taille * Math.min(
            PART_MAX_GROUPE,
            Math.max(PLANCHER_GROUPE, PART_MAX_GROUPE * partVolume),
          );
          // ── VOLUMÉTRIE RESPECTÉE ────────────────────────────────────────
          // Avant : `nb = min(1600, max(120, √nœuds × 12))`. Deux défauts de
          // vérité, mesurés : la racine ÉCRASE les rapports (75:1 réel devenait
          // 4:1) et le PLAFOND les ANNULAIT (tout ensemble au-delà de ~18 000
          // nœuds affichait exactement 1600 particules — donc 75 209 et 300 000
          // nœuds étaient indiscernables).
          // Ici le budget de particules est RÉPARTI AU PRORATA des effectifs
          // réels : chaque ensemble reçoit la part du budget égale à sa part de
          // nœuds. Les rapports sont donc exacts, et il n'y a plus de plafond qui
          // aplatisse — seulement un plancher pour qu'un petit ensemble reste
          // visible (et ce plancher est le seul écart, en bas d'échelle).
          const totalNoeuds = Math.max(1, modules.reduce((a, x) => a + x.count, 0));
          const sousAmas = Math.max(5, Math.min(12, Math.round(2 + m.count / maxC * 8)));
          const nb = Math.max(60, Math.round(BUDGET_PARTICULES * (m.count / totalNoeuds)));
          const parAmas = Math.max(12, Math.round(nb / sousAmas));
          const pos = new Float32Array(parAmas * sousAmas * 3);
          let w = 0;
          for (let s2 = 0; s2 < sousAmas; s2++) {
            const aa = s2 * 2.399 + i;             // déterministe
            const bb = Math.sin(s2 * 1.618 + i * 0.7) * 0.8;
            const d2 = new THREE.Vector3(Math.cos(aa), bb, Math.sin(aa)).normalize();
            const c2 = centre.clone().add(d2.multiplyScalar(rayon * 0.85));
            for (let k = 0; k < parAmas; k++) {
              const a3 = Math.random() * Math.PI * 2;
              const ph = Math.acos(2 * Math.random() - 1);
              const rr = rayon * 0.45 * Math.cbrt(Math.random());
              pos[w++] = c2.x + rr * Math.sin(ph) * Math.cos(a3);
              pos[w++] = c2.y + rr * Math.cos(ph);
              pos[w++] = c2.z + rr * Math.sin(ph) * Math.sin(a3);
            }
          }
          const g = new THREE.BufferGeometry();
          g.setAttribute("position", new THREE.BufferAttribute(pos, 3));
          // La couleur dit une FAMILLE ORGANISATIONNELLE RÉELLE : le premier
          // segment du nom (le dossier porteur). Avant, elle venait de l'INDEX de
          // l'ensemble (`typeColor(i)`) : six amas colorés se lisaient comme six
          // familles distinctes alors qu'aucune n'existait — du décor qui imitait
          // une classification. Une couleur sans signification doit être neutre,
          // jamais décorative.
          const pts = new THREE.Points(g, materiauGroupe(couleurFamille(m.name)));
          it.grp.add(pts);
          // `userData` rend l'ensemble DESIGNALBE : c'est ce qui permet de le
          // cliquer pour entrer dans SA declinaison (et pas dans boules[0]).
          pts.userData = { genre: "ensemble", qn: wd.qn, nom: m.name, count: m.count };
          // LE GROUPE PORTE SA RÉGION : c'est elle qui accueillera son graphe,
          // et c'est elle que l'enveloppe dessinera. Sans ces deux nombres, la
          // scène ne sait pas OÙ est le groupe — seulement qu'il existe.
          it.boules.push({ name: m.name, count: m.count, pts, centre: centre.clone(), rayon });
        });
        // ── L'ENVELOPPE DU GROUPE VISÉ : LA DEUXIÈME POUPÉE ────────────────
        // Un fil de fer à l'échelle du groupe, dans la couleur de sa famille :
        // c'est ce qui rend « le groupe » visible COMME UN LIEU, et non comme
        // un amas qu'on traverse. Elle se place sur le groupe choisi à chaque
        // image (une seule enveloppe par monde, pas une par groupe).
        {
          const env = new THREE.LineSegments(
            new THREE.EdgesGeometry(new THREE.SphereGeometry(1, 20, 12)),
            new THREE.LineBasicMaterial({
              color: new THREE.Color("#9fd6ff"), transparent: true, opacity: 0,
            }),
          );
          env.visible = false;
          it.enveloppe = env;
          it.grp.add(env);
        }
        // Liens RÉELS entre modules, quand le moteur en expose.
        const maxW = Math.max(1, ...arcs.map((a) => a.weight));
        for (const a of arcs) {
          const p1 = positions.get(a.src), p2 = positions.get(a.dst);
          if (!p1 || !p2) continue;
          // Tissu : fibres courbes additives, pas un trait plein.
          it.grp.add(fibre(p1, p2, a.weight, maxW));
        }
        it.pret = true;
        const boulesNommees = modules.map((m) => ({ name: m.name, count: m.count }));
        interieurBoulesRef.current = boulesNommees;
        /**
         * ── LA MESURE DE DENSITÉ, PUBLIÉE POUR ÊTRE MESURÉE ────────────────
         * `montres` / `total` / `geste` sont calculés ICI, une seule fois, par la
         * même loi que les gestes : densifier est alors un état OBSERVABLE, et le
         * contrôle peut exiger ce qui compte — densifier augmente `montres` sans
         * toucher `prefixe` ni `rang` ; approfondir fait l'inverse.
         */
        const decisionNiveau = decisionDensite({
          total: tousModules.length,
          montres: modules.length,
          feuille: it.feuille,
          plafondAtteint: it.plafondAtteint,
          // OÙ VIT LA PAGE SUIVANTE : un échantillon du graphe se densifie par un
          // échantillon plus large, un agrégat serveur par un `offset` — deux
          // appels différents, et la loi doit savoir lequel (règle 3 bis).
          source: sourceAxe ? "graphe" : "serveur",
          couverture: sourceAxe ? couvertureGraphe(wd.qn) : undefined,
        });
        publierOrganisation({
          prefixe: it.prefixe, rang: it.rang, page: it.page,
          tous: tousModules.length, montres: modules.length,
          plafondAtteint: it.plafondAtteint,
          feuille: !!it.feuille,
          geste: decisionNiveau.geste,
          // ── DANS CETTE VUE, TOUTE DENSIFICATION EST HONORABLE ─────────────
          // Mémoire (redessin), agrégat serveur (`offset`) ET échantillon du
          // graphe (plus de centres) : les trois chemins existent ici. Le
          // `&& !requiertAppel` du premier jet datait du temps où le troisième
          // n'existait pas — il masquait le geste exactement là où la navigation
          // se fait (un niveau descendu est toujours un échantillon).
          densifiable: decisionNiveau.geste === "densifier",
          // Ce qui a servi à trancher : `source` dit QUEL appel densifie, et
          // `couverture` dit POURQUOI (part du monde réellement lue).
          sourceDensite: sourceAxe ? "graphe" : "serveur",
          couverture: sourceAxe ? couvertureGraphe(wd.qn) : undefined,
          centres: it.centresGraphe,
          // ── LE CURSEUR DE PAGINATION EST PUBLIÉ, DONC MESURABLE ───────────
          // `offsetBrut` est ce que la PROCHAINE page serveur sautera. Sans lui,
          // un contrôle ne peut pas distinguer « la page suivante a été demandée »
          // de « la même page a été redemandée » : deux gestes qui se ressemblent
          // à l'écran et n'ont pas du tout le même effet.
          offsetBrut: it.offsetBrut,
          pagesVides: it.pagesVides ?? 0,
          raison: decisionNiveau.raison,
        });
        setInterieur({
          qn: wd.qn,
          n: modules.length,
          total: tousModules.length,
          // ── LA DÉCISION, CALCULÉE ICI ET PUBLIÉE TELLE QUELLE ─────────────
          // Elle est la MÊME que celle des gestes (`__etages` la reprend) : deux
          // calculs divergeraient, et le panneau annoncerait une densification
          // que le geste refuserait. `source`/`couverture` font donc partie des
          // arguments, sans quoi la copie de l'écran serait plus bête que la loi.
          decision: decisionDensite({
            total: tousModules.length,
            montres: modules.length,
            feuille: it.feuille,
            plafondAtteint: it.plafondAtteint,
            source: sourceAxe ? "graphe" : "serveur",
            couverture: sourceAxe ? couvertureGraphe(wd.qn) : undefined,
          }),
          page: it.page,
          plafondAtteint: it.plafondAtteint,
          source: sourceAxe ? "axe" : "familles",
          axe: sourceAxe ? dimensionRef.current : undefined,
          prefixe: it.prefixe,
          rang: it.rang,
          feuille: it.feuille,
          classes: it.classes,
          top: modules.slice(0, 3).map((m) => ({ name: m.name, count: m.count })),
          // TOUS les ensembles, pas les trois premiers : chaque boule de la
          // scène a désormais sa puce nommée, avec son effectif réel.
          boules: boulesNommees,
        });
      }).catch(() => { it.charge = false; });
    };

    /**
     * AXE DU TEMPS — l'histoire réelle d'un monde, au lieu de sa structure.
     *
     * `docs/viz-projection-densite.md` le réclamait : « Flux temporel (stores
     * runtime/cognitifs) : Session → Message → ToolCall a un sens chronologique.
     * La projection doit pouvoir basculer d'un espace des voisinages à un axe
     * du temps. » Les dates existent (`Session.createdAt`, mesuré : 605 sessions
     * sur l'ensemble des mondes) et n'étaient projetées nulle part.
     *
     * On ne fabrique rien : les bornes sortent des dates réelles du store, et
     * chaque tranche est datée. Un store sans session n'a PAS d'axe du temps —
     * on ne propose pas un axe vide.
     */
    const chargerTemps = (wd: WeaveWorld) => {
      const it = interieurs.get(wd.qn);
      const gr = graphes.get(wd.qn);
      if (!it || !gr || gr.chargeTemps) return;
      gr.chargeTemps = true;
      const taille = layout.orbits.get(wd.qn)?.size ?? 30;
      return import("../lib/api").then(({ runQuery }) =>
        runQuery(wd.path, "MATCH (n:Session) RETURN n.createdAt AS t ORDER BY t", "gql")
          .then((r) => {
            const ts: number[] = (r?.rows || [])
              .map((row: unknown[]) => Number(row[0]))
              .filter((x: number) => Number.isFinite(x) && x > 0);
            if (ts.length < 3) { gr.chargeTemps = false; return; }
            const t0 = ts[0], t1 = ts[ts.length - 1];
            const parts = 6;
            const bornes: { d: number; n: number }[] = [];
            for (let b = 0; b < parts; b++) {
              const lo = t0 + ((t1 - t0) * b) / parts;
              const hi = t0 + ((t1 - t0) * (b + 1)) / parts;
              bornes.push({ d: lo, n: ts.filter((x) => x >= lo && x < hi).length });
            }
            setTemps({
              qn: wd.qn, sessions: ts.length, t0, t1,
              tranches: bornes.map((b) => ({ d: b.d, n: b.n })),
            });
            // Le groupe TEMPS vit à part : on ne le construit qu'une fois, et
            // l'axe ne fait que l'afficher ou le cacher.
            const gTemps = new THREE.Group();
            gTemps.visible = false;
            const maxN = Math.max(1, ...bornes.map((b) => b.n));
            bornes.forEach((b, i) => {
              // L'axe est une LIGNE : la position encode la date, la taille
              // encode le nombre de sessions de la période.
              const x = ((i + 0.5) / parts - 0.5) * taille * 2.4;
              const rayon = taille * (0.06 + 0.2 * (b.n / maxN));
              const nb = Math.max(20, Math.round(60 * (b.n / maxN)) + 20);
              const pos = new Float32Array(nb * 3);
              const col = new Float32Array(nb * 3);
              const c = new THREE.Color(typeColor(i));
              for (let k = 0; k < nb; k++) {
                const a = Math.random() * Math.PI * 2;
                const ph = Math.acos(2 * Math.random() - 1);
                const rr = rayon * Math.cbrt(Math.random());
                pos[k * 3] = x + rr * Math.sin(ph) * Math.cos(a);
                pos[k * 3 + 1] = rr * Math.cos(ph);
                pos[k * 3 + 2] = rr * Math.sin(ph) * Math.sin(a);
                col[k * 3] = c.r; col[k * 3 + 1] = c.g; col[k * 3 + 2] = c.b;
              }
              const g2 = new THREE.BufferGeometry();
              g2.setAttribute("position", new THREE.BufferAttribute(pos, 3));
              g2.setAttribute("aColor", new THREE.BufferAttribute(col, 3));
              const tailles = new Float32Array(nb).fill(7);
              g2.setAttribute("aTaille", new THREE.BufferAttribute(tailles, 1));
              gTemps.add(new THREE.Points(g2, materiauMonde()));
            });
            // Le fil du temps : une ligne qui relie les périodes.
            gTemps.add(new THREE.Line(
              new THREE.BufferGeometry().setFromPoints([
                new THREE.Vector3(-taille * 1.2, 0, 0),
                new THREE.Vector3(taille * 1.2, 0, 0),
              ]),
              new THREE.LineBasicMaterial({ color: 0x9fd6ff, transparent: true, opacity: 0.5 }),
            ));
            it.grp.add(gTemps);
            gr.grpTemps = gTemps;
            gr.pretTemps = true;
          })
          .catch(() => { gr.chargeTemps = false; }),
      );
    };

    /**
     * Peuple l'étage GRAPHE : le graphe RÉEL du plus gros ensemble du monde
     * (nœuds, arêtes, triangles réels K3). On entre par un nom d'ensemble, qui
     * résout comme centre — vérifié : `deepseek_harness` donne 60 nœuds et
     * 11 077 arêtes réelles (CALLS, CONTAINS_FILE, HAS_BRANCH).
     */
    /**
     * Peuple l'étage graphe, en DEUX PHASES — parce que la mesure l'impose.
     *
     * Timeline mesurée : `hops=2 limit=300` coûte **27 826 ms** (l'expansion du
     * second niveau est le poste dominant de toute la descente), alors que
     * `hops=1` coûte **0,84 s**. Attendre le graphe complet, c'est faire patienter
     * 31 secondes devant un étage vide — inacceptable, et c'est exactement ce que
     * la timeline a rendu visible.
     *
     *   phase « rapide » (hops=1) : le graphe RÉEL du voisinage, en moins d'une
     *   seconde — nœuds, arêtes, types. L'étage est peuplé tout de suite ;
     *   phase « complète » (hops=2) : les MOTIFS (K3/K4) arrivent ensuite et
     *   remplacent la projection. `hops=1` étant une étoile, les motifs y sont
     *   nuls par construction — d'où la seconde phase, sans laquelle on perdrait
     *   les triangles.
     */
    const chargerGraphe = (
      wd: WeaveWorld, centre: string, mode: "rapide" | "complet" = "complet",
      /**
       * ── LE GRAPHE D'UN NIVEAU DE LA HIÉRARCHIE ──────────────────────────
       * Un groupe de la descente fractale n'est pas UN nœud : c'est un ENSEMBLE de
       * nœuds, désigné par un chemin de tokens. Le moteur sait rendre le voisinage
       * de PLUSIEURS centres (`center=a,b,c` — vérifié : 4 centres → 200, 21
       * nœuds), donc chaque sous-groupe peut avoir SON graphe, à chaque niveau.
       * Sans cela, entrer dans `packages` changeait l'intérieur mais laissait le
       * graphe du niveau précédent : la navigation ne suivait qu'à moitié.
       */
      centres: string[] = [],
    ) => {
      const gr = graphes.get(wd.qn);
      const it = interieurs.get(wd.qn);
      if (!gr) return;
      /**
       * ── LE GARDE DOIT PORTER SUR LE CENTRE, PAS SEULEMENT SUR « DÉJÀ CHARGÉ » ─
       *
       * MESURÉ (2026-09-15) : `if (mode === "rapide" && (gr.charge || gr.pret))`
       * refusait TOUT rechargement dès la première réussite. Or `chargerGraphe`
       * est précisément la voie du RECENTRAGE : le clic sur un nœud de la scène
       * (`onClick` → `genre === "noeud"`) et la descente le long de l'axe
       * l'appellent avec un AUTRE centre. Le garde les renvoyait donc tous les
       * deux sans rien faire : le graphe restait figé sur sa première entrée.
       * Deux symptômes, une cause — et le second était invisible : « ⤓ descendre »
       * ajoutait bien une poupée au fil, mais la matière ne changeait pas (mêmes
       * 20 000 nœuds, même entrée) : une profondeur annoncée et non produite.
       *
       * On ne recharge donc que ce qui est RÉELLEMENT déjà là : même monde ET même
       * centre. Un centre différent EST une demande de recentrage.
       */
      const centreActuel = noeudsGrapheRef.current?.qn === wd.qn
        ? noeudsGrapheRef.current.centre : null;
      const memeCentre = centreActuel === centre;
      /**
       * ── LE MÊME CENTRE AVEC PLUS DE CENTRES EST UNE DENSIFICATION ─────────
       *
       * Le garde « même centre » a été écrit pour le RECENTRAGE (un centre
       * différent = une demande de recentrage) ; il refusait donc, sans un mot, le
       * cas symétrique : le MÊME niveau redemandé avec un échantillon plus large.
       * Or c'est exactement ce que fait « ＋ densifier » quand le niveau est
       * découpé sur le graphe (`densite.ts`, règle 3 bis) : le budget passe de 32 à
       * 64 centres, et le garde renvoyait la demande — le geste aurait annoncé une
       * densification et n'aurait rien émis. Un budget plus grand EST une demande
       * de densification, au même titre qu'un centre différent est un recentrage.
       */
      const budget = centres.length;
      const densifie = mode === "rapide" && gr.pret && memeCentre && budget > (gr.centresUtilises ?? 0);
      // 1. Une requête du MÊME mode est déjà en vol : on ne la double pas (le
      //    centre suivant sera pris en compte par le geste d'après).
      // Journal des gardes : « lequel a refusé » est la seule question qui compte
      // quand un rechargement ne se produit pas.
      const jr = (etagesRef.current as Record<string, unknown>).chargements =
        ((etagesRef.current as Record<string, unknown>).chargements ?? { demandes: 0, enVol: 0, memeCentre: 0, lancees: 0 }) as
        { demandes: number; enVol: number; memeCentre: number; lancees: number; dernier: string; liberes?: number };
      jr.demandes++;
      jr.dernier = centre;
      if (mode === "rapide" ? gr.charge : gr.chargeComplet) { jr.enVol++; return; }
      // 2. Le graphe affiché EST déjà celui du centre demandé — ET au même budget.
      if (mode === "rapide" && gr.pret && memeCentre && !densifie) { jr.memeCentre++; return; }
      if (mode === "complet" && gr.pretComplet && memeCentre && !densifie) { jr.memeCentre++; return; }
      jr.lancees++;
      /**
       * ── UN CHARGEMENT NE DOIT PAS RESTER « EN VOL » POUR TOUJOURS ──────────
       * MESURÉ : après une descente réussie (20 000 → 157 nœuds, entrée changée),
       * `gr.charge` restait vrai PLUS DE QUARANTE SECONDES, et le geste suivant
       * était donc refusé par le garde « une requête du même mode est déjà en
       * vol ». Le bouton avait l'air vivant et ne faisait plus rien : c'est la
       * pire forme de panne, celle qui ne se plaint pas. La cause est une attente
       * qui ne se dénoue pas (résolution de nom ou flux WS). On ne peut pas
       * garantir qu'une requête réseau se termine, mais on peut BORNER L'ATTENTE :
       * passé ce délai le verrou se libère, et le geste suivant repart. La
       * délivrance est comptée (`liberes`) — elle est visible, pas silencieuse.
       */
      const delai = window.setTimeout(() => {
        if (gr.charge || gr.chargeComplet) {
          gr.charge = false; gr.chargeComplet = false;
          jr.liberes = (jr.liberes ?? 0) + 1;
        }
      }, 20_000);
      /**
       * 3. Sinon on charge — et `gr.pret` RESTE tel quel PENDANT le chargement :
       *    l'ancienne matière reste à l'écran jusqu'à ce que la nouvelle arrive,
       *    et c'est seulement là qu'on vide le groupe (`recentrage`, plus bas).
       *    Poser `pret = false` ici ferait disparaître le graphe pendant toute la
       *    requête : l'écran vide entre deux étages que ce fichier s'interdit.
       */
      const recentrage = !memeCentre;
      if (mode === "rapide") gr.charge = true; else gr.chargeComplet = true;
      // ── UNE « FAMILLE » QUI COUVRE TOUT LE STORE N'EST PAS UNE ENTRÉE ──
      // Mesuré sur `drivers-net` : `/api/viz/modules` rend UN SEUL ensemble,
      // « drivers » (310 547 nœuds) — le store entier. Viser ce nom, c'est viser
      // un PRÉFIXE : on paie d'abord un `hops=2 limit=300` inutile (lent sur
      // 310 k nœuds) et on ne découvre la dégradation qu'après. On le sait
      // AVANT, en comparant l'effectif de la famille à celui du monde —
      // l'information est déjà dans la charge utile du tissage.
      const familleCouvreTout = !!(
        it?.boules[0] && wd.nodes > 0 && it.boules[0].count >= 0.5 * wd.nodes
      );
      /**
       * ── UN NOM DE FAMILLE N'EST PAS UN qualified_name ──────────────────────
       *
       * `/api/viz/modules` rend des FAMILLES — le premier segment porteur du
       * chemin (`deepseek_harness`, `banc_obrain`, `aletheia` sur ce store) — et
       * ce ne sont PAS des nœuds. Un `qualified_name` réel porte toujours un
       * séparateur `.` ou `:` (`home.triviere.projects.deepseek_harness`,
       * `…:branch:master`).
       *
       * MESURÉ sur la page servie, descente complète à la molette :
       *     → /api/viz/subgraph …center=deepseek_harness&hops=1&limit=20000  ← 400
       *     → /api/viz/subgraph …center=deepseek_harness&hops=2&limit=200000 ← 400
       *     et AUCUN appel de résolution `/api/functions` derrière.
       * Résultat : la chronologie annonçait « graphe ✓ », `__etages.graphe.ouv`
       * valait 1 — et le panneau « Graphe : … » n'apparaissait JAMAIS. Le FOND de
       * la descente était donc vide, en silence, alors qu'il est la raison d'être
       * des deux étages précédents.
       *
       * La cause est structurelle : `fetchSubgraph` REJETTE sur un 400, or c'est
       * `.then((g0) => …)` qui portait le repli par recherche. Un rejet saute le
       * `.then` — le repli ne pouvait donc pas se déclencher, et la chaîne
       * retombait sur le `.catch` qui se contente de libérer le verrou. Deux
       * corrections, et il faut les deux :
       *   1. on RÉSOUT D'ABORD quand le nom ne peut pas être un qn (pas de
       *      séparateur) : l'appel voué au refus n'est même plus émis ;
       *   2. le repli devient ATTEIGNABLE EN CAS D'ÉCHEC, pas seulement en cas de
       *      réponse pauvre — un rejet doit mener à la résolution, jamais à un
       *      étage vide. `familleCouvreTout` reste l'autre cas connu.
       */
      const nomResolu = /[.:]/.test(centre);
      // ── LA MÉMOIRE DE RÉSOLUTION EST LIÉE AU CENTRE QU'ELLE A RÉSOLU ───────
      // `gr.entree` retenait le qn résolu pour UN centre ; le réutiliser pour un
      // autre ferait charger le voisinage du mauvais nœud — silencieusement. On
      // ne réutilise donc la résolution que si c'est le même centre demandé.
      const dejaResolu = gr.entreePour === centre ? gr.entree : undefined;
      const entreeIllisible = !nomResolu && !dejaResolu;
      const cible = dejaResolu ?? centre;
      const taille = layout.orbits.get(wd.qn)?.size ?? 30;
      const multi = centres.length > 0;
      return import("../lib/api").then(({ fetchSubgraph, searchFunctions }) =>
        // hops=2, et non 1 : mesuré, à hops=1 l'échantillon est une ÉTOILE
        // (300 nœuds mais 299 arêtes internes — les autres partent vers
        // l'extérieur), donc K3 = 0 PAR CONSTRUCTION et la projection ne pouvait
        // pas montrer de motifs. À hops=2 : 199 triangles sur 300 nœuds.
        // ── UN CENTRE ILLISIBLE NE DEVIENT PAS LISIBLE EN REDEMANDANT ────────
        // MESURÉ, et c'est une régression que j'avais moi-même introduite : avec
        // un axe de RELATION, les groupes de l'étage intérieur s'appellent
        // « bord ∂ » / « intérieur » — des noms qui ne sont pas des nœuds. Le
        // court-circuit ne couvrait que la phase « rapide », donc la phase
        // « motifs » émettait quand même l'appel, et le serveur répondait 400
        // (« centre introuvable : bord ∂ »). Le 400 que j'avais supprimé pour les
        // noms de familles revenait par une autre porte.
        // Quand le centre ne peut pas être un qualified_name, les DEUX phases
        // passent par la résolution : on ne paie pas deux fois un refus connu.
        ((entreeIllisible || (familleCouvreTout && mode === "rapide"))
          // Court-circuit : on économise l'appel le plus coûteux de la chaîne,
          // puisque l'on sait déjà qu'il conclura à une entrée inexploitable.
          ? Promise.resolve({ nodes: [], edges: [] } as unknown as Awaited<ReturnType<typeof fetchSubgraph>>)
          // ── LA LISTE DE CENTRES, OU LE CENTRE RÉSOLU ──────────────────────
          // Un groupe de la hiérarchie n'est pas UN nœud mais un ENSEMBLE : le
          // moteur accepte plusieurs centres (`center=a,b,c`) et rend leur
          // voisinage commun. C'est ce qui donne à chaque sous-groupe SON graphe,
          // au lieu de laisser celui du niveau précédent.
          : fetchSubgraph(wd.path, multi ? centres : [cible],
            // ── LE GAIN EST DANS LE BUDGET, PAS DANS LA PROFONDEUR ──────────
            // Mesuré sur ce store, centre src :
            //   hops=1 :   534 nœuds   533 arêtes     116 ms
            //   hops=2 : 8 000 nœuds 27 319 arêtes    2 611 ms
            //   hops=3 : 8 000 nœuds 48 574 arêtes   27 314 ms  <-- 10x le temps
            //                                                      pour 0 nœud de plus
            // `hops=3` atteint le même plafond mais paie la traversée complète
            // avant de tronquer : c'est un pur gaspillage, et j'avais tort de le
            // choisir au tour précédent.
            // En revanche le BUDGET était très en dessous du possible : 150 et
            // 300 nœuds demandés là où `hops=2` en rend 8 000 pour le MÊME coût de
            // traversée. On garde donc la profondeur d'origine (1 puis 2) et on
            // relève les budgets de ×6 et ×13 — gratuit en latence.
            mode === "rapide" ? 1 : 2,
            // PAS DE PLAFOND : la phase rapide demande déjà bien plus que ce que
            // l'écran montrait (900), et la phase complète demande ce que le
            // moteur peut servir. Le coût est la TRAVERSÉE, pas le nombre — mesuré :
            // `hops=2` rend 8 000 nœuds au même prix que 300.
            //
            // ── MESURÉ, ET CE PLAFOND-CI EST JUSTIFIÉ PAR LA MÊME RÈGLE ───────
            // Une fois l'entrée RÉSOLUE, la phase « motifs » demandait 200 000 et
            // recevait, sur ce store :
            //     hops=2 limit=200000 → 75 210 nœuds · 88 667 arêtes · 38 Mo
            //     hops=2 limit= 20000 → 20 000 nœuds · 88 667 arêtes · 16 Mo
            // LES ARÊTES SONT IDENTIQUES — donc les MOTIFS aussi : les triangles et
            // tétraèdres se comptent sur les arêtes, pas sur les nœuds isolés.
            // Demander 200 000 n'achetait donc que 55 210 points de plus, pour
            // 22 Mo de charge utile et un rendu trois fois plus lourd — sur une
            // vue dont la qualité première doit être la fluidité. On s'aligne sur
            // le budget de la phase rapide, celui qui contient déjà TOUTES les
            // arêtes. Rien n'est caché : le panneau annonce « N nœuds sur M ».
            20_000, "energy")
            // ── UN ÉCHEC DOIT MENER À LA RÉSOLUTION, PAS À UN ÉTAGE VIDE ─────
            // `fetchSubgraph` rejette sur un 400 (« centre introuvable ») : sans
            // ce `.catch`, le `.then` ci-dessous — qui PORTE le repli par
            // recherche — n'était jamais atteint, et le fond de la descente
            // restait vide sans un mot. On transforme donc l'échec en « réponse
            // pauvre », ce que le repli sait déjà traiter.
            .catch(() => null as unknown as Awaited<ReturnType<typeof fetchSubgraph>>))
          .then(async (g0) => {
            // ── ENTRÉE RÉSOLUE PAR RECHERCHE ─────────────────────────────
            // Mesuré : sur le monde « linux », l'ensemble le plus gros s'appelle
            // `drivers` (963 403 nœuds) — un PRÉFIXE de qn, pas un nœud : le
            // sous-graphe rend 1 nœud et 0 arête. Les VRAIS nœuds, eux,
            // fonctionnent (`tcp_v4_rcv` → 36 nœuds, `bus_register` → 12).
            // On cherche donc un nœud réel sous ce nom et on garde le candidat
            // le MIEUX CONNECTÉ — pas le premier venu, qui peut être une variable
            // isolée (constaté : `drivers_kset` → 1 nœud).
            let g = g0;
            let entree = gr.entree ?? centre;
            // La SEULE condition qui compte : la réponse est-elle exploitable ?
            // (Le court-circuit et le rejet produisent tous deux `null`/vide, donc
            // ce test les couvre tous les deux — une condition par cause serait
            // une de plus à oublier.)
            // Avec des qn déjà résolus (multi-centres), la recherche de repli
            // n'a aucun sens : le « centre » est un CHEMIN, pas un nom de nœud.
            if (!multi && (!g?.nodes?.length || g.nodes.length < 5)) {
              // ── UN SEUL APPEL DE RÉSOLUTION, PAS QUATRE ────────────────
              // Profil mesuré : chaque appel de sous-graphe coûte ~1,5 s de
              // coût FIXE (indépendant du volume : degré 11 077 → 1,84 s ;
              // degré 27 → 1,37 s), et l'expansion du 2ᵉ niveau ajoute 4 à 11 s.
              // La version précédente essayait jusqu'à QUATRE candidats pour
              // choisir « le mieux connecté » : jusqu'à 4 × 1,5 s pour décider…
              // et elle finissait par choisir le nœud LE PLUS COÛTEUX. On prend
              // donc le premier résultat utilisable et on fait UN appel.
              const { items } = await searchFunctions(wd.path, centre).catch(() => ({ items: [] }));
              const cand = (items || []).find((x) => x.qn && x.type !== "Field") || (items || [])[0];
              if (cand?.qn) {
                entree = cand.qn;
                // LA PROFONDEUR DE LA PHASE EST CONSERVÉE : la résolution sert
                // aussi bien l'étage « organisation » (hops=1, tout de suite) que
                // les MOTIFS (hops=2 — à hops=1 l'échantillon est une étoile, donc
                // K3=0 par construction). Un `hops=1` en dur ici perdait les
                // motifs de la phase complète.
                const prof = mode === "rapide" ? 1 : 2;
                const bud = 20_000;
                g = await fetchSubgraph(wd.path, [entree], prof, bud, "energy").catch(() => g);
                // On RETIENT ce qui a marché : la phase suivante n'aura plus à
                // rechercher, ni à réémettre l'appel refusé.
                if (g?.nodes?.length) { gr.entree = entree; gr.entreePour = centre; }
              }
            }
            if (!g?.nodes?.length) return;
            if (mode === "complet" || recentrage) {
              // La phase complète REMPLACE la projection, et un recentrage aussi :
              // on vide le groupe, sinon les deux jeux de nœuds se superposeraient.
              gr.grp.clear();
            }
            const gN = new THREE.BufferGeometry();
            const pos = new Float32Array(g.nodes.length * 3);
            const col = new Float32Array(g.nodes.length * 3);
            const taille2 = new Float32Array(g.nodes.length);
            const chal = new Float32Array(g.nodes.length);
            const idx = new Map<string, number>();
            const R = taille * 1.15; // disposition interne : TOUTE la boule
            // ── LA POUPÉE D'ACCUEIL : UNE SEULE AUTORITÉ, ICI ────────────────
            // La disposition ci-dessus reste à l'échelle du monde (elle est
            // mesurée, avec ses motifs) ; c'est le GROUPE qui décide de la taille
            // finale et de la place. On calcule donc ici, une fois, ce que la
            // boucle appliquera : où est le groupe dont ce graphe est le contenu,
            // et de quel facteur il faut réduire la disposition pour qu'elle
            // tienne DEDANS. `echelle` sert aussi aux tailles de nœuds : sans
            // elle, les points garderaient leur taille absolue dans un volume
            // 200 fois plus dense — un bloc saturé au lieu d'un graphe lisible.
            // (L'échelle des tailles est ce qui préserve l'ENCRE totale, propriété
            // déjà tenue par la loi en 1/√N juste en dessous.)
            {
              const itG = interieurs.get(wd.qn);
              const bouleG = itG?.boules.find((x) => x.name === centre);
              if (bouleG) {
                gr.ensemble = centre;
                gr.centreGroupe = bouleG.centre.clone();
                gr.echelle = Math.min(1, bouleG.rayon / Math.max(1e-3, R));
              } else if (!gr.centreGroupe) {
                // Cas réel : on a cliqué un NŒUD, donc recentré le graphe sur son
                // voisinage. Ce nœud appartient à un groupe : la poupée d'accueil
                // ne change pas, on garde donc celle où l'on est déjà — la
                // remplacer par « toute la boule » ferait sortir le graphe de son
                // groupe, ce que ce correctif existe précisément pour empêcher.
                gr.centreGroupe = new THREE.Vector3();
                gr.echelle = 1;
              }
            }
            const gph = Math.PI * (3 - Math.sqrt(5));
            g.nodes.forEach((nd, i) => {
              const h = (() => { let x = 0; for (const c of String(nd.id)) x = (x * 31 + c.charCodeAt(0)) >>> 0; return x; })();
              const y = 1 - (i / Math.max(1, g.nodes.length - 1)) * 2;
              const rad = Math.sqrt(Math.max(0, 1 - y * y));
              const a = gph * i + (h % 360);
              pos[i * 3] = R * rad * Math.cos(a);
              pos[i * 3 + 1] = R * y * 0.8;
              pos[i * 3 + 2] = R * rad * Math.sin(a);
              // ── LA COULEUR DOIT DIRE QUELQUE CHOSE — OU RIEN ────────────────
              // Elle valait `typeColor(h % 10)`, où `h` est un HACHAGE de
              // l'identifiant : une couleur SANS signification, qui imitait une
              // classification. Le commentaire annonçait « type réel » ; le code
              // faisait un hachage. C'est exactement ce que ce fichier condamne
              // ailleurs pour les ensembles (« du décor qui imite une
              // classification ; une couleur sans signification doit être
              // neutre, jamais décorative »).
              // Le moteur fournit `community_id` (détection Leiden) : c'est une
              // classification RÉELLE. On la peint. Sans elle, on reste NEUTRE
              // plutôt que de décorer.
              const c = nd.community_id != null
                ? new THREE.Color(typeColor(nd.community_id))
                : new THREE.Color(0x8a93a6); // neutre assumé : aucun sens à peindre
              col[i * 3] = c.r; col[i * 3 + 1] = c.g; col[i * 3 + 2] = c.b;
              // TAILLE ∝ 1/√N : l'ENCRE TOTALE RESTE CONSTANTE.
              //
              // La taille était FIXE (7 unités-monde). Avec 300 nœuds cela se lit ;
              // avec 4 000 — ce que ce palier demande maintenant — l'écran sature
              // en taches pleines et l'on ne voit plus rien. La surface couverte
              // vaut N·taille² : pour la garder constante quand N est multiplié
              // par 13, la taille doit être divisée par √13. Le détail suit donc
              // l'échelle au lieu de l'écraser.
              taille2[i] = 7 * Math.sqrt(Math.min(1, 300 / Math.max(1, g.nodes.length)))
                * (gr.echelle ?? 1);
              chal[i] = Math.max(0, Math.min(1, nd.energy ?? 0));
              idx.set(String(nd.id), i);
            });
            /**
             * ── LA DISPOSITION « GRAPHE » DOIT ÊTRE CONSERVÉE À PART ────────
             *
             * MESURÉ (2026-09-15) : `gr.posGraphe` était LU à deux endroits —
             * la migration vers l'espace des signaux — et **JAMAIS ASSIGNÉ**.
             * La condition `if (montrableG && gr.posGraphe && gr.posSignaux)`
             * était donc TOUJOURS fausse : l'étage des signaux, décrit ici sur
             * vingt lignes (nœuds ET arêtes qui voyagent ensemble), ne pouvait
             * pas s'exécuter. Une profondeur qui existait sur le papier et
             * mourait à une ligne manquante.
             *
             * Il faut une COPIE, et non `pos` lui-même : `pos` EST le tampon
             * vivant du `BufferAttribute` (juste en dessous), que la migration
             * écrit à chaque image. La lui donner comme origine ferait lire la
             * valeur déjà mélangée — le graphe dériverait vers les signaux sans
             * jamais pouvoir en revenir, et la moindre image le ferait glisser
             * un peu plus loin. L'origine doit être INTACTE.
             */
            gr.posGraphe = pos.slice();
            gN.setAttribute("position", new THREE.BufferAttribute(pos, 3));
            gN.setAttribute("aColor", new THREE.BufferAttribute(col, 3));
            gN.setAttribute("aTaille", new THREE.BufferAttribute(taille2, 1));
            gN.setAttribute("aChaleur", new THREE.BufferAttribute(chal, 1));
            gN.setAttribute("aEmphase", new THREE.BufferAttribute(new Float32Array(g.nodes.length).fill(1), 1));
            {
              const ptsG = new THREE.Points(gN, materiauNoeud());
              ptsG.userData = { genre: "noeud", qn: wd.qn };
              gr.pts = ptsG;
              gr.grp.add(ptsG);
            }
            // Arêtes réelles (segments), puis triangles réels K3.
            const seg: number[] = [];
            const paires: [number, number][] = [];
            const adj = new Map<number, Set<number>>();
            for (const e of g.edges || []) {
              const a = idx.get(String(e.src)), b = idx.get(String(e.dst));
              if (a == null || b == null) continue;
              paires.push([a, b]);
              seg.push(pos[a * 3], pos[a * 3 + 1], pos[a * 3 + 2],
                       pos[b * 3], pos[b * 3 + 1], pos[b * 3 + 2]);
              if (!adj.has(a)) adj.set(a, new Set());
              if (!adj.has(b)) adj.set(b, new Set());
              adj.get(a)!.add(b); adj.get(b)!.add(a);
            }
            if (seg.length) {
              const gE = new THREE.BufferGeometry();
              gE.setAttribute("position", new THREE.Float32BufferAttribute(seg, 3));
              const lignesG = new THREE.LineSegments(gE, new THREE.LineBasicMaterial({
                // 11 077 aretes reelles : en additif tres bas, la densite de
                // tissage devient visible au lieu de saturer en barres pleines.
                color: 0x4aa8ff, transparent: true, opacity: 0.045,
                blending: THREE.AdditiveBlending, depthWrite: false,
              }));
              lignesG.userData = { genre: "arete", qn: wd.qn };
              gr.lignes = lignesG;
              gr.grp.add(lignesG);
            }
            // MOTIFS RÉELS : K3 (triangles) puis K4 (tétraèdres), calculés sur
            // l'adjacence échantillonnée — pas décorés. Un K4 est un triangle
            // auquel on cherche un quatrième sommet adjacent aux TROIS autres ;
            // on réutilise les triangles déjà trouvés plutôt que de repartir de
            // zéro.
            const tri: number[] = [];
            const tetra: number[] = [];
            let compte = 0;
            let compte4 = 0;
            const sommets = (i: number) => (i * 3);
            adj.forEach((nbs, a) => {
              const arr = [...nbs];
              for (let i = 0; i < arr.length && compte < 120; i++)
                for (let j = i + 1; j < arr.length && compte < 120; j++) {
                  if (!adj.get(arr[i])?.has(arr[j])) continue;
                  const A = arr[i], B = arr[j];
                  tri.push(pos[sommets(a)], pos[sommets(a) + 1], pos[sommets(a) + 2],
                           pos[sommets(A)], pos[sommets(A) + 1], pos[sommets(A) + 2],
                           pos[sommets(B)], pos[sommets(B) + 1], pos[sommets(B) + 2]);
                  compte++;
                  // ── K4 : un quatrième sommet adjacent aux trois ──
                  if (compte4 < 40) {
                    for (const d of adj.get(A) ?? []) {
                      if (d === a || d === B) continue;
                      if (!adj.get(a)?.has(d) || !adj.get(B)?.has(d)) continue;
                      // 4 faces du tétraèdre (a, A, B, d)
                      for (const [x, y, z] of [[a, A, B], [a, A, d], [a, B, d], [A, B, d]] as const) {
                        tetra.push(
                          pos[sommets(x)], pos[sommets(x) + 1], pos[sommets(x) + 2],
                          pos[sommets(y)], pos[sommets(y) + 1], pos[sommets(y) + 2],
                          pos[sommets(z)], pos[sommets(z) + 1], pos[sommets(z) + 2],
                        );
                      }
                      compte4++;
                      break;
                    }
                  }
                }
            });
            if (tri.length) {
              const gT = new THREE.BufferGeometry();
              gT.setAttribute("position", new THREE.Float32BufferAttribute(tri, 3));
              gr.grp.add(new THREE.Mesh(gT, new THREE.MeshBasicMaterial({
                color: 0x21d4b4, transparent: true, opacity: 0.12,
                side: THREE.DoubleSide, depthWrite: false,
              })));
            }
            if (tetra.length) {
              const gK4 = new THREE.BufferGeometry();
              gK4.setAttribute("position", new THREE.Float32BufferAttribute(tetra, 3));
              gr.grp.add(new THREE.Mesh(gK4, new THREE.MeshBasicMaterial({
                color: 0xffd166, transparent: true, opacity: 0.1,
                side: THREE.DoubleSide, depthWrite: false,
              })));
            }
            // ── ESPACE DES SIGNAUX ───────────────────────────────────────
            // Chaque nœud est replacé selon son VECTEUR réel : courbure (x),
            // énergie (y), degré dans l'échantillon (z). Si ces composantes sont
            // dégénérées (mêmes valeurs partout), on le DIT au lieu de projeter
            // un point unique en faisant croire à une structure.
            const cur = g.nodes.map((nd) => nd.curvature ?? 0);
            const ene = g.nodes.map((nd) => nd.energy ?? 0);
            const deg = g.nodes.map((_, i) => adj.get(i)?.size ?? 0);
            /**
             * ── `Math.min(...a)` ÉTAIT UNE BOMBE À RETARDEMENT ──────────────
             *
             * Cet appel étalait le tableau en ARGUMENTS. Mesuré : V8 lève
             * `RangeError: Maximum call stack size exceeded` au-delà de ~125 000
             * arguments (limite constatée entre 125 258 et 125 292 sur Node
             * 22.19.0 / Chrome). Or la phase « complet » demande 200 000 nœuds
             * (voir `limit` plus haut) : la phase complète mourait donc APRÈS
             * avoir payé la traversée — et l'erreur était AVALÉE en silence par
             * le `.catch(() => { gr.charge = false; })`, si bien que rien ne le
             * disait. Un seul parcours rend min, max et le nombre de valeurs
             * distinctes, sans étalement : le coût est le même à 160 qu'à
             * 200 000 nœuds, et il ne dépend plus de la taille de la pile.
             */
            const stat = (a: number[]) => {
              let min = Infinity, max = -Infinity, vues = 0;
              const distinctes = new Set<number>();
              for (let i = 0; i < a.length; i++) {
                const v = a[i];
                if (Number.isFinite(v)) {
                  if (v < min) min = v;
                  if (v > max) max = v;
                  if (!distinctes.has(v)) { distinctes.add(v); vues++; }
                }
              }
              return {
                min: Number.isFinite(min) ? min : 0,
                max: Number.isFinite(max) ? max : 0,
                distinctes: vues,
              };
            };
            const sc = stat(cur), se = stat(ene), sd = stat(deg);
            const sig = { courbure: sc, energie: se, degre: sd,
              degeneré: sc.distinctes < 2 && se.distinctes < 2 };
            gr.sig = sig;
            // ⚠ CET APPEL AVAIT ÉTÉ EMPORTÉ par le découpage fautif du round 4 :
            // le setter était déclaré mais jamais invoqué, donc l'étage SIGNAUX
            // ne s'affichait JAMAIS — sur aucun store. Ma restauration d'alors
            // était incomplète : je n'avais rétabli que les lignes du graphe.
            setSignaux({ qn: wd.qn, ...sig, n: g.nodes.length });
            const posSig = new Float32Array(g.nodes.length * 3);
            const norm = (v: number, st: { min: number; max: number }) =>
              st.max - st.min < 1e-9 ? 0 : ((v - st.min) / (st.max - st.min)) * 2 - 1;
            g.nodes.forEach((_, i) => {
              posSig[i * 3] = norm(cur[i], sc) * R;
              posSig[i * 3 + 1] = norm(ene[i], se) * R;
              posSig[i * 3 + 2] = norm(deg[i], sd) * R * 0.6;
            });
            gr.paires = paires;
            // Le SECOND bout du voyage : la position de chaque nœud dans l'espace
            // des signaux (courbure · énergie · degré), qui existait déjà mais
            // n'était jamais remise à la boucle — voir `gr.posGraphe` plus haut.
            gr.posSignaux = posSig;
            // DIMENSION DE GROUPEMENT : la CHAÎNE de nom, pas le libellé.
            // Erreur corrigée : `label` du sous-graphe est le libellé
            // d'affichage (le qn lui-même), donc grouper dessus donnait un
            // groupe par nœud — inutile. Le type d'entité n'est pas exposé par
            // cet endpoint ; on groupe donc sur ce qu'on possède réellement :
            // le DOSSIER PORTEUR (voir `parentDe`), qui est exactement la
            // « chaîne » dont parle la demande.
            //
            // ⚠ ICI VIVAIT LA SECONDE RÈGLE DE PARENTÉ (`parts[parts.length-2]`
            // sans filtrer `__file__`). Elle divergeait de celle de
            // `etiquetteDimension` sur TOUS les qn marqués `__file__` — c'est-à-
            // dire tous les fichiers indexés —, donc `infos[i].type` valait
            // « rst » là où la puce annonçait « msg_zerocopy », et le filtre
            // (`infos[i].type === filtre`) ne pouvait rien allumer. Les deux
            // passent maintenant par la MÊME fonction.
            const fichiers = new Map<string, string>();
            gr.noeudsInfo = g.nodes.map((nd) => {
              const qn = String(nd.qn || nd.id);
              if (nd.file_path) fichiers.set(qn, nd.file_path);
              return { qn, type: parentDe(qn, nd.file_path) };
            });
            fichiersRef.current = fichiers;
            // LA MATIÈRE PREMIÈRE DE LA FRATRIE : les nœuds BRUTS du sous-graphe.
            // `noeudsInfo` ne porte que `{qn, type}` — insuffisant pour indexer
            // (`file_path`, `community_id`, `scc_id` sont nécessaires, et c'est
            // justement ce que l'endpoint fournit et que personne ne lisait).
            noeudsGrapheRef.current = { qn: wd.qn, centre: entree, noeuds: g.nodes };
            // ── L'ENTRÉE S'AJOUTE AU CHEMIN (si ce n'est pas la même) ────────
            // On ne pousse que sur un CHANGEMENT : sans cela, un simple
            // rafraîchissement empilerait dix fois la même entrée et le fil
            // deviendrait illisible dès qu'on redemande le graphe en cours.
            {
              const ch = gr.entrees ?? (gr.entrees = []);
              const dernier = ch[ch.length - 1];
              if (!dernier || dernier.nom !== entree) {
                ch.push({ nom: entree, centres: multi ? [...centres] : undefined });
                if (ch.length > 12) ch.shift();
                setCheminGraphe((ch ?? []).slice());
              }
            }
            // Le graphe a changé : l'index de fratrie de l'ancien ne vaut plus.
            indexNoeudRef.current = null;
            setFratrieNoeud(null);
            // ⚠ Ces lignes avaient été EMPORTÉES par un découpage trop large de
            // mon propre patch (elles vivaient entre `setTypesGraphe(` et
            // `void it;`). Conséquence mesurée : `gr.pret` n'était jamais posé,
            // donc l'étage graphe ne s'affichait PLUS — et le contrôle du
            // continuum l'a vu. Ne pas re-découper à l'aveugle ici.
            // DÉGRADATION VISIBLE. Mesuré : sur le monde « linux », l'ensemble
            // le plus gros s'appelle « drivers » (963 403 nœuds) — mais c'est un
            // PRÉFIXE de qn, pas un nœud : le sous-graphe rend 1 nœud et 0 arête.
            // Sans ce garde, l'étage graphe s'afficherait VIDE en laissant croire
            // qu'il n'y a rien à voir, alors que l'entrée est mal choisie.
            gr.degrade = g.nodes.length < 5;
            if (gr.degrade) {
              gr.motif = `entrée non exploitable (« ${centre} » est un préfixe, pas un nœud)`;
            }
            gr.noeuds = g.nodes.length;
            gr.aretes = (g.edges || []).length;
            // Ce que ce graphe a COÛTÉ en centres : c'est ce que le prochain
            // « ＋ densifier » devra dépasser pour valoir la peine.
            gr.centresUtilises = centres.length;
            // QUEL GROUPE CE GRAPHE MONTRE VRAIMENT. Sans cette trace, l'étage
            // graphe ne sait pas de quel groupe il est le contenu : il se
            // dessinait à l'échelle du monde, et l'enveloppe ne pouvait pas dire
            // « c'est le graphe DE CE GROUPE ». C'est l'argument réellement
            // utilisé par le chargeur (choix de l'utilisateur, sinon le plus gros
            // ensemble non-artefact) — la seule autorité.
            gr.ensemble = centre;
            gr.types = [...new Set((g.edges || []).map((e) => e.type))];
            gr.motif = compte4 > 0 ? `${compte} triangles · ${compte4} tétraèdres` : `${compte} triangles`;
            window.clearTimeout(delai);
            if (mode === "rapide") { gr.pret = true; gr.charge = false; } else { gr.pretComplet = true; gr.chargeComplet = false; }
            setGraphe({
              total: wd.nodes,
              qn: wd.qn, centre: entree, noeuds: gr.noeuds, aretes: gr.aretes,
              types: gr.types, motif: gr.motif, degrade: gr.degrade,
            });
            const comm = new Map<string, number>();
            g.nodes.forEach((nd) => {
              if (nd.community_id !== undefined && nd.community_id !== null) {
                comm.set(String(nd.qn || nd.id), Number(nd.community_id));
              }
            });
            setCommunautes(comm);
            const courb = new Map<string, number>();
            g.nodes.forEach((nd) => {
              if (typeof nd.curvature === "number") {
                courb.set(String(nd.qn || nd.id), nd.curvature);
              }
            });
            setCourbures(courb);
            setQnsGraphe(gr.noeudsInfo.map((x) => x.qn));
            // ── LA DESCENTE LE LONG DE L'AXE N'EST ACQUITTÉE QUE MAINTENANT ──
            // Le graphe est arrivé ET il est centré sur le nœud demandé : c'est
            // le seul instant où l'on peut dire « on est descendu d'un cran ».
            {
              const d = descentePendanteRef.current;
              if (d) {
                descentePendanteRef.current = null;
                const apres = `${entree}|${gr.noeuds}`;
                if (apres !== d.avant) {
                  cheminAxeRef.current = [...cheminAxeRef.current, d];
                  setCheminAxe(cheminAxeRef.current.slice(-6));
                } else {
                  /**
                   * ── LE SUBSTITUT DOIT ÊTRE DIT ──────────────────────────────
                   * MESURÉ : la descente demandait le nœud le plus important d'un
                   * groupe de l'axe, le moteur rendait MOINS DE 5 NŒUDS pour ce
                   * centre (son voisinage n'est pas exploitable), et `chargerGraphe`
                   * retombait alors sur un ancêtre résolu par recherche. Le graphe
                   * affiché n'était donc PAS celui qu'on avait demandé — et rien ne
                   * le disait : on croyait être descendu.
                   * Un plafond silencieux fait croire que la donnée n'existe pas ;
                   * c'est pire qu'une donnée manquante. On NOMME le substitut, et on
                   * n'ajoute rien au fil (aucune profondeur n'a été prise).
                   */
                  setVise({
                    genre: "noeud", nom: entree,
                    detail: `aucune profondeur prise : le graphe n'a pas changé (le nœud visé « ${d.noeud.split(/[.:]/).slice(-2).join(".")} » n'a pas de voisinage exploitable, ou c'est déjà celui qui est chargé) — affiché : « ${entree.split(/[.:]/).slice(-2).join(".")} »`,
                  });
                }
              }
            }
            void it;
          })
          .catch(() => { window.clearTimeout(delai); gr.charge = false; gr.chargeComplet = false; }),
      );
    };

    // ── GRAMMAIRE COMMUNE (voir ./grammar.ts) ────────────────────────────
    // Cette vue n'avait AUCUN contrôle clavier : elle s'aligne désormais sur
    // les autres (←/→ parcourir, ↑/⌂ tout voir, ↓ entrer, +/− densifier).
    const HOME_POS = new THREE.Vector3(0, 520, 1180);
    const anim = {
      on: false, k: 0,
      from: new THREE.Vector3(), to: new THREE.Vector3(),
      tFrom: new THREE.Vector3(), tTo: new THREE.Vector3(),
    };
    /**
     * ── LE CRAN ATTEINT, RETENU (voir la tenue dans la boucle) ───────────────
     * Un cran demandé se pose dans SA bande, puis le décor bouge et la distance
     * — celle qui MESURE le palier — en sort. On retient donc ce que le cran a
     * atteint, pour pouvoir le tenir au lieu de le perdre en silence.
     */
    const cranTenuRef = { current: null as null | { niv: number; qn: string; reprises: number } };
    /** Nombre de réajustements avant de conclure « ce cran ne tient pas ici ». */
    const CRAN_REPRISES_MAX = 3;
    const flyTo = (pos: THREE.Vector3, target: THREE.Vector3) => {
      anim.from.copy(camera.position); anim.to.copy(pos);
      anim.tFrom.copy(ctrl.target); anim.tTo.copy(target);
      anim.on = true; anim.k = 0;
    };

      // ── CADRER UNE POUPÉE — UNE SEULE IMPLÉMENTATION ───────────────────────
    // « On cadre le groupe de l'extérieur, le plus large possible sans
    // refermer l'étage. » On a essayé de cadrer LA BOULE DU MONDE (marge sur
    // le rayon du monde) : MESURÉ, `camDist` partait à 589 unités, `t` tombait
    // à 0,076, l'intérieur se refermait (`poupees` à zéro) et la boucle de
    // niveau renvoyait la caméra chez elle. La raison est de structure : ici,
    // un étage est défini par la PROXIMITÉ — on ne peut pas « reculer pour
    // tout voir » sans éteindre ce qu'on voulait montrer.
    // À 4,2 rayons de groupe, la nuée occupe ~1/4 du champ, ses nœuds ~1/4, et
    // la PAROI DU MONDE l'entoure : le groupe est vu DANS son monde, sans
    // perdre l'étage. Sert au clic sur la scène ET à l'encart « poupées ».
    const cadrerGroupe = (qnMonde: string, nom: string) => {
      const wn = worldNodes.get(qnMonde);
      const boule = interieurs.get(qnMonde)?.boules.find((x) => x.name === nom);
      if (!wn || !boule) return;
      const geo = boule.pts.geometry;
      if (!geo.boundingSphere) geo.computeBoundingSphere();
      const rayon = geo.boundingSphere?.radius ?? 4;
      const P = boule.pts.localToWorld(geo.boundingSphere?.center.clone() ?? new THREE.Vector3());
      const fov = (camera.fov * Math.PI) / 180;
      const d = (rayon * CADRAGE_POUPEE) / Math.max(0.05, Math.sin(fov / 2));
      cibleUtilisateurRef.current = { pos: P.clone(), nom };
      flyTo(P.clone().add(new THREE.Vector3(0, d * 0.24, d * 0.97)), P);
    };
    cadrerGroupeRef.current = cadrerGroupe;
    /**
     * ── ENTRER DANS UN GROUPE DE L'AXE (cadrer + mettre en valeur) ────────────
     *
     * Le groupe d'un axe n'a pas de `boule` : il n'est pas un ensemble de
     * l'organisation, c'est une CLASSE des nœuds du graphe (une communauté, un
     * bord ∂, une extension…). On le cadre donc sur les positions RÉELLES de ses
     * nœuds — la disposition du graphe est conservée (`posGraphe`), ce qui est
     * exactement ce que `cadrerGroupe` ne pouvait pas faire faute de points.
     *
     * Le rayon vient de la sphère englobante du groupe, et la distance de la
     * MÊME formule que partout ailleurs (d = R / sin(θ/2), marge comprise) : un
     * groupe petit et un groupe grand se lisent donc à la même taille apparente.
     * Aucune constante nouvelle.
     */
    const entrerAxe = (qnMonde: string, type: string) => {
      const g = graphes.get(qnMonde);
      const infos = g?.noeudsInfo;
      if (!g?.pret || !infos?.length || !g.posGraphe) return false;
      const box = new THREE.Box3();
      const v = new THREE.Vector3();
      let n = 0;
      infos.forEach((x, i) => {
        if (etiquetteDimension(x.qn, dimensionRef.current) !== type) return;
        v.set(g.posGraphe![i * 3], g.posGraphe![i * 3 + 1], g.posGraphe![i * 3 + 2]);
        box.expandByPoint(v);
        n++;
      });
      if (!n) return false;
      const centre = box.getCenter(new THREE.Vector3());
      const rayon = Math.max(1e-3, box.getSize(new THREE.Vector3()).length() / 2);
      const P = g.grp.localToWorld(centre.clone());
      const fov = (camera.fov * Math.PI) / 180;
      const d = (rayon * CADRAGE_POUPEE) / Math.max(0.05, Math.sin(fov / 2));
      cibleUtilisateurRef.current = { pos: P.clone(), nom: type };
      flyTo(P.clone().add(new THREE.Vector3(0, d * 0.24, d * 0.97)), P);
      // La mise en valeur passe par le mécanisme d'EMPHASE qui existe déjà — on
      // n'en ajoute pas un second.
      setFiltreGraphe(type);
      setVise({
        genre: "ensemble", nom: type,
        detail: `groupe de l'axe « ${dimensionRef.current} » — ${n} nœuds du graphe`,
      });
      return true;
    };
    entrerAxeRef.current = entrerAxe;
    /**
     * ENTRER / REMONTER — le chemin de la hiérarchie, et rien d'autre.
     *
     * On écrit le PRÉFIXE et le RANG (la loi de `fractale.ts`), on vide l'étage,
     * et on le reconstruit. Le serveur re-découpe alors le niveau suivant : les
     * sous-groupes RÉELS du groupe qu'on vient d'ouvrir, sur tout le store.
     * Aucune donnée n'est inventée : c'est le même agrégat que `StoreOverview`
     * utilise, appliqué au même endroit.
     */
    /**
     * ── LES NŒUDS D'UN CHEMIN — résoudre un groupe en ses nœuds ──────────────
     *
     * Un groupe de la hiérarchie est un CHEMIN de tokens (`deepseek_harness.
     * packages`), pas un nœud : le moteur ne peut pas en rendre le voisinage
     * directement. Mais le graphe CHARGÉ contient des qn réels, et chacun porte ce
     * chemin dans son propre nom. On sélectionne ceux qui le contiennent, dans
     * l'ordre d'importance du graphe (l'ordre du serveur), et on les donne comme
     * CENTRES : le moteur expanse alors leurs voisinages et rend le graphe RÉEL du
     * sous-groupe. Borné : au-delà de `maxCentres`, l'expansion coûterait cher sans
     * rien ajouter — elle atteindrait les mêmes voisins.
     */
    const noeudsDuPrefixe = (qnMonde: string, prefixe: string, maxCentres = 32): string[] => {
      const tokens = prefixe.split(".").filter(Boolean);
      const infos = graphes.get(qnMonde)?.noeudsInfo;
      if (!tokens.length || !infos?.length) return [];
      const out: string[] = [];
      for (const x of infos) {
        const parts = String(x.qn).split(/[.:]/).filter(Boolean);
        let k = 0;
        for (const seg of parts) {
          if (seg === tokens[k]) { k++; if (k === tokens.length) break; }
        }
        if (k === tokens.length) out.push(x.qn);
        if (out.length >= maxCentres) break;
      }
      return out;
    };
    const rechargerInterieur = (qnMonde: string) => {
      const iv = interieurs.get(qnMonde);
      const wd = worlds.find((w) => w.qn === qnMonde);
      if (!iv || !wd) return;
      /**
       * ── LE GRAPHE SUIT LE NIVEAU (relevé AVANT de vider l'étage) ──────────
       * Les qn du niveau visé se lisent dans le graphe COURANT : il faut donc les
       * relever avant tout rechargement, sinon la source aurait déjà disparu.
       */
      const centres = (iv.prefixe || iv.classes.length)
        // Le budget du niveau, et non un 32 en dur : c'est LUI que « ＋ densifier »
        // fait grandir quand le niveau est découpé sur un échantillon du graphe
        // (`densite.ts`, règle 3 bis). Un nombre fixe ici rendait le geste
        // impossible à tenir — il aurait annoncé une densification sans levier.
        ? noeudsCourants(qnMonde, iv.prefixe, iv.classes, centresDuNiveau(qnMonde)) : [];
      iv.grp.clear();
      iv.boules = [];
      iv.enveloppe = undefined;
      iv.charge = false;
      iv.pret = false;
      // ── LA PAGE REVIENT À LA PREMIÈRE : LE NIVEAU A CHANGÉ ─────────────────
      // Densifier et approfondir ne se mélangent pas. Sans cette remise, entrer
      // dans un sous-groupe héritait de la page du niveau précédent : on
      // dessinait 96 groupes d'un niveau qui n'en demandait pas tant, et la
      // « première » page d'un niveau n'existait plus jamais.
      iv.page = FRACTALE.pasPage;
      // Le niveau montré change : la cible et le groupe désigné qui s'y
      // rapportaient n'ont plus de sens.
      ensembleChoisi.current = null;
      setVise(null);
      setFiltreGraphe(null);
      void chargerInterieur(wd);
      // ── ET LE GRAPHE SUIT ────────────────────────────────────────────────
      // Sans cela, entrer dans `packages` changeait l'intérieur et laissait le
      // graphe du niveau précédent : la navigation ne suivait qu'à moitié.
      const libelle = [iv.prefixe, ...iv.classes.map((c) => `${c.axe}=${c.valeur}`)]
        .filter(Boolean).join(" ∩ ");
      if (libelle) {
        if (centres.length) chargerGraphe(wd, libelle, "rapide", centres);
        else setVise({
          genre: "ensemble", nom: iv.prefixe,
          detail: `aucun nœud de « ${libelle} » dans le graphe chargé : le graphe reste au niveau précédent (les sous-groupes, eux, sont bien ceux du serveur)`,
        });
      } else if (iv.entreeRacine) {
        // Retour aux racines : le graphe reprend son entrée de monde.
        chargerGraphe(wd, iv.entreeRacine, "rapide");
      }
    };
    /**
     * ENTRER — dans un TOKEN (l'arbre) ou dans une CLASSE (l'axe).
     *
     * Les deux coordonnées descendent, et c'est ce qui rend l'organisation
     * disponible à tous les étages : au niveau `deepseek_harness.packages`, on peut
     * descendre d'un token (« client ») COMME d'une classe (« bord ∂ »), et les
     * deux se composent — chaque pas resserre le même ensemble de nœuds.
     */
    entrerGroupeRef.current = (qnMonde, nom, source = "token") => {
      const iv = interieurs.get(qnMonde);
      if (!iv) return;
      if (source === "classe") {
        iv.classes = [...iv.classes, { axe: dimensionRef.current, valeur: nom }];
      } else {
        iv.prefixe = iv.prefixe ? `${iv.prefixe}.${nom}` : nom;
        iv.rang = iv.rang + 1;
      }
      rechargerInterieur(qnMonde);
    };
    remonterGroupeRef.current = (qnMonde) => {
      const iv = interieurs.get(qnMonde);
      if (!iv) return;
      // On défait le DERNIER pas fait, quelle que soit sa coordonnée.
      if (iv.classes.length) {
        iv.classes = iv.classes.slice(0, -1);
      } else if (iv.prefixe) {
        const parts = iv.prefixe.split(".");
        parts.pop();
        iv.prefixe = parts.join(".");
        iv.rang = Math.max(1, iv.rang - 1);
      } else return;
      rechargerInterieur(qnMonde);
    };
    /**
     * ALLER À UN NIVEAU DU CHEMIN — le clic sur un maillon du fil.
     *
     * Remonter d'un groupe à la fois est juste, mais lent quand on est descendu
     * de six niveaux : on veut pouvoir revenir DIRECTEMENT à un ancêtre. Le rang
     * se déduit du nombre de tokens (`rangRacine + profondeur`), donc le chemin
     * porte à lui seul la position — aucune pile à tenir en parallèle.
     */
    allerClassesRef.current = (qnMonde, n) => {
      const iv = interieurs.get(qnMonde);
      if (!iv) return;
      iv.classes = iv.classes.slice(0, Math.max(0, n));
      rechargerInterieur(qnMonde);
    };
    allerPrefixeRef.current = (qnMonde, prefixe) => {
      const iv = interieurs.get(qnMonde);
      if (!iv) return;
      iv.prefixe = prefixe;
      iv.rang = Math.max(1, iv.rang - (iv.prefixe.split(".").length - (prefixe ? prefixe.split(".").length : 0)));
      rechargerInterieur(qnMonde);
    };
    /**
     * ENTRER DANS LE PLUS GROS SOUS-GROUPE — le geste `Page ↓`.
     *
     * Au clavier, on ne désigne pas une puce du regard : on descend. Le plus gros
     * sous-groupe est celui qui porte le plus de matière, donc celui par lequel
     * une descente a le plus de chances d'être utile — et c'est déjà le critère
     * du tri du serveur (`enfantsDe` trie par effectif décroissant).
     */
    /**
     * REVENIR À UNE ENTRÉE DU CHEMIN DE GRAPHE — reprise EXACTE.
     *
     * On réutilise les `centres` enregistrés à l'époque : pour un sous-groupe,
     * c'est la liste de qn qui l'a produit ; pour un nœud, son qn. Recharger par
     * une simple recherche donnerait « un » voisinage, pas CELUI qu'on avait — et
     * on perdrait la position au lieu de la retrouver.
     */
    revenirEntreeRef.current = (qnMonde, i) => {
      const wd = worlds.find((w) => w.qn === qnMonde);
      const ch = graphes.get(qnMonde)?.entrees ?? [];
      const e = ch[i];
      if (!wd || !e) return;
      // On tronque le chemin à l'entrée choisie : ce qui venait APRÈS n'est plus
      // le chemin courant, et le garder ferait croire qu'on peut y revenir.
      ch.length = i + 1;
      setCheminGraphe(ch.slice());
      chargerGraphe(wd, e.nom, "rapide", e.centres ?? []);
    };
    entrerPlusGrosRef.current = (qnMonde) => {
      const iv = interieurs.get(qnMonde);
      const gros = iv?.boules?.[0];
      if (!iv || !gros || iv.feuille) return false;
      entrerGroupeRef.current?.(qnMonde, gros.name);
      return true;
    };
    /**
     * ── DENSIFIER — LA PAGE SUIVANTE DU MÊME NIVEAU ────────────────────────
     *
     * Ce geste ne touche NI `prefixe` NI `rang` NI `classes` : c'est ce qui le
     * distingue d'« approfondir », et c'est mesurable (`__etages.organisation`
     * publie les deux, et le contrôle d'acceptation exige que densifier ne
     * change QUE le nombre dessiné).
     *
     * Ce qu'il ne fait PAS, et c'est délibéré :
     *   · il ne recharge PAS le graphe : le niveau n'a pas changé, donc le
     *     graphe du niveau est toujours le bon — le recharger paierait un appel
     *     lourd (mesuré : hops=2 sur un gros store, 12,8 s) pour rien, et
     *     remettrait le chemin d'entrées à zéro ;
     *   · il ne vide PAS la cible (`vise`) : on densifie ce qu'on regarde.
     */
    densifierNiveauRef.current = (qnMonde) => {
      const iv = interieurs.get(qnMonde);
      const wd = worlds.find((w) => w.qn === qnMonde);
      if (!iv || !wd) return false;
      const total = iv.tous.length;
      const source: "graphe" | "serveur" = iv.sourceAxe ? "graphe" : "serveur";
      const d = decisionDensite({
        total, montres: Math.min(iv.page, total),
        feuille: iv.feuille, plafondAtteint: iv.plafondAtteint, intention: "plus",
        source,
        couverture: source === "graphe" ? couvertureGraphe(qnMonde) : undefined,
      });
      if (d.geste !== "densifier") return false;
      // ── DENSIFIER UN ÉCHANTILLON : PLUS DE CENTRES, MÊME NIVEAU (règle 3 bis) ─
      // `dePlus === undefined` est la marque de CE cas : la loi a dit « densifier »
      // sans pouvoir promettre un nombre, parce que la suite est un échantillon
      // plus large. Le geste double donc le budget de centres du niveau et
      // recharge le niveau — préfixe, rang et classes INCHANGÉS (approfondir, lui,
      // appellerait `entrerGroupeRef`). Mesuré : un niveau descendu est TOUJOURS
      // dans ce cas (350 nœuds lus sur 52 979 réels), donc sans ce chemin le geste
      // « densifier » n'aurait rien fait là où l'on navigue.
      if (d.requiertAppel && d.dePlus === undefined) {
        if (iv.centresGraphe >= PAGE.centresMax) {
          setVise({
            genre: "ensemble", nom: iv.prefixe || "racines",
            detail: `l'échantillon est au plafond (${PAGE.centresMax} centres) : densifier ne peut plus l'élargir — `
              + "descendez d'un cran pour voir plus fin",
          });
          return false;
        }
        iv.centresGraphe = Math.min(PAGE.centresMax, iv.centresGraphe * 2);
        iv.page = FRACTALE.pasPage;   // le niveau change de matière : on repart d'une page lisible
        rechargerInterieur(qnMonde);
        return true;
      }
      // ── LA PAGE SUIVANTE EST-ELLE EN MÉMOIRE, OU CHEZ LE SERVEUR ? ────────
      // En mémoire : un redessin suffit (aucun appel). Chez le serveur (page
      // pleine) : on paie la requête, et on AJOUTE la page reçue à la matière du
      // niveau — le niveau ne change pas, seul ce qu'on en connaît grandit.
      if (d.requiertAppel) {
        // ── LE SKIP COMPTE DES LIGNES BRUTES, PAS DES GROUPES RETENUS ────────
        // `iv.tous.length` est FILTRÉ (tokens valides, effectifs > 1) : paginer
        // avec lui ferait recouvrir chaque page de ce que nos filtres ont écarté.
        // Mesuré sur `runtime-dev` (`deepseek_harness`, rang 4) : 200 lignes
        // brutes rendues, un nom vide de 8 438 nœuds écarté → 50 lignes de
        // recouvrement, et une progression deux fois plus lente que nécessaire.
        const offset = iv.offsetBrut;
        void pageServeur(wd.path, iv.prefixe, iv.rang, offset)
          .then((p) => {
            // ── UNE PAGE SANS LIGNE ET UNE PAGE SANS GROUPE SONT DEUX CHOSES ───
            // MESURÉ (instance isolée, `deepseek_harness`, rang 4) : `offset=200`
            // rend 200 lignes dont l'effectif MAXIMAL est **1** — donc ZÉRO groupe
            // après le filtre `count > 1`. Le premier jet sortait sur
            // `!p.tous.length` (« plus rien au-delà »), ce qui confondait « rien
            // d'utilisable » avec « rien du tout » : le geste réémettait la MÊME
            // page indéfiniment (cinq appels identiques mesurés, zéro état changé),
            // et l'offre ne s'éteignait jamais. Deux cas, donc, et deux issues :
            //   · `p.bruts === 0` : le serveur n'a plus de ligne → c'est FINI ;
            //   · `p.bruts > 0` mais rien de gardé : la page n'apprend rien, et
            //     c'est la branche ci-dessous qui compte les pages vides.
            if (!p.bruts) {
              iv.plafondAtteint = false;               // le serveur a rendu une page VIDE
              publierOrganisation({ offsetBrut: iv.offsetBrut, pagesVides: iv.pagesVides ?? 0 });
              return;
            }
            const connus = new Set(iv.tous.map((g) => g.name));
            // `GroupeFractal` nomme `nom`, la scène nomme `name` : on traduit à
            // la frontière, une fois, au lieu de laisser deux formes circuler.
            const arrives = p.tous
              .map((g) => ({ name: g.nom, count: g.count }))
              .filter((g) => !connus.has(g.name));
            // Rien de NOUVEAU : le serveur a rendu des lignes, mais toutes déjà
            // connues (recouvrement) ou écartées par les filtres. On avance le
            // compte brut quand même, pour que le geste suivant progresse au lieu
            // de redemander la même page ; et si le serveur ne coupe plus, on
            // éteint le plafond — c'est LUI qui décide, jamais nous.
            if (!arrives.length) {
              iv.offsetBrut = offset + p.bruts;
              iv.pagesVides = (iv.pagesVides ?? 0) + 1;
              // Le curseur est REPUBLIÉ à chaque page : c'est lui qui dit si le
              // geste avance, et un état qui ne se publie qu'au chargement ne
              // permettrait pas de le mesurer pendant la pagination.
              publierOrganisation({ offsetBrut: iv.offsetBrut, pagesVides: iv.pagesVides });
              // ── UNE PAGE QUI N'APPREND RIEN NE S'OFFRE PLUS ────────────────
              // MESURÉ : l'agrégat est trié par effectif DÉCROISSANT, donc les
              // pages 2..N sont le PETIT RESTE — des tokens d'effectif 1, que
              // `count > 1` écarte par construction. Cinq gestes d'affilée ont
              // ainsi émis cinq `/api/viz/space` à `offset` croissant sans ajouter
              // UN SEUL groupe : un geste qui ne rapporte rien et reste offert est
              // un geste décoratif. Au-delà d'une page vide on continue (la
              // suivante peut porter des effectifs 2), mais après DEUX, le niveau
              // est épuisé POUR L'AFFICHAGE et on le dit — le serveur, lui, peut
              // encore avoir des lignes ; elles ne se voient pas ici.
              if (iv.pagesVides >= 2) {
                iv.plafondAtteint = false;
                // ── L'OFFRE SUIT L'ÉTAT, PAS LE CONTRAIRE ───────────────────
                // MESURÉ : l'état interne était bien éteint (plus aucun appel émis
                // aux gestes suivants), mais `__etages.organisation` publiait
                // encore `plafondAtteint: true` — la puce restait donc OFFERTE et
                // ne faisait plus rien : le geste décoratif, par la porte de
                // l'affichage. On republie donc le verdict en même temps.
                publierOrganisation({
                  plafondAtteint: false, geste: "approfondir", densifiable: false,
                  offsetBrut: iv.offsetBrut, pagesVides: iv.pagesVides,
                  raison: "les pages suivantes du niveau n'apportent aucun groupe d'effectif > 1",
                });
                setVise({
                  genre: "ensemble", nom: iv.prefixe || "racines",
                  detail: `${iv.pagesVides} pages suivantes (${iv.offsetBrut} lignes) sans un groupe d'effectif > 1 : `
                    + "ce niveau est épuisé pour l'affichage — descendez d'un cran pour voir plus fin",
                });
                return;
              }
              iv.plafondAtteint = p.plafondAtteint;
              if (!iv.plafondAtteint) return;
              setVise({
                genre: "ensemble", nom: iv.prefixe || "racines",
                detail: `la page ${iv.offsetBrut} lignes n'apporte aucun groupe nouveau `
                  + "(l'agrégat est trié par effectif : les pages suivantes sont le petit reste)",
              });
              return;
            }
            iv.pagesVides = 0;
            iv.offsetBrut = offset + p.bruts;
            iv.tous = [...iv.tous, ...arrives];
            iv.plafondAtteint = p.plafondAtteint;
            publierOrganisation({
              tous: iv.tous.length, offsetBrut: iv.offsetBrut, pagesVides: 0,
              plafondAtteint: iv.plafondAtteint,
            });
            const nouvelle = pageSuivante(iv.page, iv.tous.length);
            if (nouvelle <= iv.page) return;
            iv.page = nouvelle;
            iv.grp.clear();
            iv.boules = [];
            iv.enveloppe = undefined;
            iv.charge = false;
            iv.pret = false;
            void chargerInterieur(wd, true);
          })
          .catch(() => { /* le geste échoué ne laisse pas l'étage vide */ });
        return true;
      }
      // La page suivante SEULEMENT : `pageSuivante` borne au total reçu, donc on
      // ne dessine jamais un groupe qu'on n'a pas.
      const nouvelle = pageSuivante(iv.page, total);
      if (nouvelle <= iv.page) return false;
      iv.page = nouvelle;
      // On redessine l'étage — `redessin = true`, donc AUCUNE requête et la même
      // source de découpage. Le graphe, la cible et le chemin restent.
      iv.grp.clear();
      iv.boules = [];
      iv.enveloppe = undefined;
      iv.charge = false;
      iv.pret = false;
      void chargerInterieur(wd, true);
      return true;
    };
    /**
     * ── RECENTRER LE GRAPHE SUR UN NŒUD, DEPUIS LE PANNEAU ─────────────────
     *
     * Le clic sur un nœud de la scène fait exactement ceci
     * (`setVise({genre:"noeud"})` + `chargerGraphe(wd, qn, "rapide")`). La
     * fratrie affichée dans le panneau doit pouvoir en faire autant : sinon les
     * frères seraient nommés mais inertes — nommer sans pouvoir y aller ne
     * répond qu'à la moitié de la demande.
     *
     * On expose donc la voie qui marche (même patron que `cadrerGroupeRef`),
     * au lieu d'en réécrire une seconde dans le rendu.
     */
    chargerNoeudRef.current = (qnMonde: string, qnNoeud: string) => {
      const wd = worlds.find((w) => w.qn === qnMonde);
      if (!wd) return;
      setVise({ genre: "noeud", nom: qnNoeud, detail: "frère choisi — nouveau centre du graphe" });
      chargerGraphe(wd, qnNoeud, "rapide");
    };
    let focusW = -1;
    /** Dernière cible choisie par la caméra (pour ne la poser qu'une fois). */
    let dernierAutoRef: { current: string | null } = { current: null };
    /**
     * Cible de l'image PRÉCÉDENTE (clavier ou caméra).
     *
     * Le calcul des positions tourne AVANT celui de la cible : sans cette
     * mémoire, on ne pouvait geler l'orbite que du monde visé au clavier, et le
     * monde choisi par la caméra continuait d'orbiter pendant qu'on s'en
     * approchait — il glissait hors de portée et la descente RECULAIT (mesuré :
     * 46 % → 25 %). Une image de retard est sans conséquence ; l'absence de gel,
     * non.
     */
    let ciblePrecedente: string | null = null;
    /** Angle figé par monde : on n'explore pas une cible qui fuit. */
    const angleGele = new Map<string, number>();
    /**
     * ── LA RÉFÉRENCE DE CADRAGE, CONTINUE À TRAVERS LES ÉCHELLES ───────────
     *
     * « T'assurer de la transition de target au niveau échelle. » Le défaut
     * était structurel : la caméra changeait de CORPS de référence sans que rien
     * ne transitionne. Passer du monde (rayon 30) à un groupe (rayon 8,7) faisait
     * sauter d'un coup la butée de rapprochement (2,4 → 2,2 pour la valeur, mais
     * surtout le RAYON qui la fonde changeait de 3,5×) et le centre d'orbite
     * d'une dizaine d'unités : sous les doigts, la molette changeait de loi en
     * plein geste, et l'œil perdait l'objet qu'il suivait.
     *
     * Cette référence est donc POURSUIVIE, jamais posée : le rayon est
     * interpolé en LOGARITHME (chaque doublement d'échelle prend la même durée,
     * comme la descente elle-même), la position en linéaire. `frac` porte la
     * part du rayon qui sert de butée (8 % pour un monde, 25 % pour un groupe) et
     * transitionne avec le reste, pour que la loi de zoom ne saute pas non plus.
     */
    const cadreRef = {
      init: false,
      pos: new THREE.Vector3(),
      rayon: 1,
      frac: 0.08,
      genre: "cosmos" as "cosmos" | "monde" | "groupe" | "noeud",
      nom: "" as string,
    };
    const qnList = worlds.map((w) => w.qn);
    const gotoWorld = (i: number) => {
      const n = qnList.length;
      if (!n) return;
      focusW = ((i % n) + n) % n;
      const qn = qnList[focusW];
      const node = worldNodes.get(qn);
      const o = layout.orbits.get(qn);
      if (!node || !o) return;
      // Distance de VISÉE : environ 8 rayons — on voit le monde en entier.
      // L'ancienne cible (3,4 rayons) collait la caméra à la surface : la
      // descente n'avait alors aucun trajet à parcourir et sautait à 100 %
      // d'un coup (constaté à la mesure), au lieu d'être continue.
      const P = node.mesh.position.clone();
      cibleUtilisateurRef.current = { pos: P.clone(), nom: qn };
      flyTo(P.clone().add(new THREE.Vector3(0, o.size * 3.2, o.size * 8.2)), P);
      setFocusQn(qn);
      // Poser le SUJET de la pièce : les autres faces le liront.
      const wd = worlds.find((w) => w.qn === qn);
      setSubject({
        store: wd?.path || "",
        qn,
        label: (wd?.label || qn).replace(/ \(.*\)/, ""),
        via: "cosmos",
      });
    };
    const overview = () => {
      focusW = -1;
      setFocusQn(null);
      cibleUtilisateurRef.current = null;
      // Le chemin le long de l'axe APPARTIENT à l'échelle où il a été parcouru :
      // revenir à l'espace l'efface, sinon il annoncerait une profondeur qui
      // n'est plus celle qu'on regarde.
      cheminAxeRef.current = [];
      setCheminAxe([]);
      flyTo(HOME_POS, new THREE.Vector3(0, 0, 0));
    };
    retourRef.current = overview; // le retour devient accessible depuis l'interface
    /**
     * PLONGER : descendre PLUSIEURS niveaux d'un seul geste.
     *
     * Descendre niveau par niveau est utile pour comprendre la structure, mais
     * pénible quand on sait déjà où l'on va. On place la caméra directement à
     * la distance du dernier étage : les étages intermédiaires se chargent alors
     * en chemin (leur chargement est déclenché par la DESCENTE, pas par un clic
     * sur chacun), ce qui évite un état vide.
     */
    const plonger = () => {
      if (focusW < 0 && qnList.length) gotoWorld(0);
      const qn = focusW >= 0 ? qnList[focusW] : null;
      if (!qn) return;
      const node = worldNodes.get(qn);
      const o = layout.orbits.get(qn);
      if (!node || !o) return;
      /**
       * ── L'AXE DE NAVIGATION : LE MÊME GESTE, UNE AUTRE RELATION ────────────
       *
       * « Descendre » ne veut pas forcément dire « suivre la descendance ». Le
       * dépôt connaît déjà plusieurs découpages RÉELS de la même matière —
       * nom (module · parent · extension), COMMUNAUTÉ du moteur (`community_id`,
       * Leiden), courbure (bords ∂ / intérieur), strate. Ils servaient à colorer
       * et à filtrer ; ils deviennent un AXE DE NAVIGATION.
       *
       * Concrètement, au dernier cran, le geste descend dans le groupe le plus
       * important DE L'AXE CHOISI, en demandant au moteur le voisinage de son
       * nœud le plus important (`chargerNoeudRef`, la même voie que la fratrie).
       * Le nouveau niveau se re-découpe avec LE MÊME AXE : la polymorphie est
       * donc récursive, et un même geste explore six structures différentes.
       *
       * Le sous-graphe est ordonné par importance (`order: importance:degree`,
       * vérifié dans les trames du serveur) : le premier nœud d'un groupe est son
       * nœud le plus important. Aucun classement n'est inventé ici.
       */
      /** Le plus gros groupe de l'axe — une seule autorité de découpage. */
      const groupeAxeCourant = (): { type: string; qns: string[] } | null => {
        const iv0 = interieurs.get(qn);
        // L'axe s'applique AU NIVEAU COURANT (chemin ∩ classes déjà entrées).
        const parType = groupesAxe(qn, noeudsCourants(qn, iv0?.prefixe ?? "", iv0?.classes ?? []));
        if (!parType) return null;
        let meilleur: { type: string; qns: string[] } | null = null;
        for (const [type, qns] of parType) {
          if (!meilleur || qns.length > meilleur.qns.length) meilleur = { type, qns };
        }
        return meilleur;
      };
      /**
       * ── UN GESTE NOMMÉ « DESCENDRE » NE RECULE JAMAIS, ET IL MÈNE AU BOUT ──
       *
       * MESURÉ sur la page servie, à l'étage le plus profond : clic
       * « ⤓ descendre » → `dedans — 85 %` devient `monde — 63 %`. Le bouton
       * RECULAIT. La cause : une destination calculée par une formule (ici
       * `CADRAGE_POUPEE = 4,2` sur le rayon du groupe) peut tomber PLUS LOIN que
       * là où l'on est déjà. Un cran qui recule n'est pas un cran ; on borne donc
       * le geste par l'endroit où l'on est.
       *
       * Le geste lit le PALIER courant (`palierDe`, l'autorité unique d'échelle)
       * et va au suivant :
       *   espace → monde : on entre ;
       *   monde  → le GROUPE désigné s'il est plus profond, sinon le cran groupe ;
       *   groupe → SIGNAUX  : le fond de l'échelle — la matière se réorganise
       *             selon courbure · énergie · degré (le curseur `tS`).
       * Au dernier palier, il ne bouge pas — et surtout pas en arrière.
       */
      const R = o.size || 30;
      const dNow = camera.position.distanceTo(node.mesh.position);
      const loin = R * 14, presT = R * 0.25;
      const tNow = Math.max(0, Math.min(1,
        Math.log(loin / Math.max(1e-3, dNow)) / Math.log(loin / presT)));
      /** Une destination (position, distance) est-elle plus profonde que `t` ? */
      const plusProfonde = (P: THREE.Vector3, d: number, t: number) => {
        const dd = Math.max(1e-3, P.distanceTo(node.mesh.position) + d);
        return Math.log(loin / dd) / Math.log(loin / presT) > t;
      };
      const palierIci = palierDe(dNow, R);
      /** Voler au palier `p`, centré sur le monde. */
      const volerPalier = (p: 1 | 2 | 3) => {
        const P = node.mesh.position.clone();
        const d = distancePourPalier(p, R);
        cibleUtilisateurRef.current = { pos: P.clone(), nom: qn };
        flyTo(P.clone().add(new THREE.Vector3(0, d * 0.22, d * 0.97)), P);
      };
      // ── 1. ON N'EST PAS ENCORE DANS LE MONDE : ON Y ENTRE ────────────────
      if (palierIci === 0) { volerPalier(1); return; }
      // ── 2. LE GROUPE DÉSIGNÉ, S'IL EST PLUS PROFOND QUE LÀ OÙ L'ON EST ────
      // `localToWorld` tient compte de TOUTES les transformations (le monde
      // grossit pendant la descente) — seule façon d'être sûr de viser le groupe
      // et pas son fantôme. Le rayon est celui du groupe RÉELLEMENT dessiné.
      const choix = ensembleChoisi.current;
      const it = interieurs.get(qn);
      const boule = choix && choix.qn === qn && it
        ? it.boules.find((b) => b.name === choix.nom)
        : undefined;
      if (boule) {
        const P = boule.pts.localToWorld(boule.centre.clone());
        const rayon = Math.max(
          1e-3,
          boule.rayon * (node.mesh.scale.x || 1) * (it!.grp.scale.x || 1),
        );
        const pres = Math.max(0.4, rayon * 0.8);
        if (plusProfonde(P, pres, tNow)) {
          cibleUtilisateurRef.current = { pos: P.clone(), nom: choix!.nom };
          flyTo(P.clone().add(new THREE.Vector3(0, pres * 0.7, pres * 1.4)), P);
          return;
        }
      }
      // ── 3. LE PALIER SUIVANT, ET SEULEMENT S'IL DESCEND ──────────────────
      if (palierIci < 3) { volerPalier((palierIci + 1) as 2 | 3); return; }
      // ── 4. AU DERNIER CRAN : DESCENDRE LE LONG DE L'AXE ──────────────────
      // Le cran « signaux » est le fond de l'ÉCHELLE, pas le fond du MONDE : on
      // peut encore entrer dans le groupe que l'axe désigne, en demandant au
      // moteur le voisinage de son nœud le plus important. C'est un cran de
      // profondeur de plus, et il est RÉCURSIF — le nouveau graphe se découpera
      // avec le même axe.
      const axe = groupeAxeCourant();
      // ── ON N'ENTRE PAS DEUX FOIS DANS LE MÊME NŒUD ────────────────────────
      // MESURÉ : sans ce garde, le geste était un POINT FIXE — le premier nœud
      // d'un groupe étant son nœud le plus important, et le graphe étant déjà
      // centré sur lui, « descendre » le ré-élisait à chaque fois. Le chemin
      // s'allongeait donc (`chemin` 1 puis 2, deux poupées 🎯 identiques) pendant
      // que le graphe ne changeait PAS (mêmes 20 000 nœuds, même entrée). Une
      // profondeur qui s'annonce sans se produire est pire qu'une absence de
      // profondeur : elle fait croire qu'on avance.
      const centreActuel = noeudsGrapheRef.current?.centre ?? null;
      const cand = axe ? axe.qns.filter((x) => x !== centreActuel) : [];
      if (axe && cand.length) {
        const visé = cand[0];                 // ordonné par importance (serveur)
        setVise({
          genre: "noeud", nom: visé,
          detail: `groupe de l'axe « ${dimensionRef.current} » — ${axe.type} (${axe.qns.length} nœuds), entré par le plus important`,
        });
        chargerNoeudRef.current?.(qn, visé);
        /**
         * ── LA DESCENTE LE LONG DE L'AXE NE DÉPLACE PAS LA CAMÉRA ────────────
         *
         * Elle a d'abord cadré le nœud visé. MESURÉ, et rejeté : ce nœud peut être
         * LOIN du centre du monde, et l'échelle — qui se mesure en distance au
         * centre du monde — retombait alors de 100 % à 0 %. Le panneau annonçait
         * « espace » à l'instant où l'on venait d'entrer dans un nœud, et les
         * boutons de cran devenaient inertes (le cran restait marqué appliqué).
         *
         * La profondeur n'a pas besoin de ce vol pour être réelle : le GRAPHE
         * change (mesuré : 20 000 → 5 nœuds, entrée différente), le fil des poupées
         * l'enregistre, et l'emphase allume le groupe. La caméra, elle, reste au
         * cran où l'utilisateur l'a mise — un geste de profondeur ne doit pas
         * déplacer l'échelle.
         */
        void R;
        // L'AXE DEVIENT VISIBLE : le groupe qu'on vient d'entrer est mis en
        // valeur par le mécanisme d'emphase qui existe déjà (aucun second).
        setFiltreGraphe(axe.type);
        // ── LE CHEMIN S'ALLONGERA SI LE GRAPHE ARRIVE VRAIMENT ─────────────
        // On POSE la descente pendante ; c'est l'arrivée d'un graphe centré sur
        // ce nœud qui l'ajoutera au fil (voir `chargerGraphe`). Un geste refusé
        // ou sans effet ne laisse donc AUCUNE trace : le fil ne ment pas.
        descentePendanteRef.current = {
          axe: dimensionRef.current, groupe: axe.type, noeud: visé,
          // L'IDENTITÉ du graphe AVANT le geste : c'est elle qui dira si quelque
          // chose a réellement changé (l'entrée seule ne suffit pas — mesuré :
          // même entrée, 20 000 nœuds avant et 1 805 après).
          avant: `${noeudsGrapheRef.current?.centre ?? ""}|${graphes.get(qn)?.noeuds ?? -1}`,
        };
      }
      // Déjà au cran le plus bas ET aucun groupe d'axe à entrer : on ne bouge pas
      // (et surtout pas en arrière).
    };
    zoomProfondRef.current = plonger;
    /**
     * Vise le monde d'un foyer. Correspondance TOLÉRANTE (`qn`, `label` ou
     * `path`), parce que le champ `world` d'un foyer peut porter l'un ou
     * l'autre selon la source. Si rien ne correspond, on NE BOUGE PAS : mieux
     * vaut un clic sans effet qu'un saut vers un monde arbitraire.
     */
    const viserFocus = (f: WeaveFocus) => {
      const cible = (f.world || f.qn || "").trim();
      if (!cible) return;
      const i = worlds.findIndex((w) =>
        w.qn === cible || w.label === cible || w.path === cible ||
        w.qn.endsWith(cible) || w.label.startsWith(cible));
      if (i >= 0) gotoWorld(i);
    };
    viserFocusRef.current = viserFocus;
    const detachKeys = bindViewKeys({
      next: () => gotoWorld(focusW + 1),
      prev: () => gotoWorld(focusW < 0 ? 0 : focusW - 1),
      down: () => {
        // ── « ↓ ENTRER » DOIT DESCENDRE, PAS CHANGER DE BASE ────────────────
        // MESURÉ sur la page servie (8432) : `down` appelait `onOpen`,
        // c'est-à-dire `setActive(path)` — un CHANGEMENT DE BASE. Sur la base
        // déjà active, l'effet était donc rigoureusement nul : `t` 12 % avant,
        // 12 % après, distance caméra 369,2 → 369,2, `verrou` inchangé. Or le
        // panneau annonce « ↓ entrer pour de bon », `VIEW_KEYS_HINT` annonce
        // « ↓ entrer », et `Entrée` est mappé sur ce même geste : c'était donc
        // LE seul geste de descente annoncé au clavier — et il ne descendait pas.
        //
        // `plonger` est la voie qui descend réellement (celle du bouton ⤓), et
        // elle tient compte du groupe déjà désigné. On l'appelle : ↓ devient le
        // pas suivant de la descente, à quelque échelle qu'on soit.
        // La « vue dédiée » (changer de base) reste sur le double-clic, qui est
        // précisément ce que la légende annonce.
        plonger();
      },
      /**
       * `Page ↓` entre dans le plus gros sous-groupe, `Page ↑`/Retour arrière
       * remontent d'un niveau — la même grammaire que le reste de l'application.
       *
       * MON PREMIER GESTIONNAIRE NE FAISAIT RIEN, et c'était mesurable : il
       * exigeait `focusW >= 0`, c'est-à-dire une visée faite AU CLAVIER (←/→).
       * Or au cran « monde » la visée vient de la CAMÉRA : `focusW` vaut -1, la
       * condition était donc toujours fausse et `Page ↓` n'avait aucun effet.
       * On retombe donc sur le monde que la caméra regarde (`ciblePrecedente`,
       * posée par la boucle), puis sur le premier — c'est exactement la règle que
       * `plonger` applique déjà pour choisir sa cible.
       */
      entrer: () => {
        const qn = focusW >= 0 ? qnList[focusW] : (ciblePrecedente ?? qnList[0]);
        if (qn) entrerPlusGrosRef.current?.(qn);
      },
      remonter: () => {
        const qn = focusW >= 0 ? qnList[focusW] : (ciblePrecedente ?? qnList[0]);
        if (!qn) return;
        // L'ORDRE COMPTE : on remonte d'abord la HIÉRARCHIE (sous-groupes), et
        // seulement une fois aux racines on remonte le CHEMIN DE GRAPHE — qui,
        // lui, se parcourt par centres (clic sur un nœud). Un seul geste couvre
        // donc les deux profondeurs, dans l'ordre où on les a prises.
        const iv = interieurs.get(qn);
        if (iv?.prefixe) { remonterGroupeRef.current?.(qn); return; }
        const ch = graphes.get(qn)?.entrees ?? [];
        if (ch.length > 1) { revenirEntreeRef.current?.(qn, ch.length - 2); return; }
        revenirEntreeRef.current?.(qn, 0);
      },
      up: overview,
      home: overview,
      // `d` — DENSIFIER LE NIVEAU (la page suivante), et non descendre d'un cran.
      // Même cible que le geste de `Page ↓` : le monde que l'on regarde.
      densifier: () => {
        const qn = interieur?.qn ?? (focusW >= 0 ? qnList[focusW] : (ciblePrecedente ?? qnList[0]));
        if (qn) densifierNiveauRef.current?.(qn);
      },
      zoomIn: () => zoomBy(camera, ctrl, 0.75),
      zoomOut: () => zoomBy(camera, ctrl, 1.33),
      // Échap doit RAMENER, pas seulement oublier le niveau visé : sans le vol,
      // la caméra restait dans le monde (mesuré : 100 % avant, 100 % après).
      escape: () => overview(),
    });

    // ── boucle ──
    const clock = new THREE.Clock();
    let raf = 0;

    /**
     * ── TAMPONS STABLES DE LA BOUCLE ─────────────────────────────────────────
     *
     * La boucle allouait, À CHAQUE IMAGE : un tableau `positions`, un objet
     * `{qn, p, mass}` par monde, un `Vector3` cloné par monde, et un `Vector3`
     * pour l'offset d'étiquette par monde. Sur 12 mondes à 60 fps, cela fait
     * ~1 500 allocations par seconde qui alimentent le ramasse-miettes — et une
     * pause GC se voit à l'œil, c'est exactement ce qui casse la sensation de
     * fluidité. Ces tampons vivent hors de la boucle et sont RÉUTILISÉS.
     *
     * Sûreté : le seul consommateur de `positions` est la boucle de déformation
     * de CETTE image (`for (const q of positions)`), il ne retient rien — vérifié
     * avant d'écrire ceci. Réutiliser est donc sans effet de bord, et l'ordre
     * d'itération (qui compte pour les puits gravitationnels) est préservé.
     */
    const positions: { qn: string; p: THREE.Vector3; mass: number }[] = [];
    const labelOffset = new THREE.Vector3();
    const tmp = new THREE.Vector3();
    const stepOf = (t01: number) => {
      const steps = wv.densification?.steps || [];
      if (!steps.length) return null;
      const k = Math.min(steps.length - 1, Math.max(0, Math.round(t01 * (steps.length - 1))));
      return steps[k];
    };
    const loop = () => {
      raf = requestAnimationFrame(loop);
      const dt = figeRef.current ? 0 : clock.getDelta();
      const nowReel = performance.now();
      // ⏸ FIGER : toute l'animation (orbites, pulsations, nappe) s'arrête sur
      // une image. Ce n'est pas qu'un confort de mesure — c'est ce qu'on veut
      // pour LIRE un graphe : une scène qui bouge en permanence rend
      // illisible ce qu'on essaie justement de distinguer. C'est aussi ce qui
      // rend l'effet d'une sélection mesurable (bruit de fond ~0).
      if (!figeRef.current) nowFigeRef.current = nowReel;
      const now = nowFigeRef.current;
      // densification : lecture automatique → rejoue le développement réel
      if (playingRef.current) {
        const t = densityRef.current + dt * 0.06;
        if (t >= 1) { setDensity(1); setPlaying(false); }
        else setDensity(t);
      }
      // ── DESCENTE CONTINUE ────────────────────────────────────────────────
      // La distance caméra à la boule visée donne un facteur 0→1 lissé. Rien
      // n'est commuté : plus on s'approche, plus la boule s'ouvre, plus les
      // autres reculent, plus la toile s'efface. C'est le même geste que
      // d'entrer dans une ville sur une carte — la carte ne change pas.
      let t = 0;
      let tS = 0;
      /**
       * CONTRACTION DE L'INTÉRIEUR, calculée pendant la descente et appliquée
       * une seule fois, avec la révélation.
       *
       * Bug trouvé à la sonde : la révélation posait `scale = 0,55 + 0,45·ouv`
       * soit 1,00 en profondeur, et elle s'exécute APRÈS le calcul de
       * contraction — elle l'ÉCRASAIT donc à chaque image. La contraction
       * tournait pour rien, ce qui explique qu'elle n'ait eu aucun effet
       * mesurable. Deux facteurs d'échelle sur le même objet doivent être
       * fusionnés, pas posés l'un après l'autre.
       */
      let contractionInterne = 1;
      // ── ENTRÉE NATURELLE ────────────────────────────────────────────────
      // Constat de terrain : sans visée préalable au clavier, la descente ne
      // démarrait JAMAIS — il fallait d'abord appuyer sur ←/→ ou cliquer. Or la
      // navigation naturelle, c'est zoomer VERS ce qu'on regarde. On détermine
      // donc le monde visé par la caméra elle-même : le plus proche ET le mieux
      // aligné avec l'axe de visée. Viser au clavier reste possible et prime.
      let cibleQn = focusQnRef.current;
      const autoCible = !cibleQn;
      // ── LA CIBLE NE LÂCHE PAS PENDANT L'APPROCHE ─────────────────────────
      //
      // Bug rapporté : « j'avance et je repars instantanément en arrière ».
      // MESURÉ : en zoomant, `t` retombait à 0, `boules` de 1 à 0 et `pret` à
      // NON — autrement dit LA CIBLE CHANGEAIT. La sélection automatique
      // re-choisissait un monde à chaque image ; quand elle basculait, `t` se
      // recalculait depuis zéro et l'intérieur du nouveau monde (non chargé)
      // remplaçait celui qu'on était en train d'ouvrir. Vérifié par contraste :
      // dix secondes SANS RIEN TOUCHER → t, apparent, pret et boules stables.
      // Le déclencheur est donc bien le zoom, et le défaut est la versatilité de
      // la cible.
      //
      // Une cible ne se lâche que si l'on s'en est VRAIMENT éloigné : tant qu'on
      // approche, elle tient. C'est la même règle d'hystérésis que pour les
      // étages, appliquée à ce qui les déclenche.
      const garderCible = (() => {
        if (!ciblePrecedente) return null;
        const n = worldNodes.get(ciblePrecedente);
        if (!n) return null;
        const R = layout.orbits.get(ciblePrecedente)?.size || 30;
        const dist = camera.position.distanceTo(n.mesh.position);
        return dist < R * 25 ? ciblePrecedente : null;
      })();
      // INSTRUMENT : on publie la cible elle-même, pour voir si elle bascule.
      (etagesRef.current as Record<string, unknown>).cible = {
        choisie: cibleQn ?? null,
        precedente: ciblePrecedente ?? null,
        gardee: garderCible ?? null,
        focus: focusQnRef.current ?? null,
        // ── LA FRATRIE DU NŒUD VISÉ, EN CHIFFRES ───────────────────────────
        // Publiée depuis la boucle parce que c'est là que l'instrument vit,
        // mais CALCULÉE ailleurs (une fois par graphe, à la demande) : on ne
        // fait ici que relire un index déjà construit, donc le coût par image
        // reste une lecture de `Map`.
        fratrie_noeud: (() => {
          const idx = indexNoeudRef.current;
          const v = viseRef.current;
          if (!idx || !v || v.genre !== "noeud") return null;
          const k = idx.index.cle.get(v.nom);
          if (!k) return { qn: v.nom, indexe: false, n: idx.index.n, ms: idx.ms };
          const f = fratrieDe(idx.index, k);
          const tot = (g: number) => f.groupes[g]?.total ?? 0;
          return {
            qn: v.nom, indexe: true, n: idx.index.n, ms: idx.ms,
            parent: f.parent, fichier: f.fichier ?? null,
            communaute: f.communaute ?? null, scc: f.scc ?? null,
            freres: tot(0), co_fichiers: tot(1), soeurs: tot(2),
            co_modules: tot(3), cycle: tot(4), homonymes: tot(5),
          };
        })(),
      };
      if (garderCible) cibleQn = garderCible;
      else if (!cibleQn && worldNodes.size) {
        const fwd = new THREE.Vector3();
        camera.getWorldDirection(fwd);
        let meilleur = Infinity;
        worldNodes.forEach((node, qn) => {
          const vers = node.mesh.position.clone().sub(camera.position);
          const dist = vers.length();
          const align = vers.normalize().dot(fwd);
          if (align < 0.2) return; // derrière ou hors champ : on n'y entre pas
          const score = dist / Math.max(0.2, align);
          if (score < meilleur) { meilleur = score; cibleQn = qn; }
        });
      }
      ciblePrecedente = cibleQn;
      const cible = cibleQn ? worldNodes.get(cibleQn) : null;
      // ── LA MOLETTE DOIT VISER LE MONDE, PAS LE CENTRE DE L'UNIVERS ──────
      // Bug mesuré : la molette rapprochait la caméra de l'ORIGINE (cible
      // d'OrbitControls) ; au-delà de six crans elle la traversait et ressortait
      // de l'autre côté — la descente retombait à « espace 0 % ». Zoomez vers un
      // monde et la caméra fonçait vers le centre du système. Quand la cible est
      // choisie par la caméra (et non au clavier), on fait donc d'elle le centre
      // d'orbite : le zoom se rapproche alors de ce que l'on regarde, ce qui est
      // le geste naturel.
      // La cible SUIT le monde tant que c'est la caméra qui l'a choisie. La
      // poser une seule fois ne suffisait pas : le monde orbitait encore, et la
      // caméra fonçait vers une position déjà périmée — mesuré, elle le
      // dépassait et la descente retombait de 48 % à 25 %.
      // ── LE CORPS DE RÉFÉRENCE : LE GROUPE QUAND IL Y EN A UN ──────────────
      // Défaut rapporté : « le zoom reste attaché à la dimension du monde quand
      // je sélectionne une sous-catégorie ». Exact, et les trois grandeurs qui
      // commandaient la caméra venaient TOUTES du monde : la cible d'orbite
      // (`cible.mesh.position`), la butée de rapprochement (`r0 * 0.08`), et
      // l'échelle logarithmique de descente. Rien ne lisait la sélection : on
      // pouvait viser un groupe, la molette continuait de se régler sur le
      // monde — et son centre est du VIDE, entre les amas. On ne pouvait donc ni
      // zoomer vers la sous-catégorie, ni la centrer.
      //
      // Ici la sélection DEVIENT la référence : le centre d'orbite, la butée et
      // l'échelle de distance sont ceux du groupe choisi, dès lors que son
      // intérieur est chargé (avant, on retombe sur le monde — un cadre honnête
      // vaut mieux qu'un cadre vide). Ce qui n'est PAS touché : `t` (la descente)
      // reste mesurée sur le monde, parce que c'est elle qui pilote les étages ;
      // changer sa référence ferait sauter le niveau à chaque clic de groupe.
      const choixGroupe = ensembleChoisi.current;
      const itSel = cibleQn ? interieurs.get(cibleQn) : undefined;
      const bouleSel = choixGroupe && choixGroupe.qn === cibleQn && itSel
        ? itSel.boules.find((b) => b.name === choixGroupe.nom)
        : undefined;
      let centreRef: THREE.Vector3 | null = null;
      let rayonRef = 0;
      if (bouleSel && cible && itSel) {
        centreRef = bouleSel.pts.localToWorld(bouleSel.centre.clone());
        // Le rayon RÉEL à l'écran : la boule du groupe, l'échelle du monde et
        // celle de l'étage intérieur s'appliquent toutes les trois.
        rayonRef = Math.max(
          1e-3,
          bouleSel.rayon * (cible.mesh.scale.x || 1) * (itSel.grp.scale.x || 1),
        );
      }
      // ── LA RÉFÉRENCE DE CE FRAME, ET SON CORPS ────────────────────────────
      // Un seul endroit décide de ce que la caméra vise et de l'échelle sur
      // laquelle sa molette se règle : ici. `anim` (le vol) est la SEULE autre
      // autorité, et elle prime pendant qu'elle tourne — deux écrivains sur
      // `ctrl.target` dans la même image, c'était la poursuite qui écrasait le
      // vol ou l'inverse selon l'ordre, donc une transition qui dépendait d'un
      // ordre d'instructions.
      const r0Monde = cibleQn ? (layout.orbits.get(cibleQn)?.size || 30) : 30;
      const posVoulue = centreRef ?? (cible ? cible.mesh.position : null);
      const rayonVoulu = centreRef ? rayonRef : r0Monde;
      const fracVoulue = centreRef ? 0.25 : 0.08;
      const genreVoulu: "monde" | "groupe" = centreRef ? "groupe" : "monde";
      if (posVoulue) {
        if (!cadreRef.init) {
          // Premier frame : on prend la référence telle quelle, sans transition
          // (il n'y a rien à quitter).
          cadreRef.init = true;
          cadreRef.pos.copy(posVoulue);
          cadreRef.rayon = Math.max(1e-3, rayonVoulu);
          cadreRef.frac = fracVoulue;
        } else {
          // Poursuite CONTINUE, à taux indépendant du framerate. Le rayon est
          // interpolé en LOG : de 30 à 8,7, les deux échelles se traversent au
          // même rythme visuel qu'un doublement de distance.
          const k = 1 - Math.exp(-dt * 2.2);
          cadreRef.pos.lerp(posVoulue, k);
          const lr = Math.max(1e-3, cadreRef.rayon);
          const lv = Math.max(1e-3, rayonVoulu);
          cadreRef.rayon = Math.exp(Math.log(lr) + (Math.log(lv) - Math.log(lr)) * k);
          cadreRef.frac += (fracVoulue - cadreRef.frac) * k;
        }
        cadreRef.genre = genreVoulu;
        cadreRef.nom = centreRef ? (choixGroupe?.nom ?? "") : (cibleQn ?? "");
      }
      const verrou = cibleUtilisateurRef.current;
      if (!anim.on && cadreRef.init) {
        // La cible suit la référence POURSUIVIE : c'est la transition elle-même,
        // et elle est la même à toutes les échelles.
        ctrl.target.lerp(cadreRef.pos, Math.min(1, 1 - Math.exp(-dt * 6)));
        if (centreRef) dernierAutoRef.current = cibleQn;
        if (!autoCible && !centreRef) dernierAutoRef.current = null;
      } else if (verrou && !anim.on) {
        // Le choix de l'utilisateur tient : on rejoint sa cible sans la remplacer.
        ctrl.target.lerp(verrou.pos, Math.min(1, 1 - Math.exp(-dt * 4)));
      }
      if (cible && cibleQn) {
        const r0 = r0Monde;
        // ── LA BUTÉE SUIT LE CORPS VISÉ ─────────────────────────────────────
        // Sur le monde : on s'arrête à son cœur (8 % de son rayon). Sur un
        // groupe : on doit pouvoir ENTRER dedans, donc la butée est celle du
        // groupe (25 % de son rayon, plancher 0,4) — sinon la molette se bloque
        // à 2,4 unités pour un groupe de rayon 8,7 et l'on reste dehors.
        // La butée suit le rayon POURSUIVI (et non la valeur visée) : la loi de
        // zoom se déplace donc en continu pendant un changement d'échelle, au
        // lieu de sauter sous les doigts de l'utilisateur en plein geste.
        ctrl.minDistance = Math.max(
          centreRef ? 0.4 : 1.2,
          cadreRef.rayon * cadreRef.frac,
        );
        // `d` reste la distance au MONDE : c'est elle qui alimente `t`, `tS` et
        // la contraction de l'étage intérieur — donc les changements d'étage. La
        // rapporter au groupe ferait TOMBER la descente de 100 % à ~60 % au
        // premier clic de groupe, et rejouerait exactement le défaut documenté
        // plus bas (« j'avance et je reparts en arrière »). La distance au corps
        // VISÉ sert au cadrage, pas à la profondeur.
        const d = camera.position.distanceTo(cible.mesh.position);
        const dRef = centreRef ? camera.position.distanceTo(centreRef) : d;
        // ── ÉCHELLE LOGARITHMIQUE, ET LA CAMÉRA ENTRE VRAIMENT ───────────
        // DEUX DÉFAUTS mesurés, qui expliquent « je ne suis toujours pas dans
        // une cellule » :
        //  1. `pres` valait 2,2 rayons : à 100 % de descente la caméra était
        //     DEHORS. Tout ce que j'appelais « dedans » était « arrêté devant ».
        //     La caméra va maintenant jusqu'à 0,25 rayon — dans la cellule.
        //  2. la course était LINÉAIRE sur 14 rayons : 93 % de la descente se
        //     passait dans le vide, et les échelles qui comptent (surface,
        //     intérieur) étaient écrasées dans les derniers centièmes. On passe
        //     en LOGARITHMIQUE : chaque doublement de distance prend la même
        //     part de course, comme sur une carte. Les paliers deviennent alors
        //     réguliers : 2 rayons ≈ 48 %, 1 rayon ≈ 66 %, 0,5 ≈ 83 %, 0,25 = 100 %.
        const loin = r0 * 14;
        const pres = r0 * 0.25;
        const echelle = Math.log(loin / Math.max(1e-3, pres));
        t = Math.max(0, Math.min(1, Math.log(loin / Math.max(1e-3, d)) / echelle));
        // ÉTAGE SIGNAUX : on continue d'avancer au-delà de l'entrée, jusqu'au
        // cœur (0,25 → 0,08 rayon) : la matière se réorganise en espace des
        // signaux sans qu'on ait à changer de vue.
        const presS = r0 * 0.08;
        tS = Math.max(0, Math.min(1,
          Math.log(Math.max(1e-3, pres) / Math.max(1e-3, d)) /
          Math.log(Math.max(1e-3, pres) / Math.max(1e-3, presS))));
        // La coquille des amas suit la caméra : la matière reste à une distance
        // RELATIVE constante, donc toujours autour de soi, à toute profondeur.
        // ⚠ IL FAUT DIVISER PAR L'ÉCHELLE DU MONDE.
        // Mesuré à la sonde : le monde est mis à l'échelle ×4,5 (`msc=4.500`).
        // Toutes les distances de l'intérieur sont donc multipliées par 4,5 :
        // les amas posés à 31–51 unités locales finissaient à 140–230 unités,
        // pour une caméra arrêtée à 7,3. Ma contraction précédente (×0,25)
        // était DIX FOIS trop faible — c'est pour cela qu'elle n'avait aucun
        // effet visible, et que la cellule restait vide. On rapporte donc la
        // contraction au monde RÉEL, échelle comprise.
        const echelleMonde = cible?.mesh.scale.x || 1;
        contractionInterne = Math.max(
          0.02, Math.min(1.25, d / Math.max(1, r0 * 0.6 * echelleMonde)),
        );
        // ── INSTRUMENT : LE CADRAGE EST-IL RÉGLÉ SUR LA SÉLECTION ? ─────────
        // « Le zoom reste attaché à la dimension du monde quand je sélectionne
        // une sous-catégorie. » Ces nombres rendent la réponse VÉRIFIABLE au lieu
        // de la croire : sur quoi le cadrage est réglé, la butée réellement
        // appliquée, et à quelle distance la caméra se tient du corps visé.
        // Publié ici parce que `dRef`/`rayonRef` vivent dans cette portée.
        (etagesRef.current as Record<string, unknown>).cadrage_zoom = {
          reference: cadreRef.genre === "groupe" ? "groupe" : "monde",
          nom: cadreRef.nom,
          butee: +ctrl.minDistance.toFixed(3),
          distance: +dRef.toFixed(2),
          rayon_vise: +(centreRef ? rayonRef : r0).toFixed(2),
          rayon_cadre: +cadreRef.rayon.toFixed(2),
          // 0 = la référence poursuivie a rejoint la référence voulue : la
          // transition d'échelle est terminée. C'est ce nombre qui dit si une
          // bascule d'échelle est EN COURS ou stabilisée.
          ecart_transition: +(
            Math.abs(Math.log(Math.max(1e-3, cadreRef.rayon) / Math.max(1e-3, centreRef ? rayonRef : r0)))
          ).toFixed(3),
          ecrivain: anim.on ? "vol" : "poursuite",
        };
      }
      // On remonte la valeur CONTINUE à l'interface, mais à cadence bornée
      // (~5 fois par seconde) : un rendu React par image ne servirait à rien et
      // coûterait cher. Le chiffre affiché suit donc vraiment la caméra.
      // On remonte AUSSI le PALIER (0..3) : `t` sature à 1 sous 0,25 R et ne peut
      // donc pas distinguer « groupe » de « signaux ». Le palier se lit sur la
      // distance, avec les seuils du curseur (`palierDe`) — une seule définition,
      // partagée par le panneau, les boutons et le rapport de niveau.
      if (nowReel - dernierMajRef.current > 200) {
        dernierMajRef.current = nowReel;
        setDescente(t);
        const Rc = cibleQn ? (layout.orbits.get(cibleQn)?.size || 30) : 30;
        const pc = cible ? palierDe(camera.position.distanceTo(cible.mesh.position), Rc) : 0;
        if (pc !== palierRef.current) { palierRef.current = pc; setPalier(pc); }
        const enVol = !!cibleQn && !!graphes.get(cibleQn)?.charge;
        if (enVol !== chargementEnVolRef.current) { chargementEnVolRef.current = enVol; setChargementEnVol(enVol); }
      }

      const step = stepOf(densityRef.current);
      const alive = step ? step.weight : Infinity;

      // positions orbitales (képlériennes) — tampon STABLE, pas de tableau neuf
      let pi = 0;
      for (const wd of worlds) {
        const o = layout.orbits.get(wd.qn)!;
        const node = worldNodes.get(wd.qn)!;
        let a = o.phase + now * 0.00028 * o.w * 900;
        // UN MONDE QU'ON A ENTRÉ NE BOUGE PLUS. Mesuré : à l'intérieur, le
        // monde continuait d'orbiter et glissait sous la caméra — la descente
        // RECULAIT (100 % → 98 → 96) et l'intérieur devenait intenable à
        // explorer. On gèle son orbite tant qu'on est dedans (et on la reprend
        // en sortant) : on n'explore pas une cible qui fuit.
        const gele = wd.qn === (focusQnRef.current || ciblePrecedente) && t > 0.12;
        if (gele) {
          if (!angleGele.has(wd.qn)) angleGele.set(wd.qn, a);
          a = angleGele.get(wd.qn)!;
        } else if (angleGele.has(wd.qn)) {
          angleGele.delete(wd.qn);
        }
        tmp.set(Math.cos(a) * o.r, Math.sin(o.inc) * Math.sin(a) * o.r * 0.22, Math.sin(a) * o.r);
        node.mesh.position.copy(tmp).multiplyScalar(1);
        node.mesh.position.y += Math.sin(o.inc) * 40;
        node.mesh.rotation.y += dt * 0.25;
        node.label.position.copy(node.mesh.position).add(labelOffset.set(0, o.size + 26, 0));
        node.pos.copy(node.mesh.position);
        // Réutilisation : l'entrée est créée UNE fois, puis mutée.
        let ent = positions[pi];
        if (!ent) {
          ent = { qn: "", p: new THREE.Vector3(), mass: 0 };
          positions[pi] = ent;
        }
        ent.qn = wd.qn;
        ent.p.copy(node.mesh.position);
        ent.mass = wd.mass ?? 0.3;
        pi += 1;
      }

      positions.length = pi; // borne le tampon à ce qui vient d'être rempli
      // ── LE SOL S'AFFAISSE À LA MASSE ──
      const attr = sheetGeo.attributes.position as THREE.BufferAttribute;
      const colAttr = sheetGeo.attributes.color as THREE.BufferAttribute;
      for (let i = 0; i < attr.count; i++) {
        const x = sheetBase[i * 3], z = sheetBase[i * 3 + 2];
        let sag = 0;
        let dmin = Infinity; // distance a la matiere la plus proche
        for (const q of positions) {
          const dx = x - q.p.x, dz = z - q.p.z;
          const d2 = dx * dx + dz * dz;
          if (d2 < dmin) dmin = d2;
          // puits gravitationnel : profondeur ∝ masse, largeur ∝ masse
          sag -= (150 * q.mass + 40) / (1 + d2 / (200 * 200 * (0.5 + q.mass)));
        }
        // ── RELIEF THERMODYNAMIQUE DE LA VOILE ──
        //
        // `t` est un POTENTIEL (profondeur normalisée du puits gravitationnel).
        // On le lit comme une TEMPÉRATURE normalisée, et on la fait agir sur
        // DEUX choses — c'est là que le tissage exprime la thermodynamique :
        //
        //  1. la GÉOMÉTIE : une ondulation thermique dont l'amplitude ET la
        //     longueur d'onde suivent la température. Le froid garde une
        //     respiration faible (la voile n'est jamais figée) ; le chaud soulève
        //     une houle courte. Cette ondulation INTERFÈRE avec le jitter
        //     déterministe des mailles : c'est l'interférence des deux qui tisse
        //     le relief, pas l'une ou l'autre seule ;
        //  2. la COULEUR, sobre et PLAFONNÉE. L'ancienne rampe montait à
        //     (0,94, 0,62, 0,32) — quasi blanc — et en mélange additif la
        //     superposition saturait la voile en blanc pur. La nouvelle rampe
        //     reste mate : ardoise froide (0,16 0,24 0,40) -> ambre éteint
        //     (0,46 0,40 0,24). Le maximum est à 0,46, très loin du blanc : ce
        //     qui éclaire, c'est la DENSITÉ du tissage, pas la brillance du fil.
        //     L'usage d'une racine garde le cœur chaud lisible sans l'écraser.
        const temp = Math.sqrt(Math.min(1, Math.max(0, -sag / 260)));
        const phase = x * 0.021 + z * 0.013;
        const relief = Math.sin(phase) * (4 + 26 * temp) * (0.25 + 0.75 * temp);
        // courbure en coupe : quadratique en rayon, nulle au centre
        const rr = Math.sqrt(x * x + z * z) / (SPAN * 0.5);
        attr.setY(i, sag + relief + rr * rr * LIFT);
        // ── EXTINCTION LOIN DE LA MATIÈRE ──
        //
        // Sans ça la nappe est un mur de lignes qui occupe tout l'écran : l'œil
        // ne voit qu'elle, et le relief ne se lit plus puisqu'il n'a pas de
        // vide autour de lui. Le tissage s'éteint donc à mesure qu'on s'éloigne
        // de la matière la plus proche : la toile devient une trame qui se
        // dissout dans le noir, et seuls les puits restent lisibles.
        // (le coût est nul : `dmin` est calculé dans la boucle de déformation)
        const fade = 1 / (1 + dmin / (700 * 700));
        // La température module la teinte ET la présence : loin de la matière
        // le fil n'émet presque plus.
        const k = fade * (0.30 + 0.70 * temp);
        colAttr.setXYZ(
          i,
          k * (0.16 + 0.30 * temp),
          k * (0.24 + 0.16 * temp),
          k * (0.40 - 0.16 * temp),
        );
      }
      attr.needsUpdate = true;
      colAttr.needsUpdate = true;
      // Deux fois par seconde au lieu d'une fois par image : la sphère englobante
      // est une borne de culling, pas une mesure à jour au pixel.
      if (nowReel - sphereMajRef.current > 500) {
        sphereMajRef.current = nowReel;
        sheetGeo.computeBoundingSphere();
      }
      sheet.position.y = SHEET_Y;

      // ── LA TOILE SE TISSE ──
      let budget = alive;
      const totalW = fibers.reduce((s, f) => s + f.total, 0) || 1;
      for (const f of fibers) {
        const wa = worldNodes.get(f.a)!, wb = worldNodes.get(f.b)!;
        const A = wa.mesh.position, B = wb.mesh.position;
        const lift = 40 + 120 * Math.min(1, f.total / maxW);
        const mid = A.clone().add(B).multiplyScalar(0.5);
        mid.y += lift;
        f.curve.v0.copy(A); f.curve.v1.copy(mid); f.curve.v2.copy(B);
        const n = f.weights.length;
        // Combien de synapses de CETTE fibre sont nées à l'instant t ?
        const share = (f.total / totalW) * (step ? step.weight : totalW);
        const k = Math.max(1, Math.min(n, Math.round((share / f.total) * n)));
        const attr2 = f.points.geometry.attributes.position as THREE.BufferAttribute;
        for (let i = 0; i < n; i++) {
          const p = f.curve.getPointAt(Math.min(0.999, f.weights[i]));
          attr2.setXYZ(i,
            p.x + f.jitter[i * 3] * 0.35,
            p.y + f.jitter[i * 3 + 1] * 0.35,
            p.z + f.jitter[i * 3 + 2] * 0.35);
        }
        attr2.needsUpdate = true;
        f.points.geometry.setDrawRange(0, k);
        // La toile recule quand on entre dans un monde : on ne peut pas lire
        // l'intérieur d'une boule à travers le tissage de l'univers.
        (f.points.material as THREE.PointsMaterial).opacity =
          (budget > 0 ? 0.95 : 0.25) * (1 - 0.9 * t);
        budget -= k;
      }

      // Les liens de parenté structurelle suivent leurs mondes (et pointillés
      // recalculés : les pointillés se mesurent en distance, pas en temps).
      for (const kl of kinshipLines) {
        const wa = worldNodes.get(kl.a), wb = worldNodes.get(kl.b);
        if (!wa || !wb) continue;
        const A = wa.mesh.position.clone(), B = wb.mesh.position.clone();
        const mid = A.clone().add(B).multiplyScalar(0.5);
        const c = new THREE.QuadraticBezierCurve3(A, mid, B);
        kl.line.geometry.dispose();
        kl.line.geometry = new THREE.BufferGeometry().setFromPoints(c.getPoints(48));
        kl.line.computeLineDistances();
      }

      // vol de caméra (navigation clavier commune)
      if (anim.on) {
        anim.k = Math.min(1, anim.k + dt * 2.4);
        const e = 1 - Math.pow(1 - anim.k, 3);
        camera.position.lerpVectors(anim.from, anim.to, e);
        ctrl.target.lerpVectors(anim.tFrom, anim.tTo, e);
        if (anim.k >= 1) anim.on = false;
      }
      /**
       * ── LE VOL PUBLIE SON PROPRE ÉTAT (dernière autorité sans mesure) ──────
       * MESURÉ : le cran vise 59,47 et la caméra reste à ~713, en descendant de
       * 2 unités par seconde ; `reprises` monte à 3 sans que rien ne bouge. Or la
       * boucle n'a QU'UN écrivain de caméra — ce vol. On publie donc ses entrées
       * et ses distances (`de` = d'où il part, `vers` = où il en est, `total` =
       * la longueur prévue) : si `total` vaut ~650 et que la caméra n'avance pas,
       * c'est le vol qui est écrasé ; si `total` vaut ~0, c'est la CIBLE qui est
       * fausse ; si `on` reste faux, le vol n'est jamais parti.
       */
      (etagesRef.current as Record<string, unknown>).vol = {
        on: anim.on, k: +anim.k.toFixed(3),
        de: +camera.position.distanceTo(anim.from).toFixed(2),
        vers: +camera.position.distanceTo(anim.to).toFixed(2),
        total: +anim.from.distanceTo(anim.to).toFixed(2),
        cible: [+anim.to.x.toFixed(1), +anim.to.y.toFixed(1), +anim.to.z.toFixed(1)],
        cam: [+camera.position.x.toFixed(1), +camera.position.y.toFixed(1), +camera.position.z.toFixed(1)],
      };
      // le monde visé se signale (même code visuel que la sélection ailleurs),
      // et un monde CONSULTÉ par l'engine s'allume proportionnellement à son
      // énergie — c'est le graphe qui s'active en direct, pas une animation.
      // ── LA DEMANDE DE NIVEAU AGIT ICI, DANS LA BOUCLE ─────────────────────
      //
      // Un `useEffect` séparé ne pouvait pas fonctionner : `worldNodes` et
      // `layout` sont des variables LOCALES de cette boucle, et `overview` vit
      // dans un autre hook. Placé hors périmètre, l'effet ne compilait pas —
      // `tsc`, désormais en PORTE, a arrêté la construction avant tout
      // déploiement. Troisième fois que je place du code où c'est pratique
      // plutôt que là où la donnée vit : la règle est appliquée, pas répétée.
      //
      // Les boutons de dimension demandent un niveau ; la scène descend avec SA
      // géométrie (d = marge·R / sin(θ/2)), celle qui place la taille apparente
      // dans la plage qui OUVRE l'étage voulu : ~0,77 pour l'intérieur (seuil
      // 0,37), ~1,7 pour le graphe (seuil 0,60). Aucun réglage nouveau.
      // La DEMANDE se lit dans la référence (voir `niveauDemandeRef` : `niveau`
      // est figé dans la fermeture de cet effet, qui ne se relance pas — sinon
      // les trois boutons de vue ne produisaient rien, mesuré sur la page servie).
      const niv = niveauDemandeRef.current;
      /**
       * ── LE JOURNAL DES CRANS : UNE DEMANDE DÉPOSÉE SE DIT ──────────────────
       *
       * MESURÉ (2026-09-15) : les quatre boutons de crân n'ont produit AUCUN effet
       * — `t` s'arrêtait à 0,1201 et n'en bougeait plus, le bouton actif restait
       * « 🌌 Cosmos », aucun graphe n'était chargé — et RIEN ne le disait. La
       * cause est structurelle à ce bloc : `else if (worldNodes.size)` n'a pas de
       * branche pour « pas de monde connu », donc la demande tombe dans le vide
       * (pas de `flyTo`, et `niveauAppliqueRef` n'est pas avancé). Le dépôt du
       * graphe avait déjà appris la leçon (`chargements` : demandes, enVol,
       * memeCentre, lancees) : on tient le même journal ici, pour que le refus
       * soit MESURABLE au lieu d'être invisible.
       */
      const jn = (etagesRef.current as Record<string, unknown>).crans =
        ((etagesRef.current as Record<string, unknown>).crans ?? { demandes: 0, appliques: 0, sansMonde: 0, dernier: 0 }) as
        { demandes: number; appliques: number; sansMonde: number; dernier: number;
          mondeVise?: string; rayonVise?: number; distanceVisee?: number; distanceMonde?: number;
          /** Réajustements de la tenue de bande (`-1` = ce cran ne tient pas ici). */
          reprises?: number;
          /** Le VERDICT de la tenue : ce qu'elle mesure, et si elle juge devoir agir. */
          tenue?: { niv: number; d: number; R: number; palier: number; reprises: number } };
      if (niv !== niveauAppliqueRef.current) {
        jn.demandes++;
        jn.dernier = niv;
        // Un cran d'échelle est un DÉPLACEMENT : le chemin parcouru le long de
        // l'axe repart de zéro. Le garder ferait croire qu'on est encore entré
        // dans les mêmes nœuds, alors que le graphe va être rechargé.
        if (cheminAxeRef.current.length) { cheminAxeRef.current = []; setCheminAxe([]); }
        if (niv === 0) {
          cibleUtilisateurRef.current = null;
          flyTo(HOME_POS, new THREE.Vector3(0, 0, 0));
          niveauAppliqueRef.current = 0;
          jn.appliques++;
        } else if (worldNodes.size) {
          let cibleN = focusQnRef.current;
          if (!cibleN || !worldNodes.has(cibleN)) {
            let meilleur = Infinity;
            worldNodes.forEach((node, qn) => {
              const dd = camera.position.distanceTo(node.mesh.position);
              if (dd < meilleur) { meilleur = dd; cibleN = qn; }
            });
          }
          const nodeN = cibleN ? worldNodes.get(cibleN) : null;
          if (nodeN) {
            const RN = layout.orbits.get(cibleN as string)?.size || 30;
            const fovN = (camera.fov * Math.PI) / 180;
            // ── LE NIVEAU « GRAPHE » CADRE LE GROUPE, PAS LE MONDE ───────────
            // Le graphe vit désormais DANS la région du groupe choisi (voir la
            // boucle, plus bas). Cadrer le monde à 0,55 rayon — l'ancien réglage
            // du niveau 2 — viserait donc à côté : le graphe est un petit objet
            // excentré dans la boule. On cadre sa région, et on garde la formule
            // (d = R / sin(θ/2)) pour que la taille apparente soit la même
            // quelle que soit la dimension du groupe.
            const itN = interieurs.get(cibleN as string);
            const nomG = graphes.get(cibleN as string)?.ensemble
              ?? (ensembleChoisi.current?.qn === cibleN ? ensembleChoisi.current.nom : null);
            const bouleN = itN && nomG ? itN.boules.find((x) => x.name === nomG) : undefined;
            let PN = nodeN.mesh.position.clone();
            // ── LE CRAN EST UN PALIER DE L'ÉCHELLE, PAS UNE DISTANCE AU JUGÉ ──
            // On vise `distancePourPalier(niv)` — la même échelle que celle que le
            // panneau affiche et qui ouvre les étages — et la distance s'en déduit.
            // Un cran exprimé en rayons pouvait tomber dans la bande d'à côté :
            // mesuré, le cran « 🔺 Graphe » à 0,55 R tombait à `t` = 0,63, soit
            // DANS la bande « monde », pendant que le bouton allumé restait
            // « 🧠 Monde ». Le cran et son libellé se contredisaient.
            const tVise = niv === 2 ? CRAN_T.groupe : CRAN_T.monde;
            // LA VISÉE EST GARANTIE DANS LA BANDE DU CRAN (`viserCran`) : mesuré, les
            // crâns « groupe » et « signaux » visaient juste et la caméra se posait
            // dans la bande « monde ». Et la distance COURANTE est passée pour qu'un
            // cran plus profond ne vise jamais plus loin que là où l'on est.
            let dN = viserCran(niv as Palier, RN, camera.position.distanceTo(nodeN.mesh.position));
            if (niv === 3) {
              // ── LE QUATRIÈME CRAN VA AU CŒUR, PAS AU GROUPE ────────────────
              // L'espace des signaux est défini par la distance (0,08 R, cf.
              // `presS`) : c'est là que la matière se réorganise selon courbure,
              // énergie et degré. Cadrer un groupe n'y mènerait pas.
              PN = nodeN.mesh.position.clone();
            } else if (niv === 2 && bouleN) {
              // ── LE GRAPHE VIT DANS SON GROUPE : ON CADRE LE GROUPE ─────────
              // `localToWorld` tient compte de TOUTES les transformations (le
              // monde grossit pendant la descente) — seule façon d'être sûr de
              // viser le groupe et pas son fantôme. Le rayon est celui du groupe
              // RÉELLEMENT dessiné : boule du monde et contraction de l'étage
              // intérieur s'appliquent toutes deux.
              const PGrp = bouleN.pts.localToWorld(bouleN.centre.clone());
              const rayonMonde = (nodeN.mesh.scale.x || 1) * (itN?.grp.scale.x || 1);
              const rayonVise = Math.max(1e-3, bouleN.rayon * rayonMonde);
              // MÊME CADRAGE QUE LE CLIC (une seule constante, `CADRAGE_POUPEE`).
              const dGroupe = (rayonVise * CADRAGE_POUPEE) / Math.max(0.05, Math.sin(fovN / 2));
              // ── MAIS LE CRAN NE RECULE JAMAIS ──────────────────────────────
              // MESURÉ : `CADRAGE_POUPEE = 4,2` sur un GROS groupe (région
              // jusqu'à 0,34 R) donne 8,4 × 0,34 ≈ 2,9 R — plus loin que le
              // cadrage du monde (2,6 R). Cadrer le groupe ÉLOIGNAIT donc la
              // caméra : `t` reculait de 42 % à 26 % et le rapport de niveau
              // retombait à 0. Un cran qui recule n'est pas un cran. On ne prend
              // donc le cadrage du groupe que s'il PLACE la caméra dans la bande
              // visée ; sinon on garde la distance qui, par construction, y est.
              const dGrpSurMonde = PGrp.distanceTo(nodeN.mesh.position) + dGroupe;
              const loin = RN * 14, pres = RN * 0.25;
              const tGrp = Math.log(loin / Math.max(1e-3, dGrpSurMonde)) / Math.log(loin / pres);
              if (tGrp >= tVise) { PN = PGrp; dN = dGroupe; }
            }
            cibleUtilisateurRef.current = { pos: PN.clone(), nom: cibleN as string };
            /**
             * ── LE CRAN PUBLIE SON UNITÉ (respect de la représentation) ────────
             * Le vol vise `dN = distancePourPalier(niv, RN)` ; le PALIER, lui, se
             * mesure sur `r0Monde`. Si ces deux rayons ne sont pas le MÊME nombre,
             * le cran est honoré dans une unité et jugé dans une autre : mesuré,
             * la caméra se pose à 486,92 là où le cran « monde » valait 129, le
             * voyant reste « Cosmos » et l'on croit que rien ne s'est passé.
             * On publie donc l'unité du vol À CÔTÉ de celle de la mesure
             * (`monde.rayon_cadre`), pour qu'un écart d'unité soit visible au
             * lieu d'être déduit.
             */
            jn.mondeVise = String(cibleN).slice(-48);
            jn.rayonVise = +RN.toFixed(2);
            jn.distanceVisee = +dN.toFixed(2);
            jn.distanceMonde = +camera.position.distanceTo(nodeN.mesh.position).toFixed(2);
            flyTo(PN.clone().add(new THREE.Vector3(0, dN * 0.22, dN * 0.97)), PN);
            niveauAppliqueRef.current = niv;
            jn.appliques++;
            /**
             * ── UNE SEULE AUTORITÉ DE MONDE : L'ÉTAT, PAS LA RÉFÉRENCE ────────
             *
             * MESURÉ (2026-09-15), par l'état du vol lui-même :
             *     vol   = { on: false, k: 1, vers: 0, total: 903,02 }  ← le vol ARRIVE
             *     crans = { mondeVise: world:aletheia-chat-locale-n3-gelee-12-09, R 25,99 }
             *     espaces = { R_local: 56,4, d_locale: 748 }            ← AUTRE monde
             * Le cran visait un monde ; le palier — donc le voyant, donc la réponse
             * à « suis-je arrivé ? » — se mesurait sur un autre, 748 unités plus
             * loin. Le vol était parfait et mal jugé : le cran ne pouvait pas
             * s'allumer.
             * La boucle suit `focusQnRef.current` (l. 4617), mais cette référence est
             * réécrite à CHAQUE rendu depuis l'état (`focusQnRef.current = focusQn`) :
             * y écrire est effacé au rendu suivant — essayé, mesuré, sans effet.
             * Un `ref` qui re-synchronise un état est un CACHE, pas une autorité :
             * on écrit donc l'ÉTAT, et la boucle suit d'elle-même au rendu suivant.
             * Viser, mesurer et annoncer parlent alors du MÊME monde.
             */
            setFocusQn(String(cibleN));
            dernierAutoRef.current = String(cibleN);
            /**
             * ── LE CRAN SE RETIENT : IL TIENT LA BANDE QU'IL A ATTEINTE ───────
             *
             * MESURÉ (2026-09-15) : le vol vers le cran « monde » se pose à 96,73
             * — dans la bande (43,6 → 200,9) — puis la caméra se retrouve à 486,75
             * et `t` retombe de 0,45 à 0,12 : le voyant reste « 🌌 Cosmos »,
             * l'intérieur ne s'ouvre pas, aucun graphe n'est demandé. La cause
             * n'est pas le vol (0,42 s, non annulé) mais le DÉCOR : le monde
             * s'éloigne pendant la descente, la caméra reste, et la distance `d`
             * — celle qui mesure le palier — sort de la bande toute seule.
             * Ce fichier connaît déjà ce piège (l. 3768 : « camDist partait à
             * 589, t tombait à 0,076, la boucle de niveau renvoyait la caméra
             * chez elle ») ; il avait été corrigé pour le CLIC sur un groupe, pas
             * pour les CRANS.
             * On retient donc ce que le cran a atteint : tant que la demande n'a
             * pas changé, on réajuste la distance quand le palier MESURÉ retombe
             * sous celui demandé. Borné (`CRAN_REPRISES_MAX`) et COMPTÉ : un cran
             * qui se reprend trois fois sans tenir est un cran qui ne peut pas
             * tenir, et on le saura.
             */
            cranTenuRef.current = { niv, qn: String(cibleN), reprises: 0 };
          } else {
            // Le monde VISÉ n'est pas rendu (pas encore de toile, ou qn absent) :
            // on le DIT, au lieu de laisser le bouton allumé contredire la scène.
            jn.sansMonde++;
            setVise({
              genre: "ensemble", nom: PALIERS[niv],
              detail: `le cran « ${PALIERS[niv]} » n'a pas de monde à cadrer pour l'instant : `
                + "les mondes de la toile ne sont pas encore rendus — restez sur un cran, il se peuplera",
            });
          }
        } else {
          // AUCUN monde rendu : on le COMPTE et on le DIT UNE fois. On n'avance
          // PAS `niveauAppliqueRef` — la toile peut se tisser ensuite, et la
          // demande doit alors s'appliquer (la consommer ici la perdrait pour de
          // bon). Le rapport n'est posé qu'une fois : `setVise` à chaque image
          // ferait re-rendre la page 60 fois par seconde.
          if (jn.sansMonde++ === 0) {
            setVise({
              genre: "ensemble", nom: PALIERS[niv],
              detail: `aucun monde n'est rendu dans la scène : le cran « ${PALIERS[niv]} » ne peut pas se cadrer `
                + "(la toile n'est pas encore tissée sur cet instant)",
            });
          }
        }
      }
      /**
       * ── LA TENUE DU CRAN : LA BANDE ATTEINTE NE SE PERD PAS TOUTE SEULE ───
       *
       * MESURÉ : le cran « monde » se pose à 96,73 (bande 43,6 → 200,9) puis la
       * caméra se retrouve à 486,75, `t` retombe de 0,45 à 0,12, le voyant reste
       * « 🌌 Cosmos » et l'intérieur ne s'ouvre jamais. Le vol n'est pas en cause
       * (0,42 s, jamais annulé) : c'est le DÉCOR qui s'éloigne pendant la
       * descente, alors que la caméra, elle, ne bouge plus — et `d`, la distance
       * qui MESURE le palier, sort de la bande sans qu'on ait rien fait.
       * Ce fichier connaît déjà ce piège (l. 3768 : « camDist partait à 589,
       * `t` tombait à 0,076, la boucle de niveau renvoyait la caméra chez elle ») ;
       * il avait été corrigé pour le CLIC sur un groupe, jamais pour les CRANS.
       * On rétablit donc la DISTANCE (jamais la direction : un utilisateur qui a
       * tourné autour du monde ne se le voit pas reprendre), tant que la demande
       * n'a pas changé. Borné (`CRAN_REPRISES_MAX`) et COMPTÉ : un cran qui se
       * reprend trois fois sans tenir est un cran qui ne peut pas tenir ici, et
       * on le dit au lieu de le reprendre indéfiniment.
       */
      const tenu = cranTenuRef.current;
      if (tenu && !anim.on && niveauDemandeRef.current === tenu.niv) {
        const wd = worldNodes.get(tenu.qn);
        if (wd) {
          const R = layout.orbits.get(tenu.qn)?.size || 30;
          const P = wd.mesh.position;
          /**
           * ── LA TENUE PUBLIE SON PROPRE VERDICT ────────────────────────────
           * MESURÉ (cran « signaux », curseur à la souris) : la visée est JUSTE
           * (63,05 → 12,60 → 2,20, même monde, monotone) et la caméra ne rejoint
           * jamais (338 → 270 → 225), sans qu'aucune `reprise` n'apparaisse. Pour
           * savoir si la tenue refuse de jouer (palier mesuré ≥ demandé) ou si
           * elle joue sans effet, il faut LIRE sa mesure : le palier qu'elle
           * calcule, le rayon qu'elle emploie et la distance qu'elle voit.
           */
          const dTenu = camera.position.distanceTo(P);
          const pTenu = palierDe(dTenu, R);
          jn.tenue = { niv: tenu.niv, d: +dTenu.toFixed(2), R: +R.toFixed(2), palier: pTenu, reprises: tenu.reprises };
          if (pTenu < tenu.niv) {
            if (tenu.reprises < CRAN_REPRISES_MAX) {
              const dv = viserCran(tenu.niv as Palier, R);   // garantie dans la bande
              const dir = camera.position.clone().sub(P).normalize();
              flyTo(P.clone().add(dir.multiplyScalar(dv)), P.clone());
              tenu.reprises++;
              jn.reprises = tenu.reprises;
            } else if (jn.reprises !== -1) {
              jn.reprises = -1;
              setVise({
                genre: "ensemble", nom: PALIERS[tenu.niv],
                detail: `le cran « ${PALIERS[tenu.niv]} » ne tient pas : la distance à « ${tenu.qn} » sort de sa bande `
                  + `plus vite qu'on ne la rétablit (${CRAN_REPRISES_MAX} reprises, comptées)`,
              });
            }
          } else if (tenu.reprises > 0 && jn.reprises !== -1) {
            jn.reprises = tenu.reprises;   // il a fallu reprendre, et il tient : le compte le montre
          }
        }
      }
      // ── QUEL GROUPE EST LE « DEDANS » DE CE MONDE ? ─────────────────────────
      // DEUX GRANDEURS, ET IL FAUT LES DEUX — c'est le défaut mesuré : après un
      // clic sur le groupe « ts », l'interface annonçait « Ensemble visé : ts »
      // pendant que la scène gardait son enveloppe sur « deepseek_harness » (le
      // groupe dont le graphe était chargé). La vue ne s'adaptait donc PAS au
      // geste : elle attendait que le chargement veuille bien suivre.
      //   - `groupeVise` est LE GESTE : ce que l'utilisateur vient de désigner,
      //     sinon ce qui est chargé. C'est lui qui décide de l'enveloppe et de la
      //     mise en retrait — donc ce que l'œil voit immédiatement.
      //   - `groupeCharge` est LE CONTENU : le groupe dont les nœuds sont
      //     réellement affichés. Il ne doit pas s'éteindre pendant les quelques
      //     secondes que met son remplaçant à charger (mesuré : 4,4 à 5,3 s).
      const groupeVise = (qn: string): string | null => {
        const c = ensembleChoisi.current;
        if (c && c.qn === qn) return c.nom;
        return graphes.get(qn)?.ensemble ?? null;
      };
      const groupeCharge = (qn: string): string | null => graphes.get(qn)?.ensemble ?? null;

      worldNodes.forEach((node, qn) => {
        // « on » = monde effectivement visé, au clavier OU par la caméra.
        const on = qn === (focusQnRef.current || cibleQn);
        const wd = worlds.find((w) => w.qn === qn);
        const e = wd ? (energyRef.current.get(wd.path) || 0) : 0;
        // ── LE CALCUL DE PROXIMITÉ REMONTÉ ICI : UNE SEULE AUTORITÉ ─────────
        //
        // `fermeture` (le monde s'efface) et `ouv` (sa composition apparaît)
        // étaient calculés dans DEUX périmètres et sur DEUX grandeurs : `t` sur
        // la CIBLE, `apparent` sur LE MONDE DONT ON DESSINE L'INTÉRIEUR. Mesuré
        // en désaccord : `t` montait à 0,885 pendant qu'`apparent` retombait de
        // 0,26 à 0,12 — le monde s'effaçait donc sans rien révéler.
        //
        // On calcule ici, une fois, la grandeur qui décide de tout : la TAILLE
        // APPARENTE de CE monde. Les deux effets s'en dérivent.
        const rMondeI = layout.orbits.get(qn)?.size || 30;
        const posMondeI = node.mesh.position;
        const fovRadI = (camera.fov * Math.PI) / 180;
        const apparentI = (() => {
          const dist = Math.max(1e-3, camera.position.distanceTo(posMondeI));
          const haut = (2 * rMondeI) / (2 * dist * Math.tan(fovRadI / 2));
          return Math.min(haut, haut / Math.max(0.2, camera.aspect));
        })();
        const cleEtageI = "int:" + qn;
        const etI = lissageRef.current.get(cleEtageI) ?? { v: apparentI, ouvert: false };
        etI.v += (apparentI - etI.v) * 0.10;              // filtre du premier ordre
        const ouvrirI = lisser((etI.v - 0.12) / 0.25);
        const garderI = lisser((etI.v - 0.06) / 0.25);    // fermeture plus tardive
        let ouv = on ? (etI.ouvert ? Math.max(garderI, ouvrirI) : ouvrirI) : 0;
        if (ouv > 0.02) etI.ouvert = true;
        if (etI.v < 0.04) etI.ouvert = false;
        if (!on) ouv = 0;
        lissageRef.current.set(cleEtageI, etI);
        // ── N'EXPORTER QUE LE MONDE VISÉ (instrument corrigé) ──────────────
        // La boucle passe sur TOUS les mondes : écrire à chaque tour faisait
        // mesurer le DERNIER d'entre eux, jamais la cible. C'est ce qui m'a fait
        // lire « ouv = 0 » alors que la grandeur d'ouverture valait 0,259, bien
        // au-dessus du seuil de 0,12. Un instrument qui lit autre chose que ce
        // qu'on croit est pire que pas d'instrument du tout.
        if (on) {
          const itI = interieurs.get(qn);
          etagesRef.current.interieur = {
            ouv: +ouv.toFixed(4), apparent: +apparentI.toFixed(4), lisse: +etI.v.toFixed(4),
            ouvert: etI.ouvert,
            pret: !!(itI && itI.pret),
            boules: itI ? itI.boules.length : -1,
          };
        }
        // Ouverture CONTINUE de la boule visée ; les autres reculent.
        const ouverture = on ? 1 + 1.5 * t : 1 - 0.55 * t;
        const boost = (on ? EMPHASIS.active : 1 + 0.5 * e) * ouverture;
        node.mesh.scale.setScalar(boost);
        // Les nuages des mondes non visés s'effacent pendant la descente : on
        // entre DANS un monde, pas dans six.
        node.mesh.children.forEach((ch) => {
          if ((ch as THREE.Points).isPoints) {
            const pm = (ch as THREE.Points).material as THREE.ShaderMaterial;
            const cible = on ? 0.96 : Math.max(0.05, 0.85 * (1 - 0.9 * t));
            if (pm.uniforms?.uOpacite) pm.uniforms.uOpacite.value = cible;
          }
        });
        // ── LA CELLULE SE FERME ──────────────────────────────────────────
        // Constat de terrain : une fois dedans, on ne se sentait PAS dans une
        // cellule — la nappe, les autres mondes et la toile continuaient de
        // briller autour, et la composition ressemblait à une dalle posée dans
        // le vide (vérifié à la capture : un bloc plein, sans paroi). On ferme
        // donc l'espace : au-delà de la moitié de la descente, tout ce qui
        // n'est pas le monde où l'on entre s'éteint, et sa cage devient la
        // paroi — elle s'illumine et s'ouvre légèrement pour nous contenir.
        {
          // On ne ferme pas « en approchant » mais en FRANCHISSANT la surface :
          // la paroi se referme entre 60 % (encore dehors) et 80 % (dedans).
          // ── DEUX AUTORITÉS POUR UNE MÊME TRANSITION : CONSTAT, CONTRAINTE, PLAN ──
          //
          // MESURÉ AUX ÉTAGES EUX-MÊMES : `t` (la descente, calculée sur la
          // CIBLE) monte à 0,885 pendant qu'`apparent` (calculé sur LE MONDE
          // DONT ON DESSINE L'INTÉRIEUR) RETOMBE de 0,26 à 0,12. Deux mondes
          // différents, donc deux autorités — et elles se contredisent : le monde
          // s'efface sans que sa composition n'apparaisse jamais.
          //
          // J'ai tenté de dériver `fermeture` de `ouv` : INCOMPILABLE, et c'est
          // une vraie contrainte de structure, pas un détail —
          //     `fermeture` vit dans la BOUCLE DES MONDES (ici),
          //     `ouv` vit dans le BLOC INTÉRIEUR, plus bas dans la même image.
          // Unifier exige donc de REMONTER le calcul d'`apparent` et d'`ouv` en
          // tête de boucle, pas de les référencer. Tant que ce déplacement n'est
          // pas fait, on garde la formule connue-bonne, plutôt que de livrer une
          // référence pendante (ce que le premier essai a produit, tsc l'a
          // arrêté — mais le build était déjà passé : discipline corrigée, tsc
          // est désormais une PORTE, pas un rapport).
          // Le monde s'efface EXACTEMENT quand sa composition se révèle : même
          // grandeur, même instant. Les deux ne peuvent plus diverger — c'est
          // vrai par construction, plus par coïncidence de deux réglages.
          const fermeture = on ? Math.min(1, ouv * 1.15) : 0;
          // ── POINT DE MESURE INTERNE (placé là où `t` ET `fermeture` existent) ──
          // On n'exporte que des grandeurs dont le périmètre est CERTAIN : `t`,
          // `fermeture`, la distance caméra-cible. Exporter une variable hors
          // périmètre a déjà produit un bundle cassé (TDZ) : la leçon est de
          // placer la mesure là où la donnée vit, pas où c'est pratique.
          // N'EXPORTER QUE LE MONDE VISÉ — MÊME CORRECTION QUE POUR L'INTÉRIEUR.
          // La boucle passe sur TOUS les mondes : sans ce garde, le DERNIER monde
          // écrasait la mesure du monde visé, et l'on lisait « fermeture 0 »,
          // « cosmos 0 », « graphe 0 » pour une scène qui montrait exactement
          // l'inverse. Mesuré : la cible affichait `t = 0,8456` avec
          // `fermeture = 0` — deux nombres qui ne peuvent pas coexister.
          if (on) (etagesRef.current as Record<string, unknown>).monde = {
            t: +t.toFixed(4),
            fermeture: +fermeture.toFixed(4),
            verrou: cibleUtilisateurRef.current?.nom ?? null,
            camDist: +camera.position.distanceTo(ctrl.target).toFixed(2),
            // ── L'ÉTAGE DES SIGNAUX, MESURÉ LÀ OÙ IL SE PRODUIT ─────────────
            // C'est le fond de la descente, et il n'était VÉRIFIABLE nulle part :
            // son curseur (`tS`) et ses deux tampons de disposition vivaient dans
            // la boucle et n'en sortaient pas. Or c'est exactement l'étage dont
            // j'avais besoin de prouver qu'il s'exécute (il ne s'exécutait pas —
            // `posGraphe` n'était jamais assigné). On publie donc `tS` et la
            // présence des deux origines : le mélange devient un nombre, pas une
            // impression. Mesure placée là où la donnée vit, comme le reste.
            signaux: {
              tS: +tS.toFixed(4),
              melange: +(tS > 0.001).valueOf(),
              origineGraphe: !!graphes.get(qn)?.posGraphe,
              origineSignaux: !!graphes.get(qn)?.posSignaux,
            },
            // ── L'AXE DE NAVIGATION, MESURÉ ─────────────────────────────────
            // « Polymorphe » ne doit pas être une intention : on publie l'axe
            // ACTIF, combien de groupes il distingue, et lequel est le plus gros.
            // Un axe qui ne distingue rien (`groupes < 2`) le dit ici aussi.
            axe: (() => {
              const ivm = interieurs.get(qn);
              const parType = groupesAxe(qn, noeudsCourants(qn, ivm?.prefixe ?? "", ivm?.classes ?? []));
              const g = graphes.get(qn);
              if (!g?.pret) return { axe: dimensionRef.current, groupes: 0, plus: null, n: 0 };
              const par = new Map<string, number>();
              if (parType) for (const [t, qns] of parType) par.set(t, qns.length);
              else if (g.noeudsInfo?.length) {
                // Un axe qui ne distingue rien : on publie QUAND MÊME le compte
                // (1 groupe) — c'est l'annonce honnête, pas un silence.
                par.set(etiquetteDimension(g.noeudsInfo[0].qn, dimensionRef.current), g.noeudsInfo.length);
              }
              let plus: string | null = null, n = 0;
              for (const [t, c] of par) if (c > n) { plus = t; n = c; }
              return {
                axe: dimensionRef.current,
                groupes: par.size,
                plus,
                n,
                segmente: par.size > 1,
                // ── LA PROFONDEUR PARCOURUE LE LONG DE L'AXE ───────────────────
                // Combien de fois on est entré dans un groupe de l'axe, et par
                // quel nœud. C'est ce qui rend la descente polymorphe VÉRIFIABLE :
                // sans ce compte, « récursif » serait une intention.
                chemin: cheminAxeRef.current.length,
                dernier: cheminAxeRef.current[cheminAxeRef.current.length - 1]?.noeud ?? null,
              };
            })(),
          };
          (window as unknown as Record<string, unknown>).__etages = etagesRef.current;
          // RAPPORT DU NIVEAU RÉEL : les boutons doivent refléter ce que la scène
          // montre, pas ce qu'on a cliqué. On le déduit des ouvertures mesurées.
          const nInter = (etagesRef.current.interieur as { ouv?: number } | undefined)?.ouv ?? 0;
          const nGraph = (etagesRef.current.graphe as { ouv?: number } | undefined)?.ouv ?? 0;
          /**
           * ── LE NIVEAU RAPPORTÉ EST CELUI DE L'ÉCHELLE, PAS D'UNE OUVERTURE ──
           *
           * L'ouverture `ouvG` de l'étage graphe mesure la TAILLE APPARENTE DU
           * MONDE : elle dépasse 0,5 dès que le monde remplit l'écran — donc
           * aussi bien au cran « Monde » qu'au cran « Graphe ». En déduire le
           * niveau faisait donc allumer « 🔺 Graphe » alors qu'on venait de
           * cliquer « 🧠 Monde » : MESURÉ, `t` = 42 % et bouton actif « Graphe ».
           * L'encart annonçait l'étage suivant au lieu de celui où l'on est —
           * exactement ce que le fil des poupées existe pour éviter.
           *
           * Le niveau se lit donc dans le PALIER (`palierDe`, l'autorité unique
           * d'échelle, la même que le mot affiché et que les crans) : un cran, son
           * libellé et le voyant ne peuvent plus se contredire. Trois conditions
           * honnêtes s'y ajoutent — un palier n'est annoncé que si l'étage
           * correspondant est réellement OUVERT (`nInter`), jamais avant que le
           * monde soit visé, et le palier « signaux » exige en plus que la
           * réorganisation soit réellement à l'œuvre (`tS`, publié ci-dessus).
           */
          if (!on) return;   // le rapport ne concerne QUE le monde visé
          const niveauReel: NiveauCosmos = (() => {
            const p = palierDe(camera.position.distanceTo(node.mesh.position),
              layout.orbits.get(qn)?.size || 30);
            if (p >= 3) return tS > 0.02 && nGraph > 0.5 ? 3 : 2;
            if (p === 2) return nInter > 0.5 ? 2 : 1;
            if (p === 1) return nInter > 0.5 ? 1 : 0;
            return 0;
          })();
          /**
           * ── LE RAPPORT SE MESURE AUSSI LUI-MÊME ────────────────────────────
           *
           * Le voyant des crans est resté sur « 🌌 Cosmos » aux QUATRE crans
           * pendant un essai, alors que le mot du panneau (qui lit le même palier)
           * suivait correctement. Deux lectures du même fait qui divergent : il
           * fallait savoir LAQUELLE des entrées du rapport était en cause (`on` ?
           * le palier ? l'ouverture de l'étage ?) sans relire le code en
           * espérant tomber juste. On publie donc les entrées elles-mêmes.
           */
          (etagesRef.current as Record<string, unknown>).rapports = { ...rapportsRef.current };
          /**
           * ── DEUX ESPACES DANS LA MÊME MESURE : ON LES PUBLIE SÉPARÉMENT ─────
           *
           * MESURÉ : le cran « monde » vise `distancePourPalier(1, R)` = 96,73 et
           * la caméra se retrouve à 486,75 — un facteur **5,24**. Un facteur entier
           * de cette taille n'est pas un réglage : c'est une UNITÉ. La caméra vit
           * dans la SCÈNE (espace monde), alors que la cible du vol et le palier
           * sont calculés sur `mesh.position` — la position LOCALE, dans un parent
           * (le conteneur du monde) qui porte sa propre transformation. Tant que
           * la scène n'applique qu'une seule échelle, les deux coïncident ; dès
           * que le parent en porte une, viser et mesurer ne parlent plus de la
           * même longueur — et le voyant annonce un cran que la scène n'a pas.
           * On publie donc les DEUX espaces et l'échelle du parent, pour que
           * l'écart soit une mesure et non une déduction.
           */
          const posMondeReelle = new THREE.Vector3();
          node.mesh.getWorldPosition(posMondeReelle);
          const echParent = new THREE.Vector3();
          node.mesh.getWorldScale(echParent);
          (etagesRef.current as Record<string, unknown>).espaces = {
            d_locale: +camera.position.distanceTo(node.mesh.position).toFixed(2),
            d_monde: +camera.position.distanceTo(posMondeReelle).toFixed(2),
            /** Rayon du monde : en unités LOCALES (là où il est défini)… */
            R_local: +(layout.orbits.get(qn)?.size || 30).toFixed(2),
            /** …et tel qu'il est RÉELLEMENT dessiné (échelle du parent comprise). */
            R_monde: +((layout.orbits.get(qn)?.size || 30) * Math.abs(echParent.x || 1)).toFixed(2),
            echelle_parent: +echParent.x.toFixed(4),
          };
          (etagesRef.current as Record<string, unknown>).rapport = {
            palier: palierDe(camera.position.distanceTo(node.mesh.position),
              layout.orbits.get(qn)?.size || 30),
            nInter: +nInter.toFixed(4), nGraph: +nGraph.toFixed(4), tS: +tS.toFixed(4),
            reel: niveauReel,
          };
          if (niveauReel !== dernierNiveauRef.current) {
            dernierNiveauRef.current = niveauReel;
            onNiveau?.(niveauReel);
            // Compteur de rapports : si le compteur monte et que le voyant d'App
            // ne suit pas, la faute est dans App ; s'il ne monte pas, elle est ici.
            rapportsRef.current = { n: rapportsRef.current.n + 1, dernier: niveauReel };
            // ── L'ENCART « POUPÉES » DOIT DIRE LE NIVEAU RÉEL ────────────────
            // Le rapport à l'App ne suffit pas : il remonte une fois, et l'encart
            // se rend hors de cette boucle. On tient donc un état, mis à jour
            // SEULEMENT au changement de niveau (un `setState` par image
            // relancerait un rendu React à 60 Hz — la faute que ce fichier a déjà
            // payée avec le GC).
            if (niveauReel !== niveauSceneRef.current) {
              niveauSceneRef.current = niveauReel;
              setNiveauScene(niveauReel);
            }
          }
          const cage = cages.get(qn);
          if (cage) {
            const m = cage.material as THREE.MeshBasicMaterial;
            m.opacity = on ? 0.1 + 0.42 * fermeture : 0.1;
            cage.scale.setScalar(1 + 0.12 * fermeture);
            // MESURABLE : la paroi du monde EST la première poupée. On l'exporte
            // pour que l'emboîtement se vérifie par des nombres (opacité de la
            // cage, retrait des frères, échelle du graphe) et non à l'œil.
            if (on) (etagesRef.current.monde as Record<string, unknown>).cage = +m.opacity.toFixed(4);
          }
          if (fermeture > 0) {
            // ── LE COSMOS RESTE EN VEILLEUSE : LA POUPÉE DU DEHORS NE MEURT PAS ──
            // Avant : `effacer = 1 - fermeture` — à l'entrée, le cosmos
            // s'éteignait À ZÉRO. Mesuré à l'écran : une fois dans un groupe, plus
            // rien ne disait de quel monde on venait, ni qu'il y en avait
            // d'autres. Or « s'emboîter comme des poupées russes » demande
            // exactement l'inverse : chaque niveau continue d'ÊTRE REPRÉSENTÉ
            // quand on ouvre le suivant, en retrait mais lisible.
            // Le plancher n'est pas un réglage : c'est la limite en dessous de
            // laquelle une enveloppe n'est plus une information (0,14 = on la
            // voit, on ne la lit plus).
            const effacer = Math.max(0.14, 1 - fermeture);
            if (on) (etagesRef.current.monde as Record<string, unknown>).cosmos = +effacer.toFixed(4);
            (sheet.material as THREE.ShaderMaterial).uniforms.uOpacite.value =
              0.09 * (1 - 0.85 * t) * effacer;
            // La TOILE (fibres entre mondes) et les liens de parenté s'éteignent
            // aussi : ce sont des relations ENTRE mondes, elles n'ont pas de sens
            // quand on est à l'intérieur de l'un d'eux.
            const effacerLigne = (mat: THREE.Material & { opacity?: number }) => {
              if (typeof mat.opacity !== "number") return;
              if (mat.userData.repos === undefined) mat.userData.repos = mat.opacity;
              mat.opacity = (mat.userData.repos as number) * effacer;
            };
            fibers.forEach((f) => {
              effacerLigne(f.points.material as THREE.Material & { opacity?: number });
            });
            lignesFlux.forEach((l) => {
              effacerLigne(l.material as THREE.Material & { opacity?: number });
            });
            kinshipLines.forEach((k) => {
              effacerLigne(k.line.material as THREE.Material & { opacity?: number });
            });
            scene.children.forEach((o: any) => {
              if (o.isPoints && o !== node.mesh) {
                const m = o.material as THREE.PointsMaterial;
                if (m && "opacity" in m) {
                  if (o.userData.opaciteRepos === undefined) o.userData.opaciteRepos = m.opacity ?? 1;
                  m.opacity = (o.userData.opaciteRepos as number) * effacer;
                }
              }
            });
          }
        }

        // ── RÉVÉLATION DE L'ÉTAGE INTÉRIEUR ──────────────────────────────
        // On APPROCHE (t > 0,22) → on lance le chargement AVANT de le montrer.
        // On le montre quand il est PRÊT, et seulement alors on efface la
        // composition de surface : jamais d'écran vide entre deux étages.
        const it = interieurs.get(qn);
        // Les chargements d'étage sont pilotés par la TIMELINE (voir plus bas),
        // un par un, dans l'ordre : plus de requêtes concurrentes.
        if (on && wd) {
          /**
           * ── CHANGER D'AXE RE-SEGMENTE L'ÉTAGE ─────────────────────────────
           * L'intérieur d'un monde est DÉJÀ construit avec les groupes de l'axe
           * précédent : sans reconstruction, choisir « communauté » laisserait à
           * l'écran les familles de chemins, et le panneau annoncerait un axe qui
           * n'a rien changé. On vide donc l'étage intérieur et on relance la
           * timeline — le graphe, lui, n'est pas rechargé (ses gardes le
           * protègent), donc le coût est celui de la seule organisation.
           */
          if (sourceConstruite !== sourceGroupes()) {
            sourceConstruite = sourceGroupes();
            reconstructionsInterieur += 1;
            (etagesRef.current as Record<string, unknown>).segmentation = {
              axe: dimensionRef.current,
              // `familles` (l'agrégat serveur, sur le monde entier) ou `axe`
              // (les groupes d'une relation, sur l'échantillon du graphe).
              source: sourceConstruite,
              reconstructions: reconstructionsInterieur,
            };
            interieurs.forEach((iv) => {
              iv.grp.clear();
              iv.boules = [];
              iv.charge = false;
              iv.pret = false;
              iv.enveloppe = undefined;
            });
            dernierMondeTimeline = null;
          }
          if (dernierMondeTimeline !== qn) { dernierMondeTimeline = qn; resetTimeline(); }
          const grT = graphes.get(qn);
          avancerTimeline(t, wd, it, grT);
          const resume = evenements
            .map((e) => `${e.label} ${e.etat === "pret" ? "✓" : e.etat === "charge" ? "…" : e.etat === "echec" ? "✗" : "—"}${e.ms ? ` ${e.ms}ms` : ""}`)
            .join(" · ");
          if (resume !== timelinePubliee) { timelinePubliee = resume; setTimeline(resume); }
        }
        // AXE DU TEMPS : chargé en même temps que l'intérieur (il vient d'une
        // autre source, mais il doit être prêt quand on le demande).
        // (l'histoire est pilotée par la timeline, en second)
        {
          const g2 = graphes.get(qn);
          if (g2 && it) {
            const surTemps = !!(on && axeTempsRef.current && g2.pretTemps);
            if (g2.grpTemps) g2.grpTemps.visible = surTemps && (t > 0.4);
            // Les ensembles s'effacent quand on regarde le temps : deux axes ne
            // se superposent pas, sinon on ne lit ni l'un ni l'autre.
            const masquerEnsembles = surTemps && t > 0.4;
            if (masquerEnsembles) {
              it.boules.forEach((b) => { b.pts.visible = false; });
            } else if (it.pret) {
              // FONDU AU LIEU D'UN POP. L'étage graphe s'ouvre par une rampe
              // (`scale` 0,6→1 et `uOpacite` progressif) ; l'intérieur, lui,
              // apparaissait d'un coup (`visible = true`). Deux étages qui
              // s'ouvrent différemment, ça se voit et ça casse la fluidité de la
              // descente. On lui donne le même traitement.
              // Écrit défensivement : si le matériau n'a pas d'`uOpacite`, on se
              // contente de rendre visible — aucun risque de casser l'affichage.
              // Le fondu suit désormais la FENÊTRE DE VISIBILITÉ (0,20 -> 0,44).
              // Avant, il commençait à t = 0,05 alors que l'étage ne devenait
              // visible qu'à 0,56 : il ne servait donc à RIEN, la visibilité
              // décidant seule. Un fondu qui ne s'applique pas est un fondu mort.
              const ouvI = Math.max(0, Math.min(1, (t - 0.22) / 0.22));
              // ── LE GROUPE CHOISI EST SITUÉ PARMI SES FRÈRES ─────────────────
              // Le graphe du groupe visé s'ouvre maintenant DANS son groupe (voir
              // plus bas). Si les frères disparaissaient à ce moment-là, on
              // perdrait ce qui fait un groupe : sa place. Ils restent donc
              // affichés en retrait — c'est la deuxième poupée qui contient la
              // troisième — pendant que le groupe visé reste à pleine matière.
              const nomVise = groupeVise(qn);
              // CE QUI EST RÉELLEMENT AFFICHÉ : le groupe dont les nœuds sont
              // posés dans la boule. Il garde sa matière pendant que son
              // remplaçant charge (4,4 à 5,3 s mesurés) ; sans cela, cliquer un
              // groupe ferait disparaître le graphe avant que le nouveau soit prêt
              // — un écran vide entre deux étages, exactement ce que ce fichier
              // interdit par ailleurs.
              const nomGraphe = groupeCharge(qn);
              // On retient l'opacité RÉELLEMENT appliquée : celle du groupe visé et
              // celle d'un frère. Sans ces deux nombres, « les frères restent
              // visibles en retrait » serait une intention, pas un fait.
              let ouvVise = -1;
              let ouvFrere = -1;
              it.boules.forEach((b) => {
                if (!b.pts.visible) b.pts.visible = true;
                // DEUX FAMILLES DE MATÉRIAUX. Les niveaux « monde » et
                // « graphe » utilisent des ShaderMaterial (uniform `uOpacite`) ;
                // les ENSEMBLES de l'étage intérieur utilisent un
                // `PointsMaterial` natif — qui n'a PAS d'`uniforms`, seulement une
                // propriété `opacity`. Ma première version ne testait que
                // `uOpacite` : elle ne trouvait rien et ne faisait donc RIEN, en
                // silence — le fondu que je croyais avoir ajouté n'existait pas.
                // On couvre les deux, et on ne dépend plus d'une supposition.
                // Restent à pleine matière : le groupe DÉSIGNÉ (le geste) et le
                // groupe dont le graphe est DÉJÀ affiché (le contenu). Les autres
                // reculent — mais restent : c'est ce qui situe le groupe choisi
                // dans son monde, plutôt que de le laisser seul au monde.
                const ouvB = nomVise && b.name !== nomVise && b.name !== nomGraphe
                  ? ouvI * 0.26
                  : ouvI;
                if (b.name === nomVise) ouvVise = ouvB;
                else if (b.name !== nomGraphe && ouvFrere < 0) ouvFrere = ouvB;
                const m = b.pts.material as THREE.Material & {
                  opacity?: number;
                  uniforms?: { uOpacite?: { value: number } };
                };
                if (m?.uniforms?.uOpacite) {
                  m.uniforms.uOpacite.value = ouvB;
                } else if (typeof m?.opacity === "number") {
                  m.opacity = ouvB; // PointsMaterial : `transparent` est déjà posé
                }
              });
              // ── L'ENVELOPPE DU GROUPE VISÉ ─────────────────────────────────
              // Elle se place sur le groupe choisi et s'illumine avec la
              // descente : c'est la paroi de la poupée qu'on ouvre, et c'est
              // elle qui rend lisible « le graphe est DANS ce groupe ».
              if (it.enveloppe) {
                const b = nomVise ? it.boules.find((x) => x.name === nomVise) : undefined;
                it.enveloppe.visible = !!b && ouvI > 0.02;
                if (b) {
                  it.enveloppe.position.copy(b.centre);
                  it.enveloppe.scale.setScalar(b.rayon * 1.06);
                  const em = it.enveloppe.material as THREE.LineBasicMaterial;
                  em.color.copy(couleurFamille(b.name));
                  em.opacity = 0.55 * ouvI;
                  // MESURE : les poupées internes, en opacité réelle (la paroi du
                  // monde, elle, est exportée dans `monde.cage`).
                  const tailleMonde = layout.orbits.get(qn)?.size ?? 30;
                  (etagesRef.current.interieur as Record<string, unknown>).poupees = {
                    groupe_vise: +ouvVise.toFixed(3),
                    groupe_frere: +ouvFrere.toFixed(3),
                    enveloppe_groupe: +em.opacity.toFixed(3),
                    // ── LES FRÈRES : COMBIEN, ET LESQUELS ───────────────────
                    // Une opacité de frère ne dit pas COMBIEN de frères sont
                    // visibles, ni lesquels. On exporte donc aussi le nombre de
                    // boules réellement dessinées et leurs noms : c'est la
                    // vérification que « les frères sont nommés » n'est pas une
                    // intention. `nommes` doit valoir `boules` : une boule
                    // dessinée sans puce serait un frère muet.
                    freres_boules: it.boules.length,
                    freres_nommes: interieurBoulesRef.current.length,
                    freres_effectif: interieurBoulesRef.current.reduce((a, x) => a + x.count, 0),
                    // LA LOI D'ÉCHELLE, VÉRIFIABLE PAR LE CALCUL : si le volume
                    // dit la part réelle, alors rayon³ ÷ nœuds doit être LE MÊME
                    // nombre pour tous les groupes d'un monde — et ce nombre vaut
                    // `PART_MAX_GROUPE³ × 10⁶ / nœuds du monde`, donc il DIMINUE
                    // quand l'ensemble grandit. C'est la signature de la loi : un
                    // monde plus gros donne des poupées relativement plus petites.
                    // Un groupe au plancher (`PLANCHER_GROUPE`) est le seul à
                    // sortir de la constante, et il le doit — il est là pour
                    // rester désignable. On exporte tout cela pour que la loi se
                    // CONTRÔLE au lieu de se croire.
                    // Contrôle fait hors scène, sur les données réelles
                    // (`/api/viz/modules`), rayon normalisé par le rayon du monde :
                    //   linux-full3   23 groupes, r³/n = 0,030 pour TOUS
                    //   runtime-dev    6 groupes, r³/n = 0,310 pour les 3 non plancher
                    //     (tsx, ts, yml tombent au plancher : 12,5 · 25 · 41,7)
                    // 23 groupes totalisant 1,55 M de nœuds contre 6 groupes pour
                    // 127 k : la constante est 10 fois plus petite sur le monde 12
                    // fois plus gros — proportions toutes relatives, comme voulu.
                    loi: it.boules.slice(0, 8).map((x) => {
                      const r = x.rayon / tailleMonde;
                      return {
                        nom: x.name, n: x.count, r: +r.toFixed(4),
                        r3parNoeud: +((1e6 * r * r * r) / Math.max(1, x.count)).toFixed(2),
                      };
                    }),
                  };
                }
              }
            }
          }
        }
        // ── ÉTAGE GRAPHE : on l'approche (t > 0,78) → on le CHARGE, et on ne
        // le montre qu'ensuite. Il entre par le plus gros ensemble du monde.
        const gr = graphes.get(qn);
        // On déclenche TÔT (dès 0,45 de descente) parce que la source est LENTE :
        // mesuré, `hops=2` sur un gros store coûte 12,8 s au premier appel (puis
        // 0,01 s en cache). Attendre 0,78 laissait l'étage vide au moment où on
        // le regarde — « se peuple avant d'être remplacé » vaut aussi pour le
        // TEMPS de chargement, pas seulement pour l'ordre des étages.
        // (le graphe est piloté par la timeline, en troisième)
        if (gr) {
          // Le graphe remplace l'organisation une fois franchement dedans.
          // L'étage graphe vient APRÈS les groupes (0,62 contre 0,20) : la
          // séquence « on voit la composition, puis on entre dans le détail »
          // est maintenant l'ordre de la descente elle-même.
          // FENÊTRE 0,50 -> 0,72 : le détail APRÈS la composition, sans
          // recouvrir la précédente — deux étages qui se croisent produisent
          // deux sauts au lieu d'une transition.
          // Même règle que l'étage intérieur — la proximité de CE monde.
          // Même unité pour le détail : la taille à l'écran.
          const rMondeG = layout.orbits.get(wd?.qn ?? "")?.size || 30;
          const posMondeG = worldNodes.get(wd?.qn ?? "")?.mesh.position;
          const fovRad = (camera.fov * Math.PI) / 180; // local : le bloc graphe précède
          const apparentG = (() => {
            if (!posMondeG) return 0;
            const dist = Math.max(1e-3, camera.position.distanceTo(posMondeG));
            const haut = (2 * rMondeG) / (2 * dist * Math.tan(fovRad / 2));
            return Math.min(haut, haut / Math.max(0.2, camera.aspect));
          })();
          const cleG = "gr:" + (wd?.qn ?? "");
          const eg = lissageRef.current.get(cleG) ?? { v: apparentG, ouvert: false };
          eg.v += (apparentG - eg.v) * 0.10;
          const ouvrirG = lisser((eg.v - 0.30) / 0.30);
          const garderG = lisser((eg.v - 0.20) / 0.30);
          let ouvG = on ? (eg.ouvert ? Math.max(garderG, ouvrirG) : ouvrirG) : 0;
          if (ouvG > 0.02) eg.ouvert = true;
          if (eg.v < 0.12) eg.ouvert = false;
          if (!on) ouvG = 0;
          lissageRef.current.set(cleG, eg);
          if (on) etagesRef.current.graphe = {
            ouv: +ouvG.toFixed(4), apparent: +apparentG.toFixed(4), lisse: +eg.v.toFixed(4),
            ouvert: eg.ouvert,
          };
          const montrableG = gr.pret && ouvG > 0;
          gr.grp.visible = montrableG;
          if (montrableG) {
            // ── LE GRAPHE S'OUVRE **DANS** SON GROUPE ───────────────────────
            // Défaut de fond, mesuré dans le code : `chargerGraphe` disposait les
            // nœuds avec `R = taille × 1,15` — « le graphe occupe la boule
            // entière » — centré sur l'ORIGINE DU MONDE. Autrement dit, à l'instant
            // où l'on ouvrait un groupe, ce groupe cessait d'exister : sa matière
            // était remplacée par un graphe à l'échelle du monde. C'est
            // exactement ce que « poupées russes » interdit.
            // La disposition est inchangée (elle est mesurée, avec ses motifs) :
            // on la place DANS la région du groupe, à l'échelle que le chargeur a
            // fixée (`gr.echelle`, `gr.centreGroupe`) — une seule autorité, dans
            // le même périmètre que la donnée.
            const envG = gr.centreGroupe && gr.echelle != null
              ? { centre: gr.centreGroupe, echelle: gr.echelle }
              : null;
            // ── LA POUPÉE DEDANS EST DÉJÀ CONTRACTÉE ────────────────────────
            // `it.grp` (le groupe intérieur) porte une CONTRACTION d'échelle,
            // appliquée à l'étage intérieur. Les nuées des groupes en héritent
            // (elles sont ses enfants) ; le graphe, lui, est enfant de la BOULE.
            // Le poser sur `centre` sans cette contraction le placerait donc à
            // côté de son enveloppe — deux repères pour un même lieu, l'écart que
            // ce fichier paie à chaque fois qu'il laisse deux autorités cohabiter.
            const sIt = it?.grp.scale.x || 1;
            if (envG) {
              gr.grp.position.copy(envG.centre).multiplyScalar(sIt);
              gr.grp.scale.setScalar(envG.echelle * sIt * (0.6 + 0.4 * ouvG));
            } else {
              gr.grp.position.set(0, 0, 0);
              gr.grp.scale.setScalar(0.6 + 0.4 * ouvG);
            }
            etagesRef.current.enveloppe = (() => {
              // MESURE DU RENDEUR, pas du calcul : on exporte la position MONDE du
              // groupe visé et celle du graphe, telles que three.js les compose.
              // Si les deux coïncident, l'emboîtement est vrai dans la scène, pas
              // seulement dans l'intention du code.
              const nomV = groupeVise(qn);
              const b = nomV ? it?.boules.find((x) => x.name === nomV) : undefined;
              const pGroupe = b ? b.pts.localToWorld(b.centre.clone()) : null;
              const pGraphe = gr.grp.getWorldPosition(new THREE.Vector3());
              return {
                ensemble: nomV,
                centre_local: envG ? envG.centre.toArray().map((v) => +v.toFixed(2)) : null,
                echelle_graphe: envG ? +(envG.echelle * sIt).toFixed(4) : 1,
                contraction_interne: +sIt.toFixed(4),
                groupe_monde: pGroupe ? pGroupe.toArray().map((v) => +v.toFixed(2)) : null,
                graphe_monde: pGraphe.toArray().map((v) => +v.toFixed(2)),
                ecart_monde: pGroupe ? +pGroupe.distanceTo(pGraphe).toFixed(3) : null,
              };
            })();
            // ── CADRAGE : OÙ TOMBENT LES NIVEAUX À L'ÉCRAN ──────────────────
            // « Il faut voir les niveaux ENSEMBLE. » C'est vérifiable : on projette
            // chaque niveau en coordonnées écran (NDC : −1..1, 0 = centre) et on
            // mesure sa taille apparente. Si le monde ET le groupe sont dans
            // |NDC| ≤ 1, l'emboîtement est visible d'un seul regard — pas
            // successivement.
            {
              const proj = (p: THREE.Vector3) => p.clone().project(camera);
              const rayonEcran = (centre: THREE.Vector3, rayon: number) => {
                const a = proj(centre);
                const b2 = proj(centre.clone().add(new THREE.Vector3(0, rayon, 0)));
                return Math.abs((b2.y - a.y) / 2); // fraction de la hauteur d'écran
              };
              const nM = worldNodes.get(qn);
              const cM = nM ? nM.mesh.position.clone() : null;
              const nomV2 = groupeVise(qn);
              const bV = nomV2 ? it?.boules.find((x) => x.name === nomV2) : undefined;
              const cV = bV ? bV.pts.localToWorld(bV.centre.clone()) : null;
              const scM = nM?.mesh.scale.x || 1;
              const scI = it?.grp.scale.x || 1;
              const mNdc = cM ? proj(cM) : null;
              const vNdc = cV ? proj(cV) : null;
              const dans = (p: THREE.Vector3 | null) => !!p && Math.abs(p.x) <= 1 && Math.abs(p.y) <= 1;
              (etagesRef.current as Record<string, unknown>).cadrage = {
                monde_ndc: mNdc ? [+mNdc.x.toFixed(2), +mNdc.y.toFixed(2)] : null,
                monde_rayon_ecran: cM ? +rayonEcran(cM, (layout.orbits.get(qn)?.size ?? 30) * scM).toFixed(3) : null,
                groupe_ndc: vNdc ? [+vNdc.x.toFixed(2), +vNdc.y.toFixed(2)] : null,
                groupe_rayon_ecran: cV && bV ? +rayonEcran(cV, bV.rayon * scM * scI).toFixed(3) : null,
                graphe_rayon_ecran: cV && gr.echelle != null
                  ? +rayonEcran(cV, (layout.orbits.get(qn)?.size ?? 30) * 1.15 * (gr.echelle * scI)).toFixed(3)
                  : null,
                niveaux_ensemble: dans(mNdc) && dans(vNdc),
              };
            }
            // ── LE CONTENU EN RETARD SUR LE GESTE : ON LE DIT ───────────────
            // Entre le clic et le chargement du nouveau graphe (mesuré : 4,4 à
            // 18,7 s), les nœuds affichés appartiennent encore au groupe
            // PRÉCÉDENT — ils sont donc dans une autre poupée que celle qu'on
            // vient de désigner. Les laisser à pleine intensité ferait croire à
            // deux sélections ; on les met en attente, franchement en retrait.
            // (Les deux noms sont recalculés ICI : le bloc intérieur qui les
            // déclare est un périmètre fermé — tsc l'a arrêté, et c'est tant mieux.)
            const enAttente = (() => {
              const a = groupeVise(qn);
              const b2 = groupeCharge(qn);
              return !!a && !!b2 && a !== b2;
            })();
            const kAttente = enAttente ? 0.22 : 1;
            gr.grp.children.forEach((ch) => {
              const mat = (ch as THREE.Mesh).material as THREE.Material & { opacity?: number };
              if (!mat) return;
              if (mat.transparent === false) { mat.transparent = true; }
              const base = ch.type === "LineSegments" ? 0.35 : 0.12;
              if (typeof mat.opacity === "number") {
                if ((ch as THREE.Mesh).type === "Points" || ch.type === "Points") {
                  const u = (mat as THREE.ShaderMaterial).uniforms?.uOpacite;
                  if (u) u.value = 0.95 * ouvG * kAttente;
                } else {
                  mat.opacity = base * ouvG * 3 * kAttente;
                }
              }
            });
          }
          // ── EMPHASE : ce qui ressort s'illumine, le reste baisse ───────
          // Trois sources se composent, dans cet ordre de priorité :
          //   1. la sélection de GROUPE (par type, quel qu'il soit) ;
          //   2. le CONTEXTE de l'engine : les nœuds que le modèle a lus
          //      s'allument — « voir le contexte s'illuminer dans le graph » ;
          //   3. l'absence de sélection : tout à égalité.
          {
            const pts = gr.grp.children.find((ch) => (ch as THREE.Points).isPoints) as THREE.Points | undefined;
            const infos = gr.noeudsInfo;
            if (pts && infos) {
              const attr = pts.geometry.getAttribute("aEmphase") as THREE.BufferAttribute;
              const arr = attr.array as Float32Array;
              const filtre = filtreRef.current;
              // « Afin qu'on distingue facilement ce qui ressort de l'ensemble » :
              // baisser les seuls nœuds non sélectionnés ne suffit PAS — mesuré,
              // 70 nœuds atténués ne changeaient la lumière de la zone que de
              // 0,15 %, noyés par l'univers qui continue de briller. Quand une
              // sélection est active, le fond recule donc AUSSI : la nappe, la
              // toile et les autres mondes passent en retrait.
              const retrait = filtre ? 0.28 : 1;
              (sheet.material as THREE.ShaderMaterial).uniforms.uOpacite.value =
                0.09 * (1 - 0.85 * t) * retrait;
              scene.children.forEach((o: any) => {
                if (o.isPoints && o !== pts && !gr.grp.getObjectById(o.id)) {
                  const m = o.material as THREE.PointsMaterial;
                  if (!m || !("opacity" in m)) return;
                  // On mémorise l'opacité de REPOS la première fois et on la
                  // RÉTABLIT, on ne multiplie pas : la version précédente
                  // faisait `opacity *= 0.3` à chaque image, donc l'opacité
                  // s'effondrait vers zéro en une fraction de seconde — un bug
                  // que seule la mesure a montré.
                  if (o.userData.opaciteRepos === undefined) {
                    o.userData.opaciteRepos = m.opacity ?? 1;
                  }
                  m.opacity = (o.userData.opaciteRepos as number) * (filtre ? 0.3 : 1);
                }
              });
              const ctx = new Set(
                (activationRef.current?.hits || [])
                  .filter((h) => h.energy > 0.12)
                  .map((h) => h.qn),
              );
              // ── LA PUCE DE FILTRE ALLUME-T-ELLE VRAIMENT ? ────────────────
              // C'est LE défaut que la règle unique répare : la puce annonçait
              // « msg_zerocopy » (étiquette filtrée sur `__file__`) pendant que
              // `infos[i].type` valait « rst » (avant-dernier segment brut), et
              // la comparaison ci-dessous ne pouvait donc RIEN allumer. On
              // compte ce que la RÈGLE allume — avant le bonus de contexte, qui
              // fausserait la mesure — et on l'exporte : `allumes` doit valoir
              // l'effectif annoncé par la puce.
              let allumesParLaRegle = 0;
              for (let i = 0; i < infos.length; i++) {
                let e = 0.16; // tout ce qui ne ressort pas
                if (filtre) {
                  if (infos[i].type === filtre) { e = 1; allumesParLaRegle++; }
                } else {
                  e = 1;
                }
                if (ctx.has(infos[i].qn)) e = Math.max(e, 1); // contexte lu
                arr[i] = e;
              }
              if (filtre) {
                (etagesRef.current as Record<string, unknown>).filtre = {
                  etiquette: filtre, allumes: allumesParLaRegle, total: infos.length,
                };
              } else {
                // Aucun filtre : on ne laisse pas traîner la mesure précédente
                // (un instrument qui garde une valeur périmée fait lire un état
                // qui n'existe plus).
                (etagesRef.current as Record<string, unknown>).filtre = {
                  etiquette: null, allumes: infos.length, total: infos.length,
                };
              }
              attr.needsUpdate = true;
              // Les arêtes suivent : une arête ne se lit que si ses deux bouts
              // ressortent — sinon elle brouille ce qu'on veut distinguer.
              const lignes = gr.grp.children.find((ch) => ch.type === "LineSegments") as THREE.LineSegments | undefined;
              if (lignes && gr.paires) {
                const col = lignes.geometry.getAttribute("color");
                if (!col) {
                  const n = gr.paires.length * 2;
                  lignes.geometry.setAttribute("color", new THREE.BufferAttribute(new Float32Array(n * 3).fill(1), 3));
                }
                const ca = lignes.geometry.getAttribute("color") as THREE.BufferAttribute;
                const carr = ca.array as Float32Array;
                gr.paires.forEach(([a, b], k) => {
                  const e = Math.min(arr[a], arr[b]);
                  for (let c = 0; c < 3; c++) {
                    carr[k * 6 + c] = 0.25 + 0.75 * e;
                    carr[k * 6 + 3 + c] = 0.25 + 0.75 * e;
                  }
                });
                ca.needsUpdate = true;
                (lignes.material as THREE.LineBasicMaterial).vertexColors = true;
                (lignes.material as THREE.LineBasicMaterial).needsUpdate = true;
              }
            }
          }

          // ── MIGRATION VERS L'ESPACE DES SIGNAUX ───────────────────────
          // Les nœuds ET leurs arêtes voyagent ensemble : une arête qui reste
          // plantée pendant que ses extrémités partent ne veut rien dire.
          if (montrableG && gr.posGraphe && gr.posSignaux) {
            const pts = gr.grp.children.find((ch) => (ch as THREE.Points).isPoints) as THREE.Points | undefined;
            const lignes = gr.grp.children.find((ch) => ch.type === "LineSegments") as THREE.LineSegments | undefined;
            if (pts) {
              const attr = pts.geometry.getAttribute("position") as THREE.BufferAttribute;
              const arr = attr.array as Float32Array;
              for (let i = 0; i < arr.length; i++) {
                arr[i] = gr.posGraphe[i] * (1 - tS) + gr.posSignaux[i] * tS;
              }
              attr.needsUpdate = true;
            }
            // Les triangles décrivent la disposition « graphe » : en espace des
            // signaux ils n'ont plus de sens, on les efface plutôt que de les
            // laisser mentir.
            gr.grp.children.forEach((ch) => {
              if (ch.type === "Mesh") (ch as THREE.Mesh).visible = tS < 0.3;
            });
            if (lignes && gr.paires) {
              const la = lignes.geometry.getAttribute("position") as THREE.BufferAttribute;
              const larr = la.array as Float32Array;
              gr.paires.forEach(([a, b], k) => {
                for (let c = 0; c < 3; c++) {
                  larr[k * 6 + c] = gr.posGraphe![a * 3 + c] * (1 - tS) + gr.posSignaux![a * 3 + c] * tS;
                  larr[k * 6 + 3 + c] = gr.posGraphe![b * 3 + c] * (1 - tS) + gr.posSignaux![b * 3 + c] * tS;
                }
              });
              la.needsUpdate = true;
            }
          }
          // Les ensembles s'effacent SEULEMENT si le graphe est construit —
          // et ils NE DISPARAISSENT PAS : mesuré, `(1 - ouvG)` les annulait
          // complètement au niveau le plus profond, si bien que la cellule se
          // vidait de sa matière et qu'il ne restait que quelques centaines de
          // points de graphe. On est DANS un lieu, pas devant une poignée de
          // grains : l'organisation recule à 30 %, elle reste le tissu du lieu.
          if (it && montrableG) {
            it.grp.children.forEach((ch) => {
              const mat = (ch as THREE.Mesh).material as THREE.Material & { opacity?: number };
              if (!mat || typeof mat.opacity !== "number") return;
              // ⚠ ON RÉTABLIT, ON NE MULTIPLIE PAS. La version `*= (1 - 0,7·ouvG)`
              // tournait À CHAQUE IMAGE : l'opacité était donc multipliée par 0,3
              // soixante fois par seconde et la matière s'éteignait en quelques
              // dixièmes de seconde. Mesuré à la capture : la scène se vidait à
              // mesure qu'on entrait — 87,6 % de contenu à « monde 48 % », plus
              // que 13,8 % à « dedans 100 % ». C'est le même piège que pour le
              // fond : une opacité qui dépend d'un facteur CONTINU doit être
              // POSÉE, jamais accumulée.
              if (ch.userData.opaciteRepos === undefined) {
                ch.userData.opaciteRepos = mat.opacity;
              }
              mat.opacity = (ch.userData.opaciteRepos as number) * (1 - 0.7 * ouvG);
            });
          }
        }
        if (it) {
          // LES GROUPES DOIVENT SE VOIR AVANT D'ÊTRE DEDANS.
          //
          // Constat (utilisateur) : « la décomposition en sous-groupes devrait se
          // faire avant vis-à-vis de mon zoom, sinon je suis dans le cœur du
          // monde avant de voir les groupes qui le composent ». Mesuré dans le
          // code : l'intérieur n'apparaissait qu'à t = 0,56 — soit APRÈS avoir
          // franchi la paroi. Autrement dit on traversait la coquille sans avoir
          // jamais vu de quoi elle est faite : l'ordre de la descente était
          // inversé par rapport à la compréhension.
          // On ouvre donc la décomposition dès 0,20, et elle est complète à 0,44 :
          // on voit les groupes pendant qu'on approche, pas après être entré.
          // FENÊTRE 0,15 -> 0,35 : les groupes, AVANT le cœur du monde.
          // ── LE DÉTAIL PAR PROXIMITÉ, PAS PAR FRACTION DE COURSE ──────────
          // Le seuil était une fraction de descente (`t`) : la décomposition
          // s'ouvrait donc au même instant quelle que soit la taille du monde —
          // et jamais PENDANT qu'on approche. On le pilote par la DISTANCE RÉELLE
          // à la cible, rapportée au rayon du monde : le groupe se détaille sous
          // les yeux pendant le zoom, et d'autant plus tôt que le monde est grand.
          // ── LA PROXIMITÉ DE CE MONDE-LÀ, PAS CELLE DE LA CIBLE ──────────
          // Les rampes utilisaient la distance à la CIBLE (le monde visé, ou
          // l'objet verrouillé) : un SEUL monde se détaillait — celui qu'on
          // visait — et il fallait l'avoir cliqué pour ça. Or on voit plusieurs
          // mondes à la fois : ce sont EUX qui doivent se composer, chacun selon
          // SA distance à la caméra. C'est la seule formulation qui rende le
          // détail CONTINU : on n'entre pas dans un monde pour le voir se
          // composer, il se compose parce qu'il s'approche.
          //
          // Et le seuil est BIEN plus tôt qu'avant (14 rayons au lieu de 7) :
          // mesuré à l'usage, la composition arrivait encore après le recalibrage
          // du clic sur un monde. Elle doit être là AVANT.
          // ── LA BONNE UNITÉ : LA TAILLE À L'ÉCRAN, PAS LES RAYONS ─────────
          // Les seuils étaient en RAYONS DE MONDE (14 puis 5) : ils ignoraient
          // donc la taille de l'écran. Sur un téléphone, un monde à 14 rayons est
          // DÉJÀ ÉNORME — on était dedans avant que la composition n'apparaisse.
          // C'est la TAILLE APPARENTE qui compte, elle, indépendante de
          // l'appareil : un monde doit se composer quand il occupe une part
          // donnée de l'écran. Et comme un écran étroit contraint la LARGEUR avant
          // la hauteur, on prend la dimension la plus contraignante — sinon le
          // mobile déclencherait toujours plus tard que le bureau.
          // `ouv` est calculé EN TÊTE DE BOUCLE (voir plus haut) : on le réutilise
          // au lieu de le recalculer sur une autre grandeur. Une seule autorité.
          const montrable = it.pret && ouv > 0;
          it.grp.visible = montrable;
          // ── FONDU CROISÉ : LE MONDE S'EFFACE QUAND SA COMPOSITION APPARAÎT ──
          //
          // Sans lui les deux se SUPERPOSENT et le contenu saute d'un coup.
          // Mesuré après avoir ouvert les groupes plus tôt : +33 points en UNE
          // SEULE étape de descente, et les ruptures passaient de 1 à 4. Ouvrir
          // plus tôt sans effacer le monde ne fait que déplacer le saut : c'est
          // la transition qu'il faut travailler, pas seulement le seuil.
          //
          // On POSE l'opacité, on ne la multiplie pas (motif `opaciteRepos`) :
          // accumulée à chaque image, elle s'éteint en quelques dixièmes de
          // seconde — le bug d'opacité déjà constaté dans ce fichier.
          {
            // `wd` est typé possiblement indéfini dans cette portée : on garde
            // la garde explicite plutôt qu'un `!` — une opacité appliquée au
            // mauvais monde serait invisible et donc indétectable.
            const meshMonde = wd ? worldNodes.get(wd.qn)?.mesh : undefined;
            if (meshMonde) {
              for (const o of [meshMonde, ...meshMonde.children]) {
                const mat = (o as THREE.Mesh).material as
                  | (THREE.Material & { opacity?: number; uniforms?: { uOpacite?: { value: number } } })
                  | undefined;
                if (!mat) continue;
                if (mat.uniforms?.uOpacite) {
                  if (o.userData.opaciteMonde === undefined) {
                    o.userData.opaciteMonde = mat.uniforms.uOpacite.value;
                  }
                  mat.uniforms.uOpacite.value =
                    (o.userData.opaciteMonde as number) * (1 - 0.85 * ouv);
                } else if (typeof mat.opacity === "number") {
                  if (o.userData.opaciteMonde === undefined) {
                    o.userData.opaciteMonde = mat.opacity;
                  }
                  mat.opacity = (o.userData.opaciteMonde as number) * (1 - 0.85 * ouv);
                }
              }
            }
          }
          if (montrable) {
            it.grp.scale.setScalar((0.55 + 0.45 * ouv) * contractionInterne);
            it.grp.children.forEach((ch) => {
              const mat = (ch as THREE.Mesh).material as THREE.Material & { opacity: number };
              if (!mat) return;
              const base = ch.type === "Line" ? 0.75 : 1;
              mat.opacity = ouv * base;
              mat.transparent = true;
            });
          }
          // La composition de surface s'efface SEULEMENT si l'intérieur existe.
          node.mesh.children.forEach((ch) => {
            if ((ch as THREE.Points).isPoints) {
              const pm = (ch as THREE.Points).material as THREE.ShaderMaterial;
              const base = on ? 0.96 : Math.max(0.05, 0.85 * (1 - 0.9 * t));
              // ── LA MATIÈRE DU MONDE RESTE COMME SUBSTANCE DE LA CELLULE ──
              // Mesuré : la nuée du monde s'éteignait entièrement (1 - ouv) quand
              // l'étage intérieur arrivait, et l'intérieur n'est qu'une COQUILLE
              // CREUSE de six amas : au centre, on regardait dans les trous entre
              // eux. Composition à « dedans 100 % » : 14 % de contenu contre
              // 87,6 % à « monde 48 % » — la scène se VIDait à mesure qu'on
              // entrait. La matière du monde demeure donc à 30 % : dans n'importe
              // quelle direction, on voit de la substance — c'est ce qui fait
              // qu'on est DANS un lieu plutôt que devant des objets épars.
              if (pm.uniforms?.uOpacite) {
                pm.uniforms.uOpacite.value = montrable ? base * (1 - 0.7 * ouv) : base;
              }
            }
          });
        }
        const m = node.mesh.material as THREE.MeshBasicMaterial;
        // un monde éteint garde sa couleur ; consulté, il blanchit vers l'or.
        m.color.copy(new THREE.Color(PLAN_COLOR[wd?.nature as keyof typeof PLAN_COLOR] || 0x8fa6bd));
        if (e > 0.01) m.color.lerp(new THREE.Color(ACCENT), Math.min(0.7, e));
        // Le nuage porte la composition réelle (couleurs par type) : on ne le
        // teinte pas, on le fait GROSSIR et s'illuminer par transparence.
        node.mesh.children.forEach((ch) => {
          if ((ch as THREE.Points).isPoints) {
            const pm = (ch as THREE.Points).material as THREE.PointsMaterial;
            pm.size = 8 * (1 + 0.9 * e);
            pm.opacity = 0.96;
          }
        });
      });

      // La nappe d'espace-temps s'efface aussi : à l'échelle d'un monde, elle
      // n'est plus l'information utile.
      (sheet.material as THREE.ShaderMaterial).uniforms.uOpacite.value = 0.09 * (1 - 0.85 * t);

      // pulsation de l'étoile
      const pulse = 1 + 0.06 * Math.sin(now * 0.0022);
      // On n'anime plus un objet caché : la pulsation ne servait qu'à lui.
      star.visible = false;
      halo.visible = false;
      void pulse;
      ctrl.update();
      renderer.render(scene, camera);
    };
    loop();

    const onResize = () => {
      camera.aspect = el.clientWidth / el.clientHeight;
      camera.updateProjectionMatrix();
      renderer.setSize(el.clientWidth, el.clientHeight);
    };
    window.addEventListener("resize", onResize);
    return () => {
      cancelAnimationFrame(raf);
      detachKeys();
      window.removeEventListener("resize", onResize);
      renderer.domElement.removeEventListener("pointermove", onMove);
      renderer.domElement.removeEventListener("click", onClick);
      renderer.domElement.removeEventListener("dblclick", onDblClick);
      renderer.domElement.removeEventListener("wheel", onWheel);
      renderer.domElement.removeEventListener("touchstart", onTouchStart);
      renderer.domElement.removeEventListener("touchmove", onTouchMove);
      renderer.domElement.removeEventListener("touchend", onTouchEnd);
      ctrl.dispose();
      // (2026-09-15, zero fuite) La scene se reconstruit a chaque entree de
      // monde / rechargement : sans disposal, CHAQUE reconstruction fuyait
      // l integralite des geometries, materiaux et textures (166 allocations
      // par scene) — la pression GC croissait et les pauses avec elle.
      scene.traverse((o: any) => {
        if (o.geometry) o.geometry.dispose();
        const mats = Array.isArray(o.material) ? o.material : o.material ? [o.material] : [];
        for (const m of mats) {
          for (const k of Object.keys(m)) {
            const v = (m as any)[k];
            if (v && v.isTexture) v.dispose();
          }
          m.dispose();
        }
      });
      scene.clear();
      el.removeChild(renderer.domElement);
      renderer.dispose();
    };
  }, [weave, layout, onOpen, activePath]);


  // (2026-09-15) TOUS LES HOOKS AVANT LES REPLIS. `montre` vivait APRES les
  // deux returns conditionnels ci-dessous : au premier rendu il s executait,
  // puis `err` arrivait (compte sans base : requetes refusees), le repli
  // retournait tot, le hook etait saute — React #300, ecran d erreur pour
  // tout compte vierge. Un hook ne vit jamais apres un return conditionnel.
  const montre = useAtomValue(montreAtom);

  // ── repli : la toile n'est pas encore tissée → ancienne projection ──
  if (weave && weave.available === false) {
    return (
      <div className="overview">
        <StageContinu mode="cosmos" activePath={activePath} onBase={onOpen} />
        <div className="ov-panel" style={{ left: 12, right: "auto", top: 58 }}>
          <p className="q-intro" style={{ margin: 0 }}>
            <b>Toile non tissée.</b> {weave.reason}
          </p>
        </div>
      </div>
    );
  }
  if (err) return <div className="overview"><p style={{ color: "#ff9e8a" }}>Cosmos : {err}</p></div>;

  const steps = weave?.densification?.steps || [];
  const step = steps.length
    ? steps[Math.min(steps.length - 1, Math.round(density * (steps.length - 1)))]
    : null;
  const t0 = weave?.clock?.t0 || 0, t1 = weave?.clock?.t1 || 0;
  const at = step ? new Date(step.t) : null;
  const span = Math.max(1, t1 - t0);

  // ── LES POUPÉES, EN CLAIR ─────────────────────────────────────────────────
  // Le monde courant est celui dont l'organisation est chargée (`interieur.qn`),
  // sinon celui qu'on vise. Le groupe est celui qu'on a DÉSIGNÉ (`vise`), sinon
  // celui dont le graphe est affiché. Les deux autorités restent distinctes :
  // l'encart doit nommer ce qu'on voit, pas ce qu'on espère.
  // (déplacé avant le retour anticipé — voir `montre` plus haut)
  const mondePoupee = (() => {
    const qn = interieur?.qn ?? focusQn;
    if (!qn) return null;
    const w = (weave?.worlds || []).find((x) => x.qn === qn);
    return { qn, label: (w?.label || "monde").replace(/ \(.*\)/, "") };
  })();
  const groupePoupee = vise?.genre === "ensemble" ? vise.nom : (graphe?.centre ?? null);

  return (
    <div className="overview">
      <div ref={host} style={{ position: "absolute", inset: 0 }} />
      {/* LES PANNEAUX DANS UNE COLONNE À PART.
          Constat mesuré : la feuille de style pose `position:absolute; left:14px;
          top:64px` pour TOUS les panneaux. Sur bureau, mes styles en ligne les
          différenciaient ; sur mobile (390×844) ils se retrouvaient à y=56, y=496
          et y=784 — dispersés, le dernier collé au bord bas, sans aucune logique.
          Ils vivent maintenant dans une colonne qui gère leur empilement, au lieu
          de dépendre d'un décalage fixe qui n'a de sens que sur grand écran. */}
      {/* ── LA COUCHE DE COMPRÉHENSION EST TOUJOURS VISIBLE ────────────────────
           Constat mesuré : `ViewPanel` se replie PAR DÉFAUT sur mobile
           (`return mode() === "desktop"`), donc `children` n'est pas rendu — la
           légende, le fil d'étage et la sélection disparaissaient purement et
           simplement de l'écran. Vérifié : « (aucune légende) » dans les trois
           dimensions. Sur un téléphone, on ne voyait donc QUE le canevas, sans
           savoir où l'on est ni ce que les formes signifient : c'est cela,
           l'affichage incohérent.
           Ces deux repères sortent du panneau repliable : ils restent visibles à
           toutes les échelles et sur tous les écrans. */}
      {/* ── LE FIL DES POUPÉES : RANGÉ SOUS LE CHROME, ET REPLIABLE ───────────
         Mesuré (reconnaissance) : `.reperes` n'avait AUCUNE règle CSS — tout en
         style inline —, il se posait à `top:62`, donc SOUS le sélecteur de vues
         (`top:54`, `z-index:41` en mobile), et rien ne pouvait le replier. C'était
         le premier poste d'occupation de la vue Cosmos sur téléphone.
         Il obéit maintenant à la même règle que le reste du chrome
         (`montre("fil")`) et se place à partir de l'empreinte publiée
         (`--chrome-top`) : il ne peut plus passer sous autre chose. */}
      {montre("fil") && (
      <div className="reperes" style={{
        position: "absolute", left: 12, right: 12,
        top: "var(--chrome-top, 62px)", zIndex: 13,
        pointerEvents: "none", display: "flex", flexWrap: "wrap", gap: 6,
      }}>
        <div style={{ pointerEvents: "auto", maxWidth: "100%" }}>
          {/* ── LE FIL DES POUPÉES : cosmos › monde › groupe › nœuds ──────────
              « La vue doit représenter un groupe DANS un monde — ça doit
              s'emboîter comme des poupées russes. » La scène est une descente :
              elle montre un étage à la fois, parce qu'un étage y est défini par
              la proximité. Le fil qui manquait n'était donc pas dans la 3D mais
              ici : chaque poupée nommée, celle où l'on se trouve allumée, et
              les autres à un clic de distance. Il vit dans `reperes`, hors des
              panneaux repliables — donc visible aussi sur téléphone. */}
          <LevelCartouche levels={[
            { label: "Cosmos", icon: "🌌", current: !focusQn && niveauScene === 0,
              onClick: () => { setFocusQn(null); retourRef.current?.(); } },
            ...(mondePoupee ? [{
              label: mondePoupee.label, icon: "🪐",
              current: !!focusQn && niveauScene <= 1,
            }] : []),
            ...(groupePoupee ? [{
              // Pas de compteur ici : `🪆 aletheia (6)` se lisait « 6 nœuds »
              // alors que 6 était le nombre d'ensembles DU MONDE. La taille du
              // groupe est celle de son graphe, juste à côté : un chiffre, une
              // seule signification.
              label: groupePoupee, icon: "🪆",
              current: niveauScene === 1,
              // Un clic RAMÈNE la poupée dans le champ : le fil sert aussi à
              // remonter, pas seulement à regarder.
              onClick: mondePoupee
                ? () => cadrerGroupeRef.current?.(mondePoupee.qn, groupePoupee)
                : undefined,
            }] : []),
            ...(graphe ? [{
              label: `${graphe.noeuds} nœuds · ${graphe.aretes} arêtes`, icon: "🔺",
              current: niveauScene === 2,
            }] : []),
            /* ── LA PROFONDEUR LE LONG DE L'AXE, ENFIN VISIBLE ────────────────
               Au dernier cran, « descendre » n'éloigne plus : il ENTRE dans le
               groupe que l'axe désigne, par son nœud le plus important, et le
               moteur rend le voisinage de ce nœud — un autre graphe, re-découpé
               par le même axe. C'est une profondeur réelle, et elle était
               INVISIBLE : rien ne disait dans quel nœud on était entré, ni combien
               de fois, alors que chaque pas remplace la matière affichée.
               Ici, chaque entrée est une poupée de plus, nommée par son axe, et
               cliquable pour y revenir — le fil sert à remonter, comme les autres. */
            ...cheminAxe.map((pas, i) => ({
              label: `${pas.noeud.split(/[.:]/).slice(-1)[0] || pas.noeud}`,
              icon: "🎯",
              current: i === cheminAxe.length - 1,
              title: `entré dans le groupe « ${pas.groupe} » de l'axe « ${pas.axe} » par le nœud ${pas.noeud}`,
              onClick: mondePoupee
                ? () => { setVise({ genre: "noeud", nom: pas.noeud, detail: `retour au voisinage de ${pas.noeud}` }); chargerNoeudRef.current?.(mondePoupee.qn, pas.noeud); }
                : undefined,
            })),
          ]} />
        </div>
        <div style={{ pointerEvents: "none", maxWidth: "100%" }}>
          <PlanLegend caption={<>Taille = importance réelle · halo = énergie · couleur = <b>famille</b> (premier segment du nom), pas un index.</>} />
        </div>
        {/* ── MON UNIVERS, EN UNE LIGNE ──────────────────────────────────────
             Ma première version listait les mondes ici : MESURÉ, la barre est
             montée à 283 px sur un écran de 844 — un tiers du téléphone mangé par
             l'affichage, alors que le principe de cette vue est « le canevas
             d'abord ». C'est la même faute que les panneaux repliés, en sens
             inverse : j'ai remplacé du vide par de l'encombrement.
             Ici : l'ÉTAT de mon univers en chiffres — présent, lisible, non
             envahissant. Le DÉTAIL (quels mondes) vit dans le panneau, où il y a
             la place, et s'ouvre quand on le demande. */}
        <div style={{ pointerEvents: "auto", maxWidth: "100%", display: "flex",
                      flexWrap: "wrap", gap: 6, alignItems: "center" }}>
          {([["connaissance", "🗺", "ma base", "ouvrir ma base"],
             ["self", "🧠", "mon IA", "ouvrir le self de mon IA"],
             ["utilisateur", "👤", "moi", "ouvrir mon monde"]] as const).map(([nat, ic, court, quoi]) => {
            const ws = (weave?.worlds || []).filter((w) => w.nature === nat);
            const fourchette = ws.slice(0, 4).map((w) => w.label.replace(/ \(.*\)/, "")).join(" · ");
            return (
              <button key={nat} className="chip"
                title={`${quoi} — ${ws.length} monde(s)${fourchette ? " : " + fourchette : " (aucun monde tissé)"}`}
                onClick={() => {
                  if (!ws.length) return;
                  setFocusQn(ws[0].qn);
                  viserFocusRef.current?.({ kind: "monde", qn: ws[0].qn, label: ws[0].label, detail: `${ws[0].nodes} nœuds` });
                }}
                style={{ opacity: ws.length ? 1 : 0.5 }}>
                {ic} {court} <b style={{ opacity: 0.85 }}>{ws.length}</b>
              </button>
            );
          })}
        </div>
        {/* ── BARRE D'ÉCHELLE — TOUJOURS VISIBLE, HORS DU PANNEAU QUI DÉFILE ────
             MESURÉ sur la page servie, et c'est le défaut le plus cher de cette
             vue : dans une fenêtre de 757 px de haut — celle de tout portable, et
             celle des contrôles de ce dépôt — le panneau « Cosmos » porte 1251 px
             de contenu pour 588 px visibles (il défile : `scrollHeight` 1251,
             `clientHeight` 588). TOUT LE BAS du panneau est donc sous la ligne de
             flottaison, et c'est précisément là que vivaient le repère d'échelle
             (« Descente : X % ») et le seul bouton de descente à la souris.
             Constat direct : « ⤓ plonger » mesuré à y = 805 pour une fenêtre de
             757 → `elementFromPoint` répondait « HORS ÉCRAN », et le clic ne
             produisait rien. Redonné à la même application une fenêtre de 1200 px,
             le même clic descendait (`t` 0 → 0,6338, distance 1141 → 61,58).
             Ce n'était donc pas la descente qui était cassée : c'était son bouton,
             hors de l'écran.
             Ces repères rejoignent `reperes` — le fil des poupées et la légende y
             sont déjà, justement parce qu'« ils restent visibles à toutes les
             échelles ». Même raison, même endroit. */}
        <div className="echelle-bar">
          <span className="eb-niv" title="Où en est la caméra entre l'espace et le dedans">
            <b>Descente</b> : {PALIERS[palier]}
            {descente > 0 ? ` — ${(descente * 100).toFixed(0)} %` : " — 0 %"}
          </span>
          <span className="eb-jauge" title="Le même chiffre, en longueur">
            <i style={{ width: `${Math.round(descente * 100)}%` }} />
          </span>
          <button className="chip" onClick={() => zoomProfondRef.current?.()}
            title={palier >= 3
              ? `Entrer dans le plus gros groupe de l'axe « ${dimension} » par son nœud le plus important — le moteur rend alors le voisinage de ce nœud, re-découpé par le même axe`
              : "Descendre d'un cran : espace → monde → groupe → signaux (le monde visé, puis le groupe désigné, puis le cœur)"}>
            ⤓ descendre
          </button>
          {/* ── DENSIFIER — LE GESTE QUI RÉPOND À « CE QUI MANQUE ICI » ───────
              DÉFAUT MESURÉ : la barre ne portait QU'UN geste de « plus », et il
              descend d'un CRAN. Un niveau qui portait 200 groupes n'en montrait
              que 24, et la seule façon de voir les autres était de changer
              d'échelle — c'est-à-dire de ne pas les voir au niveau où ils vivent.
              Ici, la loi (`densite.ts`) décide : tant qu'il reste de la matière
              REÇUE et non dessinée, densifier est offert À CÔTÉ d'approfondir, et
              le compte annoncé dit ce qu'il rapporte. */}
          {interieur?.decision?.geste === "densifier" && (
            <button className="chip" onClick={() => densifierNiveauRef.current?.(interieur.qn)}
              title={interieur.decision.requiertAppel
                ? `Densifier le MÊME niveau, en demandant la page suivante AU SERVEUR (sa page de ${PAGE.plafondAppel} est pleine) — préfixe, rang et graphe INCHANGÉS`
                : `Densifier le MÊME niveau : dessiner ${Math.min(interieur.total, interieur.n + PAGE.pas)} ensembles au lieu de ${interieur.n} (${interieur.total} reçus) — préfixe, rang et graphe INCHANGÉS, aucun appel : la matière est déjà là`}>
              ＋ densifier {interieur.decision.dePlus}{interieur.decision.requiertAppel ? " (serveur)" : ""}
            </button>
          )}
          {/* La loi dit aussi l'INVERSE : quand le niveau est entièrement montré,
              le seul « plus » est un cran plus bas — et on le dit, au lieu de
              laisser « ⤓ descendre » muet sur ce qu'il reste derrière soi. */}
          {interieur?.decision?.geste === "approfondir" && interieur.total > 0 && (
            <span className="eb-attente" title={phraseDecision({
              total: interieur.total, montres: interieur.n,
              feuille: interieur.feuille, plafondAtteint: interieur.plafondAtteint,
            }, interieur.decision)}>
              niveau entier ({interieur.n}) → approfondir
            </span>
          )}
          <button className={fige ? "chip on" : "chip"} onClick={() => setFige((f) => !f)}
            title={fige ? "Reprendre l'animation" : "Figer la scène pour la lire"}>
            {fige ? "▶" : "⏸"}
          </button>
          {/* CE QU'ON DESCEND, ET SI L'ÉTAGE SUIVANT EST PRÊT.
              Un geste de descente qui ne bouge pas doit dire POURQUOI : ici, ce
              qui est visé et l'état réel de ses chargements. */}
          {vise && <span className="eb-vise" title={vise.detail}>→ {vise.nom}</span>}
          {!interieur && <span className="eb-attente">organisation…</span>}
          {interieur && !graphe && <span className="eb-attente">graphe…</span>}
          {/* Un geste refusé doit DIRE pourquoi : tant qu'un chargement du même
              mode est en vol, « ⤓ descendre » ne peut pas partir. */}
          {chargementEnVol && <span className="eb-attente">graphe en cours…</span>}
          {/* ── L'AXE DE NAVIGATION — DANS LA BARRE, DONC À TOUS LES CRANS ────
               Descendre n'est QU'UN axe. La même matière se découpe aussi par
               communauté du moteur (Leiden), par courbure (bords ∂ contre
               intérieur), par parenté, par extension, par strate. Ces découpages
               existaient déjà — ils COLORAIENT et FILTRAIENT — mais ils vivaient
               dans le panneau qui défile, n'apparaissaient qu'une fois un graphe
               chargé, et ne changeaient rien à la descente.
               Ici : l'axe est visible en permanence, il dit combien de groupes il
               distingue, et le plus gros groupe de l'axe s'ENTRE d'un clic (cadré
               et mis en valeur). « ⤓ descendre » traverse le même axe : au dernier
               cran, il entre dans ce groupe par son nœud le plus important. */}
          <span className="eb-axe">
            <span className="qb-lbl">axe</span>
            {AXES.map((a) => (
              <button key={a.cle} className={"chip" + (dimension === a.cle ? " on" : "")}
                title={a.quoi}
                onClick={() => {
                  dimensionChoisie.current = true;
                  setDimension(a.cle);
                  setFiltreGraphe(null);   // l'ancien groupe n'a plus de sens
                }}>
                {a.court}
              </button>
            ))}
          </span>
          {typesGraphe.length > 0 && (
            <span className="eb-axe-attente" title={
              typesGraphe.length > 1
                ? `L'axe « ${dimension} » distingue ${typesGraphe.length} groupes dans ce graphe`
                : `L'axe « ${dimension} » ne distingue RIEN ici (un seul groupe de ${typesGraphe[0].n} nœuds) : change d'axe pour segmenter`}>
              {typesGraphe.length > 1 ? `${typesGraphe.length} groupes` : "1 groupe — non segmenté"}
              {qnsGraphe.length > 0 ? ` / ${qnsGraphe.length}` : ""}
            </span>
          )}
          {typesGraphe.length > 1 && mondePoupee && (
            <button className={"chip" + (filtreGraphe === typesGraphe[0].type ? " on" : "")}
              title={`Cadrer et mettre en valeur « ${typesGraphe[0].type} » — le plus gros groupe de l'axe « ${dimension} » (${typesGraphe[0].n} nœuds)`}
              onClick={() => entrerAxeRef.current?.(mondePoupee.qn, typesGraphe[0].type)}>
              ⌖ entrer : {typesGraphe[0].type}
            </button>
          )}
        </div>
      </div>
      )}
      <div className="ov-panneaux">
      <ViewPanel title="Cosmos — la toile de l'univers" width={340}
        subtitle="Le centre n'est pas fixe : il dépend de l'instant, de l'agent, de la conversation, du sujet, de l'objectif.">
        {/* (le fil d'étage et la légende ont été SORTIS d'ici — voir plus bas) */}
        <LevelCartouche levels={[
          // Cliquable EN PERMANENCE : au niveau Cosmos elle recadre la vue
          // d'ensemble, en dessous elle remonte — dans les deux cas elle fait
          // quelque chose de visible.
          { label: "Cosmos", icon: "🌌", current: !focusQn,
            onClick: () => { setFocusQn(null); retourRef.current?.(); } },
          ...(focusQn ? [{
            label: (weave?.worlds || []).find((w) => w.qn === focusQn)?.label.replace(/ \(.*\)/, "") || "monde",
            icon: "🪐", current: true,
          }] : []),
        ]} />
        <div className="chips" style={{ marginBottom: 6, flexWrap: "wrap" }}>
          {foci.map((f, i) => (
            <button key={f.kind + i} className={i === focusIdx ? "chip on" : "chip"}
              title={f.detail}
              onClick={() => { setFocusIdx(i); viserFocusRef.current?.(f); }}>
              {f.kind === "instant" ? "⏱" : f.kind === "agent" ? "🧠"
                : f.kind === "utilisateur" ? "👤" : f.kind === "conversation" ? "💬"
                : f.kind === "objectif" ? "🎯" : "🔎"} {f.label.slice(0, 18)}
            </button>
          ))}
        </div>
        {/* DÉTAIL DE MON UNIVERS : le panneau a la place, la barre n'en a pas. */}
        <div style={{ margin: "2px 0 6px" }}>
          {([["connaissance", "🗺", "ma base"], ["self", "🧠", "le self de mon IA"],
             ["utilisateur", "👤", "moi"]] as const).map(([nat, ic, quoi]) => {
            const ws = (weave?.worlds || []).filter((w) => w.nature === nat);
            return (
              <div key={nat} className="q-intro" style={{ marginTop: 2, opacity: 0.9 }}>
                <b>{ic} {quoi}</b>{" · "}
                {ws.length === 0
                  ? <span style={{ opacity: 0.6 }}>aucun monde tissé</span>
                  : ws.map((w, j) => (
                      <span key={w.qn}>
                        {j > 0 ? " · " : ""}
                        <button className="chip" title={`${w.label} — ${w.nodes} nœuds · ${w.edges} arêtes · ${w.path}`}
                          onClick={() => {
                            setFocusQn(w.qn);
                            viserFocusRef.current?.({ kind: "monde", qn: w.qn, label: w.label, detail: `${w.nodes} nœuds` });
                          }}>
                          {w.label.replace(/ \(.*\)/, "").slice(0, 22)}
                        </button>
                      </span>
                    ))}
              </div>
            );
          })}
        </div>
        {focus && <div className="q-intro" style={{ marginTop: 0, opacity: 0.85 }}>{focus.detail}</div>}
        {/* Ce que ←/→ vient de parcourir : sans cet affichage, la navigation
            clavier change la scène sans rien dire à l'utilisateur. */}
        {focusQn && (() => {
          const wd = (weave?.worlds || []).find((w) => w.qn === focusQn);
          if (!wd) return null;
          return (
            <div className="q-intro" style={{ marginTop: 2 }}>
              <b>Monde visé</b> : {wd.label.replace(/ \(.*\)/, "")} —{" "}
              <span style={{ color: PLAN_CSS[wd.nature] || "#9fd6ff" }}>{wd.nature}</span>
              {" · "}↓ entrer
            </div>
          );
        })()}

        {/* LA DENSIFICATION : l'horloge réelle */}
        <div className="chips" style={{ marginTop: 6 }}>
          <button className={playing ? "chip on" : "chip"} onClick={() => setPlaying((v) => !v)}>
            {playing ? "⏸ tissage" : "▶ tisser"}
          </button>
          <span className="qb-lbl">
            {(density * 100).toFixed(0)} % · {step ? step.weight : 0} synapses
          </span>
        </div>
        <input type="range" min={0} max={1} step={0.01} value={density}
          onChange={(e) => { setPlaying(false); setDensity(Number(e.target.value)); }}
          style={{ width: "100%" }} />
        <div className="q-intro" style={{ marginTop: 2 }}>
          {at ? <>Tissage réel : <b>{at.toLocaleString("fr-FR")}</b> —
            {" "}{Math.round(((step?.t || 0) - t0) / span * 100)} % de l'histoire
            ({weave?.clock?.sessions || 0} sessions réelles).</> : null}
        </div>

        <SubjectLine />
        {timeline && (
          <div className="q-intro" style={{ marginTop: 2, fontSize: 11, opacity: 0.85 }}>
            <b>Chargements</b> : {timeline}
          </div>
        )}


        {/* CE QUI EST VISÉ : sans cette ligne, on choisit sans voir son choix. */}
        {vise && (
          <div className="q-intro" style={{ marginTop: 2 }}>
            <b>{vise.genre === "ensemble" ? "Ensemble visé" : vise.genre === "arete" ? "Relation visée" : "Nœud visé"}</b> : {vise.nom}
            {" "}<span style={{ opacity: 0.75 }}>({vise.detail}) — la descente s'oriente dessus</span>
          </div>
        )}
        {/* ÉTAGE INTÉRIEUR : ce que l'organisation de ce monde contient. */}
        {interieur && (
          <div className="q-intro" style={{ marginTop: 2 }}>
            <b>Intérieur</b>
            {/* ── D'OÙ VIENT CE DÉCOUPAGE — ET SUR QUOI PORTENT LES EFFECTIFS ───
                Trois sources, et elles ne se lisent pas de la même façon :
                  · `racines` / `sous-groupes` : l'agrégat du SERVEUR, sur tout le
                    store (le monde entier) ;
                  · `axe` : l'axe choisi, appliqué AU NIVEAU COURANT (chemin ∩
                    classes), donc sur les nœuds du graphe chargé ;
                  · `suivants` : l'axe a saturé (une seule classe) — on continue par
                    le token suivant des nœuds QUI RESTENT, pour que les deux
                    coordonnées descendent ensemble.
                Le panneau dit laquelle : un chiffre sans sa portée est un chiffre
                qu'on croit sur le monde alors qu'il ne parle que d'un échantillon. */}
            {interieur.source === "axe"
              ? <> (axe « {interieur.axe} », au niveau courant)</>
              : interieur.source === "suivants"
                ? <> (token suivant du niveau, après les classes)</>
                : null}
            {" : "}{interieur.n}
            {interieur.total > interieur.n ? ` sur ${interieur.total}` : ""} ensembles —{" "}
            {interieur.top.map((m) => `${m.name} (${m.count})`).join(" · ")}
            {interieur.source === "axe" || interieur.source === "suivants"
              ? <span style={{ opacity: 0.75 }}> · effectifs du <b>graphe chargé</b> ({interieur.total} groupes), pas du monde entier</span>
              : null}
            {/* ── LA DÉCISION, ÉCRITE ICI PARCE QU'ELLE CHANGE LE GESTE ───────
                « 24 sur 200 » ne dit pas quoi FAIRE. La phrase de `densite.ts`
                le dit, et elle est la même que celle de la barre : ce niveau
                réclame une densification (il reste de la matière reçue), ou il
                est épuisé et le seul « plus » est un cran plus bas. */}
            {interieur.decision && (
              <div style={{ opacity: 0.85, marginTop: 2 }}>
                {phraseDecision({
                  total: interieur.total, montres: interieur.n,
                  feuille: interieur.feuille, plafondAtteint: interieur.plafondAtteint,
                }, interieur.decision)}
                {interieur.decision.geste === "densifier" && (
                  <button className="chip" style={{ marginLeft: 6 }}
                    title={interieur.decision.requiertAppel
                      ? `La page de ${PAGE.plafondAppel} est PLEINE : densifier demande la page suivante au serveur (offset ${interieur.total}), au MÊME niveau`
                      : `Dessiner la page suivante de CE niveau (${interieur.n} → ${Math.min(interieur.total, interieur.n + PAGE.pas)}) : préfixe et rang INCHANGÉS, aucun appel — la matière est déjà reçue`}
                    onClick={() => densifierNiveauRef.current?.(interieur.qn)}>
                    ＋ densifier {interieur.decision.dePlus}
                  </button>
                )}
              </div>
            )}
          </div>
        )}
        {/* ── LES FRÈRES DU GROUPE VISÉ, NOMMÉS ET CLIQUABLES ────────────────
            Ils étaient DÉJÀ à l'écran — la boucle garde les autres groupes du
            monde à `ouvI * 0,26`, et `etagesRef` exporte l'opacité réellement
            appliquée (`groupe_frere`) — mais anonymes : aucune étiquette, aucun
            effectif, aucun clic possible. Ici, une puce par boule dessinée
            (même source : `modules`, sans plafond), avec son effectif RÉEL, et
            le total annoncé. Le clic RAMÈNE la boule dans le champ
            (`cadrerGroupeRef`, déjà exposé pour le fil des poupées).
            Rien n'est inventé : le nom et l'effectif viennent de
            `/api/viz/modules`, donc du moteur. */}
        {interieur && interieur.boules.length > 1 && (
          <div className="chips" style={{ marginTop: 4, flexWrap: "wrap" }}>
            {/* ── OU L'ON EN EST DANS LA HIÉRARCHIE, ET COMMENT REMONTER ─────
                L'étage intérieur est un ARBRE (3 racines → 200 enfants → 60
                petits-enfants sur ce store). Sans ce fil, on ne savait ni à quel
                niveau on regardait, ni comment revenir : on croyait que le monde
                n'avait que six groupes. */}
            {interieur.prefixe ? (
              <span className="qb-lbl" style={{ width: "100%" }}>
                {/* ── LE CHEMIN, MAILLON PAR MAILLON, TOUS CLIQUABLES ─────────
                    Descendre de six niveaux puis remonter d'un groupe à la fois
                    est juste mais lent. Chaque maillon du chemin ramène
                    DIRECTEMENT à son niveau : c'est la « capacité de navigation »
                    au sens où on l'entend en pratique — savoir où l'on est, et
                    pouvoir revenir d'un clic à n'importe quel ancêtre. */}
                chemin :{" "}
                <button className="chip" style={{ padding: "2px 7px" }}
                  title="Revenir aux racines de ce monde"
                  onClick={() => allerPrefixeRef.current?.(interieur.qn, "")}>⌂ racines</button>
                {interieur.prefixe.split(".").map((tok, i, tous) => {
                  const p = tous.slice(0, i + 1).join(".");
                  return (
                    <Fragment key={p}>
                      <span style={{ opacity: 0.45 }}>›</span>
                      <button className={"chip" + (i === tous.length - 1 ? " on" : "")}
                        style={{ padding: "2px 7px" }}
                        title={`Revenir au niveau « ${p} »`}
                        onClick={() => allerPrefixeRef.current?.(interieur.qn, p)}>
                        {tok}
                      </button>
                    </Fragment>
                  );
                })}
                {/* ── LA SECONDE COORDONNÉE, DANS LE MÊME FIL ────────────────
                    L'axe n'est pas un réglage d'affichage : chaque classe entrée
                    RESSERRE le niveau courant. Elle se lit donc avec le chemin, et
                    chaque maillon ramène avant elle — comme pour les tokens. */}
                {(interieur.classes || []).map((c, i) => (
                  <Fragment key={`${c.axe}-${i}`}>
                    <span style={{ opacity: 0.45 }}>∩</span>
                    <button className="chip" style={{ padding: "2px 7px" }}
                      title={`Revenir avant « ${c.axe} = ${c.valeur} »`}
                      onClick={() => allerClassesRef.current?.(interieur.qn, i)}>
                      {c.axe}={c.valeur}
                    </button>
                  </Fragment>
                ))}
                {" "}(rang {interieur.rang})
                {" "}<button className="chip" title="Revenir au niveau au-dessus (Page ↑ ou Retour arrière)"
                  onClick={() => remonterGroupeRef.current?.(interieur.qn)}>↥ remonter</button>
              </span>
            ) : (
              <span className="qb-lbl" style={{ width: "100%" }}>
                racines de ce monde : <b>{interieur.boules.length}</b> groupes
                {" "}({interieur.boules.reduce((a, b) => a + b.count, 0).toLocaleString("fr-FR")} nœuds)
                {" "}— cliquer cadre, « ⤵ » entre dans ses sous-groupes (Page ↓ pour le plus gros) ;
                changer d'axe RE-DÉCOUPE CE MÊME NIVEAU (les classes s'ajoutent au chemin)
              </span>
            )}
            {interieur.boules.map((m) => {
              const viseCelui = !!vise && vise.nom === m.name;
              const grapheCelui = graphe?.qn === interieur.qn && graphe?.centre === m.name;
              return (
                <Fragment key={m.name}>
                  <button
                    className={"chip" + (viseCelui ? " on" : "")}
                    title={`${m.name} — ${m.count.toLocaleString("fr-FR")} nœuds réels`
                      + (grapheCelui ? " · graphe affiché" : "")
                      + " — cliquer pour le cadrer"}
                    onClick={() => cadrerGroupeRef.current?.(interieur.qn, m.name)}>
                    {m.name} <i>{m.count.toLocaleString("fr-FR")}</i>
                    {grapheCelui ? " 🔺" : ""}
                  </button>
                  {/* Entrer dans le groupe : le serveur re-découpe le niveau
                      suivant (sous-groupes réels, sur tout le store). Un niveau
                      FEUILLE ne le propose pas — on ne fait pas cliquer dans le
                      vide. */}
                  {!interieur.feuille && (
                    <button className="chip" style={{ padding: "2px 6px" }}
                      title={interieur.source === "axe"
                        ? `Descendre dans la classe « ${m.name} » de l'axe « ${interieur.axe} » AU NIVEAU COURANT : le niveau se resserre, il ne change pas`
                        : `Descendre dans « ${m.name} » : ses propres sous-groupes, agrégés par le serveur`}
                      onClick={() => entrerGroupeRef.current?.(
                        interieur.qn, m.name, interieur.source === "axe" ? "classe" : "token")}>
                      ⤵
                    </button>
                  )}
                </Fragment>
              );
            })}
            {cheminGraphe.length > 1 && (
              <span className="qb-lbl" style={{ width: "100%", marginTop: 2 }}>
                {/* Le trajet du GRAPHE : chaque clic sur un nœud y a ajouté une
                    entrée, et chaque maillon y retourne (mêmes centres). */}
                graphe :{" "}
                {cheminGraphe.map((e, i) => (
                  <Fragment key={`${e.nom}-${i}`}>
                    {i > 0 && <span style={{ opacity: 0.45 }}>›</span>}
                    <button className={"chip" + (i === cheminGraphe.length - 1 ? " on" : "")}
                      style={{ padding: "2px 7px" }}
                      title={e.centres?.length
                        ? `Revenir à cette entrée (${e.centres.length} centres)`
                        : "Revenir à cette entrée"}
                      onClick={() => revenirEntreeRef.current?.(interieur.qn, i)}>
                      {(e.nom.split(/[.:]/).slice(-1)[0] || e.nom).slice(0, 22)}
                    </button>
                  </Fragment>
                ))}
                {" "}({cheminGraphe.length} entrées · Page ↑ revient d'un cran)
              </span>
            )}
            {interieur.feuille && (
              <span className="qb-lbl" style={{ width: "100%", opacity: 0.75 }}>
                ce niveau est une FEUILLE : ses groupes n'ont plus de sous-groupes exploitables
                (seuil {60} nœuds) — les nœuds eux-mêmes sont dans le graphe
              </span>
            )}
          </div>
        )}
        {/* ÉTAGE GRAPHE : le graphe réel, entré par le plus gros ensemble. */}
        {graphe && (
          <div className="q-intro" style={{ marginTop: 2 }}>
            <b>Graphe</b> : {graphe.degrade
              ? <span style={{ color: "#ffb27a" }}>{graphe.motif}</span>
              : <>{graphe.noeuds} nœuds{graphe.total && graphe.total > graphe.noeuds ? ` sur ${graphe.total}` : ""} · {graphe.aretes} arêtes · {graphe.motif}</>}{" "}
            — entré par <b>{graphe.centre}</b>
            {graphe.types.length ? <> · {graphe.types.join(", ")}</> : null}
          </div>
        )}
        {/* ── LA FRATRIE DU NŒUD VISÉ, DANS LA SCÈNE ─────────────────────────
            Un nœud sélectionné ici montrait « Nœud visé : … (nouveau centre du
            graphe) » — et RIEN de ce qui l'entoure, alors que le même sous-graphe
            porte `file_path`, `community_id` et `scc_id` (jamais lus jusqu'ici).
            Chaque relation ANNONCE son effectif réel et le dossier / la
            communauté qui la définit. Un clic recentre le graphe sur le frère
            choisi — le geste du clic sur un nœud de la scène, réutilisé. */}
        {fratrieNoeud && (() => {
          /**
           * ── QUEL ÉLÉMENT MONTRE SA FRATRIE ? ────────────────────────────────
           *
           * Deux gestes réels, dans cet ordre de priorité :
           *  1. un NŒUD explicitement visé (clic sur un point de la nuée) ;
           *  2. sinon LE CENTRE DU GRAPHE AFFICHÉ — c'est le groupe que
           *     l'utilisateur vient de désigner en cliquant une BOULE, et c'est
           *     ce geste-là qui est naturel : il ne demande pas de viser un point
           *     de quelques pixels. Sans cette seconde voie, le panneau restait
           *     inatteignable en pratique (constat du propriétaire).
           *
           * L'index est consulté par qn ET par identifiant : le centre du graphe
           * est tantôt un qualified_name, tantôt l'id du nœud.
           */
          const c = fratrieNoeud.index;
          const resoudre = (nom: string | null | undefined): string | null => {
            if (!nom) return null;
            const parQn = c.cle.get(nom);
            if (parQn) return parQn;
            return c.qn.has(nom) ? nom : null;
          };
          const visite = vise?.genre === "noeud" ? vise.nom : null;
          // 3e voie, NOMMÉE : quand le centre du graphe est un PRÉFIXE (cas
          // mesuré : « deepseek_harness »), le sous-graphe ne le contient pas et
          // la fratrie restait muette. On montre alors l'élément le plus entouré
          // du graphe chargé — un nœud réel, choisi par un critère énoncé.
          const secours = resoudre(visite) || resoudre(graphe?.centre ?? null) ? null : noeudLePlusEntoure(c);
          const k = resoudre(visite) ?? resoudre(graphe?.centre ?? null) ?? secours?.cle ?? null;
          if (!k) return (
            <div className="q-intro" style={{ marginTop: 2, opacity: 0.85 }}>
              <b>Fratrie</b> : aucun élément désigné dans ce graphe
              ({c.n} nœuds indexés{graphe?.centre ? `, entré par « ${graphe.centre} » qui n'est pas un nœud du sous-graphe` : ""})
              — cliquez une boule pour charger un groupe, ou un point de la nuée pour viser un nœud.
            </div>
          );
          const fr: Fratrie = fratrieDe(c, k);
          const lignes: { titre: string; quoi: string; cle: string; membres: string[] }[] = [
            { titre: "Frères", quoi: "même dossier porteur", cle: fr.parent, membres: fr.groupes[0].membres },
            { titre: "Co-fichiers", quoi: "même fichier", cle: fr.fichier ?? "—", membres: fr.groupes[1].membres },
            { titre: "Sœurs", quoi: "même communauté", cle: fr.communaute !== undefined ? `communauté ${fr.communaute}` : "—", membres: fr.groupes[2].membres },
            { titre: "Cycle", quoi: "même composante fortement connexe", cle: fr.scc !== undefined ? `scc ${fr.scc}` : "—", membres: fr.groupes[4].membres },
          ];
          const total = fr.groupes.reduce((a, x) => a + x.total, 0);
          return (
            <div className="chips" style={{ marginTop: 4, flexWrap: "wrap" }}>
              <span className="qb-lbl" style={{ width: "100%" }}>
                fratrie de <b>{fr.nom}</b>
                {visite ? " (nœud visé)"
                  : secours ? ` (le plus entouré du graphe chargé — le centre « ${graphe?.centre ?? "?"} » n'est pas un nœud)`
                  : " (centre du graphe affiché)"} — dossier « {fr.parent} »
                {fr.fichier ? <> · fichier « {fr.fichier} »</> : null}
                {" "}· {fr.indexes} nœuds indexés en {fratrieNoeud.ms} ms ·{" "}
                {total} relations · cliquer un frère recentre le graphe
              </span>
              {lignes.map((l) => (
                <span key={l.titre} style={{ width: "100%" }}>
                  <span className="qb-lbl">{l.titre} ({l.quoi}) : <b>{l.membres.length}</b> {l.cle && l.cle !== "—" ? `— « ${l.cle} »` : ""}</span>{" "}
                  {l.membres.length === 0
                    ? <span className="chip">aucun chargé</span>
                    : l.membres.slice(0, 12).map((id) => {
                      const q = fratrieNoeud.index.qn.get(id) || id;
                      return (
                        <button key={id} className="chip"
                          title={`${q} — recentrer le graphe sur ce nœud`}
                          onClick={() => chargerNoeudRef.current?.(graphe?.qn ?? "", q)}>
                          → {fratrieNoeud.index.nom.get(id) || q}
                        </button>
                      );
                    })}
                  {l.membres.length > 12 && (
                    <span className="chip" title={`${l.membres.length} au total — les 12 premiers sont affichés`}>
                      +{l.membres.length - 12} autres (total {l.membres.length})
                    </span>
                  )}
                </span>
              ))}
            </div>
          );
        })()}
        {/* LES CONTRÔLES D'ÉCHELLE ONT DÉMÉNAGÉ DANS `reperes` (barre toujours
            visible, hors de ce panneau qui défile) : « Descente : … »,
            « ⤓ descendre » et « ⏸ figer » y sont lisibles sans défilement. Les
            garder ici en double ferait deux autorités pour un même geste — et
            deux endroits à corriger le jour où l'un des deux change. */}
        {/* AXE DU TEMPS : regarder l'HISTOIRE du monde, pas sa structure.
            Proposé seulement si le store porte des sessions datées. */}
        {temps && focusQn === temps.qn && (
          <div className="chips" style={{ marginTop: 4, flexWrap: "wrap" }}>
            <button className={axeTemps ? "chip on" : "chip"}
              onClick={() => setAxeTemps((a) => !a)}
              title="Projeter l'histoire réelle du monde (sessions datées) au lieu de sa structure">
              ⏱ temps
            </button>
            {axeTemps && (
              <span className="qb-lbl" style={{ display: "block", width: "100%", marginTop: 2 }}>
                {temps.sessions} sessions réelles, du{" "}
                {new Date(temps.t0).toLocaleDateString("fr-FR")} au{" "}
                {new Date(temps.t1).toLocaleDateString("fr-FR")} ·{" "}
                {temps.tranches.map((b, i) => `${i + 1}:${b.n}`).join(" ")}
              </span>
            )}
          </div>
        )}
        {/* SÉLECTION DE GROUPE : « qu'importe la conception, layer, type,
            chaîne » — on sélectionne par type réel, et tout le reste baisse. */}
        {/* AXES MULTI-DIMENSIONNELS — atteignables dès qu'un graphe est chargé.
            Ils étaient gatés sur `focusQn` EN PLUS de `qnsGraphe.length > 1`, donc
            invisibles tant qu'on n'était pas entré dans un monde : depuis le
            Cosmos, rien ne signalait leur existence et il n'y avait rien à
            attraper. Le seul verrou qui a un sens est celui des DONNÉES : sans
            graphe chargé, il n'y a rien à grouper. D'où le repli explicite
            ci-dessous, qui dit quoi faire au lieu de rester muet. */}
        {/* ── L'AXE EST DANS LA BARRE (`.echelle-bar`), PLUS ICI ───────────────
             Le choix d'axe vivait ICI, donc dans le panneau qui défile : il
             n'apparaissait qu'une fois un graphe chargé, et restait sous la ligne
             de flottaison dans une fenêtre de 757 px (contenu 1251 px / 588
             visibles). L'axe est un AXE DE NAVIGATION, pas un réglage de panneau :
             il est désormais dans la barre toujours visible, avec le nombre de
             groupes qu'il distingue. Le garder ici en double ferait deux autorités
             pour la même décision. */}
        {typesGraphe.length > 1 && focusQn && (
          <div className="chips" style={{ marginTop: 4, flexWrap: "wrap" }}>
            <span className="qb-lbl">ressortir</span>
            {typesGraphe.map((t) => (
              <button key={t.type} className={filtreGraphe === t.type ? "chip on" : "chip"}
                title={`Cadrer et n'illuminer que « ${t.type} » (${t.n} nœuds) ; le reste baisse`}
                onClick={() => {
                  // MÊME geste que dans la barre : cadrer ET mettre en valeur
                  // (`entrerAxe`). Le panneau sert à choisir un groupe PRÉCIS,
                  // là où la barre propose le plus gros — une seule façon d'entrer.
                  if (filtreGraphe === t.type) { setFiltreGraphe(null); return; }
                  if (!entrerAxeRef.current?.(focusQn, t.type)) setFiltreGraphe(t.type);
                }}>
                {t.type} <i>{t.n}</i>
              </button>
            ))}
            {filtreGraphe && (
              <button className="chip" onClick={() => setFiltreGraphe(null)}>tout</button>
            )}
            {filtreGraphe && (() => {
              const t = typesGraphe.find((x) => x.type === filtreGraphe);
              const total = typesGraphe.reduce((a, b) => a + b.n, 0);
              return (
                <span className="qb-lbl" style={{ display: "block", width: "100%", marginTop: 2 }}>
                  ressortent : <b>{t?.n ?? 0}</b> nœuds « {filtreGraphe} » · les{" "}
                  <b>{total - (t?.n ?? 0)}</b> autres à 16 % d'intensité
                </span>
              );
            })()}
          </div>
        )}
        {/* ÉTAGE SIGNAUX : ce que portent RÉELLEMENT les vecteurs de ce graphe.
            Quand les composantes sont dégénérées (mêmes valeurs partout), on le
            dit au lieu d'animer une projection qui ne montrerait qu'un point. */}
        {signaux && (
          <div className="q-intro" style={{ marginTop: 2 }}>
            <b>Signaux</b> ({signaux.n} nœuds) :{" "}
            {signaux.degeneré ? (
              <span style={{ color: "#ffb27a" }}>
                aucun vecteur calculé sur ce store — rien à projeter
                (lancer `obrain enrich --ricci`)
              </span>
            ) : (
              <>
                courbure {signaux.courbure.min.toFixed(2)} → {signaux.courbure.max.toFixed(2)}
                {" "}({signaux.courbure.distinctes} distinctes) · énergie{" "}
                {signaux.energie.distinctes > 1
                  ? `${signaux.energie.min.toFixed(2)} → ${signaux.energie.max.toFixed(2)}`
                  : "non alimentée"} · degré {signaux.degre.min} → {signaux.degre.max}
              </>
            )}
          </div>
        )}
        {/* ÉTAGE SÉMANTIQUE : on dit pourquoi il est VIDE, au lieu de le simuler.
            Mesuré dans ce déploiement : la recherche vectorielle (HNSW) est
            derrière la feature `vector-index` — désormais COMPILÉE dans le
            binaire (rebuild `--features ai`) — mais AUCUNE propriété de vecteur
            n'existe sur les nœuds. MESURÉ sur les bases servies :
            runtime-dev, linux-full3, agent-synapse → `embedding` = 0 et
            `vector` = 0 partout. Un étage « sémantique » n'aurait donc rien à
            projeter — et afficher une similarité inventée serait le seul moyen
            de le remplir, ce qu'il ne faut pas faire. La note dit donc la
            DONNÉE manquante, pas une limite du déploiement. */}
        {descente > 0.7 && (
          <div className="q-intro" style={{ marginTop: 2, opacity: 0.75 }}>
            <b>Sémantique</b> :{" "}
            <span style={{ color: "#ffb27a" }}>
              rien à projeter — aucun vecteur stocké. Vérifié sur les bases
              servies : `embedding` et `vector` valent 0 partout. L'index
              vectoriel est compilé ; c'est la DONNÉE qui manque, et une
              similarité inventée serait pire que ce vide.
            </span>
          </div>
        )}
        {/* DESCENTE CONTINUE : où l'on en est entre l'espace et le dedans.
            Affiché parce que c'est vérifiable — et parce que « la carte ne
            change pas, on s'approche » ne se voit pas sans repère. */}
        {(
          <div className="q-intro" style={{ marginTop: 2 }}>
            <b>Descente</b> : {PALIERS[palier]}
            {descente > 0 ? ` — ${(descente * 100).toFixed(0)} %` : " — éloignez-vous ou approchez (molette, + / −)"}
            {" · "}↓ entrer pour de bon
          </div>
        )}
        {/* Ce que l'engine consulte, en direct. */}
        {activation && activation.hits.length > 0 && (
          <div className="q-intro" style={{ marginTop: 2 }}>
            <b>Engine</b> : {activation.hits.length} nœud(s) consulté(s) —{" "}
            {activation.hits.slice(0, 3).map((h) => h.kind).join(", ")}
            {" · au plus fort "}{(Math.max(...activation.hits.map((h) => h.energy)) * 100).toFixed(0)} %
          </div>
        )}
        {/* LÉGENDE COMMUNE : mêmes noms, mêmes couleurs que les autres vues. */}
        <PlanLegend caption={<>Le sol s'affaisse sous la <b>masse réelle</b> (nœuds).</>} />
        <div className="q-intro" style={{ marginTop: 4 }}>
          <b>Quatre preuves</b>, jamais confondues :
          <span title="même qualified_name dans les deux mondes"> entité partagée</span> ·
          <span title="Session.cwd sous la racine du monde : une session réellement exécutée là"> vécu daté</span> ·
          <span title="entité self:/account: présente dans un monde"> plan traversant</span> ·
          <span style={{ color: "#5b7fa6" }} title="mêmes flux de types, volumes réels — preuve la plus faible, trait discontinu"> parenté de structure</span>.
        </div>
        <div className="qb-lbl" style={{ marginTop: 6, display: "block", opacity: 0.75 }}>
          {VIEW_KEYS_HINT}
          <br />
          clic sur un monde : on y descend · clic à nouveau : on entre dedans ·
          double-clic : vue dédiée
        </div>
        <div className="q-intro" style={{ marginTop: 4, fontSize: 11, opacity: 0.8 }}>
          {weave?.counts ? <>
            {weave.counts.worlds} mondes · {weave.counts.anchors_woven} ancres tissées ·{" "}
            {weave.counts.fibers} fibres
            {weave.counts.flow_ties ? <> (dont <b>{weave.counts.flow_ties}</b> de parenté seule)</> : null}
            {" — base synaptique "}{weave.persisted?.store ? "durable" : "non persistée"}
          </> : null}
        </div>
      </ViewPanel>

      {/* LE CHAT ET L'AJOUT DE MONDE ONT QUITTÉ CETTE SCÈNE (2026-09-16).
          Ils vivaient dans le `ViewPanel` ci-dessus, donc DANS la scène WebGL :
          leurs re-rendus suivaient ceux du canvas, et une scène qui échoue
          emportait l'ajout de monde — le geste par lequel un compte neuf obtient
          sa première base. Ils sont montés à la racine par `App` (chat et
          « Gens ») et par `PanneauMonde` (dépôt + question ponctuelle). */}



      {/* DÉTAIL DES FIBRES RÉELLES (panneau commun : repliable comme les autres) */}
      <ViewPanel title="Toile — preuves réelles" side="right" width={330} id="toile"
        style={{ bottom: 12, top: "auto" }}>
        {[...(weave?.fibers || [])]
          .sort((x, y) => (y.weight - x.weight) || ((y.flows || 0) - (x.flows || 0)))
          .map((f, i) => {
            const la = (weave?.worlds || []).find((w) => w.qn === f.a)?.label || f.a;
            const lb = (weave?.worlds || []).find((w) => w.qn === f.b)?.label || f.b;
            const kin = f.weight === 0 && (f.flows || 0) > 0;
            const ex = f.flow_sample?.[0];
            return (
              <div key={i} className="q-intro" style={{ margin: "4px 0", fontSize: 11 }}>
                <b>{la.replace(/ \(.*\)/, "")}</b> ↔ <b>{lb.replace(/ \(.*\)/, "")}</b>
                {kin ? (
                  <>
                    {" — "}
                    <span style={{ color: "#5b7fa6" }}>
                      parenté de structure : {f.flows} flux de types
                    </span>
                    {ex ? <div style={{ opacity: 0.75 }}>
                      ({ex.src})-[{ex.rel}]→({ex.dst}) — {ex.count_a} / {ex.count_b} arêtes
                    </div> : null}
                  </>
                ) : (
                  <>
                    {" — "}{f.weight} synapses
                    {f.shared ? <> · <span title="même qualified_name dans les deux mondes">{f.shared} partagées</span></> : null}
                    {f.bridged ? <> · <span title="sessions réellement exécutées dans ce territoire">{f.bridged} de vécu</span></> : null}
                    {f.flows ? <> · <span style={{ color: "#5b7fa6" }} title="mêmes flux de types">{f.flows} flux</span></> : null}
                    {f.reinforcement ? <> · tissée ×{f.reinforcement}</> : null}
                  </>
                )}
              </div>
            );
          })}
      </ViewPanel>
      </div>
      {/* LA CONVERSATION A DÉMÉNAGÉ DANS `App` (2026-09-15).
          Elle vivait ICI, donc DANS le canvas : ses re-rendus suivaient ceux de
          la scène WebGL — le chat portait la charge GPU du canvas — et son
          panneau plein écran se posait dans le même conteneur que la vue, d'où
          le recouvrement avec la vue des comptes. Sorti de la scène, le chat
          est un composant de l'application : il évolue seul, et rien ne le
          recouvre. Le bouton flottant est parti avec lui, pour la même raison
          (il était monté ici et disparaissait si la scène échouait à se monter). */}
    </div>
  );
}

export { PLAN_LABEL };
