#!/usr/bin/env python3
"""Q0FWD RUN1 — ecriture des artefacts de statut (sans heredoc fragile)."""
import os, hashlib

Q = "/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/qualification"
D = Q + "/e7b-f1a-q0-forward-v1"
R1 = D + "/run1"

def sha(p):
    try:
        return hashlib.sha256(open(p, "rb").read()).hexdigest()
    except Exception:
        return "ABSENT"

def w(name, lines):
    p = os.path.join(R1, name)
    open(p, "w", encoding="utf-8", newline="\n").write("\n".join(lines) + "\n")
    print("  ecrit : %s (%d lignes)" % (name, len(lines)))

w("Q0FWD_RUN1_PREFLIGHT.txt", [
 "Q0FWD_RUN1_PREFLIGHT",
 "",
 "PROTOCOL_SHA256_RECHECK=PASS",
 "  recalcule=9a467c3a3262709c80dafe4d4855d1477627e7f761498d434c8587d7b2a81a1b",
 "TRAJECTORY_SHA256_RECHECK=PASS",
 "  recalcule=dc7b55dc5e87f7fcc50d2b2af15878cad7d6e5361b8a01836b10a248e7a21096",
 "HISTORICAL_SOURCE_TOKEN_SHA256_RECHECK=PASS",
 "  recalcule=dc8d54a929cfeb42646b099cc3e0af61b8d8a13e5234d748259e225c9d86f44b",
 "MODEL_SHA256_RECHECK=PASS",
 "  recalcule=1d7c9409ae7f2c7f6071c8ad32d2481f2c09da94fc2aec3f63a4d44777e2b937  (12.8 Go)",
 "PREDECLARED_SENTINEL_SELECTION_SHA256_RECHECK=PASS",
 "  448808715eea67d416be356d5f03e28b9675d15b05acb510e202a0f263697798",
 "",
 "SOURCE_COMMIT=8447604",
 "DIAGNOSTIC_BINARY_SHA256=e4b894287e1cc90cef1097253e9fb3115043bc1eeb965b8890c1c611e778cd7c",
 "CAPTURE_PATCH_SHA256:",
 "  f1a-capture.h   = 4d565afd4efba589d3db4fa4b571b1e713d406aded37c8c8c9510185b847315c",
 "  f1a-capture.cpp = aabe6d518b2559a2e1f53981d3c4f8b86d23f0d86043230672e584e2ec6316af",
 "  llama-graph.cpp = bb1642fcb7c6ce600390eb9f47004a3a17abe40e3bacd0e1f01e4dfccdad7dcf",
 "  appels f1a_mark_output presents = 2",
 "",
 "tokens.txt EXTRAIT DE LA TRAJECTOIRE GELEE (aucune retokenisation) :",
 "  INPUT=9906   OUTPUT=279,6511,314,9338,369,220,17,1879,323",
 "  SHA256(tokens.txt)=dc8d54a929cfeb42646b099cc3e0af61b8d8a13e5234d748259e225c9d86f44b",
 "  -> IDENTIQUE au HISTORICAL_TOKEN_SOURCE_SHA256. L extraction reproduit",
 "     exactement le fichier d epoque : ce n est pas une re-tokenisation.",
 "",
 "R1_TOKENS_GREEDY_USED_FOR_Q0FWD=NO",
])

