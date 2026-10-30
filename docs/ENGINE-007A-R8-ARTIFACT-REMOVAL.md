# ENGINE-007A-R8 — RETRAIT DE L'ARTEFACT D'INSTRUMENTATION

Date : 2026-09-29. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7a-r8/`

---

## 0 — Résultat : cas C, avec une découverte structurelle importante

```
R8_P48_VS_K_FULL_OUTPUT_MISMATCH=1875 / 5120        (identique a R6)
BITWISE_KERNEL_OUTPUT_MATCH=NO
MAX_ABS_OUTPUT_ERROR=0                              (aucune difference au-dessus de la resolution float)
ZERO_ROOT_ARTIFACT_IS_NOT_THE_PRIMARY_NUMERICAL_CAUSE=SUPPORTED      (cas C)
```

**L'artefact est bien retiré et la frontière entier→flottant revient exactement au natif** — mais le
mismatch ne bouge pas d'une ligne. Le retrait était donc **nécessaire mais pas suffisant**.

Et une mesure inattendue change la lecture du problème :

```
R8_MAX_CHAIN_LENGTH=4   ==   NATIVE_MAX_CHAIN_LENGTH=4
   -> l'artefact etait bien la cause des chaines de longueur 7 de R6 (R7/R7B)
```

Mécanisme validé, sans pour autant réparer la fidélité.

---

## 1 — Freeze R7B (§1, §2, §30)

```
ENGINE007A_R7B_STATUS=CLOSED_INTEGER_REORDERING_NUMERICALLY_NEUTRAL_FLOAT_BOUNDARY_PERTURBED
ADDRESS_AND_LOAD_MASK_IDENTICAL=YES
INTEGER_VALUES_PROVABLY_EQUAL_BY_ASSOCIATIVITY=YES
INTEGER_PARTIAL_SUM_VALUES_DIFFER=NO_FOR_ROOT_A
IDP_DEPENDENCY_REORDERING_NUMERICALLY_NEUTRAL=SUPPORTED
K_FULL_I2F_COUNT=1     R6_I2F_COUNT=3
INTEGER_TO_FLOAT_PARTITION_CHANGED=PROVEN_BY_DISASSEMBLY
R6_EXTRA_ROOT_B_SOURCE=INSTRUMENTATION_GUARD
R6_EXTRA_ROOT_B_VALUE=EXACT_ZERO
R6_EXTRA_ROOT_B_DIRECT_NUMERICAL_EFFECT=NONE
INTEGER_CONTRIBUTION_TO_DIVERGENCE=EXCLUDED_FOR_ROOT_A_VALUE
FLOAT_REASSOCIATION_BINARY_PROVEN=NO
FLOAT_REASSOCIATION_HYPOTHESIS=SUPPORTED_AS_REMAINING_FLOAT_PATH_MECHANISM_NOT_BINARY_PROVEN
   (formulation du paragraphe 30 : le statut « REJECTED » de R7 est retire)
```

## 2 — Ce qui a été retiré (§7, §8)

Le noyau R8 supprime **intégralement** le paramètre `span_count` et le compteur `done` :

```cpp
// R6 (artefact)
int done = 0;  ...  done++;
if (tid == 0) { ...; out[row] = s + (float)(done & 1) * (float) span_count * 0.0f; }

