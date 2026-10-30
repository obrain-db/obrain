# ENGINE-007B-F1A-S2bis — CAPTURE LIÉE AU GRAPHE ET QUALIFICATION DE LA RÉFÉRENCE

Date : 2026-09-30. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7b-f1a-s2bis/`

---

## 0 — Résultat

```
ENGINE007B_F1A_STATUS=BLOCKED_GRAPH_BOUND_CAPTURE_GATE
```

Votre §2 change la nature du problème, et il a raison. Mais je n'ai pas pu mener la greffe à
terme dans cette passe. Ce qui est **acquis** et ce qui **reste** :

| gate du §40 | état |
|---|---|
| **A** `OPERAND_MAPPING` | **NON** — identifié par source, pas prouvé en exécution |
| **B** `GRAPH_BOUND_CAPTURE` | **design validé et vérifié faisable, HOOK NON IMPLÉMENTÉ** |
| **C** `LIFETIME` | non applicable tant que B n'existe pas |
| **D** `COMPLETENESS_AND_SHAPE` | non mesuré |
| **E** `Q8_1_REFERENCE` | **PASS** — 7 cas, invariants vérifiés |
| **F** `TQ2_REFERENCE` | **QUALIFIED** — table + empaquetage vérifiés |
| **G** `FULL_OPERATOR_RECONSTRUCTION` | non mesuré |

**Aucune saillance, aucun masque** (§38, §39 respectés).

---

## 1 — Inventaire autoritatif (§1)

```
E5_ROW_COUNT=2052096      E6_ROW_COUNT=61440      TOTAL_ELIGIBLE_ROWS=2113536
ROLE_AGGREGATE_SUM_MATCH=YES
TENSOR_BY_TENSOR_CSV_IS_AUTHORITATIVE=YES
```

| rôle | n | lignes |
|---|---|---|
| `ffn_gate` / `ffn_up` | 48 + 48 | 835 584 chacun |
| `attn_gate` | 36 | 221 184 |
| `attn_q` | 12 | 147 456 |
| `attn_k` | 12 | 12 288 |
| **E5 total** | **156** | **2 052 096** |
| `attn_output` (**E6**) | 12 | 61 440 |

## 2 — Le design de capture est **faisable**, et je l'ai vérifié (§2 à §5)

Votre principe est le bon, et il dissout le problème de durée de vie :

```
native     : mul_mat(w, cur)
diagnostic : cur_capture = ggml_dup(cur) ;  mul_mat(w, cur_capture)
```

La dépendance `producteur → copie → mul_mat` est alors **vérifiée par le planificateur**, et la
question devient « le graphe a-t-il exécuté une copie matérialisante devenue le `src1` du matmul
cible » — au lieu de « a-t-on copié le bon tampon au bon instant ».

**Vérifications faites dans cette passe, par lecture de source :**

```
ggml_dup existe            ggml.h:896 ; ggml.c (dup_impl)          -> GGML_OP_DUP
backend CUDA le supporte   ggml-cuda.cu:1963 ET :4906 (case GGML_OP_DUP)
   -> une copie materialisante est executable sur le backend cible
extraction                 ggml_backend_tensor_get (ggml-backend.h:93)
synchronisation            ggml_backend_sched_synchronize (llama-context.cpp:722)
execution                  ggml_backend_sched_graph_compute_async (llama-context.cpp:2942)

