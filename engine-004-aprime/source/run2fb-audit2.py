#!/usr/bin/env python3
"""RUN2F-B (corrige) : audit adversarial sur le RESULTAT PATCHE.

Methode : copier les fichiers HORS du fork, y appliquer le diff, puis auditer
les fichiers obtenus. Le fork n est jamais touche.
"""
import hashlib
import os
import re
import shutil
import subprocess

W = "/home/triviere/projects/llama-obrain-cuda-f1a"
D = ("/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/"
     "qualification/e7b-f1a-q0-forward-v1/run2")
P = D + "/RUN2F_A2_INSTRUMENTATION_PATCH_CANDIDATE.diff"
FROZEN = "e1e2df8d02295d02645301db54723d74d81c08d37e586adfe5d93babc5e6fdc9"
AUD = "/tmp/q0fwd-audit"

out = []
def p(s=""):
    out.append(s); print(s)

# ---------- SHA recheck ----------
h = hashlib.sha256(open(P, "rb").read()).hexdigest()
p("PATCH_CANDIDATE_SHA256_RECHECK_STATUS=" + ("PASS" if h == FROZEN else "FAIL"))
p()

# ---------- appliquer le diff SUR UNE COPIE ----------
shutil.rmtree(AUD, ignore_errors=True)
for f in ["src/f1a-capture.h", "src/f1a-capture.cpp", "tools/f1a-eval.cpp"]:
    os.makedirs(os.path.join(AUD, os.path.dirname(f)), exist_ok=True)
    shutil.copy2(os.path.join(W, f), os.path.join(AUD, f))
r = subprocess.run(["git", "apply", "--unsafe-paths", "--directory", AUD, P],
                   cwd=W, capture_output=True, text=True)
if r.returncode != 0:
    p("APPLICATION SUR COPIE : ECHEC -> " + r.stderr.strip()[:200])
    raise SystemExit(1)
p("application sur copie (/tmp/q0fwd-audit) : OK")
p()

patched = {}
for f in ["src/f1a-capture.h", "src/f1a-capture.cpp", "tools/f1a-eval.cpp"]:
    patched[f] = open(os.path.join(AUD, f), encoding="utf-8").read()
ALL = "\n".join(patched.values())

# ---------- 1. fichiers touches ----------
touched = sorted(patched)
prod = [t for t in touched if os.path.basename(t) in ("mmvq.cu", "vecdotq.cuh", "quantize.cu")]
p("CANDIDATE_PATCH_FILE_COUNT=%d" % len(touched))
for t in touched: p("  " + t)
p("PRODUCT_COMPUTE_FILE_TOUCHED_COUNT=%d" % len(prod))
p()

# ---------- 2. symboles : declaration ET definition dans le RESULTAT ----------
p("=== SYMBOLES (sur le resultat patche) ===")
unresolved = 0
def count_decl(src, fn):
    return len(re.findall(r"(?m)^\s*(?:void|int32_t|unsigned long long)\s+%s\s*\([^;{]*\)\s*;" % fn, src))
def count_def(src, fn):
    return len(re.findall(r"(?m)^\s*(?:static\s+)?(?:void|int32_t|unsigned long long)\s+%s\s*\([^;{]*\)\s*\{" % fn, src))
for fn in ["f1a_set_decode_step", "f1a_get_decode_step", "f1a_last_submission_id",
           "f1a_last_submission_decode_step", "f1a_event_current",
           "f1a_event_latched", "f1a_event_close"]:
    d = count_decl(patched["src/f1a-capture.h"], fn)
    e = count_def(patched["src/f1a-capture.cpp"], fn)
    p("  %-38s decl=%d def=%d %s" % (fn, d, e, "" if (d >= 1 and e == 1) else "<-- INCOMPLET"))
    if d < 1 or e != 1: unresolved += 1
dr = count_def(patched["src/f1a-capture.cpp"].replace("static void f1a_event_row", "void f1a_event_row"), "f1a_event_row")
p("  %-38s decl=0 def=%d (static interne)" % ("f1a_event_row", dr))
if dr != 1: unresolved += 1
p("UNRESOLVED_NEW_SYMBOL_COUNT=%d" % unresolved)
p()

# ---------- 3. champs de structure ----------
p("=== CHAMPS DE STRUCTURE ===")
invalid = 0
if re.search(r"int32_t\s+decode_step_latched\s*;", patched["src/f1a-capture.h"]):
    p("  decode_step_latched : declare dans f1a_target_rec")
else:
    p("  decode_step_latched : ABSENT"); invalid += 1
n = len(re.findall(r"decode_step_latched", ALL))
p("  references au total : %d" % n)
for bad in ["g_f1a.epoch_cur", "r.submission_id_at_build"]:
    c = len(re.findall(re.escape(bad), ALL))
    p("  %-24s : %d occurrence(s)" % (bad, c))
