#!/usr/bin/env python3
# ENGINE-005D : pose du sidecar de provenance.
#  1) dans le builder de couches : enregistre le POINTEUR de sortie de couche (aucune autre action)
#  2) dans ggml-cuda.cu : inclut l'en-tete et analyse le graphe construit
# Usage : python3 apply-provenance.py <src/models/qwen35.cpp> <ggml/src/ggml-cuda/ggml-cuda.cu>
import sys, os

ANCHOR_QWEN = """        cur = build_cvec(cur, il);
        cb(cur, "l_out", il);
"""
REPL_QWEN = """        cur = build_cvec(cur, il);
        cb(cur, "l_out", il);

        // [ENGINE-005D] sidecar de provenance : on enregistre le POINTEUR du tenseur de sortie
        // de cette couche. Aucune modification du tenseur, aucun renommage, aucun nœud ajoute.
        ggml_prov_record_layer_out(il, cur);
"""

ANCHOR_INC = '#include "ggml-cuda.h"\n'
REPL_INC = '#include "ggml-cuda.h"\n\n// [ENGINE-005D] sidecar de provenance (pointeur -> couche).\n#include "ggml-cuda/ggml-cuda-provenance.cuh"\n'

ANCHOR_ANALYZE = """static enum ggml_status ggml_backend_cuda_graph_compute(ggml_backend_t backend, ggml_cgraph * cgraph) {
    ggml_backend_cuda_context * cuda_ctx = (ggml_backend_cuda_context *) backend->context;
"""
REPL_ANALYZE = """static enum ggml_status ggml_backend_cuda_graph_compute(ggml_backend_t backend, ggml_cgraph * cgraph) {
    ggml_backend_cuda_context * cuda_ctx = (ggml_backend_cuda_context *) backend->context;

    // [ENGINE-005D] analyse du graphe CONSTRUIT : index des nœuds par pointeur + ancres de couche.
    // Fait avant l'execution, hors chemin chaud.
    if (ggml_prov_enabled()) {
        ggml_prov_analyze((const void **) cgraph->nodes, cgraph->n_nodes, (const char **) nullptr);
    }
"""

def fr(t, a, r, n):
    c = t.count(a)
    if c != 1:
        raise SystemExit("ANCRE INTROUVABLE OU AMBIGUE (%s) : %d" % (n, c))
    return t.replace(a, r, 1)

def main():
    if len(sys.argv) != 3:
        raise SystemExit("usage: apply-provenance.py <qwen35.cpp> <ggml-cuda.cu>")
    q, g = sys.argv[1], sys.argv[2]
    tq = open(q, encoding="utf-8").read()
    if "[ENGINE-005D]" in tq:
        raise SystemExit("QWEN DEJA PATCHE")
    tq = fr(tq, ANCHOR_QWEN, REPL_QWEN, "enregistrement de la sortie de couche")
    tq = tq.replace('#include "models.h"',
                    '#include "models.h"\n#include "../../ggml/src/ggml-cuda/ggml-cuda-provenance.cuh"', 1)
    open(q + ".tmp", "w", encoding="utf-8").write(tq); os.replace(q + ".tmp", q)
    print("OK  qwen35.cpp (enregistrement de la sortie de couche)")

    tg = open(g, encoding="utf-8").read()
    if "[ENGINE-005D]" in tg:
        raise SystemExit("GGML-CUDA DEJA PATCHE")
    tg = fr(tg, ANCHOR_INC, REPL_INC, "include")
    tg = fr(tg, ANCHOR_ANALYZE, REPL_ANALYZE, "analyse du graphe")
    open(g + ".tmp", "w", encoding="utf-8").write(tg); os.replace(g + ".tmp", g)
    print("OK  ggml-cuda.cu (include + analyse)")

if __name__ == "__main__":
    main()
