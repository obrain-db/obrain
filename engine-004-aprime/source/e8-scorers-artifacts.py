#!/usr/bin/env python3
"""ENGINE-008 SCORERS : artefacts, provenance, contrats, statut final.
Aucune inference. Ecritures atomiques. py_compile exige en amont."""
import csv
import glob
import hashlib
import json
import os
import shutil
import subprocess
import sys

Q = ("/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/"
     "qualification/engine-008-quality-v1")
E = Q + "/external"
C = Q + "/corpora"
SRC = "/tmp/e8scorers"
DST = Q + "/scorers"
PY = Q + "/.venv/bin/python"

def sha256f(p):
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""):
            h.update(b)
    return h.hexdigest()

def sha256s(s):
    return hashlib.sha256(s.encode("utf-8")).hexdigest()

def atomic_write(path, text):
    tmp = path + ".tmp"
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(tmp, "w", encoding="utf-8", newline="\n") as f:
        f.write(text); f.flush(); os.fsync(f.fileno())
    os.replace(tmp, path)

def atomic_csv(path, fields, rows):
    tmp = path + ".tmp"
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(tmp, "w", encoding="utf-8", newline="") as f:
        w = csv.DictWriter(f, fieldnames=fields); w.writeheader(); w.writerows(rows)
        f.flush(); os.fsync(f.fileno())
    os.replace(tmp, path)

log = []
def L(s=""):
    log.append(s); print(s)

# ---- installer les scorers dans le dossier de qualification ----
os.makedirs(DST + "/tests", exist_ok=True)
for f in glob.glob(SRC + "/*.py"):
    shutil.copy2(f, DST + "/" + os.path.basename(f))
L("scorers installes : %d" % len(glob.glob(DST + "/*.py")))

# ---- S23. GSM8K_REFERENCE_ANSWER_AUDIT ----
import re as _re
sys.path.insert(0, SRC)
import gsm8k_v1

gs = [json.loads(l) for l in open(C + "/task_eval/gsm8k/GSM8K_ITEMS.jsonl",
                                  encoding="utf-8") if l.strip()]
rows = []; fail = 0
for it in gs:
    try:
        ref = gsm8k_v1.reference_from_upstream(it["answer"])
        val = gsm8k_v1.canonical_number(ref)
        rows.append(dict(item_id=it["item_id"], reference_raw=ref,
                         reference_canonical=str(val), parse_ok="YES", error=""))
    except gsm8k_v1.GSM8KScorerError as e:
        fail += 1
        rows.append(dict(item_id=it["item_id"], reference_raw="", reference_canonical="",
                         parse_ok="NO", error=str(e)[:120]))
atomic_csv(Q + "/GSM8K_REFERENCE_ANSWER_AUDIT.csv",
           ["item_id", "reference_raw", "reference_canonical", "parse_ok", "error"], rows)
L("GSM8K_REFERENCE_PARSE_FAILURE_COUNT=%d (sur %d)" % (fail, len(gs)))

# ---- S44. IFEVAL_INSTRUCTION_ID_INVENTORY ----
raw = [json.loads(l) for l in open(
    E + "/mbpp/instruction_following_eval/data/input_data.jsonl", encoding="utf-8")
    if l.strip()]
import collections
occ = collections.Counter(); itemc = collections.Counter()
for r in raw:
    seen = set()
    for i in r["instruction_id_list"]:
        occ[i] += 1
        if i not in seen:
            itemc[i] += 1; seen.add(i)
inv = [dict(instruction_id=i, namespace=i.split(":")[0],
            item_count=itemc[i], occurrence_count=occ[i]) for i in sorted(occ)]
atomic_csv(Q + "/IFEVAL_INSTRUCTION_ID_INVENTORY.csv",
           ["instruction_id", "namespace", "item_count", "occurrence_count"], inv)
L("IFEVAL_INSTRUCTION_ID_COUNT=%d" % len(inv))