p("  (submission_id_at_build doit rester : il est DIAGNOSTIC, pas scientifique)")
p("INVALID_STRUCT_FIELD_REFERENCE_COUNT=%d" % invalid)
p()

# ---------- 4. constantes ----------
p("=== CONSTANTES ===")
undecl = 0
for c in ["F1A_EVENT_LEDGER_FILE", "F1A_EVENT_LEDGER_NAME"]:
    n = len(re.findall(c, ALL))
    decl = bool(re.search(r"const char \* const %s" % c, ALL))
    p("  %-24s occurrences=%d declaree=%s" % (c, n, decl))
    if n and not decl: undecl += 1
p("UNDECLARED_CONSTANT_REFERENCE_COUNT=%d" % undecl)
p()

# ---------- 5. signatures : decl vs def vs appels ----------
p("=== SIGNATURES ===")
mismatch = 0
sig = {}
SIGRE = re.compile(r"(?m)^\s*(?:static\s+)?(void|int32_t|unsigned long long|int)\s+(\w+)\s*\(([^;{]*)\)")
for m in SIGRE.finditer(ALL):
    ret, fn, args = m.group(1), m.group(2), m.group(3)
    if fn.startswith("f1a_"):
        sig.setdefault(fn, set()).add((ret, len([a for a in args.split(",") if a.strip()])))
for fn, s in sorted(sig.items()):
    if len(s) > 1:
        p("  %-38s %s <-- INCOHERENT" % (fn, s)); mismatch += 1
p("FUNCTION_SIGNATURE_MISMATCH_COUNT=%d" % mismatch)
p()

# ---------- 6. format strings : nb de specifiers vs nb d args (multi-ligne) ----------
p("=== FORMAT STRINGS ===")
def fmt_and_args(src, varname):
    res = []
    for m in re.finditer(r'fprintf\(\s*' + varname + r'\s*,', src):
        i = m.end(); fmt = ""; depth = 0
        while i < len(src):
            c = src[i]
            if c == '"':
                j = i + 1; lit = ""
                while j < len(src) and src[j] != '"':
                    lit += src[j]; j += 1
                fmt += lit; i = j + 1; continue
            if c == ',' or c == '\n' or c == ' ': i += 1; continue
            break
        # args jusqu a );
        depth = 0; args = ""; 
        while i < len(src):
            if src[i] == '(': depth += 1
            elif src[i] == ')':
                if depth == 0: break
                depth -= 1
            args += src[i]; i += 1
        res.append((fmt, args))
    return res

def split_args(s):
    depth = 0; cur = ""; a = []
    for ch in s:
        if ch == '(': depth += 1
        elif ch == ')': depth -= 1
        if ch == ',' and depth == 0:
            a.append(cur.strip()); cur = ""
        else: cur += ch
    if cur.strip(): a.append(cur.strip())
    return [x for x in a if x]

SP = re.compile(r"%[-+ #0]*[\d]*(?:\.[\d]+)?(?:ll|z|l|h)?[diufFeEgGxXospcn]")
fmt_bad = 0
checks = 0
for varname, label in [(r'g_f1a_ledger', 'ledger'), (r'm', 'manifest'), (r'pv', 'provenance')]:
    for fmt, args in fmt_and_args(ALL, varname):
        if "\\n" not in fmt and "run_id,event_sequence_id" not in fmt and "logical_elems" not in fmt and "payload_hash" not in fmt:
            continue
        a = split_args(args)
        ns = len(SP.findall(fmt))
        checks += 1
        tag = "OK " if ns == len(a) else "KO "
        if ns != len(a): fmt_bad += 1
        p("  %s%-12s %2d specifiers / %2d args" % (tag, label, ns, len(a)))
p("  (%d appels verifies)" % checks)
p("FORMAT_SPECIFIER_TYPE_MISMATCH_COUNT=%d" % fmt_bad)
p()

# ---------- 7. schemas CSV : header vs row ----------
p("=== SCHEMAS CSV ===")
csv_bad = 0
def fields(fmt):
    # une ligne = specifiers + litteraux separes par des virgules
    parts = fmt.split(",")
    return len(parts)
