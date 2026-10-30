#!/usr/bin/env python3
"""ENGINE-008 : acquisition des ressources externes, revisions PINNEES.
Aucune inference modele. Telechargement + provenance + hashes uniquement."""
import hashlib
import json
import os
import subprocess
import sys

EXT = ("/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/"
       "qualification/engine-008-quality-v1/external")
os.makedirs(EXT, exist_ok=True)

def sha256_file(p):
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""):
            h.update(b)
    return h.hexdigest()

def note(d, name, lines):
    os.makedirs(d, exist_ok=True)
    with open(os.path.join(d, name), "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(lines) + "\n")

def hf_revision(repo):
    from huggingface_hub import HfApi
    api = HfApi()
    info = api.dataset_info(repo)
    return info.sha

def hf_get(repo, filename, dest, revision):
    from huggingface_hub import hf_hub_download
    p = hf_hub_download(repo_id=repo, filename=filename, revision=revision,
                        repo_type="dataset", local_dir=dest)
    return p

def git_clone(repo, dest, commit=None):
    """Clone puis checkout d un commit exact."""
    os.makedirs(dest, exist_ok=True)
    if not os.path.exists(os.path.join(dest, ".git")):
        subprocess.run(["git", "clone", "--quiet", repo, dest], check=True)
    if commit:
        subprocess.run(["git", "-C", dest, "fetch", "--quiet", "origin", commit],
                       check=False)
        subprocess.run(["git", "-C", dest, "checkout", "--quiet", commit], check=True)
    out = subprocess.run(["git", "-C", dest, "rev-parse", "HEAD"],
                         capture_output=True, text=True)
    return out.stdout.strip()

results = {}
log = []

# ---------------- 1. WIKITEXT-2 RAW ----------------
print("=== WIKITEXT-2 ===", flush=True)
d = EXT + "/wikitext"
try:
    rev = hf_revision("Salesforce/wikitext")
    for split in ("test", "train"):
        for suffix in ("parquet", "arrow"):
            pass
    # fichiers texte canoniques
    files = []
    try:
        p = hf_get("Salesforce/wikitext", "wikitext-2-raw-v1/test-00000-of-00001.parquet",
                   d, rev)
        files.append(p)
    except Exception as e:
        log.append("parquet KO: %s" % str(e)[:120])
    try:
        p = hf_get("Salesforce/wikitext", "wikitext-2-raw-v1/train-00000-of-00001.parquet",
                   d, rev)
        files.append(p)
    except Exception as e:
        log.append("train parquet KO: %s" % str(e)[:120])
    from huggingface_hub import HfApi
    info = HfApi().dataset_info("Salesforce/wikitext", revision=rev)
    note(d, "UPSTREAM.txt", ["Salesforce/wikitext", "config=wikitext-2-raw-v1", "split=test"])
    note(d, "REVISION.txt", ["UPSTREAM_URL=https://huggingface.co/datasets/Salesforce/wikitext",
                             "UPSTREAM_REVISION=" + rev])
    note(d, "LICENSE_UPSTREAM.txt", [json.dumps(info.cardData.get("license", "UNKNOWN")
                                                if info.cardData else "UNKNOWN")])
    note(d, "FILES.sha256", [sha256_file(p) + "  " + os.path.basename(p) for p in files])
    results["wikitext"] = {"revision": rev, "files": [os.path.basename(p) for p in files],
                           "license": (info.cardData or {}).get("license", "UNKNOWN")}
    print("  revision %s, %d fichiers" % (rev[:12], len(files)), flush=True)
except Exception as e:
    results["wikitext"] = {"error": str(e)[:300]}
    print("  ECHEC: %s" % str(e)[:200], flush=True)

# ---------------- 2. GSM8K ----------------
print("=== GSM8K ===", flush=True)
d = EXT + "/gsm8k"
try:
    rev = git_clone("https://github.com/openai/grade-school-math.git", d)
    t = os.path.join(d, "grade_school_math", "data", "test.jsonl")
    n = sum(1 for _ in open(t)) if os.path.exists(t) else 0
    note(d, "UPSTREAM.txt", ["https://github.com/openai/grade-school-math",
                             "file=grade_school_math/data/test.jsonl"])
    note(d, "REVISION.txt", ["UPSTREAM_URL=https://github.com/openai/grade-school-math",
                             "UPSTREAM_REVISION=" + rev])
    lic = os.path.join(d, "LICENSE")
    note(d, "FILES.sha256", [sha256_file(t) + "  test.jsonl"] +
         ([sha256_file(lic) + "  LICENSE"] if os.path.exists(lic) else []))
    results["gsm8k"] = {"revision": rev, "test_items": n,
                        "license_file": os.path.exists(lic)}
    print("  commit %s, %d items de test" % (rev[:12], n), flush=True)
except Exception as e:
    results["gsm8k"] = {"error": str(e)[:300]}
    print("  ECHEC: %s" % str(e)[:200], flush=True)

# ---------------- 3. MMLU-PRO ----------------
print("=== MMLU-PRO ===", flush=True)
d = EXT + "/mmlu-pro"
try:
    rev = hf_revision("TIGER-Lab/MMLU-Pro")
    files = []
    for fn in ("data/test-00000-of-00001.parquet", "data/test.parquet", "test.parquet"):
        try:
            files.append(hf_get("TIGER-Lab/MMLU-Pro", fn, d, rev)); break
        except Exception:
            continue
    from huggingface_hub import HfApi
    info = HfApi().dataset_info("TIGER-Lab/MMLU-Pro", revision=rev)
    lis = info.cardData.get("license", "UNKNOWN") if info.cardData else "UNKNOWN"
    note(d, "UPSTREAM.txt", ["TIGER-Lab/MMLU-Pro", "split=test"])
    note(d, "REVISION.txt", ["UPSTREAM_URL=https://huggingface.co/datasets/TIGER-Lab/MMLU-Pro",
                             "UPSTREAM_REVISION=" + rev])
    note(d, "LICENSE_UPSTREAM.txt", [str(lis)])
    note(d, "FILES.sha256", [sha256_file(p) + "  " + os.path.basename(p) for p in files])
    results["mmlu_pro"] = {"revision": rev, "files": [os.path.basename(p) for p in files],
                           "license": lis}
    print("  revision %s, %d fichiers" % (rev[:12], len(files)), flush=True)
except Exception as e:
    results["mmlu_pro"] = {"error": str(e)[:300]}
    print("  ECHEC: %s" % str(e)[:200], flush=True)

# ---------------- 4. MBPP ----------------
print("=== MBPP ===", flush=True)
d = EXT + "/mbpp"
try:
    rev = git_clone("https://github.com/google-research/google-research.git", d)
    subprocess.run(["git", "-C", d, "sparse-checkout", "set", "mbpp"], check=False)
    cands = ["mbpp/mbpp.jsonl", "mbpp/sanitized-mbpp.json", "mbpp/mbpp-sanitized.jsonl"]
    found = [c for c in cands if os.path.exists(os.path.join(d, c))]
    note(d, "UPSTREAM.txt", ["https://github.com/google-research/google-research",
                             "dir=mbpp", "files=" + ",".join(found)])
    note(d, "REVISION.txt", ["UPSTREAM_URL=https://github.com/google-research/google-research",
                             "UPSTREAM_REVISION=" + rev])
    note(d, "FILES.sha256", [sha256_file(os.path.join(d, c)) + "  " + c for c in found])
    results["mbpp"] = {"revision": rev, "files": found}
    print("  commit %s, fichiers: %s" % (rev[:12], found), flush=True)
except Exception as e:
    results["mbpp"] = {"error": str(e)[:300]}
    print("  ECHEC: %s" % str(e)[:200], flush=True)

# ---------------- 5. IFEVAL ----------------
print("=== IFEVAL ===", flush=True)
d = EXT + "/ifeval"
try:
    subprocess.run(["git", "-C", EXT + "/mbpp", "sparse-checkout", "set",
                    "mbpp", "instruction_following_eval"], check=False)
    base = EXT + "/mbpp/instruction_following_eval"
    found = []
    for root, _, fs in os.walk(base):
        for f in fs:
            found.append(os.path.relpath(os.path.join(root, f), base))
    rev = subprocess.run(["git", "-C", EXT + "/mbpp", "rev-parse", "HEAD"],
                         capture_output=True, text=True).stdout.strip()
    note(d, "UPSTREAM.txt", ["google-research/google-research/instruction_following_eval"]
         + ["  " + f for f in sorted(found)])
    note(d, "REVISION.txt", ["UPSTREAM_URL=https://github.com/google-research/google-research",
                             "UPSTREAM_REVISION=" + rev, "dir=instruction_following_eval"])
    note(d, "FILES.sha256", [sha256_file(os.path.join(base, f)) + "  " + f
                             for f in sorted(found)])
    results["ifeval"] = {"revision": rev, "file_count": len(found)}
    print("  commit %s, %d fichiers" % (rev[:12], len(found)), flush=True)
except Exception as e:
    results["ifeval"] = {"error": str(e)[:300]}
    print("  ECHEC: %s" % str(e)[:200], flush=True)

note(EXT, "ACQUISITION_RESULTS.json", [json.dumps(results, indent=1)])
note(EXT, "ACQUISITION_LOG.txt", log or ["(aucune erreur)"])
print()
print(json.dumps(results, indent=1)[:1500])
