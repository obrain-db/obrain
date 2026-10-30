#!/usr/bin/env python3
"""ENGINE-008 PROTOCOL-HARNESS-01 -- statut terminal + checkpoint."""
import hashlib
import json
import os
import subprocess
import sys

W = ("/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/"
     "protocol-harness-01")
Q = ("/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/"
     "qualification/engine-008-quality-v1")
FORK = "/home/triviere/projects/llama-obrain-cuda-f1a"
OUT, DOCS = W + "/out", W + "/docs"


def sha(p):
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""):
            h.update(b)
    return h.hexdigest()


fz = json.load(open(OUT + "/ENGINE008_FREEZE_MANIFEST_v1.json", encoding="utf-8"))
render = open(OUT + "/RENDER_PASS1.txt", encoding="utf-8").read()
gates = open(OUT + "/SCORER_GATES_REPLAY.txt", encoding="utf-8").read()


def g(k):
    for l in render.splitlines():
        if l.startswith(k + "="):
            return l.split("=", 1)[1].strip()
    return "NOT_MEASURED"


def gg(k):
    for l in gates.splitlines():
        if l.strip().startswith(k + "="):
            return l.split("=", 1)[1].strip()
    return "NOT_MEASURED"


FROZEN = fz["all_conditions_met"]
S = f"""ENGINE-008 -- STATUT TERMINAL PROTOCOL-HARNESS-01
==========================================================================
Valeurs MESUREES. NOT_RUN / NOT_AVAILABLE / NOT_CALCULABLE lorsqu'etablies.

AGENT_TOKEN_BUDGET=UNLIMITED
ENGINE008_PASS=PROTOCOL_HARNESS_ONLY
ENGINE008_COMPARISON_OBJECTIVE=CONTROLLED_IDENTICAL_INPUTS
CHAT_TEMPLATE_AMENDMENT_STATUS=ADOPTED_COMMON_BASELINE
EMBEDDED_CHAT_TEMPLATE_MATCH=NO
ENGINE008_EFFECTIVE_CHAT_TEMPLATE_SHA256=c3cf9e34abf4f9e36c2d72165aa9c132d3e2a725b6c2586aaa3a8af9d7a81041
EFFECTIVE_TEMPLATE_BASELINE_CANDIDATE_MATCH=YES
Q1_QUALITY_LIBLLAMA_SHA_MATCH=YES
QUALITY_EVAL_LIBLLAMA_SHA256=87c42d5f5dd7ab3c10967b4d3eaac82636b6af11e2c18a7f2798bab2d01bb2b2
NATIVE_JINJA_RENDERER_QUALIFIED=YES
RENDERER_PROVENANCE_SHA256={sha(DOCS + '/ENGINE008_RENDERER_PROVENANCE_v1.json')}
QUALITY_EVAL_HARNESS_SOURCE_SHA256={sha(FORK + '/tools/engine008-quality-eval/engine008-quality-eval.cpp')}
QUALITY_EVAL_HARNESS_BINARY_SHA256={sha(W + '/build/engine008-quality-eval')}
PRODUCT_COMPUTE_SOURCE_DIFF_COUNT=0
  perimetre=src/, ggml/, include/, common/ ; reference=etat qualifie Q1 tel que trouve
  mesure=0 fichier modifie depuis le debut de la passe dans les 4 repertoires
HARNESS_SELFTEST=PASS
HARNESS_COMPUTE_GUARD_SELFTEST=PASS
HARNESS_PREFREEZE_DECODE_CALL_COUNT=0
HARNESS_PREFREEZE_MODEL_FORWARD_CALL_COUNT=0
TASK_CHAT_PARSE_SPECIAL=YES
TASK_RENDERED_INPUT_COUNT={g('TASK_RENDERED_INPUT_COUNT')}
TASK_RENDERED_TEXT_BASELINE_CANDIDATE_MISMATCH_COUNT={g('TASK_RENDERED_TEXT_BASELINE_CANDIDATE_MISMATCH_COUNT')}
TASK_INPUT_TOKEN_ID_BASELINE_CANDIDATE_MISMATCH_COUNT={g('TASK_INPUT_TOKEN_ID_BASELINE_CANDIDATE_MISMATCH_COUNT')}
TASK_INVALID_TOKEN_ID_COUNT={g('TASK_INVALID_TOKEN_ID_COUNT')}
TASK_RENDER_SECOND_PASS_TEXT_MISMATCH_COUNT=0
  mesure=les trois artefacts sont IDENTIQUES octet pour octet entre passe 1 et passe 2
TASK_RENDER_SECOND_PASS_TOKEN_MISMATCH_COUNT=0
  mesure=idem (egalite de fichier entier, plus forte qu'un decompte)
TASK_RENDER_REPRODUCIBLE=YES
MMLU_PREFIX_EXTENSION_CHECK_COUNT={g('MMLU_PREFIX_EXTENSION_CHECK_COUNT')}
MMLU_PREFIX_EXTENSION_NONPREFIX_COUNT={g('MMLU_PREFIX_EXTENSION_NONPREFIX_COUNT')}
MMLU_CANDIDATE_SUFFIX_TOKEN_MISMATCH_COUNT={g('MMLU_CANDIDATE_SUFFIX_TOKEN_MISMATCH_COUNT')}
TASK_EVAL_DATA_CONTENT_TREE_SHA256=c30d71d84712fae390be582111f0473c87ccaaad7411e1ef99b97689d69ab84a
TASK_EVAL_CONTENT_TREE_SHA256={fz['authorities']['task_eval_content_tree_sha256']}
TASK_GLOBAL_ITEM_ORDER_SHA256={sha(OUT + '/TASK_GLOBAL_ITEM_ORDER.txt')}
TOKEN_EVAL_CONTENT_TREE_V2_SHA256=0ce4512e9d73b5e1cc3beddd2222e9ad7a96592508f10930515ef8b4276cf5dd
TASK_PROMPT_CONTRACTS_V1_SHA256=fa30f16d95a564491e6d02666350ba783d6f44493ea5d04cef024e92ef0c3bc6
SCORERS_CURRENT_AUTHORITY_VERIFIED=YES
SCORERS_CURRENT_SELFTEST={gg('SCORERS_CURRENT_SELFTEST')}
  15 gates sur 15 ; SCORER_GATE_FAILED_COUNT={gg('SCORER_GATE_FAILED_COUNT')}
THRESHOLDS_CURRENT_AUTHORITY_VERIFIED=YES
GENERATE_STOP_TOKEN_IDS=[248046]
  EOS 248046 = <|im_end|> sur LES DEUX modeles ; c'est le token de cloture de
  chaque bloc de message du template, branche assistant incluse. Audit STATIQUE
  (identite de token + usage dans le template) ; confirmation runtime a faire.
  <|im_start|>=248045 ; <|endoftext|>=248044 (= token BOS).
GENERATION_LIMITS_FROZEN=YES
  GSM8K 1024 / MBPP 1024 / IFEval 4096 (repris de TASK_PROMPT_CONTRACTS_v1)
ENGINE008_STANDARD_DECODER_EXPECTED=YES
ENGINE008_MTP_RUNTIME_PROOF_METHOD_DEFINED=YES
ENGINE008_MTP_RUNTIME_PROOF_STATUS=NOT_EXERCISED_NO_FORWARD
ENGINE008_MTP_RUNTIME_CONFIRMED=NO
ENGINE008_BASELINE_RUNTIME_ROTATION_EXPECTED=NO
ENGINE008_CANDIDATE_RUNTIME_ROTATION_EXPECTED=YES
ENGINE008_CANDIDATE_WHT_RUNTIME_PROOF_METHOD_DEFINED=YES
ENGINE008_WHT_RUNTIME_PROOF_STATUS=NOT_EXERCISED_NO_FORWARD
ENGINE008_CANDIDATE_WHT_RUNTIME_CONFIRMED=NO
QUALITY_PREFREEZE_BASELINE_RESULT_COUNT=0
QUALITY_PREFREEZE_CANDIDATE_RESULT_COUNT=0
ENGINE008_PROTOCOL_SHA256={fz['protocol']['sha256']}
ENGINE008_PROTOCOL_FROZEN={'YES' if FROZEN else 'NO'}
QUALITY_EVAL_HARNESS_READY={'YES' if FROZEN else 'NO'}
ENGINE008_EXECUTION_ALLOWED=NO
FIRST_BLOCKING_GATE={'NONE' if FROZEN else 'A_DETERMINER'}
REMAINING_BLOCKERS=
  R1_preuves_runtime_non_exercees (par construction : aucun forward pass autorise)
     MTP et WHT restent METHOD_DEFINED / STATICALLY_SUPPORTED, jamais RUNTIME_CONFIRMED.
  R2_libllama-common_non_qualifiee : sha256 a87c31d2e68ebadf3c21b4718b7bf58c34cb97c9a5b1145a7fa30df82f1a4b20
     CONSTATE, sans autorite ecrite prealable. Le renderer repose sur ce composant.
  R3_clause_aucun_system_sans_autorite_materialisee : le brief exige
     SYSTEM_MESSAGE_PRESENT=NO ; aucun texte autoritaire ne le materialise. Le
     template effectif EMET neanmoins un bloc system (reasoning_instructions).
     Clause conservee comme clause de brief, comportement DECRIT non decrete.
  R4_corpus_long_contexte_non_construit : hors perimetre, non rouvert.
ENGINE008_NEXT_STEP=EXECUTER_LE_PREFLIGHT_RUNTIME_POST_FREEZE (MTP + WHT + contrat d arret)
  PUIS l evaluation qualite, sur instruction explicite separee.

--- ARRET ---
Arret a la fin de PROTOCOL-HARNESS-01. Aucun calcul modele : aucun llama_decode,
aucun llama_encode, aucun prefill, aucun forward pass, aucun logit, aucune
generation, aucune NLL/PPL, aucun benchmark qualite ni long contexte.
Aucun enchainement automatique vers l evaluation.
"""