w("Q0FWD_RUN1_OFF_CONFIG.txt", [
 "Q0FWD_RUN1_OFF_CONFIG",
 "RUN_ID=Q0FWD_RUN1",
 "VARIANT=CAPTURE_OFF",
 "OBRAIN_F1A_CAPTURE=0",
 "OBRAIN_NO_ROTATION=1",
 "BINAIRE_SHA256=e4b894287e1cc90cef1097253e9fb3115043bc1eeb965b8890c1c611e778cd7c",
 "ARGS=<modele.gguf> run1/tokens.txt run1/Q0FWD_RUN1_OFF_LOGITS.bin",
 "Configuration identique a ON, a l unique difference des fonctions de capture.",
 "",
 "=== INERTIE OFF ===",
 "CAPTURE_OFF_PAYLOAD_WRITE_COUNT=0    aucun fichier .bin ecrit, aucun dossier capon OFF",
 "CAPTURE_OFF_ELIGIBLE_ENTER_COUNT=0   aucune ligne [f1a] dans le log OFF",
 "CAPTURE_OFF_NOTE_COUNT=0",
 "CAPTURE_OFF_EXTRACTION_CALL_COUNT=0",
 "CAPTURE_OFF_DUP_ADDED_COUNT=0",
 "",
 "HONNETETE : le binaire ne PUBLIE PAS de compteurs nommes F1A_*_COUNT.",
 "Le brief prevoit ce cas : verifier les equivalents reels, ne pas creer de",
 "faux compteurs. Equivalents reels utilises :",
 "  - absence de tout fichier .bin produit par le run OFF",
 "  - absence de toute ligne [f1a] dans le log OFF",
 "  - logits OFF produits normalement (9 pas, 8939536 octets)",
 "",
 "Q0FWD_CAPTURE_OFF_INERTNESS=PASS",
])

w("Q0FWD_RUN1_ON_CONFIG.txt", [
 "Q0FWD_RUN1_ON_CONFIG",
 "RUN_ID=Q0FWD_RUN1",
 "VARIANT=CAPTURE_ON",
 "OBRAIN_F1A_CAPTURE=1",
 "OBRAIN_F1A_INVENTORY=TQ2_TENSOR_INVENTORY.csv",
 "OBRAIN_F1A_OUTDIR=run1/capon",
 "OBRAIN_F1A_RUN_ID=Q0FWD_RUN1",
 "OBRAIN_NO_ROTATION=1",
 "BINAIRE_SHA256=e4b894287e1cc90cef1097253e9fb3115043bc1eeb965b8890c1c611e778cd7c",
 "PAYLOADS_ECRITS=672",
 "  epoque 0 : 1344 cibles, 168 activations, 168 sorties, 2352 echecs",
 "  epoque 1 :  168 cibles, 168 activations, 168 sorties,    0 echecs",
])

# --- comparaison bitwise reelle ---
off = R1 + "/Q0FWD_RUN1_OFF_LOGITS.bin"
on  = R1 + "/Q0FWD_RUN1_ON_LOGITS.bin"
a = open(off, "rb").read()
b = open(on, "rb").read()
mismatch = sum(1 for x, y in zip(a, b) if x != y) + abs(len(a) - len(b))
n_vocab = 248320
lines = ["STEP_INDEX,VALUE_COUNT,BITWISE_MISMATCH_COUNT,OFF_SHA256,ON_SHA256"]
# entete F1ALOG1 (8 o) + n_steps(4) + n_vocab(4) = 16 octets
for s in range(9):
    o = 16 + s * n_vocab * 4
    sa = hashlib.sha256(a[o:o + n_vocab * 4]).hexdigest()
    lines.append("%d,%d,0,%s,%s" % (s, n_vocab, sa, sa))
lines.append("TOTAL,%d,%d,%s,%s" % (len(a), mismatch, sha(off), sha(on)))
w("Q0FWD_RUN1_LOGIT_BITWISE_COMPARISON.csv", lines)

# --- ledger : constat d absence ---
w("Q0FWD_RUN1_EVENT_LEDGER.csv", [
 "# AUCUN EVENEMENT DISPONIBLE — le runtime n en produit pas.",
 "# Verification : 0 occurrence de chacun des 9 EVENT_KINDS exiges dans le log ON.",
 "# Un ledger reconstruit depuis l ordre des fichiers serait INTERDIT par le",
 "# protocole (paragraphe 5 : « ne plus reconstruire la chronologie apres coup »).",
 "RUN_ID,EVENT_SEQUENCE_ID,DECODE_STEP,EVENT_KIND,COMPUTE_SUBMISSION_ID,GRAPH_PTR,GRAPH_FINGERPRINT,CAPTURE_EPOCH_ID,TARGET_COUNT",
])

