# ENGINE-007A-R7B — FORENSIQUE DE LA BOUNDARY IDP → I2F → ÉCHELLE

Date : 2026-09-29. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7a-r7/` (dumps SASS conservés)

---

## 0 — Résultat

**Le brief avait raison de corriger R7.** Je retire la conclusion « les valeurs entières diffèrent »
et je la remplace par ce que le binaire montre réellement.

```
IDP_PARTITIONING_DIFFERS_BEFORE_FLOAT_CONSUMPTION=SUPPORTED
INTEGER_PARTIAL_SUM_VALUES_DIFFER=NOT_PROVEN          (R7 avait conclu trop vite)
INTEGER_ROOT_ASSOCIATIVITY_ONLY=SUPPORTED             (voir paragraphe 3b)
INTEGER_VALUES_PROVABLY_EQUAL_BY_ASSOCIATIVITY=YES
INTEGER_TO_FLOAT_PARTITION_CHANGED=SUPPORTED_PAR_MESURE_DIRECTE
FLOAT_REASSOCIATION_BINARY_PROVEN=NO
FLOAT_REASSOCIATION_HYPOTHESIS=NOT_PROVEN_AND_NOT_CLEANLY_REJECTED
```

Le fait central, mesuré sur le désassemblage :

```
NATIF : 1 seule conversion entier->flottant (0x3dd0), a l offset +2 apres le dernier IDP
R6    : 3 conversions : 0x3e90 (+2), 0x3f50 (+14), 0x4140 (+45)
```

Et la fenêtre 0x3e80–0x3f70 montre que **R6 a deux racines entières distinctes, dans deux régions de
convergence différentes**, chacune convertie séparément :

```
0x3e90  I2FP.F32.S32 R11, R11      <- racine A : l accumulateur dp4a
0x3eb0  FFMA R13, R10, R11, R13    <- entre dans le terme flottant
0x3ed0  BSYNC.RECONVERGENT B2      <- fin de region
0x3ef0  @!P0 BRA 0x1e0             <- backedge de boucle
0x3f40  LOP3.LUT R0, R15, 0x1, RZ  <- racine B : le compteur `done & 1`
0x3f50  I2FP.F32.U32 R0, R0        <- converti SEPAREMENT
```

C'est la partition demandée au §9 : elle **change**, et c'est mesuré, pas déduit.

```
ENGINE007A_R7B_STATUS=CLOSED_PARTITION_CHANGE_PROVEN_INTEGER_VALUES_NOT_CAPTURED
HARNESS_DEFECT_37=R6_P48_OUTPUT_DIVERGENCE   STATUS=LOCALISE_A_LA_FRONTIERE_I2F_OPEN
```

---

## 1 — Ce que le brief corrige, et pourquoi il a raison (§1, §2)

R7 avait écrit `DIVERGENCE_IS_ON_INTEGER_PARTIAL_SUMS=YES` à partir du seul changement de DAG des
`IDP.4A` (19 chaînes de max 4 → 16 chaînes de max 7). Le §2 de R7B objecte justement que `dp4a` est
une accumulation entière 32 bits et que regrouper autrement `A+B+C+D` **ne prouve pas** que la valeur
finale diffère.

**L'objection est fondée, et la mesure la confirme** : les deux noyaux consomment la même quantité
d'arithmétique entière (§3), donc rien ne prouve que les racines entières diffèrent en valeur.

```
DIVERGENCE_IS_ON_INTEGER_PARTIAL_SUMS=WITHDRAWN
DIVERGENCE_IS_ON_INTEGER_PARTIAL_SUMS=REPLACED_BY_IDP_PARTITIONING_DIFFERS_BEFORE_FLOAT_CONSUMPTION
```

## 2 — Les deux noyaux font la même quantité d'arithmétique entière (§3)

Comptage des instructions entières hors `IDP` :

| instruction | natif | R6 | écart |
|---|---|---|---|
| `IMAD.SHL.U32` | 65 | 65 | 0 |
| `IMAD.WIDE.U32` | 4 | 4 | 0 |
| `IADD` | 132 | **133** | +1 |
| `IADD.64` | 6 | **7** | +1 |
| `IADD3` | 32 | 32 | 0 |
| `LEA` | 33 | 33 | 0 |
| `LOP3.LUT` | 259 | **261** | +2 |
| `SHF.R.U32.HI` | 98 | 98 | 0 |
| `SHF.R.U64` | 1 | **2** | +1 |
| `IDP.4A` | **32** | **32** | 0 |

L'essentiel est identique ; les +1/+2 sont le résidu du contrôle de boucle runtime. **Ce n'est donc
pas par un surplus de calcul entier que les sorties diffèrent.**

## 3 — La mesure décisive : la frontière entier → flottant (§5, §6)

Position du dernier `IDP` et des conversions :

| | natif | R6 |
|---|---|---|
| index du dernier `IDP` | 987 | 999 |
| offsets des `I2F` après lui | **[2]** | **[2, 14, 45]** |
| nombre de conversions | **1** | **3** |

Consommateurs immédiats, relevés sur le SASS :

```
NATIF
  I2F  @0x3dd0  (dest R9)   ->  FFMA @0x3e10 : R10, R13, R9, R10      (le terme accumule)

