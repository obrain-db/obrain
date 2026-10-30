# ENGINE-007B-F1A-Q0FWD-v1 — QUALIFICATION PROSPECTIVE

Date : 2026-09-30. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7b-f1a-q0-forward-v1/`

---

## 0 — Résultat

**Le protocole prospectif est créé, hashé et gelé avant toute exécution. Le run qualificatif n'a pas
été lancé : sa trajectoire d'entrée n'est pas figée.**

```
Q0_FORWARD_PROTOCOL_VERSION=v1
Q0_FORWARD_PROTOCOL_SHA256=9a467c3a3262709c80dafe4d4855d1477627e7f761498d434c8587d7b2a81a1b
Q0_FORWARD_PROTOCOL_FROZEN=YES

Q0_FORWARD_STATUS=NOT_EXECUTED
Q1_FORWARD_ALLOWED=NO
```

## 1 — Le protocole est gelé (§2)

```
Q0FWD_PROTOCOL_v1.txt       171 lignes, 7601 octets
Q0FWD_PROTOCOL_v1.sha256    empreinte enregistree
GEL A                       2026-09-30 20:48:37 +0200 — AVANT toute execution
Q0FWD_PROTOCOL_GIT_COMMIT=NOT_AVAILABLE
```

Le chemin est hors dépôt (`/home/triviere/exp-artifacts/…`). La provenance repose donc sur le
**SHA256 et l'horodatage du fichier**, pas sur git.

Le protocole contient l'intégralité des 24 sections : statuts historiques figés et non
réinterprétables, règle de transition prospective explicitement déclarée **nouvelle et non
historique**, configuration gelée, contrôle OFF, ledger événementiel, **règle** de sélection des
soumissions scientifiques (la règle, pas le nombre observé), population de cibles, pairing temporel,
référence numérique figée, portée exhaustive, sentinelles, budget, FAIL vs BLOCKED, et immuabilité.

## 2 — Les statuts historiques sont figés (§0, §24)

```
Q0_PREDECLARED_PROTOCOL_FOUND=NO
Q0_PREDECLARED_PROTOCOL_PROVENANCE=FAIL
Q0_PREDECLARED_STATUS=NOT_EVALUABLE_NO_PREDECLARED_PROTOCOL
Q0_HISTORICAL_STATUS=NOT_PASSED
Q1_HISTORICAL_ALLOWED=NO
```

Je ne les convertirai pas en PASS. Ils décrivent une **absence de protocole**, pas un échec numérique.
Même si Q0FWD-v1 passe un jour, `Q0_PREDECLARED_STATUS` restera inchangé : je n'écrirai jamais
« historical Q0 passed ».

## 3 — Pourquoi je n'ai PAS lancé le run

Votre §3 l'exige avant exécution :

```
TEACHER_FORCED_INPUT_IDS=
TEACHER_FORCED_OUTPUT_IDS=
TOKEN_TRAJECTORY_SHA256=
```

```
TEACHER_FORCED_INPUT_IDS=NON_FIGE
TEACHER_FORCED_OUTPUT_IDS=NON_FIGE
TOKEN_TRAJECTORY_SHA256=NON_FIGE
```

La trajectoire teacher-forced de 9 pas utilisée par les passes précédentes **n'a pas été retrouvée
sous une forme figée et vérifiable**. Le fichier `r1/tokens-greedy.txt` existe (256 lignes), mais
**je n'ai pas établi qu'il s'agit de la trajectoire des 9 pas qualificatifs**.

Lancer OFF puis ON maintenant produirait des mesures dont **la trajectoire d'entrée n'est pas
figée**. Ces données seraient `NOT_SCIENTIFICALLY_QUALIFIED` au regard de Q0FWD-v1 — exactement le
statut que votre §25 réserve aux captures pré-`ggml_set_output`.

**Je ne les produis donc pas.** Le protocole lui-même interdit de rattraper cela après coup : il
exige que la trajectoire soit figée *avant*.

## 4 — Rapport final (§29)

```
ENGINE007B_F1A_STATUS=BLOCKED_Q0FWD_TRAJECTORY_NOT_FROZEN

