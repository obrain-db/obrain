#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""metrics-report.py — AutoOpt : agrégateur / auto-découverte sur les métriques bridge.

Lit un fichier de métriques écrit par le logger AutoOpt du bridge obrain
(crates/obrain-cli/src/mcp_server.rs, env OBRAIN_METRICS_FILE) : une ligne JSON
par appel d'outil de lecture —

    {"ts":<ms epoch>,"tool":<nom>,"engine_us":<execution_time_ms*1000 si présent>,
     "wall_us":<délai interne>,"q":<préfixe 60 car. de la requête si query/query_graph>}

Imprime :
  1. le nombre d'échantillons et le top-10 des FORMES lentes (moyenne µs par
     forme = préfixe `q` pour query/query_graph, sinon outil ; mesure =
     engine_us, sinon wall_us) ;
  2. les RÉGRESSIONS SUSPECTES vs un fichier --baseline (même forme : moyenne
     ≥ +30 % ET écart ≥ 50 µs — le plancher évite les faux positifs de bruit
     aux échelles sub-µs) ;
  3. les SUGGESTIONS d'auto-amélioration : forme `WHERE <prop> = …` dont la
     moyenne dépasse 1 ms → commande exacte `create_property_index {prop}`
     (l'index d'égalité substrate existe déjà — outil bridge). Heuristique
     pure sur le texte (regex) ; RIEN n'est exécuté automatiquement. Comme le
     script ne voit pas le store, on peut lister les propriétés déjà indexées
     via --indexed pour supprimer les propositions correspondantes.

Exit : 0 si aucune régression, 1 si ≥ 1 régression suspecte (garde périodique),
2 sur erreur d'usage/IO. Les suggestions ne changent pas l'exit (advisory).

Usage:
  python3 metrics-report.py <fichier> [--baseline <baseline>] [--indexed p1,p2]
  python3 metrics-report.py --self-test
"""
from __future__ import annotations

import json
import os
import re
import statistics
import sys
import tempfile

SEUIL_REG_RATIO = 1.30        # +30 % de moyenne → régression suspecte
SEUIL_REG_ABS_US = 50.0       # plancher d'écart absolu (µs) anti-bruit
SEUIL_SUGGESTION_US = 1000.0  # forme `WHERE prop = …` > 1 ms
PREFIXE_Q = 60                # longueur du préfixe `q` (miroir du logger)

WHERE_EQ = re.compile(
    r"\bWHERE\s+(?:[A-Za-z_][A-Za-z0-9_]*\.)?([A-Za-z_][A-Za-z0-9_]*)\s*=",
    re.IGNORECASE,
)
TOOLS_QUERY = ("query", "query_graph")


# ── lecture / agrégation ──────────────────────────────────────────────────────

def load(path):
    """Lit le fichier JSONL → (samples, warn). Ligne invalide → warn+1."""
    samples, warn = [], 0
    with open(path, encoding="utf-8") as fh:
        for i, line in enumerate(fh, 1):
            line = line.strip()
            if not line:
                continue
            try:
                rec = json.loads(line)
            except ValueError:
                warn += 1
                continue
            if isinstance(rec, dict):
                samples.append(rec)
    return samples, warn


def us_of(rec):
    """µs d'un échantillon : engine_us (mesure moteur) sinon wall_us."""
    e = rec.get("engine_us")
    if isinstance(e, (int, float)) and not isinstance(e, bool):
        return float(e)
    w = rec.get("wall_us")
    if isinstance(w, (int, float)) and not isinstance(w, bool):
        return float(w)
    return None


def shape_of(rec):
    """Clé de forme : préfixe `q` (60 car., requêtes) sinon outil seul."""
    q = rec.get("q")
    if isinstance(q, str) and q:
        return q[:PREFIXE_Q]
    return "[tool:%s]" % rec.get("tool", "?")


def agregat(samples):
    """Regroupe par forme → liste {key, n, mean_us, tool, q} triée moyenne desc."""
    groups = {}
    for s in samples:
        u = us_of(s)
        if u is None:
            continue  # ligne sans aucune mesure exploitable
        key = shape_of(s)
        g = groups.setdefault(key, {"n": 0, "us": [], "tool": str(s.get("tool", "?")),
                                    "q": s.get("q")})
        g["n"] += 1
        g["us"].append(u)
    formes = [{"key": k, "n": g["n"], "tool": g["tool"], "q": g["q"],
               "mean_us": statistics.fmean(g["us"])}
              for k, g in groups.items()]
    formes.sort(key=lambda f: f["mean_us"], reverse=True)
    return formes


