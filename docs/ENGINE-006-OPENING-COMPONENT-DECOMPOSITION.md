# ENGINE-006 — OUVERTURE + DÉCOMPOSITION CAUSALE DU CHEMIN tq2_g128

Date : 2026-09-29. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e6a/`

---

## 0 — Les deux réponses de la passe

**1. Le gate de scope (§2) : tranché, et il y avait bien un artefact de compteur.**

Historiquement, le compteur `E6_NON_TARGET_ROLE_EXTRA_EXECUTIONS` valait 0 en R0 et **168** en
R1/R2. Lu littéralement, c'était le **cas B** (`R0_NON_TARGET_EXTRA=0, R1=168, R2=168`). La cause
est un défaut d'instrumentation : le compteur était incrémenté **deux fois par appel non-cible**
(une fois dans chacune des deux branches de la variante), donc 168 = 2 × 84 appels non-cibles, et il
ne distinguait pas « appel natif » de « travail supplémentaire ».

Compteur corrigé (natif compté **inconditionnellement**, extra séparé) :

```
                        BASE     Q+       M+       QM+
NON_TARGET_NATIVE       770      770      770      770      (constant)
NON_TARGET_EXTRA        0       1540     1540     1540      (= 2 x 770 marquages, aucun travail)
TARGET_NATIVE            72       72       72       72      (constant)
TARGET_EXTRA              0       72       72      144
```

Les 770 appels non-cibles **natifs** sont constants sur toutes les variantes : le forçage ne les
modifie pas. Aucun travail supplémentaire non-cible n'est exécuté. La seule lecture qui reste est
celle d'un **compteur de marquages** sur la branche non-cible — un défaut d'instrument, pas un
défaut de scope.

```
TARGET_SCOPE_GATE=PASS
NON_TARGET_EXTRA_MMVQ_EXECUTIONS=0
HARNESS_DEFECT_11=NON_TARGET_COUNTER_INCREMENTED_TWICE_PER_CALL_AND_CONFLATED_NATIVE_WITH_EXTRA
```

Conséquence pour 005I : la pente `1,2591 ms/repeat` avait été estimée avec un compteur de scope
imparfait. Elle est **remplacée** par la mesure décomposée ci-dessous, qui est plus fine et dont le
scope est propre.

**2. La décomposition par composant (§5-§13) : faite, et elle est sans ambiguïté.**

```
QUANTIZE_DUPLICATE_WALL_SENSITIVITY   = +0,0144 ms   (1,2 % de la pente)
MMVQ_KERNEL_DUPLICATE_WALL_SENSITIVITY = +1,1794 ms  (99,0 %)
FULL_PATH_DUPLICATE_WALL_SENSITIVITY   = +1,1917 ms
INTERACTION                            = -0,0022 ms (-0,2 %)
→ ENGINE006_PRIMARY_AXIS=MMVQ_KERNEL
```

---

## 1 — Preflight bitwise des quatre variantes (§8) : PASS

16 tokens, graphes actifs, mêmes modèle / préfixe / positions :

```
BASE    efa529aff901d4b934e8b1e29942ca305ea88f089a98d187018184874b0debf0
Q+      efa529aff901d4b934e8b1e29942ca305ea88f089a98d187018184874b0debf0
M+      efa529aff901d4b934e8b1e29942ca305ea88f089a98d187018184874b0debf0
QM+     efa529aff901d4b934e8b1e29942ca305ea88f089a98d187018184874b0debf0
ALL_BITWISE_EQUAL=YES
```

Les quatre composants sont donc **mathématiquement neutres**, chacun pour la raison que la source
annonçait (§6, §7) :

```
EXTRA_QUANTIZE_FUNCTIONALLY_NEUTRAL=SOURCE_PROVEN   (la quantize native reecrit tout le buffer)
EXTRA_MMVQ_FUNCTIONALLY_NEUTRAL=SOURCE_PROVEN       (dst en ecriture pure, meme q8_1, meme dst)
```

## 2 — Campagne composantielle (§9, §10)

Ordre pré-enregistré `BASE Q+ M+ QM+ QM+ M+ Q+ BASE`, 256 tokens, graphes actifs, un processus par
run.

| run | variante | ms/token | TPS | extra cible |
|---|---|---|---|---|
| 1 | BASE | 63,7334 | 15,6904 | 0 |
| 2 | Q+ | 63,8193 | 15,6692 | 144 |
| 3 | M+ | 65,0410 | 15,3749 | 144 |
| 4 | QM+ | 65,1206 | 15,3561 | 288 |
| 5 | QM+ | 65,1061 | 15,3595 | 288 |
| 6 | M+ | 65,1611 | 15,3466 | 144 |
| 7 | Q+ | 64,0528 | 15,6121 | 144 |
| 8 | BASE | 64,1098 | 15,5982 | 0 |

```
BASE   n=2  mean=63,9216  sd=0,1882
Q+     n=2  mean=63,9361  sd=0,1168
M+     n=2  mean=65,1011  sd=0,0601
QM+    n=2  mean=65,1133  sd=0,0072
```

### Quantités causales

```
QUANTIZE_DUPLICATE_WALL_SENSITIVITY    = Q+ - BASE  = +0,0144 ms   (rapport au bruit : 0,08 -> NON DETECTABLE)
MMVQ_KERNEL_DUPLICATE_WALL_SENSITIVITY = M+ - BASE  = +1,1794 ms   (rapport au bruit : 6,3 -> NET)
FULL_PATH_DUPLICATE_WALL_SENSITIVITY   = QM+ - BASE = +1,1917 ms
INTERACTION                            = -0,0022 ms
```

`FULL_PATH ≈ QUANTIZE + MMVQ` à 0,2 % près ⇒ le chemin est **additif** en wall marginal (§11) :

```
CHEMIN_ADDITIF_EN_WALL_MARGINAL=YES
CACHE_OR_PIPELINE_INTERACTION=NOT_DETECTED
```

## 3 — Lecture : la quantification n'est pas le sujet

La pente mesurée en 005I (1,26 ms) attribuée au chemin complet se décompose ainsi :

```
MMVQ (dot-product tq2_g128)  : 1,1794 ms  = 99,0 %
quantize_row_q8_1_cuda       : 0,0144 ms  =  1,2 %   (sous le bruit : non detecte)
interaction                  : -0,0022 ms = -0,2 %
```

Deux conséquences :

1. **Optimiser la quantification ne rapporterait rien de mesurable.** Le §13 du brief posait
   exactement cette question — la réponse est non, la quantification n'absorbe pas la pente.
2. **Le noyau MMVQ est le chemin critique.** C'est le seul axe matériel, et il est désormais désigné
   par une mesure causale décomposée, pas par un classement d'op-timing.

```
ENGINE006_PRIMARY_AXIS=MMVQ_KERNEL
ENGINE006_SECONDARY_AXIS=NONE_DETECTED
FUSE_QUANTIZE_Q8_1_WITH_TQ2_G128_MMVQ=NOT_JUSTIFIED_BY_MEASUREMENT
  (l'interaction est nulle : il n'y a rien a gagner en fusionnant)
