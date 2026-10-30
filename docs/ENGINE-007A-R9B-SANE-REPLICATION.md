# ENGINE-007A-R9B — RÉPLICATION DE R2 SUR DONNÉES SAINES

Date : 2026-09-29. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7a-r9b/`

---

## 0 — Résultat : les conclusions de R2 se répliquent sur données saines

```
INPUT_SANITY_GATE=PASS     (5/5 processus)
OUTPUT_SANITY_GATE=PASS    (5/5 processus, 7/7 bras, 0 sortie non finie)

SANE_A100_MINUS_A0_US=+2,9331      IC95 [2,8303 ; 3,0359]   effect/noise=55,9
SANE_A100_MINUS_A0_PERCENT=3,22
OLD_CONTAMINATED_A100_MINUS_A0=+3,4391 us (3,65 %)   <- reference HISTORIQUE

SANE_A50_SAME_CARDINALITY_DELTA=-0,2585 us
SANE_A25_SAME_CARDINALITY_DELTA=-1,2520 us
OLD_CONTAMINATED: A50 -0,5089 ; A25 -1,1527

ENGINE007A_R2_SANE_STATUS=CLOSED_CONCLUSIVE_REPLICATES_ON_SANE_DATA
```

Les trois affirmations scientifiques de R2 **tiennent sur données propres** :

```
USEFUL_DP4A_WORK_IS_NOT_A_DOMINANT_WALL_COMPONENT=RE-PROMOTED
DP4A_EXECUTION_THROUGHPUT_LIMIT=REJECTED_BY_SANE_ARITHMETIC_REMOVAL_EXPERIMENT
IDP_COUNT_ALONE_INSUFFICIENT=YES_RE-PROMOTED
ARITHMETIC_WORK_SITE_PLACEMENT_MATTERS=YES_RE-PROMOTED
```

---

## 1 — Correction du statut R9A (§1 à §3)

```
ENGINE007A_R9A_STATUS=CLOSED_FALSE_MISMATCH_CAUSED_BY_NAN_INPUT_METADATA
SANE_INPUT_NATIVE_NAN_COUNT=0      SANE_INPUT_R8_NAN_COUNT=0
SANE_INPUT_MISMATCH_TOTAL=0        BITWISE_KERNEL_OUTPUT_MATCH=YES
MAX_ABS_OUTPUT_ERROR=0_EXACT
FALSE_MISMATCH_ROOT_CAUSE=SYNTHETIC_TQ2_G128_SCALE_FIELD_FILLED_WITH_ARBITRARY_BITS
IEEE_NAN_COMPARISON=NaN_NEQ_NaN -> l ancien if (a[i] != b[i]) n etait pas un test valide

RETRACTED=R7_NUMERICAL_DIVERGENCE_NARRATIVE,R7B_...,R8_...
FLOAT_REASSOCIATION_HYPOTHESIS=RETIRED_NO_DIVERGENCE_EXISTS
KERNEL_NUMERICAL_RESULT_SENSITIVE_TO_CODEGEN_PERTURBATION=NOT_ESTABLISHED_BY_R7_R8
R8_NATIVE_EQUIVALENCE_GATE=PASS_ON_SANE_INPUTS
R6_NATIVE_EQUIVALENCE_GATE=PASS_ON_SANE_INPUTS
   (R6 AVEC l artefact a valeur nulle : MISMATCH=0, P12_KBX_SEQUENCE_MATCH_NATIVE=YES)

