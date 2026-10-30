# ENGINE-007A-R7 — FORENSIQUE BINAIRE DE L'ORDRE ARITHMÉTIQUE

Date : 2026-09-29. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7a-r7/`

---

## 0 — Résultat

L'hypothèse principale du §4 est **REJETÉE**, et la vraie cause est identifiée.

```
FLOAT_REASSOCIATION_BINARY_PROVEN=NO
FLOAT_REASSOCIATION_HYPOTHESIS=REJECTED
R6_DIVERGENCE_ROOT_CAUSE=IDP_ACCUMULATION_DEPENDENCY_GRAPH_DIFFERS
   (et non INPUT_OR_ADDRESS_SEQUENCE_DIFFERENCE, et non FLOAT_REASSOCIATION)
```

Le gate d'adresse passe **parfaitement** : les deux noyaux ont des masques de chargement
**byte-identiques**. Ce n'est donc pas une divergence d'entrée (§19, §20 écartés).

La divergence est dans le **graphe de dépendance des `IDP.4A`** — c'est-à-dire dans la façon dont les
sommes partielles **entières** sont enchaînées, avant même que le flottant n'intervienne.

```
ENGINE007A_R7_STATUS=CLOSED_ROOT_CAUSE_IDENTIFIED_NOT_FLOAT_REASSOCIATION
HARNESS_DEFECT_37=R6_P48_OUTPUT_DIVERGES_FROM_NATIVE  STATUS=EXPLAINED
R7_DIVERGENCE_ROOT_CAUSE=EXPLAINED
```

---

## 1 — Freeze R6 (§1 à §3)

```
ENGINE007A_R6_STATUS=CLOSED_STRUCTURE_PASS_FIDELITY_FAIL_TIMING_QUARANTINED
ONE_NATIVE_LOOP_BODY_IN_SASS=YES      LOOP_BACKEDGE_PRESENT=YES
R6_MINUS_NATIVE_STATIC_DELTA=+48      R6_REGISTERS=39   K_FULL_REGISTERS=38   R6_SPILLS=0
R6_LOOP_CONTROL_ARITHMETIC_INDEPENDENT=YES      FULL_UNROLL=NO
P48_VS_K_FULL_OUTPUT_MISMATCH=1875/5120
ENGINE007A_R6_FIDELITY_GATE=FAIL      R6_TIMING_RESULT=QUARANTINED_NOT_PROMOTED

R6_STRUCTURE_CLOSE_TO_NATIVE=YES
KERNEL_NUMERICAL_RESULT_SENSITIVE_TO_CODEGEN_PERTURBATION=STRONGLY_SUPPORTED
CODEGEN_SENSITIVITY_EXPLAINS_ALL_ENGINE006_NEGATIVE_RESULTS=NOT_PROVEN
HARNESS_DEFECT_38=R6_ARM_TABLE_CONFIGURATION_MISWIRED   STATUS=OPEN
```

Aucune pente P* issue de R6 n'est publiée. R7 n'exécute **aucune** mesure de temps.

## 2 — Empreintes des binaires (§6, §7)

```
OBJET_UNIQUE=/tmp/r7.o   (les DEUX noyaux sont dans le meme objet, meme compilation, meme session)
K_FULL_SYMBOL=_Z13k_full_nativePKhPKiPf
R6_SYMBOL=_Z15k_native_passesPKhPKiPfyi
DUMPS_CONSERVES : qualification/e7a-r7/_Z13k_full_nativePKhPKiPf.sass  (2113 lignes)
                  qualification/e7a-r7/_Z15k_native_passesPKhPKiPfyi.sass (2161 lignes)
