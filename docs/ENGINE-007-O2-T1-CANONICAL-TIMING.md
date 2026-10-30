# ENGINE-007-O2-T1 — CHRONOMÉTRAGE MURAL CANONIQUE
## NATIF 4 WARPS contre O2 DENSE 6 WARPS

Date : 2026-09-29. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7-o2-t1/`

---

## 0 — Résultat

**La redistribution dense est une régression : O2 est plus lent de 0,83 µs (0,99 %).**

```
ENGINE007O2_STATUS=CLOSED_SIX_WARP_GEOMETRY_REJECTED
SIX_WARP_GEOMETRY=REJECTED_BY_WALL_TIMING
DENSE_EPISODE_REDISTRIBUTION=NO_MATERIAL_FULL_GEOMETRY_GAIN
R12_E2_SAME_VS_DIFFERENT_EFFECT=DOES_NOT_GENERALIZE_MATERIALLY_TO_FULL_SIX_EPISODE_GEOMETRY

NATIVE_MEAN_US=83,2744      O2_MEAN_US=84,1012
O2_MINUS_NATIVE_US=+0,8268  IC95=[+0,7010 ; +0,9526]   EFFECT_TO_NOISE=12,88
KERNEL_WALL_REDUCTION_PERCENT=−0,9929      KERNEL_SPEEDUP=0,990169
```

L'effet est **petit en amplitude mais parfaitement net en signe** : l'intervalle de confiance est
loin de zéro et la régression est présente sur **les 5 processus**, sans exception.

**Ce n'est pas un cas C ambigu** — c'est un cas C avec un signe établi, que le §11 range en cas D.
Je le classe selon votre règle : **`CLOSED_SIX_WARP_GEOMETRY_REJECTED`**, en précisant que la marge
est sous 1 % (voir §5).

---

## 1 — Autorisation et gel de la qualification (§1)

```
ENGINE007O2_STATUS=QUALIFIED_TIMING_PENDING  ->  ENGINE007O2_T1_TIMING_ALLOWED=YES
PATCH_SHA256=8ada1e0b9ccd602331f324d566d0d6027937108b6ae45073659b8c08dbb135e9
NATIVE_THREADS_PER_BLOCK=128      O2_THREADS_PER_BLOCK=192
NATIVE_PHYSICAL_WARPS=4           O2_PHYSICAL_WARPS=6
NATIVE_EPISODE_MAPPING={2,2,1,1}  O2_EPISODE_MAPPING={1,1,1,1,1,1}
SAME_LOGICAL_KBX_SET=YES          SAME_DYNAMIC_KBX_EXECUTION_COUNT=192
WEIGHT/Q8/SCALE_ADDRESS_SET_IDENTICAL=YES
SAME_IDP_WORK=YES                 SAME_TQ2_DECODE_WORK=YES
BITWISE_OUTPUT_MISMATCH_COUNTS_BY_SEED=0,0,0,0,0
NATIVE_REGISTERS=38  O2_REGISTERS=38   NATIVE_SPILLS=0  O2_SPILLS=0
NATIVE_RESIDENT_WARPS_PER_SM=48   O2_RESIDENT_WARPS_PER_SM=48
O2_OCCUPANCY_CAPACITY_CONFOUND=NO
```

## 2 — Gel des binaires et intégrité (§4, §14)

```
NATIVE_BINARY_SHA256=677ca39cd52521003b5892376652d02679b6a61c20d1a5b3aeff9cf1c96fd1c6
O2_BINARY_SHA256=677ca39cd52521003b5892376652d02679b6a61c20d1a5b3aeff9cf1c96fd1c6
   -> les deux noyaux vivent dans UN seul binaire, compile en une fois : aucune difference de
      drapeaux n est possible entre eux.
