# ENGINE-007B-F1A-Q0FWD-v1-RUN2B — APPLICATION M1–M3

Date : 2026-09-30. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../qualification/e7b-f1a-q0-forward-v1/run2/`

---

## 0 — Résultat

**J'ai écrit le patch, puis je l'ai vérifié avant de l'appliquer. La vérification l'a invalidé.**

```
Q0_FORWARD_STATUS=BLOCKED
FIRST_BLOCKING_GATE=INSTRUMENTATION_VERIFICATION
FIRST_FAILURE=NONE
Q1_FORWARD_ALLOWED=NO
```

Aucun patch appliqué, aucun build, aucun run OFF/ON. **Aucun fichier du fork n'a été modifié** —
vérifié par `git status`.

```
PLUMBING_FAILURE_COUNT=29
PLUMBING_FAILURE_29=Q0FWD_TEMPORAL_PROVENANCE_CONTEXT_NOT_LATCHED_AND_RUNTIME_EVENT_LEDGER_MISSING
PLUMBING_FAILURE_29_STATUS=OPEN
```

## 1 — Ce que la vérification a démasqué

J'avais écrit un patch M1–M2 complet. Il référençait **deux choses qui n'existent pas** :

```
1. g_f1a.epoch_cur        -> N EXISTE PAS
   struct f1a_state n est PAS declaree dans f1a-capture.h ; elle est interne au .cpp.
   Ses champs reels : inited, enabled, inventory_path, outdir, run_id.
   Le champ epoch vit dans f1a_capture_ctx (f1a-capture.h:27), pas dans f1a_state.

2. F1A_EVENT_LEDGER_NAME  -> N EXISTE PAS
   Les seules constantes sont les variables d environnement :
     f1a-capture.cpp:13  F1A_INVENTORY_ENV = "OBRAIN_F1A_INVENTORY"
     f1a-capture.cpp:14  F1A_OUTDIR_ENV    = "OBRAIN_F1A_OUTDIR"
     f1a-capture.cpp:15  F1A_ENABLE_ENV    = "OBRAIN_F1A_CAPTURE"
   Les noms de fichiers sont des litteraux inline.
```

**C'est la deuxième fois dans cette session que je trouve une erreur de ma part en vérifiant avant
d'agir, et non après.** C'est le bon ordre — et c'est précisément la discipline qui manquait dans
les passes où j'ai dû rétracter des conclusions.

## 2 — Corrections identifiées

```
C1. le ledger s appelle par un litteral : "/Q0FWD_RUN2_EVENT_LEDGER.csv"
C2. f1a_event NE DOIT PAS lire g_f1a.epoch_cur. Signature corrigee avec un
    parametre capture_epoch_id EXPLICITE -- l appelant le connait.
C3. f1a_event teste g_f1a.enabled ; legal car defini DANS le .cpp.
```

### Signature corrigée de `f1a_event`

```c
void f1a_event(const char * kind,
               int32_t decode_step_override,   // -1 = lire le contexte courant
               unsigned long long capture_epoch_id,
               unsigned long long submission_id,
               const void * graph_ptr,
               unsigned long long graph_fingerprint,
               int eligible_targets,
               int captured_targets);
```

`decode_step_override` est **nécessaire**, pas cosmétique : les événements `CAPTURE_*` surviennent à
`f1a_capture_end_epoch`, donc **hors du pas courant**. Ils doivent recevoir le pas **latché**. C'est
ton point 4, et la signature doit le rendre impossible à contourner.

## 3 — Ce qui était correct dans mon patch (vérifié)

```
V1. struct f1a_target_rec est bien dans f1a-capture.h:23 ; l ajout du champ
    decode_step_latched en fin de struct est valide.