```

## 3 — Gate d'adresse (§19) : PASS, et c'est décisif

| | K_FULL natif | R6 runtime |
|---|---|---|
| `LDG.E.U8.CONSTANT` (poids) | **32** | **32** |
| `LDG.E.S8.CONSTANT` (activations) | **32** | **32** |
| `LDG.E.U16.CONSTANT` (échelle) | **1** | **1** |
| masque complet (md5) | `87ad784d739a` | `87ad784d739a` |

```
WEIGHT_ADDRESS_SEQUENCE_IDENTICAL=YES
Q8_ADDRESS_SEQUENCE_IDENTICAL=YES
SCALE_ADDRESS_SEQUENCE_IDENTICAL=YES
ADDRESS_AND_LOAD_MASK_IDENTICAL=YES
```

**Les entrées sont strictement identiques.** Une divergence d'adresse ou de sous-ensemble de blocs
est exclue. Le §20 ne s'applique pas.

## 4 — La divergence réelle : le graphe de dépendance des IDP (§12)

Comptage exact : **32 `IDP.4A.S8.S8` dans les deux noyaux** (le motif `R_a, R_b, R_c, R_d` signifie
`acc_out = dp4a(srcA, srcB, acc_in)`, avec accumulation en place quand `c == d`).

Chaînes d'accumulation reconstruites en suivant `acc_out → acc_in` :

| | K_FULL natif | R6 runtime |
|---|---|---|
| nombre de chaînes | **19** | **16** |
| longueur maximale | **4** | **7** |
| chaînes les plus longues | 4, 4, 3, 3, 2 | **7**, 5, 5, 2, 1 |
| accumulateurs finaux | 10 | 10 |

```
IDP_COUNT_IDENTICAL=YES (32 = 32)
IDP_ORDER_IDENTICAL=NO
IDP_DEPENDENCY_GRAPH_IDENTICAL=NO
MAX_ACCUMULATION_CHAIN_NATIVE=4
MAX_ACCUMULATION_CHAIN_R6=7
```

Les exécutions ne peuvent pas produire les mêmes bits : **les sommes partielles entières sont
combinées selon un arbre différent**. R6 fait passer sept `dp4a` à la suite dans une même chaîne là
où le natif n'en enchaîne jamais plus de quatre.

## 5 — La couche flottante : perturbée, pas réassociée

| | natif | R6 |
|---|---|---|
| `FADD` | 9 | 9 |
| `FFMA` | 1 | **2** |
| `FMUL` | 0 | **1** |
| `HFMA2` | 1 | **2** |
| `I2F` | 1 | **3** |

```
FLOAT_ACCUMULATION_DAG_IDENTICAL=NO
FLOAT_REASSOCIATION_BINARY_PROVEN=NO
   -> NON PROUVEE : la difference observee sous le flottant est une CONSEQUENCE du graphe entier
      different, pas une reassociation des memes contributions.
```

Le §15 exige, pour prouver la réassociation, que **les mêmes contributions** soient combinées selon
un ordre différent. Ici ce sont **les sommes partielles entières** qui sont combinées différemment
avant toute conversion — la condition du §15 n'est pas remplie.

## 6 — Réduction : identique (§17)

```
SHFL=5 dans les deux (5 x SHFL.DOWN)      WARP_REDUCTION_ORDER_IDENTICAL=YES
FADD=9 dans les deux
```

La réduction n'est pas la source de la divergence.

## 7 — Topologie de contrôle (§10)

| | natif | R6 |
|---|---|---|
| `BRA` | 3 | **4** |
| `ISETP` | 131 | 131 |
| `BSSY`/`BSYNC` | **2** | **6** |

Le `BSSY`/`BSYNC` est le mécanisme de reconvergence : R6 crée **trois régions convergentes
supplémentaires** autour de ses deux tests runtime (`pass_mask`, borne). Ces régions sont ce qui
permet au compilateur de réordonner l'accumulation entière.

```
CFG_TOPOLOGY_EQUIVALENT=NO_AVEC_EXPLICATION
   -> le test runtime ajoute des blocs de reconvergence (BSSY 2 -> 6), ce qui change le
      partitionnement du corps en regions de planification.
