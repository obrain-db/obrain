# ENGINE-006H — GÉOMÉTRIE WARPS-PAR-LIGNE : REJETÉE

Date : 2026-09-29. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e6h/`

---

## 0 — Résultat

**Aucune géométrie ne gagne. L'axe est fermé, et il est fermé de façon instructive.**

```
WPR1_FULL_US=80,03    WPR2_FULL_US=79,87    WPR4_FULL_US=79,93    WPR8_FULL_US=79,98
WPR2_DELTA_US=-0,072     WPR2_SPEEDUP_RATIO=0,9991

ACTIVE_WARPS_PER_SM : WPR1=24   WPR2=48   WPR4=48   WPR8=48   -> WPR1_OCCUPANCY_RISK=HIGH (confirme)
```

Étendue totale des quatre géométries : **0,2 %**. Le critère du §22 (« gain > bruit, reproductible »)
n'est pas franchi.

Deux faits annexes sont plus importants que le résultat lui-même :

1. **Le test décisif est négatif dans les deux sens.** WPR1 tourne à **24 warps actifs par SM** au
   lieu de 48 — moitié moins de parallélisme de warp — et **ne perd rien**. WPR2 conserve 48 warps
   avec 2 warps coopérant au lieu de 4, et **ne gagne rien**. Une géométrie qui ne bouge ni en
   perdant la moitié des warps ni en réduisant la coordination n'est pas le levier.
2. **La fidélité bit-à-bit n'est PAS préservée** : 3193 lignes sur 5120 diffèrent entre WPR2 et
   WPR4. Une géométrie qui change la partition change l'**ordre** de la somme flottante. Ce n'est
   pas une perte de justesse mathématique, mais cela disqualifie le niveau de preuve que les passes
   précédentes exigeaient.

```
ENGINE006H_STATUS=CLOSED_NO_GEOMETRY_GAIN
WARPS_PER_ROW_GEOMETRY_HEADROOM=NOT_DETECTED_AROUND_WPR4
PRODUCT_CAMPAIGN=NOT_RUN
NEXT_ACTION=BUILD_G1_EXACT_SPECIALIZATION   (regle du §28)
```

---

## 1 — Freeze 006G (§1, §2, §3)

```
ENGINE006G_G2_STATUS=REJECTED_BY_MEASUREMENT
DP4A_STATIC_INSTRUCTION_COUNT=8
DP4A_DYNAMIC_EXECUTION_COUNT=384_PAR_WARP_EN_WPR4   (reporte separement, conformement au §1)
INDEPENDENT_ACCUMULATOR_COUNT=2
G2_ACC2_GAIN=WITHIN_NOISE
DP4A_DEPENDENCY_OPTIMIZATION_HEADROOM=NOT_DETECTED
CURRENT_COMPILER_ALREADY_EXPOSES_MULTIPLE_DP4A_ACCUMULATORS=YES
REDUCTION_SHFL_COUNT=5   REDUCTION_ADD_COUNT=5   REDUCTION_STAGE_COUNT=1   REDUCTION_DOMINATES=NO

STREAM_CACHE_SENSITIVITY=4.10_TO_4.37
FULL_CACHE_SENSITIVITY=NOT_POSITIVE
STREAM_MICROBENCH_IS_CACHE_RESIDENT=CONFIRMED_BY_HOT_COLD_CONTROL
FULL_KERNEL_STRONG_DRAM_SENSITIVITY=NOT_OBSERVED
MEMORY_BOUND=EMPIRICALLY_DISFAVORED_NOT_FORMALLY_EXCLUDED
COMPUTE_BOUND=NOT_PROVEN   ISSUE_THROUGHPUT_LIMITED=NOT_PROVEN   LATENCY_LIMITED=NOT_PROVEN

ENGINE006G_G1_STATUS=AUDITED_CEILING_ESTIMATED_LOW_PROTOTYPE_NOT_BUILT
GENERIC_MACHINERY_DYNAMIC_PATH_INSTRUCTIONS=40_TO_50
G1_STATIC_CEILING=5_TO_6_PERCENT
G1_RESTE_OUVERT=YES
IF_ENGINE006H_FAILS_BUILD_G1=DECLENCHE
```

## 2 — L'invariant du §5, vérifié analytiquement

```
rows/block=1 : 128 threads  blocs=5120  total_warps=20480  blocs/SM=12  warps actifs/SM=48  vagues=8,9
rows/block=2 : 256 threads  blocs=2560  total_warps=20480  blocs/SM= 6  warps actifs/SM=48  vagues=8,9
rows/block=4 : 512 threads  blocs=1280  total_warps=20480  blocs/SM= 3  warps actifs/SM=48  vagues=8,9

