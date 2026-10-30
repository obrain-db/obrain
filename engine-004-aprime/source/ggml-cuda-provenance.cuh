// ggml-cuda-provenance.cuh — ENGINE-005D : provenance de construction (sidecar de pointeurs).
//
// Associer chaque NOEUD EXECUTE a une COUCHE, sans toucher au graphe, sans renommer de tenseur,
// sans ajouter d'operation backend :
//   1. pendant la construction, on enregistre le POINTEUR du tenseur de sortie de chaque couche ;
//   2. une fois le graphe construit, on retro-trouve ces pointeurs dans cgraph->nodes ;
//   3. on attribue par INTERVALLES entre ancres consecutives (ordre topologique verifie).
//
// Inerte sans GGML_LAYER_PROVENANCE=1.

#pragma once

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <cstdint>
#include <map>
#include <string>
#include <vector>
#include <algorithm>

struct ggml_prov_state {
    // --- enregistre pendant la construction
    std::map<int, const void *> layer_out;                  // il -> pointeur sortie de couche

    // --- analyse apres construction
    std::map<const void *, int> node_index_by_ptr;
    std::map<int, int>          layer_out_node_index;       // il -> index dans cgraph
    std::vector<int>            node_layer;                 // index -> couche (-1 inconnue, -2 post)

    // --- temps par couche (alimente par l'instrument, une fois par noeud execute)
    std::map<int, double>             layer_ms;
    std::map<int, unsigned long long> layer_n;
    std::map<std::string, double>     layer_op_ms;          // "<couche>|<op>" -> ms

    // --- signatures MUL_MAT
    struct mm_rec {
        unsigned long long calls = 0;
        std::string        role;
        std::string        type_name;
        long long          M = 0, N = 0, K = 0;
        double             ms = 0.0;
        int                layer = -1;
    };
    std::map<std::string, mm_rec> mm_sig;
    std::map<std::string, mm_rec> mm_role;      // agrege par (classe, role, type)
    std::map<const void *, std::string> weight_role_of_ptr;   // [E5G-R2] pointeur -> role
};

inline ggml_prov_state & ggml_prov() {
    static ggml_prov_state s;
    return s;
}

inline bool ggml_prov_enabled() {
    static const bool on = getenv("GGML_LAYER_PROVENANCE") != nullptr;
    return on;
}

// ---------------------------------------------------------------- pendant la construction
inline void ggml_prov_record_layer_out(int il, const void * t) {
    if (!ggml_prov_enabled() || il < 0 || t == nullptr) {
        return;
    }
    ggml_prov().layer_out[il] = t;
}

// ---------------------------------------------------------------- analyse du graphe construit
inline void ggml_prov_analyze(const void ** nodes, int n_nodes) {
    if (!ggml_prov_enabled()) {
        return;
    }
    ggml_prov_state & st = ggml_prov();

    st.node_index_by_ptr.clear();
    for (int i = 0; i < n_nodes; ++i) {
        st.node_index_by_ptr[nodes[i]] = i;
    }

    st.layer_out_node_index.clear();
    for (const auto & kv : st.layer_out) {
        auto it = st.node_index_by_ptr.find(kv.second);
        if (it != st.node_index_by_ptr.end()) {
            st.layer_out_node_index[kv.first] = it->second;
        }
    }

    // attribution par intervalles : couche i = (index sortie i-1, index sortie i]
    st.node_layer.assign((size_t) n_nodes, -1);
    std::vector<std::pair<int, int>> anchors;   // (index, couche)
    for (const auto & kv : st.layer_out_node_index) {
        anchors.emplace_back(kv.second, kv.first);
    }
    std::sort(anchors.begin(), anchors.end());
    int prev = -1;
    for (const auto & a : anchors) {
        for (int i = prev + 1; i <= a.first && i < n_nodes; ++i) {
            st.node_layer[(size_t) i] = a.second;
        }
        prev = a.first;
    }
    for (int i = prev + 1; i < n_nodes; ++i) {
        st.node_layer[(size_t) i] = -2;   // POST_MODEL
    }
}

inline int ggml_prov_layer_of_index(int i) {
    ggml_prov_state & st = ggml_prov();
    if (i < 0 || (size_t) i >= st.node_layer.size()) {
        return -1;
    }
    return st.node_layer[(size_t) i];
}

// ---------------------------------------------------------------- temps par couche
inline void ggml_prov_acc(int layer, const char * op, double ms) {
    if (!ggml_prov_enabled()) {
        return;
    }
    ggml_prov_state & st = ggml_prov();
    st.layer_ms[layer] += ms;
    st.layer_n[layer]  += 1;
    if (op != nullptr) {
        char k[160];
        snprintf(k, sizeof(k), "%d|%s", layer, op);
        st.layer_op_ms[k] += ms;
    }
}

// ---------------------------------------------------------------- signatures MUL_MAT
inline void ggml_prov_record_matmul(int layer, const void * wptr, const char * role, const char * type_name,
                                    long long M, long long N, long long K, double ms) {
    if (!ggml_prov_enabled() || role == nullptr) {
        return;
    }
    ggml_prov_state & st = ggml_prov();
    char key[320];
    snprintf(key, sizeof(key), "%d|%s|%s|%lld|%lld|%lld", layer, role,
             type_name ? type_name : "?", M, N, K);
    if (wptr != nullptr) {
        st.weight_role_of_ptr[wptr] = role;      // [E5G-R2] table de jointure
    }
    ggml_prov_state::mm_rec & r = st.mm_sig[key];
    r.calls++;
    r.role = role;
    r.type_name = type_name ? type_name : "?";
    r.M = M; r.N = N; r.K = K;
    r.ms += ms;
    r.layer = layer;

    // agregation par role, en separant recurrent / attention (architecture : il % 4 == 3)
    const char * cls = (layer < 0) ? "hors" : ((layer % 4) == 3 ? "attention" : "recurrent");
    char rkey[224];
    snprintf(rkey, sizeof(rkey), "%s|%s|%s", cls, role, type_name ? type_name : "?");
    ggml_prov_state::mm_rec & rr = st.mm_role[rkey];
    rr.calls++;
    rr.ms += ms;
    rr.role = role;
    rr.type_name = type_name ? type_name : "?";
    rr.M = M; rr.N = N; rr.K = K;
    rr.layer = layer;
}
