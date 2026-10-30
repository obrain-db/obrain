#!/usr/bin/env python3
"""ENGINE-007B-F1A-Q0-R5 — R5-c/C4 : vraie racine vivante du graphe soumis.

CONSTAT DE MESURE : targets_soumis=168 cur_bad=0 mm_bad=0 cap_bad=0 down_bad=129.

cur_bad / mm_bad / cap_bad a zero : les relations src[]/op figees sont BONNES.
down_bad=129 : j'avais utilise un SUBSTITUT comme racine vivante
    live_out = cc->targets.back().t_mm_capture
ce qui est faux par construction : aucun mm_capture n'atteint un autre mm_capture.
C'est exactement ce que le paragraphe C4 interdit (« ne pas supposer que le dernier
noeud du tableau est automatiquement la sortie pertinente »).

Correctif : determiner la racine du DAG soumis = noeud qu AUCUN autre noeud ne consomme.
"""
import sys

C = "/home/triviere/projects/llama-obrain-cuda-f1a/src/f1a-capture.cpp"
H = "/home/triviere/projects/llama-obrain-cuda-f1a/src/f1a-capture.h"

OLD = """        // R5-c/C4 : racine vivante = noeud declare comme sortie du graphe soumis.
        // On ne suppose PAS que c est le dernier du tableau : on prend le noeud
        // dont aucun autre noeud soumis ne consomme la valeur, parmi les derniers.
        const ggml_tensor * live_out = nullptr;
        if (!cc->targets.empty()) live_out = cc->targets.back().t_mm_capture;"""

NEW = """        // R5-c/C4 : RACINE VIVANTE. Ni le dernier noeud du tableau, ni un substitut :
        // la racine du DAG soumis, c est-a-dire un noeud qu AUCUN autre ne consomme.
        const ggml_tensor * live_out = f1a_declared_graph_output();"""

ACC = """// R5-c/C4 : racine vivante du dernier graphe soumis.
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

ANCH = "const void * f1a_sub_node_src(const void * node, int k) {"


def main():
    s = open(C, encoding="utf-8").read()
    for old, new, name in ((OLD, NEW, "live_out"), (ANCH, ACC + ANCH, "accesseur racine")):
        n = s.count(old)
        if n != 1:
            raise SystemExit("[%s] ancre trouvee %d fois -- rien n'est ecrit" % (name, n))
        s = s.replace(old, new, 1)
    open(C, "w", encoding="utf-8").write(s)

    h = open(H, encoding="utf-8").read()
    a = "const void * f1a_sub_node_src(const void * node, int k);"
    if "f1a_declared_graph_output" not in h:
        if h.count(a) != 1:
            raise SystemExit("ancre d'en-tete introuvable")
        h = h.replace(a, "const ggml_tensor * f1a_declared_graph_output(void);\n" + a, 1)
        open(H, "w", encoding="utf-8").write(h)
    print("racine vivante corrigee")


if __name__ == "__main__":
    main()