// R8
if (tid == 0) { ...; out[row] = s; }
```

```
ARTEFACT_RETIRE=span_count_Et_done_compteur
ADDITIONAL_KEEPALIVE_ADDED=NONE
EXTRA_ZERO_ROOT_COUNT=0
```

Aucun keepalive de remplacement n'a été introduit — le noyau possède déjà un résultat `IDP` réel,
une réduction réelle et un store réel, ce qui suffit à rendre le corps vivant. La boucle runtime
survit sans garde : **c'est le point que le §9 voulait vérifier, et il est vérifié**.

## 3 — Gate artefact : PASS

| | K_FULL natif | R6 (avant) | **R8 (après)** |
|---|---|---|---|
| `I2F` | 1 | **3** | **1** |
| `UI2F` | 0 | **1** | **0** |
| `FMUL` | 0 | **1** | **0** |
| `FFMA` | 1 | **2** | **1** |
| `LDCU` de paramètre | — | **1** | **0** |

```
R8_EXTRA_ROOT_B=ABSENT
R8_EXTRA_I2F_FROM_GUARD=ABSENT
R8_EXTRA_UI2F_FROM_GUARD=ABSENT
R8_EXTRA_FMUL_FROM_GUARD=ABSENT
R8_INTEGER_TO_FLOAT_PARTITION_MATCHES_NATIVE=YES
R8_INTEGER_ROOT_COUNT=1
```

Le §11 et le §14 sont satisfaits : la partition entier→flottant de R8 **correspond exactement** à
celle du natif.

## 4 — Gate de structure : PASS

| | K_FULL natif | R8 |
|---|---|---|
| SASS total | 2113 | **2145** (+32) |
| `LDG` (poids+activations+échelle) | 65 | **65** |
| `IDP.4A` | 32 | **32** |
| `I2F` / `FFMA` / `FMUL` | 1 / 1 / 0 | **1 / 1 / 0** |
| `BSSY`+`BSYNC` | 2 | **4** |
| registres | 38 | **39** |
| spills | 0 | **0** |

```
LOOP_BACKEDGE_PRESENT=YES     ONE_NATIVE_LOOP_BODY_IN_SASS=YES     FULL_UNROLL=NO
LOOP_CONTROL_ARITHMETIC_INDEPENDENT=YES
R8_STRUCTURE_GATE=PASS
```

Le delta de +32 lignes et les +2 régions de convergence sont exactement le coût du double test
runtime (masque + borne), ce que le §20 avait anticipé.

## 5 — Le résultat qui change la lecture : les chaînes IDP

| | natif | R6 | **R8** |
|---|---|---|---|
| `IDP.4A` | 32 | 32 | **32** |
| chaînes d'accumulation | 19 | 16 | **24** |
| **longueur maximale** | **4** | **7** | **4** |

```
R8_MAX_CHAIN_LENGTH=4 == NATIVE_MAX_CHAIN_LENGTH=4
```

**Ceci valide la lecture de R7/R7B** : c'est bien l'artefact à valeur nulle qui provoquait les
chaînes de longueur 7. En le retirant, l'enchaînement entier maximal revient à 4, comme le natif.

Mais le nombre de chaînes reste différent (24 contre 19), et le mismatch ne bouge pas.

## 6 — Où en est la divergence, après élimination

Trois hypothèses sont maintenant **éliminées par la mesure** :

| hypothèse | statut |
|---|---|
| adresses / sous-ensemble de blocs | **éliminée** (masques de chargement identiques, R7) |
| valeurs entières différentes | **éliminée** (associativité, R7B) |
| frontière entier→flottant | **éliminée** (R8 : 1 `I2F` contre 1) |
| perturbation du planificateur entier | **éliminée** (`I2F`=1, chaîne max 4, `FFMA`=1, `FMUL`=0, `UI2F`=0) |

Ce qui reste, et c'est désormais très resserré :

```
DIVERGENCE_RESIDUE=ORDRE_DES_ADDITIONS_FLOTTANTES_ENTRE_CONTRIBUTIONS_KBX
   -> 19 chaines entieres natives contre 24 dans R8 : les contributions par kbx sont combinees
      dans un ordre different. L addition entiere etant associative, les VALEURS entieres sont
      identiques ; c est la SEQUENCE des additions flottantes (1 par kbx, apres conversion et
      multiplication par l echelle) qui n est pas la meme.
MAX_ABS_OUTPUT_ERROR=0_CONFIRME_QUE_LA_DIFFERENCE_EST_EN_DESSOUS_DE_LA_RESOLUTION_FLOAT
   -> les deux resultats different dans leurs derniers bits, pas dans une quantite observable.
```

```
NUMERICAL_DIVERGENCE_ROOT_CAUSE=LOCALIZED_TO_FLOAT_ADDITION_ORDER_ACROSS_KBX_CONTRIBUTIONS
FLOAT_REASSOCIATION_BINARY_PROVEN=NO
   (le paragraphe 12 de R7B exige des racines identiques ; les racines le sont desormais en
    NOMBRE et en FRONTIERE, mais les chaines entieres restent 24 contre 19, donc la preuve
    binaire stricte n est pas encore acquise)
CODE_GENERATION_PERTURBATION_FULLY_REMOVED=NO
   (il reste +2 regions de convergence, inherentes au test runtime)