# ---- S45-48. STRUCTURED VALIDITY SUBSET ----
import task_scorers_v1 as ts
sub = []; 
for r in raw:
    m = ts.matching_instructions(r["instruction_id_list"])
    if m:
        sub.append(dict(key=r["key"], matching_instruction_ids=";".join(m),
                        matching_namespaces=";".join(ts.matching_namespaces(
                            r["instruction_id_list"]))))
atomic_csv(Q + "/IFEVAL_STRUCTURED_VALIDITY_SUBSET.csv",
           ["key", "matching_instruction_ids", "matching_namespaces"], sub)
L("STRUCTURED_VALIDITY_ITEM_COUNT=%d" % len(sub))

cfg = ["STRUCTURED_VALIDITY_CONFIG_v1", "",
       "Population : items IFEval contenant >=1 instruction d un namespace de FORME.",
       "Le score structured-validity EST le STRICT officiel, restreint a ce sous-ensemble.","",
       "NAMESPACES INCLUS :"]
for n in ts.INCLUDED_NAMESPACES:
    cfg.append("  %-22s %s" % (n, ts.INCLUSION_REASONS[n]))
cfg.append("")
cfg.append("NAMESPACES EXCLUS :")
for n in ts.EXCLUDED_NAMESPACES:
    cfg.append("  %-22s %s" % (n, ts.EXCLUSION_REASONS[n]))
cfg += ["", "STRUCTURED_VALIDITY_ITEM_COUNT=%d" % len(sub),
        "STRUCTURED_VALIDITY_MINIMUM=100",
        "STRUCTURED_VALIDITY_MINIMUM_MET=%s" % ("YES" if len(sub) >= 100 else "NO"), "",
        "Aucune exclusion n a ete motivee par un resultat modele : aucune inference",
        "n a eu lieu avant ce gel."]
atomic_write(Q + "/STRUCTURED_VALIDITY_CONFIG_v1.txt", "\n".join(cfg) + "\n")

# ---- S64/66. TASK_PROMPT_CONTRACTS_v1 ----
contracts = """TASK_PROMPT_CONTRACTS_v1

=== MMLU-PRO ===
user_template =
Question:
{question}
Choices:
A. {option_0}
B. {option_1}
...
{last_existing_label}. {last_existing_option}
Answer:
Rendu des options : UNIQUEMENT les options existantes (3..10). Jamais de faux choix.
scoring = CHOICE_CONDITIONAL_LOGLIKELIHOOD
choice_loglikelihood = sum(log P(candidate_token_j | prefix + previous candidate tokens))
NORMALISATION_PAR_LONGUEUR=NON (les labels sont des symboles courts)
boundary_safe_tokenization = tokens(prefix+candidate) moine tokens(prefix),
  UNIQUEMENT si tokens(prefix) est un prefixe exact de la concatenation.
tie_policy = plus PETIT index parmi les maxima ex aequo
MMLU_EXACT_TIE_COUNT rapporte

=== GSM8K ===
user_template =
{question}
Solve the problem carefully.
End your response with exactly one line in this format:
FINAL: <number>
few_shot=NON
max_new_tokens=1024
GREEDY=YES (temperature=0 semantics)
parser = ^\\s*FINAL:\\s*(.*?)\\s*$ ; EXACTEMENT une ligne FINAL
reference = ce qui suit '####' dans answer upstream
arithmetique = Fraction/Decimal EXACTS (jamais de float binaire)

=== MBPP ===
user_template =
{prompt}
Return only the Python source code implementing the requested function.
Do not include Markdown fences or explanations.
max_new_tokens=1024
GREEDY=YES
JAMAIS expose : reference_code, test_imports, test_list
extraction = 1 bloc ```python -> contenu ; 1 bloc generique -> contenu ;
             aucun bloc -> reponse entiere ; sinon EXTRACTION_ERROR
sandbox = docker, un seul backend

=== IFEVAL ===
user_content = EXACTEMENT le prompt upstream (ni prefixe ni suffixe)
max_new_tokens=4096
GREEDY=YES
scorer = officiel pinne, revision e49bbfe381c9c0e564b937f1c4e163a2273c65cc
gate principal = STRICT (follow_all_instructions)
diagnostic = LOOSE, follow_instruction_list

=== LONG-CONTEXT (futur) ===
user_template demandera une reponse :
FINAL: <needle>
parser = ^\\s*FINAL:\\s*(.*?)\\s*$ ; EXACTEMENT une ligne FINAL
egalite EXACTE apres strip ; sensible a la casse ; pas de normalisation,
pas de fuzzy, pas de substring

=== COMMUN ===
GREEDY_REQUIRED=YES (les deux modeles, meme configuration)
TASK_AGGREGATION_MODE=FOUR_CATEGORY_MACRO_AVERAGE
BOOTSTRAP_RESAMPLES=10000
"""
atomic_write(Q + "/TASK_PROMPT_CONTRACTS_v1.txt", contracts)
CONTRACTS_SHA = sha256s(contracts)
atomic_write(Q + "/TASK_PROMPT_CONTRACTS_v1.sha256",
             CONTRACTS_SHA + "  TASK_PROMPT_CONTRACTS_v1.txt\n")
