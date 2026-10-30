# ENGINE-007B-F1A-Q0FWD-v1-RUN2 — INSTRUMENTATION TEMPORELLE

Date : 2026-09-30. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../qualification/e7b-f1a-q0-forward-v1/run2/`

---

## 0 — Résultat

**J'ai fait l'audit exigé par le §5. Je n'ai PAS appliqué le patch, et je le dis plutôt que de
livrer un binaire non vérifié.**

```
Q0_FORWARD_STATUS=BLOCKED
FIRST_BLOCKING_GATE=INSTRUMENTATION_NOT_APPLIED
BLOCKED_INSTRUMENTATION_NOT_APPLIED
Q1_FORWARD_ALLOWED=NO
```

```
RUN1_STATUS=BLOCKED_EVENT_LEDGER          (permanent)
RUN1_PAYLOADS_REUSED_FOR_RUN2=NO
RUN1_NUMERIC_VALIDATION_REUSED_FOR_RUN2=NO
```

Aucun run OFF/ON, aucun payload, aucun mélange avec Run1.

## 1 — La découverte structurante de l'audit

**Le compteur de soumission EXISTE DÉJÀ, et il est au bon endroit.**

```
src/llama-context.cpp:2951   ggml_backend_sched_graph_compute_async(sched, gf);
src/llama-context.cpp:2955   f1a_mark_compute_submission(gf, (int) status);

src/f1a-capture.cpp:90       unsigned long long f1a_ctr_submission = 0;
src/f1a-capture.cpp:128          f1a_ctr_submission++;
src/f1a-capture.cpp:132          s.submission_id = f1a_ctr_submission;
```

Le commentaire du fork le dit lui-même : *« HOOK AUTORITAIRE DE SOUMISSION DE CALCUL. C'est ICI, et
seulement ici, qu'un graphe est prouvé SOUMIS. »*

Il est **occurrence-based** et conforme au §7 : incrémenté uniquement sur une vraie soumission,
jamais sur un graph build / prepare / fingerprint / discovery. Constaté en Run1 : submissions 1 et 2
avec **le même `gf`** et **des IDs distincts** — exactement ce que le §7 exige.

```
ACTUAL_COMPUTE_SUBMIT_FUNCTION=ggml_backend_sched_graph_compute_async (l.2951)
```

`submission_id` est **déjà** propagé dans `GRAPH_CAPTURE_MANIFEST.csv` et `PAYLOAD_PROVENANCE.csv`.

## 2 — Ce qui manque réellement : plus étroit que prévu

```
M1  un contexte de pas : f1a_set_decode_step(s) pose par f1a-eval.cpp:77, lu par la capture
    -> LE point bloquant : decode_step n existe nulle part aujourd hui
M2  propagation de decode_step dans GRAPH_CAPTURE_MANIFEST.csv et
    PAYLOAD_PROVENANCE.csv, COPIE au moment de l enregistrement
M3  un ledger evenementiel ecrit a la volee, event_sequence_id monotone
```

## 3 — Le piège que l'audit a révélé

`CAPTURE_BEGIN` / `CAPTURE_EXTRACT` / `CAPTURE_END` se produisent dans `f1a_capture_end_epoch`,
appelé depuis `llama-context.cpp:1784` **après la synchronisation**. À cet instant, `f1a-eval` est
**déjà au pas suivant** (ou a terminé).

**Lire « current_decode_step » naïvement donnerait une valeur fausse.** Il faut mémoriser le
`decode_step` dans l'enregistrement de cible au moment de `f1a_capture_note`
(`llama-graph.cpp:1534`) et l'utiliser à la finalisation.

C'est exactement ce que ton §6 dit : *« enregistrer la valeur copiée au moment de l'événement, pas
relire plus tard un compteur mutable. »* L'audit l'a confirmé **avant** le patch, pas après.

## 4 — La correspondance `s` → `OUTPUT_IDS[s]` (§4)

```
FILE  tools/f1a-eval.cpp:77   for (int32_t s = 0; s < n_steps; ++s)
LINE  74   llama_decode    <- PREFILL de toks[0]=9906, HORS pas scientifique
LINE  86   llama_decode    <- INJECTION de toks[s+1], DANS le pas s
LINE  79   logits lus AVANT l injection du pas suivant

