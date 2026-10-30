#!/usr/bin/env python3
"""ENGINE-007B-F1A-Q0-N3quater — reconstruction TQ2_G128 x q8_1 sur octets QUALIFIES.

Lecture des poids : origine ALIGNEE (align_up(header_end, 32) + offset), formule corrigee
et verifiee octet a octet contre libggml en N3ter.

Modele arithmetique derive de la source (vecdotq.cuh:732) :
    pour chaque tranche iqs de 32 :
        sumi = somme_{j=0..7} dp4a(vi[j], get_int_b4(q8.qs, j))
        y   += d_tq2 * d_q8 * sumi
avec vi[j] = 4 codes ternaires empiles en int8 dans un mot 32 bits.
"""
import json, math, os, struct, sys
import numpy as np

M = "/home/triviere/projects/models/Qwen3.8-27B-OBRAIN-v3.gguf"
DUMP = "/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/qualification/e7b-f1a/gguf_dump_cible.json"
CAP = "/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/qualification/e7b-f1a-q0-n1bis/capon"
D = "/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/qualification/e7b-f1a-q0-n3quater"
os.makedirs(D, exist_ok=True)

TERNARY = {0: -1, 1: 0, 2: 1, 3: 1}


def roundf(v):
    f = float(v)
    return int(math.floor(f + 0.5)) if f >= 0 else int(math.ceil(f - 0.5))


def q8_1_block(x):
    """Semantique ggml-quants.c:302 quantize_row_q8_1_ref, roundf explicite."""
    x = np.asarray(x, dtype=np.float32)
    amax = float(np.max(np.abs(x)))
    d32 = np.float32(amax) / np.float32(127.0)
    d16 = np.float16(d32)
    id32 = np.float32(1.0) / np.float32(d16) if d16 != 0 else np.float32(0.0)
    qs = np.zeros(32, dtype=np.int8)
    s = 0
    for j in range(16):
        a = roundf(np.float32(x[j]) * id32)
        b = roundf(np.float32(x[16 + j]) * id32)
        a = max(-128, min(127, a)); b = max(-128, min(127, b))
        qs[j] = a; qs[16 + j] = b
        s += a + b
    return float(d16), qs, s, amax, float(d32)


def get_int_b4(qs, j):
    b0, b1, b2, b3 = (int(qs[4*j]), int(qs[4*j+1]), int(qs[4*j+2]), int(qs[4*j+3]))
    u = (b0 & 0xFF) | ((b1 & 0xFF) << 8) | ((b2 & 0xFF) << 16) | ((b3 & 0xFF) << 24)
    return u - (1 << 32) if u >= (1 << 31) else u


def dp4a(a, b):
    acc = 0
    for k in range(4):
        va = (a >> (8*k)) & 0xFF
        if va >= 128: va -= 256
        vb = (b >> (8*k)) & 0xFF
        if vb >= 128: vb -= 256
        acc += va * vb
    return acc


def decode_block(qs32):
    """128 valeurs ternaires, ordre petit-boutiste dans l octet."""
    out = np.zeros(128, dtype=np.int32)
    for j in range(8):
        b = int(qs32[j])
        for p in range(4):
            out[4*j + p] = TERNARY[(b >> (2*p)) & 3]
    return out


def reference_row(wbytes, x, K, want_blocks=False):
    nblk = K // 128
    x = np.asarray(x, dtype=np.float32)
    contrib = []
    total = np.float32(0.0)
    for kbx in range(nblk):
        qs32 = wbytes[kbx*34: kbx*34 + 32]
        d16 = struct.unpack("<e", wbytes[kbx*34+32: kbx*34+34])[0]
        w = decode_block(qs32)
        row = np.float32(0.0)
        subs = []
        for iqs in range(4):
            dq, qs8, sq, amax, dpre = q8_1_block(x[kbx*128 + 32*iqs: kbx*128 + 32*iqs + 32])
            sumi = 0
            for j in range(8):
                vi = 0
                for p in range(4):
                    vi |= (int(w[32*iqs + 4*j + p]) & 0xFF) << (8*p)
                sumi += dp4a(vi, get_int_b4(qs8, j))
            term = np.float32(np.float32(d16) * np.float32(dq) * np.float32(sumi))
            row = np.float32(row + term)
            subs.append((iqs, sumi, float(d16), dq, term, amax, dpre, sq))
        contrib.append((kbx, float(d16), row, subs, qs32))
        total = np.float32(total + row)
    return (float(total), contrib) if want_blocks else float(total)