Q0_PREDECLARED_PROTOCOL_FOUND=NO
Q0_PREDECLARED_STATUS=NOT_EVALUABLE_NO_PREDECLARED_PROTOCOL

Q0_FORWARD_PROTOCOL_VERSION=v1
Q0_FORWARD_PROTOCOL_SHA256=9a467c3a3262709c80dafe4d4855d1477627e7f761498d434c8587d7b2a81a1b
Q0_FORWARD_PROTOCOL_FROZEN=YES

Q0FWD_DECODE_STEPS=9
OBSERVED_COMPUTE_SUBMISSIONS=4_NON_RELIES
REQUIRED_SCIENTIFIC_SUBMISSIONS=NON_DEFINI_LEDGER_NON_PRODUIT
CAPTURED_SCIENTIFIC_SUBMISSIONS=NON_DEFINI
REFERENCE_SAMPLE_TEMPORAL_PAIRING=PARTIAL
Q0FWD_TEMPORAL_COVERAGE=NOT_EXECUTED

TOTAL_REQUIRED_TARGET_SAMPLES=NON_DEFINI
TOTAL_CAPTURED_TARGET_SAMPLES=NON_DEFINI
SUBMITTED_TARGET_CAPTURE_COMPLETENESS=NOT_EXECUTED
PAYLOAD_PROVENANCE_COMPLETE=NOT_EXECUTED
CAPTURE_CONTENT_VALIDITY=NOT_EXECUTED

Q0FWD_CAPTURE_OFF_INERTNESS=NOT_EXECUTED
LOGIT_BITWISE_MISMATCH_COUNT=NOT_EXECUTED
Q0FWD_CAPTURE_GRAFT_NUMERICALLY_TRANSPARENT=NOT_EXECUTED

INDEPENDENT_ROW_OUTPUT_INDEXING_TEST=PASS
ROLE_COVERAGE_COUNT=6
PREDECLARED_SELECTION_HASH_VERIFIED=YES
SHA256_SENTINELLES=448808715eea67d416be356d5f03e28b9675d15b05acb510e202a0f263697798
Q0FWD_SENTINEL_SUBSET_VALIDATION=NOT_EXECUTED

TOTAL_NUMERICALLY_RECONSTRUCTED_TARGET_SAMPLES=0
FAILED_TARGET_SAMPLE_COUNT=0
Q0FWD_FULL_NATIVE_REFERENCE_VALIDATION=NOT_EXECUTED

NATIVE_Q8_INTERMEDIATE_COMPARISON=SOURCE_QUALIFIED_NOT_DIRECTLY_OBSERVED

Q0_FORWARD_STATUS=NOT_EXECUTED
Q1_FORWARD_ALLOWED=NO

Q0_HISTORICAL_STATUS=NOT_PASSED
Q1_HISTORICAL_ALLOWED=NO

