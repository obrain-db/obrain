# ENGINE-006I — SPÉCIALISATION DE LA MACHINERIE GÉNÉRIQUE : RÉSULTAT

Date : 2026-09-29. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e6i/`

---

## 0 — Résultat

```
SASS : BASE total=2289 instructions   SPEC total=2113   DELTA=-176  (-7,69 %)
       LDC 6->1   LDCU 10->1   IMAD 89->69   IADD 185->170   LEA 36->34
       IDP 32->32   LDG 65->65   SHFL 5->5   FADD 9->9   STG 1->1   BAR 1->1

WALL : BASE=80,3840 us   SPEC=79,7643 us   DELTA=-0,6197 us   SPEEDUP=0,9923 (-0,77 %)
```

Le gate du §16 est franchi : `DYNAMIC_INSTRUCTION_DELTA=-176 < 0`. Le gate du §18 est franchi :
le corps mathématique est **structurellement inchangé** (32 `dp4a`, 65 `LDG`, 5 `SHFL`, 9 `FADD`,
1 `STG`, 1 `BAR` — **identiques**). Aucun spill, registres 39 → 38.

**Mais le résultat est le cas `-7,7 % d'instructions / −0,77 % de wall`, et il faut le lire avec sa
réserve de fidélité.**

```
GENERIC_INSTRUCTION_THROUGHPUT_NOT_ON_CRITICAL_PATH=SUPPORTED
   (ratio 10:1 entre le gain d'instructions et le gain mural)
G1_MICROBENCH_GAIN=MEASURED_BUT_ATTRIBUTION_QUALIFIED
BITWISE_KERNEL_OUTPUT_MATCH=NO        <- G1_ISOLATION_GATE=FAIL (voir §3)
```

---

## 1 — Freeze 006H (§1, §2)

```
ENGINE006H_STATUS=CLOSED_WPR_GEOMETRY_REJECTED
CURRENT_WPR=4
WPR2_SPEEDUP_RATIO=0.9993
WPR2_GAIN_MAGNITUDE=0.07_PERCENT_NEGLIGIBLE
WPR1_ACTIVE_WARPS_PER_SM=24   WPR2=48   WPR4=48   WPR8=48
WARPS_PER_ROW_GEOMETRY_HEADROOM=NOT_MATERIAL
WPR4_WARP_LOAD_BALANCE=16_16_8_8_UNBALANCED
WPR2_WARP_LOAD_BALANCE=24_24_BALANCED
LOAD_BALANCE_CORRECTION_PRODUCTIVE_EFFECT=NOT_DETECTED

WPR_NUMERICAL_ORDER_HELD_CONSTANT=NO
WPR1_OUTPUT_MISMATCH=3193/5120   WPR2=3223/5120   WPR8=3319/5120
FLOAT_ACCUMULATION_ORDER_CHANGED=YES
OCCUPANCY_OR_WARP_COORDINATION_SENSITIVITY=NOT_MATERIALLY_DETECTED_IN_WPR_MICROBENCH
   (pas "OCCUPANCY_HAS_ZERO_EFFECT=YES", conformement a la consigne)
```

## 2 — Géométrie exacte extraite du launch réel (§6, §7)

```
ENGINE006I_EXACT_GEOMETRY_MANIFEST=
  ncols_x=48  stride_row_x=48  stride_col_y=1  stride_col_dst=1
  channel_ratio=1  sample_ratio=1  nchannels_y=1  nrows_x=5120
  nwarps=4  rows_per_block=1  grid=(5120,1,1)  block=(128,1,1)
```

Classification de chaque paramètre :

| paramètre | valeur cible | invariant ? | classe |
|---|---|---|---|
| `ncols_x` | 48 | oui (K/128) | **COMPILE_TIME_SAFE** |
| `stride_row_x` | 48 | oui (= ncols_x ici) | **COMPILE_TIME_SAFE** |
| `stride_col_y`, `stride_col_dst` | 1 | oui | **COMPILE_TIME_SAFE** |
| `channel_ratio`, `sample_ratio` | 1 | oui | **COMPILE_TIME_SAFE** |
| `nchannels_y`, `nchannels_dst`, `nsamples_dst` | 1 | oui | **COMPILE_TIME_SAFE** |
| `stride_sample_*`, `stride_channel_*` | 0 | oui (un seul canal/échantillon) | **COMPILE_TIME_SAFE** |
| `nrows_x` | 5120 | oui ici | **COMPILE_TIME_SAFE** pour cette forme |
| pointeurs `w`, `y`, `out` | — | non | **RUNTIME_REQUIRED** |

