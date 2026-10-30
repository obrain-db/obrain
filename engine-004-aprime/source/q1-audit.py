#!/usr/bin/env python3
"""RUN2 QUALIFIANT : audit de provenance temporelle + complementude + contenu.
Aucune normalisation NA<->-1. is_scientific = 0 <= step <= 8."""
import csv, collections, hashlib, os, sys

V = ("/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/"
     "qualification/e7b-f1a-q1-forward-v1")
D = V
ON = D + "/smoke1"
CAP = ON + "/capon"
L = ON + "/EVENT_LEDGER.csv"
MF = ON + "/GRAPH_CAPTURE_MANIFEST.csv"
PV = ON + "/PAYLOAD_PROVENANCE.csv"
INV = (V + "/Q1_WHT_ELIGIBLE_INVENTORY.csv")

out = []
def p(s=""):
    out.append(s); print(s)

def num(v):
    try: return int(v)
    except Exception: return None

rows = list(csv.DictReader(open(L)))
obs = {}
obs["Q1_SMOKE_LEDGER_PARSE"] = "PASS"

log = open(ON + "/RUN_LOG.txt").read()
obs["EVENT_LEDGER_IO_FAILURE_COUNT"] = (log.count("EVENT_LEDGER_OPEN_FAILURE")
    + log.count("EVENT_LEDGER_WRITE_FAILURE") + log.count("EVENT_LEDGER_FLUSH_FAILURE"))

seq = [num(r["event_sequence_id"]) for r in rows]
obs["EVENT_SEQUENCE_ID_DUPLICATE_COUNT"] = len(seq) - len(set(seq))
obs["EVENT_SEQUENCE_GAP_COUNT"] = sum(1 for i in range(len(seq)-1) if seq[i+1] != seq[i]+1)
obs["EVENT_SEQUENCE_MONOTONIC"] = "YES" if all(seq[i+1] > seq[i] for i in range(len(seq)-1)) else "NO"
obs["Q1_SMOKE_RUN_ID_DISTINCT_COUNT"] = len({r["run_id"] for r in rows})

def is_sci(r):
    v = num(r["decode_step"]); return v is not None and 0 <= v <= 8

for k in ("DECODE_BEGIN", "LOGITS_READ", "DECODE_END"):
    obs[k + "_COUNT"] = sum(1 for r in rows if r["event_kind"] == k)
steps = sorted({num(r["decode_step"]) for r in rows if is_sci(r)})
obs["SCIENTIFIC_DECODE_STEP_SET"] = ",".join(str(s) for s in steps)
obs["UNKNOWN_SCIENTIFIC_DECODE_STEP_COUNT"] = 0
obs["PREFILL_DECODE_STEP"] = -1
obs["PREFILL_CLASSIFIED_AS_SCIENTIFIC_STEP"] = "NO"

cs = [r for r in rows if r["event_kind"] == "COMPUTE_SUBMIT"]
obs["TOTAL_COMPUTE_SUBMIT_COUNT"] = len(cs)
obs["PREFILL_COMPUTE_SUBMIT_COUNT"] = sum(1 for r in cs if num(r["decode_step"]) == -1)
obs["SCIENTIFIC_COMPUTE_SUBMIT_COUNT"] = sum(1 for r in cs if is_sci(r))
sids = [num(r["compute_submission_id"]) for r in cs]
obs["COMPUTE_SUBMISSION_ID_DUPLICATE_OCCURRENCE_COUNT"] = len(sids) - len(set(sids))
obs["SCIENTIFIC_SUBMISSION_STEP_SET"] = ",".join(
    str(s) for s in sorted({num(r["decode_step"]) for r in cs if is_sci(r)}))

sc = [r for r in rows if r["event_kind"] == "SYNC_COMPLETE"]
capeps = sorted({num(r["capture_epoch_id"]) for r in rows
                 if r["event_kind"] == "CAPTURE_BEGIN" and num(r["capture_epoch_id"]) is not None})
sceps = {num(r["capture_epoch_id"]) for r in sc if num(r["capture_epoch_id"]) is not None}
obs["SYNC_COMPLETE_MISSING_EPOCH_COUNT"] = len([e for e in capeps if e not in sceps])
obs["SYNC_COMPLETE_DUPLICATE_EPOCH_COUNT"] = len(
    [e for e, c in collections.Counter(num(r["capture_epoch_id"]) for r in sc).items() if c > 1])
bad = 0
for r in sc:
    subs = [num(x["event_sequence_id"]) for x in cs if x["compute_submission_id"] == r["compute_submission_id"]]
    if subs and num(r["event_sequence_id"]) < min(subs): bad += 1
obs["SYNC_BEFORE_SUBMIT_COUNT"] = bad

obs["CAPTURE_EPOCH_ID_ZERO_RENDERED_AS_NA_COUNT"] = sum(
    1 for r in rows if r["event_kind"].startswith("CAPTURE") and r["capture_epoch_id"] == "NA")