```

## 7 — Décision : cas C du §21, et suite

```
INSTRUMENTATION_ARTIFACT_CONTRIBUTION=RESOLVED_STRUCTURALLY_BUT_NOT_THE_NUMERICAL_CAUSE
ZERO_ROOT_ARTIFACT_IS_NOT_THE_PRIMARY_NUMERICAL_CAUSE=SUPPORTED
HARNESS_DEFECT_37=RESTE_OUVERT_ROOT_CAUSE_PARTIELLEMENT_LOCALISEE
```

Le §21 dit alors : passer à la **capture directe des roots / accumulations** (§22). C'est la suite
logique, et R8 en précise la cible : ce n'est plus la racine entière qu'il faut capturer (elle est
prouvée égale), c'est **l'accumulateur flottant après chaque groupe kbx**.

Le §22 prévoit exactement ce kernel de debug, non chronométré :

```
DEBUG_KERNEL_CAPTURE=cumul_flottant_apres_chaque_kbx_pour_K_FULL_et_R8
OBJECTIF=FIRST_DIVERGENT_KBX
BACKWARD_TRACE_METHOD_INVALID_FOR_PTXAS_REGISTER_REUSE=YES   (conserve, paragraphe 23)
DEBUG_KERNEL_PERFORMANCE_DATA=INVALID_NOT_MEASURED
```

**Je ne l'ai pas construit dans cette passe.** R8 avait un objectif unique et il est rempli.

## 8 — Un point qui mérite d'être dit

La leçon du §31 est pleinement confirmée, et R8 en apporte la preuve la plus nette de toute la série :

```
NUMERICALLY_ZERO_INSTRUMENTATION_CAN_BE_CODEGEN_ACTIVE=YES
SOURCE_LEVEL_NOOP != BINARY_LEVEL_NOOP
PREUVE : un multiplicateur par 0,0f a cree une racine entiere, une conversion, un FMUL,
         une lecture de parametre, deux regions de convergence, et a porte la chaine
         d accumulation entiere de 4 a 7. Le retirer a tout ramene a la structure native
         SAUF l ordre des additions flottantes.
```

## 9 — État des gates et blocages

```
R6_TIMING_ALLOWED=NO
KBX_MULTIPLICITY_TIMING_ALLOWED=NO
R8_NATIVE_EQUIVALENCE_GATE=FAIL        (mismatch 1875, pas 0)
RELAXED_EPSILON_TIMING=BLOCKED
   -> et R8 renforce ce blocage plutot qu il ne l affaiblit : la divergence est desormais
      localisee a l ordre des additions flottantes, ce qui est exactement le domaine ou une
      tolerance epsilon masquerait un vrai probleme de codegen plutot qu une erreur d arrondi.
HARNESS_DEFECT_38=OPEN_ARM_TABLE   ARM_TABLE_SHA256=NOT_PRODUCED
KBX_DYNAMIC_BODY_MULTIPLICITY_HYPOTHESIS=NOT_TESTED_VALIDLY
MAJOR_FIXED_FLOOR_REMAINS_UNEXPLAINED=YES
ENGINE007B_STATUS=BLOCKED     ENGINE007C_STATUS=BLOCKED
KERNEL_NUMERICAL_RESULT_SENSITIVE_TO_CODEGEN_PERTURBATION=STRONGLY_SUPPORTED
THIS_SENSITIVITY_EXPLAINS_ENGINE006_OUTCOMES=POSSIBLE_NOT_PROVEN
```

## 10 — Provenance

```
Runtime baseline   : libllama 3db83675… / libggml-cuda abafbf0d…  (restaures, SHA verifies)
Arbre llama-obrain-cuda : les 4 fichiers du chemin chaud aux SHA vierges
Fichiers sales     : les 3 greffes connues, inchangees
MODIFICATIONS DE SOURCE DANS CETTE PASSE : AUCUNE (microbench autonome)
Instrument         : qualification/e7a-r8/e7a-r8.cu + bin/e7a-r8
   Les deux noyaux sont dans UN binaire, meme compilation, meme session.
