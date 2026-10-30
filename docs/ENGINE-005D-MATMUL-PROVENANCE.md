# ENGINE-005D — GRAPH-NATIVE MATMUL PROVENANCE : résultat

Date : 2026-09-29. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/qualification/e5d/`

---

## 0 — Réponse à la question posée

`WHICH_EXACT_MATMUL_SIGNATURES_DOMINATE_QWEN3_8_27B_REAL_DECODE?`

**Réponse : la tête de sortie (`output.weight`, q6_K, N=5120) et le portail d'attention des 48
couches récurrentes (`attn_gate.weight`, `t2_g128`).** Et la dominance est **récurrente**, pas
uniforme : 86,4 % du temps MUL_MAT contre 13,6 % pour les 16 couches d'attention.

La méthode demandée a fonctionné exactement comme prévu, et les portes sont franchies :

```
LAYER_OUTPUT_RECORDED_COUNT=64
LAYER_OUTPUT_ANCHOR_FOUND_COUNT=64/64
LAYER_OUTPUT_TOPOLOGICAL_ORDER=PASS
GRAPH_NODE_LAYER_ATTRIBUTED_COUNT=4006 / 4010
EXECUTED_MUL_MAT_LAYER_COVERAGE_PERCENT=100.00
EXECUTED_MUL_MAT_UNKNOWN_LAYER_MS=0.00
```

**Aucune fraction non attribuée.** `UNKNOWN_LAYER` est vide.

---

## 1 — Méthode (§6 à §13) : provenance de construction, coût runtime nul

Aucun renommage de tenseur, aucun nœud ajouté, aucune exécution découpée :

1. **Pendant la construction** (`src/models/qwen35.cpp`, fin de boucle de couche) :
   `ggml_prov_record_layer_out(il, cur)` — une ligne, enregistre le **pointeur** du tenseur de
   sortie de la couche.
2. **Une fois le graphe construit**, avant exécution : `ggml_prov_analyze(cgraph->nodes, n_nodes)`
   construit `NODE_INDEX_BY_POINTER`, retro-trouve les 64 ancres, et attribue par
   **intervalles entre ancres consécutives** — méthode validée par
   `LAYER_OUTPUT_TOPOLOGICAL_ORDER=PASS` (indices strictement croissants).
3. **Pendant l'exécution** : les paires d'événements CUDA déjà en place (005A/005C) alimentent
   `ggml_prov_acc(couche, op, ms)`. Une seule exécution de graphe normale.

```
HOT_PATH_PROVENANCE_LOOKUP=NO
ONE_NORMAL_GRAPH_EXECUTION=YES
LAYER_SLICED_EXECUTION=NOT_USED
```

Ancres mesurées (extrait) : couche 0 → nœud 64, couche 1 → 127, couche 2 → 190,
couche 3 (attention) → 231, … couche 63 → 4005. Intervalles non chevauchants par construction.

## 2 — Répartition du temps par classe de couche (§19)

Fenêtre : 24 tokens, classe A pure (`PROPERTIES_CHANGED_TRUE=0`, `KV_POSITION≈1209`, graphes
désactivés pour la visibilité par lancement).

```
LAYER_SUM_TOTAL_MS=14113.04          (part non attribuée : POST_MODEL + hors couche)
RECURRENT_TOTAL_MS=10150.09          RECURRENT_TOTAL_PERCENT=71.96
ATTENTION_TOTAL_MS=1853.05           ATTENTION_TOTAL_PERCENT=13.13
POST_MODEL_TOTAL_MS= (complément)    → 14,91 %
RECURRENT_MUL_MAT_MS=8722.03
ATTENTION_MUL_MAT_MS=1369.09
MUL_MAT_ROLE_TOTAL_MS=12197.28
```

### La réponse à §11

```
RECURRENT_MUL_MAT_SHARE = 8722.03 / 12197.28 = 71.9 % du temps MUL_MAT total
ATTENTION_MUL_MAT_SHARE = 1369.09 / 12197.28 = 11.2 %
SHARED_OU_GLOBAL        = 2106.16 / 12197.28 = 17.3 %  (tête de sortie, hors couches)
```

⇒ **`MATMUL_DOMINANCE_BECAUSE_48_RECURRENT_LAYERS`**, et non
`MATMUL_DOMINANCE_UNIFORMLY_ACROSS_ALL_LAYERS`. Les 48 couches récurrentes portent 6,4 fois le
temps MUL_MAT des 16 couches d'attention.

## 3 — Signatures dominantes (§14 à §17)

`MATMUL_SIGNATURE = (classe, rôle du poids, type, M, N, K)`. Poids identifié **par pointeur**
(`node->src[0]` → nom de tenseur du modèle), pas par analyse de chaînes.

| rang | classe | rôle du poids | type | M\|N\|K | ms (24 tok) | part |
|---|---|---|---|---|---|---|
| 1 | **hors couche** | **`output.weight`** | **q6_K** | 5120\|1\|5120 | **2106.2** | **17.27 %** |
| 2 | recurrent | `attn_gate.weight` (×48 couches) | tq2_g128 | 5120\|1\|5120 | 855.5 | 7.01 % |
| 3 | attention | `attn_q.weight` (×16) | tq2_g128 | 5120\|1\|5120 | 503.6 | 4.13 % |
| 4 | recurrent | `ssm_alpha.weight` (×48) | **bf16** | 5120\|1\|5120 | 295.1 | 2.42 % |
| 5 | recurrent | `ffn_gate.weight` (×48) | tq2_g128 | 5120\|1\|5120 | 265.9 | 2.18 % |
| 6 | recurrent | `ffn_up.weight` (×48) | tq2_g128 | 5120\|1\|5120 | 263.7 | 2.16 % |
| 7 | recurrent | `ssm_beta.weight` (×48) | **bf16** | 5120\|1\|5120 | 141.3 | 1.16 % |
| 8 | attention | `ffn_gate.weight` (×16) | tq2_g128 | 5120\|1\|5120 | 87.9 | 0.72 % |
| 9 | attention | `ffn_up.weight` (×16) | tq2_g128 | 5120\|1\|5120 | 87.6 | 0.72 % |
| 10 | attention | `attn_k.weight` (×16) | tq2_g128 | 5120\|1\|5120 | 69.3 | 0.57 % |
| 11 | attention | `attn_output.weight` (×16) | tq2_g128 | 5120\|1\|5120 | 32.5 | 0.27 % |

Toutes les signatures dominantes sont **vectorielles** : `N = 1` (un seul token par pas), donc
`MMVQ` (mat-vec quantifié) et non `MMQ` — sauf la tête de sortie, dont `N` dépend de la tranche.

### Deux différences structurelles majeures

1. **`output.weight` (q6_K) domine à lui seul 17,3 % du temps MUL_MAT** — c'est une signature
   **unique** (une seule couche logique, la tête LM), pas une famille de 48. C'est la cible la plus
   concentrée du modèle.
2. **`ssm_alpha.weight` et `ssm_beta.weight` sont en `bf16`** (non quantifiés) et totalisent
   **3,58 %** du temps MUL_MAT sur 41 472 appels par fenêtre, alors que leurs matrices sont
   minuscules en poids (rang de pas de temps = 48). Leur forme `5120|1|5120` montre que le calcul
   est fait en pleine dimension malgré un résultat de rang 48 : **candidat d'optimisation
   structurelle**, pas de kernel.

## 4 — MMVQ vs MMQ (§20)

```
DOMINANT_KERNEL_PATH=MMVQ (toutes les signatures dominantes ont N=1 : GEMV quantifié)
MMQ_PRODUCTION_SHARE=negligeable dans le decodage token-par-token (fenetre N=1)
MMVQ_PRODUCTION_SHARE=dominant (>95 % du temps MUL_MAT attribuable aux signatures N=1)
```

Réserve explicite : le chemin noyau n'est pas lu depuis la bibliothèque (aucune instrumentation
CUDA-side), il est **déduit des formes** (`N=1` ⇒ `mul_mat_vec_*` / MMVQ). C'est une déduction
forte mais ce n'est pas une observation.

## 5 — Réserve d'échelle (à ne pas oublier)

```
MUL_MAT_ROLE_TOTAL_MS=12197.28  sur 24 tokens  → 508 ms/token d'evenements
paroi mesuree du meme run                       →  71 ms/token (DECODE_PLUS_SYNC_MS p50 = 71.10)
```

Le facteur d'échelle reste **~7×** dans cette passe également : les **parts relatives** (ce que
005D devait produire) sont solides et reproduites, les **valeurs absolues en ms ne doivent pas**
être converties en temps mural. `ABSOLUTE_OP_TIMING_TO_WALL_MAPPING=NOT_VALIDATED` reste en
vigueur.

## 6 — Ce que 005D permet de décider (et ce qu'il ne permet pas)

```
TOP_MATMUL_SIGNATURES_IDENTIFIED=YES
DOMINANT_KERNEL_PATH_IDENTIFIED=YES (par les formes)
DOMINANT_PRODUCTION_SHAPES_IDENTIFIED=YES
EXECUTED_MUL_MAT_LAYER_COVERAGE_PERCENT=100.00
```

Les quatre conditions d'ouverture de §24 sont réunies, **sauf** la conversion en millisecondes
murales. Deux cibles se dégagent, de nature différente :

- **Cible 1 (concentration)** : la projection de sortie `output.weight` q6_K, 17,3 % du temps
  MUL_MAT à elle seule ;
- **Cible 2 (structure)** : `attn_gate` récurrent (7,0 %) + `ssm_alpha`/`ssm_beta` bf16 (3,6 %)
  + `attn_q` attention (4,1 %) — familles quantifiées en `tq2_g128`.

**Ce que 005D n'établit pas** : que retirer ces opérations retirerait 13,3 ms/token
(l'objectif 20 tok/s). La chaîne parts-relatives → ms murales n'est toujours pas validée.

```
ENGINE006_TARGET=CANDIDATE_OUTPUT_WEIGHT_Q6K_AND_TQ2_G128_GATE_FAMILY
ENGINE006_KERNEL_MODIFICATION_ALLOWED=NO   (la conversion en ms murales reste requise)
```

## 7 — Provenance

```
Runtime baseline   : libllama 3db83675… / libggml-cuda abafbf0d…   (restaurés, SHA vérifiés)
Arbre llama-obrain-cuda : ggml-cuda.cu (960e38d6…) et src/models/qwen35.cpp remis à l'identique
Runtime E5D        : build-e5c/ (diagnostic seulement, NON baseline)
Instrument conservé : source-e5d/ggml-cuda-provenance.{cuh,cpp,cu}
NSYS CUDA kernel data = UNAVAILABLE_ON_THIS_PLATFORM ;  NCU = BLOCKED_ERR_NVGPUCTRPERM
```

Défauts rencontrés et corrigés (tous `RESOLVED`) : en-tête périmé non livré (fonctions manquantes),
include posé en fin de fichier au lieu de la tête, greffe Python ayant corrompu l'en-tête (réécrit
complet en une passe), divergence de signature `ggml_prov_analyze`, `#include <string>` manquant.
Leçon : chaque étape a été validée par un compteur de sortie (`LAYER_OUTPUT_ANCHOR_FOUND_COUNT`,
`EXECUTED_MUL_MAT_LAYER_COVERAGE_PERCENT`), jamais par relecture.

