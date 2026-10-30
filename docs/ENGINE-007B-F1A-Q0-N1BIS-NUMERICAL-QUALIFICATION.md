# ENGINE-007B-F1A-Q0-N1bis — QUALIFICATION NUMÉRIQUE (réparation du manifeste)

Date : 2026-09-30. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7b-f1a-q0-n1bis/`

---

## 0 — Résultat

**Trois gates fermés, un gate bloquant : la reconstruction de l'opérateur échoue massivement.**

```
ENGINE007B_F1A_STATUS=BLOCKED_OPERATOR_REFERENCE_GATE
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
```

| gate | état |
|---|---|
| **N1bis-A** manifeste réparé | **PASS** — vérifié sur deux époques |
| **N1bis-B** sélection E6 | **corrigée** — mon nom de tenseur n'existait pas |
| **N1bis-C** build | **PASS** |
| **N1bis-E** transparence bitwise | **PASS** — sur le **nouvel** essai |
| **N1bis-J** reconstruction native | **ÉCHEC** — erreurs de 4 ordres de grandeur |

## 1 — HARNESS_DEFECT_52 : réparé et vérifié

Le contrat du §1 est respecté : troncature + un seul en-tête au début, ajout ensuite, plus un
`run_id` pour qu'un mode ajout ne mélange jamais deux exécutions.

```
manifeste : 673 lignes, 1 SEUL en-tete, 2 epoques presentes (0 et 1), 672 payloads

HARNESS_DEFECT_52_STATUS=RESOLVED_VERIFIED_BY_TWO_EPOCH_REGRESSION
```

## 2 — La sélection E6 : mon erreur était ailleurs

`blk.16.attn_output.weight` est **absent de l'inventaire autoritaire**. Les `attn_output` E6 sont
aux couches **11, 15, 19, 23, 27, 31, 35, 39, 43, 47, 51, 55** — `full_attention_interval=4`.

**L'écrasement du manifeste n'était donc pas la cause de l'absence en N1 : c'était ma sélection.**
Votre §2 avait raison de me demander de ne pas l'attribuer automatiquement au défaut 52.

Sélection corrigée, figée **avant** tout calcul d'erreur :

```
blk.20.ffn_up.weight        FFN_E5         K=5120  episodes=5
blk.16.attn_gate.weight     ATTENTION_E5   K=5120  episodes=5
blk.15.attn_output.weight   ATTN_OUTPUT_E6 K=6144  episodes=6
Q0_REFERENCE_SELECTION.sha256=57dcc76c83dc352ebe0a7f3f7027958e3586e2cc5c46fa37d399391d89074a46
```

## 3 — Transparence bitwise, confirmée sur le nouvel essai

```
Q0_OFF_LOGITS.bin sha256=53aa87fb70d101c7e98525e148f8c97b8441112a6728dacc28312c2dd5df0dd5
Q0_ON_LOGITS.bin  sha256=53aa87fb70d101c7e98525e148f8c97b8441112a6728dacc28312c2dd5df0dd5

LOGIT_BITWISE_MISMATCH_COUNT=0
Q0_CAPTURE_GRAFT_NUMERICALLY_TRANSPARENT=YES
```

Trajectoire identique (`cmp` sur le fichier de tokens), même modèle, même configuration. La
transparence n'est pas héritée de N1 : elle est **re-mesurée** sur le nouvel essai.

## 4 — Le gate bloquant : la reconstruction échoue

```
budget preenregistre : REL_L2 <= 1e-3, COSINE >= 0.9999, MAX_ABS <= 1e-2 x max|mm_base|

blk.20.ffn_up.weight        abs=1.167752e+05   rel=4.674e+04   ECHEC
blk.16.attn_gate.weight     abs=9.396767e+02   rel=4.310e+04   ECHEC
blk.15.attn_output.weight   abs=2.210024e+00   rel=1.000e+00   ECHEC
```

**L'écart du troisième cas vaut exactement 1.0.** Ce n'est pas un écart numérique : c'est un
**désaccord de nature** — la référence et la sortie native sont dans un rapport 1:1, ou l'une des
deux est nulle.

Ce que je **ne peux pas** faire, et que je ne fais pas :

```
NON ATTRIBUABLE A CUDA        -- trop grand pour un arrondi ou un ordre d operations
NON ATTRIBUABLE A LA WHT      -- desactivee en Q0 (OBRAIN_NO_ROTATION=1)
NON ATTRIBUABLE A LA CAPTURE  -- transparence bitwise mesuree
```

Le §10 demande d'isoler le **premier niveau** de divergence. Je m'arrête donc au premier, sans
ouvrir d'archéologie SASS :

```
(a) MON APPARIEMENT LIGNE <-> VALEUR mm_base EST PEUT-ETRE FAUX
    mm_base fait 17408 colonnes ; je lis la ligne 0 et je la compare a mm_base[0].
    Je n ai jamais etabli QUELLE unite de sortie correspond a la ligne 0,
    ni que l ordre des lignes du tenseur capture est celui que je suppose.
    -> a eliminer EN PREMIER, avant toute autre hypothese.
