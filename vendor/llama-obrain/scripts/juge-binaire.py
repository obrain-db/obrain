#!/usr/bin/env python3
"""juge-binaire.py — l'énumération est-elle le vrai obstacle ?

Mesure qui a motivé ce script (étalonnage du 2026-09-20 23:25, référence Q4_K_M, n=3) :

    famille            forme de la réponse        plafond référence
    degre              un SEUL nœud               3/3  100 %
    redondance         OUI / NON                  2/3   67 %
    atteignabilite     énumérer un ENSEMBLE       1/3   33 %
    cycles             énumérer un ENSEMBLE       0/3    0 %

Les deux familles qui départagent sont exactement celles qui ne demandent PAS d'énumérer. Et sur
`atteignabilite`, la seule question à réponse unique (['fn_1009']) fait partie de l'unique réussite.
Le modèle exécute donc l'opération ; ce qu'il ne sait pas faire, c'est rendre l'ensemble COMPLET.

Ce script teste la conséquence directement : on repose la MÊME question de structure, mais découpée
en questions fermées — « le nœud X appartient-il à un cycle ? OUI/NON ». Si le plafond passe de 0 %
à ~100 %, l'obstacle était l'énumération, et non la capacité de raisonner sur le graphe : le juge
doit être réécrit en questions fermées, pas abandonné.

Les oracles sont ceux du juge, importés — jamais réécrits : une seule vérité terrain.

Usage :
    python3 scripts/juge-binaire.py <port> --familles cycles,atteignabilite --n 3 [--par-question 3]
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import random
import sys
import time
import urllib.request
from collections import defaultdict

JUGE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "juge-structurel.py")
JOURNAL = "/mnt/obrain/juge/binaire.jsonl"


def charger_juge(chemin: str):
    spec = importlib.util.spec_from_file_location("juge_structurel", chemin)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def demander(port: int, invite: str) -> tuple[str, float]:
    """Réglages IDENTIQUES au juge — seule la question change. C'est tout l'intérêt."""
    corps = json.dumps({
        "messages": [{"role": "user", "content": invite}],
        "n_predict": 64, "temperature": 0.0, "stream": False,
        "chat_template_kwargs": {"enable_thinking": False},
    }).encode()
    req = urllib.request.Request(f"http://127.0.0.1:{port}/v1/chat/completions",
                                 data=corps, headers={"Content-Type": "application/json"})
    t0 = time.time()
    r = json.load(urllib.request.urlopen(req, timeout=1800))
    return (r["choices"][0]["message"].get("content") or "").strip(), time.time() - t0


def poser(port, invite, attendu_oui: bool, jl, contexte: dict) -> bool:
    rep, dt = demander(port, invite)
    r = rep.strip().upper()
    bon = (r.startswith("OUI") if attendu_oui else r.startswith("NON"))
    jl.write(json.dumps({**contexte, "attendu": "OUI" if attendu_oui else "NON",
                         "obtenu": rep[:120], "bon": bool(bon), "dt_s": round(dt, 1),
                         "question": invite[-260:]}, ensure_ascii=False) + "\n")
    print(f"    {'✓' if bon else '✗'} ({dt:4.0f}s) attendu {'OUI' if attendu_oui else 'NON':3s} "
          f"| obtenu {rep[:40]!r}")
    return bon


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("port", type=int)
    ap.add_argument("--n", type=int, default=3)
    ap.add_argument("--seed", type=int, default=20260920)
    ap.add_argument("--familles", default="cycles,atteignabilite")
    ap.add_argument("--par-question", type=int, default=3,
                    help="sous-questions fermées posées par question de structure")
    a = ap.parse_args()

    try:
        urllib.request.urlopen(f"http://127.0.0.1:{a.port}/health", timeout=5)
    except Exception as e:
        print(f"  serveur injoignable sur :{a.port} — {e}")
        return 2

    mod = charger_juge(JUGE)
    os.makedirs(os.path.dirname(JOURNAL), exist_ok=True)
    jl = open(JOURNAL, "a", buffering=1)

    print(f"\n=== juge-binaire — port {a.port}, {a.par_question} sous-questions/question, "
          f"graine {a.seed} ===")
    bilan_total = [0, 0]
    for nom in a.familles.split(","):
        nom = nom.strip()
        f = mod.FAMILLES.get(nom)
        if f is None:
            continue
        rng = random.Random(a.seed)
        ok = tot = 0
        n_q = 0
        tent = 0
        while n_q < a.n and tent < a.n * 12:
            tent += 1
            nn = rng.randint(7, 12)
            noeuds, aretes0 = mod.gen_graphe(rng, nn, rng.randint(8, 14))
            q = f(noeuds, aretes0, rng)
            if q is None:
                continue
            if len(q) == 3:
                question, attendu, aretes = q
            else:
                question, attendu = q
                aretes = aretes0
            n_q += 1
            graphe = mod.rendre(noeuds, aretes)
            print(f"\n  ── {nom} question {n_q} (ensemble attendu : "
                  f"{len(attendu) if isinstance(attendu, list) else attendu}) ──")

            if nom == "cycles":
                dedans = set(attendu) if isinstance(attendu, list) else set()
                dehors = [x for x in noeuds if x not in dedans]
                # on interroge des DEUX côtés : sinon un modèle qui répond toujours OUI
                # (« tout est dans un cycle ») obtiendrait un score parfait
                dedans_liste = sorted(dedans)
                rng.shuffle(dedans_liste)
                rng.shuffle(dehors)
                choix = [(x, True) for x in dedans_liste[:2]] + [(x, False) for x in dehors[:1]]
                for x, oui in choix[:a.par_question]:
                    inv = (f"Voici un graphe orienté typé, une arête par ligne :\n{graphe}\n\n"
                           f"Dans ce graphe, le nœud {x} appartient-il à un cycle ? Un nœud "
                           f"appartient à un cycle s'il existe un chemin non vide de lui-même vers "
                           f"lui-même. Réponds par OUI ou NON, rien d'autre.")
                    tot += 1
                    ok += poser(a.port, inv, oui, jl, {"famille": nom, "question": n_q, "noeud": x})

            elif nom == "atteignabilite":
                m = __import__("re").search(r"depuis (fn_\d+)", question)
                depart = m.group(1) if m else None
                dedans = set(attendu) if isinstance(attendu, list) else set()
                from collections import defaultdict as _dd
                succ = _dd(set)
                for (u, _, v) in aretes:
                    succ[u].add(v)
                niveau1 = succ[depart]
                # un nœud à exactement 1 arête est l'appât : il est *proche* mais la réponse est NON
                appats = [x for x in niveau1 if x not in dedans]
                dedans_liste = sorted(dedans)
                rng.shuffle(dedans_liste)
                rng.shuffle(appats)
                choix = [(x, True) for x in dedans_liste[:2]] + [(x, False) for x in appats[:1]]
                for x, oui in choix[:a.par_question]:
                    inv = (f"Voici un graphe orienté typé, une arête par ligne :\n{graphe}\n\n"
                           f"Dans ce graphe, le nœud {x} est-il atteignable depuis {depart} en "
                           f"EXACTEMENT 2 arêtes ? Réponds par OUI ou NON, rien d'autre.")
                    tot += 1
                    ok += poser(a.port, inv, oui, jl, {"famille": nom, "question": n_q, "noeud": x})
            else:
                print(f"    (famille {nom} non binaire — ignorée)")
        taux = ok / tot if tot else 0.0
        bilan_total[0] += ok
        bilan_total[1] += tot
        print(f"  ══ {nom:16s} {ok}/{tot} = {100*taux:5.1f} %  [questions FERMÉES]")
    jl.close()
    if bilan_total[1]:
        print(f"\n  TOTAL fermé : {bilan_total[0]}/{bilan_total[1]} = "
              f"{100*bilan_total[0]/bilan_total[1]:.1f} %  "
              f"(à comparer aux plafonds 0 % et 33 % en énumération)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
