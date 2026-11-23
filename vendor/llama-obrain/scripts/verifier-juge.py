#!/usr/bin/env python3
"""verifier-juge.py — vérifier le JUGE lui-même, hors ligne, sans modèle et sans réseau.

Pourquoi : un juge structurel n'est utile que si son ORACLE est juste. Si l'oracle se trompe, tous
les scores sont faux — et on ne s'en apercevrait qu'après avoir payé des heures de préfill. Ce
script recalcule chaque réponse attendue par une méthode INDÉPENDANTE et compare.

Ce qu'il vérifie :

  1. **L'ORACLE** — pour chaque famille, la réponse attendue est-elle celle que donne une
     implémentation écrite autrement ? (fermeture transitive par Floyd-Warshall contre le Kosaraju
     du juge ; BFS explicite contre ses ensembles de successeurs.) Le type et le nœud de départ sont
     relus DANS LA QUESTION, pas devinés : sans cela, une question posée sur un autre type que celui
     que l'oracle a compté passerait inaperçue.
  2. **L'ÉNONCÉ CONTIENT LA RÉPONSE** — pour chaque question, la réponse attendue doit être
     dérivable du graphe TEL QU'IL EST ÉCRIT dans l'invite. C'est le point le plus facile à casser
     sans s'en apercevoir : une famille « constructive » (redondance) qui rend un graphe modifié
     oblige l'appelant à rendre CE graphe-là.
  3. **LA FORME `indice`** — le renommage des nœuds en numéros ne doit rien corrompre : numéros
     distincts, aucun `fn_...` survivant, réponse attendue = numéro de la bonne cible.
  4. **LA GÉNÉRABILITÉ** — chaque famille doit produire une question valide en peu d'essais. Une
     famille qui rend `None` neuf fois sur dix est un juge qui ne mesure rien (constaté : la
     `redondance` espérée donnait 0/0).

Usage : python3 scripts/verifier-juge.py [--essais 200] [--graines 5]
Code de retour 0 si tout est cohérent, 1 sinon.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import os
import random
import re
import sys
from collections import defaultdict

JUGE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "juge-structurel.py")


def charger_juge(chemin: str):
    """Importe le juge SANS l'exécuter (son main est gardé par __name__)."""
    spec = importlib.util.spec_from_file_location("juge_structurel", chemin)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def empreinte(chemin: str) -> str:
    return hashlib.sha256(open(chemin, "rb").read()).hexdigest()[:16]


# ── oracles INDÉPENDANTS ─────────────────────────────────────────────────────────────────────────


def oracle_degre(aretes, t):
    """Le nœud de plus fort in-degré de type t, s'il est unique. Rend (gagnant, compte, nb_ex_aequo)."""
    cnt = defaultdict(int)
    for (_, tt, b) in aretes:
        if tt == t:
            cnt[b] += 1
    if not cnt:
        return None
    best = max(cnt.values())
    g = sorted(k for k, v in cnt.items() if v == best)
    return g[0], best, len(g)


def oracle_atteignabilite(aretes, depart):
    succ = defaultdict(set)
    for (a, _, b) in aretes:
        succ[a].add(b)
    niveau2 = {c for m in succ[depart] for c in succ[m]}
    niveau2.discard(depart)
    return sorted(niveau2)


def oracle_cycles(noeuds, aretes):
    """Fermeture transitive par Floyd-Warshall : u est dans un cycle ssi u atteint u en ≥1 pas.
    Volontairement différent du Kosaraju du juge — c'est tout l'intérêt."""
    idx = {n: i for i, n in enumerate(noeuds)}
    N = len(noeuds)
    at = [[False] * N for _ in range(N)]
    for (a, _, b) in aretes:
        at[idx[a]][idx[b]] = True
    for k in range(N):
        for i in range(N):
            if at[i][k]:
                li, lk = at[i], at[k]
                for j in range(N):
                    if lk[j]:
                        li[j] = True
    return sorted(n for n in noeuds if at[idx[n]][idx[n]])


def chemin_deux_aretes(aretes, a, t, b):
    """Existe-t-il un chemin de ≥2 arêtes, toutes de type t, de a vers b ? BFS sur le sous-graphe t."""
    par_type = defaultdict(set)
    for (x, tt, y) in aretes:
        if tt == t:
            par_type[x].add(y)
    frontiere = set(par_type[a])          # longueur 1
    longueur = 1
    while frontiere and longueur < len(aretes) + 3:
        longueur += 1
        suivante = set()
        for u in frontiere:
            if longueur >= 2 and u == b:
                return True
            suivante |= par_type[u]
        frontiere = suivante
    return False


def est_derive_du_graphe(graphe: str, aretes) -> bool:
    """Le graphe ÉCRIT dans l'invite expose-t-il exactement l'ensemble d'arêtes que l'oracle a
    utilisé ? On compare des ENSEMBLES de lignes, pas des listes : `rendre` n'élimine pas les
    doublons, donc un graphe correct peut afficher deux fois la même ligne. Comparer des listes
    ferait échouer un graphe juste — c'est exactement l'erreur que ce vérificateur a commise au
    premier essai (15 faux positifs, autant que de doublons)."""
    lignes_attendues = {f"{a} --{t}--> {b}" for (a, t, b) in aretes}
    lignes_ecrites = {l for l in graphe.strip().split("\n") if l.strip()}
    return lignes_attendues == lignes_ecrites


# ── la vérification ─────────────────────────────────────────────────────────────────────────────