def main():
    d = json.load(open(DUMP))
    byname = {t["name"]: t for t in d["tensors"]}
    ALIGN = 32
    DS = d["data_start"]
    DATA = (DS + ALIGN - 1)//ALIGN*ALIGN
    print("DATA_START_ALIGNE=%d (brut=%d, padding=%d)" % (DATA, DS, DATA-DS))

    # selection FIGEE : 3 tenseurs, lignes 0 et 1
    sel = [("blk.20.ffn_up.weight", "FFN_E5", "E5", 44),
           ("blk.16.attn_gate.weight", "ATTENTION_E5", "E5", 29),
           ("blk.15.attn_output.weight", "ATTN_OUTPUT_E6", "E6", None)]
    # target_id par nom depuis le manifeste
    import csv
    man = list(csv.DictReader(open(CAP+"/GRAPH_CAPTURE_MANIFEST.csv")))
    tid = {}
    for r in man:
        if r["kind"] == "activation":
            tid.setdefault(r["weight_name"], r["target_id"])

    f = open(M, "rb")
    rows = []
    scale_rows = []
    tern_rows = []
    for name, role, fam, _ in sel:
        t = byname[name]
        K, N = t["dims"][0], t["dims"][1]
        stride = (K//128)*34
        t_id = tid.get(name)
        if t_id is None:
            print("  %-30s PAS DE TARGET_ID" % name); continue
        ap = "%s/ep0_sub1_t%s_c1_activation.bin" % (CAP, t_id)
        bp = "%s/ep0_sub1_t%s_c1_base_output.bin" % (CAP, t_id)
        if not os.path.exists(ap) or not os.path.exists(bp):
            # nom reel avec capture_id
            cand = [x for x in os.listdir(CAP) if "_t%s_" % t_id in x and x.endswith("_activation.bin")]
            if not cand:
                print("  %-30s payloads absents" % name); continue
            ap = CAP+"/"+cand[0]
            bp = ap.replace("_activation.bin", "_base_output.bin")
        act = np.fromfile(ap, dtype=np.float32)
        base = np.fromfile(bp, dtype=np.float32)
        print("  %-28s K=%-5d N=%-6d act=%d base=%d" % (name, K, N, act.size, base.size))
        for row in (0, 1):
            f.seek(DATA + t["offset"] + row*stride)
            wb = f.read(stride)
            assert len(wb) == stride
            y, contrib = reference_row(wb, act, K, want_blocks=True)
            nat = float(base[row])
            err = abs(y - nat)
            rel = err/abs(nat) if abs(nat) > 1e-12 else float('nan')
            rows.append((name, role, fam, row, K, t_id, y, nat, err, rel))
            # echelles du bloc 0 et du dernier bloc
            for bx in (0, K//128 - 1):
                d16 = struct.unpack("<e", wb[bx*34+32: bx*34+34])[0]
                scale_rows.append((name, row, bx, wb[bx*34+32], wb[bx*34+33], float(d16)))
            # ternaire : bloc 0
            w0 = decode_block(wb[:32])
            for i in range(128):
                raw = (wb[(i//4)] >> (2*(i % 4))) & 3
                tern_rows.append((name, row, 0, i, raw, int(w0[i]), TERNARY[raw]))
    f.close()

    print()
    print("  %-28s %-4s %-14s %-14s %-11s %-11s" % ("tenseur","ln","reference","native","abs_err","rel_err"))
    for n, role, fam, row, K, t_id, y, nat, err, rel in rows:
        print("  %-28s %-4d %-14.6f %-14.6f %-11.4e %-11s" % (n, row, y, nat, err, "%.3e"%rel if not math.isnan(rel) else "DEGEN"))
    print()
    ok = all((not math.isnan(r[9])) and r[9] <= 1e-3 for r in rows) if rows else False
    print("  lignes testees=%d  sous budget rel_l2<=1e-3 : %s" % (len(rows), "OUI" if ok else "NON"))

    with open(D+"/N3QUATER_ROW_NATIVE_COMPARISON.csv","w") as fo:
        fo.write("tensor,role,family,row,K,target_id,reference,native,abs_error,relative_error\n")
        for r in rows: fo.write(",".join(str(x) for x in r)+"\n")
    with open(D+"/N3QUATER_SCALE_DECODING.csv","w") as fo:
        fo.write("tensor,row,block_index,d_raw_0,d_raw_1,d_fp32\n")
        for r in scale_rows: fo.write(",".join(str(x) for x in r)+"\n")
    with open(D+"/N3QUATER_TERNARY_DECODING.csv","w") as fo:
        fo.write("tensor,row,block,weight_index,raw_code,reference_value,independent_value\n")
        for r in tern_rows: fo.write(",".join(str(x) for x in r)+"\n")

    ds = [r[5] for r in scale_rows]
    print("  echelles bloc 0 / dernier bloc : min=%.6f max=%.6f moyenne=%.6f" % (min(ds), max(ds), sum(ds)/len(ds)))
    print("  ternaire : %d valeurs decodees, deux decodeurs concordent = %s"
          % (len(tern_rows), all(r[5] == r[6] for r in tern_rows)))


if __name__ == "__main__":
    main()