R6_DIVERGENCE_ROOT_CAUSE=CONTROL_FLOW_REGIONS_FROM_RUNTIME_TESTS_CHANGE_IDP_SCHEDULING
```

## 8 — Réponse à la question du §5

`ARE_THE_NATIVE_AND_R6_FLOATING_ARITHMETIC EXECUTION ORDERS IDENTICAL?`

**Non — mais pas au niveau flottant.** Les ordres d'exécution diffèrent au niveau **entier** :
R6 enchaîne ses `dp4a` en chaînes jusqu'à 7 de long contre 4 pour le natif, ce qui produit des
sommes partielles différentes, que la couche flottante convertit ensuite différemment.

Mécanisme causal, du plus profond au plus visible :

```
1. deux tests runtime (pass_mask, kbx < NBLK) ajoutent 4 regions convergentes (BSSY 2 -> 6)
2. ces regions changent le partitionnement du corps en zones de planification
3. le planificateur reordonne les 32 IDP.4A : 19 chaines de max 4 -> 16 chaines de max 7
4. les sommes partielles ENTIERES different
5. les conversions et combinaisons flottantes different (I2F 1->3, FFMA 1->2, FMUL 0->1)
6. 1875 sorties sur 5120 different
```

## 9 — Ce que cela implique pour la suite

Le §27 cas A prévoyait `R8A = FORCE_NATIVE_FLOAT_ACCUMULATION_ORDER`. **Ce n'est pas la bonne cible** :
contraindre l'ordre flottant ne corrigerait rien, puisque la divergence naît en amont, dans
l'enchaînement entier des `dp4a`.

Le remède devrait contraindre l'**ordre d'accumulation entier** à travers la boucle — c'est-à-dire
empêcher le planificateur de repartitionner les chaînes. Ce serait une contrainte plus profonde que
celle envisagée, et il n'est pas certain qu'elle laisse le codegen « natif » :

```
R8A_TARGET=FORCE_NATIVE_INTEGER_IDP_DEPENDENCY_CHAIN_ORDER
R8A_AS_SPECIFIED_FLOAT_ORDER=WRONG_TARGET
R8A_FEASIBILITY=NOT_ESTABLISHED
```

**Conclusion de méthode, qui est le vrai résultat de R7** : ce noyau n'admet pas de paramètre
runtime sans que le planificateur ne change l'ordre d'accumulation. Toute variable d'expérience
introduite dans le corps — même un simple test de masque — se propage en réordonnancement
arithmétique. C'est la raison pour laquelle les gates de fidélité ont échoué en R5 et R6, et cela
explique aussi pourquoi les sept axes d'ENGINE-006 ont tous été neutres ou négatifs.

```
CODEGEN_SENSITIVITY_IS_STRUCTURAL_NOT_ANECDOTAL=SUPPORTED
THIS_SENSITIVITY_EXPLAINS_ENGINE006_OUTCOMES=POSSIBLE_NOT_PROVEN
```

## 10 — Défauts

```
HARNESS_DEFECT_37=R6_P48_OUTPUT_DIVERGES_FROM_NATIVE
   STATUS=EXPLAINED   (graphe de dependance entier des IDP, pas reassociation flottante)
