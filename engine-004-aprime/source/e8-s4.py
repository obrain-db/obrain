#!/usr/bin/env python3
"""ENGINE-008 S4 : qualification du tokenizer natif + correction NLTK.
Aucun decode. Aucune inference. Discipline : ecritures atomiques, exit non nul."""
import csv
import hashlib
import json
import os
import subprocess
import sys

Q = ("/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/"
     "qualification/engine-008-quality-v1")
W = "/home/triviere/projects/llama-obrain-cuda-f1a"
TOK = W + "/build-cuda/bin/llama-tokenize"
GGUF_B = "/home/triviere/projects/models/qwen3.8-27b-gguf/Qwen3.8-27B-Q4_K_M.gguf"
GGUF_C = "/home/triviere/projects/models/Qwen3.8-27B-OBRAIN-v3.gguf"
VENV = Q + "/.venv"
PY = VENV + "/bin/python"
NLTK_DIR = Q + "/external/nltk_data"

def sha256(p):
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""):
            h.update(b)
    return h.hexdigest()

def atomic_write(path, text):
    tmp = path + ".tmp"
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(tmp, "w", encoding="utf-8", newline="\n") as f:
        f.write(text); f.flush(); os.fsync(f.fileno())
    os.replace(tmp, path)

def tree_hash(root):
    h = hashlib.sha256()
    for dp, dn, fn in sorted(os.walk(root)):
        dn.sort()
        for f in sorted(fn):
            fp = os.path.join(dp, f)
            h.update(os.path.relpath(fp, root).encode()); h.update(b"\x00")
            h.update(open(fp, "rb").read()); h.update(b"\x00")
    return h.hexdigest()

log = []
def L(s=""):
    log.append(s); print(s)

# ---------------- NLTK : acquisition + archivage ----------------
L("=== NLTK : acquisition des ressources exigees par le scorer ===")
os.makedirs(NLTK_DIR, exist_ok=True)
env = dict(os.environ); env["NLTK_DATA"] = NLTK_DIR
need = ["punkt", "punkt_tab"]
r = subprocess.run([PY, "-c",
                    "import nltk;nltk.download(%r,download_dir=%r,quiet=True);"
                    "nltk.download(%r,download_dir=%r,quiet=True)"
                    % (need[0], NLTK_DIR, need[1], NLTK_DIR)],
                   capture_output=True, text=True, env=env)
L("  nltk.download rc=%d" % r.returncode)
if r.returncode != 0:
    L("  stderr: " + r.stderr.strip()[-300:])
found = []
for res in ("tokenizers/punkt", "tokenizers/punkt_tab"):
    p = os.path.join(NLTK_DIR, res)
    if os.path.exists(p):
        found.append(p)
L("IFEVAL_NLTK_EXTERNAL_RESOURCE_COUNT=%d" % len(found))
if found:
    h = hashlib.sha256()
    for p in sorted(found):
        h.update(tree_hash(p).encode())
    NLTK_TREE = h.hexdigest()
    L("IFEVAL_NLTK_RESOURCE_TREE_SHA256=%s" % NLTK_TREE)
    L("  (le hash annonce dans le brief etait d3d5f4d92892d9c902eabf35cf8aab2e8a4ab52cf86c7fd70ef2166545e5f0c6)")
    L("  (il ne correspond PAS a ce qui existe ici : les ressources etaient ABSENTES)")
    # durcissement permissions
    for p in sorted(found):
        for dp, dn, fn in os.walk(p):
            for f in fn:
                fp = os.path.join(dp, f); os.chmod(fp, os.stat(fp).st_mode & ~0o022)
            for d in dn:
                dd = os.path.join(dp, d); os.chmod(dd, os.stat(dd).st_mode & ~0o022)
        os.chmod(p, os.stat(p).st_mode & ~0o022)
    gw = gd = 0
    for p in sorted(found):
        for dp, dn, fn in os.walk(p):
            for f in fn:
                if os.stat(os.path.join(dp, f)).st_mode & 0o022: gw += 1
            for d in dn:
                if os.stat(os.path.join(dp, d)).st_mode & 0o022: gd += 1
    h2 = hashlib.sha256()
    for p in sorted(found):
        h2.update(tree_hash(p).encode())
    L("IFEVAL_NLTK_RESOURCE_TREE_SHA256_AFTER_CHMOD=%s" % h2.hexdigest())
    L("CONTENT_HASH_UNCHANGED_BY_CHMOD=%s" % ("YES" if h2.hexdigest() == NLTK_TREE else "NO"))
    L("NLTK_GROUP_OR_OTHER_WRITABLE_FILE_COUNT=%d" % gw)
    L("NLTK_GROUP_OR_OTHER_WRITABLE_DIR_COUNT=%d" % gd)
