#!/usr/bin/env python3
"""ENGINE-007B-F1A-S2bis-I1 — greffe DIAGNOSTIQUE de capture liee au graphe.

Transformation, pour les seules projections TQ2_G128 eligibles, EN TROIS LIGNES :

    ggml_tensor * res = ggml_mul_mat(ctx0, w, cur);
    // greffe :
    res = ggml_dup(ctx0, res);        <- mm_capture : NOEUD MATERIALISE, sur le chemin vivant
    res = ggml_dup(ctx0, cur);        <- cur_capture ; puis on refait le matmul sur la copie

Concretement, le remplacement est :

    ggml_tensor * res = ggml_mul_mat(ctx0, w, cur);

devient

    ggml_tensor * res;
    if (capture && eligible(w)) {
        ggml_tensor * cc = ggml_dup(ctx0, cur);     // cur_capture
        res = ggml_mul_mat(ctx0, w, cc);            // le matmul consomme la COPIE
        res = ggml_dup(ctx0, res);                  // mm_capture, AVANT toute contribution LoRA
        f1a_capture_note(...);
    } else {
        res = ggml_mul_mat(ctx0, w, cur);           // chemin natif, INCHANGE
    }

Le reste de la fonction (w_s, boucle LoRA, tout l'aval) est INTACT et s'applique a `res`,
donc a `mm_capture`. Aucun bloc n'est duplique.

Les deux dup sont sur le chemin vivant : `res` est retourne par la fonction, donc le
planificateur execute forcement les deux copies.

Inerte sans OBRAIN_F1A_CAPTURE=1. Chaque ancre doit apparaitre exactement une fois.
"""
import sys

MARK = "// [F1A-CAPTURE]"

ANCHOR_INC = '#include "llama-graph.h"'
REPL_INC = '''#include "llama-graph.h"

// [F1A-CAPTURE] greffe diagnostique (ENGINE-007B-F1A-S2bis-I1). Inerte sans OBRAIN_F1A_CAPTURE=1.
// Ne touche aucun noyau CUDA, aucun format de quantification, aucun poids de modele.
#include "f1a-capture.h"'''

ANCHOR_RAW = """          ggml_tensor * cur,
          ggml_tensor * w_s) const {"""
REPL_RAW = """          ggml_tensor * cur,
          ggml_tensor * w_s) const {
    // [F1A-CAPTURE] activation telle qu'elle ARRIVE ; la rotation WHT ci-dessous peut la remplacer.
    f1a_capture_ctx * const f1a_cc = f1a_capture_get_ctx();
    ggml_tensor * const f1a_cur_raw = cur;"""

ANCHOR_MM = "    ggml_tensor * res = ggml_mul_mat(ctx0, w, cur);"
REPL_MM = """    // [F1A-CAPTURE] capture diagnostique : inerte sans OBRAIN_F1A_CAPTURE=1.
    // `cur` est ici EXACTEMENT l'activation consommee par le matmul (brute ou repliee par WHT).
    ggml_tensor * res;
    if (f1a_cc != nullptr && f1a_eligible(w)) {
        ggml_tensor * const f1a_cur_cap = ggml_dup(ctx0, cur);
        ggml_tensor * const f1a_mm_raw  = ggml_mul_mat(ctx0, w, f1a_cur_cap);
        res = ggml_dup(ctx0, f1a_mm_raw);   // mm_capture : AVANT toute contribution LoRA
        f1a_capture_note(f1a_cc, w, f1a_cur_raw, cur, f1a_cur_cap, f1a_mm_raw, res);
    } else {
        res = ggml_mul_mat(ctx0, w, cur);
    }"""

def once(text, anchor, name):
    n = text.count(anchor)
    if n != 1:
        raise SystemExit("ANCRE '%s' : %d occurrence(s), attendu 1 -- rien n'est ecrit" % (name, n))

def main():
    if len(sys.argv) < 2:
        raise SystemExit("usage: apply-f1a-capture.py <chemin/llama-graph.cpp>")
    path = sys.argv[1]
    text = open(path, encoding="utf-8").read()
    if "f1a-capture.h" in text:
        raise SystemExit("deja applique")
    once(text, ANCHOR_INC, "include")
    once(text, ANCHOR_RAW, "signature build_lora_mm")
    once(text, ANCHOR_MM,  "mul_mat")
    text = text.replace(ANCHOR_INC, REPL_INC, 1)
    text = text.replace(ANCHOR_RAW, REPL_RAW, 1)
    text = text.replace(ANCHOR_MM,  REPL_MM,  1)
    open(path, "w", encoding="utf-8").write(text)
    print("greffe appliquee : %s" % path)

if __name__ == "__main__":
    main()
