#!/usr/bin/env python3
# ENGINE-006C : pose le drapeau __constant__ g_e6c_decode_sr depuis GGML_E6C_DECODE_SR,
# une fois par appel de ggml_cuda_mul_mat_vec_q. Cout : une lecture de variable statique.
# Usage : python3 apply-sr-flag.py <ggml/src/ggml-cuda/mmvq.cu>
import sys, os

ANCHOR = """    GGML_TENSOR_BINARY_OP_LOCALS;

    cudaStream_t stream = ctx.stream();"""

REPL = """    GGML_TENSOR_BINARY_OP_LOCALS;

    cudaStream_t stream = ctx.stream();

    // [ENGINE-006C] selection du decodage ternaire a force reduite (diagnostic).
    // Sans GGML_E6C_DECODE_SR, le drapeau vaut 0 et le chemin d'origine est seul emprunte.
    {
        static const int e6c_sr = (getenv("GGML_E6C_DECODE_SR") != nullptr) ? 1 : 0;
        static int e6c_sr_set = -1;
        if (e6c_sr_set != e6c_sr) {
            cudaMemcpyToSymbol(g_e6c_decode_sr, &e6c_sr, sizeof(int));
            e6c_sr_set = e6c_sr;
        }
    }"""

def main():
    if len(sys.argv) != 2:
        raise SystemExit("usage: apply-sr-flag.py <mmvq.cu>")
    p = sys.argv[1]
    t = open(p, encoding="utf-8").read()
    if "[ENGINE-006C]" in t:
        raise SystemExit("DEJA PATCHE")
    n = t.count(ANCHOR)
    if n != 1:
        raise SystemExit("ANCRE INTROUVABLE OU AMBIGUE : %d" % n)
    t = t.replace(ANCHOR, REPL, 1)
    if "#include <cstdlib>" not in t:
        t = t.replace('#include "mmvq.cuh"', '#include "mmvq.cuh"\n\n#include <cstdlib>   // [ENGINE-006C] getenv', 1)
    open(p + ".tmp", "w", encoding="utf-8").write(t)
    os.replace(p + ".tmp", p)
    print("ECRIT %s" % p)

if __name__ == "__main__":
    main()