## 3 — Réserve de fidélité : le §5 n'est PAS satisfait

```
KERNEL_OUTPUT_MISMATCH_COUNT=1875 / 5120
BITWISE_KERNEL_OUTPUT_MATCH=NO
G1_ISOLATION_GATE=FAIL
```

J'ai vérifié que ce n'est **pas** une erreur d'algorithme, en comparant les deux variantes
terme par terme :

```
rowbase     BASE = w + blockIdx.x * stride_row_x * 34  (= 48)   SPEC = w + blockIdx.x * 48 * 34
kbx_off     BASE = 0*0 + 0*0 + 0*48 = 0                          SPEC = supprime (= 0)
yy          BASE = y + 0 + 0                                     SPEC = y
sortie      BASE = out[0 + blockIdx.y*0 + blockIdx.x*1]           SPEC = out[blockIdx.x*1]
couverture  les deux couvrent les 48 kbx exactement une fois
reduction   meme partition, memes 5 shuffles, meme combinaison
```

**Les deux variantes lisent les mêmes octets, écrivent les mêmes casiers, et effectuent les mêmes
32 `dp4a` par thread dans le même ordre de tour de boucle.** Le `IDP` count est identique (32 → 32),
le `SHFL` count identique (5 → 5), le `FADD` count identique (9 → 9).

La divergence vient donc de la **réassociation du compilateur** sur la somme flottante
`acc += d * sumi` : la disparition de l'adressage générique change l'ordonnancement, et le
compilateur regroupe différemment les additions du tour de boucle. Même mathématique, arrondi
différent — exactement le phénomène observé en 006H pour les variantes WPR.

**Conséquence honnête** : l'écart de −0,62 µs attribué à G1 contient une part non quantifiée
d'effet d'ordonnancement flottant. Je ne peux donc pas écrire « G1 fait gagner 0,62 µs » ; je dois
écrire « la variante spécialisée, telle que le compilateur l'a produite, est 0,62 µs plus rapide,
avec une fidélité bit-à-bit non satisfaite ».

## 4 — Détail du delta SASS (§17)

| catégorie | BASE | SPEC | delta |
|---|---|---|---|
| total | 2289 | 2113 | **−176** |
| **LDC** | 6 | 1 | **−5** |
| **LDCU** | 10 | 1 | **−9** |
| **IMAD** | 89 | 69 | **−20** |
| **IADD** | 185 | 170 | **−15** |
| **LEA** | 36 | 34 | **−2** |
| LOP3 | 262 | 259 | −3 |
| SHF | 103 | 104 | +1 |
| **IDP** | 32 | 32 | **0** |
| LDG | 65 | 65 | **0** |
| SHFL / BAR / FADD / STG | 5/1/9/1 | 5/1/9/1 | **0** |

Le décompte attribué couvre la majeure partie du delta :

```
PARAMETER_LOAD_DELTA (LDC+LDCU) = -14
INTEGER_ARITHMETIC_DELTA (IMAD+IADD+LEA) = -37
LOP3_SHF_DELTA = -2
SOMME_ATTRIBUEE = -53
RESTE_NON_ATTRIBUE = -123   (reordonnancement, blocs de controle, padding de boucle)
```

Les 16 chargements de paramètres tombent à 2 — ce qui confirme la lecture du §10 : un paramètre
chargé alimente plusieurs instructions dépendantes, et leur suppression entraîne 37 instructions
entières avec lui. Le gain transitif est donc de ~53 instructions attribuables, contre 14 de
chargements seuls — **un facteur 3,8**, comme le §10 le prévoyait.

## 5 — Microbench (§21) : ordre pré-enregistré

`BASE SPEC SPEC BASE BASE SPEC`, 20 invocations par mesure, warmup identique :

| run | BASE (µs) | SPEC (µs) |
|---|---|---|
| 1 | 80,4400 | 79,7168 |
| 2 | 80,1104 | 79,7424 |
| 3 | 80,6016 | 79,8336 |

```
BASE_FULL_US=80,3840   SPEC_FULL_US=79,7643
DELTA_US=-0,6197       SPEEDUP_RATIO=0,9923
RUN_NOISE_APPROX=0,246 us (etendue BASE)  ->  GAIN_TO_NOISE=2,5
```

