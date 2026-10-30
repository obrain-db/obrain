// MonDE — primitives pures (couleurs, format, cache de session, textures,
// disposition organique). Découpage (Étape F) : aucun état/hook ici.
import * as THREE from "three";

export const COL: Record<string, string> = {
  Function: "#4aa8ff", Field: "#6fd08c", Struct: "#ffd166", File: "#21d4b4",
  Enum: "#b07aa1", Variable: "#f5d63d", Type: "#7fd1ff", Method: "#ff7a5c",
  Class: "#e15759", Module: "#ffa03c", Macro: "#ffe066", Interface: "#ff6b9d",
  Session: "#9b8cff", Message: "#4aa8ff", ToolCall: "#ffa03c", ToolResult: "#ff6b9d",
  Step: "#21d4b4", SelfState: "#b07aa1", Task: "#6fd08c",
};
export const MOD_COL = ["#5aa2ff", "#ff8a5c", "#5fd68a", "#ffd166", "#b07aa1", "#2fd4b8", "#ff6b9d", "#9b8cff", "#f5d63d", "#7fd1ff", "#ffa03c", "#e15759"];
export const fmt = (n: number) => Math.round(n).toLocaleString("fr-FR").replace(/,/g, " ");
export const hashC = (s: string) => { let h = 2166136261; for (const c of s) { h ^= c.charCodeAt(0); h = Math.imul(h, 16777619); } return h >>> 0; };
export const MAX_DEPTH = 12;
// Nom d'ensemble = segment de chemin sûr (même jeu que la validation serveur) :
// les noms avec espaces/ponctuation (titres de documents…) ne sont pas des
// sous-dossiers explorables → exclus des ensembles ET de la descendance.
export const ensOk = (n: string) => !!n && !/^[0-9]+$/.test(n) && !/[^A-Za-z0-9._@+%-]/.test(n);
// BRUIT STRUCTUREL : paquets npm versionnés (vitest@4, @scope…), fichiers
// racine récurrents (README, LICENSE, tsconfig, configs…) — encombrent les
// anneaux de niveaux profonds sans être des sous-ensembles du code utile.
export const NOISE_RE = /^@|@\d|(^|[._-])(readme|license|licence|contributing|contribute|contributors?|changelog|copying|notice|authors|security|support|governance|package|tsconfig|vitest|jest|eslint|prettier|knip|lefthook|tsdown|typedoc|gitignore|editorconfig|agents|brand_guidelines)([._@-]|$)/i;
export const ensGood = (n: string) => ensOk(n) && !NOISE_RE.test(n);

export interface Ens { name: string; count: number; color: string }
export interface Link { src: string; dst: string; weight: number }

// Cache de session des agrégats lourds (stores immuables : 15 min).
export const AGG_TTL = 15 * 60 * 1000;
export const aggCache = new Map<string, { t: number; v: unknown }>();
export async function agg<T>(key: string, fn: () => Promise<T>): Promise<T> {
  const hit = aggCache.get(key);
  if (hit && Date.now() - hit.t < AGG_TTL) return hit.v as T;
  const v = await fn();
  aggCache.set(key, { t: Date.now(), v });
  return v;
}

// Texture disque lumineux (nœud) : cœur clair, anneau, bord net.
export function discTexture(color: string): THREE.CanvasTexture {
  const cv = document.createElement("canvas"); cv.width = 64; cv.height = 64;
  const g = cv.getContext("2d")!;
  const c = new THREE.Color(color);
  const grad = g.createRadialGradient(32, 32, 2, 32, 32, 30);
  grad.addColorStop(0, "#ffffff");
  grad.addColorStop(0.25, c.clone().lerp(new THREE.Color(0xffffff), 0.4).getStyle());
  grad.addColorStop(0.7, c.getStyle());
  grad.addColorStop(1, c.clone().multiplyScalar(0.55).getStyle());
  g.fillStyle = grad;
  g.beginPath(); g.arc(32, 32, 30, 0, Math.PI * 2); g.fill();
  g.strokeStyle = c.clone().lerp(new THREE.Color(0xffffff), 0.5).getStyle();
  g.lineWidth = 2.5;
  g.beginPath(); g.arc(32, 32, 27, 0, Math.PI * 2); g.stroke();
  return new THREE.CanvasTexture(cv);
}
export function glowTexture(color: string): THREE.CanvasTexture {
  const cv = document.createElement("canvas"); cv.width = 128; cv.height = 128;
  const g = cv.getContext("2d")!;
  const c = new THREE.Color(color);
  const grad = g.createRadialGradient(64, 64, 4, 64, 64, 64);
  grad.addColorStop(0, c.clone().lerp(new THREE.Color(0xffffff), 0.35).getStyle());
  grad.addColorStop(0.4, c.getStyle());
  grad.addColorStop(1, "rgba(0,0,0,0)");
  g.fillStyle = grad; g.fillRect(0, 0, 128, 128);
  return new THREE.CanvasTexture(cv);
}

