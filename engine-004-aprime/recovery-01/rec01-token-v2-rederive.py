#!/usr/bin/env python3
"""ENGINE-008 RECOVERY-01 -- re-derivation LECTURE SEULE du tree hash TOKEN v2.

Reimplemente la definition EXACTE enregistree dans source/e8-s8.py (S8.3),
sans executer e8-s8.py lui-meme (qui reecrirait STAGE_S8_STATUS.txt et
TOKEN_EVAL_RETOKENIZATION_AUDIT.txt, artefacts geles).

AUCUN llama_decode. La seule operation modele est la TOKENISATION via
llama-tokenize --ids (chargement du vocabulaire, aucun forward pass).

Ecrit uniquement sous STAGING. Ne touche jamais l arbre d autorite.

Definition reproduite (e8-s8.py l.167-186) :
  sha256(
    b"RULES_V2\\x00" + rules_v2_bytes
    + b"\\x00TOKENIZER\\x00" + b"49930cc8..."
    + b"\\x00TOOL\\x00" + hex(sha256(TOKENIZER_TOOL))
    + b"\\x00BOS=NO\\x00EOS=NO\\x00PARSE_SPECIAL=NO\\x00NO_ESCAPE=YES"
    + pour chaque unite : b"\\x00UNIT\\x00" + group|item_id|content_sha|ids_csv
    + pour chaque fichier : b"\\x00FILE\\x00" + nom + b"\\x00" + octets
  )
"""
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
GGUF_C = "/home/triviere/projects/models/Qwen3.8-27B-OBRAIN-v3.gguf"
QUOTA = {"wikitext": 32768, "obrain_docs": 16384, "obrain_rust": 16384}
RUST_CAP = 1024
EXPECTED = "0ce4512e9d73b5e1cc3beddd2222e9ad7a96592508f10930515ef8b4276cf5dd"
STAGING = sys.argv[1] if len(sys.argv) > 1 else "/tmp/rec01"

DECODE_CALL_COUNT = 0  # invariant : aucun llama_decode dans ce script


def sha256s(s):
    return hashlib.sha256(s.encode("utf-8")).hexdigest()


def canon(t):
    return t.replace("\r\n", "\n").replace("\r", "\n")


def tokenize(text, model):
    """Tokenisation seule -- CHARGE LE VOCABULAIRE, NE FAIT AUCUN DECODE."""
    with tempfile.NamedTemporaryFile("w", encoding="utf-8", suffix=".txt",
                                     delete=False, newline="") as f:
        f.write(text)
        p = f.name
    try:
        r = subprocess.run([TOK, "-m", model, "--ids", "--no-bos",
                            "--no-parse-special", "--no-escape",
                            "--log-disable", "-f", p],
                           capture_output=True, text=True)
        if r.returncode != 0:
            raise RuntimeError("tokenize rc=%d %s" % (r.returncode, r.stderr[:200]))
        return json.loads(r.stdout.strip())
    finally:
        os.unlink(p)


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
    ordered = sorted(((sha256s(c), iid, loc, c) for iid, loc, c in items),
                     key=lambda x: x[0])
    units = []
    acc = 0
    files = set()
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
                          scored_end_index=len(sel),
                          scored_token_count=len(sel) - 1))
        acc += len(sel) - 1
    return units, acc


