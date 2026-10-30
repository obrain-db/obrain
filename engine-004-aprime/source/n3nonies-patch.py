#!/usr/bin/env python3
"""N3nonies — applique ggml_set_output() aux tenseurs de capture de la greffe.

Mecanisme prouve : ggml-alloc.c:690, ggml_gallocr_free_node ne protege que les
tenseurs portant GGML_TENSOR_FLAG_OUTPUT. Sans ce drapeau, la region de stockage
de cur_capture et mm_capture est liberee des leur derniere consommation, donc bien
avant l'extraction de fin d'epoque. C'est ce que le fork fait lui-meme pour ses
propres copies de sortie (ggml-backend.cpp:1342).
"""
import sys

ROOT = "/home/triviere/projects/llama-obrain-cuda-f1a"

H = ROOT + "/src/f1a-capture.h"
C = ROOT + "/src/f1a-capture.cpp"
G = ROOT + "/src/llama-graph.cpp"

DECL_ANCHOR = "bool f1a_capture_active(const f1a_capture_ctx * cc);"

DECL_ADD = DECL_ANCHOR + """

// N3nonies : marque un tenseur de capture avec GGML_TENSOR_FLAG_OUTPUT.
// ggml-alloc.c:690 : sans ce drapeau, ggml_gallocr_free_node LIBERE la region
// des la derniere consommation -- donc avant notre extraction de fin d epoque.
// Le fork fait exactement cela pour ses propres copies de sortie (backend.cpp:1342).
void f1a_mark_output(struct ggml_tensor * t);"""

DEF_ANCHOR = "void f1a_mark_compute_submission(struct ggml_cgraph * gf, int submission_rc) {"

DEF_ADD = """void f1a_mark_output(struct ggml_tensor * t) {
    if (t != nullptr) {
        ggml_set_output(t);
    }
}

"""

GRAFT_OLD = """        ggml_tensor * const f1a_cur_cap = ggml_dup(ctx0, cur);
        ggml_tensor * const f1a_mm_raw  = ggml_mul_mat(ctx0, w, f1a_cur_cap);
        res = ggml_dup(ctx0, f1a_mm_raw);   // mm_capture : AVANT toute contribution LoRA"""

GRAFT_NEW = """        ggml_tensor * const f1a_cur_cap = ggml_dup(ctx0, cur);
        f1a_mark_output(f1a_cur_cap);   // N3nonies : sinon ggml-alloc libere sa region
        ggml_tensor * const f1a_mm_raw  = ggml_mul_mat(ctx0, w, f1a_cur_cap);
        res = ggml_dup(ctx0, f1a_mm_raw);   // mm_capture : AVANT toute contribution LoRA
        f1a_mark_output(res);               // N3nonies : idem pour mm_capture"""


def patch(path, needle, replacement, label):
    s = open(path).read()
    if "f1a_mark_output" in s and needle not in s:
        print("  %-28s DEJA APPLIQUE" % label)
        return
    n = s.count(needle)
    if n != 1:
        print("  %-28s ANCRE INTROUVABLE OU AMBIGUE (%d occurrences)" % (label, n))
        sys.exit(1)
    open(path, "w").write(s.replace(needle, replacement, 1))
    print("  %-28s PATCHED" % label)


print("=== application du correctif N3nonies ===")
patch(H, DECL_ANCHOR, DECL_ADD, "f1a-capture.h declaration")
patch(C, DEF_ANCHOR, DEF_ADD + DEF_ANCHOR, "f1a-capture.cpp fonction")
patch(G, GRAFT_OLD, GRAFT_NEW, "llama-graph.cpp greffe")
print()
print("=== verification : les deux marquages sont en place ===")
s = open(G).read()
i = s.find("f1a_cur_cap = ggml_dup")
print(s[i - 40:i + 620])