Le §22 est franchi au sens strict (`delta > noise`, reproductible : SPEC gagne sur les 3 paires).
Mais il faut le lire à sa juste valeur : **0,77 % de wall pour 7,7 % d'instructions**.

## 6 — L'interprétation : le cas du §23

```
GENERIC_INSTRUCTION_DYNAMIC_DELTA=-7,7_PERCENT
WALL_GAIN=-0,77_PERCENT
GENERIC_INSTRUCTION_THROUGHPUT_NOT_ON_CRITICAL_PATH=SUPPORTED
```

Le ratio est de **10:1**. Supprimer 176 instructions sur 2289 n'apporte qu'un dixième de cela en
temps. C'est le résultat que le §23 qualifiait de « très fort pour la fermeture d'ENGINE-006 », et il
est cohérent avec tout le reste du dossier :

| mesure | résultat |
|---|---|
| 006F | le volume de poids n'explique pas le coût (ratio 9,65) |
| 006G | aucune sensibilité mémoire ; pas de gain ILP DP4A ; réduction minimale |
| 006H | WPR1 à 24 warps actifs/SM (moitié du parallélisme) ne perd rien |
| **006I** | **−7,7 % d'instructions → −0,77 % de wall** |

Cinq mesures convergent : **le noyau n'est pas limité par le débit d'émission d'instructions**, ni
par la mémoire, ni par le parallélisme de warp, ni par les dépendances DP4A, ni par la coordination.
Ce qui reste est le travail arithmétique lui-même — les 32 `dp4a` par thread, 3,9 M de MAC par
matmul à N=1.

Je ne promus toujours pas `ISSUE_THROUGHPUT_LIMITED=YES` ni `COMPUTE_BOUND=YES`. Mais je peux
désormais écrire, avec cinq mesures derrière :

```
DP4A_EXECUTION_THROUGHPUT_LIMIT=SUPPORTED_BY_CONVERGENT_EVIDENCE_NOT_HARDWARE_COUNTER_PROVEN
```

## 7 — Deux erreurs de ma part, signalées

**Premier jet : +161 % d'instructions.** J'avais ajouté un `#pragma unroll 2` dans la boucle de
SPEC, que BASE n'avait pas. Le SASS est passé de 2289 à 5985 lignes (déroulement du corps par 2).
C'était une comparaison invalide — je mesurais l'effet du déroulement, pas de la spécialisation.
Corrigé en retirant le pragma : la boucle est maintenant identique dans les deux variantes.

```
HARNESS_DEFECT_27=PRAGMA_UNROLL_AJOUTE_DANS_UNE_SEULE_VARIANTE
   -> comparaison invalide (SPEC +161 %) ; corrige ; aucune conclusion tiree du binaire fautif.
```

**Deuxième : la fidélité.** Ma conception initiale supposait que G1 devait être bit-à-bit
(§5). Elle ne l'est pas — et j'ai d'abord cherché une erreur d'algorithme avant de conclure à la
réassociation du compilateur. Le résultat est reporté avec ce gate en échec, pas contourné.

## 8 — Décision

Le §22 exige deux choses : `delta > noise` et reproductibilité. Les deux sont acquises. Mais :

```
G1_ISOLATION_GATE=FAIL              (bitwise non preserve)
G1_MICROBENCH_GAIN=+0,77_PERCENT
G1_PRODUCT_JUSTIFICATION=INSUFFISANTE
   -> 0,77 % de gain local, avec une fidelite bit a bit non preservee, n'atteint pas le seuil
      qu'ENGINE-006 s'est fixe pour justifier une campagne produit complete (fidelite bit a bit
      + gain produit mesure). Le precedent de 006C-R2 est explicite : un gain local de 16-18 %
      s'est traduit par 0 % au produit.
```

**Je ne lance donc pas la campagne produit du §25.** Le gain local de 0,77 % est très en dessous du
seuil où une campagne produit complète (6 runs de 256 jetons, plusieurs dizaines de minutes, plus la
validation de fidélité modèle) se justifie — d'autant que 006C-R2 a mesuré que la traduction local →
produit peut être nulle.

Ce que cela laisse :

```
ENGINE006_LOCAL_INSTRUCTION_SHAPING_HEADROOM=EXHAUSTED_BY_TESTED_AXES
```

Sept axes testés, sept résultats négatifs ou négligeables (§29). Le critère de fermeture du §32 est
réuni : un delta SASS **négatif** a été obtenu (−176), et **n'apporte pas de gain mural matériel**
(−0,77 %, avec une réserve de fidélité par-dessus).