```

Note d'honnêteté : la pente reste une **sensibilité marginale à cache chaud**, pas le coût natif
(§15 du brief 005I). Ce que la décomposition ajoute, c'est la **répartition** de cette sensibilité
entre les deux composants — et cette répartition est robuste (99 % contre 1,2 %, avec une
interaction nulle).

## 4 — Ce que cette passe n'a pas fait

```
ENGINE006_PROFILING_STATIQUE_CIBLE=NOT_DONE        (§14)
MEMORY_BOUND=NOT_PROVEN      COMPUTE_BOUND=NOT_PROVEN      OCCUPANCY_LIMITED=NOT_PROVEN   (§15)
ENGINE006B_FIRST_KERNEL_MODIFICATION=NOT_STARTED            (§16)
```

Je n'ai pas ouvert la modification noyau : le gate de sensibilité dit *quel composant* compte, il ne
dit pas *pourquoi* il coûte. Choisir un axe d'optimisation maintenant serait exactement l'erreur que
005F-R2 avait évitée sur `output.weight`. Les relevés statiques du §14 (géométrie de lancement,
`rows_per_block`, occupation de registres, largeur de vecteur, motif de déquantification
`tq2_g128`, structure de réduction) restent à faire **sur le composant dominant — le noyau MMVQ
seul**, maintenant que la quantification est hors sujet.

Un point de méthode pour la suite : la fenêtre de comptage de 8 jetons donnait 72 appels ciblés
(9 par jeton) alors que la campagne en rapporte 144 par jeton. Les deux relevés sont publiés tels
quels ; la réserve de 005G (`ONE_DISPATCH_SELECTION_EQUALS_ONE_LOGICAL_EXECUTION=NOT_PROVEN`) reste
valable et interdit de moyenner ces deux chiffres.

## 5 — Rappel des seuils (§19)

```
BASELINE_FREEZE_ENGINE004=      BACKEND_EQUIVALENT_TPS 15,7983   GREEDY_TPS 15,6330
BASELINE_APPROX_MS_PER_TOKEN=   63,3
OBJECTIF_20_TOK_S=              50,0 ms  ->  il reste ~13,3 ms/token a retirer
```

La sensibilité de 1,19 ms par repeat **ne prédit pas** ce gain disponible : elle prouve que ce
travail est sur le chemin temporel, elle ne chiffre pas ce qu'une optimisation rendrait. Le gain
devra être mesuré sur le workload produit normal (§18), pas déduit.

## 6 — Provenance

```
Runtime baseline   : libllama 3db83675… / libggml-cuda abafbf0d…   (restaurés, SHA vérifiés)
Arbre llama-obrain-cuda : mmvq.cu 2bf9a270… , ggml-cuda.cu 960e38d6… , qwen35.cpp f0fd1a3c…
                          (les trois identiques a l'etat vierge)
Runtime instrumente : build-e5i (source vierge + census + cible de graphe + decomposition 006A)
Instruments conserves : source-e6a/{mmvq-006a-patched.cu, apply-dup.py, ggml-cuda-census.*}
Sequence rejouee   : qualification/run003/run003-tokens.txt (256 tokens teacher-forced)
Note d'arbre : src/llama-context.cpp et src/llama-graph.cpp restent modifies dans l'arbre ; ce
               n'est PAS de cette passe (aucune de nos passes ne les touche) — signale, non modifie.
```

## 7 — Ledger

```
ENGINE005I_STATUS=CLOSED_CAUSAL_SENSITIVITY_SCOPE_CORRECTED
TARGET_SCOPE_GATE=PASS
NON_TARGET_EXTRA_MMVQ_EXECUTIONS=0
HARNESS_DEFECT_11=NON_TARGET_COUNTER_INCREMENTED_TWICE_AND_CONFLATED
NATIVE_MMVQ_DUPLICATE_WORK_WALL_SENSITIVITY_MS_PER_REPEAT=SUPERSEDED_BY_COMPONENT_DECOMPOSITION
ATTN_GATE_TQ2_MMVQ_CAUSAL_WALL_SENSITIVITY=MEASURED
TARGETED_MMVQ_WORK_IS_ON_WALL_CRITICAL_PATH=SUPPORTED
ATTN_GATE_NATIVE_WALL_COST=NOT_DERIVABLE
ENGINE006_STATUS=OPENED_COMPONENT_DECOMPOSITION_DONE
ENGINE006_TARGET=recurrent_attn_gate.weight/tq2_g128/decode_s1_1/MMVQ
ENGINE006_TARGET_POPULATION=36_TQ2_WEIGHTS
NON_TARGET_RECURRENT_ATTN_GATE_Q4K_WEIGHT_COUNT=12
ENGINE006_KERNEL_MODIFICATION_ALLOWED=YES
ENGINE006_ACTION=OPTIMIZE_CURRENT_MMVQ_FOR_EXACT_TQ2_G128_DECODE_SIGNATURE
ENGINE006_PRIMARY_AXIS=MMVQ_KERNEL
ENGINE006_SECONDARY_AXIS=NONE_DETECTED
ENGINE006B_FIRST_KERNEL_MODIFICATION=NOT_STARTED
EXTRA_QUANTIZE_FUNCTIONALLY_NEUTRAL=SOURCE_PROVEN
EXTRA_MMVQ_FUNCTIONALLY_NEUTRAL=SOURCE_PROVEN
ALL_BITWISE_EQUAL_4_VARIANTS=YES
BASE_MEAN_MS_PER_TOKEN=63.9216
QPLUS_MEAN_MS_PER_TOKEN=63.9361
MPLUS_MEAN_MS_PER_TOKEN=65.1011
QMPLUS_MEAN_MS_PER_TOKEN=65.1133
QUANTIZE_DUPLICATE_WALL_SENSITIVITY_MS=0.0144
MMVQ_KERNEL_DUPLICATE_WALL_SENSITIVITY_MS=1.1794
FULL_PATH_DUPLICATE_WALL_SENSITIVITY_MS=1.1917
INTERACTION_MS=-0.0022
CHEMIN_ADDITIF_EN_WALL_MARGINAL=YES
MEMORY_BOUND=NOT_PROVEN
COMPUTE_BOUND=NOT_PROVEN
OCCUPANCY_LIMITED=NOT_PROVEN
NEXT_MEASUREMENT=STATIC_TARGETED_AUDIT_OF_MMVQ_KERNEL_COMPONENT
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