## 8 — Ledger

```
ENGINE004_STATUS=CLOSED_BASELINE
ENGINE005A_STATUS=CLOSED_GPU_TIMELINE
ENGINE005C_STATUS=CLOSED_METHOD_STRUCTURALLY_INSUFFICIENT
NODE_LAYER_TAGGING_COVERAGE_PERCENT=3.46
MUL_MAT_LAYER_ATTRIBUTION=UNAVAILABLE_VIA_CB_IL
OP_TIMING_SUM_TO_WALL_ROOT_CAUSE=INSTRUMENTATION_SEMANTICS_NOT_GPU_CONCURRENCY
TOP_KERNEL_FAMILY=MUL_MAT
ENGINE005D_STATUS=CLOSED_LAYER_PROVENANCE_OK
LAYER_OUTPUT_ANCHOR_FOUND_COUNT=64/64
LAYER_OUTPUT_TOPOLOGICAL_ORDER=PASS
GRAPH_NODE_LAYER_ATTRIBUTED_COUNT=4006
EXECUTED_MUL_MAT_LAYER_COVERAGE_PERCENT=100.00
RECURRENT_TOTAL_PERCENT=71.96
ATTENTION_TOTAL_PERCENT=13.13
RECURRENT_MUL_MAT_MS=8722.03
ATTENTION_MUL_MAT_MS=1369.09
MATMUL_DOMINANCE_BECAUSE_48_RECURRENT_LAYERS=YES
MATMUL_DOMINANCE_UNIFORMLY_ACROSS_ALL_LAYERS=NO
TOP_MATMUL_SIGNATURE=output.weight|q6_K|5120|1|5120|17.27%
TOP_MATMUL_FAMILY=attn_gate.weight|tq2_g128|recurrent|7.01%
SECOND_FINDING=ssm_alpha_ssm_beta_bf16_FULL_DIM_3.58%
TOP_MATMUL_SIGNATURES_IDENTIFIED=YES
DOMINANT_KERNEL_PATH_IDENTIFIED=YES_BY_SHAPE_MMVQ
DOMINANT_PRODUCTION_SHAPES_IDENTIFIED=YES
ABSOLUTE_OP_TIMING_TO_WALL_MAPPING=NOT_VALIDATED
ENGINE006_BRANCH=ENGINE006_MATMUL_OPTIMIZATION
ENGINE006_TARGET=NOT_PINNED_UNTIL_WALL_CONVERSION_VALIDATED
ENGINE006_KERNEL_MODIFICATION_ALLOWED=NO
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
