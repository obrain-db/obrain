#!/usr/bin/env python3
"""ENGINE-008 DATA -- S2 (archive Python) + S3 (qualification tokenizer) + rebuild TASK."""
import csv, glob, hashlib, json, os, subprocess, sys

Q = ("/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/"
     "qualification/engine-008-quality-v1")
E = Q + "/external"
C = Q + "/corpora"
PY = Q + "/.venv/bin/python"
TOK = "/home/triviere/projects/llama-obrain-cuda-f1a/build-cuda/bin/llama-tokenize"
BASE_GGUF = "/home/triviere/projects/models/qwen3.8-27b-gguf/Qwen3.8-27B-Q4_K_M.gguf"
CAND_GGUF = "/home/triviere/projects/models/Qwen3.8-27B-OBRAIN-v3.gguf"
VOCAB = 248320


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


# ============================================== S2 : archive Python
lines = []
PKGS = ["pyarrow", "absl-py", "langdetect", "nltk", "immutabledict"]
DEST = E + "/python-packages"
os.makedirs(DEST, exist_ok=True)
# versions reellement installees
inst = {}
out = subprocess.run([PY, "-m", "pip", "freeze"], capture_output=True, text=True).stdout
for l in out.splitlines():
    if "==" in l:
        n, v = l.split("==", 1)
        inst[n.lower()] = v
for p in PKGS:
    v = inst.get(p.lower()) or inst.get(p.lower().replace("-", "_"))
    if not v:
        lines.append("  %s : VERSION INTROUVABLE" % p); continue
    # wheel si dispo, sinon sdist
    subprocess.run([PY, "-m", "pip", "download", "--no-deps", "--only-binary=:all:",
                    "%s==%s" % (p, v), "-d", DEST], capture_output=True, text=True)
    got = glob.glob(DEST + "/%s-*" % p.replace("-", "_")) + glob.glob(DEST + "/%s-*" % p)
    if not got:
        subprocess.run([PY, "-m", "pip", "download", "--no-deps", "--no-binary=:all:",
                        "%s==%s" % (p, v), "-d", DEST], capture_output=True, text=True)
        got = glob.glob(DEST + "/%s-*" % p.replace("-", "_")) + glob.glob(DEST + "/%s-*" % p)
    lines.append("  %-16s %-10s %s" % (p, v, [os.path.basename(g) for g in got]))

# provenance
prov = [("package", "version", "artifact_filename", "artifact_type", "sha256")]
art = sorted(glob.glob(DEST + "/*"))
for a in art:
    b = os.path.basename(a)
    at = "wheel" if b.endswith(".whl") else ("sdist" if b.endswith((".tar.gz", ".zip")) else "other")
    pkg = b.split("-")[0]
    prov.append((pkg, inst.get(pkg.lower(), "?"), b, at, sha_file(a)))
with open(Q + "/PYTHON_PACKAGE_PROVENANCE.csv", "w", newline="", encoding="utf-8") as f:
    csv.writer(f).writerows(prov)
atomic(Q + "/PYTHON_PACKAGE_ARTIFACTS.sha256",
       "".join("%s  %s\n" % (sha_file(a), os.path.basename(a)) for a in art))
cover = {p for p in prov[1:]}
complete = len({r[0] for r in prov[1:]}) >= 5
stage("S2", complete, lines + ["PYTHON_DEPENDENCY_ARTIFACT_ARCHIVE_COMPLETE=%s"
                               % ("YES" if complete else "NO")] +
      ["  artefacts archives : %d" % len(art)])

# ============================================== S3 : tokenizer selftest
lines = []
TOKSHA = sha_file(TOK)
lines.append("NATIVE_TOKENIZER_TOOL_PATH=" + TOK)
lines.append("NATIVE_TOKENIZER_TOOL_SHA256=" + TOKSHA)

FIX = {
    "ascii": "The quick brown fox jumps over the lazy dog.",
    "fr_accents": "Où sont les éléphants ? À côté de la rivière, très près.",
    "unicode": "日本語のテキストと Émojis 🐘🔬 et symboles ∑∫√",
    "newlines": "ligne1\n\nligne3\n\n\nligne6",
    "rust": "fn main() {\n    let x: u32 = 42;\n    println!(\"{}\", x);\n}",
    "json": '{"a":1,"b":[true,null,"x"]}',
    "spaces": "   espaces avant et apres   ",
    "special_like": "<|im_start|>system<|im_end|> [INST] <<SYS>>",
}
fx = Q + "/external/tokenizer_fixtures"
os.makedirs(fx, exist_ok=True)
for k, v in FIX.items():
    open(fx + "/" + k + ".txt", "w", encoding="utf-8", newline="\n").write(v)


def tokenize(gguf, text):
    """llama-tokenize : sortie 'tokens' par defaut. On passe le texte par stdin."""
    # CLI reel : --ids (sortie [1,2,3] parseable), --stdin, --no-bos,
    # --no-parse-special (le texte n est jamais interprete comme token de controle),
    # --log-disable. Il n existe PAS de --no-eos : EOS n est pas ajoute par defaut.
    r = subprocess.run([TOK, "-m", gguf, "--ids", "--stdin", "--no-bos",
                        "--no-parse-special", "--log-disable"],
                       input=text.encode(), capture_output=True)
    if r.returncode != 0:
        raise RuntimeError("llama-tokenize rc=%d : %s" % (r.returncode, r.stderr.decode()[:300]))
    return json.loads(r.stdout.decode().strip())


# detecter le format de sortie
probe = tokenize(BASE_GGUF, FIX["ascii"])
lines.append("  exemple de sortie : %s" % str(probe[:12]))
if not isinstance(probe, list):
    stage("S3", False, lines + ["  sortie non parseable en liste JSON"])

mismatch = 0
allids = {}
for k, v in sorted(FIX.items()):
    a = tokenize(BASE_GGUF, v)
    b = tokenize(BASE_GGUF, v)
    if a != b:
        mismatch += 1
        lines.append("  REPEATABILITE KO sur %s" % k)
    allids[k] = a
stage("S3", mismatch == 0, lines + ["TOKENIZER_TOOL_REPEATABILITY_MISMATCH_COUNT=%d" % mismatch])

print()
print("\n".join(lines))
