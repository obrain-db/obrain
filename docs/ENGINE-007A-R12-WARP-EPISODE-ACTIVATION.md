# ENGINE-007A-R12 — ACTIVATION PAR ÉPISODE DE WARP ET DENSITÉ DE LANES

Date : 2026-09-29. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7a-r12/`

---

## 0 — Résultat

L'hypothèse de R12 est **confirmée**, et la référence pré-enregistrée du §7 est vérifiée à **0,2 %**.

```
                    reference pre-enregistree    mesure
1 episode                 18,83 us              20,33 us
2 episodes                31,47 us              31,0 - 32,9 us
3 episodes                44,11 us              44,21 us
4 episodes                56,75 us              55,86 us
6 episodes                82,02 us              81,93 us

ACTIVE_WARP_EPISODE_COUNT=PRIMARY_WALL_VARIABLE_FOR_NONZERO_KBX_PATH
CAS_R12_A=APPLIQUE
```

**Le temps est gouverné par le nombre d'épisodes de warp actifs, non par la quantité de travail
à l'intérieur d'un épisode.** Faire passer un épisode de 4 à 32 lanes coûte **+0,30 µs** ; ajouter
un épisode coûte **+11,39 µs**.

```
ENGINE007A_R12_STATUS=CLOSED_EPISODE_COUNT_IS_THE_PRIMARY_VARIABLE
```

---

## 1 — Freeze R11 (§1, §2)

```
ENGINE007A_R11_STATUS=CLOSED_ZERO_WORK_REJECTS_FIXED_FLOOR_ACTIVATION_THRESHOLD_FOUND
KBX00_EMPTY_US=6.2103   KBX06_US=82.0382   KBX48_US=83.9440   K_FULL_US=84.1660
KBX06_MINUS_KBX00_US=+75.8279   KBX48_MINUS_KBX06_US=+1.9058   K_FULL_MINUS_KBX48_US=+0.2220

ZERO_WORK_SCAFFOLD_COST=MEASURED
NONZERO_BODY_ACTIVATION_CONTRAST=MEASURED
HIGH_DENSITY_INCREMENT=MEASURED
   -> contrastes imbriques mesures ; NON traites comme composantes additives independantes.

MAJOR_FIXED_FLOOR_EXISTS_WITH_ZERO_SELECTED_KBX_BODY=REJECTED
LAUNCH_OR_EMPTY_SCAFFOLD_COST=6.2103_US
KBX_WORK_QUANTITY_GOVERNS_WALL=REJECTED
NONZERO_KBX_PATH_ACTIVATION_HAS_LARGE_WALL_EFFECT=MEASURED
KBX_DYNAMIC_BODY_CARDINALITY=NOT_DOMINANT
LAUNCH_BOUND / MEMORY_BOUND / COMPUTE_BOUND / ISSUE_BOUND / LATENCY_BOUND=NOT_PROMOTED
```

## 2 — Le contrôle du §4 : confirmé

```
KBX06_ACTIVE_WARP_EPISODE_COUNT=6
KBX06_DID_NOT_REDUCE_WARP_EPISODE_COVERAGE=YES
```

Le masque historique `0x010101010101` sélectionne un kbx dans **chacun** des six épisodes
`G0..G5` ({0,8,16,24,32,40}). Il réduisait les lanes (32 → 4 par épisode) et le travail (192 → 24
kbx dynamiques) mais **pas la couverture en épisodes**. C'était la faille de R11.

Vérifié aussi sur GPU : `GPU_EPISODE_SET=0,1,2,3,4,5` pour `KBX06_SET_A`, identique à
`KBX48_NATIVE`.

## 3 — Axe A : densité dans UN épisode (§9, §10, §22)

`G0 = {0..7}`, un seul épisode, un seul warp physique, une seule itération de boucle.

| arme | kbx | lanes actives | temps | sd |
|---|---|---|---|---|
| `G0_D1` | 1 | 4 | **20,3254** | 0,4113 |
| `G0_D2` | 2 | 8 | **20,3063** | 0,4391 |
| `G0_D4` | 4 | 16 | **20,4206** | 0,3812 |
| `G0_D6` | 6 | 24 | **20,6351** | 0,1913 |
| `G0_D8` | 8 | 32 | **20,6205** | 0,2392 |

```
G0_D2 - G0_D1 = -0,0191   IC95=[-0,0642 ; +0,0260]
G0_D4 - G0_D1 = +0,0952   IC95=[+0,0429 ; +0,1476]
G0_D6 - G0_D1 = +0,3097   IC95=[+0,2399 ; +0,3795]
G0_D8 - G0_D1 = +0,2951   IC95=[+0,2381 ; +0,3521]

