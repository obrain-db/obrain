#!/usr/bin/env python3
"""juge-structurel.py — mesurer un modèle sur des STRUCTURES DE GRAPHE, pas sur du texte.

Pourquoi ce juge existe (l'idée vient du propriétaire du dépôt) : *« le graphe évolue constamment, et
c'est surtout sur des structures logiques de graphe qu'il m'intéresse de mesurer »*. Deux
conséquences de conception :

  1. **Les questions sont ENGENDRÉES**, jamais écrites à la main. Un corpus de questions figé pourrit
     dès que le graphe change ; un générateur, non.
  2. **Le graphe est l'ORACLE.** Chaque famille a une vérité terrain que le graphe calcule lui-même :
     aucun juge LLM, aucune annotation humaine, aucun « score » subjectif.

Pourquoi c'est plus discriminant que la perplexité (MESURÉ ailleurs dans ce dépôt) : ces familles
testent la **précision sur des identifiants et des relations** — distinguer `a→b` de `b→a`, tenir un
chemin de trois sauts, ne pas confondre deux voisins. La perplexité moyenne, elle, est dominée par la
fluidité, qui survit le plus longtemps à la quantification.

Usage :
    python3 scripts/juge-structurel.py <port> [--n 12] [--seed 20260920] [--familles degre,cycles,attainabilite,redondance]

Le modèle est interrogé par l'API OpenAI-compatible de llama-server :
    llama-server -m <modele> --jinja -c 4096 -ngl 0 --no-warmup

Sortie : un taux de réussite PAR FAMILLE (et non un score agrégé, qui masquerait quelle capacité
casse). Codes de retour : 0 si tout est au-dessus du seuil, 1 sinon, 2 si le serveur ne répond pas.
"""

from __future__ import annotations

import argparse
import json
import random
import sys
import time
import urllib.request
from collections import defaultdict

# ── Le graphe : engendré, typé, avec des identifiants qui se ressemblent ────────────────────────
#
# Les identifiants sont volontairement PROCHES (`fn_0421` / `fn_0412`) : c'est ce que le chat obrain
# rencontre vraiment (des noms de fonctions du noyau), et c'est là que la quantification fait mal.

TYPES = ("APPELLE", "CONTIENT", "IMPORTE")

# rempli par --selection : "brute" (tout le graphe) ou "oracle" (seules les lignes du nœud)
SELECTION = ["brute"]

# rempli par --bornage : "exact" (exactement N) ou "exclu" (N sauts mais PAS N-1)
BORN = ["exact"]


def gen_graphe(rng: random.Random, n_noeuds: int, n_aretes: int):
    """Un graphe orienté typé. On évite les doublons d'arêtes, on garde la possibilité de cycles."""
    noeuds = [f"fn_{1000 + i}" for i in range(n_noeuds)]
    aretes = set()
    while len(aretes) < n_aretes:
        a = rng.choice(noeuds)
        b = rng.choice(noeuds)
        if a == b:
            continue
        aretes.add((a, rng.choice(TYPES), b))
    return noeuds, sorted(aretes)


def relabelliser(texte: str, lab: dict) -> str:
    """Remplace les identifiants par leurs libellés (indices). Les identifiants ont tous la même
    longueur, donc l'ordre de remplacement est sans conséquence — mais on trie par longueur
    décroissante par prudence si la forme des noms change un jour."""
    for k in sorted(lab, key=len, reverse=True):
        texte = texte.replace(k, lab[k])
    return texte


def rendre(noeuds, aretes) -> str:
    """Le sous-graphe tel qu'il entrerait dans une invite : ordre CANONIQUE (trié), un fait par ligne."""
    lignes = [f"{a} --{t}--> {b}" for (a, t, b) in sorted(aretes)]
    return "\n".join(lignes)


# ── Les quatre familles : question + oracle, toutes deux calculées ──────────────────────────────


