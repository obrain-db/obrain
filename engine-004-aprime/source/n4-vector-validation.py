#!/usr/bin/env python3
"""N4 — validation VECTORIELLE complete des trois tenseurs.

Selection FIGEE avant consultation des erreurs : toutes les lignes du tenseur.
Budget preenregistre, inchange :
    RELATIVE_L2 <= 1e-3
    COSINE      >= 0.9999
    MAX_ABS     <= 1e-2 * max_abs(native_mm_base)

Reference : semantique q8_1 du noyau (roundf, d fp32), table ternaire du noyau.
"""
import csv, json, math, os, struct, sys
import numpy as np

M = "/home/triviere/projects/models/Qwen3.8-27B-OBRAIN-v3.gguf"
DUMP = "/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/qualification/e7b-f1a/gguf_dump_cible.json"
CAP = "/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/qualification/e7b-f1a-q0-n3nonies/capon"
D = "/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/qualification/e7b-f1a-q0-n4"

TERNARY = {0: -1, 1: 0, 2: 1, 3: 1}
TARGETS = ["blk.20.ffn_up.weight", "blk.16.attn_gate.weight", "blk.15.attn_output.weight"]


def roundf(v):
    v = float(v)
    return int(math.floor(v + 0.5)) if v >= 0.0 else int(math.ceil(v - 0.5))


def q8_new(x):
    x = np.asarray(x, dtype=np.float32)
    amax = float(np.max(np.abs(x)))
    if amax == 0.0:
        return 0.0, [0] * 32
    d = np.float32(amax) / np.float32(127.0)
    qs = [max(-128, min(127, roundf(np.float32(xi) / d))) for xi in x]
    return float(np.float16(d)), qs


def gb4(qs, j):
    u = (int(qs[4*j]) & 0xFF) | ((int(qs[4*j+1]) & 0xFF) << 8) \
        | ((int(qs[4*j+2]) & 0xFF) << 16) | ((int(qs[4*j+3]) & 0xFF) << 24)
    return u - (1 << 32) if u >= (1 << 31) else u


def dp4a(a, b):
    acc = 0
    for k in range(4):
        va = (a >> (8*k)) & 0xFF
        va = va - 256 if va >= 128 else va
        vb = (b >> (8*k)) & 0xFF
        vb = vb - 256 if vb >= 128 else vb
        acc += va * vb
    return acc


def decode_block(qs32):
    return [TERNARY[(int(qs32[j]) >> (2*p)) & 3] for j in range(32) for p in range(4)]


def row_value(wb, act, K):
    nblk = K // 128
    tot = np.float32(0.0)
    for kbx in range(nblk):
        qs32 = wb[kbx*34:kbx*34+32]
        d16 = struct.unpack("<e", wb[kbx*34+32:kbx*34+34])[0]
        w = decode_block(qs32)
        r = np.float32(0.0)
        for iqs in range(4):
            dq, qs8 = q8_new(act[kbx*128 + 32*iqs: kbx*128 + 32*iqs + 32])
            s = 0
            for j in range(8):
                vi = 0
                for p in range(4):
                    vi |= (int(w[32*iqs + 4*j + p]) & 0xFF) << (8*p)
                s += dp4a(vi, gb4(qs8, j))
            r = np.float32(r + np.float32(np.float32(d16) * np.float32(dq) * np.float32(s)))
        tot = np.float32(tot + r)
    return float(tot)


def main():
    d = json.load(open(DUMP))
    by = {t["name"]: t for t in d["tensors"]}
    DATA = (d["data_start"] + 31) // 32 * 32
    rows = list(csv.DictReader(open(CAP + "/GRAPH_CAPTURE_MANIFEST.csv", newline="")))
    act, bo = {}, {}
    for r in rows:
        a = np.fromfile(r["path"], dtype=np.float32)
        (act if r["kind"] == "activation" else bo)[(r["epoch"], r["weight_name"])] = a

    f = open(M, "rb")
    vec_rows = []
    summary = []
    for name in TARGETS:
        t = by[name]
        K, N = t["dims"][0], t["dims"][1]
        stride = (K // 128) * 34
        key = ("0", name)
        if key not in act:
            print("  %s : pas d activation ep0" % name)
            continue
        a = act[key]
        native = bo[key]
        assert native.size == N, (native.size, N)
        print("=== %s  K=%d  N=%d  -> reconstruction des %d lignes ===" % (name, K, N, N))
        ref = np.empty(N, dtype=np.float64)
        for row in range(N):
            f.seek(DATA + t["offset"] + row * stride)
            wb = f.read(stride)
            ref[row] = row_value(wb, a, K)
        nat = native.astype(np.float64)
        err = ref - nat
        max_abs = float(np.abs(err).max())
        mean_abs = float(np.abs(err).mean())
        rel_l2 = float(np.linalg.norm(err) / np.linalg.norm(nat)) if np.linalg.norm(nat) > 0 else float("inf")
        cos = float(np.dot(ref, nat) / (np.linalg.norm(ref) * np.linalg.norm(nat)))
        nat_amax = float(np.abs(nat).max())
        budget_max_abs = 1e-2 * nat_amax
        ok = (rel_l2 <= 1e-3) and (cos >= 0.9999) and (max_abs <= budget_max_abs)
        print("  MAX_ABS=%.6g  (budget %.6g)   MEAN_ABS=%.6g" % (max_abs, budget_max_abs, mean_abs))
        print("  RELATIVE_L2=%.6g  (budget 1e-3)      COSINE=%.10f  (budget 0.9999)" % (rel_l2, cos))
        print("  -> %s" % ("PASS" if ok else "FAIL"))
        print()
        summary.append((name, K, N, max_abs, mean_abs, rel_l2, cos, nat_amax, budget_max_abs, ok))
    f.close()

    with open(D + "/N4_VECTOR_VALIDATION.csv", "w") as fo:
        fo.write("weight_name,K,N,max_abs,mean_abs,relative_l2,cosine,native_amax,budget_max_abs,verdict\n")
        for s in summary:
            fo.write(",".join(str(x) for x in s) + "\n")

    print("=== SYNTHESE ===")
    for s in summary:
        print("  %-28s N=%-6d rel_l2=%.4g cos=%.8f  %s" % (s[0], s[2], s[5], s[6], "PASS" if s[9] else "FAIL"))
    allok = all(s[9] for s in summary)
    print()
    print("  N4_NATIVE_REFERENCE_VALIDATION=%s" % ("PASS" if allok else "FAIL"))
    print("  (budgets preenregistres, non modifies apres mesure)")


if __name__ == "__main__":
    main()