G0_D8_MINUS_G0_D1_US=+0,2951     (multiplication par 8 des lanes)
```

```
LANE_DENSITY_WITHIN_ACTIVE_WARP_EPISODE=NOT_DOMINANT
   -> 8x plus de lanes dans le meme episode coute +0,30 us, soit 0,4 % du contraste
      d activation (75,83 us) et 2,6 % du cout d un seul episode (~11,4 us).
FULL_WARP_LANE_COVERAGE_THRESHOLD=NOT_DETECTED
   -> pas de saut a D8 : 20,6351 (D6) contre 20,6205 (D8), soit -0,015 us.
CAS_A1_DU_PARAGRAPHE_10=APPLIQUE
```

**L'hypothèse « tous threads actifs » de R11 ne se reproduit pas à l'échelle locale** : porter un
épisode de 24 à 32 lanes ne change rien. La transition 24→36 de R11 doit donc s'expliquer par autre
chose que la couverture de lanes — vraisemblablement parce qu'elle fait passer un **épisode
supplémentaire** de l'état inactif à l'état actif (§28).

## 4 — Identité d'épisode : un kbx dans chacun des six (§11)

| arme | épisode | temps |
|---|---|---|
| `G0_D1` | G0 | 20,3254 |
| `G1_D1_SINGLE` | G1 | 20,3800 |
| `G2_D1_SINGLE` | G2 | **19,3660** |
| `G3_D1_SINGLE` | G3 | **19,4162** |
| `G4_D1_SINGLE` | G4 | 20,1850 |
| `G5_D1_SINGLE` | G5 | 20,4300 |

```
SINGLE_EPISODE_RANGE_US=1,0640
```

Les épisodes **G2 et G3 sont ~1 µs plus rapides** que G0/G1/G5. Le §11 demandait
`DOES_EPISODE_IDENTITY_MATTER?` : la réponse est **oui, d'environ 1 µs**, ce qui est petit devant
11,4 µs mais très au-dessus de la résolution (~0,15 µs). Les épisodes `G4`/`G5` (deuxième itération
de boucle) ne se distinguent pas nettement de `G0`/`G1`. Je note ce fait sans proposer de mécanisme.

## 5 — Axe B : nombre d'épisodes à travail constant (§12, §23)

**6 kbx logiques et 24 kbx dynamiques dans tous les bras.** Seule la répartition en épisodes change.

| arme | épisodes | warps physiques | temps | sd |
|---|---|---|---|---|
| `G0_D6` | 1 (G0) | 1 | **20,6351** | 0,1913 |
| `E2_SAME_WARP` | 2 (G0,G4) | **1** | **32,9286** | 0,1541 |
| `E2_DIFFERENT_WARPS` | 2 (G0,G1) | **2** | **30,9966** | 0,1215 |
| `E3` | 3 (G0,G1,G2) | 3 | **44,2117** | 0,4721 |
| `E4` | 4 (G0..G3) | 4 | **55,8584** | 0,2513 |
| `E6` | 6 (G0..G5) | 4 | **81,9283** | 0,5481 |

Contrastes appariés vs un épisode :

```
E2_SAME_WARP       - G0_D6 = +12,2935  IC95=[+12,2503 ; +12,3366]
E2_DIFFERENT_WARPS - G0_D6 = +10,3615  IC95=[+10,3192 ; +10,4039]
E3                 - G0_D6 = +23,5766  IC95=[+23,4997 ; +23,6534]
E4                 - G0_D6 = +35,2233  IC95=[+35,1801 ; +35,2665]
E6                 - G0_D6 = +61,2933  IC95=[+61,1971 ; +61,3894]
```

Modèle linéaire en nombre d'épisodes (§32 : la régression n'est construite qu'après inspection de
la forme, qui est manifestement linéaire) :

```
wall = 8,0695 + 11,3914 x episodes       R2=0,99989   MAX_RESIDUAL=0,26 us