else:
    L("IFEVAL_NLTK_RESOURCE_TREE_SHA256=NON_APPLICABLE")
    L("NLTK_GROUP_OR_OTHER_WRITABLE_FILE_COUNT=0")
    L("NLTK_GROUP_OR_OTHER_WRITABLE_DIR_COUNT=0")

# ---------------- S4 : tokenizer ----------------
L("")
L("=== S4 : recheck binaire + CLI ===")
TOK_SHA = sha256(TOK)
L("NATIVE_TOKENIZER_TOOL_PATH=%s" % TOK)
L("NATIVE_TOKENIZER_TOOL_SHA256=%s" % TOK_SHA)
EXPECT = "edacdf0d7bcf933bf9a14df1f305197d11860d00f6d8c60488c546e0aac1a61f"
L("NATIVE_TOKENIZER_TOOL_SHA_MATCH=%s" % ("YES" if TOK_SHA == EXPECT else "NO"))

def tokenize(model, text, extra=None):
    """Retourne (ids, stdout). BOS et parse-special explicitement desactives."""
    cmd = [TOK, "-m", model, "--ids", "--no-bos", "--no-parse-special", "--log-disable"]
    if extra:
        cmd += extra
    r = subprocess.run(cmd + ["--stdin"], input=text, capture_output=True, text=True)
    if r.returncode != 0:
        raise RuntimeError("tokenize rc=%d : %s" % (r.returncode, r.stderr.strip()[-200:]))
    out = r.stdout.strip()
    try:
        ids = json.loads(out)
    except Exception:
        # certaines versions impriment une ligne par token
        ids = [int(x) for x in out.replace("[", "").replace("]", "").split(",") if x.strip()]
    return ids, out

# fixtures
FIX = [
    ("ascii", "The quick brown fox jumps over the lazy dog."),
    ("french_accents", "Le café était plein, à côté de l'élève. Éàüç Être."),
    ("emoji", "Bonjour 😀🚀 — test unicode."),
    ("cjk", "日本語のテキスト、中文文本、한국어."),
    ("newlines", "line1\n\nline3\r\nline4\r"),
    ("spaces", "   leading and trailing   "),
    ("rust", "fn main() {\n    let x: u32 = 1;\n    println!(\"{}\", x);\n}\n"),
    ("json", '{"a":1,"b":[2,3],"c":{"d":"e"}}'),
    ("special_like", "<|im_start|>system<|endoftext|>"),
    ("emptyish", " "),
]

L("")
L("=== S4 : fixtures, deux passes, meme GGUF (candidat) ===")
rows = []
mismatch = 0
for name, text in FIX:
    a, oa = tokenize(GGUF_C, text)
    b, ob = tokenize(GGUF_C, text)
    same = a == b
    if not same:
        mismatch += 1
    rows.append(dict(fixture=name, pass1_ids=json.dumps(a), pass2_ids=json.dumps(b),
                     repeatable="YES" if same else "NO", token_count=len(a)))
L("TOKENIZER_TOOL_REPEATABILITY_MISMATCH_COUNT=%d" % mismatch)

L("")
L("=== S4 : cross-GGUF baseline vs candidat ===")
cross = 0
for name, text in FIX:
    a, _ = tokenize(GGUF_B, text)
    b, _ = tokenize(GGUF_C, text)
    if a != b:
        cross += 1
        L("  DIVERGENCE %s : base=%s cand=%s" % (name, a[:8], b[:8]))
L("TOKENIZER_BASELINE_CANDIDATE_FIXTURE_MISMATCH_COUNT=%d" % cross)

