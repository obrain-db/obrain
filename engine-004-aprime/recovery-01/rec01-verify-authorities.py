#!/usr/bin/env python3
"""ENGINE-008 RECOVERY-01 -- verification des autorites + manifeste.

LECTURE SEULE sur l arbre d autorite. Les sorties vont sous STAGING.
Aucun llama_decode. Aucune ecriture dans l arbre gele.

Recalcule, quand la definition est connue, chaque hash d autorite et le
compare a la valeur publiee. Ne complete JAMAIS un SHA par approximation.
"""
import hashlib
import json
import os
import subprocess
import sys

Q = ("/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/"
     "qualification/engine-008-quality-v1")
STAGING = sys.argv[1]
STAGING = STAGING if STAGING.startswith("/") else os.path.join("/tmp", STAGING)
os.makedirs(STAGING, exist_ok=True)

LLAMA_DECODE_CALL_COUNT = 0

AUTH = {
    "TASK_PROMPT_CONTRACTS_V1":
        "fa30f16d95a564491e6d02666350ba783d6f44493ea5d04cef024e92ef0c3bc6",
    "ENGINE008_SCORER_PROVENANCE":
        "6d9e68cf38de67267be8ec68583ff2746dac37a944233eb5468100e8192d1ffa",
    "TASK_EVAL_DATA_CONTENT_TREE":
        "c30d71d84712fae390be582111f0473c87ccaaad7411e1ef99b97689d69ab84a",
    "ENGINE008_SELECTION_RULES_V2":
        "3c1a16d7053f693400b3a8d858733f6a884301e34c8796e8a482370d5a369552",
    "Q1_LIBLLAMA":
        "87c42d5f5dd7ab3c10967b4d3eaac82636b6af11e2c18a7f2798bab2d01bb2b2",
    "Q1_TOKENIZER_TOOL":
        "edacdf0d7bcf933bf9a14df1f305197d11860d00f6d8c60488c546e0aac1a61f",
    "QUALITY_BASELINE_MODEL":
        "31629f53165ab6a7dad8c9847dcfd1fdf55829dac1e6e748f4a68581b0033d34",
    "QUALITY_CANDIDATE_MODEL":
        "1d7c9409ae7f2c7f6071c8ad32d2481f2c09da94fc2aec3f63a4d44777e2b937",
}

Q1_LIB = "/home/triviere/projects/llama-obrain-cuda-f1a/build-cuda/bin/libllama.so.0"
Q1_TOK = "/home/triviere/projects/llama-obrain-cuda-f1a/build-cuda/bin/llama-tokenize"
GGUF_B = "/home/triviere/projects/models/qwen3.8-27b-gguf/Qwen3.8-27B-Q4_K_M.gguf"
GGUF_C = "/home/triviere/projects/models/Qwen3.8-27B-OBRAIN-v3.gguf"

L = []


def P(s=""):
    L.append(s)
    print(s, flush=True)


def sha(p):
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for b in iter(lambda: f.read(1 << 22), b""):
            h.update(b)
    return h.hexdigest()


def tree_hash(root):
    """Definition EXACTE de e8s9-final.py / engine008-corpora-task.py."""
    h = hashlib.sha256()
    for dp, _dn, fn in sorted(os.walk(root)):
        for f in sorted(fn):
            fp = os.path.join(dp, f)
            h.update(os.path.relpath(fp, root).encode())
            h.update(b"\x00")
            h.update(open(fp, "rb").read())
            h.update(b"\x00")
    return h.hexdigest()


def check(name, got, expect=None):
    expect = expect or AUTH.get(name)
    ok = (got == expect)
    P("%-42s %s" % (name, "MATCH" if ok else "MISMATCH"))
    P("    computed=%s" % got)
    P("    expected=%s" % expect)
    return ok