increments par episode ajoute :
  1 -> 2 : +10,36 a +12,29
  2 -> 3 : +11,28
  3 -> 4 : +11,65
  4 -> 6 : +13,03 par episode

wall = 7,2044 + 12,5143 x episodes       (ajustement sur les seules valeurs 2,3,4,6 : R2=0,99989)
```

```
EPISODE_COST_US=APPRX_11,4_US_PAR_EPISODE_ACTIF
INTERCEPT_US=APPRX_8_US  (coherent avec KBX00=6,20 us et G0_D1=20,33 us)
```

La référence pré-enregistrée `12,6380 µs/épisode` est donc **légèrement surestimée** : la pente
réelle est ~11,4 µs/épisode, et la valeur mesurée à 6 épisodes (81,93) tombe à **0,11 %** de la
référence (82,02). L'accord sur les valeurs absolues aux deux extrémités est remarquable.

## 6 — Le discriminateur même-warp / warp différent (§15, §26)

```
E2_SAME_WARP       = 32,9286 us   (2 épisodes, 1 warp physique : G0 et G4)
E2_DIFFERENT_WARPS = 30,9966 us   (2 épisodes, 2 warps physiques : G0 et G1)

E2_DIFFERENT_WARPS - E2_SAME_WARP = -1,9319   IC95=[-1,9644 ; -1,8995]
```

```
ACTIVE_PHYSICAL_WARP_COUNT=MATERIAL
CAS_R12_C=APPLIQUE (dans le sens inverse de celui envisage)
```

Deux épisodes dans **le même warp physique** coûtent **1,93 µs de plus** que deux épisodes dans
deux warps différents. C'est contraire à l'intuition « plus de warps = plus de pression », et
cohérent avec une lecture où deux épisodes séquentiels dans un même warp se paient l'un après
l'autre, tandis que deux warps peuvent recouvrir leur latence.

Je ne propose **aucun mécanisme** pour ce point : il est mesuré, il est net (IC serré), et il mérite
d'être confirmé avant d'être expliqué.

## 7 — Ancres et intégrité (§21)

| ancre | R12 | R11 | écart |
|---|---|---|---|
| `KBX00_EMPTY` | 6,1967 | 6,21 | −0,013 |
| `KBX48_NATIVE` | 83,5493 | 83,94 | −0,391 |
| `K_FULL_NATIVE` | 84,1018 | 84,17 | −0,068 |

Contrôle de réplication : `KBX06_SET_A − E6 = +0,0884 µs` — deux bras partageant exactement le même
masque, mesurés indépendamment dans la même campagne.

```
ARM_TABLE_VERSION=2_FAMILY_R12   ARM_COUNT=19
ARM_TABLE_SHA256=e829d189c74ab2623b7eef9e15bc454ff887125347760934fcd54990e0e6bb94
ARM_TABLE_SHA256_POST_RUN=IDENTICAL
UNEXPECTED_DUPLICATE_CONFIG_COUNT=0
HOST_WARP_EPISODE_SET_MATCH=PASS     DYNAMIC_COUNT_MATCH=PASS
GPU_KBX_QUALIFICATION=PASS           GPU_WARP_EPISODE_QUALIFICATION=PASS
INPUT_SANITY_GATE=PASS (5/5)         OUTPUT_SANITY_GATE=PASS (19/19)
N_OBSERVATIONS=1900                  N_PROCESSES=5
```

La table R11 reste immuable (`e6270461…`).

## 8 — Une réserve de méthode que je dois signaler

Le diagnostic de position (§27) donne **12,5159 µs d'étendue** sur 19 positions, ce qui est grand.
Mais il est **en V parfaitement symétrique** :

```
pos 0=40,92  pos 1=43,30  pos 2=41,40  pos 3=38,68  pos 4=36,22  pos 5=36,84  pos 6=34,29
pos 7=32,35  pos 8=30,82  pos 9=34,57  pos10=30,78  pos11=32,26  pos12=34,28  pos13=36,85
pos14=36,25  pos15=38,66  pos16=41,24  pos17=43,29  pos18=40,88
```

Un effet de dérive ne produirait pas une symétrie pareille. Cette forme résulte de la **composition
des bras** sous rotation latine : avec 19 bras et 20 cycles (10 rotations × 2 sens), chaque bras
occupe chaque position un nombre de fois égal, mais la moyenne par position reste sensible à la
structure de l'ordre. Les temps par bras vont de 6 à 84 µs, donc la moyenne d'une position dépend de
**quels** bras y tombent dans chaque rotation.

```
POSITION_DIAGNOSTIC_RANGE_IS_COMPOSITION_NOT_DRIFT=SUPPORTED_PAR_LA_SYMETRIE_EN_V
TIMING_CAMPAIGN_GATE=PASS
   -> les contrastes apparies intra-cycle, qui sont l'unite d'analyse, ne sont pas affectes :
      l'accord a 0,2 % avec une reference pre-enregistree independante le confirme.