CHANGED_CODEGEN_STRUCTURE=YES     CHANGED_NUMERICAL_OUTPUT=NO_ON_VALID_INPUTS
STRUCTURAL_CODEGEN_FACTS=VALID    STRUCTURAL_DIFFERENCE_IMPLIES_NUMERICAL_DIVERGENCE=FALSE
```

## 2 — Générateur canonique et gates d'entrée (§4 à §6, §14)

Le champ d'échelle est désormais engendré explicitemment, bloc par bloc :

```cpp
const unsigned e = 9u + (unsigned) ((b * 2654435761u) % 5u);   // exposant 9..13 -> [2^-6, 2^-2]
const unsigned m = (unsigned) ((b * 40503u) % 1024u);
const uint16_t hb = (uint16_t) ((e << 10) | m);
memcpy(&blk->d, &hb, 2);
```

Validation sur l'hôte **avant** exécution GPU :

```
TQ2_BLOCK_COUNT=245760
FINITE_SCALE_COUNT=245760
NAN_SCALE_COUNT=0
INF_SCALE_COUNT=0
SUBNORMAL_SCALE_COUNT=0
ZERO_SCALE_COUNT=0
HALF_SCALE_MIN_APPROX=0x1p-6        HALF_SCALE_MAX_APPROX=0x1.ffcp-2
ACTIVATION_METADATA_NONFINITE_COUNT=0
SYNTHETIC_FLOAT_METADATA_FIELD_COUNT=1   (d de tq2_g128 ; ds de q8_1 verifie aussi)
INVALID_FLOAT_METADATA_COUNT=0
INPUT_SANITY_GATE=PASS
```

## 3 — Gate de sortie (§15)

```
OUTPUT_NONFINITE_A0=0        OUTPUT_NONFINITE_A25_A=0     OUTPUT_NONFINITE_A25_B=0
OUTPUT_NONFINITE_A50_EVEN=0  OUTPUT_NONFINITE_A50_ODD=0   OUTPUT_NONFINITE_A75=0
OUTPUT_NONFINITE_A100=0
OUTPUT_SANITY_GATE=PASS
```

## 4 — Parité des noyaux (§13)

Les sept variantes sont **le même code** que R2 ; seule la génération d'entrée change.

| variante | SASS total | `IDP.4A` | `LOP3` | `SHF` | `SHFL` | `FADD` | `BAR` | `STG` | spills |
|---|---|---|---|---|---|---|---|---|---|
| A0 | **2209** | 0 | 292 | 104 | 5 | 10 | 1 | 2 | 0 |
| A50_EVEN | **2241** | 16 | 292 | 104 | 5 | 10 | 1 | 2 | 0 |
| A50_ODD | **2241** | 16 | 292 | 104 | 5 | 10 | 1 | 2 | 0 |
| A100 | **2273** | 32 | 292 | 104 | 5 | 10 | 1 | 2 | 0 |

Ces valeurs sont **rigoureusement identiques** à celles relevées en R2 (contaminé) : le device code
est inchangé, `SASS_MANIFEST_MATCH_OLD_R2=YES` pour la totalité des compteurs du §13, et
`SPILLS=0` sur les sept variantes.

**Réserve de méthode** : je n'ai pas produit séparément les `OLD_R2_CUBIN_KERNEL_HASH` et
`SANE_R2_CUBIN_KERNEL_HASH` comme le §13 le demandait idéalement. L'égalité est établie par le
manifeste SASS complet (total, opérandes, registres, spills), ce qui porte la même information mais
n'est pas un hash de cubin. Je le signale plutôt que de le passer sous silence.

## 5 — Protocole et campagne (§16)

```
KERNELS_PER_BATCH=2048      WARMUP_BATCHES=3        ONE_EVENT_PAIR_PER_BATCH
FRESH_PROCESS_PER_CAMPAIGN  PROCESS_COUNT=5         LATIN_ROTATION (7 bras x 2 sens)
NEUTRAL_WORK_BETWEEN_ARMS   PROCESS_MEAN_PRIMARY_UNIT   PAIRED_CONTRASTS
N_OBSERVATIONS=560          N_CYCLES_PER_PROCESS=16
```

Moyennes par bras :

| bras | n | moyenne | sd | min | max |
|---|---|---|---|---|---|
| A0 | 80 | **88,1842** | 0,7022 | 86,0630 | 89,6547 |
| A25_A | 80 | **88,2681** | 0,7077 | 86,0819 | 89,6505 |
| A25_B | 80 | **89,5201** | 0,9035 | 87,5768 | 90,8640 |
| A50_EVEN | 80 | **88,8118** | 0,8719 | 86,3120 | 90,3342 |
| A50_ODD | 80 | **89,0703** | 0,8497 | 86,6777 | 90,5216 |
| A75 | 80 | **89,7268** | 0,8454 | 87,9883 | 91,3253 |
| A100 | 80 | **91,1173** | 0,8366 | 88,8270 | 92,2988 |

Moyennes par processus (§17 : l'unité statistique principale) :

| pid | A0 | A25_A | A25_B | A50_EVEN | A50_ODD | A75 | A100 |
|---|---|---|---|---|---|---|---|
| 0 | 87,068 | 87,203 | 88,187 | 87,716 | 87,987 | 88,242 | 90,006 |
| 1 | 88,095 | 88,168 | 88,940 | 88,251 | 88,453 | 89,593 | 90,520 |
| 2 | 88,305 | 88,458 | 89,986 | 88,870 | 89,589 | 90,211 | 91,531 |
| 3 | 88,542 | 88,740 | 90,275 | 89,326 | 89,260 | 90,315 | 91,499 |
| 4 | 88,912 | 88,773 | 90,213 | 89,895 | 90,062 | 90,273 | 92,030 |

## 6 — Contraste principal (§18) : réplication

```
SANE_A100_MINUS_A0_US=+2,9331      sd=0,4692      IC95=[2,8303 ; 3,0359]
SANE_A100_MINUS_A0_PERCENT=3,22
EFFECT_TO_NOISE=55,9

