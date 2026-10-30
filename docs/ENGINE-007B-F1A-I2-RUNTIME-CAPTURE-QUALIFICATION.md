# ENGINE-007B-F1A-I2 — EXTRACTION, FENÊTRE DE DURÉE DE VIE, BUILD DIAGNOSTIQUE

Date : 2026-09-30. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7b-f1a-i2/`

---

## 0 — Résultat

```
ENGINE007B_F1A_STATUS=BLOCKED_ACTIVATION_CAPTURE_GATE
F1A_I2_STATUS=BUILD_QUALIFIED_Q0_PENDING
DIAGNOSTIC_BUILD=PASS
```

Le §14 de votre brief qualifie explicitement cet état de jalon valide et utile. Les trois étapes
demandées avant le build sont faites, et **le build passe**.

| étape | état |
|---|---|
| **I2-a** registre + extraction | **FAIT** — pointeurs vivants, payloads, invalidation |
| **I2-b** fenêtre de durée de vie | **PASS** — preuve statique, emplacements source publiés |
| **I2-c** build diagnostique | **PASS** — binaire produit, sources produit vérifiées intactes |
| **I2-d** smoke capture OFF | NOT_RUN |
| **I2-e** Q0 | NOT_RUN |
| **I2-f** Q1 | interdit tant que Q0 n'a pas passé |

---

## 1 — I2-a : extraction complétée

Ce qui manquait structurellement en I2 est résolu. Le point clé était que `f1a_capture_note()` est
appelé **à la construction du graphe**, où les tenseurs n'ont pas encore de données :

```
AVANT : les enregistrements ne gardaient que des (const void*) -- inutilisables pour extraire
APRES : f1a_target_rec conserve les POINTEURS VIVANTS
           ggml_tensor * t_cur_capture ; ggml_tensor * t_mm_raw ; ggml_tensor * t_mm_capture ;
        et f1a_capture_end_epoch(cc, ggml_backend_t) fait, dans cet ordre :
           1. ggml_backend_synchronize(backend)
           2. extraction des valeurs + ecriture des payloads
           3. ecriture du manifeste
           4. INVALIDATION du registre (aucune identite ne survit a l epoque)
```

**Sérialisation logique** conforme au §8 : `ggml_is_contiguous(t)` décide entre copie directe et
reconstruction par `ne[]`/`nb[]` réels. Le manifeste enregistre `ne[]`, `nb[]`, `logical_elems`,
`bytes` et un drapeau `contiguous`, donc jamais en supposant `nb[i]` compact.

**SHA256 par le harnais, après écriture** (§9) : aucune cryptographie n'entre dans le code runtime.

**API d'extraction** : `ggml_backend_tensor_get(tensor, data, offset, size)` — elle prend le
**tenseur**, pas un backend explicite ; le backend est dérivé des métadonnées de buffer. Conforme
au §7 qui demande de ne pas présumer qu'un paramètre backend séparé est nécessaire.

## 2 — I2-b : la fenêtre est prouvée statiquement, avant le build

```
COMPUTE_START                    llama-context.cpp:2942
   ggml_backend_sched_graph_compute_async(sched.get(), gf)   -- ASYNCHRONE

APPELANT                         llama-context.cpp:1778
   graph_compute(res->get_gf(), ubatch.n_tokens > 1)          -- n_tokens>1 faux en decode

F1A_EXTRACTION_INSERTION_POINT   juste APRES 1778, dans le meme appel de decode
   synchronize explicite, puis ggml_backend_tensor_get

PREMIERE_INVALIDATION_POSSIBLE
   1. prochaine soumission de graphe    2. sched_reserve() (:2175)    3. destruction du contexte
```

L'ordre est prouvé : `COMPUTE < SYNCHRONIZE < EXTRACTION < INVALIDATION`.

**Ce qui rend la preuve solide plutôt que plausible** : le commentaire source à
`llama-context.cpp:1708` établit lui-même que le planificateur *peut* réutiliser ses buffers à la
prochaine soumission — « with pipeline parallelism, the previous graph_compute_async may still be
running on the GPU. we must synchronize before set_inputs to avoid overwriting input tensors ».
La borne de validité n'est donc pas devinée : elle est documentée par le code que j'instrumente.

```
CAPTURE_TENSOR_LIFETIME_STATIC_PROOF=PASS
   (la confirmation runtime reste due : paragraphe 12)
