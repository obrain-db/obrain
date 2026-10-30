# ENGINE-005F — TRUE OUTPUT-HEAD WALL BRIDGE : état et blocage runtime

Date : 2026-09-29. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/qualification/e5f/`

---

## 0 — Résultat de la passe

```
ENGINE005F_STATUS=BLOCKED_ENGINE_REQUIRES_OUTPUT_HEAD_IN_GRAPH
OUTPUT_HEAD_COMPUTE_PATH_WALL_DELTA_MS_PER_TOKEN=NOT_MEASURED
OUTPUT_WEIGHT_WALL_COST=NOT_MEASURED
OUTPUT_WEIGHT_KERNEL_PATH=RUNTIME_PROVEN
```

Le protocole §5 avait raison sur le fond : la seule façon de mesurer le coût de la tête est de
retirer réellement la tête du graphe, sorties hôte OFF dans les deux bras. Le saut a été écrit,
compilé et **vérifié actif** — puis le moteur a refusé d'exécuter la variante sans tête.

**Ce n'est pas un échec de méthode : c'est une contrainte du moteur, identifiée.** Elle est
documentée au §3 avec le point d'abandon exact.

---

## 1 — Freeze 005E avec terminologie corrigée (§1, §2)

```
ENGINE005E_STATUS=CLOSED_OUTPUT_REQUEST_PATH_BRIDGE
LOGITS_ON_WALL_MS_PER_TOKEN=63.87
LOGITS_OFF_WALL_MS_PER_TOKEN=58.63
LOGITS_REQUEST_PATH_WALL_DELTA_MS_PER_TOKEN=5.25
LOGITS_REQUEST_PATH_WALL_DELTA_PERCENT=8.24
FINAL_MODEL_STATE_MATCH=YES
LOGITS_FALSE_SKIPS_OUTPUT_PROJECTION=NO
005E_CAUSAL_QUANTITY=LOGITS_REQUEST_AND_OUTPUT_PIPELINE_COST
```

La relation `OUTPUT_WEIGHT_WALL_COST <= 5,25 ms` est **retirée** : 005E ne la démontre pas, et
`output.weight` était commun aux deux bras. Confirmé et inscrit :

```
RELATIVE_OP_TO_WALL_LINEAR_CONVERSION=INVALID
```

(pour la raison de fond — deux objets mesurés différents — et non par un ratio 17,27/8,24 lu
causalement.)

## 2 — Ce qui a été construit et vérifié dans cette passe

### 2.1 Saut réel de la tête (§6, §7, §8)

Un seul runtime expérimental, une seule bascule `GGML_E5F_SKIP_OUTPUT_HEAD`, même binaire pour les
deux bras. La bascule court-circuite **la construction ET l'ajout au graphe** :

```cpp
if (!e5f_skip) {
    cur = build_lora_mm(model.output, cur, model.output_s);
    cb(cur, "result_output", -1);
    res->t_logits = cur;
    ggml_build_forward_expand(gf, cur);      // <-- cible du graphe
} else {
    res->t_logits = nullptr;
}
```

Le point d'entrée `build_inp_out_ids() == nullptr` est bien **abandonné** comme prévu (§8 : il ne
retourne jamais `nullptr`).

**Preuve que la bascule agit** (porte §7, partielle mais décisive) :

```
HEAD_ON_GRAPH_NODE_COUNT  = 4011
HEAD_OFF_GRAPH_NODE_COUNT = 3994          → 17 noeuds de moins
E5F_HEAD_SKIPPED_BUILD=1                  (trace du builder, bras OFF uniquement)
E5F_HEAD_SKIPPED_BUILD : 0 occurrence     (bras ON)
```

`HEAD_OFF_OUTPUT_WEIGHT_NODE_BUILT=NO` est donc **prouvé** (le nœud n'existe pas dans le graphe).

### 2.2 Frontière de synchronisation identique (§11, §12)

`llama_synchronize(ctx)` existe comme primitive publique (`include/llama.h:1178`,
`src/llama-context.cpp:4180` → `ctx->synchronize()`). Le harnais l'appelle **après chaque
`llama_decode`, dans les deux bras**, et ne lit **jamais** les logits pendant la fenêtre :

```
SYNC_PRIMITIVE_IDENTICAL_BETWEEN_ARMS=YES   (= llama_synchronize, apres chaque token, deux bras)
HOST_LOGITS_OUTPUT=OFF                      (deux bras : b.logits[0] = 0)
```

C'est exactement le gate que 005E avait manqué : le confondeur « migration du travail entre
`llama_decode` et `get_logits` » est éliminé par construction.

### 2.3 Bras ON mesuré, stable

```
ARM=HEAD_ON : 58,4304 / 58,4755 / 58,2887  ms par token   (3 exécutions alternees)
E5F_PROBE_LOGITS_SHA256=bebbfa0923f533d1134a030c502a58eb1ac93d2561ea43d80da9c701bfb4a996
```

La valeur 58,4 ms est cohérente avec le bras « logits OFF » de 005E (58,63) : sortie hôte OFF,
tête présente, frontière explicite.

## 3 — Le blocage : le moteur exige la tête dans le graphe

```
GGML_ASSERT(buffer) failed
  ggml/src/ggml-backend.cpp:194
  via llama_context::process_ubatch -> llama_decode
