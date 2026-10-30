# ENGINE-007A-R10 — CHRONOMÉTRAGE DE LA MULTIPLICITÉ KBX NATIVE

Date : 2026-09-29. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7a-r10/`

---

## 0 — Résultat

```
ENGINE007A_R10_STATUS=CLOSED_KBX_COUNT_NOT_DOMINANT

RUNTIME_LOOP_TIMING_EQUIVALENCE=PASS
KBX_SAME_CARDINALITY_PRIMARY_GATE=PASS
KBX_COUNT_MONOTONICITY=FAIL
KBX_DYNAMIC_BODY_MULTIPLICITY=NOT_DOMINANT

RETIRER 75 % DES CORPS KBX NATIFS (48 -> 12 blocs) NE REND QUE 2,13 us SUR 84,08 us = 2,54 %
```

**L'hypothèse majeure ouverte depuis R4 est réfutée** : la multiplicité dynamique des corps kbx
n'explique pas le plancher fixe. Et le résultat converge avec R2-SANE, qui avait mesuré **3,22 %**
pour le retrait de *toute* l'arithmétique de produit scalaire. Deux expériences indépendantes, deux
leviers différents, tous deux **faibles**.

```
ARM_TABLE_SHA256_POST_RUN=e71b8358c80333f7526dc55c40bfcd5a92dd20fe99cfb1b6ba652bcd470a5ba7
   == ARM_TABLE_PRE_RUN  ->  R10_TIMING_RESULT=VALID
INPUT_SANITY_GATE=PASS (5/5 processus)      OUTPUT_SANITY_GATE=PASS (12/12 bras)
N_OBSERVATIONS=1440      N_PROCESSES=5      KERNELS_PER_BATCH=2048      N_CYCLES=24 (12 bras)
```

---

## 1 — Qualification figée (§0)

```
ENGINE007A_R10_STATUS=ARM_TABLE_QUALIFICATION_DONE_TIMING_PENDING
HARNESS_DEFECT_38=RESOLVED
ARM_TABLE_SHA256=e71b8358c80333f7526dc55c40bfcd5a92dd20fe99cfb1b6ba652bcd470a5ba7
ARM_TABLE_SELF_CHECK=PASS   ARM_TABLE_HOST_SIMULATION=PASS   GPU_KBX_QUALIFICATION=PASS
UNEXPECTED_DUPLICATE_CONFIG_COUNT=0
KBX48_NATIVE_KBX_SEQUENCE_MATCH_K_FULL=YES   KBX48_NATIVE_OUTPUT_MISMATCH=0
ENGINE007A_R10_TIMING_ALLOWED=YES
KERNEL_DEVICE_CODE_PARITY=PASS_BY_COMPLETE_SASS_MANIFEST   CUBIN_IDENTITY=NOT_RECORDED
```

La table a été recalculée **après** la campagne (§29) : empreinte identique. Aucun réglage n'a été
modifié pendant la campagne (§30) : masques, taille de lot, warmup et nombre de processus sont
restés figés.

Diagnostic de position (§27) :

```
MEAN_US_BY_POSITION_RANGE=0.1457 us   (12 positions : 82,459 a 82,605)
```
```
TIMING_CAMPAIGN_GATE=PASS
```

## 2 — Étage 1 : le plein runtime vaut-il le plein natif ? (§4, §5)

| | moyenne | sd | n |
|---|---|---|---|
| `K_FULL_NATIVE` | **84,3000** | 0,7469 | 120 |
| `KBX48_NATIVE` | **84,0828** | 0,6557 | 120 |

Moyennes par processus :

| pid | `K_FULL_NATIVE` | `KBX48_NATIVE` |
|---|---|---|
| 0 | 83,367 | 83,090 |
| 1 | 84,283 | 84,206 |
| 2 | 84,308 | 84,165 |
| 3 | 84,761 | 84,394 |
| 4 | 84,781 | 84,559 |

```
FULL_RUNTIME_MINUS_NATIVE_US=-0,2173    sd=0,6125    IC95=[-0,3269 ; -0,1077]
FULL_RUNTIME_OVER_NATIVE_RATIO=0,99742
```

**Interprétation.** L'écart est **statistiquement réel** (l'IC95 exclut zéro) mais
**numériquement négligeable** : **0,22 µs sur 84,3 µs, soit 0,26 %**, très en dessous de toute
résolution de mesure utilisée dans ce programme (0,509 µs en R2-SANE ; le diagnostic de position
lui-même vaut 0,15 µs).

Conformément au §5, j'applique le critère `abs(écart) <= measurement_resolution` avec
`measurement_resolution=0,50 µs` :

```
RUNTIME_LOOP_TIMING_EQUIVALENCE=PASS
   -> la famille runtime est un proxy causal propre du noyau natif.
   -> le bras plein runtime est marginalement PLUS RAPIDE que le natif, dans le mauvais sens pour
      un effet de codegen : le test de masque et le bloc de reconvergence supplementaires ne
      coutent rien de mesurable.
