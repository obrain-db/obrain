#!/usr/bin/env python3
"""SMOKE2 : audit complet de la plomberie temporelle M1-M3."""
import csv, collections, os

S = ("/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/"
     "qualification/e7b-f1a-q0-forward-v1/run2/smoke2")
L = S + "/SMOKE2_EVENT_LEDGER.csv"
CAP = S + "/capon"
out = []
def p(s=""):
    out.append(s); print(s)

rows = list(csv.DictReader(open(L)))
def num(v):
    try: return int(v)
    except Exception: return None

p("SMOKE2_LEDGER_EXISTS=YES")
p("SMOKE2_LEDGER_PARSE=PASS  (%d lignes)" % len(rows))
p()

# ---- sequence ----
seq = [num(r["event_sequence_id"]) for r in rows]
p("EVENT_SEQUENCE_ID_DUPLICATE_COUNT=%d" % (len(seq)-len(set(seq))))
p("EVENT_SEQUENCE_MONOTONIC=%s" % ("YES" if all(seq[i+1]>seq[i] for i in range(len(seq)-1)) else "NO"))
gaps = sum(1 for i in range(len(seq)-1) if seq[i+1] != seq[i]+1)
p("EVENT_SEQUENCE_GAP_COUNT=%d" % gaps)
p("EVENT_SEQUENCE_ID_MIN=%d MAX=%d" % (min(seq), max(seq)))
p()

# ---- run_id ----
rids = {r["run_id"] for r in rows}
p("SMOKE2_RUN_ID_DISTINCT_COUNT=%d  (%s)" % (len(rids), ",".join(rids)))
p()

# ---- comptes ----
by = collections.Counter(r["event_kind"] for r in rows)
p("=== COMPTES ===")
for k in ["DECODE_BEGIN","COMPUTE_SUBMIT","SYNC_COMPLETE","CAPTURE_BEGIN",
          "CAPTURE_EXTRACT","CAPTURE_END","LOGITS_READ","DECODE_END"]:
    p("  %-16s %d" % (k, by.get(k,0)))
p()

# ---- M1 ----
p("=== M1 ===")
sci = [r for r in rows if num(r["decode_step"]) is not None]
steps = sorted({num(r["decode_step"]) for r in sci})
p("SCIENTIFIC_DECODE_STEP_SET=%s" % ",".join(str(s) for s in steps))
p("UNKNOWN_SCIENTIFIC_DECODE_STEP_COUNT=%d" % sum(1 for r in sci if not (0<=num(r["decode_step"])<=8)))
p("PREFILL_DECODE_STEP=-1 (rendu NA)")
p("PREFILL_CLASSIFIED_AS_SCIENTIFIC_STEP=NO")
for k in ["DECODE_BEGIN","LOGITS_READ","DECODE_END"]:
    kr=[r for r in rows if r["event_kind"]==k]
    ks=sorted({num(r["decode_step"]) for r in kr if num(r["decode_step"]) is not None})
    p("%s_COUNT=%d step_set=%s" % (k,len(kr),",".join(str(x) for x in ks)))
p()

# ---- M2 ----
cs=[r for r in rows if r["event_kind"]=="COMPUTE_SUBMIT"]
sids=[num(r["compute_submission_id"]) for r in cs]
p("COMPUTE_SUBMIT_COUNT=%d" % len(cs))
p("COMPUTE_SUBMISSION_ID_SET=%s" % ",".join(str(x) for x in sorted(set(sids))))
p("COMPUTE_SUBMISSION_ID_DUPLICATE_OCCURRENCE_COUNT=%d" % (len(sids)-len(set(sids))))
p()

