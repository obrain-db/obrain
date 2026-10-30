#!/usr/bin/env python3
"""Tests de la reference F1A-S2bis. Aucune saillance, aucun masque."""
import numpy as np, struct, sys, math
import f1a_ref as R

def hdr(t): print("="*66); print(t); print("="*66)

# ---------------------------------------------------------- §29 q8_1
hdr("Q8_1_REFERENCE_UNIT_TESTS  (paragraphe 29)")
cases = {
 "all_zeros":            np.zeros(32, dtype=np.float32),
 "constant_positive":    np.full(32, 3.5, dtype=np.float32),
 "constant_negative":    np.full(32, -2.25, dtype=np.float32),
 "alternating_signs":    np.array([1.0 if i%2==0 else -1.0 for i in range(32)], dtype=np.float32),
 "near_zero":            np.full(32, 1e-8, dtype=np.float32),
 "large_finite":         np.array([1e4*(-1)**i for i in range(32)], dtype=np.float32),
}
rng = np.random.default_rng(12345)
cases["random_finite"] = (rng.standard_normal(32)*2.0).astype(np.float32)

ok = True
print("%-20s %-12s %-12s %-8s %s" % ("cas","d_fp16","s_fp16","sum(qs)","verif"))
for name, x in cases.items():
    d16, s16, qs = R.q8_1_block_ref(x)
    amax = float(np.max(np.abs(x)))
    # invariants attendus de la source
    inv = []
    if amax == 0:
        inv.append("d=0" if d16 == 0 else "d!=0 -> ECHEC")
    else:
        # d doit valoir fp16(amax/127)
        exp = np.float16(np.float32(amax)/np.float32(127.0))
        inv.append("d_ok" if d16 == exp else "d_ECHEC")
    idv = (np.float32(1.0)/np.float32(d16)) if d16 != 0 else np.float32(0.0)
    # chaque qs doit valoir round(x*id) borne
    bad = 0
    for j in range(32):
        v = R._roundf(np.float32(x[j])*idv)
        v = max(-128, min(127, v))
        if int(qs[j]) != v: bad += 1
    inv.append("qs_ok" if bad==0 else "qs_%d_ECHEC"%bad)
    exp_s = np.float16(np.float32(int(np.sum(qs.astype(np.int32))))*np.float32(d16))
    inv.append("s_ok" if s16 == exp_s else "s_ECHEC")
    if "ECHEC" in "".join(inv): ok = False
    print("%-20s %-12s %-12s %-8d %s" % (name, float(d16), float(s16), int(np.sum(qs.astype(np.int32))), ",".join(inv)))
print()
print("Q8_1_REFERENCE_UNIT_TESTS=%s" % ("PASS" if ok else "FAIL"))
print("ZERO_AMAX_BEHAVIOR=d=0_et_id=0_donc_qs_tous_nuls  (id=0 => x*id=0 => round=0)")
print("ROUNDING=roundf_half_away_from_zero  (implemente explicitement ; np.rint serait faux)")
# test dedie du demi-ecarte-de-zero : 0.5, 1.5, 2.5, -0.5, -2.5
import numpy as _np
_d = _np.float16(1.0); _id = _np.float32(1.0)/_np.float32(_d)   # d=1 -> id=1
_cases = [(0.5,1),(1.5,2),(2.5,3),(-0.5,-1),(-1.5,-2),(-2.5,-3),(0.4999,0),(-0.4999,0)]
_ok = all(R._roundf(_np.float32(v)) == e for v,e in _cases)
print("ROUNDING_HALF_CASES_OK=%s  %s" % ("YES" if _ok else "NO",
      [ (v, R._roundf(_np.float32(v)), e) for v,e in _cases ]))

# ---------------------------------------------------------- TQ2 decode
hdr("TQ2_G128_REFERENCE_TESTS  (paragraphes 30, 31)")
tq_ok = True
# table de decodage
for code, exp in ((0,-1),(1,0),(2,1),(3,1)):
    got = R.TERNARY[code]
    if got != exp: tq_ok = False
    print("  code %d -> %+d  attendu %+d  %s" % (code, got, exp, "ok" if got==exp else "ECHEC"))
# poids de tous les codes, une tranche
qs32 = bytes([0b11100100]*4 + [0]*28)
w,d = R.tq2_decode_block(qs32, 1.0)
print("  octet 0x%02X -> 32 premiers poids : %s" % (0b11100100, list(w[:8])))
# 0b11100100 : bits 0-1 = 00 -> -1 ; 2-3 = 01 -> 0 ; 4-5 = 10 -> +1 ; 6-7 = 11 -> +1
print("  attendu (p=0..3) : [-1, 0, +1, +1]  ->  %s" % ("ok" if list(w[:4])==[-1,0,1,1] else "ECHEC"))
if list(w[:4]) != [-1,0,1,1]: tq_ok = False
print("  TQ2_G128_REFERENCE_DECODE=%s" % ("QUALIFIED" if tq_ok else "FAIL"))

# ---------------------------------------------------------- partition
hdr("EPISODE_PARTITION_VALIDATION  (paragraphes 31, 37)")
for K, eps in ((5120,5),(6144,6)):
    iv = [(e*1024,(e+1)*1024) for e in range(eps)]
    gaps = all(iv[i][1]==iv[i+1][0] for i in range(len(iv)-1))
    cover = iv[0][0]==0 and iv[-1][1]==K
    print("  K=%d : %d episodes [%d,%d) .. [%d,%d)" % (K,eps,iv[0][0],iv[0][1],iv[-1][0],iv[-1][1]))
    print("        NO_GAPS=%s  FULL_COVER=%s  NO_OVERLAPS=YES(par construction)"
          % ("YES" if gaps else "NO", "YES" if cover else "NO"))
# verification numerique : la somme des episodes egale le total
rng = np.random.default_rng(7)
for K,eps in ((5120,5),(6144,6)):
    nblk = K//128
    wb = bytearray()
    for b in range(nblk):
        wb += bytes(rng.integers(0,256,nblk and 32,dtype=np.uint8))
        wb += struct.pack("<e", float(np.float16(rng.uniform(0.01,0.5))))
    x = (rng.standard_normal(K)*1.5).astype(np.float32)
    per, tot = R.reference_row(bytes(wb), x, K)
    s = np.float32(sum(np.float32(v) for v in per))
    diff = abs(float(s)-float(tot))
    print("  K=%d : somme(episodes)=%.6f  total=%.6f  |ecart|=%.3e  PARTITION_COMPLETE=%s"
          % (K, float(s), float(tot), diff, "YES" if diff==0.0 else "NO(ordre d accumulation)"))
