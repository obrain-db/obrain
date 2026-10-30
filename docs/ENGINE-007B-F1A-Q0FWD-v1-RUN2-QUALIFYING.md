# ENGINE-007B-F1A-Q0FWD-v1 — RUN2 QUALIFIANT

Date : 2026-09-30. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../qualification/e7b-f1a-q0-forward-v1/run2/{off,on}/`

---

## 0 — Où en est le Run2

**Tous les gates de provenance, de complétude, de contenu et de transparence passent.
Les 30 sentinelles passent. L'exhaustif est en cours.**

```
Q0_FORWARD_STATUS=BLOCKED
FIRST_BLOCKING_GATE=FULL_NATIVE_REFERENCE_VALIDATION_IN_PROGRESS
Q1_FORWARD_ALLOWED=NO
```

Je ne déclare pas PASS : le §39 l'exige seulement si `Q0FWD_FULL_NATIVE_REFERENCE_VALIDATION=PASS`,
et cette mesure n'est pas terminée.

## 1 — Préflight immuable

```
PROTOCOL_SHA256_RECHECK=PASS
TRAJECTORY_SHA256_RECHECK=PASS
HISTORICAL_SOURCE_TOKEN_SHA256_RECHECK=PASS
MODEL_SHA256_RECHECK=PASS
PREDECLARED_SENTINEL_SELECTION_SHA256_RECHECK=PASS
RUNTIME_RETOKENIZATION=NO
   sha256(tokens runtime)=dc8d54a9...f44b  = hash historique exact
RUN2_F1A_EVAL_SHA256=79ba1c44...b80a
RUN2_LIBLLAMA_SHA256=87c42d5f...b2b2
RUNTIME_LIBLLAMA_MATCHES_INTENDED_RUN2_BUILD=YES
```

## 2 — OFF, ON, et la transparence

```
RUN2_OFF  RC=0  9 pas  8939536 octets  sha 53aa87fb70d101c7e98525e148f8c97b8441112a6728dacc28312c2dd5df0dd5
RUN2_ON   RC=0  9 pas  8939536 octets  sha 53aa87fb70d101c7e98525e148f8c97b8441112a6728dacc28312c2dd5df0dd5

LOGIT_BITWISE_MISMATCH_COUNT=0
Q0FWD_CAPTURE_GRAFT_NUMERICALLY_TRANSPARENT=YES
OFF_CAPTURE_ACTIVITY_COUNT=0
RUN1_PAYLOADS_REUSED_FOR_RUN2=NO   SMOKE_PAYLOADS_REUSED_FOR_RUN2=NO
```

Les deux fichiers sont **identiques au bit près**, et identiques aussi aux logits Run1 — cohérent avec un
modèle et une trajectoire déterministes.

## 3 — Provenance temporelle (tous PASS)

```
ON_LEDGER_PARSE=PASS          EVENT_LEDGER_IO_FAILURE_COUNT=0
EVENT_SEQUENCE_ID_DUPLICATE_COUNT=0   EVENT_SEQUENCE_GAP_COUNT=0   EVENT_SEQUENCE_MONOTONIC=YES

PREFILL_DECODE_STEP=-1        PREFILL_CLASSIFIED_AS_SCIENTIFIC_STEP=NO
SCIENTIFIC_DECODE_STEP_SET=0,1,2,3,4,5,6,7,8    UNKNOWN=0
DECODE_BEGIN_COUNT=9  LOGITS_READ_COUNT=9  DECODE_END_COUNT=9

TOTAL_COMPUTE_SUBMIT_COUNT=10   PREFILL=1   SCIENTIFIC=9
COMPUTE_SUBMISSION_ID_DUPLICATE_OCCURRENCE_COUNT=0

SYNC_COMPLETE_MISSING_EPOCH_COUNT=0   DUPLICATE=0   BEFORE_SUBMIT=0
CAPTURE_EVENT_STEP/SUBMISSION/EPOCH_MISMATCH_COUNT = 0, 0, 0
CAPTURE_EPOCH_ID_ZERO_RENDERED_AS_NA_COUNT=0
DECODE_STEP_MINUS_ONE_LEDGER/MANIFEST/PROVENANCE = -1 / -1 / -1
DECODE_STEP_SERIALIZATION_MISMATCH_COUNT=0
AMBIGUOUS_EPOCH_SUBMISSION_MAPPING_COUNT=0   TARGET_SUBMISSION_STEP_MISMATCH_COUNT=0

