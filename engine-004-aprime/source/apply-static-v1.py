#!/usr/bin/env python3
# ENGINE-006C-R2 : V1 en substitution STATIQUE, sans aucun commutateur runtime.
#
# Remplace uniquement l'expression de mapping ternaire dans vec_dot_tq2_g128_q8_1.
# Aucun if, aucun drapeau, aucune variable d'environnement, aucune table, aucun chemin alternatif.
# La fonction contient uniquement V1.
#
# Usage : python3 apply-static-v1.py <ggml/src/ggml-cuda/vecdotq.cuh>
import sys, os

OLD = """        const int b = qs[j];
        const int c0 = (b >> 0) & 3;
        const int c1 = (b >> 2) & 3;
        const int c2 = (b >> 4) & 3;
        const int c3 = (b >> 6) & 3;
        vi[j] = ((c0 > 2 ? 1 : c0 - 1) & 0xFF)       |
                (((c1 > 2 ? 1 : c1 - 1) & 0xFF) <<  8) |
                (((c2 > 2 ? 1 : c2 - 1) & 0xFF) << 16) |
                (((c3 > 2 ? 1 : c3 - 1) & 0xFF) << 24);"""

NEW = """        const unsigned b = (unsigned) qs[j];
        const int c0 = (int) ((b >> 0) & 3u);
        const int c1 = (int) ((b >> 2) & 3u);
        const int c2 = (int) ((b >> 4) & 3u);
        const int c3 = (int) ((b >> 6) & 3u);
        // [ENGINE-006C-R2] force reduite : (c>2 ? 1 : c-1) sans comparaison.
        const int v0 = (0xFF & ((c0 - 1) >> 31)) | (0x01 & ((1 - c0) >> 31));
        const int v1 = (0xFF & ((c1 - 1) >> 31)) | (0x01 & ((1 - c1) >> 31));
        const int v2 = (0xFF & ((c2 - 1) >> 31)) | (0x01 & ((1 - c2) >> 31));
        const int v3 = (0xFF & ((c3 - 1) >> 31)) | (0x01 & ((1 - c3) >> 31));
        vi[j] = (v0 & 0xFF) | ((v1 & 0xFF) << 8) | ((v2 & 0xFF) << 16) | ((v3 & 0xFF) << 24);"""

def main():
    if len(sys.argv) != 2:
        raise SystemExit("usage: apply-static-v1.py <vecdotq.cuh>")
    p = sys.argv[1]
    t = open(p, encoding="utf-8").read()
    if "[ENGINE-006C-R2]" in t:
        raise SystemExit("DEJA PATCHE")
    n = t.count(OLD)
    if n != 1:
        raise SystemExit("ANCRE INTROUVABLE OU AMBIGUE : %d occurrence(s)" % n)
    open(p + ".tmp", "w", encoding="utf-8").write(t.replace(OLD, NEW, 1))
    os.replace(p + ".tmp", p)
    print("ECRIT %s" % p)

if __name__ == "__main__":
    main()