def q_degre(noeuds, aretes, rng):
    """Quel nœud a le plus d'arêtes ENTRANTES de type X ? (oracle : comptage)"""
    t = rng.choice(TYPES)
    cnt = defaultdict(int)
    for (_, tt, b) in aretes:
        if tt == t:
            cnt[b] += 1
    if not cnt:
        return None
    best = max(cnt.values())
    gagnants = sorted(k for k, v in cnt.items() if v == best)
    # deux garde-fous : une réponse trop banale (tout le monde à 1) n'apprend rien, et un ex æquo
    # rendrait la question ambiguë — donc injuste envers le modèle
    if best < 2 or len(gagnants) != 1:
        return None
    return (f"Dans ce graphe, quel nœud a le plus d'arêtes ENTRANTES de type {t} ? "
            f"Réponds par le nom du nœud seul, rien d'autre.",
            gagnants[0])


def q_atteignabilite(noeuds, aretes, rng):
    """Depuis X, quels nœuds sont atteignables en exactement 2 arêtes (tous types) ? (oracle : BFS)"""
    succ = defaultdict(set)
    for (a, _, b) in aretes:
        succ[a].add(b)
    depart = rng.choice(noeuds)
    if not succ[depart]:
        return None
    niveau2 = set()
    for m in succ[depart]:
        niveau2 |= succ[m]
    niveau2.discard(depart)
    if not (1 <= len(niveau2) <= 6):
        return None
    attendus = sorted(niveau2)
    # MÊME ensemble attendu, DEUX formulations. C'est le seul changement : « exactement 2 » laisse
    # l'exclusion implicite ; « 2 mais pas 1 » la dit. Si le score bascule, le défaut n'est pas la
    # capacité mais la CONSIGNE — et la correction est dans le contrat d'invite, pas dans les bits.
    if BORN[0] == "exclu":
        niveau1 = sorted(x for x in succ[depart])
        question = (f"Dans ce graphe, quels nœuds sont atteignables depuis {depart} en DEUX arêtes "
                    f"MAIS PAS en une seule ? (ceux atteignables en 1 arête : {', '.join(niveau1)}) "
                    f"Réponds par les noms séparés par des virgules, rien d'autre.")
    else:
        question = (f"Dans ce graphe, quels nœuds sont atteignables depuis {depart} en EXACTEMENT "
                    f"2 arêtes ? Réponds par les noms séparés par des virgules, rien d'autre.")
    return (question, attendus)


def q_cycles(noeuds, aretes, rng):
    """Parmi ces arêtes, lesquelles appartiennent à un cycle ? (oracle : SCC de taille > 1)"""
    succ = defaultdict(list)
    pred = defaultdict(list)
    for (a, _, b) in aretes:
        succ[a].append(b)
        pred[b].append(a)

    # Kosaraju, version directe : composantes fortement connexes
    vus, ordre = set(), []

    def dfs(u):
        vus.add(u)
        for v in succ[u]:
            if v not in vus:
                dfs(v)
        ordre.append(u)

    for u in noeuds:
        if u not in vus:
            dfs(u)

    comp, vus2 = {}, set()

    def rdfs(u, c):
        vus2.add(u)
        comp[u] = c
        for v in pred[u]:
            if v not in vus2:
                rdfs(v, c)

    for u in reversed(ordre):
        if u not in vus2:
            rdfs(u, u)

    taille = defaultdict(int)
    for u in noeuds:
        taille[comp[u]] += 1

    dans_cycle = sorted({n for n in noeuds if taille[comp[n]] > 1})
    if not (1 <= len(dans_cycle) <= 6):
        return None
    # Phrase RÉÉCRITE : « directement ou indirectement » invitait à inclure les nœuds qui MÈNENT à un
    # cycle, alors que l'oracle ne rend que ceux qui en font partie. La référence échouait 4/4 sur
    # l'ancienne formulation — un juge que la référence rate n'est pas un juge.
    return (f"Dans ce graphe, quels nœuds appartiennent à un cycle ? Un nœud appartient à un "
            f"cycle s'il existe un chemin non vide de lui-même vers lui-même. "
            f"Réponds par les noms séparés par des virgules, rien d'autre.",
            dans_cycle)


