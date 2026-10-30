# ENGINE-007B-F1A-Q0-N3quater — RECONSTRUCTION SUR OCTETS QUALIFIÉS

Date : 2026-09-30. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7b-f1a-q0-n3quater/`

---

## 0 — Résultat

**La correction d'adresse était la bonne : les échelles sont devenues plausibles et l'écart a chuté
de 4 ordres de grandeur. Mais un écart systématique subsiste, et j'ai identifié son niveau.**

```
FIRST_DIVERGENCE_LAYER=TQ2_TERNARY_CODE_MAPPING
F1A_Q0_N3QUATER_STATUS=BLOCKED_TQ2_TERNARY_DECODE_GATE
```

## 1 — Ce que la correction d'adresse a produit

```
DATA_START_ALIGNE=10996480  (brut 10996452, padding 28)

echelles AVANT correction : min=-0.02473  max=3400.00000  moyenne=402.2   -> ABSURDE
echelles APRES correction : min= 0.011322 max=  0.020218  moyenne=0.0156  -> PLAUSIBLE
```

```
TQ2_BLOCK_SIZE_PROVEN=YES_34
TQ2_SCALE_BYTE_OFFSET_PROVEN=YES_32
TQ2_SCALE_FP16_CONVERSION_PROVEN=YES
```

## 2 — La reconstruction, et le progrès mesurable

| tenseur | ligne | référence | native | abs_err | **rel_err** |
|---|---|---|---|---|---|
| `blk.20.ffn_up` | 0 | 0.837062 | −2.498384 | 3.3354e+00 | 1.335 |
| `blk.20.ffn_up` | 1 | 0.413061 | 0.868136 | 4.5508e-01 | 0.524 |
| `blk.16.attn_gate` | 0 | 1.911774 | 0.021803 | 1.8900e+00 | 86.68 |
| `blk.16.attn_gate` | 1 | 0.042953 | 1.861307 | 1.8184e+00 | 0.977 |
| `blk.15.attn_output` | 0 | **0.000000** | 2.210024 | 2.2100e+00 | 1.000 |
| `blk.15.attn_output` | 1 | **0.000000** | 4.009287 | 4.0093e+00 | 1.000 |

**Les écarts sont désormais du même ordre que les valeurs** (rel 0.5 à 1.0) au lieu de 4,7 × 10⁴.
La divergence grossière est éliminée. Il reste un écart **systématique** — ce qui désigne une
erreur de décodage, pas une erreur de lecture.

```
N3QUATER_NATIVE_REFERENCE_VALIDATION=FAIL
```

## 3 — Le décodage ternaire : la preuve qu'il est faux

Distribution des codes 2 bits sur la ligne 0 :

```
blk.20.ffn_up      : 0=1449   1=2256   2=1415   3=0     (total 5120)
blk.16.attn_gate   : 0=1442   1=2194   2=1484   3=0     (total 5120)
blk.15.attn_output : 0=1755   1=2642   2=1747   3=0     (total 6144)
```

**Deux faits :**

1. **le code 3 n'apparaît JAMAIS** — 0 sur 5120 et sur 6144 ;
2. avec la table actuelle `{0:−1, 1:0, 2:+1, 3:+1}`, **43 à 44 % des poids deviennent ZÉRO**.

Or un poids ternaire équilibré compte typiquement **~1/3 de zéros**, pas 44 %. Et le cas E6 donne
une référence **exactement nulle** sur les deux lignes — signature d'un décodage qui annule trop de
poids.

### L'hypothèse à tester

```
{0: 0, 1: -1, 2: +1}   -- trois valeurs distinctes, code 3 inutilise
```

Les proportions observées (28 % / 44 % / 28 %) donneraient alors un ternaire **équilibré**, ce qui
est la signature attendue.

### La tension avec la source — que je ne tranche pas

Le commentaire de `vecdotq.cuh:732` dit explicitement :

```
0 -> -1, 1 -> 0, 2 -> +1, 3 -> +1 (replié, comme la référence CPU ggml_vec_dot_tq2_g128_q8_K
et le shader Vulkan)
```

La table du **noyau** est donc bien celle-là. Mais elle transforme 44 % des poids en zéro. Deux
explications possibles, et **aucune n'est prouvée dans cette passe** :

- (i) la quantification a réellement 43-44 % de zéros pour ce modèle ;
- (ii) le **quantificateur** a encodé `0` sous le code 1 et `-1` sous le code 0, et la table du
  noyau est l'inverse du codeur — ce qui serait un défaut du couple codeur/décodeur, pas de ma
  référence.

**Vérifier (i) contre (ii) demande de lire le quantificateur TQ2_G128, pas le décodeur.** C'est
exactement ce que je n'ai pas fait.

```
TQ2_TERNARY_CODE_MAPPING=NOT_QUALIFIED
TQ2_WITHIN_BLOCK_WEIGHT_ORDER=NOT_QUALIFIED
```

## 4 — Chemins natifs identifiés (§2)

```
NATIVE_Q8_PREPARATION_PATH_IDENTIFIED=YES    mmvq.cu:1311  quantize_row_q8_1_cuda
NATIVE_TQ2_VEC_DOT_PATH_IDENTIFIED=YES       vecdotq.cuh:732  vec_dot_tq2_g128_q8_1
NATIVE_SCALE_APPLICATION_PATH_IDENTIFIED=YES return d * __low2float(bq8_1[iqs].ds) * sumi
SELECTED_SAMPLE_PAIRING=PASS   activation et mm_capture de la meme cible/soumission
```

## 5 — Mes propres erreurs dans cette passe

```
HARNESS_DEFECT_54=TERNARY_TEST_DECODED_32_VALUES_INSTEAD_OF_128
   mon test des permutations de table comparait 32 valeurs par bloc au lieu de 128
   (j ai confondu la largeur de la tranche iqs avec la largeur du bloc).
   STATUS=OPEN -- le calcul PRINCIPAL est correct, le TEST ne l est pas.
   -> les trois permutations n ont donc PAS pu etre evaluees. Je ne les evalue pas ici.