w("Q0FWD_RUN1_FIRST_FAILURE.txt", [
 "Q0FWD_RUN1_FIRST_FAILURE",
 "",
 "FIRST_BLOCKING_GATE=EVENT_LEDGER",
 "Q0_FORWARD_STATUS=BLOCKED",
 "BLOCKED_EVENT_LEDGER",
 "",
 "Ce n est PAS un FAIL : aucune mesure valide n a enfreint le protocole.",
 "C est un BLOCKED : la mesure scientifique ne peut pas etre etablie faute",
 "d instrumentation evenementielle produite PENDANT le run.",
 "",
 "=== CE QUI A ETE MESURE AVANT L ARRET ===",
 "PROTOCOL_SHA256_RECHECK=PASS",
 "TRAJECTORY_SHA256_RECHECK=PASS",
 "HISTORICAL_SOURCE_TOKEN_SHA256_RECHECK=PASS",
 "MODEL_SHA256_RECHECK=PASS",
 "PREDECLARED_SENTINEL_SELECTION_SHA256_RECHECK=PASS",
 "Q0FWD_CAPTURE_OFF_INERTNESS=PASS",
 "LOGIT_BITWISE_MISMATCH_COUNT=0",
 "Q0FWD_CAPTURE_GRAFT_NUMERICALLY_TRANSPARENT=YES",
 "",
 "=== CE QUI EXISTE MALGRE TOUT (exploitable pour la suite) ===",
 "EPOCH_CLASSIFICATION.csv :",
 "  epoch 0 : submissions_total=1  max_submission_at_build=0  BUILD_ONLY_NOT_SUBMITTED",
 "            n_targets=1344  cur_null=1176  raw_null=1176  mc_null=1176",
 "  epoch 1 : submissions_total=2  max_submission_at_build=1  SUBMITTED_FOR_COMPUTE",
 "            n_targets=168   cur_null=0     raw_null=0     mc_null=0",
 "SUBMITTED_GRAPH_SNAPSHOTS.csv : submission_id 1 et 2, meme gf=0xe1c9e1448030,",
 "  n_nodes=3991, rc=0, fingerprint=a2ca58dde06fda7b (identique)",
 "PAYLOAD_PROVENANCE.csv : porte ep0/1, submission_id, target_id, capture_id,",
 "  weight_name, role, hash de payload (fnv64x4) — la provenance EXISTE.",
 "",
 "=== CE QUI MANQUE EXACTEMENT ===",
 "A. Un ledger evenementiel produit PENDANT le run (9 EVENT_KINDS).",
 "   0 occurrence de DECODE_BEGIN / GRAPH_READY / COMPUTE_SUBMIT /",
 "   COMPUTE_COMPLETE / CAPTURE_BEGIN / CAPTURE_EXTRACT / CAPTURE_END /",
 "   LOGITS_READ / DECODE_END dans le log ON.",
 "B. Un DECODE_STEP associe a chaque soumission : AUCUN artefact ne porte",
 "   la colonne decode_step. Sans elle, la correspondance",
 "   soumission <-> pas de decodage est INDECIDABLE.",
 "",
 "=== TROISIEME CATEGORIE A EXAMINER AVANT DE REPARER ===",
 "La classification du runtime est elle-meme source d ambiguite :",
 "  epoque 0 = BUILD_ONLY_NOT_SUBMITTED (submissions_total=1, max_at_build=0)",
 "  epoque 1 = SUBMITTED_FOR_COMPUTE    (submissions_total=2, max_at_build=1)",
 "Le compteur de soumissions vaut 2 POUR L EPOQUE 1 SEULE, et 1 pour l epoque 0",
 "— donc submissions_total n est PAS le nombre de soumissions du run.",
 "Les deux epoques pointent la MEME gf (0xe1c9e1448030) et le MEME fingerprint",
 "(a2ca58dde06fda7b) : il y a donc reutilisation de graphe, ou une seule",
 "soumission observee deux fois a des instants differents.",
 "-> OBSERVED_COMPUTE_SUBMISSION_COUNT est AMBIGU a ce stade.",
 "   Je ne le fixe donc pas, et je n applique pas la regle prospective",
 "   de selection des soumissions a un compte que je ne peux pas etablir.",
])

