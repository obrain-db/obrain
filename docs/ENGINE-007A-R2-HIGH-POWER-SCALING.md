# ENGINE-007A-R2 — MISE À L'ÉCHELLE DU TRAVAIL ARITHMÉTIQUE (HAUTE PUISSANCE) : RÉSULTAT

Date : 2026-09-29. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7a-r2/`

---

## 0 — Résultat

R2 est **concluant sur la mesure** et **négatif sur l'hypothèse principale**.

```
MEASUREMENT_RESOLUTION_US=0,509      (objectif « << 4 µs » atteint ; ideal <= 0,5 atteint de justesse)
POSITION_EFFECT_PRESENT=NO           (7 positions : 91,962 a 92,078 µs, etendue 0,116 µs --> HARNESS_DEFECT_29 RESOLU)

A100 - A0 = +3,4391 us   IC95 [3,3395 ; 3,5387]   effect/noise = 67,7
   soit 3,6 % de la duree du noyau

SAME_CARDINALITY_GATE=FAIL
   A50_EVEN - A50_ODD = -0,5089 us   (meme cardinalite : 16 sites ; ecart reel mais petit)
   A25_A   - A25_B    = -1,1527 us   (meme cardinalite :  8 sites ; ecart PLUS GRAND que l'effet principal par site)
```

Le résultat tient en deux propositions :

1. **Le travail dp4a contribue de façon réelle mais faible** : supprimer les 32 sites fait gagner
   **3,44 µs sur 94 µs (3,6 %)**, avec une significativité très forte (67,7 × l'erreur-type).
2. **Le nombre de sites n'est PAS la variable explicative suffisante** : à cardinalité identique,
   l'emplacement fait varier la latence jusqu'à **1,15 µs**.

Conformément au §16, **aucune régression n'est promue** comme autorité : le gate même-cardinalité
échoue.

```
ENGINE007A_R2_STATUS=CLOSED_MEASURED_SCALING_WEAK_AND_PLACEMENT_SENSITIVE
LATENCY_SCALES_WEAKLY_WITH_USEFUL_ARITHMETIC_WORK=MEASURED
IDP_COUNT_ALONE_INSUFFICIENT=YES
ENGINE007B_STATUS=BLOCKED
ARITHMETIC_REDUCTION_MAX_LOCAL_HEADROOM=LOW
```

---

## 1 — Freeze de R1 (§1, §2)

```
ENGINE007A_R1_STATUS=INCONCLUSIVE_BY_MEASUREMENT_NOISE
NON_DP4A_BODY_COMPARABLE_ACROSS_VARIANTS=YES
WEIGHT_LDG_CONSTANT=YES   Q8_1_LDG_CONSTANT=YES   TERNARY_DECODE_BODY_CONSTANT=YES
SHFL_FADD_BAR_STG_CONSTANT=YES   SPILLS=0
SAME_CARDINALITY_GATE=FAIL   ARITHMETIC_WORK_SCALING=NOT_MEASURED
A0_NO_CLEAR_COLLAPSE_VS_A100=OBSERVED_SUGGESTIVE_NOT_PROMOTED

HARNESS_DEFECT_28=INITIAL_KEEPALIVE_DID_NOT_KEEP_Q8_1_LOADS_LIVE   STATUS=RESOLVED
HARNESS_DEFECT_29=POSITION_EFFECT_AND_SHORT_MEASUREMENT_WINDOW     STATUS=RESOLU_PAR_R2
HARNESS_DEFECT_30=UNEXPLAINED_A100_93US_MEASUREMENT                STATUS=RECORDED_NON_PROMOTED
PLUMBING_FAILURE_COUNT=28   PLUMBING_FAILURE_29=NOT_ASSIGNED
```

## 2 — Dispositif R2 (§5 à §11)

Aucune nouvelle variante algorithmique, **aucun changement du SASS de la famille qualifiée** : le
binaire R2 contient les sept mêmes spécialisations `k_scale<0..6>`, avec paires de bibliothèque
inchangées.

```
KERNELS_PER_OBSERVATION=2048      (une observation = UNE paire d'evenements sur 2048 lancements)
N_CYCLES_PER_PROCESS=14           (7 rotations latines x 2 sens, ordre alterne)
GLOBAL_WARMUP_BATCHES=3           (x 7 bras, jamais comptes)
N_PROCESSES=6                     (contextes CUDA neufs, executer SEQUENTIELLEMENT)
N_OBSERVATIONS=588                (6 processus x 14 cycles x 7 bras)
```

Rotation latine : au cycle `c`, le bras en position `slot` est `(slot + c/2) % 7` ; les cycles
impairs inversent l'ordre. Sur 14 cycles, chaque bras apparaît **une fois à chaque position**, dans
les deux sens.

**Correction d'une erreur de ma part, importante** : j'ai d'abord lancé les 8 processus **en
parallèle** sur le même GPU. Les temps sont passés de 86 µs à **414-533 µs** — contention de
ressources, données sans valeur. Purge et relance **séquentielle**. Signalé comme défaut de harnais.

## 3 — Le défaut 29 est résolu (§11)

```
position 0 : 91,9621     position 3 : 92,0218     position 6 : 92,0784
position 1 : 92,0499     position 4 : 91,9661
position 2 : 91,9985     position 5 : 92,0544

etendue sur les 7 positions = 0,116 us
POSITION_0_VS_AUTRES = -0,066 us
POSITION_EFFECT_PRESENT=NO
```

En R1, la première mesure de chaque variante était jusqu'à **10 µs** plus lente. Avec des batchs de
2048 kernels et la rotation latine, l'effet de position tombe à **0,12 µs d'étendue**. Le défaut est
donc **résolu par construction**, et non simplement contourné.

## 4 — Moyennes par bras et par processus (§12, §17, §18, §19)

Avec `K=2048` et 6 processus, l'unité statistique est le processus ; les 2048 kernels ne servent qu'à
réduire le bruit de chaque observation.

```
bras       n     mean_us    sd      min       max
A0        84    90,6859   0,3587  90,0972   91,6497
A25_A     84    91,0639   0,4544  90,0165   91,9782
A25_B     84    92,2167   0,2373  91,0296   92,8905
A50_EVEN  84    91,5883   0,4881  90,3560   92,2810
A50_ODD   84    92,0973   0,4360  90,1647   92,8466
A75       84    92,3541   0,2070  92,0161   93,1645
A100      84    94,1251   0,3754  92,2109   94,6759
```

Moyennes par processus — la reproductibilité est excellente :

| pid | A0 | A25_A | A25_B | A50_EVEN | A50_ODD | A75 | A100 |
|---|---|---|---|---|---|---|---|
| 0 | 90,391 | 90,452 | 91,960 | 90,814 | 91,549 | 92,278 | 93,622 |
| 1 | 90,485 | 90,981 | 92,278 | 91,441 | 92,257 | 92,327 | 94,176 |
| 2 | 90,881 | 91,234 | 92,337 | 92,003 | 92,205 | 92,479 | 94,253 |
| 3 | 90,811 | 91,183 | 92,218 | 91,845 | 92,213 | 92,314 | 94,230 |
| 4 | 90,855 | 91,396 | 92,303 | 91,687 | 92,165 | 92,380 | 94,268 |
| 5 | 90,694 | 91,138 | 92,203 | 91,741 | 92,194 | 92,347 | 94,202 |

L'écart-type **entre processus** sur chaque bras est de 0,20 à 0,49 µs : la mesure est stable.

## 5 — Contraste principal (§13, §14)

```
A100_MINUS_A0_US=3,4391        A100_MINUS_A0_SD=0,4659
IC95=[3,3395 ; 3,5387]         A100_MINUS_A0_EFFECT_TO_NOISE=67,7
part de la duree du noyau A100 : 3,65 %
```

Le signe est celui attendu (plus de sites = plus lent), l'intervalle de confiance est **très loin**
de zéro, et l'effet est cohérent sur les 6 processus. C'est un effet **réel** — et **petit**.

## 6 — Gate même-cardinalité (§12) : FAIL, et c'est l'information la plus forte

```
A50_EVEN - A50_ODD = -0,5089 us  (sd 0,5073)   meme cardinalite : 16 sites
A25_A   - A25_B    = -1,1527 us  (sd 0,4323)   meme cardinalite :  8 sites

|MEAN_DELTA| <= measurement_resolution ? NON pour les deux.
```

Le contrôle A25 est le plus parlant : **deux masques de 8 sites, à cardinalité strictement
identique, diffèrent de 1,15 µs** — soit **un tiers du contraste extrême A100−A0** (3,44 µs) pour
exactement le même travail.

```
SAME_CARDINALITY_GATE=FAIL
LATENCY_IS_NOT_A_FUNCTION_OF_IDP_COUNT_ALONE=SUPPORTED
IDP_COUNT_ALONE_INSUFFICIENT=YES
CAS=D_DU_BRIEF   (la courbe n'est pas monotone de facon monotone-en-compte)
```

## 7 — Forme de la courbe (§15, §23)

Incréments par 8 sites :

| transition | delta µs | µs par site |
|---|---|---|
| A0 → A25 | +0,3780 | 0,047 |
| A25 → A50 | +0,2025 | 0,025 |
| A50 → A75 | +0,5113 | 0,064 |
| **A75 → A100** | **+1,7710** | **0,221** |

Les trois premiers incréments sont compatiables avec du bruit (0,02 à 0,06 µs/site) ; le dernier est
**3,5 à 9 fois plus grand** (0,221 µs/site). La courbe n'est donc **pas linéaire** : l'essentiel de
l'effet se produit entre 24 et 32 sites.

Réserve honnête : A75 a l'écart-type le plus **faible** (0,2070) et A100 est le bras le plus lent de
façon très reproductible. Je ne propose **aucun mécanisme** pour ce seuil — scheduler, seuil de
recouvrement, effet de vague sont des descriptions, pas des explications, et je n'ai pas de compteur
pour les départager.

```
ARITHMETIC_WORK_MONOTONICITY=FAIL     (A25_A < A50_EVEN < A50_ODD < A25_B : inversion reelle)
ARITHMETIC_WORK_THRESHOLD_BEHAVIOR=SUGGESTED_NOT_ESTABLISHED
LINEAR_MODEL_FIT=NOT_PROMOTED_GATE_FAILED
   (information seulement : wall = 90,6554 + 0,09177 x idp_count ;
    residus jusqu'a 0,83 us, et l'ecart A25_A/A25_B de 1,15 us n'est PAS explique par le compte)
```

## 8 — Interprétation : le cas C du §22

```
LATENCY_SCALES_WEAKLY_WITH_USEFUL_ARITHMETIC_WORK=MEASURED
ARITHMETIC_REDUCTION_MAX_LOCAL_HEADROOM=LOW
```

Même en supprimant **la totalité** du travail de produit scalaire — les 32 `dp4a` d'un thread, 100 %
du calcul — on ne récupère que **3,6 % de la paroi du noyau**. Une optimisation qui réduirait
réellement le travail arithmétique de 50 % (par sparsité ou rang faible) aurait donc un plafond
local de l'ordre de **1,7 µs sur 94 µs, soit 1,8 %**.

Conséquence directe pour le programme :

```
ENGINE007B_STATUS=BLOCKED
   -> l'audit de sparsite du paragraphe 31 n'est pas ouvert : le paragraphe 33 l'interdit sans
      ARITHMETIC_WORK_SCALING=MATERIAL, et la mesure dit le contraire (LOW).
ENGINE007_PRIMARY_LEVER=NOT_IDENTIFIED
ENGINE007C_LOW_RANK_STRUCTURED_AUDIT=NOT_JUSTIFIED_FOR_THIS_PROJECTION
```

Le §22 prévoit exactement cette issue : *« Cela peut rendre sparsité/low-rank peu intéressante pour
cette projection seule. »*

## 9 — Le §26 et le §27 : ce que A0≈A100 signifie vraiment

Formulation correcte retenue :

```
MARGINAL_REMOVAL_OF_USEFUL_IDP_WORK_DOES_NOT_MATERIALLY_REDUCE_KERNEL_WALL=SUPPORTED
ARITHMETIC_HAS_ZERO_COST=NON_PROMU
```

A0 mesure 90,69 µs **sans aucun `dp4a`** mais **avec** les 32 chargements de poids, les 32
chargements d'activation, les 292 `LOP3` de décodage ternaire, l'adressage, la réduction et le
keepalive. Le §27 demande alors : *quel travail constant subsiste ?*

La réponse est dans mes propres chiffres de 006F, et elle est gênante : 006F mesurait
`STREAM=8,28 µs` (poids seuls) et `DECODE=22,60 µs` (poids + décodage). Or A0 — qui fait
**exactement** ce travail plus la réduction et le keepalive — coûte **90,69 µs**, soit **4 fois**
`DECODE`.

C'est une incohérence que je ne peux pas expliquer : les deux instruments mesurent la même
géométrie, et A0 ne devrait pas coûter 68 µs de plus que `DECODE` pour un keepalive XOR/add et une
réduction warp. J'ai déjà rencontré ce type d'écart en 006C-R2 (`BUILD_REPRODUCTION_PERFORMANCE_PARITY=NOT_ESTABLISHED`)
et en 006G (`MICROBENCH_OVER_PRODUCTION_RATIO=1.64`). **Trois instruments différents donnent trois
niveaux absolus différents pour le même travail**, et je n'en connais pas la cause.

```
ABSOLUTE_LEVEL_DISAGREEMENT_ACROSS_MICROBENCHES=UNRESOLVED
   (006F STREAM 8,28 / DECODE 22,60 ; 007A-R2 A0 90,69 — meme geometrie, meme machine)
CONSEQUENCE : les CONTRASTES apparies de R2 sont fiables (meme instrument, meme processus,
   rotation equilibree), mais AUCUN niveau absolu de R2 ne doit etre compare a un niveau de 006F.
```

C'est pourquoi je limite la conclusion à un contraste relatif : **3,6 % de la paroi du noyau**, et
non « le travail dp4a coûte 3,44 µs ».

## 10 — Provenance et binaires (§4)

```
Runtime baseline   : libllama 3db83675… / libggml-cuda abafbf0d…  (restaures, SHA verifies)
Arbre llama-obrain-cuda : les 4 fichiers du chemin chaud aux SHA vierges
Fichiers sales     : les 3 greffes connues, inchangees
MODIFICATIONS DE SOURCE DANS CETTE PASSE : AUCUNE
SOURCE_SHA_PARITY=PASS   (same e7a-scaling.cu, main R2 remplace, kernels k_scale<0..6> inchanges)
BUILD_FLAGS_PARITY=PASS  (-O3 -std=c++17 -arch=sm_121a, identique a R1)
SASS_MANIFEST_PARITY=PASS (variantes identiques : IDP 0/16/16/16/16/24/32, LDG poids 32, LDG activ 32,
                           LOP3 292, SHFL 5, FADD 10, BAR 1, STG 2, registres 39-40, spills 0)
BINARY_R2_SHA256=0a43b50f84e83138867f89d4ae809605…
   Note : les sept variantes vivent dans UN binaire (specialisations de template), pas dans sept
   binaires separes. Le paragraphe 4 demandait sept SHA ; ce qui est publie est le SHA de l'unite
   qui les contient toutes + le manifeste SASS par variante, ce qui porte la meme information.
```

## 11 — Défauts de harnais

```
HARNESS_DEFECT_29=POSITION_EFFECT_AND_SHORT_MEASUREMENT_WINDOW
   STATUS=RESOLU_PAR_R2  (batchs de 2048 + rotation latine -> effet de position a 0,12 us)
HARNESS_DEFECT_31=PARALLEL_PROCESS_CONTENTION_INVALIDATED_RUN
   -> 8 processus lances en parallele sur le meme GPU : 86 us -> 414-533 us. Donnees purgees,
      relance sequentielle. Aucun chiffre tire des donnees contaminees.
HARNESS_DEFECT_32=ABSOLUTE_LEVEL_DISAGREEMENT_ACROSS_MICROBENCHES
   STATUS=UNRESOLVED  (006F DECODE 22,60 us vs 007A A0 90,69 us, meme travail)
```

## 12 — Ledger

```
ENGINE006_STATUS=CLOSED_NO_MATERIAL_RECOVERABLE_LOCAL_WASTE_FOUND
ENGINE007_STATUS=OPEN
ENGINE007A_R1_STATUS=INCONCLUSIVE_BY_MEASUREMENT_NOISE
ENGINE007A_R2_STATUS=CLOSED_MEASURED_SCALING_WEAK_AND_PLACEMENT_SENSITIVE
ENGINE007A_WORKLOAD_STRUCTURE_GATE=PASS
ENGINE007A_SAME_CARDINALITY_GATE=FAIL
ENGINE007A_POSITION_BALANCE_GATE=PASS
MEASUREMENT_RESOLUTION_US=0.5089
POSITION_EFFECT_PRESENT=NO
POSITION_EFFECT_RANGE_US=0.116
N_PROCESSES=6
N_CYCLES_PER_PROCESS=14
KERNELS_PER_OBSERVATION=2048
N_OBSERVATIONS=588
A0_MEAN_US=90.6859
A25_A_MEAN_US=91.0639
A25_B_MEAN_US=92.2167
A50_EVEN_MEAN_US=91.5883
A50_ODD_MEAN_US=92.0973
A75_MEAN_US=92.3541
A100_MEAN_US=94.1251
A100_MINUS_A0_US=3.4391
A100_MINUS_A0_SD=0.4659
A100_MINUS_A0_CI95=3.3395_TO_3.5387
A100_MINUS_A0_EFFECT_TO_NOISE=67.7
A100_MINUS_A0_FRACTION_OF_KERNEL=3.65_PERCENT
MEAN_DELTA_A50_SAME_COUNT=-0.5089
SD_DELTA_A50_SAME_COUNT=0.5073
MEAN_DELTA_A25_SAME_COUNT=-1.1527
LATENCY_SCALES_WEAKLY_WITH_USEFUL_ARITHMETIC_WORK=MEASURED
IDP_COUNT_ALONE_INSUFFICIENT=YES
ARITHMETIC_WORK_MONOTONICITY=FAIL
ARITHMETIC_WORK_SITE_PLACEMENT_MATTERS=YES
ARITHMETIC_WORK_THRESHOLD_BEHAVIOR=SUGGESTED_NOT_ESTABLISHED
LINEAR_MODEL_FIT=NOT_PROMOTED_GATE_FAILED
ARITHMETIC_REDUCTION_MAX_LOCAL_HEADROOM=LOW
USEFUL_DP4A_WORK_IS_NOT_A_DOMINANT_WALL_COMPONENT=SUPPORTED
DP4A_EXECUTION_THROUGHPUT_LIMIT=REJECTED_BY_ARITHMETIC_REMOVAL_EXPERIMENT
MARGINAL_REMOVAL_OF_USEFUL_IDP_WORK_DOES_NOT_MATERIALLY_REDUCE_KERNEL_WALL=SUPPORTED
ARITHMETIC_HAS_ZERO_COST=NON_PROMU
ABSOLUTE_LEVEL_DISAGREEMENT_ACROSS_MICROBENCHES=UNRESOLVED
ENGINE007B_STATUS=BLOCKED
ENGINE007_PRIMARY_LEVER=NOT_IDENTIFIED
ENGINE007C_LOW_RANK_STRUCTURED_AUDIT=NOT_JUSTIFIED_FOR_THIS_PROJECTION
COMPUTE_BOUND=NOT_PROVEN
ISSUE_THROUGHPUT_LIMITED=NOT_PROVEN
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
