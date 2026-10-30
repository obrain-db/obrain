#!/usr/bin/env python3
# ENGINE-005G : pose les compteurs de recensement au POINT DE DISPATCH de ggml_cuda_mul_mat.
# La classification reproduit exactement l'ordre de priorite du code (mmvf > mmf > mmvq > mmq > other).
# Aucun effet sur l'execution : seuls des compteurs atomiques sont incrementes.
# Usage : python3 apply-census.py <ggml/src/ggml-cuda/ggml-cuda.cu>
import sys, os

ANCHOR_INC = '#include "ggml-cuda.h"\n'
REPL_INC = '#include "ggml-cuda.h"\n\n// [ENGINE-005G] recensement des routes noyau. Inerte sans GGML_DISPATCH_CENSUS=1.\n#include "ggml-cuda/ggml-cuda-census.cuh"\n'

ANCHOR_DISPATCH = """    if (ggml_cuda_should_use_mmvf(src0->type, cc, src0->ne, src0->nb, ne11)) {"""

REPL_DISPATCH = """    // [ENGINE-005H] recensement des PREDICATS REELS de la cascade, plus la signature de forme.
    // Les quatre predicats sont evalues independamment (et non en cascade) : c'est precisement
    // ce que la porte « le chemin alternatif est-il structurellement supporte ? » demande.
    if (ggml_census_enabled()) {
        const bool p_mmvf = ggml_cuda_should_use_mmvf(src0->type, cc, src0->ne, src0->nb, ne11);
        const bool p_mmf  = ggml_cuda_should_use_mmf(src0->type, cc, warp_size, src0->ne, src0->nb, ne11, /*mul_mat_id =*/ false);
        const bool p_mmvq = ggml_cuda_should_use_mmvq(src0->type, cc, ne11);
        const bool p_mmq  = ggml_cuda_should_use_mmq(src0->type, cc, ne11, /*n_experts =*/ 0);
        const int  pred   = (p_mmvf ? 1 : 0) | (p_mmf ? 2 : 0) | (p_mmvq ? 4 : 0) | (p_mmq ? 8 : 0);
        const int  c_path = p_mmvf ? GGML_CENSUS_MMVF : p_mmf ? GGML_CENSUS_MMF : p_mmvq ? GGML_CENSUS_MMVQ
                          : p_mmq  ? GGML_CENSUS_MMQ  : GGML_CENSUS_OTHER;
        const int64_t d[6] = { src0->ne[0], src0->ne[1], src0->ne[2], src0->ne[3], src1->ne[1], src1->ne[2] };
        ggml_census_record_sig(src0, pred, c_path, (int) src0->type, d, ggml_get_name(src0));
    }

    if (ggml_cuda_should_use_mmvf(src0->type, cc, src0->ne, src0->nb, ne11)) {"""

def fr(t, a, r, n):
    c = t.count(a)
    if c != 1:
        raise SystemExit("ANCRE INTROUVABLE OU AMBIGUE (%s) : %d" % (n, c))
    return t.replace(a, r, 1)

def main():
    if len(sys.argv) != 2:
        raise SystemExit("usage: apply-census.py <ggml-cuda.cu>")
    p = sys.argv[1]
    t = open(p, encoding="utf-8").read()
    if "[ENGINE-005G]" in t:
        raise SystemExit("DEJA PATCHE")
    t = fr(t, ANCHOR_INC, REPL_INC, "include")
    t = fr(t, ANCHOR_DISPATCH, REPL_DISPATCH, "point de dispatch")
    open(p + ".tmp", "w", encoding="utf-8").write(t)
    os.replace(p + ".tmp", p)
    print("ECRIT %s" % p)

if __name__ == "__main__":
    main()