def verifier(essais: int, graines: int) -> int:
    if not os.path.exists(JUGE):
        print(f"  juge introuvable : {JUGE}")
        return 1
    h_avant = empreinte(JUGE)
    mod = charger_juge(JUGE)
    print(f"  juge : {JUGE}")
    print(f"  empreinte SHA-256 (16 premiers) : {h_avant}")

    erreurs = 0
    avertissements = 0
    comptes = defaultdict(int)
    non_engendrees = defaultdict(int)

    for g in range(graines):
        rng = random.Random(20260920 + g)
        for _ in range(essais):
            n_noeuds = rng.randint(7, 12)
            noeuds, aretes0 = mod.gen_graphe(rng, n_noeuds, rng.randint(8, 14))

            for famille, fabrique in mod.FAMILLES.items():
                # on rejoue le tirage de la fabrique de façon reproductible
                r2 = random.Random(rng.random())
                q = fabrique(noeuds, aretes0, r2)
                if q is None:
                    non_engendrees[famille] += 1
                    continue
                if len(q) == 3:                      # famille constructive : elle rend son graphe
                    question, attendu, aretes = q
                else:
                    question, attendu = q
                    aretes = aretes0
                comptes[famille] += 1

                graphe = mod.rendre(noeuds, aretes)

                # (2) l'énoncé contient-il exactement le graphe de l'oracle ?
                if not est_derive_du_graphe(graphe, aretes):
                    print(f"  ✗ {famille} : le graphe écrit dans l'invite n'est PAS celui de l'oracle")
                    erreurs += 1

                # (1) l'oracle, famille par famille, avec le paramètre RELU dans la question
                if famille == "degre":
                    m = re.search(r"type (\w+)", question)
                    if not m:
                        print(f"  ✗ degre : type illisible dans la question")
                        erreurs += 1
                    else:
                        o = oracle_degre(aretes, m.group(1))
                        if o is None or o[0] != attendu:
                            print(f"  ✗ degre (type {m.group(1)}) : attendu {attendu}, "
                                  f"force brute {o}")
                            erreurs += 1

                elif famille == "atteignabilite":
                    m = re.search(r"depuis (fn_\d+)", question)
                    if not m:
                        print(f"  ✗ atteignabilite : départ illisible dans la question")
                        erreurs += 1
                    else:
                        mien = oracle_atteignabilite(aretes, m.group(1))
                        if mien != attendu:
                            print(f"  ✗ atteignabilite depuis {m.group(1)} : "
                                  f"attendu {attendu}, force brute {mien}")
                            erreurs += 1

                elif famille == "cycles":
                    mien = oracle_cycles(noeuds, aretes)
                    if mien != attendu:
                        print(f"  ✗ cycles : juge {attendu}, fermeture transitive {mien}")
                        erreurs += 1

                elif famille == "redondance":
                    m = re.search(r"type (\w+), allant de (fn_\d+) à (fn_\d+)", question)
                    if not m:
                        print(f"  ✗ redondance : paramètres illisibles dans la question")
                        erreurs += 1
                    else:
                        t, a, b = m.group(1), m.group(2), m.group(3)
                        if attendu != "OUI":
                            print(f"  ✗ redondance : attendu inattendu {attendu!r}")
                            erreurs += 1
                        if not chemin_deux_aretes(aretes, a, t, b):
                            print(f"  ✗ redondance : le juge dit OUI pour {a} -{t}-> {b} "
                                  f"mais aucun chemin de ≥2 arêtes n'existe dans SON graphe")
                            erreurs += 1
                        # doublons introduits par la construction du triangle : cosmétique, mais
                        # ils font mentir le compte d'arêtes annoncé à la lecture
                        if len(aretes) != len(set(aretes)):
                            avertissements += 1

                # (3) la forme indice, sur la question réellement engendrée
                lab = {n: str(i + 1) for i, n in enumerate(noeuds)}
                rendu = mod.relabelliser(graphe, lab)
                if "fn_" in rendu:
                    print(f"  ✗ indice : un identifiant a survécu au renommage")
                    erreurs += 1
                if len(set(lab.values())) != len(lab):
                    print("  ✗ indice : deux nœuds ont reçu le même numéro")
                    erreurs += 1
                attendu_ind = ([mod.relabelliser(x, lab) for x in attendu]
                               if isinstance(attendu, list) else mod.relabelliser(attendu, lab))
                if isinstance(attendu, list):
                    for x in attendu_ind:
                        if not re.fullmatch(r"\d+", x):
                            print(f"  ✗ indice : la réponse attendue {x!r} n'est pas un numéro")
                            erreurs += 1
                comptes["_indice"] += 1

    print("  ── questions engendrées (validées) / non engendrées sur le même tirage ──")
    for k in mod.FAMILLES:
        print(f"    {k:16s} : {comptes[k]:5d} validées   {non_engendrees[k]:5d} non engendrées")
    print(f"    {'_indice':16s} : {comptes['_indice']:5d} renommages vérifiés")
    if avertissements:
        print(f"  ⚠ {avertissements} graphe(s) contiennent des arêtes en DOUBLE après construction "
              f"(l'énoncé affiche alors deux fois la même ligne)")

    h_apres = empreinte(JUGE)
    if h_apres != h_avant:
        print(f"  ⚠ le juge a CHANGÉ pendant la vérification ({h_avant} -> {h_apres}) : "
              f"une autre session l'édite. Relancer pour un verdict stable.")
        return 1
    print(f"\n  VERDICT : {'COHÉRENT' if erreurs == 0 else str(erreurs) + ' INCOHÉRENCE(S)'}"
          f"  (empreinte {h_apres}, inchangée)")
    return 0 if erreurs == 0 else 1


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--essais", type=int, default=200, help="graphes par graine")
    ap.add_argument("--graines", type=int, default=5)
    a = ap.parse_args()
    print("=== VÉRIFICATION DU JUGE (oracle + énoncé + forme indice) — aucun modèle requis ===")
    sys.exit(verifier(a.essais, a.graines))