OLD_CONTAMINATED_A100_MINUS_A0=+3,4391 us (3,65 %)     <- HISTORIQUE
```

L'écart entre les deux campagnes est de **0,51 µs (15 %)**, et l'ordre de grandeur comme la
significativité sont reproduits. **Supprimer la totalité des 32 `dp4a` ne rend que ~3 % de la paroi**
— sur données propres comme sur données contaminées.

## 7 — Gate même-cardinalité (§17) : résultat affiné

| contrôle | R2 contaminé | **R2-SANE** |
|---|---|---|
| A50_EVEN − A50_ODD (16 sites) | −0,5089 | **−0,2585** (sd 0,4438) |
| A25_A − A25_B (8 sites) | −1,1527 | **−1,2520** (sd 0,5098) |

Le contrôle **A50 est passé sous le seuil de résolution** (0,5089 µs) : à 16 sites, l'emplacement ne
compte pas de façon détectable. C'est un affinage, pas une contradiction.

Le contrôle **A25 reste massif** : deux masques de **8 sites strictement identiques en nombre**
diffèrent de **1,25 µs**, soit **43 % du contraste extrême A100−A0** (2,93 µs). Le signal se
reproduit sur les 5 processus, toujours dans le même sens (`A25_B` plus lent).

```
IDP_COUNT_ALONE_INSUFFICIENT=YES_CONFIRMED_ON_SANE_DATA
ARITHMETIC_WORK_SITE_PLACEMENT_MATTERS=YES_CONFIRMED_ON_SANE_DATA
PLACEMENT_EFFECT_AT_CARDINALITY_8=1.25_US_43_PERCENT_OF_FULL_CONTRAST
PLACEMENT_EFFECT_AT_CARDINALITY_16=NOT_DETECTED_BELOW_RESOLUTION
```

C'est le résultat le plus fort de R2, et il est **plus net sur données saines** : à cardinalité 8,
l'emplacement compte presque autant que la moitié du travail total.

## 8 — Monotonie et forme de la courbe (§19)

```
A0        ( 0 sites) 88,1842
A25_A     ( 8 sites) 88,2681
A25_B     ( 8 sites) 89,5201
A50_EVEN  (16 sites) 88,8118
A50_ODD   (16 sites) 89,0703
A75       (24 sites) 89,7268
A100      (32 sites) 91,1173

A0  -> A25  : +0,7099 us   (0,089 us/site)
A25 -> A50  : +0,0470 us   (0,006 us/site)
A50 -> A75  : +0,7857 us   (0,098 us/site)
A75 -> A100 : +1,3905 us   (0,174 us/site)
```

La progression est croissante en moyenne, avec les deux inversions attendues entre masques de même
cardinalité. L'incrément marginal **quadruple** sur le dernier quart (0,006 → 0,174 µs/site) : la
même forme non linéaire qu'en R2 contaminé, où le dernier incrément était déjà 3,5 à 9 fois plus
grand. **Le `ARITHMETIC_WORK_THRESHOLD_BEHAVIOR=SUGGESTED` de R2 est donc renforcé, pas affaibli.**

## 9 — Décision (§19, §20)

`Case A` du §19 s'applique : les conclusions de R2 se reproduisent. Je les re-promus **sur données
saines** :

```
USEFUL_DP4A_WORK_IS_NOT_A_DOMINANT_WALL_COMPONENT=SUPPORTED_CLEAN_DATA
DP4A_EXECUTION_THROUGHPUT_LIMIT=REJECTED_BY_SANE_ARITHMETIC_REMOVAL_EXPERIMENT
IDP_COUNT_ALONE_INSUFFICIENT=YES_CLEAN_DATA
ARITHMETIC_WORK_SITE_PLACEMENT_MATTERS=YES_CLEAN_DATA
SANE_A100_MINUS_A0_IS_THE_AUTHORITATIVE_VALUE=YES
OLD_CONTAMINATED_VALUE_RETAINED_AS_HISTORICAL_ONLY=YES
```

Le §10 est respecté : **ENGINE-006 reste fermé**. Ses conclusions produit reposaient sur des charges
de travail modèle réelles, pas sur ce générateur synthétique, et rien ne permet d'inférer une
contamination en amont sans preuve.

## 10 — Défauts et statuts

```
HARNESS_DEFECT_37=RESOLVED_FALSE_MISMATCH_FROM_NAN_COMPARATOR_INPUTS
   -> ce n etait PAS un defaut de fidelite du noyau. Le noyau a toujours ete correct.
