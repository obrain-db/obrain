#!/usr/bin/env python3
"""qa_obrain_source.py — question→réponse branché sur obrain, pour un store STRUCTUREL.

Pourquoi ce script existe (MESURÉ, 2026-09-21) : `qa_obrain.py` ne peut pas fonctionner sur le store
disponible, et la cause n'est ni l'IAM ni les données perdues. Elle est plus simple et plus
intéressante — c'est un DÉSACCORD DE SCHÉMA :

    qa_obrain.py::pick_facts  lit  `RETURN n.qualified_name, n.text`
    or   MATCH (n:Function) WHERE n.text IS NOT NULL RETURN count(n)  →  0

**Aucun** des 105 546 nœuds `Function` du store ne porte `text` — ni `code`, `body`, `source`,
`content` ni `doc`. Ce store est **structurel** : il porte un POINTEUR vers la source
(`file_path` + `start_line` + `end_line`), pas la source. C'est un choix d'indexation, pas une
avarie : les zones de propriétés font 2,40 Go et 105 546 fonctions sont là.

Le remède est donc une RÉSOLUTION, pas une réparation de base : on lit la tranche de fichier que le
nœud désigne. Vérifié : `/tmp/linux_src/drivers/net/` + `file_path` résout, et les plages de lignes
tombent dans le fichier.

Ce que ce script NE fait PAS : il ne modifie rien dans le graphe, il ne écrit pas, il n'arme ni ne
désarme aucune garde. Il LIT avec l'identité fournie par `OBRAIN_IAM_TOKEN` — la voie documentée
(docs/IAM-BRANCHE-2026-09-13.md §3).

Usage :
    export OBRAIN_IAM_TOKEN=$(... copié depuis ~/.config/obrain-harness/iam-token.env)
    python3 qa_obrain_source.py --dry "<question>"          # côté graphe seul, aucun modèle
    python3 qa_obrain_source.py --port 8099 "<question>"    # flux complet
"""

from __future__ import annotations
import json, os, re, sys, time, urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(os.path.dirname(HERE), "obrain-adapter"))
from adapter import ObrainBridgeRO, metrics_from_inject, post_json

OBRAIN = "/home/triviere/projects/banc-obrain/target/release/obrain"
# le store SAIN. L'autre copie (/tmp/kv-linux-driversnet-neuf.obrain) a perdu ses propriétés :
# `label_only=333263` — toutes ses étiquettes sont des cosses. Vérifié par les fichiers :
# sa zone de propriétés fait 16 Ko là où celle-ci en fait 2,40 Go.
STORE = os.environ.get("OBRAIN_STORE_SRC",
                       "/tmp/kv-linux-driversnet-v2.obrain.bak-avant-v3-20260914")
SRC_ROOT = os.environ.get("LINUX_SRC", "/tmp/linux_src")
# préfixe déterminé EMPIRIQUEMENT, pas supposé : les 8 premiers file_path tirés du store
# résolvent tous sous cette racine
SRC_PREFIXES = ("drivers/net/", "drivers/", "")
N_GEN = int(os.environ.get("OBRAIN_N_GEN", "90"))
# MESURÉ le 2026-09-21 (docs/REPRISE-TERNARISATION-2026-09-20.md, annexe A7) : à 90 jetons, les CINQ
# cibles essayées rendent une réponse TRONQUÉE (255-379 caractères, toutes coupées en pleine phrase),
# et deux d'entre elles partent en raisonnement — que /jspace/inject_knowledge ne ferme pas — et
# n'écrivent alors AUCUNE réponse. Le budget est donc réglable sans toucher au code :
#     OBRAIN_N_GEN=500 python3 qa_obrain_source.py --port 8122 "<question>"
# Un score de qualité obtenu à 90 jetons mesure le budget, pas la lecture.
MAX_FACT_CAR = 1400


def resoudre_source(file_path: str, a, z) -> str | None:
    """Rend la tranche de source que le nœud DÉSIGNE. C'est tout le remède : lire le pointeur."""
    if not file_path:
        return None
    for pref in SRC_PREFIXES:
        p = os.path.join(SRC_ROOT, pref, file_path)
        if os.path.exists(p):
            try:
                with open(p, "r", encoding="utf-8", errors="replace") as f:
                    lignes = f.readlines()
            except OSError:
                continue
            if a and z and 1 <= int(a) <= len(lignes):
                fin = min(int(z), len(lignes)) if z else int(a)
                return "".join(lignes[int(a) - 1:fin])
            return "".join(lignes[:40])
    return None


MOTS_VIDES = {"que", "fait", "fonction", "explique", "linux", "kernel", "driver", "comment",
              "quelle", "quel", "role", "rôle", "dans", "avec", "pour", "the", "what"}