NATIVE_SOURCE_SHA256=233b6c2e9464865190946603f254cab455e04a963a0de5bbf57a8098ee576ed4
O2_SOURCE_SHA256=c39a110cbfac3f288e3059dbf2a9e6d4f5b46e101a2fc98e2325d0d2b26f417a
HARNESS_SOURCE_SHA256=5d6102b89cded6e4b8b5b5e9ee0a06c11d76a65c58b472cfadc35fa78ffda105
PATCH_SHA256=8ada1e0b9ccd602331f324d566d0d6027937108b6ae45073659b8c08dbb135e9
```

Après campagne, **les quatre empreintes sont identiques** au bit près :

```
NATIVE_BINARY_SHA256_POST=677ca39cd52521003b5892376652d02679b6a61c20d1a5b3aeff9cf1c96fd1c6
O2_BINARY_SHA256_POST=677ca39cd52521003b5892376652d02679b6a61c20d1a5b3aeff9cf1c96fd1c6
NATIVE_SOURCE_SHA256_POST=233b6c2e9464865190946603f254cab455e04a963a0de5bbf57a8098ee576ed4
O2_SOURCE_SHA256_POST=c39a110cbfac3f288e3059dbf2a9e6d4f5b46e101a2fc98e2325d0d2b26f417a
```

## 3 — Pré-vol et post-vol (§5, §13)

Pré-vol, vérifié **dans chaque processus avant le moindre chronométrage** :

```
INPUT_SANITY_GATE=PASS (5/5)          NAN_SCALE_COUNT=0   INF_SCALE_COUNT=0
SUBNORMAL_SCALE_COUNT=0               ZERO_SCALE_COUNT=0   FINITE_SCALE_COUNT=245760
INVALID_FLOAT_METADATA_COUNT=0
NATIVE_OUTPUT_NAN_COUNT=0             O2_OUTPUT_NAN_COUNT=0
NATIVE_OUTPUT_INF_COUNT=0             O2_OUTPUT_INF_COUNT=0
PRE_TIMING_BITWISE_MISMATCH_COUNT=0   OUTPUT_SANITY_GATE=PASS
```

Post-vol, retest indépendant sur 5 graines après la campagne :

```
POST_TIMING_BITWISE_MISMATCH_COUNT=0
POST_OUTPUT_NAN_COUNT=0   POST_OUTPUT_INF_COUNT=0
BITWISE_KERNEL_OUTPUT_MATCH=YES       OUTPUT_SANITY_GATE=PASS
O2_T1_RESULT=VALID                    (aucune perte de fidelite)
```

## 4 — Protocole et données brutes (§6, §7, §8, §9)

```
CANONICAL_MICROBENCH_TIMING_PROTOCOL=K2048_WARMUP3_LATIN_MULTI_PROCESS
KERNELS_PER_BATCH=2048   WARMUP_BATCHES=3   ONE_EVENT_PAIR_PER_BATCH=YES
FRESH_PROCESS_PER_CAMPAIGN=YES   PROCESS_COUNT=5
PROCESS_LEVEL_MEAN_PRIMARY_UNIT=YES   PAIRED_CONTRASTS=YES
N_OBSERVATIONS=200   N_PAIRS=100   N_PROCESSES=5
ALTERNANCE=NATIF->O2 / O2->NATIF selon la parite du cycle
NATIVE_FIRST_COUNT=50     O2_FIRST_COUNT=50      (equilibre exact)
AUCUNE MODIFICATION DU PROTOCOLE APRES OBSERVATION
```

| bras | n | moyenne | sd | min | max |
|---|---|---|---|---|---|
| `K_FULL_NATIVE` | 100 | **83,2744** | 0,6844 | 81,9165 | 84,6806 |
| `O2_6WARP` | 100 | **84,1012** | **0,2479** | 83,8841 | 85,6858 |

Moyennes par processus et deltas appariés :

| pid | natif | O2 | delta (O2−natif) |
|---|---|---|---|
| 0 | 82,0602 | 83,9684 | **+1,9082** |
| 1 | 83,2290 | 84,0701 | +0,8411 |
| 2 | 83,4740 | 84,1067 | +0,6327 |
| 3 | 83,7351 | 84,2369 | +0,5018 |
| 4 | 83,8734 | 84,1238 | +0,2503 |

```
NATIVE_PROCESS_MEANS_US=82,0602,83,2290,83,4740,83,7351,83,8734
O2_PROCESS_MEANS_US=83,9684,84,0701,84,1067,84,2369,84,1238
PAIRED_DELTA_PROCESS_MEANS_US=+1,9082,+0,8411,+0,6327,+0,5018,+0,2503
   -> les cinq deltas sont POSITIFS : aucune inversion de signe
