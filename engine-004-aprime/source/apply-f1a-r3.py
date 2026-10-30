#!/usr/bin/env python3
"""ENGINE-007B-F1A-Q0-R3 — patch d'instrumentation : instantanes de soumission.

Remplace le hook R2 (qui ne gardait qu'un compteur et un dernier pointeur) par un
instantane FIGE des noeuds du graphe AU MOMENT de l'appel de soumission, avant toute
reutilisation de l'objet ggml_cgraph.

Ajoute f1a_ptr_in_submitted_graph() : la seule definition operationnelle valide de
« ce pointeur appartient-il a un graphe effectivement soumis ».
"""
import sys

C = "/home/triviere/projects/llama-obrain-cuda-f1a/src/f1a-capture.cpp"
H = "/home/triviere/projects/llama-obrain-cuda-f1a/src/f1a-capture.h"

OLD = """void f1a_mark_compute_submission(const void * gf, int n_nodes) {
    // HOOK AUTORITAIRE : c est ICI que la soumission est prouvee.
    f1a_ctr_submission++;
    f1a_last_gf = gf;
    f1a_last_gf_nodes = n_nodes;
}"""

NEW = """// R3 : instantanes figes AU MOMENT de la soumission, avant toute reutilisation.
struct f1a_sub_snap {
    unsigned long long submission_id;
    const void *       gf;
    int                n_nodes;
    int                rc;
    std::vector<const void *> node_ptrs;
    unsigned long long fingerprint;
};
static std::vector<f1a_sub_snap> g_f1a_snaps;

void f1a_mark_compute_submission(struct ggml_cgraph * gf, int submission_rc) {
    // HOOK AUTORITAIRE : c est ICI, et nulle part ailleurs, qu une soumission est prouvee.
    f1a_ctr_submission++;
    f1a_last_gf = (const void *) gf;
    f1a_last_gf_nodes = gf ? ggml_graph_n_nodes(gf) : 0;
    f1a_sub_snap s;
    s.submission_id = f1a_ctr_submission;
    s.gf            = (const void *) gf;
    s.n_nodes       = f1a_last_gf_nodes;
    s.rc            = submission_rc;
    s.fingerprint   = 1469598103934665603ULL;   // FNV-1a sur (ptr, op) et les sources
    for (int i = 0; i < s.n_nodes; ++i) {
        struct ggml_tensor * t = ggml_graph_node(gf, i);
        if (t == nullptr) continue;
        s.node_ptrs.push_back((const void *) t);
        unsigned long long x = (unsigned long long)(uintptr_t) t ^ ((unsigned long long) t->op << 48);
        s.fingerprint = (s.fingerprint ^ x) * 1099511628211ULL;
        for (int k = 0; k < GGML_MAX_SRC; ++k) {
            if (t->src[k] == nullptr) continue;
            unsigned long long y = (unsigned long long)(uintptr_t) t->src[k];
            s.fingerprint = (s.fingerprint ^ y) * 1099511628211ULL;
        }
    }
    g_f1a_snaps.push_back(s);
    fprintf(stderr, "[f1a-sub] submission=%llu gf=%p nodes=%d rc=%d fp=%016llx\\n",
            f1a_ctr_submission, (void *) gf, f1a_last_gf_nodes, submission_rc, s.fingerprint);
}

bool f1a_ptr_in_submitted_graph(const void * p) {
    if (p == nullptr) return false;
    for (size_t i = 0; i < g_f1a_snaps.size(); ++i) {
        const std::vector<const void *> & v = g_f1a_snaps[i].node_ptrs;
        for (size_t j = 0; j < v.size(); ++j) if (v[j] == p) return true;
    }
    return false;
}

void f1a_dump_submitted_snapshots() {
    FILE * f = fopen((g_f1a.outdir + "/SUBMITTED_GRAPH_SNAPSHOTS.csv").c_str(), "w");
    if (f == nullptr) return;
    fprintf(f, "submission_id,gf,n_nodes,submission_rc,fingerprint\\n");
    for (size_t i = 0; i < g_f1a_snaps.size(); ++i) {
        const f1a_sub_snap & s = g_f1a_snaps[i];
        fprintf(f, "%llu,%p,%d,%d,%016llx\\n", s.submission_id, s.gf, s.n_nodes, s.rc, s.fingerprint);
    }
    fclose(f);
}"""


def main():
    s = open(C, encoding="utf-8").read()
    if "f1a_sub_snap" in s:
        print("deja applique"); return
    if s.count(OLD) != 1:
        raise SystemExit("ancre du hook R2 introuvable (ou multiple) : rien n'est ecrit")
    s = s.replace(OLD, NEW, 1)
    # dump des instantanes a la fin d epoque
    a = '    fprintf(stderr, "[f1a-ctr] ctx_active='
    if s.count(a) != 1:
        raise SystemExit("ancre du dump compteurs introuvable : rien n'est ecrit")
    s = s.replace(a, '    f1a_dump_submitted_snapshots();\n' + a, 1)
    open(C, "w", encoding="utf-8").write(s)

    h = open(H, encoding="utf-8").read()
    h = h.replace("void f1a_mark_compute_submission(const void * gf, int n_nodes);",
                  "void f1a_mark_compute_submission(struct ggml_cgraph * gf, int submission_rc);\n"
                  "bool f1a_ptr_in_submitted_graph(const void * p);\n"
                  "void f1a_dump_submitted_snapshots();")
    open(H, "w", encoding="utf-8").write(h)
    print("R3 : instantanes de soumission integres")


if __name__ == "__main__":
    main()
