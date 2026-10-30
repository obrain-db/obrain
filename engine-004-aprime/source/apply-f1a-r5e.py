#!/usr/bin/env python3
"""ENGINE-007B-F1A-Q0-R5 (reprise) — R5-c/C4 : atteignabilite vers la sortie DECLAREE.

DEUX ECHECS PRECEDENTS, tous deux dus a une DEVINETTE topologique :
    live_out = dernier mm_capture        -> 129 echecs
    live_out = noeud non consomme        -> 153 echecs
Le brief interdit de deviner (« identifier les racines REELLEMENT utilisees »).

CE QUE J'AI TROUVE EN LISANT LA SOURCE :
    src/llama-graph.cpp:1305 et suivantes appellent ggml_set_output(t_logits), etc.
    ggml/include/ggml.h:662  GGML_TENSOR_FLAG_OUTPUT = 2  « is an output for the GGML
    compute graph »
La sortie est donc DECLAREE par un DRAPEAU, pas par une position dans le tableau.

Correctif : figer tensor->flags dans le snapshot, puis exiger que mm_capture atteigne
AU MOINS UN noeud portant GGML_TENSOR_FLAG_OUTPUT dans le graphe soumis.
"""
import sys

C = "/home/triviere/projects/llama-obrain-cuda-f1a/src/f1a-capture.cpp"
H = "/home/triviere/projects/llama-obrain-cuda-f1a/src/f1a-capture.h"

# --- 1. figer le drapeau dans le noeud snapshot
OLD_NS = """struct f1a_node_snap {
    const void * ptr;
    int          op;
    const void * src[3];
};"""
NEW_NS = """struct f1a_node_snap {
    const void * ptr;
    int          op;
    int          flags;          // R5-c/C4 : porte GGML_TENSOR_FLAG_OUTPUT ?
    const void * src[3];
};"""

OLD_FILL = """        f1a_node_snap ns;
        ns.ptr = (const void *) t;
        ns.op  = (int) t->op;"""
NEW_FILL = """        f1a_node_snap ns;
        ns.ptr   = (const void *) t;
        ns.op    = (int) t->op;
        ns.flags = (int) t->flags;
        if (t->flags & GGML_TENSOR_FLAG_OUTPUT) s.n_outputs++;"""

OLD_S = """    std::vector<f1a_node_snap> nodes;         // R5-c : relations structurelles figees
    unsigned long long fingerprint;
};"""
NEW_S = """    std::vector<f1a_node_snap> nodes;         // R5-c : relations structurelles figees
    int                n_outputs;             // R5-c/C4 : sorties DECLAREES du graphe
    unsigned long long fingerprint;
};"""

# --- 2. remplacement du calcul d'atteignabilite
OLD_ACC = """// R5-c/C4 : racine vivante du dernier graphe soumis.
// On ne PRESUME pas que c est le dernier noeud du tableau : on cherche un noeud dont
// aucune source d aucun autre noeud ne depend. C est la racine du DAG.
const ggml_tensor * f1a_declared_graph_output(void) {
    if (g_f1a_snaps.empty()) return nullptr;
    const f1a_sub_snap & s = g_f1a_snaps.back();
    if (s.nodes.empty()) return nullptr;
    for (size_t i = s.nodes.size(); i-- > 0; ) {
        const void * cand = s.nodes[i].ptr;
        bool consumed = false;
        for (size_t j = 0; j < s.nodes.size() && !consumed; ++j) {
            for (int k = 0; k < 3; ++k) {
                if (s.nodes[j].src[k] == cand) { consumed = true; break; }
            }
        }
        if (!consumed) return (const ggml_tensor *) cand;
    }
    return (const ggml_tensor *) s.nodes.back().ptr;
}

"""

NEW_ACC = """// R5-c/C4 : atteignabilite vers une sortie DECLAREE du graphe soumis.
// La sortie n est pas devinee : elle porte GGML_TENSOR_FLAG_OUTPUT, pose par
// ggml_set_output() (voir src/llama-graph.cpp:1305 et suivantes).
// On calcule l ensemble des ancetres de TOUTES les sorties declarees, en remontant
// src[] depuis chacune : un noeud atteint une sortie si et seulement s il est dans
// cet ensemble. Une passe lineaire, pas un BFS par cible.
int f1a_submitted_graph_output_count(void) {
    return g_f1a_snaps.empty() ? 0 : g_f1a_snaps.back().n_outputs;
}

bool f1a_node_reaches_declared_output(const void * node) {
    if (node == nullptr || g_f1a_snaps.empty()) return false;
    const f1a_sub_snap & s = g_f1a_snaps.back();
    // marquer les ancetres des sorties declarees, en remontant
    std::vector<const void *> work;
    std::vector<const void *> ance;
    for (size_t i = 0; i < s.nodes.size(); ++i) {
        if (s.nodes[i].flags & GGML_TENSOR_FLAG_OUTPUT) {
            work.push_back(s.nodes[i].ptr);
            ance.push_back(s.nodes[i].ptr);
        }
    }
    while (!work.empty()) {
        const void * cur = work.back();
        work.pop_back();
        for (size_t j = 0; j < s.nodes.size(); ++j) {
            if (s.nodes[j].ptr != cur) continue;
            for (int k = 0; k < 3; ++k) {
                const void * p = s.nodes[j].src[k];
                if (p == nullptr) continue;
                bool seen = false;
                for (size_t z = 0; z < ance.size(); ++z) if (ance[z] == p) { seen = true; break; }
                if (seen) continue;
                ance.push_back(p);
                work.push_back(p);
            }
        }
    }
    for (size_t z = 0; z < ance.size(); ++z) if (ance[z] == node) return true;
    return false;
}

"""

# --- 3. usage dans la verification des roles
OLD_USE = "            const int down_ok = f1a_reaches_from_to(r.t_mm_capture, live_out, 32);"
NEW_USE = "            const int down_ok = f1a_node_reaches_declared_output(r.t_mm_capture) ? 1 : 0;"

OLD_DECL = """        const ggml_tensor * live_out = f1a_declared_graph_output();"""
NEW_DECL = """        const int n_declared_out = f1a_submitted_graph_output_count();
        fprintf(stderr, "[f1a-out] sorties DECLAREES du graphe soumis = %d\\n", n_declared_out);"""


def main():
    s = open(C, encoding="utf-8").read()
    for old, new, name in ((OLD_NS, NEW_NS, "struct noeud"),
                           (OLD_S, NEW_S, "struct snapshot"),
                           (OLD_FILL, NEW_FILL, "remplissage flags"),
                           (OLD_ACC, NEW_ACC, "atteignabilite"),
                           (OLD_USE, NEW_USE, "usage down_ok"),
                           (OLD_DECL, NEW_DECL, "declaration live_out")):
        n = s.count(old)
        if n != 1:
            raise SystemExit("[%s] ancre trouvee %d fois -- rien n'est ecrit" % (name, n))
        s = s.replace(old, new, 1)
    open(C, "w", encoding="utf-8").write(s)

    h = open(H, encoding="utf-8").read()
    h = h.replace("const ggml_tensor * f1a_declared_graph_output(void);",
                  "int  f1a_submitted_graph_output_count(void);\n"
                  "bool f1a_node_reaches_declared_output(const void * node);")
    open(H, "w", encoding="utf-8").write(h)
    print("R5-c/C4 : atteignabilite vers la sortie DECLAREE integree")


if __name__ == "__main__":
    main()
