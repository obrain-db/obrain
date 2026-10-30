# ENGINE-005C — LAYER-LEVEL ATTRIBUTION : réponse établie

Date : 2026-09-29 (2ᵉ passe). Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/qualification/e5c/`

---

## 0 — Réponse à la question posée

`WHERE_INSIDE_THE_64_MODEL_LAYERS_IS_THE_TOKEN_TIME_SPENT?`

**La question n'est pas répondable par une attribution par noms de nœuds dans ce moteur**, et la
raison est maintenant mesurée, pas supposée :

```
NODE_LAYER_TAGGING_COVERAGE_PERCENT = 3.46 %
```

Sur **401 912 opérations** mesurées dans la fenêtre, **3,46 % seulement** portent un indice de
couche. Et les opérations qui en portent ne sont pas celles qui coûtent :

| opération | attention_ms | recurrent_ms |
|---|---|---|
| `SET_ROWS` | 78.64 | 0.00 |
| `SCALE` | 0.00 | 1.87 |

**`MUL_MAT` — 87 % du temps GPU — ne porte aucun indice de couche.** C'est exactement l'opération
sur laquelle porte la question. L'attribution par nom est donc **structurellement** inapte, et non
« pas encore au point ».

```
LAYER_ATTRIBUTION=NOT_DELIVERED
LAYER_ATTRIBUTION_BLOCKER=LAYER_INDEX_ABSENT_FOR_QUANTIZED_MATMUL_NODES
LAYER_SUM_TO_WALL_RATIO=NOT_MEASURABLE (l'attribution est incomplète par construction)
```

Aucun chiffre de répartition récurrent/attention n'est publié : il n'a pas été mesuré.

---

## 1 — Ce qui est livré et solide

### 1.1 Classification exacte des 64 couches (§9)

Lue dans les métadonnées du modèle (`Qwen3.8-27B-OBRAIN-v3.gguf`, parseur GGUF direct) :

```
general.architecture           = qwen35
qwen35.block_count             = 65        (64 couches + 1 tête nextn)
qwen35.full_attention_interval = 4
qwen35.attention.head_count    = 24   head_count_kv = 4
qwen35.attention.key_length    = 256  value_length  = 256
qwen35.embedding_length        = 5120 feed_forward_length = 17408
qwen35.ssm.conv_kernel = 4   state_size = 128   group_count = 16
qwen35.ssm.time_step_rank = 48   inner_size = 6144
```

`full_attention_interval = 4` ⇒ attention sur `il % 4 == 3` :

```
ATTENTION_LAYER_COUNT=16
ATTENTION_LAYER_INDICES=[3,7,11,15,19,23,27,31,35,39,43,47,51,55,59,63]
RECURRENT_LAYER_COUNT=48
RECURRENT_LAYER_INDICES=[0,1,2,4,5,6,8,9,10,12,13,14,16,17,18,20,21,22,24,25,26,28,29,30,
                         32,33,34,36,37,38,40,41,42,44,45,46,48,49,50,52,53,54,56,57,58,60,61,62]
```

### 1.2 Le gate d'accounting : ce que la mesure par calcul donne

Diagnostic de sûreté de l'instrument (une ligne par calcul de graphe, présent dans toutes les
exécutions) :

```
E5C_COMPUTE nodes=4010 measured=926 sum_ms=29.5      (répétable : 29.4 / 29.6 / 38.1 selon le mode)
```

- **926 nœuds sur 4010 sont réellement exécutés** par calcul (les autres sont des vues repliées) ;
- la somme de leurs durées d'événements vaut **~29,5 ms** par calcul ;
- avec **2 calculs par token** (mesuré), l'ordre de grandeur recoupe la paroi
  (`DECODE_PLUS_SYNC_MS p50 = 70,66 ms` sur le même run, graphes désactivés).

C'est un progrès sur l'anomalie 6,77× de 005A : **mesurée par calcul, restreinte aux nœuds
exécutés et avec synchronisation préalable, l'instrumentation redonne un ordre de grandeur
cohérent avec la paroi.** La cause de l'inflation de 6,77× est identifiée (§3).

### 1.3 Fenêtre conforme

```
PROFILED_TOKEN_COUNT=24       PROFILED_WINDOW_KV_TRANSITION_COUNT=0
PROPERTIES_CHANGED_TRUE=0     (classe A pure)
KV_POSITION_AT_WINDOW_START=1209   (identique à 005A)
```

---

## 2 — Pourquoi le nom de nœud ne peut pas porter la couche

Trois faits mesurés dans le code et par la mesure :

1. `llm_graph_context::cb()` (51 sites d'appel) reçoit bien `il`, mais **ne renomme pas** le
   tenseur : il ne fait qu'appeler `cb_func`. Vérifié dans le source.
2. La couche **n'atteint pas** les nœuds de matmul quantifiés. Toute la fusion de la famille
   GDN/attention/MLP passe par des nœuds créés hors des 51 sites `cb()`.
3. Le « sauvetage » tenté (inscrire `_l<k>` dans le nom au moment de `cb()`) **fonctionne**
   (les 64 couches apparaissent, `attention` reçoit 864 opérations `SET_ROWS` par token) mais ne
   couvre que **3,46 %** des opérations — donc il ne sert à rien pour la question posée :

```
RECURRENT_LAYER_TOTAL_MS=1.77     ATTENTION_LAYER_TOTAL_MS=78.35
LAYER_SUM_TOTAL_MS=14112.87       HORS_COUCHE_MS=14032.75  (99,43 %)
```

Ces quatre chiffres ne sont **pas** une répartition : ils mesurent l'échec de la couverture.

## 3 — Anomalie 6,77× : cause identifiée

Les pistes de §17/§18 ont été testées :

| question | réponse mesurée |
|---|---|
| `ARE_TIMED_INTERVALS_NESTED?` | NON — les paires encadrent la même instruction, pas d'imbrication |
| `ARE_MULTIPLE_EVENTS_MEASURING_THE_SAME_GPU_WORK?` | **OUI** — les paires étaient créées pour **tous** les nœuds, y compris les 3 084 non exécutés, et mesurées quand même |
| `DO_GRAPH_REPLAY_EVENTS_REPORT_PER_NODE_EXECUTION_OR_HIGH_LEVEL_OP_SPANS?` | par exécution de nœud, sur **plusieurs itérations** de la boucle de capture (résultats amortis au maximum entre itérations) |
| `ARE_MULTIPLE_STREAMS_ACTUALLY_USED?` | NON — aucun événement concurrent enregistré pour ce modèle |
| `CAN_ONE_KERNEL_BE_ATTRIBUTED_TO_MULTIPLE_GGML_OPS?` | OUI — une opération fusionnée compte pour un site `cb()` mais exécute plusieurs nœuds |

Et deux erreurs de mesure franches, diagnostiquées par des erreurs CUDA explicites :

```
restreindre aux nœuds exécutés   : sans quoi  cudaEventElapsedTime → "invalid resource handle"
synchroniser avant de mesurer    : sans quoi  cudaEventElapsedTime → "device not ready"
```

`OP_TIMING_SUM_TO_WALL_RATIO=6.77` est donc **un artefact d'attribution**, pas une preuve de
concurrence GPU — conforme à l'avertissement §18. Après correction, l'ordre de grandeur
redevient cohérent (§1.2).

```
OP_TIMING_SUM_TO_WALL_RATIO=6.77
ROOT_CAUSE=INSTRUMENTATION_SEMANTICS_NOT_GPU_CONCURRENCY
ABSOLUTE_OP_TIMING_TO_WALL_MAPPING=NOT_VALIDATED
```

## 4 — Correctif spécifié (même conclusion que la 1ʳᵉ passe, désormais étayée)

L'information de couche doit être **posée dans le chemin chaud d'exécution**, pas extraite d'un
nom. Deux voies :

1. **Instrumentation au niveau du scheduler** (`ggml_backend_sched`) : les nœuds y sont parcourus
   en blocs par graphe ; enregistrer la couche courante depuis la structure du graphe llama
   (`llm_graph_context` connaît `il` par construction de bloc) et la poser dans une variable
   thread-locale lue par l'instrument de timing. Pas de dépendance aux noms.
2. **Wrapper d'exécution par couche** : exécuter la boucle de nœuds par tranches de couche, avec
   une paire d'événements par tranche (intervalles non chevauchants par construction, cf. §6 du
   protocole).

La voie 2 est celle qui satisfait exactement la contrainte « NON_OVERLAPPING_LAYER_INTERVALS » du
protocole. Elle exige de connaître les bornes d'index de chaque couche dans `cgraph->nodes`, ce que
la construction du graphe llama fournit si on l'enregistre au moment du `cb()` (les 51 sites
donnent les bornes attention/MLP, pas la totalité) — d'où la préférence pour la voie 1.

## 5 — Provenance et incidents

```
Runtime baseline   : libllama 3db83675… / libggml-cuda abafbf0d…   (restaurés, SHA vérifiés)
Arbre llama-obrain-cuda : ggml-cuda.cu remis à l'identique (960e38d6…)
                          llama-graph.cpp : version patchée conservée en artefact
                          (source-e5c/llama-graph.patched.cpp)
Runtime E5C        : build-e5c/ (diagnostic seulement, NON baseline)
NSYS CUDA kernel data  = UNAVAILABLE_ON_THIS_PLATFORM
NCU hardware counters  = BLOCKED_ERR_NVGPUCTRPERM
```

Incidents de cette passe (tous `RESOLVED`, `AFFECTS_PROMOTED_RESULT=NO`) :

| # | défaut | détection | correction |
|---|---|---|---|
| 1 | mesure de nœuds non exécutés | `invalid resource handle` | liste des nœuds exécutés |
| 2 | mesure sans synchronisation | `device not ready` | `cudaEventSynchronize` avant lecture |
| 3 | événements dans une capture | crash | passe faite graphes désactivés |
| 4 | état statique en double (harnais + bibliothèque) | sortie vide malgré `ENABLED=YES` | définition unique dans un `.cu` dédié, surface `extern "C"` |
| 5 | bloc d'accumulation absent du binaire compilé | compteur `E5C_COMPUTE` muet | script de patch réécrit et vérifié |
| 6 | noms de fonctions divergents | erreurs de compilation explicites | renommage aligné |

## 6 — Ledger

```
ENGINE004_STATUS=CLOSED_BASELINE
ENGINE005A_STATUS=CLOSED_GPU_TIMELINE
TOP_KERNEL_FAMILY=MUL_MAT
TOP_KERNEL_FAMILY_PERCENT_OP_TIMING=86.2_TO_91.7_PERCENT
MATMUL_DOMINATES_OPERATION_TIMING=YES
MATMUL_DOMINATES_TRUE_WALL_CRITICAL_PATH=STRONGLY_SUPPORTED_NOT_YET_QUANTIFIED
MATMUL_DOMINANCE_DEPENDS_ON_CUDA_GRAPH=NO
HOST_LAUNCH_OVERHEAD_IS_PRIMARY_LIMITER=NO
OP_TIMING_SUM_TO_WALL_RATIO=6.77
OP_TIMING_SUM_TO_WALL_ROOT_CAUSE=INSTRUMENTATION_SEMANTICS_NOT_GPU_CONCURRENCY
ABSOLUTE_OP_TIMING_TO_WALL_MAPPING=NOT_VALIDATED
GPU_ACTIVE_FRACTION=NOT_MEASURABLE
GPU_IDLE_GAPS=NOT_MEASURABLE
NSYS_CUDA_KERNEL_DATA=UNAVAILABLE_ON_THIS_PLATFORM
NCU_STATUS=BLOCKED_ERR_NVGPUCTRPERM
ENGINE005B_STATUS=BLOCKED_BY_PLATFORM_PERMISSIONS_AND_CUPTI_SUPPORT
ENGINE005C_STATUS=ATTEMPTED_BLOCKED_BY_LAYER_INDEX_ABSENCE
LAYER_ATTRIBUTION=NOT_DELIVERED
NODE_LAYER_TAGGING_COVERAGE_PERCENT=3.46
LAYER_INDEX_ABSENT_FOR_QUANTIZED_MATMUL_NODES=YES
MEASURED_NODES_EXECUTED_PER_COMPUTE=926_OF_4010
MEASURED_EVENT_SUM_PER_COMPUTE_MS=29.5
COMPUTES_PER_TOKEN=2
ATTENTION_LAYER_INDICES=[3,7,11,15,19,23,27,31,35,39,43,47,51,55,59,63]
RECURRENT_LAYER_COUNT=48
NEXT_MEASUREMENT=SCHEDULER_LEVEL_LAYER_TAG_OR_PER_LAYER_EXECUTION_SLICES
ENGINE006_BRANCH=ENGINE006_MATMUL_OPTIMIZATION
ENGINE006_TARGET=NOT_YET_PINNED (TOP_MATMUL_SIGNATURES_IDENTIFIED requis d'abord)
ENGINE004_KV_LADDER_OPTIONAL=YES
ENGINE005_BLOCKED_BY_KV_LADDER=NO
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```

## 7 — Ce que 005C n'a PAS permis de trancher

```
MATMUL_DOMINANCE_BECAUSE_48_RECURRENT_LAYERS    : NON TRANCHÉ
MATMUL_DOMINANCE_UNIFORMLY_ACROSS_ALL_LAYERS    : NON TRANCHÉ
WHICH_20_PERCENT_OF_MATMUL_CALLS_CONSUME_80_PERCENT_OF_MATMUL_TIME : NON MESURÉ
TOP_MATMUL_SIGNATURES                            : NON IDENTIFIÉES (§12/§14)
MMVQ_VS_MMQ_SHAPE_DISTRIBUTION                   : NON MESURÉE
DO_THE_TOP_N_MATMUL_SIGNATURES_COVER_ENOUGH_WORK_TO_PLAUSIBLY_REMOVE_13.3MS : NON RÉPONDU
```

`ENGINE006_TARGET` reste **NON ÉPINGLÉ** : ouvrir une optimisation MMVQ/MMQ maintenant reviendrait
à optimiser un kernel peut-être important au benchmark et marginal dans Qwen3.8-27B réel — ce que
§22 interdit explicitement.
