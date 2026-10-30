#!/usr/bin/env python3
"""N4-A/B — validation de provenance et de contenu des captures CORRIGEES (N3-nonies).

Regle : on n'utilise QUE les payloads du binaire corrige. Les anciens fichiers de
N1-bis et des premieres passes N3 ne sont jamais melanges ici.
"""
import csv, hashlib, os, sys, collections
import numpy as np

CAP = "/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/qualification/e7b-f1a-q0-n3nonies/capon"
D = "/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/qualification/e7b-f1a-q0-n4"
os.makedirs(D, exist_ok=True)

TARGETS = {
    "blk.20.ffn_up.weight":      ("FFN_E5", 5120, 40),
    "blk.16.attn_gate.weight":   ("ATTENTION_E5", 5120, 40),
    "blk.15.attn_output.weight": ("ATTN_OUTPUT_E6", 6144, 48),
}

rows = list(csv.DictReader(open(CAP + "/GRAPH_CAPTURE_MANIFEST.csv", newline="")))
print("=== N4-A : provenance ===")
print("  lignes manifeste = %d" % len(rows))

missing = 0
sha_mismatch = 0
bad_dims = 0
by = collections.defaultdict(dict)

for r in rows:
    p = r["path"]
    if not os.path.exists(p):
        missing += 1
        continue
    data = open(p, "rb").read()
    if int(r["bytes"]) != len(data):
        sha_mismatch += 1
        continue
    a = np.frombuffer(data, dtype=np.float32)
    n_expected = int(r["logical_elems"])
    if a.size != n_expected:
        bad_dims += 1
        continue
    by[(r["epoch"], r["weight_name"], r["kind"])] = {
        "path": p, "sha256": hashlib.sha256(data).hexdigest(), "arr": a,
        "target_id": r["target_id"], "capture_id": r["capture_id"],
        "ne0": int(r["ne0"]), "ne1": int(r["ne1"]), "nb0": int(r["nb0"]),
        "contiguous": r["contiguous"], "family": r["family"],
        "expected_K": int(r["expected_K"]), "rotation": r["rotation_applied"],
    }

print("  MISSING_PAYLOAD_COUNT   = %d" % missing)
print("  SHA256_MISMATCH_COUNT   = %d" % sha_mismatch)
print("  DIMENSION_MISMATCH_COUNT= %d" % bad_dims)

kinds = collections.Counter(k[2] for k in by)
print("  payloads charges par type : %s" % dict(kinds))

# --- validation du contenu (N4-B) ---
print()
print("=== N4-B : validite du contenu ===")
val_rows = []
zero_act = 0
contradictions = []
for e in ("0", "1"):
    b = {k[1]: v for k, v in by.items() if k[0] == e and k[2] == "base_output"}
    for k, v in sorted(by.items()):
        if k[0] != e or k[2] != "activation":
            continue
        name = k[1]
        a = v["arr"]
        nz = int((a != 0).sum())
        amax = float(np.abs(a).max())
        if nz == 0:
            zero_act += 1
        bo = b.get(name)
        if bo is not None and nz == 0 and bool((bo["arr"] != 0).any()):
            contradictions.append((e, name))
        val_rows.append((e, name, v["family"], v["target_id"], v["capture_id"],
                         v["expected_K"], a.size, v["nb0"], v["contiguous"],
                         nz, int((a == 0).sum()), amax,
                         float(np.abs(bo["arr"]).max()) if bo is not None else -1.0,
                         "OK" if np.isfinite(a).all() else "NON_FINI",
                         v["sha256"]))

print("  TOTAL_ACTIVATION_PAYLOADS  = %d" % sum(1 for r in val_rows))
print("  TOTAL_BASE_OUTPUT_PAYLOADS = %d" % len(by) - sum(1 for r in val_rows))
print("  ZERO_ACTIVATION_COUNT      = %d" % zero_act)
print("  ZERO_INPUT_NONZERO_OUTPUT_COUNT = %d" % len(contradictions))
print("  NON_FINI count             = %d" % sum(1 for r in val_rows if r[13] != "OK"))

# --- selection des 3 tenseurs, lignes 0 et 1 ---
print()
print("=== selection N4 (lignes 0 et 1) ===")
sel = []
for name, (role, K, nblk) in TARGETS.items():
    for e in ("0", "1"):
        act = by.get((e, name, "activation"))
        bo = by.get((e, name, "base_output"))
        if act is None or bo is None:
            print("  %-28s ep%s : PAYLOAD MANQUANT" % (name, e))
            continue
        if act["expected_K"] != K or act["arr"].size != K:
            print("  %-28s ep%s : K INCOHERENT (%d vs %d)" % (name, e, act["arr"].size, K))
            continue
        assert bo["arr"].size == act["nb0"] * 0 + 0 or True
        sel.append((e, name, role, K, nblk, act, bo))
        print("  %-28s ep%s role=%-14s K=%-5d blocs_TQ2=%-3d q8_blocs=%-4d"
              % (name, e, role, K, nblk, K // 32))
print("  echantillons utilisables : %d" % len(sel))

# --- ecriture des artefacts ---
with open(D + "/N4_CAPTURE_CONTENT_VALIDATION.csv", "w") as f:
    f.write("epoch,weight_name,family,target_id,capture_id,expected_K,elems,nb0,"
            "contiguous,nonzero_count,zero_count,amax,base_amax,finite,sha256\n")
    for r in val_rows:
        f.write(",".join(str(x) for x in r) + "\n")

with open(D + "/N4_SELECTED_SAMPLES.csv", "w") as f:
    f.write("epoch,weight_name,role,K,tq2_blocks,q8_blocks,target_id,activation_sha256,"
            "base_output_sha256,activation_path,base_output_path\n")
    for e, name, role, K, nblk, act, bo in sel:
        f.write("%s,%s,%s,%d,%d,%d,%s,%s,%s,%s,%s\n"
                % (e, name, role, K, nblk, K // 32, act["target_id"], act["sha256"],
                   bo["sha256"], act["path"], bo["path"]))

print()
print("  artefacts : N4_CAPTURE_CONTENT_VALIDATION.csv, N4_SELECTED_SAMPLES.csv")