TOTAL_WARPS constant ; warps actifs/SM constant ; nombre de vagues constant
MORE_ROWS_PER_BLOCK_ALONE=NO_STRONG_PARALLELISM_MECHANISM_IDENTIFIED
```

L'invariant du §5 est **confirmé** : « plus de lignes par bloc » ne change rien au parallélisme.
L'axe est écarté avant d'être codé, conformément au §5.

## 3 — Modèle analytique par WPR (§6 à §9)

| WPR | threads/bloc | blocs | blocs/SM (threads) | blocs/SM (38 regs) | actifs | warps actifs/SM | vagues | kbx/warp | dp4a/warp |
|---|---|---|---|---|---|---|---|---|---|
| 1 | 32 | 5120 | 48 | 48 | *plafond blocs* | — | — | 48 | 1536 |
| 2 | 64 | 5120 | 24 | 24 | 24 | **48** | 4,4 | 24 | 768 |
| 4 | 128 | 5120 | 12 | 12 | 12 | **48** | 8,9 | 12 | 384 |
| 8 | 256 | 5120 | 6 | 6 | 6 | **48** | 17,8 | 6 | 192 |

```
MATHEMATICAL_WORK_PER_OUTPUT_ROW_UNCHANGED=YES   (1536 dp4a et 1536 octets decodes par ligne,
                                                  dans les quatre variantes)
```

## 4 — Couverture kbx : exacte dans toutes les variantes (§11, §12)

Simulation fidèle du mapping produit (`qi = QI_G128 = 4`, `vdr = 1`, pas = `vdr·nwarps·32/qi`) :

```
nwarps=1  threads= 32  pas= 8  : 192/192 paires, 0 manquante, 0 doublon
nwarps=2  threads= 64  pas=16  : 192/192 paires, 0 manquante, 0 doublon
nwarps=4  threads=128  pas=32  : 192/192 paires, 0 manquante, 0 doublon
nwarps=8  threads=256  pas=64  : 192/192 paires, 0 manquante, 0 doublon

NO_MISSING_KBX=YES   NO_DUPLICATE_KBX=YES   (pour les quatre)
```

Partition par warp — et c'est ici qu'apparaît un fait qui n'avait pas été relevé :

```
WPR2 : warp0 = 24 kbx contigus [0..23]        warp1 = 24 kbx [24..47]
WPR4 : warp0 = 16 kbx [0..7, 32..39]          warp1 = 16 kbx [8..15, 40..47]
       warp2 =  8 kbx [16..23]                warp3 =  8 kbx [24..31]
