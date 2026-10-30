# ENGINE-007B-F1A-Q0FWD-v1-RUN2C — AUDIT ET DESIGN CORRIGÉ

Date : 2026-09-30. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../qualification/e7b-f1a-q0-forward-v1/run2/`

---

## 0 — Résultat

**Le point décisif du brief (§7) est tranché : `submission_id_at_build` n'identifie PAS l'occurrence
de soumission. Et la bonne source était déjà dans le fork.**

```
EPOCH_SUBMISSION_ASSOCIATION_DESIGN=PROVEN_UNAMBIGUOUS
SUBMISSION_ID_AT_BUILD_IS_SCIENTIFIC=NO_PROVEN
EXISTING_SUBMISSION_COUNTER_REUSED=YES
```

```
Q0_FORWARD_STATUS=BLOCKED
FIRST_BLOCKING_GATE=INSTRUMENTATION_VERIFICATION
FAILED_PATCH_APPLIED=NO        PATCH_APPLICATION_ALLOWED=NO
PLUMBING_FAILURE_29_STATUS=OPEN
```

Fork toujours intact — `git status` vérifié.

## 1 — Les trois instants où `f1a_ctr_submission` est lu

C'est tout le problème, et il fallait les séparer :

```
f1a-capture.cpp:128   f1a_ctr_submission++;
   DANS f1a_mark_compute_submission, appele depuis llama-context.cpp:2955,
   APRES ggml_backend_sched_graph_compute_async (l.2951).
   -> le hook autoritaire de soumission.

f1a-capture.cpp:348   r.submission_id_at_build = f1a_ctr_submission;
   DANS f1a_capture_note, appele depuis llama-graph.cpp:1534, c est-a-dire
   pendant la CONSTRUCTION du graphe du pas s.
   -> a cet instant la soumission du pas s n a PAS encore eu lieu.

f1a-capture.cpp:616   fprintf(pv, "...", r.epoch, f1a_ctr_submission, ...)
   DANS f1a_capture_end_epoch, APRES ggml_backend_synchronize (l.430).
   -> le compteur vaut le nombre de soumissions DEJA effectuees.
```

## 2 — La réponse aux trois questions du §7

```
WHAT_COUNTER_DOES_IT_CAPTURE =
  f1a_ctr_submission : compteur MONOTONE GLOBAL, incremente une seule fois par
  vraie soumission, au hook autoritaire. Occurrence-based : deux invocations
  distinctes recoivent deux IDs distincts, meme avec le meme gf.

WHEN_IS_IT_SAMPLED =
  TROIS instants distincts (l.128, l.348, l.616). C est la source de l ambiguite.

CAN_IT_IDENTIFY_THE_FUTURE_SUBMISSION_OCCURRENCE=NO   pour l.348
```

**Preuve indépendante, dans les données Run1 elles-mêmes :**

```
epoch,submissions_total,max_submission_at_build,classification
0,    1,                0,                     BUILD_ONLY_NOT_SUBMITTED
1,    2,                1,                     SUBMITTED_FOR_COMPUTE
```

`max_submission_at_build` = `submissions_total − 1` **aux deux époques**. Et le fork **nomme
lui-même** ce champ `max_submission_AT_BUILD` — le décalage est **structurel**, pas accidentel.

Le décalage est constant ici, mais je ne le déclare pas constant en général : il dépend du nombre
d'appels à `f1a_mark_compute_submission` survenus entre la construction et la fin d'époque. Si ce
nombre n'est pas toujours 1, l'égalité « réel = at_build + 1 » est fausse.

```
-> a NE PAS utiliser comme identifiant scientifique. Le brief avait raison.
```

## 3 — La bonne source était déjà dans le fork

`f1a_ctr_submission` lu **dans `f1a_capture_end_epoch`** (l.616, l.622) est la bonne occurrence, et
**le fork s'en sert déjà** :

```
l.607   nommage des payloads : "ep%llu_sub%llu_t%d_c%d_%s.bin"   (verifie : ep0_sub1_...)
l.622   colonne submission_id de PAYLOAD_PROVENANCE.csv          (verifie : epoch 0 -> 1)
```

Le fork a même un commentaire qui **prévient du piège** :

> « R5-d/D1 : identifiant de la soumission QUI VIENT DE SE TERMINER, et non celui du moment de la
> construction (risque de décalage de un). »

Mon audit a retrouvé exactement le même piège — par la source, et confirmé par les données.

## 4 — Preuve d'unicité de l'association époque ↔ soumission

```
1. UNE epoque est creee par execution de graphe (commentaire l.204) et finalisee
   par f1a_capture_end_epoch.
2. f1a_capture_end_epoch (llama-context.cpp:1784) s execute APRES la soumission
   (l.2951/2955) et APRES ggml_backend_synchronize (l.430).