```

## 5 — Statistiques primaires (§9)

```
O2_MINUS_NATIVE_US=+0,8268
O2_MINUS_NATIVE_CI95_US=[+0,7010 ; +0,9526]
PAIRED_SD_US=0,6420        EFFECT_TO_NOISE=12,88
KERNEL_WALL_REDUCTION_PERCENT=−0,9929        KERNEL_SPEEDUP=0,990169
```

Un point que je dois signaler sans en tirer de conclusion : **O2 est nettement plus stable**
(sd 0,2479 contre 0,6844, soit **2,8 fois moins de variance**). Le contraste apparié gagne donc en
précision, et l'intervalle de confiance est serré — ce qui rend le signe négatif **solide**, pas
marginal. Une géométrie à 6 warps de travail uniforme produit un temps plus reproductible, mais
**plus lent**.

**Réserve de calibrage** : la régression de 0,99 % est du même ordre que la résolution de mesure
utilisée ailleurs dans ce programme (0,5 à 0,6 µs, soit ~0,7 % d'un noyau de 84 µs). Elle est
cependant **résolue statistiquement** ici (effect/noise 12,9) parce que le contraste apparié
supprime la dérive commune. Je maintiens donc la conclusion « pas de gain, et un signe défavorable
établi », mais je ne la présente **pas** comme une régression matérielle : voir §7.

## 6 — Diagnostic de position corrigé (§10)

Conformément à `HARNESS_DEFECT_43`, j'utilise l'effet intra-bras et l'effet sur le contraste
apparié, **pas** la plage des moyennes par position :

```
NATIVE_WITHIN_ARM_POSITION_EFFECT_US=−0,0538
O2_WITHIN_ARM_POSITION_EFFECT_US=−0,0641
PAIRED_CONTRAST_WHEN_NATIVE_FIRST_US=+0,8858       (n=50)
PAIRED_CONTRAST_WHEN_O2_FIRST_US=+0,7679           (n=50)
PAIRED_CONTRAST_POSITION_EFFECT_US=+0,1179   <- AUTORITAIRE
```

L'effet de position sur le contraste vaut **0,118 µs**, soit **7 fois moins** que le contraste mesuré
(0,827 µs), et il est équilibré par construction (50/50). **Il n'invalide pas le résultat.**
Le nouveau diagnostic, tel que le §10 le demandait, se comporte correctement sur une campagne à fort
écart inter-bras.

## 7 — Interprétation : cas D, avec le périmètre exact du §15

```
SIX_WARP_GEOMETRY=REJECTED_BY_WALL_TIMING
DENSE_EPISODE_REDISTRIBUTION=NO_MATERIAL_FULL_GEOMETRY_GAIN
R12_E2_SAME_VS_DIFFERENT_EFFECT=DOES_NOT_GENERALIZE_MATERIALLY_TO_FULL_SIX_EPISODE_GEOMETRY
```

L'hypothèse O2 est **close**. La redistribution ne réduit pas la paroi au plein domaine dense : elle
l'augmente légèrement.

**Ce que la conclusion autorise, et rien de plus** (§15) :

```
AUTORISE :
   MEME_TRAVAIL_DENSE + MEME_CAPACITE_THEORIQUE_DE_WARPS_RESIDENTS
   + AFFECTATION_EPISODE->WARP_PHYSIQUE_DIFFERENTE
   -> PAROI_LEGEREMENT_SUPERIEURE_EN_O2

