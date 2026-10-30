#!/usr/bin/env python3
"""ENGINE-008 DATA -- S7 (retokenisation independante) + S8 (cross-GGUF + hashes).
Repart des CONTENUS sources canoniques, jamais des IDs deja ecrits."""
import glob, hashlib, json, os, subprocess, sys

Q = ("/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/"
     "qualification/engine-008-quality-v1")
E = Q + "/external"
C = Q + "/corpora"
TOK = "/home/triviere/projects/llama-obrain-cuda-f1a/build-cuda/bin/llama-tokenize"
BASE = "/home/triviere/projects/models/qwen3.8-27b-gguf/Qwen3.8-27B-Q4_K_M.gguf"
CAND = "/home/triviere/projects/models/Qwen3.8-27B-OBRAIN-v3.gguf"
BOR = "/home/triviere/projects/banc-obrain"
VOCAB = 248320


def sha(s):
    return hashlib.sha256(s.encode("utf-8") if isinstance(s, str) else s).hexdigest()


def sha_file(p):
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""):
            h.update(b)
    return h.hexdigest()


def atomic(path, data):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    t = path + ".tmp"
    with open(t, "w", encoding="utf-8", newline="\n") as f:
        f.write(data); f.flush(); os.fsync(f.fileno())
    os.replace(t, path)


def stage(name, ok, lines):
    atomic(Q + "/STAGE_%s_STATUS.txt" % name,
           "\n".join(["STAGE_%s_STATUS=%s" % (name, "PASS" if ok else "FAIL")] + lines) + "\n")
    print("STAGE_%s_STATUS=%s" % (name, "PASS" if ok else "FAIL"))


def canon(t):
    return t.replace("\r\n", "\n").replace("\r", "\n")


def tokenize(gguf, text):
    r = subprocess.run([TOK, "-m", gguf, "--ids", "--stdin", "--no-bos",
                        "--no-parse-special", "--log-disable"],
                       input=text.encode("utf-8"), capture_output=True)
    if r.returncode != 0:
        raise RuntimeError("tokenize rc=%d: %s" % (r.returncode, r.stderr.decode()[:200]))
    return json.loads(r.stdout.decode().strip())


# ---- recharger les sources canoniques et retrouver les items selectionnes
rows = [json.loads(l) for l in open(C + "/token_eval/TOKEN_EVAL_INPUTS.jsonl")]
need = {r["source_content_sha256"]: r for r in rows}
print("items selectionnes : %d" % len(rows))

texts = {}
import pyarrow.parquet as pq
f = sorted(glob.glob(E + "/wikitext/**/test-*.parquet", recursive=True))[0]
for i, v in enumerate(pq.read_table(f).column("text").to_pylist()):
    if v and v.strip():
        t = canon(v)
        h = sha(t)
        if h in need:
            texts[h] = t
for p in sorted(glob.glob(BOR + "/docs/**/*.md", recursive=True)) + \
         sorted(glob.glob(BOR + "/crates/**/*.rs", recursive=True)):
    try:
        t = canon(open(p, encoding="utf-8").read())
    except UnicodeDecodeError:
        continue
    if t.strip():
        h = sha(t)
        if h in need:
            texts[h] = t
print("contenus canoniques retrouves : %d / %d" % (len(texts), len(need)))
if len(texts) != len(need):
    missing = [k for k in need if k not in texts]
    print("MANQUANTS : %s" % missing[:5])
    sys.exit(2)

# ---------------- S7 : retokenisation independante (depuis le texte)
mism = 0
for h, r in need.items():
    again = tokenize(BASE, texts[h])[:r["input_token_count"]]
    if again != r["input_token_ids"]:
        mism += 1
        print("  ECART retokenisation : %s" % r["item_id"])
stage("S7", mism == 0, ["TOKENIZATION_SECOND_PASS_MISMATCH_COUNT=%d" % mism,
                        "TOKENIZATION_REPRODUCIBLE=%s" % ("YES" if mism == 0 else "NO")])