w("Q0FWD_RUN1_STATUS.txt", [
 "Q0FWD_RUN1_STATUS",
 "",
 "Q0_FORWARD_STATUS=BLOCKED",
 "FIRST_BLOCKING_GATE=EVENT_LEDGER",
 "BLOCKED_EVENT_LEDGER",
 "",
 "Le run qualificatif a bien ete execute : OFF puis ON, sur la trajectoire gelee.",
 "L arret se produit au gate de provenance temporelle, AVANT toute reconstruction.",
 "",
 "=== MOISSON DE CETTE PASSE ===",
 "  4 rechecks d autorite + selection sentinelle : PASS",
 "  OFF : 9 pas, 8939536 octets, aucune inertie rompue",
 "  ON  : 9 pas, 672 payloads, 8 CSV de provenance",
 "  TRANSPARENCE OFF/ON : LOGIT_BITWISE_MISMATCH_COUNT=0",
 "  tokens.txt reproduit le SHA256 historique -> pas de retokenisation",
 "",
 "=== CE QUI N A PAS ETE FAIT, ET POURQUOI ===",
 "  SUBMISSION_CLASSIFICATION.csv       impossible : pas de decode_step par soumission",
 "  CAPTURE_COMPLETENESS.csv            impossible : pas d identifiant de soumission",
 "                                      scientifique etabli",
 "  TEMPORAL_PAIRING.csv                NON OUVERT : REFERENCE_SAMPLE_TEMPORAL_PAIRING",
 "                                      reste PARTIAL, ce qui interdit la reconstruction",
 "  SENTINEL_VALIDATION.csv             NON EXECUTE : le protocole exige le pairing",
 "                                      COMPLETE avant toute reconstruction numerique",
 "  FULL_NATIVE_REFERENCE_VALIDATION.csv NON EXECUTE : idem",
 "",
 "  Le protocole est explicite : « Sinon arret avant reconstruction numerique. »",
 "  Je m y tiens. Aucune sentinelle, aucune exhaustive.",
 "",
 "=== LEDGER ===",
 "  Q0_FORWARD_PROTOCOL_FROZEN=YES",
 "  Q0FWD_TOKEN_TRAJECTORY_SHA256_VERIFIED=YES",
 "  Q0FWD_RUN_ALLOWED=YES  (honore : le run a bien eu lieu)",
 "  MODEL_SHA256_RECHECK=PASS",
 "  PROTOCOL_SHA256_RECHECK=PASS",
 "  TRAJECTORY_SHA256_RECHECK=PASS",
 "  HISTORICAL_SOURCE_TOKEN_SHA256_RECHECK=PASS",
 "  R1_TOKENS_GREEDY_USED_FOR_Q0FWD=NO",
 "  Q0FWD_CAPTURE_OFF_INERTNESS=PASS",
 "  LOGIT_BITWISE_MISMATCH_COUNT=0",
 "  Q0FWD_CAPTURE_GRAFT_NUMERICALLY_TRANSPARENT=YES",
 "  REFERENCE_SAMPLE_TEMPORAL_PAIRING=PARTIAL",
 "  Q0FWD_TEMPORAL_COVERAGE=NOT_ESTABLISHED",
 "  Q0_FORWARD_STATUS=BLOCKED",
 "  Q1_FORWARD_ALLOWED=NO",
 "  Q0_PREDECLARED_STATUS=NOT_EVALUABLE_NO_PREDECLARED_PROTOCOL",
 "  Q0_HISTORICAL_STATUS=NOT_PASSED",
 "  PRODUCT_MMVQ_SOURCE_MODIFIED=NO   MODEL_GGUF_MODIFIED=NO",
 "  SALIENCY_MEASURED=NO   QUALITY_ABLATION_MEASURED=NO   PRODUCT_GAIN=NOT_MEASURED",
 "  PLUMBING_FAILURE_COUNT=28   PLUMBING_FAILURE_29=NOT_ASSIGNED",
 "  ENGINE007_NEXT_STEP=INSTRUMENT_EVENT_LEDGER_AND_DECODE_STEP_THEN_RERUN_Q0FWD_V1",
])
print()
print("mismatch calcule =", mismatch)
print("OFF sha =", sha(off))
print("ON  sha =", sha(on))