INTERDIT_TANT_QUE_LES_COMPTEURS_MATERIELS_MANQUENT :
   LATENCY_HIDING_PROVEN=NO
   SCHEDULER_BOTTLENECK_PROVEN=NO
   ISSUE_BOTTLENECK_PROVEN=NO
   RUNTIME_OCCUPANCY_PROVEN_EQUAL=NO
```

**Aucun mécanisme n'est proposé.** Le §11 interdit explicitement d'inventer une explication de
scheduler après coup, et je m'y tiens. Je note seulement, comme **fait mesuré**, qu'à capacité de
warps résidents égale (48/SM) la variante à 6 warps et 8 blocs/SM est plus lente que la variante à
4 warps et 12 blocs/SM — ce qui est une observation, pas une explication.

## 8 — Pourquoi R12 ne prédisait pas O2 (§12)

R12 avait mesuré, **sur une topologie creuse** (2 épisodes seulement) :

```
E2_SAME_WARP=32,9286   E2_DIFFERENT_WARPS=30,9966   DELTA=−1,9319 us
```

O2 généralise ce signal à la géométrie **pleine** à 6 épisodes, et **il disparaît — et s'inverse très
légèrement**. Je n'ai **pas** extrapolé `2 × 1,9319` comme gain attendu, conformément au §12. Le
résultat confirme que cette extrapolation aurait été fausse : à 6 épisodes, le compromis change de
signe.

C'est le troisième effet de cette série qui ne survit pas au changement d'échelle (après la
cardinalité `IDP` et la cardinalité `kbx`). Le motif est cohérent : **ce noyau ne se laisse pas
extrapoler d'une topologie à une autre**.

## 9 — Suite

```
ENGINE007_NEXT_STEP=RETOUR_A_L_ELIMINATION_STRUCTUREE_ALIGNEE_SUR_LES_EPISODES
```

Le §18 interdit d'enchaîner automatiquement sur un balayage 5/7/8 warps — je ne le fais pas.
L'hypothèse de redistribution dense simple est fermée. Les axes restants :

1. **élimination structurée alignée** (branche O1) : rendre des épisodes entiers inactifs, ce que R11
   chiffre à 74,8 % de réduction de paroi entre 6 et 1 épisode ;
2. **topologie causale différente**, à motiver par une nouvelle mesure et non par un balayage.

```
ENGINE007B_STATUS=UNBLOCKED_FOR_STRUCTURED_EPISODE_FEASIBILITY_ONLY
ENGINE007B_PRODUCT_OPTIMIZATION_ALLOWED=NO
ENGINE007C_STATUS=BLOCKED
ENGINE007A_R13_STATUS=BLOCKED_MISSING_HARDWARE_COUNTER_PERMISSION
   -> aucun nouvel essai ncu pendant O2-T1, conformement au paragraphe 19.
```

## 10 — Portée produit (§16)

```
PRODUCT_GAIN=NOT_MEASURED
BACKEND_TOK_S_GAIN=NOT_MEASURED
END_TO_END_GAIN=NOT_MEASURED
```

`ENGINE007O2_PROD1` **n'est pas ouvert** : le §17 le conditionne à un gain matériel, qui n'existe pas.

## 11 — Harness ledger (§20)

```
HARNESS_DEFECT_43=RESOLVED_CORRECTED_POSITION_DIAGNOSTIC
   -> le nouveau diagnostic (intra-bras + contraste apparie) a ete utilise ici et fonctionne :
      0,118 us sur une campagne ou l ancienne plage aurait ete ininterpretable.
