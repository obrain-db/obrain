# ENGINE-007A — MISE À L'ÉCHELLE DU TRAVAIL ARITHMÉTIQUE : MESURE NON CONCLUANTE

Date : 2026-09-29. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7a/`

---

## 0 — Résultat

**Les gates de construction sont franchis. Le gate scientifique ne l'est pas.**

```
ENGINE007A_KEEPALIVE_GATE=PASS
ENGINE007A_WORKLOAD_STRUCTURE_GATE=PASS
ENGINE007A_SAME_CARDINALITY_GATE=FAIL
ENGINE007A_STATUS=INCONCLUSIVE_INSTRUMENT_CANNOT_RESOLVE_THE_EFFECT
```

Le fait central, et il est net :

```
A0   (0 site dp4a)   : 82,2 à 87,1 us
A100 (32 sites dp4a) : 86,2 à 90,7 us

supprimer 100 % du travail dp4a ne fait PAS tomber le temps de facon nette
   -> l'ecart A0/A100 (4 a 5 us) est de l'ordre de la dispersion que j'ai mesuree
   -> il ne peut PAS etre attribue au travail arithmetique avec cet instrument
```

Conformément au §20, **je ne construis pas la régression** : le gate même-cardinalité doit passer
d'abord, et il échoue.

---

## 1 — Construction des variants (§5, §6)

Sept variantes **séparément compilées**, aucun commutateur runtime, masques distribués sur les
32 sites (4 tranches × 8 positions) :

| variante | sites actifs | masque |
|---|---|---|
| A0 | 0 | aucun |
| A25_A | 8 | `j` paire (4 tranches × 2) |
| A25_B | 8 | `j` impaire (4 tranches × 2) |
| A50_EVEN | 16 | tranches 0 et 2 complètes |
| A50_ODD | 16 | tranches 1 et 3 complètes |
| A75 | 24 | tranches 0,1,2 |
| A100 | 32 | tous |

Le masque est un paramètre de **template** (constexpr) et la boucle interne est entièrement
déroulée : la sélection est résolue à la compilation, sans branchement par site.

## 2 — Gate de keepalive (§2, §3, §4) : PASS, après une correction importante

Le keepalive consomme **les deux opérandes** de chaque site :

```cpp
const int dec = decode_byte((int) qs[8*t + j]);   // poids
const int act = (int) yy[kbx].qs[t*8 + j];        // activation
keep ^= dec;                                      // garde le decodage vivant
keep += act;                                      // garde les activations vivantes
if (site_active(t, j, MODE)) acc = dp4a(dec, act, acc);
```

Les accumulateurs sont initialisés depuis un **zéro runtime** (`runtime_zero`, argument de noyau)
pour empêcher le repliement de constante quand aucun site n'est actif (§11).

**La correction était nécessaire.** Mon premier jet ne gardait vivant que le décodage du poids, et le
relevé SASS a montré :

```
M0 (premier jet) : 32 LDG.E.U8.CONSTANT (poids)    0 LDG.E.S8 (activations)   0 IDP
M6 (premier jet) : 32 LDG.E.U8.CONSTANT (poids)   32 LDG.E.S8 (activations)  32 IDP
```

Le compilateur **repliait le chargement q8_1 dans l'opérande mémoire de l'IDP** : dès qu'un site
était désactivé, l'activation disparaissait. Le gate du §4 (`A0_Q8_1_LDG_COUNT == A100`) aurait
échoué. Signalé comme défaut de construction, pas comme résultat.

Après correction, le relevé SASS est **plat sur toute la famille** :

| | M0 | M3 | M4 | M6 |
|---|---|---|---|---|
| `LDG.E.U8` (poids) | **32** | **32** | **32** | **32** |
| `LDG.E.S8` (activations) | **32** | **32** | **32** | **32** |
| `IDP.4A` | **0** | **16** | **16** | **32** |
| `LOP3` (décodage) | **292** | **292** | **292** | **292** |
| `SHF` | 104 | 104 | 104 | 104 |
| `SHFL` / `FADD` / `BAR` / `STG` | 5/10/1/2 | 5/10/1/2 | 5/10/1/2 | 5/10/1/2 |
| registres | 39 | 40 | 40 | 40 |
| spills | 0 | 0 | 0 | 0 |

```
WEIGHT_LDG_COUNT_CONSTANT=YES
Q8_1_LDG_COUNT_CONSTANT=YES
TERNARY_DECODE_BODY_CONSTANT=YES
SHFL_COUNT_CONSTANT=YES   FADD_COUNT_CONSTANT=YES
BAR_COUNT_CONSTANT=YES    STG_COUNT_CONSTANT=YES
LAUNCH_GEOMETRY_CONSTANT=YES
NON_DP4A_BODY_COMPARABLE_ACROSS_VARIANTS=YES
```

Le SASS total croît **exactement** de 28 instructions par 8 sites (`M0=2209`, `M3=2241`,
`M6=2273`) : 3,5 instructions par site dp4a (l'`IDP` plus son adressage). L'instrument est propre.

## 3 — Le problème : un biais de position non maîtrisé

### Campagne 1 — ordre pré-enregistré, sans intercalaire

```
run01 A100 95,5536   run02 A50_EVEN 83,3648   run03 A0 82,3744   run04 A75 84,0336
run05 A25_A 82,2752  run06 A50_ODD 85,3008    run07 A50_ODD 83,8064  run08 A25_A 82,1968
run09 A75 84,0384    run10 A0 82,5680         run11 A50_EVEN 82,9056 run12 A100 85,7856
```

Regroupé par variante, le motif est visible :

| variante | run A | run B |
|---|---|---|
| A100 | 95,55 | 85,79 |
| A50_EVEN | 83,36 | 82,91 |
| A0 | 82,37 | 82,57 |
| A75 | 84,03 | 84,04 |
| A25_A | 82,28 | 82,20 |
| A50_ODD | 85,30 | 83,81 |

**La première mesure de chaque variante est systématiquement plus lente** (jusqu'à +10 µs pour
A100). C'est un artefact d'accès initial / d'état GPU, pas un effet du travail — d'autant que A100 a
le **plus grand nombre** d'`IDP` et se trouve être la variante la plus lente, ce qui donnerait une
corrélation inversée.

### Campagne 3 — avec travail neutre intercalé (§ conçu pour corriger ce biais)

J'ai ajouté, entre chaque mesure, un parcours neutre identique (une matrice jumelle, kernel
identique) pour égaliser l'état GPU :

```
run01 A100 86,20   run02 A50_EVEN 82,87   run03 A0 81,98   run04 A75 84,01
run05 A25_A 81,96  run06 A50_ODD 84,02    run07 A50_ODD 83,82 run08 A25_A 82,50
run09 A75 84,04    run10 A0 84,01         run11 A50_EVEN 84,10 run12 A100 93,31
```

Le biais persiste : `run01=86,20` et `run12=93,31`. **A100 est mesuré aux deux positions extrêmes**,
et la dernière mesure reste à 93,31 µs malgré l'intercalaire — ce que je **n'explique pas**.

## 4 — Gate même-cardinalité (§7, §8) : FAIL

A50_EVEN et A50_ODD font **exactement 16 sites dp4a chacun**, mais sur des emplacements différents :

| campagne | A50_EVEN | A50_ODD | \|écart\| |
|---|---|---|---|
| 1 | 84,6384 | 84,0128 | 0,63 µs |
| 2 | 83,1352 | 84,5536 | **1,42 µs** |
| 3 | 83,4848 | 83,9208 | 0,44 µs |

```
SAME_WORK_COUNT_DIFFERENT_PLACEMENT_CONTROL=FAILS
LATENCY_IS_NOT_A_FUNCTION_OF_IDP_COUNT_ALONE=SUPPORTED
SIMPLE_LINEAR_MODEL_VERSUS_COUNT=INVALID
```

Même travail, même topologie de réduction, même chargements : jusqu'à **1,42 µs** d'écart. Le §8
dit alors de ne pas ajuster de modèle linéaire et d'enquêter sur l'ordonnancement avant 007B.
C'est exactement ce que je fais : **pas de régression, pas de promotion**.

## 5 — Dispersion inter-campagnes

| variante | camp. 1 | camp. 2 | camp. 3 | étendue |
|---|---|---|---|---|
| A0 | 87,14 | 82,47 | 82,99 | **4,67 µs** |
| A25_A | 83,90 | 82,24 | 82,23 | 1,67 |
| A50_EVEN | 84,64 | 83,14 | 83,48 | 1,51 |
| A50_ODD | 84,01 | 84,55 | 83,92 | 0,64 |
| A75 | 86,91 | 84,04 | 84,03 | 2,88 |
| A100 | 86,16 | 90,67 | 89,76 | **4,51 µs** |

La dispersion inter-campagnes sur une **même** variante (jusqu'à 4,7 µs) est du même ordre que
l'effet cherché (A0−A100 ≈ 4 à 5 µs). **L'instrument ne peut donc pas résoudre l'effet.**

## 6 — Interprétation honnête (§23, §24, §25, §26)

Aucun des cas A/B/C/D ne peut être promu :

- **Cas A** (mise à l'échelle linéaire) : non — le §23 exige le gate même-cardinalité ET la
  monotonie ET un R² élevé. Le gate échoue.
- **Cas C** (seuil) : non — il faudrait `A100≈A75≈A50` puis chute vers A0. Les trois campagnes ne
  montrent pas ce motif de façon reproductible.
- **Cas D** (même compte, latence différente) : **c'est le cas observé**, mais l'écart
  A50_EVEN/A50_ODD (0,44 à 1,42 µs) est du même ordre que la dispersion de mesure, donc il ne
  démontre pas non plus que l'emplacement compte.

Ce qui reste défendable, avec des précautions :

```
A0_TO_A100_WALL_DIFFERENCE=4_TO_5_US
   sur une duree de noyau de ~85 us, soit ~5 a 6 %