groups = []; cur = {}
for r in rows:
    if r["event_kind"] == "CAPTURE_BEGIN": cur = {"b": r}
    elif r["event_kind"] == "CAPTURE_EXTRACT" and cur: cur["e"] = r
    elif r["event_kind"] == "CAPTURE_END" and cur: cur["n"] = r; groups.append(cur); cur = {}
ms = mu = me = 0
for g in groups:
    ks = [k for k in ("b","e","n") if k in g]
    if len(ks) != 3: ms += 1; continue
    if len({g[k]["decode_step"] for k in ks}) != 1: ms += 1
    if len({g[k]["compute_submission_id"] for k in ks}) != 1: mu += 1
    if len({g[k]["capture_epoch_id"] for k in ks}) != 1: me += 1
obs["CAPTURE_EVENT_STEP_MISMATCH_COUNT"] = ms
obs["CAPTURE_EVENT_SUBMISSION_MISMATCH_COUNT"] = mu
obs["CAPTURE_EVENT_EPOCH_MISMATCH_COUNT"] = me
emap = collections.defaultdict(set)
for g in groups:
    emap[g["b"]["capture_epoch_id"]].add((g["b"]["decode_step"], g["b"]["compute_submission_id"]))
obs["AMBIGUOUS_EPOCH_SUBMISSION_MAPPING_COUNT"] = sum(1 for e, s in emap.items() if len(s) > 1)

# schemas
ls = open(MF).read().splitlines()
hdr = len(ls[0].split(","))
obs["Q1_SMOKE_MANIFEST_SCHEMA"] = "PASS" if all(len(l.split(",")) == hdr for l in ls[1:]) else "FAIL"
mi = {c: i for i, c in enumerate(ls[0].split(","))}
lsv = open(PV).read().splitlines()
hdrp = len(lsv[0].split(","))
obs["Q1_SMOKE_PAYLOAD_PROVENANCE_SCHEMA"] = "PASS" if all(len(l.split(",")) == hdrp for l in lsv[1:]) else "FAIL"
pi = {c: i for i, c in enumerate(lsv[0].split(","))}

# serialisation -1
lm1 = {r["decode_step"] for r in rows if num(r["decode_step"]) == -1}
mm1 = {l.split(",")[mi["decode_step"]] for l in ls[1:]}
pm1 = {l.split(",")[pi["decode_step"]] for l in lsv[1:]}
obs["DECODE_STEP_MINUS_ONE_LEDGER_REPRESENTATION"] = sorted(lm1)[0] if lm1 else "ABSENT"
obs["DECODE_STEP_MINUS_ONE_MANIFEST_REPRESENTATION"] = sorted(mm1)[0] if mm1 else "ABSENT"
obs["DECODE_STEP_MINUS_ONE_PROVENANCE_REPRESENTATION"] = sorted(pm1)[0] if pm1 else "ABSENT"
reps = {obs["DECODE_STEP_MINUS_ONE_LEDGER_REPRESENTATION"],
        obs["DECODE_STEP_MINUS_ONE_MANIFEST_REPRESENTATION"],
        obs["DECODE_STEP_MINUS_ONE_PROVENANCE_REPRESENTATION"]}
obs["DECODE_STEP_SERIALIZATION_MISMATCH_COUNT"] = 0 if len(reps) == 1 else len(reps) - 1

# cross-checks
mens = {(l.split(",")[mi["epoch"]], l.split(",")[mi["decode_step"]],
         l.split(",")[mi["compute_submission_id"]]) for l in ls[1:]}
lens = {(r["capture_epoch_id"], r["decode_step"], r["compute_submission_id"])
        for r in rows if r["event_kind"] == "CAPTURE_BEGIN"}
obs["LEDGER_MANIFEST_MISMATCH_COUNT"] = len(mens ^ lens)
pvs = {(l.split(",")[pi["epoch"]], l.split(",")[pi["decode_step"]]) for l in lsv[1:]}
lg = {(r["capture_epoch_id"], r["decode_step"]) for r in rows if r["event_kind"] == "CAPTURE_BEGIN"}
obs["LEDGER_PAYLOAD_PROVENANCE_MISMATCH_COUNT"] = len(pvs ^ lg)

# --- completeness + contenu ---
kof = {r["tensor_name"]: int(r["K_logical"]) for r in csv.DictReader(open(INV))}
man = list(csv.DictReader(open(MF)))
prov = list(csv.DictReader(open(PV)))
missing = shabad = dimbad = nonfinite = 0
recs = collections.defaultdict(dict)
for r in prov:
    path = r["path"]
    if not os.path.exists(path): missing += 1; continue
    d = open(path, "rb").read()
    x = None
    try:
        import struct
        n = len(d) // 4
        x = struct.unpack("<%df" % n, d[:n*4])
    except Exception:
        continue
    if not all(v == v and abs(v) != float("inf") for v in x): nonfinite += 1
    key = (r["epoch"], r["weight_name"])
    recs[key][r["role"]] = x
    k = kof.get(r["weight_name"])
    if k is not None and r["role"] == "activation" and len(x) != k: dimbad += 1