V2. f1a_mark_output est bien a cet endroit du .cpp.
V3. f1a_capture_note contient bien la ligne r.submission_id_at_build = ... (ancre du latch).
V4. Le header du manifeste est bien la ligne fprintf(m, "run_id,epoch,...").
V5. g_f1a est declare ligne 30 du .cpp, accessible depuis le meme fichier.
```

## 4 — Rapport final (§47)

```
ENGINE007B_F1A_STATUS=BLOCKED_INSTRUMENTATION_VERIFICATION
Q0_FORWARD_PROTOCOL_VERSION=v1
RUN1_STATUS=BLOCKED_EVENT_LEDGER
RUN1_PAYLOADS_REUSED_FOR_RUN2=NO
PLUMBING_FAILURE_COUNT=29
PLUMBING_FAILURE_29=Q0FWD_TEMPORAL_PROVENANCE_CONTEXT_NOT_LATCHED_AND_RUNTIME_EVENT_LEDGER_MISSING
PLUMBING_FAILURE_29_STATUS=OPEN
M1_DECODE_STEP_CONTEXT_IMPLEMENTED=NO_PATCH_CORRIGE_NON_APPLIQUE
DECODE_STEP_LATCHED_AT_CAPTURE_NOTE=NO
EXISTING_SUBMISSION_COUNTER_REUSED=YES
M3_RUNTIME_LEDGER_IMPLEMENTED=NO
RUN2_INSTRUMENTATION_PATCH_SHA256=NON_APPLIQUE
RUN2_BINARY_SHA256=NON_PRODUIT
PRODUCT_COMPUTE_SOURCE_DIFF_COUNT=NON_MESURE
PROTOCOL_SHA256_RECHECK=NON_REFET
TRAJECTORY_SHA256_RECHECK=NON_REFET
HISTORICAL_SOURCE_TOKEN_SHA256_RECHECK=NON_REFET
MODEL_SHA256_RECHECK=NON_REFET
SENTINEL_SELECTION_SHA256_RECHECK=NON_REFET
Q0FWD_CAPTURE_OFF_INERTNESS=NON_EXECUTE
DECODE_BEGIN_COUNT=NON_MESURE
LOGITS_READ_COUNT=NON_MESURE
DECODE_END_COUNT=NON_MESURE
DECODE_STEP_SET=NON_MESURE
OBSERVED_COMPUTE_SUBMISSION_COUNT=NON_MESURE
REQUIRED_SCIENTIFIC_SUBMISSION_COUNT=NON_MESURE
SUBMISSION_WITH_UNKNOWN_DECODE_STEP_COUNT=NON_MESURE
COMPUTE_SUBMISSION_ID_DUPLICATE_OCCURRENCE_COUNT=NON_MESURE
TARGET_SUBMISSION_STEP_MISMATCH_COUNT=NON_MESURE
SUBMISSION_DECODE_STEP_MAPPING=NON_ETABLI
AMBIGUOUS_EPOCH_SUBMISSION_MAPPING_COUNT=NON_MESURE
CAPTURE_EPOCH_SUBMISSION_MAPPING=NON_ETABLI
LEDGER_MANIFEST_MISMATCH_COUNT=NON_MESURE
LEDGER_PAYLOAD_PROVENANCE_MISMATCH_COUNT=NON_MESURE
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
FIRST_BLOCKING_GATE=INSTRUMENTATION_VERIFICATION
FIRST_FAILURE=NONE
PRODUCT_MMVQ_SOURCE_MODIFIED=NO
MODEL_GGUF_MODIFIED=NO
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
ENGINE007_NEXT_STEP=APPLY_CORRECTED_M1_M3_THEN_REBUILD_AND_RUN2_OFF_ON
```

## 5 — Livrables réellement créés

```
RUN2_EVENT_INSTRUMENTATION_SOURCE_AUDIT.txt   (passe precedente)
RUN2_PATCH_NOT_APPLIED.txt                    (passe precedente)
RUN2_INSTRUMENTATION_PATCH_DESIGN.txt         63 lignes : l erreur, les 3 corrections,
                                              la signature corrigee, les 5 points verifies
Q0FWD_RUN2_STATUS.txt                         synthese et ledger
```

**Non créés** : `RUN2_INSTRUMENTATION_PATCH.diff`, `RUN2_INSTRUMENTATION_PATCH.sha256`,
`RUN2_BUILD_PROVENANCE.txt`, `RUN2_SMOKE_STATUS.txt`, `Q0FWD_RUN2_PREFLIGHT.txt`,
`Q0FWD_RUN2_OFF_LOG.txt`, `Q0FWD_RUN2_OFF_LOGITS.bin`, `Q0FWD_RUN2_OFF_EVENT_LEDGER.csv`,
`Q0FWD_RUN2_ON_LOG.txt`, `Q0FWD_RUN2_ON_LOGITS.bin`, `Q0FWD_RUN2_EVENT_LEDGER.csv`,
`Q0FWD_RUN2_SUBMISSION_CLASSIFICATION.csv`, `Q0FWD_RUN2_CAPTURE_EPOCH_MAPPING.csv`,
`Q0FWD_RUN2_GRAPH_CAPTURE_MANIFEST.csv`, `Q0FWD_RUN2_PAYLOAD_PROVENANCE.csv`,
`Q0FWD_RUN2_CAPTURE_COMPLETENESS.csv`, `Q0FWD_RUN2_TEMPORAL_PAIRING.csv`,
`Q0FWD_RUN2_LOGIT_BITWISE_COMPARISON.csv`, `Q0FWD_RUN2_SENTINEL_VALIDATION.csv`,
`Q0FWD_RUN2_FULL_NATIVE_REFERENCE_VALIDATION.csv`.

## 6 — Ce qui reste à faire, dans l'ordre

```
1. appliquer la signature CORRIGEE de f1a_event (C1, C2, C3)
2. cabler les 8 appels d evenements :
     f1a-eval.cpp            f1a_set_decode_step(-1) avant le prefill
                             f1a_set_decode_step(s) en tete de boucle (l.77)
                             DECODE_BEGIN / LOGITS_READ / DECODE_END par pas
     llama-context.cpp:2955  COMPUTE_SUBMIT avec le pas courant
     f1a-capture.cpp:425     CAPTURE_BEGIN / CAPTURE_EXTRACT / CAPTURE_END
                             avec le pas LATCHE de l epoque
3. completer les LIGNES du manifeste (le header est deja prevu par mon patch)
4. ajouter les 4 colonnes a PAYLOAD_PROVENANCE (l.614-618)
5. rebuild, PRODUCT_COMPUTE_SOURCE_DIFF_COUNT=0, smoke, puis OFF/ON
```

Le patch est petit et **entièrement spécifié**. Les deux erreurs qui l'ont bloqué sont nommées, et
les corrections sont écrites.

## 7 — Note sur le bookkeeping

`PLUMBING_FAILURE_29` est assigné et reste `OPEN`. Ton §44 dit de ne le fermer qu'après observation
runtime d'un ledger correct — ce qui n'a pas eu lieu. Je ne le ferme donc pas.

Je note aussi, comme tu l'as demandé au §1, que **le compteur de soumission n'est pas le défaut 29** :
il existe déjà et son positionnement source est qualifié.
