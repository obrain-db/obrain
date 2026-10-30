#!/usr/bin/env python3
"""ENGINE-008-DATA : audit de structure et de comptes des donnees acquises.
Lecture seule. Aucune inference. Aucune selection."""
import glob
import hashlib
import json
import os
import sys

E = ("/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/"
     "qualification/engine-008-quality-v1/external")
Q = os.path.dirname(E)

out = []
def p(s=""):
    out.append(s); print(s)

# ================= MMLU-PRO =================
p("=== MMLU-PRO ===")
import pyarrow.parquet as pq
f = sorted(glob.glob(E + "/mmlu-pro/*.parquet"))
p("  fichiers parquet : %d" % len(f))
if not f:
    p("MMLU_PRO_PARSE_ERROR_COUNT=Echec: aucun parquet")
    sys.exit(1)
t = pq.read_table(f[0])
p("  fichier   : %s" % os.path.basename(f[0]))
p("  sha256    : %s" % hashlib.sha256(open(f[0], "rb").read()).hexdigest())
p("MMLU_PRO_RAW_TEST_COUNT=%d" % t.num_rows)
p()
p("MMLU_PRO_SCHEMA :")
for name, typ in zip(t.column_names, t.schema.types):
    p("    %-16s %s" % (name, typ))
p()
# comptes par categorie
catcol = "category" if "category" in t.column_names else None
counts = {}
parse_err = 0
if catcol:
    for c in t.column(catcol).to_pylist():
        if c is None:
            parse_err += 1
            continue
        counts[str(c)] = counts.get(str(c), 0) + 1
p("MMLU_PRO_CATEGORY_COUNT=%d" % len(counts))
p("MMLU_PRO_CATEGORY_COUNTS :")
low = []
for k in sorted(counts):
    flag = "" if counts[k] >= 100 else "   <-- MOINS DE 100"
    if counts[k] < 100:
        low.append(k)
    p("    %-28s %5d%s" % (k, counts[k], flag))
p()
p("MMLU_PRO_CATEGORIES_BELOW_100=%d" % len(low))
if low:
    p("  -> %s" % ", ".join(low))
p("MMLU_PRO_SELECTION_TARGET_IF_ALL_GE_100=%d" % (len(counts) * 100))
p("MMLU_PRO_ACHIEVABLE_SELECTED_COUNT=%d" % sum(min(c, 100) for c in counts.values()))
p()

# validation structurelle d'un item
row0 = t.slice(0, 1).to_pylist()[0]
p("EXEMPLE D ITEM (cles):")
for k in sorted(row0):
    v = row0[k]
    p("    %-16s %s" % (k, (str(v)[:70] + "...") if len(str(v)) > 70 else v))
p()
# integrite : options et reponse
optcol = "options" if "options" in t.column_names else None
anscol = "answer" if "answer" in t.column_names else None
if optcol and anscol:
    bad = 0
    lens = {}
    for r in t.to_pylist():
        o = r[optcol]; a = r[anscol]
        lens[len(o)] = lens.get(len(o), 0) + 1
        if not isinstance(o, list) or len(o) < 2:
            bad += 1; continue
        try:
            idx = ord(str(a).strip().upper()) - ord("A")
        except Exception:
            bad += 1; continue
        if idx < 0 or idx >= len(o):
            bad += 1
    p("MMLU_PRO_ITEM_STRUCTURE_INVALID_COUNT=%d" % bad)
    p("MMLU_PRO_OPTION_COUNT_DISTRIBUTION=%s" % json.dumps(lens))
p("MMLU_PRO_PARSE_ERROR_COUNT=%d" % parse_err)
p()

# ================= autres sources : comptes bruts =================
p("=== GSM8K ===")
g = E + "/gsm8k/grade_school_math/data/test.jsonl"
if os.path.exists(g):
    n = 0; bad = 0
    for line in open(g):
        line = line.strip()
        if not line: continue
        n += 1
        try:
            r = json.loads(line)
            if "question" not in r or "answer" not in r: bad += 1
        except Exception:
            bad += 1
    p("GSM8K_RAW_TEST_COUNT=%d" % n)
    p("GSM8K_PARSE_ERROR_COUNT=%d" % bad)
    p("GSM8K_FIELDS=%s" % sorted(json.loads(open(g).readline()).keys()))
else:
    p("GSM8K_FILE_ABSENT")

p()
p("=== MBPP ===")
for c in ("mbpp/mbpp.jsonl", "mbpp/sanitized-mbpp.json"):
    fp = E + "/" + c
    if os.path.exists(fp):
        if c.endswith(".jsonl"):
            n = sum(1 for l in open(fp) if l.strip())
            p("MBPP_%s_COUNT=%d" % (os.path.basename(c).replace(".", "_"), n))
            r = json.loads(open(fp).readline())
            p("  champs : %s" % sorted(r.keys()))
        else:
            d = json.load(open(fp))
            p("MBPP_%s_COUNT=%d" % (os.path.basename(c).replace(".", "_"), len(d)))

p()
p("=== IFEVAL ===")
base = E + "/mbpp/instruction_following_eval"
if os.path.isdir(base):
    for root, _, fs in os.walk(base):
        for fn in sorted(fs):
            rel = os.path.relpath(os.path.join(root, fn), base)
            p("  %-52s %d octets" % (rel, os.path.getsize(os.path.join(root, fn))))
    inp = base + "/data/input_data.jsonl"
    if os.path.exists(inp):
        n = sum(1 for l in open(inp) if l.strip())
        p("IFEVAL_RAW_COUNT=%d" % n)
        r = json.loads(open(inp).readline())
        p("  champs : %s" % sorted(r.keys()))
else:
    p("IFEVAL_ABSENT")

p()
p("=== WIKITEXT ===")
w = E + "/wikitext/wikitext-2-raw-v1"
if os.path.isdir(w):
    for root, _, fs in os.walk(w):
        for fn in sorted(fs):
            p("  %s  %d octets" % (fn, os.path.getsize(os.path.join(root, fn))))

open(Q + "/DATASET_AUDIT.txt", "w", encoding="utf-8", newline="\n").write("\n".join(out) + "\n")
print()
print("audit ecrit")