3. f1a-eval est MONO-THREAD : la boucle l.77 n avance qu apres le retour de
   llama_decode, qui a lui-meme declenche soumission + fin d epoque.
4. Donc a l instant de l.616, f1a_ctr_submission est FIGE et vaut l ID de la
   soumission qui vient de se terminer. Une seule valeur possible.
```

```
EPOCH_SUBMISSION_ASSOCIATION_DESIGN=PROVEN_UNAMBIGUOUS
```

**Sans aucun recours au `gf`.** Le piège que tu redoutais (deux soumissions partageant le même `gf`)
est **déjà traité** par le fork : les noms de fichiers distinguent la soumission, et le commentaire
R6-2 le dit.

## 5 — Design corrigé de M1–M3

```
M1a  f1a-capture.h:23  + int32_t decode_step_latched = -1;  dans f1a_target_rec
     pose dans f1a_capture_note depuis le contexte de pas courant
M1b  f1a-capture.cpp   + static int32_t g_f1a_decode_step = -1;
                       + f1a_set_decode_step / f1a_get_decode_step
     tools/f1a-eval.cpp : f1a_set_decode_step(-1) avant le prefill (l.74),
                          f1a_set_decode_step(s) en tete de boucle (l.77)

M2a  f1a_capture_end_epoch : lire f1a_ctr_submission A L ENTREE
       const unsigned long long sub_at_end = f1a_ctr_submission;
     -> on REUTILISE la valeur qui sert deja a nommer les payloads (l.607)
M2b  MANIFESTE : + decode_step, compute_submission_id  (header l.451 ET lignes)
M2c  PAYLOAD_PROVENANCE : + run_id, decode_step, capture_epoch_id
     (submission_id y est DEJA, en 2e colonne)
M3   + f1a_event_current(kind, submission_id, gf, ep, eligible, captured)
     + f1a_event_latched(kind, decode_step, submission_id, epoch_id, ...)
```

**Le sentinel `-1` n'a plus deux sens** : `f1a_event_latched` exige un pas **explicite**, et `-1` y
signifie légitimement « hors pas scientifique ». C'est l'option recommandée de ton §4.

## 6 — Un piège de schéma que j'ai relevé

Le manifeste est ouvert en mode `"a"` dès la 2ᵉ époque, avec un header écrit **une seule fois**
(`s_manifest_header_done`). Un changement de schéma doit donc être cohérent entre les deux époques
du même run. Dans un dossier neuf le fichier est créé en `"w"`, donc c'est sûr — mais le smoke devra
vérifier `HEADER_COLUMN_COUNT == EVERY_ROW_COLUMN_COUNT` sur **les deux** époques.

## 7 — Rapport final (§43)

```
ENGINE007B_F1A_STATUS=BLOCKED_INSTRUMENTATION_VERIFICATION
Q0_FORWARD_STATUS=BLOCKED
Q1_FORWARD_ALLOWED=NO
FIRST_BLOCKING_GATE=INSTRUMENTATION_VERIFICATION
FIRST_FAILURE=NONE
PLUMBING_FAILURE_COUNT=29
PLUMBING_FAILURE_29=Q0FWD_TEMPORAL_PROVENANCE_CONTEXT_NOT_LATCHED_AND_RUNTIME_EVENT_LEDGER_MISSING
PLUMBING_FAILURE_29_STATUS=OPEN
FAILED_PATCH_APPLIED=NO
PATCH_APPLICATION_ALLOWED=NO
PATCH_REFERENCES_NONEXISTENT_PREPATCH_SYMBOL_COUNT=NOT_MEASURED_CANDIDATE_NOT_PRODUCED
EVENT_API_SENTINEL_AMBIGUITY=NONE_BY_DESIGN (f1a_event_current / f1a_event_latched)
EPOCH_SUBMISSION_ASSOCIATION_DESIGN=PROVEN_UNAMBIGUOUS
M1_DECODE_STEP_CONTEXT_IMPLEMENTED=NO
DECODE_STEP_LATCHED_AT_CAPTURE_NOTE=NO
EXISTING_SUBMISSION_COUNTER_REUSED=YES
M3_RUNTIME_LEDGER_IMPLEMENTED=NO
RUN2_INSTRUMENTATION_PATCH_SHA256=NON_APPLIQUE
RUN2_BINARY_SHA256=NON_PRODUIT
PRODUCT_COMPUTE_SOURCE_DIFF_COUNT=NON_MESURE
SMOKE_LEDGER_PARSE=NOT_EXECUTED
SMOKE_EVENT_SEQUENCE_DUPLICATE_COUNT=NOT_EXECUTED
SMOKE_SCIENTIFIC_EVENT_WITH_UNKNOWN_STEP_COUNT=NOT_EXECUTED
SMOKE_TARGET_SUBMISSION_STEP_MISMATCH_COUNT=NOT_EXECUTED
RUN2_QUALIFYING_EXECUTION_ALLOWED=NO
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
PREFILL_CLASSIFIED_AS_SCIENTIFIC_STEP=NON_MESURE
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
REFERENCE_SAMPLE_TEMPORAL_PAIRING=PARTIAL
SUBMITTED_TARGET_CAPTURE_COMPLETENESS=NON_MESURE
PAYLOAD_PROVENANCE_COMPLETE=NON_MESURE
CAPTURE_CONTENT_VALIDITY=NON_MESURE
LOGIT_BITWISE_MISMATCH_COUNT=NON_MESURE
Q0FWD_CAPTURE_GRAFT_NUMERICALLY_TRANSPARENT=NON_MESURE_POUR_RUN2
ROLE_COVERAGE_COUNT=6_SELECTION_FIGEE
Q0FWD_SENTINEL_FAILED_SAMPLE_COUNT=0
Q0FWD_SENTINEL_SUBSET_VALIDATION=NOT_EXECUTED
TOTAL_REQUIRED_TARGET_SAMPLES=NON_MESURE
TOTAL_CAPTURED_TARGET_SAMPLES=NON_MESURE
TOTAL_NUMERICALLY_RECONSTRUCTED_TARGET_SAMPLES=0
FAILED_TARGET_SAMPLE_COUNT=0
Q0FWD_FULL_NATIVE_REFERENCE_VALIDATION=NOT_EXECUTED
PRODUCT_MMVQ_SOURCE_MODIFIED=NO
MODEL_GGUF_MODIFIED=NO
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
ENGINE007_NEXT_STEP=PRODUCE_AND_STATICALLY_VERIFY_PATCH_CANDIDATE
```

## 8 — Livrables réellement créés

```
RUN2C_SUBMISSION_ID_AT_BUILD_AUDIT.txt   129 lignes : les 3 instants, les 3 reponses,
                                         la preuve d unicite, le design corrige M1-M3