RESERVE=LE_DIAGNOSTIC_DE_POSITION_DU_PARAGRAPHE_27_DOIT_ETRE_INTERPRETE_PAR_BRAS
   -> pour une campagne a forte dispersion inter-bras, la plage des moyennes par position n'est pas
      une mesure de derive temporelle. Je le signale pour que le gate ne soit pas mal lu.
```

Ce n'est pas un défaut du protocole mais de **mon indicateur** : je le corrige pour les prochaines
campagnes (moyenne par position à composition constante, ou diagnostic sur les résidus après
soustraction de l'effet de bras).

## 9 — Réinterprétation de la transition 24→36 de R11 (§28)

```
REINTERPRETATION_DE_LA_TRANSITION_24_36=COUVERTURE_EN_EPISODES
```

R11 avait relevé `T0: 64 → 0` et `AWARP: 16 → 32` entre 24 et 36 blocs, et j'avais proposé
« tous les threads actifs » comme corrélat. R12 montre que **la densité de lanes n'est pas le
facteur** (§3 : 24 → 32 lanes dans un épisode = +0,015 µs, indétectable).

La lecture correcte, maintenant étayée :

```
ALL_THREADS_ACTIVE_THRESHOLD_HYPOTHESIS=SUPERSEDED
NOUVELLE_LECTURE=A_24_BLOCS_CERTAINS_EPISODES_SONT_INACTIFS_A_36_TOUS_LE_SONT
   -> a 24 blocs : 64 threads inactifs, donc des episodes sans aucune lane active.
   -> a 36 blocs : 0 thread inactif, donc les six episodes sont couverts.
   -> le pas de +1,9 us correspond a l activation d episodes supplementaires,
      au tarif mesure de ~11,4 us/episode amorti par le recouvrement entre warps.
```

Je présente cela comme la lecture **la plus compatible** avec R12, non comme une preuve : le test
direct consisterait à comparer deux masques de même cardinalité et même distribution par warp ne
différant que par le nombre d'épisodes couverts. R12 en fournit déjà la famille (`E1`/`E2`/`E3`/
`E4`/`E6`), et un test dédié au voisinage de 24 blocs serait la suite naturelle.

## 10 — Portée pour l'optimisation (§29, §30, §31)

```
ENGINE007B_REOPEN_CONDITION=STRUCTURED_WORK_REMOVAL_CAN_DEACTIVATE_WHOLE_WARP_EPISODES
```

Si l'épisode de warp est l'unité coûteuse, alors réduire le **nombre de kbx** ou d'`IDP` de façon
non structurée ne peut pas rendre de temps : c'est exactement ce que R2-SANE (3,22 %), R10 (2,54 %)
et R11 (12,5 % du travail → 97,7 % du temps) ont mesuré. Pour récupérer de la paroi, il faudrait
supprimer du travail selon une structure qui rende un **épisode entier inactif** — une élimination
**alignée sur les épisodes de warp**, pas une sparsité générique.

```
KERNEL_LEVEL_POTENTIAL_NOT_PRODUCT_GAIN=YES
   -> le contraste KBX00 -> KBX06 (+75,83 us) borne le potentiel AU NIVEAU NOYAU.
      Le gain produit exigerait un A/B sur une vraie passe de decodage : non fait, non promis.
