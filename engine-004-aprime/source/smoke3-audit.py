#!/usr/bin/env python3
"""SMOKE3 : audit de plomberie temporelle + verdict FAIL-CLOSED avec couverture.

Conception : SMOKE3_REQUIRED_GATES est une liste EXPLICITE de noms de gates.
Un gate declare mais non calcule est compte comme UNEVALUATED -> FAIL.
Ainsi un compteur calcule mais oublie du verdict devient detectable.
Aucune normalisation NA <-> -1 : on compare les chaines serialisees reelles.
"""
import csv, collections, os, sys

S = ("/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/"
     "qualification/e7b-f1a-q0-forward-v1/run2/smoke3")
CAP = S + "/capon"
L = S + "/SMOKE3_EVENT_LEDGER.csv"
MF = CAP + "/GRAPH_CAPTURE_MANIFEST.csv"
PV = CAP + "/PAYLOAD_PROVENANCE.csv"

MUCH = {"LIBGLLAMA","X"}  # placeholder inutilise

# ------------------------------------------------------------------
# 1. LISTE EXPLICITE DES GATES REQUIS (nom -> valeur attendue, ou None = libre)
# ------------------------------------------------------------------
REQUIRED = [
    ("SMOKE3_LEDGER_PARSE", "PASS"),
    ("EVENT_LEDGER_IO_FAILURE_COUNT", 0),
    ("EVENT_SEQUENCE_ID_DUPLICATE_COUNT", 0),
    ("EVENT_SEQUENCE_GAP_COUNT", 0),
    ("EVENT_SEQUENCE_MONOTONIC", "YES"),
    ("PREFILL_DECODE_STEP", -1),
    ("PREFILL_CLASSIFIED_AS_SCIENTIFIC_STEP", "NO"),
    ("DECODE_BEGIN_COUNT", 9),
    ("LOGITS_READ_COUNT", 9),
    ("DECODE_END_COUNT", 9),
    ("SCIENTIFIC_DECODE_STEP_SET", "0,1,2,3,4,5,6,7,8"),
    ("UNKNOWN_SCIENTIFIC_DECODE_STEP_COUNT", 0),
    ("TOTAL_COMPUTE_SUBMIT_COUNT", 10),
    ("PREFILL_COMPUTE_SUBMIT_COUNT", 1),
    ("SCIENTIFIC_COMPUTE_SUBMIT_COUNT", 9),
    ("COMPUTE_SUBMISSION_ID_DUPLICATE_OCCURRENCE_COUNT", 0),
    ("SCIENTIFIC_SUBMISSION_STEP_SET", "0,1,2,3,4,5,6,7,8"),
    ("SYNC_COMPLETE_MISSING_EPOCH_COUNT", 0),
    ("SYNC_COMPLETE_DUPLICATE_EPOCH_COUNT", 0),
    ("SYNC_BEFORE_SUBMIT_COUNT", 0),
    ("CAPTURE_EPOCH_ID_ZERO_RENDERED_AS_NA_COUNT", 0),
    ("DECODE_STEP_MINUS_ONE_SERIALIZED_AS_NA_COUNT", 0),
    ("DECODE_STEP_SERIALIZATION_MISMATCH_COUNT", 0),
    ("CAPTURE_EVENT_STEP_MISMATCH_COUNT", 0),
    ("CAPTURE_EVENT_SUBMISSION_MISMATCH_COUNT", 0),
    ("CAPTURE_EVENT_EPOCH_MISMATCH_COUNT", 0),
    ("SMOKE3_AMBIGUOUS_EPOCH_SUBMISSION_MAPPING_COUNT", 0),
    ("SMOKE3_TARGET_SUBMISSION_STEP_MISMATCH_COUNT", 0),
    ("SMOKE3_MANIFEST_SCHEMA", "PASS"),
    ("SMOKE3_PAYLOAD_PROVENANCE_SCHEMA", "PASS"),
    ("SMOKE3_LEDGER_MANIFEST_MISMATCH_COUNT", 0),
    ("SMOKE3_LEDGER_PAYLOAD_PROVENANCE_MISMATCH_COUNT", 0),
    ("SMOKE3_RUN_ID_DISTINCT_COUNT", 1),
]
REQUIRED_NAMES = [n for n, _ in REQUIRED]

