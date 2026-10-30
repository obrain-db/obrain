// Monde tissé (2026-09-10 v3) — rendu soigné « constellation » :
//  • racine = tous les projets/modules (anneau), liens agrégés réels ;
//  • sélection → plongée dans le sous-espace (anneau interne + liens internes),
//    autres projets en satellites contextuels (liens vers l'extérieur) ;
//  • fil d'échelle cliquable (remontée directe), deep-link ?dive=a/b/c ;
//  • feuilles → bascule unitaire (graphe 3D réel).
//
// Rendu : nœuds = disques lumineux bornés + étiquettes DOM nettes, arêtes =
// courbes fines avec FLUX animé, grille orbitale discrète, halo de sélection,
// fond épuré (aucun nuage aléatoire = bruit).
import { useEffect, useRef, useState } from "react";
import { useAtomValue, useAtom } from "jotai";
import * as THREE from "three";
import type { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import { applyRelief, bindViewKeys, makeCamera, makeRenderer, orbitFor, RELIEF, VIEW_KEYS_HINT } from "../graph/grammar";
import SubjectLine from "../graph/SubjectLine";
import EngineAsk from "../graph/EngineAsk";
import { LevelCartouche, PlanLegend } from "../graph/ViewPanel";
import { subjectAtom, seuilAtom } from "../app/store";
import { useSetAtom } from "jotai";
import {
  fetchOverview, fetchModules, fetchModuleArcs, fetchLabelPairs, fetchSpace, searchFunctions, isAbort,
} from "../lib/api";
import { layersAtom, organicModeAtom, montreAtom } from "../app/store";
import { PAGE, pageSuivante } from "../graph/densite";
import {
  AGG_TTL, COL, MAX_DEPTH, MOD_COL, NOISE_RE, agg, discTexture, ensGood, ensOk, fmt,
  forceLayout, glowTexture, hashC, type Ens, type Link,
} from "../graph/monde/util";
export default function StoreOverview({ store, onExit, onDrill }: {
  store: string; onExit: () => void; onDrill?: (qn: string) => void;
}) {
  const host = useRef<HTMLDivElement>(null);
  const labelsRef = useRef<HTMLDivElement>(null);
  const ctrlRef = useRef<OrbitControls | null>(null);
  const camRef = useRef<THREE.PerspectiveCamera | null>(null);
  const prevStore = useRef(store);
  const rootKind = useRef<"modules" | "projets">("projets");

  const [data, setData] = useState<any>(null);
  const [err, setErr] = useState("");
  const [mode, setMode] = useState<"modules" | "projets" | "familles" | "">("");
  const [stage, setStage] = useState("");
  const [ens, setEns] = useState<Ens[]>([]);
  const [links, setLinks] = useState<Link[]>([]);
  const [flux, setFlux] = useState<Record<string, number>>({});
  const [rootRank, setRootRank] = useState<number | null>(null);
  const [rootLinks, setRootLinks] = useState<Link[]>([]);
  const [crumb, setCrumb] = useState<string[]>(() =>
    (new URLSearchParams(window.location.search).get("dive") || "")
      .split("/").map((s) => s.trim()).filter(Boolean));
  const [ext, setExt] = useState<{ name: string; color: string; w: number }[]>([]);
  const [selId, setSelId] = useState<string | null>(null);
  const setSubject = useSetAtom(subjectAtom);
  // Panneau repliable — le ✕ de ce panneau QUITTE la vue Monde : au doigt, c'est
  // un piège. Défaut replié sur mobile (l'écran est petit, le canvas d'abord).
  // Clé PAR CONTEXTE (mobile / bureau) : replier au doigt ne doit pas replier
  // aussi le grand écran — voir graph/ViewPanel.tsx pour le constat.
  const clePanneau = () => "obrain.panel.monde." +
    (typeof window === "undefined" || window.innerWidth > 720 ? "desktop" : "mobile");
  const [panneauOuvert, setPanneauOuvert] = useState<boolean>(() => {
    try {
      const v = localStorage.getItem(clePanneau());
      if (v !== null) return v === "1";
    } catch { /* navigation privée : on retombe sur le défaut */ }
    return clePanneau().endsWith("desktop");
  });
  const basculerPanneau = () => setPanneauOuvert((o) => {
    try { localStorage.setItem(clePanneau(), o ? "0" : "1"); } catch { /* ignoré */ }
    return !o;
  });
  const [neighbors, setNeighbors] = useState<{ name: string; w: number }[]>([]);
  /** Dépliage du voisinage par flux (le plafond d'AFFICHAGE est annoncé). */
  const [nbTout, setNbTout] = useState(false);
  const [pinned, setPinned] = useState<Set<string>>(new Set()); // multi-sélection (afficher/masquer)
  const [topView, setTopView] = useState(false); // vue du dessus (plan, tous les projets)
  const [mapOn, setMapOn] = useState(false); // carte-mère « big picture »
  const [helpOn, setHelpOn] = useState(false);
  const [branches, setBranches] = useState<{ parent: string; kids: Ens[] }[]>([]); // aperçu branches racine
  const [foldFam, setFoldFam] = useState(false); // repli/dépli des familles de connaissances
  const foldRef = useRef(false); foldRef.current = foldFam;
  // Seuil et critère partagés avec l'Atlas (voir `seuilAtom`) : une seule
  // grandeur, donc plus deux réglages qui prétendent dire la même chose.
  const [seuil, setSeuil] = useAtom(seuilAtom);
  const thetaBranch = seuil.part;
  const thetaMode = seuil.mode;
  const setThetaBranch = (v: number) => setSeuil((s) => ({ ...s, part: v }));
  const setThetaMode = (f: (m: "part" | "poids") => "part" | "poids") =>
    setSeuil((s) => ({ ...s, mode: f(s.mode) }));
  const thetaRef = useRef(0); thetaRef.current = thetaBranch;
  const thetaModeRef = useRef<"part" | "poids">("part"); thetaModeRef.current = thetaMode;
  const [ensFull, setEnsFull] = useState<Ens[]>([]); // liste complète (révélation progressive)
  const [hoverName, setHoverName] = useState<string | null>(null); // rameaux au survol

  const organic = useAtomValue(organicModeAtom);
  // La même règle d'affichage que le reste du chrome : ce qui se montre est
  // CALCULÉ (écran + geste de réduction + décisions), jamais décidé sur place.
  const montre = useAtomValue(montreAtom);
  const organicRef = useRef(organic); organicRef.current = organic;
  const layers = useAtomValue(layersAtom);
  const layersRef = useRef(layers); layersRef.current = layers;
  const pinnedRef = useRef<Set<string>>(new Set()); pinnedRef.current = pinned;
  const topRef = useRef(topView); topRef.current = topView;
  // Refs miroirs + état du préchauffage de survol (une requête à la fois,
  // jamais annulée une fois partie → la donnée reste dans le cache).
  const storeR = useRef(store); storeR.current = store;
  const hoverNameR = useRef<string | null>(hoverName); hoverNameR.current = hoverName;
  const selIdR = useRef<string | null>(selId); selIdR.current = selId;
  const hoverGen = useRef(0);
  const hoverBusy = useRef(false);
  const hoverJobR = useRef<{ name: string; prefix: string; rank: number } | null>(null);
  const mapCanvas = useRef<HTMLCanvasElement | null>(null);
  const mapOnR = useRef(false); mapOnR.current = mapOn;
  const mapXf = useRef<{ x0: number; z0: number; s: number; ox: number; oy: number } | null>(null);
  const ensR = useRef<Ens[]>([]); ensR.current = ens;
  const linksR = useRef<Link[]>([]); linksR.current = links;
  const keepPose = useRef<{ p: THREE.Vector3; t: THREE.Vector3; up: THREE.Vector3; top: boolean } | null>(null);
  const keepStore = useRef<string>("");

  /**
   * ── LES VOISINS D'UN ENSEMBLE, VUS DU PANNEAU ─────────────────────────────
   *
   * Le même calcul que `neighborsOf` dans la scène (qui sert au survol), mais
   * lisible depuis le rendu : les puces du panneau doivent pouvoir se
   * SÉLECTIONNER l'une l'autre. Il lit la MÊME liste d'arêtes
   * (`apiRef.current.edges`, les flux réels du niveau) — deux sources
   * donneraient deux voisinages.
   */
  const voisinsParFlux = (name: string): { name: string; w: number }[] =>
    apiRef.current.edges
      .filter((a) => a.s === name || a.d === name)
      .map((a) => ({ name: a.s === name ? a.d : a.s, w: a.w }))
      .sort((a, b) => b.w - a.w);

  const apiRef = useRef<{
    discs: THREE.Sprite[]; edges: { line: THREE.Line; s: string; d: string; w: number; pts: THREE.Vector3[] }[];
    rings: THREE.Line[]; sats: THREE.Line[];
    pos: Map<string, THREE.Vector3>; satPos: Map<string, THREE.Vector3>;
    hover: string | null; sel: string | null; focus: THREE.Vector3 | null;
    scene: THREE.Scene | null; ctx: THREE.Group | null;
    ctxLabels: Map<string, HTMLDivElement>;
    ctxPos: Map<string, { p: THREE.Vector3; kind: "anc" | "child" }>;
    ghostDiscs: { sp: THREE.Sprite; name: string; kind: "anc" | "child" }[];
    lineage: THREE.Group | null;
    lineageDiscs: { sp: THREE.Sprite; name: string }[];
    lineageLabels: Map<string, HTMLDivElement>;
    lineagePos: Map<string, THREE.Vector3>;
    branch: THREE.Group | null;
    branchDiscs: { sp: THREE.Sprite; name: string; parent: string; count: number }[];
    branchLabels: Map<string, HTMLDivElement>;
    branchPos: Map<string, THREE.Vector3>;
    branchBase: Map<string, THREE.Vector3>;
    hoverG: THREE.Group | null;
    hoverDiscs: { sp: THREE.Sprite; name: string; parent: string }[];
    hoverLabels: Map<string, HTMLDivElement>;
    hoverPos: Map<string, THREE.Vector3>;
  }>({
    discs: [], edges: [], rings: [], sats: [], pos: new Map(), satPos: new Map(),
    hover: null, sel: null, focus: null,
    scene: null, ctx: null, ctxLabels: new Map(), ctxPos: new Map(), ghostDiscs: [],
    lineage: null, lineageDiscs: [], lineageLabels: new Map(), lineagePos: new Map(),
    branch: null, branchDiscs: [], branchLabels: new Map(), branchPos: new Map(), branchBase: new Map(),
    hoverG: null, hoverDiscs: [], hoverLabels: new Map(), hoverPos: new Map(),
  });
  const tipRef = useRef<HTMLDivElement>(null);

  const depth = crumb.length;
  const prefixNow = crumb.join(".");
  const crumbKey = prefixNow;

  const drillInto = (token: string) => {
    if (!onDrill || !token || token === "Projets") return;
    // un qn propre (symboles uniquement) peut être ancré tel quel
    const okQn = (q: string) => /^[a-zA-Z0-9_]+(\.[a-zA-Z0-9_]+){1,}$/.test(q || "");
    if (okQn(token)) { onDrill(token); return; }
    // sinon : chercher un vrai hub (fonction/méthode) dans le sous-espace
    searchFunctions(store, token).then((r) => {
      const items = (((r as any)?.items) || []) as { qn: string; type?: string }[];
      const inTok = (q: string) => q.split(".").includes(token) || q.includes("." + token + ".");
      const pool = items.filter((h) => okQn(h.qn) && inTok(h.qn));
      const code = pool.filter((h) => h.type === "Function" || h.type === "Method");
      const hub = (code.length ? code : pool).sort((a, b) => b.qn.length - a.qn.length)[0];
      if (hub) { onDrill(hub.qn); return; }
      setStage(`… ${token} : aucune représentation unitaire (fichiers documents ?) — choisissez une entrée ci-dessous`);
    }).catch(() => setStage(`… ${token} : plongée unitaire indisponible ici`));
  };
  const descendTo = (name: string) => {
    if (name === "Projets") { setCrumb([]); return; }
    if (crumb.length >= MAX_DEPTH) { drillInto(name); return; }
    setCrumb((c) => [...c, name]);
  };
  const togglePin = (name: string) => setPinned((p) => {
    const n = new Set(p);
    if (n.has(name)) n.delete(name); else n.add(name);
    return n;
  });
  const clearPinned = () => setPinned(new Set());
  const toEns = (list: { name: string; count: number }[], max: number): Ens[] =>
    list
      .filter((x) => ensGood(x.name) && x.count >= 2)
      .slice(0, max)
      .map((x, i) => ({ name: x.name, count: x.count, color: MOD_COL[i % MOD_COL.length] }));

  // ── chargement par échelle ──
  useEffect(() => {
    const ctl = new AbortController();
    const sig = ctl.signal;
    let dead = false;
    (async () => {
      try {
        const first = prevStore.current !== store;
        const hadStore = prevStore.current !== "";
        prevStore.current = store;
        if (first && hadStore) setCrumb([]);
        if (first) { setData(null); setMode(""); setEns([]); setLinks([]); setFlux({}); }
        setErr(""); setSelId(null); setNeighbors([]);
        setBranches((b) => (b.length ? [] : b));
        setEnsFull([]);
        setStage(depth === 0 ? "aperçu du store…" : `ouverture de ${crumb[depth - 1]}…`);
        const ov = await agg(store + "|ov", () => fetchOverview(store, sig));
        if (dead) return;
        setData(ov);

        if (depth === 0) {
          const modsR = await agg(store + "|mods", () => fetchModules(store, sig));
          if (dead) return;
          const mods = (modsR.modules || []) as { name: string; count: number }[];
          const rank: number | null = modsR.rank ?? null;
          setRootRank(rank);
          const thr = Math.max(50, Math.round((ov.nodes || 1) / 4000));
          const mFull = toEns(mods.filter((x) => x.count >= thr), 200);
          const m = mFull.slice(0, 44);
          setEnsFull(mFull);
          const names = new Set(mFull.map((x) => x.name));
          const strong = m.length >= 3 && rank != null && (mods[0]?.count || 0) >= (ov.nodes || 1) * 0.02;
          if (strong) {
            // PROJETS immédiat : pas d'attente des arcs (parfois vides = coût
            // perdu) ; on affiche les branches tout de suite, puis si des arcs
            // inter-ensembles arrivent on passe en « tissu » (modules).
            rootKind.current = "projets";
            setEns(m); setLinks([]); setRootLinks([]); setMode("projets"); setExt([]);
            setStage("projets — branches…");
            agg(store + "|arcs", () => fetchModuleArcs(store, 48, 320, sig)).then((arcs) => {
              if (dead || arcs.length < 3) return;
              rootKind.current = "modules";
              const ls: Link[] = arcs.filter((a) => names.has(a.src) && names.has(a.dst))
                .map((a) => ({ src: a.src, dst: a.dst, weight: a.weight }));
              setEns(m); setLinks(ls); setRootLinks(ls); setExt([]);
              setMode("modules"); // morph tissu (arcs réels présents)
            }).catch(() => {});
            const top6 = m.slice(0, Math.min(6, m.length));
            const vals = await Promise.allSettled(top6.map((pr) =>
              agg(store + "|br|" + pr.name, () => fetchSpace(store, pr.name, (rank ?? 2) + 1, 14, sig, false))));
            if (dead) return;
            const bb: { parent: string; kids: Ens[] }[] = [];
            vals.forEach((r, i) => {
              if (r.status === "fulfilled") {
                const kids = toEns((r.value as any).ensembles || [], 3);
                if (kids.length) bb.push({ parent: top6[i].name, kids });
              }
            });
            setBranches(bb);
          } else {
            // départage modules vs familles : seuls les stores faibles ont
            // besoin des arcs pour trancher
            const arcs = await agg(store + "|arcs", () => fetchModuleArcs(store, 48, 320, sig));
            if (dead) return;
            if (arcs.length >= 3 && m.length >= 3) {
              rootKind.current = "modules";
              const ls: Link[] = arcs.filter((a) => names.has(a.src) && names.has(a.dst))
                .map((a) => ({ src: a.src, dst: a.dst, weight: a.weight }));
              setEns(m); setLinks(ls); setRootLinks(ls); setMode("modules"); setExt([]);
            } else {
              const lab = (ov.by_label || []).filter((x: any) => x.count > 0).slice(0, 14);
              setEns(lab.map((x: any, i: number) => ({
                name: x.label, count: x.count, color: COL[x.label] || MOD_COL[i % MOD_COL.length] })));
              setMode("familles"); setStage("flux internes…");
              const lp = await agg(store + "|pairs", () => fetchLabelPairs(store, sig));
              if (dead) return;
              const lnames = new Set(lab.map((x: any) => x.label));
              const acc = new Map<string, number>();
              lp.cross.forEach((c) => {
                if (!lnames.has(c.src) || !lnames.has(c.dst)) return;
                acc.set(`${c.src}\u0001${c.dst}`, (acc.get(`${c.src}\u0001${c.dst}`) || 0) + c.weight);
              });
              setLinks([...acc.entries()]
                .map(([k, weight]) => { const [src, dst] = k.split("\u0001"); return { src, dst, weight }; })
                .sort((x, y) => y.weight - x.weight).slice(0, 200));
              setFlux(lp.same);
            }
          }
          setStage("");
        } else {
          let rr = rootRank;
          if (rr == null) {
            const mr = await agg(store + "|mods", () => fetchModules(store, sig));
            rr = mr.rank ?? 2;
            setRootRank(rr);
          }
          const rank = rr + depth;
          const sp = await agg(`${store}|sp|${prefixNow}|${rank}`, () => fetchSpace(store, prefixNow, rank, 60, sig));
          if (dead) return;
          const kidsFull = toEns(sp.ensembles || [], 200);
          if (kidsFull.length < 2) {
            if (onDrill) {
              const leaf = crumb[depth - 1];
              setStage(`… ${leaf} : pas de sous-ensembles — vue unitaire`);
              setCrumb((c) => c.slice(0, -1));
              drillInto(leaf);
            } else setCrumb((c) => c.slice(0, -1));
            return;
          }
          const kids = kidsFull.slice(0, 48);
          setEnsFull(kidsFull);
          const knames = new Set(kidsFull.map((x) => x.name));
          const ls: Link[] = (sp.arcs || [])
            .filter((a) => knames.has(a.src) && knames.has(a.dst))
            .slice(0, 280)
            .map((a) => ({ src: a.src, dst: a.dst, weight: a.weight }));
          setEns(kids); setLinks(ls);
          setMode(depth >= 1 ? rootKind.current : "modules");
          setStage("");
          if (depth === 1 && rootLinks.length > 0) {
            const parent = crumb[0];
            const others = rootLinks
              .filter((l) => l.src === parent || l.dst === parent)
              .map((l) => ({ name: l.src === parent ? l.dst : l.src, w: l.weight }))
              .sort((a, b) => b.w - a.w).slice(0, 8)
              .map((o, i) => ({ name: o.name, color: MOD_COL[hashC(o.name) % MOD_COL.length], w: o.w }));
            setExt(others);
          } else setExt([]);
          // ARBRE DÉVELOPPÉ (2e saut) : dans un sous-espace projets, on déploie
          // aussi les enfants des 4 ensembles dominants → vue multi-hop directe
          if (rootKind.current === "projets" && depth >= 1) {
            const topB = kids.slice(0, 4);
            const vals2 = await Promise.allSettled(topB.map((k) =>
              agg(store + "|br2|" + prefixNow + "." + k.name, () =>
                fetchSpace(store, (prefixNow ? prefixNow + "." : "") + k.name, rank + 1, 8, sig, false))));
            if (dead) return;
            const bb2: { parent: string; kids: Ens[] }[] = [];
            vals2.forEach((r, i) => {
              if (r.status === "fulfilled") {
                const sub = toEns((r.value as any).ensembles || [], 2);
                if (sub.length) bb2.push({ parent: topB[i].name, kids: sub });
              }
            });
            if (bb2.length) setBranches(bb2);
          }
        }
      } catch (e: any) { if (!dead && !isAbort(e)) { setErr(String(e?.message || e)); setStage(""); } }
    })();
    return () => { dead = true; ctl.abort(); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [store, crumbKey]);

  // La sélection devient le SUJET de la pièce : les autres faces le liront.
  // (Ce miroir est ce qui fait qu'on peut viser ici et retrouver la même chose
  //  ailleurs — « toutes les faces d'une même pièce ».)
  useEffect(() => {
    if (!selId) return;
    setSubject({ store, qn: selId, label: String(selId).slice(0, 42), via: "monde" });
  }, [selId, store, setSubject]);

  // ── clavier : GRAMMAIRE COMMUNE (voir ../graph/grammar.ts) ──────────────
  // Les quatre vues partageaient quatre grammaires différentes. Celle-ci
  // reprend exactement les gestes communs (↑ remonter, ↓/Entrée entrer,
  // ⌂/r tout voir, +/− densifier, Échap fermer, h aide) ; les touches propres
  // à la vue (ici `t` pour la coupe de tête) restent en supplément explicite.
  useEffect(() => {
    const detach = bindViewKeys({
      escape: () => { setSelId(null); apiRef.current.sel = null; clearPinned(); },
      help: () => setHelpOn((o) => !o),
      up: () => {
        if (crumbKey) { setSelId(null); apiRef.current.sel = null; clearPinned(); setCrumb((c) => c.slice(0, -1)); }
      },
      home: () => toRoot(),
      down: () => { if (selId) { const n = selId; apiRef.current.sel = null; setSelId(null); clearPinned(); descendTo(n); } },
      zoomIn: () => doZoom(0.7),
      zoomOut: () => doZoom(1.4),
    });
    // Extension propre à la vue, hors grammaire commune.
    const extra = (e: KeyboardEvent) => {
      const tag = (e.target as HTMLElement)?.tagName || "";
      if (tag === "INPUT" || tag === "TEXTAREA" || e.metaKey || e.ctrlKey || e.altKey) return;
      const k = e.key.toLowerCase();
      if (k === "t") { e.preventDefault(); applyTop(!topRef.current); }
      // ── `d` — DENSIFIER LE NIVEAU (la même touche que dans les autres vues) ──
      // Cette vue pagine DÉJÀ ses ensembles (« ⤵ révéler N de plus »), mais avec
      // son propre pas (48) et sans touche : la même loi avait donc deux réglages
      // et deux gestes selon la face regardée. Ici elle rejoint `densite.ts` —
      // même pas, même page, même signification (densifier ≠ approfondir).
      if (k === "d" && ensFull.length > ens.length) {
        e.preventDefault();
        setEns((cur) => ensFull.slice(0, pageSuivante(cur.length, ensFull.length)));
      }
      if (e.key === "Backspace" && crumbKey) { e.preventDefault(); setCrumb((c) => c.slice(0, -1)); }
    };
    window.addEventListener("keydown", extra);
    return () => { detach(); window.removeEventListener("keydown", extra); };
  });

  // ── « pages » : chaque vue a une URL stable (?store=&dive=…) + titre ──
  useEffect(() => {
    const p = new URLSearchParams(window.location.search);
    if (store) p.set("store", store);
    if (crumbKey) p.set("dive", crumbKey.replace(/\./g, "/"));
    else p.delete("dive");
    const q = p.toString();
    const url = q ? window.location.pathname + "?" + q : window.location.pathname;
    // historique réel : chaque niveau = une entrée → précédent/suivant fluides
    if (window.location.pathname + window.location.search !== url) {
      window.history.pushState(null, "", url);
    }
  }, [crumbKey, store]);
  useEffect(() => {
    const onPop = () => {
      const p = new URLSearchParams(window.location.search);
      const d = (p.get("dive") || "").split("/").filter(Boolean);
      setCrumb(d);
      setSelId(null); apiRef.current.sel = null; clearPinned(); setHoverName(null);
    };
    window.addEventListener("popstate", onPop);
    return () => window.removeEventListener("popstate", onPop);
  }, []);
  useEffect(() => {
    if (!data) return;
    const t = mode === "familles" ? "Familles"
      : crumbKey ? crumbKey.replace(/\./g, " · ") : "Tous les projets";
    document.title = `${t} — Obrain`;
  }, [crumbKey, mode, data]);

  // ── scène (rendu constellation) ──
  useEffect(() => {
    const el = host.current;
    const lbl = labelsRef.current;
    if (!el || !lbl || !data) return;
    const full = mode !== "" && ens.length > 0;
    const sub = depth > 0;
    const api = apiRef.current;
    api.discs = []; api.edges = []; api.rings = []; api.sats = []; api.pos = new Map(); api.satPos = new Map();
    api.hover = null; api.sel = null; api.focus = null;
    api.scene = null; api.ctx = null; api.ghostDiscs = []; api.lineage = null; api.lineageDiscs = [];
    api.branch = null; api.branchDiscs = []; api.hoverG = null; api.hoverDiscs = [];
    for (const d of api.ctxLabels.values()) d.remove();
    api.ctxLabels = new Map(); api.ctxPos = new Map();
    for (const d of api.lineageLabels.values()) d.remove();
    api.lineageLabels = new Map(); api.lineagePos = new Map();
    for (const d of api.branchLabels.values()) d.remove();
    api.branchLabels = new Map(); api.branchPos = new Map();
    for (const d of api.hoverLabels.values()) d.remove();
    api.hoverLabels = new Map(); api.hoverPos = new Map();
    const R = 250;

    const scene = new THREE.Scene();
    api.scene = scene;
    api.ctx = new THREE.Group(); scene.add(api.ctx);
    api.lineage = new THREE.Group(); scene.add(api.lineage);
    api.branch = new THREE.Group(); scene.add(api.branch);
    api.hoverG = new THREE.Group(); scene.add(api.hoverG);
    const w = el.clientWidth, h = el.clientHeight;
    const camera = makeCamera(w, h);
    if (topRef.current) {
      camera.up.set(0, 0, -1);
      camera.position.set(0, 1600, 4); camera.lookAt(0, 0, 0);
    } else {
      camera.up.set(0, 1, 0);
      camera.position.set(0, 150, sub ? 380 : 800); camera.lookAt(0, -10, 0);
    }
    // Relief COMMUN, calé sur la distance de cadrage RÉELLE de la vue
    // (vue de dessus à 1600, vue de face à ~800) — l'ancien FogExp2 dupliquait
    // le brouillard avec ses propres constantes.
    applyRelief(scene, camera.position.length());
    camRef.current = camera;
    const renderer = makeRenderer(w, h);
    renderer.domElement.style.touchAction = "none";
    el.appendChild(renderer.domElement);
    const cv = renderer.domElement;
    cv.style.opacity = "0.15";
    // Profil de caméra COMMUN (voir ../graph/grammar.ts).
    const ctrl = orbitFor(camera, renderer.domElement, { minDistance: 120, maxDistance: 2600 });
    (ctrl as any).touches = { ONE: (THREE as any).TOUCH.ROTATE, TWO: (THREE as any).TOUCH.DOLLY_PAN };
    ctrlRef.current = ctrl;
    ctrl.target.set(0, topRef.current ? 0 : -10, 0);
    ctrl.update();
    // transition fluide : conserver la pose caméra d'un niveau à l'autre
    const kp = keepPose.current;
    if (kp && keepStore.current === store && kp.top === topRef.current) {
      camera.up.copy(kp.up);
      camera.position.copy(kp.p);
      ctrl.target.copy(kp.t);
      ctrl.update();
    }
    scene.add(new THREE.AmbientLight(0x8899aa, 1.0));

    // ── sol quadrillé discret (référence de profondeur, neutre) ──
    const gridCol = 0x1a2733;
    const S = R * 1.7;
    {
      const seg = 12;
      for (let i = 0; i <= seg; i++) {
        const t = -S + (2 * S * i) / seg;
        const gz = new THREE.BufferGeometry().setFromPoints([new THREE.Vector3(t, -46, -S), new THREE.Vector3(t, -46, S)]);
        scene.add(new THREE.Line(gz, new THREE.LineBasicMaterial({ color: gridCol, transparent: true, opacity: 0.22 })));
        const gx = new THREE.BufferGeometry().setFromPoints([new THREE.Vector3(-S, -46, t), new THREE.Vector3(S, -46, t)]);
        scene.add(new THREE.Line(gx, new THREE.LineBasicMaterial({ color: gridCol, transparent: true, opacity: 0.22 })));
      }
    }

    // ── positions des ensembles : disposition organique 3D (liens réels) ──
    if (full) {
      const posMap = forceLayout(ens, links, R, sub);
      for (const [name, v] of posMap) api.pos.set(name, v);
    }

    // ── satellites (contexte) ──
    if (full && sub && ext.length > 0 && mode !== "familles") {
      const R2 = R;
      ext.forEach((s, si) => {
        const ang = (si / Math.max(1, ext.length)) * Math.PI * 2 - Math.PI / 2;
        const v = new THREE.Vector3(Math.cos(ang) * R2, (hashC(s.name) % 16) - 8, Math.sin(ang) * R2);
        api.satPos.set(s.name, v);
        // petit disque satellite
        const dt = discTexture(s.color);
        const sp = new THREE.Sprite(new THREE.SpriteMaterial({ map: dt, transparent: true, depthWrite: false, opacity: 0.85 }));
        (sp as any).userData = { name: s.name, sat: true, r: 9 };
        sp.scale.set(18, 18, 1);
        sp.position.copy(v);
        scene.add(sp);
        api.discs.push(sp);
        // lien vers le centre (liens vers l'extérieur)
        const curve: THREE.Vector3[] = [];
        const mid = v.clone().multiplyScalar(0.5); mid.y += 22;
        for (let k = 0; k <= 16; k++) { const t = k / 16, it = 1 - t;
          curve.push(new THREE.Vector3(it * it * v.x + 2 * it * t * mid.x + t * t * 0,
            it * it * v.y + 2 * it * t * mid.y, it * it * v.z + 2 * it * t * mid.z)); }
        const g = new THREE.BufferGeometry().setFromPoints(curve);
        const m = new THREE.LineBasicMaterial({ color: new THREE.Color(s.color), transparent: true, opacity: 0.28 });
        const ln = new THREE.Line(g, m);
        ln.visible = layersRef.current.edges;
        scene.add(ln);
        api.sats.push(ln);
      });
    }

    // ── nœuds-disques lumineux + étiquettes DOM ──
    const labelEls = new Map<string, HTMLDivElement>();
    if (full) {
      const maxC = Math.max(1, ...ens.map((e) => e.count));
      const subF = sub ? 0.8 : 1;
      ens.forEach((e) => {
        const p = api.pos.get(e.name); if (!p) return;
        const r = subF * (10 + 15 * Math.sqrt(e.count / maxC));
        // halo doux
        const gt = glowTexture(e.color);
        const glow = new THREE.Sprite(new THREE.SpriteMaterial({ map: gt, transparent: true,
          depthWrite: false, opacity: 0.55, blending: THREE.AdditiveBlending }));
        glow.position.copy(p); glow.scale.set(r * 5, r * 5, 1); glow.renderOrder = 0;
        scene.add(glow);
        // disque
        const dt = discTexture(e.color);
        const sp = new THREE.Sprite(new THREE.SpriteMaterial({ map: dt, transparent: true, depthWrite: false }));
        (sp as any).userData = { name: e.name, r };
        sp.scale.set(r * 2, r * 2, 1); sp.position.copy(p); sp.renderOrder = 1;
        scene.add(sp);
        api.discs.push(sp);
        // étiquette DOM (nette)
        const div = document.createElement("div");
        div.className = "monde-label";
        div.innerHTML = `<span class="ml-dot" style="background:${e.color}"></span><span class="ml-name">${e.name.replace(/</g, "&lt;")}</span><span class="ml-cnt">${fmt(e.count)}</span>`;
        lbl.appendChild(div);
        labelEls.set(e.name, div);
      });
    }

    // ── lignée persistante (ancêtres chaînés, tour verticale à gauche) ──
    if (full && depth > 0 && api.lineage) {
      const lg = api.lineage;
      const chain = ["Projets", ...crumb];
      let prevY: THREE.Vector3 | null = null;
      chain.forEach((an, i) => {
        const y = -30 + i * 52;
        const p = new THREE.Vector3(-R * 1.22, y, 0);
        const gt = glowTexture("#c8a45c");
        const glow = new THREE.Sprite(new THREE.SpriteMaterial({ map: gt, transparent: true, depthWrite: false, opacity: 0.35, blending: THREE.AdditiveBlending }));
        glow.position.copy(p); glow.scale.set(30, 30, 1); lg.add(glow);
        const dt = discTexture("#c8a45c");
        const sp = new THREE.Sprite(new THREE.SpriteMaterial({ map: dt, transparent: true, depthWrite: false, opacity: 0.7 }));
        (sp as any).userData = { name: an, lineage: true };
        sp.position.copy(p); sp.scale.set(15, 15, 1); lg.add(sp);
        api.lineageDiscs.push({ sp, name: an });
        const div = document.createElement("div");
        div.className = "monde-label ghost";
        div.innerHTML = `<span class="ml-dot" style="background:#c8a45c"></span><span class="ml-name">${i === 0 ? "🏠 " : "↰ "}${an.replace(/</g, "&lt;")}</span>`;
        lbl.appendChild(div);
        api.lineageLabels.set(an, div);
        api.lineagePos.set(an, p.clone());
        if (prevY) {
          const g = new THREE.BufferGeometry().setFromPoints([prevY, p]);
          const m = new THREE.LineBasicMaterial({ color: 0xc8a45c, transparent: true, opacity: 0.4, depthWrite: false });
          lg.add(new THREE.Line(g, m));
        }
        prevY = p;
      });
    }

    // ── branches racine (mode projets sans liens) : vrais enfants en aperçu,
    //    suspendus sous chaque projet → la profondeur est démontrée d'emblée
    if (full && mode === "projets" && branches.length > 0 && api.branch) {
      const bg = api.branch;
      branches.forEach((br) => {
        const pp = api.pos.get(br.parent);
        if (!pp) return;
        br.kids.forEach((k, j) => {
          const p = pp.clone().add(new THREE.Vector3((j - 1) * 72, -84 - (j % 2) * 14, 0));
          const gt = glowTexture(k.color);
          const glow = new THREE.Sprite(new THREE.SpriteMaterial({ map: gt, transparent: true, depthWrite: false, opacity: 0.32, blending: THREE.AdditiveBlending }));
          glow.position.copy(p); glow.scale.set(26, 26, 1); bg.add(glow);
          const dt = discTexture(k.color);
          const sp = new THREE.Sprite(new THREE.SpriteMaterial({ map: dt, transparent: true, depthWrite: false, opacity: 0.8 }));
          (sp as any).userData = { name: k.name, branch: true, parent: br.parent };
          sp.position.copy(p); sp.scale.set(12, 12, 1); bg.add(sp);
          api.branchDiscs.push({ sp, name: k.name, parent: br.parent, count: k.count });
          api.branchBase.set(br.parent + "\u0001" + k.name, sp.position.clone());
          const div = document.createElement("div");
          div.className = "monde-label ghost";
          div.innerHTML = `<span class="ml-dot" style="background:${k.color}"></span><span class="ml-name">↳ ${k.name.replace(/</g, "&lt;")}</span>`;
          lbl.appendChild(div);
          const key = k.name + "·" + br.parent;
          api.branchLabels.set(key, div);
          api.branchPos.set(key, p.clone());
          // parent → enfant : la géométrie raconte l'arbre
          const g = new THREE.BufferGeometry().setFromPoints([pp, p]);
          const m = new THREE.LineBasicMaterial({ color: new THREE.Color(k.color), transparent: true, opacity: 0.35, depthWrite: false });
          bg.add(new THREE.Line(g, m));
        });
      });
    }

    // ── arêtes : courbes fines + flux animé ──
    const flow: { edge: number; offset: number }[] = [];
    if (full) {
      const linkData = links.slice(0, 220);
      const maxW = Math.max(1, ...linkData.map((l) => l.weight));
      linkData.forEach((l) => {
        const p = api.pos.get(l.src), q = api.pos.get(l.dst);
        if (!p || !q) return;
        const n = Math.sqrt(Math.min(1, l.weight / maxW));
        const lift = (sub ? 8 : 14) + n * (sub ? 34 : 60);
        const mid = p.clone().add(q).multiplyScalar(0.5); mid.y += lift;
        const pts: THREE.Vector3[] = [];
        for (let k = 0; k <= 24; k++) {
          const t = k / 24, it = 1 - t;
          pts.push(new THREE.Vector3(it * it * p.x + 2 * it * t * mid.x + t * t * q.x,
            it * it * p.y + 2 * it * t * mid.y + t * t * q.y,
            it * it * p.z + 2 * it * t * mid.z + t * t * q.z));
        }
        const c0 = new THREE.Color(0x3f7fd6), c1 = new THREE.Color(0x7fd6ff);
        const col = c0.lerp(c1, Math.min(1, 0.2 + n * 0.8));
        const g = new THREE.BufferGeometry().setFromPoints(pts);
        const m = new THREE.LineBasicMaterial({ color: col, transparent: true, opacity: 0.16 + n * 0.5,
          blending: THREE.AdditiveBlending, depthWrite: false });
        const line = new THREE.Line(g, m);
        line.visible = layersRef.current.edges;
        scene.add(line);
        api.edges.push({ line, s: l.src, d: l.dst, w: l.weight, pts });
        // particules de flux sur les arêtes dominantes
        if (flow.length < 260) {
          const pn = Math.min(3, 1 + Math.round(n * 3));
          for (let k = 0; k < pn; k++) flow.push({ edge: api.edges.length - 1, offset: Math.random() });
        }
      });
      api.edges.sort((x, y) => y.w - x.w);

      // anneaux de flux (mode familles)
      if (mode === "familles") {
        ens.forEach((e) => {
          const f = flux[e.name] ?? 0;
          if (f <= 0) return;
          const p = api.pos.get(e.name); if (!p) return;
          const rr = 26 + Math.min(46, 9 * Math.log10(f + 1));
          const rp: THREE.Vector3[] = [];
          for (let k = 0; k <= 64; k++) { const a = (k / 64) * Math.PI * 2;
            rp.push(new THREE.Vector3(p.x + Math.cos(a) * rr, p.y - 4, p.z + Math.sin(a) * rr)); }
          const g = new THREE.BufferGeometry().setFromPoints(rp);
          const m = new THREE.LineBasicMaterial({ color: new THREE.Color(e.color), transparent: true,
            depthWrite: false, opacity: Math.min(0.9, 0.3 + Math.log10(f + 1) * 0.08) });
          const ring = new THREE.Line(g, m);
          ring.visible = layersRef.current.edges;
          scene.add(ring);
          api.rings.push(ring);
        });
      }
    }

    // particules de flux (Points)
    const NP = flow.length;
    const flowPos = new Float32Array(NP * 3);
    let flowGeo: THREE.BufferGeometry | null = null;
    let flowPts: THREE.Points | null = null;
    if (NP > 0) {
      flowGeo = new THREE.BufferGeometry();
      flowGeo.setAttribute("position", new THREE.BufferAttribute(flowPos, 3));
      flowPts = new THREE.Points(flowGeo, new THREE.PointsMaterial({
        color: 0xa7e3ff, size: 2.2, sizeAttenuation: false, transparent: true, opacity: 0.9,
        blending: THREE.AdditiveBlending, depthWrite: false,
      }));
      scene.add(flowPts);
    }

    // ── boucle : LOD des arêtes + flux + étiquettes + focus ──
    const proj = new THREE.Vector3();
    let raf = 0, lastLod = 0, inter = false, lastMap = 0, lastLbl = 0;
    ctrl.addEventListener("start", () => { inter = true; api.focus = null; });
    ctrl.addEventListener("end", () => { inter = false; });
    const ringR = R * (sub ? 0.62 : 1);
    // DEGRÉ RÉEL : poids cumulé des liens de chaque ensemble (pour le critère θ)
    const weightByEns = new Map<string, number>();
    for (const l of links) {
      weightByEns.set(l.src, (weightByEns.get(l.src) || 0) + l.weight);
      weightByEns.set(l.dst, (weightByEns.get(l.dst) || 0) + l.weight);
    }
    const loop = () => {
      raf = requestAnimationFrame(loop);
      const now = performance.now();
      const fc = api.focus;
      if (fc) {
        const to = ctrl.target;
        const view = camera.position.clone().sub(to);
        const len = Math.max(120, view.length());
        to.lerp(fc, 0.10);
        const desired = fc.clone().add(view.clone().setLength(len * 0.9));
        camera.position.lerp(desired, 0.10);
        camera.lookAt(to);
        if (camera.position.distanceTo(desired) < 3) api.focus = null;
      }
      if (now - lastLod > 120) {
        lastLod = now;
        const dist = camera.position.distanceTo(ctrl.target);
        const budget = dist > ringR * 3.6 ? 16 : dist > ringR * 2.2 ? 42 : dist > ringR * 1.4 ? 110 : 100000;
        const top = Math.max(1, api.edges[0]?.w || 1);
        const pin = pinnedRef.current;
        const anyPin = pin.size > 0;
        for (let i = 0; i < api.edges.length; i++) {
          const a = api.edges[i];
          const hl = api.hover != null && (api.hover === a.s || api.hover === a.d);
          a.line.visible = i < budget && layersRef.current.edges;
          const inPin = !anyPin || (pin.has(a.s) && pin.has(a.d));
          (a.line.material as THREE.LineBasicMaterial).opacity = inPin
            ? (hl ? 1 : (0.16 + Math.sqrt(Math.min(1, a.w / top)) * 0.5))
            : 0.04;
        }
        // atténuer les nœuds non-épinglés (focus multi-sélection)
        api.discs.forEach((sp) => {
          const ud = (sp as any).userData;
          const base = ud.sat ? 0.85 : 1;
          (sp.material as THREE.SpriteMaterial).opacity = (!anyPin || pin.has(ud.name)) ? base : 0.2;
        });
        api.ghostDiscs.forEach((g) => {
          (g.sp.material as THREE.SpriteMaterial).opacity = (!anyPin || pin.has(g.name)) ? 0.72 : 0.16;
        });
        if (flowPts) flowPts.visible = layersRef.current.edges;
      }
      // flux animé
      if (flowPts && layersRef.current.edges) {
        const t = now / 2200;
        for (let i = 0; i < flow.length; i++) {
          const e = api.edges[flow[i].edge];
          if (!e) continue;
          const tt = (t + flow[i].offset) % 1;
          const idx = Math.min(e.pts.length - 2, Math.floor(tt * (e.pts.length - 1)));
          const lt = (tt * (e.pts.length - 1)) - idx;
          const A = e.pts[idx], B = e.pts[idx + 1];
          flowPos[i * 3] = A.x + (B.x - A.x) * lt;
          flowPos[i * 3 + 1] = A.y + (B.y - A.y) * lt;
          flowPos[i * 3 + 2] = A.z + (B.z - A.z) * lt;
        }
        if (flowGeo) (flowGeo.getAttribute("position") as THREE.BufferAttribute).needsUpdate = true;
      }
      if (now - lastLbl > 100) { lastLbl = now;
// étiquettes DOM projetées (nettes)
      for (const [name, div] of labelEls) {
        const p = api.pos.get(name); if (!p) continue;
        proj.copy(p).project(camera);
        if (proj.z > 1 || proj.z < -1) { div.style.display = "none"; continue; }
        const x = (proj.x * 0.5 + 0.5) * w;
        const y = (-proj.y * 0.5 + 0.5) * h;
        div.style.display = "block";
        div.style.transform = `translate(-50%, 0) translate(${x.toFixed(1)}px, ${(y + 14).toFixed(1)}px)`;
        div.classList.toggle("on", api.hover === name || api.sel === name);
        div.classList.toggle("dim", pinnedRef.current.size > 0 && !pinnedRef.current.has(name));
      }
      // étiquettes du contexte hiérarchique (↑ ancêtres / ↓ enfants)
      for (const [name, div] of api.ctxLabels) {
        const rec = api.ctxPos.get(name); if (!rec) continue;
        proj.copy(rec.p).project(camera);
        if (proj.z > 1 || proj.z < -1) { div.style.display = "none"; continue; }
        const x = (proj.x * 0.5 + 0.5) * w;
        const y = (-proj.y * 0.5 + 0.5) * h;
        div.style.display = "block";
        div.style.transform = `translate(-50%, 0) translate(${x.toFixed(1)}px, ${(y + 8).toFixed(1)}px)`;
        div.classList.toggle("dim", pinnedRef.current.size > 0 && rec.kind === "child" && !pinnedRef.current.has(name));
      }
      // étiquettes de la lignée (ancêtres chaînés)
      for (const [name, div] of api.lineageLabels) {
        const p = api.lineagePos.get(name); if (!p) continue;
        proj.copy(p).project(camera);
        if (proj.z > 1 || proj.z < -1) { div.style.display = "none"; continue; }
        const x = (proj.x * 0.5 + 0.5) * w;
        const y = (-proj.y * 0.5 + 0.5) * h;
        div.style.display = "block";
        div.style.transform = `translate(-50%, 0) translate(${x.toFixed(1)}px, ${(y + 6).toFixed(1)}px)`;
      }
      // étiquettes des branches racine
      for (const [key, div] of api.branchLabels) {
        const p = api.branchPos.get(key); if (!p) continue;
        proj.copy(p).project(camera);
        if (proj.z > 1 || proj.z < -1) { div.style.display = "none"; continue; }
        const x = (proj.x * 0.5 + 0.5) * w;
        const y = (-proj.y * 0.5 + 0.5) * h;
        div.style.display = "block";
        div.style.transform = `translate(-50%, 0) translate(${x.toFixed(1)}px, ${(y + 6).toFixed(1)}px)`;
      }
      // étiquettes des rameaux de survol
      for (const [key, div] of api.hoverLabels) {
        const p = api.hoverPos.get(key); if (!p) continue;
        proj.copy(p).project(camera);
        if (proj.z > 1 || proj.z < -1) { div.style.display = "none"; continue; }
        const x = (proj.x * 0.5 + 0.5) * w;
        const y = (-proj.y * 0.5 + 0.5) * h;
        div.style.display = "block";
        div.style.transform = `translate(-50%, 0) translate(${x.toFixed(1)}px, ${(y + 6).toFixed(1)}px)`;
      }
      }
      if (mapOnR.current && now - lastMap > 250) { lastMap = now; paintMap(); }
      // REPLI FRACTAL des branches de connaissances : chaque enfant rejoint son
      // parent (repli) ou revient à sa position d'observation (dépli).
      if (api.branch) {
        // index nom → sprite des branches (pour un repli EN CASCADE : un parent
        // qui se replie entraîne ses enfants, niveau par niveau)
        if (!(api as any)._byName) {
          const m = new Map<string, THREE.Sprite>();
          for (const d of api.branchDiscs) m.set(d.name, d.sp);
          (api as any)._byName = m;
        }
        const byName = (api as any)._byName as Map<string, THREE.Sprite>;
        // critère : part (count) OU poids réel des liens (degré) — au choix
        const mode = thetaModeRef.current;
        const score = (name: string, count: number) =>
          mode === "poids" ? (weightByEns.get(name) || 0) : count;
        const tot = new Map<string, number>();
        for (const br of branches) {
          const s2 = br.kids.reduce((a, k) => a + score(k.name, k.count), 0);
          tot.set(br.parent, s2 || 1);
        }
        const th = thetaRef.current;
        for (const d of api.branchDiscs) {
          const base = api.branchBase.get(d.parent + "\u0001" + d.name);
          if (!base) continue;
          const share = score(d.name, d.count) / (tot.get(d.parent) || 1);
          // FORMULE : une branche sous le seuil reste repliée (non déployée)
          const under = share < th;
          const parentLive = byName.get(d.parent)?.position;
          const target = (foldRef.current || under)
            ? (parentLive || api.branchPos.get(d.parent) || base) : base;
          d.sp.position.lerp(target, 0.12);
        }
      }
      ctrl.update(); renderer.render(scene, camera);
    };
    loop();

    // ── interaction ──
    const ray = new THREE.Raycaster(); const mouse = new THREE.Vector2();
    const dom = renderer.domElement;
    const down = { x: 0, y: 0 }; let dragging = false; let lastMove = 0;
    dom.onpointerdown = (ev: PointerEvent) => { down.x = ev.clientX; down.y = ev.clientY; dragging = false; };
    dom.onpointerup = (ev: PointerEvent) => { if (Math.hypot(ev.clientX - down.x, ev.clientY - down.y) > 6) dragging = true; };
    const pickAt = (ev: { clientX: number; clientY: number }): { name: string; sat: boolean; ghost?: boolean; lineage?: boolean; branch?: boolean; parent?: string; kind?: "anc" | "child" } | null => {
      if (!full) return null;
      const r = dom.getBoundingClientRect();
      mouse.set(((ev.clientX - r.left) / r.width) * 2 - 1, -((ev.clientY - r.top) / r.height) * 2 + 1);
      ray.setFromCamera(mouse, camera);
      const hit = ray.intersectObjects(api.discs, false)[0];
      if (hit) {
        const ud = (hit.object as any).userData;
        return { name: ud.name, sat: !!ud.sat };
      }
      const ln = ray.intersectObjects(api.lineageDiscs.map((g) => g.sp), false)[0];
      if (ln) {
        const ud = (ln.object as any).userData;
        return { name: ud.name, sat: false, lineage: true };
      }
      const br = ray.intersectObjects(api.branchDiscs.map((g) => g.sp), false)[0];
      if (br) {
        const ud = (br.object as any).userData;
        return { name: ud.name, sat: false, branch: true, parent: ud.parent };
      }
      const hv = ray.intersectObjects(api.hoverDiscs.map((g) => g.sp), false)[0];
      if (hv) {
        const ud = (hv.object as any).userData;
        return { name: ud.name, sat: false, branch: true, parent: ud.parent };
      }
      const gh = ray.intersectObjects(api.ghostDiscs.map((g) => g.sp), false)[0];
      if (gh) {
        const ud = (gh.object as any).userData;
        return { name: ud.name, sat: false, ghost: true, kind: ud.kind };
      }
      return null;
    };
    /**
     * ── LES VOISINS D'UN ENSEMBLE : TOUS, PAR POIDS RÉEL DÉCROISSANT ────────
     *
     * Avant : `.slice(0, 7)`. Deux défauts d'un coup — la liste était coupée
     * SANS LE DIRE (rien n'annonçait qu'il en manquait), et la coupure
     * s'appliquait APRÈS le filtre mais AVANT tout tri : les sept premiers
     * liens dans l'ordre d'insertion, pas les sept plus forts.
     *
     * Ce n'est pas une fratrie pour autant, et il faut le dire : deux fichiers
     * du même dossier sans flux entre eux n'apparaîtront jamais ici. La
     * parenté de STRUCTURE vit dans `graph/fratrie.ts` (facettes « Frères ») ;
     * ici, c'est un voisinage par FLUX, mesuré — désormais complet et annoncé.
     */
    const neighborsOf = (name: string) =>
      api.edges
        .filter((a) => a.s === name || a.d === name)
        .map((a) => ({ name: a.s === name ? a.d : a.s, w: a.w }))
        .sort((a, b) => b.w - a.w);
    dom.onpointermove = (ev: PointerEvent) => {
      if (ev.pointerType !== "mouse" || ev.buttons !== 0) return;
      const t = performance.now(); if (t - lastMove < 60) return; lastMove = t;
      const hit = pickAt(ev);
      const name = hit ? hit.name : null;
      if (name === api.hover) return;
      api.hover = name;
      setHoverName(hit && !hit.sat && !hit.ghost && !hit.lineage && !hit.branch ? name : null);
      dom.style.cursor = name ? "pointer" : "default";
      const tip = tipRef.current; if (!tip) return;
      if (!name) { tip.style.display = "none"; return; }
      if (hit && hit.sat) {
        const s = ext.find((x) => x.name === name);
        tip.textContent = `${name}  ·  ${s ? fmt(s.w) + " appels vers " + prefixNow : ""}  — cliquer pour ouvrir`;
      } else {
        const e = ens.find((x) => x.name === name);
        const fl = flux[name];
        const str = api.edges.filter((a) => a.s === name || a.d === name)
          .sort((a, b) => b.w - a.w)[0];
        const sTxt = str ? ` · ${fmt(str.w)} → ${str.s === name ? str.d : str.s}` : "";
        tip.textContent = `${name}  ·  ${e ? fmt(e.count) : ""}${neighborsOf(name).length ? "  ·  " + neighborsOf(name).length + " liens" : ""}${sTxt}${fl ? "  ·  flux " + fmt(fl) : ""}${mode !== "familles" ? "  ·  dbl-clic = plonger" : ""}`;
      }
      tip.style.display = "block";
      const r = dom.getBoundingClientRect();
      tip.style.left = (ev.clientX - r.left + 14) + "px";
      tip.style.top = (ev.clientY - r.top - 30) + "px";
    };
    dom.onpointerleave = () => { api.hover = null; setHoverName(null); const tip = tipRef.current; if (tip) tip.style.display = "none"; };
    dom.onclick = (ev: MouseEvent) => {
      if (dragging) return;
      const hit = pickAt(ev);
      if (!hit) { api.sel = null; setSelId(null); setNeighbors([]); clearPinned(); return; }
      if (hit.sat) { descendTo(hit.name); return; }
      if (hit.branch && hit.parent) {
        // rameau (racine ou survol) : plonger dans son sous-espace
        const parent = hit.parent;
        if (crumb.length + 2 <= MAX_DEPTH) setCrumb((c) => [...c, parent, hit.name]);
        setSelId(null); api.sel = null; clearPinned();
        return;
      }
      if (hit.lineage) {
        // ancêtre de la lignée : remonter au niveau correspondant
        if (hit.name === "Projets") { setCrumb([]); setSelId(null); api.sel = null; clearPinned(); return; }
        const i = crumb.indexOf(hit.name);
        if (i >= 0) { setCrumb((c) => c.slice(0, i + 1)); setSelId(null); api.sel = null; clearPinned(); }
        return;
      }
      if (hit.ghost) {
        if (hit.kind === "child") { togglePin(hit.name); return; } // afficher/masquer un enfant
        // ancêtre ↑ : remonter au niveau correspondant (pas de drill)
        if (hit.name === "Projets") { setCrumb([]); setSelId(null); api.sel = null; clearPinned(); return; }
        const i = crumb.indexOf(hit.name);
        if (i >= 0) { setCrumb((c) => c.slice(0, i + 1)); setSelId(null); api.sel = null; clearPinned(); }
        return;
      }
      api.sel = hit.name; setSelId(hit.name);
      togglePin(hit.name);
      setNeighbors(neighborsOf(hit.name));
      setNbTout(false);
    };
    dom.ondblclick = (ev: MouseEvent) => {
      if (dragging) return;
      const hit = pickAt(ev);
      if (!hit) return;
      if (hit.branch && hit.parent) {
        const parent = hit.parent;
        if (crumb.length + 2 <= MAX_DEPTH) setCrumb((c) => [...c, parent, hit.name]);
        setSelId(null); api.sel = null; clearPinned();
        return;
      }
      if (hit.sat) { descendTo(hit.name); return; }
      if (hit.ghost || hit.lineage) return;
      if (mode !== "familles") descendTo(hit.name);
    };

    const onR = () => { const cw = el.clientWidth, ch = el.clientHeight; if (!cw || !ch) return;
      camera.aspect = cw / ch; camera.updateProjectionMatrix();
      renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, RELIEF.pixelRatioCap));
      renderer.setSize(cw, ch); };
    window.addEventListener("resize", onR);
    // fondu d'arrivée
    let fid = requestAnimationFrame(function fade() {
      const p = parseFloat(cv.style.opacity || "0.15");
      const next = Math.min(1, p + 0.12);
      cv.style.opacity = String(next);
      if (next < 1) fid = requestAnimationFrame(fade);
    });
    return () => {
      // mémoriser la pose caméra pour la transition vers le prochain niveau
      const cprev = camRef.current, tprev = ctrlRef.current;
      if (cprev && tprev) {
        keepPose.current = { p: cprev.position.clone(), t: tprev.target.clone(), up: cprev.up.clone(), top: topRef.current };
        keepStore.current = store;
      }
      cancelAnimationFrame(raf); cancelAnimationFrame(fid); window.removeEventListener("resize", onR);
      ctrl.dispose(); ctrlRef.current = null; camRef.current = null;
      el.removeChild(renderer.domElement); renderer.dispose();
      api.discs = []; api.edges = []; api.rings = []; api.sats = []; api.pos = new Map(); api.satPos = new Map();
      api.sel = null; api.hover = null; api.focus = null;
      while (lbl.firstChild) lbl.removeChild(lbl.firstChild);
      setSelId(null); setNeighbors([]);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [data, mode, ens, links, flux, crumbKey, branches]);

  // liens ↔ couche « Appels »
  useEffect(() => {
    const v = layers.edges;
    for (const a of apiRef.current.edges) a.line.visible = v;
    for (const r of apiRef.current.rings) r.visible = v;
    for (const s of apiRef.current.sats) s.visible = v;
  }, [layers.edges]);

  // ── rameaux au survol : PRÉCHAUFFAGE intelligent ──
  // Un seul agrégat réseau à la fois (jamais de rafale) ; une fois parti il
  // N'EST JAMAIS annulé → le résultat est conservé (agg cache) pour les survols
  // suivants. Le rendu n'a lieu que si le survol est toujours sur le nœud.
  useEffect(() => {
    const api = apiRef.current;
    if (api.hoverG) api.hoverG.clear();
    for (const d of api.hoverLabels.values()) d.remove();
    api.hoverLabels = new Map(); api.hoverPos = new Map(); api.hoverDiscs = [];
    hoverGen.current++;
    const gen = hoverGen.current;
    hoverJobR.current = null;
    if (!hoverName || !api.scene || !api.hoverG || mode === "familles") return;
    if (selId === hoverName) return; // le panneau de sélection couvre déjà
    if (branches.some((b) => b.parent === hoverName)) return; // enfants déjà déployés
    if (!api.pos.get(hoverName)) return;
    const rank = (rootRank ?? 2) + depth + 1;
    const cprefix = (prefixNow ? prefixNow + "." : "") + hoverName;
    hoverJobR.current = { name: hoverName, prefix: cprefix, rank };
    const pump = () => {
      if (hoverBusy.current) return;
      const j = hoverJobR.current;
      if (!j) return;
      hoverBusy.current = true;
      agg(`${storeR.current}|hv|${j.prefix}|${j.rank}`,
        () => fetchSpace(storeR.current, j.prefix, j.rank, 12, undefined, false))
        .then((sp) => {
          const api2 = apiRef.current;
          if (gen !== hoverGen.current) return; // contexte changé : on garde en cache
          if (hoverNameR.current !== j.name || selIdR.current === j.name) return;
          if (!api2.hoverG) return;
          const node2 = api2.pos.get(j.name);
          if (!node2) return;
          const kids = (sp.ensembles || []).filter((x: any) => ensGood(x.name) && x.count >= 2).slice(0, 3);
          const g = api2.hoverG!;
          const lbl = labelsRef.current!;
          kids.forEach((k: any, i: number) => {
            const p = node2.clone().add(new THREE.Vector3((i - (kids.length - 1) / 2) * 62, -58 - (i % 2) * 12, 0));
            const col = MOD_COL[i % MOD_COL.length];
            const gt = glowTexture(col);
            const glow = new THREE.Sprite(new THREE.SpriteMaterial({ map: gt, transparent: true, depthWrite: false, opacity: 0.28, blending: THREE.AdditiveBlending }));
            glow.position.copy(p); glow.scale.set(22, 22, 1); g.add(glow);
            const dt = discTexture(col);
            const sp = new THREE.Sprite(new THREE.SpriteMaterial({ map: dt, transparent: true, depthWrite: false, opacity: 0.8 }));
            (sp as any).userData = { name: k.name, branch: true, parent: j.name };
            sp.position.copy(p); sp.scale.set(10, 10, 1); g.add(sp);
            api2.hoverDiscs.push({ sp, name: k.name, parent: j.name });
            const div = document.createElement("div");
            div.className = "monde-label ghost";
            div.innerHTML = `<span class="ml-dot" style="background:${col}"></span><span class="ml-name">↳ ${k.name.replace(/</g, "&lt;")}</span>`;
            lbl.appendChild(div);
            const key = k.name + "·" + j.name;
            api2.hoverLabels.set(key, div);
            api2.hoverPos.set(key, p.clone());
            const g2 = new THREE.BufferGeometry().setFromPoints([node2, p]);
            const m = new THREE.LineBasicMaterial({ color: 0x2fd4b8, transparent: true, opacity: 0.4, depthWrite: false });
            g.add(new THREE.Line(g2, m));
          });
        })
        .catch(() => {})
        .finally(() => {
          hoverBusy.current = false;
          const nxt = hoverJobR.current;
          if (nxt && (nxt.name !== j.name || nxt.prefix !== j.prefix || nxt.rank !== j.rank)) pump();
        });
    };
    pump();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [hoverName, crumbKey, selId, store, rootRank, mode, depth, prefixNow, branches]);
  useEffect(() => {
    const api = apiRef.current;
    if (api.ctx) api.ctx.clear();
    for (const d of api.ctxLabels.values()) d.remove();
    api.ctxLabels = new Map(); api.ctxPos = new Map(); api.ghostDiscs = [];
    if (!selId || !api.scene || !api.ctx) return;
    if (branches.some((b) => b.parent === selId)) return; // déjà déployé (branches auto)
    const node = api.pos.get(selId);
    if (!node) return;
    const ctx = api.ctx;
    const lbl = labelsRef.current!;

    const addGhost = (name: string, color: string, pos: THREE.Vector3, glyph: string, kind: "anc" | "child") => {
      const gt = glowTexture(color);
      const glow = new THREE.Sprite(new THREE.SpriteMaterial({ map: gt, transparent: true, depthWrite: false, opacity: 0.4, blending: THREE.AdditiveBlending }));
      glow.position.copy(pos); glow.scale.set(34, 34, 1); ctx.add(glow);
      const dt = discTexture(color);
      const sp = new THREE.Sprite(new THREE.SpriteMaterial({ map: dt, transparent: true, depthWrite: false, opacity: 0.72 }));
      (sp as any).userData = { name, ghost: true, kind };
      sp.position.copy(pos); sp.scale.set(13, 13, 1); ctx.add(sp);
      api.ghostDiscs.push({ sp, name, kind });
      const div = document.createElement("div");
      div.className = "monde-label ghost";
      div.innerHTML = `<span class="ml-dot" style="background:${color}"></span><span class="ml-name">${glyph}${name.replace(/</g, "&lt;")}</span>`;
      lbl.appendChild(div);
      api.ctxLabels.set(name, div);
      api.ctxPos.set(name, { p: pos.clone(), kind });
    };
    const line = (a: THREE.Vector3, b: THREE.Vector3, color: string) => {
      const g = new THREE.BufferGeometry().setFromPoints([a, b]);
      const m = new THREE.LineBasicMaterial({ color: new THREE.Color(color), transparent: true, opacity: 0.5, depthWrite: false });
      const ln = new THREE.Line(g, m); ln.renderOrder = 3; ctx.add(ln);
    };

    // descendance : enfants (éventail sous le nœud, nœud → enfant)
    // (la lignée ascendante est persistante, rendue dans la scène)
    const ctl = new AbortController();
    const childRank = (rootRank ?? 2) + depth + 1;
    const cprefix = (prefixNow ? prefixNow + "." : "") + selId;
    const token = selId + "|" + cprefix + "|" + childRank;
    let dead = false;
    agg(`${store}|ctx|${token}`, () => fetchSpace(store, cprefix, childRank, 24, ctl.signal, false)).then((sp) => {
      if (dead || api.sel !== selId) return;
      const kids = (sp.ensembles || []).filter((x: any) => ensGood(x.name) && x.count >= 2).slice(0, 8);
      const childPos = (i: number) => {
        const ang = (i / Math.max(1, kids.length)) * Math.PI * 2 - Math.PI / 2;
        return node.clone().add(new THREE.Vector3(Math.cos(ang) * 96, -56, Math.sin(ang) * 96));
      };
      kids.forEach((k: any, i: number) => {
        const p = childPos(i);
        addGhost(k.name, MOD_COL[i % MOD_COL.length], p, "↳ ", "child");
        line(node, p, "#2fd4b8");
      });
      // CASCADE : petits-enfants des 2 dominants (vue 3 sauts connectée)
      const top2 = kids.slice(0, 2);
      Promise.all(top2.map((k: any, gi: number) =>
        agg(`${store}|gc|${cprefix}.${k.name}|${childRank + 1}`,
          () => fetchSpace(store, cprefix + "." + k.name, childRank + 1, 8, ctl.signal, false))
          .then((sp2: any) => ({
            gi,
            list: ((sp2 && sp2.ensembles) || []).filter((x: any) => ensGood(x.name) && x.count >= 2).slice(0, 2),
          }))
          .catch(() => ({ gi, list: [] })),
      )).then((res) => {
        if (dead || api.sel !== selId) return;
        res.forEach(({ gi, list }) => {
          list.forEach((k2: any, j: number) => {
            const base = childPos(gi);
            const p = base.clone().add(new THREE.Vector3((j - (list.length - 1) / 2) * 52, -56, 0));
            addGhost(k2.name, MOD_COL[(gi * 4 + j) % MOD_COL.length], p, "· ", "child");
            line(base, p, "#7fd6b0");
          });
        });
      }).catch(() => {});
    }).catch(() => {});
    return () => { dead = true; ctl.abort(); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selId, crumbKey, depth, prefixNow, rootRank, store, branches]);

  const doZoom = (k: number) => { const c = ctrlRef.current, cam = camRef.current; if (!c || !cam) return;
    const dir = cam.position.clone().sub(c.target);
    dir.setLength(Math.min(c.maxDistance, Math.max(c.minDistance, dir.length() * k)));
    cam.position.copy(c.target).add(dir); c.update(); };
  const toRoot = () => { setCrumb([]); setSelId(null); apiRef.current.sel = null; setNeighbors([]); clearPinned(); };
  const applyTop = (top: boolean) => {
    setTopView(top);
    if (top && depth > 0) {
      // descendre à la racine : le rebuild réinitialise la caméra en plan
      setSelId(null); apiRef.current.sel = null; clearPinned();
      setCrumb([]);
      return;
    }
    const c = ctrlRef.current, cam = camRef.current;
    if (c && cam) {
      if (top) { cam.up.set(0, 0, -1); cam.position.set(0, 1600, 4); c.target.set(0, 0, 0); }
      else { cam.up.set(0, 1, 0); cam.position.set(0, depth > 0 ? 96 : 132, depth > 0 ? 380 : 800); c.target.set(0, -10, 0); }
      c.update();
    }
  };
  const fitView = () => { const c = ctrlRef.current, cam = camRef.current; if (!c || !cam) return;
    setTopView(false);
    cam.up.set(0, 1, 0);
    cam.position.set(0, depth > 0 ? 96 : 132, depth > 0 ? 380 : 800); c.target.set(0, -10, 0); c.update(); };

  // ── carte-mère « big picture » : projection 2D du périmètre + repère de vue
  const paintMap = () => {
    const cv = mapCanvas.current;
    if (!cv) return;
    const ctx = cv.getContext("2d");
    if (!ctx) return;
    const api = apiRef.current;
    const W = cv.width, H = cv.height;
    ctx.clearRect(0, 0, W, H);
    const pts: THREE.Vector3[] = [...api.pos.values()];
    if (!pts.length) return;
    let x0 = Infinity, x1 = -Infinity, z0 = Infinity, z1 = -Infinity;
    for (const p of pts) { if (p.x < x0) x0 = p.x; if (p.x > x1) x1 = p.x; if (p.z < z0) z0 = p.z; if (p.z > z1) z1 = p.z; }
    const pad = 12;
    const ww = Math.max(1, x1 - x0), hh = Math.max(1, z1 - z0);
    const s = Math.min((W - 2 * pad) / ww, (H - 2 * pad) / hh);
    const ox = (W - ww * s) / 2, oy = (H - hh * s) / 2;
    mapXf.current = { x0, z0, s, ox, oy };
    const P = (x: number, z: number) => ({ x: ox + (x - x0) * s, y: oy + (z - z0) * s });
    ctx.fillStyle = "rgba(5,8,13,.88)";
    ctx.fillRect(0, 0, W, H);
    ctx.lineWidth = 1;
    for (const e of api.edges) {
      const a = api.pos.get(e.s), b = api.pos.get(e.d);
      if (!a || !b) continue;
      const pa = P(a.x, a.z), pb = P(b.x, b.z);
      ctx.strokeStyle = "rgba(79,195,255,.22)";
      ctx.beginPath(); ctx.moveTo(pa.x, pa.y); ctx.lineTo(pb.x, pb.y); ctx.stroke();
    }
    const ens2 = ensR.current;
    for (const [name, v] of api.pos) {
      const c = (ens2.find((e) => e.name === name) || {} as Ens).color || "#7fd1ff";
      const q = P(v.x, v.z);
      ctx.beginPath(); ctx.arc(q.x, q.y, 2.6, 0, 6.283);
      ctx.fillStyle = c; ctx.fill();
      if (api.sel === name) { ctx.strokeStyle = "#fff"; ctx.lineWidth = 1.4; ctx.beginPath(); ctx.arc(q.x, q.y, 5, 0, 6.283); ctx.stroke(); }
    }
    const cam = camRef.current, ctrl = ctrlRef.current;
    if (cam && ctrl) {
      ctx.strokeStyle = "rgba(127,209,255,.75)"; ctx.lineWidth = 1.2;
      ctx.beginPath();
      const v = new THREE.Vector3();
      let started = false;
      const corners = [[-1, -1], [1, -1], [1, 1], [-1, 1]] as const;
      for (const [nx, ny] of corners) {
        v.set(nx, ny, 0.5).unproject(cam);
        const dir = v.clone().sub(cam.position);
        const len = dir.length(); if (len < 1e-6) continue;
        dir.divideScalar(len);
        if (dir.y > -1e-4) continue;
        const t = -cam.position.y / dir.y;
        if (t <= 0) continue;
        const wp = cam.position.clone().add(dir.multiplyScalar(t));
        const q = P(wp.x, wp.z);
        if (!started) { ctx.moveTo(q.x, q.y); started = true; } else ctx.lineTo(q.x, q.y);
      }
      if (started) { ctx.closePath(); ctx.stroke(); }
      const tg = ctrl.target;
      const q = P(tg.x, tg.z);
      ctx.fillStyle = "rgba(255,255,255,.9)";
      ctx.beginPath(); ctx.arc(q.x, q.y, 1.9, 0, 6.283); ctx.fill();
    }
  };
  const mapGo = (px: number, py: number) => {
    const xf = mapXf.current; const ctrl = ctrlRef.current; const cam = camRef.current;
    if (!xf || !ctrl || !cam) return;
    const wx = xf.x0 + (px - xf.ox) / xf.s;
    const wz = xf.z0 + (py - xf.oy) / xf.s;
    ctrl.target.set(wx, 0, wz);
    cam.lookAt(ctrl.target); ctrl.update();
  };

  const fluxSum = Object.values(flux).reduce((a, b) => a + b, 0);
  return <div className="overview">
    <div ref={host} style={{ position: "absolute", inset: 0 }} />
    <div ref={labelsRef} className="monde-labels" />
    <div ref={tipRef} className="monde-tip" />
    {helpOn && montre("aide") && (
      <div className="monde-help">
        <b>Clavier &amp; vue</b>
        <span>Échap — effacer sélection/épingles</span>
        <span>← — remonter d'une échelle</span>
        <span>Entrée — plonger (sélection)</span>
        <span>R — racine · T — vue du dessus</span>
        <span>H / ? — cette aide · +/- — zoom</span>
        <span>Clic — sélection · Double-clic — plonger</span>
        <span>Survol — aperçu · Shift clic — épingler</span>
        <span>⊙ vue du dessus · 🗺 carte-mère</span>
      </div>
    )}
    {stage && !err && <div className="monde-loading">… {stage}</div>}
    {mapOn && montre("carte") && <canvas ref={mapCanvas} width={260} height={170} className="monde-map"
      onPointerDown={(e) => { const r = e.currentTarget.getBoundingClientRect();
        mapGo(e.clientX - r.left, e.clientY - r.top); }} />}
    {montre("zoom") && (
    <div className="zoomctl">
      <button title="Zoom avant" onClick={() => doZoom(0.7)}>+</button>
      <button title="Zoom arrière" onClick={() => doZoom(1.4)}>&#8722;</button>
      <button title="Vue d'ensemble" onClick={fitView}>&#8962;</button>
      {/* ── LE ZOOM ESSENTIEL, PUIS LE RESTE SELON L'ÉCRAN ────────────────
          Mesuré : cette colonne faisait 10 boutons × 44 px (cible tactile) plus
          les espacements, soit 44 × 494 px — **59 % de la hauteur d'un écran de
          844 px**, pour des réglages fins qu'on ne touche pas au doigt. Les trois
          gestes qui servent à se déplacer restent toujours là ; les autres
          obéissent à la même règle d'affichage que le reste du chrome. */}
      {montre("zoom_avance") && (
        <>
          <button title="Vue du dessus — tous les projets" className={topView ? "on" : ""}
            onClick={() => applyTop(!topView)}>⊙</button>
          <button title="Critère du seuil : part (nb d'items) ou poids des liens (degré)"
            className="on" onClick={() => setThetaMode((m) => (m === "part" ? "poids" : "part"))}>
            θ:{thetaMode === "part" ? "part" : "poids"}</button>
          {([0, 0.01, 0.05, 0.2] as const).map((v) => (
            <button key={v} title={`Ne déployer que les branches ≥ ${Math.round(v * 100)} % du parent`}
              className={thetaBranch === v ? "on" : ""} onClick={() => setThetaBranch(v)}>
              {v === 0 ? "θ0" : `θ${Math.round(v * 100)}`}</button>))}
          <button title="Replier / déplier les familles de connaissances" className={foldFam ? "on" : ""}
            onClick={() => setFoldFam((v) => !v)}>⤡</button>
        </>
      )}
      {montre("carte") && (
        <button title="Carte-mère (big picture)" className={mapOn ? "on" : ""}
          onClick={() => { const n = !mapOn; setMapOn(n); if (n) requestAnimationFrame(paintMap); }}>🗺</button>
      )}
      {montre("aide") && (
        <button title="Aide / raccourcis (H)" className={helpOn ? "on" : ""} onClick={() => setHelpOn((o) => !o)}>?</button>
      )}
    </div>
    )}

    <div className={"ov-panel" + (panneauOuvert ? "" : " replie")}>
      <div className="ov-panel-tete">
        <h2 style={{ margin: 0, fontSize: 13 }}>Monde — organisation des connaissances</h2>
        <button className="ov-panel-repli" onClick={basculerPanneau}
          aria-expanded={panneauOuvert}
          aria-label={panneauOuvert ? "Replier le panneau" : "Déplier le panneau"}>
          {panneauOuvert ? "▾" : "▸"}
        </button>
        <button className="ov-x" style={{ position: "static" }}
          title="Fermer la vue Monde" onClick={onExit}>✕</button>
      </div>
      {/* Fil d'étage COMMUN (graph/ViewPanel.tsx) : même vocabulaire que le
          Cosmos et l'Atlas — chaque niveau franchi se clique pour revenir. */}
      <LevelCartouche levels={[
        { label: mode === "familles" ? "Store" : "Projets", icon: "🌍",
          current: crumb.length === 0, onClick: toRoot },
        ...crumb.map((c, i) => ({
          label: c,
          current: i === crumb.length - 1,
          onClick: () => {
            setSelId(null); apiRef.current.sel = null; clearPinned();
            setCrumb((cc) => cc.slice(0, i + 1));
          },
        })),
      ]} />
      {!data && !err && <p style={{ opacity: .7 }}>Lecture du store entier…</p>}
      {err && <p style={{ color: "#ff6b9d" }}>{err.slice(0, 200)}</p>}
      {stage && !err && <p className="ov-stage">… {stage}</p>}
      {data && <>
        <h2>{depth === 0 ? "Tous les projets" : `Sous-espace : ${crumb.join(" · ")}`}</h2>
        {ensFull.length > ens.length && (
          <button className="chip" style={{ margin: "2px 0 6px" }}
            onClick={() => setEns((e) => ensFull.slice(0, pageSuivante(e.length, ensFull.length)))}
            title={`Révéler les ensembles suivants (liste complète déjà chargée) — page de ${PAGE.pas}, la même loi que dans l'Atlas et le Cosmos (« d »)`}>
            ⤵ révéler {Math.min(PAGE.pas, ensFull.length - ens.length)} de plus
          </button>)}
        <div className="ov-tot">
          <div><b>{fmt(data.nodes ?? 0)}</b><span>nœuds</span></div>
          <div><b>{fmt(data.edges ?? 0)}</b><span>arêtes</span></div>
          {mode !== "" && <div><b>{ens.length}</b><span>{mode === "familles" ? "familles" : "ensembles"}</span></div>}
          {mode === "modules"
            ? <div><b>{links.length}</b><span>liens tissés</span></div>
            : mode === "projets"
              ? <div><b>{ext.length}</b><span>projets reliés</span></div>
              : mode === "familles"
                ? <div><b>{fmt(fluxSum)}</b><span>flux internes</span></div>
                : null}
        </div>
        {/* Rappel de la grammaire clavier COMMUNE aux vues (graph/grammar.ts) :
            « ↑ remonte » doit vouloir dire la même chose partout. */}
        {/* Rappel de la grammaire clavier COMMUNE aux vues (graph/grammar.ts) :
            « ↑ remonte » doit vouloir dire la même chose dans toutes les vues. */}
        <SubjectLine />
        <EngineAsk />
        <PlanLegend caption={<>Anneaux = familles réelles ; cordes = flux entre familles.</>} />
        <div className="qb-lbl" style={{ display: "block", marginTop: 6, opacity: .75 }}>{VIEW_KEYS_HINT}</div>
        {mode === "projets" && depth === 0 && (
          <div className="ov-mode">Tous les projets : cliquez (ou double-clic) un projet pour plonger dans son sous-espace — ses liens vers les autres projets restent symbolisés en satellites. Remontez via le fil d'échelle ou ⌂.</div>
        )}
        {mode === "projets" && depth > 0 && (
          <div className="ov-mode">Sous-espace de <b>{crumb[0]}</b> — anneau interne = ses constituants et leurs appels réels ; satellites = projets liés. Double-clic pour descendre encore.</div>
        )}
        {mode === "modules" && (
          <div className="ov-mode">Liens = appels réels agrégés. Approchez pour tisser davantage ; double-clic ou « plonger » pour le sous-espace / l'unité.</div>
        )}
        {mode === "familles" && (
          <div className="ov-mode">Familles de nœuds : les anneaux matérialisent les flux internes, les cordes les échanges entre familles.</div>
        )}
        <div className="ov-bars">
          {(data.by_label || []).slice(0, 12).map((x: any) => (
            <div key={x.label} className="ov-row" style={{ color: COL[x.label] || "#cfe3f5" }}>
              <span className="ov-name">{x.label}</span>
              <span className="ov-n">{fmt(x.count)}</span>
            </div>
          ))}
        </div>
      </>}
    </div>

    {selId && (
      <div className="monde-focus">
        <button className="ctl-x" aria-label="Fermer" onClick={() => { apiRef.current.sel = null; setSelId(null); clearPinned(); }}>×</button>
        <b className="mf-name">{selId}</b>
        <span className="mf-legend" title="Contexte hiérarchique">↑ parent · ↓ enfant</span>
        {pinned.size > 0 && (
          <button className="chip on" onClick={clearPinned} title="Tout désépingler">◉ {pinned.size} épinglé{pinned.size > 1 ? "s" : ""} · ✕</button>
        )}
        {mode !== "familles" && (
          <button className="btn btn-primary"
            onClick={() => { const n = selId; apiRef.current.sel = null; setSelId(null); clearPinned(); descendTo(n); }}>
            {depth >= MAX_DEPTH ? "⤓ Unitaire" : "↧ Plonger (sous-espace)"}</button>
        )}
        {mode !== "familles" && (
          <button className="btn" title="Représentation unitaire immédiate"
            onClick={() => { const n = selId; apiRef.current.sel = null; setSelId(null); clearPinned(); drillInto(n); }}>⤓ Unitaire</button>
        )}
        {mode === "familles" && flux[selId] != null && (
          <span className="chip" title="Relations internes à la famille">flux interne <i>{fmt(flux[selId])}</i></span>
        )}
        {/* ── LE MÊME NIVEAU : LES FRÈRES DE CET ENSEMBLE ────────────────────
            Dans cette vue, l'unité n'est pas un nœud mais un ENSEMBLE du niveau
            courant (module en mode « modules », famille de type en mode
            « familles »). Ses frères sont donc les autres ensembles du MÊME
            niveau — une donnée réelle (`ens`, la liste chargée), pas une
            déduction : on annonce l'effectif, et chaque puce sélectionne son
            frère, ce qui permet d'en faire le tour sans repasser par le
            canevas. */}
        {ens.length > 1 && (
          <div className="monde-nb">
            <span className="mf-legend" title={`Les autres ${mode === "familles" ? "familles" : "ensembles"} de ce niveau`}>
              même niveau : <b>{ens.length - 1}</b> {mode === "familles" ? "familles" : "ensembles"}
            </span>
            {ens.filter((e) => e.name !== selId).slice(0, nbTout ? undefined : 14).map((e) => (
              <button key={e.name} className="chip"
                title={`${e.name} — ${fmt(e.count)} nœuds réels · sélectionner ce frère`}
                onClick={() => {
                  apiRef.current.sel = e.name; setSelId(e.name);
                  setNeighbors(voisinsParFlux(e.name)); setNbTout(false);
                }}>
                {e.name} <i>{fmt(e.count)}</i>
              </button>
            ))}
            {!nbTout && ens.length - 1 > 14 && (
              <button className="chip" title={`Afficher les ${ens.length - 1} — aucune troncature silencieuse`}
                onClick={() => setNbTout(true)}>+{ens.length - 1 - 14} autres</button>
            )}
          </div>
        )}
        {neighbors.length > 0 && <div className="monde-nb">
          <span className="mf-legend" title="Voisinage par FLUX réels (et non par parenté de structure)">
            voisins par flux : <b>{neighbors.length}</b>
          </span>
          {neighbors.slice(0, nbTout ? undefined : 14).map((nb) => (
            <button key={nb.name} className="chip" title={`${fmt(nb.w)} appels — sélectionner ce voisin`}
              onClick={() => {
                apiRef.current.sel = nb.name; setSelId(nb.name);
                setNeighbors(voisinsParFlux(nb.name)); setNbTout(false);
              }}>{nb.name} <i>{fmt(nb.w)}</i></button>
          ))}
          {!nbTout && neighbors.length > 14 && (
            <button className="chip" title={`Afficher les ${neighbors.length} voisins`}
              onClick={() => setNbTout(true)}>+{neighbors.length - 14} autres</button>
          )}
        </div>}
      </div>
    )}

    {depth > 0 && (
      <button className="monde-up" title="Remonter d'une échelle"
        onClick={() => { setSelId(null); apiRef.current.sel = null; clearPinned(); setCrumb((c) => c.slice(0, -1)); }}>↥ remonter</button>
    )}
  </div>;
}