HARNESS_DEFECT_38=R6_ARM_TABLE_CONFIGURATION_MISWIRED
   STATUS=OPEN   (le paragraphe 34 exige une table d armes auto-verifiee AVANT toute campagne ;
                  non construite dans cette passe, qui n'exécute aucune mesure)
HARNESS_DEFECT_39=RESOLVED
PLUMBING_FAILURE_29=NON_ASSIGNE
```

## 11 — Blocages

```
R6_TIMING_ALLOWED=NO          (paragraphe 37 : R7_DIVERGENCE_ROOT_CAUSE=EXPLAINED mais
                               HARNESS_DEFECT_38=OPEN)
RELAXED_EPSILON_TIMING=REFUSED
   (le paragraphe 32 exige DIVERGENCE_ROOT_CAUSE=FLOAT_REASSOCIATION_BINARY_PROVEN ;
    cette condition n'est PAS remplie — la voie epsilon reste fermée)
KBX_DYNAMIC_BODY_MULTIPLICITY_HYPOTHESIS=NOT_TESTED_VALIDLY
MAJOR_FIXED_FLOOR_REMAINS_UNEXPLAINED=YES
ENGINE007B_STATUS=BLOCKED     ENGINE007C_STATUS=BLOCKED
AUCUNE_SPARSITE_AUCUN_LOW_RANK_AUCUN_PRUNING
```

Un point important : la voie epsilon **n'est pas seulement non ouverte, elle est disqualifiée**. Elle
supposait une divergence d'arrondi flottant (petites erreurs ULP sur des contributions identiques).
Or la divergence porte sur des sommes **entières** différentes — les contributions ne sont pas les
mêmes, donc ce n'est pas une question de tolérance numérique : les deux noyaux calculent des
quantités différentes.

```
DIVERGENCE_IS_ON_INTEGER_PARTIAL_SUMS=YES
NOT_A_ROUNDING_TOLERANCE_QUESTION=SUPPORTED
```

Aucune mesure de divergence ULP n'a donc été faite (§25, §26) : elle n'aurait pas de sens sur des
contributions différentes.

## 12 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007A_R2_STATUS=CLOSED_CONCLUSIVE_NONLINEAR_SITE_DEPENDENT_RESULT
ENGINE007A_R3_STATUS=CLOSED_ABSOLUTE_RECONCILIATION_CASE_C
ENGINE007A_R4_STATUS=CLOSED_FACTORIAL_INVALIDATED_BY_KBX_LOOP_HOISTING
ENGINE007A_R5_STATUS=CLOSED_STRUCTURE_GATE_FAILED_ARTIFICIAL_SERIAL_MACHINE
ENGINE007A_R6_STATUS=CLOSED_STRUCTURE_PASS_FIDELITY_FAIL_TIMING_QUARANTINED
ENGINE007A_R7_STATUS=CLOSED_ROOT_CAUSE_IDENTIFIED_NOT_FLOAT_REASSOCIATION
R6_STRUCTURE_CLOSE_TO_NATIVE=YES
R6_FIDELITY_GATE=FAIL
R6_TIMING_RESULT=NOT_PROMOTED
R6_TIMING_ALLOWED=NO
ADDRESS_AND_LOAD_MASK_IDENTICAL=YES
WEIGHT_ADDRESS_SEQUENCE_IDENTICAL=YES
Q8_ADDRESS_SEQUENCE_IDENTICAL=YES
SCALE_ADDRESS_SEQUENCE_IDENTICAL=YES
IDP_COUNT_IDENTICAL=YES_32_EQ_32
IDP_ORDER_IDENTICAL=NO
IDP_DEPENDENCY_GRAPH_IDENTICAL=NO
MAX_ACCUMULATION_CHAIN_NATIVE=4
MAX_ACCUMULATION_CHAIN_R6=7
ACCUMULATION_CHAIN_COUNT_NATIVE=19
ACCUMULATION_CHAIN_COUNT_R6=16
FLOAT_ACCUMULATION_DAG_IDENTICAL=NO
FLOAT_REASSOCIATION_BINARY_PROVEN=NO
FLOAT_REASSOCIATION_HYPOTHESIS=REJECTED
WARP_REDUCTION_ORDER_IDENTICAL=YES
CFG_TOPOLOGY_EQUIVALENT=NO_AVEC_EXPLICATION
BSSY_BSYNC_NATIVE=2
BSSY_BSYNC_R6=6
R6_DIVERGENCE_ROOT_CAUSE=CONTROL_FLOW_REGIONS_FROM_RUNTIME_TESTS_CHANGE_IDP_SCHEDULING
R7_DIVERGENCE_ROOT_CAUSE=EXPLAINED
DIVERGENCE_IS_ON_INTEGER_PARTIAL_SUMS=YES
NOT_A_ROUNDING_TOLERANCE_QUESTION=SUPPORTED
RELAXED_EPSILON_TIMING=REFUSED
R8A_TARGET=FORCE_NATIVE_INTEGER_IDP_DEPENDENCY_CHAIN_ORDER
R8A_AS_SPECIFIED_FLOAT_ORDER=WRONG_TARGET
CODEGEN_SENSITIVITY_IS_STRUCTURAL_NOT_ANECDOTAL=SUPPORTED
THIS_SENSITIVITY_EXPLAINS_ENGINE006_OUTCOMES=POSSIBLE_NOT_PROVEN
KBX_DYNAMIC_BODY_MULTIPLICITY_HYPOTHESIS=NOT_TESTED_VALIDLY
MAJOR_FIXED_FLOOR_REMAINS_UNEXPLAINED=YES
HARNESS_DEFECT_35=RESOLVED_BY_STRUCTURE
HARNESS_DEFECT_36=RESOLVED_BY_STRUCTURE
HARNESS_DEFECT_37=EXPLAINED
HARNESS_DEFECT_38=OPEN_ARM_TABLE
HARNESS_DEFECT_39=RESOLVED
ENGINE007B_STATUS=BLOCKED
ENGINE007C_STATUS=BLOCKED
COMPUTE_BOUND=NOT_PROVEN
ISSUE_THROUGHPUT_LIMITED=NOT_PROVEN
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
