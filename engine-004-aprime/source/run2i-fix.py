#!/usr/bin/env python3
"""RUN2I : correction C. decode_step=-1 doit etre serialise "-1" partout."""
import difflib, hashlib, os, shutil, sys

W = "/home/triviere/projects/llama-obrain-cuda-f1a"
TMP = "/tmp/q0fwd-i"
D = ("/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/"
     "qualification/e7b-f1a-q0-forward-v1/run2")
OUT = D + "/RUN2I_DECODE_STEP_SERIALIZATION_FIX.diff"
FILES = ["src/f1a-capture.cpp"]

shutil.rmtree(TMP, ignore_errors=True)
for f in FILES:
    os.makedirs(os.path.join(TMP, os.path.dirname(f)), exist_ok=True)
    shutil.copy2(os.path.join(W, f), os.path.join(TMP, f))
J = "\n"

p = os.path.join(TMP, "src/f1a-capture.cpp")
s = open(p, encoding="utf-8").read()
old = J.join([
 "    char dsbuf[16];",
 '    if (decode_step < 0) snprintf(dsbuf, sizeof dsbuf, "NA");',
 '    else                 snprintf(dsbuf, sizeof dsbuf, "%d", decode_step);',
])
new = J.join([
 "    char dsbuf[16];",
 "    // RUN2I : politique canonique de decode_step. -1 EST la valeur semantique",
 "    // « hors pas scientifique » : elle se serialise \"-1\", jamais \"NA\".",
 "    // Le manifeste et la provenance ecrivent deja -1 par %d : les trois",
 "    // artefacts partagent desormais la MEME representation, ce qui rend la",
 "    // jointure ledger <-> manifeste <-> provenance possible sans normalisation.",
 "    // (capture_epoch_id garde sa politique distincte : applicabilite + valeur.)",
 '    snprintf(dsbuf, sizeof dsbuf, "%d", decode_step);',
])
n = s.count(old)
if n != 1:
    print("ANCRE KO : %d occurrences" % n); sys.exit(1)
open(p, "w", encoding="utf-8").write(s.replace(old, new, 1))
print("  ok  DECODE_STEP_SERIALIZATION_CANONICALIZATION")

lines = []
for f in FILES:
    a = open(os.path.join(W, f), encoding="utf-8").read().splitlines(keepends=True)
    b = open(os.path.join(TMP, f), encoding="utf-8").read().splitlines(keepends=True)
    d = list(difflib.unified_diff(a, b, fromfile="a/" + f, tofile="b/" + f, n=3))
    if d:
        lines.extend(d)
txt = "".join(lines)
open(OUT, "w", encoding="utf-8", newline="\n").write(txt)
h = hashlib.sha256(txt.encode()).hexdigest()
open(D + "/RUN2I_DECODE_STEP_SERIALIZATION_FIX.sha256", "w").write(
    h + "  RUN2I_DECODE_STEP_SERIALIZATION_FIX.diff\n")
add = sum(1 for l in lines if l.startswith("+") and not l.startswith("+++"))
rem = sum(1 for l in lines if l.startswith("-") and not l.startswith("---"))
hunks = sum(1 for l in lines if l.startswith("@@"))
print()
print("RUN2I_PATCH_SHA256=" + h)
print("FILES=%d HUNKS=%d ADDED=%d REMOVED=%d" % (len(FILES), hunks, add, rem))
print("CLASSIFICATION: DECODE_STEP_SERIALIZATION_CANONICALIZATION=%d hunk(s), OTHER=0" % hunks)