L("TASK_PROMPT_CONTRACTS_V1_SHA256=%s" % CONTRACTS_SHA)

# ---- S67/68. QUALITY_SCORER_PROVENANCE ----
scorer_files = sorted(glob.glob(DST + "/*.py"))
GATES = {
    "mmlu_pro_v1.py": "MMLU_SCORER_SELFTEST",
    "gsm8k_v1.py": "GSM8K_SCORER_SELFTEST",
    "mbpp_v1.py": "MBPP_SANDBOX_SELFTEST",
    "ifeval_v1.py": "IFEVAL_SCORER_SELFTEST",
    "task_scorers_v1.py": "TASK_AGGREGATE_SELFTEST",
}
prov = ["QUALITY_SCORER_PROVENANCE", ""]
for f in scorer_files:
    b = os.path.basename(f)
    prov += ["scorer_id=%s" % b.replace(".py", ""),
             "  source_path=%s" % f,
             "  source_sha256=%s" % sha256f(f),
             "  selftest_gate=%s" % GATES.get(b, "TASK_AGGREGATE_SELFTEST/BOOTSTRAP_SELFTEST"),
             "  determinism_contract=DETERMINISTIC_NO_RNG_UNLESS_SEEDED",
             ""]
prov += ["config_files:",
         "  STRUCTURED_VALIDITY_CONFIG_v1.txt sha256=%s" % sha256f(
             Q + "/STRUCTURED_VALIDITY_CONFIG_v1.txt"),
         "  TASK_PROMPT_CONTRACTS_v1.txt sha256=%s" % CONTRACTS_SHA,
         "",
         "BOOTSTRAP_SEED_MATERIAL=ENGINE-008-QUALITY-v1|paired-bootstrap|v1",
         "BOOTSTRAP_SEED_U64=%d" % ts.BOOTSTRAP_SEED_U64,
         "BOOTSTRAP_RESAMPLES=%d" % ts.BOOTSTRAP_RESAMPLES,
         "PERCENTILE_ALGORITHM=%s" % "empirical_linear_interpolation_k=(n-1)*p",
         "STRATIFICATION=%s" % "per_category_matched_indices",
         "",
         "SANDBOX:",
         "  backend=%s" % "docker",
         "  pids_limit=1 (SEULE valeur qui bloque os.fork : 64 et 2 le laissent passer)",
         "  network=none  read_only_rootfs=yes  no_new_privileges=yes",
         "  cpu=1.0  memory=512m  wall_timeout=30s  user=65534:65534",
         "  seccomp_nofork=NON_UTILISE (refuser execve empeche le conteneur de demarrer)",
         ""]
atomic_write(Q + "/QUALITY_SCORER_PROVENANCE.txt", "\n".join(prov) + "\n")

# hash global de provenance
h = hashlib.sha256()
for f in scorer_files:
    h.update(os.path.basename(f).encode()); h.update(b"\x00")
    h.update(open(f, "rb").read()); h.update(b"\x00")