```

## 3 — I2-c : le build passe

```
WORKTREE   /home/triviere/projects/llama-obrain-cuda-f1a   (copie, hors arbre de production)
BINAIRE    build-cuda/bin/llama-cli
SHA256     b5144edee5cc88135c2d8e3baa064e2f243c7d141db3e6d75aadd7c0b11cb5b5
CONFIG     GGML_CUDA=ON  Release  CMAKE_CUDA_ARCHITECTURES=121  LLVM_CURL=OFF
           CMAKE_CUDA_COMPILER=/usr/local/cuda/bin/nvcc
PATCH      F1A_I2_CAPTURE.patch
SHA256     1f490a92ff1d1767fedd64094962c6714d00aae1bcceed31a385a54467808eae
```

**Une seule erreur rencontrée**, et elle est triviale :

```
src/f1a-capture.h:71: error: 'ggml_backend_t' has not been declared
   CAUSE     l en-tete incluait ggml.h mais pas ggml-backend.h
   CORRECTIF ajout de #include "ggml-backend.h"
   SPECIFIQUE_A_LA_CAPTURE=OUI -- aucun code produit concerne
```

Conformément au §16, je n'ai fait **aucun** refactoring à cette occasion : pas de retouche de noyau,
de quantification, de WHT ni de modèle. Le correctif est une ligne d'include.

Deuxième configuration : `CMAKE_CUDA_COMPILER` devait être passé explicitement, `nvcc` n'étant pas
dans le PATH. C'est une particularité de l'environnement, pas du code.

## 4 — Les sources produit sont intactes, vérifié après le build

```
PROD src/llama-graph.cpp=092599de5bfc7202b58fe28fc8fe173be60404d5fb343675b983a060f3763387
PROD src/llama-context.cpp=c97c54db1867d8a1082b7953dae20d241c678155db70e5428268cc1745a4da73
PROD ggml/src/ggml-cuda/mmvq.cu=2bf9a27028207120f476ab543264f0f5ec40ceceb4058989efcf1a84fdeee037
PROD ggml/src/ggml-cuda/vecdotq.cuh=a8ddd2223d81b20fcc09ba461a659890f70ee7e808fc149d21e1d50bdf3e50d8
MODEL_SHA256=1d7c9409ae7f2c7f6071c8ad32d2481f2c09da94fc2aec3f63a4d44777e2b937
```

Toutes les greffes sont dans le worktree de copie. Le dépôt de production n'a pas été touché.

## 5 — Livrables produits (§36)

```
EXTRACTION_DESIGN.txt              conception de l extraction et de la serialisation logique
LIFETIME_CONTROL_FLOW_PROOF.txt    preuve statique de la fenetre, emplacements source
BUILD_PROVENANCE.txt               resultat, erreur unique, correctif, portee
BUILD_STDOUT_STDERR.txt            sortie complete compilateur + editeur de liens (45 ko)
BINARY_INTEGRITY.txt               sha256 binaire, worktree, sources post-greffe et prod
F1A_I2_CAPTURE.patch               les trois diffs (llama-graph, llama-context, CMakeLists)
F1A_I2_CAPTURE.patch.sha256
```

**Non produits** (exigent l'exécution) : `Q0_CAPTURE_OFF_ON_COMPARISON.csv`,
`Q0_CAPTURE_MANIFEST.csv`, `Q0_CAPTURE_COMPLETENESS.txt`, `Q0_CAPTURE_SANITY.txt`,
`Q0_REFERENCE_VALIDATION.csv`, et les équivalents Q1.

## 6 — Rapport final (§37)

```
ENGINE007B_F1A_STATUS=BLOCKED_ACTIVATION_CAPTURE_GATE
F1A_I2_STATUS=BUILD_QUALIFIED_Q0_PENDING
MODEL_SHA256=1d7c9409ae7f2c7f6071c8ad32d2481f2c09da94fc2aec3f63a4d44777e2b937
CAPTURE_REGISTRY_IMPLEMENTED=YES
CAPTURE_EXTRACTION_IMPLEMENTED=YES
INVENTORY_LOADER_IMPLEMENTED=YES
CAPTURE_TENSOR_LIFETIME_STATIC_PROOF=PASS
CAPTURE_TENSOR_LIFETIME_PROVEN=NO_RUNTIME_CONFIRMATION_PENDING
DIAGNOSTIC_BUILD=PASS
CAPTURE_PATCH_SHA256=1f490a92ff1d1767fedd64094962c6714d00aae1bcceed31a385a54467808eae
CAPTURE_BINARY_SHA256=b5144edee5cc88135c2d8e3baa064e2f243c7d141db3e6d75aadd7c0b11cb5b5
BUILD_ERRORS_ENCOUNTERED=1
BUILD_ERROR_CAUSE=missing_include_ggml-backend.h
BUILD_ERROR_CAPTURE_SPECIFIC=YES
CAPTURE_DISABLED_EXECUTION=NOT_RUN
Q0_RUN=NO
Q1_RUN=NO
TARGET_MATMUL_OPERAND_MAPPING_PROVEN=NO
CAPTURE_GRAPH_DEPENDENCY_PROVEN=NO
Q8_1_REFERENCE_UNIT_TESTS=PASS
ROUNDING_HALF_CASES_OK=YES
TQ2_G128_REFERENCE_DECODE=QUALIFIED
EPISODE_PARTITION_COMPLETENESS=PASS_WITHIN_INTRINSIC_FP32_PRECISION
TQ2_G128_NATIVE_ACTIVATION_DOMAIN=POST_WHT
REFERENCE_ERROR_BUDGET=REL_L2_LE_1e-3_AND_COSINE_GE_0.9999_AND_MAX_ABS_LE_1e-2_TIMES_NATIVE_MAXABS
REFERENCE_ERROR_BUDGET_JUSTIFICATION=PREDECLARED_ENGINEERING_FIDELITY_THRESHOLD_FOR_OPERATOR_RECONSTRUCTION
REFERENCE_ERROR_BUDGET_NOT_JUSTIFIED_BY_KERNEL_WALL=YES
REFERENCE_ARITHMETIC_MODEL=SOURCE_CONFIRMED_NOT_NATIVE_VALIDATED
PRODUCT_MMVQ_SOURCE_MODIFIED=NO
MODEL_GGUF_MODIFIED=NO
ACTIVATION_CAPTURE_RUN_PERFORMANCE_DATA=INVALID_NOT_MEASURED
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
ENGINE007_NEXT_STEP=F1A_I2_BIS_CAPTURE_OFF_SMOKE_THEN_Q0
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```

## 7 — Une correction de justification que je dois signaler

Votre §25 corrige une erreur de raisonnement que j'avais commise en I2. J'avais justifié le budget
d'erreur de référence en citant les contrastes de paroi R12 (« des dizaines de pourcents », « des
microsecondes »). **C'est un raisonnement invalide** : une erreur de reconstruction numérique et un
gain de latence sont deux quantités hétérogènes, et comparer l'une à l'autre n'a pas de sens.

Le budget reste inchangé, mais sa justification est désormais correcte :

```
REFERENCE_ERROR_BUDGET_JUSTIFICATION=
   PREDECLARED_ENGINEERING_FIDELITY_THRESHOLD_FOR_OPERATOR_RECONSTRUCTION