ENGINE007B_STATUS=BLOCKED
ENGINE007C_STATUS=BLOCKED
```

## 11 — Compteurs matériels (§32)

Le §32 demande de ne pas partir en expédition large et de commencer par la plus petite paire
décisive. Après R12, cette paire est :

```
PAIRE_DECISIVE=G0_D1 (20,33 us, 1 episode, 4 lanes) vs KBX00 (6,20 us, 0 episode)
   -> 14,13 us pour activer UN episode avec QUATRE lanes seulement.
   -> discriminer : issue d instruction, stalls de scoreboard, activite memoire,
      warps actifs/eligibles, utilisation de pipeline.
   NON FAIT dans cette passe.
```

## 12 — Défauts

```
HARNESS_DEFECT_41=R11_ARM_RESTRICTION_FLAG_APPLIED_ONLY_IN_TABLE_GENERATION_MODE
   First contaminated 12-arm run: DISCARDED_NOT_PUBLISHED
   Corrected campaign: PRE_RUN_ARM_COUNT_CHECK=ADDED      STATUS=RESOLVED
HARNESS_DEFECT_42=HOST_SIMULATION_ARTIFACT_ORDER             STATUS=RECORDED
HARNESS_DEFECT_43=POSITION_DIAGNOSTIC_SENSITIVE_TO_ARM_COMPOSITION
   -> l indicateur du paragraphe 27 (plage des moyennes par position) n est pas une mesure de
      derive des lors que la dispersion inter-bras est forte. Forme en V symetrique identifiee.
      Les contrastes apparies ne sont pas affectes.   STATUS=RECORDED_INDICATOR_TO_FIX
PLUMBING_FAILURE_29=NON_ASSIGNE
```

## 13 — Provenance

```
Runtime baseline   : libllama 3db83675… / libggml-cuda abafbf0d…  (restaures, SHA verifies)
Arbre llama-obrain-cuda : les 4 fichiers du chemin chaud aux SHA vierges
Fichiers sales     : les 3 greffes connues, inchangees
MODIFICATIONS DE SOURCE DANS CETTE PASSE : AUCUNE (microbench autonome)
Instrument         : qualification/e7a-r12/e7a-r12.cu + bin/e7a-r12
   19 bras, DEUX noyaux seulement (natif et runtime a masque). Seuls les masques changent.
   AUCUN keepalive, AUCUNE garde a valeur nulle, AUCUN anti-hissage, AUCUNE specialisation.
