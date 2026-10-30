#!/usr/bin/env python3
"""ENGINE-008 S5-S7 : corpus TOKEN v2 (regles Rust amendees).
Aucune inference. Tokenisation via llama-tokenize natif uniquement."""
import csv
import glob
import hashlib
import json
import os
import subprocess
import sys
import tempfile

Q = ("/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/"
     "qualification/engine-008-quality-v1")
E = Q + "/external"
C = Q + "/corpora/token_eval/v2"
TOK = "/home/triviere/projects/llama-obrain-cuda-f1a/build-cuda/bin/llama-tokenize"
GGUF_B = "/home/triviere/projects/models/qwen3.8-27b-gguf/Qwen3.8-27B-Q4_K_M.gguf"
GGUF_C = "/home/triviere/projects/models/Qwen3-27B-PLACEHOLDER.gguf"  # remplace plus bas
GGUF_C = "/home/triviere/projects/models/Qwen3.8-27B-OBRAIN-v3.gguf"
VOCAB = 248320
QUOTA = {"wikitext": 32768, "obrain_docs": 16384, "obrain_rust": 16384}
RUST_CAP = 1024
RUST_MIN_FILES = 16

def sha256s(s):
    return hashlib.sha256(s.encode("utf-8")).hexdigest()

def atomic_write(path, text):
    tmp = path + ".tmp"
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(tmp, "w", encoding="utf-8", newline="\n") as f:
        f.write(text); f.flush(); os.fsync(f.fileno())
    os.replace(tmp, path)

def canon(t):
    return t.replace("\r\n", "\n").replace("\r", "\n")

def tokenize(text, model=GGUF_C):
    """Contrat gele : --no-bos --no-parse-special --no-escape."""
    with tempfile.NamedTemporaryFile("w", encoding="utf-8", suffix=".txt",
                                     delete=False, newline="") as f:
        f.write(text); p = f.name
    try:
        r = subprocess.run([TOK, "-m", model, "--ids", "--no-bos", "--no-parse-special",
                            "--no-escape", "--log-disable", "-f", p],
                           capture_output=True, text=True)
        if r.returncode != 0:
            raise RuntimeError("tokenize rc=%d : %s" % (r.returncode, r.stderr.strip()[-200:]))
        return json.loads(r.stdout.strip())
    finally:
        os.unlink(p)

# ---------------- regles v2 ----------------
V1 = open(Q + "/ENGINE008_SELECTION_RULES_v1.txt", encoding="utf-8").read()
V2 = V1.replace("ENGINE008_SELECTION_RULES_v1", "ENGINE008_SELECTION_RULES_v2") + """

=== AMENDEMENT v2 (strate obrain_rust UNIQUEMENT) ===
MAX_RUST_SCORED_TARGETS_PER_SOURCE_FILE=1024
MIN_RUST_DISTINCT_SOURCE_FILE_COUNT=16
SELECTION_RULES_V2_REASON=
PREVENT_SINGLE_SOURCE_FILE_FROM_DOMINATING_25_PERCENT_OF_TOKEN_EVAL
SELECTION_RULES_V2_WIKITEXT_CHANGE=NO
SELECTION_RULES_V2_OBRAIN_DOCS_CHANGE=NO
SELECTION_RULES_V2_OBRAIN_RUST_CHANGE=YES

Fenetre deterministe si un fichier Rust depasse 1024 targets utilisables :
  h = unsigned_big_endian(first_8_bytes(SHA256(canonical_content)))
  window_start_target = h % (usable_targets - 1024 + 1)
  -> 1025 input tokens selectionnes pour 1024 scored targets

PREINFERENCE=YES  PRE_PROTOCOL_FREEZE=YES  NOT_RESULT_DRIVEN=YES

=== CONTRAT DE TOKENISATION (etabli empiriquement) ===
TOKENIZER_FLAGS=--ids --no-bos --no-parse-special --no-escape
TOKEN_EVAL_NO_ESCAPE=YES   (le texte est pris LITTERALEMENT ; sans ce flag,
                            un backslash-n du texte devient un vrai newline :
                            token 1639 vs token 198, mesure)
TOKEN_EVAL_ADD_EOS=NO      (aucun flag --no-eos n existe ; pas d EOS observe en fin)
"""
atomic_write(Q + "/ENGINE008_SELECTION_RULES_v2.txt", V2)
RULES_V2_SHA = sha256s(V2)
atomic_write(Q + "/ENGINE008_SELECTION_RULES_v2.sha256",
             RULES_V2_SHA + "  ENGINE008_SELECTION_RULES_v2.txt\n")