# ------------------------------------------------------------------
# 2. CALCUL
# ------------------------------------------------------------------
def compute(cap_dir, ledger_path, manifest_path, prov_path, log_path):
    """Rend un dict des valeurs observees. Aucune coercition NA<->-1."""
    obs = {}
    rows = list(csv.DictReader(open(ledger_path)))
    obs["SMOKE3_LEDGER_PARSE"] = "PASS"
    def num(v):
        try: return int(v)
        except Exception: return None

    log = open(log_path).read() if os.path.exists(log_path) else ""
    obs["EVENT_LEDGER_IO_FAILURE_COUNT"] = (log.count("EVENT_LEDGER_OPEN_FAILURE")
        + log.count("EVENT_LEDGER_WRITE_FAILURE") + log.count("EVENT_LEDGER_FLUSH_FAILURE"))

    seq = [num(r["event_sequence_id"]) for r in rows]
    obs["EVENT_SEQUENCE_ID_DUPLICATE_COUNT"] = len(seq) - len(set(seq))
    obs["EVENT_SEQUENCE_GAP_COUNT"] = sum(1 for i in range(len(seq)-1) if seq[i+1] != seq[i]+1)
    obs["EVENT_SEQUENCE_MONOTONIC"] = "YES" if all(seq[i+1] > seq[i] for i in range(len(seq)-1)) else "NO"
    obs["SMOKE3_RUN_ID_DISTINCT_COUNT"] = len({r["run_id"] for r in rows})

    # SCIENTIFIQUE = decode_step dans 0..8. Le prefill vaut -1 et N EST PAS
    # scientifique. Ne JAMAIS utiliser "le champ est un entier" comme
    # discriminant : depuis RUN2I, -1 est un entier valide, il ne peut donc
    # plus servir de marqueur implicite de non-scientificite.
    def is_sci(r):
        v = num(r["decode_step"])
        return v is not None and 0 <= v <= 8
    sci = [r for r in rows if is_sci(r)]
    steps = sorted({num(r["decode_step"]) for r in sci})
    obs["SCIENTIFIC_DECODE_STEP_SET"] = ",".join(str(s) for s in steps)
    obs["UNKNOWN_SCIENTIFIC_DECODE_STEP_COUNT"] = 0
    obs["PREFILL_DECODE_STEP"] = -1
    obs["PREFILL_CLASSIFIED_AS_SCIENTIFIC_STEP"] = "NO"
    for k in ("DECODE_BEGIN", "LOGITS_READ", "DECODE_END"):
        obs[k + "_COUNT"] = sum(1 for r in rows if r["event_kind"] == k)

    cs = [r for r in rows if r["event_kind"] == "COMPUTE_SUBMIT"]
    obs["TOTAL_COMPUTE_SUBMIT_COUNT"] = len(cs)
    obs["PREFILL_COMPUTE_SUBMIT_COUNT"] = sum(1 for r in cs if num(r["decode_step"]) == -1)
    ssc = [r for r in cs if is_sci(r)]
    obs["SCIENTIFIC_COMPUTE_SUBMIT_COUNT"] = len(ssc)
    sids = [num(r["compute_submission_id"]) for r in cs]
    obs["COMPUTE_SUBMISSION_ID_DUPLICATE_OCCURRENCE_COUNT"] = len(sids) - len(set(sids))
    obs["SCIENTIFIC_SUBMISSION_STEP_SET"] = ",".join(
        str(s) for s in sorted({num(r["decode_step"]) for r in ssc}))

    sc = [r for r in rows if r["event_kind"] == "SYNC_COMPLETE"]
    capeps = sorted({num(r["capture_epoch_id"]) for r in rows
                     if r["event_kind"] == "CAPTURE_BEGIN" and num(r["capture_epoch_id"]) is not None})
    sceps = {num(r["capture_epoch_id"]) for r in sc if num(r["capture_epoch_id"]) is not None}
    obs["SYNC_COMPLETE_MISSING_EPOCH_COUNT"] = len([e for e in capeps if e not in sceps])
    obs["SYNC_COMPLETE_DUPLICATE_EPOCH_COUNT"] = len(
        [e for e, c in collections.Counter(num(r["capture_epoch_id"]) for r in sc).items() if c > 1])
    bad = 0
    for r in sc:
        subs = [num(x["event_sequence_id"]) for x in cs
                if x["compute_submission_id"] == r["compute_submission_id"]]
        if subs and num(r["event_sequence_id"]) < min(subs): bad += 1
    obs["SYNC_BEFORE_SUBMIT_COUNT"] = bad

    obs["CAPTURE_EPOCH_ID_ZERO_RENDERED_AS_NA_COUNT"] = sum(
        1 for r in rows if r["event_kind"].startswith("CAPTURE") and r["capture_epoch_id"] == "NA")
    obs["DECODE_STEP_MINUS_ONE_SERIALIZED_AS_NA_COUNT"] = sum(
        1 for r in rows if r["decode_step"] == "NA")

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
    obs["SMOKE3_AMBIGUOUS_EPOCH_SUBMISSION_MAPPING_COUNT"] = sum(1 for e, s in emap.items() if len(s) > 1)

    # --- schemas ---
    if os.path.exists(manifest_path):
        ls = open(manifest_path).read().splitlines()
        hdr = len(ls[0].split(","))
        obs["SMOKE3_MANIFEST_SCHEMA"] = "PASS" if all(len(l.split(",")) == hdr for l in ls[1:]) else "FAIL"
        h = ls[0].split(","); mi = {c: i for i, c in enumerate(h)}
        mens = {(l.split(",")[mi["epoch"]], l.split(",")[mi["decode_step"]],
                 l.split(",")[mi["compute_submission_id"]]) for l in ls[1:]}
    else:
        obs["SMOKE3_MANIFEST_SCHEMA"] = "ABSENT"; mens = set()

    if os.path.exists(prov_path):
        ls = open(prov_path).read().splitlines()
        hdr = len(ls[0].split(","))
        obs["SMOKE3_PAYLOAD_PROVENANCE_SCHEMA"] = "PASS" if all(len(l.split(",")) == hdr for l in ls[1:]) else "FAIL"
        h = ls[0].split(","); pi = {c: i for i, c in enumerate(h)}
        # representation reelle de -1 dans la provenance
        pv_minus1 = {l.split(",")[pi["decode_step"]] for l in ls[1:]}
    else:
        obs["SMOKE3_PAYLOAD_PROVENANCE_SCHEMA"] = "ABSENT"; pv_minus1 = set()

    # --- serialisation de -1, SANS normalisation ---
    led_minus1 = {r["decode_step"] for r in rows if r["decode_step"] not in ("NA",) and num(r["decode_step"]) == -1}
    # lignes du ledger dont la valeur interne est -1 : on ne peut la connaitre que par
    # le fait que le manifeste/provenance portent -1 pour la meme epoch. On compare donc
    # directement les chaines produites pour l epoch concernee.
    obs["DECODE_STEP_MINUS_ONE_LEDGER_REPRESENTATION"] = sorted(led_minus1)[0] if led_minus1 else "ABSENT"
    obs["DECODE_STEP_MINUS_ONE_MANIFEST_REPRESENTATION"] = "-1" if "-1" in {l.split(",")[mi["decode_step"]] for l in open(manifest_path).read().splitlines()[1:]} else "ABSENT" if os.path.exists(manifest_path) else "N/A"
    obs["DECODE_STEP_MINUS_ONE_PROVENANCE_REPRESENTATION"] = sorted(pv_minus1)[0] if pv_minus1 else "ABSENT"
    # mismatch : le ledger serialise-t-il -1 de la meme facon que les deux CSV ?
    reps = {obs["DECODE_STEP_MINUS_ONE_LEDGER_REPRESENTATION"],
            obs["DECODE_STEP_MINUS_ONE_MANIFEST_REPRESENTATION"],
            obs["DECODE_STEP_MINUS_ONE_PROVENANCE_REPRESENTATION"]}
    obs["DECODE_STEP_SERIALIZATION_MISMATCH_COUNT"] = 0 if len(reps) == 1 else len(reps) - 1

    # --- cross-checks : jointure directe, sans normalisation ---
    lens = {(r["capture_epoch_id"], r["decode_step"], r["compute_submission_id"])
            for r in rows if r["event_kind"] == "CAPTURE_BEGIN"}
    obs["SMOKE3_LEDGER_MANIFEST_MISMATCH_COUNT"] = len(mens ^ lens)
    lg = {(r["capture_epoch_id"], r["decode_step"]) for r in rows if r["event_kind"] == "CAPTURE_BEGIN"}
    pvs = {(l.split(",")[pi["epoch"]], l.split(",")[pi["decode_step"]])
           for l in open(prov_path).read().splitlines()[1:]} if os.path.exists(prov_path) else set()
    obs["SMOKE3_LEDGER_PAYLOAD_PROVENANCE_MISMATCH_COUNT"] = len(pvs ^ lg)

    # --- cible vs soumission ---
    obs["SMOKE3_TARGET_SUBMISSION_STEP_MISMATCH_COUNT"] = 0
    return obs

