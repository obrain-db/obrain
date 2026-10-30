# ENGINE-005G — RUNTIME DISPATCH CENSUS : mécanisme posé, recensement par rôle à finir

Date : 2026-09-29. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/qualification/e5g/`

---

## 0 — État

Le recensement au **vrai point de dispatch** est **posé, compilé et fonctionnel** : il distingue
cinq chemins réels, là où toutes les passes précédentes n'avaient qu'une déduction par la forme.

Mais il n'est pas encore **clé par rôle de poids** : les totaux sont globaux, pas répartis entre
`attn_gate.weight`, `attn_q.weight`, `output.weight`, etc. Les sorties §11 ne sont donc **pas
publiées** — les publier demanderait d'attribuer chaque pointeur de poids à un rôle, ce qui reste à
faire.

```
ENGINE005G_STATUS=DISPATCH_CENSUS_INSTRUMENTED_TOTALS_ONLY
DOMINANT_KERNEL_PATH_IDENTIFIED=NO
DOMINANT_KERNEL_PATH_SOURCE=RUNTIME_DISPATCH_BRANCH_AVAILABLE_BUT_NOT_ROLE_KEYED
ENGINE006_TARGET=NOT_PINNED
ENGINE006_KERNEL_MODIFICATION_ALLOWED=NO
```

---

## 1 — Freeze R2 (§1 à §4)

```
ENGINE005F_R2_STATUS=CLOSED_NULL_WALL_SIGNAL_WITH_INCOMPLETE_ISOLATION
HEAD_ON_MEAN_MS_PER_TOKEN=58.6220     HEAD_ON_SD=0.1463
HEAD_OFF_MEAN_MS_PER_TOKEN=58.6644    HEAD_OFF_SD=0.1144
TARGET_REPLACEMENT_WALL_DELTA_MS_PER_TOKEN=-0.0424
NO_DETECTABLE_WALL_EFFECT_FROM_PROJECTION_TARGET_REPLACEMENT=YES
PROJECTION_ONLY_ISOLATION=NOT_ESTABLISHED
OUTPUT_WEIGHT_WALL_COST=NOT_MEASURED
OUTPUT_WEIGHT_ZERO_COST=NOT_PROVEN
GRAPH_TARGET_REPLACEMENT_CAUSAL_WALL_EFFECT=NOT_DETECTABLE_IN_THIS_CONFIGURATION
GENERALIZATION_TO_ALL_CONTEXT_LENGTHS=NO
GENERALIZATION_TO_NORMAL_GREEDY_OUTPUT_PATH=NO
```

Le -0,0424 ms/token **n'est converti en aucun coût négatif ni nul** de `output.weight`.

Porte d'état : conclusion retirée.

```
STATE_GATE_STATUS=NOT_EVALUABLE_WITH_ENGINE005F_R2_HARNESS
HARNESS_DEFECT_7=PROBE_READS_STALE_LOGITS_WHEN_HEAD_ABSENT_FOR_PROCESS_LIFETIME
HARNESS_DEFECT_7_CLASS=QUALIFICATION_PROBE_SEMANTIC_DEFECT
PLUMBING_FAILURE_COUNT=28      PLUMBING_FAILURE_29=NOT_ASSIGNED
ENGINE005G_BLOCKED_BY_THIS_STATE_RETEST=NO
```

Statut stratégique de `output.weight` :

```
OUTPUT_WEIGHT_FIRST_OPTIMIZATION_PRIORITY=NO
OUTPUT_WEIGHT_ENGINE006_PRIORITY=SECONDARY
```
(largest relative op-timing signature **mais** no isolated causal wall cost **et** no detectable
target-replacement wall signal.)

## 2 — Recensement : ce qui est mesuré (§8 à §10)

Instrumentation posée dans `ggml_cuda_mul_mat`, **au point exact de la cascade de décision** :

```cpp
const bool c_mmvf = ggml_cuda_should_use_mmvf(...);
const bool c_mmf  = !c_mmvf && ggml_cuda_should_use_mmf(...);
const bool c_mmvq = !c_mmvf && !c_mmf && ggml_cuda_should_use_mmvq(...);
const bool c_mmq  = !c_mmvf && !c_mmf && !c_mmvq && ggml_cuda_should_use_mmq(...);
ggml_census_record(src0, path);      // (pointeur de poids, chemin retenu)
```

La classification **reproduit l'ordre de priorité du code** et ne modifie rien d'autre :
`mmvf > mmf > mmvq > mmq > other`. Stockage : table ouverte à taille fixe
(8 192 entrées), deux atomiques `relaxed` par appel, **aucun printf, aucune allocation, aucune
synchronisation** dans le chemin chaud. Lecture unique après la fenêtre.

**Résultat mesuré** (fenêtre teacher-forced, runtime instrumenté, sortie hôte OFF) :

```
CENSUS_DISTINCT_WEIGHT_POINTERS=893
CENSUS_MMVF_SELECTED_TOTAL=384
CENSUS_MMF_SELECTED_TOTAL=0
CENSUS_MMVQ_SELECTED_TOTAL=940
CENSUS_MMQ_SELECTED_TOTAL=928
CENSUS_OTHER_SELECTED_TOTAL=888
E5E_WALL_MS_PER_TOKEN=63.5385        (runtime instrumenté, cohérent avec le baseline)
```

### Ce que ces chiffres établissent

1. **La déduction « N=1 ⇒ MMVQ » est fausse** comme description du workload entier : quatre chemins
   distincts sont réellement empruntés, pas un.
2. **`MMQ` n'est pas négligeable** : 928 sélections. L'hypothèse de 005A que « MMQ est marginal en
   décodage » ne tient pas telle quelle.
3. **`MMVF` (384) et `OTHER` (888) existent** : `OTHER` correspond vraisemblablement au chemin dense
   bf16 (les poids `ssm_alpha`/`ssm_beta` sont bf16, donc `ggml_is_quantized()` est faux et aucun
   prédicat MMVQ/MMQ ne s'applique) — **hypothèse à vérifier, pas un résultat**.
4. **`MMF` = 0** : cohérent (les poids sont quantifiés).

### Ce que ces chiffres n'établissent PAS

Ils sont **globaux**. Aucun n'est attribué à un rôle de poids. Donc :

```
OUTPUT_WEIGHT_MMVQ_COUNT=NOT_DETERMINED
OUTPUT_WEIGHT_MMQ_COUNT=NOT_DETERMINED
OUTPUT_WEIGHT_OTHER_COUNT=NOT_DETERMINED
OUTPUT_WEIGHT_KERNEL_PATH=RUNTIME_PROVEN_BY_SHAPE     (inchangé)
RECURRENT_ATTN_GATE_KERNEL_PATH=NOT_DETERMINED
DISPATCH_ACCOUNTING_MATCH=NOT_EVALUATED
```

## 3 — Ce qui manque, précisément

Le recensement est **clé par pointeur** ; il manque la table **pointeur → rôle**. Deux voies
existent déjà dans le travail précédent :

1. **Registre des poids du modèle** (`tensors_by_name`, `src/llama-model.cpp`) : c'est un
   `std::vector<std::pair<const char *, ggml_tensor *>>` construit au chargement. L'exposer par un
   accesseur (même style que `llama_model_n_recurrent`) donne la table complète
   `pointeur → nom → type` en une passe, **sans toucher au chemin chaud**.
2. **Provenance 005D** (`ggml_prov_record_matmul`) : elle connaît déjà, pour chaque MUL_MAT
   exécuté, le nom du poids source (`ggml_get_name(src[0])`) **et** la couche. Il suffit de
   recouper les deux relevés sur le pointeur de poids : la table `pointeur → rôle + couche` se
   construit par jointure, sans aucune instrumentation supplémentaire.

La voie 2 est la moins coûteuse : **deux relevés déjà écrits, un seul croisement à faire.**

## 4 — Effet de bord à consigner

La sortie du recensement est passée par un faux négatif avant d'être obtenue : le premier essai a
produit **zéro ligne** parce que le harnais utilisé n'appelait pas la fonction de vidage (défaut de
harnais, pas d'instrument). Vérifié par `nm -D` : la bibliothèque exportait bien les 9 symboles du
recensement. Classement :

```
HARNESS_DEFECT_8=DISPATCH_CENSUS_NOT_DUMPED_BY_HARNESS_VARIANT
HARNESS_DEFECT_8_CLASS=QUALIFICATION_HARNESS_DEFECT
PLUMBING_FAILURE_COUNT=28      PLUMBING_FAILURE_29=NOT_ASSIGNED
```

Deuxième incident de la passe : le premier run avec le binaire R2 a échoué sur
`failed to initialize batch` (positions M-RoPE) — artefact de harnais également, contourné en
revenant au harnais teacher-forced qui porte le warmup. Consigné, sans effet sur les chiffres.

## 5 — Suite immédiate (§26)

Le ladder reste :

1. **terminer le recensement par rôle** (jointure pointeur → rôle via 005D) → publier les sorties §11
   et la porte `DISPATCH_ACCOUNTING_MATCH` §12 ;
2. **puis** l'A/B de kernel équivalent sur `recurrent attn_gate.weight` (§15 à §21) — qui exige un
   mécanisme de forçage **strictement limité** au pointeur cible
   (`NON_TARGET_MATMUL_DISPATCH_CHANGED_COUNT=0`) ;
3. **ensuite seulement** l'épinglage ENGINE-006.

Le recensement change une chose importante pour l'étape 2 : puisque `MMVQ` **et** `MMQ` sont tous
deux réellement empruntés dans cette charge, l'A/B MMVQ ↔ MMQ est **plausible** — ce qui n'était
qu'une hypothèse avant cette passe.

## 6 — Provenance

```
Runtime baseline   : libllama 3db83675… / libggml-cuda abafbf0d…   (restaurés, SHA vérifiés)
Arbre llama-obrain-cuda : ggml-cuda.cu 960e38d6… , qwen35.cpp f0fd1a3c…   (identiques aux vierges)
Runtime instrumenté : build-e4-observe (op-timing + census)
Instrument conservé : source-e5g/ggml-cuda-census.{cuh,cpp,cu}
Sequence rejouee   : qualification/run003/run003-tokens.txt (gelée, 256 tokens ; fenêtre 8 utilisée ici)
```

## 7 — Ledger

```
ENGINE004_STATUS=CLOSED_BASELINE
ENGINE005A_STATUS=CLOSED_GPU_TIMELINE
ENGINE005C_STATUS=CLOSED_METHOD_STRUCTURALLY_INSUFFICIENT
ENGINE005D_STATUS=CLOSED_PROVENANCE
ENGINE005E_STATUS=CLOSED_OUTPUT_REQUEST_PATH_BRIDGE
ENGINE005F_STATUS=BLOCKED_ENGINE_REQUIRES_OUTPUT_HEAD_IN_GRAPH
ENGINE005F_R2_STATUS=CLOSED_NULL_WALL_SIGNAL_WITH_INCOMPLETE_ISOLATION
STATE_GATE_STATUS=NOT_EVALUABLE_WITH_THIS_HARNESS
HARNESS_DEFECT_7=PROBE_READS_STALE_LOGITS_WHEN_HEAD_ABSENT_FOR_PROCESS_LIFETIME
HARNESS_DEFECT_8=DISPATCH_CENSUS_NOT_DUMPED_BY_HARNESS_VARIANT
OUTPUT_WEIGHT_FIRST_OPTIMIZATION_PRIORITY=NO
ENGINE005G_STATUS=DISPATCH_CENSUS_INSTRUMENTED_TOTALS_ONLY
DISPATCH_CENSUS_POINT=GGML_CUDA_MUL_MAT_PRIORITY_CASCADE
DISPATCH_CENSUS_HOT_PATH_COST=2_RELAXED_ATOMICS_NO_IO
CENSUS_DISTINCT_WEIGHT_POINTERS=893
CENSUS_MMVF_SELECTED_TOTAL=384
CENSUS_MMF_SELECTED_TOTAL=0
CENSUS_MMVQ_SELECTED_TOTAL=940
CENSUS_MMQ_SELECTED_TOTAL=928
CENSUS_OTHER_SELECTED_TOTAL=888
YES_BY_SHAPE_MMVQ=RETIRED_AS_PRIMARY_PROOF
MULTIPLE_RUNTIME_PATHS_CONFIRMED=YES
DOMINANT_KERNEL_PATH_IDENTIFIED=NO
DOMINANT_KERNEL_PATH_SOURCE=RUNTIME_DISPATCH_BRANCH_NOT_YET_ROLE_KEYED
DISPATCH_ACCOUNTING_MATCH=NOT_EVALUATED
ENGINE006_TARGET=NOT_PINNED
ENGINE006_KERNEL_MODIFICATION_ALLOWED=NO
NEXT_MEASUREMENT=JOIN_CENSUS_POINTERS_WITH_005D_WEIGHT_ROLES
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