RELAXED_EPSILON_TIMING=CLOSED_NO_LONGER_APPLICABLE
   -> il n y a aucun ecart numerique a tolerer : l equivalence est bit a bit sur entrees valides.
HARNESS_DEFECT_38=OPEN_ARM_TABLE            ARM_TABLE_SHA256=NOT_PRODUCED
   -> a resoudre AVANT tout timing de multiplicite kbx.
HARNESS_DEFECT_40=SYNTHETIC_TQ2_G128_GENERATOR_FILLED_FP16_SCALE_WITH_ARBITRARY_BITS   STATUS=RESOLVED_IN_R9A
R2_R3_R4_R5_R6_R8_TIMING_STATUS=HISTORICAL_CONTAMINATED_NOT_PROMOTED_PENDING_SANE_REPLICATION
   -> R2 est desormais replique (cette passe). R3/R4/R5/R6/R8 restent en attente.
NA_INPUT_TIMING_EQUIVALENCE=NOT_ASSUMED
PLUMBING_FAILURE_29=NON_ASSIGNE
```

**Errata à porter sur les rapports historiques** (§8, §38, §39) — je ne les réécris pas, je les
marque :

| rapport | marquage |
|---|---|
| R7, R7B, R8 | `HISTORICAL_FORENSIC_BASED_ON_FALSE_MISMATCH` — `NUMERICAL_INTERPRETATION_RETRACTED_BY_R9A`, `STRUCTURAL_SASS_OBSERVATIONS_RETAINED` |
| R4, R5, R6 | `INPUT_DATA_CONTAMINATION_PRESENT=YES` — `TIMING_PROMOTION=PENDING_SANE_REPLICATION` |
| R2 (contaminé) | repliqué par R9B ; valeur historique conservée à côté |

## 11 — Provenance

```
Runtime baseline   : libllama 3db83675… / libggml-cuda abafbf0d…  (restaures, SHA verifies)
Arbre llama-obrain-cuda : les 4 fichiers du chemin chaud aux SHA vierges
Fichiers sales     : les 3 greffes connues, inchangees
MODIFICATIONS DE SOURCE DANS CETTE PASSE : AUCUNE (microbench autonome)
Instrument         : qualification/e7a-r9b/e7a-r9b.cu + bin/e7a-r9b
   Les sept variantes sont dans UN binaire, meme compilation, meme session.
   Seule la generation d entree differe de R2 ; les noyaux sont identiques (manifeste SASS egal).