RUNTIME_CONTROL_CODEGEN_EFFECT=NOT_MEASURED
CAS_D_DU_PARAGRAPHE_23=NE_S_APPLIQUE_PAS
```

## 3 — Étage 2 : à cardinalité fixe, l'emplacement change-t-il la paroi ? (§6 à §9)

### Contrôle principal : `KBX12` (12 blocs, 48 kbx dynamiques, 16/16/8/8)

| arme | moyen | sd |
|---|---|---|
| `KBX12_SET_A` | **82,0205** | 0,2979 |
| `KBX12_SET_B` | **81,9145** | 0,2060 |
| `KBX12_SET_C` | **81,9151** | 0,2454 |

```
KBX12_RANGE_US=0,1060

A - B = +0,1060   IC95=[+0,0679 ; +0,1441]
A - C = +0,1054   IC95=[+0,0660 ; +0,1449]
B - C = -0,0006   IC95=[-0,0304 ; +0,0293]
```

```
KBX12_PLACEMENT_EFFECT=NOT_MATERIALLY_DETECTED
KBX_SAME_CARDINALITY_PRIMARY_GATE=PASS
```
```
CARDINALITY_MATCH=YES    WARP_LOAD_BALANCE_MATCH=YES    PLACEMENT_DIFFERS=YES
```

L'écart A/B et A/C est statistiquement détectable (0,11 µs) mais **cinq fois sous la résolution de
mesure**. B et C sont **indiscernables** (IC95 contenant zéro). Aucun effet d'emplacement matériel
à cette cardinalité.

### Contrôle secondaire : `KBX24` (24 blocs, 96 dynamiques, 32/32/16/16)

```
KBX24_A - KBX24_B = +0,1021   IC95=[+0,0331 ; +0,1710]
KBX24_PLACEMENT_EFFECT=DETECTED_BUT_IMMATERIAL
```

Détectable, mais de la même amplitude que `KBX12` : **0,10 µs sur 82 µs, soit 0,12 %**. Je ne le
promus pas comme matériel.

### `KBX08` reste descriptif (§10)

```
KBX08_A - KBX08_B = -0,0034
KBX08_WARP_BALANCE_CONFOUND=YES (12/8/4/8 contre 12/12/4/4)
   -> descriptif seulement, aucune preuve primaire tirée.
```

### `KBX06` (§11)

```
KBX_COUNT_PER_WARP_A=8/8/4/4    KBX_COUNT_PER_WARP_B=8/8/4/4   -> IDENTIQUES
KBX06_SAME_CARDINALITY_CONTROL_VALID=YES
KBX06_A - KBX06_B = -0,0915
```

Décidé **avant** observation, comme le §11 l'exige.

## 4 — Étage 3 : la paroi échelle-t-elle avec le nombre de kbx ? (§12 à §17)

Le gate de placement primaire passe, donc la courbe est construite.

| cardinalité | kbx dynamiques | moyenne µs | sd | IC95 | par warp |
|---|---|---|---|---|---|
| 6 | 24 | **82,0321** | 0,4000 | [81,982 ; 82,083] | 8/8/4/4 |
| 12 | 48 | **81,9501** | 0,2575 | [81,923 ; 81,977] | 16/16/8/8 |
| 24 | 96 | **82,1401** | 0,2432 | [82,109 ; 82,171] | 32/32/16/16 |
| 36 | 180 | **84,0252** | 1,2697 | [83,798 ; 84,252] | 56/60/32/32 |
| 48 | 192 | **84,0828** | 0,6557 | [83,965 ; 84,200] | 64/64/32/32 |

(`K_FULL_NATIVE` = 84,3000 est affiché séparément, **non inclus** dans la régression, §13.)

### Monotonie (§15)

```
T6=82,0321  T12=81,9501  T24=82,1401  T36=84,0252  T48=84,0828

T6  -> T12 : -0,0821 us
T12 -> T24 : +0,1901 us
T24 -> T36 : +1,8850 us
T36 -> T48 : +0,0576 us

