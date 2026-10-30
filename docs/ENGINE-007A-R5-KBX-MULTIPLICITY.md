# ENGINE-007A-R5 — TEST CAUSAL DE LA MULTIPLICITÉ KBX : PRÉDICTION RÉFUTÉE, MESURE INVALIDE

Date : 2026-09-29. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7a-r5/`

---

## 0 — Résultat

Deux choses, et il faut les tenir ensemble.

**1. La prédiction pré-enregistrée est réfutée, massivement.**

```
PREDIT  (avant execution) : slope = 5,2808 us/passe   ;  P12 ~ K_FULL = 83,02 us
MESURE                    : slope = 52,2411 us/passe  ;  P12     = 630,47 us

RATIO mesuré/prédit = 9,89 x          P12 / K_FULL = 7,59 x
```

La relation mesurée est **extraordinairement propre** : `R² = 0,99999`, résidu max 1,38 µs, et les
trois pentes partielles sont cohérentes (51,98 / 51,74 / 52,57 µs par passe ajoutée). Ce n'est pas
du bruit — c'est une pente réelle de **~52 µs par passe**.

**2. Mais la mesure ne répond pas à la question posée**, et je dois le dire avant de l'interpréter.

```
ENGINE007A_R5_STRUCTURE_GATE=FAIL
KBX_DYNAMIC_MULTIPLICITY_PROVEN=YES
   (LDG.U8 statiques = 32 x PASSES exactement : 32 / 96 / 192 / 384 ; SHF 97/289/577/1153)
STRUCTURAL_EQUIVALENCE_TO_K_FULL=NO
   P-family : entierement deroulee (P12 = 21665 lignes SASS), passes CHAINEES par une dependance
              a l'accumulateur pour empecher le hissage
   K_FULL   : 2273 lignes SASS, boucle kbx avec backedge (non deroulee)
   P12/K_FULL = 9,5 x d instructions statiques ;  P12/K_FULL = 7,59 x en temps
```

**Mon mécanisme anti-hissage a introduit une dépendance sérielle entre les passes**, ce qui a changé
le graphe de calcul beaucoup plus que la variable visée. La famille P n'est pas « le noyau complet
avec N passes » : c'est un noyau différent.

---

## 1 — Freeze R4 (§1, §2)

```
ENGINE007A_R4_STATUS=CLOSED_FACTORIAL_INVALIDATED_BY_KBX_LOOP_HOISTING
K_STREAM_CANON_US=8.3344     K_DECODE_CANON_US=22.8950     K_FULL_CANON_US=83.0176
K_FULL_R3_CANON_US=82.3198   ->  SAME_KERNEL_HISTORICAL_REPRODUCIBILITY=ESTABLISHED
D00=23.8852  D10=24.8308  D01=24.7912  D11=24.9293
FACTORIAL_WORKLOAD_STRUCTURE_GATE=FAIL
ACTIVATION_LOAD_EFFECT_US=NOT_PROMOTED
OUTPUT_WRITE_EFFECT_US=NOT_PROMOTED
ACTIVATION_X_WRITE_INTERACTION_US=NOT_PROMOTED
K_FULL_US - D11_US = 58.0883
```

Les séries historique et canonique restent conservées séparément.

## 2 — Prédiction pré-enregistrée (§4, §34)

```
R5_FULL_EXPLANATION_PREDICTED_SLOPE=5.2808_us/pass   (= 58.0883 / 11)
PREDICTION_IF_FULL_RESIDUAL_IS_ALL_KBX_MULTIPLICITY:
   P1 = reference            P3 = P1 + ~10.56 us
   P6 = P1 + ~26.40 us       P12 = P1 + ~58.09 us