## 9 — Provenance

```
Runtime baseline   : libllama 3db83675… / libggml-cuda abafbf0d…  (restaures, SHA verifies)
Arbre llama-obrain-cuda : les 4 fichiers du chemin chaud aux SHA vierges
Fichiers sales     : les 3 greffes connues, inchangees
MODIFICATIONS DE SOURCE DANS CETTE PASSE : AUCUNE (variantes en microbench autonome)
Instrument         : qualification/e6i/e6i-spec.cu + bin/e6i-spec
Note de methode    : la specialisation a ete prototypee en microbench autonome plutot que dans
   mmvq.cu. Le noyau y reproduit fidelement la machinerie generique (fastdiv/fastmodulo des
   canaux, adressage derive des parametres, garde generique) et le corps mathematique exact.
   Validation croisee : le k_wpr<4> de 006H, ecrit independamment, donnait 79,93 us, identique
   au k_full de 006F/G — les trois microbenchs mesurent la meme chose a 0,02 % pres.
```

## 10 — Ledger

```
ENGINE006_STATUS=OPEN_PENDING_CLOSURE_DECISION
ENGINE006H_STATUS=CLOSED_WPR_GEOMETRY_REJECTED
ENGINE006G_G1_STATUS=BUILT_AND_MEASURED
ENGINE006I_STATUS=CLOSED_NEGATIVE_SASS_DELTA_NO_MATERIAL_WALL_GAIN
WPR_NUMERICAL_ORDER_HELD_CONSTANT=NO
OCCUPANCY_OR_WARP_COORDINATION_SENSITIVITY=NOT_MATERIALLY_DETECTED_IN_WPR_MICROBENCH
WARPS_PER_ROW_GEOMETRY_HEADROOM=NOT_MATERIAL
ENGINE006I_EXACT_GEOMETRY_MANIFEST=OBTAINED
COMPILE_TIME_SAFE_PARAMETER_COUNT=12
RUNTIME_REQUIRED_PARAMETER_COUNT=3
BASE_SASS_TOTAL=2289
SPEC_SASS_TOTAL=2113
DYNAMIC_INSTRUCTION_DELTA=-176
DYNAMIC_INSTRUCTION_DELTA_PERCENT=-7.69
PARAMETER_LOAD_DELTA=-14
INTEGER_ARITHMETIC_DELTA=-37
TRANSITIVE_TO_DIRECT_PARAMETER_LOAD_RATIO=3.8
MATH_BODY_STRUCTURALLY_UNCHANGED=YES
IDP_4A_DELTA=0
WEIGHT_LDG_DELTA=0
SHFL_DELTA=0
FADD_DELTA=0
SPEC_SPILLS=0
BASE_REGISTERS=39   SPEC_REGISTERS=38   REGISTER_DELTA=-1
KERNEL_OUTPUT_MISMATCH_COUNT=1875
BITWISE_KERNEL_OUTPUT_MATCH=NO
G1_ISOLATION_GATE=FAIL
FLOAT_REASSOCIATION_BY_COMPILER=YES
BASE_FULL_US=80.3840
SPEC_FULL_US=79.7643
DELTA_US=-0.6197
SPEEDUP_RATIO=0.9923
GAIN_TO_NOISE=2.5
GENERIC_INSTRUCTION_THROUGHPUT_NOT_ON_CRITICAL_PATH=SUPPORTED
INSTRUCTION_TO_WALL_GAIN_RATIO=10_TO_1
G1_MICROBENCH_GAIN=MEASURED_BUT_ATTRIBUTION_QUALIFIED
G1_PRODUCT_JUSTIFICATION=INSUFFISANTE
PRODUCT_CAMPAIGN=NOT_RUN
DP4A_EXECUTION_THROUGHPUT_LIMIT=SUPPORTED_BY_CONVERGENT_EVIDENCE_NOT_HARDWARE_COUNTER_PROVEN
ISSUE_THROUGHPUT_LIMITED=NOT_PROVEN
COMPUTE_BOUND=NOT_PROVEN
MEMORY_BOUND=EMPIRICALLY_DISFAVORED_NOT_FORMALLY_EXCLUDED
ENGINE006_LOCAL_INSTRUCTION_SHAPING_HEADROOM=EXHAUSTED_BY_TESTED_AXES
ENGINE007_ALLOWED=PENDING_ENGINE006_CLOSURE
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