```

**La géométrie de production est déséquilibrée** : deux warps traitent 16 blocs, deux autres 8.
Le pas vaut `nwarps·8`, donc WPR2 rééquilibre à 24/24 et WPR8 à 8/8.

```
CURRENT_WPR4_WARP_LOAD_BALANCE=16_16_8_8_UNBALANCED
WPR2_WARP_LOAD_BALANCE=24_24_BALANCED
```

WPR2 corrige donc un déséquilibre réel — et malgré cela, il ne gagne rien (§6). C'est un argument
supplémentaire contre l'axe.

## 5 — Occupation mesurée par l'API CUDA (§8), pas extrapolée

`cudaFuncGetAttributes` + `cudaOccupancyMaxActiveBlocksPerMultiprocessor` sur les kernels réels :

| variante | registres | smem | threads | blocs/SM | warps/bloc | **warps actifs/SM** |
|---|---|---|---|---|---|---|
| WPR1 | 38 | 4 o | 32 | 24 | 1 | **24** |
| WPR2 | 38 | 8 o | 64 | 24 | 2 | **48** |
| WPR4 | 38 | 16 o | 128 | 12 | 4 | **48** |
| WPR8 | 37 | 32 o | 256 | 6 | 8 | **48** |

```
WPR1_OCCUPANCY_RISK=HIGH_CONFIRME   (le plafond de 24 blocs resident par SM limite a 24 warps)
WPR2_ACHIEVES_SAME_ACTIVE_WARPS_AS_WPR4=YES   (l'hypothese optimiste du §7 est verifiee)
```

Le mécanisme envisagé au §7 est donc **réellement disponible** : WPR2 atteint bien 48 warps actifs
par SM avec seulement 2 warps coopérant par ligne. C'est ce qui rend le résultat nul significatif —
l'hypothèse n'a pas échoué faute de conditions, elle a échoué **avec** les conditions réunies.

## 6 — Microbench (§21, §22)

Ordre pré-enregistré `WPR4 WPR2 WPR2 WPR4 WPR4 WPR2` puis trois répétitions supplémentaires :

| run | WPR2 (µs) | WPR4 (µs) | écart |
|---|---|---|---|
| 1 | 79,9312 | 79,9216 | −0,0096 |
| 2 | 79,8512 | 79,9280 | −0,0768 |
| 3 | 79,8432 | 79,9040 | −0,0608 |
| 4 | 79,8352 | 79,9520 | −0,1168 |
| 5 | 79,8896 | 79,9232 | −0,0336 |
| 6 | 79,8496 | 79,9152 | −0,0656 |

```
WPR2_MEAN=79,8667   WPR4_MEAN=79,9240   DELTA=-0,0573 us
WPR2_SPEEDUP_RATIO=0,9993
```

Le signe est **cohérent sur les 6 runs** (WPR2 toujours un peu plus rapide), donc l'effet est
probablement réel — mais il vaut **0,07 %** de la durée du noyau. Le §22 exige « gain > bruit
**et** reproductible » : la reproductibilité est là, l'amplitude utile ne l'est pas.

```
WPR2_GAIN_IS_DIRECTIONALLY_CONSISTENT=YES
WPR2_GAIN_MAGNITUDE=0.07_PERCENT_NEGLIGIBLE
PROMOTED=NO
```

**Contrôle de cohérence du microbench** : mon `k_wpr<4>` donne 79,93 µs, identique à `k_full` de
006F/G (79,93 µs). Les deux implémentations indépendantes convergent — le microbench mesure bien la
même chose que les passes précédentes, malgré la réduction en mémoire partagée ajoutée ici.

## 7 — Fidélité : NON préservée (§18)

```
WPR1_VS_WPR4_OUTPUT_MISMATCH=3193 / 5120
WPR2_VS_WPR4_OUTPUT_MISMATCH=3223 / 5120
WPR8_VS_WPR4_OUTPUT_MISMATCH=3319 / 5120
OUTPUT_BITWISE_MATCH_WPR2_VS_WPR4=NO
```

63 % des lignes diffèrent. La raison est structurelle et sans mystère : **changer la partition des
kbx change l'ordre des additions flottantes**, et l'addition flottante n'est pas associative. Ce n'est
pas une erreur de calcul — c'est un arrondi différent.

Conséquence méthodologique, à porter pour toute suite :

```
GEOMETRY_CHANGE_BREAKS_BITWISE_KERNEL_FIDELITY=YES
KERNEL_LEVEL_BITWISE_ACCEPTANCE_TEST_UNAVAILABLE_FOR_GEOMETRY_VARIANTS
   -> une variante de geometrie exigera une validation AU NIVEAU MODELE (probe logits, sequence)
      et non plus seulement la comparaison kernel bit a bit utilisee depuis 006C.
```

C'est un argument de coût qui s'ajoute au gain nul : adopter WPR2 imposerait une validation
complète au niveau modèle pour 0,07 % de gain.

## 8 — Interprétation (§23, §24, §25)

Le §23 prévoyait : si `ACTIVE_WARPS_PER_SM_WPR2 ≈ WPR4` **et** WPR2 gagne, alors la géométrie WPR4
a une surcharge récupérable. Les deux conditions ne sont pas réunies : l'occupation est identique
(48 = 48, mesuré par l'API) mais le gain est nul.

```
CURRENT_WPR4_GEOMETRY_HAS_RECOVERABLE_COORDINATION_OR_SCHEDULING_OVERHEAD=NO
WARPS_PER_ROW_GEOMETRY_HEADROOM=NOT_DETECTED_AROUND_WPR4
```

Le fait le plus fort est WPR1 : **24 warps actifs par SM au lieu de 48** — la moitié du parallélisme
de warp — et **aucune perte**. Cela indique que le noyau n'est pas limité par le nombre de warps
résidents, ni par la coordination inter-warp, ni par le déséquilibre de charge (WPR2 l'annule, sans
effet). Combiné à 006G (pas de sensibilité mémoire, pas de gain ILP DP4A, réduction minimale), le
faisceau d'indices pointe vers une limite qui ne se voit **ni dans la géométrie, ni dans la mémoire,
ni dans la structure de dépendances** — donc vers le **débit d'exécution des `dp4a` eux-mêmes**.

Mais je ne promus pas `ISSUE_THROUGHPUT_LIMITED=YES` : c'est une hypothèse cohérente avec cinq
mesures négatives, pas une mesure. Le §2 l'interdit explicitement.

## 9 — Suite (§28) : G1 devient obligatoire

```
ENGINE006H_STATUS=CLOSED_NO_GEOMETRY_GAIN
NEXT_ACTION=BUILD_G1_EXACT_SPECIALIZATION
G1_STATIC_NEGATIVE_INSTRUCTION_DELTA=40_TO_50
G1_EXPECTED_CEILING=5_TO_6_PERCENT
```

La règle du §28 s'applique : G1 est désormais le **seul axe restant** avec un delta d'instructions
statique négatif identifié. Il doit être construit avant toute fermeture d'ENGINE-006, même si son
plafond attendu est modeste.

Bilan des axes à ce stade :

| axe | statut | preuve |
|---|---|---|
| V1 décodage ternaire | rejeté | produit non mesurable, SASS +40 |
| vectorisation des poids | rejeté | alignement 2 o, delta instructions nul |
| résidence q8_1 partagée | rejeté | zéro redondance inter-warp |
| ILP / accumulateurs DP4A | rejeté | gain 0,14 µs puis −0,02 µs |
| réduction | déjà minimale | 5 shuffles, 1 barrière |
| géométrie WPR | **rejeté** | 0,07 %, et WPR1 à 24 warps ne perd rien |
| machinerie générique (G1) | **à construire** | delta statique −40 à −50 |

## 10 — Provenance

```
Runtime baseline   : libllama 3db83675… / libggml-cuda abafbf0d…  (restaures, SHA verifies)
Arbre llama-obrain-cuda : les 4 fichiers du chemin chaud aux SHA vierges
Fichiers sales     : les 3 greffes connues, inchangees
MODIFICATIONS DE SOURCE DANS CETTE PASSE : AUCUNE
Instrument         : qualification/e6h/e6h-wpr.cu + bin/e6h-wpr
```

Défaut de harnais de cette passe — un **faux résultat intermédiaire** que je signale :

```
HARNESS_DEFECT_26=MODELE_DE_COUVERTURE_KBX_FAUX_POUR_LA_BASELINE
   Mon premier modèle analytique concluait que la baseline WPR4 ne couvrait que 32 blocs sur 48
   ("NO_MISSING_KBX=NO" sur le mapping de production !). C'était une erreur de modèle : le pas vaut
   nwarps*8, pas 128. Corrige par simulation fidele depuis la source : 192/192 paires, 0 manquante.
   Aucune conclusion n'a ete tiree du modele fautif.
```

## 11 — Ledger

```
ENGINE006_STATUS=OPEN
ENGINE006G_G2_STATUS=REJECTED_BY_MEASUREMENT
ENGINE006G_G3_STATUS=CLOSED_REDUCTION_MINIMAL
ENGINE006G_G1_STATUS=DEFERRED_BUT_NOT_CLOSED
ENGINE006H_STATUS=CLOSED_NO_GEOMETRY_GAIN
FULL_KERNEL_STRONG_DRAM_SENSITIVITY=NOT_OBSERVED
MEMORY_BOUND=EMPIRICALLY_DISFAVORED_NOT_FORMALLY_EXCLUDED
DP4A_DEPENDENCY_HEADROOM=NOT_DETECTED
DP4A_STATIC_INSTRUCTION_COUNT=8
DP4A_DYNAMIC_EXECUTION_COUNT_WPR4=384_PAR_WARP
MORE_ROWS_PER_BLOCK_ALONE=NO_STRONG_PARALLELISM_MECHANISM_IDENTIFIED
NO_MISSING_KBX=YES
NO_DUPLICATE_KBX=YES
CURRENT_WPR4_WARP_LOAD_BALANCE=16_16_8_8_UNBALANCED
WPR2_WARP_LOAD_BALANCE=24_24_BALANCED
WPR1_ACTIVE_WARPS_PER_SM=24
WPR2_ACTIVE_WARPS_PER_SM=48
WPR4_ACTIVE_WARPS_PER_SM=48
WPR8_ACTIVE_WARPS_PER_SM=48
WPR1_OCCUPANCY_RISK=HIGH_CONFIRMED
WPR2_ACHIEVES_SAME_ACTIVE_WARPS_AS_WPR4=YES
WPR2_DELTA_US=-0.0573
WPR2_SPEEDUP_RATIO=0.9993
WPR2_GAIN_IS_DIRECTIONALLY_CONSISTENT=YES
WPR2_GAIN_MAGNITUDE=0.07_PERCENT_NEGLIGIBLE
WARPS_PER_ROW_GEOMETRY_HEADROOM=NOT_DETECTED_AROUND_WPR4
CURRENT_WPR4_GEOMETRY_HAS_RECOVERABLE_COORDINATION_OVERHEAD=NO
OUTPUT_BITWISE_MATCH_WPR2_VS_WPR4=NO
GEOMETRY_CHANGE_BREAKS_BITWISE_KERNEL_FIDELITY=YES
PRODUCT_CAMPAIGN=NOT_RUN
MICROBENCH_INTERNAL_CONSISTENCY=WPR4_79.93_US_EQUALS_006F_K_FULL
NEXT_AXIS=G1_EXACT_SPECIALIZATION_MANDATORY
ENGINE007_ALLOWED=NO
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