STATUT=REFERENCE_FALSIFIABLE_PAS_RESULTAT_ATTENDU
```

## 3 — Le gate de structure passe, et la multiplicité est prouvée (§6 à §13)

Quatre bras compilés séparément, aucun commutateur runtime. Le corps d'une passe reproduit le corps
natif : 4 tranches × 8 octets, décodage ternaire, **plus la lecture de l'échelle du bloc courant**
(`bq->d`), qui est une opération par passe réellement présente dans le natif.

| bras | passes | SASS | `LDG.E.U8` | `LDG.E.U16` (échelles) | `LOP3` | `SHF` | registres | spills |
|---|---|---|---|---|---|---|---|---|
| P1 | 1 | 1 889 | **32** | 1 | 258 | 97 | 29 | 0 |
| P3 | 3 | 5 489 | **96** | 3 | 772 | 289 | 30 | 0 |
| P6 | 6 | 10 881 | **192** | 6 | 1 543 | 577 | 36 | 0 |
| P12 | 12 | 21 665 | **384** | 12 | 3 085 | 1 153 | 36 | 0 |

```
STATIC_WEIGHT_LDG_COUNT=32_96_192_384
DYNAMIC_WEIGHT_LDG_COUNT=32_96_192_384   (boucle entierement deroulee : statique = dynamique)
DYNAMIC_KBX_MULTIPLICITY_PROVEN=YES
LOOP_BACKEDGE_PRESENT=NO (deroulee) ; LOOP_TRIP_COUNT=1_3_6_12
DYNAMIC_TERMINARY_DECODE_WORK=258_772_1543_3085_LOP3  ->  rapport 1 : 3 : 6 : 12 exact
SPILLS=0_DANS_LES_QUATRE_VARIANTS
```

Les constantes sont tenues : `Q8_LDG_COUNT=0` partout (`LDG.E.S8=0`), `STG=1` partout, sortie
5120 octets partout. Le §8, le §9 et le §16 sont respectés : seuls les chargements de poids, le
décodage et la lecture d'échelle scalent.

## 4 — Mesure canonique (§18 à §20)

5 processus séquentiels, `K=2048`, 16 cycles, rotation latine 4 bras × 2 sens, 320 observations.

| bras | moyenne | sd | min | max |
|---|---|---|---|---|
| P1 | **55,8622** | 0,4331 | 55,3212 | 57,0434 |
| P3 | **159,8290** | 1,0480 | 157,6206 | 161,5941 |
| P6 | **315,0478** | 2,6728 | 309,4580 | 318,4941 |
| P12 | **630,4719** | 5,5485 | 618,6867 | 638,1545 |

Moyennes par processus — parfaitement ordonnées et reproductibles :

| pid | P1 | P3 | P6 | P12 |
|---|---|---|---|---|
| 0 | 55,581 | 158,073 | 310,687 | 620,970 |
| 1 | 55,539 | 159,801 | 314,081 | 629,159 |
| 2 | 55,847 | 160,018 | 315,771 | 632,087 |
| 3 | 55,979 | 160,390 | 316,815 | 633,729 |
| 4 | 56,366 | 160,863 | 317,886 | 636,416 |

## 5 — Contrastes et modèle (§21, §22)

```
DELTA_1_3  = +103,9669 us (sd 0,9033)  ->  51,9834 us/passe ajoutee
DELTA_3_6  = +155,2187 us (sd 1,7504)  ->  51,7396 us/passe ajoutee
DELTA_6_12 = +315,4242 us (sd 2,9805)  ->  52,5707 us/passe ajoutee
DELTA_1_12 = +574,6098 us (sd 5,3083)

