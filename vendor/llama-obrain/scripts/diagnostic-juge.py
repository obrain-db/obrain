#!/usr/bin/env python3
"""diagnostic-juge.py — reproduire les questions du juge HORS LIGNE et regarder l'oracle en face.

Pourquoi : la référence Q4_K_M obtient 0/3 sur `cycles` et 1/3 sur `atteignabilite`, toujours en
rendant un SOUS-ENSEMBLE de la réponse attendue. Deux explications opposées, et il faut trancher
avant d'accuser le modèle :

  - **le modèle est imprécis** → le juge est valide, ces familles sont simplement discriminantes ;
  - **l'oracle ou la phrase est en cause** → le juge est INJUSTE, et tous les scores sont faux.

Le cas suspect, précis : `atteignabilite` demande les nœuds « atteignables en EXACTEMENT 2 arêtes ».
L'oracle construit le niveau 2 et n'en retire QUE le nœud de départ — il CONSERVE un successeur
direct qui serait aussi atteignable en 2 pas. Un modèle qui lit « exactement 2 » comme « par un
chemin de longueur 2, et non par un chemin plus court » l'omettrait, et aurait raison de le faire.

Ce script rejoue le tirage du juge à l'identique (même graine, même consommation du générateur) et
affiche, pour chaque question : le graphe, la réponse attendue, et pour chaque nœud attendu sa
classe de distance réelle depuis le départ. Aucun modèle, aucun réseau.

Usage : python3 scripts/diagnostic-juge.py [--seed 20260920] [--n 3] [--famille atteignabilite]
"""

from __future__ import annotations

import argparse
import importlib.util
import os
import random
import sys
from collections import defaultdict

JUGE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "juge-structurel.py")


def charger(chemin):
    spec = importlib.util.spec_from_file_location("juge_structurel", chemin)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def distances(aretes, depart, maxpas=4):
    """Distance minimale de `depart` à chaque nœud, par BFS. Rend {noeud: distance}."""
    succ = defaultdict(set)
    for (a, _, b) in aretes:
        succ[a].add(b)
    dist = {depart: 0}
    frontiere = [depart]
    pas = 0
    while frontiere and pas < maxpas:
        pas += 1
        suivante = []
        for u in frontiere:
            for v in succ[u]:
                if v not in dist:
                    dist[v] = pas
                    suivante.append(v)
        frontiere = suivante
    return dist


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--seed", type=int, default=20260920)
    ap.add_argument("--n", type=int, default=3)
    ap.add_argument("--famille", default="atteignabilite")
    a = ap.parse_args()

    mod = charger(JUGE)
    print(f"=== DIAGNOSTIC HORS LIGNE — famille {a.famille}, graine {a.seed}, n={a.n} ===\n")

    rng = random.Random(a.seed)
    fabrique = mod.FAMILLES[a.famille]
    essais = tentatives = 0
    suspectes = 0

    while essais < a.n and tentatives < a.n * 12:
        tentatives += 1
        n_noeuds = rng.randint(7, 12)
        noeuds, aretes0 = mod.gen_graphe(rng, n_noeuds, rng.randint(8, 14))
        q = fabrique(noeuds, aretes0, rng)
        if q is None:
            continue
        if len(q) == 3:
            question, attendu, aretes = q
        else:
            question, attendu = q
            aretes = aretes0
        essais += 1

        print(f"── question {essais} " + "─" * 60)
        print("   GRAPHE :")
        for ligne in mod.rendre(noeuds, aretes).split("\n"):
            print(f"     {ligne}")
        print(f"\n   QUESTION : {question}")
        print(f"   ATTENDU  : {attendu}")

        if a.famille == "atteignabilite":
            import re
            m = re.search(r"depuis (fn_\d+)", question)
            d = distances(aretes, m.group(1))
            print(f"\n   distance minimale depuis {m.group(1)} :")
            for n in sorted(noeuds):
                if n in d:
                    marque = ""
                    if isinstance(attendu, list) and n in attendu and d[n] == 1:
                        marque = "   ← ATTENDU alors qu'atteignable en 1 SEUL pas (ambiguïté)"
                        suspectes += 1
                    print(f"     {n} : {d[n]} pas{marque}")
            # un nœud attendu atteignable en 1 pas est le cas litigieux
            litigieux = [n for n in (attendu or []) if d.get(n) == 1]
            if litigieux:
                print(f"   ⚠ {len(litigieux)} nœud(s) attendu(s) atteignable(s) en 1 pas : {litigieux}")
                print(f"     → un modèle qui lit « EXACTEMENT 2 » comme « pas en 1 » les omet,")
                print(f"       et l'oracle le compte alors comme une FAUTE.")

        if a.famille == "cycles":
            mien = mod.q_cycles.__globals__  # noqa: F841  (lisibilité : l'oracle est le juge lui-même)
            print(f"\n   (l'oracle est l'ensemble des nœuds d'une composante fortement connexe > 1)")

        print()

    print(f"=== fin — {essais} question(s) reproduite(s), {suspectes} nœud(s) litigieux ===")
    return 0


if __name__ == "__main__":
    sys.exit(main())