(b) le layout TQ2_G128 lu dans le GGUF ne correspond pas au decodage du noyau pour
    cette ligne (largeur de ligne, groupement par 128)
(c) la quantification q8_1 a un decalage de bloc entre poids et activation
```

**Le point (a) est de la même famille que mes erreurs précédentes** : j'ai supposé qu'une position
dans un fichier correspondait à une grandeur, sans l'établir. C'est le troisième épisode de ce type
dans cette série (N1 : `awk` sur le mauvais index ; R6 : étiquette interprétée comme un décalage).

Tant que (a) n'est pas éliminé, une comparaison numérique n'a **pas de sens** : je ne sais pas si je
compare les bonnes quantités.

## 5 — Rapport final (§14)

```
ENGINE007B_F1A_STATUS=BLOCKED_OPERATOR_REFERENCE_GATE
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
HARNESS_DEFECT_52_STATUS=RESOLVED_VERIFIED_BY_TWO_EPOCH_REGRESSION
DIAGNOSTIC_BUILD=PASS
MODEL_SHA256=1d7c9409ae7f2c7f6071c8ad32d2481f2c09da94fc2aec3f63a4d44777e2b937
Q0_DECODE_TOKEN_COUNT=9
Q0_COMPUTE_SUBMISSION_COUNT=4
Q0_CAPTURE_ON_OFF_LOGITS_BITWISE_MATCH=YES
LOGIT_BITWISE_MISMATCH_COUNT=0
Q0_CAPTURE_COMPLETENESS=PASS_POUR_LA_POPULATION_CAPTUREE
REFERENCE_SAMPLE_CAPTURE_PAIRING=PARTIAL_IDENTITES_RESOLUES_NON_VALIDEES
TOTAL_PAYLOAD_COUNT=672
PAYLOAD_SHA256_VERIFIED=YES_POUR_L_EXECUTION_R6
SELECTED_E5_TENSOR_NAMES=blk.20.ffn_up.weight,blk.16.attn_gate.weight
SELECTED_E6_TENSOR_NAMES=blk.15.attn_output.weight
ROTATION_APPLIED_TARGET_COUNT=NON MESURE_DANS_CETTE_EXECUTION
CAPTURED_E5_ACTIVATION_K_MATCH=NON MESURE
CAPTURED_E6_ACTIVATION_K_MATCH=NON MESURE
ACTIVATION_NAN_COUNT=NON MESURE
ACTIVATION_INF_COUNT=NON MESURE
BASE_OUTPUT_NAN_COUNT=NON MESURE
BASE_OUTPUT_INF_COUNT=NON MESURE
Q8_1_REFERENCE_UNIT_TESTS=PASS_ACQUIS_NON_REJoue
ROUNDING_HALF_CASES_OK=YES
TQ2_G128_REFERENCE_DECODE=QUALIFIED
EPISODE_PARTITION_COMPLETENESS=PASS_WITHIN_INTRINSIC_FP32_PRECISION
REFERENCE_ERROR_BUDGET=REL_L2_LE_1e-3_AND_COSINE_GE_0.9999_AND_MAX_ABS_LE_1e-2_TIMES_NATIVE_MAXABS
REFERENCE_VALIDATION_MAX_ABS=1.167752e+05
REFERENCE_VALIDATION_MEAN_ABS=3.902e+04
REFERENCE_VALIDATION_RELATIVE_L2=4.674e+04
REFERENCE_VALIDATION_COSINE=NON_DEFINI_ECHEC_MASSIF
Q0_EPISODE_REFERENCE_RECONSTRUCTS_FULL_OPERATOR=FAIL
REFERENCE_ARITHMETIC_MODEL=SOURCE_CONFIRMED_NOT_NATIVE_VALIDATED
PRODUCT_MMVQ_SOURCE_MODIFIED=NO
MODEL_GGUF_MODIFIED=NO
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
ENGINE007_NEXT_STEP=N2_ESTABLISH_ROW_TO_OUTPUT_UNIT_CORRESPONDENCE_BEFORE_ANY_NUMERIC_COMPARISON
```

## 6 — Livrables réellement créés

```
Q0_N1BIS_STATUS.txt                      gates franchis, gate bloquant, causes ordonnees
Q0_NATIVE_OPERATOR_REFERENCE_VALIDATION.csv   les trois echecs par tenseur
Q0_REFERENCE_SELECTION.csv / .sha256     selection corrigee, figee avant calcul
Q0_ON_LOGITS.bin                         logits du nouvel essai ON
Q0_TOKEN_TRAJECTORY.txt / .sha256        trajectoire figee, identique a N1
capon/GRAPH_CAPTURE_MANIFEST.csv         673 lignes, 2 epoques, 1 en-tete
capon/*.bin                              672 payloads
on.txt                                   journal brut
```

**Non créés** : `MANIFEST_TWO_EPOCH_REGRESSION.txt` (le test de régression a été fait par la mesure
du manifeste : 673 lignes, 1 en-tête, 2 époques — non archivé séparément),
`N1BIS_BUILD_PROVENANCE.txt`, `N1BIS_PATCH_PROVENANCE.txt`, `N1BIS_CONFIG_PROVENANCE.txt`,
`Q0_SUBMISSION_DECODE_MAPPING.csv`, `Q0_GRAPH_CAPTURE_MANIFEST.csv` (recopié sous `capon/`),
`Q0_CAPTURE_COMPLETENESS.txt`, `Q0_CAPTURE_SANITY.txt`, `Q0_OFF_ON_NUMERIC_COMPARISON.csv`
(la comparaison est un `cmp` binaire : SHA256 identiques), `Q8_1_REFERENCE_TESTS.txt`,
`TQ2_G128_REFERENCE_TESTS.txt`, `Q0_REFERENCE_AGGREGATES.txt`.

## 7 — Ce que N2 doit faire, dans cet ordre

```
N2-a  ETABLIR la correspondance ligne <-> unite de sortie, AVANT toute comparaison.
      Ne pas supposer que mm_base[row] correspond a la ligne row du poids.
      Verifier sur un cas ou la reponse est calculable independamment :
      par exemple un poids connu (token_embd, output.weight) ou une ligne isolee.
N2-b  une fois (a) etabli, refaire le produit scalaire sur UNE ligne et comparer.
      Si l ecart reste massif, alors (b) puis (c) deviennent pertinents.
N2-c  seulement si (a) et (b) sont elimines : inspecter le SASS.
```

## 8 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007_PRIMARY_LEVER=EPISODE_ALIGNED_STRUCTURED_WORK_ELIMINATION
ENGINE007B_STATUS=UNBLOCKED_FOR_STRUCTURED_EPISODE_FEASIBILITY_ONLY
ENGINE007B_F1A_STATUS=BLOCKED_OPERATOR_REFERENCE_GATE
F1A_Q0_R7_STATUS=SUBMITTED_GRAPH_CAPTURE_INFRASTRUCTURE_QUALIFIED
HARNESS_DEFECT_52=RESOLVED_VERIFIED_BY_TWO_EPOCH_REGRESSION
DIAGNOSTIC_BUILD=PASS
CAPTURE_DISABLED_RUNTIME_PATH_INERT=YES
Q0_CAPTURE_ON_OFF_LOGITS_BITWISE_MATCH=YES
Q0_CAPTURE_GRAFT_NUMERICALLY_TRANSPARENT=YES
OFF_ON_TOKEN_TRAJECTORY_MATCH=YES
Q0_EPISODE_REFERENCE_RECONSTRUCTS_FULL_OPERATOR=FAIL
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
Q8_1_REFERENCE_UNIT_TESTS=PASS
ROUNDING_HALF_CASES_OK=YES
TQ2_G128_REFERENCE_DECODE=QUALIFIED
EPISODE_PARTITION_COMPLETENESS=PASS_WITHIN_INTRINSIC_FP32_PRECISION
HARNESS_DEFECT_46=RESOLVED
HARNESS_DEFECT_47=RESOLVED
HARNESS_DEFECT_48=RESOLVED
HARNESS_DEFECT_49=RESOLVED
HARNESS_DEFECT_50=RESOLVED_VERIFIED_BY_OFF_ON_DISCRIMINATION
HARNESS_DEFECT_51=RETRACTED_AFTER_MEASUREMENT
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
