# ENGINE-007B-F1A-S2bis-I1 — GREFFE DE CAPTURE LIÉE AU GRAPHE : ÉTAT D'IMPLÉMENTATION

Date : 2026-09-30. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7b-f1a-s2bis-i1/`

---

## 0 — Résultat

```
ENGINE007B_F1A_STATUS=BLOCKED_GRAPH_BOUND_CAPTURE_GATE
```

**Le patch est écrit, appliqué et vérifié par diff. Il n'est ni compilé ni exécuté.**

Je m'arrête ici plutôt que de livrer un build à moitié fait ou d'annoncer des chiffres que je
n'aurais pas produits. Ce qui est fait et ce qui reste :

| étape | état |
|---|---|
| script de greffe, 3 ancres vérifiées | **FAIT** — diff minimal confirmé |
| en-tête : déclarations du registre | **FAIT** |
| en-tête : **implémentations** | **NON ÉCRIT** |
| chargement de l'inventaire pour l'éligibilité | **NON ÉCRIT** |
| build du binaire diagnostique | **NON TENTÉ** |
| exécution, extraction, manifeste | **NON TENTÉ** |
| non-intrusivité OFF/ON | **NON MESURÉE** |
| validation de la référence contre la sortie native | **NON MESURÉE** |

---

## 1 — Le patch, et ce qu'il fait exactement (§2 à §4, §41)

```
SITE = src/llama-graph.cpp :: llm_graph_context::build_lora_mm
ANCRES = 3 (include, signature, mul_mat) -- chacune exactement 1 occurrence, sinon rien n'est ecrit
LIGNES_AJOUTEES = 18        LIGNES_MODIFIEES = 1 (declaration de `res` scindee)
```

Transformation, **chirurgicale** :

```cpp
    ggml_tensor * res;
    if (f1a_cc != nullptr && f1a_eligible(w)) {
        ggml_tensor * const f1a_cur_cap = ggml_dup(ctx0, cur);        // cur_capture
        ggml_tensor * const f1a_mm_raw  = ggml_mul_mat(ctx0, w, f1a_cur_cap);
        res = ggml_dup(ctx0, f1a_mm_raw);                             // mm_capture, AVANT LoRA
        f1a_capture_note(f1a_cc, w, f1a_cur_raw, cur, f1a_cur_cap, f1a_mm_raw, res);
    } else {
        res = ggml_mul_mat(ctx0, w, cur);                             // chemin natif, INCHANGE
    }
```

Le reste de la fonction — `w_s`, la boucle LoRA, tout l'aval — **n'est pas touché** et s'applique
à `res`, donc à `mm_capture`. Aucun bloc n'est dupliqué.

**Pourquoi ce choix plutôt qu'un `return` anticipé.** Ma première version copiait la boucle LoRA
dans la branche de capture et retournait tôt. C'est un risque inutile : deux copies d'une même
logique divergent silencieusement. La version retenue ne modifie qu'**une ligne** et laisse
l'aval intact.

**Les deux `dup` sont sur le chemin vivant** : `res` est retourné par la fonction, donc le
planificateur exécute forcément les deux copies. C'est la propriété que votre §3 exige.

```
REFERENCE_TARGET_OUTPUT_IS_BASE_MUL_MAT_RESULT=YES_PAR_CONSTRUCTION
   -> mm_capture = dup(mm_raw) est pris AVANT `if (w_s)` et AVANT la boucle LoRA
