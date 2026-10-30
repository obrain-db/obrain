// Graph3D — moteur de représentation conceptuelle (Three.js, patterns
// aletheia) : nœuds instanciés, arêtes batchées vertexColors, faces/hyperfaces,
// ∂ contour, communautés, synapses. État piloté par jotai (props dérivées).
import { useEffect, useRef } from "react";
import { useAtomValue, useSetAtom } from "jotai";
import * as THREE from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import { applyRelief, bindViewKeys, LOD, makeCamera, makeRenderer, orbitFor, RELIEF, zoomBy } from "./grammar";
import { PlanLegend, ViewPanel } from "./ViewPanel";
import SubjectLine from "./SubjectLine";
import { subjectAtom } from "../app/store";
import { layersAtom, requestTokenAtom, selectedNodeAtom, statsAtom,
  activeStoreAtom, centersAtom, hopsAtom, limitAtom, overlayAtom, lastGraphAtom,
  suggestionsAtom, organicModeAtom } from "../app/store";
import { fetchSubgraph, fetchInfo, type GraphNode, type JsonGraph } from "../lib/api";
import { adjacency, triangles, cliquesK4, hyperGroups, contourEdges } from "../lib/topology";

const PAL = [0x6fd08c, 0x4aa8ff, 0xe15759, 0xf5d63d, 0xb07aa1, 0x21d4b4, 0xffa03c];
const hexI = (s?: string) => (s ? parseInt(s.slice(1), 16) : 0);
const hashS = (s: string) => { let h = 2166136261; for (const c of s) { h ^= c.charCodeAt(0); h = Math.imul(h, 16777619); } return h >>> 0; };

