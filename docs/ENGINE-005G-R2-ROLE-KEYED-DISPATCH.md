# ENGINE-005G-R2 — ROLE-KEYED RUNTIME DISPATCH : fait

Date : 2026-09-29. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/qualification/e5gr2/`

---

## 0 — Résultat

La jointure demandée est faite, **dans le même processus, clé par pointeur de poids**, et elle est
**complète** :

```
ROLE_KEYED_DISPATCH_TOTAL=77413
GLOBAL_DISPATCH_TOTAL=77413
UNJOINED_DISPATCH_COUNT=0
UNJOINED_DISPATCH_PERCENT=0.0000
UNJOINED_WEIGHT_POINTERS=0
```

Aucune disparition silencieuse : **100 % des sélections sont attribuées à un rôle de poids.**

Et le résultat est **plus intéressant que prévu** : la plupart des signatures **empruntent plusieurs
chemins**. La réponse n'est donc pas « MMVQ » ni « MMQ » — c'est `MIXED`, avec la distribution.

```
RECURRENT_ATTN_GATE_KERNEL_PATH=MIXED(MMVQ99% + OTHER1% + MMQ0.4%)
OUTPUT_WEIGHT_KERNEL_PATH=MMVQ            ← RUNTIME_PROVEN (100 % MMVQ)
```

---

## 1 — Freeze 005G (§1 à §3)

```
ENGINE005G_GLOBAL_CENSUS=MEASURED
CENSUS_DISTINCT_WEIGHT_POINTERS=893
CENSUS_MMVF_SELECTED_TOTAL=384
CENSUS_MMF_SELECTED_TOTAL=0
CENSUS_MMVQ_SELECTED_TOTAL=940
CENSUS_MMQ_SELECTED_TOTAL=928
CENSUS_OTHER_SELECTED_TOTAL=888
MULTIPLE_MATMUL_DISPATCH_PATHS_ACTIVE_IN_REAL_DECODE=YES
MMVQ_EXCLUSIVE_DECODE_HYPOTHESIS=REJECTED
MMQ_SELECTION_COUNT_NON_NEGLIGIBLE=YES
MMQ_WALL_TIME_NON_NEGLIGIBLE=NOT_CLAIMED          (les compteurs mesurent des selections, pas un cout)
MMVF_SELECTED_TOTAL_VISIBLE=YES
OTHER_SELECTED_TOTAL_VISIBLE=YES
OTHER_IS_BF16_SSM_PATH=HYPOTHESIS                 (desormais TESTEE, cf. §3)
```

## 2 — Gate de namespace (§4 à §6) : satisfaite par construction

La question « les pointeurs de 005D et 005G sont-ils dans le même espace ? » est **résolue par
construction** plutôt que supposée : les deux relevés sont produits **dans le même processus**, la
jointure se fait sur le pointeur brut, et la preuve est le taux de jointure lui-même.

```
SAME_PROCESS_JOIN=YES
RAW_POINTER_JOIN_ALLOWED=YES_BY_CONSTRUCTION
STABLE_POINTER_OFFSET_JOIN_VALIDATED=N/A
NO_HOT_PATH_NAME_LOOKUP=YES
NO_HOT_PATH_LOGGING=YES
```

Le harnais active `GGML_LAYER_PROVENANCE=1` et `GGML_LAYER_TIMING=1` : la provenance range
`pointeur de poids → rôle` (chemin froid), le census range `pointeur → chemin retenu` (chemin
chaud, deux atomiques `relaxed`), et le rapport joint les deux **après la fenêtre**.

## 3 — Table par rôle (§8, §11, §12, §18)

Fenêtre : 24 tokens (+4 de pré-roll), graphes désactivés, runtime instrumenté.
**Appels par token** (÷28), pour la lisibilité.

| rôle | MMVF | MMVQ | MMQ | OTHER | total/tok | ROUTE |
|---|---|---|---|---|---|---|
| `attn_gate.weight` (récurrent, tq2_g128) | 0 | 448,71 | 1,71 | 3,86 | 454,29 | **MIXED** MMVQ 99 % + OTHER 1 % + MMQ 0,4 % |
| `attn_q.weight` (attention, tq2_g128) | 0 | 149,57 | 0,57 | 1,29 | 151,43 | **MIXED** MMVQ 99 % |
| `ssm_alpha.weight` (récurrent, bf16) | 447,43 | 0 | 0 | 6,86 | 454,29 | **MIXED** MMVF 98 % + OTHER 2 % |
| `ssm_beta.weight` (récurrent, bf16) | 447,43 | 0 | 0 | 6,86 | 454,29 | **MIXED** MMVF 98 % + OTHER 2 % |
| `ffn_gate.weight` (récurrent) | 0 | 1,71 | 2,29 | 5,14 | 9,14 | **MIXED** OTHER 56 % + MMQ 25 % + MMVQ 19 % |
| `ffn_up.weight` (récurrent) | 0 | 1,71 | 2,29 | 5,14 | 9,14 | **MIXED** OTHER 56 % + MMQ 25 % + MMVQ 19 % |
| `output.weight` (global, q6_K) | 0 | **9,32** | 0 | 0 | 9,32 | **MMVQ** (100 %) |
| `ssm_out.weight` | 0 | 447,43 | 6,86 | 0 | 454,29 | MIXED MMVQ 98 % + MMQ 2 % |
| `attn_qkv.weight` | 0 | 447,43 | 6,86 | 0 | 454,29 | MIXED MMVQ 98 % + MMQ 2 % |
| `attn_output.weight` / `attn_k` / `attn_v` | 0 | ≈99 % | ≈0,4 % | ≈0,8 % | — | MIXED |

### Trois faits qui sortent de cette table

1. **`output.weight` est 100 % MMVQ** — première signature dont la route est **prouvée au runtime**
   (`OUTPUT_WEIGHT_KERNEL_PATH=RUNTIME_PROVEN`, source `RUNTIME_DISPATCH_BRANCH`). La déduction par
   la forme est **confirmée**, et `YES_BY_SHAPE_MMVQ` est retiré comme preuve principale.

2. **`ssm_alpha.weight` et `ssm_beta.weight` ne prennent ni MMVQ ni MMQ** : ils prennent **MMVF à
   98 %** (+ 2 % OTHER). L'hypothèse §18/§25 « chemin dense bf16 » est **confirmée en substance**
   mais le nom exact est **MMVF** (`mul_mat_vec_f`), pas un « OTHER » générique :
   ```
   DENSE_BF16_PATH_FOR_SSM_ALPHA_BETA=MMVF_RUNTIME_PROVEN_98_PERCENT
   SSM_ALPHA_BETA_OPTIMIZATION_CLASS=STRUCTURAL_OR_DENSE_BF16_PATH
   ```

3. **`ffn_gate`/`ffn_up` récurrents sont massivement OTHER (56 %)** — un chemin non
   MMVQ/MMQ/MMVF pour des poids qui, eux, sont quantifiés (`tq2_g128`). C'est inattendu et non
   expliqué à ce stade : soit une fusion (GLU) qui rebascule le dispatch, soit un prédicat
   supplémentaire. **Signalé comme observation, pas interprété.**

## 4 — Gate d'accounting (§9, §10)

```
ROLE_KEYED_DISPATCH_TOTAL == GLOBAL_DISPATCH_TOTAL = 77413          → PASS
UNJOINED_DISPATCH_PERCENT=0.0000                                    → PASS
DISPATCH_ACCOUNTING_MATCH=YES
```

Comparaison 005D : les deux relevés portent sur des fenêtres **différentes** (005D : 24 tokens
graphes actifs ; ici : 24 tokens graphes désactivés + provenance active). Les nombres bruts ne sont
donc **pas** comparés directement ; la normalisation par token est la seule forme valable et elle
est publiée ci-dessus en appels/token.

## 5 — Capability gate pour l'A/B (§13, §14, §15) — LE POINT DÉCISIF

`attn_gate` prend **MMVQ à 99 %**. La question « MMQ serait-il valide pour cette signature
exacte ? » se lit directement dans la cascade de prédicats. Pour `tq2_g128` :

- `ggml_cuda_should_use_mmvq(type, cc, ne11)` : pour `GGML_TYPE_TQ2_G128`, retourne
  `ne11 <= MMVQ_MAX_BATCH_SIZE_TQ2_G128` — **domaine étendu spécifique à ce type**, placé avant les
  politiques par architecture (`mmvq.cu:290`).
- `ggml_cuda_should_use_mmq(type, cc, ne11, n_experts)` : **également applicable** quand `ne11`
  dépasse le domaine MMVQ.

Le fait mesuré le confirme : **les deux chemins sont empruntés pour des poids `tq2_g128`** dans la
même charge (`attn_gate` : MMVQ 12 564 + MMQ 48 ; `ssm_out`/`attn_qkv` : MMVQ 12 528 + MMQ 192).
Autrement dit, la bascule MMVQ ↔ MMQ **existe réellement** et **se déclenche déjà** pour cette
famille de types.

```
RECURRENT_ATTN_GATE_KERNEL_PATH=MIXED_MMVQ_99_PERCENT
ALTERNATE_KERNEL_IMPLEMENTATION_EXISTS=YES                      (MMQ, pour tq2_g128)
ALTERNATE_KERNEL_IS_VALID_FOR_THIS_EXACT_SHAPE_QTYPE=LIKELY_BUT_NOT_PROVEN
ATTN_GATE_MMVQ_VS_MMQ_AB_FEASIBLE=PLAUSIBLE_PENDING_VALIDITY_PROOF
ENGINE005G_AB_ALLOWED=NO                                        (preuve de validite exacte requise)
```

Ce qui reste à prouver pour autoriser l'A/B, au sens strict de §14 : que **forcer MMQ sur les
exactes shapes d'`attn_gate`** produit un résultat **mathématiquement équivalent** (mêmes valeurs,
ou au minimum même trajectoire de tokens). Le fait que MMVQ soit choisi à 99 % suggère que
`ne11` est **dans** le domaine MMVQ pour ces appels — donc forcer MMQ sortirait du domaine pour
lequel MMVQ a été préféré, ce qui n'est pas la même chose que « deux chemins équivalents se
disputent la même shape ».

**Reformulation honnête** : la bascule existe pour ce type, mais les deux chemins ne servent
peut-être pas la même `ne11`. C'est précisément ce qu'un A/B validé devrait établir, et c'est la
prochaine mesure.

## 6 — État de 005G (§20)

```
ENGINE005G_STATUS=CLOSED_ROLE_KEYED_RUNTIME_DISPATCH
DOMINANT_KERNEL_PATH_IDENTIFIED=YES
DOMINANT_KERNEL_PATH_SOURCE=RUNTIME_DISPATCH_BRANCH
YES_BY_SHAPE_MMVQ=RETIRED_AS_PRIMARY_PROOF
DISPATCH_ACCOUNTING_MATCH=YES
UNJOINED_DISPATCH_PERCENT=0.0000
```

## 7 — Défauts de la passe (hors plumbing)

```
HARNESS_DEFECT_8=DISPATCH_CENSUS_NOT_DUMPED_BY_HARNESS_VARIANT      STATUS=RESOLVED  AFFECTS_PROMOTED_RESULT=NO
HARNESS_DEFECT_9=PROVENANCE_TABLE_EMPTY_BECAUSE_TIMING_FLAG_MISSING  STATUS=RESOLVED  AFFECTS_PROMOTED_RESULT=NO
HARNESS_DEFECT_10=LAYER_TIMING_EVENTS_ILLEGAL_DURING_GRAPH_CAPTURE   STATUS=RESOLVED_BY_DISABLING_GRAPHS  AFFECTS_PROMOTED_RESULT=NO
MROPE_BATCH_INIT_INCIDENT_ON_R2_HARNESS                              STATUS=RESOLVED  AFFECTS_PROMOTED_RESULT=NO
PLUMBING_FAILURE_COUNT=28      PLUMBING_FAILURE_29=NOT_ASSIGNED
```

Détail du défaut 9, instructif : la première jointure a rendu **100 % non joint** parce que la table
`pointeur → rôle` est alimentée par le chemin d'instrumentation par couche, qui n'était pas activé
(seule `GGML_LAYER_PROVENANCE` était posée). Détecté par le compteur de jointure lui-même — pas par
relecture.

## 8 — Provenance

```
Runtime baseline   : libllama 3db83675… / libggml-cuda abafbf0d…   (restaurés, SHA vérifiés)
Arbre llama-obrain-cuda : ggml-cuda.cu 960e38d6… , qwen35.cpp f0fd1a3c…   (identiques aux vierges)
Runtime instrumenté : build-e4-observe (timing + provenance + census + jointure)
Instruments conservés : source-e5g/ (census) , source-e5d/ (provenance)
```

## 9 — Ledger

```
ENGINE005D_STATUS=CLOSED_PROVENANCE
ENGINE005F_R2_STATUS=CLOSED_NULL_WALL_SIGNAL_WITH_INCOMPLETE_ISOLATION
ENGINE005G_STATUS=CLOSED_ROLE_KEYED_RUNTIME_DISPATCH
GLOBAL_DISPATCH_TOTAL=77413
ROLE_KEYED_DISPATCH_TOTAL=77413
UNJOINED_DISPATCH_PERCENT=0.0000
DISPATCH_ACCOUNTING_MATCH=YES
MULTIPLE_MATMUL_DISPATCH_PATHS_ACTIVE_IN_REAL_DECODE=YES
MMVQ_EXCLUSIVE_DECODE_HYPOTHESIS=REJECTED
MMQ_SELECTION_COUNT_NON_NEGLIGIBLE=YES
OUTPUT_WEIGHT_KERNEL_PATH=MMVQ
OUTPUT_WEIGHT_KERNEL_PATH_SOURCE=RUNTIME_DISPATCH_BRANCH
OUTPUT_WEIGHT_KERNEL_PATH_PROMOTION=RUNTIME_PROVEN
RECURRENT_ATTN_GATE_KERNEL_PATH=MIXED_MMVQ_99_PERCENT
RECURRENT_ATTN_GATE_KERNEL_PATH_SOURCE=RUNTIME_DISPATCH_BRANCH
SSM_ALPHA_BETA_DENSE_PATH=MMVF_RUNTIME_PROVEN_98_PERCENT
FFN_GATE_UP_RECURRENT_OTHER_SHARE=56_PERCENT_UNEXPLAINED
ALTERNATE_KERNEL_IMPLEMENTATION_EXISTS=YES
ALTERNATE_KERNEL_IS_VALID_FOR_THIS_EXACT_SHAPE_QTYPE=LIKELY_BUT_NOT_PROVEN
ATTN_GATE_MMVQ_VS_MMQ_AB_FEASIBLE=PLAUSIBLE_PENDING_VALIDITY_PROOF
ENGINE005G_AB_ALLOWED=NO
ENGINE006_TARGET=NOT_PINNED
ENGINE006_KERNEL_MODIFICATION_ALLOWED=NO
NEXT_MEASUREMENT=ATTN_GATE_MMQ_FORCING_VALIDITY_AND_EQUIVALENCE_TEST
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
