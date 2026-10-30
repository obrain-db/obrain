# ENGINE-005I VOLET 2 — IDEMPOTENCE DE SOURCE ET POPULATION CIBLE

Date : 2026-09-29. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/qualification/e5i/`

---

## 0 — Résultat de la passe

Deux livrables, deux réponses :

1. **Gate d'idempotence : FRANCHIE par la source.** `DUPLICATE_WORK_EXPERIMENT_ALLOWED=YES` —
   les portes A à F sont satisfaites, chacune par une preuve de code (§1).
2. **Population cible : 36 poids, pas 48** — et le compte est désormais expliqué (§2). C'est une
   correction importante pour le périmètre du futur `ENGINE006_TARGET`.

La campagne R0/R1/R2 n'a **pas** été exécutée : elle est spécifiée et prête (§4), mais je ne
publierai pas de chiffres de sensibilité murale que je n'ai pas mesurés.

```
TARGET_MMVQ_REEXECUTION_SEMANTICALLY_IDEMPOTENT=SOURCE_PROVEN
DUPLICATE_WORK_EXPERIMENT_ALLOWED=YES
TARGET_WEIGHT_POPULATION=36 (tq2_g128) sur 48 recurrent attn_gate
TARGET_MMVQ_CAUSAL_WALL_SENSITIVITY=NOT_MEASURED
ENGINE006_TARGET=NOT_PINNED
ENGINE006_KERNEL_MODIFICATION_ALLOWED=NO
```

---

## 1 — Revue de source du chemin réel (§3 à §10)

### Chemin exact pour `qtype=tq2_g128`, `s1=1`

```
ggml_cuda_mul_mat
  └─ ggml_cuda_should_use_mmvq(tq2_g128, cc, ne11=1) == true   (domaine de capacite 005H)
     └─ ggml_cuda_mul_mat_vec_q                (mmvq.cu)
        ├─ get_vec_dot_q_cuda : case GGML_TYPE_TQ2_G128 -> vec_dot_tq2_g128_q8_1
        ├─ ggml_cuda_pool_alloc<char> src1_q8_1   -> quantize_row_q8_1_cuda(src1)   [kernel 1]
        └─ kernel MMVQ                             [kernel 2]
