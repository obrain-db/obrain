# ENGINE-007A-R3 — RÉCONCILIATION DES NIVEAUX ABSOLUS

Date : 2026-09-29. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7a-r3/`

---

## 0 — Résultat

La matrice 2×2 du §23 est mesurée, et elle tranche.

```
noyau            protocole_006F    protocole_R2
A_006F           79,9392           82,3198
B_A100           88,4909           88,9729

HARNESS_EFFECT (noyau A) : +2,3806 us      HARNESS_EFFECT (noyau B) : +0,4820 us
KERNEL_EFFECT  (proto 006F) : +8,5517 us   KERNEL_EFFECT  (proto R2) : +6,6531 us
INTERACTION : -1,8986 us
```

L'**effet noyau (8,5 / 6,7 µs) domine l'effet harnais (2,4 / 0,5 µs) par un facteur ~7**.

```
ABSOLUTE_LEVEL_DISAGREEMENT_ROOT_CAUSE=KERNEL_X_HARNESS_INTERACTION
   -> CAS C du paragraphe 24, avec effet NOYAU DOMINANT
CROSS_EXPERIMENT_ABSOLUTE_TIMING_COMPARABILITY=NOT_ESTABLISHED_POUR_LES_COMPARAISONS_INTER-NOYAUX
CANONICAL_MICROBENCH_TIMING_PROTOCOL=DEFINED
```

Et un contrôle capital : **le noyau A reproduit 006F à 0,01 µs près** (79,9392 contre 79,93). Le
désaccord n'est donc **pas** un problème de mes instruments : ils mesurent fidèlement. Ce sont les
**noyaux** qui ne font pas le même travail.

---

## 1 — Freeze R2 (§1 à §4)

```
ENGINE007A_R2_STATUS=CLOSED_CONCLUSIVE_NONLINEAR_SITE_DEPENDENT_RESULT
POSITION_EFFECT_PRESENT=NO
HARNESS_DEFECT_29=RESOLVED_BY_CONSTRUCTION
MEASUREMENT_RESOLUTION_US=0.509
A100_MINUS_A0_US=3.4391   CI95=[3.3395,3.5387]   EFFECT_TO_NOISE=67.7
FULL_IDP_REMOVAL_WALL_EFFECT_PERCENT=3.65
A50_EVEN_MINUS_A50_ODD_US=-0.5089   A25_A_MINUS_A25_B_US=-1.1527
SAME_CARDINALITY_GATE=FAIL
IDP_COUNT_ALONE_INSUFFICIENT=YES
ARITHMETIC_WORK_SITE_PLACEMENT_MATTERS=YES
ARITHMETIC_WORK_MONOTONICITY=FAIL
LINEAR_MODEL_FIT=NOT_PROMOTED
LATENCY_SCALES_LINEARLY_WITH_USEFUL_ARITHMETIC_WORK=NON_PROMU

