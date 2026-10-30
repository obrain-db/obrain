// Strates simpliciales et contours calculées sur le sous-graphe (client).
import type { GraphEdge, GraphNode } from "./api";

export interface Adj { [k: string]: Set<string> }
export function adjacency(edges: GraphEdge[]): Adj {
  const adj: Adj = {};
  for (const e of edges) {
    (adj[e.src] ??= new Set()).add(e.dst);
    (adj[e.dst] ??= new Set()).add(e.src);
  }
  return adj;
}
export function triangles(adj: Adj, maxT = 400): string[][] {
  const out: string[][] = [];
  for (const u of Object.keys(adj)) {
    const ns = [...adj[u]];
    for (let i = 0; i < ns.length && out.length < maxT; i++)
      for (let j = i + 1; j < ns.length && out.length < maxT; j++)
        if (adj[ns[i]].has(ns[j])) {
          const k = [u, ns[i], ns[j]].sort().join("·");
          if (!out.some(t => t.join("·") === k)) out.push([u, ns[i], ns[j]].sort());
        }
  }
  return out;
}
export function cliquesK4(adj: Adj, tri: string[][], max = 24): string[][] {
  const out: string[][] = [];
  for (const t of tri) {
    const [a, b, c] = t;
    const common = [...adj[a]].filter(x => adj[b].has(x) && adj[c].has(x));
    for (const d of common) {
      const k = [a, b, c, d].sort().join("·");
      if (!out.some(x => x.join("·") === k)) { out.push([a,b,c,d].sort()); if (out.length >= max) break; }
    }
  }
  return out;
}
export function hyperGroups(edges: GraphEdge[]): { src: string; type: string; members: string[] }[] {
  const g = new Map<string, Set<string>>();
  for (const e of edges) {
    const k = `${e.src}${e.type}`;
    if (!g.has(k)) g.set(k, new Set());
    g.get(k)!.add(e.dst);
  }
  return [...g.entries()].filter(([, s]) => s.size >= 3)
    .map(([k, s]) => { const [src, type] = k.split(""); return { src, type, members: [...s] }; });
}
export const contourEdges = (edges: GraphEdge[]) => edges.filter(e => e.ricci != null && e.ricci < 0);
export const degreeOf = (nodes: GraphNode[], edges: GraphEdge[]) => {
  const d: Record<string, number> = {}; nodes.forEach(n => d[n.id] = 0);
  edges.forEach(e => { d[e.src] = (d[e.src] ?? 0) + 1; d[e.dst] = (d[e.dst] ?? 0) + 1; });
  return d;
};