# ---------------- S8 : cross-GGUF corpus + fixtures
xm = 0
for h, r in need.items():
    cand = tokenize(CAND, texts[h])[:r["input_token_count"]]
    if cand != r["input_token_ids"]:
        xm += 1
        print("  ECART cross-GGUF : %s" % r["item_id"])

FIX = {"ascii": "Le renard brun saute par-dessus le chien paresseux.",
       "unicode": "日本語 Émojis 🐘🔬 ∑∫√",
       "rust": "fn main() { let x: u32 = 42; }",
       "json": '{"a":1,"b":[true,null,"x"]}',
       "fr": "Où sont les éléphants ? À côté de la rivière.", }
fixxm = 0
for k, v in FIX.items():
    a = tokenize(BASE, v)
    b = tokenize(CAND, v)
    if a != b:
        fixxm += 1
        print("  ECART fixture %s : base=%s cand=%s" % (k, a[:6], b[:6]))

TOKSHA = sha_file(TOK)
RULES_SHA = open(Q + "/ENGINE008_SELECTION_RULES_v1.sha256").read().split()[0]
TOKLOG = "49930cc8a75188cf69a07c57914f0aabe383db11494aa03ed15629adee4c7e33"
hh = hashlib.sha256()
for r in sorted(rows, key=lambda x: x["item_id"]):
    hh.update(r["source_content_sha256"].encode()); hh.update(b"\x00")
    hh.update(json.dumps(r["input_token_ids"], separators=(",", ":")).encode()); hh.update(b"\x00")
for p in ("TOKEN_EVAL_SELECTION.csv", "TOKEN_EVAL_INPUTS.jsonl"):
    hh.update(open(C + "/token_eval/" + p, "rb").read()); hh.update(b"\x00")
hh.update(RULES_SHA.encode()); hh.update(b"\x00")
hh.update(TOKLOG.encode()); hh.update(b"\x00")
hh.update(TOKSHA.encode()); hh.update(b"\x00")
hh.update(b"BOS=NOT_ADDED;EOS=NOT_ADDED;PARSE_SPECIAL=NO")
TOKEN_TREE = hh.hexdigest()

counts = {}
for g in ("wikitext", "obrain_docs", "obrain_rust"):
    counts[g] = sum(r["scored_token_count"] for r in rows if r["source_group"] == g)
total = sum(counts.values())
invalid = sum(1 for r in rows for t in r["input_token_ids"] if not (0 <= t < VOCAB))
srcfiles = {}
for g in ("wikitext", "obrain_docs", "obrain_rust"):
    srcfiles[g] = len({r["source_content_sha256"] for r in rows if r["source_group"] == g})

stage("S8", xm == 0 and fixxm == 0, [
    "TOKEN_CORPUS_BASELINE_CANDIDATE_TOKEN_ID_MISMATCH_COUNT=%d" % xm,
    "TOKENIZER_BASELINE_CANDIDATE_FIXTURE_MISMATCH_COUNT=%d" % fixxm,
    "TOKEN_EVAL_CONTENT_TREE_SHA256=" + TOKEN_TREE,
    "TOKEN_WIKITEXT_SCORED_COUNT=%d" % counts["wikitext"],
    "TOKEN_OBRAIN_DOCS_SCORED_COUNT=%d" % counts["obrain_docs"],
    "TOKEN_OBRAIN_RUST_SCORED_COUNT=%d" % counts["obrain_rust"],
    "TOKEN_EVAL_SCORED_TOKEN_COUNT=%d" % total,
    "TOKEN_EVAL_INVALID_TOKEN_ID_COUNT=%d" % invalid,
    "TOKEN_EVAL_ITEM_COUNT=%d" % len(rows),
    "SOURCE_FILES: wikitext=%d obrain_docs=%d obrain_rust=%d"
    % (srcfiles["wikitext"], srcfiles["obrain_docs"], srcfiles["obrain_rust"]),
])
print(json.dumps({"counts": counts, "total": total, "invalid": invalid,
                  "srcfiles": srcfiles, "tree": TOKEN_TREE}, indent=1))