def main():
    os.makedirs(STAGING, exist_ok=True)
    L = []

    def P(s=""):
        L.append(s)
        print(s, flush=True)

    P("ENGINE-008 RECOVERY-01 -- RE-DERIVATION LECTURE SEULE DU TREE HASH TOKEN v2")
    P("=" * 74)
    P("METHOD=REIMPLEMENTATION_EXACTE_DE_e8-s8.py_S8.3")
    P("e8-s8.py_EXECUTED=NO  (il reecrirait les artefacts geles)")
    P("LLAMA_DECODE_CALL_COUNT=%d" % DECODE_CALL_COUNT)
    P("MODEL_FORWARD_PASS=NO")
    P("OPERATION_MODELE=TOKENISATION_SEULE (llama-tokenize --ids)")
    P("STAGING=%s" % STAGING)
    P()

    rules = open(Q + "/ENGINE008_SELECTION_RULES_v2.txt", "rb").read()
    toolsha = hashlib.sha256(open(TOK, "rb").read()).hexdigest()
    P("RULES_V2_SHA256=%s" % hashlib.sha256(rules).hexdigest())
    P("TOKENIZER_TOOL_SHA256=%s" % toolsha)
    P()

    stored = {}
    for l in open(C + "/TOKEN_EVAL_INPUTS_v2.jsonl", encoding="utf-8"):
        if l.strip():
            d = json.loads(l)
            stored[(d["source_group"], d["item_id"])] = d
    P("STORED_UNIT_COUNT=%d" % len(stored))

    corpus_units = []
    mismatch = 0
    nchecked = 0
    for grp, items, quota, rust in (("wikitext", pop_wikitext(), QUOTA["wikitext"], False),
                                    ("obrain_docs", pop_docs(), QUOTA["obrain_docs"], False),
                                    ("obrain_rust", pop_rust(), QUOTA["obrain_rust"], True)):
        units, acc = rebuild(grp, items, quota, GGUF_C, rust)
        P("  %-13s rebuilt=%d units=%d" % (grp, acc, len(units)))
        corpus_units += units
        for u in units:
            k = (grp, u["item_id"])
            if k not in stored:
                mismatch += 1
                P("    ABSENT du JSONL : %s" % u["item_id"])
                continue
            s = stored[k]
            nchecked += 1
            if s["input_token_ids"] != u["input_token_ids"]:
                mismatch += 1
                P("    MISMATCH %s" % u["item_id"])

    P()
    P("TOKENIZATION_SECOND_PASS_MISMATCH_COUNT=%d" % mismatch)
    P("UNITS_COMPARED=%d" % nchecked)
    P("STORED_EQ_REBUILT=%s" % ("YES" if len(stored) == len(corpus_units) else "NO"))

    h = hashlib.sha256()
    h.update(b"RULES_V2\x00")
    h.update(rules)
    h.update(b"\x00TOKENIZER\x00")
    h.update(b"49930cc8a75188cf69a07c57914f0aabe383db11494aa03ed15629adee4c7e33")
    h.update(b"\x00TOOL\x00")
    h.update(toolsha.encode())
    h.update(b"\x00BOS=NO\x00EOS=NO\x00PARSE_SPECIAL=NO\x00NO_ESCAPE=YES")
    for u in corpus_units:
        h.update(b"\x00UNIT\x00")
        h.update(u["source_group"].encode())
        h.update(b"|")
        h.update(u["item_id"].encode())
        h.update(b"|")
        h.update(u["canonical_content_sha256"].encode())
        h.update(b"|")
        h.update(",".join(str(t) for t in u["input_token_ids"]).encode())
    for f in ("TOKEN_EVAL_SELECTION_v2.csv", "TOKEN_EVAL_INPUTS_v2.jsonl"):
        h.update(b"\x00FILE\x00")
        h.update(f.encode())
        h.update(b"\x00")
        h.update(open(C + "/" + f, "rb").read())
    tree = h.hexdigest()

    P()
    P("TOKEN_EVAL_CONTENT_TREE_V2_SHA256=%s" % tree)
    P("AUTHORITY_EXPECTED              =%s" % EXPECTED)
    P("TOKEN_REFERENCE_HASH_MATCH=%s" % ("YES" if tree == EXPECTED else "NO"))
    P()
    P("LLAMA_DECODE_CALL_COUNT=%d" % DECODE_CALL_COUNT)
    P("TOKEN_REFERENCE_HASH_REDERIVED=%s"
      % ("YES" if tree == EXPECTED else "NO"))

    out = os.path.join(STAGING, "REC01_TOKEN_V2_REDERIVATION.txt")
    with open(out, "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(L) + "\n")
    print("\nECRIT: %s" % out)
    return 0 if tree == EXPECTED else 1


if __name__ == "__main__":
    sys.exit(main())