def regressions(formes_cur, formes_base):
    """Formes communes dont la moyenne actuelle ≥ +30 % (et ≥ 50 µs d'écart)."""
    base = {f["key"]: f["mean_us"] for f in formes_base}
    out = []
    for f in formes_cur:
        b = base.get(f["key"])
        if b is None or b <= 0:
            continue
        if f["mean_us"] >= b * SEUIL_REG_RATIO and (f["mean_us"] - b) >= SEUIL_REG_ABS_US:
            out.append({"key": f["key"], "tool": f["tool"],
                        "base_us": b, "cur_us": f["mean_us"],
                        "ratio": f["mean_us"] / b})
    out.sort(key=lambda r: r["ratio"], reverse=True)
    return out


def suggestions(formes, indexed=()):
    """Formes `WHERE <prop> = …` (query/query_graph) > 1 ms sans index connu."""
    indexed = set(indexed)
    vus = set()
    out = []
    for f in formes:  # tri desc : la forme la plus lente d'abord
        if f["mean_us"] <= SEUIL_SUGGESTION_US:
            break  # plus aucune forme au-dessus du seuil ensuite
        if f["tool"] not in TOOLS_QUERY or not isinstance(f["q"], str):
            continue
        m = WHERE_EQ.search(f["q"])
        if not m:
            continue
        prop = m.group(1)
        if prop in indexed or prop in vus:
            continue
        vus.add(prop)
        out.append({"prop": prop, "mean_us": f["mean_us"]})
    return out


# ── rendu ─────────────────────────────────────────────────────────────────────

def fmt_ms(us):
    return f"{us / 1000.0:.2f}"


def affiche_forme(f):
    if isinstance(f["q"], str) and f["q"]:
        texte = f["q"]
        if len(texte) >= PREFIXE_Q:
            texte += "…"
        return f"[{f['tool']}] «{texte}»"
    return f"[{f['tool']}] (sans requête)"


def render(samples, warn, formes, regs, sugs, path, baseline_path=None):
    outils = sorted({s.get("tool", "?") for s in samples})
    print("== metrics-report ==")
    print(f"fichier      : {path}")
    if baseline_path:
        print(f"baseline     : {baseline_path}")
    print(f"échantillons : {len(samples)}  (formes : {len(formes)}, outils : {', '.join(outils) or '—'})")
    print("mesure       : engine_us (sinon wall_us)")
    print(f"règle        : régression = moyenne ≥ +30 % (et ≥ {SEUIL_REG_ABS_US:.0f} µs d'écart) ; suggestion = `WHERE prop = …` > 1 ms")
    if warn:
        print(f"AVERTISSEMENT : {warn} ligne(s) JSON invalide(s) ignorée(s)")
    print()
    print("TOP-10 FORMES LENTES (moyenne µs/échantillon)")
    if not formes:
        print("  (aucun échantillon exploitable)")
    for i, f in enumerate(formes[:10], 1):
        print(f"  #{i:<2} {f['mean_us']:>10.1f} µs  n={f['n']:<4} {affiche_forme(f)}")
    print()
    print("RÉGRESSIONS SUSPECTES vs baseline (même forme, moyenne ≥ +30 %)")
    if not regs:
        print("  (aucune)")
    for r in regs:
        print(f"  RÉGRESSION suspecte : {r['key']}  baseline={r['base_us']:.1f} µs → "
              f"actuel={r['cur_us']:.1f} µs (+{(r['ratio'] - 1) * 100:.1f} %)")
    print()
    print("SUGGESTIONS AUTO-AMÉLIORATION (index d'égalité substrate)")
    if not sugs:
        print("  (aucune forme `WHERE prop = …` > 1 ms — rien à proposer)")
    for s in sugs:
        print(f"  SUGGESTION : create_property_index sur '{s['prop']}' "
              f"(forme lente {fmt_ms(s['mean_us'])} ms — attendu ~5 µs après index)")
    return 1 if regs else 0


# ── self-test (python3 metrics-report.py --self-test) ─────────────────────────

