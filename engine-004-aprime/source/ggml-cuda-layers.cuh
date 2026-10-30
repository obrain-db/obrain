// ggml-cuda-layers.cuh — ENGINE-005C : attribution du temps GPU par COUCHE.
//
// Ce fichier est inclus UNIQUEMENT par la bibliotheque (ggml-cuda.cu). Le harnais ne doit PAS
// l'inclure : sinon l'etat statique existerait en double et le vidage lirait une copie vide
// (defaut constate : « LAYER_TIMING_ENABLED=YES » sans aucune sortie E5C).
// Le harnais n'utilise que deux points d'entree exportes, declares a la main :
//   void  ggml_layer_t_dump_now(void);
//   bool  ggml_layer_timing_enabled(void);
//
// Temps mesures par paires d'evenements CUDA, ranges par couche extraite du nom `..._l<k>`.
// Inerte sans GGML_LAYER_TIMING=1.

#pragma once

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <cstdint>
#include <map>
#include <string>
#include <vector>
#include <algorithm>

struct ggml_layer_rec {
    double             ms = 0.0;
    unsigned long long n  = 0;
    std::map<std::string, double>             op_ms;
    std::map<std::string, unsigned long long> op_n;
};

struct ggml_layer_state {
    std::map<int, ggml_layer_rec>            layer;   // -1 = hors couche
    std::vector<int>                         attention;
    unsigned long long                       tokens = 0;
};

// Etat reel du programme (le mot-cle `static` porte sur la DECLARATION, pas sur la definition
// fournie par la bibliotheque : une seule instance pour tout le processus).
extern ggml_layer_state ggml_layer_state_g;

// Predicat interne (la bibliotheque l'utilise). L'export C porte un autre nom de liaison.
inline bool ggml_layer_timing_env_on() {
    static const bool on = getenv("GGML_LAYER_TIMING") != nullptr;
    return on;
}

// Indice de couche depuis un nom de nœud : dernier suffixe "_l<entier>".
inline int ggml_layer_from_name(const char * name) {
    if (name == nullptr) {
        return -1;
    }
    int res = -1;
    const char * p = name;
    while ((p = strstr(p, "_l")) != nullptr) {
        const char * q = p + 2;
        if (*q >= '0' && *q <= '9') {
            res = atoi(q);
        }
        p += 2;
    }
    return res;
}

inline void ggml_layer_acc(int layer, const char * op, double ms) {
    ggml_layer_rec & r = ggml_layer_state_g.layer[layer];
    r.ms += ms;
    r.n  += 1;
    if (op != nullptr) {
        r.op_ms[op] += ms;
        r.op_n[op]  += 1;
    }
}

inline void ggml_layer_token() {
    ggml_layer_state_g.tokens += 1;
}

inline bool ggml_layer_is_attention(int layer) {
    for (int v : ggml_layer_state_g.attention) {
        if (v == layer) {
            return true;
        }
    }
    return false;
}

inline void ggml_layer_set_attention_from_env() {
    const char * e = getenv("GGML_LAYER_TIMING_ATTENTION");
    if (e == nullptr) {
        return;
    }
    ggml_layer_state_g.attention.clear();
    const char * p = e;
    while (*p != '\0') {
        char * end = nullptr;
        const long k = strtol(p, &end, 10);
        if (end == p) {
            break;
        }
        ggml_layer_state_g.attention.push_back((int) k);
        p = end;
        while (*p == ',' || *p == ' ') {
            ++p;
        }
    }
}
