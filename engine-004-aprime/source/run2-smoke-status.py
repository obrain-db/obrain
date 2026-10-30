#!/usr/bin/env python3
"""RUN2 smoke : livrables de statut. Le smoke ECHOUE : deux defauts reels."""
import csv, os

S = ("/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/"
     "qualification/e7b-f1a-q0-forward-v1/run2/smoke")
L = S + "/capon/EVENT_LEDGER.csv"

def w(name, lines):
    open(S + "/" + name, "w", encoding="utf-8", newline="\n").write("\n".join(lines) + "\n")
    print("  ecrit : " + name)

rows = list(csv.DictReader(open(L)))
by = {}
for r in rows:
    by.setdefault(r["event_kind"], []).append(r)

# ---------------- F1A_EVAL build provenance ----------------
w("F1A_EVAL_RUN2_BUILD_PROVENANCE.txt", [
 "F1A_EVAL_RUN2_BUILD_PROVENANCE",
 "",
 "F1A_EVAL_MANUAL_BUILD_COMMAND=g++ -O2 -o <bin>/f1a-eval tools/f1a-eval.cpp \\",
 "    -I ggml/include -I include -I src -I common -I tools \\",
 "    -L <bin> -lllama -lggml -lggml-base -Wl,-rpath,<bin>",
 "",
 "  PROVENANCE : f1a-eval n est PAS une cible CMake (grep sur CMakeLists.txt,",
 "  tools/CMakeLists.txt, src/CMakeLists.txt : 0 occurrence). Il a donc toujours",
 "  ete compile manuellement. La commande ci-dessus est celle utilisee a chaque",
 "  passe de cette session, et son include/link set est verifie par :",
 "    - le linkage runtime resolu (ldd) vers le build-cuda/bin attendu",
 "    - l execution reussie (RC=0, 9 pas, 672 payloads)",
 "",
 "F1A_EVAL_BUILD_RC=0",
 "F1A_EVAL_BUILD_DIAGNOSTIC_ERROR_COUNT=0",
 "F1A_EVAL_RUN2_SHA256=79ba1c449864da8be4b9fa53313a179c2ddad96bac1e814d77f470a8f2e7b80a",
 "F1A_EVAL_PREVIOUS_SHA256=e4b894287e1cc90cef1097253e9fb3115043bc1eeb965b8890c1c611e778cd7c",
 "F1A_EVAL_BINARY_CHANGED=YES",
 "",
 "F1A_EVAL_LINKED_LIBLLAMA_PATH=" + "/home/triviere/projects/llama-obrain-cuda-f1a/build-cuda/bin/libllama.so.0",
 "F1A_EVAL_LINKED_LIBLLAMA_SHA256=a9a788e166ea245cf718d919962af0d46ea05c4b197437d37b16074a0cb1a5d8",
 "F1A_EVAL_LINKS_RUN2_LIBLLAMA=YES",
 "  RUNPATH=/home/triviere/projects/llama-obrain-cuda-f1a/build-cuda/bin",
 "  -> la libllama.so.0 REELLEMENT resolue est celle du build Run2.",
 "INSTRUMENTATION_SYMBOLS_IN_LIBLLAMA=6  (reverifie sur la lib qui sera chargee)",
])

# ---------------- audits ----------------
w("SMOKE_STEP_AUDIT.txt", [
 "SMOKE_STEP_AUDIT",
 "",
 "PREFILL : token 9906",
 "  PREFILL_DECODE_STEP=-1 (serialise NA par la politique NA)",
 "  PREFILL_CLASSIFIED_AS_SCIENTIFIC_STEP=NO",
 "  -> aucun DECODE_BEGIN scientifique pour le prefill",
 "",
 "NEUF PAS SCIENTIFIQUES",
 "  SCIENTIFIC_DECODE_STEP_SET=0,1,2,3,4,5,6,7,8",
 "  UNKNOWN_SCIENTIFIC_DECODE_STEP_COUNT=0",
 "  DECODE_BEGIN_COUNT=9   step_set=0..8",
 "  LOGITS_READ_COUNT=9    step_set=0..8",
 "  DECODE_END_COUNT=9     step_set=0..8",
 "",
 "  -> L INSTRUMENTATION M1 FONCTIONNE. Les neuf pas sont exactement couverts,",
 "     le prefill est exclu, et aucun LOGITS_READ scientifique n est a -1.",
])

w("SMOKE_SUBMISSION_AUDIT.txt", [
 "SMOKE_SUBMISSION_AUDIT",
 "",
 "COMPUTE_SUBMIT_COUNT=9",
 "COMPUTE_SUBMISSION_ID_SET=2,3,4,5,6,7,8,9,10",
 "COMPUTE_SUBMISSION_ID_DUPLICATE_OCCURRENCE_COUNT=0",
 "  -> occurrence-based CONFIRME : 9 soumissions scientifiques, 9 IDs distincts.",
 "  (la soumission 1 est celle du prefill, hors population scientifique)",
 "",
 "  mapping pas -> soumission :",
 "    step 0 -> sid 2      step 1 -> sid 3      step 2 -> sid 4",
 "    step 3 -> sid 5      step 4 -> sid 6      step 5 -> sid 7",
 "    step 6 -> sid 8      step 7 -> sid 9      step 8 -> sid 10",
 "",
 "SYNC_COMPLETE_COUNT=1        <-- ANOMALIE : 1 seul pour 2 epoques",
 "  SYNC_BEFORE_SUBMIT_COUNT=0  (aucun SYNC avant son SUBMIT)",
])

