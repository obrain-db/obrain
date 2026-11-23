#!/usr/bin/env python3
"""identifier-candidat.py — dire QUELLE politique a produit un GGUF, sans le charger en mémoire.

Pourquoi : le balayage écrivait ses candidats dans /tmp et la note de reprise dit « aucun candidat
attn_qkv/ssm_out n'est sauvegardé ». Deux fichiers de 11 Go dorment pourtant dans /tmp. Avant de
relancer 4 min de quantification pour les reproduire, on lit ce qu'ils SONT.

La taille ne suffit pas à identifier une politique (deux politiques différentes peuvent tomber au
même octet près) : on compte les TYPES par famille de tenseurs, ce qui est la signature exacte du
sélecteur `obrain_tensor_quant_type` de src/llama-quant.cpp.

Lecture SANS dépendance : on ne lit que l'en-tête GGUF (magic, métadonnées, table des tenseurs) —
soit quelques centaines de kilo-octets, jamais les 11 Go de poids. Pas de numpy requis.

Usage : python3 scripts/identifier-candidat.py <fichier.gguf> [<fichier.gguf> ...]
"""

from __future__ import annotations

import os
import re
import struct
import sys
from collections import Counter, defaultdict

# ggml/include/ggml.h, enum ggml_type
TYPES = {
    0: "F32", 1: "F16", 2: "Q4_0", 3: "Q4_1", 6: "Q5_0", 7: "Q5_1", 8: "Q8_0", 9: "Q8_1",
    10: "Q2_K", 11: "Q3_K", 12: "Q4_K", 13: "Q5_K", 14: "Q6_K", 15: "Q8_K",
    16: "IQ2_XXS", 17: "IQ2_XS", 18: "IQ3_XXS", 19: "IQ1_S", 20: "IQ4_NL", 21: "IQ3_S",
    22: "IQ2_S", 23: "IQ4_XS", 24: "I8", 25: "I16", 26: "I32", 27: "I64", 28: "F64",
    29: "IQ1_M", 30: "BF16", 34: "TQ1_0", 35: "TQ2_0", 39: "MXFP4", 40: "NVFP4",
    41: "Q1_0", 42: "Q2_0", 43: "TQ2_G128",
}

# types de valeurs de métadonnées GGUF (pour sauter proprement)
TAILLE_FIXE = {0: 1, 1: 1, 2: 2, 3: 2, 4: 4, 5: 4, 6: 4, 7: 1, 10: 8, 11: 8, 12: 8}

FAMILLES = ("attn_qkv", "attn_v", "attn_output", "ssm_out", "ffn_down", "ffn_gate", "ffn_up",
            "token_embd", "output")


def lire_entete(chemin: str):
    """Rend (meta, tenseurs) où tenseurs = [(nom, ne, type_id, offset)]."""
    with open(chemin, "rb") as f:
        magic = f.read(4)
        if magic != b"GGUF":
            raise ValueError(f"pas un GGUF : {magic!r}")
        version, = struct.unpack("<I", f.read(4))
        n_tensors, = struct.unpack("<Q", f.read(8))
        n_kv, = struct.unpack("<Q", f.read(8))

        def lire_chaine() -> str:
            n, = struct.unpack("<Q", f.read(8))
            return f.read(n).decode("utf-8", "replace")

        def sauter_valeur(t: int) -> None:
            if t == 8:                      # STRING
                n, = struct.unpack("<Q", f.read(8))
                f.seek(n, 1)
            elif t == 9:                    # ARRAY
                st, = struct.unpack("<I", f.read(4))
                n, = struct.unpack("<Q", f.read(8))
                for _ in range(n):
                    sauter_valeur(st)
            else:
                f.seek(TAILLE_FIXE[t], 1)

        meta = {}
        for _ in range(n_kv):
            cle = lire_chaine()
            t, = struct.unpack("<I", f.read(4))
            if t == 8:
                meta[cle] = lire_chaine()
            else:
                sauter_valeur(t)

        tenseurs = []
        for _ in range(n_tensors):
            nom = lire_chaine()
            nd, = struct.unpack("<I", f.read(4))
            ne = struct.unpack(f"<{nd}Q", f.read(8 * nd))
            tid, = struct.unpack("<I", f.read(4))
            off, = struct.unpack("<Q", f.read(8))
            tenseurs.append((nom, ne, tid, off))
    return version, meta, tenseurs


def famille(nom: str) -> str:
    for f in FAMILLES:
        if nom.endswith(f + ".weight"):
            return f
    return nom.split(".")[-2] if "." in nom else nom


def type_nom(tid: int) -> str:
    return TYPES.get(tid, f"INCONNU({tid})")


def rapport(chemin: str) -> None:
    version, meta, tenseurs = lire_entete(chemin)
    taille = os.path.getsize(chemin)
    print(f"\n=== {chemin} ===")
    print(f"  taille       : {taille:,} octets  ({taille/2**30:.2f} Gio / {taille/1e9:.2f} Go)")
    print(f"  GGUF v{version}  |  {len(tenseurs)} tenseurs")
    for cle in ("general.architecture", "general.name", "qwen35.block_count",
                "general.file_type", "obrain.rotation"):
        if cle in meta:
            print(f"  {cle:22s}: {meta[cle]}")

    par_famille: dict[str, Counter] = defaultdict(Counter)
    q4k_par_couche = defaultdict(set)
    for nom, ne, tid, _ in tenseurs:
        f = famille(nom)
        tn = type_nom(tid)
        par_famille[f][tn] += 1
        m = re.match(r"blk\.(\d+)\.", nom)
        if m and tn in ("Q4_K", "Q5_K", "Q6_K"):
            q4k_par_couche[int(m.group(1))].add(tn)

    print("  ── types par famille ──")
    for f in sorted(par_famille, key=lambda k: (-sum(par_famille[k].values()), k)):
        total = sum(par_famille[f].values())
        detail = ", ".join(f"{k}×{v}" for k, v in sorted(par_famille[f].items()))
        print(f"    {f:16s} ({total:3d}) {detail}")

    print("  ── signature de politique (familles ÉPARGNÉES du ternaire) ──")
    epargnees = []
    for f in FAMILLES:
        c = par_famille.get(f)
        if not c:
            continue
        autres = {k: v for k, v in c.items() if k != "TQ2_G128"}
        if not autres:
            continue
        # un « épargné » minoritaire = quelques couches seulement (bords), majoritaire = toute la famille
        dominant = max(autres, key=autres.get)
        part = autres[dominant] / sum(c.values())
        genre = "TOUTE la famille" if part > 0.9 else f"{autres[dominant]}/{sum(c.values())} couches (bords)"
        epargnees.append(f"    {f:16s} : {autres}  ← {dominant} sur {genre}")
    print("\n".join(epargnees) if epargnees else "    (aucune — tout ternaire)")

    n_layer = int(meta.get("qwen35.block_count", 0)) or (max(q4k_par_couche) + 1 if q4k_par_couche else 0)
    if n_layer:
        bas = sorted(c for c in q4k_par_couche if c < n_layer / 2)
        haut = sorted(c for c in q4k_par_couche if c >= n_layer / 2)
        # les bords : couches hautes en Q4_K hors attention, ou simplement la plage
        print(f"  ── couches {n_layer} ; surchargées en Q4_K/Q5_K/Q6_K : "
              f"{len(q4k_par_couche)} distinctes ──")
        if bas or haut:
            print(f"    plage basse {bas[:14] if bas else '—'} … haute {haut[-14:] if haut else '—'}")


if __name__ == "__main__":
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(2)
    for c in sys.argv[1:]:
        rapport(c)
