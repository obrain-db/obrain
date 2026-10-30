// ggml-cuda-census.cuh — ENGINE-005H : domaine de dispatch EXACT.
//
// Au point de selection de ggml_cuda_mul_mat, on enregistre non seulement le chemin retenu mais
// les QUATRE PREDICATS REELS utilises par la cascade, plus la signature de forme et le nom du poids.
//
// Cle = hash(pointeur du poids, qtype, masque de predicats, dimensions consultees par la cascade)
//   masque : bit0 can_mmvf, bit1 can_mmf, bit2 can_mmvq, bit3 can_mmq
//   dimensions : src0 ne[0..3], src1 ne[1..2] (les seules consultees par les predicats)
//
// Aucune allocation, aucun printf, aucune synchronisation dans le chemin chaud : deux atomiques
// relaxed par appel. Lecture unique apres la fenetre.
//
// Inerte sans GGML_DISPATCH_CENSUS=1.

#pragma once

#include <atomic>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>

enum ggml_census_path {
    GGML_CENSUS_MMVF   = 0,
    GGML_CENSUS_MMF    = 1,
    GGML_CENSUS_MMVQ   = 2,
    GGML_CENSUS_MMQ    = 3,
    GGML_CENSUS_OTHER  = 4,
    GGML_CENSUS_PATH_N = 5,
};

inline const char * ggml_census_path_name(int p) {
    switch (p) {
        case GGML_CENSUS_MMVF:  return "MMVF";
        case GGML_CENSUS_MMF:   return "MMF";
        case GGML_CENSUS_MMVQ:  return "MMVQ";
        case GGML_CENSUS_MMQ:   return "MMQ";
        default:                return "OTHER";
    }
}

struct ggml_census_sig {
    uint64_t    key;          // cle complete (voir plus haut)
    uint64_t    count;
    const void * wptr;
    char        name[64];
    uint32_t    qtype;
    uint8_t     pred;         // masque des predicats
    uint8_t     path;         // chemin retenu
    int64_t     ne00, ne01, ne02, ne03;   // dimensions du poids
    int64_t     s1ne1, s1ne2;             // dimensions de l'activation
};

struct ggml_census_state {
    static constexpr int CAP = 16384;
    std::atomic<uint64_t> key[CAP];     // 0 = libre
    std::atomic<uint64_t> count[CAP];
    std::atomic<const void *> wptr[CAP];
    std::atomic<uint32_t> meta[CAP];    // qtype(16) | pred(8) | path(8)
    int64_t dims[CAP][6];               // ecrits une seule fois, avant publication de la cle
    char    name[CAP][64];
};

inline ggml_census_state & ggml_census() {
    static ggml_census_state s;
    return s;
}

inline bool ggml_census_enabled() {
    static const bool on = getenv("GGML_DISPATCH_CENSUS") != nullptr;
    return on;
}

// hash bon marche des dimensions consultees par la cascade
static inline uint64_t ggml_census_dim_hash(const int64_t * d, int n) {
    uint64_t h = 1469598103934665603ull;
    for (int i = 0; i < n; ++i) {
        h ^= (uint64_t) d[i];
        h *= 1099511628211ull;
    }
    return h;
}

// wptr : pointeur du tenseur de poids (src0). pred : masque des predicats.
// path : chemin retenu. d[6] : ne00,ne01,ne02,ne03,s1ne1,s1ne2. qtype : type du poids.
// name : nom du tenseur de poids (peut etre nullptr).
inline void ggml_census_record_sig(const void * wptr, int pred, int path, int qtype,
                                   const int64_t * d, const char * name) {
    if (!ggml_census_enabled() || wptr == nullptr) {
        return;
    }
    ggml_census_state & st = ggml_census();

    const uint64_t dh = ggml_census_dim_hash(d, 6);
    const uint64_t k  = ((uint64_t) (uintptr_t) wptr << 23)
                      ^ (dh * 0x9E3779B97F4A7C15ull)
                      ^ ((uint64_t) qtype << 12) ^ ((uint64_t) (pred & 0xF) << 8) ^ (uint64_t) (path & 0xF);
    const uint64_t kk = k ? k : 1;

    uint64_t h = kk;
    for (int probe = 0; probe < 128; ++probe) {
        const uint64_t i = (h + (uint64_t) probe) & (ggml_census_state::CAP - 1);
        const uint64_t cur = st.key[i].load(std::memory_order_relaxed);
        if (cur == kk) {
            st.count[i].fetch_add(1, std::memory_order_relaxed);
            return;
        }
        if (cur == 0) {
            // remplir les champs annexes AVANT de publier la cle
            st.wptr[i].store(wptr, std::memory_order_relaxed);
            st.meta[i].store((uint32_t) ((qtype & 0xFFFF) | ((pred & 0xFF) << 16) | ((path & 0xFF) << 24)),
                             std::memory_order_relaxed);
            for (int j = 0; j < 6; ++j) {
                st.dims[i][j] = d[j];
            }
            if (name != nullptr) {
                snprintf(st.name[i], sizeof(st.name[i]), "%s", name);
            } else {
                st.name[i][0] = '\0';
            }
            uint64_t expect = 0;
            if (st.key[i].compare_exchange_strong(expect, kk, std::memory_order_relaxed)) {
                st.count[i].fetch_add(1, std::memory_order_relaxed);
                return;
            }
            if (st.key[i].load(std::memory_order_relaxed) == kk) {
                st.count[i].fetch_add(1, std::memory_order_relaxed);
                return;
            }
        }
    }
}
