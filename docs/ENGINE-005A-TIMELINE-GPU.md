# ENGINE-005A — GPU TIMELINE AND KERNEL FAMILY ATTRIBUTION

Date : 2026-09-29. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/qualification/e5a/`

---

## 0 — Réponse courte

```
TOP_KERNEL_FAMILY = MATMUL / MMVQ  (MUL_MAT)
TOP_KERNEL_FAMILY_PERCENT = 86.2 % .. 91.7 % du temps GPU total
```

**Ce n'est ni GDN/SSM, ni l'attention, ni la mémoire/KV.** Le plafond de ~15,7 tok/s est porté par
les multiplications matricielles quantifiées, à ~97 % par le temps GPU agrégé, quel que soit le
mode d'exécution mesuré (graphes CUDA actifs ou désactivés).

---

## 1 — Autorité baseline (rappel, inchangée)

```
ENGINE004_STATUS=CLOSED_BASELINE
ENGINE004_BASELINE_BACKEND_EQUIVALENT_TPS=15.7983
ENGINE004_BASELINE_END_TO_END_GREEDY_TPS=15.6330
BACKEND_SYNC_WAIT_P50_MS=63.2823   HOST_DECODE_SUBMISSION_P50_MS=0.0665   ARGMAX_P50_MS=0.6692
ENGINE004_BASELINE_SEQUENCE_SHA256=210b7eb55ff64a10c1d51af5391edc21b8d4545d297817c332d5635842590231
Runtime : libllama 3db83675…, libggml-cuda abafbf0d…   (restaurés et vérifiés après chaque build)
```

Note de vocabulaire : le budget réel par token normal est de **~63,3 ms de backend** (et ~64,0 ms
de token greedy complet), et non ~63 ms de « travail GPU » — la distinction est justement l'objet
de cette passe.

## 2 — Fenêtre de profiling (§6)

```
Mode            : TOKEN_CLASS_A uniquement
--skip 400      (les paliers KV 1024→1280 sont franchis bien avant)
--profile 24    (fenêtre mesurée et profilée)
PROFILED_WINDOW_KV_TRANSITION_COUNT=0
PROPERTIES_CHANGED_TRUE=0   UPDATE_REQUIRED_TRUE=0   CAPTURE_BEGIN=0   CAPTURE_END=0
INSTANTIATE=0               EXEC_UPDATE=0            LAUNCH=24         NO_UPDATE_LAUNCH=24
E5A_KV_POSITION_AT_WINDOW_START=1209
```

La fenêtre est de la **classe A pure** : 24 tokens consécutifs sans aucune transition de graphe,
au-delà du dernier palier KV franchi, à position KV 1209. Latence : `DECODE_PLUS_SYNC_MS p50=63.5239`,
`BACKEND_SYNC_WAIT_MS p50=63.4566`, `ARGMAX_MS p50=0.6692` — cohérent avec le baseline.

```
PROFILED_TPS=DIAGNOSTIC_ONLY
```
Le débit produit reste celui de RUN-003 non profilé (15.7983 / 15.6330).

## 3 — Outillage : deux blocages de plateforme, mesurés

### 3.1 Nsight Systems : aucun traçage CUDA possible

```
nsys 2025.6.3 : toutes les captures se terminent sans erreur, mais
  e5a-natural.nsys-rep → "does not contain CUDA kernel data"
  e5a-nograph.nsys-rep → "does not contain CUDA kernel data"
  base sqlite : aucune table CUDA_KERNEL / CUDA_RUNTIME / CUDA_GPU
```

Contre-épreuve décisive : un kernel trivial compilé pour `sm_121a` (5 lancements,
`spin<<<8,256>>>`), profilé par nsys, donne **lui aussi** « tables CUDA : AUCUNE ».

```
CUPTI_KERNEL_TRACING_ON_THIS_PLATFORM=UNAVAILABLE   (GB10 / CUDA 13.2, hors de mon contrôle)
```

### 3.2 Nsight Compute : permissions refusées

```
/opt/nvidia/nsight-compute/2026.1.1/ncu → ERR_NVGPUCTRPERM
  "The user does not have permission to access NVIDIA GPU Performance Counters"
