#!/usr/bin/env python3
"""ENGINE-008 S8 : retokenisation independante + cross-GGUF + tree hash final.
Repart des CONTENUS SOURCES, jamais du JSONL produit (verification independante)."""
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
GGUF_C = "/home/triviere/projects/models/Qwen3.8-27B-OBRAIN-v3.gguf"
QUOTA = {"wikitext": 32768, "obrain_docs": 16384, "obrain_rust": 16384}
RUST_CAP = 1024
VOCAB = 248320

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

def tokenize(text, model):
    with tempfile.NamedTemporaryFile("w", encoding="utf-8", suffix=".txt",
                                     delete=False, newline="") as f:
        f.write(text); p = f.name
    try:
        r = subprocess.run([TOK, "-m", model, "--ids", "--no-bos", "--no-parse-special",
                            "--no-escape", "--log-disable", "-f", p],
                           capture_output=True, text=True)
        if r.returncode != 0:
            raise RuntimeError("tokenize rc=%d" % r.returncode)
        return json.loads(r.stdout.strip())
    finally:
        os.unlink(p)

# ---- populations (relues depuis les SOURCES) ----
import pyarrow.parquet as pq

def pop_wikitext():
    t = pq.read_table(E + "/wikitext/wikitext-2-raw-v1/test-00000-of-00001.parquet")
    return [("wikitext:row%04d" % i, "test-00000-of-00001.parquet#row%d" % i, canon(v))
            for i, v in enumerate(t.column("text").to_pylist()) if v and v.strip()]

def pop_docs():
    root = "/home/triviere/projects/banc-obrain/docs"
    return [("obrain_docs:" + os.path.relpath(p, root), p,
             canon(open(p, encoding="utf-8").read()))
            for p in sorted(glob.glob(root + "/**/*.md", recursive=True))
            if canon(open(p, encoding="utf-8").read()).strip()]

def pop_rust():
    root = "/home/triviere/projects/banc-obrain/crates"
    return [("obrain_rust:" + os.path.relpath(p, root), p,
             canon(open(p, encoding="utf-8").read()))
            for p in sorted(glob.glob(root + "/**/*.rs", recursive=True))
            if canon(open(p, encoding="utf-8").read()).strip()]

def rebuild(group, items, quota, model, rust=False):
    ordered = sorted(((sha256s(c), iid, loc, c) for iid, loc, c in items), key=lambda x: x[0])
    units = []; acc = 0; files = set()
    for h, iid, loc, content in ordered:
        if acc >= quota:
            break
        ids = tokenize(content, model)
        if len(ids) < 2:
            continue
        usable = len(ids) - 1
        if rust:
            if loc in files:
                continue
            allocated = min(usable, RUST_CAP)
        else:
            allocated = usable
        take = min(allocated, quota - acc)
        if rust and usable > RUST_CAP:
            hb = int.from_bytes(bytes.fromhex(h)[:8], "big")
            start = hb % (usable - RUST_CAP + 1)
            sel = ids[start:start + take + 1]
        else:
            sel = ids[:take + 1]
        if len(sel) < 2:
            continue
        files.add(loc)
        units.append(dict(item_id=iid, source_group=group, source_location=loc,
                          canonical_content_sha256=h, input_token_ids=sel,
                          input_token_count=len(sel), scored_start_index=1,
                          scored_end_index=len(sel), scored_token_count=len(sel) - 1))
        acc += len(sel) - 1
    return units, acc

stored = {}
for l in open(C + "/TOKEN_EVAL_INPUTS_v2.jsonl", encoding="utf-8"):
    if l.strip():
        d = json.loads(l); stored[(d["source_group"], d["item_id"])] = d

log = []
def L(s=""):
    log.append(s); print(s)

L("=== S8.1 : retokenisation INDEPENDANTE depuis les sources (candidat) ===")
mismatch = 0; nchecked = 0
corpus_units = []
for grp, items, quota, rust in (("wikitext", pop_wikitext(), QUOTA["wikitext"], False),
                                ("obrain_docs", pop_docs(), QUOTA["obrain_docs"], False),
                                ("obrain_rust", pop_rust(), QUOTA["obrain_rust"], True)):
    units, acc = rebuild(grp, items, quota, GGUF_C, rust)
    L("  %-13s rebuilt=%d scored=%d units=%d" % (grp, acc, acc, len(units)))
    corpus_units += units
    for u in units:
        k = (grp, u["item_id"])
        if k not in stored:
            mismatch += 1; L("    ABSENT du JSONL : %s" % u["item_id"]); continue
        s = stored[k]; nchecked += 1
        if s["input_token_ids"] != u["input_token_ids"]:
            mismatch += 1
            if mismatch <= 3:
                L("    MISMATCH %s : %d vs %d ids" % (u["item_id"],
                                                      len(s["input_token_ids"]),
                                                      len(u["input_token_ids"])))