export default function Graph3D({ onDbl }: { onDbl?: (id: string) => void }) {
  const host = useRef<HTMLDivElement>(null);
  const store = useAtomValue(activeStoreAtom);
  const centers = useAtomValue(centersAtom);
  const hops = useAtomValue(hopsAtom);
  const limit = useAtomValue(limitAtom);
  const overlay = useAtomValue(overlayAtom);
  const layers = useAtomValue(layersAtom);
  const organic = useAtomValue(organicModeAtom);
  const token = useAtomValue(requestTokenAtom);
  const selId = useAtomValue(selectedNodeAtom);
  const setStats = useSetAtom(statsAtom);
  const setSelected = useSetAtom(selectedNodeAtom);
  const setSubject = useSetAtom(subjectAtom);
  const setLastGraph = useSetAtom(lastGraphAtom);
  const setSuggestions = useSetAtom(suggestionsAtom);
  const sceneRef = useRef<{scene: THREE.Scene; camera: THREE.PerspectiveCamera;
    renderer: THREE.WebGLRenderer; controls: OrbitControls; root: THREE.Group;
    inst: THREE.InstancedMesh|null; segs: THREE.LineSegments|null;
    edges: any[]; nodes: GraphNode[]; pos: Map<string, THREE.Vector3>;
    tri: any; k4: any; he: any; ctr: any; byId: string[]} | null>(null);
  const key = `${store}|${centers.join(",")}|${hops}|${limit}|${overlay}|${token}`;
  const layerKey = `${layers.edges}${layers.tri}${layers.k4}${layers.hyper}${layers.contour}${layers.spectral}${layers.synapse}|${organic}`;

  // init moteur une fois
  useEffect(() => {
    const el = host.current!;
    const scene = new THREE.Scene();
    const grid = new THREE.GridHelper(1400, 20, 0x1b2632, 0x141b24);
    grid.position.y = -120; grid.material.transparent = true; (grid.material as any).opacity = 0.5;
    scene.add(grid);
    const camera = makeCamera(el.clientWidth, el.clientHeight);
    camera.position.set(320, 180, 320);
    applyRelief(scene, camera.position.length());
    const renderer = makeRenderer(el.clientWidth, el.clientHeight);
    renderer.domElement.style.touchAction = "none";
    el.appendChild(renderer.domElement);
    // Profil de caméra COMMUN (voir ./grammar.ts) : seul le couple de distances
    // reste propre à l'échelle de la vue (2 → 1800), ajusté ensuite par fitView.
    const controls = orbitFor(camera, renderer.domElement, { minDistance: 2, maxDistance: 1800 });
    controls.enableZoom = true; controls.enableRotate = true; controls.enablePan = true;
    (controls as any).touches = { ONE: (THREE as any).TOUCH.ROTATE, TWO: (THREE as any).TOUCH.DOLLY_PAN };
    const root = new THREE.Group(); scene.add(root);
    scene.add(new THREE.AmbientLight(0xffffff, .6));
    const dl = new THREE.DirectionalLight(0xffffff, .75); dl.position.set(300,420,220); scene.add(dl);
    sceneRef.current = { scene, camera, renderer, controls, root, inst: null, segs: null,
      edges: [], nodes: [], pos: new Map(), tri: null, k4: null, he: null, ctr: null, byId: [] };
    const onResize = () => { const w = el.clientWidth, h = el.clientHeight; if (!w || !h) return;
      camera.aspect = w / h; camera.updateProjectionMatrix();
      renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, RELIEF.pixelRatioCap));
      renderer.setSize(w, h); };
    window.addEventListener("resize", onResize);
    let rafId = 0;
    const raf = () => { rafId = requestAnimationFrame(raf);
      if (document.hidden) return;
      controls.update();
      const c = sceneRef.current!;
      const nnow = performance.now();
      if (nnow - lastDen > 150) {
        lastDen = nnow;
        const Rc = (c as any).rad || 120;
        const dc = c.camera.position.distanceTo(c.controls.target);
        const zc = Rc / Math.max(1, dc);
        const lv = zc < 0.07 ? 0 : zc < 0.16 ? 1 : zc < 0.38 ? 2 : 3;
        if (lv !== (c as any).den) { (c as any).den = lv; densityApply(c, lv); }
      }
      if (layersRotateRef.current) c.root.rotation.y += 0.0016;
      if (organicRef.current && c.segs) {
        const m = c.segs.material as THREE.LineBasicMaterial;
        m.opacity = 0.45 + 0.35 * Math.abs(Math.sin(performance.now() * 0.004));
      }
      renderer.render(scene, camera); };
    raf();
    return () => { cancelAnimationFrame(rafId); window.removeEventListener("resize", onResize);
      controls.dispose();
      el.removeChild(renderer.domElement); renderer.dispose(); sceneRef.current = null; };
  }, []);
  const layersRotateRef = useRef(false); layersRotateRef.current = !!layers.rotate;
  const organicRef = useRef(false); organicRef.current = organic;
  const selPosRef = useRef<THREE.Vector3 | null>(null);
  let lastDen = 0;
  const lastDataRef = useRef<JsonGraph | null>(null);
  const hoverRef = useRef<{ id: string | null; base: Float32Array | null;
    map: Map<string, Set<string>> | null; adj: Map<string, Set<string>> | null }>({
      id: null, base: null, map: null, adj: null });
  const tipRef = useRef<HTMLDivElement>(null);

  // layout BFS par couches
  function layout(nodes: GraphNode[], es: Set<string>) {
    const first = nodes[0]?.id ?? ""; const layer = new Map([[first, 0]]); const q = [first];
    while (q.length) { const u = q.shift()!; for (const n of nodes)
      if (!layer.has(n.id) && (es.has(u+''+n.id)||es.has(n.id+''+u)))
        { layer.set(n.id, (layer.get(u) ?? 0)+1); q.push(n.id); } }
    const maxL = Math.max(1, ...layer.values());
    const byL = new Map<number, string[]>();
    nodes.forEach((n,i) => { const l = layer.has(n.id) ? layer.get(n.id)! : maxL + 0.6 + (i%3)*0.3;
      (byL.get(l) ?? byL.set(l, []).get(l)!).push(n.id); });
    const R0 = 60, pos = new Map<string, THREE.Vector3>(); const golden = Math.PI*(3-Math.sqrt(5));
    for (const [l, ids] of byL) { const R = R0*(1+l);
      ids.forEach((id,i) => { const y = 1-(i/Math.max(ids.length-1,1))*2;
        const rad = Math.sqrt(Math.max(0,1-y*y)); const a = golden*i + hashS(id)%360;
        pos.set(id, new THREE.Vector3(R*rad*Math.cos(a), R*y, R*rad*Math.sin(a))); }); }
    return pos;
  }
  function clearWorld(){ const c = sceneRef.current!; while (c.root.children.length)
    c.root.remove(c.root.children[0]); c.inst = null; c.segs = null; c.pos = new Map();
    c.nodes = []; c.edges = []; c.byId = [];
    if (c.scene) c.scene.children.forEach((o: any) => { if (o.isGridHelper) o.visible = !organicRef.current; }); }
  function nodeColor(n: GraphNode, i: number){
    if (layers.spectral) { const cid = (n.community_id ?? n.scc_id ?? null);
      if (cid != null) return PAL[cid % PAL.length]; }
    return hexI(n.color_hint) || PAL[i % PAL.length];
  }

  useEffect(() => {
    if (!store || centers.length === 0 || !sceneRef.current) return;
    const ctl = new AbortController();
    const sig = ctl.signal;
    setStats("… chargement");
    let dead = false;
    Promise.all([fetchSubgraph(store, centers, hops, limit, overlay, sig), fetchInfo(store, sig)])
      .then(([data, info]) => { if (dead) return; setSuggestions([]);
        const ctx = info && info.nodes_estimate > 0
          ? " · store ~" + Math.round(info.nodes_estimate / 1000) + "k nœuds" : "";
        requestAnimationFrame(() => { buildWorld(data); setStats(
        `${data.stats.node_count} nœuds · ${data.stats.edge_count} arêtes · K3 ${(sceneRef.current as any).triCount ?? 0} · ∂ ${(sceneRef.current as any).ctrCount ?? 0}` +
        (data.stats.truncated ? " · TRONQUÉ" : "")); }); })
      .catch((e: any) => {
        if (dead || e?.name === "AbortError") return; // changement de config : annulé
        const raw = String(e?.message || e);
        let msg = raw; try { const j = JSON.parse(raw.slice(raw.indexOf("{"))); msg = j.error || raw; } catch { /* brut */ }
        const m = msg.match(/candidats : (.+?) \(donner/);
        setSuggestions(m ? m[1].split(", ").map(x => x.trim()).filter(Boolean) : []);
        setStats("⚠ " + msg.slice(0, 140));
      });
    return () => { dead = true; ctl.abort(); };
  }, [key]);

  function buildWorld(data: JsonGraph) {
    lastDataRef.current = data;
    const c = sceneRef.current!; clearWorld();
    const { nodes, edges } = data; c.nodes = nodes; c.edges = edges;
    const adj = adjacency(edges as any);
    const es = new Set(edges.map(e => `${e.src}${e.dst}`));
    const pos = layout(nodes, es); c.pos = pos; c.byId = nodes.map(n => n.id);
    // nœuds instanciés
    const heavy = nodes.length > 600 || edges.length > 6000;
    const geo = new THREE.SphereGeometry(1, heavy ? 8 : 14, heavy ? 6 : 12);
    const mat = new THREE.MeshPhongMaterial({ color: 0xffffff });
    const inst = new THREE.InstancedMesh(geo, mat, nodes.length);
    const m4 = new THREE.Matrix4(); const q4 = new THREE.Quaternion();
    nodes.forEach((n,i) => { const p = pos.get(n.id); if (!p) return;
      const s = (n.scc_id != null || n.engram_id) ? 1.25 : 1;
      m4.compose(p, q4, new THREE.Vector3(s,s,s)); inst.setMatrixAt(i, m4);
      inst.setColorAt(i, new THREE.Color(nodeColor(n,i))); });
    inst.instanceMatrix.needsUpdate = true;
    if (inst.instanceColor) inst.instanceColor.needsUpdate = true;
    c.inst = inst; c.root.add(inst);
    const base = new Float32Array(nodes.length * 3);
    for (let i = 0; i < nodes.length; i++) {
      const col = new THREE.Color(nodeColor(nodes[i], i));
      base[i*3] = col.r; base[i*3+1] = col.g; base[i*3+2] = col.b;
    }
    const hoverAdj = new Map<string, Set<string>>();
    edges.forEach(e => {
      if (!hoverAdj.has(e.src)) hoverAdj.set(e.src, new Set());
      hoverAdj.get(e.src)!.add(e.dst);
      if (!hoverAdj.has(e.dst)) hoverAdj.set(e.dst, new Set());
      hoverAdj.get(e.dst)!.add(e.src);
    });
    hoverRef.current.base = base; hoverRef.current.adj = hoverAdj; hoverRef.current.id = null;
    // labels (limités)
    const labelled = nodes.filter(n => n.label || n.qn)
      .sort((a, b) => (b.energy ?? 0) - (a.energy ?? 0));
    const labelSprites: THREE.Sprite[] = [];
    if (labelled.length <= 260 && layers.labels) labelled.forEach(n => {
      const cv = document.createElement("canvas"); cv.width = 512; cv.height = 64;
      const ctx = cv.getContext("2d")!; const t = (n.label || n.qn || "").slice(0, 26);
      ctx.font = "30px ui-monospace, monospace"; ctx.fillStyle = "#eaf2fa";
      ctx.fillText(t, 8, 44);
      const sp = new THREE.Sprite(new THREE.SpriteMaterial({ map: new THREE.CanvasTexture(cv),
        transparent: true, depthWrite: false }));
      sp.position.copy(pos.get(n.id)!); sp.position.y += 12; sp.scale.set(70,9,1);
      labelSprites.push(sp); c.root.add(sp); });
    if ((c as any).ly) (c as any).ly.labels = labelSprites;
    // arêtes batchées vertexColors
    const pts: number[] = []; const cols: number[] = [];
    edges.forEach(e => { const a = pos.get(e.src), b = pos.get(e.dst); if (!a || !b) return;
      let col: number;
      if (layers.synapse && ((e.weight ?? 0) > 0 || (e.pheromone ?? 0) > 0)) {
        const st = Math.min(1, Math.max(0, e.weight || e.pheromone || 0));
        col = new THREE.Color(0x0a0f14).lerp(new THREE.Color(0xffe066), st).getHex();
      } else col = hexI(e.color_hint) || (e.type === "CALLS" ? 0x4aa8ff : 0xb07aa1);
      const C = new THREE.Color(col);
      pts.push(a.x,a.y,a.z,b.x,b.y,b.z); cols.push(C.r,C.g,C.b,C.r,C.g,C.b); });
    const gE = new THREE.BufferGeometry();
    gE.setAttribute("position", new THREE.Float32BufferAttribute(pts,3));
    gE.setAttribute("color", new THREE.Float32BufferAttribute(cols,3));
    const segs = new THREE.LineSegments(gE, new THREE.LineBasicMaterial({ vertexColors: true,
      transparent: true, opacity: .85 }));
    segs.visible = !!layers.edges; c.segs = segs; c.root.add(segs);
    // triangles — calculés seulement si la strate (ou K4 qui en dépend) est active
    const wantTri = !!layers.tri || !!layers.k4;
    const tri = wantTri ? triangles(adj) : [];
    const tp: number[] = [];
    tri.forEach(t => { const p = t.map(id => pos.get(id)); if (p.some(x => !x)) return;
      tp.push(p[0]!.x,p[0]!.y,p[0]!.z, p[1]!.x,p[1]!.y,p[1]!.z, p[2]!.x,p[2]!.y,p[2]!.z); });
    const gT = new THREE.BufferGeometry();
    gT.setAttribute("position", new THREE.Float32BufferAttribute(tp,3));
    const triM = new THREE.Mesh(gT, new THREE.MeshBasicMaterial({ color: 0x21d4b4,
      transparent: true, opacity: .14, side: THREE.DoubleSide, depthWrite: false }));
    triM.visible = !!layers.tri; c.root.add(triM); (c as any).triCount = tri.length;
    // hyperfaces K4 — LOD : pas de cliques à très grand volume, ni si strate inactive
    const k4 = (!heavy && layers.k4) ? cliquesK4(adj, tri) : [];
    const kp: number[] = [], kc: number[] = [];
    const KC = new THREE.Color(0xffd166);
    k4.forEach(f => { const p = f.map(id => pos.get(id)); if (p.some(x => !x)) return;
      [[0,1],[0,2],[0,3],[1,2],[1,3],[2,3]].forEach(([i,j]) => {
        kp.push(p[i]!.x,p[i]!.y,p[i]!.z,p[j]!.x,p[j]!.y,p[j]!.z);
        kc.push(KC.r,KC.g,KC.b,KC.r,KC.g,KC.b); }); });
    const gK = new THREE.BufferGeometry();
    gK.setAttribute("position", new THREE.Float32BufferAttribute(kp,3));
    gK.setAttribute("color", new THREE.Float32BufferAttribute(kc,3));
    const k4s = new THREE.LineSegments(gK, new THREE.LineBasicMaterial({ vertexColors: true }));
    k4s.visible = !!layers.k4; c.root.add(k4s);
    // hyperarêtes
    const he = layers.hyper ? hyperGroups(edges as any) : [];
    const hp: number[] = [], hc: number[] = [];
    const HC = new THREE.Color(0xff7a5c);
    he.forEach(h => { const s = pos.get(h.src); if (!s) return;
      h.members.forEach(m => { const m2 = pos.get(m); if (!m2) return;
        hp.push(s.x,s.y,s.z,m2.x,m2.y,m2.z); hc.push(HC.r,HC.g,HC.b,HC.r,HC.g,HC.b); }); });
    const gH = new THREE.BufferGeometry();
    gH.setAttribute("position", new THREE.Float32BufferAttribute(hp,3));
    gH.setAttribute("color", new THREE.Float32BufferAttribute(hc,3));
    const hes = new THREE.LineSegments(gH, new THREE.LineBasicMaterial({ vertexColors: true }));
    hes.visible = !!layers.hyper; c.root.add(hes);
    // contour
    const ctr = layers.contour ? contourEdges(edges as any) : [];
    const cp: number[] = [], cc: number[] = [];
    const CT = new THREE.Color(0xff6b9d);
    ctr.forEach(e => { const a = pos.get(e.src), b = pos.get(e.dst); if (!a || !b) return;
      cp.push(a.x,a.y,a.z,b.x,b.y,b.z); cc.push(CT.r,CT.g,CT.b,CT.r,CT.g,CT.b); });
    const gC = new THREE.BufferGeometry();
    gC.setAttribute("position", new THREE.Float32BufferAttribute(cp,3));
    gC.setAttribute("color", new THREE.Float32BufferAttribute(cc,3));
    const cts = new THREE.LineSegments(gC, new THREE.LineBasicMaterial({ vertexColors: true,
      transparent: true, opacity: .95 }));
    cts.visible = !!layers.contour; c.root.add(cts); (c as any).ctrCount = ctr.length;
    (c as any).ly = { triM, k4s, hes, cts, labels: [], heavy };
    // ── Vue ORGANIQUE : halos + anneaux sur les hubs d'énergie/communauté ──
    if (organic) {
      const orga = new THREE.Group(); orga.name = "organique";
      const hubs = [...nodes].sort((a, b) => (b.energy ?? 0) - (a.energy ?? 0)).slice(0, 46);
      hubs.forEach((n) => {
        const pv = pos.get(n.id); if (!pv) return;
        const col = new THREE.Color(nodeColor(n, 0)).lerp(new THREE.Color(0xffffff), 0.25);
        const cv = document.createElement("canvas"); cv.width = 64; cv.height = 64;
        const g2 = cv.getContext("2d")!; const grad = g2.createRadialGradient(32,32,4,32,32,30);
        grad.addColorStop(0, "#ffffff"); grad.addColorStop(0.35, "#"+col.getHexString());
        grad.addColorStop(1, "rgba(0,0,0,0)");
        g2.fillStyle = grad; g2.fillRect(0, 0, 64, 64);
        const sp = new THREE.Sprite(new THREE.SpriteMaterial({ map: new THREE.CanvasTexture(cv),
          transparent: true, opacity: 0.55, depthWrite: false, blending: THREE.AdditiveBlending }));
        sp.position.copy(pv); sp.scale.set(34, 34, 1); orga.add(sp);
        const ring = new THREE.BufferGeometry().setFromPoints(
          Array.from({ length: 28 }, (_, k) => {
            const a = (k / 28) * Math.PI * 2;
            return new THREE.Vector3(pv.x + Math.cos(a) * 7.5, pv.y + Math.sin(a) * 7.5 * 0.5, pv.z);
          }));
        const ln = new THREE.Line(ring, new THREE.LineBasicMaterial({ color: col, transparent: true, opacity: 0.5 }));
        orga.add(ln);
      });
      c.root.add(orga);
    }

    // multi-échelle : recadrage caméra sur l'ensemble + mémorisation du graphe
    const pts3: THREE.Vector3[] = [...pos.values()];
    if (pts3.length) {
      const box = new THREE.Box3().setFromPoints(pts3);
      const sph = box.getBoundingSphere(new THREE.Sphere());
      (c as any).rad = sph.radius;
      c.controls.minDistance = Math.max(2, sph.radius * 0.04);
      c.controls.maxDistance = Math.min(1800, Math.max(600, sph.radius * 7));
      c.camera.position.copy(sph.center).add(new THREE.Vector3(
        sph.radius * 1.9, sph.radius * 1.25, sph.radius * 1.9));
      c.controls.target.copy(sph.center); c.controls.update();
    }
    setLastGraph(data);
    // picking + clic
    const ray = new THREE.Raycaster(); const mouse = new THREE.Vector2();
    const dom = c.renderer.domElement;
    const down = { x: 0, y: 0 }; let drag = false;
    dom.onpointerdown = (ev: PointerEvent) => { down.x = ev.clientX; down.y = ev.clientY; drag = false; };
    dom.onpointerup = (ev: PointerEvent) => { if (Math.hypot(ev.clientX - down.x, ev.clientY - down.y) > 6) drag = true; };
    const pick = (ev: PointerEvent) => { const r = dom.getBoundingClientRect();
      mouse.set(((ev.clientX-r.left)/r.width)*2-1, -((ev.clientY-r.top)/r.height)*2+1);
      ray.setFromCamera(mouse, c.camera);
      const hit = ray.intersectObject(c.inst!, false)[0];
      return hit && hit.instanceId != null ? c.byId[hit.instanceId] : null; };
    dom.onclick = ev => { if (drag) return; const id = pick(ev); if (id) setSelected(id); else setSelected(null); };
    dom.ondblclick = ev => { if (drag) return; const r = dom.getBoundingClientRect();
      mouse.set(((ev.clientX-r.left)/r.width)*2-1, -((ev.clientY-r.top)/r.height)*2+1);
      ray.setFromCamera(mouse, c.camera);
      const hit = ray.intersectObject(c.inst!, false)[0];
      if (hit && hit.instanceId != null) {
        const id = c.byId[hit.instanceId];
        if (onDbl) { onDbl(id); return; }
        c.controls.target.copy(c.pos.get(id)!); c.controls.update(); } };
    const small = nodes.length <= 2200; // survol : info-bulle + voisinage (perf)
    const highlight = (id: string | null) => {
      const inst = c.inst; const hr = hoverRef.current;
      if (!inst || !hr.base || !inst.instanceColor) return;
      if (id === hr.id) return;
      hr.id = id;
      const arr = inst.instanceColor.array as Float32Array;
      for (let i = 0; i < c.byId.length; i++) {
        const inSet = id != null && (c.byId[i] === id ||
          (hr.adj?.get(id)?.has(c.byId[i]) ?? false));
        if (inSet) { arr[i*3] = hr.base[i*3]; arr[i*3+1] = hr.base[i*3+1]; arr[i*3+2] = hr.base[i*3+2]; }
        else { arr[i*3] = hr.base[i*3] * 0.16; arr[i*3+1] = hr.base[i*3+1] * 0.16; arr[i*3+2] = hr.base[i*3+2] * 0.16; }
      }
      inst.instanceColor!.needsUpdate = true;
      const tip = tipRef.current; if (!tip) return;
      if (id == null) { tip.style.display = "none"; return; }
      const node = c.nodes.find(n => n.id === id);
      tip.textContent = (node?.label || id.split(".").pop() || id) + (node?.file_path ? "  · " + node.file_path : "");
      tip.style.display = "block";
    };
    if (!small) { highlight(null); }
    let lastT = 0;
    if (small) {
      dom.onpointermove = (ev: PointerEvent) => {
        if (ev.pointerType !== "mouse" || ev.buttons !== 0) return;
        const now = performance.now(); if (now - lastT < 50) return; lastT = now;
        const r = dom.getBoundingClientRect();
        mouse.set(((ev.clientX - r.left) / r.width) * 2 - 1, -((ev.clientY - r.top) / r.height) * 2 + 1);
        ray.setFromCamera(mouse, c.camera);
        const hit = ray.intersectObject(c.inst!, false)[0];
        highlight(hit && hit.instanceId != null ? c.byId[hit.instanceId] : null);
        const tip = tipRef.current; if (tip && hit) {
          tip.style.left = (ev.clientX - r.left + 14) + "px"; tip.style.top = (ev.clientY - r.top + 14) + "px"; }
      };
      dom.onpointerleave = () => highlight(null);
    } else { dom.onpointermove = null; dom.onpointerleave = null; }
  }
  useEffect(() => { const c = sceneRef.current; if (!c) return;
    selPosRef.current = selId && c.pos.get(selId) ? c.pos.get(selId)! : null;
  }, [selId]);
  // toggles de visibilité / couleur : reconstruction LOCALE depuis le dernier
  // graphe (aucun refetch réseau) — spectral/synapse/appliquent immédiatement.
  useEffect(() => {
    const c = sceneRef.current; if (!c || !lastDataRef.current) return;
    requestAnimationFrame(() => buildWorld(lastDataRef.current!));
  }, [layerKey]);
  const tipStyle: React.CSSProperties = { position: "absolute", zIndex: 6, display: "none",
    pointerEvents: "none", background: "rgba(10,14,19,.92)", border: "1px solid #2a3440",
    borderRadius: 8, padding: "4px 9px", fontSize: 11, color: "#eaf2fa", maxWidth: 340,
    overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" };
  const densityApply = (c: any, level: number) => {
    const ly = c.ly; if (!ly) return;
    const setV = (o: any, v: boolean) => { if (o) o.visible = v; };
    setV(ly.triM, !!layers.tri && level >= 1);
    setV(ly.k4s, !!layers.k4 && level >= 2);
    setV(ly.hes, !!layers.hyper && level >= 2);
    setV(ly.cts, !!layers.contour && level >= 2);
    // Budget d'étiquettes issu du LOD COMMUN (graph/grammar.ts) : le même
    // éloignement doit alléger de la même façon dans toutes les vues.
    const budget = LOD.budgets[Math.max(0, Math.min(LOD.budgets.length - 1, level))];
    const sp = ly.labels || []; const sel = selPosRef.current;
    for (let i = 0; i < sp.length; i++) {
      const keep = i < budget || (sel != null && sp[i].position.distanceTo(sel) < 55);
      sp[i].visible = keep && !!layers.labels;
    }
  };
  const doZoom = (k: number) => { const c = sceneRef.current; if (!c) return;
    const dir = c.camera.position.clone().sub(c.controls.target);
    dir.setLength(Math.min(c.controls.maxDistance, Math.max(c.controls.minDistance, dir.length() * k)));
    c.camera.position.copy(c.controls.target).add(dir);
    c.camera.lookAt(c.controls.target); c.controls.update(); };
  const fitView = () => { const c = sceneRef.current; if (!c || !c.pos.size) return;
    const pts: THREE.Vector3[] = [...c.pos.values()];
    const box = new THREE.Box3().setFromPoints(pts);
    const sph = box.getBoundingSphere(new THREE.Sphere());
      c.controls.minDistance = Math.max(2, sph.radius * 0.04);
      c.controls.maxDistance = Math.min(1800, Math.max(600, sph.radius * 7));
    c.camera.position.copy(sph.center).add(new THREE.Vector3(
      sph.radius * 2.0, sph.radius * 1.3, sph.radius * 2.0));
    c.controls.target.copy(sph.center); c.controls.update();
    // La mise au point a changé : le relief la suit (brouillard au-delà du cadre).
    applyRelief(c.scene, c.camera.position.distanceTo(c.controls.target)); };
  // ── GRAMMAIRE COMMUNE (voir ./grammar.ts) ──────────────────────────────
  // Cette vue n'avait que +/− (par boutons) : elle s'aligne sur les autres.
  useEffect(() => {
    const detach = bindViewKeys({
      down: () => { if (selId) onDbl?.(selId); },
      home: () => fitView(),
      up: () => fitView(),
      zoomIn: () => doZoom(0.72),
      zoomOut: () => doZoom(1.4),
      escape: () => setSelected(null),
      next: () => cycle(1),
      prev: () => cycle(-1),
    });
    return detach;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selId, onDbl]);

  // La sélection devient le SUJET de la pièce : les autres faces le liront.
  useEffect(() => {
    if (!selId) return;
    setSubject({ store, qn: selId, label: String(selId).slice(0, 42), via: "graphe" });
  }, [selId, store, setSubject]);

  /** Parcourir les nœuds affichés : le même geste que dans les autres vues. */
  const cycle = (dir: number) => {
    const c = sceneRef.current;
    if (!c || !c.pos.size) return;
    const ids = [...c.pos.keys()];
    const i = selId ? ids.indexOf(selId) : -1;
    const nxt = ids[((i + dir) % ids.length + ids.length) % ids.length];
    if (nxt) setSelected(nxt);
  };

  return <div ref={host} className="canvas-host" style={{ position: "absolute", inset: 0 }}>
    <div ref={tipRef} style={tipStyle} />
    <ViewPanel title="Graphe — formes du substrat" width={300}
      subtitle="Un seul substrat : $\partial$, cliques K3/K4, hyperfaces, synapses.">
      <SubjectLine />
      <PlanLegend caption={<>Couleur = type ; taille et halo suivent l'énergie.</>} />
    </ViewPanel>
    <div className="zoomctl">
      <button aria-label="Zoom avant" title="Zoom avant" onClick={() => doZoom(0.72)}>+</button>
      <button aria-label="Zoom arrière" title="Zoom arrière" onClick={() => doZoom(1.4)}>−</button>
      <button aria-label="Vue d'ensemble" title="Recentrer / vue d'ensemble" onClick={fitView}>⌂</button>
    </div>
  </div>;
}