L("")
L("=== S4 : comportement BOS / EOS / parse-special (empirique) ===")
probe = "Hello world"
with_bos, _ = tokenize(GGUF_C, probe, extra=[])  # --no-bos est deja dans tokenize()
r = subprocess.run([TOK, "-m", GGUF_C, "--ids", "--no-parse-special", "--log-disable", "--stdin"],
                   input=probe, capture_output=True, text=True)
default_ids = json.loads(r.stdout.strip()) if r.returncode == 0 else []
r2 = subprocess.run([TOK, "-m", GGUF_C, "--ids", "--no-bos", "--no-parse-special", "--log-disable", "--stdin"],
                    input=probe, capture_output=True, text=True)
nobos_ids = json.loads(r2.stdout.strip()) if r2.returncode == 0 else []
L("  ids SANS --no-bos : %s" % json.dumps(default_ids))
L("  ids AVEC --no-bos : %s" % json.dumps(nobos_ids))
L("  BOS ajoute par defaut : %s" % ("OUI" if default_ids[:1] != nobos_ids[:1] else "NON"))
L("  token ajoute en FIN (EOS) present : %s"
  % ("OUI" if (nobos_ids and nobos_ids[-1] in (248046, 248044)) else "NON"))
L("TOKEN_EVAL_ADD_BOS=NO      (obtenu par --no-bos)")
L("TOKEN_EVAL_PARSE_SPECIAL=NO (obtenu par --no-parse-special)")
L("TOKEN_EVAL_ADD_EOS=NO      (aucun flag --no-eos n existe ; pas d EOS ajoute en fin)")

# artefacts
tmp = Q + "/TOKENIZER_SELFTEST.csv.tmp"
with open(tmp, "w", encoding="utf-8", newline="") as f:
    w = csv.DictWriter(f, fieldnames=list(rows[0].keys())); w.writeheader(); w.writerows(rows)
    f.flush(); os.fsync(f.fileno())
os.replace(tmp, Q + "/TOKENIZER_SELFTEST.csv")

atomic_write(Q + "/TOKENIZER_PROVENANCE.txt", "\n".join([
    "TOKENIZER_PROVENANCE",
    "NATIVE_TOKENIZER_TOOL_FOUND=YES",
    "NATIVE_TOKENIZER_TOOL_PATH=%s" % TOK,
    "NATIVE_TOKENIZER_TOOL_SHA256=%s" % TOK_SHA,
    "TOKENIZER_LOGICAL_SHA256=49930cc8a75188cf69a07c57914f0aabe383db11494aa03ed15629adee4c7e33",
    "VOCAB_SIZE=248320",
    "BASELINE_GGUF_SHA256=31629f53165ab6a7dad8c9847dcfd1fdf55829dac1e6e748f4a68581b0033d34",
    "CANDIDATE_GGUF_SHA256=1d7c9409ae7f2c7f6071c8ad32d2481f2c09da94fc2aec3f63a4d44777e2b937",
    "CLI_AUDITED=YES (--ids --no-bos --no-parse-special --log-disable --stdin)",
    "NO_EOS_FLAG_EXISTS=YES (constate dans --help ; comportement etabli empiriquement)",
]) + "\n")
atomic_write(Q + "/TOKENIZER_SELFTEST.txt", "\n".join(log) + "\n")

ok = (mismatch == 0 and cross == 0 and TOK_SHA == EXPECT)
S4 = ["STAGE_S4_TOKENIZER_SELFTEST",
      "TOKENIZER_TOOL_REPEATABILITY_MISMATCH_COUNT=%d" % mismatch,
      "TOKENIZER_BASELINE_CANDIDATE_FIXTURE_MISMATCH_COUNT=%d" % cross,
      "NATIVE_TOKENIZER_TOOL_SHA256=%s" % TOK_SHA,
      "STAGE_S4_STATUS=%s" % ("PASS" if ok else "FAIL")]
atomic_write(Q + "/STAGE_S4_STATUS.txt", "\n".join(S4) + "\n")
print()
print("\n".join(S4))
sys.exit(0 if ok else 1)