def main():
    P("ENGINE-008 RECOVERY-01 -- VERIFICATION DES AUTORITES")
    P("=" * 74)
    P("HOST=%s" % os.uname().nodename)
    P("LLAMA_DECODE_CALL_COUNT=%d" % LLAMA_DECODE_CALL_COUNT)
    P("WRITES_TO_AUTHORITY_TREE=NO")
    P("STAGING=%s" % STAGING)
    P()

    results = {}

    P("=== §5.1 ARBRE DATA TASK ===")
    if os.path.isdir(Q + "/corpora/task_eval"):
        gt = tree_hash(Q + "/corpora/task_eval")
        results["TASK_EVAL_DATA_CONTENT_TREE"] = check(
            "TASK_EVAL_DATA_CONTENT_TREE", gt)
    else:
        P("TASK_EVAL_DATA_CONTENT_TREE                  ABSENT")
        results["TASK_EVAL_DATA_CONTENT_TREE"] = False
    P()

    P("=== §5.2 CONTRATS ET SEUILS ===")
    for name, rel in (("TASK_PROMPT_CONTRACTS_V1", "TASK_PROMPT_CONTRACTS_v1.txt"),
                      ("ENGINE008_SELECTION_RULES_V2", "ENGINE008_SELECTION_RULES_v2.txt")):
        p = Q + "/" + rel
        if os.path.exists(p):
            results[name] = check(name, sha(p))
        else:
            P("%-42s ABSENT" % name)
            results[name] = False
    for extra in ("QUALITY_THRESHOLDS_v1.txt", "STRUCTURED_VALIDITY_CONFIG_v1.txt"):
        p = Q + "/" + extra
        P("%-42s sha256=%s" % (extra, sha(p) if os.path.exists(p) else "ABSENT"))
    P()

    P("=== §5.3 PROVENANCE DES SCORERS (recalculee depuis le disque) ===")
    DST = Q + "/scorers"
    if os.path.isdir(DST):
        files = sorted(f for f in os.listdir(DST)
                       if f.endswith(".py"))
        h = hashlib.sha256()
        for f in files:
            h.update(f.encode())
            h.update(b"\x00")
            h.update(open(os.path.join(DST, f), "rb").read())
            h.update(b"\x00")
        for extra in ("STRUCTURED_VALIDITY_CONFIG_v1.txt",
                      "TASK_PROMPT_CONTRACTS_v1.txt"):
            h.update(extra.encode())
            h.update(b"\x00")
            h.update(open(Q + "/" + extra, "rb").read())
            h.update(b"\x00")
        h.update(open(Q + "/QUALITY_SCORER_PROVENANCE.txt", "rb").read())
        results["ENGINE008_SCORER_PROVENANCE"] = check(
            "ENGINE008_SCORER_PROVENANCE", h.hexdigest())
        P("    fichiers scorers : %d" % len(files))
        for f in files:
            P("      %s  %s" % (sha(os.path.join(DST, f)), f))
    else:
        P("SCORERS                                       ABSENT")
        results["ENGINE008_SCORER_PROVENANCE"] = False
    P()

    P("=== §4 IDENTITE DU BINAIRE Q1 ET DE SON OUTIL ===")
    for name, p in (("Q1_LIBLLAMA", Q1_LIB), ("Q1_TOKENIZER_TOOL", Q1_TOK)):
        if os.path.exists(p):
            results[name] = check(name, sha(p))
            P("    path=%s" % os.path.realpath(p))
        else:
            P("%-42s ABSENT (%s)" % (name, p))
            results[name] = False
    P()

    P("=== §4bis MODELES D AUTORITE (chemins documentes) ===")
    models = {}
    for name, p in (("QUALITY_BASELINE_MODEL", GGUF_B),
                    ("QUALITY_CANDIDATE_MODEL", GGUF_C)):
        if os.path.exists(p):
            sz = os.path.getsize(p)
            P("%s : %d octets, hachage en cours..." % (name, sz))
            g = sha(p)
            results[name] = check(name, g)
            models[name] = dict(path=p, size=sz, sha256=g,
                                matches_authority=(g == AUTH[name]))
        else:
            P("%-42s ABSENT (%s)" % (name, p))
            results[name] = False
            models[name] = dict(path=p, present=False)
    P()

    # -------- manifeste --------
    P("=== MANIFESTE DE L ARBRE D AUTORITE ===")
    manifest = []
    total = 0
    for dp, dn, fn in os.walk(Q):
        dn.sort()
        for f in sorted(fn):
            fp = os.path.join(dp, f)
            try:
                st = os.stat(fp)
            except OSError:
                continue
            rel = os.path.relpath(fp, Q)
            total += st.st_size
            manifest.append(dict(path=rel, size=st.st_size, sha256=sha(fp)))
    P("FILE_COUNT=%d" % len(manifest))
    P("TOTAL_BYTES=%d" % total)
    P()

    P("=== RESULTAT GLOBAL ===")
    npass = sum(1 for v in results.values() if v)
    P("AUTHORITY_CHECK_PASS_COUNT=%d" % npass)
    P("AUTHORITY_CHECK_FAIL_COUNT=%d" % (len(results) - npass))
    for k, v in sorted(results.items()):
        P("  %-42s %s" % (k, "PASS" if v else "FAIL"))
    P()
    P("LLAMA_DECODE_CALL_COUNT=%d" % LLAMA_DECODE_CALL_COUNT)

    with open(os.path.join(STAGING, "REC01_AUTHORITY_VERIFICATION.txt"), "w",
              encoding="utf-8", newline="\n") as f:
        f.write("\n".join(L) + "\n")
    with open(os.path.join(STAGING, "RECT01_MANIFEST_RAW.json"), "w",
              encoding="utf-8", newline="\n") as f:
        json.dump({"host": os.uname().nodename, "root": Q,
                   "file_count": len(manifest), "total_bytes": total,
                   "authority_checks": results,
                   "models": models,
                   "files": manifest}, f, indent=1, sort_keys=True)
    print("\nECRIT: REC01_AUTHORITY_VERIFICATION.txt, RECT01_MANIFEST_RAW.json")
    return 0 if all(results.values()) else 1


if __name__ == "__main__":
    sys.exit(main())
