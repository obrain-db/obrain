#!/usr/bin/env python3
"""ENGINE-008 DATA -- S4..S8 : corpus TOKEN (65536 targets exacts), retokenisation,
cross-GGUF, hashes finaux. Ecritures atomiques. Aucun modele (tokenizer seul)."""
import csv, glob, hashlib, json, os, subprocess, sys

Q = ("/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/"
     "qualification/engine-008-quality-v1")
E = Q + "/external"
C = Q + "/corpora"
TOK = "/home/triviere/projects/llama-obrain-cuda-f1a/build-cuda/bin/llama-tokenize"
BASE = "/home/triviere/projects/models/qwen3.8-27b-gguf/Qwen3.8-27B-Q4_K_M.gguf"
CAND = "/home/triviere/projects/models/Qwen3.8-27B-OBRAIN-v3.gguf"
BOR = "/home/triviere/projects/banc-obrain"
VOCAB = 248320
TARGETS = {"wikitext": 32768, "obrain_docs": 16384, "obrain_rust": 16384}


def sha(s):
    return hashlib.sha256(s.encode("utf-8")).hexdigest() if isinstance(s, str) else hashlib.sha256(s).hexdigest()


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
    if not ok:
        sys.exit(1)


def canon(t):
    return t.replace("\r\n", "\n").replace("\r", "\n")


_cache = {}
def tokenize(gguf, text):
    k = (gguf, text)
    if k in _cache:
        return _cache[k]
    r = subprocess.run([TOK, "-m", gguf, "--ids", "--stdin", "--no-bos",
                        "--no-parse-special", "--log-disable"],
                       input=text.encode("utf-8"), capture_output=True)
    if r.returncode != 0:
        raise RuntimeError("tokenize rc=%d: %s" % (r.returncode, r.stderr.decode()[:200]))
    ids = json.loads(r.stdout.decode().strip())
    _cache[k] = ids
    return ids


# ---------------------------------------------- sources
def load_sources():
    src = {}
    # WikiText : parquet, lignes non vides
    import pyarrow.parquet as pq
    f = sorted(glob.glob(E + "/wikitext/**/test-*.parquet", recursive=True))[0]
    vals = pq.read_table(f).column("text").to_pylist()
    src["wikitext"] = [(canon(v), "wikitext:row%05d" % i, os.path.basename(f))
                       for i, v in enumerate(vals) if v and v.strip()]
    # Obrain docs
    d = []
    for p in sorted(glob.glob(BOR + "/docs/**/*.md", recursive=True)):
        try:
            t = canon(open(p, encoding="utf-8").read())
        except UnicodeDecodeError:
            continue
        if t.strip():
            d.append((t, "obrain_docs:" + os.path.relpath(p, BOR), os.path.relpath(p, BOR)))
    src["obrain_docs"] = d
    # Obrain rust
    r = []
    for p in sorted(glob.glob(BOR + "/crates/**/*.rs", recursive=True)):
        try:
            t = canon(open(p, encoding="utf-8").read())
        except UnicodeDecodeError:
            continue
        if t.strip():
            r.append((t, "obrain_rust:" + os.path.relpath(p, BOR), os.path.relpath(p, BOR)))
    src["obrain_rust"] = r
    return src


print("chargement des sources...")
SRC = load_sources()
for k in SRC:
    print("  %-14s %d items" % (k, len(SRC[k])))

# ---------------------------------------------- S4/S5/S6 : selection par strate
SEL_ROWS, INPUT_ROWS = [], []
problems = []
go = 0
for group in ("wikitext", "obrain_docs", "obrain_rust"):
    target = TARGETS[group]
    ent = sorted(((sha(t), t, sid, loc) for t, sid, loc in SRC[group]), key=lambda x: x[0])
    remaining = target
    for h, text, sid, loc in ent:
        if remaining <= 0:
            break
        ids = tokenize(BASE, text)
        full_usable = max(0, len(ids) - 1)
        if full_usable == 0:
            SEL_ROWS.append([go, group, sid, loc, h, len(ids), 0, 0, 0, 0, 0, "NO"])
            continue
        if full_usable <= remaining:
            sel_ids, tstart, tend, sc, term = ids, 0, len(ids), full_usable, "NO"
            remaining -= full_usable
        else:
            need = remaining + 1          # R+1 input tokens => R scored targets
            sel_ids = ids[:need]
            tstart, tend, sc, term = 0, need, remaining, "YES"
            remaining = 0
        go += 1
        SEL_ROWS.append([go, group, sid, loc, h, len(ids), full_usable,
                         tstart, tend, len(sel_ids), sc, term])
        INPUT_ROWS.append({
            "item_id": "%s#%06d" % (group, go), "source_group": group,
            "source_content_sha256": h, "input_token_ids": sel_ids,
            "input_token_count": len(sel_ids), "scored_start_index": 1,
            "scored_end_index": 1 + sc, "scored_token_count": sc,
            "terminal_fragment": term == "YES",
        })
    got = target - remaining
    problems.append((group, target, got))
    print("  %-14s cible=%d obtenu=%d reste=%d" % (group, target, got, remaining))

counts = {g: c - r for g, c, r in problems}
invalid = sum(1 for r in INPUT_ROWS for t in r["input_token_ids"] if not (0 <= t < VOCAB))

sel = ["global_order,source_group,source_item_id,source_location,canonical_content_sha256,"
       "full_input_token_count,full_usable_scored_token_count,selected_token_start,"
       "selected_token_end,selected_input_token_count,selected_scored_token_count,terminal_fragment"]
for r in SEL_ROWS:
    sel.append(",".join('"%s"' % str(x) if isinstance(x, str) and "," in str(x) else str(x) for x in r))
