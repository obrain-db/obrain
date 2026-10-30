#!/usr/bin/env python3
# ENGINE-006C : substitution du decodage ternaire par la variante a force reduite (V1).
#
# Portee STRICTEMENT limitee a vec_dot_tq2_g128_q8_1 (qtype=tq2_g128 uniquement : c'est la seule
# fonction de vecdotq.cuh dont le corps contient cette sequence d'extraction). Aucun autre chemin
# du moteur n'est touche.
#
# Fidélité : le mapping produit est identique octet pour octet (0->0xFF, 1->0x00, 2->0x01, 3->0x01),
# verifie bit a bit sur 524288 blocs / 16 777 216 symboles dans le microbench ENGINE-006C.
#
# Bascule : GGML_E6C_DECODE_SR=1 active la variante ; sans la variable, le code d'origine reste seul.
# Usage : python3 apply-decode-sr.py <ggml/src/ggml-cuda/vecdotq.cuh>
import sys, os

OLD = """    int vi[8];
#pragma unroll
    for (int j = 0; j < 8; ++j) {
        const int b = qs[j];
        const int c0 = (b >> 0) & 3;
        const int c1 = (b >> 2) & 3;
        const int c2 = (b >> 4) & 3;
        const int c3 = (b >> 6) & 3;
        vi[j] = ((c0 > 2 ? 1 : c0 - 1) & 0xFF)       |
                (((c1 > 2 ? 1 : c1 - 1) & 0xFF) <<  8) |
                (((c2 > 2 ? 1 : c2 - 1) & 0xFF) << 16) |
                (((c3 > 2 ? 1 : c3 - 1) & 0xFF) << 24);
    }"""

NEW = """    int vi[8];
    // [ENGINE-006C] variante a force reduite, selectionnee par un drapeau __constant__ pose par
    // l'hote (ggml_cuda_mul_mat_vec_q lit GGML_E6C_DECODE_SR une fois par appel). Les deux
    // formulations sont compilees : aucune modification du chemin chaud hors de cette fonction.
    // Mapping identique (0->0xFF, 1->0x00, 2->0x01, 3->0x01) sans comparaison :
    //   is0 = (c-1)>>31 -> -1 si c==0 ; ge2 = (1-c)>>31 -> -1 si c>=2
    //   valeur = (0xFF & is0) | (0x01 & ge2)
    if (g_e6c_decode_sr != 0) {
#pragma unroll
        for (int j = 0; j < 8; ++j) {
            const unsigned b = (unsigned) qs[j];
            const int c0 = (int) ((b >> 0) & 3u);
            const int c1 = (int) ((b >> 2) & 3u);
            const int c2 = (int) ((b >> 4) & 3u);
            const int c3 = (int) ((b >> 6) & 3u);
            const int v0 = (0xFF & ((c0 - 1) >> 31)) | (0x01 & ((1 - c0) >> 31));
            const int v1 = (0xFF & ((c1 - 1) >> 31)) | (0x01 & ((1 - c1) >> 31));
            const int v2 = (0xFF & ((c2 - 1) >> 31)) | (0x01 & ((1 - c2) >> 31));
            const int v3 = (0xFF & ((c3 - 1) >> 31)) | (0x01 & ((1 - c3) >> 31));
            vi[j] = (v0 & 0xFF) | ((v1 & 0xFF) << 8) | ((v2 & 0xFF) << 16) | ((v3 & 0xFF) << 24);
        }
    } else {
#pragma unroll
        for (int j = 0; j < 8; ++j) {
            const int b = qs[j];
            const int c0 = (b >> 0) & 3;
            const int c1 = (b >> 2) & 3;
            const int c2 = (b >> 4) & 3;
            const int c3 = (b >> 6) & 3;
            vi[j] = ((c0 > 2 ? 1 : c0 - 1) & 0xFF)       |
                    (((c1 > 2 ? 1 : c1 - 1) & 0xFF) <<  8) |
                    (((c2 > 2 ? 1 : c2 - 1) & 0xFF) << 16) |
                    (((c3 > 2 ? 1 : c3 - 1) & 0xFF) << 24);
        }
    }"""

def main():
    if len(sys.argv) != 2:
        raise SystemExit("usage: apply-decode-sr.py <vecdotq.cuh>")
    p = sys.argv[1]
    t = open(p, encoding="utf-8").read()
    if "[ENGINE-006C]" in t:
        raise SystemExit("DEJA PATCHE")
    n = t.count(OLD)
    if n != 1:
        raise SystemExit("ANCRE INTROUVABLE OU AMBIGUE : %d occurrence(s)" % n)
    t = t.replace(OLD, NEW, 1)
    # getenv : inclure <cstdlib> si absent
    if "#include <cstdlib>" not in t:
        t = t.replace("#pragma once",
                      "#pragma once\n\n#include <cstdlib>\n\n"
                      "// [ENGINE-006C] drapeau de selection du decodage a force reduite.\n"
                      "__constant__ int g_e6c_decode_sr;", 1)
    open(p + ".tmp", "w", encoding="utf-8").write(t)
    os.replace(p + ".tmp", p)
    print("ECRIT %s" % p)

if __name__ == "__main__":
    main()
