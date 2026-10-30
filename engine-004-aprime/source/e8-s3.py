#!/usr/bin/env python3
"""ENGINE-008 S3 : archive reproductible des dependances Python + selftest offline.
Discipline : py_compile, ecritures atomiques, exit code non nul, pas de except-fourre-tout."""
import csv
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile

Q = ("/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/"
     "qualification/engine-008-quality-v1")
PKG = Q + "/external/python-packages"
VENV = Q + "/.venv"
PY = VENV + "/bin/python"

REQUIRED = ["pyarrow==25.0.1", "absl-py==2.5.0", "langdetect==1.0.9",
            "nltk==3.10.3", "immutabledict==4.3.1"]
# deps de build indispensables au sdist langdetect (deja source d'echec)
BUILD_DEPS = ["setuptools", "six", "wheel"]

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

os.makedirs(PKG, exist_ok=True)

# ---- 1. wheels pour tout, puis sdist explicite pour langdetect ----
def pip_dl(args, label):
    r = subprocess.run([PY, "-m", "pip", "download", "--quiet"] + args + ["-d", PKG],
                       capture_output=True, text=True)
    print("  %-42s rc=%d" % (label, r.returncode))
    if r.returncode != 0:
        print("    stderr: %s" % r.stderr.strip()[:300])
    return r.returncode

print("=== telechargement des artefacts ===")
for spec in REQUIRED:
    pip_dl(["--only-binary=:all:", spec], "wheel " + spec)
pip_dl(["--no-deps", "--no-binary=:all:", "langdetect==1.0.9"], "sdist langdetect")
pip_dl(["--only-binary=:all:"] + BUILD_DEPS, "deps de build")

# ---- 2. provenance ----
arts = sorted(os.listdir(PKG))
prov = []
seen = {}
for a in arts:
    if not (a.endswith(".whl") or a.endswith(".tar.gz") or a.endswith(".zip")):
        continue
    name = a.split("-")[0]
    ver = a.split("-")[1] if len(a.split("-")) > 1 else "?"
    kind = "wheel" if a.endswith(".whl") else "sdist"
    seen[name] = (ver, a, kind)
for name in sorted(seen):
    ver, a, kind = seen[name]
    prov.append(dict(package=name, version=ver, artifact_filename=a, artifact_type=kind,
                     sha256=sha256(os.path.join(PKG, a)),
                     reason="build dependency" if name in ("setuptools", "six", "wheel") else "engine-008 runtime"))
tmp = Q + "/PYTHON_PACKAGE_PROVENANCE.csv.tmp"
with open(tmp, "w", encoding="utf-8", newline="") as f:
    w = csv.DictWriter(f, fieldnames=["package", "version", "artifact_filename",
                                      "artifact_type", "sha256", "reason"])
    w.writeheader(); w.writerows(prov)
    f.flush(); os.fsync(f.fileno())
os.replace(tmp, Q + "/PYTHON_PACKAGE_PROVENANCE.csv")
atomic_write(Q + "/PYTHON_PACKAGE_ARTIFACTS.sha256",
             "".join("%s  %s\n" % (sha256(os.path.join(PKG, a)), a) for a in arts
                     if a.endswith((".whl", ".tar.gz", ".zip"))))
print("  artefacts archives : %d" % len(prov))

# ---- 3. selftest offline dans un venv neuf ----
print()
print("=== selftest de reinstallation OFFLINE ===")
TMP = tempfile.mkdtemp(prefix="e8-reinstall-")
ok = False
log = []
try:
    r = subprocess.run([sys.executable, "-m", "venv", TMP], capture_output=True, text=True)
    log.append("venv rc=%d" % r.returncode)
    tpy = os.path.join(TMP, "bin", "python")
    # --no-index : AUCUN reseau. Installation exclusive depuis l archive.
    r = subprocess.run([tpy, "-m", "pip", "install", "--quiet", "--no-index",
                        "--find-links", PKG] + REQUIRED, capture_output=True, text=True)
    log.append("pip install --no-index rc=%d" % r.returncode)
    if r.returncode != 0:
        log.append("stderr: " + r.stderr.strip()[-600:])
    else:
        chk = subprocess.run([tpy, "-c",
            "import importlib.metadata as m,pyarrow,absl,langdetect,nltk,immutabledict;"
            "print('pyarrow='+pyarrow.__version__);"
            "print('absl-py='+m.version('absl-py'));"
            "print('langdetect='+m.version('langdetect'));"
            "print('nltk='+m.version('nltk'));"
            "print('immutabledict='+m.version('immutabledict'))"],
            capture_output=True, text=True)
        log.append("imports rc=%d" % chk.returncode)
        log.append(chk.stdout.strip())
        log.append(chk.stderr.strip()[-400:])
        if chk.returncode == 0:
            # le scorer IFEval doit aussi s importer dans ce venv
            code = ("import sys;sys.path.insert(0," + repr(Q + "/external/mbpp") + ");"
                    "from instruction_following_eval import instructions_registry as R;"
                    "print('INSTRUCTION_DICT=' + str(len(R.INSTRUCTION_DICT)))")
            r2 = subprocess.run([tpy, "-c", code], capture_output=True, text=True)
            log.append("ifeval scorer rc=%d : %s" % (r2.returncode, r2.stdout.strip()))
            log.append(r2.stderr.strip()[-400:])
            ok = (r2.returncode == 0)
