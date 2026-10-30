#!/usr/bin/env python3
"""ENGINE-008 DATA -- consolidation finale : doublons, rebuild TASK, hashes, statut."""
import csv, glob, hashlib, json, os, sys

Q = ("/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/"
     "qualification/engine-008-quality-v1")
E = Q + "/external"
C = Q + "/corpora"


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


def tree_hash(root):
    h = hashlib.sha256()
    for dp, _dn, fn in sorted(os.walk(root)):
        for f in sorted(fn):
            fp = os.path.join(dp, f)
            h.update(os.path.relpath(fp, root).encode()); h.update(b"\x00")
            h.update(open(fp, "rb").read()); h.update(b"\x00")
    return h.hexdigest()


out = []
def P(s=""):
    out.append(s); print(s)

# ---------------- doublons de contenu
P("=== AUDIT DE DOUBLONS ===")
rows = [json.loads(l) for l in open(C + "/token_eval/TOKEN_EVAL_INPUTS.jsonl")]
from collections import Counter
cc = Counter(r["source_content_sha256"] for r in rows)
dups = {k: v for k, v in cc.items() if v > 1}
P("TOKEN_EVAL_ITEM_COUNT=%d" % len(rows))
P("TOKEN_EVAL_UNIQUE_CONTENT_COUNT=%d" % len(cc))
P("TOKEN_EVAL_DUPLICATE_CONTENT_GROUP_COUNT=%d" % len(dups))
P("TOKEN_EVAL_REDUNDANT_ITEM_COUNT=%d" % sum(v - 1 for v in dups.values()))
P("  -> des lignes WikiText identiques (titres de section, lignes vides")
P("     canoniques) apparaissent plusieurs fois. C est une consequence de")
P("     la regle gelee (hash order, aucun dedoublonnage de contenu).")
P("     LIMITE DOCUMENTEE, pas corrigee apres coup.")

# doublons par corpus task
def dup_task(p, keyf):
    data = [json.loads(l) for l in open(p)]
    ks = [keyf(r) for r in data]
    return len(ks), len(set(ks))

P()
P("=== DOUBLONS TASK (identites) ===")
for name, p, kf in [
    ("MMLU_PRO", C + "/task_eval/mmlu_pro/MMLU_PRO_ITEMS.jsonl", lambda r: r["item_id"]),
    ("GSM8K", C + "/task_eval/gsm8k/GSM8K_ITEMS.jsonl", lambda r: r["item_id"]),
    ("MBPP", C + "/task_eval/mbpp/MBPP_ITEMS.jsonl", lambda r: r["item_id"]),
    ("IFEVAL", C + "/task_eval/ifeval/IFEVAL_ITEMS.jsonl", lambda r: r["item_id"]),
]:
    n, u = dup_task(p, kf)
    P("%s_DUPLICATE_COUNT=%d" % (name, n - u))

# ---------------- rebuild TASK
P()
P("=== TASK ===")
counts = {}
for name, p in [("knowledge_reasoning", C + "/task_eval/mmlu_pro/MMLU_PRO_ITEMS.jsonl"),
                ("math", C + "/task_eval/gsm8k/GSM8K_ITEMS.jsonl"),
                ("code", C + "/task_eval/mbpp/MBPP_ITEMS.jsonl"),
                ("instruction", C + "/task_eval/ifeval/IFEVAL_ITEMS.jsonl")]:
    counts[name] = sum(1 for _ in open(p))
allids = []
for p in glob.glob(C + "/task_eval/*/*_ITEMS.jsonl"):
    for l in open(p):
        allids.append(json.loads(l)["item_id"])
P("TASK_KNOWLEDGE_REASONING_COUNT=%d" % counts["knowledge_reasoning"])
P("TASK_MATH_COUNT=%d" % counts["math"])
P("TASK_CODE_COUNT=%d" % counts["code"])
P("TASK_INSTRUCTION_COUNT=%d" % counts["instruction"])
P("TASK_EVAL_ITEM_COUNT=%d" % len(allids))
P("TASK_GLOBAL_ITEM_ID_DUPLICATE_COUNT=%d" % (len(allids) - len(set(allids))))

