#!/usr/bin/env python3
# ENGINE-006A : decomposition causale par composant (quantize / mmvq) du chemin tq2_g128 cible.
import sys, os

HDR = r"""#include <cstdlib>
#include <cstdio>
#include <cstring>
#include <atomic>

namespace e6_dup {
    // mode : 0 = BASE, 1 = Q+ (quantize en plus), 2 = M+ (mmvq en plus), 3 = QM+ (les deux)
    inline int mode() {
        static const int m = []() { const char * e = getenv("GGML_E6_COMPONENT"); return e ? atoi(e) : 0; }();
        return m;
    }
    inline bool by_name() {
        static const bool b = getenv("GGML_E6_TARGET_BY_NAME") != nullptr;
        return b;
    }
    inline std::atomic<unsigned long long> & c_nat_t()  { static std::atomic<unsigned long long> v{0}; return v; }
    inline std::atomic<unsigned long long> & c_nat_n()  { static std::atomic<unsigned long long> v{0}; return v; }
    inline std::atomic<unsigned long long> & c_ext_t()  { static std::atomic<unsigned long long> v{0}; return v; }
    inline std::atomic<unsigned long long> & c_ext_n()  { static std::atomic<unsigned long long> v{0}; return v; }
}
"""
BODY = r"""    // [ENGINE-006A] sequence native = quantize_row_q8_1_cuda(src1) PUIS le noyau MMVQ.
    // Decomposition causale : les repetitions sont des composants distincts, chacun neutre.
    //   mode 0 BASE : quantize ; mmvq
    //   mode 1 Q+   : extra_quantize ; quantize ; mmvq        (la quantize native reecrit tout)
    //   mode 2 M+   : quantize ; mmvq ; extra_mmvq            (meme q8_1, meme dst)
    //   mode 3 QM+  : extra_quantize ; quantize ; mmvq ; extra_mmvq
    static const bool e6_by_name = e6_dup::by_name();
    const bool e6_role = e6_by_name && src0->name[0] != '\0' && strstr(src0->name, "attn_gate") != nullptr;
    const bool e6_candidate = (src0->type == GGML_TYPE_TQ2_G128) && (ncols_dst == 1) && (ids == nullptr);
    const bool e6_is_target = e6_candidate && e6_role;

    auto e6_quantize = [&]() {
        quantize_row_q8_1_cuda(src1_d, nullptr, src1_q8_1.get(), src0->type, ne10, s11_obs, s12_obs, s13_obs,
                               ne10_padded, ne11, ne12, ne13, stream);
    };
    auto e6_mmvq = [&]() {
        mul_mat_vec_q_switch_type(
            src0->data, src0->type, src1_q8_1.get(), ids_d, fusion_local, dst_d, ne00,
            ne01,              ncols_dst,     s01, stride_col_y,     stride_col_dst,
            ne02, nchannels_y, nchannels_dst, s02, stride_channel_y, stride_channel_dst,
            ne03,              ne3,           s03, s13,              s3,               ids_stride, stream);
    };

    // Comptage NATIF : inconditionnel, sur toute variante, cible ou non.
    if (e6_is_target) { e6_dup::c_nat_t().fetch_add(1, std::memory_order_relaxed); }
    else              { e6_dup::c_nat_n().fetch_add(1, std::memory_order_relaxed); }

    const int e6_mode = e6_dup::mode();

    if (e6_is_target && (e6_mode == 1 || e6_mode == 3)) {
        e6_dup::c_ext_t().fetch_add(1, std::memory_order_relaxed);
        e6_quantize();                       // extra quantize AVANT la native (neutre : reecrite ensuite)
    } else if (!e6_is_target && e6_mode != 0) {
        e6_dup::c_ext_n().fetch_add(1, std::memory_order_relaxed);
    }

    e6_quantize();
    e6_mmvq();

    if (e6_is_target && (e6_mode == 2 || e6_mode == 3)) {
        e6_dup::c_ext_t().fetch_add(1, std::memory_order_relaxed);
        e6_mmvq();                           // extra mmvq APRES la native (meme q8_1, meme dst)
    } else if (!e6_is_target && e6_mode != 0) {
        e6_dup::c_ext_n().fetch_add(1, std::memory_order_relaxed);
    }
}

extern "C" void e6_dup_report(void) {
    std::fprintf(stderr, "E6_MODE=%d\n", e6_dup::mode());
    std::fprintf(stderr, "E6_TARGET_NATIVE_MMVQ_OBSERVATIONS=%llu\n",
                 (unsigned long long) e6_dup::c_nat_t().load(std::memory_order_relaxed));
    std::fprintf(stderr, "E6_NON_TARGET_NATIVE_OBSERVATIONS=%llu\n",
                 (unsigned long long) e6_dup::c_nat_n().load(std::memory_order_relaxed));
    std::fprintf(stderr, "E6_TARGET_EXTRA_FULL_SEQUENCE_EXECUTIONS=%llu\n",
                 (unsigned long long) e6_dup::c_ext_t().load(std::memory_order_relaxed));
    std::fprintf(stderr, "E6_NON_TARGET_ROLE_EXTRA_EXECUTIONS=%llu\n",
                 (unsigned long long) e6_dup::c_ext_n().load(std::memory_order_relaxed));
    std::fflush(stderr);
}
"""

