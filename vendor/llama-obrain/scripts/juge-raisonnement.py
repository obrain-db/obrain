#!/usr/bin/env python3
"""juge-raisonnement.py — le juge mesure-t-il le MODÈLE ou la CONFIGURATION ?

Le constat qui commande cet essai (MESURÉ le 21/09 à 00:00 sur n3) : la RÉFÉRENCE Q4_K_M — le
meilleur modèle disponible, PPL 2,3983 — obtient 0/3 sur la famille `cycles` et 1/3 sur
`atteignabilite`, et ses échecs sont toujours des SOUS-ENSEMBLES de la réponse attendue. Or le juge
interroge avec `chat_template_kwargs: {"enable_thinking": False}` : le modèle doit donc résoudre un
problème multi-sauts SANS brouillon, en une seule passe.

Deux lectures opposées, et le choix change tout :
  - si le plafond reste bas avec brouillon → ces familles ne discriminent pas, l'instrument est à
    refaire ;
  - si le plafond monte avec brouillon → l'instrument était valide, c'était le RÉGLAGE qui mesurait
    « sait-il faire sans réfléchir », pas « sait-il faire ».

D'où un essai APPARIÉ : les mêmes questions, mot pour mot, posées deux fois au même modèle servi une
seule fois, la seule variable étant l'autorisation de raisonner. Les questions sont reproduites par
le générateur du juge lui-même (même graine, même consommation du générateur), donc rien n'est écrit
à la main et les deux bras sont comparables question par question.

Usage :
  python3 scripts/juge-raisonnement.py <port> [--n 3] [--seed 20260920] [--familles cycles,atteignabilite]
  python3 scripts/juge-raisonnement.py <port> --journal /mnt/obrain/juge/raisonnement.tsv
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


def charger(chemin):
    spec = importlib.util.spec_from_file_location("juge_structurel", chemin)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def demander(port: int, invite: str, reflechir: bool, n_predict: int) -> tuple[str, str, float]:
    """Rend (réponse_nettoyée, raisonnement, secondes).

    `enable_thinking` est LE commutateur du gabarit Qwen3.8 (MESURÉ : `thinking_budget_tokens: 0`
    n'en est pas un). Selon la version du serveur, le raisonnement revient soit dans un champ
    `reasoning_content`, soit inline dans `content` avant `</think>` — on gère les deux, sinon on
    compterait le brouillon comme une réponse."""
    corps = json.dumps({
        "messages": [{"role": "user", "content": invite}],
        "n_predict": n_predict,
        "temperature": 0.0,
        "stream": False,
        "chat_template_kwargs": {"enable_thinking": reflechir},
    }).encode()
    req = urllib.request.Request(f"http://127.0.0.1:{port}/v1/chat/completions",
                                 data=corps, headers={"Content-Type": "application/json"})
    t0 = time.time()
    r = json.load(urllib.request.urlopen(req, timeout=7200))
    dt = time.time() - t0
    msg = r["choices"][0]["message"]
    contenu = (msg.get("content") or "")
    raison = (msg.get("reasoning_content") or "")
    if "</think>" in contenu:                     # raisonnement inline
        raison, _, contenu = contenu.partition("</think>")
    return contenu.strip(), raison.strip(), dt


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("port", type=int)
    ap.add_argument("--n", type=int, default=3)
    ap.add_argument("--seed", type=int, default=20260920)
    ap.add_argument("--familles", default="cycles,atteignabilite")
    ap.add_argument("--journal", default="/mnt/obrain/juge/raisonnement.tsv")
    ap.add_argument("--n-predict-reflexion", type=int, default=2048,
                    help="budget du bras AVEC brouillon. Doit être LARGE : un budget épuisé rend une "
                         "réponse vide qui se lit comme un échec du modèle (MESURÉ : à 1024, le "
                         "ternaire v3 épuise son budget en plein raisonnement).")
    a = ap.parse_args()

    try:
        urllib.request.urlopen(f"http://127.0.0.1:{a.port}/health", timeout=5)
    except Exception as e:
        print(f"  serveur injoignable sur :{a.port} — {e}")
        return 2

    import hashlib
    h = hashlib.sha256(open(JUGE, "rb").read()).hexdigest()[:16]
    mod = charger(JUGE)
    os.makedirs(os.path.dirname(a.journal), exist_ok=True)
    fh = open(a.journal, "a")

    print(f"=== JUGE vs RAISONNEMENT — port {a.port}, graine {a.seed}, n={a.n}, "
          f"juge {h} ===")
    print(f"    familles {a.familles} — mêmes questions dans les deux bras\n")

    rng = random.Random(a.seed)
    for nom in a.familles.split(","):
        nom = nom.strip()
        fabrique = mod.FAMILLES.get(nom)
        if fabrique is None:
            continue
        print(f"── {nom} " + "─" * 60)
        essais = tentatives = 0
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
            invite = (f"Voici un graphe orienté typé, une arête par ligne :\n{graphe}\n\n{question}")
            essais += 1

            ligne = {"famille": nom, "essai": essais, "attendu": attendu}
            for reflechir in (False, True):
                etiquette = "avec" if reflechir else "sans"
                # ATTENTION : le budget du bras « avec brouillon » doit être LARGE. MESURÉ le
                # 2026-09-21 : à 1024 jetons, le ternaire `v3` a épuisé son budget EN PLEIN
                # RAISONNEMENT et rendu une réponse VIDE — un score de 0 qui ne dit rien du modèle
                # et tout de mon plafond. Le raisonnement du ternaire dépasse 2 000 caractères là où
                # celui de la référence en fait 1 602 : un budget calibré sur la référence ne suffit
                # pas au candidat, et c'est précisément ce qu'on cherche à comparer.
                budget = a.n_predict_reflexion if reflechir else 64
                try:
                    rep, raison, dt = demander(a.port, invite, reflechir, budget)
                except Exception as e:
                    print(f"   ✗ appel ({etiquette}) : {e}")
                    break
                bon = mod.noter(rep, attendu)
                # un budget épuisé se NOMME, au lieu de se déguiser en échec du modèle
                epuise = reflechir and not rep and dt > 0
                ligne[f"{etiquette}_reponse"] = rep[:400]
                ligne[f"{etiquette}_raisonnement_car"] = len(raison)
                ligne[f"{etiquette}_bon"] = bon
                ligne[f"{etiquette}_budget_epuise"] = epuise
                ligne[f"{etiquette}_secondes"] = round(dt, 1)
                print(f"   [{etiquette:4s} brouillon] {'✓' if bon else '✗'}  {dt:5.0f}s  "
                      f"brouillon {len(raison):5d} car."
                      + ("   ⚠ BUDGET ÉPUISÉ — réponse vide, score non interprétable" if epuise else ""))
                print(f"        attendu : {attendu}")
                print(f"        obtenu  : {rep[:200]!r}")
            ligne["graphe"] = graphe
            ligne["question"] = question
            fh.write(json.dumps(ligne, ensure_ascii=False) + "\n")
            fh.flush()
        print()

    # synthèse chiffrée
    fh.close()
    print("=== SYNTHÈSE (relire le journal pour le détail) ===")
    print(f"    journal : {a.journal}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
