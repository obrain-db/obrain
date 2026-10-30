#!/usr/bin/env python3
"""SMOKE3 : livrables de statut."""
import os

R = ("/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/"
     "qualification/e7b-f1a-q0-forward-v1/run2")
S = R + "/smoke3"

def w(d, name, lines):
    open(os.path.join(d, name), "w", encoding="utf-8", newline="\n").write("\n".join(lines) + "\n")
    print("  " + name)

w(R, "RUN2I_BUILD_PROVENANCE.txt", [
 "RUN2I_BUILD_PROVENANCE",
 "",
 "RUN2I_DECODE_STEP_SERIALIZATION_FIX.diff",
 "  SHA256=af9cf46b1872f651501657ff5df457db23e5aea020952bf51f5595a61bd9b182",
 "  FILES=1 (src/f1a-capture.cpp)  HUNKS=1  ADDED=7  REMOVED=2",
 "  CLASSIFICATION :",
 "    DECODE_STEP_SERIALIZATION_CANONICALIZATION = 1 hunk",
 "    SMOKE_VERDICT_GATE_FIX = hors binaire (script d audit, pas de diff C++)",
 "    OTHER = 0",
 "  UNCLASSIFIED_HUNK_COUNT=0",
 "",
 "TARGETED_COMPILE_CHECK=PASS  (fsyntax-only, 0 diagnostic)",
 "FULL_REBUILD=100% Built target llama  (RC=0, erreurs=0)",
 "",
 "RUN2I_LIBLLAMA_PATH=/home/triviere/projects/llama-obrain-cuda-f1a/build-cuda/bin/libllama.so",
 "RUN2I_LIBLLAMA_SHA256=87c42d5f5dd7ab3c10967b4d3eaac82636b6af11e2c18a7f2798bab2d01bb2b2",
 "F1A_EVAL_SOURCE_CHANGED=NO",
 "F1A_EVAL_REBUILT=NO",
 "F1A_EVAL_SHA256=79ba1c449864da8be4b9fa53313a179c2ddad96bac1e814d77f470a8f2e7b80a",
 "F1A_EVAL_LINKED_LIBLLAMA_PATH=/home/triviere/projects/llama-obrain-cuda-f1a/build-cuda/bin/libllama.so.0",
 "F1A_EVAL_LINKED_LIBLLAMA_SHA256=87c42d5f5dd7ab3c10967b4d3eaac82636b6af11e2c18a7f2798bab2d01bb2b2",
 "RUNTIME_LIBLLAMA_MATCHES_INTENDED_RUN2I_BUILD=YES",
 "INSTRUMENTATION_SYMBOLS_IN_RUNTIME_LIBLLAMA=7  (6 + f1a_event_io_failure_count)",
 "PRODUCT_COMPUTE_SOURCE_DIFF_COUNT=0",
 "",
 "f1a-eval n a PAS ete recompile : sa source est inchangee, et son linkage",
 "runtime a ete reverifie vers la nouvelle libllama (87c42d5f).",
])

w(S, "SMOKE3_STEP_AUDIT.txt", [
 "SMOKE3_STEP_AUDIT",
 "PREFILL_DECODE_STEP=-1        (serialise \"-1\", plus \"NA\")",
 "PREFILL_CLASSIFIED_AS_SCIENTIFIC_STEP=NO",
 "DECODE_BEGIN_COUNT=9   LOGITS_READ_COUNT=9   DECODE_END_COUNT=9",
 "SCIENTIFIC_DECODE_STEP_SET=0,1,2,3,4,5,6,7,8   (le -1 du prefill est EXCLU)",
 "UNKNOWN_SCIENTIFIC_DECODE_STEP_COUNT=0",
 "M1 = PASS",
])