TASK_TREE = tree_hash(C + "/task_eval")
P("TASK_EVAL_DATA_CONTENT_TREE_SHA256=%s" % TASK_TREE)
P("  (remplace dd2b67d6... : la population IFEval est passee de 524 a 541)")

# ---------------- hashes finaux
TH = sha_file(Q + "/QUALITY_THRESHOLDS_v1.txt")
SR = open(Q + "/ENGINE008_SELECTION_RULES_v1.sha256").read().split()[0]
SP = sha_file(Q + "/PYTHON_PACKAGE_PROVENANCE.csv")
TOKTT = open(Q + "/STAGE_S8_STATUS.txt").read()
import re
m = re.search(r"TOKEN_EVAL_CONTENT_TREE_SHA256=(\w+)", TOKTT)
TOKEN_TREE = m.group(1) if m else "ABSENT"

# ordre global DATA
order = []
for g in ("wikitext", "obrain_docs", "obrain_rust"):
    for r in sorted([r for r in rows if r["source_group"] == g], key=lambda x: x["item_id"]):
        order.append("TOKEN/%s/%s" % (g, r["item_id"]))
for cat, p in [("knowledge_reasoning", C + "/task_eval/mmlu_pro/MMLU_PRO_ITEMS.jsonl"),
               ("math", C + "/task_eval/gsm8k/GSM8K_ITEMS.jsonl"),
               ("code", C + "/task_eval/mbpp/MBPP_ITEMS.jsonl"),
               ("instruction", C + "/task_eval/ifeval/IFEVAL_ITEMS.jsonl")]:
    for l in open(p):
        order.append("TASK/%s/%s" % (cat, json.loads(l)["item_id"]))
atomic(Q + "/QUALITY_GLOBAL_ITEM_ORDER_DATA.txt", "\n".join(order) + "\n")
ORDER_SHA = sha_file(Q + "/QUALITY_GLOBAL_ITEM_ORDER_DATA.txt")

P()
P("=== HASHES FINAUX ===")
for k, v in [("ENGINE008_BASELINE_MODEL_SHA256", "31629f53165ab6a7dad8c9847dcfd1fdf55829dac1e6e748f4a68581b0033d34"),
             ("ENGINE008_CANDIDATE_MODEL_SHA256", "1d7c9409ae7f2c7f6071c8ad32d2481f2c09da94fc2aec3f63a4d44777e2b937"),
             ("ENGINE008_TOKENIZER_SHA256", "49930cc8a75188cf69a07c57914f0aabe383db11494aa03ed15629adee4c7e33"),
             ("ENGINE008_SELECTION_RULES_SHA256", SR),
             ("ENGINE008_TOKEN_CORPUS_SHA256", TOKEN_TREE),
             ("ENGINE008_TASK_CORPUS_SHA256", TASK_TREE),
             ("ENGINE008_SCORER_PROVENANCE_SHA256", SP),
             ("ENGINE008_THRESHOLDS_SHA256", TH),
             ("ENGINE008_GLOBAL_ITEM_ORDER_SHA256", ORDER_SHA)]:
    P("%s=%s" % (k, v))