RUN2_MANIFEST_SCHEMA=PASS          RUN2_PAYLOAD_PROVENANCE_SCHEMA=PASS
LEDGER_MANIFEST_MISMATCH_COUNT=0   LEDGER_PAYLOAD_PROVENANCE_MISMATCH_COUNT=0
AMBIGUOUS_PAIR_COUNT=0   MISMATCHED_PAIR_COUNT=0   REFERENCE_SAMPLE_TEMPORAL_PAIRING=COMPLETE
```

**La jointure directe fonctionne**, sans normalisation : les trois artefacts partagent la même
représentation de `decode_step`, y compris `-1`.

`PLUMBING_FAILURE_29_STATUS=RESOLVED_VERIFIED_BY_RUN2_RUNTIME_EVENT_LEDGER` — fermé ici, comme le §22
l'autorise : les gates runtime de provenance Run2 passent.

## 4 — Complétude, contenu, sentinelles

```
MISSING_PAYLOAD_COUNT=0   SHA256_MISMATCH_COUNT=0
DIMENSION_MISMATCH_COUNT=0   NONFINITE_PAYLOAD_COUNT=0
SUBMITTED_TARGET_CAPTURE_COMPLETENESS=PASS   PAYLOAD_PROVENANCE_COMPLETE=YES
ZERO_INPUT_NONZERO_OUTPUT_CONTRADICTION_COUNT=0   CAPTURE_CONTENT_VALIDITY=PASS

ROLE_COVERAGE_COUNT=6   TOTAL_SENTINEL_TENSORS=30
SENTINEL_REQUIRED_SAMPLE_COUNT=60   CAPTURED=60   RECONSTRUCTED=60   FAILED=0
Q0FWD_SENTINEL_SUBSET_VALIDATION=PASS
```

60 échantillons = 30 tenseurs × **2 époques**, les deux effectivement présentes. `REL_L2` entre
**1.2871e-07** et **1.6026e-07**, `COSINE ≥ 0.99999974`. Budget `REL_L2 ≤ 1e-3` /
`COSINE ≥ 0.9999` / `MAX_ABS ≤ 1e-2·max_abs` tenu sur chaque échantillon individuellement.

## 5 — Deux choses que je dois te dire

**1. Le ledger OFF n'existe pas.**

`f1a_event_row` retourne tôt si `!g_f1a.enabled`, et le flag capture est OFF. **Aucun événement n'est
écrit en mode OFF.** Les compteurs temporels OFF du §8 ne peuvent donc pas être évalués.

C'est une conséquence de conception, pas une mesure manquante. Les deux objectifs du run OFF —
inertie prouvée, logits produits pour la comparaison bitwise — sont satisfaits. Mais si le protocole
exige un ledger OFF, il faut sortir le ledger du garde `g_f1a.enabled`, donc **un nouveau run OFF**.

**2. Mon audit avait oublié `CAPTURE_CONTENT_VALIDITY`.**

Mon premier script Run2 ne le calculait pas, alors que le §25 l'exige. C'est la même classe de défaut
que SMOKE2 — un gate absent du verdict — sauf qu'ici il n'était même pas calculé. **Corrigé et calculé
dans cette passe** avec le détecteur validé par le contrôle négatif de la clôture du défaut 56.

## 6 — Exhaustif en cours

```
FULL_REQUIRED_SAMPLE_COUNT=336_ESTIME   (168 cibles x 2 epoques)
FULL_RECONSTRUCTED_SAMPLE_COUNT=IN_PROGRESS
Q0FWD_FULL_NATIVE_REFERENCE_VALIDATION=NOT_EXECUTED_IN_PROGRESS
```

Lancé en tâche détachée (`nohup`). Durée estimée ~2,4 h au rythme mesuré (60 échantillons en 1583 s).
**Je ne le déclare pas avant son résultat.**

## 7 — Ce que le Run2 répond, dans l'ordre exigé

1. **Les artefacts appartiennent-ils sans ambiguïté au bon step, à la bonne occurrence de soumission
   et à la bonne epoch ?** — **OUI.** Prouvé par le ledger runtime, la jointure directe des trois
   artefacts, et `REFERENCE_SAMPLE_TEMPORAL_PAIRING=COMPLETE`.
2. **L'instrumentation est-elle transparente au calcul produit ?** — **OUI.** OFF et ON sont
   bit-identiques, et `PRODUCT_COMPUTE_SOURCE_DIFF_COUNT=0`.
3. **Les sorties natives respectent-elles le budget pré-déclaré ?** — **PARTIELLEMENT PROUVÉ** :
   60/60 sentinelles sur les 6 rôles et 2 époques, décision exhaustive en cours.

`LOCAL_CORRECTNESS != WORKFLOW_CORRECTNESS != MEASUREMENT_VALIDITY` — les trois niveaux ont été
traités séparément, dans cet ordre.

## 8 — Historique immuable

```
Q0_PREDECLARED_PROTOCOL_FOUND=NO
Q0_PREDECLARED_STATUS=NOT_EVALUABLE_NO_PREDECLARED_PROTOCOL
Q0_HISTORICAL_STATUS=NOT_PASSED
Q1_HISTORICAL_ALLOWED=NO
```

Rien de tout cela ne sera réécrit, même si le Run2 passe.