w(S, "SMOKE3_SUBMISSION_AUDIT.txt", [
 "SMOKE3_SUBMISSION_AUDIT",
 "TOTAL_COMPUTE_SUBMIT_COUNT=10      <- attendu, PAS 9",
 "PREFILL_COMPUTE_SUBMIT_COUNT=1     (submission 1, decode_step=-1)",
 "SCIENTIFIC_COMPUTE_SUBMIT_COUNT=9  (submissions 2..10, steps 0..8)",
 "COMPUTE_SUBMISSION_ID_DUPLICATE_OCCURRENCE_COUNT=0",
 "SCIENTIFIC_SUBMISSION_STEP_SET=0,1,2,3,4,5,6,7,8",
 "M2 = PASS",
])

w(S, "SMOKE3_MANIFEST_SCHEMA_AUDIT.txt", [
 "SMOKE3_MANIFEST_SCHEMA_AUDIT",
 "SMOKE3_MANIFEST_SCHEMA=PASS",
 "24 colonnes, toutes les lignes conformes, toutes les epochs presentes (0 ET 1).",
 "Le piege append multi-epoch est re-teste, pas suppose.",
])

w(S, "SMOKE3_PAYLOAD_PROVENANCE_SCHEMA_AUDIT.txt", [
 "SMOKE3_PAYLOAD_PROVENANCE_SCHEMA_AUDIT",
 "SMOKE3_PAYLOAD_PROVENANCE_SCHEMA=PASS",
 "18 colonnes, toutes les lignes conformes.",
])

w(S, "SMOKE3_CROSSCHECK_AUDIT.txt", [
 "SMOKE3_CROSSCHECK_AUDIT",
 "",
 "JOINTURE DIRECTE, SANS NORMALISATION.",
 "",
 "DECODE_STEP_MINUS_ONE_LEDGER_REPRESENTATION=-1",
 "DECODE_STEP_MINUS_ONE_MANIFEST_REPRESENTATION=-1",
 "DECODE_STEP_MINUS_ONE_PROVENANCE_REPRESENTATION=-1",
 "DECODE_STEP_SERIALIZATION_MISMATCH_COUNT=0",
 "DECODE_STEP_MINUS_ONE_SERIALIZED_AS_NA_COUNT=0",
 "",
 "SMOKE3_LEDGER_MANIFEST_MISMATCH_COUNT=0",
 "SMOKE3_LEDGER_PAYLOAD_PROVENANCE_MISMATCH_COUNT=0",
 "",
 "Les trois artefacts partagent desormais la MEME representation : la jointure",
 "sur (run_id, decode_step, compute_submission_id, capture_epoch_id) est",
 "possible directement. Compare SMOKE2 : MISMATCH=2 des deux cotes.",
])

w(S, "SMOKE3_VERDICT_NEGATIVE_CONTROL.txt", [
 "SMOKE3_VERDICT_NEGATIVE_CONTROL",
 "",
 "Le script de verdict est lui-meme qualifie : ses gates sont une LISTE",
 "EXPLICITE (SMOKE3_REQUIRED_GATES), et un gate declare mais non calcule est",
 "compte UNEVALUATED -> FAIL. Un compteur calcule mais oublie du verdict",
 "devient donc detectable : c est exactement le defaut qui avait fait afficher",
 "PASS a tort en SMOKE2.",
 "",
 "Injection sur copies synthetiques, AUCUN CSV reel modifie :",
 "  SMOKE3_LEDGER_MANIFEST_MISMATCH_COUNT force a 1              -> FAIL",
 "  SMOKE3_LEDGER_PAYLOAD_PROVENANCE_MISMATCH_COUNT force a 1    -> FAIL",
 "",
 "SMOKE_VERDICT_NEGATIVE_CONTROL_COUNT=2",
 "SMOKE_VERDICT_NEGATIVE_CONTROL_FALSE_PASS_COUNT=0",
])

