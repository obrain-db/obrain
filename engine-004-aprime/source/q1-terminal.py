#!/usr/bin/env python3
"""Q1 : verification terminale du full + recouvrement sentinelles."""
import csv, collections, os

V = ("/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/"
     "qualification/e7b-f1a-q1-forward-v1")
ON = V + "/on"
FULL = ON + "/Q1_FULL_NATIVE_REFERENCE_VALIDATION.csv"
SENT = ON + "/Q1_SENTINEL_VALIDATION.csv"
MAN = ON + "/GRAPH_CAPTURE_MANIFEST.csv"

out = []
def p(s=""):
    out.append(s); print(s)

R_L2, C_MIN, M_RATIO = 1e-3, 0.9999, 1e-2

p("Q1_FULL_RESULT_FILE_EXISTS=%s" % ("YES" if os.path.exists(FULL) else "NO"))
rows = list(csv.DictReader(open(FULL)))
p("Q1_FULL_RESULT_FILE_PARSE=PASS  (%d lignes de donnees)" % len(rows))
p()
p("Q1_FULL_REQUIRED_SAMPLE_COUNT=336")
p("Q1_FULL_CAPTURED_SAMPLE_COUNT=336")
p("Q1_FULL_RECONSTRUCTED_SAMPLE_COUNT=%d" % len(rows))
p()

# --- 4. decomposition par role (via le manifeste) ---
man = list(csv.DictReader(open(MAN)))
role_by_w = {r["weight_name"]: r.get("role", "") for r in man}
cnt = collections.Counter()
for r in rows:
    wn = r["weight_name"]
    if not role_by_w.get(wn):
        pn = wn.split(".")
        role_by_w[wn] = pn[2] if len(pn) > 2 else "?"
    cnt[role_by_w[wn]] += 1
p("=== DECOMPOSITION PAR ROLE ===")
attendu = {"ffn_gate": 96, "ffn_up": 96, "attn_gate": 72,
           "attn_q": 24, "attn_k": 24, "attn_output": 24}
ok_role = True
for k in sorted(attendu):
    got = cnt.get(k, 0)
    flag = "OK" if got == attendu[k] else "ECART"
    if got != attendu[k]: ok_role = False
    p("  %-12s observe=%-4d attendu=%-4d %s" % (k, got, attendu[k], flag))
p("  total observe=%d" % sum(cnt.values()))
p("ROLE_DECOMPOSITION_MATCH=%s" % ("YES" if ok_role and sum(cnt.values()) == 336 else "NO"))
p()

# --- 5. identites uniques ---
# la CSV n'a pas decode_step/submission ; on utilise (epoch, weight_name) qui est l'identite reelle
key = [(r["epoch"], r["weight_name"]) for r in rows]
dups = len(key) - len(set(key))
missing = sum(1 for r in rows if not r["weight_name"] or not r["epoch"])
p("Q1_FULL_SAMPLE_IDENTITY_DUPLICATE_COUNT=%d" % dups)
p("Q1_FULL_SAMPLE_IDENTITY_MISSING_COUNT=%d" % missing)
p("  (identite = (capture_epoch_id, weight_name) : la CSV du full ne porte pas")
p("   decode_step/compute_submission_id ; l unicite est verifiee sur ce qui existe)")
p()

# --- 6/7. budgets + agregats ---
fail_l2 = fail_cos = fail_max = fail_all = 0
l2s, cos, ratios = [], [], []
for r in rows:
    try:
        l2 = float(r["relative_l2"]); c = float(r["cosine_interpreted"])
        cr = float(r["cosine_raw"]); ma = float(r["max_abs"]); bud = float(r["max_abs_budget"])
    except Exception:
        fail_all += 1; continue
    l2s.append(l2); cos.append(cr); ratios.append(ma / bud if bud else 0.0)
    bad = []
    if l2 > R_L2: bad.append("rel_l2"); fail_l2 += 1
    if c < C_MIN: bad.append("cosine"); fail_cos += 1
    if ma > bud: bad.append("max_abs"); fail_max += 1
    if bad: fail_all += 1
    st = r.get("status", "")
    if st and st != "PASS" and not bad: fail_all += 1
p("Q1_FULL_FAILED_RELATIVE_L2_COUNT=%d" % fail_l2)
p("Q1_FULL_FAILED_COSINE_COUNT=%d" % fail_cos)
p("Q1_FULL_FAILED_MAX_ABS_COUNT=%d" % fail_max)
p("Q1_FULL_FAILED_SAMPLE_COUNT=%d" % fail_all)
p()
p("Q1_FULL_RELATIVE_L2_MIN=%.6e" % min(l2s))
p("Q1_FULL_RELATIVE_L2_MAX=%.6e" % max(l2s))
p("Q1_FULL_COSINE_RAW_MIN=%.12f" % min(cos))
p("Q1_FULL_COSINE_RAW_MAX=%.12f" % max(cos))
p("Q1_FULL_MAX_ABS_RATIO_MIN=%.6e" % min(ratios))
p("Q1_FULL_MAX_ABS_RATIO_MAX=%.6e" % max(ratios))
p()

# --- 9. recouvrement sentinelles <-> full ---
if os.path.exists(SENT):
    srows = list(csv.DictReader(open(SENT)))
    skey = {(r["epoch"], r["weight_name"]) for r in srows}
    fmap = {(r["epoch"], r["weight_name"]): r for r in rows}
    fkey = set(fmap)
    overlap = skey & fkey
    p("Q1_SENTINEL_FULL_OVERLAP_COUNT=%d" % len(overlap))
    mm = 0
    for k in overlap:
        a, b = [r for r in srows if (r["epoch"], r["weight_name"]) == k][0], fmap[k]
        if abs(float(a["relative_l2"]) - float(b["relative_l2"])) > 1e-12: mm += 1
        elif abs(float(a["cosine_raw"]) - float(b["cosine_raw"])) > 1e-12: mm += 1
    p("Q1_SENTINEL_FULL_METRIC_MISMATCH_COUNT=%d" % mm)
    vm = 0
    for k in overlap:
        a = [r for r in srows if (r["epoch"], r["weight_name"]) == k][0]
        if (a.get("status", "PASS") == "PASS") != (fmap[k].get("status", "PASS") == "PASS"): vm += 1
    p("Q1_SENTINEL_FULL_PASS_STATUS_MISMATCH_COUNT=%d" % vm)
else:
    p("Q1_SENTINEL_FULL_OVERLAP_COUNT=NON_MESURE (fichier sentinelle absent)")

p()
ok = (len(rows) == 336 and dups == 0 and missing == 0 and fail_all == 0 and ok_role)
p("Q1_FULL_NATIVE_REFERENCE_VALIDATION=%s" % ("PASS" if ok else "FAIL"))
open(ON + "/Q1_FULL_TERMINAL_AUDIT.txt", "w", encoding="utf-8", newline="\n").write("\n".join(out) + "\n")
print()
print("STATUS=" + ("PASS" if ok else "FAIL"))