print("ENGINE008_SELECTION_RULES_V2_SHA256=" + RULES_V2_SHA)

# ---------------- populations ----------------
import pyarrow.parquet as pq

def pop_wikitext():
    t = pq.read_table(E + "/wikitext/wikitext-2-raw-v1/test-00000-of-00001.parquet")
    out = []
    for i, v in enumerate(t.column("text").to_pylist()):
        if v and v.strip():
            c = canon(v)
            out.append(("wikitext:row%04d" % i, "test-00000-of-00001.parquet#row%d" % i, c))
    return out

def pop_docs():
    root = "/home/triviere/projects/banc-obrain/docs"
    out = []
    for p in sorted(glob.glob(root + "/**/*.md", recursive=True)):
        c = canon(open(p, encoding="utf-8", errors="strict").read())
        if c.strip():
            out.append(("obrain_docs:" + os.path.relpath(p, root), p, c))
    return out

def pop_rust():
    root = "/home/triviere/projects/banc-obrain/crates"
    out = []
    for p in sorted(glob.glob(root + "/**/*.rs", recursive=True)):
        c = canon(open(p, encoding="utf-8", errors="strict").read())
        if c.strip():
            out.append(("obrain_rust:" + os.path.relpath(p, root), p, c))
    return out

# ---------------- construction d une strate ----------------
def build(group, items, quota, rust=False):
    """Parcourt dans l ordre SHA croissant, remplit le quota exact."""
    ordered = sorted(((sha256s(c), iid, loc, c) for iid, loc, c in items), key=lambda x: x[0])
    units = []
    acc = 0
    files_used = set()
    for h, iid, loc, content in ordered:
        if acc >= quota:
            break
        ids = tokenize(content)
        if len(ids) < 2:
            continue
        usable = len(ids) - 1
        if rust:
            if loc in files_used:
                continue
            if usable > RUST_CAP:
                allocated = RUST_CAP
            else:
                allocated = usable
        else:
            allocated = usable
        remaining = quota - acc
        take = min(allocated, remaining)
        if rust and usable > RUST_CAP and allocated == RUST_CAP:
            hb = int.from_bytes(bytes.fromhex(h)[:8], "big")
            start = hb % (usable - RUST_CAP + 1)
            if take == RUST_CAP:
                sel = ids[start:start + RUST_CAP + 1]
            else:
                sel = ids[start:start + take + 1]
        else:
            sel = ids[:take + 1]
        if len(sel) < 2:
            continue
        scored = len(sel) - 1
        if scored <= 0:
            continue
        files_used.add(loc)
        units.append(dict(item_id=iid, source_group=group, source_location=loc,
                          canonical_content_sha256=h, input_token_ids=sel,
                          input_token_count=len(sel), scored_start_index=1,
                          scored_end_index=len(sel), scored_token_count=scored,
                          terminal_fragment="YES" if (take < allocated or
                                                      len(sel) < len(ids)) else "NO",
                          full_input_token_count=len(ids),
                          full_usable_scored_token_count=usable))
        acc += scored
    return units, acc

print()
print("=== S5 WIKITEXT ===")
w_units, w_acc = build("wikitext", pop_wikitext(), QUOTA["wikitext"])
print("  TOKEN_WIKITEXT_SCORED_COUNT=%d  units=%d" % (w_acc, len(w_units)))

print("=== S6 OBRAIN DOCS ===")
d_units, d_acc = build("obrain_docs", pop_docs(), QUOTA["obrain_docs"])
print("  TOKEN_OBRAIN_DOCS_SCORED_COUNT=%d  units=%d" % (d_acc, len(d_units)))

print("=== S7 OBRAIN RUST ===")
r_units, r_acc = build("obrain_rust", pop_rust(), QUOTA["obrain_rust"], rust=True)
print("  TOKEN_OBRAIN_RUST_SCORED_COUNT=%d  units=%d  fichiers distincts=%d"
      % (r_acc, len(r_units), len({u["source_location"] for u in r_units})))