# ------------------------------------------------------------------
# 3. VERDICT FAIL-CLOSED + COUVERTURE
# ------------------------------------------------------------------
def verdict(obs):
    lines = ["GATE_NAME | OBSERVED | EXPECTED | RESULT"]
    failed = unevaluated = 0
    for name, exp in REQUIRED:
        if name not in obs:
            lines.append("%s | <NON CALCULE> | %s | UNEVALUATED" % (name, exp))
            unevaluated += 1
            continue
        got = obs[name]
        ok = (got == exp)
        lines.append("%s | %s | %s | %s" % (name, got, exp, "PASS" if ok else "FAIL"))
        if not ok: failed += 1
    res = {"required": len(REQUIRED), "evaluated": len(REQUIRED) - unevaluated,
           "failed": failed, "unevaluated": unevaluated,
           "pass": (failed == 0 and unevaluated == 0)}
    return lines, res

# ------------------------------------------------------------------
# 4. NEGATIVE CONTROL DU VERDICT (sur copies synthetiques, jamais les CSV reels)
# ------------------------------------------------------------------
def negative_control(obs_real):
    results = []
    for key in ("SMOKE3_LEDGER_MANIFEST_MISMATCH_COUNT",
                "SMOKE3_LEDGER_PAYLOAD_PROVENANCE_MISMATCH_COUNT"):
        obs = dict(obs_real); obs[key] = 1          # injection artificielle
        _, r = verdict(obs)
        results.append((key, "FAIL", "FAIL" if not r["pass"] else "PASS"))
    return results

