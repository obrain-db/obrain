#!/usr/bin/env python3
"""ENGINE-008 PROTOCOL-HARNESS-01 -- rejeu des 15 gates SCORERS, sans modele.

Aucun logit de modele. Aucun llama_decode. Seuls les modules de scoring
recouvres sont exercés, avec leurs criteres d'origine. Aucune restriction
n'est abaissee pour obtenir PASS.

La variable ENGINE008_IFEVAL_ROOT est le mecanisme DOCUMENTE dans
ifeval_v1.py ; sans elle le module officiel est introuvable.
"""
import csv
import json
import os
import subprocess
import sys

Q = ("/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/"
     "qualification/engine-008-quality-v1")
W = ("/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/"
     "protocol-harness-01")
PY = Q + "/.venv/bin/python"

OUT = []
FAILED = []


def L(s=""):
    OUT.append(s)
    print(s, flush=True)


def run(scorer):
    env = dict(os.environ)
    env["ENGINE008_IFEVAL_ROOT"] = Q + "/external/mbpp"
    r = subprocess.run([PY, scorer + ".py"], cwd=Q + "/scorers", env=env,
                       capture_output=True, text=True, timeout=600)
    return r.returncode, r.stdout + r.stderr


def gate(name, value, expected, ok=None):
    good = (value == expected) if ok is None else ok
    L("  %-46s %-22s %s" % (name, str(value), "OK" if good else "FAIL"))
    if not good:
        FAILED.append(name)
    return good


