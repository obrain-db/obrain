#!/usr/bin/env python3
"""Garde DoD de la couche viz (P0-v06, voir docs/FULL-OBRAIN-VIZ-ROADMAP §5).

Vérifie sur un corpus réel (généré si aucun store fourni) :
  1. viz_subgraph 2-hops sur un nœud connu → non vide ET bien formé (pas
     d'arête pendante, ids uniques) ET déterministe octet-à-octet ;
  2. échappements writers (dot/mermaid) — verrouillés par golden en unit, ici
     contrôle structurel de sortie (comptes de lignes cohérents) ;
  3. bornes : limit → `truncated: true` + comptes attendus ;
  4. parité des comptes : export_all (json-graph v1) == comptage store
     (MATCH count) ;
  5. bench (--bench [store]) : latence export entier + subgraph 2-hops (3 runs,
     p50/p95 empiriques) sur le store fourni ou le corpus généré.

Usage :
  scripts/guard-viz.sh                  # garde complète (corpus jetable)
  scripts/guard-viz.sh --bench [store]  # garde + bench chiffré
  VIZ_STORE=<store> scripts/guard-viz.sh # garde sur un store réel existant
"""
from __future__ import annotations

import hashlib
import json
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

BIN = Path(os.environ.get("OBRAIN_BIN", "target/debug/obrain")).resolve()
CORPUS_NODES = 80  # chaîne g.0..g.79 (+ une étoile) — suffisant, garde rapide
CENTER = "g.0"


def run(args, timeout=240):
    p = subprocess.run(
        [str(BIN), *args],
        capture_output=True,
        text=True,
        timeout=timeout,
    )
    return p


def run_node(jsdir: str, script: str):
    p = subprocess.run(
        ["node", "--input-type=module", "-e", script],
        capture_output=True,
        text=True,
        cwd=jsdir,
        timeout=120,
    )
    if p.returncode != 0:
        raise SystemExit(f"guard: node render échoué : {p.stderr[-800:]}")
    return p


def parse_json(text):
    return json.loads(text)


def create_corpus() -> str:
    """Store jetable (moteur classique init) : chaîne CALLS g.0→g.1→…→g.N."""
    tmp = tempfile.mkdtemp(prefix="obrain-viz-guard-")
    store = Path(tmp) / "corpus.obrain"
    r = run(["init", str(store)])
    if r.returncode != 0:
        raise SystemExit(f"guard: init a échoué : {r.stderr[-800:]}")
    clauses = []
    names = []
    for i in range(CORPUS_NODES):
        v = f"v{i}"
        names.append(v)
        clauses.append(f"CREATE ({v}:Function {{qualified_name: 'g.{i}'}})")
    for i in range(CORPUS_NODES - 1):
        clauses.append(f"CREATE ({names[i]})-[:CALLS]->({names[i + 1]})")
    # Étoile sur g.10 pour un peu de densité (hubs).
    clauses.append("CREATE (hub:Function {qualified_name: 'g.hub'})")
    for j in range(5):
        clauses.append(
            f"CREATE (s{j}:Function {{qualified_name: 'g.star{j}'}})"
        )
        clauses.append("CREATE (hub)-[:CALLS]->(s" + str(j) + ")")
    statement = " ".join(clauses) + " RETURN count(v0)"
    r = run(["query", str(store), statement, "--format", "json"])
    if r.returncode != 0:
        raise SystemExit(f"guard: création du corpus échouée : {r.stderr[-800:]}")
    return str(store)


def check(ok: bool, msg: str):
    print(("PASS  " if ok else "FAIL  ") + msg)
    if not ok:
        sys.exit(1)


def artifact(store: str, center: str, hops: int, fmt: str, extra=None) -> str:
    args = ["viz", "subgraph", store, center, "--hops", str(hops), "--as", fmt]
    if extra:
        args += extra
    r = run(args)
    if r.returncode != 0:
        raise SystemExit(f"guard: viz {fmt} échoué : {r.stderr[-800:]}")
    return r.stdout