INTERRUPTEUR=OBRAIN_F1A_CAPTURE=1 ; defaut : inerte (aucun dup, aucun registre, aucune E/S)
```

Le diff complet est archivé : `F1A_S2BIS_I1_CAPTURE.patch` (sha256 dans le `.sha256` joint).

## 2 — Une découverte qui conditionne la validation (§8, §31, §34)

En lisant le site, j'ai trouvé quelque chose que les passes précédentes n'avaient pas relevé et
qui **change la nature de l'activation capturée** :

```cpp
if (w->type == GGML_TYPE_TQ2_G128 && getenv("OBRAIN_NO_ROTATION") == nullptr) {
    ...
    cur = it->second;              // activation REPLIEE par la transformee WHT
    ...
}
ggml_tensor * res = ggml_mul_mat(ctx0, w, cur);
```

**L'activation consommée par le matmul `TQ2_G128` n'est pas l'activation brute : c'est l'activation
après application de la transformée WHT**, mise en cache dans `obrain_rot` et partagée entre tous
ses consommateurs.

Conséquences, et elles sont importantes pour F1 :

1. **La référence que j'ai qualifiée en S2bis n'est pas celle qu'il faut valider.** Elle quantifie
   `x` en q8_1 puis décode les poids ternaires. Or le noyau déployé reçoit `WHT(x)`, pas `x`.
   Valider la référence contre la sortie native avec l'activation brute **échouerait**, et
   l'échec serait imputé à tort à la référence.

2. **La bonne cible de capture est `cur` APRÈS la rotation**, donc à l'endroit exact où j'ai posé la
   greffe — après le bloc `if`, sur la ligne `ggml_mul_mat`. C'est ce que le patch fait.

3. **Le partage `obrain_rot` fournit la déduplication gratuitement** : plusieurs projections d'une
   même activation reçoivent le **même pointeur** `cur`. La clé de registre « identité de `cur` »
   du §10 est donc directement opérante, sans heuristique.

4. **La rotation dépend d'un `getenv`** : avec `OBRAIN_NO_ROTATION` posé, l'activation est brute.
   La greffe enregistre `f1a_cur_raw` **et** `cur` pour que le manifeste distingue les deux cas —
   sans quoi on ne saurait pas ce qu'on a capturé.

```
CAPTURED_ACTIVATION_IS_POST_WHT=YES_WHEN_ROTATION_ACTIVE
GREFFE_ENREGISTRE_ACTIVATION_BRUTE_ET_ACTIVATION_CONSOMMEE=YES
REFERENCE_VALIDATED_IN_S2BIS_TARGETS_RAW_ACTIVATION=NEEDS_CORRECTION_OR_ROTATION_DISABLED
```

Je n'ai **pas** corrigé la référence dans cette passe. La voie la plus simple pour la
qualification serait de lancer le workload avec `OBRAIN_NO_ROTATION=1` — l'activation est alors
brute, et la référence S2bis s'applique telle quelle — puis, dans un second temps, de valider
la référence sur l'activation repliée.

## 3 — Ce que le registre doit encore faire (§7, §10, §15, §26)

L'en-tête `f1a-capture.h` déclare l'interface ; les implémentations manquent. Ce qui reste à
écrire, avec les contraintes du brief :

```
f1a_capture_get_ctx()   retourne nullptr sauf si OBRAIN_F1A_CAPTURE=1 ; une EPOQUE par graphe
                        (paragraphe 7 : pas de singleton global non scope)
f1a_eligible(w)         type == GGML_TYPE_TQ2_G128 (43) ET nom present dans
                        TQ2_TENSOR_INVENTORY.csv -- charge une fois, jamais par prefixe
f1a_capture_note(...)   cle = (epoch, identite de cur, dtype, ne[], nb[]) -> reutilisation
                        de cur_capture quand plusieurs poids consomment le meme cur
extraction              apres ggml_backend_sched_graph_compute_async (:2942) et
                        ggml_backend_sched_synchronize (:722), avant tout reset
                        -> ggml_backend_tensor_get respectant ne[]/nb[] reels
```

## 4 — Aucune mutation des sources produit ou du modèle (§5, §41)

```
PRODUCT_MMVQ_SOURCE_MODIFIED=NO
MODEL_GGUF_MODIFIED=NO
GREFFE_APPLIQUEE_A_UNE_COPIE (/tmp/f1a-dry/llama-graph.cpp) -- pas au depot de production
BASE_REPO_HEAD=8447604f7f0c3b48a77be46de2466a5b23e27826
SHA256 mmvq.cu=2bf9a27028207120f476ab543264f0f5ec40ceceb4058989efcf1a84fdeee037
SHA256 vecdotq.cuh=a8ddd2223d81b20fcc09ba461a659890f70ee7e808fc149d21e1d50bdf3e50d8
SHA256 llama-graph.cpp=092599de5bfc7202b58fe28fc8fe173be60404d5fb343675b983a060f3763387
ACTIVATION_CAPTURE_RUN_PERFORMANCE_DATA=INVALID_NOT_MEASURED
SALIENCY_MEASURED=NO    QUALITY_ABLATION_MEASURED=NO
```

La greffe a été appliquée à une **copie** pour produire le diff, conformément au §5 qui demande un
worktree diagnostique. Le dépôt de production reste intact.

## 5 — Défaut de harnais à consigner (§37)

```
HARNESS_DEFECT_46=Q8_1_REFERENCE_USED_BANKERS_ROUNDING_INSTEAD_OF_C_ROUNDF
   -> ma premiere version de la reference q8_1 utilisait np.rint (half-to-even) au lieu de
      roundf (demi-ecarte de zero). Sur 2.5 : np.rint donne 2, roundf donne 3.
      Detecte par relecture de ma propre sortie, PAS par un gate.
      Corrige : roundf implemente explicitement, test des cas limites ajoute
      (ROUNDING_HALF_CASES_OK=YES sur 8 cas : 0.5 1.5 2.5 -0.5 -1.5 -2.5 0.4999 -0.4999).
   STATUS=RESOLVED_BEFORE_SALIENCY_OR_NATIVE_REFERENCE_CONCLUSION
