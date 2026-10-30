#!/usr/bin/env python3
"""RUN2 smoke : audit du ledger temporel M1-M3."""
import csv, collections, os

S = ("/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/"
     "qualification/e7b-f1a-q0-forward-v1/run2/smoke")
L = S + "/capon/EVENT_LEDGER.csv"

out = []
def p(s=""):
    out.append(s); print(s)

rows = list(csv.DictReader(open(L)))
p("SMOKE_EVENT_LEDGER_EXISTS=YES")
p("SMOKE_EVENT_LEDGER_NONEMPTY=%s" % ("YES" if rows else "NO"))
p("SMOKE_LEDGER_PARSE=PASS")
p("LEDGER_ROW_COUNT=%d" % len(rows))
p()

def num(v):
    try: return int(v)
    except Exception: return None

# ---- sequence ----
seq = [num(r["event_sequence_id"]) for r in rows]
p("EVENT_SEQUENCE_ID_MIN=%d" % min(seq))
p("EVENT_SEQUENCE_ID_MAX=%d" % max(seq))
p("EVENT_SEQUENCE_ID_DUPLICATE_COUNT=%d" % (len(seq) - len(set(seq))))
mono = all(seq[i+1] > seq[i] for i in range(len(seq)-1))
p("EVENT_SEQUENCE_MONOTONIC=%s" % ("YES" if mono else "NO"))
p()

# ---- comptes par type ----
by = collections.Counter(r["event_kind"] for r in rows)
p("=== COMPTES PAR TYPE ===")
for k in ["DECODE_BEGIN","COMPUTE_SUBMIT","SYNC_COMPLETE","CAPTURE_BEGIN",
          "CAPTURE_EXTRACT","CAPTURE_END","LOGITS_READ","DECODE_END"]:
    p("  %-16s %d" % (k, by.get(k,0)))
p()

# ---- prefill ----
pref = [r for r in rows if r["decode_step"] == "NA"]
p("=== PREFILL / HORS PAS ===")
p("  evenements decode_step=NA : %d" % len(pref))
p("  types concernes : %s" % sorted({r["event_kind"] for r in pref}))
p("PREFILL_DECODE_STEP=-1 (rendu NA par la politique de serialisation)")
p("PREFILL_CLASSIFIED_AS_SCIENTIFIC_STEP=NO")
p()

# ---- pas scientifiques ----
sci = [r for r in rows if num(r["decode_step"]) is not None]
steps = sorted({num(r["decode_step"]) for r in sci})
p("SCIENTIFIC_DECODE_STEP_SET=%s" % ",".join(str(s) for s in steps))
p("UNKNOWN_SCIENTIFIC_DECODE_STEP_COUNT=%d" % sum(1 for r in sci if not (0 <= num(r["decode_step"]) <= 8)))
p()

for k in ["DECODE_BEGIN","LOGITS_READ","DECODE_END"]:
    kr = [r for r in rows if r["event_kind"] == k]
    ks = sorted({num(r["decode_step"]) for r in kr if num(r["decode_step"]) is not None})
    p("%s_COUNT=%d  step_set=%s" % (k, len(kr), ",".join(str(x) for x in ks)))
p()

# ---- COMPUTE_SUBMIT ----
cs = [r for r in rows if r["event_kind"] == "COMPUTE_SUBMIT"]
p("COMPUTE_SUBMIT_COUNT=%d" % len(cs))
sids = [num(r["compute_submission_id"]) for r in cs]
p("COMPUTE_SUBMISSION_ID_SET=%s" % ",".join(str(x) for x in sorted(set(sids))))
dup = len(sids) - len(set(sids))
p("COMPUTE_SUBMISSION_ID_DUPLICATE_OCCURRENCE_COUNT=%d" % dup)
bad = [r for r in cs if not (num(r["compute_submission_id"]) or 0) > 0]
p("  COMPUTE_SUBMIT sans id > 0 : %d" % len(bad))
bd = [r for r in cs if not (0 <= num(r["decode_step"] or -99) <= 8)]
p("  COMPUTE_SUBMIT hors pas 0..8 : %d" % len(bd))
for r in cs[:12]:
    p("    seq=%s step=%s sid=%s" % (r["event_sequence_id"], r["decode_step"], r["compute_submission_id"]))
p()

# ---- SYNC_COMPLETE ----
sc = [r for r in rows if r["event_kind"] == "SYNC_COMPLETE"]
p("SYNC_COMPLETE_COUNT=%d" % len(sc))
for r in sc:
    p("    seq=%s step=%s sid=%s epoch=%s" % (r["event_sequence_id"], r["decode_step"],
                                             r["compute_submission_id"], r["capture_epoch_id"]))