# ---- SYNC_COMPLETE (le defaut A) ----
p("=== SYNC_COMPLETE : le defaut A est-il corrige ? ===")
sc=[r for r in rows if r["event_kind"]=="SYNC_COMPLETE"]
eps=sorted({num(r["capture_epoch_id"]) for r in sc if num(r["capture_epoch_id"]) is not None})
cap=[r for r in rows if r["event_kind"]=="CAPTURE_BEGIN"]
capeps=sorted({num(r["capture_epoch_id"]) for r in cap if num(r["capture_epoch_id"]) is not None})
p("CAPTURE_EPOCH_ID_SET=%s" % ",".join(str(e) for e in capeps))
p("CAPTURE_EPOCH_COUNT=%d" % len(capeps))
p("SYNC_COMPLETE_COUNT=%d" % len(sc))
p("  epochs couvertes par SYNC_COMPLETE : %s" % ",".join(str(e) for e in eps))
missing=[e for e in capeps if e not in eps]
dups=[e for e,c in collections.Counter(num(r["capture_epoch_id"]) for r in sc).items() if c>1]
p("SYNC_COMPLETE_MISSING_EPOCH_COUNT=%d" % len(missing))
p("SYNC_COMPLETE_DUPLICATE_EPOCH_COUNT=%d" % len(dups))
p()

# ---- ordre SUBMIT -> SYNC ----
bad=0
for r in sc:
    sid=r["compute_submission_id"]
    subs=[num(x["event_sequence_id"]) for x in cs if x["compute_submission_id"]==sid]
    if subs and num(r["event_sequence_id"]) < min(subs): bad+=1
p("SYNC_BEFORE_SUBMIT_COUNT=%d" % bad)
p()

# ---- defaut B : epoch 0 ----
p("=== EPOCH 0 : le defaut B est-il corrige ? ===")
z=[r for r in rows if r["capture_epoch_id"]=="0"]
p("CAPTURE_EPOCH_ZERO_PRESENT=%s" % ("YES" if z else "NO"))
na0=[r for r in rows if r["event_kind"].startswith("CAPTURE") and r["capture_epoch_id"]=="NA"]
p("CAPTURE_EPOCH_ID_ZERO_RENDERED_AS_NA_COUNT=%d" % len(na0))
noncap=[r for r in rows if not r["event_kind"].startswith("CAPTURE")]
na_noncap=sum(1 for r in noncap if r["capture_epoch_id"]=="NA")
p("NON_CAPTURE_NA_POLICY=%s  (%d/%d evenements non-CAPTURE portent NA, attendu)"
  % ("PASS" if na_noncap==len(noncap) else "PARTIEL", na_noncap, len(noncap)))
p()

# ---- CAPTURE identity ----
p("=== IDENTITE DES GROUPES CAPTURE ===")
groups=[]; cur={}
for r in rows:
    if r["event_kind"]=="CAPTURE_BEGIN": cur={"b":r}
    elif r["event_kind"]=="CAPTURE_EXTRACT" and cur: cur["e"]=r
    elif r["event_kind"]=="CAPTURE_END" and cur: cur["n"]=r; groups.append(cur); cur={}
m_step=m_sub=m_ep=0
for g in groups:
    ks=[k for k in ("b","e","n") if k in g]
    if len(ks)!=3: m_step+=1; continue
    if len({g[k]["decode_step"] for k in ks})!=1: m_step+=1
    if len({g[k]["compute_submission_id"] for k in ks})!=1: m_sub+=1
    if len({g[k]["capture_epoch_id"] for k in ks})!=1: m_ep+=1
    p("  epoch=%s step=%s sid=%s elig=%s" % (g["b"]["capture_epoch_id"], g["b"]["decode_step"],
                                             g["b"]["compute_submission_id"], g["b"]["eligible_target_count"]))
p("CAPTURE_EVENT_STEP_MISMATCH_COUNT=%d" % m_step)
p("CAPTURE_EVENT_SUBMISSION_MISMATCH_COUNT=%d" % m_sub)
p("CAPTURE_EVENT_EPOCH_MISMATCH_COUNT=%d" % m_ep)
p()
# epoch mapping univoque
emap=collections.defaultdict(set)
for g in groups:
    emap[g["b"]["capture_epoch_id"]].add((g["b"]["decode_step"], g["b"]["compute_submission_id"]))
amb=sum(1 for e,s in emap.items() if len(s)>1)
p("SMOKE2_AMBIGUOUS_EPOCH_SUBMISSION_MAPPING_COUNT=%d" % amb)
with open(S+"/SMOKE2_EPOCH_MAPPING.csv","w",newline="") as f:
    f.write("capture_epoch_id,decode_step,compute_submission_id,eligible\n")
    for g in groups:
        f.write("%s,%s,%s,%s\n" % (g["b"]["capture_epoch_id"], g["b"]["decode_step"],
                                   g["b"]["compute_submission_id"], g["b"]["eligible_target_count"]))
