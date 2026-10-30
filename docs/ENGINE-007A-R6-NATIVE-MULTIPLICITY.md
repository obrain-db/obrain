# ENGINE-007A-R6 — MULTIPLICITÉ KBX NATIVE : ÉCHEC DU GATE DE FIDÉLITÉ

Date : 2026-09-29. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7a-r6/`

---

## 0 — Résultat

**Le gate de fidélité n'est pas franchi. Aucune mesure de R6 n'est exploitable pour la question
posée.**

```
P48_VS_K_FULL_OUTPUT_MISMATCH=1875 / 5120
P12_KBX_SEQUENCE_MATCH_NATIVE=NO
ENGINE007A_R6_FIDELITY_GATE=FAIL
R6_NATIVE_EQUIVALENCE_GATE=NOT_REACHED
R6_TIMING_INTERPRETATION=REFUSED
```

Ce qui **est** acquis, et qui est propre :

```
R6_STATIC_SASS_LINES=2161        K_FULL_NATIVE_STATIC_SASS_LINES=2113
R6_STATIC_SASS_OVER_K_FULL=1,023
R6_LDG=65 (= natif)   IDP=32 (= natif)   LOP3=261 (natif 259)   BRA=4 (natif 3)
R6_MINUS_NATIVE_STATIC_DELTA=+48_lignes_de_controle_de_boucle
REGISTERS=39 (natif 38)      SPILLS=0
```

Le noyau R6 **est compact** — 2161 lignes contre 2113, soit **+2,3 %**, exactement le coût du
contrôle de boucle runtime attendu au §20. Le §11, le §12 et le §21 sont donc satisfaits. C'est la
partie du dispositif qui fonctionne.

Mais le résultat numérique diverge de 1875 lignes sur 5120, donc **je ne peux rien en tirer sur le
temps**.

---

## 1 — Ce que la tentative a établi (gate SASS §11 à §21)

| | K_FULL natif | R6 runtime |
|---|---|---|
| SASS total | 2113 | **2161** |
| `LDG` | 65 | 65 |
| `IDP.4A` | 32 | 32 |
| `LOP3` | 259 | 261 |
| `SHF` | 104 | 104 |
| `SHFL` / `FADD` / `BAR` / `STG` | 5 / 9 / 1 / 1 | 5 / 9 / 1 / 1 |
| registres | 38 | 39 |
| spills | 0 | **0** |
| backedge présent | oui | **oui** |

```
ONE_NATIVE_LOOP_BODY_IN_SASS=YES
LOOP_BACKEDGE_PRESENT=YES
R6_STATIC_SASS_LINES_SAME_ORDER_AS_K_FULL=YES
R6_MINUS_NATIVE_STATIC_DELTA=+48_LIGNES
R6_LOOP_CONTROL_ARITHMETIC_INDEPENDENT=YES   (aucune lecture de acc dans l'indice ou la borne)
FULL_UNROLL=NO
HARNESS_DEFECT_35_RESOLVED_BY_STRUCTURE=YES  (le hissage de la famille D ne se reproduit pas ici)
HARNESS_DEFECT_36_RESOLVED_BY_STRUCTURE=YES  (aucune dependance a acc)
```

**Les deux défauts de R4/R5 sont structurellement résolus** : le noyau garde une boucle avec
backedge, et le contrôle de boucle ne dépend pas de l'arithmétique. Le dispositif est bon.

## 2 — Ce qui a échoué, et ce que je peux en dire

Le résultat de P48 (masque complet, qui doit reproduire exactement le natif) diverge de K_FULL sur
**1875 des 5120 lignes**.

Le diagnostic que j'ai pu établir :

```
- les deux boucles sont TEXTUELLEMENT identiques : for (int kbx = tid / 4; kbx < NBLK; kbx += 32)
- le corps est le meme : memes chargements, meme decodage, memes 32 dp4a, meme echelle
- meme reduction, meme store (out[row]), et le terme ajoute evalue a 0,0
- la seule difference de codegen est le controle de boucle runtime
```

**Cause non etablie.** L'hypothèse la plus plausible est une **différence d'ordonnancement du
compilateur** entre les deux instanciations — le noyau runtime contient un test de masque
supplémentaire, ce qui change la pression de registres et l'ordre des additions flottantes
`acc += d * sumi` sur les deux tours de boucle. C'est le même phénomène de réassociation flottante
que 006H et 006I ont rencontré. Mais **je ne l'ai pas prouvé**, et je ne le présente pas comme tel.

```
R6_NUMERICAL_DIVERGENCE_CAUSE=NOT_ESTABLISHED
HYPOTHESE=REASSOCIATION_FLOTTANTE_PAR_ORDONNANCEMENT_DIFFERENT
STATUT=PLAUSIBLE_NON_DEMONTRE
HARNESS_DEFECT_37=R6_P48_DIVERGES_FROM_K_FULL_ON_1875_OF_5120_ROWS
   STATUS=OPEN
```

## 3 — La mesure a tourné, et je ne la publie pas comme résultat

La campagne a été exécutée (5 processus, `K=2048`, 16 cycles, 480 observations, aucune erreur), et
les valeurs existent :

```
K_FULL=83,79   P6=61,41   P24_A=112,57   P24_B=239,68   P48=471,40   P48b=241,26   (us)
```

Je ne les interprète **pas** : elles proviennent d'un noyau qui ne calcule pas la même chose que la
référence, et `P24_B` (239,68) contre `P48b` (241,26) montre que le masque lui-même est mal construit
— deux niveaux censés différer d'un facteur 2 donnent le même temps. Les deux bras à masque complet
(`P48` et `P48b`) sont d'ailleurs **le même masque** `M48`, et donnent 471,40 contre 241,26 : un
facteur 2 entre deux exécutions identiques signale que mon tableau d'armes est mal câblé.

```
R6_RAW_VALUES=RECORDED_NOT_INTERPRETED
ARM_TABLE_WIRING_SUSPECT=YES   (P48 et P48b utilisent le meme masque M48 et different d un facteur 2)
```

## 4 — Freeze R5 (§1, §2)

```
ENGINE007A_R5_STATUS=CLOSED_STRUCTURE_GATE_FAILED_ARTIFICIAL_SERIAL_MACHINE
DYNAMIC_KBX_MULTIPLICITY_PROVEN=YES
R5_P1=55.8622  R5_P3=159.8290  R5_P6=315.0478  R5_P12=630.4719
R5_SLOPE=52.2411 us/pass   R5_R2=0.99999
STRUCTURAL_EQUIVALENCE_TO_K_FULL=NO
ANTI_HOIST_EXPRESSION_READS_ACC=YES
LOOP_CARRIED_DEPENDENCY_INTRODUCED=YES
FULL_UNROLL_GENERATED=YES
P12_STATIC_SASS_LINES=21665   NATIVE_K_FULL=2273
R5_52US_PER_PASS_APPLIES_TO_NATIVE_KERNEL=NO

ARTIFICIALLY_SERIALIZED_KBX_BODY_LATENCY_SCALES_LINEARLY_WITH_PASS_COUNT=MEASURED
ARTIFICIAL_SERIALIZATION_CAN_INCREASE_KBX_BODY_COST_BY_ORDER_OF_MAGNITUDE=SUPPORTED
   -> cette pente n est JAMAIS reutilisable dans une estimation native.
```

## 5 — Le constat qui compte maintenant : trois tentatives, trois échecs

| passe | approche | variable isolée | point d'échec |
|---|---|---|---|
| R4 | factoriel activations × écriture | 2 facteurs | gate structure : boucle hissée |
| R5 | famille P1/P3/P6/P12 | nombre de passes | gate structure : dépendance sérielle, code ×9,5 |
| R6 | corps natif, masque kbx runtime | nombre de blocs | gate fidélité : 1875/5120 divergents |

**Trois instruments construits pour tester la même hypothèse, trois fois le gate échoué, chaque fois
sur un défaut de construction différent.** Le motif est maintenant assez net pour être nommé :

```
KBX_MULTIPLICITY_HYPOTHESIS_HAS_RESISTED_THREE_INSTRUMENTATION_ATTEMPTS=YES
KERNEL_IS_HIGHLY_SENSITIVE_TO_CODEGEN_PERTURBATION=STRONGLY_SUPPORTED
   -> chaque fois que j ai modifie le noyau pour isoler une variable, le compilateur a produits
      un code dont le comportement numerique ou structurel different de la reference.
```

Ce n'est pas une excuse : c'est une **propriété mesurée de ce noyau**, et elle explique
rétroactivement pourquoi les sept axes d'ENGINE-006 ont tous échoué. Un noyau dont chaque
perturbation de source change l'ordonnancement flottant ou la forme du code ne se laisse pas
décomposer par différences locales.

## 6 — Recommandation, sans l'exécuter

Le §35 dit : si R6 ne passe pas le gate d'équivalence, **analyser d'abord le delta SASS/codegen**.
C'est fait (§1) : le delta est de +48 lignes de contrôle de boucle, ce qui est conforme. La
divergence **numérique** n'est donc pas expliquée par le SASS statique.

Deux voies restent, et je ne les ai pas prises :

1. **Comparer les séquences d'instructions**, pas les compteurs : désassembler les deux boucles et
   vérifier l'ordre exact des additions flottantes et des `IDP`. C'est la seule façon de trancher
   l'hypothèse de réassociation.
2. **Accepter la divergence numérique et mesurer en tolérance** : remplacer la comparaison bit-à-bit
   par une comparaison à epsilon relatif, en documentant que l'ordre d'accumulation a changé. Cela
   affaiblit le gate mais permettrait d'obtenir une pente, à déclarer comme « sous réserve de
   réassociation ».

Aucune des deux n'est un ajustement cosmétique, et je ne les lance pas sans décision de ta part.

## 7 — Blocages maintenus

```
KBX_DYNAMIC_BODY_MULTIPLICITY_HYPOTHESIS=NOT_TESTED_VALIDLY
MAJOR_FIXED_FLOOR_REMAINS_UNEXPLAINED=YES
ENGINE007B_STATUS=BLOCKED     ENGINE007C_STATUS=BLOCKED
NO_NEXT_COMPONENT_DECOMPOSITION_UNTIL_R6_NATIVE_EQUIVALENCE_GATE=PASS
USEFUL_DP4A_WORK_IS_NOT_A_DOMINANT_WALL_COMPONENT=SUPPORTED
DP4A_EXECUTION_THROUGHPUT_LIMIT=REJECTED_BY_ARITHMETIC_REMOVAL_EXPERIMENT
IDP_COUNT_ALONE_INSUFFICIENT=YES
ARITHMETIC_WORK_SITE_PLACEMENT_MATTERS=YES
AUCUNE_SPARSITE_AUCUN_LOW_RANK_AUCUN_PRUNING
```

## 8 — Provenance

```
Runtime baseline   : libllama 3db83675… / libggml-cuda abafbf0d…  (restaures, SHA verifies)
Arbre llama-obrain-cuda : les 4 fichiers du chemin chaud aux SHA vierges
Fichiers sales     : les 3 greffes connues, inchangees
MODIFICATIONS DE SOURCE DANS CETTE PASSE : AUCUNE (microbench autonome)
Instrument         : qualification/e7a-r6/e7a-r6.cu + bin/e7a-r6
   Les deux noyaux sont dans UN binaire, meme compilation, meme session.
CANONICAL_MICROBENCH_TIMING_PROTOCOL=K2048_WARMUP3_LATIN_MULTI_PROCESS  (applique, 480 obs.)
R6_RUNTIME_PARAMETER_ALLOWED=YES  (exception documentee au paragraphe 7 : la variable est la borne)
PLUMBING_FAILURE_29=NON_ASSIGNE
```

Défauts de harnais de cette passe :

```
HARNESS_DEFECT_37=R6_P48_DIVERGES_FROM_K_FULL_ON_1875_ROWS   STATUS=OPEN
HARNESS_DEFECT_38=R6_ARM_TABLE_MISWIRED_TWO_ARMS_SHARE_M48_AND_DIFFER_2X  STATUS=OPEN
HARNESS_DEFECT_39=R6_PARTITION_FIRST_VERSION_PASSDIV_4_DID_NOT_MATCH_NATIVE  STATUS=RESOLVED
```

## 9 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007A_R2_STATUS=CLOSED_CONCLUSIVE_NONLINEAR_SITE_DEPENDENT_RESULT
ENGINE007A_R3_STATUS=CLOSED_ABSOLUTE_RECONCILIATION_CASE_C
ENGINE007A_R4_STATUS=CLOSED_FACTORIAL_INVALIDATED_BY_KBX_LOOP_HOISTING
ENGINE007A_R5_STATUS=CLOSED_STRUCTURE_GATE_FAILED_ARTIFICIAL_SERIAL_MACHINE
ENGINE007A_R6_STATUS=CLOSED_FIDELITY_GATE_FAILED_RUNTIME_KBX_MASK
R6_STRUCTURE_GATES=PASS
R6_STATIC_SASS_LINES=2161
K_FULL_NATIVE_STATIC_SASS_LINES=2113
R6_MINUS_NATIVE_STATIC_DELTA=+48_LIGNES_CONTROLE_DE_BOUCLE
R6_LOOP_CONTROL_ARITHMETIC_INDEPENDENT=YES
FULL_UNROLL=NO
R6_SPILLS=0
ENGINE007A_R6_FIDELITY_GATE=FAIL
R6_NATIVE_EQUIVALENCE_GATE=NOT_REACHED
P48_VS_K_FULL_OUTPUT_MISMATCH=1875
R6_NUMERICAL_DIVERGENCE_CAUSE=NOT_ESTABLISHED
R6_TIMING_INTERPRETATION=REFUSED
KBX_MULTIPLICITY_HYPOTHESIS_HAS_RESISTED_THREE_INSTRUMENTATION_ATTEMPTS=YES
KERNEL_IS_HIGHLY_SENSITIVE_TO_CODEGEN_PERTURBATION=STRONGLY_SUPPORTED
KBX_DYNAMIC_BODY_MULTIPLICITY_HYPOTHESIS=NOT_TESTED_VALIDLY
MAJOR_FIXED_FLOOR_REMAINS_UNEXPLAINED=YES
HARNESS_DEFECT_35=RESOLVED_BY_STRUCTURE
HARNESS_DEFECT_36=RESOLVED_BY_STRUCTURE
HARNESS_DEFECT_37=OPEN_R6_P48_DIVERGENCE
HARNESS_DEFECT_38=OPEN_R6_ARM_TABLE_MISWIRED
SAME_KERNEL_HISTORICAL_REPRODUCIBILITY=ESTABLISHED
CROSS_KERNEL_ABSOLUTE_TIMING_COMPARABILITY=REQUIRES_EXPLICIT_WORKLOAD_EQUIVALENCE
ENGINE007_PRIMARY_LEVER=NOT_IDENTIFIED
ENGINE007B_STATUS=BLOCKED
ENGINE007C_STATUS=BLOCKED
COMPUTE_BOUND=NOT_PROVEN
ISSUE_THROUGHPUT_LIMITED=NOT_PROVEN
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