IDP_ASSOCIATED_INCREMENT=4_TO_5_US_IF_REAL
   mais NON distinguable de la dispersion mesuree dans cette passe
USEFUL_ARITHMETIC_WORK_EXPLAINS_SMALL_FRACTION_OF_WALL=CONSISTENT_WITH_DATA_NOT_DEMONSTRATED
```

Le fait le plus solide est **qualitatif et robuste** : supprimer **la totalité** des 32 `dp4a`
d'un thread — 100 % du travail de produit scalaire — ne produit aucune chute franche du temps. Le
noyau de 85 µs reste à 82-87 µs sans aucun `dp4a`. C'est cohérent avec tout le dossier 006
(aucune transformation locale n'a jamais rien rapporté) et cela **affaiblit** l'hypothèse
`DP4A_EXECUTION_THROUGHPUT_LIMIT` que 006I avait laissée comme la plus probable.

Mais cela ne la remplace pas par une autre : je ne sais toujours pas ce qui domine les 85 µs.

```
DP4A_EXECUTION_THROUGHPUT_LIMIT=WEAKENED
   (cas B du paragraphe 24 : le travail utile explique une petite fraction de la paroi)
COMPUTE_BOUND=NOT_PROVEN      ISSUE_THROUGHPUT_LIMITED=NOT_PROVEN
COMPARISON_TO_006F_DECODE_22_60_US=A0_82_TO_87_US_DIFFERENT_WITH_EXPLANATION
   (l'instrument porte un keepalive XOR + add sur 32 sites, absent de 006F : les deux ne sont pas
    comparables directement ; l'ecart de ~60 us est du meme ordre que le keepalive lui-meme)
```

## 7 — Décision

```
ENGINE007A_STATUS=INCONCLUSIVE_INSTRUMENT_LIMITED
ENGINE007B_STATUS=BLOCKED
ENGINE007_PRIMARY_LEVER=NOT_IDENTIFIED
```

Le §30 exige `LATENCY_SCALES_WITH_USEFUL_ARITHMETIC_WORK=MEASURED` pour ouvrir 007B. Ce n'est pas
obtenu. 007B reste bloqué — je n'ouvre pas l'audit de sparsité sur une base non établie, et le §33
l'interdit explicitement sans `ARITHMETIC_WORK_SCALING=MATERIAL`.

**Ce que je proposerais pour rendre la mesure concluante** (à décider, je ne l'ai pas fait) : la
dispersion domine parce que chaque bras n'est mesuré que 2 fois et que 20 invocations de 85 µs ne
sont pas assez longues pour amortir la dérive. Passer à des mesures de plusieurs milliers
d'invocations par bras, avec les bras **entrelacés individuellement** plutôt que groupés par
mesure, et sur plusieurs processus pour éliminer l'effet de position, donnerait la résolution
nécessaire. Sans cela, tout résultat A0/A100 restera dans le bruit.

## 8 — Provenance

```
Runtime baseline   : libllama 3db83675… / libggml-cuda abafbf0d…  (restaures, SHA verifies)
Arbre llama-obrain-cuda : les 4 fichiers du chemin chaud aux SHA vierges
Fichiers sales     : les 3 greffes connues, inchangees
MODIFICATIONS DE SOURCE DANS CETTE PASSE : AUCUNE (variantes en microbench autonome)
Instrument         : qualification/e7a/e7a-scaling.cu + bin/e7a-scaling
PLUMBING_FAILURE_29=NON_ASSIGNE   (les defauts ci-dessous sont des defauts de harnais, cf. paragraphe 37)
```

Défauts de harnais consignés :

```
HARNESS_DEFECT_28=KEEPALIVE_NE_GARDAIT_PAS_LES_ACTIVATIONS
   -> le compilateur repliait le chargement q8_1 dans l'operande memoire de l'IDP ; les activations
      disparaissaient des qu'un site etait desactive. Le gate du paragraphe 4 aurait echoue.
      Detecte par releve SASS AVANT toute mesure. Corrige. Aucun chiffre tire du keepalive fautif.
HARNESS_DEFECT_29=EFFET_DE_POSITION_NON_MAITRISE_DANS_L_ORDRE_DE_MESURE
   -> la premiere mesure de chaque variante est systematiquement plus lente ; un travail neutre
      intercale reduit mais n'elimine pas l'effet. Non resolu : c'est la cause de l'inconclusion.
HARNESS_DEFECT_30=PREMIERE_MESURE_A100_NON_EXPLIQUEE_A_93_US
   -> persiste apres intercalaire. Aucune interpretation tentee.
```

## 9 — Ledger

```
ENGINE006_STATUS=CLOSED_NO_MATERIAL_RECOVERABLE_LOCAL_WASTE_FOUND
ENGINE007_ALLOWED=YES
ENGINE007_STATUS=OPEN
ENGINE007A_STATUS=INCONCLUSIVE_INSTRUMENT_LIMITED
ENGINE007A_PRIMARY_VARIABLE=USEFUL_IDP_4A_COUNT
ENGINE007A_VARIANTS_BUILT=7
ENGINE007A_SAME_CARDINALITY_CONTROL=FAILS
ENGINE007A_KEEPALIVE_GATE=PASS
ENGINE007A_WORKLOAD_STRUCTURE_GATE=PASS
KEEPALIVE_PRESENT_IN_ALL_VARIANTS=YES
KEEPALIVE_IMPLEMENTATION_IDENTICAL_ACROSS_VARIANTS=YES
A0_WEIGHT_LDG_COUNT_EQ_A100=YES_32_EQ_32
A0_Q8_1_LDG_COUNT_EQ_A100=YES_32_EQ_32
A0_TERNARY_DECODE_PATH_PRESENT=YES
A100_TERNARY_DECODE_PATH_PRESENT=YES
WEIGHT_LDG_COUNT_CONSTANT=YES
Q8_1_LDG_COUNT_CONSTANT=YES
TERNARY_DECODE_BODY_CONSTANT=YES
SHFL_COUNT_CONSTANT=YES
FADD_COUNT_CONSTANT=YES
BAR_COUNT_CONSTANT=YES
STG_COUNT_CONSTANT=YES
LAUNCH_GEOMETRY_CONSTANT=YES
REGISTERS_RANGE=39_TO_40
SPILLS=0_IN_ALL_SEVEN_VARIANTS
A0_US_MEAN=82.47_TO_87.14
A25_US_MEAN=82.23_TO_83.90
A50_EVEN_US_MEAN=83.14_TO_84.64
A50_ODD_US_MEAN=83.92_TO_84.55
A75_US_MEAN=84.03_TO_86.91
A100_US_MEAN=86.16_TO_90.67
CROSS_CAMPAIGN_DISPERSION_MAX=4.67_US
A0_TO_A100_DIFFERENCE=4_TO_5_US
EFFECT_TO_DISPERSION_RATIO=APPROX_1
ARITHMETIC_WORK_MONOTONICITY=NOT_ESTABLISHED
LINEAR_MODEL_FIT=NOT_CONSTRUCTED_GATE_FAILED
LATENCY_IS_NOT_A_FUNCTION_OF_IDP_COUNT_ALONE=SUPPORTED
USEFUL_ARITHMETIC_WORK_EXPLAINS_SMALL_FRACTION_OF_WALL=CONSISTENT_NOT_DEMONSTRATED
DP4A_EXECUTION_THROUGHPUT_LIMIT=WEAKENED
COMPUTE_BOUND=NOT_PROVEN
ISSUE_THROUGHPUT_LIMITED=NOT_PROVEN
ENGINE007B_STATUS=BLOCKED
ENGINE007_PRIMARY_LEVER=NOT_IDENTIFIED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