finally:
    shutil.rmtree(TMP, ignore_errors=True)

print("\n".join("  " + l for l in log))
atomic_write(Q + "/PYTHON_ENVIRONMENT_REINSTALL_SELFTEST.txt",
             "PYTHON_ENVIRONMENT_REINSTALL_SELFTEST=%s\n\n" % ("PASS" if ok else "FAIL") +
             "\n".join(log) + "\n")

# ---- 4. NLTK : ressources + durcissement des permissions ----
print()
print("=== NLTK ressources ===")
nltk_lines = []
r = subprocess.run([PY, "-c", "import nltk,os;print(nltk.data.path)"],
                   capture_output=True, text=True)
nltk_lines.append("nltk.data.path=" + r.stdout.strip())
found = []
for base in ["/home/triviere/nltk_data", "/usr/share/nltk_data", "/usr/local/share/nltk_data",
             "/usr/lib/nltk_data", "/usr/local/lib/nltk_data"]:
    for res in ("tokenizers/punkt", "tokenizers/punkt_tab"):
        p = os.path.join(base, res)
        if os.path.exists(p):
            found.append(p)
for p in found:
    nltk_lines.append("resource=" + p)
nltk_lines.append("IFEVAL_NLTK_EXTERNAL_RESOURCE_COUNT=%d" % len(found))

def tree_hash(root):
    h = hashlib.sha256()
    for dp, dn, fn in sorted(os.walk(root)):
        dn.sort()
        for f in sorted(fn):
            fp = os.path.join(dp, f)
            h.update(os.path.relpath(fp, root).encode()); h.update(b"\x00")
            h.update(open(fp, "rb").read()); h.update(b"\x00")
    return h.hexdigest()

if found:
    # hash AVANT durcissement
    agg = hashlib.sha256()
    for p in sorted(found):
        agg.update(tree_hash(p).encode())
    pre = agg.hexdigest()
    nltk_lines.append("IFEVAL_NLTK_RESOURCE_TREE_SHA256=%s" % pre)
    # durcissement : retirer write groupe/autres
    for p in sorted(found):
        for dp, dn, fn in os.walk(p):
            for f in fn:
                fp = os.path.join(dp, f)
                m = os.stat(fp).st_mode & 0o777
                os.chmod(fp, m & ~0o022)
            for d in dn:
                dpth = os.path.join(dp, d)
                m = os.stat(dpth).st_mode & 0o777
                os.chmod(dpth, m & ~0o022)
        m = os.stat(p).st_mode & 0o777
        os.chmod(p, m & ~0o022)
    gw = gd = 0
    for p in sorted(found):
        for dp, dn, fn in os.walk(p):
            for f in fn:
                if os.stat(os.path.join(dp, f)).st_mode & 0o022: gw += 1
            for d in dn:
                if os.stat(os.path.join(dp, d)).st_mode & 0o022: gd += 1
    agg2 = hashlib.sha256()
    for p in sorted(found):
        agg2.update(tree_hash(p).encode())
    post = agg2.hexdigest()
    nltk_lines.append("IFEVAL_NLTK_RESOURCE_TREE_SHA256_AFTER_CHMOD=%s" % post)
    nltk_lines.append("NLTK_GROUP_OR_OTHER_WRITABLE_FILE_COUNT=%d" % gw)
    nltk_lines.append("NLTK_GROUP_OR_OTHER_WRITABLE_DIR_COUNT=%d" % gd)
    nltk_lines.append("CONTENT_HASH_UNCHANGED_BY_CHMOD=%s" % ("YES" if pre == post else "NO"))
else:
    nltk_lines.append("IFEVAL_NLTK_RESOURCE_TREE_SHA256=NON_APPLICABLE")
    nltk_lines.append("NLTK_GROUP_OR_OTHER_WRITABLE_FILE_COUNT=0")
    nltk_lines.append("NLTK_GROUP_OR_OTHER_WRITABLE_DIR_COUNT=0")
print("\n".join("  " + l for l in nltk_lines))
atomic_write(Q + "/NLTK_RESOURCE_AUDIT.txt", "\n".join(nltk_lines) + "\n")

complete = len(prov) >= 6 and any(p["package"] == "langdetect" and p["artifact_type"] == "sdist"
                                  for p in prov)
L = ["STAGE_S3_PYTHON_ARCHIVE", "PYTHON_DEPENDENCY_ARTIFACT_ARCHIVE_COMPLETE=%s"
     % ("YES" if complete else "NO"),
     "PYTHON_ENVIRONMENT_REINSTALL_SELFTEST=%s" % ("PASS" if ok else "FAIL"),
     "ARTIFACT_COUNT=%d" % len(prov)] + nltk_lines + \
    ["STAGE_S3_STATUS=%s" % ("PASS" if (complete and ok) else "FAIL")]
atomic_write(Q + "/STAGE_S3_STATUS.txt", "\n".join(L) + "\n")
print()
print("\n".join(L))
sys.exit(0 if (complete and ok) else 1)
