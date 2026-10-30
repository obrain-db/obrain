#!/usr/bin/env python3
"""N4-C/D — reference q8_1 FIDELE au noyau CUDA `quantize_q8_1` (quantize.cu:5).

Semantique extraite de la source, ligne par ligne :

    const float xi = ...;                    // fp32
    float amax = fabsf(xi);
    float sum  = xi;
    amax = warp_reduce_max<QK8_1>(amax);     // max sur 32 elements
    sum  = warp_reduce_sum<QK8_1>(sum);      // SOMME des valeurs fp32
    const float  d = amax / 127.0f;
    const int8_t q = amax == 0.0f ? 0 : roundf(xi / d);
    ...
    y[ib].ds = make_half2(d, sum);

POINTS CRITIQUES que ma reference precedente ratait :

  1. L'ARRONDI se fait contre `xi / d` avec `d` en FP32 NON arrondi.
     Ma reference faisait `xi * (1/d_fp16)` avec `d_fp16` = l'echelle APRES conversion fp16.
     Deux sémantiques differentes -> des qs differents.
  2. `roundf` = arrondi a mi-chemin EN S'ELOIGNANT DE ZERO.
     `np.rint` = arrondi a mi-chemin VERS LE PAIR (banker's). Ce n'est PAS equivalent.
  3. `sum` est la somme des xi FP32 (pas des qs). C'est le champ `s` de block_q8_1.
  4. `d` est stocke en fp16 -> la valeur REELLE utilisee par vecdot est d_fp16.
     L'arrondi se fait donc avec d, mais l'application de l'echelle avec d_fp16.
"""
import csv, hashlib, math, os, struct, sys
import numpy as np

BASE = "/home/triviere/exp-artifacts/engine-004-steacy-state"  # corrige plus bas
CAP = "/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/qualification/e7b-f1a-q0-n3nonies/capon"
D = "/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/qualification/e7b-f1a-q0-n4"
os.makedirs(D, exist_ok=True)


def roundf(v):
    """CUDA roundf : arrondi a mi-chemin en s'eloignant de zero."""
    v = float(v)
    return int(math.floor(v + 0.5)) if v >= 0.0 else int(math.ceil(v - 0.5))


def q8_1_native(x):
    """Reference FIDELE au noyau quantize_q8_1. x = 32 floats."""
    x = np.asarray(x, dtype=np.float32)
    amax = float(np.max(np.abs(x)))
    s_sum = float(np.sum(x, dtype=np.float32))
    d = np.float32(amax) / np.float32(127.0)          # fp32, NON arrondi
    if amax == 0.0:
        qs = [0] * 32
        d16 = np.float16(0.0)
    else:
        qs = [roundf(float(np.float32(xi) / d)) for xi in x]
        qs = [max(-128, min(127, q)) for q in qs]
        d16 = np.float16(d)                            # stocke en fp16
    return {
        "amax": amax, "d_fp32": float(d), "d_fp16_bits": int(d16.view(np.uint16)),
        "d_fp16": float(d16), "qs": qs, "sum_qs": int(sum(qs)),
        "s_metadata": s_sum, "s_fp16": float(np.float16(s_sum)),
    }


def q8_1_OLD_reference(x):
    """Ma reference PRECEDENTE (N3quater) : arrondi contre 1/d_fp16, np.rint.
    Conservee pour MESURER l'ecart entre les deux semantiques."""
    x = np.asarray(x, dtype=np.float32)
    amax = float(np.max(np.abs(x)))
    d32 = np.float32(amax) / np.float32(127.0)
    d16 = np.float16(d32)
    id32 = np.float32(1.0) / np.float32(d16) if d16 != 0 else np.float32(0.0)
    qs = np.zeros(32, dtype=np.int8)
    for j in range(16):
        a = int(np.rint(np.float32(x[j]) * id32))
        b = int(np.rint(np.float32(x[16 + j]) * id32))
        qs[j] = max(-128, min(127, a))
        qs[16 + j] = max(-128, min(127, b))
    return {"d_fp16": float(d16), "qs": [int(v) for v in qs]}


