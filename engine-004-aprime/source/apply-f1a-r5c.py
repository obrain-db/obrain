#!/usr/bin/env python3
"""ENGINE-007B-F1A-Q0-R5 — R5-c : figer les relations src[] et les op au point de soumission.

CONSTAT (R5-a) : le snapshot existant conserve node_ptrs (identites) et calcule une empreinte
sur (ptr, op, src), mais il NE CONSERVE PAS op ni src[]. On peut donc prouver l'APPARTENANCE
d'un pointeur au graphe soumis, pas sa RELATION STRUCTURELLE.

Or le paragraphe C3 interdit explicitement de remplacer les tests de relation par une simple
recherche de pointeur dans node_ptrs.

Correctif : etendre f1a_sub_snap avec les noeuds COMPLETS (ptr, op, src[0..2]) figes au moment
de la soumission, avant toute reutilisation. Aucune refonte du systeme de snapshot : il est
complete, pas remplace.
"""
import sys

C = "/home/triviere/projects/llama-obrain-cuda-f1a/src/f1a-capture.cpp"
H = "/home/triviere/projects/llama-obrain-cuda-f1a/src/f1a-capture.h"

# --- 1. struct de noeud fige + acces aux soumissions
OLD_STRUCT = """struct f1a_sub_snap {
    unsigned long long submission_id;
    const void *       gf;
    int                n_nodes;
    int                rc;
    std::vector<const void *> node_ptrs;
    unsigned long long fingerprint;
};"""

NEW_STRUCT = """// R5-c : noeud FIGE au moment de la soumission (op et sources compris).
struct f1a_node_snap {
    const void * ptr;
    int          op;
    const void * src[3];
};

struct f1a_sub_snap {
    unsigned long long submission_id;
    const void *       gf;
    int                n_nodes;
    int                rc;
    std::vector<const void *> node_ptrs;      // conserve pour f1a_ptr_in_submitted_graph
    std::vector<f1a_node_snap> nodes;         // R5-c : relations structurelles figees
    unsigned long long fingerprint;
};

// R5-c : acces en lecture aux soumissions figees, pour la qualification des relations.
const void * f1a_sub_node_src(const void * node, int k);   // -1 si non trouve
int          f1a_sub_node_op(const void * node);           // -1 si non trouve
bool         f1a_reaches_from_to(const void * from, const void * to, int max_hops);"""

# --- 2. remplir nodes[] dans le hook
OLD_FILL = """        s.node_ptrs.push_back((const void *) t);
        unsigned long long x = (unsigned long long)(uintptr_t) t ^ ((unsigned long long) t->op << 48);"""
NEW_FILL = """        s.node_ptrs.push_back((const void *) t);
        f1a_node_snap ns;
        ns.ptr = (const void *) t;
        ns.op  = (int) t->op;
        for (int k = 0; k < 3; ++k) ns.src[k] = (const void *) t->src[k];
        s.nodes.push_back(ns);
        unsigned long long x = (unsigned long long)(uintptr_t) t ^ ((unsigned long long) t->op << 48);"""
s_old = OLD_FILL

# --- 3. acces + parcours de dependance
ACCESS = """
// ---------------------------------------------------------------- R5-c : acces aux figes

const void * f1a_sub_node_src(const void * node, int k) {
    if (node == nullptr || k < 0 || k > 2) return nullptr;
    for (size_t i = 0; i < g_f1a_snaps.size(); ++i) {
        const std::vector<f1a_node_snap> & v = g_f1a_snaps[i].nodes;
        for (size_t j = 0; j < v.size(); ++j) {
            if (v[j].ptr == node) return v[j].src[k];
        }
    }
    return nullptr;
}

int f1a_sub_node_op(const void * node) {
    if (node == nullptr) return -1;
    for (size_t i = 0; i < g_f1a_snaps.size(); ++i) {
        const std::vector<f1a_node_snap> & v = g_f1a_snaps[i].nodes;
        for (size_t j = 0; j < v.size(); ++j) {
            if (v[j].ptr == node) return v[j].op;
        }
    }
    return -1;
}

// Parcours en avant : 'from' atteint-il 'to' en suivant les aretes src -> consommateur ?
// On construit la liste des consommateurs implicite : un noeud consomme 'from' si from est
// l'une de ses sources. Bornage explicite pour ne jamais boucler.
bool f1a_reaches_from_to(const void * from, const void * to, int max_hops) {
    if (from == nullptr || to == nullptr) return false;
    std::vector<const void *> frontier;
    std::vector<const void *> seen;
    frontier.push_back(from);
    seen.push_back(from);
    for (int hop = 0; hop < max_hops; ++hop) {
        std::vector<const void *> next;
        for (size_t f = 0; f < frontier.size(); ++f) {
            for (size_t i = 0; i < g_f1a_snaps.size(); ++i) {
                const std::vector<f1a_node_snap> & v = g_f1a_snaps[i].nodes;
                for (size_t j = 0; j < v.size(); ++j) {
                    bool consumes = false;
                    for (int k = 0; k < 3; ++k) if (v[j].src[k] == frontier[f]) { consumes = true; break; }
                    if (!consumes) continue;
                    if (v[j].ptr == to) return true;
                    bool already = false;
                    for (size_t z = 0; z < seen.size(); ++z) if (seen[z] == v[j].ptr) { already = true; break; }
                    if (already) continue;
                    seen.push_back(v[j].ptr);
                    next.push_back(v[j].ptr);
                }
            }
        }
        if (next.empty()) return false;
        frontier.swap(next);
    }
    return false;
}
"""


def main():
    s = open(C, encoding="utf-8").read()
    for old, new, name in ((OLD_STRUCT, NEW_STRUCT, "struct de snapshot"),
                           (OLD_FILL, NEW_FILL, "remplissage des noeuds")):
        n = s.count(old)
        if n != 1:
            raise SystemExit("[%s] ancre trouvee %d fois -- rien n'est ecrit" % (name, n))
        s = s.replace(old, new, 1)

    # inserer les fonctions d'acces juste avant f1a_eligible
    anchor = "bool f1a_eligible(const ggml_tensor * w) {"
    if s.count(anchor) != 1:
        raise SystemExit("ancre de f1a_eligible introuvable")
    s = s.replace(anchor, ACCESS + "\n" + anchor, 1)
    open(C, "w", encoding="utf-8").write(s)

    h = open(H, encoding="utf-8").read()
    a = "bool f1a_ptr_in_submitted_graph(const void * p);"
    if h.count(a) != 1:
        raise SystemExit("ancre d'en-tete introuvable")
    h = h.replace(a, a + """
// R5-c : relations structurelles des noeuds du graphe soumis.
const void * f1a_sub_node_src(const void * node, int k);
int          f1a_sub_node_op(const void * node);
bool         f1a_reaches_from_to(const void * from, const void * to, int max_hops);""", 1)
    open(H, "w", encoding="utf-8").write(h)
    print("R5-c : snapshot etendu aux relations structurelles")


if __name__ == "__main__":
    main()
