#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
[obrain APPEND] MESURE BOUT EN BOUT DE L'IMPORT INCREMENTAL D'ETAT.

CE QU'ON MESURE, ET POURQUOI CETTE MESURE EXISTE :
  - baseline (ce que le code faisait AVANT) : ajouter la page k a un contexte
    exigeait un REMPLACEMENT TOTAL, donc l'import de tout le contexte
    (P jetons = P x 65 556 octets en f16 sur le modele de production) ;
  - avec l'ajout incremental : l'import de la SEULE page k (M jetons).
  On mesure les octets REELLEMENT importes et le temps d'import, et on verifie
  par une QUESTION dont la reponse est dans la page 0 (loin derriere) que la
  page 0 est TOUJOURS LA apres l'ajout de la page 1.

Usage : python3 mesure-append-e2e.py <port> [n_page0] [n_page1]
"""

import json
import sys
import time
import urllib.request

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 9999
CIBLE_P0 = int(sys.argv[2]) if len(sys.argv) > 2 else 2048
CIBLE_P1 = int(sys.argv[3]) if len(sys.argv) > 3 else 512
BASE = "http://127.0.0.1:%d" % PORT

CODE_SECRET = "ROUGE-7"


def post(chemin, corps, timeout=1800):
    donnees = json.dumps(corps).encode()
    req = urllib.request.Request(BASE + chemin, data=donnees,
                                headers={"Content-Type": "application/json"})
    t0 = time.time()
    with urllib.request.urlopen(req, timeout=timeout) as r:
        brut = r.read()
    return json.loads(brut), (time.time() - t0) * 1000.0


def jetons(texte):
    r, _ = post("/tokenize", {"content": texte})
    return r["tokens"]


def texte_de(n_jetons, graine):
    """Texte de remplissage dont la tokenisation atteint ~n_jetons."""
    # Une phrase repetee, variee par la graine pour que les pages diffèrent.
    base = ("Le dossier %d contient des notes de travail sans importance "
            "particuliere pour la question posee a la fin. " % graine)
    n = jetons(base)
    k = max(1, n_jetons // len(n))
    return base * k


def extraire_page(fait, pos_start=None, prerequis=None, etiquette=""):
    corps = {"fact_text": fait, "extract_only": True}
    if pos_start is not None:
        corps["pos_start"] = pos_start
    if prerequis:
        corps["prerequis_refs"] = prerequis
    r, ms = post("/jspace/inject_knowledge", corps)
    if r.get("error"):
        raise SystemExit("extraction %s : %s" % (etiquette, json.dumps(r)[:800]))
    ref = r["text"].split(":", 1)[1]
    print("  [extract] %-8s ref=%s page=[%s, %s] recr_from=%s jetons=%s octets=%s (%.0f ms)"
          % (etiquette, ref, r.get("page_pos_min"), r.get("page_pos_max"),
             r.get("recr_from"), r.get("fact_tokens"), r.get("blob_bytes"), ms))
    return ref, r


def ajouter(ref, pmin, pmax, etiquette="", reset=False):
    corps = {"state_append": True, "state_ref": ref, "pos_min": pmin, "pos_max": pmax}
    if reset:
        # Remise a zero EXPLICITE : la sequence reservee est partagee par toutes
        # les requetes J-space (l'extraction y laisse le prefixe qu'elle decode).
        corps["state_reset"] = True
    r, ms = post("/jspace/inject_knowledge", corps)
    if r.get("error"):
        print("  [append]  %-8s REFUSE : %s" % (etiquette, r["error"]))
        return None, r
    print("  [append]  %-8s ref=%s page=[%s, %s] IMPORTES=%s octets en %.1f ms -> seq=[%s, %s] cellules=%s"
          % (etiquette, ref, r.get("page_pos_min"), r.get("page_pos_max"),
             r.get("import_bytes"), r.get("ms_state_import"), r.get("seq_pos_min"),
             r.get("seq_pos_max"), r.get("seq_cells")))
    return r, r


def demander(prerequis, question, n_gen=16):
    r, ms = post("/jspace/inject_knowledge",
                 {"prerequis_refs": prerequis, "prompt": question,
                  "n_generate": n_gen, "greedy": True})
    if r.get("error"):
        raise SystemExit("question : %s" % json.dumps(r)[:800])
    return r.get("text", ""), r, ms


def main():
    print("=== MESURE BOUT EN BOUT : import incremental de page (port %d) ===" % PORT)
    print("date : %s" % time.strftime("%Y-%m-%d %H:%M:%S"))

    # 0. le serveur repond ?
    with urllib.request.urlopen(BASE + "/health", timeout=60) as r:
        print("sante :", r.read().decode().strip())

    # ── 1. FABRICATION DES PAGES ────────────────────────────────────────────
    fait0 = ("Fait a retenir : le code secret est %s. " % CODE_SECRET) + texte_de(CIBLE_P0, 1)
    fait1 = texte_de(CIBLE_P1, 2)
    n0 = len(jetons(fait0))
    n1 = len(jetons(fait1))
    print("\n-- pages : page 0 ~%d jetons (le code secret y est), page 1 ~%d jetons" % (n0, n1))

    t0 = time.time()
    ref0, r0 = extraire_page(fait0, etiquette="page0")
    p0max = r0["page_pos_max"]
    ref1, r1 = extraire_page(fait1, pos_start=p0max + 1, prerequis=[ref0], etiquette="page1")
    p1max = r1["page_pos_max"]

    fait_full = fait0 + " " + fait1
    reffull, rfull = extraire_page(fait_full, etiquette="complet")
    print("  (fabrication des 3 etats : %.0f s)" % (time.time() - t0))

    total = p1max + 1

    # ── 2. AJOUT INCREMENTAL (la capacite nouvelle) ─────────────────────────
    print("\n-- AJOUT INCREMENTAL : page 0 puis page 1")
    mesures = []
    for essai in range(3):
        a0, _ = ajouter(ref0, r0["page_pos_min"], p0max, etiquette="page0#%d" % essai, reset=True)
        a1, _ = ajouter(ref1, r1["page_pos_min"], p1max, etiquette="page1#%d" % essai)
        if a0 and a1:
            mesures.append((a0["import_bytes"], a0["ms_state_import"],
                            a1["import_bytes"], a1["ms_state_import"],
                            a1["seq_pos_min"], a1["seq_pos_max"]))

    # ── 3. BASELINE : REMPLACEMENT TOTAL (le comportement d'avant) ──────────
    print("\n-- BASELINE (remplacement total) : import de TOUT le contexte [0, %d]" % p1max)
    base = []
    for essai in range(3):
        b, _ = ajouter(reffull, rfull["page_pos_min"], rfull["page_pos_max"],
                       etiquette="complet#%d" % essai, reset=True)
        if b:
            base.append((b["import_bytes"], b["ms_state_import"], b["seq_pos_max"]))

    # ── 4. LA PAGE 0 EST-ELLE TOUJOURS LA ? (preuve semantique) ─────────────
    print("\n-- QUESTION dont la reponse est dans la PAGE 0 (apres ajout de la page 1)")
    question = "Quel est le code secret ? Reponds uniquement par le code."
    rep_assemble, _, ms_asm = demander([ref0, ref1], question)
    rep_complet, _, ms_full = demander([reffull], question)
    print("  contexte ASSEMBLE (%d pages ajoutees) : %r (%.0f ms)"
          % (2, rep_assemble.strip()[:160], ms_asm))
    print("  contexte COMPLET (remplacement)      : %r (%.0f ms)"
          % (rep_complet.strip()[:160], ms_full))

    # ── 5. SYNTHESE CHIFFREE ────────────────────────────────────────────────
    print("\n=== SYNTHESE ===")
    print("jetons du contexte final : %d (page0 %d + page1 %d)"
          % (total, p0max + 1, p1max - p0max))
    if mesures:
        oct_p0 = mesures[0][0]
        oct_p1 = mesures[0][2]
        ms_p0 = sum(m[1] for m in mesures) / len(mesures)
        ms_p1 = sum(m[3] for m in mesures) / len(mesures)
        print("AJOUT    : page 0 = %d octets (%.1f ms), page 1 = %d octets (%.1f ms)"
              % (oct_p0, ms_p0, oct_p1, ms_p1))
        print("AJOUT    : cout d'un ajout de page 1 (delta)          = %d octets, %.1f ms" % (oct_p1, ms_p1))
        print("AJOUT    : positions finales %s..%s (cellules %s)"
              % (mesures[0][4], mesures[0][5], mesures[0][5] - mesures[0][4] + 1))
    if base:
        oct_b = base[0][0]
        ms_b = sum(m[1] for m in base) / len(base)
        print("BASELINE : import du contexte ENTIER (remplacement)   = %d octets, %.1f ms" % (oct_b, ms_b))
    if mesures and base:
        oct_p1 = mesures[0][2]
        ms_p1 = sum(m[3] for m in mesures) / len(mesures)
        ms_b = sum(m[1] for m in base) / len(base)
        oct_b = base[0][0]
        print("GAIN     : octets x%.2f  (%.1f Mo -> %.1f Mo)"
              % (oct_b / oct_p1, oct_b / 1048576.0, oct_p1 / 1048576.0))
        print("GAIN     : temps  x%.2f  (%.1f ms -> %.1f ms)"
              % (ms_b / ms_p1 if ms_p1 > 0 else 0, ms_b, ms_p1))
    ok = CODE_SECRET in rep_assemble
    print("PREUVE   : la page 0 a SURVECU a l'ajout de la page 1 : %s (%r)"
          % ("OUI" if ok else "NON", rep_assemble.strip()[:80]))
    print("date fin : %s" % time.strftime("%Y-%m-%d %H:%M:%S"))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