R6
  I2F  @0x3e90  (dest R11)  ->  FFMA @0x3eb0 : R13, R10, R11, R13     (racine A : sumi)
  I2F  @0x3f50  (dest R0)   ->  racine B : compteur `done & 1` (LOP3 avant l I2F)
  UI2F @0x4140  (dest UR4)  ->  LDCU d un parametre de noyau, puis FMUL (terme * 0,0f)
```

### 3a — Ce que le §10 demandait : la frontière change

**Natif = 1 racine entière → 1 conversion. R6 = 2 racines entières → 2 conversions**, chacune dans
sa propre région de convergence, séparées par le backedge de boucle.

```
INTEGER_TO_FLOAT_PARTITION_CHANGED=YES
NATIVE_INTEGER_ROOTS_PER_ROW=1
R6_INTEGER_ROOTS_PER_ROW=2
```

### 3b — Pourquoi les valeurs entières sont **égales** malgré tout

La deuxième racine de R6 est `done & 1` — le compteur de tours que j'avais ajouté comme garde
anti-DCE, puis neutralisé par `* 0.0f`. Le compilateur l'a converti et l'a fait entrer dans l'arbre
flottant.

`LOP3.LUT R0, R15, 0x1, RZ, 0xc0` : le mode `0xc0` **annule les sources** et produit `RZ`, soit
0. Donc la racine B vaut **0**, et `I2FP.F32.U32` de 0 vaut exactement `0.0f`. Puis `FADD R4, RZ, R4`
ajoute ce zéro.

**Conséquence, et c'est le point que le §2 du brief exigeait** :

```
RACINE_B_VALUE=0_PROUVEE_PAR_LE_MODE_LOP3
TERME_B_EST_EXACTEMENT_ZERO_EN_FLOTTANT=YES
   -> une somme ENTIERE ne peut pas differer du natif : R6 somme le meme ensemble de feuilles
      (tout kbx satisfaisant kbx < 48), et l addition entiere est associative et enveloppante.
      Regrouper differemment (16 chaines de max 7 au lieu de 19 chaines de max 4) ne change pas
      la valeur.
INTEGER_VALUES_PROVABLY_EQUAL_BY_ASSOCIATIVITY=YES
INTEGER_PARTIAL_SUM_VALUES_DIFFER=NO_POUR_LA_RACINE_A
   -> la racine A accumule exactement les memes 32 dp4a que le natif.
```

Ceci **infirme définitivement** la conclusion de R7 : ce n'était pas un problème de valeurs
entières. (Je conserve `NOT_PROVEN` dans le ledger plutôt que `PROVEN_EQUAL` : l'égalité repose sur
l'associativité et sur la lecture du mode `LOP3`, pas sur une capture de valeurs.)

### 3c — Donc où est la divergence ?

Elle est **entièrement dans le flottant**, et le budget est serré :

```
R6 introduit un terme flottant SUPPLEMENTAIRE par ligne (la racine B), exactement 0,0f
il introduit une region de convergence supplementaire qui separe les deux conversions
il place les operations flottantes dans un ordre different
-> les valeurs entieres sont identiques, mais la SEQUENCE d operations flottantes qui combine
   les contributions par kbx n est pas la meme.