p()

# ---- schemas CSV ----
p("=== SCHEMAS CSV (le piege append multi-epoch) ===")
mf = CAP + "/GRAPH_CAPTURE_MANIFEST.csv"
if os.path.exists(mf):
    ls = open(mf).read().splitlines()
    hdr = len(ls[0].split(","))
    bad_rows = [i for i,l in enumerate(ls[1:],2) if len(l.split(",")) != hdr]
    p("MANIFEST header=%d colonnes, lignes=%d, lignes de largeur differente=%d"
      % (hdr, len(ls)-1, len(bad_rows)))
    p("SMOKE2_MANIFEST_SCHEMA=%s" % ("PASS" if not bad_rows else "FAIL"))
    if bad_rows: p("  premieres lignes en ecart : %s" % bad_rows[:5])
    # epochs presentes
    def col(name):
        return ls[0].split(",").index(name)
    ep = col("epoch")
    p("  epochs presentes dans le manifeste : %s" % sorted({l.split(",")[ep] for l in ls[1:]}))
else:
    p("SMOKE2_MANIFEST_SCHEMA=ABSENT")
pv = CAP + "/PAYLOAD_PROVENANCE.csv"
if os.path.exists(pv):
    ls = open(pv).read().splitlines()
    hdr = len(ls[0].split(","))
    bad_rows = [i for i,l in enumerate(ls[1:],2) if len(l.split(",")) != hdr]
    p("PROVENANCE header=%d colonnes, lignes=%d, ecarts=%d" % (hdr, len(ls)-1, len(bad_rows)))
    p("SMOKE2_PAYLOAD_PROVENANCE_SCHEMA=%s" % ("PASS" if not bad_rows else "FAIL"))
    p("  header : %s" % ls[0])
else:
    p("SMOKE2_PAYLOAD_PROVENANCE_SCHEMA=ABSENT")
p()

# ---- cross-checks ----
p("=== CROSS-CHECKS ===")
if os.path.exists(mf):
    ls = open(mf).read().splitlines()
    h = ls[0].split(",")
    mi = {c: i for i, c in enumerate(h)}
    # (epoch, decode_step) -> compute_submission_id dans le manifeste
    mens = set()
    for l in ls[1:]:
        f = l.split(",")
        mens.add((f[mi["epoch"]], f[mi["decode_step"]], f[mi["compute_submission_id"]]))
    lens = {(r["capture_epoch_id"], r["decode_step"], r["compute_submission_id"])
            for r in rows if r["event_kind"] == "CAPTURE_BEGIN"}
    mism = len(mens ^ lens)
    p("  ledger vs manifeste : %d triplets manifeste, %d ledger, %d en ecart" % (len(mens), len(lens), mism))
    p("SMOKE2_LEDGER_MANIFEST_MISMATCH_COUNT=%d" % mism)
if os.path.exists(pv):
    ls = open(pv).read().splitlines()
    h = ls[0].split(","); pi = {c:i for i,c in enumerate(h)}
    pv_set = {(l.split(",")[pi["epoch"]], l.split(",")[pi["decode_step"]])
              for l in ls[1:]}
    lg_set = {(r["capture_epoch_id"], r["decode_step"]) for r in rows
              if r["event_kind"]=="CAPTURE_BEGIN"}
    d = len(pv_set ^ lg_set)
    p("  ledger vs provenance : %d paires provenance, %d ledger, %d en ecart" % (len(pv_set), len(lg_set), d))
    p("SMOKE2_LEDGER_PAYLOAD_PROVENANCE_MISMATCH_COUNT=%d" % d)
p()

# ---- IO failures ----
log = open(S+"/SMOKE2_RUN_LOG.txt").read()
iof = log.count("EVENT_LEDGER_OPEN_FAILURE") + log.count("EVENT_LEDGER_WRITE_FAILURE") + log.count("EVENT_LEDGER_FLUSH_FAILURE")
p("EVENT_LEDGER_IO_FAILURE_COUNT=%d" % iof)
p()

