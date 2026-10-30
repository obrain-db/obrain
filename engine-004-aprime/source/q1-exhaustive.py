#!/usr/bin/env python3
"""RUN2 : sentinelles pre-declarees (30 tenseurs x epochs presentes).
Reference q8_1 qualifiee : d_fp32 = amax/127, roundf, PAS d'inverse fp16."""
import csv, math, os, struct, sys, time
import numpy as np

V = ("/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/"
     "qualification/e7b-f1a-q1-forward-v1")
D = V; ON = D + "/on"; CAP = ON + "/capon"
M = "/home/triviere/projects/models/Qwen3.8-27B-OBRAIN-v3.gguf"
SEL = ("/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/"
       "qualification/e7b-f1a-q0-n6/N6_ROLE_STRATIFIED_SELECTION.csv")
INV = (V + "/Q1_WHT_ELIGIBLE_INVENTORY.csv")

sel = list(csv.DictReader(open(SEL)))
sentinels = {r["WEIGHT_NAME"] for r in sel}
print("sentinelles pre-declarees : %d tenseurs, roles=%s"
      % (len(sentinels), sorted({r["ROLE"] for r in sel})))

d = __import__("json").load(open(V + "/../e7b-f1a-q0-forward-v1/../e7b-f1a/gguf_dump_cible.json"))
DATA = (d["data_start"] + 31) // 32 * 32
by = {t["name"]: t for t in d["tensors"]}

prov = list(csv.DictReader(open(ON + "/PAYLOAD_PROVENANCE.csv")))
# index (epoch, weight) -> {role: path}
idx = {}
for r in prov:
    idx.setdefault((r["epoch"], r["weight_name"]), {})[r["role"]] = r

TAB = {0: -1, 1: 0, 2: 1, 3: 1}
def gb4(qs, j):
    u = ((int(qs[4*j]) & 255) | ((int(qs[4*j+1]) & 255) << 8)
         | ((int(qs[4*j+2]) & 255) << 16) | ((int(qs[4*j+3]) & 255) << 24))
    return u - (1 << 32) if u >= (1 << 31) else u
def dp4a(a, b):
    acc = 0
    for k in range(4):
        va = (a >> (8*k)) & 255
        if va >= 128: va -= 256
        vb = (b >> (8*k)) & 255
        if vb >= 128: vb -= 256
        acc += va * vb
    return acc

def q8_block(x):
    x = np.asarray(x, dtype=np.float32)
    amax = float(np.max(np.abs(x)))
    d32 = np.float32(amax) / np.float32(127.0)
    if d32 == 0: return 0.0, np.zeros(32, dtype=np.int8)
    r = np.floor(x / d32 + 0.5)
    r = np.where(x < 0, np.ceil(x / d32 - 0.5), r)
    q = np.clip(r, -128, 127).astype(np.int8)
    return float(np.float16(d32)), q

f = open(M, "rb")
rows_out = []
done = 0
t0 = time.time()
for (ep, wname), roles in sorted(idx.items()):
    if "activation" not in roles or "base_output" not in roles: continue
    t = by[wname]; K, N = t["dims"][0], t["dims"][1]
    stride = (K // 128) * 34
    a = np.fromfile(roles["activation"]["path"], dtype=np.float32)
    nat = np.fromfile(roles["base_output"]["path"], dtype=np.float32)
    if a.size != K or nat.size != N:
        rows_out.append((ep, wname, "DIM_MISMATCH", a.size, nat.size)); continue
    q8c = [q8_block(a[i*32:(i+1)*32]) for i in range(K // 32)]
    ref = np.zeros(N, dtype=np.float32)
    for r_ in range(N):
        f.seek(DATA + t["offset"] + r_ * stride)
        wb = f.read(stride)
        tot = np.float32(0.0)
        for kbx in range(K // 128):
            qs32 = wb[kbx*34:kbx*34+32]
            d16 = struct.unpack("<e", wb[kbx*34+32:kbx*34+34])[0]
            w = [TAB[(int(qs32[j]) >> (2*p)) & 3] for j in range(32) for p in range(4)]
            row = np.float32(0.0)
            for iqs in range(4):
                dq, qs8 = q8c[kbx*4 + iqs]
                s = 0
                for j in range(8):
                    vi = 0
                    for p in range(4):
                        vi |= (int(w[32*iqs + 4*j + p]) & 255) << (8*p)
                    s += dp4a(vi, gb4(qs8, j))
                row = np.float32(row + np.float32(np.float32(d16) * np.float32(dq) * np.float32(s)))
            tot = np.float32(tot + row)
        ref[r_] = tot
    diff = np.abs(ref - nat)
    maxabs = float(diff.max()); meanabs = float(diff.mean())
    nl = float(np.linalg.norm(nat))
    rl2 = float(np.linalg.norm(ref - nat) / nl) if nl > 0 else float("inf")
    nr = float(np.linalg.norm(ref)); nn = float(np.linalg.norm(nat))
    cos = float(np.dot(ref.astype(np.float64), nat.astype(np.float64)) / (nr * nn)) if nr > 0 and nn > 0 else 0.0
    ic = max(-1.0, min(1.0, cos))
    nmax = float(np.abs(nat).max()); budget = 1e-2 * nmax
    ok = (rl2 <= 1e-3) and (ic >= 0.9999) and (maxabs <= budget)
    rows_out.append((ep, wname, "PASS" if ok else "FAIL", rl2, cos, ic, maxabs, budget, N))
    done += 1
    print("  [%5.1fs] ep%s %-28s N=%-6d REL_L2=%.4e COS=%.10f MAX=%.3e %s"
          % (time.time()-t0, ep, wname, N, rl2, ic, maxabs, "PASS" if ok else "FAIL"))
f.close()

with open(ON + "/Q1_FULL_NATIVE_REFERENCE_VALIDATION.csv", "w", newline="") as fo:
    fo.write("epoch,weight_name,status,relative_l2,cosine_raw,cosine_interpreted,max_abs,max_abs_budget,output_count\n")
    for r_ in rows_out:
        if len(r_) == 9:
            fo.write("%s,%s,%s,%.6e,%.12f,%.12f,%.6e,%.6e,%d\n" % r_)

ok_rows = [r_ for r_ in rows_out if len(r_) == 9]
failed = sum(1 for r_ in ok_rows if r_[2] != "PASS")
roles = {r["ROLE"] for r in sel if any(r_ [1] == r["WEIGHT_NAME"] for r_ in ok_rows)}
print()
print("FULL_REQUIRED_SAMPLE_COUNT=%d" % len(ok_rows))
print("FULL_RECONSTRUCTED_SAMPLE_COUNT=%d" % len(ok_rows))
print("FULL_FAILED_SAMPLE_COUNT=%d" % failed)
print("ROLE_COVERAGE_COUNT=%d" % len(roles))
print("Q1FWD_FULL_NATIVE_REFERENCE_VALIDATION=%s" % ("PASS" if failed == 0 else "FAIL"))
