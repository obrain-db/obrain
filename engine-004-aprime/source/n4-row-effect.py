#!/usr/bin/env python3
"""N4-H — effet de la semantique q8_1 sur la reconstruction des lignes.

Toutes choses EGALES PAR AILLEURS : memes octets de poids (lecteur corrige,
origine alignee), meme table ternaire (celle du noyau), meme application
d'echelle (d_tq2 * d_q8 * sumi), meme accumulation sequentielle fp32.

On ne fait varier QUE la semantique de quantification q8_1 :
  OLD = arrondi contre 1/d_fp16 avec np.rint   (ma reference N3quater)
  NEW = arrondi contre xi/d_fp32 avec roundf   (noyau quantize_q8_1)
"""
import csv, json, math, os, struct, sys
import numpy as np

M = "/home/triviere/projects/models/Qwen3.8-27B-OBRAIN-v3.gguf"
DUMP = "/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/qualification/e7b-f1a/gguf_dump_cible.json"
CAP = "/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/qualification/e7b-f1a-q0-n3nonies/capon"
D = "/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/qualification/e7b-f1a-q0-n4"

TERNARY = {0: -1, 1: 0, 2: 1, 3: 1}   # table du noyau (vecdotq.cuh:751)


def roundf(v):
    v = float(v)
    return int(math.floor(v + 0.5)) if v >= 0.0 else int(math.ceil(v - 0.5))


def q8_new(x):
    """Noyau quantize_q8_1 : roundf(xi / d) avec d fp32 non arrondi."""
    x = np.asarray(x, dtype=np.float32)
    amax = float(np.max(np.abs(x)))
    if amax == 0.0:
        return 0.0, [0] * 32
    d = np.float32(amax) / np.float32(127.0)
    qs = [max(-128, min(127, roundf(np.float32(xi) / d))) for xi in x]
    return float(np.float16(d)), qs


def q8_old(x):
    """Ancienne reference : 1/d_fp16, np.rint."""
    x = np.asarray(x, dtype=np.float32)
    amax = float(np.max(np.abs(x)))
    d16 = np.float16(np.float32(amax) / np.float32(127.0))
    id32 = np.float32(1.0) / np.float32(d16) if d16 != 0 else np.float32(0.0)
    qs = [max(-128, min(127, int(np.rint(np.float32(xi) * id32)))) for xi in x]
    return float(d16), qs


def get_int_b4(qs, j):
    u = (int(qs[4 * j]) & 0xFF) | ((int(qs[4 * j + 1]) & 0xFF) << 8) \
        | ((int(qs[4 * j + 2]) & 0xFF) << 16) | ((int(qs[4 * j + 3]) & 0xFF) << 24)
    return u - (1 << 32) if u >= (1 << 31) else u


def dp4a(a, b):
    acc = 0
    for k in range(4):
        va = (a >> (8 * k)) & 0xFF
        va = va - 256 if va >= 128 else va
        vb = (b >> (8 * k)) & 0xFF
        vb = vb - 256 if vb >= 128 else vb
        acc += va * vb
    return acc


def decode_block(qs32):
    return [TERNARY[(int(qs32[j]) >> (2 * p)) & 3] for j in range(32) for p in range(4)]


def row_value(wb, act, K, q8fn):
    nblk = K // 128
    tot = np.float32(0.0)
    for kbx in range(nblk):
        qs32 = wb[kbx * 34:kbx * 34 + 32]
        d16 = struct.unpack("<e", wb[kbx * 34 + 32:kbx * 34 + 34])[0]
        w = decode_block(qs32)
        assert len(w) == 128
        r = np.float32(0.0)
        for iqs in range(4):
            dq, qs8 = q8fn(act[kbx * 128 + 32 * iqs: kbx * 128 + 32 * iqs + 32])
            s = 0
            for j in range(8):
                vi = 0
                for p in range(4):
                    vi |= (int(w[32 * iqs + 4 * j + p]) & 0xFF) << (8 * p)
                s += dp4a(vi, get_int_b4(qs8, j))
            r = np.float32(r + np.float32(np.float32(d16) * np.float32(dq) * np.float32(s)))
        tot = np.float32(tot + r)
    return float(tot)


def main():
    d = json.load(open(DUMP))
    by = {t["name"]: t for t in d["tensors"]}
    DATA = (d["data_start"] + 31) // 32 * 32

    rows = list(csv.DictReader(open(CAP + "/GRAPH_CAPTURE_MANIFEST.csv", newline="")))
    act = {}
    bo = {}
    for r in rows:
        a = np.fromfile(r["path"], dtype=np.float32)
        if r["kind"] == "activation":
            act[(r["epoch"], r["weight_name"])] = a
        else:
            bo[(r["epoch"], r["weight_name"])] = a

    targets = ["blk.20.ffn_up.weight", "blk.16.attn_gate.weight", "blk.15.attn_output.weight"]
    f = open(M, "rb")
    out = []
    out.append("  %-26s %-3s %-11s %-11s %-10s %-11s %-10s"
               % ("tenseur", "ln", "NATIF", "REF ancienne", "rel_OLD", "REF nouvelle", "rel_NEW"))
    res = []
    for name in targets:
        t = by[name]
        K = t["dims"][0]
        stride = (K // 128) * 34
        for row in (0, 1):
            key = ("0", name)
            if key not in act:
                continue
            a = act[key]
            native = float(bo[key][row])
            f.seek(DATA + t["offset"] + row * stride)
            wb = f.read(stride)
            v_old = row_value(wb, a, K, q8_old)
            v_new = row_value(wb, a, K, q8_new)
            r_old = abs(v_old - native) / abs(native) if abs(native) > 1e-12 else float("inf")
            r_new = abs(v_new - native) / abs(native) if abs(native) > 1e-12 else float("inf")
            out.append("  %-26s %-3d %-11.6f %-11.6f %-10.4g %-11.6f %-10.4g"
                       % (name, row, native, v_old, r_old, v_new, r_new))
            res.append((name, row, native, v_old, v_new, r_old, r_new))
    f.close()
    print("=== N4-H : effet de la semantique q8_1 (epoque 0, captures corrigees) ===")
    print("\n".join(out))
    print()
    better = sum(1 for r in res if r[6] < r[5])
    print("  lignes ou la NOUVELLE semantique est MEILLEURE : %d / %d" % (better, len(res)))
    print("  mediane rel_OLD = %.4g    mediane rel_NEW = %.4g"
          % (float(np.median([r[5] for r in res])), float(np.median([r[6] for r in res]))))
    with open(D + "/N4_ROW_NATIVE_COMPARISON.csv", "w") as fo:
        fo.write("weight_name,row,native,reference_old_q8,reference_new_q8,rel_old,rel_new\n")
        for r in res:
            fo.write(",".join(str(x) for x in r) + "\n")


if __name__ == "__main__":
    main()