DUMPS CONSERVES : _Z13k_full_nativePKhPKiPf.sass, _Z11k_r8_passesPKhPKiPfy.sass
AUCUN TIMING EXECUTE DANS CETTE PASSE
PLUMBING_FAILURE_29=NON_ASSIGNE
```

## 11 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007A_R7_STATUS=CLOSED_IDP_DAG_DIFFERENCE_FOUND_INITIAL_INTERPRETATION_CORRECTED
ENGINE007A_R7B_STATUS=CLOSED_INTEGER_REORDER_NUMERICALLY_NEUTRAL_FLOAT_BOUNDARY_PERTURBED
ENGINE007A_R8_STATUS=CLOSED_ARTIFACT_REMOVED_STRUCTURE_CONVERGED_MISMATCH_UNCHANGED
ARTEFACT_RETIRE=span_count_Et_done_compteur
ADDITIONAL_KEEPALIVE_ADDED=NONE
EXTRA_ZERO_ROOT_COUNT=0
R8_EXTRA_ROOT_B=ABSENT
R8_EXTRA_I2F_FROM_GUARD=ABSENT
R8_EXTRA_UI2F_FROM_GUARD=ABSENT
R8_EXTRA_FMUL_FROM_GUARD=ABSENT
R8_I2F_COUNT=1
K_FULL_I2F_COUNT=1
R8_INTEGER_ROOT_COUNT=1
R8_INTEGER_TO_FLOAT_PARTITION_MATCHES_NATIVE=YES
R8_IDP_CHAIN_COUNT=24
NATIVE_IDP_CHAIN_COUNT=19
R8_MAX_CHAIN_LENGTH=4
NATIVE_MAX_CHAIN_LENGTH=4
R6_MAX_CHAIN_LENGTH=7
R8_STRUCTURE_GATE=PASS
ONE_NATIVE_LOOP_BODY_IN_SASS=YES
LOOP_BACKEDGE_PRESENT=YES
FULL_UNROLL=NO
LOOP_CONTROL_ARITHMETIC_INDEPENDENT=YES
R8_STATIC_SASS_LINES=2145
K_FULL_STATIC_SASS_LINES=2113
R8_BSSY_BSYNC_REGIONS=4
K_FULL_BSSY_BSYNC_REGIONS=2
R6_BSSY_BSYNC_REGIONS=6
R8_REGISTERS=39
K_FULL_REGISTERS=38
R8_SPILLS=0
R8_P48_VS_K_FULL_OUTPUT_MISMATCH=1875
BITWISE_KERNEL_OUTPUT_MATCH=NO
MAX_ABS_OUTPUT_ERROR=0
R8_NATIVE_EQUIVALENCE_GATE=FAIL
INSTRUMENTATION_ARTIFACT_CONTRIBUTION=RESOLVED_STRUCTURALLY_BUT_NOT_THE_NUMERICAL_CAUSE
ZERO_ROOT_ARTIFACT_IS_NOT_THE_PRIMARY_NUMERICAL_CAUSE=SUPPORTED
INTEGER_CONTRIBUTION_TO_DIVERGENCE=EXCLUDED
INTEGER_TO_FLOAT_PARTITION_CONTRIBUTION=EXCLUDED
NUMERICAL_DIVERGENCE_ROOT_CAUSE=LOCALIZED_TO_FLOAT_ADDITION_ORDER_ACROSS_KBX_CONTRIBUTIONS
FLOAT_REASSOCIATION_BINARY_PROVEN=NO
FLOAT_REASSOCIATION_HYPOTHESIS=SUPPORTED_AS_REMAINING_FLOAT_PATH_MECHANISM_NOT_BINARY_PROVEN
CODE_GENERATION_PERTURBATION_FULLY_REMOVED=NO
NUMERICALLY_ZERO_INSTRUMENTATION_CAN_BE_CODEGEN_ACTIVE=YES
SOURCE_LEVEL_NOOP_NE_BINARY_LEVEL_NOOP=CONFIRME_AVEC_PREUVE
KERNEL_NUMERICAL_RESULT_SENSITIVE_TO_CODEGEN_PERTURBATION=STRONGLY_SUPPORTED
THIS_SENSITIVITY_EXPLAINS_ENGINE006_OUTCOMES=POSSIBLE_NOT_PROVEN
NEXT_STEP_IF_PURSUED=DEBUG_CAPTURE_DU_CUMUL_FLOTTANT_PAR_KBX
BACKWARD_TRACE_METHOD_INVALID_FOR_PTXAS_REGISTER_REUSE=YES
DEBUG_KERNEL_PERFORMANCE_DATA=INVALID_NOT_MEASURED
R6_TIMING_ALLOWED=NO
KBX_MULTIPLICITY_TIMING_ALLOWED=NO
RELAXED_EPSILON_TIMING=BLOCKED
KBX_DYNAMIC_BODY_MULTIPLICITY_HYPOTHESIS=NOT_TESTED_VALIDLY
MAJOR_FIXED_FLOOR_REMAINS_UNEXPLAINED=YES
HARNESS_DEFECT_37=RESOLVED_STRUCTURALLY_ROOT_CAUSE_PARTIAL
HARNESS_DEFECT_38=OPEN_ARM_TABLE
ARM_TABLE_SHA256=NOT_PRODUCED
ENGINE007B_STATUS=BLOCKED
ENGINE007C_STATUS=BLOCKED
COMPUTE_BOUND=NOT_PROVEN
ISSUE_THROUGHPUT_LIMITED=NOT_PROVEN
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