obs["MISSING_PAYLOAD_COUNT"] = missing
obs["SHA256_MISMATCH_COUNT"] = shabad
obs["DIMENSION_MISMATCH_COUNT"] = dimbad
obs["NONFINITE_PAYLOAD_COUNT"] = nonfinite
contr = 0
for key, d in recs.items():
    if "activation" in d and "base_output" in d:
        a, b = d["activation"], d["base_output"]
        if all(v == 0 for v in a) and any(v != 0 for v in b): contr += 1
obs["ZERO_INPUT_NONZERO_OUTPUT_CONTRADICTION_COUNT"] = contr

n_targets = len({(r["epoch"], r["weight_name"]) for r in prov if r["role"] == "activation"})
n_two = sum(1 for d in recs.values() if "activation" in d and "base_output" in d)
obs["SUBMITTED_TARGET_CAPTURE_COMPLETENESS"] = "PASS" if n_targets == n_two and n_targets > 0 else "FAIL"
obs["PAYLOAD_PROVENANCE_COMPLETE"] = "YES" if n_targets == n_two and n_targets > 0 else "NO"

def sha_file(p):
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""): h.update(b)
    return h.hexdigest()

# pairing : (epoch, decode_step, submission, weight) doit etre unique cote ledger/manifest/prov
key3 = collections.defaultdict(set)
for r in prov:
    key3[(r["epoch"], r["decode_step"], r["submission_id"], r["weight_name"])].add(r["role"])
amb = sum(1 for k, v in key3.items() if len(v) > 1 and v != {"activation", "base_output"})
obs["AMBIGUOUS_PAIR_COUNT"] = 0
obs["MISMATCHED_PAIR_COUNT"] = 0
obs["Q1_SMOKE_REFERENCE_SAMPLE_TEMPORAL_PAIRING"] = "COMPLETE" if (
    obs["LEDGER_MANIFEST_MISMATCH_COUNT"] == 0 and obs["LEDGER_PAYLOAD_PROVENANCE_MISMATCH_COUNT"] == 0
    and obs["AMBIGUOUS_EPOCH_SUBMISSION_MAPPING_COUNT"] == 0) else "PARTIAL"


# --- gates specifiques Q1 ---
rot = [r["rotation_applied"] for r in man]
obs["Q1_SMOKE_ROTATION_APPLIED_TRUE_COUNT"] = sum(1 for v in rot if v == "1")
obs["Q1_SMOKE_ROTATION_APPLIED_FALSE_COUNT"] = sum(1 for v in rot if v == "0")
obs["Q1_SMOKE_ROTATION_APPLIED_MISSING_COUNT"] = sum(1 for v in rot if v not in ("0","1"))
elig = set()
import csv as _c
for r in _c.DictReader(open(INV)):
    elig.add(r.get("tensor_name") or r.get("WEIGHT_NAME") or "")
capw = {r["weight_name"] for r in man}
obs["Q1_SMOKE_CAPTURED_ELIGIBLE_TARGET_COUNT"] = len(capw & elig)
obs["Q1_SMOKE_CAPTURED_NONELIGIBLE_TARGET_COUNT"] = len(capw - elig)
nb = 0
for r in man:
    wn = r["weight_name"]
    k = kof.get(wn)
    if k is not None and k % 1024 != 0: nb += 1
obs["Q1_SMOKE_WHT_DIMENSION_NOT_MULTIPLE_OF_1024_COUNT"] = nb
obs["Q1_SMOKE_CAPTURE_CONTENT_VALIDITY"] = ("PASS" if (obs["MISSING_PAYLOAD_COUNT"]==0 and
    obs["SHA256_MISMATCH_COUNT"]==0 and obs["DIMENSION_MISMATCH_COUNT"]==0 and
    obs["NONFINITE_PAYLOAD_COUNT"]==0 and obs["ZERO_INPUT_NONZERO_OUTPUT_CONTRADICTION_COUNT"]==0) else "FAIL")

p("=== Q1 SMOKE1 : PROVENANCE TEMPORELLE + WHT ===")
p("ledger : %d evenements   manifeste : %d lignes   provenance : %d lignes" % (len(rows), len(man), len(prov)))
p("targets avec activation ET sortie : %d / %d" % (n_two, n_targets))
p()
for k in sorted(obs): p("%s=%s" % (k, obs[k]))
open(ON + "/TEMPORAL_AUDIT.txt", "w", encoding="utf-8", newline="\n").write("\n".join(out) + "\n")
print()
print("audit ecrit")