def store_counts(store: str):
    def count(query: str) -> int:
        r = run(["query", store, query, "--format", "json"])
        if r.returncode != 0:
            raise SystemExit(f"guard: query échouée : {r.stderr[-800:]}")
        rows = parse_json(r.stdout)
        val = rows[0][next(iter(rows[0]))]
        return int(val) if val is not None else 0

    return count("MATCH (n) RETURN count(n)"), count("MATCH ()-[r]->() RETURN count(r)")


def main():
    args = sys.argv[1:]
    bench_only = "--bench" in args
    bench_store = None
    if "--bench" in args:
        i = args.index("--bench")
        bench_store = args[i + 1] if len(args) > i + 1 and not args[i + 1].startswith("--") else None

    env_store = os.environ.get("VIZ_STORE")
    own_corpus = env_store is None and bench_store is None
    store = env_store or bench_store
    tmp_kept = None
    if store is None:
        store = create_corpus()
        tmp_kept = Path(store).parent
    print(f"guard: store = {store}")

    if not bench_only:
        # 1. Non vide + bien formé + déterministe (json-graph v1, 2-hops).
        a1 = artifact(store, CENTER, 2, "json")
        a2 = artifact(store, CENTER, 2, "json")
        check(
            hashlib.sha256(a1.encode()).hexdigest() == hashlib.sha256(a2.encode()).hexdigest(),
            "déterminisme octet-à-octet (2 runs identiques)",
        )
        g = parse_json(a1)
        check(g.get("schema") == "obrain/json-graph@1", f"schéma json-graph v1 (got {g.get('schema')})")
        nodes = g["nodes"]
        edges = g["edges"]
        check(len(nodes) > 0 and len(edges) > 0, f"sous-graphe non vide ({len(nodes)} nœuds / {len(edges)} arêtes)")
        ids = [n["id"] for n in nodes]
        check(len(ids) == len(set(ids)), "ids uniques")
        id_set = set(ids)
        dangling = [e for e in edges if e["src"] not in id_set or e["dst"] not in id_set]
        check(not dangling, "aucune arête pendante")
        check(g["stats"]["node_count"] == len(nodes) and g["stats"]["edge_count"] == len(edges),
              "stats cohérentes avec le contenu")

        # 2. Sorties writers structurellement cohérentes (comptes).
        dot = artifact(store, CENTER, 2, "dot")
        mmd = artifact(store, CENTER, 2, "mermaid")
        dot_decl = sum(
            1
            for line in dot.splitlines()
            if line.startswith('    "') and "[label=" in line and "->" not in line
        )
        dot_edges = dot.count(" -> ")
        check(dot.startswith("digraph obrain_viz {") and dot_decl == len(nodes) and dot_edges == len(edges),
              f"DOT : {dot_decl} déclarations / {dot_edges} arêtes == sous-graphe")
        mmd_edges = mmd.count(" -->|")
        check(mmd.startswith("flowchart LR") and mmd_edges == len(edges),
              f"Mermaid : {mmd_edges} arêtes typées == sous-graphe")

        # 3. Bornes : limit → truncated + comptes attendus.
        lim = parse_json(artifact(store, "g.hub", 1, "json", ["--limit", "2"]))
        check(
            lim["stats"]["truncated"] is True and len(lim["nodes"]) == 2,
            f"limit=2 → truncated={lim['stats'].get('truncated')} (nodes={len(lim['nodes'])})",
        )

            # 3bis (V4-C, optionnel) : rendu DOT réel → SVG (WASM @viz-js/viz)
        # si VIZ_RENDER=1 et le paquet node est joignable (VIZ_JS_DIR).
        if os.environ.get("VIZ_RENDER") == "1":
            jsdir = os.environ.get("VIZ_JS_DIR", "/tmp/pertinence")
            dot2 = artifact(store, CENTER, 2, "dot")
            script = (
                "const {instance}=await import('@viz-js/viz');const viz=await instance();"
                f"const svg=viz.renderString({dot2!r},{{format:'svg'}});"
                "const nodes=(svg.match(/class=\"node\"/g)||[]).length;"
                "const edges=(svg.match(/class=\"edge\"/g)||[]).length;"
                f"console.log(JSON.stringify({{ok:svg.includes('<svg'),nodes,edges,expectNodes:{len(nodes)},expectEdges:{len(edges)}}}));"
            )
            r = run_node(jsdir, script)
            import json as _json
            res = _json.loads(r.stdout.strip().splitlines()[-1])
            check(
                res.get("ok")
                and res.get("nodes") == res.get("expectNodes")
                and res.get("edges") == res.get("expectEdges"),
                f"rendu DOT→SVG réel : {res.get('nodes')} nœuds / {res.get('edges')} arêtes == sous-graphe",
            )
        else:
            print("SKIP  rendu SVG (optionnel — VIZ_RENDER=1 + paquet @viz-js/viz requis)")

    # 4. Parité export_all == comptage store.
        exp_r = run(["viz", "export", store, "--as", "json"])
        check(exp_r.returncode == 0, "viz export s'exécute")
        exp = parse_json(exp_r.stdout)
        sn, se = store_counts(store)
        check(exp["stats"]["node_count"] == sn and exp["stats"]["edge_count"] == se,
              f"parité export_all : {exp['stats']['node_count']} nœuds / {exp['stats']['edge_count']} arêtes == store ({sn}/{se})")

        # 5 (V4-G2a) : serve — lancement, requête, kill, parité avec le sous-graphe.
        import urllib.request as _u
        port = 18924
        srv = subprocess.Popen(
            [str(BIN), "viz", "serve", "--store", store, "--port", str(port)],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        try:
            body = None
            for _ in range(20):
                try:
                    with _u.urlopen(
                        f"http://127.0.0.1:{port}/api/viz/subgraph?center={CENTER}&hops=2",
                        timeout=2,
                    ) as r:
                        body = json.load(r)
                    break
                except Exception:
                    time.sleep(0.25)
            ok_endpoint = (
                body is not None
                and body.get("stats", {}).get("node_count") == len(nodes)
                and body.get("stats", {}).get("edge_count") == len(edges)
                and isinstance(body.get("stats", {}).get("truncated"), bool)
            )
            check(
                ok_endpoint,
                f"serve /api/viz/subgraph : {body.get('stats', {}) if body else None} "
                f"== sous-graphe ({len(nodes)}n/{len(edges)}e, truncated bool)",
            )
            try:
                with _u.urlopen(f"http://127.0.0.1:{port}/", timeout=2) as r:
                    home = r.read().decode()
                check(
                    "Obrain Dashboard" in home,
                    "serve / : dashboard React servi",
                )
            except Exception as e:
                check(False, f"serve / : viewer injoignable ({e})")
        finally:
            srv.kill()

    # 6. Bench (toujours si demandé) : export entier + subgraph 2-hops ×3.
    if bench_only or bench_store is not None or env_store is not None:
        times_exp, times_sub = [], []
        for _ in range(3):
            t0 = time.perf_counter()
            run(["viz", "export", store, "--as", "json"])
            times_exp.append((time.perf_counter() - t0) * 1000)
            t0 = time.perf_counter()
            run(["viz", "subgraph", store, CENTER, "--hops", "2", "--as", "json"])
            times_sub.append((time.perf_counter() - t0) * 1000)
        med = lambda xs: round(statistics.median(xs), 1)
        p95 = lambda xs: round(sorted(xs)[int(len(xs) * 0.95)], 1)
        sn, se = store_counts(store)
        print(f"bench: store={store} ({sn} nœuds / {se} arêtes)")
        print(f"bench: export_all json   — median {med(times_exp)} ms / p95 {p95(times_exp)} ms")
        print(f"bench: subgraph 2-hops   — median {med(times_sub)} ms / p95 {p95(times_sub)} ms")

    if tmp_kept is not None:
        shutil.rmtree(tmp_kept, ignore_errors=True)
    print("guard-viz: OK")


if __name__ == "__main__":
    main()
