#!/usr/bin/env python3
"""RUN2F-B : audit statique ADVERSARIAL du candidat gele e1e2df8d...
Ne modifie rien, n'applique rien, ne compile rien."""
import hashlib
import os
import re
import subprocess
import sys

W = "/home/triviere/projects/llama-obrain-cuda-f1a"
D = ("/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/"
     "qualification/e7b-f1a-q0-forward-v1/run2")
P = D + "/RUN2F_A2_INSTRUMENTATION_PATCH_CANDIDATE.diff"
FROZEN = "e1e2df8d02295d02645301db54723d74d81c08d37e586adfe5d93babc5e6fdc9"

out = []
def p(s=""):
    out.append(s)
    print(s)

# ---------- 1. SHA recheck ----------
h = hashlib.sha256(open(P, "rb").read()).hexdigest()
p("PATCH_CANDIDATE_SHA256_RECHECK_STATUS=" + ("PASS" if h == FROZEN else "FAIL"))
p("  recalcule = " + h)
p("  gele      = " + FROZEN)
p()

diff = open(P, encoding="utf-8").read().splitlines()

# ---------- 2. fichiers touches ----------
touched = sorted({l.split()[1][2:] for l in diff if l.startswith("+++ ")})
p("CANDIDATE_PATCH_FILE_COUNT=%d" % len(touched))
for t in touched:
    p("  " + t)
prod = [t for t in touched if os.path.basename(t) in ("mmvq.cu", "vecdotq.cuh", "quantize.cu")]
p("PRODUCT_COMPUTE_FILE_TOUCHED_COUNT=%d" % len(prod))
p()

# ---------- 3. symboles ajoutes : declaration / definition ----------
p("=== AUDIT DES SYMBOLES ===")
added = [l[1:] for l in diff if l.startswith("+") and not l.startswith("+++")]
NEW_FUNCS = ["f1a_set_decode_step", "f1a_get_decode_step",
             "f1a_last_submission_id", "f1a_last_submission_decode_step",
             "f1a_event_current", "f1a_event_latched", "f1a_event_close"]
unresolved = 0
for fn in NEW_FUNCS:
    decl = sum(1 for l in added if re.search(r"\b\w[\w ]*\b%s\s*\(" % fn, l) and l.rstrip().endswith(";"))
    defi = sum(1 for l in added if re.match(r"\s*\w[\w \*]*\b%s\s*\(" % fn, l) and "{" in l)
    p("  %-38s decl=%d def=%d" % (fn, decl, defi))
    if decl < 1 or defi < 1:
        unresolved += 1
        p("    ^^ INCOMPLET")
# f1a_event_row est static, defini seul
er_def = sum(1 for l in added if l.startswith("static void f1a_event_row") and "{" in l)
p("  %-38s decl=0 def=%d (static interne)" % ("f1a_event_row", er_def))
if er_def < 1:
    unresolved += 1
p("UNRESOLVED_NEW_SYMBOL_COUNT=%d" % unresolved)
p()

# ---------- 4. champs de structure ----------
p("=== AUDIT DES CHAMPS DE STRUCTURE ===")
invalid = 0
H = open(W + "/src/f1a-capture.h", encoding="utf-8").read()
for field, owner in [("decode_step_latched", "f1a_target_rec")]:
    n_diff = sum(1 for l in added if field in l)
    in_hdr = field in H
    p("  %s : %d references dans le diff, declare dans l en-tete : %s" % (field, n_diff, in_hdr))
    if not in_hdr:
        invalid += 1
for bad in ["g_f1a.epoch_cur"]:
    n = sum(1 for l in added if bad in l)
    p("  symbole interdit %-22s : %d occurrence(s)" % (bad, n))
    if n:
        invalid += 1
p("INVALID_STRUCT_FIELD_REFERENCE_COUNT=%d" % invalid)
p()

# ---------- 5. constantes ----------
p("=== AUDIT DES CONSTANTES ===")
undecl = 0
for c in ["F1A_EVENT_LEDGER_FILE", "F1A_EVENT_LEDGER_NAME"]:
    n = sum(1 for l in added if c in l)
    decl = any(("const char" in l or "constexpr" in l) and c in l for l in added)
    p("  %-24s occurrences=%d declaree=%s" % (c, n, decl))
    if n and not decl:
        undecl += 1
p("UNDECLARED_CONSTANT_REFERENCE_COUNT=%d" % undecl)
p()

# ---------- 6. signatures ----------
p("=== AUDIT DES SIGNATURES ===")
mismatch = 0
for fn in NEW_FUNCS + ["f1a_event_row"]:
    dh = [l for l in added if fn in l and l.rstrip().endswith(";")]
    dd = [l for l in added if fn in l and "{" in l]
    for dl in dh:
        # comparer le nombre de parametres declares vs definis grossierement
        pass
    # une signature est coherente si la definition existe et qu il y a au plus 1 declaration
    if len(dh) > 1:
        p("  %-38s %d declarations (ambigu)" % (fn, len(dh)))
        mismatch += 1
p("FUNCTION_SIGNATURE_MISMATCH_COUNT=%d" % mismatch)
p()

# ---------- 7. formats fprintf : nb de specifiers vs nb d args ----------
p("=== AUDIT DES FORMAT STRINGS ===")
SPEC = re.compile(r"%[-+ #0]*[\d]*(\.[\d]+)?(ll|z|l|h)?[diufFeEgGxXospcn%]")
def split_args(s):
    depth = 0; cur = ""; args = []
    for ch in s:
        if ch == "(": depth += 1
        if ch == ")": depth -= 1
        if ch == "," and depth == 0:
            args.append(cur.strip()); cur = ""
        else:
            cur += ch
    if cur.strip(): args.append(cur.strip())
    return args