KBX_COUNT_MONOTONICITY=FAIL
```

La courbe est **plate de 6 à 24 blocs**, puis **saute de 1,89 µs entre 24 et 36**, puis **redevient
plate de 36 à 48**. Coût par bloc ajouté, par tranche :

| tranche | coût par bloc |
|---|---|
| 6 → 12 | **+0,0137 µs/bloc** (négatif) |
| 12 → 24 | +0,0158 µs/bloc |
| **24 → 36** | **+0,1571 µs/bloc** |
| 36 → 48 | +0,0048 µs/bloc |

**Un seul intervalle porte tout le coût.** Je ne propose **aucun mécanisme** pour ce seuil : les
effets de vague, la saturation du planificateur ou un seuil de recouvrement sont des descriptions,
pas des explications, et je n'ai pas de compteur pour les départager.

### Régression (§16, §17) — construite car les deux gates le permettent, à interpréter avec réserve

```
wall = 81,3685 + 0,05863 x KBX_COUNT              R2=0,83124   MAX_RESIDUAL=0,6356 us
   residus : +0,312  -0,122  -0,636  +0,546  -0,100

wall = 81,3489 + 0,01386 x DYNAMIC_KBX_COUNT      R2=0,90659   MAX_RESIDUAL=0,5396 us

NATIVE_RUNTIME_KBX_SLOPE_US_PER_KBX=0,05863
SLOPE_US_PER_DYNAMIC_KBX=0,01386
```

La monotonie échouant, ces pentes **ne sont pas promues comme le modèle de la relation**. Le
`R2` modeste (0,83 / 0,91) et des résidus de 0,64 µs — six fois la résolution de mesure — disent la
même chose : une droite ne décrit pas ces données.

## 5 — Effet principal : les contrastes qui répondent à la question (§19)

```
KBX48 - KBX12 (A) = +2,0622   IC95=[+1,9648 ; +2,1597]   2,45 % du noyau plein
KBX48 - KBX12 (B) = +2,1682   IC95=[+2,0688 ; +2,2676]   2,58 %
KBX48 - KBX06 (A) = +2,0964   IC95=[+2,0172 ; +2,1756]   2,49 %
KBX48 - KBX06 (B) = +2,0049   IC95=[+1,9080 ; +2,1017]   2,38 %
```

```
DOES_REMOVING_LARGE_AMOUNTS_OF_NATIVE_KBX_WORK_MATERIALLY_REDUCE_WALL? = NO
   retirer 75 % des blocs (48 -> 12) rend 2,13 us sur 84,08 us = 2,54 %
   retirer 87,5 % des blocs (48 -> 6) rend 2,05 us sur 84,08 us = 2,44 %
```

**Passer de 12 à 6 blocs ne rend rien** (les deux contrastes 48→6 et 48→12 se recouvrent). Le
travail kbx retiré au-delà de 75 % est entièrement gratuit.

## 6 — Décision (§20 à §22, §32)

`Case B` du §21 s'applique sans ambiguïté :

```
KBX_DYNAMIC_BODY_MULTIPLICITY=NOT_DOMINANT
   -> l'hypothese du grand plancher fixe de R4 est refutee.
ARITHMETIC_WORK_SCALING_WITH_KBX_COUNT=NOT_ESTABLISHED
   -> la monotonie echoue et la regression a un R2 modeste ; aucune pente n'est promue.
ENGINE007_NEXT_STEP=DECOMPOSE_ONE_NATIVE_KBX_BODY=NON_JUSTIFIE_PAR_CES_DONNEES
   -> le paragraphe 20 ne l'autorise que dans le cas A. Il ne s'applique pas.