// DISPOSITION ORGANIQUE 3D : les ensembles sont placés par simulation de
// forces pilotées par les LIENS RÉELS (poids) — les couples fortement liés se
// rapprochent en branches, le tout se déploie en profondeur (pas « plat »).
// Déterministe : seed = anneau, itérations fixes → stable entre rebuilds.
export function forceLayout(ens: Ens[], links: Link[], R: number, sub: boolean): Map<string, THREE.Vector3> {
  const pos = new Map<string, THREE.Vector3>();
  const sorted = [...ens].sort((a, b) => b.count - a.count);
  const names = sorted.map((e) => e.name);
  const N = names.length;
  if (!N) return pos;
  const maxC = Math.max(1, ...sorted.map((e) => e.count));
  const ringF = sub ? 0.62 : 1;
  sorted.forEach((e, i) => {
    const size = Math.sqrt(e.count / maxC);
    const ang = (i / N) * Math.PI * 2 - Math.PI / 2 + 0.6;
    const rad = R * ringF * (0.7 + 0.4 * size) + (hashC(e.name) % 24);
    pos.set(e.name, new THREE.Vector3(Math.cos(ang) * rad, (hashC(e.name + "y") % 20) - 10, Math.sin(ang) * rad));
  });
  if (N < 2 || links.length === 0) return pos;
  const maxW = Math.max(1, ...links.map((l) => l.weight));
  const k = R * 1.15;
  const iters = Math.min(300, 150 + N * 6);
  for (let it = 0; it < iters; it++) {
    const fx = new Float64Array(N), fy = new Float64Array(N), fz = new Float64Array(N);
    // répulsion (toutes paires, en 3D)
    for (let i = 0; i < N; i++) for (let j = i + 1; j < N; j++) {
      const a = pos.get(names[i])!, b = pos.get(names[j])!;
      const dx = a.x - b.x, dy = a.y - b.y, dz = a.z - b.z;
      const d2 = Math.max(400, dx * dx + dy * dy + dz * dz);
      const d = Math.sqrt(d2);
      const f = (k * k) / d2 * 1.2;
      const ex = dx / d * f, ey = dy / d * f, ez = dz / d * f;
      fx[i] += ex; fy[i] += ey; fz[i] += ez;
      fx[j] -= ex; fy[j] -= ey; fz[j] -= ez;
    }
    // attraction le long des liens, pondérée par le poids (spring)
    for (const l of links) {
      const i = names.indexOf(l.src), j = names.indexOf(l.dst);
      if (i < 0 || j < 0) continue;
      const a = pos.get(l.src)!, b = pos.get(l.dst)!;
      const w = l.weight / maxW;
      const s = (0.1 + w * 0.65) * 0.55;
      const dx = b.x - a.x, dy = b.y - a.y, dz = b.z - a.z;
      const d = Math.max(20, Math.sqrt(dx * dx + dy * dy + dz * dz));
      const ex = dx / d * s, ey = dy / d * s, ez = dz / d * s;
      fx[i] += ex; fy[i] += ey; fz[i] += ez;
      fx[j] -= ex; fy[j] -= ey; fz[j] -= ez;
    }
    const damp = 0.14;
    const maxR = R * (sub ? 0.98 : 1.6);
    for (let i = 0; i < N; i++) {
      const p = pos.get(names[i])!;
      p.x += fx[i] * damp; p.y += fy[i] * damp; p.z += fz[i] * damp;
      p.multiplyScalar(0.99); // gravité douce
      const len = p.length();
      if (len > maxR) p.multiplyScalar(maxR / len);
    }
  }
  // profondeur maîtrisée : on garde du relief sans écraser la lecture
  for (const n of names) { const p = pos.get(n)!; p.y *= 0.55; }
  return pos;
}