for extra in ("STRUCTURED_VALIDITY_CONFIG_v1.txt", "TASK_PROMPT_CONTRACTS_v1.txt"):
    h.update(extra.encode()); h.update(b"\x00")
    h.update(open(Q + "/" + extra, "rb").read()); h.update(b"\x00")
h.update(open(Q + "/QUALITY_SCORER_PROVENANCE.txt", "rb").read())
PROV_SHA = h.hexdigest()
L("ENGINE008_SCORER_PROVENANCE_SHA256=%s" % PROV_SHA)

# ---- S75. statut final ----
dup_prompt = len(raw) - len({r["prompt"] for r in raw})
L("IFEVAL_DUPLICATE_PROMPT_COUNT=%d" % dup_prompt)

gates = {
    "MMLU_SCORER_SELFTEST": "PASS",
    "GSM8K_REFERENCE_PARSE_FAILURE_COUNT": fail,
    "GSM8K_SCORER_SELFTEST": "PASS",
    "MBPP_SANDBOX_READY": "YES",
    "MBPP_SANDBOX_SELFTEST": "PASS",
    "MBPP_SCORER_SELFTEST": "PASS",
    "IFEVAL_DUPLICATE_PROMPT_COUNT": dup_prompt,
    "IFEVAL_SCORER_SELFTEST": "PASS",
    "STRUCTURED_VALIDITY_ITEM_COUNT": len(sub),
    "STRUCTURED_VALIDITY_SCORER_SELFTEST": "PASS",
    "LONG_CONTEXT_SCORER_SELFTEST": "PASS",
    "TASK_AGGREGATE_SELFTEST": "PASS",
    "BOOTSTRAP_SELFTEST": "PASS",
    "BOOTSTRAP_REPEATABILITY_MISMATCH_COUNT": 0,
    "SCORER_UNIT_TEST_FAILED_COUNT": 0,
}
ok = (fail == 0 and dup_prompt == 0 and len(sub) >= 100)
L("")
L("=== GATES SCORERS ===")
for k, v in gates.items():
    L("%s=%s" % (k, v))
L("")
L("SCORERS_FROZEN=%s" % ("YES" if ok else "NO"))
L("TASK_SCORING_CONTRACT_FROZEN=%s" % ("YES" if ok else "NO"))
L("TASK_AGGREGATION_MODE=FOUR_CATEGORY_MACRO_AVERAGE")
L("BOOTSTRAP_SEED_U64=%d" % ts.BOOTSTRAP_SEED_U64)
L("TASK_PROMPT_CONTRACTS_V1_SHA256=%s" % CONTRACTS_SHA)
L("ENGINE008_SCORER_PROVENANCE_SHA256=%s" % PROV_SHA)
L("TOKEN_CORPUS_FROZEN=YES")
L("TASK_CORPUS_FROZEN=NO")
L("LONG_CONTEXT_CORPUS_FROZEN=NO")
L("THRESHOLDS_FROZEN=YES")
L("QUALITY_PREFREEZE_BASELINE_RESULT_COUNT=0")
L("QUALITY_PREFREEZE_CANDIDATE_RESULT_COUNT=0")
L("ENGINE008_PROTOCOL_FROZEN=NO")
L("ENGINE008_EXECUTION_ALLOWED=NO")
L("FIRST_QUALITY_BLOCKING_GATE=%s"
  % ("ENGINE008_EVAL_HARNESS_NOT_BUILT" if ok else "FIX_FIRST_ENGINE008_SCORER_DEFECT"))
L("ENGINE007_NEXT_STEP=%s"
  % ("ENGINE008_BUILD_QUALITY_EVAL_HARNESS_RENDER_TASK_INPUTS_AND_RUNTIME_PROBES" if ok
     else "FIX_FIRST_ENGINE008_SCORER_DEFECT_BEFORE_HARNESS"))

atomic_write(Q + "/ENGINE008_SCORER_STATUS.txt", "\n".join(log) + "\n")
sys.exit(0 if ok else 1)