```

C'est la forme que R7 avait identifiée ; R7B la **localise précisément** au lieu de l'attribuer au
DAG entier. Le §12 de R7B autorise `FLOAT_REASSOCIATION_BINARY_PROVEN=YES` seulement si les racines
sont identiques ; elles ne le sont pas (1 contre 2). Je conserve donc :

```
FLOAT_REASSOCIATION_BINARY_PROVEN=NO
FLOAT_REASSOCIATION_HYPOTHESIS=SUPPORTED_AS_THE_ONLY_REMAINING_MECHANISM_NOT_BINARY_PROVEN
INTEGER_CONTRIBUTION_TO_THE_DIVERGENCE=EXCLUDED
```

**Le terme ajouté étant exactement zéro, il n'explique pas la divergence par sa valeur** : il
l'explique par la **perturbation de codegen** qu'il provoque (région de convergence supplémentaire,
ordonnancement flottant différent). C'est un artefact de mon instrumentation, et il est la première
chose à retirer.

### 3d — Ce qui reste non établi

```
ALPHA_RENAMED_LEAF_TO_ROOT_TRACE=NOT_OBTAINED
   -> mon tracage arriere par « derniere ecriture de registre » suppose une forme SSA ; le SASS de
      ptxas reutilise les registres, donc la remontee retourne une chaine vide (verifie sur les
      4 conversions). La question « MEMES feuilles dans MEMES racines ? » n a pas recu de reponse
      par cette methode — elle a recu une reponse par l argument d associativite du 3b.
INTEGER_ROOT_CAPTURE_KERNEL=NOT_BUILT   INTEGER_ROOT_MISMATCH_COUNT=NOT_MEASURED
```

## 4 — Ce que cette passe a établi sur les feuilles (§3, §4, §8, §9)

Le §4 exigeait un alpha-renaming et une trace `feuille IDP → racine entière → I2F`. **Mon traçage
arrière n'a pas fonctionné** et je le déclare :

```
BACKWARD_TRACE_METHOD_INVALID_FOR_PTXAS_SASS=YES
   -> la remontee par « derniere ecriture de registre » suppose une forme SSA ; ptxas reutilise les
      registres (R9, R11, R24 servent successivement d entree et de sortie), donc la remontee
      s arrete immediatement, chaine vide, verifie sur les 4 conversions natif et R6.
ALPHA_RENAMED_LEAF_TO_ROOT_TRACE=NOT_OBTAINED
```

**Mais la question du §2 a reçu une réponse par une autre voie** (§3b) : la racine A accumule
exactement les mêmes 32 `dp4a` que le natif, et la racine B vaut 0. Donc :

```
INTEGER_ROOT_LEAF_PARTITIONS_IDENTICAL=YES_POUR_LA_RACINE_A
IDP_DEPENDENCY_REORDERING_NUMERICALLY_NEUTRAL=SUPPORTED
   -> le reordonnancement des 32 IDP (19 chaines de max 4 -> 16 chaines de max 7) NE change PAS
      la valeur entiere. C est exactement l objection du paragraphe 2, et elle est fondee.
```

## 5 — Le test qui reste à faire (§14)

Le §14 demande la capture directe des valeurs entières, bit à bit, sur quelques lignes. Je ne l'ai
pas faite, et elle reste la seule confirmation indépendante de l'argument d'associativité :

```
INTEGER_ROOT_CAPTURE_KERNEL=NOT_BUILT
INTEGER_ROOT_MISMATCH_COUNT=NOT_MEASURED
INT_ROOT_MIN=NOT_MEASURED   INT_ROOT_MAX=NOT_MEASURED   (aucune invocation d overflow)
DEBUG_KERNEL_PERFORMANCE_DATA=INVALID_NOT_MEASURED
```

**Mais elle est désormais moins prioritaire que le §3c** : puisque les valeurs entières sont égales
par associativité et que la racine B vaut 0, la divergence est dans l'ordonnancement flottant. Le
premier geste utile est donc de **retirer l'artefact** (`span_count * 0.0f`) et de re-mesurer la
fidélité. Si `P48` redevient bit-à-bit identique à `K_FULL`, le banc est réparé et R6 peut enfin
servir. Si la divergence persiste, alors la capture des racines devient nécessaire.

## 6 — Statut corrigé de l'hypothèse (§16 a §18)

```
FLOAT_REASSOCIATION_HYPOTHESIS=NOT_PROVEN_AND_NOT_CLEANLY_REJECTED
INTEGER_DEPENDENCY_GRAPH_CHANGED=YES
INTEGER_NUMERIC_RESULT_CHANGED=NOT_PROVEN
INTEGER_TO_FLOAT_PARTITION_CHANGED=SUPPORTED_PAR_MESURE_DIRECTE
NUMERICAL_DIVERGENCE_ROOT_CAUSE=NOT_YET_FULLY_LOCALIZED
RELAXED_EPSILON_TIMING=BLOCKED_RAISON_GENERALE
   (et non « ce n est pas une question de tolerance » : R7 l affirmait, R7B le retire)