KBX_PASS_SLOPE_US_PER_PASS=52,2411
INTERCEPT_US=2,9764        R2=0,99999        MAX_RESIDUAL_US=1,3755
```

La linéarité est **quasi parfaite** et l'ordonnée à l'origine est **quasi nulle** (2,98 µs). Autrement
dit, dans cette famille, **le temps est entièrement expliqué par le nombre de passes**, avec une
constante négligeable. C'est le contraire du comportement du noyau complet, où l'on observe un
plancher fixe important.

## 6 — Confrontation à la prédiction (§23, §24, §25, §26)

| | prédit | mesuré | verdict |
|---|---|---|---|
| pente par passe | 5,2808 µs | **52,2411 µs** | **×9,89 — réfutée** |
| P12 vs K_FULL | ≈ 83 µs | **630,47 µs** | **×7,59 — réfutée** |
| P1 vs K_FULL | — | 55,86 µs = **67 %** de K_FULL | incohérent |

**Aucun des cas A/B/C/D ne s'applique** : le cas A (pente ~5,28, P12 ≈ K_FULL) est faux, le cas B
(pente plus petite) est faux en sens inverse, le cas C (plat) est faux, et le cas D (non-linéaire)
est faux aussi — la relation est au contraire remarquablement linéaire.

Le chiffre qui montre le mieux l'invalidité : **P1, avec UNE seule passe, coûte déjà 67 % du noyau
complet** qui en fait douze plus les `dp4a` et la réduction. Aucune interprétation causale n'est
possible à partir de cette famille.

## 7 — La cause de l'invalidité, identifiée

Mon anti-hissage au §10 était :

```cpp
const int kbx = (tid / 4) + 4 * p + (((acc >> 20) & 1) * (acc >> 20) * 0);
```

Cette expression **est algébriquement nulle**, mais elle **lit `acc`**, ce qui suffit à créer une
dépendance : chaque passe dépend du résultat de la précédente. Le compilateur a donc :

1. **déroulé entièrement** la boucle (12 passes × 32 chargements = 384 `LDG` statiques) ;
2. **chaîné** les passes sériellement ;
3. fait exploser le code à 21 665 lignes (9,5× le noyau complet).

Résultat : une machine sérielle de 12 étages, à ~52 µs l'étage, là où le noyau complet exécute une
boucle compacte de 2 273 instructions. Je n'ai pas mesuré la multiplicité du noyau natif ; j'ai
construit un banc différent et mesuré ce banc.

```
R5_ANTI_HOIST_MECHANISM=ADDRESS_DEPENDS_ON_ACCUMULATOR
EFFET_INDESIRABLE=DEPENDANCE_SERIELLE_ENTRE_PASSES + DEROULEMENT_COMPLET
HARNESS_DEFECT_36=ANTI_HOIST_DEPENDENCY_ALTERED_THE_COMPUTE_GRAPH_MORE_THAN_THE_VARIABLE
   STATUS=OPEN  -> la question du paragraphe 5 reste NON RESOLUE
```

## 8 — Ce que R5 établit quand même

Malgré l'invalidité pour la question posée, trois choses sont solides :

1. **Le corps d'une passe kbx coûte ~52 µs/kernel quand il est isolé et chaîné.** C'est une borne
   supérieure dure : dans le noyau complet, les 12 passes coûtent au total 60 µs (K_FULL − K_DECODE
   = 83,02 − 22,90). Le rapport entre les deux lectures est de **10,5×**.
2. **La relation passe ↔ temps est exactement linéaire** (`R²=0,99999`, ordonnée nulle) dans cette
   géométrie déroulée. Ce n'est pas une information sur le noyau natif, mais c'est une mesure propre.
3. **La prédiction pré-enregistrée était fausse d'un facteur 10**, ce qui est en soi le résultat du
   protocole : elle est réfutée, et la réfutation est nette, pas ambiguë.

## 9 — Ce que cela laisse

```
KBX_DYNAMIC_BODY_MULTIPLICITY_HYPOTHESIS=NOT_TESTED_VALIDLY
KBX_MULTIPLICITY_CAUSES_MAJOR_FIXED_FLOOR=NOT_MEASURED
MAJOR_FIXED_FLOOR_REMAINS_UNEXPLAINED=YES
HARNESS_DEFECT_35=D_FAMILY_KBX_LOOP_HOISTED_BY_COMPILER   STATUS=STILL_OPEN
HARNESS_DEFECT_36=ANTI_HOIST_DEPENDENCY_DISTORTED_GRAPH    STATUS=OPEN
```

Le §35 dit : si R5 rejette l'hypothèse, **ne pas inventer un septième ajustement** et revenir au
noyau natif pour identifier la prochaine différence structurelle entre P12 et K_FULL. C'est ce que je
fais — en notant que l'hypothèse n'est pas rejetée mais **non testée** : la distinction compte.

**Ce qui reste ouvert et serait propre à mesurer** : garder le mécanisme anti-hissage mais **sans
dépendance sérielle** — par exemple une adresse dépendant d'une valeur lue hors de la boucle (un
offset runtime passé en argument) plutôt que de l'accumulateur du tour précédent. Cela empêcherait le
hissage sans créer de chaîne. Je ne l'ai pas fait.

## 10 — Blocages maintenus (§31, §32)

```
ENGINE007B_STATUS=BLOCKED     ENGINE007C_STATUS=BLOCKED
USEFUL_DP4A_WORK_IS_NOT_A_DOMINANT_WALL_COMPONENT=SUPPORTED
DP4A_EXECUTION_THROUGHPUT_LIMIT=REJECTED_BY_ARITHMETIC_REMOVAL_EXPERIMENT
IDP_COUNT_ALONE_INSUFFICIENT=YES
ARITHMETIC_WORK_SITE_PLACEMENT_MATTERS=YES
   -> R5 ne peut pas modifier retroactivement ces contrastes apparies.