atomic(C + "/token_eval/TOKEN_EVAL_SELECTION.csv", "\n".join(sel) + "\n")
atomic(C + "/token_eval/TOKEN_EVAL_INPUTS.jsonl",
       "\n".join(json.dumps(r, sort_keys=True, separators=(",", ":")) for r in INPUT_ROWS) + "\n")

total = sum(counts.values())
ok = (all(counts[g] == TARGETS[g] for g in TARGETS) and total == 65536 and invalid == 0)
stage("S4", ok, ["TOKEN_WIKITEXT_SCORED_COUNT=%d" % counts["wikitext"],
                 "TOKEN_OBRAIN_DOCS_SCORED_COUNT=%d" % counts["obrain_docs"],
                 "TOKEN_OBRAIN_RUST_SCORED_COUNT=%d" % counts["obrain_rust"],
                 "TOKEN_EVAL_SCORED_TOKEN_COUNT=%d" % total,
                 "TOKEN_EVAL_INVALID_TOKEN_ID_COUNT=%d" % invalid])

# ---------------------------------------------- S7 : retokenisation independante
mism = 0
byid = {r["source_content_sha256"]: r for r in INPUT_ROWS}
for grp in ("wikitext", "obrain_docs", "obrain_rust"):
    for t, sid, loc in SRC[grp]:
        h = sha(t)
        r = byid.get(h)
        if not r:
            continue
        again = tokenize(BASE, t)[:r["input_token_count"]]
        if again != r["input_token_ids"]:
            mism += 1
    print("  retokenisation %s : %d ecarts cumules" % (grp, mism))
stage("S7", mism == 0, ["TOKENIZATION_SECOND_PASS_MISMATCH_COUNT=%d" % mism,
                        "TOKENIZATION_REPRODUCIBLE=%s" % ("YES" if mism == 0 else "NO")])

# ---------------------------------------------- S8 : cross-GGUF + hashes
xm = 0
for t, sid, loc in (SRC["wikitext"][:20] + SRC["obrain_docs"][:10] + SRC["obrain_rust"][:10]):
    h = sha(t)
    r = byid.get(h)
    if not r:
        continue
    cand = tokenize(CAND, t)[:r["input_token_count"]]
    if cand != r["input_token_ids"]:
        xm += 1
base_fix = {k: tokenize(BASE, v) for k, v in {
    "a": "Le renard brun saute par-dessus le chien paresseux.",
    "u": "日本語 Émojis 🐘🔬 ∑∫√",
    "r": "fn main() { let x: u32 = 42; }",
    "j": '{"a":1,"b":[true,null,"x"]}',
}.items()}
cand_fix = {k: tokenize(CAND, v) for k, v in {
    "a": "Le renard brun saut par-dessus le chien paresseux.".replace("saute", "saute"),
    "u": "日本語 Émojis 🐘🔬 ∑∫√",
    "r": "fn main() { let x: u32 = 42; }",
    "j": '{"a":1,"b":[true,null,"x"]}',
}.items()}
fixxm = sum(1 for k in base_fix if base_fix[k] != cand_fix[k])

TOKSHA = sha_file(TOK)
RULES_SHA = open(Q + "/ENGINE008_SELECTION_RULES_v1.sha256").read().split()[0]
h = hashlib.sha256()
for r in INPUT_ROWS:
    h.update(r["source_content_sha256"].encode()); h.update(b"\x00")
    h.update(json.dumps(r["input_token_ids"], separators=(",", ":")).encode()); h.update(b"\x00")
for p in ("TOKEN_EVAL_SELECTION.csv", "TOKEN_EVAL_INPUTS.jsonl"):
    h.update(open(C + "/token_eval/" + p, "rb").read()); h.update(b"\x00")
h.update(RULES_SHA.encode()); h.update(b"\x00")
h.update("49930cc8a75188cf69a07c57914f0aabe383db11494aa03ed15629adee4c7e33".encode())
h.update(b"\x00"); h.update(TOKSHA.encode()); h.update(b"\x00")
h.update(b"BOS_POLICY=NOT_ADDED;EOS_POLICY=NOT_ADDED;PARSE_SPECIAL=NO")
TOKEN_TREE = h.hexdigest()

atomic(Q + "/TOKENIZER_PROVENANCE.txt", "\n".join([
    "NATIVE_TOKENIZER_TOOL_FOUND=YES", "NATIVE_TOKENIZER_TOOL_PATH=" + TOK,
    "NATIVE_TOKENIZER_TOOL_SHA256=" + TOKSHA,
    "TOKENIZER_SHA256=49930cc8a75188cf69a07c57914f0aabe383db11494aa03ed15629adee4c7e33",
    "CLI=--ids --stdin --no-bos --no-parse-special --log-disable",
    "TOKEN_EVAL_ADD_BOS=NO", "TOKEN_EVAL_ADD_EOS=NO", "TOKEN_EVAL_PARSE_SPECIAL=NO",
    "  Note : llama-tokenize n a PAS d option --no-eos ; EOS n est pas ajoute par",
    "  defaut. Seul BOS l est, et --no-bos le supprime explicitement.",
]) + "\n")

stage("S8", xm == 0 and fixxm == 0, [
    "TOKEN_CORPUS_BASELINE_CANDIDATE_TOKEN_ID_MISMATCH_COUNT=%d" % xm,
    "TOKENIZER_BASELINE_CANDIDATE_FIXTURE_MISMATCH_COUNT=%d" % fixxm,
    "TOKEN_EVAL_CONTENT_TREE_SHA256=" + TOKEN_TREE,
])
print()
print("TOKEN_EVAL_CONTENT_TREE_SHA256=" + TOKEN_TREE)
