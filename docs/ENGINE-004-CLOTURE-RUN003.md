# ENGINE-004 — CLÔTURE DU FORENSIC A″ ET BASELINE RUN-003

Date : 2026-09-29. Machine : `spark-c578` (DGX, GB10, CUDA 13.2).
Artefacts : `/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/`
Run baseline canonique : `qualification/run003/`.

---

## 1 — Verdict causal A″ (promu)

```
ENGINE004_BOUNDARY_FORENSIC=COMPLETE
NATURAL_LONG_RUN_GRAPH_TRANSITIONS_OBSERVED=YES
RUN2_CHANGED_GRAPH_PROPERTY=KV_CACHE_VIEW_SHAPE
RUN2_CHANGED_GRAPH_NODE=cache_k_l3
RUN2_CHANGED_GRAPH_NODE_INDEX=213/4010
KV_CACHE_VIEW_NE=[256,4,1024,1] -> [256,4,1280,1]
KV_CACHE_VIEW_BYTES=2097152 -> 2621440
KV_CACHE_VIEW_BYTE_DELTA=524288
KV_CACHE_VIEW_STRIDES_CHANGED=NO
KV_CACHE_VIEW_DATA_POINTER_CHANGED=NO
CUDA_GRAPH_UID=36 -> 39
GRAPH_INVALIDATION_MECHANISM=KV_CACHE_CAPACITY_VIEW_SHAPE_CHANGE
```

## 2 — Ancienne hypothèse retirée

```
MEASUREMENT_BOUNDARY_CAUSAL=NO
NATURAL_KV_CAPACITY_TRANSITION_CAUSAL=YES
MESURE_WINDOW_TRANSITION_INVALIDATES_GRAPH_STATE : hypothèse ABANDONNÉE
```

Soutien indépendant : A/B/C = NO sur le token de test (le protocole de mesure n'est pas la
cause), et A″ reproduit la transition au cours du long run naturel **sans aucun changement de
protocole**.

## 3 — Contradiction compteur/trace : résolue

```
CUDA_COUNTER_TRANSITION_COUNT=1
OBSERVER_RING_TRANSITION_COUNT=1
DEBUG_TRACE_WARMUP_CYCLE_COUNT=2
DEBUG_TRACE_GRAPH_ID_COUNT=2
COUNTER_TRACE_CONSISTENCY=PASS
COUNTER_TRACE_CONTRADICTION=RESOLVED
WARMUP_CYCLE_1=PREFIX/PREWINDOW
WARMUP_CYCLE_2=KV_CAPACITY_TRANSITION_DURING_GENERATION
```