CHAINE DE DUREE DE VIE : build -> compute_async -> synchronize -> EXTRACTION -> reset
```

`ggml_dup` est un `GGML_OP_DUP`, **pas une vue** :

```
CAPTURE_TENSOR_ALIASES_SOURCE_STORAGE=NO
CAPTURE_TENSOR_MATERIALIZED_BY_GRAPH=YES
```

```
CAPTURE_DESIGN=GRAPH_BOUND_MATERIALIZED_SRC1_COPY_VALIDATED_NOT_IMPLEMENTED
CAPTURE_HOOK=NONE_IMPLEMENTED
CAPTURE_GRAPH_DEPENDENCY_PROVEN=NOT_MEASURED
TARGET_MATMUL_USES_CAPTURE_TENSOR_AS_SRC1=NOT_IMPLEMENTED
CAPTURE_TENSOR_LIFETIME_PROVEN=NOT_MEASURED
```

**Je n'ai pas écrit la greffe**, et je préfère le dire que de livrer un patch non qualifié. Le
design est validé : les quatre briques existent et sont exécutables sur ce backend. Ce qui manque
est le travail d'implémentation (`build_lora_mm` + registre de clones + manifeste + extraction).

## 3 — Références : **implémentées et testées** (§28 à §31)

### q8_1 — `Q8_1_REFERENCE_UNIT_TESTS=PASS`

| cas | `d` fp16 | `s` fp16 | Σqs | invariants |
|---|---|---|---|---|
| toutes nulles | 0,0 | 0,0 | 0 | d=0, qs ok, s ok |
| constante positive | 0,027557 | 112,0 | 4064 | tous ok |
| constante négative | 0,017715 | −72,0 | −4064 | tous ok |
| signes alternés | 0,007874 | 0,0 | 0 | tous ok |
| proche de zéro | 0,0 | 0,0 | 0 | tous ok |
| grandes valeurs finies | 78,75 | 0,0 | 0 | tous ok |
| aléatoires finies | 0,041229 | 10,2266 | 248 | tous ok |

Pour chaque cas, `d == fp16(amax/127)`, chaque `qs[j] == roundf(x[j]·id)` borné, et
`s == fp16(Σqs · d)`.

```
ZERO_AMAX_BEHAVIOR=d=0_et_id=0_donc_qs_tous_nuls
```

**Un défaut que j'ai trouvé et corrigé dans ma propre référence.** Ma première version utilisait
`np.rint`, qui fait du **half-to-even**, alors que la source C utilise `roundf`, **demi-écarté de
zéro**. Sur `2.5`, `np.rint` donne 2 et `roundf` donne 3 — la différence est réelle et se
propagerait dans toute la référence. J'ai implémenté `roundf` explicitement et ajouté un test dédié :

```
ROUNDING_HALF_CASES_OK=YES
   ( 0,5->1  1,5->2  2,5->3  -0,5->-1  -1,5->-2  -2,5->-3  0,4999->0  -0,4999->0 )
```

*Réserve* : ce défaut a été trouvé **par relecture de ma sortie**, pas par un test qui l'aurait
attrapé. Le test qui l'attrape n'existait pas — je l'ai ajouté après coup. Je le signale parce que
c'est la troisième fois dans cette série qu'une erreur de mon propre harnais est découverte par
inspection et non par gate.

### TQ2_G128 — `TQ2_G128_REFERENCE_DECODE=QUALIFIED`

```
code 0 -> -1    code 1 -> 0    code 2 -> +1    code 3 -> +1      (les 4 verifies)

octet 0xE4 (0b11100100) -> 4 premiers poids = [-1, 0, +1, +1]
   bits 0-1 = 00 -> -1 ;  2-3 = 01 -> 0 ;  4-5 = 10 -> +1 ;  6-7 = 11 -> +1
   -> empaquetage petit-boutiste dans l octet CONFIRME
```

## 4 — Partition des épisodes (§31, §37)

```
K=5120 : 5 episodes [0,1024) .. [4096,5120)     NO_GAPS=YES  FULL_COVER=YES  NO_OVERLAPS=YES
K=6144 : 6 episodes [0,1024) .. [5120,6144)     NO_GAPS=YES  FULL_COVER=YES  NO_OVERLAPS=YES
```

Vérification numérique de complétude : la somme des contributions par épisode et le total
diffèrent de **1,9e-06 sur 22,1** (soit **8,6e-08 relatif**).

```
EPISODE_PARTITION_COMPLETENESS=PASS_WITHIN_INTRINSIC_FP32_PRECISION
```

Ce n'est **pas** un défaut de partition, et je documente pourquoi plutôt que de le lisser : la
partition est complète **par construction** (chaque kbx appartient à exactement un épisode, et
chaque contribution est ajoutée une fois au total et une fois à son épisode). L'écart est l'erreur
d'arrondi fp32 entre **deux ordres d'accumulation différents** de la même quantité mathématique —
le total suit l'ordre kbx-major, la somme des épisodes suit l'ordre d'épisode. Un calcul exact
demanderait une accumulation en double précision, ce qui ne correspondrait plus au modèle
arithmétique déployé.

## 5 — Modèle arithmétique de la référence (§33)

```
REFERENCE_ARITHMETIC_MODEL=
   par tranche iqs in 0..3 :
      sumi = somme_{j=0..7} dp4a(vi[j], get_int_b4(q8.qs, j))
      y_tranche = fp32( d_weight_fp16 * d_act_q8_fp16 * sumi )
   y_kbx = somme des 4 tranches, accumulee en fp32
   y_row = somme des kbx