```

Point qui mérite d'être dit nettement : **décomposer un corps kbx n'a plus de sens** au vu de ces
chiffres. Le corps entier vaut en moyenne `2,13 / 36 = 0,059 µs` par bloc retiré, et même cette
moyenne est portée par un seul intervalle. Aucune composante interne ne peut rendre davantage que
le tout.

## 7 — Mise en regard avec R2-SANE : deux leviers, deux résultats faibles

| expérience | ce qui est retiré | gain | part |
|---|---|---|---|
| **R2-SANE** | les 32 sites `dp4a` (arithmétique de produit scalaire seule) | 2,93 µs | **3,22 %** |
| **R10** | 75 % des corps kbx natifs (chargements + échelle + décodage + adressage + `dp4a`) | 2,13 µs | **2,54 %** |

**Le tout vaut moins que la partie.** Retirer l'intégralité du corps répété — qui contient
l'arithmétique *et* tout le reste — rend **moins** que retirer la seule arithmétique dans
l'expérience R2. Ratio R10/R2-SANE = **0,79**.

Ce n'est pas une contradiction : les deux expériences portent sur des noyaux différents (famille A
avec keepalive contre famille kbx native). Mais cela établit solidement que **ni le nombre de sites
`dp4a`, ni le nombre de corps kbx ne dominent la paroi de ~84 µs**, et que R10 reste une
caractérisation de la famille runtime, non une mesure du coût par passe native.

**Réserve importante, conforme au §23** : l'étage 1 a montré que je ne peux pas dériver directement
un coût par passe native depuis la famille runtime. Mais ce point est ici **secondaire** : comme
aucun levier de cardinalité n'apparaît de toute façon, l'impossibilité de dériver une pente native
ne change pas la conclusion.

## 8 — §24 : ne pas mélanger les deux variables

```
R2-SANE_VARIABLE=IDP_SITE_COUNT          -> REMOVE_ALL_IDP_WALL_EFFECT=3,22 %
R10_VARIABLE=KBX_BODY_COUNT              -> REMOVE_75_PERCENT_KBX_WALL_EFFECT=2,54 %
LES_DEUX_SONT_DES_EXPERIENCES_DIFFERENTES=YES
```

Aucune contradiction avec R2 : les deux mesures sont faibles et cohérentes entre elles.

## 9 — Blocages et état

```
ENGINE007B_STATUS=BLOCKED
ENGINE007C_STATUS=BLOCKED
   -> toujours bloques. Deux leviers mesures, deux effets faibles ; aucun levier structurel
      convaincant n'existe a ce stade. Le paragraphe 31 est respecte : aucun bras spectaculaire
      n a fait lever le blocage.
ENGINE007_PRIMARY_LEVER=NOT_IDENTIFIED
MAJOR_FIXED_FLOOR_REMAINS_UNEXPLAINED=YES
   -> R4 avait identifie une hypothese, R10 la refute. Le plancher reste inexplique, mais une
      hypothese de moins.
KBX_DYNAMIC_BODY_MULTIPLICITY_HYPOTHESIS=REJECTED_BY_VALID_TEST
```

Le prochain axe ne peut pas être un huitième réglage local : les deux leviers de cardinalité
mesurables sont épuisés.

## 10 — Provenance

```
Runtime baseline   : libllama 3db83675… / libggml-cuda abafbf0d…  (restaures, SHA verifies)
Arbre llama-obrain-cuda : les 4 fichiers du chemin chaud aux SHA vierges
Fichiers sales     : les 3 greffes connues, inchangees
MODIFICATIONS DE SOURCE DANS CETTE PASSE : AUCUNE (microbench autonome)
Instrument         : qualification/e7a-r10/e7a-r10.cu + bin/e7a-r10
   Douze bras, DEUX noyaux seulement (natif et runtime a masque) : les onze bras a masque
   partagent le meme code, seul le masque runtime change.
   CODEGEN_CONSTANT_ACROSS_PARTIAL_ARMS=YES
   ADDITIONAL_KEEPALIVE=NONE   ANTI_HOIST_EXPRESSION=NONE   ZERO_VALUED_GUARD=NONE