HARNESS_DEFECT_45=RESOLVED_MANGLED_SYMBOL_EXTRACTION
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```

## 12 — Livrables

Tous produits dans `qualification/e7-o2-t1/` :

```
TIMING_RAW.csv                200 observations brutes
PROCESS_MEANS.txt             moyennes par processus, deltas apparies
PAIRED_CONTRASTS.txt          contraste primaire, IC, effect/noise, signe
POSITION_DIAGNOSTIC.txt       diagnostic corrige (defect 43)
FIDELITY_PRECHECK.txt         gates d entree/sortie/fidelite par processus, AVANT chronometrage
FIDELITY_POSTCHECK.txt        retest 5 graines APRES campagne
BINARY_INTEGRITY.txt          empreintes avant et apres
docs/ENGINE-007-O2-T1-CANONICAL-TIMING.md     ce rapport
```

## 13 — Rapport final (§22)

```
ENGINE007O2_STATUS=CLOSED_SIX_WARP_GEOMETRY_REJECTED
NATIVE_MEAN_US=83.2744
NATIVE_SD_US=0.6844
O2_MEAN_US=84.1012
O2_SD_US=0.2479
O2_MINUS_NATIVE_US=+0.8268
O2_MINUS_NATIVE_CI95_US=[+0.7010,+0.9526]
KERNEL_WALL_REDUCTION_PERCENT=-0.9929
KERNEL_SPEEDUP=0.990169
PAIRED_SD_US=0.6420
EFFECT_TO_NOISE=12.88
NATIVE_PROCESS_MEANS_US=82.0602,83.2290,83.4740,83.7351,83.8734
O2_PROCESS_MEANS_US=83.9684,84.0701,84.1067,84.2369,84.1238
PAIRED_DELTA_PROCESS_MEANS_US=+1.9082,+0.8411,+0.6327,+0.5018,+0.2503
NATIVE_WITHIN_ARM_POSITION_EFFECT_US=-0.0538
O2_WITHIN_ARM_POSITION_EFFECT_US=-0.0641
PAIRED_CONTRAST_POSITION_EFFECT_US=+0.1179
PRE_TIMING_BITWISE_MISMATCH_COUNT=0
POST_TIMING_BITWISE_MISMATCH_COUNT=0
NATIVE_BINARY_SHA256=677ca39cd52521003b5892376652d02679b6a61c20d1a5b3aeff9cf1c96fd1c6
O2_BINARY_SHA256=677ca39cd52521003b5892376652d02679b6a61c20d1a5b3aeff9cf1c96fd1c6
PATCH_SHA256=8ada1e0b9ccd602331f324d566d0d6027937108b6ae45073659b8c08dbb135e9
DENSE_EPISODE_REDISTRIBUTION=NO_MATERIAL_FULL_GEOMETRY_GAIN_SIGNE_DEFAVORABLE
PHYSICAL_WARP_REUSE_PENALTY=DOES_NOT_GENERALIZE_TO_FULL_SIX_EPISODE_GEOMETRY
O2_VARIANCE_OBSERVATION=sd_2.8x_LOWER_BUT_MEAN_SLOWER
PRODUCT_GAIN=NOT_MEASURED
ENGINE007_NEXT_STEP=RETOUR_A_L_ELIMINATION_STRUCTUREE_ALIGNEE_SUR_LES_EPISODES
ENGINE007_PRIMARY_LEVER=WARP_EPISODE_LEVEL_STRUCTURED_EXECUTION
ENGINE007B_STATUS=UNBLOCKED_FOR_STRUCTURED_EPISODE_FEASIBILITY_ONLY
ENGINE007B_PRODUCT_OPTIMIZATION_ALLOWED=NO
ENGINE007C_STATUS=BLOCKED
ENGINE007A_R13_STATUS=BLOCKED_MISSING_HARDWARE_COUNTER_PERMISSION
HARNESS_DEFECT_43=RESOLVED_CORRECTED_POSITION_DIAGNOSTIC
HARNESS_DEFECT_45=RESOLVED_MANGLED_SYMBOL_EXTRACTION
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