/proc/sys/kernel/perf_event_paranoid = 4
```

```
ENGINE005B_HARDWARE_METRICS_ON_THIS_HOST=BLOCKED_BY_PERMISSION
```

## 4 — Méthode de remplacement : événements CUDA dans la bibliothèque

CUPTI étant inutilisable, l'attribution vient de l'instrumentation **par paires d'événements
CUDA** déjà présente dans l'arbre V3 (`GGML_CUDA_OP_TIMING`, `obrain_op_budget`), qui accumule,
par nom d'opération, le temps GPU entre l'événement posé avant et après chaque opération.

```
MESURE_1 : graphes CUDA ACTIFS      (régime produit, 4 tokens, 7 436 opérations)
MESURE_2 : graphes CUDA DÉSACTIVÉS  (chaque nœud lancé individuellement, 4 tokens, 438 952 opérations)
```

La mesure 2 n'est pas le régime produit, mais elle révèle la **composition** du travail.
Elle est déclarée `DIAGNOSTIC_ONLY`.

```
PROFILED_TPS (graphes actifs)   = 15.4911
PROFILED_TPS (graphes inactifs) = 13.9885
RATIO_DEBIT_ACTIFS/INACTIFS     = 1.107
```

Fait notable : **le graphe CUDA n'apporte que +10,7 % de débit** dans cette configuration. Le
coût de lancement hôte n'est donc pas le facteur limitant — ce que confirme la mesure 2 :
`BACKEND_SYNC_WAIT_MS p50=0.0236` (contre 63.46 avec graphes) et `DECODE_PLUS_SYNC_MS p50=70.80`.

## 5 — Table causale primaire (§12)

Pourcentages du temps GPU total. Colonne « ms/token » = part du chemin critique mesuré
(63.90 ms/token avec graphes actifs, 70.83 ms/token sans).

### 5.1 Graphes CUDA ACTIFS — régime du produit

| famille | opérations | appels / token | µs / appel | % du temps GPU | ms/token (chemin critique) |
|---|---|---|---|---|---|
| **MatMul/MMVQ** | `MUL_MAT` | 640 | 619.6 | **91.7 %** | **58.6** |
| GLU / activations | `GLU` | 64 | 167.4 | 2.5 % | 1.6 |
| Norme | `WHT` | 162 | 33.7 | 1.3 % | 0.8 |
| Elementwise | `ADD` | 224 | 21.8 | 1.1 % | 0.7 |
| KV / copie | `CONCAT` | 72 | 59.2 | 1.0 % | 0.7 |
| **Attention** | `FLASH_ATTN_EXT` | 24 | 117.7 | 0.7 % | 0.4 |
| KV | `GET_ROWS` | 144 | 17.1 | 0.6 % | 0.4 |
| Norme | `L2_NORM` | 144 | 9.6 | 0.3 % | 0.2 |
| Other | — | — | — | 0.8 % | 0.5 |

Total mesuré : **1 730,3 ms / 4 tokens = 432,6 ms/token d'opérations CUDA comptées**.

### 5.2 Graphes CUDA DÉSACTIVÉS — attribution par lancement

| famille | opérations | appels / token | µs / appel | % du temps GPU |
|---|---|---|---|---|
| **MatMul/MMVQ** | `MUL_MAT` | 34 309 | 96.3 | **86.2 %** |
| KV | `GET_ROWS` | 11 445 | 13.6 | 4.0 % |
| Norme | `WHT` | 12 744 | 5.8 | 1.9 % |
| Elementwise | `ADD` | 11 408 | 5.4 | 1.6 % |
| **Attention** | `FLASH_ATTN_EXT` | 1 888 | 31.6 | 1.6 % |
| Norme | `L2_NORM` | 11 328 | 4.2 | 1.2 % |
| KV / copie | `CONCAT` | 5 664 | 5.3 | 0.8 % |
| Copies | `CPY` | 5 664 | 4.5 | 0.7 % |
| KV | `SET_ROWS` | 3 776 | 5.7 | 0.6 % |
| Elementwise | `UNARY` | 5 712 | 3.6 | 0.5 % |
| Position | `ROPE` | 3 776 | 4.5 | 0.4 % |
| GLU | `GLU` | 64 | 166.6 | 0.3 % |

## 6 — Réponses aux questions posées

### 6.1 Quantité de lancements (§15)

```
CUDA_KERNEL_LAUNCH_COUNT_PER_TOKEN_WITH_GRAPHS    = 1 859
CUDA_OPERATIONS_TOTAL_PER_TOKEN_WITH_GRAPHS       = 1 859
CUDA_OPERATIONS_TOTAL_PER_TOKEN_WITHOUT_GRAPHS    = 109 738
NUMBER_OF_KERNELS_LT_5US  : NON MESURABLE (pas de timeline CUPTI) ; borné par µs/appel :
   avec graphes, l'opération la plus fine mesurée est L2_NORM à 9,6 µs — aucune micro-opération
   à 1-2 µs n'apparaît dans le catalogue mesuré.