HARNESS_DEFECT_55=python3_-i_CONSUMED_THE_SSH_HEREDOC_AS_INTERPRETER_INPUT
   meme famille que le defaut 47. Aucune mesure affectee (le test n a pas tourne).
   STATUS=RESOLVED
```

## 6 — Rapport final (§14)

```
ENGINE007B_F1A_STATUS=BLOCKED_OPERATOR_REFERENCE_GATE
F1A_Q0_N3QUATER_STATUS=BLOCKED_TQ2_TERNARY_DECODE_GATE
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
HARNESS_DEFECT_53_STATUS=RESOLVED_VERIFIED_BY_INDEPENDENT_MULTITENSOR_BYTE_COMPARISON
INDEPENDENT_GGUF_ROW_BYTE_AGREEMENT=YES
SELECTED_SAMPLE_PAIRING=PASS
NATIVE_Q8_PREPARATION_PATH_IDENTIFIED=YES
NATIVE_TQ2_VEC_DOT_PATH_IDENTIFIED=YES
NATIVE_SCALE_APPLICATION_PATH_IDENTIFIED=YES
TQ2_SCALE_FP16_CONVERSION_PROVEN=YES
TQ2_TESTED_SCALE_CONVERSION_MISMATCH_COUNT=0
TQ2_BLOCK_SIZE_PROVEN=YES_34
TQ2_SCALE_BYTE_OFFSET_PROVEN=YES_32
TQ2_TERNARY_CODE_MAPPING=NOT_QUALIFIED
TQ2_WITHIN_BLOCK_WEIGHT_ORDER=NOT_QUALIFIED
Q8_BLOCK_ALIGNMENT_WITH_TQ2=NOT_MEASURED
Q8_1_REFERENCE_UNIT_TESTS=PASS_ACQUIS_NON_REJoue
ROUNDING_HALF_CASES_OK=YES
NATIVE_Q8_QUANTIZATION_SEMANTICS_QUALIFIED=NOT_MEASURED
BLOCK_0_INTEGER_DOT_VERIFIED=NO_NON_ATTEINT
BLOCK_0_SCALE_APPLICATION_VERIFIED=NO_NON_ATTEINT
BLOCK_0_REFERENCE_INTERNAL_AGREEMENT=NO_NON_ATTEINT
FULL_ROW_RECONSTRUCTION_COMPLETED=YES_MAIS_SUR_DECODAGE_NON_QUALIFIE
TESTED_ROW_NATIVE_COMPARISON_COMPLETED=YES
REFERENCE_VALIDATION_SCOPE=6_LIGNES_3_TENSEURS
REFERENCE_VALIDATION_MAX_ABS=4.0093e+00
REFERENCE_VALIDATION_MEAN_ABS=2.3159e+00
REFERENCE_VALIDATION_RELATIVE_L2=FAIL
REFERENCE_VALIDATION_COSINE=NON_DEFINI_POUR_6_SCALAIRES
N3QUATER_NATIVE_REFERENCE_VALIDATION=FAIL
FIRST_DIVERGENCE_LAYER=TQ2_TERNARY_CODE_MAPPING
CODE_3_NEVER_OCCURS_IN_DATA=YES
ZERO_WEIGHT_FRACTION_UNDER_CURRENT_TABLE=0.44
INDEPENDENT_ROW_OUTPUT_INDEXING_TEST=NOT_EXECUTED
REFERENCE_SAMPLE_TEMPORAL_PAIRING=PARTIAL
Q0_CAPTURE_GRAFT_NUMERICALLY_TRANSPARENT=YES
PRODUCT_MMVQ_SOURCE_MODIFIED=NO
MODEL_GGUF_MODIFIED=NO
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
ENGINE007_NEXT_STEP=QUALIFY_TQ2_CODE_TO_TERNARY_MAPPING_FROM_QUANTIZER_SOURCE
```

## 7 — Livrables réellement créés

```
N3QUATER_STATUS.txt                 acquis, echec, preuves, defauts, gates
N3QUATER_ROW_NATIVE_COMPARISON.csv  les 6 lignes : reference, natif, erreurs
N3QUATER_SCALE_DECODING.csv         echelles lues sur octets qualifies
N3QUATER_TERNARY_DECODING.csv       768 valeurs decodees (deux decodeurs)
```

**Non créés** (étapes non atteintes) : `N3QUATER_SOURCE_ARITHMETIC_TRACE.txt`,
`N3QUATER_REFERENCE_SAMPLE_SELECTION.{csv,sha256}`, `N3QUATER_SCALE_CONVERSION_VALIDATION.txt`,
`N3QUATER_TERNARY_VALIDATION.txt`, `N3QUATER_Q8_BLOCK_ALIGNMENT.csv`, `N3QUATER_Q8_QUANTIZATION.csv`,
`N3QUATER_Q8_REFERENCE_TESTS.txt`, `N3QUATER_BLOCK_0_INTEGER_DOT.csv`,
`N3QUATER_BLOCK_0_SCALE_APPLICATION.csv`, `N3QUATER_BLOCK_0_VALIDATION.txt`,
`N3QUATER_FULL_ROW_CONTRIBUTIONS.csv`, `N3QUATER_VECTOR_VALIDATION.csv`,
`N3QUATER_REFERENCE_AGGREGATES.txt`, `N3QUATER_FIRST_DIVERGENCE.txt`,
`N3QUATER_REFERENCE_PROVENANCE.txt`, `N3QUATER_CONFIG_PROVENANCE.txt`.

## 8 — La prochaine étape, et elle est précise

Le gate bloquant se départage par **une seule lecture de source** : le **quantificateur** TQ2_G128,
pas le décodeur.

```
N3quinquies-a  trouver la fonction de QUANTIFICATION TQ2_G128 dans le fork
               (celle qui a produit le GGUF, ou sa référence CPU)
               et lire COMMENT elle encode -1, 0, +1 en codes 2 bits.