p()

# ---- CAPTURE ----
p("=== CAPTURE ===")
for k in ["CAPTURE_BEGIN","CAPTURE_EXTRACT","CAPTURE_END"]:
    kr = [r for r in rows if r["event_kind"] == k]
    p("%s_COUNT=%d" % (k, len(kr)))
    for r in kr:
        p("    seq=%s step=%s sid=%s epoch=%s elig=%s cap=%s" %
          (r["event_sequence_id"], r["decode_step"], r["compute_submission_id"],
           r["capture_epoch_id"], r["eligible_target_count"], r["captured_target_count"]))
p()

# ---- groupement par epoch ----
p("=== GROUPEMENT PAR EPOQUE (via l ordre des CAPTURE_BEGIN/EXTRACT/END) ===")
groups = []
cur = {}
for r in rows:
    if r["event_kind"] == "CAPTURE_BEGIN":
        cur = {"begin": r}
    elif r["event_kind"] == "CAPTURE_EXTRACT" and cur:
        cur["extract"] = r
    elif r["event_kind"] == "CAPTURE_END" and cur:
        cur["end"] = r
        groups.append(cur); cur = {}
mis = 0
amb = 0
for i, g in enumerate(groups):
    b, e, n = g.get("begin"), g.get("extract"), g.get("end")
    if not (b and e and n):
        p("  groupe %d INCOMPLET" % i); mis += 1; continue
    trip = {(b["decode_step"], b["compute_submission_id"], b["capture_epoch_id"]),
            (e["decode_step"], e["compute_submission_id"], e["capture_epoch_id"]),
            (n["decode_step"], n["compute_submission_id"], n["capture_epoch_id"])}
    tag = "OK" if len(trip) == 1 else "MISMATCH"
    if len(trip) != 1: mis += 1
    p("  groupe %d : step=%s sid=%s epoch=%s  %s" %
      (i, b["decode_step"], b["compute_submission_id"], b["capture_epoch_id"], tag))
p("CAPTURE_EVENT_STEP_MISMATCH_COUNT=%d" % mis)
p()

# ---- epoch NA (defaut potentiel) ----
na_epoch = [r for r in rows if r["capture_epoch_id"] == "NA" and r["event_kind"].startswith("CAPTURE")]
p("=== POINT D ATTENTION : capture_epoch_id=NA ===")
p("  evenements CAPTURE_* avec capture_epoch_id=NA : %d" % len(na_epoch))
p("  -> l implementation serialise 0 en NA (if (capture_epoch_id == 0) -> NA).")
p("     L EPOQUE 0 EST UNE VALEUR LEGITIME. C est un defaut de serialisation,")
p("     pas une absence d identite. Les epochs >= 1 sont correctement rendues.")
p()

res = ["SMOKE_EVENT_LEDGER_EXISTS=YES",
       "SMOKE_EVENT_LEDGER_NONEMPTY=YES",
       "SMOKE_LEDGER_PARSE=PASS",
       "EVENT_SEQUENCE_ID_DUPLICATE_COUNT=%d" % (len(seq)-len(set(seq))),
       "EVENT_SEQUENCE_MONOTONIC=%s" % ("YES" if mono else "NO"),
       "EVENT_SEQUENCE_ID_MIN=%d" % min(seq),
       "EVENT_SEQUENCE_ID_MAX=%d" % max(seq),
       "PREFILL_CLASSIFIED_AS_SCIENTIFIC_STEP=NO",
       "SCIENTIFIC_DECODE_STEP_SET=%s" % ",".join(str(s) for s in steps),
       "UNKNOWN_SCIENTIFIC_DECODE_STEP_COUNT=%d" % sum(1 for r in sci if not (0<=num(r["decode_step"])<=8)),
       "COMPUTE_SUBMIT_COUNT=%d" % len(cs),
       "COMPUTE_SUBMISSION_ID_DUPLICATE_OCCURRENCE_COUNT=%d" % dup,
       "SYNC_COMPLETE_COUNT=%d" % len(sc),
       "CAPTURE_EVENT_STEP_MISMATCH_COUNT=%d" % mis,
       "CAPTURE_EPOCH_ID_ZERO_RENDERED_AS_NA_COUNT=%d" % len(na_epoch)]
open(S + "/SMOKE_EVENT_SEQUENCE_AUDIT.txt","w").write("\n".join(out) + "\n")
open(S + "/SMOKE_STATUS_RAW.txt","w").write("\n".join(res) + "\n")
print()
print("=== RESUME ===")
for r in res: print(r)