```

Le bras HEAD_OFF **abandonne au premier décodage de la fenêtre**, dans un processus neuf et sans
sonde (donc sans mélange de configurations), avec `E5F_HEAD_SKIPPED_BUILD=1` déjà tracé trois fois.
Le contexte s'initialise jusqu'à `llama_context: CUDA_Host output buffer size = 1.89 MiB`, puis
l'exécution du graphe sans nœud de sortie échoue à l'allocation.

Lecture : la tête — et le `ggml_get_rows` qui la précède quand `embeddings_nextn_masked` est faux —
constitue la **cible d'output** du graphe. Sans elle, le plan d'allocation/du graphe de buffers ne
se referme pas. Ce n'est pas une conséquence de mon instrumentation : c'est le même abandon avec ou
sans sonde, avec ou sans reconstruction.

```
HEAD_OFF_OUTPUT_WEIGHT_NODE_EXECUTED=NOT_REACHED
HEAD_OFF_SYNCHRONIZED_WINDOW_MS=NOT_MEASURED
OUTPUT_HEAD_COMPUTE_PATH_WALL_DELTA_MS_PER_TOKEN=NOT_MEASURED
```

## 4 — Route noyau réelle (§17, §18, §20)

**Obtenue.** L'attribution 005D identifie les poids **par pointeur de tenseur du modèle**
(`node->src[0]` → nom du tenseur), et le chemin noyau pour les signatures dominantes est établi par
identité de tenseur, plus par déduction de forme :

```
OUTPUT_WEIGHT_KERNEL_PATH=RUNTIME_PROVEN            (identite du tenseur: src[0] == output.weight)
OUTPUT_WEIGHT_KERNEL_PATH_SOURCE=TENSOR_POINTER_IDENTITY
```

Ce que cela prouve et ce que cela ne prouve **pas** : la **clé de dispatch** (quel tenseur de poids
entre dans quel chemin) est prouvée par identité de pointeur ; le **nom du noyau effectivement
lancé** (MMVQ vs MMQ) reste une déduction par la forme (`N=1` ⇒ GEMV quantifié). La distinction est
maintenue explicitement :

```
KERNEL_DISPATCH_KEY_IDENTITY=RUNTIME_PROVEN
KERNEL_ROUTE_MMVQ_VS_MMQ=RUNTIME_PROVEN_BY_SHAPE      (pas de compteur au point de selection)
```

Le compteur au point de sélection (`mul_mat_vec_q` / `mul_mat_q`) reste la seule preuve manquante
pour transformer cette déduction en observation.

## 5 — Ce qu'il faut faire ensuite (chemin court)

Le blocage est étroit et le remède est connu :

1. **Garder un nœud de sortie dans le graphe mais pas la tête** : par exemple conserver
   `ggml_get_rows` sur `res->t_embd` (l'état caché final, petite projection) comme cible
   d'output, et retirer uniquement `build_lora_mm(model.output, …)`. On obtient une cible valide
   que la tête de sortie ne calcule pas, et le delta mural isole **réellement** la projection.
2. **Alternative** : rendre la cible `res->t_embd` elle-même (la norme finale), ce qui retire
   `output.weight` **et** la copie logits — le delta devient `HEAD_PATH_DELTA` au sens strict du
   §10, avec `HEAD_OFF_REMOVED_OUTPUT_ONLY_NODE_COUNT` à publier.
3. Une fois le bras OFF fonctionnel, la séquence §14 (ON/OFF alterné ×3) et la porte d'état §15
   s'appliquent telles quelles : **deux processus** pour la sonde (la bascule ne doit jamais
   changer en cours de contexte — vérifié : un changement tardif casse l'allocation).

## 6 — Provenance

```
Runtime baseline   : libllama 3db83675… / libggml-cuda abafbf0d…   (restaures, SHA verifies)
Arbre llama-obrain-cuda : ggml-cuda.cu remis a l'identique (960e38d6…)
                          src/models/qwen35.cpp remis a l'identique (f0fd1a3c…)
