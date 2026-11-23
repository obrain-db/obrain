#!/usr/bin/env python3
"""juge-reflexion.py — l'échec des familles dures vient-il du MODÈLE ou du SILENCE imposé ?

L'hypothèse, née d'une mesure (plafond sur la référence Q4_K_M, 2026-09-20 23:30) :

    cycles          0/3 =   0 %   (attendu 5 nœuds, obtenu 3 ; attendu 6, obtenu 5)
    atteignabilite  1/3 =  33 %   (attendu 2 nœuds, obtenu 1)

Les réponses sont bien formées — donc la notation n'est pas en cause. Le modèle rend un
SOUS-ENSEMBLE : il sous-énumère. Or `demander()` du juge envoie `enable_thinking: False`, avec ce
commentaire : « pour ces questions la réponse est un identifiant, pas un raisonnement ». Une
fermeture transitive EST un raisonnement, et un budget de 64 jetons ne permet pas de le tenir.

Ce script teste donc UNE seule variable : on repose **les mêmes questions** en laissant le modèle
penser, avec un budget de jetons suffisant. Tout le reste est emprunté au juge — ses familles, ses
oracles, sa notation — pour qu'aucune différence de résultat ne puisse venir d'ailleurs.

`--penser` est le seul interrupteur. `--sans-penser` rejoue la configuration d'origine, ce qui
permet de VÉRIFIER que ce script reproduit bien les questions du juge (mêmes réponses attendues)
avant de conclure quoi que ce soit du changement.

Usage :
    python3 scripts/juge-reflexion.py <port> --familles cycles,atteignabilite --n 3
    python3 scripts/juge-reflexion.py <port> --familles cycles --n 3 --sans-penser
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

JUGE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "juge-structurel.py")
JOURNAL = "/mnt/obrain/juge/reflexion.jsonl"


def charger_juge(chemin: str):
    spec = importlib.util.spec_from_file_location("juge_structurel", chemin)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def demander(port: int, invite: str, penser: bool, n_predict: int) -> tuple[str, float, dict]:
    """Identique au juge, sauf `enable_thinking` et le budget de jetons."""
    corps = json.dumps({
        "messages": [{"role": "user", "content": invite}],
        "n_predict": n_predict,
        "temperature": 0.0,
        "stream": False,
        "chat_template_kwargs": {"enable_thinking": penser},
    }).encode()
    req = urllib.request.Request(f"http://127.0.0.1:{port}/v1/chat/completions",
                                 data=corps, headers={"Content-Type": "application/json"})
    t0 = time.time()
    r = json.load(urllib.request.urlopen(req, timeout=1800))
    ch = r["choices"][0]
    msg = ch.get("message") or {}
    # le contenu peut porter le raisonnement balisé ; on garde les deux vues pour le diagnostic
    contenu = (msg.get("content") or "").strip()
    refus = (msg.get("reasoning_content") or "")
    return contenu, time.time() - t0, {"finish": ch.get("finish_reason"),
                                       "reasoning_car": len(refus),
                                       "usage": r.get("usage", {})}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("port", type=int)
    ap.add_argument("--n", type=int, default=3)
    ap.add_argument("--seed", type=int, default=20260920)
    ap.add_argument("--familles", default="cycles,atteignabilite")
    ap.add_argument("--penser", dest="penser", action="store_true", default=True)
    ap.add_argument("--sans-penser", dest="penser", action="store_false")
    ap.add_argument("--n-predict", type=int, default=0,
                    help="0 = 1024 si on pense, 64 sinon (le réglage du juge)")
    a = ap.parse_args()

    n_predict = a.n_predict or (1024 if a.penser else 64)
    try:
        urllib.request.urlopen(f"http://127.0.0.1:{a.port}/health", timeout=5)
    except Exception as e:
        print(f"  serveur injoignable sur :{a.port} — {e}")
        return 2

    mod = charger_juge(JUGE)
    os.makedirs(os.path.dirname(JOURNAL), exist_ok=True)
    jl = open(JOURNAL, "a", buffering=1)

    mode = "AVEC réflexion" if a.penser else "SANS réflexion (réglage du juge)"
    print(f"\n=== juge-reflexion — port {a.port}, {mode}, n_predict={n_predict}, "
          f"graine {a.seed} ===")

    for nom in a.familles.split(","):
        nom = nom.strip()
        fabrique = mod.FAMILLES.get(nom)
        if fabrique is None:
            continue
        # ── on REJOUE la boucle du juge à l'identique, pour retomber sur les MÊMES questions ──
        rng = random.Random(a.seed)
        ok = essais = 0
        tentatives = 0
        while essais < a.n and tentatives < a.n * 12:
            tentatives += 1
            n_noeuds = rng.randint(7, 12)
            noeuds, aretes = mod.gen_graphe(rng, n_noeuds, rng.randint(8, 14))
            q = fabrique(noeuds, aretes, rng)
            if q is None:
                continue
            if len(q) == 3:
                question, attendu, aretes = q
            else:
                question, attendu = q
            graphe = mod.rendre(noeuds, aretes)
            invite = f"Voici un graphe orienté typé, une arête par ligne :\n{graphe}\n\n{question}"
            try:
                reponse, dt, diag = demander(a.port, invite, a.penser, n_predict)
            except Exception as e:
                print(f"  erreur d'appel : {e}")
                break
            bon = mod.noter(reponse, attendu)
            essais += 1
            ok += bon
            jl.write(json.dumps({"famille": nom, "penser": a.penser, "n_predict": n_predict,
                                 "graine": a.seed, "attendu": attendu, "obtenu": reponse[:400],
                                 "bon": bool(bon), "dt_s": round(dt, 1), **diag,
                                 "question": question}, ensure_ascii=False) + "\n")
            marque = "✓" if bon else "✗"
            print(f"  {marque} ({dt:5.0f}s, {diag['finish']}, raisonnement {diag['reasoning_car']} car.) "
                  f"attendu {attendu} | obtenu {reponse[:90]!r}")
        taux = ok / essais if essais else 0.0
        print(f"  ── {nom:16s} {ok}/{essais} = {100*taux:5.1f} %  [{mode}]\n")
    jl.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