GENERATEUR=SAIN_QUALIFIE_R9A_R2_SANE (echelles fp16 finies normales non nulles)
CANONICAL_MICROBENCH_TIMING_PROTOCOL=K2048_WARMUP3_LATIN_MULTI_PROCESS  (respecte a la lettre)
AUCUN REGLAGE MODIFIE EN COURS DE CAMPAGNE
PLUMBING_FAILURE_29=NON_ASSIGNE
```

## 11 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007A_R2_SANE_STATUS=CLOSED_CONCLUSIVE_REPLICATED_ON_VALID_INPUTS
ENGINE007A_R10_STATUS=CLOSED_KBX_COUNT_NOT_DOMINANT
ARM_TABLE_SHA256_PRE_RUN=e71b8358c80333f7526dc55c40bfcd5a92dd20fe99cfb1b6ba652bcd470a5ba7
ARM_TABLE_SHA256_POST_RUN=e71b8358c80333f7526dc55c40bfcd5a92dd20fe99cfb1b6ba652bcd470a5ba7
R10_TIMING_RESULT=VALID
INPUT_SANITY_GATE=PASS
OUTPUT_SANITY_GATE=PASS
N_OBSERVATIONS=1440
N_PROCESSES=5
N_CYCLES_PER_PROCESS=24
KERNELS_PER_BATCH=2048
MEAN_US_BY_POSITION_RANGE=0.1457
TIMING_CAMPAIGN_GATE=PASS
K_FULL_NATIVE_MEAN_US=84.3000
KBX48_NATIVE_MEAN_US=84.0828
FULL_RUNTIME_MINUS_NATIVE_US=-0.2173
FULL_RUNTIME_MINUS_NATIVE_CI95=[-0.3269,-0.1077]
FULL_RUNTIME_OVER_NATIVE_RATIO=0.99742
RUNTIME_LOOP_TIMING_EQUIVALENCE=PASS
RUNTIME_CONTROL_CODEGEN_EFFECT=NOT_MEASURED
CAS_D_NE_S_APPLIQUE_PAS=YES
KBX12_SET_A_MEAN_US=82.0205
KBX12_SET_B_MEAN_US=81.9145
KBX12_SET_C_MEAN_US=81.9151
KBX12_RANGE_US=0.1060
KBX12_PAIRWISE_AB_US=+0.1060
KBX12_PAIRWISE_AC_US=+0.1054
KBX12_PAIRWISE_BC_US=-0.0006
KBX12_PLACEMENT_EFFECT=NOT_MATERIALLY_DETECTED
KBX_SAME_CARDINALITY_PRIMARY_GATE=PASS
KBX24_A_MINUS_B_US=+0.1021
KBX24_A_MINUS_B_CI95=[+0.0331,+0.1710]
KBX24_PLACEMENT_EFFECT=DETECTED_BUT_IMMATERIAL
KBX06_SAME_CARDINALITY_CONTROL_VALID=YES
KBX06_A_MINUS_B_US=-0.0915
KBX08_WARP_BALANCE_CONFOUND=YES_RESULTS_DESCRIPTIVE_ONLY
KBX08_A_MINUS_B_US=-0.0034
T6_US=82.0321
T12_US=81.9501
T24_US=82.1401
T36_US=84.0252
T48_US=84.0828
KBX_COUNT_MONOTONICITY=FAIL
KBX_COUNT_CRITICAL_INTERVAL=24_TO_36_+1.8850_US
KBX_COST_PER_BLOCK_6_12=+0.0137
KBX_COST_PER_BLOCK_12_24=+0.0158
KBX_COST_PER_BLOCK_24_36=+0.1571
KBX_COST_PER_BLOCK_36_48=+0.0048
NATIVE_RUNTIME_KBX_SLOPE_US_PER_KBX=0.05863
SLOPE_US_PER_DYNAMIC_KBX=0.01386
KBX_COUNT_FIT_R2=0.83124
KBX_COUNT_FIT_MAX_RESIDUAL_US=0.6356
KBX_COUNT_FIT_PROMOTED=NO_MONOTONICITY_FAILED
KBX48_MINUS_KBX12_US=+2.1327
KBX48_MINUS_KBX12_PERCENT=2.54
KBX48_MINUS_KBX06_US=+2.0507
KBX48_MINUS_KBX06_PERCENT=2.44
KBX_DYNAMIC_BODY_MULTIPLICITY=NOT_DOMINANT
MAJOR_FIXED_FLOOR_ROOT_CAUSE=REPEATED_NATIVE_KBX_BODY_WORK=REJECTED_BY_VALID_TEST
KBX_DYNAMIC_BODY_MULTIPLICITY_HYPOTHESIS=REJECTED_BY_VALID_TEST
R4_HISTORICAL_PREDICTION=5.2808_US_PER_MISSING_PASS
R4_HISTORICAL_PREDICTION_STATUS=HISTORICAL_FALSIFIABLE_REFERENCE_ONLY_FALSIFIED_BY_R10
R2_SANE_LEVER_EFFECT=3.22_PERCENT
R10_LEVER_EFFECT=2.54_PERCENT
R10_OVER_R2_SANE_RATIO=0.79
BOTH_CARDINALITY_LEVERS_MEASURED_AND_WEAK=YES
ENGINE007_PRIMARY_LEVER=NOT_IDENTIFIED
MAJOR_FIXED_FLOOR_REMAINS_UNEXPLAINED=YES
ENGINE007B_STATUS=BLOCKED
ENGINE007C_STATUS=BLOCKED
HARNESS_DEFECT_37=RESOLVED
HARNESS_DEFECT_38=RESOLVED
HARNESS_DEFECT_40=RESOLVED_R9A
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