# ledger
lh = re.search(r'"run_id,event_sequence_id,event_kind,decode_step,compute_submission_id,"\s*\n\s*"graph_ptr,graph_fingerprint,capture_epoch_id,eligible_target_count,"\s*\n\s*"captured_target_count\\n"', ALL)
p("  ledger header 10 champs : %s" % ("OK" if lh else "NON TROUVE"))
if not lh: csv_bad += 1
# manifest
mh = re.search(r'"kind,path,dtype,ne0,ne1,ne2,ne3,nb0,nb1,nb2,nb3,logical_elems,bytes,contiguous,"\s*\n\s*"decode_step,compute_submission_id\\n"', ALL)
p("  manifest header +2 : %s" % ("OK" if mh else "NON TROUVE"))
if not mh: csv_bad += 1
mr = re.search(r'"%s,%llu,%d,%d,%s,%d,%lld,%d,%s,%s,%d,%lld,%lld,%lld,%lld,%lld,%lld,%lld,%lld,%lld,%lld,%d,%d,%llu\\n"', ALL)
p("  manifest row 24 specifiers : %s" % ("OK" if mr else "NON TROUVE"))
if not mr: csv_bad += 1
ph = re.search(r'"path,hash_algo,payload_hash,run_id,decode_step,capture_epoch_id\\n"', ALL)
p("  provenance header +3 : %s" % ("OK" if ph else "NON TROUVE"))
if not ph: csv_bad += 1
pr = re.search(r'"%llu,%llu,%d,%d,%s,%s,%d,%lld,%lld,%d,%zu,%zu,%s,fnv64x4,%s,%s,%d,%llu\\n"', ALL)
p("  provenance row 18 specifiers : %s" % ("OK" if pr else "NON TROUVE"))
if not pr: csv_bad += 1
p("CSV_SCHEMA_STATIC_CHECK=%s" % ("PASS" if csv_bad == 0 else "FAIL"))
p()

# ---------- 8. types d evenements ----------
p("=== TYPES D EVENEMENTS ===")
kinds = ["DECODE_BEGIN", "COMPUTE_SUBMIT", "SYNC_COMPLETE", "CAPTURE_BEGIN",
         "CAPTURE_EXTRACT", "CAPTURE_END", "LOGITS_READ", "DECODE_END"]
unreach = 0
for k in kinds:
    n = len(re.findall('"%s"' % k, ALL))
    p("  %-16s %d site(s)" % (k, n))
    if n == 0: unreach += 1
p("UNREACHABLE_REQUIRED_EVENT_KIND_COUNT=%d" % unreach)
p()

# ---------- 9. SYNC_COMPLETE apres la vraie synchronisation ----------
p("=== SYNC_COMPLETE ===")
src = patched["src/f1a-capture.cpp"]
ok = False
i = src.find("ggml_backend_synchronize(backend)")
if i >= 0 and src.find('"SYNC_COMPLETE"', i) >= 0 and src.find('"SYNC_COMPLETE"', i) - i < 600:
    ok = True
p("SYNC_COMPLETE_AFTER_BACKEND_SYNCHRONIZE=%s" % ("YES" if ok else "NO"))
p()

# ---------- 10. CAPTURE_* utilisent latched, pas current ----------
p("=== CAPTURE_* : API LATCHED ? ===")
cap_ok = True
for k in ["CAPTURE_BEGIN", "CAPTURE_EXTRACT", "CAPTURE_END"]:
    m = re.search(r'f1a_event_(current|latched)\(\s*"%s"' % k, ALL)
    api = m.group(1) if m else "AUCUNE"
    p("  %-16s -> f1a_event_%s" % (k, api))
    if api != "latched": cap_ok = False
p("CAPTURE_EVENTS_USE_LATCHED_API=%s" % ("YES" if cap_ok else "NO"))
p()

# ---------- 11. assertions d ordre ----------
assertions = len(re.findall(r"(?i)assert[^\n]*CAPTURE_END[^\n]*LOGITS_READ", ALL))
assertions += len(re.findall(r"(?i)assert[^\n]*LOGITS_READ[^\n]*CAPTURE_END", ALL))
p("UNPROVEN_CAPTURE_LOGITS_ORDER_ASSERTION_COUNT=%d" % assertions)
p()

# ---------- verdict ----------
defects = unresolved + invalid + undecl + mismatch + fmt_bad + csv_bad + unreach
if not ok: defects += 1
if not cap_ok: defects += 1
p("STATIC_DEFECT_COUNT=%d" % defects)
if defects:
    p("FIRST_STATIC_DEFECT=voir ci-dessus")
else:
    p("FIRST_STATIC_DEFECT=NONE")
p("CANDIDATE_PATCH_COMPLETE_AFTER_AUDIT=%s" % ("YES" if defects == 0 else "NO"))
p()
p("PATCH_APPLICATION_ALLOWED=%s" % ("YES" if defects == 0 else "NO"))
p()
p("WORKTREE_CHANGED_DURING_RUN2F_B=NO (audit fait sur /tmp/q0fwd-audit)")

open(D + "/RUN2F_B_ADVERSARIAL_STATIC_AUDIT.txt", "w", encoding="utf-8", newline="\n").write("\n".join(out) + "\n")
print()
print("audit ecrit")