def self_test():
    """Fichier synthétique : forme lente → top + suggestion ; forme sous le
    seuil de régression → PAS de fausse alerte ; vraie régression → exit 1."""
    q_lent = "MATCH (n:Function) WHERE n.qualified_name = 'home.zz' RETURN n.qualified_name"
    q_rapide = "MATCH (n:Function) RETURN count(n) AS n"          # pas de WHERE → jamais suggérée
    q_name = "MATCH (n) WHERE n.name = 'fn' RETURN n.name"        # 300 µs → sous le seuil de suggestion
    q_reg = "MATCH (n:File) WHERE n.file_path = '/x' RETURN n.file_path"

    def ligne(us, q, tool="query"):
        return {"ts": 1, "tool": tool, "engine_us": us,
                "wall_us": us + 50, "q": q}

    # Actuel : forme lente n=2 (moyenne 2000 µs) → suggestion ; count(n) à
    # 1100 µs vs 1000 µs en baseline (+10 % < +30 %) → PAS de régression ;
    # name stable (300 µs partout) ; file_path en vraie régression
    # (100 → 300 µs, +200 %).
    cur = [ligne(1800, q_lent), ligne(2200, q_lent),
           ligne(300, q_name), ligne(1100, q_rapide),
           ligne(300, q_reg)]
    base = [ligne(1000, q_rapide), ligne(300, q_name), ligne(100, q_reg)]

    formes_cur, formes_base = agregat(cur), agregat(base)
    regs = regressions(formes_cur, formes_base)
    sugs = suggestions(formes_cur)

    # 1) Top-10 : la forme lente `qualified_name` y figure avec sa moyenne.
    top = {f["key"]: f for f in formes_cur[:10]}
    assert any("qualified_name" in k for k in top), "forme lente absente du top"
    f_lente = next(f for f in formes_cur if "qualified_name" in f["key"])
    assert abs(f_lente["mean_us"] - 2000.0) < 1e-6, f_lente

    # 2) Suggestion : exactement une, pour qualified_name (dédoublonnage sur 2
    #    échantillons), avec la commande exacte ; pas de suggestion pour name
    #    (300 µs < 1 ms) ni pour file_path (300 µs malgré sa régression).
    props = [s["prop"] for s in sugs]
    assert props == ["qualified_name"], f"suggestions : {sugs}"
    attendu = ("SUGGESTION : create_property_index sur 'qualified_name' "
               "(forme lente 2.00 ms — attendu ~5 µs après index)")
    assert attendu == ("SUGGESTION : create_property_index sur 'qualified_name' "
                       f"(forme lente {fmt_ms(2000.0)} ms — attendu ~5 µs après index)"), attendu

    # 3) Régressions : file_path oui (+200 %) ; count(n) NON (+10 % < +30 %) ;
    #    name NON (stable). → pas de faux positif sous le seuil.
    keys_reg = {r["key"] for r in regs}
    assert any("file_path" in k for k in keys_reg), f"régression manquante : {regs}"
    assert not any("count(n)" in k for k in keys_reg), f"fausse régression : {regs}"
    assert not any("n.name" in k for k in keys_reg), f"fausse régression : {regs}"

    # 4) Exit code : régression → 1 ; sans baseline → 0.
    assert (1 if regs else 0) == 1
    assert (1 if regressions(formes_cur, []) else 0) == 0

    # 5) Lecture JSONL réelle depuis un fichier temporaire.
    with tempfile.NamedTemporaryFile("w", suffix=".jsonl", delete=False,
                                     encoding="utf-8") as fh:
        for s in cur + base:
            fh.write(json.dumps(s) + "\n")
        tmp = fh.name
    try:
        samples, warn = load(tmp)
        assert warn == 0 and len(samples) == len(cur) + len(base)
    finally:
        os.unlink(tmp)

    # 6) --indexed supprime la suggestion (propriété déjà indexée au store).
    assert suggestions(formes_cur, indexed=["qualified_name"]) == []
    print("self-test OK : top-lente, suggestion > 1 ms (dédoublonnée), "
          "régression +30 %, pas de faux positif sous seuil, exit 0/1")


# ── main ──────────────────────────────────────────────────────────────────────

def main(argv):
    import argparse
    ap = argparse.ArgumentParser(
        prog="metrics-report.py",
        description="Agrégateur AutoOpt : top-10 formes lentes, régressions vs "
                    "--baseline (+30 %), suggestions create_property_index.",
    )
    ap.add_argument("fichier", nargs="?", help="fichier de métriques JSONL (OBRAIN_METRICS_FILE)")
    ap.add_argument("--baseline", metavar="F", help="fichier baseline (mêmes formes)")
    ap.add_argument("--indexed", metavar="p1,p2", help="propriétés déjà indexées (supprime les suggestions)")
    ap.add_argument("--self-test", action="store_true", help="auto-test du script")
    args = ap.parse_args(argv)

    if args.self_test:
        self_test()
        return 0
    if not args.fichier:
        ap.error("fichier requis")
    indexed = [p.strip() for p in (args.indexed or "").split(",") if p.strip()]
    try:
        samples, warn = load(args.fichier)
    except OSError as e:
        print(f"metrics-report: {e}", file=sys.stderr)
        return 2
    formes = agregat(samples)
    if args.baseline:
        try:
            bsamples, bwarn = load(args.baseline)
        except OSError as e:
            print(f"metrics-report: baseline illisible : {e}", file=sys.stderr)
            return 2
        warn += bwarn
        formes_base = agregat(bsamples)
    else:
        formes_base = []
    regs = regressions(formes, formes_base) if args.baseline else []
    sugs = suggestions(formes, indexed=indexed)
    return render(samples, warn, formes, regs, sugs, args.fichier, args.baseline)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