def q_redondance(noeuds, aretes, rng):
    """CONSTRUITE, pas espérée : dans un graphe aléatoire, un chemin de deux arêtes du même type est
    rare — le premier run n'a engendré AUCUNE question en 72 tentatives (0/0). On choisit donc un
    triangle a→m→b, on retire l'arête directe a→b si elle existe, et on la pose : la redondance est
    garantie par construction, l'oracle n'a plus qu'à la constater."""
    if len(noeuds) >= 3:
        t = rng.choice(TYPES)
        a, m, b = rng.sample(noeuds, 3)
        aretes = [e for e in aretes if e != (a, t, b)]           # pas de doublon
        aretes = list(aretes) + [(a, t, m), (m, t, b), (a, t, b)]
        return (f"Dans ce graphe, existe-t-il un chemin de DEUX arêtes ou plus, toutes de type {t}, "
                f"allant de {a} à {b} ? Réponds par OUI ou NON, rien d'autre.",
                "OUI", aretes)
    return None


def q_redondance_ancien(noeuds, aretes, rng):
    """Cette arête directe est-elle impliquée par un chemin d'au moins 2 arêtes du même type ?
    (oracle : fermeture transitive de type, puis recherche d'un chemin de longueur ≥ 2)"""
    par_type = defaultdict(lambda: defaultdict(set))
    for (a, t, b) in aretes:
        par_type[t][a].add(b)
    cands = []
    for (a, t, b) in aretes:
        # chemin a → ... → b de longueur ≥ 2, tous les maillons du même type
        for m in par_type[t].get(a, ()):  # premier maillon
            if m == b:
                continue
            if b in par_type[t].get(m, ()):  # second maillon
                cands.append((a, t, b))
                break
    if not cands:
        return None
    rng.shuffle(cands)
    a, t, b = cands[0]
    # Phrase RÉÉCRITE : « impliquée » est un mot de logique, pas une consigne. On demande
    # explicitement l'existence d'un chemin, ce qui est exactement ce que calcule l'oracle.
    return (f"Dans ce graphe, existe-t-il un chemin de DEUX arêtes ou plus, toutes de type {t}, "
            f"allant de {a} à {b} ? Réponds par OUI ou NON, rien d'autre.",
            "OUI")


def q_voisins(noeuds, aretes, rng):
    """LIRE un voisinage — la seule question dont on sait qu'elle échoue en vue « brute ».
    Le mode de sélection est porté par `--selection` : le graphe entier, ou les SEULES lignes dont
    le nœud est la source. C'est l'opération que le substrat fait nativement (une requête), et le
    test dit si la capacité est PERDUE par la quantification ou seulement DÉPLACÉE."""
    succ = defaultdict(set)
    for (a, _, b) in aretes:
        succ[a].add(b)
    cands = [n for n in noeuds if 1 <= len(succ[n]) <= 4]
    if not cands:
        return None
    x = rng.choice(cands)
    attendus = sorted(succ[x])
    if SELECTION[0] == "oracle":
        vues = [e for e in aretes if e[0] == x]     # la sélection est faite POUR le modèle
    else:
        vues = aretes                                # le modèle doit la faire
    return (f"Quels sont les nœuds atteignables depuis {x} en UNE SEULE arête ? "
            f"Réponds par les noms séparés par des virgules, rien d'autre.",
            attendus, vues)


FAMILLES = {
    "degre": q_degre,
    "atteignabilite": q_atteignabilite,
    "cycles": q_cycles,
    "redondance": q_redondance,
    "voisins": q_voisins,
}


# ── Interrogation et notation ───────────────────────────────────────────────────────────────────


def demander(port: int, invite: str) -> tuple[str, float]:
    corps = json.dumps({
        "messages": [{"role": "user", "content": invite}],
        "n_predict": 64,
        "temperature": 0.0,
        "stream": False,
        # le non-raisonnement est le mode où ces modèles répondent le plus vite ; et pour ces
        # questions la réponse est un identifiant, pas un raisonnement
        "chat_template_kwargs": {"enable_thinking": False},
    }).encode()
    req = urllib.request.Request(f"http://127.0.0.1:{port}/v1/chat/completions",
                                 data=corps, headers={"Content-Type": "application/json"})
    t0 = time.time()
    r = json.load(urllib.request.urlopen(req, timeout=600))
    return (r["choices"][0]["message"].get("content") or "").strip(), time.time() - t0