p = OUT + "/ENGINE008_PROTOCOL_HARNESS_STATUS.txt"
with open(p, "w", encoding="utf-8", newline="\n") as f:
    f.write(S)

# ---------------------------------------------------------------- checkpoint
ck = []
for root in (DOCS, OUT, W + "/authority", W + "/build"):
    for fn in sorted(os.listdir(root)):
        fp = os.path.join(root, fn)
        if not os.path.isfile(fp):
            continue
        if fn.endswith((".tmp",)):
            continue
        if not fn.endswith((".md", ".json", ".txt", ".py", ".cpp", ".jinja")):
            continue
        if fn in ("TASK_RENDERED_INPUTS.jsonl", "MMLU_CHOICES_INPUTS.jsonl",
                  "TASK_RENDERED_INPUTS_PASS2.jsonl", "MMLU_CHOICES_INPUTS_PASS2.jsonl"):
            continue  # volumineux : conserves sur place, hashes au manifeste
        ck.append({"path": os.path.relpath(fp, W), "bytes": os.path.getsize(fp),
                   "sha256": sha(fp)})
ck.sort(key=lambda e: e["path"])
blob = json.dumps({"checkpoint": "ENGINE008_PROTOCOL_HARNESS_CHECKPOINT_v1",
                   "root": W, "host": "spark-c578", "file_count": len(ck),
                   "files": ck}, indent=1, sort_keys=True, ensure_ascii=False)
with open(OUT + "/ENGINE008_PROTOCOL_HARNESS_CHECKPOINT_v1.json", "w",
          encoding="utf-8", newline="\n") as f:
    f.write(blob)

print("ENGINE008_PROTOCOL_FROZEN=%s" % ("YES" if FROZEN else "NO"))
print("QUALITY_EVAL_HARNESS_READY=%s" % ("YES" if FROZEN else "NO"))
print("ENGINE008_PROTOCOL_SHA256=%s" % fz["protocol"]["sha256"])
print("CHECKPOINT_FILES=%d" % len(ck))
print("STATUS=%s" % p)
sys.exit(0 if FROZEN else 1)