N3quinquies-b  si elle encode 0 -> code 1 et -1 -> code 0, alors la table du noyau
               est l inverse du codeur, et c est un defaut du couple, pas de ma reference.
               si elle encode 0 -> code 1 et -1 -> code 0 de facon coherente avec le
               noyau, alors la quantification a reellement 44 pourcent de zeros et ma
               reference est correcte -- l ecart vient d ailleurs.
N3quinquies-c  corriger le test des permutations (defaut 54) et evaluer les trois tables
               sur les 6 lignes avant de conclure.
```

**Le fait que la référence E6 soit exactement nulle est le meilleur indice** : un décodage qui
annule tous les poids d'une ligne ne peut pas etre le bon.

## 9 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007_PRIMARY_LEVER=EPISODE_ALIGNED_STRUCTURED_WORK_ELIMINATION
ENGINE007B_STATUS=UNBLOCKED_FOR_STRUCTURED_EPISODE_FEASIBILITY_ONLY
ENGINE007B_F1A_STATUS=BLOCKED_OPERATOR_REFERENCE_GATE
F1A_Q0_N3QUATER_STATUS=BLOCKED_TQ2_TERNARY_DECODE_GATE
F1A_Q0_N3TER_STATUS=GGUF_ADDRESS_ROOT_CAUSE_PROVEN_AND_BYTES_REQUALIFIED
F1A_Q0_R7_STATUS=SUBMITTED_GRAPH_CAPTURE_INFRASTRUCTURE_QUALIFIED
HARNESS_DEFECT_53=RESOLVED_VERIFIED_BY_INDEPENDENT_MULTITENSOR_BYTE_COMPARISON
GGUF_DATA_START_ALIGNMENT_RULE=PROVEN
TQ2_SCALE_FP16_CONVERSION_PROVEN=YES
TQ2_TERNARY_CODE_MAPPING=NOT_QUALIFIED
CODE_3_NEVER_OCCURS_IN_DATA=YES
ZERO_WEIGHT_FRACTION_UNDER_CURRENT_TABLE=0.44
FIRST_DIVERGENCE_LAYER=TQ2_TERNARY_CODE_MAPPING
N3QUATER_NATIVE_REFERENCE_VALIDATION=FAIL
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
HARNESS_DEFECT_54=OPEN_TERNARY_TEST_DECODED_32_VALUES_INSTEAD_OF_128
HARNESS_DEFECT_55=RESOLVED
Q8_1_REFERENCE_UNIT_TESTS=PASS
ROUNDING_HALF_CASES_OK=YES
EPISODE_PARTITION_COMPLETENESS=PASS_WITHIN_INTRINSIC_FP32_PRECISION
HARNESS_DEFECT_46=RESOLVED
HARNESS_DEFECT_47=RESOLVED
HARNESS_DEFECT_48=RESOLVED
HARNESS_DEFECT_49=RESOLVED
HARNESS_DEFECT_50=RESOLVED_VERIFIED_BY_OFF_ON_DISCRIMINATION
HARNESS_DEFECT_51=RETRACTED_AFTER_MEASUREMENT
HARNESS_DEFECT_52=RESOLVED_VERIFIED_BY_TWO_EPOCH_REGRESSION
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
