#!/usr/bin/env python3
"""ENGINE-007B-F1A-S2bis — reference exacte TQ2_G128 x Q8_1.

Semantique DERIVEE DE LA SOURCE COURANTE, pas inferee :
  ggml-cuda/vecdotq.cuh:732-767   vec_dot_tq2_g128_q8_1
  ggml-cuda/vecdotq.cuh:114       VDR_TQ2_G128_Q8_1_MMVQ = 1, qi = 4
  ggml/src/ggml-quants.c:302      quantize_row_q8_1_ref
  ggml/include/ggml.h:439         GGML_TYPE_TQ2_G128 = 43, bloc 34 o, blck 128

MODELE ARITHMETIQUE NATIF (par tranche de 32, une par appel iqs) :
    sumi = somme_{j=0..7} dp4a(vi[j], get_int_b4(q8.qs, j))
    y    = d_weight * low_half(q8.ds) * sumi
avec vi[j] = 4 codes ternaires empiles en int8 dans un mot 32 bits.
"""
import struct, math, numpy as np

# ---------------------------------------------------------------- q8_1

def _roundf(v):
    """roundf() du C : arrondi au plus proche, DEMI-ECARTE DE ZERO.
    np.rint fait du half-to-even : sur les valeurs exactement a .5 la difference est reelle,
    donc la reference DOIT implementer roundf explicitement (constate a la 1re execution)."""
    f = float(v)
    return int(math.floor(f + 0.5)) if f >= 0 else int(math.ceil(f - 0.5))

def q8_1_block_ref(x):
    """x : 32 float32. Retourne (d_fp16_bits, s_fp16_bits, qs int8[32]).
    Reproduit ggml-quants.c:302 quantize_row_q8_1_ref."""
    x = np.asarray(x, dtype=np.float32)
    assert x.shape == (32,)
    amax = float(np.max(np.abs(x)))
    d32 = np.float32(amax) / np.float32(127.0)
    d16 = np.float16(d32)                      # d arrondi en fp16 AVANT usage
    id32 = np.float32(1.0) / np.float32(d16) if d16 != 0 else np.float32(0.0)
    qs = np.zeros(32, dtype=np.int8)
    s = 0
    for j in range(16):
        v0 = np.float32(x[j]) * id32
        v1 = np.float32(x[16 + j]) * id32
        a = _roundf(v0); b = _roundf(v1)
        a = max(-128, min(127, a)); b = max(-128, min(127, b))
        qs[j] = a; qs[16 + j] = b
        s += a + b
    s16 = np.float16(np.float32(s) * np.float32(d16))
    return d16, s16, qs

def fp16_bits(h):
    return struct.unpack("<H", struct.pack("<e", float(h)))[0]

def low_half_of_ds(d16, s16):
    """__low2float(bq8_1.ds) : ggml_half2 = {d, s} -> la moitie BASSE est d."""
    return np.float32(d16)

def get_int_b4(qs, j):
    """4 octets consecutifs empiles en little-endian (get_int_b4)."""
    b0, b1, b2, b3 = int(qs[4*j]), int(qs[4*j+1]), int(qs[4*j+2]), int(qs[4*j+3])
    u = (b0 & 0xFF) | ((b1 & 0xFF) << 8) | ((b2 & 0xFF) << 16) | ((b3 & 0xFF) << 24)
    return u - (1 << 32) if u >= (1 << 31) else u

def dp4a(a, b):
    """__dp4a : 4 produits int8 accumules en int32."""
    acc = 0
    for k in range(4):
        va = (a >> (8*k)) & 0xFF
        if va >= 128: va -= 256
        vb = (b >> (8*k)) & 0xFF
        if vb >= 128: vb -= 256
        acc += va * vb
    return acc

# ---------------------------------------------------------------- TQ2_G128

TERNARY = {0: -1, 1: 0, 2: 1, 3: 1}     # vecdotq.cuh : repli 3 -> +1

def tq2_decode_block(qs32, d16):
    """Retourne 128 valeurs entieres {-1,0,+1} et l'echelle fp16 du bloc."""
    assert len(qs32) == 32
    out = np.zeros(128, dtype=np.int32)
    for j in range(8):
        b = int(qs32[j])
        for p in range(4):
            out[4*j + p] = TERNARY[(b >> (2*p)) & 3]
    return out, np.float32(d16)


# ---------------------------------------------------------------- ligne complete

def q8_1_for_activation(x, K):
    """Quantifie l'activation en blocs q8_1, un par tranche de 32, ordre kbx-major."""
    nblk = K // 128
    out = []
    for kbx in range(nblk):
        for iqs in range(4):
            a = x[kbx*128 + 32*iqs : kbx*128 + 32*iqs + 32]
            out.append(q8_1_block_ref(a))
    return out

def reference_row(weight_bytes, x, K, per_episode=True):
    """Reference exacte d'une ligne.

    weight_bytes : K/128 blocs de 34 octets
    x            : K valeurs float32 (activation logique)
    Retourne (contributions par episode, total).

    MODELE ARITHMETIQUE : celui du noyau deploye, tranche par tranche —
    dp4a entier puis y = d_weight * d_act_q8(fp16) * sumi, accumule en fp32.
    """
    nblk = K // 128
    assert len(weight_bytes) == nblk * 34
    x = np.asarray(x, dtype=np.float32)
    assert x.shape == (K,)
    q8 = q8_1_for_activation(x, K)
    eps = K // 1024
    per = np.zeros(eps, dtype=np.float32)
    total = np.float32(0.0)
    for kbx in range(nblk):
        qs32 = weight_bytes[kbx*34 : kbx*34 + 32]
        d16 = struct.unpack("<e", weight_bytes[kbx*34+32 : kbx*34+34])[0]
        w, d = tq2_decode_block(qs32, d16)
        row = np.float32(0.0)
        for iqs in range(4):
            d16a, s16a, qs8 = q8[kbx*4 + iqs]
            sumi = 0
            for j in range(8):
                vi = 0
                for p in range(4):
                    vi |= (int(w[32*iqs + 4*j + p]) & 0xFF) << (8*p)
                sumi += dp4a(vi, get_int_b4(qs8, j))
            term = np.float32(np.float32(d) * np.float32(d16a) * np.float32(sumi))
            row = np.float32(row + term)
        per[kbx // 8] = np.float32(per[kbx // 8] + row)
        total = np.float32(total + row)
    if per_episode:
        return per, total
    return total