PRODUCT_MMVQ_SOURCE_MODIFIED=NO
MODEL_GGUF_MODIFIED=NO
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
ENGINE007_NEXT_STEP=FREEZE_TEACHER_FORCED_TRAJECTORY_THEN_RUN_Q0FWD_V1
```

## 5 — Livrables réellement créés

```
Q0FWD_PROTOCOL_v1.txt      le protocole complet, 171 lignes
Q0FWD_PROTOCOL_v1.sha256   son empreinte
Q0FWD_RUN_CONFIG.txt       configuration gelee + champs de trajectoire NON FIGES
Q0FWD_STATUS.txt           ce qui est fait, ce qui bloque, ledger
```

**Non créés** : `Q0FWD_TOKEN_TRAJECTORY.txt`, `Q0FWD_SOURCE_PROVENANCE.txt`,
`Q0FWD_BUILD_PROVENANCE.txt`, `Q0FWD_OFF_LOGITS.bin`, `Q0FWD_ON_LOGITS.bin`,
`Q0FWD_LOGIT_BITWISE_COMPARISON.txt`, `Q0FWD_EVENT_LEDGER.csv`,
`Q0FWD_SUBMISSION_CLASSIFICATION.csv`, `Q0FWD_DECODE_SUBMISSION_MAPPING.csv`,
`Q0FWD_TEMPORAL_PAIRING.csv`, `Q0FWD_TEMPORAL_COVERAGE.txt`, `Q0FWD_CAPTURE_COMPLETENESS.csv`,
`Q0FWD_PAYLOAD_PROVENANCE.csv`, `Q0FWD_SENTINEL_SELECTION.csv` (+ `.sha256`),
`Q0FWD_SENTINEL_VALIDATION.csv`, `Q0FWD_FULL_NATIVE_REFERENCE_VALIDATION.csv`,
`Q0FWD_ROLE_SUMMARY.csv`, `Q0FWD_SUBMISSION_SUMMARY.csv`, `Q0FWD_FIRST_FAILURE.txt`.

## 6 — Ce qu'il faut faire ensuite, précisément

**Une seule chose bloque le run : figer la trajectoire.**

```
1. retrouver la trajectoire teacher-forced des 9 pas utilisee par les passes N*
   (chercher dans le harnais e6c-r1 / f1a-eval : comment les 9 pas sont pilotes)
2. en extraire TEACHER_FORCED_INPUT_IDS et TEACHER_FORCED_OUTPUT_IDS
3. calculer TOKEN_TRAJECTORY_SHA256
4. ecrire Q0FWD_TOKEN_TRAJECTORY.txt
   -> a ce moment seulement, le run OFF/ON devient conforme au protocole gele
5. executer le controle OFF, puis ON avec ledger evenementiel
6. sentinelles, puis exhaustif, puis decision Q0_FORWARD_STATUS
```

**Si la trajectoire historique est irrécupérable**, le §3 du protocole autorise une trajectoire
**nouvelle et déterministe** — mais il faut alors la définir, l'enregistrer intégralement, hasher, et
**déclarer explicitement qu'il s'agit de la trajectoire prospective Q0FWD-v1**. Ce n'est pas une
défaite : c'est une des deux options que ton brief prévoit.

## 7 — Note sur l'honnêteté du gel

Le protocole a été écrit **avant** toute mesure. C'est vérifiable : son horodatage (20:48:37) est
antérieur à tout artefact Q0FWD de mesure, et aucun fichier de résultat n'existe dans le dossier.
C'est ce qui le distingue de Q0 historique — et c'est la seule chose que je pouvais garantir dans
cette passe.

## 8 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007B_F1A_STATUS=BLOCKED_Q0FWD_TRAJECTORY_NOT_FROZEN
Q0_FORWARD_PROTOCOL=ENGINE-007B-F1A-Q0FWD-v1
Q0_FORWARD_PROTOCOL_SHA256=9a467c3a3262709c80dafe4d4855d1477627e7f761498d434c8587d7b2a81a1b
Q0_FORWARD_PROTOCOL_FROZEN=YES
Q0_FORWARD_STATUS=NOT_EXECUTED
Q1_FORWARD_ALLOWED=NO
Q0_PREDECLARED_STATUS=NOT_EVALUABLE_NO_PREDECLARED_PROTOCOL
Q0_HISTORICAL_STATUS=NOT_PASSED
Q1_HISTORICAL_ALLOWED=NO
INDEPENDENT_ROW_OUTPUT_INDEXING_TEST=PASS
ROLE_COVERAGE_COUNT=6
HISTORICAL_PRE_PRESERVATION_CAPTURE_SET=NOT_SCIENTIFICALLY_QUALIFIED
HARNESS_DEFECT_56=RESOLVED
HARNESS_DEFECT_57=RESOLVED
PRODUCT_MMVQ_SOURCE_MODIFIED=NO
MODEL_GGUF_MODIFIED=NO
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