Q0FWD_RUN2_STATUS.txt                    synthese et ledger
```

**Non créés** : `RUN2C_PATCH_PREAPPLICATION_VERIFICATION.txt`,
`RUN2C_INSTRUMENTATION_PATCH_CANDIDATE.diff`, `RUN2C_INSTRUMENTATION_PATCH.diff`,
`RUN2C_INSTRUMENTATION_PATCH.sha256`, `RUN2_BUILD_PROVENANCE.txt`, `run2/smoke/*`,
`Q0FWD_RUN2_PREFLIGHT.txt`, `Q0FWD_RUN2_OFF_EVENT_LEDGER.csv`, `Q0FWD_RUN2_OFF_LOGITS.bin`,
`Q0FWD_RUN2_EVENT_LEDGER.csv`, `Q0FWD_RUN2_ON_LOGITS.bin`,
`Q0FWD_RUN2_SUBMISSION_CLASSIFICATION.csv`, `Q0FWD_RUN2_CAPTURE_EPOCH_MAPPING.csv`,
`Q0FWD_RUN2_GRAPH_CAPTURE_MANIFEST.csv`, `Q0FWD_RUN2_PAYLOAD_PROVENANCE.csv`,
`Q0FWD_RUN2_TEMPORAL_PAIRING.csv`, `Q0FWD_RUN2_CAPTURE_COMPLETENESS.csv`,
`Q0FWD_RUN2_LOGIT_BITWISE_COMPARISON.csv`, `Q0FWD_RUN2_SENTINEL_VALIDATION.csv`,
`Q0FWD_RUN2_FULL_NATIVE_REFERENCE_VALIDATION.csv`.

## 9 — Ce qui reste, dans l'ordre de ton §44

```
1. produire RUN2C_INSTRUMENTATION_PATCH_CANDIDATE.diff
2. verifier CHAQUE symbole nouveau contre la source reelle
   -> PATCH_REFERENCES_NONEXISTENT_PREPATCH_SYMBOL_COUNT doit valoir 0
3. verifier manifest header + rows ET provenance header + rows
4. appliquer seulement alors
5. compile ciblee, full rebuild, PRODUCT_COMPUTE_SOURCE_DIFF_COUNT=0
6. smoke non qualificatif
7. Run2 OFF, Run2 ON
8. ledger / mapping / pairing / completeness / transparence
9. fermer PLUMBING_FAILURE_29 seulement a ce moment
10. sentinelles, puis exhaustif
```

Le design est corrigé et l'association est **prouvée**. Le patch n'est pas appliqué : produire le
candidat, vérifier ses symboles et compiler en cible sont exactement les étapes où le patch
précédent a échoué. Je ne les bâcle pas.