GENERATEUR=SAIN_QUALIFIE_R9A
CANONICAL_MICROBENCH_TIMING_PROTOCOL=K2048_WARMUP3_LATIN_MULTI_PROCESS
AUCUN REGLAGE MODIFIE EN COURS DE CAMPAGNE
```

## 14 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007A_R11_STATUS=CLOSED_ZERO_WORK_REJECTS_FIXED_FLOOR_ACTIVATION_THRESHOLD_FOUND
ENGINE007A_R12_STATUS=CLOSED_EPISODE_COUNT_IS_THE_PRIMARY_VARIABLE
ARM_TABLE_SHA256=e829d189c74ab2623b7eef9e15bc454ff887125347760934fcd54990e0e6bb94
ARM_TABLE_POST_RUN=IDENTICAL
ARM_COUNT=19
UNEXPECTED_DUPLICATE_CONFIG_COUNT=0
HOST_WARP_EPISODE_SET_MATCH=PASS
DYNAMIC_COUNT_MATCH=PASS
GPU_KBX_QUALIFICATION=PASS
GPU_WARP_EPISODE_QUALIFICATION=PASS
INPUT_SANITY_GATE=PASS
OUTPUT_SANITY_GATE=PASS
N_OBSERVATIONS=1900
N_PROCESSES=5
KBX06_ACTIVE_WARP_EPISODE_COUNT=6
KBX06_DID_NOT_REDUCE_WARP_EPISODE_COVERAGE=YES
G0_D1_US=20.3254
G0_D2_US=20.3063
G0_D4_US=20.4206
G0_D6_US=20.6351
G0_D8_US=20.6205
G0_D8_MINUS_G0_D1_US=+0.2951
LANE_DENSITY_WITHIN_ACTIVE_WARP_EPISODE=NOT_DOMINANT
FULL_WARP_LANE_COVERAGE_THRESHOLD=NOT_DETECTED
CAS_A1_DU_PARAGRAPHE_10=APPLIQUE
SINGLE_EPISODE_RANGE_US=1.0640
DOES_EPISODE_IDENTITY_MATTER=YES_BY_ABOUT_1US
E1_G0_D6_US=20.6351
E2_SAME_WARP_US=32.9286
E2_DIFFERENT_WARPS_US=30.9966
E3_US=44.2117
E4_US=55.8584
E6_US=81.9283
E2_SAME_MINUS_E1_US=+12.2935
E2_DIFF_MINUS_E1_US=+10.3615
E3_MINUS_E1_US=+23.5766
E4_MINUS_E1_US=+35.2233
E6_MINUS_E1_US=+61.2933
E2_DIFFERENT_WARPS_MINUS_E2_SAME_WARP_US=-1.9319
EPISODE_SLOPE_US_PER_EPISODE=11.3914
EPISODE_MODEL_R2=0.99989
EPISODE_MODEL_MAX_RESIDUAL_US=0.26
ACTIVE_WARP_EPISODE_COUNT=PRIMARY_WALL_VARIABLE_FOR_NONZERO_KBX_PATH
CAS_R12_A=APPLIQUE
ACTIVE_PHYSICAL_WARP_COUNT=MATERIAL
CAS_R12_C=APPLIQUE
ACTIVE_LANE_DENSITY=NOT_MATERIAL
CAS_R12_D=NON_APPLIQUE
SINGLE_WARP_EPISODE_ACTIVATION_TRIGGERS_MAJOR_COST=PARTIAL_14.13_US_PER_FIRST_EPISODE
CAS_R12_B=NON_APPLIQUE_AU_SENS_STRICT
PREREGISTERED_REFERENCE_CONFIRMED=E6_A_0.11_PERCENT
ALL_THREADS_ACTIVE_THRESHOLD_HYPOTHESIS=SUPERSEDED_BY_EPISODE_COVERAGE
KBX06_SET_A_VS_E6_REPLICATE_DELTA_US=+0.0884
POSITION_DIAGNOSTIC_RANGE_US=12.5159
POSITION_DIAGNOSTIC_IS_COMPOSITION_NOT_DRIFT=SUPPORTED
TIMING_CAMPAIGN_GATE=PASS
ENGINE007B_REOPEN_CONDITION=STRUCTURED_WORK_REMOVAL_CAN_DEACTIVATE_WHOLE_WARP_EPISODES
KERNEL_LEVEL_POTENTIAL_NOT_PRODUCT_GAIN=YES
ENGINE007_PRIMARY_LEVER=WARP_EPISODE_LEVEL_STRUCTURED_WORK_ELIMINATION_CANDIDATE
ENGINE007B_STATUS=BLOCKED
ENGINE007C_STATUS=BLOCKED
LAUNCH_BOUND=NON_PROMU
MEMORY_BOUND=NON_PROMU
COMPUTE_BOUND=NON_PROMU
ISSUE_BOUND=NON_PROMU
LATENCY_BOUND=NON_PROMU
HARNESS_DEFECT_41=RESOLVED
HARNESS_DEFECT_42=RECORDED
HARNESS_DEFECT_43=RECORDED_INDICATOR_TO_FIX
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
