#!/usr/bin/env python3
"""ENGINE-007B-F1A-Q0-N1 — validation de la reference q8_1 x TQ2_G128 contre mm_base natif.

Entrees :
  - un dump GGUF (offsets et dimensions des tenseurs TQ2_G128)
  - un payload d activation capture (cur_capture), F32
  - un payload de sortie native (mm_base), F32

Methode : pour chaque ligne echantillonnee, on lit les poids TQ2_G128 reels dans le
GGUF, on quantifie l activation en q8_1 selon la semantique source, on decode les codes
ternaires, et on reconstruit la sortie par tranches avec l'ordre dp4a -> double echelle fp16.

Aucune dequantification en flottant avant le produit scalaire : le modele arithmetique
est celui du noyau deploye.
"""
import csv, json, os, struct, sys, math
import numpy as np

GGUF = "/home/triviere/projects/models/Qwen3.8-27B-OBRAIN-v3.gguf"
DUMP = "/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/qualification/e7b-f1a/gguf_dump_cible.json"
D = "/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/qualification/e7b-f1a-q0-n1"

TERNARY = {0: -1, 1: 0, 2: 1, 3: 1}


def roundf(v):
    f = float(v)
    return int(math.floor(f + 0.5)) if f >= 0 else int(math.ceil(f - 0.5))


def q8_1_block(x):
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
    return d16, qs


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


def decode_block(qs32, d16):
    out = np.zeros(128, dtype=np.int32)
    for j in range(8):
        b = int(qs32[j])
        for p in range(4):
            out[4*j + p] = TERNARY[(b >> (2*p)) & 3]
    return out, np.float32(d16)


def reference_row(weight_bytes, x, K):
    """Reference exacte, tranche par tranche : dp4a entier puis double echelle fp16."""
    nblk = K // 128
    x = np.asarray(x, dtype=np.float32)
    acc = np.float32(0.0)
    for kbx in range(nblk):
        qs32 = weight_bytes[kbx*34 : kbx*34 + 32]
        d16 = struct.unpack("<e", weight_bytes[kbx*34+32 : kbx*34+34])[0]
        w, d = decode_block(qs32, d16)
        for iqs in range(4):
            d16a, qs8 = q8_1_block(x[kbx*128 + 32*iqs : kbx*128 + 32*iqs + 32])
            sumi = 0
            for j in range(8):
                vi = 0
                for p in range(4):
                    vi |= (int(w[32*iqs + 4*j + p]) & 0xFF) << (8*p)
                sumi += dp4a(vi, get_int_b4(qs8, j))
            term = np.float32(np.float32(d) * np.float32(d16a) * np.float32(sumi))
            acc = np.float32(acc + term)
    return np.float32(acc)


def main():
    selection = sys.argv[1]                    # CSV : tensor,layer,role,family,row,target_id
    dump = json.load(open(DUMP))
    byname = {t["name"]: t for t in dump["tensors"]}
    data_start = dump["data_start"]

    f = open(GGUF, "rb")
    rows_out = []
    with open(D + "/Q0_NATIVE_OPERATOR_REFERENCE_VALIDATION.csv", "w", newline="") as fo:
        w = csv.writer(fo)
        w.writerow(["tensor", "layer", "role", "family", "row", "target_id",
                    "K", "max_abs_error", "mean_abs_error", "relative_l2", "cosine",
                    "native_max_abs", "ref_max_abs", "budget_rel_l2_ok", "budget_cosine_ok",
                    "budget_max_abs_ok"])
        for sel in csv.DictReader(open(selection)):
            name = sel["tensor"]; row = int(sel["row"]); tid = int(sel["target_id"])
            t = byname[name]
            K, N = t["dims"][0], t["dims"][1]
            nbytes_row = (K // 128) * 34
            f.seek(data_start + t["offset"] + row * nbytes_row)
            wbytes = f.read(nbytes_row)
            assert len(wbytes) == nbytes_row

            ap = os.path.join(sel["payload_dir"], "ep0_sub1_t%d_c1_activation.bin" % tid)
            bp = os.path.join(sel["payload_dir"], "ep0_sub1_t%d_c1_base_output.bin" % tid)
            if not os.path.exists(ap) or not os.path.exists(bp):
                w.writerow([name, sel["layer"], sel["role"], sel["family"], row, tid,
                            K, "PAYLOAD_MISSING", "", "", "", "", "", "", "", ""])
                continue
            act = np.fromfile(ap, dtype=np.float32)
            base = np.fromfile(bp, dtype=np.float32)
            if act.size != K or row >= base.size:
                w.writerow([name, sel["layer"], sel["role"], sel["family"], row, tid,
                            K, "SHAPE_MISMATCH", "", "", "", "", "", "", "", ""])
                continue
            ref = np.float64(reference_row(wbytes, act, K))
            nat = np.float64(base[row])
            err = abs(ref - nat)
            rel = err / abs(nat) if abs(nat) > 1e-12 else float('nan')
            natmax = float(np.max(np.abs(base)))
            # COSINE sur la ligne : une seule valeur par ligne -> non defini.
            # On rapporte donc l ecart relativement au maximum de la sortie native.
            w.writerow([name, sel["layer"], sel["role"], sel["family"], row, tid, K,
                        "%.6e" % err, "%.6e" % err,
                        "%.6e" % rel if not math.isnan(rel) else "DEGENERE",
                        "NON_DEFINI_LIGNE_UNIQUE",
                        "%.6e" % natmax, "%.6e" % abs(ref),
                        "YES" if (not math.isnan(rel) and rel <= 1e-3) else "NO",
                        "NON_DEFINI",
                        "YES" if err <= 1e-2 * natmax else "NO"])
            rows_out.append((name, err, rel, natmax))
    f.close()
    print("  echantillons evalues :", len(rows_out))
    for n, e, r, m in rows_out:
        print("    %-34s err=%.6e rel=%s" % (n, e, ("%.3e" % r) if not math.isnan(r) else "DEGENERE"))


if __name__ == "__main__":
    main()