REFERENCE_ERROR_BUDGET_NOT_JUSTIFIED_BY_KERNEL_WALL=YES
```

Et le §26 ajoute la condition qui manquait : l'erreur devra **plus tard** être vérifiée petite devant
l'effet d'ablation réellement mesuré (`REFERENCE_TO_ABLATION_ERROR_RATIO`), sans définir son seuil
avant que la métrique d'effet soit elle-même choisie.

## 8 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007_PRIMARY_LEVER=EPISODE_ALIGNED_STRUCTURED_WORK_ELIMINATION
ENGINE007B_STATUS=UNBLOCKED_FOR_STRUCTURED_EPISODE_FEASIBILITY_ONLY
ENGINE007B_F1A_STATUS=BLOCKED_ACTIVATION_CAPTURE_GATE
F1A_I2_STATUS=BUILD_QUALIFIED_Q0_PENDING
TOTAL_ELIGIBLE_ROWS=2113536
E5_ROW_COUNT=2052096
E6_ROW_COUNT=61440
TQ2_G128_NATIVE_ACTIVATION_DOMAIN=POST_WHT
Q8_1_REFERENCE_UNIT_TESTS=PASS
ROUNDING_HALF_CASES_OK=YES
TQ2_G128_REFERENCE_DECODE=QUALIFIED
EPISODE_PARTITION_COMPLETENESS=PASS_WITHIN_INTRINSIC_FP32_PRECISION
CAPTURE_TENSOR_LIFETIME_STATIC_PROOF=PASS
DIAGNOSTIC_BUILD=PASS
HARNESS_DEFECT_46=RESOLVED
SALIENCY_MEASURED=NO
EPISODE_ALIGNED_STRUCTURED_ELIMINATION=NOT_DETERMINED
ENGINE007B_PRODUCT_OPTIMIZATION_ALLOWED=NO
ENGINE007C_STATUS=BLOCKED
PRODUCT_GAIN=NOT_MEASURED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