w("SMOKE_MANIFEST_SCHEMA_AUDIT.txt", [
 "SMOKE_MANIFEST_SCHEMA_AUDIT",
 "STATUS=NON_EXECUTE_DANS_CETTE_PASSE",
 "  Le smoke s arrete sur le premier defaut (paragraphe 33).",
 "  Les schemas CSV seront audites quand le smoke passera.",
])

w("SMOKE_PAYLOAD_PROVENANCE_SCHEMA_AUDIT.txt", [
 "SMOKE_PAYLOAD_PROVENANCE_SCHEMA_AUDIT",
 "STATUS=NON_EXECUTE_DANS_CETTE_PASSE",
 "  Arret sur le premier defaut (paragraphe 33).",
])

w("SMOKE_CROSSCHECK_AUDIT.txt", [
 "SMOKE_CROSSCHECK_AUDIT",
 "STATUS=NON_EXECUTE_DANS_CETTE_PASSE",
 "  Les cross-checks ledger/manifest/provenance ne sont pas executes :",
 "  le ledger est incomplet (SYNC_COMPLETE manquant sur l epoque 0), donc",
 "  tout cross-check serait fait sur une base partielle.",
])

# ---------------- epoch mapping ----------------
groups = []
cur = {}
for r in rows:
    if r["event_kind"] == "CAPTURE_BEGIN": cur = {"b": r}
    elif r["event_kind"] == "CAPTURE_EXTRACT" and cur: cur["e"] = r
    elif r["event_kind"] == "CAPTURE_END" and cur: cur["n"] = r; groups.append(cur); cur = {}
with open(S + "/SMOKE_EPOCH_MAPPING.csv", "w", newline="") as f:
    f.write("group_index,capture_epoch_id,decode_step,compute_submission_id,eligible,step_consistent\n")
    for i, g in enumerate(groups):
        b = g["b"]
        trip = {(g[k]["decode_step"], g[k]["compute_submission_id"], g[k]["capture_epoch_id"])
                for k in ("b","e","n") if k in g}
        f.write("%d,%s,%s,%s,%s,%s\n" % (i, b["capture_epoch_id"], b["decode_step"],
                b["compute_submission_id"], b["eligible_target_count"],
                "YES" if len(trip)==1 else "NO"))
print("  ecrit : SMOKE_EPOCH_MAPPING.csv")

# ---------------- verdict ----------------
w("SMOKE_STATUS.txt", [
 "SMOKE_STATUS=FAIL",
 "RUN2_QUALIFYING_EXECUTION_ALLOWED=NO",
 "",
 "FIRST_SMOKE_DEFECT=SYNC_COMPLETE_DROPPED_ON_FIRST_EPOCH_OUTDIR_NOT_YET_CREATED",
 "",
 "=== LE DEFECT 1, EN PRECIS ===",
 "",
 "Localisation dans le fichier PATCHE src/f1a-capture.cpp :",
 "  526    if (backend != nullptr) ggml_backend_synchronize(backend);",
 "  530    f1a_event_row(\"SYNC_COMPLETE\", ...);   <-- emis ICI",
 "  533    if (mkdir(g_f1a.outdir.c_str(), 0755) != 0) { }",
 "",
 "  SYNC_COMPLETE est emis AVANT que mkdir ne cree le dossier de sortie.",
 "  Sur l EPOQUE 0, g_f1a.outdir n existe pas encore : fopen echoue dans",
 "  f1a_event_row, qui fait `return` SANS ECRIRE et SANS SIGNALER.",
 "  L evenement est donc PERDU SILENCIEUSEMENT.",
 "",
 "  PREUVE : SYNC_COMPLETE_COUNT=1 alors qu il y a 2 epoques.",
 "  L unique SYNC_COMPLETE (seq=7) porte epoch=1, apres la creation du dossier.",
 "",
 "  GRAVITE : c est exactement la classe de defaut que toute cette campagne",
 "  combat — une instrumentation qui perd un evenement sans le dire. Sur le",
 "  Run2 qualificatif, l epoque 0 perdrait son SYNC_COMPLETE de la meme facon.",
 "",
 "=== LE DEFECT 2, EN PRECIS ===",
 "FIRST_SMOKE_DEFECT_2=CAPTURE_EPOCH_ID_ZERO_SERIALISED_AS_NA",
 "",
 "  L implementation serialise 0 en NA :",
 "    if (capture_epoch_id == 0) snprintf(epbuf, sizeof epbuf, \"NA\");",
 "  Or l EPOQUE 0 EST UNE VALEUR LEGITIME. Consequence : les 3 evenements",
 "  CAPTURE_* de l epoque 0 portent capture_epoch_id=NA.",
 "  CAPTURE_EPOCH_ID_ZERO_RENDERED_AS_NA_COUNT=3",
 "",
 "  (meme politique appliquee a compute_submission_id : inoffensif, les IDs",
 "   commencent a 1. Mais la politique est fausse pour l epoch.)",
 "",
 "=== CE QUI FONCTIONNE MALGRE TOUT ===",
 "  M1 : 9 DECODE_BEGIN, 9 LOGITS_READ, 9 DECODE_END, steps 0..8, prefill exclu",
 "  M2 : 9 COMPUTE_SUBMIT, IDs 2..10 distincts, occurrence-based confirme",
 "  M3 : ledger cree, event_sequence_id 1..43 monotone, 0 doublon",
 "  CAPTURE_EVENT_STEP_MISMATCH_COUNT=0 sur les groupes CAPTURE complets",
 "",
 "=== ARRET (paragraphe 33) ===",
 "  Je ne corrige pas et je ne relance pas de variante dans cette passe.",
 "PLUMBING_FAILURE_29_STATUS=OPEN",
 "ENGINE007_NEXT_STEP=FIX_FIRST_SMOKE_INSTRUMENTATION_DEFECT",
])
print("  ecrit : SMOKE_STATUS.txt")
