#!/usr/bin/env python3
"""SMOKE2 : livrables restants."""
import os

R = ("/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/"
     "qualification/e7b-f1a-q0-forward-v1/run2")
S = R + "/smoke2"

def w(d, name, lines):
    open(os.path.join(d, name), "w", encoding="utf-8", newline="\n").write("\n".join(lines) + "\n")
    print("  " + name)

w(R, "RUN2H_SMOKE_FIX_PATCH_INFO.txt", [
 "RUN2H_SMOKE_FIX_PATCH",
 "SHA256=48e2494aabb0a92d03a61d7e43e8bd5c7f5269b95a76b149e9e5901c547ced2a",
 "FILES=2 (src/f1a-capture.h, src/f1a-capture.cpp)  HUNKS=6  ADDED=52  REMOVED=11",
 "",
 "CLASSIFICATION DES HUNKS :",
 "  SMOKE_FIX_LEDGER_DIRECTORY_ORDER        : f1a_ensure_outdir() + appel en tete de f1a_event_row",
 "  SMOKE_FIX_LEDGER_IO_FAILURE_VISIBILITY  : g_f1a_event_io_fail, 3 fprintf stderr, f1a_event_io_failure_count()",
 "  SMOKE_FIX_CAPTURE_EPOCH_SERIALIZATION   : parametre capture_epoch_applicable, current=0 / latched=1",
 "  OTHER                                   : 0",
 "UNCLASSIFIED_HUNK_COUNT=0",
 "",
 "PRODUCT_COMPUTE_SOURCE_DIFF_COUNT=0 (mmvq.cu / vecdotq.cuh / quantize.cu non touches)",
])

w(R, "RUN2H_BUILD_PROVENANCE.txt", [
 "RUN2H_BUILD_PROVENANCE",
 "TARGETED_COMPILE_CHECK=PASS  (fsyntax-only f1a-capture.cpp, 0 diagnostic)",
 "FULL_REBUILD=100% Built target llama  (cmake --build . --target llama, erreurs=0)",
 "F1A_EVAL_BUILD_RC=0",
 "",
 "RUN2H_LIBLLAMA_BUILD_PATH=/home/triviere/projects/llama-obrain-cuda-f1a/build-cuda/bin/libllama.so",
 "RUN2H_LIBLLAMA_BUILD_SHA256=3f0f1390168d061f67651c1c8f4570bef61fa3d5fdcf2427b5c76ba2f629082b",
 "RUN2H_F1A_EVAL_PATH=/home/triviere/projects/llama-obrain-cuda-f1a/build-cuda/bin/f1a-eval",
 "RUN2H_F1A_EVAL_SHA256=79ba1c449864da8be4b9fa53313a179c2ddad96bac1e814d77f470a8f2e7b80a",
 "",
 "NOTE : f1a-eval SHA est INCHANGE (79ba1c44) car le patch RUN2H ne touche que",
 "f1a-capture.{h,cpp}. f1a-eval.cpp n a pas ete modifie : son binaire est",
 "legitimement identique. Le changement est dans libllama.so.",
])

w(R, "RUN2H_RUNTIME_LIBRARY_PROVENANCE.txt", [
 "RUN2H_RUNTIME_LIBRARY_PROVENANCE",
 "",
 "=== RECONCILIATION DU PARAGAPHE 15 ===",
 "Le brief soupconnait une ambiguite entre deux hashes. Il avait raison :",
 "le champ ambigu RUN2_BINARY_SHA256=79ba1c44... designait en fait f1a-eval,",
 "PAS libllama. Verification :",
 "",
 "  79ba1c449864da8be4b9fa53313a179c2ddad96bac1e814d77f470a8f2e7b80a",
 "    -> .../build-cuda/bin/f1a-eval        (le HARNAIS)",
 "  a9a788e166ea245cf718d919962af0d46ea05c4b197437d37b16074a0cb1a5d8",
 "    -> .../build-cuda/bin/libllama.so.0.0.135  AVANT le rebuild RUN2H",
 "",
 "  Les trois noms libllama.so / libllama.so.0 / libllama.so.0.0.135 sont des",
 "  SYMLINKS vers le MEME fichier. Aucune collision : c est le meme artefact",
 "  a deux instants de build.",
 "",
 "  a9a788e1 = libllama AVANT correctif",
 "  3f0f1390 = libllama APRES correctif RUN2H",
 "",
 "=== ETAT APRES REBUILD ===",
 "  libllama.so        -> sha 3f0f1390...  (symlink)",
 "  libllama.so.0      -> sha 3f0f1390...  (symlink)",
 "  libllama.so.0.0.135-> sha 3f0f1390...  (fichier reel)",
 "",
 "F1A_EVAL_LINKED_LIBLLAMA_PATH=/home/triviere/projects/llama-obrain-cuda-f1a/build-cuda/bin/libllama.so.0",
 "F1A_EVAL_LINKED_LIBLLAMA_SHA256=3f0f1390168d061f67651c1c8f4570bef61fa3d5fdcf2427b5c76ba2f629082b",
 "RUNTIME_LIBLLAMA_MATCHES_INTENDED_RUN2H_BUILD=YES",
 "",
 "INSTRUMENTATION_SYMBOLS_IN_RUNTIME_LIBLLAMA=7",
 "  6 attendus :",
 "    f1a_set_decode_step, f1a_last_submission_id, f1a_last_submission_decode_step,",
 "    f1a_event_current, f1a_event_latched, f1a_event_close",
 "  +1 ajoute par RUN2H : f1a_event_io_failure_count",
 "  -> 7 est l attendu APRES correctif, pas 6. L ecart est explique, pas tolere.",
])