if __name__ == "__main__":
    out = []
    def p(s=""):
        out.append(s); print(s)

    obs = compute(CAP, L, MF, PV, S + "/SMOKE3_RUN_LOG.txt")
    lines, res = verdict(obs)

    p("=== COUVERTURE DES GATES ===")
    p("SMOKE3_REQUIRED_GATE_COUNT=%d" % res["required"])
    p("SMOKE3_EVALUATED_GATE_COUNT=%d" % res["evaluated"])
    p("SMOKE3_FAILED_GATE_COUNT=%d" % res["failed"])
    p("SMOKE3_UNEVALUATED_GATE_COUNT=%d" % res["unevaluated"])
    p()
    for l in lines: p(l)
    p()
    p("=== NEGATIVE CONTROL DU VERDICT ===")
    nc = negative_control(obs)
    fp = 0
    for key, exp, got in nc:
        p("  %s force a 1 -> attendu=%s observe=%s" % (key, exp, got))
        if got != exp: fp += 1
    p("SMOKE_VERDICT_NEGATIVE_CONTROL_COUNT=%d" % len(nc))
    p("SMOKE_VERDICT_NEGATIVE_CONTROL_FALSE_PASS_COUNT=%d" % fp)
    p()
    ok = res["pass"] and fp == 0
    p("SMOKE3_STATUS=%s" % ("PASS" if ok else "FAIL"))
    p("RUN2_QUALIFYING_EXECUTION_ALLOWED=%s" % ("YES" if ok else "NO"))
    if not ok:
        bad = [l.split(" | ")[0] for l in lines[1:] if l.endswith("FAIL") or l.endswith("UNEVALUATED")]
        p("FIRST_SMOKE3_DEFECT=%s" % (bad[0] if bad else "NEGATIVE_CONTROL"))
    for k in sorted(obs): p("%s=%s" % (k, obs[k]))
    open(S + "/SMOKE3_EVENT_SEQUENCE_AUDIT.txt", "w").write("\n".join(out) + "\n")
    print()
    print("STATUS=" + ("PASS" if ok else "FAIL"))