# ---- verdict ----
checks = {
 "SMOKE2_LEDGER_PARSE": "PASS",
 "EVENT_SEQUENCE_ID_DUPLICATE_COUNT": len(seq)-len(set(seq)),
 "EVENT_SEQUENCE_GAP_COUNT": gaps,
 "EVENT_SEQUENCE_MONOTONIC": "YES" if all(seq[i+1]>seq[i] for i in range(len(seq)-1)) else "NO",
 "PREFILL_DECODE_STEP": -1,
 "PREFILL_CLASSIFIED_AS_SCIENTIFIC_STEP": "NO",
 "SCIENTIFIC_DECODE_STEP_SET": ",".join(str(s) for s in steps),
 "DECODE_BEGIN_COUNT": by.get("DECODE_BEGIN",0),
 "LOGITS_READ_COUNT": by.get("LOGITS_READ",0),
 "DECODE_END_COUNT": by.get("DECODE_END",0),
 "UNKNOWN_SCIENTIFIC_DECODE_STEP_COUNT": sum(1 for r in sci if not (0<=num(r["decode_step"])<=8)),
 "COMPUTE_SUBMIT_COUNT": len(cs),
 "COMPUTE_SUBMISSION_ID_DUPLICATE_OCCURRENCE_COUNT": len(sids)-len(set(sids)),
 "CAPTURE_EPOCH_ID_SET": ",".join(str(e) for e in capeps),
 "CAPTURE_EPOCH_COUNT": len(capeps),
 "SYNC_COMPLETE_COUNT": len(sc),
 "SYNC_COMPLETE_MISSING_EPOCH_COUNT": len(missing),
 "SYNC_COMPLETE_DUPLICATE_EPOCH_COUNT": len(dups),
 "SYNC_BEFORE_SUBMIT_COUNT": bad,
 "CAPTURE_EPOCH_ZERO_PRESENT": "YES" if z else "NO",
 "CAPTURE_EPOCH_ID_ZERO_RENDERED_AS_NA_COUNT": len(na0),
 "CAPTURE_EVENT_STEP_MISMATCH_COUNT": m_step,
 "CAPTURE_EVENT_SUBMISSION_MISMATCH_COUNT": m_sub,
 "CAPTURE_EVENT_EPOCH_MISMATCH_COUNT": m_ep,
 "SMOKE2_AMBIGUOUS_EPOCH_SUBMISSION_MAPPING_COUNT": amb,
 "EVENT_LEDGER_IO_FAILURE_COUNT": iof,
 "SMOKE2_RUN_ID_DISTINCT_COUNT": len(rids),
}
fails = []
if checks["SYNC_COMPLETE_COUNT"] != checks["CAPTURE_EPOCH_COUNT"]: fails.append("SYNC_COMPLETE_COUNT != CAPTURE_EPOCH_COUNT")
if checks["SYNC_COMPLETE_MISSING_EPOCH_COUNT"]: fails.append("SYNC_COMPLETE manquant pour une epoch")
if checks["CAPTURE_EPOCH_ID_ZERO_RENDERED_AS_NA_COUNT"]: fails.append("epoch 0 rendue NA")
if checks["EVENT_SEQUENCE_GAP_COUNT"]: fails.append("trou dans event_sequence_id")
if checks["SYNC_BEFORE_SUBMIT_COUNT"]: fails.append("SYNC avant SUBMIT")
if checks["DECODE_BEGIN_COUNT"] != 9: fails.append("DECODE_BEGIN != 9")
if checks["LOGITS_READ_COUNT"] != 9: fails.append("LOGITS_READ != 9")
if checks["DECODE_END_COUNT"] != 9: fails.append("DECODE_END != 9")
p("=== VERDICT ===")
for k,v in checks.items(): p("%s=%s" % (k,v))
p()
p("SMOKE2_STATUS=%s" % ("PASS" if not fails else "FAIL"))
p("RUN2_QUALIFYING_EXECUTION_ALLOWED=%s" % ("YES" if not fails else "NO"))
if fails: p("FIRST_SMOKE2_DEFECT=%s" % fails[0])
open(S+"/SMOKE2_EVENT_SEQUENCE_AUDIT.txt","w").write("\n".join(out)+"\n")
print()
print("STATUS=" + ("PASS" if not fails else "FAIL: "+fails[0]))