def pick_facts(question: str, bridge: ObrainBridgeRO, k: int = 2):
    """Sélectionne ≤k fonctions par mots-clés, puis RÉSOUT leur source depuis le pointeur."""
    toks = re.findall(r"[A-Za-z_][A-Za-z0-9_]{2,}", question)
    faits, vus = [], set()
    for t in toks:
        low = t.lower()
        if low in MOTS_VIDES:
            continue
        if not re.search(r"(_|probe|open|close|init|poll|read|write|xmit|stop|start)", low):
            continue
        try:
            r = bridge.query(
                f"MATCH (n:Function) WHERE n.qualified_name CONTAINS '{t}' "
                f"RETURN n.qualified_name, n.file_path, n.start_line, n.end_line LIMIT 1")
        except Exception as e:
            print(f"  [graine] requête refusée pour {t!r} : {str(e)[:120]}")
            continue
        if not (r and r[0]):
            continue
        qn = str(r[0][0])
        if qn in vus:
            continue
        src = resoudre_source(*r[0][1:4])
        if not src:
            print(f"  [graine] {qn[:60]} : source introuvable (file_path={r[0][1]})")
            continue
        vus.add(qn)
        faits.append(f"// {qn}\n// {r[0][1]}:{r[0][2]}-{r[0][3]}\n{src}"[:MAX_FACT_CAR])
        if len(faits) >= k:
            break
    return faits


def repli(bridge: ObrainBridgeRO):
    """Si les mots-clés ne donnent rien : une fonction e1000 qui EXISTE vraiment.
    (L'ancien repli visait `…intel.e1000.e1000_probe`, MESURÉ absent du store.)"""
    try:
        r = bridge.query("MATCH (n:Function) WHERE n.qualified_name CONTAINS 'e1000' "
                         "RETURN n.qualified_name, n.file_path, n.start_line, n.end_line LIMIT 1")
        if r and r[0]:
            src = resoudre_source(*r[0][1:4])
            if src:
                return [f"// {r[0][0]}\n// {r[0][1]}:{r[0][2]}-{r[0][3]}\n{src}"[:MAX_FACT_CAR]]
    except Exception as e:
        print(f"  [repli] {str(e)[:120]}")
    return []


def main() -> int:
    args = [a for a in sys.argv[1:]]
    dry = "--dry" in args
    if dry:
        args.remove("--dry")
    port = 8099
    if "--port" in args:
        i = args.index("--port")
        port = int(args[i + 1])
        del args[i:i + 2]
    if not args:
        print(__doc__)
        return 2
    question = args[0]

    if not os.environ.get("OBRAIN_IAM_TOKEN"):
        print("  ✗ OBRAIN_IAM_TOKEN absente : l'IAM est armé, la lecture serait refusée en 401.")
        print("    (ne PAS désarmer la garde : présenter une identité est la voie documentée)")
        return 1

    b = ObrainBridgeRO(OBRAIN, STORE)
    faits = pick_facts(question, b)
    if not faits:
        print("  aucune graine par mots-clés — repli sur une fonction e1000 connue")
        faits = repli(b)
    b.close()
    if not faits:
        print("  ✗ aucun fait : rien à injecter (le modèle serait interrogé sur du vide)")
        return 1
    fact = "\n\n".join(faits)
    print(f"[obrain] store : {STORE}")
    print(f"[obrain] {len(faits)} fait(s), {len(fact)} caractères de source RÉSOLUE :")
    for f in faits:
        print("    " + f.split("\n")[0][:110])
    print(f"[obrain] QUESTION : {question}\n")

    if dry:
        print("  (--dry : arrêt ici, aucun modèle requis)")
        print("  aperçu du fait injecté :")
        print("    " + fact[:600].replace("\n", "\n    "))
        return 0

    BASE = f"http://127.0.0.1:{port}/jspace/inject_knowledge"
    try:
        with urllib.request.urlopen(BASE.replace("/jspace/inject_knowledge", "/health"), timeout=3) as rr:
            if json.loads(rr.read().decode()).get("status") != "ok":
                print(f"  serveur non prêt sur :{port}"); return 1
    except Exception:
        print(f"  serveur injoignable sur :{port} — lancer un llama-server avec les points /jspace")
        return 1

    t0 = time.time()
    ex = post_json(BASE, {"fact_text": fact, "extract_only": True}, timeout=600)
    extract_s = time.time() - t0
    ref = str(ex.get("text", "")).replace("kv_state:", "")
    print(f"[obrain] extract {extract_s:.1f}s | blob {ex.get('blob_bytes', 0)/2**20:.0f} MiB | ref {ref[:10]}…")

    t0 = time.time()
    r = post_json(BASE, {"state_ref": ref, "prompt": question, "n_generate": N_GEN, "greedy": True},
                  timeout=600)
    wall = time.time() - t0
    m = metrics_from_inject(r)
    print("RÉPONSE :")
    print((str(r.get("text") or "").strip() or "(vide)")[:1600])
    print(f"\n[métriques wall] TFPT≈{m['tfpt_ms']} ms | wall {wall:.1f}s | "
          f"tok/s≈{N_GEN/max(wall-2, 0.1):.2f} | state_load {m['fact_or_state_load_ms']:.0f} ms")
    return 0


if __name__ == "__main__":
    sys.exit(main())