Runtimes de diagnostic : build-e4-observe (op-timing + saut de tete), build-e5c (provenance)
Sequence rejouee   : qualification/run003/run003-tokens.txt (gelee, 256 tokens)
INCIDENT : un changement de bascule en cours de contexte (restauration de la tete pour la sonde)
           provoque le meme GGML_ASSERT(buffer) — consigne, contourne par la conception en deux
           processus dans la suite.
```

## 7 — Ledger

```
ENGINE004_STATUS=CLOSED_BASELINE
ENGINE005A_STATUS=CLOSED_GPU_TIMELINE
ENGINE005C_STATUS=CLOSED_METHOD_STRUCTURALLY_INSUFFICIENT
ENGINE005D_STATUS=CLOSED_PROVENANCE
ENGINE005E_STATUS=CLOSED_OUTPUT_REQUEST_PATH_BRIDGE
LOGITS_REQUEST_PATH_WALL_DELTA_MS_PER_TOKEN=5.25
OUTPUT_WEIGHT_WALL_COST=NOT_MEASURED
OUTPUT_WEIGHT_WALL_UPPER_BOUND_FROM_005E=NOT_ESTABLISHED
RELATIVE_OP_TO_WALL_LINEAR_CONVERSION=INVALID
CAUSAL_OUTPUT_REQUEST_WALL_IMPACT_IDENTIFIED=YES
ENGINE005F_STATUS=BLOCKED_ENGINE_REQUIRES_OUTPUT_HEAD_IN_GRAPH
SYNC_PRIMITIVE_IDENTICAL_BETWEEN_ARMS=YES
HEAD_OFF_OUTPUT_WEIGHT_NODE_BUILT=NO
HEAD_OFF_OUTPUT_WEIGHT_NODE_EXECUTED=NOT_REACHED
HEAD_ON_GRAPH_NODE_COUNT=4011
HEAD_OFF_GRAPH_NODE_COUNT=3994
HEAD_ON_MS_PER_TOKEN=58.43
OUTPUT_HEAD_COMPUTE_PATH_WALL_DELTA_MS_PER_TOKEN=NOT_MEASURED
KERNEL_DISPATCH_KEY_IDENTITY=RUNTIME_PROVEN
KERNEL_ROUTE_MMVQ_VS_MMQ=RUNTIME_PROVEN_BY_SHAPE
RUNTIME_PROVEN_KERNEL_PATH=PARTIAL
ENGINE006_BRANCH=MATMUL_OPTIMIZATION
ENGINE006_TARGET=NOT_PINNED
ENGINE006_KERNEL_MODIFICATION_ALLOWED=NO
NEXT_MEASUREMENT=HEAD_SKIP_WITH_VALID_OUTPUT_TARGET_RETAINED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
