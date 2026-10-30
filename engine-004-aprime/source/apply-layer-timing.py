#!/usr/bin/env python3
# ENGINE-005C : instrumentation PAR COUCHE du temps GPU (paires d'evenements CUDA).
# Usage : python3 apply-layer-timing.py <ggml-cuda.cu>
import sys, os

ANCHOR_INCLUDE = '#include "ggml-cuda.h"\n'
REPL_INCLUDE = '#include "ggml-cuda.h"\n\n// [ENGINE-005C] attribution du temps GPU par couche. Inerte sans GGML_LAYER_TIMING=1.\n#include "ggml-cuda/ggml-cuda-layers.cuh"\n'

ANCHOR_FUNC_HEAD = "    // flag used to determine whether it is an integrated_gpu"
REPL_FUNC_HEAD = """    // [ENGINE-005C] etat par nœud : une iteration de capture peut n'executer qu'un
    // sous-ensemble des nœuds de la premiere iteration.
    const bool lt_run = ggml_layer_timing_env_on();
    std::vector<float>        lt_ms;
    std::vector<int>          lt_used;
    std::vector<int>          lt_layer;
    std::vector<const char *> lt_op;

    // flag used to determine whether it is an integrated_gpu"""

ANCHOR_LOOP = """            const bool budget = obrain_op_timing_enabled() && !use_cuda_graph;
            std::vector<cudaEvent_t>  bud_a;"""
REPL_LOOP = """            const bool budget = obrain_op_timing_enabled() && !use_cuda_graph;
            // [ENGINE-005C] fenetre d'attribution par couche.
            const bool ltiming = ggml_layer_timing_env_on();
            std::vector<cudaEvent_t> lt_a;
            std::vector<cudaEvent_t> lt_b;
            if (ltiming) {
                lt_a.resize(cgraph->n_nodes);
                lt_b.resize(cgraph->n_nodes);
                lt_ms.assign(cgraph->n_nodes, -1.0f);
                lt_layer.assign(cgraph->n_nodes, -1);
                lt_op.assign(cgraph->n_nodes, nullptr);
                lt_used.clear();
                for (int iii = 0; iii < cgraph->n_nodes; iii++) {
                    CUDA_CHECK(cudaEventCreate(&lt_a[iii]));
                    CUDA_CHECK(cudaEventCreate(&lt_b[iii]));
                    lt_layer[iii] = ggml_prov_layer_of_index(iii);   // [ENGINE-005D] provenance
                    lt_op[iii]    = ggml_op_name(cgraph->nodes[iii]->op);
                }
            }
            std::vector<cudaEvent_t>  bud_a;"""

ANCHOR_CALL = "                bool ok = ggml_cuda_compute_forward(*cuda_ctx, node);"
REPL_CALL = """                if (ltiming) { CUDA_CHECK(cudaEventRecord(lt_a[i], cuda_ctx->stream())); }
                bool ok = ggml_cuda_compute_forward(*cuda_ctx, node);
                if (ltiming) {
                    CUDA_CHECK(cudaEventRecord(lt_b[i], cuda_ctx->stream()));
                    lt_used.push_back(i);
                }"""

ANCHOR_COLLECT = """            if (budget && !bud_idx.empty()) {"""
REPL_COLLECT = """            if (ltiming) {
                // [ENGINE-005C] seuls les nœuds executes ont une paire valide, et il faut
                // synchroniser avant de mesurer ("device not ready" sinon).
                if (!lt_used.empty()) {
                    CUDA_CHECK(cudaEventSynchronize(lt_b[lt_used.back()]));
                }
                for (int iu : lt_used) {
                    float t = 0.0f;
                    CUDA_CHECK(cudaEventElapsedTime(&t, lt_a[iu], lt_b[iu]));
                    if (t > lt_ms[iu]) { lt_ms[iu] = t; }
                }
                for (int id = 0; id < cgraph->n_nodes; id++) {
                    CUDA_CHECK(cudaEventDestroy(lt_a[id]));
                    CUDA_CHECK(cudaEventDestroy(lt_b[id]));
                }
            }
            if (budget && !bud_idx.empty()) {"""

ANCHOR_POST = """    if (use_cuda_graph) {
        ggml_cuda_graph * graph = cuda_ctx->cuda_graph(graph_key);
        if (graph->instance == nullptr) { // Create executable graph from captured graph."""
REPL_POST = """    if (lt_run) {
        // [ENGINE-005C] accumulation APRES la boucle de capture, une fois par calcul de graphe.
        int    lt_measured = 0;
        double lt_sum = 0.0;
        for (int ia = 0; ia < (int) lt_ms.size(); ia++) {
            if (lt_ms[ia] >= 0.0f) {
                lt_measured++;
                lt_sum += (double) lt_ms[ia];
                ggml_layer_acc(lt_layer[ia], lt_op[ia], (double) lt_ms[ia]);
            }
        }
        std::fprintf(stderr, "E5C_COMPUTE nodes=%d measured=%d sum_ms=%.3f\\n",
                     cgraph->n_nodes, lt_measured, lt_sum);
        ggml_layer_token();
    }

    if (use_cuda_graph) {
        ggml_cuda_graph * graph = cuda_ctx->cuda_graph(graph_key);
        if (graph->instance == nullptr) { // Create executable graph from captured graph."""

def fr(text, a, r, name):
    n = text.count(a)
    if n != 1:
        raise SystemExit("ANCRE INTROUVABLE OU AMBIGUE (%s) : %d" % (name, n))
    return text.replace(a, r, 1)

def main():
    if len(sys.argv) != 2:
        raise SystemExit("usage: apply-layer-timing.py <ggml-cuda.cu>")
    path = sys.argv[1]
    text = open(path, encoding="utf-8").read()
    if "ggml-cuda-layers.cuh" in text:
        raise SystemExit("DEJA PATCHE")
    for a, r, n in ((ANCHOR_INCLUDE, REPL_INCLUDE, "include"),
                    (ANCHOR_FUNC_HEAD, REPL_FUNC_HEAD, "etat par nœud"),
                    (ANCHOR_LOOP, REPL_LOOP, "fenetre + evenements"),
                    (ANCHOR_CALL, REPL_CALL, "encadrement du nœud"),
                    (ANCHOR_COLLECT, REPL_COLLECT, "recolte"),
                    (ANCHOR_POST, REPL_POST, "accumulation")):
        text = fr(text, a, r, n)
        print("OK  %s" % n)
    tmp = path + ".e5c-tmp"
    open(tmp, "w", encoding="utf-8").write(text)
    os.replace(tmp, path)
    print("ECRIT %s (%d octets)" % (path, len(text)))

if __name__ == "__main__":
    main()