```

Numéro vérifié : le dernier défaut utilisé dans cette série est `HARNESS_DEFECT_45` (S2). **46 est
le prochain disponible** — je ne l'invente pas.

```
PLUMBING_FAILURE_COUNT=28      PLUMBING_FAILURE_29=NOT_ASSIGNED
```

## 6 — Livrables

Produits dans `qualification/e7b-f1a-s2bis-i1/` :

```
F1A_S2BIS_I1_CAPTURE.patch        le diff, 18 lignes ajoutees / 1 modifiee
F1A_S2BIS_I1_CAPTURE.patch.sha256
apply-f1a-capture.py              le script declare, ancres verifiees
f1a-capture.h                     les DECLARATIONS du registre
CONFIG_PROVENANCE.txt             provenance, etat des gates
```

**Non produits** (dépendent du build et de l'exécution) : `TARGET_OP_RUNTIME_PROOF.csv`,
`GRAPH_CAPTURE_REGISTRY.txt`, `GRAPH_CAPTURE_EDGES.csv`, `GRAPH_CAPTURE_LIFETIME_PROOF.txt`,
`CAPTURE_OFF_ON_NUMERIC_COMPARISON.csv`, `CAPTURE_MANIFEST.csv`, `CAPTURE_COMPLETENESS.txt`,
`CAPTURE_SANITY.txt`, `NATIVE_OPERATOR_REFERENCE_VALIDATION.csv`,
`INVENTORY_AGGREGATE_CHECK.txt`, `Q8_1_REFERENCE_TESTS.txt`, `TQ2_G128_REFERENCE_TESTS.txt`,
`EPISODE_PARTITION_VALIDATION.txt`.

## 7 — Rapport final (§43)

```
ENGINE007B_F1A_STATUS=BLOCKED_GRAPH_BOUND_CAPTURE_GATE
MODEL_SHA256=1d7c9409ae7f2c7f6071c8ad32d2481f2c09da94fc2aec3f63a4d44777e2b937
TOTAL_ELIGIBLE_ROWS=2113536
E5_ROW_COUNT=2052096
E6_ROW_COUNT=61440
TARGET_MATMUL_OPERAND_MAPPING_PROVEN=NO
CAPTURE_DESIGN=GRAPH_BOUND_DUP_SRC1_AND_BASE_OUTPUT
CAPTURE_PATCH_SHA256=PRODUIT_voir_F1A_S2BIS_I1_CAPTURE.patch.sha256
CAPTURE_BINARY_SHA256=NOT_PRODUCED
CAPTURE_TENSOR_ALIASES_SOURCE_STORAGE=NO_PAR_CONSTRUCTION_GGML_OP_DUP
CAPTURE_TENSOR_MATERIALIZED_BY_GRAPH=YES_PAR_CONSTRUCTION_GGML_OP_DUP
CAPTURE_GRAPH_DEPENDENCY_PROVEN=NO
TARGET_MATMUL_USES_CAPTURE_TENSOR_AS_SRC1=YES_PAR_CONSTRUCTION_DANS_LE_PATCH
DOWNSTREAM_USES_CAPTURED_BASE_OUTPUT=YES_PAR_CONSTRUCTION_DANS_LE_PATCH
CAPTURE_TENSOR_LIFETIME_PROVEN=NO
QUALIFICATION_SEQUENCE_COUNT=0
QUALIFICATION_DECODE_TOKENS=0
EXPECTED_GRAPH_TARGETS=NOT_COMPUTED
OBSERVED_EXTRACTED_TARGETS=0
MISSING_CAPTURE_COUNT=NOT_MEASURED
UNEXPECTED_CAPTURE_COUNT=NOT_MEASURED
DUPLICATE_CAPTURE_COUNT=NOT_MEASURED
AUTHORITATIVE_ELIGIBLE_TENSOR_COUNT=168
GRAPH_ENCOUNTERED_ELIGIBLE_TENSOR_COUNT=NOT_MEASURED
TARGET_INVOCATION_COUNT=0
UNIQUE_ACTIVATION_CAPTURE_COUNT=0
CAPTURED_E5_ACTIVATION_K=NOT_MEASURED
CAPTURED_E6_ACTIVATION_K=NOT_MEASURED
ACTIVATION_NAN_COUNT=NOT_MEASURED
ACTIVATION_INF_COUNT=NOT_MEASURED
BASE_OUTPUT_NAN_COUNT=NOT_MEASURED
BASE_OUTPUT_INF_COUNT=NOT_MEASURED
CAPTURE_ON_OFF_LOGITS_BITWISE_MATCH=NOT_MEASURED
LOGIT_MAX_ABS_DIFF=NOT_MEASURED
LOGIT_RELATIVE_L2=NOT_MEASURED
LOGIT_COSINE=NOT_MEASURED
ARGMAX_AGREEMENT=NOT_MEASURED
CAPTURE_GRAFT_NUMERICALLY_TRANSPARENT=NOT_MEASURED
Q8_1_REFERENCE_UNIT_TESTS=PASS
ROUNDING_HALF_CASES_OK=YES
TQ2_G128_REFERENCE_DECODE=QUALIFIED
EPISODE_PARTITION_COMPLETENESS=PASS_WITHIN_INTRINSIC_FP32_PRECISION
REFERENCE_ERROR_BUDGET=NOT_SET_PENDING_NATIVE_COMPARISON
REFERENCE_VALIDATION_MAX_ABS=NOT_MEASURED
REFERENCE_VALIDATION_MEAN_ABS=NOT_MEASURED
REFERENCE_VALIDATION_RELATIVE_L2=NOT_MEASURED
REFERENCE_VALIDATION_COSINE=NOT_MEASURED
EPISODE_REFERENCE_RECONSTRUCTS_FULL_OPERATOR=NOT_MEASURED
CAPTURED_ACTIVATION_IS_POST_WHT=YES_WHEN_ROTATION_ACTIVE
PRODUCT_MMVQ_SOURCE_MODIFIED=NO
MODEL_GGUF_MODIFIED=NO
ACTIVATION_CAPTURE_RUN_PERFORMANCE_DATA=INVALID_NOT_MEASURED
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
ENGINE007_NEXT_STEP=F1A_S2BIS_I1_BIS_IMPLEMENT_REGISTRY_AND_BUILD
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```

## 8 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007_PRIMARY_LEVER=EPISODE_ALIGNED_STRUCTURED_WORK_ELIMINATION
ENGINE007B_STATUS=UNBLOCKED_FOR_STRUCTURED_EPISODE_FEASIBILITY_ONLY
ENGINE007B_F1A_STATUS=BLOCKED_GRAPH_BOUND_CAPTURE_GATE
TOTAL_ELIGIBLE_ROWS=2113536
E5_ROW_COUNT=2052096
E6_ROW_COUNT=61440
Q8_1_REFERENCE_UNIT_TESTS=PASS
ROUNDING_HALF_CASES_OK=YES
TQ2_G128_REFERENCE_DECODE=QUALIFIED
EPISODE_PARTITION_COMPLETENESS=PASS_WITHIN_INTRINSIC_FP32_PRECISION
GRAPH_BOUND_CAPTURE_PATCH=WRITTEN_AND_DIFF_VERIFIED
GRAPH_BOUND_CAPTURE_BUILD=NOT_ATTEMPTED
GRAPH_BOUND_CAPTURE_EXECUTION=NOT_ATTEMPTED
CAPTURED_ACTIVATION_IS_POST_WHT=YES_WHEN_ROTATION_ACTIVE
SALIENCY_MEASURED=NO
EPISODE_ALIGNED_STRUCTURED_ELIMINATION=NOT_DETERMINED
ENGINE007B_PRODUCT_OPTIMIZATION_ALLOWED=NO
ENGINE007C_STATUS=BLOCKED
PRODUCT_GAIN=NOT_MEASURED
HARNESS_DEFECT_46=Q8_1_REFERENCE_USED_BANKERS_ROUNDING_INSTEAD_OF_C_ROUNDF_RESOLVED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