Deux épisodes distincts : cycle 1 dans le préfixe (avant l'ordinal de fenêtre), cycle 2 à
l'ordinal 224 (dans la fenêtre). Le compteur n'en voyait qu'un parce qu'il était remis à zéro
juste avant la fenêtre.

## 4 — A′ corrigé rétroactivement

```
APRIME_MEASURED_WINDOW_GRAPH_TRANSITION_EVENT_COUNT=1   (remplace le 0, qui était FAUX)
APRIME_OBSERVED_GRAPH_TRANSITION_RATE=1/256 = 0.00390625 event/token   (mesure de CE run)
GRAPH_TRANSITION_PERIOD_ALWAYS_256_TOKENS : NON promu (périodicité non établie)
```

## 5 — Steady-state redéfini

```
STEADY_STATE = LONG_RUN_STATISTICALLY_STABLE_AUTOREGRESSIVE_REGIME
               INCLUDING_NORMAL_AMORTIZED_KV_CAPACITY_TRANSITIONS
```

Une croissance KV normale n'invalide plus automatiquement le baseline.

## 6 — Séquence causale observée

```
normal no-update reuse
  -> KV cache visible extent reaches capacity boundary
  -> cache_k_l3 view shape changes
  -> graph properties differ
  -> cgraph uid changes 36 -> 39
  -> CUDA Graph maintenance/capture path
  -> subsequent no-update reuse
```

`UPDATE_REQUIRED_TRUE` **n'est pas** `cudaGraphExecUpdate executed` : le run observé reste
`EXEC_UPDATE=0` (l'update requis a été absorbé par la voie capture/réutilisation, sans appel
d'update exécutable).

## 7 — Le champ `OTHER`

```
EVENT_0_FIRST_CHANGED_FIELD=OTHER              (résultat historique, conservé)
EVENT_0_CHANGED_METADATA=VIEW_SHAPE_NE         (classification analytique)
EVENT_0_PRIMARY_CHANGED_DIMENSION=ne[2]
```

Aucun rerun A″ pour renommer ce champ. Pour une version future de l'observateur, ajouter
`VIEW_SHAPE` et `VIEW_BYTE_EXTENT` à l'énumération.

## 8 — Provenance observateur (inchangée)

```
OBSERVER_RUNTIME_CLASS=FORENSIC_ONLY_NONBASELINE_RUNTIME
OBSERVER_CONTAINS_ENGINE003_V3_COUNTER_PATCH=NO
OBSERVER_LIBGGML_CUDA_SHA256=885c02392d0738d8d683583c4d0b9bf1c13f4b5688e320e32464885f75d882a1
A2_TOKEN_SEQUENCE_MATCH_APRIME=YES
OBSERVER_OVERHEAD_VS_APRIME_PCT=+1.35
```

Aucun chiffre de débit A″ n'entre dans le baseline.

## 9 — Incidents (consignés séparément, hors PLUMBING)

```
PROVENANCE_INCIDENT_1=COPIED_CMAKE_BUILD_DIR_REFERENCED_ORIGINAL_BINARY_TREE   STATUS=RESOLVED  AFFECTS_PROMOTED_RESULT=NO
PROVENANCE_INCIDENT_2=STALE_LIBRARY_REUSED_AFTER_FAILED_BUILD                  STATUS=RESOLVED  AFFECTS_PROMOTED_RESULT=NO
OBSERVER_IMPLEMENTATION_DEFECT_1=CHANGED_FIELD_NOT_PROPAGATED_TO_EVENT         STATUS=RESOLVED  AFFECTS_PROMOTED_RESULT=NO
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```

---

## 10 — RUN-003 : exécution

```
ENGINE004_RUN3_ALLOWED=YES
RUN003_MODE=NATURAL_LONG_RUN_STATIONARY
RUN003_RUNTIME=ENGINE003_FROZEN_V3_RUNTIME
libllama     = 3db83675270ae68aa646aab65290fd3fb22a0e7491eaf9f49e58e8d52f74a802   (vérifié au run)
libggml-cuda = abafbf0dfe4f39d661401e0b481c7f90de547fa76a3647f51d2be9af790a7758   (vérifié au run)
OBSERVATEUR : INTERDIT pour le baseline — aucune trace d'observateur dans les DSO servies.
```

### 10.1 Logging par token (§11) : coupé **sans repatcher les DSO**

Le callback de log par défaut de ggml écrit tout sur stderr sans filtrer par niveau. Le harnais
installe donc son propre filtre via l'API publique `llama_log_set` / `ggml_log_set` :

```
PER_TOKEN_LIBRARY_TEXT_LOGGING=0
DO_NOT_PATCH_FOR_RUN003            (respecté : DSOs intactes)
LOG_FILTER_DEBUG_DROPPED=1452      (dont 0 ligne « CUDA Graph id … reused »)
LOG_FILTER_KEPT=179                (WARN/ERROR du chargement, conservés)
stderr = 179 lignes (contre 1544 lignes en A′ non filtré)
```

Le filtre vit **dans le harnais**, pas dans le runtime : la sémantique du baseline reste celle du
runtime qualifié.

### 10.2 Fidélité

```
TOKEN_SEQUENCE_VALID=YES
RUN003_OUTPUT_TOKEN_SEQUENCE_SHA256=210b7eb55ff64a10c1d51af5391edc21b8d4545d297817c332d5635842590231
```

Identique à RUN-002, A′ et A″ : même trajectoire, runtime gelé.

### 10.3 Compteurs (télémétrie V3, autorité)

```
PROPERTIES_CHANGED_TRUE=1      UPDATE_REQUIRED_TRUE=1
CAPTURE_BEGIN=1                CAPTURE_END=1        (invariant respecté)
INSTANTIATE=0                  EXEC_UPDATE=0
LAUNCH=255                     NO_UPDATE_LAUNCH=254
PREROLL_CUDA[cb=1 ce=1 inst=1 upd=0 pc=1 urT=1 urF=3 lch=3 nol=2]   (préfixe)
WINDOW_LLAMA[miu=0.0170 gb=0.4744 si=0.4340 gc=49.1656]
```

L'événement de capacité KV **est** présent dans la fenêtre et `DOES_NOT_INVALIDATE_BASELINE=YES`.

### 10.4 Frontières temporelles et distributions (§12, §13)

```
HOST_DECODE_SUBMISSION_MS  p50=0.0665 p90=0.0715 p95=0.0810 p99=0.1266 mean=0.2047 sd=1.8204 min=0.0597 max=28.5865
BACKEND_SYNC_WAIT_MS       p50=63.2823 p90=63.9251 p95=64.0623 p99=64.5525 mean=63.0931 sd=1.7748 min=37.0916 max=67.0050
DECODE_PLUS_SYNC_MS        p50=63.3495 p90=63.9936 p95=64.1622 p99=65.5894 mean=63.2978 sd=0.8522 min=61.6537 max=70.5659
ARGMAX_MS                  p50=0.6692 p90=0.6703 p95=0.6709 p99=0.6831 mean=0.6696 sd=0.0036 min=0.6677 max=0.7073
GREEDY_TOKEN_MS            p50=64.0247 p90=64.6683 p95=64.8303 p99=66.2579 mean=63.9674 sd=0.8523 min=62.3219 max=71.2342

GREEDY_CONTINUOUS_WINDOW_MS=16375.6633
BACKEND_ACTIVE_INTERVAL_SUM_MS=16204.2400
```

### 10.5 Endpoints baseline

```
ENGINE004_BASELINE_BACKEND_EQUIVALENT_TPS=15.7983
ENGINE004_BASELINE_END_TO_END_GREEDY_TPS=15.6330
```

## 11 — Coût amorti de la maintenance KV (§16)

Le token de transition (ordinal 224, 216ᵉ token de la fenêtre) et son voisinage immédiat :

| mesure | valeur |
|---|---|
| `TRANSITION_TOKEN_DECODE_PLUS_SYNC_MS` (A″, token 214) | 64.2079 |
| `LOCAL_NO_UPDATE_BASELINE_MS` (médiane 32 tokens voisins) | 63.6098 |
| `TRANSITION_EXCESS_LATENCY_MS` | **0.5981** |
| `AMORTIZED_GRAPH_MAINTENANCE_COST_PER_TOKEN_MS` | **0.002336** |
| part du coût total de la fenêtre | 0.0037 % |

Mesure indépendante sur RUN-003 (runtime gelé), token le plus lent de la fenêtre :

| mesure | valeur |
|---|---|
| token le plus lent | 216, `DECODE_PLUS_SYNC_MS` = 70.566 |
| `LOCAL_NO_UPDATE_BASELINE_MS` | 63.4629 |
| `TRANSITION_EXCESS_LATENCY_MS` | **7.1030** |
| `AMORTIZED_GRAPH_MAINTENANCE_COST_PER_TOKEN_MS` | **0.027746** |
| `HOST_DECODE_SUBMISSION_MS` du token 216 | 6.374 (le max de la fenêtre est 28.587, sur le token suivant) |

Conclusion : **le coût de la maintenance de graphe est négligeable pour le produit**
(≤ 0.028 ms/token amorti, ≤ 0.04 % du budget par token). La perturbation visible après la
transition se manifeste surtout côté **soumission hôte** (jusqu'à 28.6 ms sur un token), pas dans
la charge backend.

`KV_CAPACITY_GROWTH_QUANTUM=256` et `GRAPH_MAINTENANCE_PERIOD_AFTER_THRESHOLD≈256_tokens` : **NON
promus** — un seul palier observé. Le ladder KV (1024→1280→1536→1792) reste à faire (§17).

## 12 — Freeze ENGINE-004

```
TOKEN_SEQUENCE_VALID=YES
RUNTIME_HASHES_MATCH_FROZEN_V3=YES
HARNESS_CPP_DEFECT_4_STATUS=RESOLVED
HARNESS_CPP_DEFECT_5_STATUS=RESOLVED
DISTRIBUTIONS_STABLE=YES   (p50 identiques à ±0.4 ms sur RUN-001/002/A′/A″/RUN-003)

ENGINE004_BASELINE_BACKEND_EQUIVALENT_TPS=15.7983
ENGINE004_BASELINE_END_TO_END_GREEDY_TPS=15.6330
ENGINE004_BASELINE_SEQUENCE_SHA256=210b7eb55ff64a10c1d51af5391edc21b8d4545d297817c332d5635842590231
BACKEND_SYNC_WAIT_P50=63.2823
BACKEND_SYNC_WAIT_P95=64.0623
HOST_DECODE_SUBMISSION_P50=0.0665
ARGMAX_P50=0.6692
GRAPH_TRANSITION_EVENT_COUNT=1
GRAPH_TRANSITION_EVENT_RATE=0.00390625
KV_CAPACITY_TRANSITION_MECHANISM=cache_k_l3 shape growth

ENGINE004_STATUS=CLOSED_BASELINE
ENGINE005_ALLOWED=YES
```

## 13 — Pour ENGINE-005

Question principale inchangée : `WHAT_GPU_WORK_OCCUPIES_THE_~64_MS_PER_TOKEN?`
Le budget par token mesuré : **63.30 ms de backend** (attente de synchronisation) contre
**0.067 ms de soumission hôte** et **0.669 ms d'argmax**.

Deux populations à ne pas mélanger dans le profil kernel :

```
NORMAL_NO_UPDATE_TOKEN_GPU_WORK        (254/256 tokens) -> détermine le plafond ~15.7 tok/s
KV_CAPACITY_TRANSITION_TOKEN_GPU_WORK  (1/256 tokens)  -> coût rare, amorti (§11)
```

## 14 — Ledger

```
MODEL_RUN_COUNT=93   (87 précédents + A/B/C 1 + A′ 1 + A″ 5 + RUN-003 1, runs forensiques et
                      baseline comptés séparément : baseline=88, forensiques=5)
ENGINE004_BOUNDARY_FORENSIC=COMPLETE
A_MICROTEST=NO_TRANSITION
B_MICROTEST=NO_TRANSITION
C_MICROTEST=NO_TRANSITION
NATURAL_LONG_RUN_GRAPH_TRANSITIONS_OBSERVED=YES
RUN2_CHANGED_GRAPH_PROPERTY=KV_CACHE_VIEW_SHAPE
BOUNDARY_TRANSITION_ROOT_CAUSE=KV_CACHE_CAPACITY_VIEW_SHAPE_CHANGE
COUNTER_TRACE_CONSISTENCY=PASS
COUNTER_TRACE_CONTRADICTION=RESOLVED
MEASUREMENT_BOUNDARY_CAUSAL=NO
NATURAL_KV_CAPACITY_TRANSITION_CAUSAL=YES
APRIME_MEASURED_WINDOW_GRAPH_TRANSITION_EVENT_COUNT=1
APRIME_OBSERVED_GRAPH_TRANSITION_RATE=0.00390625
ENGINE004_RUN3_ALLOWED=YES
RUN003_MODE=NATURAL_LONG_RUN_STATIONARY
RUN003_COMPLETED=YES
RUN003_RUNTIME=ENGINE003_FROZEN_V3_RUNTIME
PER_TOKEN_LIBRARY_TEXT_LOGGING=0
ENGINE004_BASELINE_BACKEND_EQUIVALENT_TPS=15.7983
ENGINE004_BASELINE_END_TO_END_GREEDY_TPS=15.6330
AMORTIZED_GRAPH_MAINTENANCE_COST_PER_TOKEN_MS=0.002336 (token de transition) / 0.027746 (pic observé)
ENGINE004_STATUS=CLOSED_BASELINE
ENGINE005_ALLOWED=YES
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```

Note de provenance : le lien de `run003` résout `libggml.so.0` et `libggml-cpu.so.0` depuis le
répertoire de build engine-003 plutôt que depuis la copie gelée ; vérification faite,
**les deux fichiers sont identiques** (même SHA-256) — aucune divergence de runtime.