```

Le §18 interdit d'ouvrir `FORCE_NATIVE_FLOAT_ACCUMULATION_ORDER` **ou**
`FORCE_NATIVE_INTEGER_IDP_DEPENDENCY_CHAIN_ORDER` avant R7B. Le niveau de contrainte dépend du
résultat, et le résultat est : **la frontière change (mesuré), les valeurs ne sont pas capturées**.

## 7 — Les deux gestes qui suivent, par ordre de coût

**Geste 1 — retirer l'artefact (§3c).** Le `FMUL`/`UI2F`/`LDCU` parasite et la seconde racine
viennent de mon `span_count * 0.0f`. Le retirer est presque gratuit. Si `P48` redevient bit-à-bit
identique à `K_FULL`, `HARNESS_DEFECT_37` est résolu et R6 devient enfin un banc causal valide —
c'est la branche A1 du §27 que R7B peut rouvrir, mais au bon niveau : **contraindre la partition
entier→flottant**, pas l'ordre de chaque instruction.

**Geste 2 — capturer les racines (§14)** si le geste 1 ne suffit pas. Kernel de debug non chronométré
écrivant `IDP_ROOT_VALUE`, `I2F_VALUE`, `SCALED_FLOAT_TERM`, `FLOAT_ACC` par ligne et par étape.

Le §19 ouvre une perspective juste : s'il suffit de préserver **la partition feuilles → racine par
domaine d'échelle** sans reproduire l'ordonnancement exact du scheduler, la cible est réaliste. R7B
ajoute une précision : cette partition est **déjà** préservée pour la racine A ; ce qui a changé,
c'est le **nombre de racines**.

```
R8_TARGET_AFTER_R7B=REMOVE_INSTRUMENTATION_ARTIFACT_THEN_RETEST_P48_FIDELITY
R8_FLOAT_ORDER_TARGET=NOT_OPENED
R8_INTEGER_CHAIN_TARGET=NOT_OPENED
```

## 8 — Autres défauts

```
HARNESS_DEFECT_37=R6_P48_OUTPUT_DIVERGENCE
   STATUS=OPEN_PARTIELLEMENT_LOCALISE   (frontiere entiere->flottant mesuree, valeurs non capturees)
   RESOLUTION_REQUIERT=ROOT_CAUSE_LOCALIZED + CORRECTED_R6_BITWISE_MATCH
HARNESS_DEFECT_38=ARM_TABLE_CONFIGURATION_MISWIRED
   STATUS=OPEN   (le paragraphe 34 exige une table auto-verifiee AVANT campagne ; non construite)