```

Ce n'est **pas** une déquantification en flottant suivie d'un produit scalaire générique : c'est
l'entier d'abord (`dp4a`), puis une **double mise à l'échelle fp16** (`d` du poids × `d` du bloc
q8_1), conformément à `return d * __low2float(bq8_1[iqs].ds) * sumi;`. `__low2float` de `ds` est
bien la moitié **basse**, donc `d` et non `s` — vérifié dans la structure `block_q8_1`.

*Réserve honnête* : je n'ai pas confirmé ce modèle contre le SASS, seulement contre la source C++.
Le §33 mentionne « source/SASS » ; la partie SASS reste à faire au moment de la validation contre
la sortie native.

## 6 — Ce qui reste, précisément

```
S2bis-a  greffe dans build_lora_mm : si src0->type == 43 ET nom dans l inventaire
            -> cur_capture = ggml_dup(cur) ; mul_mat(w, cur_capture)
            inerte sans OBRAIN_F1A_CAPTURE=1 ; graphe identique au baseline si desactive
S2bis-b  registre de clones : cle = identite du tenseur d activation d origine
            -> reutilisation quand plusieurs poids consomment le meme cur
S2bis-c  capture de la sortie de base : mm_base = mul_mat(w, cur_capture), copie separee
S2bis-d  manifeste attendu (inventaire x tokens) et manifeste observe ; MISSING/UNEXPECTED/DUPLICATE
S2bis-e  extraction apres ggml_backend_sched_synchronize, avant reset
S2bis-f  validation reference vs sortie native : >=2 tenseurs E5, >=1 E6
```

## 7 — Aucune mutation (§5, §43)

```
PRODUCT_MMVQ_SOURCE_MODIFIED=NO
   SHA256 mmvq.cu=2bf9a27028207120f476ab543264f0f5ec40ceceb4058989efcf1a84fdeee037
   SHA256 vecdotq.cuh=a8ddd2223d81b20fcc09ba461a659890f70ee7e808fc149d21e1d50bdf3e50d8
   SHA256 llama-graph.cpp=092599de5bfc7202b58fe28fc8fe173be60404d5fb343675b983a060f3763387