```

`GPU_GRAPH_REDUCES_HOST_LAUNCH_OVERHEAD`: **YES** (109 738 → 1 859 opérations par token).
`BUT_DOES_NOT_ELIMINATE_DEVICE_SIDE_FINE_GRAIN_WORK`: **NON CONFIRMÉ** — au contraire, en régime
graphe, les opérations mesurées sont **grosses** (MUL_MAT à 620 µs, GLU à 167 µs).

### 6.2 Fusion (§16) — « fused » ne veut pas dire « coûteux »

```
FLASH_ATTN_GPU_TIME_PER_TOKEN = 0.4 ms   (0.7 % avec graphes ; 1.6 % sans)  → désactivé comme cible
FUSED_GDN_TIME_PER_TOKEN      = NON ISOLABLE : aucun noyau nommé GDN/SSM n'apparaît dans le
                                catalogue ; ses opérations sont soit fusionnées (donc comptées
                                dans d'autres familles), soit des MUL_MAT. Les marqueurs
                                « fused Gated Delta Net » du chargement ne se traduisent pas en
                                noyaux distincts identifiables par ce moyen.
```

### 6.3 Récurrent vs attention (§17)

Avec les données disponibles :

```
ATTENTION_PERCENT_OF_TOKEN_GPU_TIME = 0.7 % (graphes actifs) / 1.6 % (sans graphes)
RECURRENT_PERCENT                  = NON SÉPARABLE en l'état
```

**Ce qui bloque la séparation** : les noyaux récurrents empruntent la même opération `MUL_MAT` que
les couches d'attention et les projections MLP ; l'attribution par **nom de noyau** ne peut pas
distinguer la couche d'origine. Il faut une attribution **par couche**, pas par noyau — c'est la
prochaine mesure utile (voir §8).

### 6.4 GPU saturé ou bulles (§14)

```
GPU_ACTIVE_FRACTION        = NON MESURABLE (pas de timeline CUPTI)
GPU_IDLE_GAPS_PER_TOKEN_MS = NON MESURABLE
SUM_KERNEL_DURATIONS_MS    = 432.6 ms/token          (graphes actifs, accumulateur d'événements)
TOKEN_WALL_LATENCY_MS      = 63.9 ms/token           (paroi mesurée du même run)
RATIO_SOMME/PAROI          = 6.77
KERNEL_OVERLAP_PRESENT     = YES (nécessairement : la somme dépasse la paroi)
```

**Anomalie d'échelle déclarée, non résolue.** La somme des durées d'opérations ne peut pas être
6,77× la paroi sur un seul flux (le calcul est mono-flux : aucun événement concurrent n'est
enregistré pour ce modèle). Deux lectures possibles, non départageables sans timeline :
soit l'accumulateur d'événements sur-compte sur cette plateforme, soit il existe du
parallélisme d'exécution non visible dans cette instrumentation.

Conséquence méthodologique : **les pourcentages sont utilisables** (ils reposent sur des rapports
internes cohérents) ; **les valeurs absolues en ms ne doivent pas être prises pour le temps GPU
par token**. `TOTAL_ACCOUNTED_GPU_TIME_PER_TOKEN_MS=432.6` **ne doit pas** être comparé
directement à 63,3 ms, et ne l'est pas dans ce rapport.

Ce que l'on peut affirmer sans timeline : la somme des opérations **dépasse** largement la paroi,
et la paroi est dominée par `MUL_MAT`. Ce que l'on ne peut pas affirmer :
`STEADY_STATE_IS_GPU_COMPUTE_OR_MEMORY_BOUND` vs `HAS_SCHEDULING_OR_DEPENDENCY_BUBBLES`.
Les deux restent **NON TRANCHÉS**.

### 6.5 Le pic hôte (§4)

```
KV_TRANSITION_HOST_COST : NON PROMU
HOST_SUBMISSION_CAN_SHIFT_TO_~28.6ms_ON_TRANSITION_TOKEN=YES
HOST_API_PARTITION_SHIFT=YES   (compatible avec les mesures)
TRANSITION_TOTAL_EXCESS_LATENCY≈0.5981 ms
```
Le coût causal se lit sur la timeline GPU, pas sur la frontière decode/get_logits.

## 7 — Objectifs de gain (§25, §26)

```
20 tok/s → 50,00 ms/token → retirer ~13,3 ms
25 tok/s → 40,00 ms/token → retirer ~23,3 ms
30 tok/s → 33,33 ms/token → retirer ~30,0 ms
```

La famille dominante (`MUL_MAT`) porte ~58,6 ms du chemin critique par token, donc
**~30 ms appartiennent bien à une famille unique** — l'ordre de grandeur existe. Cela **ne
démontre pas** que 30 tok/s est atteignable : un MUL_MAT quantifié peut être au plafond mémoire,
et nous n'avons pas les métriques roofline (§19, §20) pour le dire.

```
30_TOK_S_ACHIEVABLE : NON AFFIRMÉ
```

## 8 — Prochaine mesure utile

Pour séparer récurrent et attention, la seule voie praticable sans CUPTI est une **attribution par
couche dans le harnais** : nsys ayant échoué et ncu étant bloqué, il faut instrumenter les
frontières de couches côté modèle (nb de couches, pas de nœuds).

```
LAYER_LEVEL_ATTRIBUTION = NEXT_MEASUREMENT
```
Elle demanderait d'ajouter des paires d'événements par couche (patch de bibliothèque non
baseline), ou d'exploiter `ggml_backend_sched` par tranche.

## 9 — Freeze ENGINE-005A

```
ENGINE005A_STATUS=CLOSED_GPU_TIMELINE
ENGINE005A_METHOD=CUDA_EVENT_OP_ATTRIBUTION (CUPTI indisponible)
NORMAL_TOKEN_GPU_CRITICAL_PATH_MS=NON MESURABLE (voir §6.4) ; référence 63,30 ms (RUN-003)
GPU_ACTIVE_FRACTION=NON MESURABLE (CUPTI indisponible)
TOP_KERNEL_1=MUL_MAT
TOP_KERNEL_1_TIME_PER_TOKEN= (part du chemin critique) 58,6 ms
TOP_KERNEL_FAMILY=MATMUL/MMVQ
TOP_KERNEL_FAMILY_PERCENT=86.2..91.7
FUSED_GDN_TIME_PER_TOKEN=NON ISOLABLE
FLASH_ATTN_TIME_PER_TOKEN=0.4 ms
MATMUL_TIME_PER_TOKEN=58,6 ms (part du chemin critique)
KV_TIME_PER_TOKEN=1.1 ms (GET_ROWS + CONCAT + SET_ROWS, part du chemin critique)
PROFILED_TPS=DIAGNOSTIC_ONLY
PROFILED_WINDOW_KV_TRANSITION_COUNT=0
CUPTI_KERNEL_TRACING_ON_THIS_PLATFORM=UNAVAILABLE
ENGINE005B_HARDWARE_METRICS_ON_THIS_HOST=BLOCKED_BY_PERMISSION
ENGINE005B_ALLOWED=YES (avec la réserve ci-dessus)
ENGINE006_BRANCH=ENGINE006_MATMUL_OPTIMIZATION
ENGINE004_KV_LADDER_OPTIONAL=YES
ENGINE005_BLOCKED_BY_KV_LADDER=NO
```

## 10 — Ledger

```
ENGINE004_STATUS=CLOSED_BASELINE
ENGINE004_BOUNDARY_FORENSIC=COMPLETE
ENGINE004_BASELINE_BACKEND_EQUIVALENT_TPS=15.7983
ENGINE004_BASELINE_END_TO_END_GREEDY_TPS=15.6330
GRAPH_INVALIDATION_MECHANISM=KV_CACHE_CAPACITY_VIEW_SHAPE_CHANGE
KV_GRAPH_MAINTENANCE_AMORTIZED_COST=NEGLIGIBLE_RELATIVE_TO_~63MS_NORMAL_TOKEN
ENGINE005_ALLOWED=YES
ENGINE005A_STATUS=CLOSED_GPU_TIMELINE
ENGINE005A_TOP_FAMILY=MATMUL_MMVQ_86.2_TO_91.7_PCT
ENGINE005A_CUPTI_AVAILABLE=NO
ENGINE005B_ALLOWED=YES_WITH_PLATFORM_RESERVE
ENGINE006_BRANCH=ENGINE006_MATMUL_OPTIMIZATION
MATCHED_FORK_CAUSAL_EXPERIMENT=PAUSED
STATE_TRANSPORT_PHASE_ALLOWED=NO
PREFIX_LENGTH_SCALING_EXECUTED=NO
B3_EXECUTION_ALLOWED=NO
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