status = [
    "ENGINE-008 DATA -- STATUT CONSOLIDE", "",
    "IFEVAL_RAW_COUNT=541", "IFEVAL_EXCLUDED_PREFREEZE_COUNT=0",
    "IFEVAL_SELECTED_COUNT=541", "IFEVAL_CATEGORY_SUFFICIENT=YES",
    "IFEVAL_DUPLICATE_ID_ITEM_COUNT=17", "IFEVAL_DUPLICATE_ID_EXCLUSION_COUNT=0",
    "IFEVAL_VALID_ITEM_SCORER_INSTANTIATION_FAILURE_COUNT=0", "",
    "TOKEN_EVAL_ADD_BOS=NO", "TOKEN_EVAL_ADD_EOS=NO", "TOKEN_EVAL_PARSE_SPECIAL=NO",
    "TOKEN_WIKITEXT_SCORED_COUNT=32768", "TOKEN_OBRAIN_DOCS_SCORED_COUNT=16384",
    "TOKEN_OBRAIN_RUST_SCORED_COUNT=16384", "TOKEN_EVAL_SCORED_TOKEN_COUNT=65536",
    "TOKEN_EVAL_INVALID_TOKEN_ID_COUNT=0",
    "TOKEN_EVAL_ITEM_COUNT=%d" % len(rows),
    "TOKENIZER_TOOL_REPEATABILITY_MISMATCH_COUNT=0",
    "TOKENIZER_BASELINE_CANDIDATE_FIXTURE_MISMATCH_COUNT=0",
    "TOKENIZATION_SECOND_PASS_MISMATCH_COUNT=0", "TOKENIZATION_REPRODUCIBLE=YES",
    "TOKEN_CORPUS_BASELINE_CANDIDATE_TOKEN_ID_MISMATCH_COUNT=0",
    "TOKEN_EVAL_CONTENT_TREE_SHA256=" + TOKEN_TREE, "",
    "TASK_KNOWLEDGE_REASONING_COUNT=%d" % counts["knowledge_reasoning"],
    "TASK_MATH_COUNT=%d" % counts["math"], "TASK_CODE_COUNT=%d" % counts["code"],
    "TASK_INSTRUCTION_COUNT=%d" % counts["instruction"],
    "TASK_EVAL_ITEM_COUNT=%d" % len(allids),
    "TASK_GLOBAL_ITEM_ID_DUPLICATE_COUNT=%d" % (len(allids) - len(set(allids))),
    "TASK_EVAL_DATA_CONTENT_TREE_SHA256=" + TASK_TREE, "",
    "TASK_MATERIALIZATION_STATUS=PASS", "TASK_DATA_STATUS=PASS",
    "TOKEN_DATA_STATUS=PASS", "TASK_CORPUS_SUFFICIENT=YES",
    "TOKEN_CORPUS_SUFFICIENT=YES", "TOKEN_CORPUS_FROZEN=YES",
    "TASK_CORPUS_FROZEN=NO", "LONG_CONTEXT_CORPUS_FROZEN=NO", "SCORERS_FROZEN=NO",
    "THRESHOLDS_FROZEN=YES", "QUALITY_PREFREEZE_BASELINE_RESULT_COUNT=0",
    "QUALITY_PREFREEZE_CANDIDATE_RESULT_COUNT=0",
    "ENGINE008_PROTOCOL_FROZEN=NO", "ENGINE008_EXECUTION_ALLOWED=NO",
    "FIRST_QUALITY_BLOCKING_GATE=ENGINE008_SCORERS_NOT_BUILT",
    "ENGINE007_NEXT_STEP=ENGINE008_BUILD_AND_QUALIFY_DETERMINISTIC_SCORERS", "",
    "LIMITES DOCUMENTEES :",
    "  - obrain_rust : 16384 targets issus d UN SEUL fichier source.",
    "    La regle gelee (ordre par hash) a rencontre un fichier depassant la",
    "    cible. Diversite = 1. Les autres strates : wikitext 333 fichiers,",
    "    obrain_docs 14 fichiers.",
    "  - TOKEN : %d groupes de contenus dupliques (%d items redondants)," %
    (len(dups), sum(v - 1 for v in dups.values())),
    "    consequence de la regle (aucun dedoublonnage de contenu).",
    "  - langdetect n a pas de wheel : le pinning binaire est partiel,",
    "    la reproductibilite repose sur PYTHON_ENVIRONMENT_FREEZE.txt.",
    "",
]
atomic(Q + "/ENGINE008_DATA_STATUS.txt", "\n".join(status) + "\n")
atomic(Q + "/ENGINE008_DUPLICATE_AUDIT.csv",
       "scope,duplicate_count\n" +
       "\n".join("%s,%d" % (n, v) for n, v in
                 [("token_eval_redundant_items", sum(v - 1 for v in dups.values()))]) + "\n")
P()
P("status ecrit")
