import { useEffect, useRef, useState } from "react";
import * as THREE from "three";
import { applyRelief, bindViewKeys, EMPHASIS, emphasis, makeCamera, makeRenderer, orbitFor, VIEW_KEYS_HINT, zoomBy } from "../graph/grammar";
import { LevelCartouche, PlanLegend, ViewPanel } from "../graph/ViewPanel";
import SubjectLine from "../graph/SubjectLine";
import EngineAsk from "../graph/EngineAsk";
import { peakOf, useActivation } from "../graph/useActivation";
import { ACCENT, typeColor } from "../graph/grammar";
import { subjectAtom, seuilAtom, centersAtom, requestTokenAtom, hopsAtom, limitAtom, selectedNodeAtom, lastGraphAtom } from "../app/store";
import { useSetAtom, useAtom, useAtomValue } from "jotai";
import type { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import { fetchSignatures, fetchSubgraph, searchFunctions } from "../lib/api";
import {
  FRACTALE, cheminEnfant, directions, enfantsDe, estFeuille, nuee, particules,
  rangRacine, rayons, tailleApparente, type GroupeFractal,
} from "../graph/fractale";
import { decisionDensite, pageSuivante, type Geste } from "../graph/densite";

type Sig = { label: string; path: string; nodes: number | null; edges: number | null; by_label: { label: string; count: number }[] };
const PAL = ["#7fd1ff", "#21d4b4", "#ffd166", "#ff7a5c", "#ff6b9d", "#b07aa1", "#ffe066", "#3db4ff", "#5fd68a", "#f5d63d"];
/**
 * Délai de COALESCENCE d'une demande de centre (ms).
 * Mesuré : sans lui, une rafale de clics sur des lignes de la projection
 * incrémente `requestTokenAtom` à chaque clic et déclenche un chargement par
 * clic. 180 ms est au-dessus du plus court intervalle entre deux clics d'une
 * rafale et sous le seuil où l'attente se perçoit (le sous-graphe est déjà à
 * 4 ms de moteur mesurés sur Linux e1000 : c'est le trajet qui compte).
 */
const CENTRE_DEBOUNCE_MS = 180;
const COG = ["Message", "ToolCall", "Session", "Step", "SelfState", "ToolResult"];
const cog = (s: Sig) => s.by_label.some(b => COG.includes(b.label));

function cosine(a: Sig, b: Sig): number {
  const keys = new Set([...a.by_label.map(x => x.label), ...b.by_label.map(x => x.label)]);
  const get = (s: Sig, k: string) => s.by_label.find(x => x.label === k)?.count || 0;
  let dot = 0, na = 0, nb = 0;
  for (const k of keys) { const x = get(a, k), y = get(b, k); dot += x * y; na += x * x; nb += y * y; }
  const d = Math.sqrt(na) * Math.sqrt(nb);
  return d ? dot / d : 0;
}

/** STAGE CONTINU v0 : toutes les bases sont DÉPLIÉES dans une seule scène 3D.
 *  Chaque galaxie = bulle de points colorés par sa composition réelle (types) ;
 *  arcs = similarité réelle ; voyager avec la caméra = changer d'échelle. */

/**
 * UN NŒUD DE L'ÉCHELLE — la même structure à TOUS les niveaux.
 *
 * C'est la seule façon d'obtenir une descente fractale sans plafond : si le
 * niveau 1 et le niveau 7 ne sont pas le même objet, c'est qu'on a écrit deux
 * fois la même loi, et qu'un des deux s'arrêtera avant l'autre.
 */
interface NoeudFractal {
  /** Identité du nœud : store + préfixe de qn. */
  cle: string;
  store: string;
  /** Préfixe de qn du sous-espace (vide pour la racine du store). */
  prefixe: string;
  /** Rang agrégé pour obtenir les ENFANTS de ce nœud. */
  rang: number;
  nom: string;
  count: number;
  profondeur: number;
  parent: NoeudFractal | null;
  /** Le contenant : sa nuée, ses enfants, son étiquette. */
  grp: THREE.Group;
  /** La matière du nœud lui-même. */
  pts: THREE.Points | null;
  etiquette: THREE.Sprite | null;
  enfants: NoeudFractal[];
  /** Les enfants ont été DEMANDÉS une fois (jamais deux). */
  charge: boolean;
  enVol: boolean;
  /** Fond de l'échelle : ce nœud montre ses NŒUDS, pas des sous-groupes. */
  terminal: boolean;
  /**
   * ── LA PAGE DE CE NŒUD (voir `densite.ts`) ───────────────────────────────
   * `tous` = les enfants que le SERVEUR a rendus pour ce sous-espace ; `page` =
   * combien ont été POSÉS dans la scène. Densifier est alors un ajout de
   * matière (aucun appel) ; approfondir change de nœud (préfixe, rang).
   */
  tous: GroupeFractal[];
  page: number;
  /** Le serveur a coupé à son plafond : la suite est chez lui. */
  plafondAtteint: boolean;
  /** Rampe d'ouverture lissée (0 refermé, 1 ouvert), et son hystérésis. */
  v: number;
  ouvert: boolean;
  /** Rayon LOCAL, dans le repère du parent. */
  rayon: number;
}

export default function StageContinu({ onBase, mode = "cosmos", activePath, view = "cosmos" }:
  { onBase?: (path: string, label: string) => void; mode?: "cosmos" | "monde" | "graph";
    activePath?: string; view?: "cosmos" | "monde" | "graphe" }) {
  const host = useRef<HTMLDivElement>(null);
  const setSubject = useSetAtom(subjectAtom);
  // ── LE MAILLON QUI MANQUAIT : LE CENTRE DEMANDÉ AILLEURS (2026-09-13) ──────
  // DÉFAUT MESURÉ (sonde CDP, service 8432, face « 🧠 Monde » sur « Linux e1000 ») :
  // un clic sur une ligne ancrable de la projection n'émettait ZÉRO requête
  // `/api/viz/subgraph`, alors que le panneau affichait « cliquez pour ancrer dans
  // le graphe ». Cause : `centersAtom` et `requestTokenAtom` n'étaient lus que par
  // `Graph3D` et `CenterSearch` — la face par défaut (morph) monte CE composant,
  // qui chargeait son sous-graphe avec un centre CODÉ EN DUR par libellé de store
  // et ignorait toute sélection. La boucle requête → axes → clic → graphe
  // s'arrêtait donc au clic.
  //
  // Ces trois lectures vivent dans des `ref` : le grand effet de scène ne dépend
  // que de `sig`, et un centre demandé ne doit PAS reconstruire la scène entière
  // (c'était le défaut documenté d'`App` : un rendu qui relance une scène).
  const centresAtomV = useAtomValue(centersAtom);
  const tokenV = useAtomValue(requestTokenAtom);
  const hopsV = useAtomValue(hopsAtom);
  const limitV = useAtomValue(limitAtom);
  const centreRef = useRef<{ store: string; centre: string; token: number }>({ store: "", centre: "", token: -1 });
  centreRef.current = {
    store: activePath || "",
    centre: (centresAtomV[0] || "").trim(),
    token: tokenV,
  };
  const hopsRef = useRef(hopsV);
  const limitRef = useRef(limitV);
  hopsRef.current = hopsV;
  limitRef.current = limitV;
  /** Chargeur de sous-graphe exposé par la scène (posé dans le grand effet). */
  const chargeurRef = useRef<((store: string, centre: string, token: number) => void) | null>(null);
  // ACTIVATION EN DIRECT : la même source que le Cosmos (journal partagé).
  const activation = useActivation();
  const energyRef = useRef<Map<string, number>>(new Map());
  energyRef.current = new Map((activation?.worlds || []).map((w) => [w.store, w.peak]));
  const [sig, setSig] = useState<Sig[]>([]);
  const [focus, setFocus] = useState<string | null>(null);
  const [bandSel, setBandSel] = useState<{ path: string; idx: number } | null>(null);
  const bandSelRef = useRef<{ path: string; idx: number } | null>(null);
  const bandMetaRef = useRef<Map<string, { name: string; share: number }[]>>(new Map());
  const zoomOutAllRef = useRef<(() => void) | null>(null);
  const [err, setErr] = useState<string | null>(null);
  /**
   * ── L'ÉCHELLE OÙ L'ON SE TROUVE, DITE À L'ÉCRAN ──────────────────────────
   *
   * Une descente qu'on ne peut pas LIRE est une descente qu'on ne peut pas
   * quitter : sans ce chemin, entrer de trois niveaux ne se distinguait pas
   * d'entrer d'un seul, et il n'y avait aucun moyen de remonter à un étage
   * précis. C'est la scène qui publie (elle seule sait où est la caméra) ; le
   * panneau affiche.
   */
  const [echelle, setEchelle] = useState<{
    chemin: string[];
    profondeur: number;
    /** Ce que l'étage courant contient réellement (nom + effectif). */
    groupes: { nom: string; count: number }[];
    /** Nombre de nœuds du sous-espace courant (le RÉEL, pas le dessiné). */
    total: number;
    subdivisions: boolean;
    /**
     * ── LA MÊME MESURE QUE LE COSMOS : MONTÉ / REÇU, ET LE GESTE ────────────
     * `tous` = ce que l'appel a rendu pour ce sous-espace, `montres` = ce qui est
     * POSÉ dans la scène. Sans les deux, l'Atlas reproduisait à son tour le
     * plafond silencieux que le Cosmos vient de perdre : « 24 sous-groupes »
     * alors que le serveur en a rendu 200.
     */
    tous: number;
    montres: number;
    geste: Geste;
    densifiable: boolean;
  } | null>(null);
  const echellePubliee = useRef<string>("");
  /** Remonter d'un étage (posé par la scène : elle seule connaît le chemin). */
  const remonterEchelleRef = useRef<(() => void) | null>(null);
  /** Voir l'échelle (tout replier) — même porte que ⌂. */
  const replierEchelleRef = useRef<(() => void) | null>(null);
  /** Entrer dans un sous-groupe NOMMÉ (les puces du panneau) — même geste que
   *  le clic dans la scène et que la molette : cadrer, donc ouvrir. */
  const entrerGroupeRef = useRef<((nom: string) => void) | null>(null);
  /**
   * DENSIFIER l'étage courant : poser la page suivante des sous-groupes DÉJÀ
   * reçus. C'est l'autre réponse à « davantage » — l'opposé d'entrer, qui
   * descend d'un cran (`densite.ts`).
   */
  const densifierEchelleRef = useRef<(() => void) | null>(null);
  const [synapses, setSynapses] = useState(true); // flux synaptiques animés
  const synRef = useRef(true);
  const [filter, setFilter] = useState<"tout" | "cognitif" | "code" | "riches">("tout");  // Le seuil vit dans un ATOM partagé : régler θ dans l'Atlas le règle aussi
  // dans le Monde, au lieu de laisser deux valeurs plausibles diverger.
  const [seuil, setSeuil] = useAtom(seuilAtom);

  // ── CE QUE LA FACE MONDE PUBLIE À LA BARRE DE FACETTES ────────────────────
  // DÉFAUT MESURÉ (constaté par la porte `scripts/fratrie-check.mjs`, qui l'écrit
  // désormais noir sur blanc) : en face Monde avec « ⟲ morph » actif — c'est
  // l'état par DÉFAUT — la barre de facettes n'était jamais montée, donc la
  // fratrie d'un élément restait invisible sans changer de mode. Or cette face
  // charge DÉJÀ un sous-graphe (`fetchSubgraph`, plus bas) : elle n'en publiait
  // rien. On publie donc le graphe réellement affiché et le nœud de son centre,
  // ce qui suffit à `App` (il exige que l'id sélectionné EXISTE dans le graphe
  // publié) et à la barre de facettes (elle lit ces deux mêmes atomes).
  // Publication différée par un état : on n'écrit pas d'atome pendant le rendu.
  const setLastGraph = useSetAtom(lastGraphAtom);
  const setSelected = useSetAtom(selectedNodeAtom);
  const [publieFratrie, setPublieFratrie] =
    useState<{ graph: unknown; noeud: string | null } | null>(null);
  useEffect(() => {
    if (publieFratrie === null) { setSelected(null); return; }
    setLastGraph(publieFratrie.graph as never);
    setSelected(publieFratrie.noeud);
  }, [publieFratrie, setLastGraph, setSelected]);
  const theta = seuil.part;
  const setTheta = (v: number) => setSeuil((s) => ({ ...s, part: v }));
  const filterRef = useRef<string>("tout");
  const thetaRef = useRef(0);
  filterRef.current = filter;
  synRef.current = synapses;
  bandSelRef.current = bandSel;
  thetaRef.current = theta;
  const centers = useRef(new Map<string, THREE.Vector3>());
  const radii = useRef(new Map<string, number>());
  const anim = useRef<{ from: THREE.Vector3; t0: THREE.Vector3; to: THREE.Vector3; tt: THREE.Vector3; start: number; dur: number } | null>(null);
  const focusRef = useRef<string | null>(null);
  const viewRef = useRef<"cosmos" | "monde" | "graphe">("cosmos");
  viewRef.current = view;
  const homeRef = useRef(new Map<string, THREE.Vector3>()); // position « cosmos »
  const cameraRef = useRef<THREE.PerspectiveCamera | null>(null);
  const controlsRef = useRef<OrbitControls | null>(null);

  useEffect(() => { fetchSignatures().then(d => setSig(d.stores || [])).catch(() => setSig([])); }, []);

  useEffect(() => {
    const el = host.current; if (!el || !sig.length) return;
    const scene = new THREE.Scene();
    const w = el.clientWidth, h = el.clientHeight;
    const camera = makeCamera(w, h);
    camera.position.set(0, 260, 780);
    applyRelief(scene, camera.position.length());
    const renderer = makeRenderer(w, h);
    renderer.domElement.style.touchAction = "none";
    el.appendChild(renderer.domElement);
    // Profil de caméra COMMUN (voir ../graph/grammar.ts) : perspective, amorti,
    // gestes tactiles et bornes identiques dans toutes les vues.
    //
    // ── LA BORNE BASSE ÉTAIT LE MUR DE L'ÉCHELLE (mesuré 2026-09-14) ────────
    // `minDistance: 40` datait d'une vue où 40 unités étaient « tout près » :
    // les bulles de base ont un rayon de 20 à 105, donc on pouvait entrer dans
    // une base. Mais la descente fractale place les sous-groupes DANS leur
    // parent (rayon ≈ 0,25 R), donc la même proximité RELATIVE demande une
    // distance absolue divisée par deux à chaque niveau. Mesuré sur
    // `runtime-prod` : le cadrage demandé pour `packages` valait 15,33 unités ;
    // la caméra s'arrêtait à 40 (bornée), sa taille apparente plafonnait à
    // 0,166 — sous le seuil d'ouverture (0,30) — et l'étage 3 ne se chargeait
    // JAMAIS. La descente s'arrêtait donc au premier sous-groupe, quelle que
    // soit la donnée. C'est exactement « je n'arrive pas à descendre ».
    //
    // On abaisse la borne au lieu de grossir les objets : la géométrie reste
    // honnête (un enfant EST plus petit que son parent), et c'est la caméra qui
    // a le droit d'aller voir de près.
    const ctrl = orbitFor(camera, renderer.domElement, { minDistance: 0.4, maxDistance: 2400 });
    // Le plan de coupe proche doit suivre : à 1 unité, la caméra ne peut pas
    // s'approcher d'une nuée de rayon 0,5 sans la traverser (on ne verrait que
    // l'intérieur coupé). 0,05 laisse voir un groupe de rayon 0,05 en entier.
    camera.near = 0.05;
    camera.updateProjectionMatrix();
    cameraRef.current = camera; controlsRef.current = ctrl;
    const subGroup = new Map<string, THREE.Group>();
    const bubbleMats = new Map<string, THREE.PointsMaterial>();
    const storeGroups = new Map<string, THREE.Group>(); // parent de chaque particule
    const halos = new Map<string, THREE.Line>();
    const labels = new Map<string, THREE.Sprite>();
    const branchGroups = new Map<string, THREE.Group>(); // branches (types) déployables
    const bandsOf = new Map<string, { pts: THREE.Points; share: number; dir: THREE.Vector3; col: THREE.Color; name: string }[]>();

    // DISPOSITION 3D RÉELLE (anti-« plat ») : chaque base occupe une altitude,
    // un rayon et une profondeur distincts ; les plus grosses se placent plus
    // haut (hiérarchie visuelle), les cognitives devant, le code derrière.
    const maxN = Math.max(1, ...sig.map(x => x.nodes || 1));
    const place = (s: Sig, i: number, n: number, front: boolean) => {
      const gold = Math.PI * (3 - Math.sqrt(5));
      const a = gold * (i + (front ? 0 : 2)) + (front ? 0 : 1.1);
      const taille = Math.log10(1 + (s.nodes || 1)) / Math.log10(1 + maxN); // 0..1
      const R = 230 + 70 * (i % 4);
      const alt = (taille - 0.5) * 320 + (front ? 40 : -40);
      const depth = front ? 140 : -160;
      return new THREE.Vector3(Math.cos(a) * R, alt, Math.sin(a) * R * 0.75 + depth);
    };
    const R = (s: Sig) => 14 + 16 * Math.log10(1 + (s.nodes || 1));
    const mk = new Map<string, { c: THREE.Vector3; r: number }>();
    const posOf = (s: Sig, idx: number) => place(s, idx, sig.length, cog(s));
    sig.forEach((s, idx) => {
      const C = posOf(s, idx);
      const r = R(s);
      centers.current.set(s.path, C); radii.current.set(s.path, r);
      homeRef.current.set(s.path, C.clone());
      mk.set(s.path, { c: C, r });
      // bulle : points colorés ∝ composition réelle (relief interne)
      const n = Math.min(s.nodes || 0, 220_000);
      const pos = new Float32Array(n * 3);
      const col = new Float32Array(n * 3);
      const total = Math.max(1, s.by_label.reduce((a, b) => a + b.count, 0));
      // structure interne EN COUCHES par type (relief lisible en entrant) :
      // chaque type occupe une bande radiale ∝ sa part réelle dans la bulle.
      const bands: { c: THREE.Color; r0: number; r1: number }[] = [];
      let acc = 0;
      s.by_label.slice(0, 10).forEach((b) => {
        const r0 = r * acc / total, r1 = r * (acc + b.count) / total;
        bands.push({ c: new THREE.Color(PAL[s.by_label.indexOf(b) % PAL.length]), r0, r1 });
        acc += b.count;
      });
      // PARTICULES REGROUPÉES PAR BRANCHE (type) : chaque branche est un objet
      // à part → on peut déployer/masquer une branche par FORMULE (part ≥ θ).
      const sgrp = new THREE.Group(); sgrp.position.copy(C);
      const bandObjs: { pts: THREE.Points; share: number; dir: THREE.Vector3; col: THREE.Color; name: string }[] = [];
      const sumAll = Math.max(1, s.by_label.slice(0, 10).reduce((x, y) => x + y.count, 0));
      bands.forEach((band, bi) => {
        const share = (s.by_label[bi]?.count || 0) / sumAll;
        const nb = Math.max(1, Math.round(n * share));
        const bp = new Float32Array(nb * 3);
        for (let i = 0; i < nb; i++) {
          const a = Math.random() * Math.PI * 2, ph = Math.acos(2 * Math.random() - 1);
          const rr = band.r0 + Math.random() * Math.max(1, band.r1 - band.r0);
          bp[i*3] = rr * Math.sin(ph) * Math.cos(a);
          bp[i*3+1] = rr * Math.cos(ph);
          bp[i*3+2] = rr * Math.sin(ph) * Math.sin(a);
        }
        const bg = new THREE.BufferGeometry();
        bg.setAttribute("position", new THREE.BufferAttribute(bp, 3));
        const bm = new THREE.PointsMaterial({ size: 1.7, color: band.c, sizeAttenuation: true, transparent: true, opacity: .95 });
        const bpts = new THREE.Points(bg, bm);
        bpts.userData = { path: s.path, branch: bi, share };
        sgrp.add(bpts);
        const dir = new THREE.Vector3(Math.cos(bi * 2.399), Math.sin(bi * 1.618) * 0.5,
          Math.sin(bi * 2.399)).normalize();
        bandObjs.push({ pts: bpts, share, dir, col: band.c, name: s.by_label[bi]?.label || `type${bi}` });
      });
      bubbleMats.set(s.path, (bandObjs[0]?.pts.material as THREE.PointsMaterial) || new THREE.PointsMaterial());
      bandsOf.set(s.path, bandObjs);
      bandMetaRef.current.set(s.path, bandObjs.map(b => ({ name: b.name, share: b.share })));
      storeGroups.set(s.path, sgrp);
      const halo = new THREE.Line(new THREE.BufferGeometry().setFromPoints(
        Array.from({ length: 40 }, (_, k) => { const an = (k / 40) * Math.PI * 2;
          return new THREE.Vector3(C.x + Math.cos(an) * (r + 9), C.y, C.z + Math.sin(an) * (r + 9)); })),
        new THREE.LineBasicMaterial({ color: 0x7fd1ff, transparent: true, opacity: 0 }));
      halo.name = "halo:" + s.path;
      halos.set(s.path, halo);
      scene.add(halo);
      scene.add(sgrp);
      // BRANCHES : un connecteur centre→bande par type (déployées à la sélection)
      const br = new THREE.Group();
      let acc2 = 0;
      s.by_label.slice(0, 10).forEach((b, bi) => {
        const rMid = (acc2 + b.count / 2) / Math.max(1, s.by_label.reduce((x, y) => x + y.count, 0)) * r;
        acc2 += b.count;
        const dir = new THREE.Vector3(
          Math.cos(bi * 2.399), Math.sin(bi * 1.618) * 0.5, Math.sin(bi * 2.399)).normalize();
        const col = new THREE.Color(PAL[bi % PAL.length]);
        const geo = new THREE.BufferGeometry().setFromPoints([new THREE.Vector3(0, 0, 0), dir.multiplyScalar(rMid)]);
        br.add(new THREE.Line(geo, new THREE.LineBasicMaterial({ color: col, transparent: true, opacity: 0.55 })));
      });
      br.visible = false;
      branchGroups.set(s.path, br);
      sgrp.add(br);
      // label sprite
      const cv = document.createElement("canvas"); cv.width = 512; cv.height = 72;
      const g2 = cv.getContext("2d")!;
      g2.font = "600 34px system-ui, sans-serif"; g2.fillStyle = "#eaf2fa";
      g2.fillText(s.label.replace(/ \(.*\)/, "").slice(0, 18), 10, 46);
      const sp = new THREE.Sprite(new THREE.SpriteMaterial({ map: new THREE.CanvasTexture(cv), transparent: true, depthWrite: false }));
      sp.position.set(C.x, C.y + r + 26, C.z); sp.scale.set(150, 22, 1);
      sp.userData.baseScale = new THREE.Vector3(150, 22, 1);
      labels.set(s.path, sp);
      scene.add(sp);
    });
    // arcs de similarité
    for (let i = 0; i < sig.length; i++) for (let j = i + 1; j < sig.length; j++) {
      const sim = cosine(sig[i], sig[j]);
      if (sim < 0.28) continue;
      const a = mk.get(sig[i].path)!, b = mk.get(sig[j].path)!;
      const ln = new THREE.BufferGeometry().setFromPoints([a.c, b.c]);
      scene.add(new THREE.Line(ln, new THREE.LineBasicMaterial({ color: 0x3db4ff, transparent: true, opacity: 0.25 + sim * 0.5 })));
    }
    // ── SYNAPSES : fibres courbes + influx pulsés entre les bases voisines ──
    const synapsesGrp = new THREE.Group(); scene.add(synapsesGrp);
    const pulses: { sp: THREE.Sprite; c: THREE.QuadraticBezierCurve3; t: number; v: number }[] = [];
    for (let i = 0; i < sig.length; i++) for (let j = i + 1; j < sig.length; j++) {
      const sim = cosine(sig[i], sig[j]);
      if (sim < 0.28) continue;
      const a = centers.current.get(sig[i].path), b = centers.current.get(sig[j].path);
      if (!a || !b) continue;
      const mid = a.clone().add(b).multiplyScalar(0.5)
        .add(new THREE.Vector3(0, 40 + sim * 120, 0));
      const curve = new THREE.QuadraticBezierCurve3(a.clone(), mid, b.clone());
      const pts = curve.getPoints(40);
      const geo = new THREE.BufferGeometry().setFromPoints(pts);
      synapsesGrp.add(new THREE.Line(geo, new THREE.LineBasicMaterial({ color: 0x9fd6ff,
        transparent: true, opacity: 0.18 + sim * 0.35 })));
      for (let k = 0; k < 3; k++) {
        const sp = new THREE.Sprite(new THREE.SpriteMaterial({ color: 0x9ffff0,
          transparent: true, opacity: 0.85, depthWrite: false }));
        sp.scale.set(9, 9, 1);
        synapsesGrp.add(sp);
        pulses.push({ sp, c: curve, t: k / 3, v: 0.12 + sim * 0.2 });
      }
    }
    const stars = new THREE.Points(new THREE.BufferGeometry(), new THREE.PointsMaterial({ color: 0x1a2330, size: 1.6 }));
    (() => { const g = new THREE.BufferGeometry(); const p2 = new Float32Array(3000 * 3);
      for (let k = 0; k < 3000; k++) { p2[k*3] = (Math.random() - .5) * 2400; p2[k*3+1] = (Math.random() - .5) * 2400; p2[k*3+2] = (Math.random() - .5) * 2400; }
      g.setAttribute("position", new THREE.BufferAttribute(p2, 3)); (stars as any).geometry = g; })();
    scene.add(stars);

    // clic → vol caméra vers une galaxie (pas de changement de vue)
    const ray = new THREE.Raycaster();
    const mouse = new THREE.Vector2();
    const bubbles: { p: string; c: THREE.Vector3; r: number }[] = [];
    centers.current.forEach((C, p) => { bubbles.push({ p, c: C, r: radii.current.get(p) || 60 }); });
    // ── SOUS-GRAPHE RÉEL DU CENTRE DEMANDÉ ────────────────────────────────
    // v2 : le sous-graphe au centre de la bulle (bases de code). v3 (2026-09-13) :
    // le CENTRE n'est plus codé en dur par libellé de store. Il vient de
    // `centersAtom` quand un nœud a été désigné (clic sur une ligne de la
    // projection, fil d'ancrage, recherche de centre). Le repli par libellé
    // subsiste — c'est ce que la face montrait au premier chargement — mais il
    // n'est plus la seule voie.
    const subCharge = new Map<string, string>();        // store → centre chargé
    const subCtl = new Map<string, AbortController>();  // store → fetch en vol
    // Stores dont le dernier chargement a ÉCHOUÉ : sans cette mémoire, la boucle
    // de rendu retenterait à chaque image (mesuré : un centre inexistant ferait
    // 60 requêtes/s) et l'échec resterait muet.
    const subEchec = new Map<string, string>();          // store → centre en échec
    const repliCentre = (L: string) => {
      if (/complet/i.test(L)) return "tcp_v4_rcv";
      if (/linux/i.test(L)) return "e1000_probe";
      if (/rust|obrain/i.test(L)) return "substitute_in_operator";
      return "";
    };
    /** Le centre à charger pour ce store : celui DEMANDÉ s'il vise ce store. */
    const centrePour = (p: string) => {
      const d = centreRef.current;
      if (d.centre && d.store === p) return d.centre;
      const st = sig.find(x => x.path === p);
      return st ? repliCentre(st.label) : "";
    };
    const detruire = (g: THREE.Group) => {
      g.traverse((o: any) => {
        o.geometry?.dispose?.();
        if (Array.isArray(o.material)) o.material.forEach((m: any) => m.dispose?.());
        else o.material?.dispose?.();
      });
      g.removeFromParent();
    };
    /** Charge (ou recharge) le sous-graphe de `p` au centre `centre`. */
    const chargerSub = (p: string, centre: string) => {
      const st = sig.find(x => x.path === p); const C = centers.current.get(p);
      if (!st || !C || !centre) return;
      // ANNULATION DES FETCHS EN VOL : changer de centre abandonne le chargement
      // précédent du même store (une rafale ne doit pas empiler des réponses dont
      // la dernière arrivée écraserait la plus récente demande).
      subCtl.get(p)?.abort();
      const ctl = new AbortController();
      subCtl.set(p, ctl);
      subCharge.set(p, centre);
      subEchec.delete(p);
      const r = radii.current.get(p) || 60;
      // `hops` et `nœuds max` sont ceux de l'inspecteur (`hopsAtom`/`limitAtom`) :
      // le sous-graphe d'un centre ancré doit se charger à l'échelle DEMANDÉE, pas
      // à une échelle figée (l'ancien code passait 1 saut et 220 nœuds en dur).
      const hops = Math.max(0, Math.min(3, hopsRef.current));
      const lim = Math.min(2000, Math.max(160, limitRef.current));
      fetchSubgraph(p, [centre], hops, lim, "", ctl.signal).then((d: any) => {
        const grp = new THREE.Group(); grp.name = "sub:" + p;
        const ns = (d.nodes || []).slice(0, 160);
        // ── PUBLIER CE GRAPHE POUR LA FRATRIE (voir le commentaire des atomes) ──
        // SEULEMENT s'il s'agit de l'ensemble que l'utilisateur a OUVERT
        // (`focusRef`) : plusieurs ensembles peuvent charger en parallèle, et
        // publier le dernier arrivé ferait décrire à la barre un graphe qui n'est
        // pas celui à l'écran. Le nœud publié est celui du CENTRE demandé — c'est
        // l'élément que le geste a désigné —, à défaut le premier nœud chargé.
        if (p === focusRef.current) {
          const noeudCentre = (d.nodes || []).find((n: any) => n.qn === centre)?.id
            ?? (d.nodes || [])[0]?.id ?? null;
          setPublieFratrie({ graph: d, noeud: noeudCentre != null ? String(noeudCentre) : null });
        }
        const pos = new Float32Array(ns.length * 3);
        const idx = new Map<string, number>();
        const R2 = r * 0.55;
        const gph = Math.PI * (3 - Math.sqrt(5));
        ns.forEach((nd: any, i: number) => {
          let h = 0; for (const ch of String(nd.id)) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
          const y = 1 - (i / Math.max(1, ns.length - 1)) * 2;
          const rad = Math.sqrt(Math.max(0, 1 - y * y));
          const a = gph * i + (h % 360);
          pos[i*3] = R2 * rad * Math.cos(a) * 0.6;
          pos[i*3+1] = R2 * y * 0.6;
          pos[i*3+2] = R2 * rad * Math.sin(a) * 0.6;
          idx.set(String(nd.id), i);
        });
        const gN = new THREE.BufferGeometry();
        gN.setAttribute("position", new THREE.BufferAttribute(pos, 3));
        grp.add(new THREE.Points(gN, new THREE.PointsMaterial({ color: 0x9fd6ff, size: 2.1, sizeAttenuation: true })));
        const pts: number[] = [];
        (d.edges || []).slice(0, 700).forEach((e: any) => {
          const a = idx.get(String(e.src)), b = idx.get(String(e.dst));
          if (a == null || b == null) return;
          pts.push(pos[a*3], pos[a*3+1], pos[a*3+2], pos[b*3], pos[b*3+1], pos[b*3+2]);
        });
        const gE = new THREE.BufferGeometry();
        gE.setAttribute("position", new THREE.Float32BufferAttribute(pts, 3));
        grp.add(new THREE.LineSegments(gE, new THREE.LineBasicMaterial({ color: 0x4aa8ff, transparent: true, opacity: 0.5 })));
        // TRIANGLES (K3) : le substrat prend la forme des faces réelles du graphe
        const adj = new Map<number, Set<number>>();
        (d.edges || []).forEach((e: any) => {
          const a = idx.get(String(e.src)), b = idx.get(String(e.dst));
          if (a == null || b == null) return;
          if (!adj.has(a)) adj.set(a, new Set());
          if (!adj.has(b)) adj.set(b, new Set());
          adj.get(a)!.add(b); adj.get(b)!.add(a);
        });
        const tp: number[] = [];
        let triCount = 0;
        adj.forEach((nbs, a) => {
          const arr = [...nbs];
          for (let i = 0; i < arr.length && triCount < 140; i++)
            for (let j = i + 1; j < arr.length && triCount < 140; j++) {
              if (!adj.get(arr[i])?.has(arr[j])) continue;
              const A = arr[i], B = arr[j];
              tp.push(pos[a*3], pos[a*3+1], pos[a*3+2],
                      pos[A*3], pos[A*3+1], pos[A*3+2],
                      pos[B*3], pos[B*3+1], pos[B*3+2]);
              triCount++;
            }
        });
        if (tp.length) {
          const gT = new THREE.BufferGeometry();
          gT.setAttribute("position", new THREE.Float32BufferAttribute(tp, 3));
          grp.add(new THREE.Mesh(gT, new THREE.MeshBasicMaterial({ color: 0x21d4b4,
            transparent: true, opacity: 0.16, side: THREE.DoubleSide, depthWrite: false })));
        }
        grp.scale.setScalar(0.001);
        // L'ANCIEN sous-graphe de ce store est DÉTRUIT (géométries libérées) : un
        // changement de centre ne doit pas accumuler des scènes invisibles.
        const ancien = subGroup.get(p);
        if (ancien) detruire(ancien);
        const hote = storeGroups.get(p);
        if (hote) hote.add(grp); else scene.add(grp);
        subGroup.set(p, grp);
        grp.visible = true;
        setErr(null);
      }).catch((e: any) => {
        if (e?.name === "AbortError") return;   // abandon volontaire : rien à dire
        subCharge.delete(p);
        subEchec.set(p, centre);
        // Un centre hors du store n'est PAS silencieux : sans ce message, le clic
        // semblait agir et rien ne changeait — c'est le pire des deux mondes.
        setErr(`sous-graphe « ${centre} » : ${String(e?.message || e).slice(0, 120)}`);
      });
    };
    /** Entrée idempotente : ne recharge pas un store déjà chargé pour ce centre. */
    const ensureSub = (p: string) => {
      const centre = centrePour(p);
      if (!centre) return;
      if (subCharge.get(p) === centre) {
        const g = subGroup.get(p);
        if (g) g.visible = true;   // déjà chargé : on le remet simplement visible
        return;
      }
      chargerSub(p, centre);
    };
    /**
     * ENTRÉE DE LA BOUCLE DE RENDU : uniquement le PREMIER chargement d'un store.
     *
     * La boucle tourne à chaque image : si elle pouvait relancer un chargement,
     * elle produirait une requête par image (et un échec la ferait boucler à
     * l'infini). Elle ne fait donc que le chargement initial, et jamais deux fois
     * le même — le changement de centre passe par l'entrée extérieure ci-dessous.
     */
    const ensureSubInitial = (p: string) => {
      if (subGroup.has(p) || subCharge.has(p) || subEchec.has(p)) return;
      const centre = centrePour(p);
      if (centre) chargerSub(p, centre);
    };
    // ── ENTRÉE EXTÉRIEURE : « ANCRE CE CENTRE » (debounce d'une rafale) ──────
    // Ce que la boucle et les clics de bulle font, l'ancrage doit pouvoir le
    // faire aussi. Le délai n'est pas cosmétique : une rafale de clics sur des
    // lignes de la projection incrémente le jeton à chaque fois ; sans
    // coalescence il partirait une requête par clic. Il ne pouvait pas non plus
    // être posé dans la boucle de rendu : à 60 images/s, un report perpétuel
    // n'aurait JAMAIS laissé partir la requête.
    let minuteur: ReturnType<typeof setTimeout> | null = null;
    chargeurRef.current = (store: string, centre: string, token: number) => {
      void token;
      if (!store || !centre) return;
      if (subCharge.get(store) === centre && subGroup.has(store)) return;  // déjà là
      if (minuteur) clearTimeout(minuteur);
      minuteur = setTimeout(() => { minuteur = null; chargerSub(store, centre); }, CENTRE_DEBOUNCE_MS);
    };

    // ═════════════════════════════════════════════════════════════════════════
    // DESCENTE FRACTALE — LA MÊME LOI SOUS CHAQUE MONDE
    // ═════════════════════════════════════════════════════════════════════════
    //
    // CE QUI MANQUAIT, MESURÉ (2026-09-14, service 8432, `runtime-dev.obram`,
    // 427 530 nœuds) : un clic sur une bulle de base n'émettait AUCUNE requête
    // d'exploration — sonde CDP, sept `/api/viz/activation` et rien d'autre.
    // `repliCentre` fabrique un centre à partir du LIBELLÉ du store et rend la
    // chaîne vide pour « Runtime dev (engrams) », donc `chargerSub` sortait
    // aussitôt. Et là où un sous-graphe se chargeait, sa disposition était une
    // sphère de Fibonacci : une boule de points UNIFORME — l'échelle suivante
    // n'existait pas, alors que la promesse de la vue est « les échelles sont
    // des distances, pas des pages ».
    //
    // Ici, un seul objet décrit un niveau, et la descente est RÉCURSIVE :
    //   · un nœud OUVRE ses enfants quand sa taille apparente franchit le
    //     seuil — c'est-à-dire quand la caméra s'en approche vraiment. La même
    //     formule ouvre donc un monde de rayon 20 et un monde de rayon 1 500 ;
    //   · ses enfants sont les sous-groupes RÉELS du store (`/api/viz/space`),
    //     pas un découpage inventé : les effectifs viennent du moteur ;
    //   · le rayon suit la racine cubique de la part réelle (le VOLUME dit la
    //     part), donc la scène ne ment pas sur les proportions ;
    //   · au fond de l'échelle — un sous-espace qui n'a plus deux enfants — on
    //     charge ses NŒUDS et on les regroupe par TYPE RÉEL : la boule de points
    //     s'organise enfin par groupe, au lieu de rester une nuée anonyme.
    // Rien n'est supposé : le rang de départ est celui que le serveur expose
    // (`rank` de `/api/viz/modules`), et chaque niveau descend de +1.
    const fractales = new Map<string, NoeudFractal>();      // store → racine
    const ctlFractal = new Map<string, AbortController>();  // nœud en vol → annulation
    /** Index par clé : remonter d'un étage, et désigner un groupe au clic. */
    const noeudsParCle = new Map<string, NoeudFractal>();
    let enVolFractal = 0;
    const CHARGES_CONCURRENTES = 2;
    const tmpV = new THREE.Vector3();
    const tmpS = new THREE.Vector3();

    /** Matière d'un nœud : une nuée pleine, colorée par son étage. */
    const nuéeNoeud = (rayon: number, nb: number, profondeur: number) =>
      nuee(rayon, nb, new THREE.Color(typeColor(profondeur)));

    /** Étiquette d'un groupe : NOM + effectif RÉEL (jamais un nom seul). */
    const etiquetteNoeud = (nom: string, count: number, rayon: number): THREE.Sprite => {
      const cv = document.createElement("canvas"); cv.width = 512; cv.height = 64;
      const g2 = cv.getContext("2d")!;
      g2.font = "600 30px system-ui, sans-serif"; g2.fillStyle = "#dff0ff";
      g2.fillText(`${nom} · ${count.toLocaleString("fr-FR")}`.slice(0, 30), 8, 42);
      const sp = new THREE.Sprite(new THREE.SpriteMaterial({
        map: new THREE.CanvasTexture(cv), transparent: true, depthWrite: false, opacity: 0,
      }));
      sp.position.set(0, rayon * 1.45, 0);
      sp.userData.baseScale = new THREE.Vector3(rayon * 3.2, rayon * 0.4, 1);
      sp.scale.copy(sp.userData.baseScale as THREE.Vector3);
      return sp;
    };

    /** Crée un nœud d'échelle : sa matière, son étiquette, son contenant vide. */
    const creerNoeud = (
      store: string, parent: NoeudFractal | null, nom: string, count: number,
      rayon: number, rang: number, profondeur: number, local: THREE.Vector3,
    ): NoeudFractal => {
      const grp = new THREE.Group();
      grp.position.copy(local);
      const prefixe = parent ? cheminEnfant(parent.prefixe, nom) : "";
      const n: NoeudFractal = {
        cle: `${store}|${prefixe}`, store, prefixe, rang, nom, count, profondeur,
        parent, grp, pts: null, etiquette: null, enfants: [], charge: false,
        enVol: false, terminal: false, v: 0, ouvert: false, rayon,
        // ── CE QUE L'ÉTAGE CONTIENT, ET CE QU'ON EN DESSINE ─────────────────
        // La même loi que le Cosmos (`densite.ts`) : `tous` garde la matière
        // reçue, `page` dit ce qui est posé. Sans cette mémoire, un niveau qui
        // porte 200 sous-groupes n'en montrerait que 24 — sans le dire.
        tous: [], page: FRACTALE.pasPage, plafondAtteint: false,
      };
      noeudsParCle.set(n.cle, n);
      // La RACINE d'un store n'a pas de matière propre : la bulle du store EST
      // déjà sa matière (les bandes de types). Un nœud intérieur, lui, porte sa
      // nuée — sinon « entrer » ne montrerait rien de l'objet qu'on a visé.
      grp.visible = false;
      return n;
    };

    /** Pose les enfants d'un nœud dans sa région, à la même loi à chaque fois. */
    const poserEnfants = (
      n: NoeudFractal, groupes: GroupeFractal[], rayonParent: number, terminal: boolean,
    ) => {
      const dirs = directions(groupes.length);
      const rs = rayons(groupes, rayonParent * 0.5);
      const total = Math.max(1, groupes.reduce((a, g) => a + g.count, 0));
      groupes.forEach((g, i) => {
        const local = dirs[i].clone().multiplyScalar(rayonParent * 0.55);
        const enfant = creerNoeud(
          n.store, n, g.nom, g.count, rs[i], n.rang + 1, n.profondeur + 1, local,
        );
        enfant.terminal = terminal;
        const nb = particules(g.count, total);
        enfant.pts = nuéeNoeud(rs[i], nb, enfant.profondeur);
        enfant.grp.add(enfant.pts);
        enfant.etiquette = etiquetteNoeud(g.nom, g.count, rs[i]);
        enfant.grp.add(enfant.etiquette);
        enfant.grp.scale.setScalar(0.001);
        n.grp.add(enfant.grp);
        n.enfants.push(enfant);
      });
    };

    /**
     * LE FOND DE L'ÉCHELLE : les NŒUDS du sous-espace, groupés par TYPE RÉEL.
     *
     * C'est la réponse à « j'ai des boules de points mais quand je zoom dessus
     * ça devrait aussi s'organiser par groupe ». Un sous-espace qui n'a plus
     * deux enfants ne se décompose plus par chemin ; il se décompose par ce que
     * ses items SONT (`label`, porté par chaque nœud du sous-graphe). Le centre
     * est résolu par recherche — un nom de groupe est un PRÉFIXE de qn, pas un
     * nœud, et viser un préfixe rend un nœud et zéro arête (constaté et déjà
     * documenté côté Cosmos).
     */
    const chargerFeuille = async (n: NoeudFractal, ctl: AbortController) => {
      if (n.terminal && !n.enfants.length) return;
      let qn = n.cle;   // repli : le préfixe lui-même, quand la recherche ne rend rien
      try {
        const { items } = await searchFunctions(n.store, n.nom, ctl.signal);
        const cand = (items || []).find((x) => x.qn && x.type !== "Field") || (items || [])[0];
        if (cand?.qn) qn = cand.qn;
      } catch (e) { if ((e as any)?.name === "AbortError") return; }
      let nodes: { label?: string }[] = [];
      try {
        const g = await fetchSubgraph(n.store, [qn], 1, 1200, "energy", ctl.signal);
        nodes = g?.nodes || [];
      } catch (e) { if ((e as any)?.name === "AbortError") return; }
      if (!nodes.length) { n.terminal = true; n.charge = true; return; }
      const parType = new Map<string, number>();
      for (const nd of nodes) {
        const t = nd.label && nd.label.length ? nd.label : "sans type";
        parType.set(t, (parType.get(t) || 0) + 1);
      }
      const groupes: GroupeFractal[] = [...parType.entries()]
        .map(([nom, count]) => ({ nom, count }))
        .sort((a, b) => b.count - a.count)
        .slice(0, FRACTALE.pasPage);
      // Le sous-graphe est un ÉCHANTILLON (`limit`), pas le sous-espace entier :
      // on ne prétend donc pas que `nodes.length` est l'effectif. Les effectifs
      // par type sont ceux de l'échantillon, et le nœud garde SON total réel.
      poserEnfants(n, groupes, n.parent ? n.rayon : (radii.current.get(n.store) || 60), true);
    };

    /** Charge les enfants d'un nœud — UNE fois, et au plus deux à la fois. */
    const chargerEtage = (n: NoeudFractal) => {
      if (n.charge || n.enVol || n.profondeur >= FRACTALE.profondeurMax) return;
      if (enVolFractal >= CHARGES_CONCURRENTES) return;
      n.enVol = true; enVolFractal++;
      const ctl = new AbortController();
      ctlFractal.set(n.cle, ctl);
      const fin = () => { n.enVol = false; n.charge = true; enVolFractal--; ctlFractal.delete(n.cle); };
      const rayonParent = n.parent ? n.rayon : (radii.current.get(n.store) || 60);
      // La RACINE demande son rang de départ au serveur : le front ne le devine
      // pas (mesuré : 3 pour `runtime-dev`, 2 pour `linux-full3`).
      const rang = n.parent ? Promise.resolve(n.rang) : rangRacine(n.store, ctl.signal);
      rang.then((r) => {
        n.rang = r;
        return enfantsDe(n.store, n.prefixe, r, n.page, ctl.signal);
      }).then(({ tous, montres, plafondAtteint }) => {
        // Le niveau RETIENT tout ce que l'appel a rendu : c'est ce qui rend la
        // densification possible sans nouvel appel (loi de `densite.ts`).
        n.tous = tous;
        n.plafondAtteint = plafondAtteint;
        if (!tous.length) { n.terminal = true; return; }
        // La question « est-ce une feuille ? » porte sur le NIVEAU, pas sur la
        // page : un sous-espace qui porte 200 groupes n'est pas une feuille
        // parce qu'on n'en pose que 24.
        if (estFeuille(tous, n.count)) {
          // Un seul enfant qui porte tout le sous-espace : ce n'est pas un
          // sous-groupe, c'est le même objet sous un autre nom (constaté :
          // `deepseek_harness` → `deepseek_harness`). On descend aux nœuds.
          return chargerFeuille(n, ctl);
        }
        poserEnfants(n, montres, rayonParent, false);
      }).catch((e) => { if ((e as any)?.name !== "AbortError") n.terminal = true; })
        .finally(fin);
    };

    /**
     * LE PAS DE DESCENTE, À CHAQUE IMAGE.
     *
     * Une seule visite de l'arbre : ouverture par proximité, rampes, étiquettes,
     * et publication de l'étage où l'on se trouve. La boucle ne CHARGE jamais
     * deux fois le même nœud (`charge`/`enVol`), sinon elle produirait une
     * requête par image — c'est la faute que le chargement initial du
     * sous-graphe avait déjà apprise.
     */
    let publieFractal: { cle: string; data: NonNullable<typeof echelle> } | null = null;
    /** L'étage le PLUS PROFOND actuellement ouvert : c'est « où l'on est ». */
    let noeudCourant: NoeudFractal | null = null;
    /** Dernier cadrage DEMANDÉ (mesure : distingue « non demandé » de « raté »). */
    const cadrageRef = { current: null as null | { nom: string; profondeur: number; d: number; apparent: number } };

    /** La racine fractale d'un store : sa bulle EST le premier nœud d'échelle. */
    const racineDe = (p: string): NoeudFractal => {
      const connue = fractales.get(p);
      if (connue) return connue;
      const r = radii.current.get(p) || 60;
      const n = creerNoeud(p, null, sig.find((s) => s.path === p)?.label || p,
        sig.find((s) => s.path === p)?.nodes || 0, r, 2, 0, new THREE.Vector3());
      n.pts = null;
      const hote = storeGroups.get(p);
      if (hote) hote.add(n.grp); else scene.add(n.grp);
      fractales.set(p, n);
      return n;
    };
    sig.forEach((s) => racineDe(s.path));

    /**
     * CADRER UN NŒUD À UNE TAILLE APPARENTE DONNÉE.
     *
     * C'est la seule façon de fermer un étage sans qu'il se rouvre aussitôt :
     * la loi d'ouverture est une PROXIMITÉ, donc « remonter » doit ÉLOIGNER la
     * caméra — sinon l'image suivante voit le même objet trop grand et rouvre
     * le niveau. On inverse donc la formule (d = r / (app·tan(fov/2))) au lieu
     * de poser une distance en dur, qui ne conviendrait qu'à une seule taille.
     */
    const cadrerNoeud = (n: NoeudFractal, apparent: number) => {
      n.grp.getWorldPosition(tmpV);
      const rayonMonde = Math.max(1e-3, n.rayon * n.grp.getWorldScale(tmpS).x);
      const d = rayonMonde / Math.max(0.02, apparent * Math.tan((camera.fov * Math.PI) / 360));
      const dir = camera.position.clone().sub(tmpV);
      if (dir.lengthSq() < 1e-6) dir.set(0, 0.35, 1);
      anim.current = {
        from: camera.position.clone(), t0: ctrl.target.clone(),
        to: tmpV.clone().add(dir.normalize().multiplyScalar(d)), tt: tmpV.clone(),
        start: performance.now(), dur: 620,
      };
      cadrageRef.current = { nom: n.nom, profondeur: n.profondeur, d: +d.toFixed(2), apparent };
    };
    const fermerSous = (n: NoeudFractal) => {
      n.ouvert = false; n.v = 0;
      n.enfants.forEach(fermerSous);
    };
    /** REMONTER D'UNE ÉCHELLE : replier l'étage courant et reculer la caméra. */
    remonterEchelleRef.current = () => {
      const n = noeudCourant;
      if (!n) return;
      fermerSous(n);
      // Reculer jusqu'à ce que la loi de proximité garde l'étage FERMÉ de
      // bon : sans ce geste, l'image suivante rouvrirait ce qu'on vient de
      // replier (l'ouverture est une distance, pas un drapeau).
      if (n.parent) cadrerNoeud(n.parent, 0.62); else cadrerNoeud(n, 0.30);
      publieFractal = null; echellePubliee.current = "";
      noeudCourant = null;
      setTimeout(() => setEchelle(null), 0);
    };
    /** ⌂ TOUT REPLIER : plus aucun étage ouvert sous les mondes. */
    replierEchelleRef.current = () => {
      fractales.forEach((r) => fermerSous(r));
      publieFractal = null; echellePubliee.current = "";
      noeudCourant = null;
      setTimeout(() => setEchelle(null), 0);
    };
    /** ENTRER DANS UN GROUPE NOMMÉ : le même geste que le clic ou la molette. */
    entrerGroupeRef.current = (nom: string) => {
      const depuis = noeudCourant ?? (focusRef.current ? fractales.get(focusRef.current) ?? null : null);
      if (!depuis) return;
      const cible = depuis.enfants.find((e) => e.nom === nom)
        ?? [...noeudsParCle.values()].find((n) => n.store === depuis.store && n.nom === nom);
      if (cible) cadrerNoeud(cible, 0.75);
    };
    /**
     * ── DENSIFIER L'ÉTAGE : POSER LA PAGE SUIVANTE, SANS RIEN RECHERCHER ────
     *
     * Le même geste que dans le Cosmos, et la même loi (`densite.ts`) : tant que
     * le niveau a rendu plus de sous-groupes qu'il n'en a POSÉ, densifier ajoute
     * les suivants — préfixe, rang et profondeur INCHANGÉS, aucun appel. C'est
     * la différence avec `entrerGroupeRef`, qui descend d'un cran.
     *
     * Les enfants déjà posés ne bougent pas : `directions(n)` est déterministe
     * et indexée, donc la page suivante se range à la suite de la précédente.
     */
    densifierEchelleRef.current = () => {
      const n = noeudCourant;
      if (!n) return;
      const deja = n.enfants.length;
      const total = n.tous.length;
      const d = decisionDensite({
        total, montres: deja, feuille: n.terminal, plafondAtteint: n.plafondAtteint, intention: "plus",
      });
      if (d.geste !== "densifier" || d.requiertAppel) return;
      const cible = pageSuivante(deja, total);
      if (cible <= deja) return;
      n.page = cible;
      const rayonParent = n.parent ? n.rayon : (radii.current.get(n.store) || 60);
      poserEnfants(n, n.tous.slice(deja, cible), rayonParent, false);
      // L'étage se republie de lui-même : sa clé porte le nombre d'enfants.
      publieFractal = null; echellePubliee.current = "";
    };

    const tickFractale = () => {
      scene.updateMatrixWorld();
      const fov = camera.fov, aspect = camera.aspect;
      // L'ÉTAGE COURANT EST CELUI DONT LES ENFANTS SONT MONTRÉS — pas le nœud
      // qu'on regarde. Confondre les deux publiait « rien » à l'étage 1 (les
      // groupes s'affichaient pendant que le fil restait muet) : on prend donc
      // le nœud le PLUS PROFOND réellement ouvert, et à profondeur égale le plus
      // proche de la caméra.
      let courant: NoeudFractal | null = null;
      let courantApp = 0;
      const visiter = (n: NoeudFractal, visible: boolean) => {
        if (!visible) { n.grp.visible = false; return; }
        n.grp.visible = true;
        n.grp.getWorldPosition(tmpV);
        const echelleMonde = n.grp.getWorldScale(tmpS).x;
        const rayonMonde = n.rayon * echelleMonde;
        const dist = camera.position.distanceTo(tmpV);
        const app = tailleApparente(rayonMonde, dist, fov, aspect);
        // OUVERTURE : on charge quand on APPROCHE (une fois), et l'hystérésis
        // empêche la fermeture de rejouer l'ouverture image après image.
        if (!n.charge && app > FRACTALE.seuilOuverture) chargerEtage(n);
        const cible = app > (n.ouvert ? FRACTALE.seuilGarde : FRACTALE.seuilOuverture) ? 1 : 0;
        if (cible > 0) n.ouvert = true;
        if (app < FRACTALE.seuilGarde * 0.5) n.ouvert = false;
        n.v += (cible - n.v) * 0.10;
        // La matière du nœud s'efface quand ses enfants s'y installent : sans
        // cela, les deux se superposent et on ne lit plus la décomposition.
        if (n.pts) {
          const m = n.pts.material as THREE.PointsMaterial;
          m.opacity += ((n.v > 0.02 ? 0.20 : 0.95) - m.opacity) * 0.08;
        }
        if (n.etiquette) {
          const m = n.etiquette.material as THREE.SpriteMaterial;
          // L'étiquette se lit quand on est à portée, et garde une taille
          // apparente stable : facteur ∝ distance / rayon, borné.
          const f = Math.max(0.35, Math.min(6, dist / Math.max(1e-3, rayonMonde * 2.6)));
          m.opacity += ((n.v > 0.02 || n.profondeur === 0 ? 0.92 : 0) - m.opacity) * 0.08;
          const b = n.etiquette.userData.baseScale as THREE.Vector3;
          n.etiquette.scale.set(b.x * f, b.y * f, 1);
        }
        if (n.v > 0.5 && (!courant
          || n.profondeur > courant.profondeur
          || (n.profondeur === courant.profondeur && app > courantApp))) {
          courant = n; courantApp = app;
        }
        for (const e of n.enfants) {
          // L'apparition des enfants suit la rampe du PARENT : c'est elle qui
          // dit « on est entré », et elle est continue (aucun saut de page).
          e.grp.scale.setScalar(Math.max(0.001, n.v));
          visiter(e, n.v > 0.02);
        }
      };
      fractales.forEach((racine, p) => {
        const auFoyer = focusRef.current === p;
        if (!auFoyer) { racine.v = 0; racine.ouvert = false; visiter(racine, false); return; }
        visiter(racine, true);
      });
      // ── PUBLIER L'ÉTAGE RÉEL (fil des poupées) ────────────────────────────
      // Un seul écrivain, et seulement quand la clé change : publier à chaque
      // image ferait re-rendre React 60 fois par seconde.
      const choisi = courant as NoeudFractal | null;
      noeudCourant = choisi;
      if (choisi) {
        const chemin: string[] = [];
        let cur: NoeudFractal | null = choisi;
        while (cur && cur.profondeur > 0) { chemin.unshift(cur.nom); cur = cur.parent; }
        publieFractal = {
          cle: choisi.cle,
          data: {
            chemin,
            profondeur: choisi.profondeur,
            // Ce que l'étage CONTIENT : les groupes réellement dessinés, avec
            // leur effectif réel — la même source que les boules de la scène.
            groupes: choisi.enfants.map((e) => ({ nom: e.nom, count: e.count })),
            total: choisi.count,
            // Un étage sans subdivision doit le DIRE : sinon on croit que la
            // donnée n'existe pas, ce qui est pire qu'une donnée manquante.
            subdivisions: choisi.enfants.length > 0,
            // ── DENSIFIER OU APPROFONDIR : LA DÉCISION, CALCULÉE PAR LA LOI ──
            // Elle est la même que celle du Cosmos (même module), donc les deux
            // vues ne peuvent pas proposer deux gestes différents au même moment.
            tous: choisi.tous.length,
            montres: choisi.enfants.length,
            geste: decisionDensite({
              total: choisi.tous.length, montres: choisi.enfants.length,
              feuille: choisi.terminal, plafondAtteint: choisi.plafondAtteint,
            }).geste,
            densifiable: (() => {
              const d = decisionDensite({
                total: choisi.tous.length, montres: choisi.enfants.length,
                feuille: choisi.terminal, plafondAtteint: choisi.plafondAtteint, intention: "plus",
              });
              // ── ON N'OFFRE PAS UN GESTE QU'ON REFUSE ────────────────────────
              // `densifierEchelleRef` s'arrête sur `requiertAppel` (l'Atlas ne paie
              // pas encore la page du serveur). Publier `true` dans ce cas ferait
              // apparaître une puce « ＋ densifier » qui ne fait RIEN : un geste
              // décoratif, exactement ce que ce fichier refuse ailleurs. La loi dit
              // toujours `densifier` — c'est son verdict, et `geste` le publie ;
              // c'est l'OFFRE qui est bornée à ce que le geste sait faire.
              return d.geste === "densifier" && !d.requiertAppel;
            })(),
          },
        };
      } else {
        publieFractal = null;
      }
      const clePub = publieFractal
        ? `${publieFractal.cle}|${publieFractal.data.profondeur}|${publieFractal.data.groupes.length}`
        : "";
      if (clePub !== echellePubliee.current) {
        echellePubliee.current = clePub;
        const data = publieFractal ? publieFractal.data : null;
        // Différé : on ne pose pas d'état React DANS la boucle de rendu.
        setTimeout(() => setEchelle(data), 0);
      }
    };
    // ── INSTRUMENT DE MESURE ─────────────────────────────────────────────────
    // La descente se VÉRIFIE au lieu de se raconter : ce point expose, image par
    // image, l'étage courant, sa profondeur, ses groupes et leur effectif réel.
    // C'est ce qui permet à `scripts/fractale-check.mjs` d'affirmer que la
    // descente a eu lieu — plutôt que de croire une capture d'écran.
    const publierMesure = () => {
      const n = noeudCourant;
      const mesure = {
        etage: n ? { nom: n.nom, profondeur: n.profondeur, count: n.count, prefixe: n.prefixe } : null,
        chemin: publieFractal?.data.chemin ?? [],
        groupes: publieFractal?.data.groupes ?? [],
        subdivisions: publieFractal?.data.subdivisions ?? false,
        // ── CE QUE LE CONTRÔLE MESURE DE LA DÉCISION ─────────────────────────
        // `tous` / `montres` / `geste` sont publiés tels quels : densifier est un
        // état observable, et non une intention dans un commentaire.
        tous: publieFractal?.data.tous ?? 0,
        montres: publieFractal?.data.montres ?? 0,
        geste: publieFractal?.data.geste ?? null,
        densifiable: publieFractal?.data.densifiable ?? false,
        noeudsCharges: noeudsParCle.size,
        enVol: enVolFractal,
        cam: {
          pos: [+camera.position.x.toFixed(2), +camera.position.y.toFixed(2), +camera.position.z.toFixed(2)],
          cible: [+ctrl.target.x.toFixed(2), +ctrl.target.y.toFixed(2), +ctrl.target.z.toFixed(2)],
          distance: +camera.position.distanceTo(ctrl.target).toFixed(2),
          min: ctrl.minDistance, max: ctrl.maxDistance,
          cadrage: cadrageRef.current,
        },
        // ARBRE COURANT : ce qui est visité, sa taille apparente, et s'il est
        // chargé. C'est ce qui distingue « on n'approche pas » de « on approche
        // mais le chargement ne part pas » — deux pannes très différentes.
        arbre: (() => {
          const out: unknown[] = [];
          const parProfondeur = (x: NoeudFractal) => {
            if (out.length > 40) return;
            let app = 0;
            if (x.grp.visible) {
              x.grp.getWorldPosition(tmpV);
              app = tailleApparente(x.rayon * x.grp.getWorldScale(tmpS).x,
                camera.position.distanceTo(tmpV), camera.fov, camera.aspect);
            }
            out.push({
              nom: x.nom, p: x.profondeur, v: +x.v.toFixed(2), app: +app.toFixed(3),
              charge: x.charge, enVol: x.enVol, terminal: x.terminal, enfants: x.enfants.length,
              visible: x.grp.visible, rayon: +x.rayon.toFixed(2),
            });
            x.enfants.forEach(parProfondeur);
          };
          fractales.forEach(parProfondeur);
          return out;
        })(),
      };
      (window as unknown as Record<string, unknown>).__fractale = mesure;
    };

    renderer.domElement.addEventListener("pointermove", (ev) => {
      const b = el.getBoundingClientRect();
      mouse.set(((ev.clientX - b.left) / b.width) * 2 - 1, -((ev.clientY - b.top) / b.height) * 2 + 1);
      ray.setFromCamera(mouse, camera);
      const over = bubbles.some(bu =>
        ray.ray.distanceSqToPoint(bu.c) < (bu.r + 40) * (bu.r + 40));
      renderer.domElement.style.cursor = over ? "pointer" : "grab";
    });
    renderer.domElement.addEventListener("click", (ev) => {
      const b = el.getBoundingClientRect();
      mouse.set(((ev.clientX - b.left) / b.width) * 2 - 1, -((ev.clientY - b.top) / b.height) * 2 + 1);
      ray.setFromCamera(mouse, camera);
      // 1) ÉTIQUETTES cliquables (on peut sélectionner un monde par son NOM)
      const labelHits = ray.intersectObjects([...labels.values()], false);
      if (labelHits.length) {
        const sp = labelHits[0].object as THREE.Sprite;
        const found = [...labels.entries()].find(([, x]) => x === sp);
        if (found) {
          setBandSel(null); setFocus(found[0]); focusRef.current = found[0];
          ensureSub(found[0]); onBase?.(found[0], sig.find(x => x.path === found[0])?.label || found[0]);
          return;
        }
      }
      let pick: { p: string; c: THREE.Vector3; r: number; d: number } | null = null;
      for (const bu of bubbles) {
        // rayon de hit élargi : le « monde » visible inclut ses particules
        // débordantes et son étiquette (posée à r+26) — un clic pile dessus
        // doit être pris en compte.
        const hit = ray.ray.intersectSphere(new THREE.Sphere(bu.c, bu.r * 1.0 + 40), new THREE.Vector3());
        if (!hit) continue;
        const d = ray.ray.origin.distanceTo(hit);
        if (!pick || d < pick.d) pick = { p: bu.p, c: bu.c, r: bu.r, d };
      }
      // ── ENTRER DANS UN GROUPE COMME DANS UN MONDE ─────────────────────────
      // Sans cela, la descente fractale n'existerait qu'à la MOLETTE : un groupe
      // ne se désignerait pas, et « cliquer une boule pour entrer dedans » —
      // le geste que la vue promet — ne vaudrait que pour l'échelle 0. Le
      // groupe le plus PROCHE devant la caméra l'emporte sur le monde qui le
      // contient (sinon le clic serait toujours avalé par la bulle du store).
      let pickGroupe: { n: NoeudFractal; c: THREE.Vector3; r: number; d: number } | null = null;
      const sphere = new THREE.Sphere();
      for (const n of noeudsParCle.values()) {
        if (n.profondeur === 0 || !n.grp.visible) continue;
        const rayonMonde = n.rayon * n.grp.getWorldScale(tmpS).x;
        if (!(rayonMonde > 0)) continue;
        n.grp.getWorldPosition(tmpV);
        sphere.center.copy(tmpV); sphere.radius = rayonMonde * 1.05;
        const hit = ray.ray.intersectSphere(sphere, new THREE.Vector3());
        if (!hit) continue;
        const d = ray.ray.origin.distanceTo(hit);
        if (!pickGroupe || d < pickGroupe.d) pickGroupe = { n, c: tmpV.clone(), r: rayonMonde, d };
      }
      if (pickGroupe && (!pick || pickGroupe.d <= pick.d)) {
        const n = pickGroupe.n;
        setBandSel(null);
        // ENTRER = cadrer le groupe à une taille apparente qui OUVRE ses
        // enfants : c'est le même geste que la molette, donc le même résultat.
        cadrerNoeud(n, 0.75);
        return;
      }
      // si un monde est déjà visé : le clic peut entrer dans une BRANCHE
      if (!pick && focusRef.current) {
        ray.params.Points = { threshold: 6 } as any;
        const barr = bandsOf.get(focusRef.current) || [];
        for (let bi = 0; bi < barr.length; bi++) {
          const hits = ray.intersectObject(barr[bi].pts, false);
          if (hits.length) { setBandSel({ path: focusRef.current, idx: bi }); return; }
        }
      }
      if (pick) {
        setBandSel(null);
        setFocus(pick.p); focusRef.current = pick.p; ensureSub(pick.p);
        anim.current = {
          from: camera.position.clone(), t0: ctrl.target.clone(),
          to: pick.c.clone().add(new THREE.Vector3(0, pick.r * 0.7, pick.r * 3)),
          tt: pick.c.clone(), start: performance.now(), dur: 750,
        };
        onBase?.(pick.p, sig.find(x => x.path === pick.p)?.label || pick.p);
      }
    });
    let raf = 0;
    zoomOutAllRef.current = () => {
      setBandSel(null); setFocus(null); focusRef.current = null;
      // Plus d'ensemble ouvert : la fratrie publiée ne décrit plus rien à l'écran.
      setPublieFratrie(null);
      anim.current = { from: camera.position.clone(), t0: ctrl.target.clone(),
        to: new THREE.Vector3(0, 260, 900), tt: new THREE.Vector3(0, 0, 0),
        start: performance.now(), dur: 700 };
    };
    const loop = () => { raf = requestAnimationFrame(loop);
      // SYNAPSES : influx qui circulent le long des fibres (perception vivante)
      synapsesGrp.visible = synRef.current;
      if (synRef.current) {
        for (const p of pulses) {
          p.t = (p.t + p.v * 0.01) % 1;
          const pos = p.c.getPointAt(p.t);
          p.sp.position.copy(pos);
          const s2 = 0.6 + 0.5 * Math.abs(Math.sin(performance.now() * 0.004 + p.t * 6));
          p.sp.scale.set(9 * s2, 9 * s2, 1);
        }
      }
      // PERSPECTIVE CONSTANTE : l'anneau et l'étiquette d'un monde gardent une
      // taille perceptible même en zoomant dedans (facteur ∝ distance/rayon).
      halos.forEach((hl, path) => {
        const C = centers.current.get(path); const rr = radii.current.get(path) || 40;
        if (!C) return;
        const d = camera.position.distanceTo(C);
        const f = Math.max(1, Math.min(9, d / (rr * 3)));
        hl.scale.setScalar(f);
        const lb = labels.get(path);
        if (lb) { const b = lb.userData.baseScale as THREE.Vector3;
          lb.scale.set(b.x * f * 0.55, b.y * f * 0.55, 1); }
      });
      // ALLUMAGE PAR L'ENGINE : un substrat consulté blanchit vers l'or,
      // proportionnellement à l'énergie relue du journal d'activation.
      bandsOf.forEach((barr, path) => {
        const e = energyRef.current.get(path) || 0;
        barr.forEach((bo) => {
          const m = bo.pts.material as THREE.PointsMaterial;
          // `bo.col` est la couleur d'origine de la branche (composition réelle) :
          // on repart toujours d'elle, jamais d'une couleur déjà teintée.
          if (e > 0.01) m.color.copy(bo.col).lerp(new THREE.Color(ACCENT), Math.min(0.75, e));
          else m.color.copy(bo.col);
        });
      });

      // FILTRES (déploie seulement certaines branches ; noyaux heuristiques à venir)
      const filt = filterRef.current;
      bubbleMats.forEach((m, path) => {
        const st = sig.find(x => x.path === path);
        const cog = !!st && st.by_label.some(b => ["Message","ToolCall","Session","Step","SelfState","ToolResult"].includes(b.label));
        const rich = !!st && (st.nodes || 0) >= 100000;
        const keep = filt === "tout" || (filt === "cognitif" && cog) || (filt === "code" && !cog) || (filt === "riches" && rich);
        m.opacity = keep ? 0.95 : 0.08;
      });
      // MODE EXPLORATION piloté par FORMULE : une branche (type) n'est déployée
      // que si sa part ≥ θ et que son monde est visé. Les autres branches sont
      // repliées (masquées) → on ne déploie que les branches voulues.
      const th = thetaRef.current;
      bandsOf.forEach((barr, path) => {
        const focused = (focusRef.current === path) || (viewRef.current === "graphe" && activePath === path);
        const selB = bandSelRef.current;
        barr.forEach((bo) => {
          const deploy = focused && bo.share >= th;
          bo.pts.visible = !focusRef.current || deploy || !focused;
          const isSel = !!selB && selB.path === path && barr.indexOf(bo) === selB.idx;
          // DESCENDANCE : les branches de la base visée s'écartent (éclaté), la
          // branche choisie s'ouvre encore plus → navigation niveau par niveau.
          // Emphase COMMUNE (voir ../graph/grammar.ts) : mêmes rapports partout.
          bo.pts.scale.setScalar(deploy ? (isSel ? EMPHASIS.selected : EMPHASIS.active) : EMPHASIS.idle);
          (bo.pts.material as THREE.PointsMaterial).size =
            emphasis(1.7, isSel ? "selected" : deploy ? "active" : "idle");
        });
      });
      branchGroups.forEach((br, path) => {
        const on = focusRef.current === path;
        br.visible = on;
        br.children.forEach((ln, bi) => {
          const share = bandsOf.get(path)?.[bi]?.share ?? 1;
          ln.visible = on && share >= th;
        });
      });
      // ── MÊME SUBSTRAT, AUTRE FORME ────────────────────────────────────
      // Les mêmes particules changent de forme selon la projection :
      //  cosmos = bulles réparties en 3D ; monde = anneau des bases (ensemble
      //  tissé) ; graphe = la base active au centre, les autres repliées.
      const vw = viewRef.current;
      const idxOf = new Map(sig.map((x, i) => [x.path, i]));
      const total = Math.max(1, sig.length);
      storeGroups.forEach((g, path) => {
        const home = homeRef.current.get(path);
        if (!home) return;
        const i = idxOf.get(path) || 0;
        let target = home, sc = 1;
        if (vw === "monde") {
          const a = (i / total) * Math.PI * 2;
          target = new THREE.Vector3(Math.cos(a) * 430, Math.sin(a * 1.5) * 60, Math.sin(a) * 300);
          sc = 1.15;
        } else if (vw === "graphe") {
          if (activePath && path === activePath) { target = new THREE.Vector3(0, 0, 0); sc = 1.45; }
          else { target = home; sc = 0.03; }
        }
        g.position.lerp(target, 0.09);
        const v = g.scale.x + (sc - g.scale.x) * 0.09;
        g.scale.setScalar(v);
        const C2 = centers.current.get(path); if (C2) C2.copy(g.position);
      });
      if (vw === "graphe" && activePath) {
        ensureSubInitial(activePath);
        // morph d'apparence SEULEMENT si un sous-graphe réel existe pour cette
        // base (sinon rien à révéler → on laisse les particules visibles)
        const hasSub = subGroup.has(activePath);
        bandsOf.forEach((barr, path) => {
          barr.forEach((bo) => {
            const m = bo.pts.material as THREE.PointsMaterial;
            m.opacity = hasSub ? (path === activePath ? 0.22 : 0.08) : 0.95;
          });
        });
      } else {
        bandsOf.forEach((barr, path) => {
          // ── LA MATIÈRE DU MONDE S'EFFACE QUAND SA DÉCOMPOSITION APPARAÎT ──
          // Sans cela, les sous-groupes réels se dessinent PAR-DESSUS les
          // particules de composition : deux organisations superposées, donc
          // aucune lisible. Le monde garde 22 % de son encre — assez pour situer
          // la matière, trop peu pour concurrencer la structure.
          const ouvert = (fractales.get(path)?.v ?? 0) > 0.02;
          barr.forEach((bo) => {
            (bo.pts.material as THREE.PointsMaterial).opacity = ouvert ? 0.22 : 0.95;
          });
        });
      }
      // REPLI FRACTAL : chaque groupe (particules d'une base) se replie vers son
      // parent quand on ne le regarde pas ; la base visée se déplie.
      storeGroups.forEach((g, path) => {
        const target = !focusRef.current ? 1 : (focusRef.current === path ? 1.25 : 0.02);
        const k = target > g.scale.x ? 0.10 : 0.14;
        const v = g.scale.x + (target - g.scale.x) * k;
        g.scale.setScalar(v);
      });
      scene.children.forEach((o: any) => {
        if (typeof o.name === "string" && o.name.startsWith("halo:")) {
          const m = o.material as THREE.LineBasicMaterial;
          m.opacity = focusRef.current === o.name.slice(5) ? 0.9 : 0.05;
        } else if (typeof o.name === "string" && o.name.startsWith("sub:")) {
          const on = focusRef.current === o.name.slice(4);
          o.visible = true;
          const target = on ? 1 : 0.001;
          o.scale.x += (target - o.scale.x) * 0.12;
          o.scale.y = o.scale.x; o.scale.z = o.scale.x;
          if (!on && o.scale.x < 0.01) o.visible = false;
        } else if (o.isPoints && o.userData && o.userData.path) {
          const m = o.material as THREE.PointsMaterial;
          m.opacity = !focusRef.current || focusRef.current === o.userData.path ? 0.95 : 0.3;
        }
      });
      if (anim.current) {
        const k = Math.min(1, (performance.now() - anim.current.start) / anim.current.dur);
        const e = 1 - Math.pow(1 - k, 3);
        camera.position.lerpVectors(anim.current.from, anim.current.to, e);
        ctrl.target.lerpVectors(anim.current.t0, anim.current.tt, e);
        if (k >= 1) anim.current = null;
      }
      ctrl.update();
      // LA DESCENTE SE JOUE ICI : après le déplacement de caméra de cette image
      // (sinon on mesurerait la proximité d'un point de vue périmé), et avant le
      // rendu.
      tickFractale();
      publierMesure();
      renderer.render(scene, camera); };
    loop();
    // NAVIGATION CLAVIER (cosmos/morph) : ←/→ changer de base, ↑ remonter d'un
    // étage, ↓ entrer, ⌂ vue d'ensemble, +/− zoom.
    // ── GRAMMAIRE COMMUNE (../graph/grammar.ts) ──────────────────────────
    const stepBase = (dir: number) => {
      const order = sig.map(x => x.path);
      const cur = focusRef.current ? order.indexOf(focusRef.current) : -1;
      const nxt = order[(cur + dir + order.length) % order.length];
      setBandSel(null); setFocus(nxt); focusRef.current = nxt; ensureSub(nxt);
      setSubject({ store: nxt, qn: nxt, label: nxt.split("/").pop() || nxt, via: "atlas" });
      const C = centers.current.get(nxt), r = radii.current.get(nxt) || 60;
      if (C) anim.current = { from: camera.position.clone(), t0: ctrl.target.clone(),
        to: C.clone().add(new THREE.Vector3(0, r * 0.6, r * 3.2)), tt: C.clone(),
        start: performance.now(), dur: 600 };
    };
    const detachKeys = bindViewKeys({
      next: () => stepBase(1),
      prev: () => stepBase(-1),
      up: () => {
        // D'ABORD L'ÉCHELLE : si un étage fractal est ouvert, ↑ remonte d'UN
        // étage (et RECULE la caméra, sinon la proximité rouvrirait aussitôt ce
        // qu'on vient de replier). Avant, ↑ sautait directement à l'Atlas : on
        // ne pouvait pas remonter d'un cran, seulement tout quitter.
        if (noeudCourant) { remonterEchelleRef.current?.(); return; }
        if (bandSelRef.current) setBandSel(null);
        else { setFocus(null); focusRef.current = null; zoomOutAllRef.current?.(); }
      },
      down: () => {
        // ENTRER : le plus gros groupe de l'étage courant, sinon la base.
        if (noeudCourant && noeudCourant.enfants.length) {
          const plus = [...noeudCourant.enfants].sort((a, b) => b.count - a.count)[0];
          cadrerNoeud(plus, 0.75);
          return;
        }
        if (!focusRef.current && sig.length) {
          const nxt = sig[0].path; setFocus(nxt); focusRef.current = nxt; ensureSub(nxt);
          return;
        }
        const r = focusRef.current ? fractales.get(focusRef.current) : null;
        if (r) cadrerNoeud(r, 0.75);
      },
      home: () => { replierEchelleRef.current?.(); zoomOutAllRef.current?.(); },
      // `d` — DENSIFIER l'étage (page suivante), pas descendre d'un cran.
      densifier: () => densifierEchelleRef.current?.(),
      zoomIn: () => zoomBy(camera, ctrl, 0.75),
      zoomOut: () => zoomBy(camera, ctrl, 1.33),
      escape: () => setBandSel(null),
    });
    const onR = () => { camera.aspect = el.clientWidth / el.clientHeight; camera.updateProjectionMatrix(); renderer.setSize(el.clientWidth, el.clientHeight); };
    window.addEventListener("resize", onR);
    return () => {
      cancelAnimationFrame(raf);
      window.removeEventListener("resize", onR);
      detachKeys(); ctrl.dispose();
      // LE NETTOYAGE DU MAILLON D'ANCRAGE : rien ne doit survivre à la scène —
      // ni le report d'une demande, ni un fetch en vol (sinon une réponse
      // arriverait dans une scène détruite).
      if (minuteur) { clearTimeout(minuteur); minuteur = null; }
      chargeurRef.current = null;
      subCtl.forEach((c) => c.abort());
      subCtl.clear();
      // Rien ne doit survivre à la scène : une descente en vol écrirait sinon
      // des enfants dans des groupes détruits.
      ctlFractal.forEach((c) => c.abort());
      ctlFractal.clear();
      remonterEchelleRef.current = null;
      replierEchelleRef.current = null;
      entrerGroupeRef.current = null;
      densifierEchelleRef.current = null;
      (window as unknown as Record<string, unknown>).__fractale = null;
      el.removeChild(renderer.domElement);
      renderer.dispose();
    };
  }, [sig]);

  // ── LA BOUCLE SE FERME ICI : requête → axes → clic → GRAPHE (2026-09-13) ───
  // `centersAtom` + `requestTokenAtom` sont ce que l'ancrage écrit (une seule
  // porte : `ancrerAtom`, voir app/store.ts). Les lire ICI est ce qui manquait :
  // la face par défaut (morph) monte ce composant, et un clic sur une ligne de
  // la projection ne produisait aucune requête de sous-graphe.
  // Les valeurs passent par des `ref` pour que la scène ne soit pas reconstruite
  // à chaque ancrage ; seul le CHARGEUR est appelé, et c'est lui qui déduplique.
  useEffect(() => {
    const { store, centre, token } = centreRef.current;
    if (!store || !centre || token < 0) return;
    chargeurRef.current?.(store, centre, token);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tokenV, centresAtomV, activePath, sig]);

  // voyage d'échelle DANS le même canvas : cosmos (loin) ⇄ monde (dans la bulle)
  useEffect(() => {
    // la scène n'existe pas encore (données non chargées ou effet principal
    // pas encore exécuté) : ne rien faire — pas de déréférencement nul
    if (!cameraRef.current || !controlsRef.current) return;
    const C = activePath ? centers.current.get(activePath) : undefined;
    const r = activePath ? radii.current.get(activePath) : undefined;
    // NB : un clic de bulle EN MODE COSMOS doit zoomer DANS la base (et non
    // revenir à la vue d'ensemble) — sinon le repli fractal ne s'activait pas.
    if (!activePath || !C || !r) {
      anim.current = null;
      focusRef.current = null;
      anim.current = { from: cameraRef.current!.position.clone(), t0: controlsRef.current!.target.clone(),
        to: new THREE.Vector3(0, 260, 780), tt: new THREE.Vector3(0, 0, 0), start: performance.now(), dur: 700 };
      return;
    }
    focusRef.current = activePath;
    setFocus(activePath);
    const dist = mode === "graph" ? r * 1.5 : r * 3.2;
    anim.current = {
      from: cameraRef.current!.position.clone(), t0: controlsRef.current!.target.clone(),
      to: C.clone().add(new THREE.Vector3(0, r * 0.6, dist)), tt: C.clone(),
      start: performance.now(), dur: 800,
    };
  }, [mode, activePath, sig]);

  return <div className="overview">
    <div ref={host} style={{ position: "absolute", inset: 0 }} />
    <ViewPanel title="Atlas — même substrat, formes multiples"
      subtitle="Un seul univers : les échelles sont des distances, pas des pages.">
      {/* Fil d'étage COMMUN (voir ../graph/ViewPanel.tsx) : Atlas › base › branche */}
      <LevelCartouche levels={[
        { label: "Atlas", icon: "🌌", current: !focus,
          onClick: () => { setBandSel(null); setFocus(null); focusRef.current = null; } },
        ...(focus ? [{
          label: focus.split("/").pop() || "base", icon: "🧠",
          current: !bandSel && !echelle,
          onClick: () => { setBandSel(null); replierEchelleRef.current?.(); },
        }] : []),
        // ── LE FIL MONTRE TOUTES LES POUPÉES, PAS SEULEMENT LA DERNIÈRE ─────
        // « Il faut voir les niveaux ENSEMBLE » : chaque sous-groupe traversé a
        // son cran dans le fil, et cliquer un cran remonte à celui-là.
        ...(echelle ? echelle.chemin.map((nom, i) => ({
          label: nom, icon: "◦",
          current: i === echelle.chemin.length - 1,
          onClick: () => remonterEchelleRef.current?.(),
        })) : []),
        ...(bandSel && bandMetaRef.current.get(bandSel.path)?.[bandSel.idx]
          ? [{ label: bandMetaRef.current.get(bandSel.path)![bandSel.idx].name, icon: "🔹", current: true }]
          : []),
      ]} />
      <div className="chips" style={{ marginBottom: 6 }}>
        <button className={synapses ? "chip on" : "chip"} onClick={() => setSynapses(v => !v)}>⚡ synapses</button>
      </div>
      {/* Rappel de la grammaire COMMUNE (graph/grammar.ts). Ce panneau sert les
          vues Monde et Graphe en mode « morph » (défaut) : sans ce libellé, les
          deux vues les plus utilisées n'affichaient pas la grammaire commune. */}
      <div className="qb-lbl" style={{ display: "block", marginBottom: 6, opacity: .75 }}>
        {VIEW_KEYS_HINT}
      </div>
      {/* BOUTONS DE NAVIGATION par étage */}
      <div className="chips" style={{ marginBottom: 6 }}>
        <button className="chip" disabled={!focus} onClick={() => {
          // REMONTER D'UN ÉTAGE D'ABORD (le geste le plus fréquent quand on est
          // descendu), puis sortir de la branche, puis de la base.
          if (echelle) { remonterEchelleRef.current?.(); return; }
          if (bandSel) setBandSel(null); else { setFocus(null); focusRef.current = null; }
        }}>◀ remonter</button>
        <button className="chip" disabled={!focus} onClick={() => { replierEchelleRef.current?.(); zoomOutAllRef.current?.(); }}>⌂ tout voir</button>
        <button className="chip" disabled={!bandSel} onClick={() => setBandSel(null)}>refermer la branche</button>
      </div>
      {/* CARTOUCHE EXPLICATIVE selon l'étage */}
      {!focus && <p className="q-intro">Étage <b>Atlas</b> : {sig.length} bases déployées. Chaque bulle = un substrat ; les arcs = similarité réelle.
        Cliquez une bulle <i>ou son nom</i> pour descendre dans la base.</p>}
      {focus && !bandSel && <p className="q-intro">Étage <b>base</b> : <b>{focus.split("/").pop()}</b> — ses branches de connaissances sont écartées
        (chaque amas = un type). Cliquez un amas pour ouvrir la branche ; cliquez le nom d'un autre monde pour changer de base.</p>}
      {/* ── L'ÉTAGE RÉEL DE LA DESCENTE FRACTALE ────────────────────────────
          DÉFAUT MESURÉ avant ce bloc : entrer dans un monde ne se distinguait pas
          d'y rester. On affiche donc ce que l'étage CONTIENT VRAIMENT — les
          sous-groupes et leurs effectifs réels, tels que le moteur les agrège —
          et on dit franchement quand il n'y a plus de subdivision, plutôt que de
          laisser croire que la donnée n'existe pas. */}
      {echelle && !bandSel && <p className="q-intro">
        Étage <b>{echelle.profondeur}</b> : <b>{[...echelle.chemin].pop() || focus?.split("/").pop()}</b> —{" "}
        {echelle.total.toLocaleString("fr-FR")} nœuds dans ce sous-espace,{" "}
        {echelle.subdivisions
          ? <>{echelle.montres}{echelle.tous > echelle.montres ? ` sur ${echelle.tous}` : ""} sous-groupe(s) réel(s) : {echelle.groupes.slice(0, 4).map((g) => `${g.nom} (${g.count.toLocaleString("fr-FR")})`).join(" · ")}{echelle.groupes.length > 4 ? " …" : ""}.</>
          : <>aucun sous-groupe : ce sous-espace est feuille, ses nœuds sont groupés par type.</>}
      </p>}
      {echelle && !bandSel && (
        <div className="chips" style={{ marginBottom: 6 }}>
          {/* Chaque sous-groupe est DÉSIGNABLE : un clic le cadre, ce qui ouvre
              l'étage suivant par la même loi que la molette. */}
          {echelle.groupes.slice(0, 6).map((g) => (
            <button key={g.nom} className="chip" title={`${g.count.toLocaleString("fr-FR")} nœuds — entrer`}
              onClick={() => entrerGroupeRef.current?.(g.nom)}>
              {g.nom} <span style={{ opacity: 0.7 }}>{g.count >= 1000 ? `${Math.round(g.count / 1000)} k` : g.count}</span>
            </button>
          ))}
          {/* ── DENSIFIER — LE GESTE QUI NE CHANGE PAS D'ÉTAGE ────────────────
              L'Atlas n'avait, pour « davantage », que des gestes qui DESCENDENT
              (entrer, la molette) : un sous-espace qui porte 200 sous-groupes
              n'en posait que 24, et rien ne le disait. Ici la loi tranche, et le
              compte annoncé dit ce que le geste rapporte. */}
          {echelle.densifiable && (
            <button className="chip"
              title={`Poser les sous-groupes suivants de CE niveau (${echelle.montres} → ${Math.min(echelle.tous, echelle.montres + FRACTALE.pasPage)} sur ${echelle.tous}) — profondeur et préfixe INCHANGÉS, aucun appel`}
              onClick={() => densifierEchelleRef.current?.()}>
              ＋ densifier {Math.min(echelle.tous, echelle.montres + FRACTALE.pasPage) - echelle.montres}
            </button>
          )}
          {!echelle.densifiable && echelle.subdivisions && echelle.tous > 0 && (
            <span className="qb-lbl" title="Tout ce que le niveau a rendu est posé : le seul « plus » est un cran plus bas">
              niveau entier ({echelle.montres}) → entrer pour approfondir
            </span>
          )}
        </div>
      )}
      {bandSel && bandMetaRef.current.get(bandSel.path)?.[bandSel.idx] && <p className="q-intro">Étage <b>branche</b> :
        <b> {bandMetaRef.current.get(bandSel.path)![bandSel.idx].name}</b> —
        {Math.round(bandMetaRef.current.get(bandSel.path)![bandSel.idx].share * 100)} % des items de la base. ◀ remonter pour revenir à la base.</p>}
      <p style={{ fontSize: 11, opacity: .75 }}>Chaque bulle = une base ; sa couleur interne = sa composition réelle (types). Arc = similarité. Clique une bulle : la caméra y vole (une seule scène, aucun changement d'écran).</p>
      <div className="chips" style={{ marginTop: 6 }}>
        {([["tout","Tout"],["cognitif","Cognitif"],["code","Code"],["riches","Riches"]] as const).map(([k,l]) => (
          <button key={k} className={filter === k ? "chip on" : "chip"} onClick={() => setFilter(k)}>{l}</button>))}
      </div>
      <div className="chips" style={{ marginTop: 4 }}>
        <span className="qb-lbl">seuil branche</span>
        {([0, 0.01, 0.05, 0.2] as const).map(v => (
          <button key={v} className={theta === v ? "chip on" : "chip"} onClick={() => setTheta(v)}>
            {v === 0 ? "0 %" : `${Math.round(v * 100)} %`}</button>))}
      </div>
      {bandSel && bandMetaRef.current.get(bandSel.path)?.[bandSel.idx] && (
        <div className="q-intro">
          Branche : <b>{bandMetaRef.current.get(bandSel.path)![bandSel.idx].name}</b>{" "}
          ({Math.round(bandMetaRef.current.get(bandSel.path)![bandSel.idx].share * 100)} % de la base)
          {" — "}
          <button className="chip" onClick={() => setBandSel(null)}>refermer</button>
        </div>
      )}
      {focus && <div className="q-intro">Foyer : <b>{focus.split("/").pop()}</b> — zoom arrière (⌂) pour revoir tout l'univers.</div>}
      {/* L'ÉCHEC D'UN ANCRAGE SE DIT : un centre absent du store laissait sinon la
          scène identique, sans aucune explication — le clic semblait mort. */}
      {err && <div className="q-intro" style={{ color: "#ff7a5c" }}>⚠ {err}</div>}
      <SubjectLine />
      <EngineAsk />
      {activation && activation.hits.length > 0 && (
        <div className="q-intro" style={{ marginTop: 2 }}>
          <b>Engine</b> : {activation.hits.length} nœud(s) consulté(s) —{" "}
          {activation.hits.slice(0, 3).map((h) => h.kind).join(", ")}
          {" · au plus fort "}{(Math.max(...activation.hits.map((h) => h.energy)) * 100).toFixed(0)} %
        </div>
      )}
      <PlanLegend caption={<>La couleur interne d'une bulle = sa composition réelle (types) ; son plan se lit à l'anneau.</>} />
    </ViewPanel>
    <div className="zoomctl">
      <button title="Zoom arrière / revoir tout l'univers" onClick={() => {
        const cam = (document.querySelector(".overview canvas") as any);
        setFocus(null);
        const cur = centers.current.get("");
        const has = cur !== undefined;
        anim.current = has ? null : null; // vol géré ci-dessous via scene — simple reset
      }}>⌂</button>
    </div>
  </div>;
}