w(S, "SMOKE3_STATUS.txt", [
 "SMOKE3_STATUS=PASS",
 "RUN2_QUALIFYING_EXECUTION_ALLOWED=YES",
 "",
 "SMOKE3_REQUIRED_GATE_COUNT=33",
 "SMOKE3_EVALUATED_GATE_COUNT=33",
 "SMOKE3_FAILED_GATE_COUNT=0",
 "SMOKE3_UNEVALUATED_GATE_COUNT=0",
 "SMOKE_VERDICT_NEGATIVE_CONTROL_COUNT=2",
 "SMOKE_VERDICT_NEGATIVE_CONTROL_FALSE_PASS_COUNT=0",
 "",
 "=== LES TROIS DEFAUTS SUCCESSIFS SONT TOUS FERMES ===",
 "  A  SYNC_COMPLETE perdu sur l epoque 0        -> corrige RUN2H, verifie SMOKE2/3",
 "  B  capture_epoch_id 0 rendue NA              -> corrige RUN2H, verifie SMOKE2/3",
 "  C  decode_step -1 en deux serialisations     -> corrige RUN2I, verifie SMOKE3",
 "  D  le script de verdict lui-meme (gate manquant) -> corrige, negative control 0 faux PASS",
 "",
 "=== CE QUE SMOKE3 PROUVE ===",
 "  M1 : 9 pas exactement couverts, prefill exclu (-1, jamais un pas)",
 "  M2 : 10 soumissions dont 1 prefill, IDs d occurrence distincts",
 "  M3 : ledger immediat, sequence 1..45 monotone, 0 doublon, 0 trou,",
 "       0 echec d E/S observable",
 "  Les 3 CSV partagent le meme schema semantique -> jointure directe possible",
 "  Les 2 CSV respectent leur schema sur TOUTES les lignes et TOUTES les epochs",
 "",
 "PLUMBING_FAILURE_29_STATUS=PATCH_AND_SMOKE_VERIFIED_AWAITING_QUALIFYING_RUN2",
 "  (RESOLVED_VERIFIED_BY_RUN2_RUNTIME_EVENT_LEDGER seulement apres le vrai Run2)",
 "ENGINE007_NEXT_STEP=RUN2_QUALIFYING_OFF_ON_WITH_RUNTIME_LEDGER",
])

w(R, "Q0FWD_RUN2_STATUS.txt", [
 "Q0_FORWARD_STATUS=BLOCKED",
 "Q1_FORWARD_ALLOWED=NO",
 "PLUMBING_FAILURE_COUNT=29",
 "PLUMBING_FAILURE_29_STATUS=PATCH_AND_SMOKE_VERIFIED_AWAITING_QUALIFYING_RUN2",
 "SMOKE1_STATUS=FAIL",
 "SMOKE2_STATUS=FAIL",
 "SMOKE3_STATUS=PASS",
 "RUN2_QUALIFYING_EXECUTION_ALLOWED=YES",
 "RUN2I_PATCH_SHA256=af9cf46b1872f651501657ff5df457db23e5aea020952bf51f5595a61bd9b182",
 "RUN2I_LIBLLAMA_SHA256=87c42d5f5dd7ab3c10967b4d3eaac82636b6af11e2c18a7f2798bab2d01bb2b2",
 "F1A_EVAL_SOURCE_CHANGED=NO",
 "F1A_EVAL_REBUILT=NO",
 "F1A_EVAL_SHA256=79ba1c449864da8be4b9fa53313a179c2ddad96bac1e814d77f470a8f2e7b80a",
 "RUNTIME_LIBLLAMA_MATCHES_INTENDED_RUN2I_BUILD=YES",
 "INSTRUMENTATION_SYMBOLS_IN_RUNTIME_LIBLLAMA=7",
 "SMOKE_VERDICT_NEGATIVE_CONTROL_FALSE_PASS_COUNT=0",
 "SMOKE3_FAILED_GATE_COUNT=0",
 "SMOKE3_UNEVALUATED_GATE_COUNT=0",
 "RUN2_OFF_STATUS=NOT_EXECUTED",
 "RUN2_ON_STATUS=NOT_EXECUTED",
 "REFERENCE_SAMPLE_TEMPORAL_PAIRING=PARTIAL",
 "PRODUCT_COMPUTE_SOURCE_DIFF_COUNT=0",
 "PRODUCT_MMVQ_SOURCE_MODIFIED=NO",
 "MODEL_GGUF_MODIFIED=NO",
 "ENGINE007_NEXT_STEP=RUN2_QUALIFYING_OFF_ON_WITH_RUNTIME_LEDGER",
])
print("livrables ecrits")