L("TOKENIZATION_SECOND_PASS_MISMATCH_COUNT=%d" % mismatch)
L("TOKENIZATION_REPRODUCIBLE=%s" % ("YES" if mismatch == 0 else "NO"))
L("  unites comparees : %d" % nchecked)
if len(stored) != len(corpus_units):
    L("  ATTENTION : JSONL=%d unites, rebuild=%d" % (len(stored), len(corpus_units)))

L("")
L("=== S8.2 : cross-GGUF sur le corpus final (baseline vs candidat) ===")
cross = 0; ncross = 0
for u in corpus_units:
    # retrouver le contenu source pour retokeniser avec l autre GGUF
    pass
srcmap = {}
for grp, items in (("wikitext", pop_wikitext()), ("obrain_docs", pop_docs()),
                   ("obrain_rust", pop_rust())):
    for iid, loc, content in items:
        srcmap[iid] = content
for u in corpus_units:
    content = srcmap.get(u["item_id"])
    if content is None:
        continue
    ids_b = tokenize(content, GGUF_B)
    # meme decoupe que le candidat : on verifie que la sequence COMPLETE differe ou non
    ids_c = tokenize(content, GGUF_C)
    ncross += 1
    if ids_b != ids_c:
        cross += 1
        if cross <= 3:
            L("    DIVERGENCE %s" % u["item_id"])
L("TOKEN_CORPUS_BASELINE_CANDIDATE_TOKEN_ID_MISMATCH_COUNT=%d" % cross)
L("  sequences comparees : %d" % ncross)

# ---- tree hash (ordre deterministe) ----
L("")
L("=== S8.3 : tree hash final ===")
h = hashlib.sha256()
h.update(b"RULES_V2\x00");   h.update(open(Q + "/ENGINE008_SELECTION_RULES_v2.txt", "rb").read())
h.update(b"\x00TOKENIZER\x00"); h.update(b"49930cc8a75188cf69a07c57914f0aabe383db11494aa03ed15629adee4c7e33")
h.update(b"\x00TOOL\x00")
h.update(hashlib.sha256(open(TOK, "rb").read()).hexdigest().encode())
h.update(b"\x00BOS=NO\x00EOS=NO\x00PARSE_SPECIAL=NO\x00NO_ESCAPE=YES")
for u in corpus_units:
    h.update(b"\x00UNIT\x00")
    h.update(u["source_group"].encode()); h.update(b"|")
    h.update(u["item_id"].encode()); h.update(b"|")
    h.update(u["canonical_content_sha256"].encode()); h.update(b"|")
    h.update(",".join(str(t) for t in u["input_token_ids"]).encode())
for f in ("TOKEN_EVAL_SELECTION_v2.csv", "TOKEN_EVAL_INPUTS_v2.jsonl"):
    h.update(b"\x00FILE\x00"); h.update(f.encode()); h.update(b"\x00")
    h.update(open(C + "/" + f, "rb").read())
TREE = h.hexdigest()
L("TOKEN_EVAL_CONTENT_TREE_V2_SHA256=%s" % TREE)

ok = (mismatch == 0 and cross == 0 and len(stored) == len(corpus_units))
S8 = ["STAGE_S8_RETOKENIZATION_HASH_AUDIT",
      "TOKENIZATION_SECOND_PASS_MISMATCH_COUNT=%d" % mismatch,
      "TOKENIZATION_REPRODUCIBLE=%s" % ("YES" if mismatch == 0 else "NO"),
      "TOKEN_CORPUS_BASELINE_CANDIDATE_TOKEN_ID_MISMATCH_COUNT=%d" % cross,
      "TOKEN_EVAL_CONTENT_TREE_V2_SHA256=%s" % TREE,
      "SEQUENCES_CROSSCHECKED=%d" % ncross,
      "STAGE_S8_STATUS=%s" % ("PASS" if ok else "FAIL")]
atomic_write(Q + "/STAGE_S8_STATUS.txt", "\n".join(S8) + "\n")
atomic_write(C + "/TOKEN_EVAL_RETOKENIZATION_AUDIT.txt", "\n".join(log) + "\n")
print()
print("\n".join(S8))
sys.exit(0 if ok else 1)