w(S, "SMOKE2_STEP_AUDIT.txt", [
 "SMOKE2_STEP_AUDIT",
 "DECODE_BEGIN_COUNT=9   step_set=0,1,2,3,4,5,6,7,8",
 "LOGITS_READ_COUNT=9    step_set=0,1,2,3,4,5,6,7,8",
 "DECODE_END_COUNT=9     step_set=0,1,2,3,4,5,6,7,8",
 "SCIENTIFIC_DECODE_STEP_SET=0,1,2,3,4,5,6,7,8",
 "UNKNOWN_SCIENTIFIC_DECODE_STEP_COUNT=0",
 "PREFILL_DECODE_STEP=-1 (rendu NA dans le ledger)",
 "PREFILL_CLASSIFIED_AS_SCIENTIFIC_STEP=NO",
 "M1 = PASS",
])

w(S, "SMOKE2_SUBMISSION_AUDIT.txt", [
 "SMOKE2_SUBMISSION_AUDIT",
 "COMPUTE_SUBMIT_COUNT=10",
 "COMPUTE_SUBMISSION_ID_SET=1,2,3,4,5,6,7,8,9,10",
 "COMPUTE_SUBMISSION_ID_DUPLICATE_OCCURRENCE_COUNT=0",
 "  ATTENTION au compte : 10 soumissions, dont la 1 est celle du PREFILL",
 "  (hors population scientifique). Les 9 scientifiques sont 2..10.",
 "SYNC_COMPLETE_COUNT=2   SYNC_BEFORE_SUBMIT_COUNT=0",
 "M2 = PASS",
])

w(S, "SMOKE2_MANIFEST_SCHEMA_AUDIT.txt", [
 "SMOKE2_MANIFEST_SCHEMA_AUDIT",
 "MANIFEST header=24 colonnes, 672 lignes de donnees",
 "lignes dont la largeur differe du header : 0",
 "epochs presentes : 0 ET 1  -> le piege append multi-epoch est REELLEMENT teste",
 "SMOKE2_MANIFEST_SCHEMA=PASS",
])

w(S, "SMOKE2_PAYLOAD_PROVENANCE_SCHEMA_AUDIT.txt", [
 "SMOKE2_PAYLOAD_PROVENANCE_SCHEMA_AUDIT",
 "PROVENANCE header=18 colonnes, 672 lignes",
 "lignes dont la largeur differe du header : 0",
 "header : epoch,submission_id,target_id,capture_id,weight_name,role,dtype,ne0,nb0,",
 "         contiguous,expected_bytes,written_bytes,path,hash_algo,payload_hash,",
 "         run_id,decode_step,capture_epoch_id",
 "SMOKE2_PAYLOAD_PROVENANCE_SCHEMA=PASS",
])

w(S, "SMOKE2_CROSSCHECK_AUDIT.txt", [
 "SMOKE2_CROSSCHECK_AUDIT",
 "",
 "SMOKE2_LEDGER_MANIFEST_MISMATCH_COUNT=2",
 "SMOKE2_LEDGER_PAYLOAD_PROVENANCE_MISMATCH_COUNT=2",
 "",
 "VALEURS REELLES :",
 "  LEDGER     : epoch 0 -> decode_step = \"NA\"",
 "  MANIFESTE  : epoch 0 -> decode_step = \"-1\"",
 "  PROVENANCE : epoch 0 -> decode_step = \"-1\"",
 "",
 "Meme valeur sous-jacente (-1), deux serialisations. La jointure sur",
 "decode_step entre le ledger et les deux CSV est donc IMPOSSIBLE.",
 "",
 "Epoch 1 est coherente partout (\"0\") : le defaut ne touche que la valeur",
 "non applicable, c est-a-dire l epoque 0 (celle du prefill).",
 "",
 "-> CROSS-CHECK NON SATISFAIT. SMOKE2_STATUS=FAIL sur ce point.",
])
print("livrables ecrits")
