# ENGINE-005F-R2 — TRUE OUTPUT.WEIGHT WALL COST : verdict sur les deux volets

Date : 2026-09-29. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/qualification/e5fr2/`

---

## 0 — Verdict

Deux questions, deux réponses nettes :

```
A. WHAT_IS_THE_WALL_COST_OF_OUTPUT_WEIGHT_PROJECTION?
   → 0,0 ms/token détectable (|Δ| = 0,042 ms sur 58,6 ms, sous le bruit inter-runs)
   → PROJECTION_ONLY_ISOLATION=NOT_ESTABLISHED, donc la quantité reste NON MESURÉE
     au sens strict : on mesure « aucune différence murale détectable », pas « coût nul »

B. WHICH_RUNTIME_KERNEL_PATH_EXECUTES_OUTPUT_WEIGHT?
   → NON MESURÉ : compteurs de dispatch NON_INSTRUMENTED
```

Et une porte d'état **non évaluable**, avec sa cause identifiée (§4) — ce n'est pas un échec
d'instrument, c'est un défaut de conception du harnais, maintenant expliqué.

```
ENGINE005F_R2_STATUS=CLOSED_NULL_WALL_DELTA_ISOLATION_UNPROVEN
OUTPUT_WEIGHT_PATH_WALL_DELTA_MS_PER_TOKEN=-0.0424
PROJECTION_ONLY_ISOLATION=NOT_ESTABLISHED
OUTPUT_WEIGHT_WALL_COST=NOT_MEASURED
OUTPUT_WEIGHT_KERNEL_PATH_SOURCE=SHAPE_DERIVATION
```

---

## 1 — Ce qui a débloqué le bras OFF (§2, §4, §5)

Entrée exacte de la projection, lue dans la source (`src/models/qwen35.cpp:230-236`) :

```
cur = get_rows(result_norm, inp_out_ids) si inp_out_ids present, sinon result_norm
res->t_embd = cur
cur = build_lora_mm(model.output, cur, model.output_s)
→ OUTPUT_WEIGHT_INPUT_IS_POST_GET_ROWS=YES
→ OUTPUT_WEIGHT_INPUT_IS_RES_T_EMBD=NO   (quand inp_out_ids est present)
```

Remède appliqué (**cible de graphe remplacée, nœud conservé**) :

```cpp
ggml_tensor * pre = cur;                                   // post-get_rows
cur = build_lora_mm(model.output, cur, model.output_s);
res->t_logits = cur;
if (!skip) ggml_build_forward_expand(gf, cur);              // ON
else       ggml_build_forward_expand(gf, ggml_dup(ctx0, pre));  // OFF : sink valide
```

```
HEAD_REMOVAL_MUST_PRESERVE_VALID_GRAPH_OUTPUT=YES   (confirmé : la variante « suppression pure » abandonnait)
HEAD_OFF_RUNTIME_EXECUTABLE=YES                     (exit 0, 256 tokens)
HEAD_OFF_SINK=dup(post_get_rows_tensor)             → isolation visée : PROJECTION SEULE
```

## 2 — Volet A : delta mural (§11 à §16)

Ordre alterné imposé **avant** observation : `ON OFF OFF ON ON OFF`. Six processus séparés, même
binaire, même modèle, séquence gelée RUN-003 (256 tokens injectés).

| bras | valeurs (ms/token) | mean | median | sd | min | max |
|---|---|---|---|---|---|---|
| ON | 58,4169 / 58,7478 / 58,7013 | **58,6220** | 58,7013 | 0,1463 | 58,4169 | 58,7478 |
| OFF | 58,5030 / 58,7358 / 58,7544 | **58,6644** | 58,7358 | 0,1144 | 58,5030 | 58,7544 |

```
PAIRED_DELTA_MEAN   = -0,0424 ms/token   (ON - OFF)
PAIRED_DELTA_MEDIAN = -0,0345 ms/token
OUTPUT_WEIGHT_PATH_WALL_SHARE ≈ -0,07 % de la paroi
SYNC_PRIMITIVE_IDENTICAL_BETWEEN_ARMS=YES   (llama_synchronize après chaque token)
SYNC_CALL_COUNT_PER_TOKEN=1
HOST_LOGITS_OUTPUT=OFF                      (les deux bras — confondeur 005E absent)
```

**Conclusion défendable** : dans cette configuration, remplacer la cible de graphe de la projection
de sortie ne produit **aucun effet mural détectable**. L'écart mesure -0,04 ms, soit **1/3 de
l'écart-type inter-runs** (0,15 ms) : statistiquement indistinguable de zéro.

**Ce qui n'est pas conclu** : que le coût soit nul. La porte d'isolation (§3) n'est pas franchie,
donc on ne peut pas exclure que la projection continue de s'exécuter dans le bras OFF (§3, indice des
1 appel MUL_MAT en moins sur 2 849).

## 3 — Isolation : non établie, et pourquoi c'est structurel ici

```
HEAD_ON_GRAPH_NODE_COUNT=4011     HEAD_OFF_GRAPH_NODE_COUNT=4011
HEAD_OFF_REMOVED_NODE_COUNT=0     (sur le compteur publie par la reservation)
ALL_REMOVED_NODES_ARE_OUTPUT_PROJECTION_EXCLUSIVE=NOT_ESTABLISHED
PROJECTION_ONLY_ISOLATION=NOT_ESTABLISHED
```

Le compteur `sched_reserve: graph nodes` est émis à la **réservation du contexte**, avant que la
bascule ne change la cible : il vaut 4011 dans les deux bras et **ne peut pas** servir de porte.

Indice réel, par l'instrumentation op-par-op :

```
ARM=ON  : MUL_MAT 1622,9 ms / 2849 appels
ARM=OFF : MUL_MAT 1617,9 ms / 2848 appels    → 1 appel de moins sur 2849 (0,035 %)
OPS_TOTAL identiques : 8 358 dans les deux bras
```

Cohérent avec l'objectif (une seule projection retirée), **trop faible pour être une porte**.

## 4 — Volet porte d'état : non évaluable, cause identifiée

```
ON_RESTORED_PROBE_LOGITS_SHA256  = bebbfa0923f533d1134a030c502a58eb1ac93d2561ea43d80da9c701bfb4a996
OFF_RESTORED_PROBE_LOGITS_SHA256 = 895365e27ed4255bccdcabe85bebf7c22aaaeebab375264d8e90eae30dc28399
FINAL_PROBE_LOGITS_MATCH=NO
PROBE_REPRODUCIBLE_WITHIN_ARM=YES      (ON 2/2 identiques, OFF 2/2 identiques)
```

Chaque bras est **parfaitement déterministe**, mais les deux diffèrent. Test décisif :

```
OFF  sonde finale        = 895365e2…
OFF  logits APRES PREFIXE = 895365e2…      → IDENTIQUES
ON   sonde finale        = bebbfa09…
ON   logits APRES PREFIXE = c149e705…      → DIFFERENTS
```

**Cause établie** : dans le bras OFF, la tête est absente du graphe pour **tout le processus**, y
compris la sonde finale. La sonde ne calcule donc rien et lit le **tampon de logits périmé du
préfixe**. Le « désaccord d'état » de la première lecture de R2 était un artefact de harnais :
il n'y a pas de divergence d'état démontrée, il y a une **sonde qui ne sonde pas**.

```
STATE_GATE_STATUS=NOT_EVALUABLE_WITH_THIS_HARNESS
FINAL_RECURRENT_STATE_MATCH=NOT_MEASURED
FINAL_KV_STATE_MATCH=NOT_MEASURED
FINAL_POSITION_STATE_MATCH=NOT_MEASURED
STATE_COMPARISON_SCOPE=canonical_probe_logits_hash_only
HARNESS_DEFECT_7=PROBE_READS_STALE_LOGITS_WHEN_HEAD_ABSENT_FOR_PROCESS_LIFETIME
```

Le remède est celui que le protocole §18 décrit et que je n'ai pas exécuté : **sérialiser l'état**
en fin de rejeu, terminer le processus, puis restaurer dans un processus canonique HEAD_ON et sonder
là. Cela demande `llama_state_seq_get_data` / `set_data` (l'état récurrent se sérialise par
séquence), ce qui n'a pas été fait ici.

## 5 — Volet B : route noyau (§19 à §23) — NON FAIT

```
OUTPUT_WEIGHT_DISPATCH_IDENTITY=RUNTIME_PROVEN        (src[0] == output.weight — 005D/005F)
OUTPUT_WEIGHT_KERNEL_PATH=RUNTIME_PROVEN_BY_SHAPE
OUTPUT_WEIGHT_KERNEL_PATH_SOURCE=SHAPE_DERIVATION
OUTPUT_WEIGHT_MMVQ_SELECTED_COUNT=NOT_INSTRUMENTED
OUTPUT_WEIGHT_MMQ_SELECTED_COUNT=NOT_INSTRUMENTED
OUTPUT_WEIGHT_OTHER_SELECTED_COUNT=NOT_INSTRUMENTED
```

Les compteurs au point de sélection n'ont pas été posés : le budget de cette passe est passé dans la
levée du blocage, la mesure murale et le diagnostic de la porte d'état. C'est une brique **bon
marché** et c'est la prochaine action, indépendante du volet mural :

- point d'insertion : `ggml_cuda_mul_mat` dans `ggml/src/ggml-cuda/ggml-cuda.cu`, aux branches
  `mul_mat_vec_q` (MMVQ), `mul_mat_q` (MMQ), et le cas résiduel ;
- clé d'identification : `src[0] == <pointeur output.weight>`, résolu une fois via le registre de
  tenseurs du modèle (identité de pointeur, pas de nom) ;
- stockage : trois `std::atomic<unsigned long long>`, lus une seule fois après la fenêtre ;
- aucun log dans le chemin chaud (`HOT_PATH_PROVENANCE_LOOKUP=NO`).

## 6 — Confrontation à 005D (§25)

```
005D_RELATIVE_MUL_MAT_SHARE(output.weight) = 17,27 %
005F_R2 WALL_SHARE (meme operation)        = non etabli (isolation insuffisante)
```

Le pont relatif→mural pour cette signature **reste ouvert** : la mesure murale existe (nulle) mais
l'isolation qui la rendrait attribuable ne l'est pas. Deux chiffres qui concernent bien la même
opération, mais dont un seul est qualifié.

## 7 — Conséquence pour ENGINE-006 (§27, §28)

```
OUTPUT_WEIGHT_CAUSAL_WALL_IMPACT_IDENTIFIED=NO_TO_NULL_SIGNAL
OUTPUT_WEIGHT_KERNEL_PATH=RUNTIME_PROVEN_BY_SHAPE
PROJECTION_ONLY_ISOLATION=NOT_ESTABLISHED
→ ENGINE006_TARGET=NOT_PINNED
→ ENGINE006_KERNEL_MODIFICATION_ALLOWED=NO
```

Conformément à §28 : le signal mural de `output.weight` étant **nul**, ce n'est pas la première
cible. La suite logique est `recurrent attn_gate.weight` (7,01 % relatif), qui exige un **A/B de
kernel équivalent** et non une suppression — donc précisément les compteurs de dispatch et un
mécanisme de forçage de chemin.

Note d'honnêteté sur la portée : le delta nul porte sur la **configuration rejouée** (256 tokens
teacher-forced, contexte ~1200). Il ne prouve pas que la projection est gratuite dans tous les
régimes, seulement qu'ici, retirer sa cible de graphe ne change pas la paroi.

## 8 — Provenance

```
Runtime baseline   : libllama 3db83675… / libggml-cuda abafbf0d…   (restaurés, SHA vérifiés)
Arbre llama-obrain-cuda : ggml-cuda.cu 960e38d6… , qwen35.cpp f0fd1a3c…   (identiques aux vierges)
Runtimes de diagnostic : build-e4-observe (op-timing + cible de graphe), build-e5c (provenance)
Sequence rejouee   : qualification/run003/run003-tokens.txt (gelée, 256 tokens)
```

## 9 — Ledger

```
ENGINE004_STATUS=CLOSED_BASELINE
ENGINE005A_STATUS=CLOSED_GPU_TIMELINE
ENGINE005C_STATUS=CLOSED_METHOD_STRUCTURALLY_INSUFFICIENT
ENGINE005D_STATUS=CLOSED_PROVENANCE
ENGINE005E_STATUS=CLOSED_OUTPUT_REQUEST_PATH_BRIDGE
ENGINE005F_STATUS=BLOCKED_ENGINE_REQUIRES_OUTPUT_HEAD_IN_GRAPH
HEAD_REMOVAL_MUST_PRESERVE_VALID_GRAPH_OUTPUT=YES
ENGINE005F_R2_STATUS=CLOSED_NULL_WALL_DELTA_ISOLATION_UNPROVEN
HEAD_OFF_RUNTIME_EXECUTABLE=YES
HEAD_OFF_SINK=dup(post_get_rows_tensor)
HEAD_OFF_OUTPUT_WEIGHT_NODE_BUILT=NOT_ESTABLISHED
SYNC_PRIMITIVE_IDENTICAL_BETWEEN_ARMS=YES
SYNC_CALL_COUNT_PER_TOKEN=1
HOST_LOGITS_OUTPUT=OFF
ON_MS_PER_TOKEN_MEAN=58.6220
OFF_MS_PER_TOKEN_MEAN=58.6644
OUTPUT_WEIGHT_PATH_WALL_DELTA_MS_PER_TOKEN=-0.0424
OUTPUT_WEIGHT_PATH_WALL_SHARE=-0.07_PERCENT
NO_DETECTABLE_WALL_EFFECT_FROM_PROJECTION_TARGET_REPLACEMENT=YES
PROJECTION_ONLY_ISOLATION=NOT_ESTABLISHED
OUTPUT_WEIGHT_WALL_COST=NOT_MEASURED
STATE_GATE_STATUS=NOT_EVALUABLE_WITH_THIS_HARNESS
HARNESS_DEFECT_7=PROBE_READS_STALE_LOGITS_WHEN_HEAD_ABSENT_FOR_PROCESS_LIFETIME
OUTPUT_WEIGHT_DISPATCH_IDENTITY=RUNTIME_PROVEN
OUTPUT_WEIGHT_KERNEL_PATH=RUNTIME_PROVEN_BY_SHAPE
OUTPUT_WEIGHT_KERNEL_PATH_SOURCE=SHAPE_DERIVATION
OUTPUT_WEIGHT_MMVQ_SELECTED_COUNT=NOT_INSTRUMENTED
ENGINE006_TARGET=NOT_PINNED
ENGINE006_KERNEL_MODIFICATION_ALLOWED=NO
NEXT_MEASUREMENT=DISPATCH_COUNTERS_AT_MUL_MAT_SELECTION_POINT
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
