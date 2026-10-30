#!/usr/bin/env python3
"""Amplificateur de corpus obrain : réplique un dump JSONL (nodes+edges) N fois.

- Renumérote les ids de nœuds en rangs CONTIGUS 1..N (le dump source peut avoir
  des trous : carto-dump a max_id=44813 pour 37678 nœuds).
- Réplique r (0..R-1) : node_id = rang + r*N_nodes ; edge_id = rang_arête + r*N_edges ;
  source/target remappés via la même table + offset.
- Propriétés/labels copiés tels quels => distribution de labels et de degrés conservée.
- Ordre de sortie : tous les blocs de nœuds (r=0..R-1) puis tous les blocs d'arêtes,
  comme le dump d'origine (nœuds avant arêtes).

Usage: gen_xn.py <dump.jsonl> <sortie.jsonl> <facteur>
"""
import json
import sys
import time


def main() -> None:
    src, dst, factor = sys.argv[1], sys.argv[2], int(sys.argv[3])
    t0 = time.monotonic()

    id_map: dict[int, int] = {}          # old node id -> rang contigu (1-based)
    node_tmpl: list[str] = []            # suffixe JSON sans l'id
    edge_tmpl: list[tuple[int, int, str]] = []  # (src_rang, tgt_rang, suffixe)

    with open(src) as f:
        for line in f:
            d = json.loads(line)
            if d["type"] == "node":
                rank = len(node_tmpl) + 1
                id_map[d["id"]] = rank
                body = json.dumps(
                    {"labels": d["labels"], "properties": d["properties"], "type": "node"},
                    separators=(", ", ": "),
                )
                node_tmpl.append(body[1:])  # sans '{' initial
            elif d["type"] == "edge":
                body = json.dumps(
                    {"edge_type": d["edge_type"], "properties": d["properties"], "type": "edge"},
                    separators=(", ", ": "),
                )
                edge_tmpl.append((id_map[d["source"]], id_map[d["target"]], body[1:]))
            else:
                raise SystemExit(f"type inconnu: {d['type']}")

    n_nodes, n_edges = len(node_tmpl), len(edge_tmpl)
    print(f"source: {n_nodes} noeuds / {n_edges} aretes ; facteur x{factor}", file=sys.stderr)

    with open(dst, "w", buffering=1 << 22) as out:
        for r in range(factor):
            off = r * n_nodes
            out.writelines(
                f'{{"id": {off + i}, {tmpl}\n' for i, tmpl in enumerate(node_tmpl, 1)
            )
        for r in range(factor):
            off = r * n_nodes
            eoff = r * n_edges
            out.writelines(
                f'{{"id": {eoff + i}, "source": {off + s}, "target": {off + t}, {tmpl}\n'
                for i, (s, t, tmpl) in enumerate(edge_tmpl, 1)
            )

    dt = time.monotonic() - t0
    print(
        f"ecrit: {factor * n_nodes} noeuds / {factor * n_edges} aretes en {dt:.1f}s",
        file=sys.stderr,
    )


if __name__ == "__main__":
    main()