MODEL_GGUF_MODIFIED=NO
BASE_REPO_HEAD=8447604f7f0c3b48a77be46de2466a5b23e27826
F1A_S2BIS_CAPTURE.patch=NON_PRODUIT   (aucune source modifiee dans cette passe)
ACTIVATION_CAPTURE_RUN_PERFORMANCE_DATA=INVALID_NOT_MEASURED
PLUMBING_FAILURE_COUNT=28             PLUMBING_FAILURE_29=NOT_ASSIGNED
```

## 8 — Livrables produits

```
Q8_1_REFERENCE_TESTS.txt         7 cas, invariants verifies, cas limites d arrondi
TQ2_G128_REFERENCE_TESTS.txt     table de decodage + empaquetage
EPISODE_PARTITION_VALIDATION.txt partition E5/E6, completude, precision intrinseque
GRAPH_CAPTURE_DESIGN.txt         design, verifications de faisabilite, chaine de duree de vie
f1a_ref.py                       la reference (q8_1 + TQ2 + ligne complete + contributions)
f1a_ref_tests.py                 la suite de tests
```

**Non produits** (dépendent du hook) : `TARGET_OP_RUNTIME_PROOF.csv`,
`GRAPH_CAPTURE_LIFETIME_PROOF.txt`, `CAPTURE_MANIFEST.csv`, `CAPTURE_COMPLETENESS.txt`,
`CAPTURE_SANITY.txt`, `REFERENCE_VALIDATION.csv`, `INVENTORY_AGGREGATE_CHECK.txt`,
`F1A_S2BIS_CAPTURE.patch`, `F1A_S2BIS_CAPTURE.patch.sha256`, `CONFIG_PROVENANCE.txt`.

## 9 — Rapport final (§46)

```
ENGINE007B_F1A_STATUS=BLOCKED_GRAPH_BOUND_CAPTURE_GATE
MODEL_SHA256=1d7c9409ae7f2c7f6071c8ad32d2481f2c09da94fc2aec3f63a4d44777e2b937
TOTAL_ELIGIBLE_ROWS=2113536
E5_ROW_COUNT=2052096
E6_ROW_COUNT=61440
ROLE_AGGREGATE_SUM_MATCH=YES
TARGET_MATMUL_OPERAND_MAPPING_PROVEN=NO
CAPTURE_DESIGN=GRAPH_BOUND_MATERIALIZED_SRC1_COPY_VALIDATED
CAPTURE_DESIGN_FEASIBILITY_VERIFIED=YES
CAPTURE_IMPLEMENTED=NO
CAPTURE_PATCH_SHA256=NOT_PRODUCED
CAPTURE_BINARY_SHA256=NOT_PRODUCED
CAPTURE_GRAPH_DEPENDENCY_PROVEN=NOT_MEASURED
TARGET_MATMUL_USES_CAPTURE_TENSOR_AS_SRC1=NOT_IMPLEMENTED
CAPTURE_TENSOR_LIFETIME_PROVEN=NOT_MEASURED
CAPTURE_TENSOR_ALIASES_SOURCE_STORAGE=NO
CAPTURE_TENSOR_MATERIALIZED_BY_GRAPH=YES
QUALIFICATION_SEQUENCE_COUNT=0
QUALIFICATION_DECODE_TOKENS=0
EXPECTED_TARGET_INVOCATIONS=NOT_COMPUTED
OBSERVED_TARGET_INVOCATIONS=0
MISSING_CAPTURE_COUNT=NOT_MEASURED
UNEXPECTED_CAPTURE_COUNT=NOT_MEASURED
DUPLICATE_CAPTURE_COUNT=NOT_MEASURED
UNIQUE_ACTIVATION_CAPTURE_COUNT=0
CAPTURED_E5_ACTIVATION_K=NOT_MEASURED
CAPTURED_E6_ACTIVATION_K=NOT_MEASURED
CAPTURED_ACTIVATION_NAN_COUNT=NOT_MEASURED
CAPTURED_ACTIVATION_INF_COUNT=NOT_MEASURED
Q8_1_REFERENCE_UNIT_TESTS=PASS
Q8_1_ROUNDING_DEFECT_FOUND_AND_FIXED=YES
TQ2_G128_REFERENCE_DECODE=QUALIFIED
EPISODE_PARTITION_COMPLETENESS=PASS_WITHIN_INTRINSIC_FP32_PRECISION
EPISODE_PARTITION_NO_GAPS=YES
EPISODE_PARTITION_NO_OVERLAPS=YES
REFERENCE_ARITHMETIC_MODEL=INTEGER_DP4A_THEN_DOUBLE_FP16_SCALING
REFERENCE_ERROR_BUDGET=NOT_SET_PENDING_NATIVE_COMPARISON
REFERENCE_VALIDATION_MAX_ABS=NOT_MEASURED
REFERENCE_VALIDATION_MEAN_ABS=NOT_MEASURED
REFERENCE_VALIDATION_RELATIVE_L2=NOT_MEASURED
REFERENCE_VALIDATION_COSINE=NOT_MEASURED
EPISODE_REFERENCE_RECONSTRUCTS_FULL_OPERATOR=NOT_MEASURED
PRODUCT_MMVQ_SOURCE_MODIFIED=NO
MODEL_GGUF_MODIFIED=NO
ACTIVATION_CAPTURE_RUN_PERFORMANCE_DATA=INVALID_NOT_MEASURED
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
ENGINE007_NEXT_STEP=F1A_S2BIS_IMPLEMENT_THE_GRAFT_AT_build_lora_mm
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```

## 10 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007_PRIMARY_LEVER=EPISODE_ALIGNED_STRUCTURED_WORK_ELIMINATION
ENGINE007B_STATUS=UNBLOCKED_FOR_STRUCTURED_EPISODE_FEASIBILITY_ONLY
ENGINE007B_F1A_STATUS=BLOCKED_GRAPH_BOUND_CAPTURE_GATE
AUTHORITATIVE_TENSOR_INVENTORY_AGGREGATES_CONSISTENT=YES
TOTAL_ELIGIBLE_ROWS=2113536
E5_ROW_COUNT=2052096
E6_ROW_COUNT=61440
E5_GEOMETRY_SOURCE_PROVEN=YES
E6_GEOMETRY_SOURCE_PROVEN=YES
TQ2_G128_REFERENCE_DECODE=QUALIFIED
Q8_1_REFERENCE_SEMANTICS=ESTABLISHED_FROM_SOURCE
Q8_1_REFERENCE_UNIT_TESTS=PASS
GRAPH_BOUND_CAPTURE=DESIGN_VALIDATED_NOT_IMPLEMENTED
SALIENCY_MEASURED=NO
EPISODE_ALIGNED_STRUCTURED_ELIMINATION=NOT_DETERMINED
ENGINE007B_PRODUCT_OPTIMIZATION_ALLOWED=NO
ENGINE007C_STATUS=BLOCKED
PRODUCT_GAIN=NOT_MEASURED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