# ---------------- tests synthetiques (N4-D) ----------------
def synthetic_tests():
    print("=== N4-D : tests synthetiques q8_1 ===")
    cases = {
        "ALL_ZERO":            [0.0] * 32,
        "SINGLE_NONZERO":      [1.0] + [0.0] * 31,
        "MIXED_POS_NEG":       [1.0, -1.0] * 16,
        "SMALL_NONZERO":       [1e-6, -1e-6] * 16,
        "SATURATION":          [127.0, -127.0] * 16,
        "UNIFORM":             [1.0] * 32,
    }
    ok = True
    for name, vals in cases.items():
        r = q8_1_native(vals)
        nz = sum(1 for q in r["qs"] if q != 0)
        finite = math.isfinite(r["d_fp16"]) and math.isfinite(r["d_fp32"])
        sym = abs(r["d_fp32"] * 127.0 - r["amax"]) < 1e-5 * max(r["amax"], 1e-30)
        print("  %-16s amax=%-11.6g d32=%-11.6g d16_bits=0x%04x qs_nonzero=%-3d sum_qs=%-5d s=%.6g %s"
              % (name, r["amax"], r["d_fp32"], r["d_fp16_bits"], nz, r["sum_qs"], r["s_metadata"],
                 "OK" if (finite and sym) else "ANOMALIE"))
        if not (finite and sym):
            ok = False
        if name == "ALL_ZERO" and nz != 0:
            print("     ECHEC : bloc nul doit donner qs tous nuls")
            ok = False
    # cas de mi-chemin : arrondi a mi-chemin s'eloignant de zero
    print()
    print("  CAS DE MI-CHEMIN (roundf = s'eloigne de zero, np.rint = vers le pair) :")
    half = [0.5 * (127.0 / 127.0), -0.5]
    # construit xi tels que xi/d vaut exactement .5 : d = amax/127, on force amax=127 -> d=1
    xs = [127.0] + [0.5, -0.5, 1.5, -1.5, 2.5, -2.5] + [0.0] * 25
    r = q8_1_native(xs)
    got = r["qs"][1:7]
    want = [1, -1, 2, -2, 3, -3]      # roundf : .5 -> 1, -.5 -> -1, 1.5 -> 2, 2.5 -> 3
    rint = [int(np.rint(v)) for v in (0.5, -0.5, 1.5, -1.5, 2.5, -2.5)]
    print("    d = %.6g" % r["d_fp32"])
    print("    xi/d  = 0.5 -0.5 1.5 -1.5 2.5 -2.5")
    print("    roundf  -> %s   <- attendu (noyau CUDA)" % got)
    print("    np.rint -> %s   <- ma reference precedente" % rint)
    if got != want:
        print("    ECHEC : roundf ne donne pas %s" % want)
        ok = False
    else:
        print("    ROUNDING_HALF_CASES_OK=YES")
    if got == rint:
        print("    (les deux coincident sur ce cas)")
    else:
        print("    -> LES DEUX SEMANTIQUES DIVERGENT SUR LES CAS A MI-CHEMIN")
    return ok


if __name__ == "__main__":
    ok = synthetic_tests()
    print()
    print("  Q8_REFERENCE_UNIT_TESTS=%s" % ("PASS" if ok else "FAIL"))

    # couverture complete 6144 et 160/192 blocs
    print()
    print("=== couverture des blocs sur les captures corrigees ===")
    rows = list(csv.DictReader(open(CAP + "/GRAPH_CAPTURE_MANIFEST.csv", newline="")))
    acts = {}
    for r in rows:
        if r["kind"] == "activation":
            acts[(r["epoch"], r["weight_name"])] = np.fromfile(r["path"], dtype=np.float32)
    for (e, name), a in sorted(acts.items()):
        K = a.size
        nb = K // 32
        div = 0
        for b in range(nb):
            x = a[b * 32:(b + 1) * 32]
            rn = q8_1_native(x)
            ro = q8_1_OLD_reference(x)
            if rn["qs"] != ro["qs"]:
                div += 1
        print("  ep%s %-28s K=%-5d blocs_q8=%-4d  blocs ou ANCIENNE et NOUVELLE reference DIVERGENT = %d"
              % (e, name, K, nb, div))