def noter(reponse: str, attendu) -> bool:
    """Comparaison normale : on accepte la casse, les espaces et la ponctuation autour."""
    norm = lambda s: s.replace(" ", "").replace(".", "").replace("`", "").strip().lower()
    if isinstance(attendu, list):
        got = {x.strip() for x in re_sub_split(reponse) if x.strip()}
        return got == set(attendu)
    return norm(reponse).startswith(norm(attendu))


def re_sub_split(s: str):
    for sep in (",", ";", "\n", " et "):
        s = s.replace(sep, ",")
    return [x for x in s.split(",") if x]


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("port", type=int)
    ap.add_argument("--n", type=int, default=12, help="questions par famille")
    ap.add_argument("--seed", type=int, default=20260920)
    ap.add_argument("--familles", default=",".join(FAMILLES))
    ap.add_argument("--seuil", type=float, default=0.5)
    ap.add_argument("--bornage", default="exact", choices=("exact", "exclu"),
                    help="exact : « exactement 2 sauts » (l'exclusion est implicite) ; exclu : "
                         "« 2 sauts mais PAS 1 » (l'exclusion est dite). Même ensemble attendu.")
    ap.add_argument("--selection", default="brute", choices=("brute", "oracle"),
                    help="brute : le modèle doit SÉLECTIONNER les lignes ; oracle : elles sont déjà "
                         "sélectionnées (ce que fait une requête du substrat).")
    ap.add_argument("--gestes", default="1", choices=("1", "2"),
                    help="1 : la question composee (2 sauts d'un coup) ; 2 : UN SAUT A LA FOIS, la "
                         "reponse du premier appel nourrissant le second. Teste la restitution de la "
                         "composition par le substrat, a ZERO bit de modele.")
    ap.add_argument("--forme", default="nom", choices=("nom", "indice"),
                    help="nom : les identifiants (fn_1003) ; indice : des numéros (1..N). "
                         "Sert à isoler l'échec de COPIE EXACTE d'un identifiant de l'échec de "
                         "RAISONNEMENT sur la structure.")
    a = ap.parse_args()

    try:
        urllib.request.urlopen(f"http://127.0.0.1:{a.port}/health", timeout=5)
    except Exception as e:
        print(f"  serveur injoignable sur :{a.port} — {e}")
        return 2

    SELECTION[0] = a.selection
    BORN[0] = a.bornage
    rng = random.Random(a.seed)
    resultats = {}
    for nom in a.familles.split(","):
        fabrique = FAMILLES.get(nom.strip())
        if fabrique is None:
            continue
        ok = essais = 0
        exemples = []
        tentatives = 0
        if nom.strip() == "atteignabilite" and a.gestes == "2":
            while essais < a.n and tentatives < a.n * 12:
                tentatives += 1
                n_noeuds = rng.randint(7, 12)
                noeuds, aretes = gen_graphe(rng, n_noeuds, rng.randint(8, 14))
                succ = defaultdict(set)
                for (x, _, y) in aretes:
                    succ[x].add(y)
                depart = rng.choice(noeuds)
                h1 = succ[depart]
                if not h1:
                    continue
                h2 = set()
                for m in h1:
                    h2 |= succ[m]
                h2.discard(depart)
                if not (1 <= len(h2) <= 6):
                    continue
                graphe = rendre(noeuds, aretes)
                # GESTE 1 : un seul saut
                i1 = f"Voici un graphe orienté typé :\n{graphe}\n\nQuels sont les nœuds atteignables " \
                     f"depuis {depart} en UNE SEULE arête ? Réponds par les noms séparés par des " \
                     f"virgules, rien d'autre."
                if a.selection == "oracle":
                    # ── LE GRAPHE FAIT LA COMPOSITION, LE MODÈLE LIT UN SAUT À LA FOIS ──────
                    # Le premier saut est fourni par le substrat (ce que rendrait un appel
                    # d'outil), puis on pose LA QUESTION VALIDÉE — mot pour mot celle de la
                    # famille `voisins`, celle qui donne 75-100 % — UNE FOIS PAR INTERMÉDIAIRE.
                    # L'union est faite par nous, pas par le modèle. Le bras précédent posait une
                    # question inédite (« depuis : v1, v2 ») et son 0/4 ne mesurait que ma
                    # formulation : c'est ce défaut-là qu'on corrige ici.
                    voisins = sorted(h1)
                    got = set()
                    for v in voisins:
                        iv = (f"Voici un graphe orienté typé :\n{graphe}\n\nQuels sont les nœuds "
                              f"atteignables depuis {v} en UNE SEULE arête ? Réponds par les noms "
                              f"séparés par des virgules, rien d'autre.")
                        rv, _ = demander(a.port, iv)
                        got |= {x for x in re_sub_split(rv) if x in set(noeuds)}
                    got.discard(depart)
                    bon = got == h2
                    essais += 1
                    ok += bon
                    if len(exemples) < 2 and not bon:
                        exemples.append((f"X={depart} | intermédiaires {voisins} | 1 saut chacun",
                                         sorted(h2), ", ".join(sorted(got))))
                    continue
                r1, _ = demander(a.port, i1)
                voisins = [x for x in re_sub_split(r1) if x in set(noeuds)]
                # GESTE 2 : un saut depuis CHACUN des voisins trouvés
                i2 = f"Voici un graphe orienté typé :\n{graphe}\n\nQuels sont les nœuds atteignables " \
                     f"en UNE SEULE arête depuis : {', '.join(voisins) if voisins else '(aucun)'} ? " \
                     f"Réponds par l'union des noms, séparés par des virgules, rien d'autre."
                r2, _ = demander(a.port, i2)
                got = {x for x in re_sub_split(r2) if x in set(noeuds)}
                got.discard(depart)
                bon = got == h2
                essais += 1
                ok += bon
                if len(exemples) < 2 and not bon:
                    exemples.append((f"{depart} → 1 saut {sorted(h1)} → 2 sauts",
                                     sorted(h2), ", ".join(sorted(got))))
            resultats["atteignabilite (2 gestes)"] = (ok, essais, exemples)
            continue
        while essais < a.n and tentatives < a.n * 12:
            tentatives += 1
            # MESURÉ : le préfill domine le coût (4,4 tok/s sur CPU, ~98 s pour 430 jetons).
            # Des graphes de 8-14 arêtes tiennent en ~150 jetons, soit ~34 s par question.
            n_noeuds = rng.randint(7, 12)
            noeuds, aretes = gen_graphe(rng, n_noeuds, rng.randint(8, 14))
            q = fabrique(noeuds, aretes, rng)
            if q is None:
                continue
            if len(q) == 3:          # la famille a construit son propre graphe (redondance)
                question, attendu, aretes = q
            else:
                question, attendu = q
            if a.forme == "indice":
                # les nœuds deviennent des numéros : l'identifiant disparaît de l'entrée ET de la
                # sortie. C'est le seul changement — si le score remonte, le défaut n'était pas la
                # structure mais la manipulation du NOM.
                lab = {n: str(i + 1) for i, n in enumerate(noeuds)}
                question = relabelliser(question, lab)
                attendu = [relabelliser(x, lab) for x in attendu] if isinstance(attendu, list) \
                          else relabelliser(attendu, lab)
                graphe = relabelliser(rendre(noeuds, aretes), lab)
            else:
                graphe = rendre(noeuds, aretes)
            invite = (f"Voici un graphe orienté typé, une arête par ligne :\n{graphe}\n\n{question}")
            try:
                reponse, dt = demander(a.port, invite)
            except Exception as e:
                print(f"  erreur d'appel : {e}")
                break
            bon = noter(reponse, attendu)
            essais += 1
            ok += bon
            if len(exemples) < 2 and not bon:
                exemples.append((question[:90], attendu, reponse[:60]))
        resultats[nom.strip()] = (ok, essais, exemples)

    print(f"\n=== JUGE STRUCTUREL — port {a.port}, {a.n} questions/famille, graine {a.seed}, "
          f"forme={a.forme} ===")
    tous_bons = True
    for nom, (ok, essais, exemples) in resultats.items():
        taux = ok / essais if essais else 0.0
        tous_bons &= taux >= a.seuil
        print(f"  {nom:16s} {ok:3d}/{essais:3d} = {100*taux:5.1f} %")
        for q, attendu, rep in exemples:
            print(f"      ✗ {q}…\n        attendu {attendu} | obtenu {rep!r}")
    return 0 if tous_bons else 1


if __name__ == "__main__":
    sys.exit(main())