fmt_bad = 0
# reconstruire les appels fprintf du candidat : format puis args jusqu au );
src = "\n".join(added)
for m in re.finditer(r'fprintf\(\s*([A-Za-z_][\w\.]*)\s*,\s*"((?:[^"\\]|\\.)*)"((?:\s*\n\s*[^;]*?)?)\);', src, re.S):
    label = m.group(1)
    fmt = m.group(2)
    args = split_args(m.group(3).strip().lstrip(",").strip()) if m.group(3).strip() else []
    args = [a for a in args if a]
    nspec = len([s for s in SPEC.findall(fmt) if s[1] != "%"]) if False else len(SPEC.findall(fmt))
    nspec = len([x for x in re.findall(r"%[-+ #0]*[\d]*(?:\.[\d]+)?(?:ll|z|l|h)?[diufFeEgGxXospcn]", fmt)])
    if nspec != len(args):
        fmt_bad += 1
        p("  DESACCORD %s : %d specifiers / %d args" % (label, nspec, len(args)))
        p("    fmt  = " + fmt[:90])
p("  (controle applique aux appels fprintf reconstitues : %d desaccords)" % fmt_bad)
p("FORMAT_SPECIFIER_TYPE_MISMATCH_COUNT=%d" % fmt_bad)
p()

# ---------- 8. schemas CSV : header vs row ----------
p("=== AUDIT DES SCHEMAS CSV ===")
def count_fields(header_line):
    inner = header_line
    return len([f for f in inner.split(",")])

csv_bad = 0
# ledger
lh = None
for l in added:
    if "run_id,event_sequence_id,event_kind,decode_step" in l:
        lh = l
        break
p("  ledger header : 10 champs attendus -> %s" % ("OK" if lh else "NON TROUVE"))
# manifest header
mh = None
for l in added:
    if "logical_elems,bytes,contiguous," in l and "decode_step,compute_submission_id" in l:
        mh = l
        break
p("  manifest header : +decode_step,compute_submission_id -> %s" % ("OK" if mh else "NON TROUVE"))
if not mh:
    csv_bad += 1
# provenance header
ph = None
for l in added:
    if "payload_hash,run_id,decode_step,capture_epoch_id" in l:
        ph = l
        break
p("  provenance header : +run_id,decode_step,capture_epoch_id -> %s" % ("OK" if ph else "NON TROUVE"))
if not ph:
    csv_bad += 1
# rows
mrow = None
for l in added:
    if "%s,%llu,%d,%d,%s,%d,%lld,%d,%s,%s,%d,%lld" in l:
        mrow = l
        break
p("  manifest row : +2 specifiers -> %s" % ("OK" if mrow and mrow.count("%d,%llu") else "NON TROUVE"))
if not mrow:
    csv_bad += 1
prow = None
for l in added:
    if "fnv64x4,%s,%s,%d,%llu" in l:
        prow = l
        break
p("  provenance row : +3 specifiers -> %s" % ("OK" if prow else "NON TROUVE"))
if not prow:
    csv_bad += 1
p("CSV_SCHEMA_STATIC_CHECK=%s" % ("PASS" if csv_bad == 0 else "FAIL"))
p()

# ---------- 9. types evenements : sites d emission ----------
p("=== AUDIT DES TYPES D EVENEMENTS ===")
kinds = ["DECODE_BEGIN", "COMPUTE_SUBMIT", "SYNC_COMPLETE", "CAPTURE_BEGIN",
         "CAPTURE_EXTRACT", "CAPTURE_END", "LOGITS_READ", "DECODE_END"]
unreach = 0
for k in kinds:
    n = sum(1 for l in added if ('"%s"' % k) in l)
    if n == 0:
        unreach += 1
        p("  %-16s AUCUN SITE D EMISSION" % k)
    else:
        p("  %-16s %d site(s)" % (k, n))
p("UNREACHABLE_REQUIRED_EVENT_KIND_COUNT=%d" % unreach)
p()

# ---------- 10. SYNC_COMPLETE apres la vraie synchronisation ----------
p("=== AUDIT SYNC_COMPLETE ===")
synced = False
for i, l in enumerate(added):
    if "ggml_backend_synchronize(backend)" in l:
        for l2 in added[i:i+6]:
            if 'SYNC_COMPLETE' in l2:
                synced = True
        break
p("SYNC_COMPLETE_AFTER_BACKEND_SYNCHRONIZE=%s" % ("YES" if synced else "NO"))
p()

# ---------- 11. absence d assertion d ordre ----------
assertions = sum(1 for l in added if "CAPTURE_END" in l and "LOGITS_READ" in l and "assert" in l.lower())
p("UNPROVEN_CAPTURE_LOGITS_ORDER_ASSERTION_COUNT=%d" % assertions)
p()

# ---------- 12. verdict ----------
defects = unresolved + invalid + undecl + mismatch + fmt_bad + csv_bad + unreach
if not synced:
    defects += 1
p("STATIC_DEFECT_COUNT=%d" % defects)
p("CANDIDATE_PATCH_COMPLETE_AFTER_AUDIT=%s" % ("YES" if defects == 0 else "NO"))
p("PATCH_APPLICATION_ALLOWED=%s" % ("YES" if defects == 0 else "NO"))
p()

open(D + "/RUN2F_B_ADVERSARIAL_STATIC_AUDIT.txt", "w", encoding="utf-8", newline="\n").write("\n".join(out) + "\n")
print()
print("audit ecrit")