USEFUL_DP4A_WORK_IS_NOT_A_DOMINANT_WALL_COMPONENT=SUPPORTED
MARGINAL_REMOVAL_OF_USEFUL_IDP_WORK_DOES_NOT_MATERIALLY_REDUCE_KERNEL_WALL=SUPPORTED
DP4A_EXECUTION_THROUGHPUT_LIMIT=REJECTED_BY_ARITHMETIC_REMOVAL_EXPERIMENT
ARITHMETIC_HAS_ZERO_COST=NO          (les IDP.4A ont un effet reel, simplement faible)
PARTIAL_ARITHMETIC_REDUCTION_GAIN=NOT_DERIVABLE_FROM_CARDINALITY_ALONE
```

Le §3 a raison sur le fond : la courbe étant non linéaire et dépendante du placement, **aucune
extrapolation** du type « 50 % du travail → 1,8 % de gain » n'est faite. On sait seulement que
retirer **les 32** sites rend 3,44 µs, et que deux configurations de cardinalité identique peuvent
différer de plus de 1,15 µs.

## 2 — La matrice 2×2 (§22, §23) : mesure directe

Deux noyaux × deux protocoles, **mêmes données, même machine, même session** :

| noyau | protocole 006F (K=20, warmup 1) | protocole R2 (K=2048, warmup 3) |
|---|---|---|
| **A** = `k_full` de 006F, sans keepalive | **79,9392** | 82,3198 |
| **B** = A100 de R2, avec keepalive explicite | 88,4909 | **88,9729** |

Décomposition :

```
HARNESS_EFFECT=+0,48_a_+2,38_us        (le protocole de chronometrage)
KERNEL_EFFECT=+6,65_a_+8,55_us         (le noyau lui-meme)
INTERACTION=-1,90_us
```

**Le protocole de chronométrage n'est pas la cause principale.** Il contribue au plus 2,4 µs, quand
l'écart à expliquer entre 80 et 94 µs est de 14 µs.

## 3 — Contrôle de reproduction : les instruments sont fidèles

```
006F   k_full mesurait  79,93 us
R3     noyau A mesure   79,9392 us    ecart = +0,0092 us
```

Le noyau A de cette passe est la **réimplémentation** du `k_full` de 006F, et il retombe sur la même
valeur à **0,01 µs près**, deux passes plus tard, dans un binaire différent. Les instruments sont
donc reproductibles.

## 4 — Le balayage de longueur de batch (§18, §19) : effet faible et borné

| K | noyau A (µs) | noyau B (µs) |
|---|---|---|
| 1 | 84,576 | 90,656 |
| 4 | 82,560 | 89,256 |
| 20 | 81,939 | 89,550 |
| 64 | 82,266 | **92,842** |
| 256 | 82,491 | 90,018 |
| 2048 | 82,477 | 90,016 |
| 8192 | 82,547 | 90,095 |

Hors `K=1` (surcoût de lancement) et l'outlier `K=64` du noyau B, la latence par kernel est **plate**
de K=20 à K=8192, avec un plateau à ~82,5 µs (noyau A) et ~90,0 µs (noyau B).

```
PER_KERNEL_LATENCY_DEPENDS_ON_BATCH_LENGTH=NO_POUR_K>=20
   (hors surcout de lancement a K=1, et un outlier non explique a K=64 sur le noyau B)