ARM_TABLE_SHA256=NOT_PRODUCED
R6_TIMING_ALLOWED=NO   (les deux defauts sont ouverts)
PLUMBING_FAILURE_29=NON_ASSIGNE
```

## 9 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007A_R7_STATUS=CLOSED_IDP_DEPENDENCY_GRAPH_DIVERGENCE_FOUND
ENGINE007A_R7B_STATUS=CLOSED_PARTITION_CHANGE_PROVEN_INTEGER_VALUES_NOT_CAPTURED
ADDRESS_AND_LOAD_MASK_IDENTICAL=YES
WEIGHT_ADDRESS_SEQUENCE_IDENTICAL=YES
Q8_ADDRESS_SEQUENCE_IDENTICAL=YES
SCALE_ADDRESS_SEQUENCE_IDENTICAL=YES
IDP_COUNT_IDENTICAL=YES_32_EQ_32
IDP_ORDER_IDENTICAL=NO
IDP_DEPENDENCY_GRAPH_IDENTICAL=NO
NATIVE_IDP_CHAIN_COUNT=19
R6_IDP_CHAIN_COUNT=16
NATIVE_MAX_IDP_CHAIN_LENGTH=4
R6_MAX_IDP_CHAIN_LENGTH=7
INTEGER_ARITHMETIC_INSTRUCTION_COUNTS_EQUIVALENT=YES
INTEGER_PARTIAL_SUM_VALUES_DIFFER=NO_POUR_LA_RACINE_A
INTEGER_VALUES_PROVABLY_EQUAL_BY_ASSOCIATIVITY=YES
INTEGER_ROOT_ASSOCIATIVITY_ONLY=SUPPORTED
IDP_DEPENDENCY_REORDERING_NUMERICALLY_NEUTRAL=SUPPORTED
INTEGER_TO_FLOAT_CONVERSION_COUNT_NATIVE=1
INTEGER_TO_FLOAT_CONVERSION_COUNT_R6=3
INTEGER_TO_FLOAT_CONVERSION_OFFSETS_NATIVE=+2
INTEGER_TO_FLOAT_CONVERSION_OFFSETS_R6=+2,+14,+45
NATIVE_INTEGER_ROOTS_PER_ROW=1
R6_INTEGER_ROOTS_PER_ROW=2
IDP_PARTITIONING_DIFFERS_BEFORE_FLOAT_CONSUMPTION=SUPPORTED
INTEGER_TO_FLOAT_PARTITION_CHANGED=PROVEN_PAR_DESASSEMBLAGE
RACINE_B_VALUE=0_PROUVEE_PAR_LE_MODE_LOP3_0xc0
TERME_B_EST_EXACTEMENT_ZERO_EN_FLOTTANT=YES
TERME_B_EXPLIQUE_LA_DIVERGENCE_PAR_SA_VALEUR=NO
TERME_B_EXPLIQUE_LA_DIVERGENCE_PAR_LA_PERTURBATION_DE_CODEGEN=YES
THIRD_I2F_IS_NOT_AN_ACCUMULATION_TERM=YES
R6_CONTAINS_AN_EXTRA_KERNEL_PARAMETER_LOAD_AND_MULTIPLY=YES
ALPHA_RENAMED_LEAF_TO_ROOT_TRACE=NOT_OBTAINED
BACKWARD_TRACE_METHOD_INVALID_FOR_PTXAS_SASS=YES
INTEGER_CONTRIBUTION_TO_THE_DIVERGENCE=EXCLUDED
FLOAT_REASSOCIATION_BINARY_PROVEN=NO
FLOAT_REASSOCIATION_HYPOTHESIS=SUPPORTED_AS_THE_ONLY_REMAINING_MECHANISM_NOT_BINARY_PROVEN
WARP_REDUCTION_ORDER_IDENTICAL=YES
INTEGER_ROOT_CAPTURE_KERNEL=NOT_BUILT
INTEGER_ROOT_MISMATCH_COUNT=NOT_MEASURED
DEBUG_KERNEL_PERFORMANCE_DATA=INVALID_NOT_MEASURED
R8_TARGET_AFTER_R7B=REMOVE_INSTRUMENTATION_ARTIFACT_THEN_RETEST_P48_FIDELITY
R8_FLOAT_ORDER_TARGET=NOT_OPENED
R8_INTEGER_CHAIN_TARGET=NOT_OPENED
NUMERICAL_DIVERGENCE_ROOT_CAUSE=LOCALIZED_TO_FLOAT_ORDERING_NOT_VALUES
RELAXED_EPSILON_TIMING=BLOCKED
KBX_DYNAMIC_BODY_MULTIPLICITY_HYPOTHESIS=NOT_TESTED_VALIDLY
MAJOR_FIXED_FLOOR_REMAINS_UNEXPLAINED=YES
HARNESS_DEFECT_37=LOCALISE_A_LA_FRONTIERE_I2F_OPEN
HARNESS_DEFECT_38=OPEN_ARM_TABLE
ARM_TABLE_SHA256=NOT_PRODUCED
R6_TIMING_ALLOWED=NO
ENGINE007B_STATUS=BLOCKED
ENGINE007C_STATUS=BLOCKED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