ANCHOR_INC = '#include "mmvq.cuh"\n'
REPL_INC = '#include "mmvq.cuh"\n\n// [ENGINE-006A] decomposition causale par composant.\n' + HDR

ANCHOR_STRIDES = "    const int64_t ne10_padded = GGML_PAD(ne10, MATRIX_ROW_PADDING);"
REPL_STRIDES = "    int64_t s11_obs = 0, s12_obs = 0, s13_obs = 0;   // [ENGINE-006A] strides de la sequence native\n" + ANCHOR_STRIDES

ANCHOR_QUANT = """        const int64_t s13 = src1->nb[3] / ts_src1;
        quantize_row_q8_1_cuda(src1_d, nullptr, src1_q8_1.get(), src0->type, ne10, s11, s12, s13, ne10_padded, ne11, ne12, ne13, stream);"""
REPL_QUANT = """        const int64_t s13 = src1->nb[3] / ts_src1;
        s11_obs = s11; s12_obs = s12; s13_obs = s13;   // [ENGINE-006A]
        quantize_row_q8_1_cuda(src1_d, nullptr, src1_q8_1.get(), src0->type, ne10, s11, s12, s13, ne10_padded, ne11, ne12, ne13, stream);"""

ANCHOR_DISPATCH = """    mul_mat_vec_q_switch_type(
        src0->data, src0->type, src1_q8_1.get(), ids_d, fusion_local, dst_d, ne00,
        ne01,              ncols_dst,     s01, stride_col_y,     stride_col_dst,
        ne02, nchannels_y, nchannels_dst, s02, stride_channel_y, stride_channel_dst,
        ne03,              ne3,           s03, s13,              s3,               ids_stride, stream);
}"""

def fr(t, a, r, n):
    c = t.count(a)
    if c != 1:
        raise SystemExit("ANCRE INTROUVABLE OU AMBIGUE (%s) : %d" % (n, c))
    return t.replace(a, r, 1)

def main():
    if len(sys.argv) != 2:
        raise SystemExit("usage: apply-dup.py <mmvq.cu>")
    p = sys.argv[1]
    t = open(p, encoding="utf-8").read()
    if "[ENGINE-006A]" in t:
        raise SystemExit("DEJA PATCHE")
    t = fr(t, ANCHOR_INC, REPL_INC, "include")
    t = fr(t, ANCHOR_STRIDES, REPL_STRIDES, "strides memorises")
    t = fr(t, ANCHOR_QUANT, REPL_QUANT, "capture des strides")
    t = fr(t, ANCHOR_DISPATCH, BODY, "sequence decomposable")
    open(p + ".tmp", "w", encoding="utf-8").write(t)
    os.replace(p + ".tmp", p)
    print("ECRIT %s" % p)

if __name__ == "__main__":
    main()