```

## 5 — Le balayage de warmup (§20) : aucun effet

| warmup | noyau A (µs) | noyau B (µs) |
|---|---|---|
| 0 | 83,585 | 90,138 |
| 1 | 83,669 | 90,077 |
| 10 | 83,922 | 90,159 |
| 100 | 83,733 | 90,088 |
| 1000 | 83,781 | 90,268 |

Étendue de 0,34 µs (noyau A) et 0,19 µs (noyau B) pour un facteur 1000 de warmup.

```
WARMUP_POLICY_EFFECT=None_MEASURABLE
AUCUNE_INTERPRETATION_THERMIQUE   (conforme au paragraphe 21 : pas de relation reproductible,
                                   donc pas d'explication)
```

## 6 — Le point critique du §14/§15 : `006F DECODE` vs `007A A0`

Le §14 exige que le mot « exactement ce travail plus… » soit remplacé par une preuve
instructionnelle. Voici ce que je peux établir et ce que je ne peux pas.

**Différence structurelle certaine** : `006F k_decode` **ne charge pas les activations** — il n'a
aucun opérande q8_1, donc aucun `LDG` d'activation, alors que `A0` en a **32** explicitement
(imposés par le keepalive corrigé). Et il **écrit 128 fois plus** :

```
006F k_decode : sink[row * 128 + tid]  ->  5120 x 128 x 4 = 2 621 440 octets = 2,50 Mo par invocation
006F k_full   : out[row]               ->  5120 x     4   =    20 480 octets =   20 Ko par invocation
007A A0       : out[row]               ->  5120 x     4   =    20 480 octets =   20 Ko par invocation

RATIO_ECRITURE_DECODE_OVER_FULL=128 x
```

**Ces deux noyaux ne font donc PAS le même travail mémoire** : l'un écrit 2,5 Mo et ne touche pas
aux activations, l'autre écrit 20 Ko et charge 1 728 octets d'activations par bloc. Les comparer
directement était mon erreur en 006G/H, et je l'ai commise deux fois.

**Ce que je ne peux pas établir** : je n'ai **pas** relancé `k_decode` et `k_full` de 006F dans cette
passe, donc je ne quantifie pas la part de l'écart 22,60 → 90,69 µs qui revient au volume d'écriture
plutôt qu'au chargement d'activation. Le ratio de 128× sur l'écriture est un **candidat fort**, pas
une preuve.

```
006F_DECODE_AND_007A_A0_ARE_NOT_SAME_WORKLOAD=YES_CONFIRME_STRUCTURELLEMENT
   (activations : 0 vs 32 chargements ; ecriture : 2,50 Mo vs 20 Ko)
ECART_22_60_VS_90_69_ATTRIBUTION=NOT_QUANTIFIEE_DANS_CETTE_PASSE
ECRITURE_VOLUME_HYPOTHESIS=CANDIDAT_FORT_NON_DEMONTRE
```

## 7 — Classification finale (§24) et protocole canonique (§25)

```
ABSOLUTE_LEVEL_DISAGREEMENT_ROOT_CAUSE=KERNEL_X_HARNESS_INTERACTION
   (CAS C : effet noyau dominant, effet harnais reel mais secondaire, interaction -1,9 us)
CROSS_EXPERIMENT_ABSOLUTE_TIMING_COMPARABILITY=NOT_ESTABLISHED
   -> les comparaisons absolues entre microbenchs d'AXES DIFFERENTS sont desormais INTERDITES.
      Une comparaison n'est licite qu'entre noyaux du MEME instrument.
```

**Protocole canonique retenu**, pour toutes les campagnes futures :

```
CANONICAL_MICROBENCH_TIMING_PROTOCOL=
  BATCH_LENGTH_K=2048
  WARMUP_KERNELS=3_batches_de_K
  EVENT_PLACEMENT=une_paire_par_batch_entier
  PROCESS_LIFECYCLE=un_processus_par_campagne, contexte CUDA neuf
  CACHE_POLICY=aucun_flush, travail_neutre_entre_bras
  ARM_ORDER=rotation_latine_avec_alternance_de_sens
  X_PROCESSES=au_moins_5
  UNITE_STATISTIQUE=moyenne_par_processus, contrastes_apparies_par_cycle
  REDUCTION=warp_shuffle_5_etages_plus_smem, identique_dans_tous_les_bras
```

Ce protocole est celui de R2, et il a donné une résolution de **0,509 µs** avec un effet de position
de **0,116 µs**.

## 8 — Statut des défauts (§26, §27)

```
HARNESS_DEFECT_29=POSITION_EFFECT_AND_SHORT_MEASUREMENT_WINDOW
   STATUS=RESOLVED_BY_LONG_BATCH_PLUS_POSITION_BALANCING   (0,116 us d'etendue sur 7 positions)
HARNESS_DEFECT_30=UNEXPLAINED_A100_93US
   STATUS=HISTORICAL_ANOMALY_QUARANTINED   (R2 remplace R1 quantitativement)
HARNESS_DEFECT_31=PARALLEL_PROCESS_CONTENTION_INVALIDATED_RUN   STATUS=RESOLVED_SEQUENTIEL
HARNESS_DEFECT_32=ABSOLUTE_LEVEL_DISAGREEMENT_ACROSS_MICROBENCHES
   STATUS=ROOT_CAUSE_IDENTIFIED_NOT_QUANTIFIED
      -> cause : noyaux non equivalents (activations 0 vs 32 ; ecriture 2,50 Mo vs 20 Ko)
         + effet harnais secondaire (0,5 a 2,4 us) + interaction
      -> la quantification exacte exige de relancer k_decode/k_full sous le protocole canonique
PLUMBING_FAILURE_COUNT=28   PLUMBING_FAILURE_29=NOT_ASSIGNED
```

## 9 — Principes méthodologiques promus (§30)

```
WITHIN_INSTRUMENT_PAIRED_CONTRAST > CROSS_EXPERIMENT_ABSOLUTE_COMPARISON
CARDINALITY_OF_USEFUL_ARITHMETIC_WORK_IS_NOT_SUFFICIENT_TO_PREDICT_LATENCY
NO_ABSOLUTE_COMPARISON_BETWEEN_INSTRUMENTS_WITHOUT_EQUIVALENCE_PROOF
```

## 10 — Provenance

```
Runtime baseline   : libllama 3db83675… / libggml-cuda abafbf0d…  (restaures, SHA verifies)
Arbre llama-obrain-cuda : les 4 fichiers du chemin chaud aux SHA vierges
Fichiers sales     : les 3 greffes connues, inchangees
MODIFICATIONS DE SOURCE DANS CETTE PASSE : AUCUNE (microbench autonome)
Instrument         : qualification/e7a-r3/e7a-r3.cu + bin/e7a-r3
   Les deux noyaux (A = 006F sans keepalive, B = A100 avec keepalive) sont dans le MEME binaire,
   meme compilation, meme session -> aucune difference de flags possible entre eux.
PLUMBING_FAILURE_29=NON_ASSIGNE
```

## 11 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007A_R2_STATUS=CLOSED_CONCLUSIVE_NONLINEAR_SITE_DEPENDENT_RESULT
USEFUL_DP4A_WORK_IS_NOT_A_DOMINANT_WALL_COMPONENT=SUPPORTED
DP4A_EXECUTION_THROUGHPUT_LIMIT=REJECTED_BY_ARITHMETIC_REMOVAL_EXPERIMENT
IDP_COUNT_ALONE_INSUFFICIENT=YES
ARITHMETIC_WORK_SITE_PLACEMENT_MATTERS=YES
FULL_IDP_REMOVAL_WALL_EFFECT_US=3.4391
FULL_IDP_REMOVAL_WALL_EFFECT_PERCENT=3.65
PARTIAL_ARITHMETIC_REDUCTION_GAIN=NOT_DERIVABLE_FROM_CARDINALITY_ALONE
ENGINE007A_R3_STATUS=CLOSED_ROOT_CAUSE_CLASSIFIED_C_QUANTIFICATION_PARTIAL
ABSOLUTE_LEVEL_DISAGREEMENT_ROOT_CAUSE=KERNEL_X_HARNESS_INTERACTION
HARNESS_EFFECT_US=0.48_TO_2.38
KERNEL_EFFECT_US=6.65_TO_8.55
INTERACTION_US=-1.8986
CROSS_EXPERIMENT_ABSOLUTE_TIMING_COMPARABILITY=NOT_ESTABLISHED
KERNEL_A_REPRODUCES_006F_FULL=YES_79.9392_VS_79.93
PER_KERNEL_LATENCY_DEPENDS_ON_BATCH_LENGTH=NO_POUR_K>=20
WARMUP_POLICY_EFFECT=NONE_MEASURABLE
006F_DECODE_AND_007A_A0_ARE_NOT_SAME_WORKLOAD=YES_CONFIRME
   (activations 0 vs 32 chargements ; ecriture 2,50 Mo vs 20 Ko)
ECART_22_60_VS_90_69_ATTRIBUTION=NOT_QUANTIFIEE
CANONICAL_MICROBENCH_TIMING_PROTOCOL=DEFINED_K2048_WARMUP3_LATIN_ROTATION_5_PROCESSES_MIN
POSITION_EFFECT_PRESENT=NO
MEASUREMENT_RESOLUTION_US=0.509
HARNESS_DEFECT_29=RESOLVED_BY_CONSTRUCTION
HARNESS_DEFECT_30=HISTORICAL_ANOMALY_QUARANTINED
HARNESS_DEFECT_32=ROOT_CAUSE_IDENTIFIED_NOT_QUANTIFIED
ENGINE007_PRIMARY_LEVER=NOT_IDENTIFIED
ENGINE007B_STATUS=BLOCKED
ENGINE007C_STATUS=BLOCKED
NEXT_STEP_IF_PURSUED=RELANCER_006F_DECODE_ET_FULL_SOUS_LE_PROTOCOLE_CANONIQUE
COMPUTE_BOUND=NOT_PROVEN
ISSUE_THROUGHPUT_LIMITED=NOT_PROVEN
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