CANONICAL_MICROBENCH_TIMING_PROTOCOL=K2048_WARMUP3_LATIN_MULTI_PROCESS  (applique)
AUCUN TIMING ISSU D UN BRAS NON FINI : gate de sortie verifie avant chaque campagne.
```

## 12 — Ordre d'exécution qui reste (§44)

```
STEP_1=R2_SANE_REPLICATION                      FAIT
STEP_2=FREEZE_OR_RETRACT_OLD_R2_CONCLUSIONS     FAIT (re-promues sur donnees saines)
STEP_3=RESOLVE_HARNESS_DEFECT_38_ARM_TABLE      A FAIRE
STEP_4=VERIFY_FULL_RUNTIME_KBX_ARM_BITWISE      FAIT (R9A : mismatch 0)
STEP_5=R10_NATIVE_KBX_MULTIPLICITY_TIMING       BLOQUE PAR STEP_3
```

## 13 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007A_R9A_STATUS=CLOSED_FALSE_MISMATCH_CAUSED_BY_NAN_INPUT_METADATA
ENGINE007A_R9B_STATUS=CLOSED_CONCLUSIVE_REPLICATES_ON_SANE_DATA
NATIVE_VS_R8_SANE_MISMATCH=0
NATIVE_VS_R6_SANE_MISMATCH=0
BITWISE_NATIVE_RUNTIME_EQUIVALENCE=PASS
R7_R7B_R8_NUMERICAL_NARRATIVE=RETRACTED
R7_R7B_R8_STRUCTURAL_SASS_OBSERVATIONS=RETAINED
FLOAT_REASSOCIATION_HYPOTHESIS=RETIRED
KERNEL_NUMERICAL_RESULT_SENSITIVE_TO_CODEGEN_PERTURBATION=RETIRED_AS_NUMERICAL_CLAIM
RELAXED_EPSILON_TIMING=CLOSED_NO_OBJECT
RETRACTED_ENGINE006_REOPENING=NO_ENGINE006_STAYS_CLOSED
INPUT_SANITY_GATE=PASS
OUTPUT_SANITY_GATE=PASS
TQ2_BLOCK_COUNT=245760
NAN_SCALE_COUNT=0
INF_SCALE_COUNT=0
SUBNORMAL_SCALE_COUNT=0
ZERO_SCALE_COUNT=0
INVALID_FLOAT_METADATA_COUNT=0
SASS_MANIFEST_MATCH_OLD_R2=YES
SANE_R2_A0_SASS=2209   SANE_R2_A50_SASS=2241   SANE_R2_A100_SASS=2273
SANE_R2_SPILLS=0
PROCESS_COUNT=5   N_CYCLES_PER_PROCESS=16   KERNELS_PER_BATCH=2048   N_OBSERVATIONS=560
SANE_A0_MEAN_US=88.1842
SANE_A25_A_MEAN_US=88.2681
SANE_A25_B_MEAN_US=89.5201
SANE_A50_EVEN_MEAN_US=88.8118
SANE_A50_ODD_MEAN_US=89.0703
SANE_A75_MEAN_US=89.7268
SANE_A100_MEAN_US=91.1173
SANE_A100_MINUS_A0_US=+2.9331
SANE_A100_MINUS_A0_PERCENT=3.22
SANE_A100_MINUS_A0_CI95=[2.8303,3.0359]
SANE_A100_MINUS_A0_EFFECT_TO_NOISE=55.9
OLD_CONTAMINATED_A100_MINUS_A0=+3.4391 (3.65 %)
SANE_A50_SAME_CARDINALITY_DELTA=-0.2585
SANE_A25_SAME_CARDINALITY_DELTA=-1.2520
PLACEMENT_EFFECT_AT_CARDINALITY_8=1.25_US_43_PERCENT_OF_FULL_CONTRAST
PLACEMENT_EFFECT_AT_CARDINALITY_16=NOT_DETECTED_BELOW_RESOLUTION
USEFUL_DP4A_WORK_IS_NOT_A_DOMINANT_WALL_COMPONENT=SUPPORTED_CLEAN_DATA
DP4A_EXECUTION_THROUGHPUT_LIMIT=REJECTED_BY_SANE_ARITHMETIC_REMOVAL_EXPERIMENT
IDP_COUNT_ALONE_INSUFFICIENT=YES_CLEAN_DATA
ARITHMETIC_WORK_SITE_PLACEMENT_MATTERS=YES_CLEAN_DATA
ARITHMETIC_WORK_THRESHOLD_BEHAVIOR=SUGGESTED_REINFORCED_CLEAN_DATA
R2_TIMING_RESULT=REPLICATED_ON_SANE_DATA
R3_R4_R5_R6_R8_TIMING_STATUS=HISTORICAL_CONTAMINATED_PENDING_SANE_REPLICATION
NA_INPUT_TIMING_EQUIVALENCE=NOT_ASSUMED
KBX_DYNAMIC_BODY_MULTIPLICITY_HYPOTHESIS=NOW_TESTABLE_AFTER_ARM_TABLE_FIX
KBX_MULTIPLICITY_TIMING_ALLOWED=PENDING_HARNESS_DEFECT_38
ENGINE007B_STATUS=BLOCKED_PENDING_R2_SANE_AND_ARM_TABLE
ENGINE007C_STATUS=BLOCKED_PENDING_R2_SANE_AND_ARM_TABLE
HARNESS_DEFECT_37=RESOLVED_BY_R9A_NO_KERNEL_DEFECT_EXISTED
HARNESS_DEFECT_38=OPEN_ARM_TABLE
HARNESS_DEFECT_40=RESOLVED_IN_R9A
ARM_TABLE_SHA256=NOT_PRODUCED
ENGINE006_STATUS=CLOSED_NO_MATERIAL_RECOVERABLE_LOCAL_WASTE_FOUND
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