STEP 0->279  1->6511  2->314  3->9338  4->369  5->220  6->17  7->1879  8->323
```

**Compter les appels `llama_decode` donnerait 10, pas 9.** Le `decode_step` est le `s` de la boucle
qui produit les logits évalués — pas un compteur d'appels. Ton §4 demandait précisément de vérifier
la boucle réelle avant de définir ; c'est fait.

## 5 — Pourquoi je m'arrête

```
1. Le patch exige 3 fichiers du fork + 1 en-tete + un rebuild CUDA complet,
   puis la preuve PRODUCT_COMPUTE_SOURCE_DIFF_COUNT=0 (§16) et une NOUVELLE
   preuve de transparence OFF/ON sur le nouveau binaire (§30).
2. Un patch M1-M3 ecrit en fin de passe, sans build ni test, produirait un
   binaire NON VERIFIE. Le protocole exige un recheck de hash et une
   transparence re-mesuree.
3. Surtout : un ledger PRESENT MAIS FAUX est pire que son absence. Run1 a
   demontre que l absence est detectable ; un ledger faux ne le serait pas.
```

Je préfère te rendre un audit exact et une spécification précise qu'un Run2 dont l'instrumentation
serait à moitié correcte.

## 6 — Rapport final (§40)

```
ENGINE007B_F1A_STATUS=BLOCKED_INSTRUMENTATION_NOT_APPLIED
Q0_FORWARD_PROTOCOL_VERSION=v1
Q0_FORWARD_PROTOCOL_SHA256=9a467c3a3262709c80dafe4d4855d1477627e7f761498d434c8587d7b2a81a1b
Q0FWD_TOKEN_TRAJECTORY_SHA256=dc7b55dc5e87f7fcc50d2b2af15878cad7d6e5361b8a01836b10a248e7a21096
RUN1_STATUS=BLOCKED_EVENT_LEDGER
RUN1_PAYLOADS_REUSED_FOR_RUN2=NO
RUN2_INSTRUMENTATION_PATCH_SHA256=NON_APPLIQUE
RUN2_BINARY_SHA256=NON_PRODUIT
PRODUCT_COMPUTE_SOURCE_DIFF_COUNT=NON_MESURE
PROTOCOL_SHA256_RECHECK=NON_REFET
TRAJECTORY_SHA256_RECHECK=NON_REFET
MODEL_SHA256_RECHECK=NON_REFET
SENTINEL_SELECTION_SHA256_RECHECK=NON_REFET
Q0FWD_CAPTURE_OFF_INERTNESS=NON_EXECUTE
DECODE_BEGIN_COUNT=NON_MESURE
DECODE_END_COUNT=NON_MESURE
DECODE_STEP_SET=NON_MESURE
OBSERVED_COMPUTE_SUBMISSION_COUNT=NON_MESURE
REQUIRED_SCIENTIFIC_SUBMISSION_COUNT=NON_MESURE
SUBMISSION_WITH_UNKNOWN_DECODE_STEP_COUNT=NON_MESURE
COMPUTE_SUBMISSION_ID_DUPLICATE_OCCURRENCE_COUNT=NON_MESURE
SUBMISSION_DECODE_STEP_MAPPING=NON_ETABLI
CAPTURE_EPOCH_SUBMISSION_MAPPING=NON_ETABLI
TOTAL_REQUIRED_TARGET_SAMPLES=NON_MESURE
TOTAL_CAPTURED_TARGET_SAMPLES=NON_MESURE
SUBMITTED_TARGET_CAPTURE_COMPLETENESS=NON_MESURE
PAYLOAD_PROVENANCE_COMPLETE=NON_MESURE
MISSING_PAYLOAD_COUNT=NON_MESURE
SHA256_MISMATCH_COUNT=NON_MESURE
DIMENSION_MISMATCH_COUNT=NON_MESURE
NONFINITE_PAYLOAD_COUNT=NON_MESURE
ZERO_INPUT_NONZERO_OUTPUT_CONTRADICTION_COUNT=NON_MESURE
CAPTURE_CONTENT_VALIDITY=NON_MESURE
FULLY_PAIRED_TARGET_SAMPLE_COUNT=NON_MESURE
AMBIGUOUS_PAIR_COUNT=NON_MESURE
MISMATCHED_PAIR_COUNT=NON_MESURE
REFERENCE_SAMPLE_TEMPORAL_PAIRING=PARTIAL
LOGIT_BITWISE_MISMATCH_COUNT=NON_MESURE
Q0FWD_CAPTURE_GRAFT_NUMERICALLY_TRANSPARENT=NON_MESURE_POUR_RUN2
INDEPENDENT_ROW_OUTPUT_INDEXING_TEST=PASS
ROLE_COVERAGE_COUNT=6_SELECTION_FIGEE
Q0FWD_SENTINEL_TARGET_SAMPLE_COUNT=0
Q0FWD_SENTINEL_FAILED_SAMPLE_COUNT=0
Q0FWD_SENTINEL_SUBSET_VALIDATION=NOT_EXECUTED
TOTAL_NUMERICALLY_RECONSTRUCTED_TARGET_SAMPLES=0
FAILED_TARGET_SAMPLE_COUNT=0
Q0FWD_FULL_NATIVE_REFERENCE_VALIDATION=NOT_EXECUTED
Q0_FORWARD_STATUS=BLOCKED
Q1_FORWARD_ALLOWED=NO
FIRST_BLOCKING_GATE=INSTRUMENTATION_NOT_APPLIED
FIRST_FAILURE=NONE
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
Q0_PREDECLARED_STATUS=NOT_EVALUABLE_NO_PREDECLARED_PROTOCOL
Q0_HISTORICAL_STATUS=NOT_PASSED
Q1_HISTORICAL_ALLOWED=NO
PRODUCT_MMVQ_SOURCE_MODIFIED=NO
MODEL_GGUF_MODIFIED=NO
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
ENGINE007_NEXT_STEP=APPLY_M1_M3_PATCH_REBUILD_THEN_RUN2_OFF_ON
```

## 7 — Livrables réellement créés

```
RUN2_EVENT_INSTRUMENTATION_SOURCE_AUDIT.txt   112 lignes, les 6 points A-F verifies
RUN2_PATCH_NOT_APPLIED.txt                    specification M1-M3 + pourquoi je m arrete
Q0FWD_RUN2_STATUS.txt                         synthese et ledger
```

**Non créés** : `RUN2_INSTRUMENTATION_PATCH.diff`, `RUN2_INSTRUMENTATION_PATCH.sha256`,
`Q0FWD_RUN2_PREFLIGHT.txt`, `Q0FWD_RUN2_OFF_CONFIG.txt`, `Q0FWD_RUN2_OFF_LOG.txt`,
`Q0FWD_RUN2_OFF_LOGITS.bin`, `Q0FWD_RUN2_OFF_EVENT_LEDGER.csv`, `Q0FWD_RUN2_ON_CONFIG.txt`,
`Q0FWD_RUN2_ON_LOG.txt`, `Q0FWD_RUN2_ON_LOGITS.bin`, `Q0FWD_RUN2_EVENT_LEDGER.csv`,
`Q0FWD_RUN2_SUBMISSION_CLASSIFICATION.csv`, `Q0FWD_RUN2_CAPTURE_EPOCH_MAPPING.csv`,
`Q0FWD_RUN2_GRAPH_CAPTURE_MANIFEST.csv`, `Q0FWD_RUN2_PAYLOAD_PROVENANCE.csv`,
`Q0FWD_RUN2_CAPTURE_COMPLETENESS.csv`, `Q0FWD_RUN2_TEMPORAL_PAIRING.csv`,
`Q0FWD_RUN2_LOGIT_BITWISE_COMPARISON.csv`, `Q0FWD_RUN2_SENTINEL_VALIDATION.csv`,
`Q0FWD_RUN2_FULL_NATIVE_REFERENCE_VALIDATION.csv`.

Run1 est **intact** : ses 8 fichiers `.txt`, ses logits et ses 672 payloads n'ont pas été touchés.

## 8 — Le patch M1-M3, prêt à appliquer

```
M1  src/f1a-capture.h    + void f1a_set_decode_step(int s); + int f1a_decode_step(void);
    src/f1a-capture.cpp  + deux fonctions, valeur COPIEe dans l enregistrement de cible
    tools/f1a-eval.cpp   + f1a_set_decode_step(s) en tete de boucle (l.77)
M2  src/f1a-capture.cpp  + colonne decode_step dans le manifeste (l.425+) et dans
                           PAYLOAD_PROVENANCE (l.614), lue depuis la cible memorisee
M3  src/f1a-capture.cpp  + ledger a ecriture immediate (fflush a chaque ligne) avec
                           event_sequence_id monotone et les 8 EVENT_KINDS
```

Le point délicat est traité par conception : les événements de capture lisent le `decode_step`
**mémorisé dans la cible**, jamais le compteur courant.