allunits = w_units + d_units + r_units
total = w_acc + d_acc + r_acc
print()
print("TOKEN_EVAL_SCORED_TOKEN_COUNT=%d" % total)

# ---------------- artefacts ----------------
os.makedirs(C, exist_ok=True)
inv = 0
for u in allunits:
    for t in u["input_token_ids"]:
        if not (0 <= t < VOCAB):
            inv += 1
print("TOKEN_EVAL_INVALID_TOKEN_ID_COUNT=%d" % inv)

tmp = C + "/TOKEN_EVAL_INPUTS_v2.jsonl.tmp"
with open(tmp, "w", encoding="utf-8", newline="\n") as f:
    for u in allunits:
        f.write(json.dumps({k: u[k] for k in
                            ("item_id", "source_group", "canonical_content_sha256",
                             "input_token_ids", "input_token_count", "scored_start_index",
                             "scored_end_index", "scored_token_count", "terminal_fragment")},
                           sort_keys=True, separators=(",", ":")) + "\n")
    f.flush(); os.fsync(f.fileno())
os.replace(tmp, C + "/TOKEN_EVAL_INPUTS_v2.jsonl")

tmp = C + "/TOKEN_EVAL_SELECTION_v2.csv.tmp"
with open(tmp, "w", encoding="utf-8", newline="") as f:
    w = csv.writer(f)
    w.writerow(["global_order", "source_group", "source_item_id", "source_location",
                "canonical_content_sha256", "full_input_token_count",
                "full_usable_scored_token_count", "selected_token_start", "selected_token_end",
                "selected_input_token_count", "selected_scored_token_count", "terminal_fragment"])
    for n, u in enumerate(allunits, 1):
        w.writerow([n, u["source_group"], u["item_id"], u["source_location"],
                    u["canonical_content_sha256"], u["full_input_token_count"],
                    u["full_usable_scored_token_count"], 0, u["input_token_count"],
                    u["input_token_count"], u["scored_token_count"], u["terminal_fragment"]])
    f.flush(); os.fsync(f.fileno())
os.replace(tmp, C + "/TOKEN_EVAL_SELECTION_v2.csv")

rust_files = {u["source_location"] for u in r_units}
rust_max = max(u["scored_token_count"] for u in r_units)
S5 = ["STAGE_S5_S7_TOKEN_CORPUS",
      "TOKEN_WIKITEXT_SCORED_COUNT=%d" % w_acc,
      "TOKEN_OBRAIN_DOCS_SCORED_COUNT=%d" % d_acc,
      "TOKEN_OBRAIN_RUST_SCORED_COUNT=%d" % r_acc,
      "TOKEN_EVAL_SCORED_TOKEN_COUNT=%d" % total,
      "TOKEN_EVAL_INVALID_TOKEN_ID_COUNT=%d" % inv,
      "RUST_DISTINCT_SOURCE_FILE_COUNT=%d" % len(rust_files),
      "RUST_MAX_SCORED_TARGETS_FROM_SINGLE_FILE=%d" % rust_max,
      "RUST_SOURCE_FILE_DUPLICATE_COUNT=0",
      "WIKITEXT_UNIT_COUNT=%d" % len(w_units),
      "OBRAIN_DOCS_UNIT_COUNT=%d" % len(d_units),
      "OBRAIN_RUST_UNIT_COUNT=%d" % len(r_units)]
ok = (w_acc == QUOTA["wikitext"] and d_acc == QUOTA["obrain_docs"]
      and r_acc == QUOTA["obrain_rust"] and total == 65536 and inv == 0
      and len(rust_files) >= RUST_MIN_FILES and rust_max <= RUST_CAP)
S5.append("STAGE_S5_S7_STATUS=%s" % ("PASS" if ok else "FAIL"))
atomic_write(Q + "/STAGE_S5_S7_STATUS.txt", "\n".join(S5) + "\n")
atomic_write(C + "/TOKEN_CORPUS_BUILD_LOG.txt", "\n".join(S5) + "\n")
print()
print("\n".join(S5))
sys.exit(0 if ok else 1)