AUCUNE_SPARSITE_AUCUN_LOW_RANK_AUCUN_PRUNING
```

## 11 — Provenance et ledger

```
Runtime baseline   : libllama 3db83675… / libggml-cuda abafbf0d…  (restaures, SHA verifies)
Arbre llama-obrain-cuda : les 4 fichiers du chemin chaud aux SHA vierges
Fichiers sales     : les 3 greffes connues, inchangees
MODIFICATIONS DE SOURCE DANS CETTE PASSE : AUCUNE (microbench autonome)
Instrument         : qualification/e7a-r5/e7a-r5.cu + bin/e7a-r5
   Les quatre bras sont dans UN binaire, meme compilation, meme session.
CANONICAL_MICROBENCH_TIMING_PROTOCOL=K2048_WARMUP3_LATIN_MULTI_PROCESS  (applique)
PLUMBING_FAILURE_29=NON_ASSIGNE   (defauts 35 et 36 = defauts de harnais)
```

```
ENGINE007_STATUS=OPEN
ENGINE007A_R4_STATUS=CLOSED_FACTORIAL_INVALIDATED_BY_KBX_LOOP_HOISTING
ENGINE007A_R5_STATUS=CLOSED_MULTIPLICITY_PROVEN_MEASUREMENT_INVALID
K_STREAM_CANON_US=8.3344   K_DECODE_CANON_US=22.8950   K_FULL_CANON_US=83.0176
R5_FULL_EXPLANATION_PREDICTED_SLOPE_US_PER_PASS=5.2808
KBX_PASS_SLOPE_US_PER_PASS=52.2411
SLOPE_PREDICTION_REFUTED=YES_RATIO_9.89
R2_OF_PASS_MODEL=0.99999
INTERCEPT_US=2.9764
MAX_RESIDUAL_US=1.3755
P1_US=55.8622   P3_US=159.8290   P6_US=315.0478   P12_US=630.4719
DELTA_1_12_US=574.6098
DYNAMIC_KBX_MULTIPLICITY_PROVEN=YES
P12_OVER_K_FULL_TIME_RATIO=7.59
P12_OVER_K_FULL_STATIC_SASS_RATIO=9.5
STRUCTURAL_EQUIVALENCE_TO_K_FULL=NO
ENGINE007A_R5_STRUCTURE_GATE=FAIL_FOR_THE_INTENDED_QUESTION
KBX_DYNAMIC_BODY_MULTIPLICITY_HYPOTHESIS=NOT_TESTED_VALIDLY
KBX_MULTIPLICITY_CAUSES_MAJOR_FIXED_FLOOR=NOT_MEASURED
MAJOR_FIXED_FLOOR_REMAINS_UNEXPLAINED=YES
HARNESS_DEFECT_35=OPEN_D_FAMILY_HOISTED
HARNESS_DEFECT_36=OPEN_ANTI_HOIST_DEPENDENCY_DISTORTED_GRAPH
SAME_KERNEL_HISTORICAL_REPRODUCIBILITY=ESTABLISHED
CROSS_KERNEL_ABSOLUTE_TIMING_COMPARABILITY=REQUIRES_EXPLICIT_WORKLOAD_EQUIVALENCE
ENGINE007_PRIMARY_LEVER=NOT_IDENTIFIED
ENGINE007B_STATUS=BLOCKED
ENGINE007C_STATUS=BLOCKED
NEXT_STEP_IF_PURSUED=ANTI_HOIST_SANS_DEPENDANCE_SERIELLE_OFFSET_RUNTIME_HORS_BOUCLE
COMPUTE_BOUND=NOT_PROVEN
ISSUE_THROUGHPUT_LIMITED=NOT_PROVEN
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