```

Séquence native = **2 kernels** : quantification des activations en Q8_1, puis le noyau de produit
scalaire vectoriel. Le tableau de paramètres par type est
`VDR_TQ2_G128_Q8_1_MMVQ` (« une tranche de 32 par appel », dit la source).

### Gate A — destination (§4)

`TARGET_KERNEL_READS_DST=NO`. Preuves : les seules écritures sont

```cpp
// mmvq.cu:702   (chemin fusionne)
dst[j*stride_col_dst + i] = result;
// mmvq.cu:781   (chemin non fusionne, c'est le notre)
dst[channel_dst*stride_channel_dst + token_idx*stride_col_dst + row0 + threadIdx.x] = tmp[threadIdx.x];
```

Recherche explicite des motifs interdits dans tout `mmvq.cu` :

```
dst[...] +=            : 0 occurrence
atomicAdd(dst, ...)    : 0 occurrence
beta * dst             : 0 occurrence
```

L'accumulation vit dans `tmp[...]` (registres) puis dans `warp_reduce_sum`. **Aucune lecture de
`dst`.** ⇒ `TARGET_KERNEL_OUTPUT_MODE=PURE_OVERWRITE`.

### Gate B — couverture du `dst` (§5)

`ALL_CONSUMED_DST_ELEMENTS_OVERWRITTEN_EACH_INVOCATION=YES`. La grille est
`dim3 block_nums(nblocks, nchannels_dst, nsamples_or_ntokens)` avec `row0 = rows_per_cuda_block *
blockIdx.x`, et l'écriture est gardée par
`threadIdx.x < c_rows_per_block && (c_rows_per_block == 1 || row0 + threadIdx.x < nrows_x)`.
Chaque ligne de `dst` est donc écrite **intégralement** à chaque invocation, sans résidu possible.

### Gate C — unicité des écrivains (§6)

`row0` est une fonction **injective** de `blockIdx.x` : les tranches de lignes de blocs distincts
sont disjointes. Aucun `atomicAdd`, aucune accumulation inter-blocs. La réduction est
**entièrement interne** au bloc (warp-level) avant l'écriture finale unique.
⇒ `PURE_OVERWRITE=SUPPORTED`.

### Gate D — temporaires (§7)

`ggml_cuda_pool_alloc<char> src1_q8_1(...)` est alloué **par appel** depuis le pool, puis rempli par
`quantize_row_q8_1_cuda(src1)` — une fonction pure de l'entrée. Une seconde invocation le réécrit
**entièrement** avec exactement les mêmes valeurs.
⇒ `TEMPORARIES_ARE_REINITIALIZED_OR_FULLY_OVERWRITTEN=YES`.

### Gate E — effets de bord (§8)

Un seul `cudaMemsetAsync` existe dans `mmvq.cu` (ligne 1301) et il est **hors de notre chemin** : il
ne s'applique que si `ggml_backend_buffer_get_usage(src0->buffer) == GGML_BACKEND_BUFFER_USAGE_COMPUTE`
(c'est-à-dire `src0` = tampon de calcul temporaire). Ici `src0` est un **poids du modèle** (buffer
model), donc le `memset` ne s'exécute pas. Aucun compteur persistant, aucun état KV, aucun état
récurrent, aucun RNG, aucun canal atomique dans ce chemin.
⇒ `TARGET_PATH_FUNCTIONAL_SIDE_EFFECTS_OUTSIDE_DST=NO`.

### Gate F — ordre du flux (§9)

Le chemin n'insère **aucune synchronisation** entre les deux kernels (pool alloc + 2 lancements sur
le même flux). Les répétitions peuvent être soumises dans l'ordre `native, extra_1, extra_2` sur le
même flux, mêmes arguments.
⇒ `EXTRA_SYNC_PER_REPEAT=0`.

### Verdict (§10)

```
TARGET_KERNEL_READS_DST=NO
TARGET_KERNEL_OUTPUT_MODE=PURE_OVERWRITE
ALL_CONSUMED_DST_ELEMENTS_OVERWRITTEN_EACH_INVOCATION=YES
TEMPORARIES_ARE_REINITIALIZED_OR_FULLY_OVERWRITTEN=YES
TARGET_PATH_FUNCTIONAL_SIDE_EFFECTS_OUTSIDE_DST=NO
EXTRA_SYNC_PER_REPEAT=0
TARGET_MMVQ_REEXECUTION_SEMANTICALLY_IDEMPOTENT=SOURCE_PROVEN
DUPLICATE_WORK_EXPERIMENT_ALLOWED=YES
```

Réserve à porter dans le benchmark : la séquence native complète est de **deux kernels** (§11).
La répétition devra porter sur **la séquence entière** (quantification + MMVQ), sinon la pente ne
représenterait pas le coût du chemin MMVQ réel. C'est un point de conception du futur instrument.

## 2 — Population cible : 36 poids, et pourquoi (§14)

Le « 36 vs 48 » n'était pas une erreur : les 48 couches récurrentes ont bien un `attn_gate`, mais
**deux quantifications différentes**.

```
TOTAL_RECURRENT_ATTN_GATE_WEIGHT_COUNT=48
ATTN_GATE_TQ2_WEIGHT_COUNT=36        (qtype 43, tq2_g128)
ATTN_GATE_OTHER_QTYPE_WEIGHT_COUNT=12 (qtype 12, q4_K)
SOMME=48  → conforme au modele
```

La répartition est **structurelle**, pas accidentelle :

```
couches recurrent dont attn_gate est q4_K (12)  : [0,1,2,4,5,6]  et  [56,57,58,60,61,62]
couches recurrent dont attn_gate est tq2_g128 (36): [8..54] (les 36 couches intermediaires)
```

Autrement dit : les **6 premières et 6 dernières** couches récurrentes gardent un `attn_gate` en
q4_K ; les 36 couches intermédiaires sont en `tq2_g128`. Cohérent avec une stratégie de
quantification qui protège les couches de bord.

**Conséquence directe pour `ENGINE006_TARGET`** : la signature cible existe sur **36 poids**, pas 48.
Le périmètre du forçage ou de l'optimisation doit être « role=attn_gate AND qtype=tq2_g128 », ce qui
désigne exactement ces 36 couches — et `NON_TARGET_EXTRA_MMVQ_EXECUTIONS=0` se vérifiera sur les 12
autres poids `attn_gate` **plus** tous les autres rôles.

Relevés : `qualification/e5i/signatures-graph-on.err` et `-graph-off.err` donnent les 48 poids avec
leur qtype ; les deux ensembles sont identiques.

## 3 — Ce qu'il reste à exécuter (§12, §15 à §20)

Instrument à construire (non fait) :

```
GGML_E6_MMVQ_EXTRA_REPEATS=0|1|2      lue au point de selection
condition de cible : src0 == <un des 36 pointeurs attn_gate qtype 43> ET chemin == MMVQ
repetition de la SEQUENCE native complete (quantification + MMVQ), meme dst, meme flux
compteurs : TARGET_NATIVE_MMVQ_SELECTIONS / TARGET_EXTRA_MMVQ_EXECUTIONS / NON_TARGET_EXTRA_MMVQ_EXECUTIONS
```

Ordre de campagne :

1. **preflight numérique** (§12) sur fenêtre courte : R0/R1/R2, comparaison bit à bit de l'état
   final et des logits de sonde. Si non-égalité bit à bit ⇒ `DUPLICATE_WORK_EXPERIMENT_VALID=NO`
   jusqu'à explication.
2. **campagne 256 tokens**, `CUDA_GRAPH=ON`, ordre fixé avant observation `R0 R1 R2 R2 R1 R0`,
   un processus propre par run, endpoints RUN-003 (`BACKEND_ACTIVE_INTERVAL_SUM_MS`,
   `BACKEND_EQUIVALENT_TPS`, `MS_PER_TOKEN`).
3. **pente** : régression `wall_ms_per_token = intercept + slope × extra_repeat_count`, puis
   `DELTA_01`, `DELTA_12`, `DUPLICATE_WORK_LINEARITY_RATIO`.

Nom du résultat, quoi qu'il arrive :
`NATIVE_MMVQ_DUPLICATE_WORK_WALL_SENSITIVITY_MS_PER_REPEAT` — **jamais** `ATTN_GATE_NATIVE_WALL_COST`
(cache chaud et répétition immédiate changent le coût marginal, §21).

Si le compteur ne s'incrémente qu'à la construction du graphe, publier en plus une preuve
structurelle du nombre de nœuds supplémentaires réellement capturés (§17).

## 4 — Provenance

```
Runtime baseline   : libllama 3db83675… / libggml-cuda abafbf0d…   (restaurés, SHA vérifiés)
Arbre llama-obrain-cuda : ggml-cuda.cu 960e38d6… , qwen35.cpp f0fd1a3c…   (identiques aux vierges)
Revue de source    : ggml/src/ggml-cuda/mmvq.cu (lecture seule)
Relevés de population : qualification/e5i/signatures-graph-{on,off}.err
AUCUNE MODIFICATION DE SOURCE dans cette passe (revue et comptage uniquement)
```

## 5 — Ledger

```
ENGINE005H_STATUS=CLOSED_DISPATCH_DOMAIN
DISPATCH_DECISION_DOMAIN_GRAPH_MODE_INDEPENDENT=YES
ATTN_GATE_DISPATCH_GRAPH_MODE_INDEPENDENT=YES
SIGNATURES_GRAPH_OFF=1738   SIGNATURES_GRAPH_ON=1738   MAPPING_DIVERGENT=0
ENGINE006_CANDIDATE_SIGNATURE=recurrent_attn_gate.weight/tq2_g128/decode_s1_1/MMVQ
TARGET_MMVQ_REEXECUTION_SEMANTICALLY_IDEMPOTENT=SOURCE_PROVEN
TARGET_KERNEL_READS_DST=NO
TARGET_KERNEL_OUTPUT_MODE=PURE_OVERWRITE
ALL_CONSUMED_DST_ELEMENTS_OVERWRITTEN_EACH_INVOCATION=YES
TEMPORARIES_ARE_REINITIALIZED_OR_FULLY_OVERWRITTEN=YES
TARGET_PATH_FUNCTIONAL_SIDE_EFFECTS_OUTSIDE_DST=NO
EXTRA_SYNC_PER_REPEAT=0
NATIVE_OPERATION_SEQUENCE_KERNEL_COUNT=2   (quantize_q8_1 + MMVQ)
DUPLICATE_WORK_EXPERIMENT_ALLOWED=YES
TOTAL_RECURRENT_ATTN_GATE_WEIGHT_COUNT=48
ATTN_GATE_TQ2_WEIGHT_COUNT=36
ATTN_GATE_OTHER_QTYPE_WEIGHT_COUNT=12
ENGINE005I_VOLET2_STATUS=SOURCE_GATE_PASSED_CAMPAIGN_NOT_RUN
ENGINE006_TARGET=NOT_PINNED
ENGINE006_KERNEL_MODIFICATION_ALLOWED=NO
NEXT_MEASUREMENT=NUMERICAL_PREFLIGHT_THEN_R0_R1_R2_256_TOKENS
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