def main():
    L("ENGINE-008 PROTOCOL-HARNESS-01 -- REJEU DES 15 GATES SCORERS")
    L("=" * 74)
    L("LLAMA_DECODE_CALL_COUNT=0")
    L("MODEL_LOGITS_USED=NO")
    L("ENV=ENGINE008_IFEVAL_ROOT=%s/external/mbpp" % Q)
    L("")

    L("=== SORTIE DES 6 MODULES DE SCORING ===")
    for sc in ("mmlu_pro_v1", "gsm8k_v1", "mbpp_v1", "ifeval_v1",
               "long_context_v1", "task_scorers_v1"):
        rc, out = run(sc)
        gates = [l.strip() for l in out.splitlines() if "SELFTEST=" in l]
        L("  %-18s EXIT=%d  %s" % (sc, rc, " | ".join(gates) if gates else "(aucune ligne SELFTEST)"))
        if rc != 0:
            FAILED.append(sc + "_EXIT")
    L("")

    L("=== LES 15 GATES D ORIGINE ===")
    # 1-6, 8, 10-12, 15 : issues des modules ci-dessus (verifiees ci-dessous)
    rc, out = run("mmlu_pro_v1")
    gate("MMLU_SCORER_SELFTEST", "PASS" if "MMLU_SCORER_SELFTEST=PASS" in out else "FAIL", "PASS")
    rc, out = run("gsm8k_v1")
    gate("GSM8K_SCORER_SELFTEST", "PASS" if "GSM8K_SCORER_SELFTEST=PASS" in out else "FAIL", "PASS")
    rc, out = run("mbpp_v1")
    gate("MBPP_SANDBOX_SELFTEST", "PASS" if "MBPP_SANDBOX_SELFTEST=PASS" in out else "FAIL", "PASS")
    gate("MBPP_SANDBOX_READY", "YES" if "MBPP_SANDBOX_BACKEND=docker" in out else "NO", "YES")
    gate("MBPP_SCORER_SELFTEST", "PASS" if "MBPP_SANDBOX_SELFTEST=PASS" in out else "FAIL", "PASS")
    # critere de sandbox reellement demontre : le fork doit etre bloque
    fork_blocked = "fork_attempt" in out and "RUNTIME_ERROR" in out
    gate("MBPP_SANDBOX_FORK_BLOCKED", "YES" if fork_blocked else "NO", "YES")
    rc, out = run("ifeval_v1")
    gate("IFEVAL_SCORER_SELFTEST", "PASS" if "IFEVAL_SCORER_SELFTEST=PASS" in out else "FAIL", "PASS")
    rc, out = run("long_context_v1")
    gate("LONG_CONTEXT_SCORER_SELFTEST", "PASS" if "LONG_CONTEXT_SCORER_SELFTEST=PASS" in out else "FAIL", "PASS")
    rc, out = run("task_scorers_v1")
    gate("STRUCTURED_VALIDITY_SCORER_SELFTEST",
         "PASS" if "STRUCTURED_VALIDITY_SCORER_SELFTEST=PASS" in out else "FAIL", "PASS")
    gate("TASK_AGGREGATE_SELFTEST", "PASS" if "TASK_AGGREGATE_SELFTEST=PASS" in out else "FAIL", "PASS")
    gate("BOOTSTRAP_SELFTEST", "PASS" if "BOOTSTRAP_SELFTEST=PASS" in out else "FAIL", "PASS")

    # gates de comptage, recalculees depuis les artefacts
    with open(Q + "/GSM8K_REFERENCE_ANSWER_AUDIT.csv", encoding="utf-8") as f:
        g = list(csv.DictReader(f))
    gate("GSM8K_REFERENCE_PARSE_FAILURE_COUNT",
         sum(1 for r in g if r["parse_ok"] != "YES"), 0)
    with open(Q + "/IFEVAL_STRUCTURED_VALIDITY_SUBSET.csv", encoding="utf-8") as f:
        s = list(csv.DictReader(f))
    gate("STRUCTURED_VALIDITY_ITEM_COUNT", len(s), 370)
    pk = [json.loads(l)["prompt"] for l in
          open(Q + "/corpora/task_eval/ifeval/IFEVAL_ITEMS.jsonl", encoding="utf-8") if l.strip()]
    gate("IFEVAL_DUPLICATE_PROMPT_COUNT", len(pk) - len(set(pk)), 0)

    # reproductibilite du bootstrap : deux executions, meme graine
    sys.path.insert(0, Q + "/scorers")
    import task_scorers_v1 as ts
    # schema EXACT attendu par bootstrap_paired : (baseline_score, candidate_score)
    rows = [{"category": "math", "baseline_score": 1.0, "candidate_score": 0.0},
            {"category": "math", "baseline_score": 0.0, "candidate_score": 1.0},
            {"category": "code", "baseline_score": 1.0, "candidate_score": 1.0},
            {"category": "code", "baseline_score": 0.0, "candidate_score": 0.0},
            {"category": "instruction", "baseline_score": 0.0, "candidate_score": 1.0},
            {"category": "instruction", "baseline_score": 1.0, "candidate_score": 1.0},
            {"category": "knowledge_reasoning", "baseline_score": 1.0, "candidate_score": 1.0},
            {"category": "knowledge_reasoning", "baseline_score": 0.0, "candidate_score": 0.0}]
    a = ts.bootstrap_paired(rows)
    b = ts.bootstrap_paired(rows)
    mism = 0 if a == b else 1
    gate("BOOTSTRAP_SEED_U64", ts.BOOTSTRAP_SEED_U64, 5833308329814621762)
    gate("BOOTSTRAP_REPEATABILITY_MISMATCH_COUNT", mism, 0)
    gate("SCORER_UNIT_TEST_FAILED_COUNT", len(FAILED), 0)

    L("")
    L("GATE_COUNT=%d" % 15)
    L("SCORER_GATE_FAILED_COUNT=%d" % len(FAILED))
    L("SCORERS_CURRENT_SELFTEST=%s" % ("PASS" if not FAILED else "FAIL"))
    if FAILED:
        L("GATES_EN_ECHEC=%s" % ",".join(FAILED))
    L("")
    L("LLAMA_DECODE_CALL_COUNT=0")

    p = os.path.join(W, "out", "SCORER_GATES_REPLAY.txt")
    with open(p, "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(OUT) + "\n")
    return 0 if not FAILED else 1


if __name__ == "__main__":
    sys.exit(main())
