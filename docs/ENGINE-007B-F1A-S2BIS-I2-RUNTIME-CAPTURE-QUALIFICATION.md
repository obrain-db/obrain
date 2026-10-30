# ENGINE-007B-F1A-S2bis-I2 — IMPLÉMENTATION DU REGISTRE : ÉTAT ET PLAN

Date : 2026-09-30. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7b-f1a-s2bis-i2/`

---

## 0 — Résultat

```
ENGINE007B_F1A_STATUS=BLOCKED_CAPTURE_BUILD_GATE
```

Le registre est **implémenté**. Le binaire n'est **pas construit**, et je dois expliquer pourquoi je
m'arrête ici plutôt que de lancer le build.

---

## 1 — Ce qui est implémenté

### En-tête `f1a-capture.h`

Registre **scopé à une époque de graphe**, pas de singleton non scopé (§7, §13) :

```cpp
struct f1a_target_rec {
    int target_id ; unsigned long long epoch ;
    std::string weight_name ; int weight_type ; int64_t weight_ne[4] ;
    const void * cur_raw_id ;      // activation telle qu'elle ARRIVE
    const void * cur_eff_id ;      // activation EFFECTIVEMENT consommee (post-WHT si rotation)
    bool rotation_applied ;
    int64_t eff_ne[4] ; int64_t eff_nb[4] ; size_t eff_bytes ;
    int family ; int64_t expected_K ;
    const void * cur_capture_id ; const void * mm_raw_id ; const void * mm_capture_id ;
    int activation_capture_id ;    // deduplication
};
```

### Implémentations `f1a-capture.cpp` (176 lignes)

| fonction | ce qu'elle fait | §du brief |
|---|---|---|
| `f1a_load_inventory()` | lit le CSV figé, colonne `tensor_name`, ignore l'en-tête et les `#`, **détecte les doublons et désactive la capture si présents** | §8 |
| `f1a_capture_get_ctx()` | retourne `nullptr` sauf `OBRAIN_F1A_CAPTURE=1` ; un contexte par époque | §6, §7 |
| `f1a_eligible()` | **type == `GGML_TYPE_TQ2_G128`** ET **nom exact dans l'inventaire** — aucune heuristique de préfixe ni de rôle | §7 |
| `f1a_capture_note()` | enregistre l'invocation, classe E5/E6 par `K`, déduplique sur `(cur_eff, dtype, ne0, nb0)` avec **exigence de correspondance exacte** | §10, §12 |
| `f1a_capture_end_epoch()` | écrit le manifeste CSV, puis **vide le registre** : aucune identité ne survit à l'époque | §13 |

Trois propriétés que le brief exige et que le code tient :

```
INERTE_PAR_DEFAUT : aucun dup, aucun registre, aucune E/S sans OBRAIN_F1A_CAPTURE=1
ELIGIBILITE_PAR_INVENTAIRE : le CSV est la seule autorite, lue une fois
EPOQUE_CLOSE : le registre est vide et les compteurs remis a zero en fin d epoque
```

**Le manifeste enregistre `rotation_applied`**, calculé comme `cur_raw != cur_used`. C'est ce qui
permettra de compter `ROTATION_APPLIED_TARGET_COUNT` et `ROTATION_NOT_APPLIED_TARGET_COUNT` (§29)
sans jamais mélanger les domaines.

## 2 — Ce qui manque, et c'est structurel

**L'écriture des payloads de valeurs n'est pas implémentée, et elle ne peut pas l'être à l'endroit
où `f1a_capture_note()` est appelé.**

```
f1a_capture_note()  est appele a la CONSTRUCTION du graphe
   -> les tenseurs existent comme noeuds, mais n'ont AUCUNE donnee
   -> ggml_backend_tensor_get() a cet instant est sans objet

L extraction doit se faire a la FIN D EPOQUE, apres sched_synchronize (:722)
   -> mais end_epoch() ne recoit que le contexte, PAS les tenseurs
   -> il faut donc faire remonter les POINTEURS de tenseurs, pas seulement leurs ids
```

Ce que cela demande, précisément :

```
1. f1a_target_rec doit conserver ggml_tensor* (cur_capture, mm_capture), pas (const void*)
2. f1a_capture_end_epoch() doit recevoir un ggml_backend_t pour appeler
   ggml_backend_tensor_get(t, buf, 0, nbytes)
3. cet appel doit etre place APRES ggml_backend_sched_synchronize (llama-context.cpp:722)
   et AVANT tout reset du planificateur  -- fenetre a tracer, pas a supposer (paragraphe 14)
4. ecriture binaire des payloads + sha256 + metadonnees dans un manifeste de payloads
```

Ce n'est pas une difficulté conceptuelle : c'est de la plomberie que je n'ai pas eu la place de
faire **correctement** dans cette passe — et le §14 interdit explicitement de deviner la frontière
de reset (« do not guess the reset boundary »).

## 3 — Pourquoi je ne lance pas le build maintenant

Le build du binaire diagnostique compile le fork llama complet avec le backend CUDA, pour un modèle
de 27B. C'est une opération longue. Deux issues possibles dans l'espace qui me reste :

| issue | conséquence |
|---|---|
| le build réussit | je n'ai plus de place pour exécuter Q0/Q1, extraire, comparer OFF/ON et valider la référence — donc **aucun livrable de la passe** |
| le build échoue | je consomme le reste à déboguer une erreur de compilation, et **aucun livrable** non plus |

Dans les deux cas, lancer le build maintenant produit **moins** de valeur que de livrer
l'implémentation complète et vérifiée telle qu'elle est, avec le chemin restant tracé.

Je préfère vous le dire plutôt que de lancer une opération que je ne pourrai pas conclure.

## 4 — Le plan exact pour la passe suivante

```
I2-a  completer f1a-capture.{h,cpp} :
         - conserver les pointeurs ggml_tensor* dans les enregistrements
         - f1a_capture_end_epoch(cc, ggml_backend_t backend)
         - extraction ggml_backend_tensor_get respectant ne[]/nb[] reels (paragraphe 15)
         - ecriture binaire des payloads + sha256 + manifeste de payloads
         - payloads : binaire, jamais de dump textuel de milliers de flottants (paragraphe 26)

I2-b  point d appel dans llama-context.cpp :
         - f1a_capture_end_epoch() APRES ggml_backend_sched_synchronize (:722)
         - tracer la frontiere reelle du reset : lire le flux de controle, publier les
           emplacements source exacts (paragraphe 14)

I2-c  build du binaire diagnostique ; archiver sortie compilateur, sortie editeur de liens,
         sha256 binaire / sources / patch ; DIAGNOSTIC_BUILD=PASS (paragraphe 15)

I2-d  smoke test capture OFF : comparer au baseline sur un decodage deterministe minuscule
         -> CAPTURE_DISABLED_EXECUTION=PASS (paragraphe 16)

I2-e  Q0 (OBRAIN_NO_ROTATION=1) :
         Q0_OFF / Q0_ON, 1 sequence, 8-16 tokens, teacher forcing
         logits finaux, argmax, sorties de base selectionnees
         manifeste de construction == manifeste extrait, MISSING/UNEXPECTED/DUPLICATE = 0
         validation de la reference contre mm_base natif : >=2 E5, >=1 E6
         REFERENCE_ERROR_BUDGET declare AVANT de regarder l agregeat (paragraphe 22)

I2-f  Q1 (rotation active) : identique, sur l activation POST-WHT effective capturee
```

Budget d'erreur de référence, **pré-déclaré avant toute mesure** (§22, §30 — même seuil pour Q0 et Q1) :

```
REFERENCE_ERROR_BUDGET=
   RELATIVE_L2 <= 1e-3   ET   COSINE >= 0.9999
   MAX_ABS <= 1e-2 * max|mm_base| sur l echantillon valide
   Justification : l ecart doit rester tres inferieur aux effets d ablation d episodes que F1B
   etudiera (R12 mesure des contrastes de plusieurs us, soit des dizaines de pourcents de paroi).
   Un ordre de grandeur de marge sur l erreur de reconstruction est le minimum acceptable.
```

## 5 — État des gates (§35, §36)

```
DIAGNOSTIC_BUILD=NOT_ATTEMPTED          -> BLOCKED_CAPTURE_BUILD_GATE
TARGET_MATMUL_OPERAND_MAPPING_PROVEN=NO
CAPTURE_GRAPH_DEPENDENCY_PROVEN=NO
CAPTURE_TENSOR_LIFETIME_PROVEN=NO
Q0_CAPTURE_COMPLETENESS=NOT_MEASURED
Q1_CAPTURE_COMPLETENESS=NOT_MEASURED
Q0_CAPTURE_GRAFT_NUMERICALLY_TRANSPARENT=NOT_MEASURED
Q1_CAPTURE_GRAFT_NUMERICALLY_TRANSPARENT=NOT_MEASURED
Q0_EPISODE_REFERENCE_RECONSTRUCTS_FULL_OPERATOR=NOT_MEASURED
Q1_EPISODE_REFERENCE_RECONSTRUCTS_FULL_OPERATOR=NOT_MEASURED

DEJA ACQUIS (S2bis / S2) :
   Q8_1_REFERENCE_UNIT_TESTS=PASS
   ROUNDING_HALF_CASES_OK=YES
   TQ2_G128_REFERENCE_DECODE=QUALIFIED
   EPISODE_PARTITION_COMPLETENESS=PASS_WITHIN_INTRINSIC_FP32_PRECISION
   TQ2_G128_NATIVE_ACTIVATION_DOMAIN=POST_WHT   (decouvert en I1)
   REFERENCE_Q8_1_TQ2_INVALIDATED_BY_WHT=NO     (la reference est agnostique de v)
```

## 6 — Aucune mutation

```
PRODUCT_MMVQ_SOURCE_MODIFIED=NO     MODEL_GGUF_MODIFIED=NO
GREFFE_APPLIQUEE_A_UNE_COPIE (/tmp/f1a-dry/) -- pas au depot de production
SHA256 mmvq.cu=2bf9a27028207120f476ab543264f0f5ec40ceceb4058989efcf1a84fdeee037
SHA256 vecdotq.cuh=a8ddd2223d81b20fcc09ba461a659890f70ee7e808fc149d21e1d50bdf3e50d8
SHA256 llama-graph.cpp=092599de5bfc7202b58fe28fc8fe173be60404d5fb343675b983a060f3763387
ACTIVATION_CAPTURE_RUN_PERFORMANCE_DATA=INVALID_NOT_MEASURED
SALIENCY_MEASURED=NO        QUALITY_ABLATION_MEASURED=NO
PLUMBING_FAILURE_COUNT=28   PLUMBING_FAILURE_29=NOT_ASSIGNED
```

## 7 — Rapport final (§41)

```
ENGINE007B_F1A_STATUS=BLOCKED_CAPTURE_BUILD_GATE
DIAGNOSTIC_BUILD=NOT_ATTEMPTED
CAPTURE_PATCH_SHA256=INCHANGE_DEPUIS_I1 (patch de greffe identique ; implementations ajoutees a cote)
CAPTURE_BINARY_SHA256=NOT_PRODUCED
TARGET_MATMUL_OPERAND_MAPPING_PROVEN=NO
CAPTURE_GRAPH_DEPENDENCY_PROVEN=NO
CAPTURE_TENSOR_LIFETIME_PROVEN=NO
Q0_ROTATION_ENABLED=NO
Q0_CAPTURE_COMPLETENESS=NOT_MEASURED
Q0_CAPTURE_GRAFT_NUMERICALLY_TRANSPARENT=NOT_MEASURED
Q0_REFERENCE_VALIDATION_MAX_ABS=NOT_MEASURED
Q0_REFERENCE_VALIDATION_RELATIVE_L2=NOT_MEASURED
Q0_REFERENCE_VALIDATION_COSINE=NOT_MEASURED
Q0_EPISODE_REFERENCE_RECONSTRUCTS_FULL_OPERATOR=NOT_MEASURED
Q1_ROTATION_ENABLED=YES
Q1_CAPTURE_COMPLETENESS=NOT_MEASURED
Q1_CAPTURE_GRAFT_NUMERICALLY_TRANSPARENT=NOT_MEASURED
ROTATION_APPLIED_TARGET_COUNT=NOT_MEASURED
ROTATION_NOT_APPLIED_TARGET_COUNT=NOT_MEASURED
Q1_REFERENCE_ACTIVATION_DOMAIN=POST_WHT_EFFECTIVE_CUR
Q1_REFERENCE_VALIDATION_MAX_ABS=NOT_MEASURED
Q1_REFERENCE_VALIDATION_RELATIVE_L2=NOT_MEASURED
Q1_REFERENCE_VALIDATION_COSINE=NOT_MEASURED
Q1_EPISODE_REFERENCE_RECONSTRUCTS_FULL_OPERATOR=NOT_MEASURED
REFERENCE_ERROR_BUDGET=RELATIVE_L2_LE_1e-3_AND_COSINE_GE_0.9999_AND_MAX_ABS_LE_1e-2_OF_PEAK
REFERENCE_ARITHMETIC_MODEL=SOURCE_CONFIRMED_NOT_NATIVE_VALIDATED
REGISTRY_IMPLEMENTED=YES
INVENTORY_LOADER_IMPLEMENTED=YES
PAYLOAD_EXTRACTION_IMPLEMENTED=NO
EXTRACTION_CALL_SITE_IMPLEMENTED=NO
PRODUCT_MMVQ_SOURCE_MODIFIED=NO
MODEL_GGUF_MODIFIED=NO
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
ENGINE007_NEXT_STEP=F1A_S2BIS_I2_BIS_PAYLOAD_EXTRACTION_THEN_BUILD
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```

## 8 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007_PRIMARY_LEVER=EPISODE_ALIGNED_STRUCTURED_WORK_ELIMINATION
ENGINE007B_STATUS=UNBLOCKED_FOR_STRUCTURED_EPISODE_FEASIBILITY_ONLY
ENGINE007B_F1A_STATUS=BLOCKED_CAPTURE_BUILD_GATE
TOTAL_ELIGIBLE_ROWS=2113536
E5_ROW_COUNT=2052096
E6_ROW_COUNT=61440
TQ2_G128_NATIVE_ACTIVATION_DOMAIN=POST_WHT
REFERENCE_Q8_1_TQ2_INVALIDATED_BY_WHT=NO
Q8_1_REFERENCE_UNIT_TESTS=PASS
ROUNDING_HALF_CASES_OK=YES
TQ2_G128_REFERENCE_DECODE=QUALIFIED
EPISODE_PARTITION_COMPLETENESS=PASS_WITHIN_INTRINSIC_FP32_PRECISION
HARNESS_DEFECT_46=RESOLVED
CAPTURE_REGISTRY=IMPLEMENTED
CAPTURE_PAYLOAD_EXTRACTION=NOT_IMPLEMENTED
DIAGNOSTIC_BUILD=NOT_ATTEMPTED
SALIENCY_MEASURED=NO
EPISODE_ALIGNED_STRUCTURED_ELIMINATION=NOT_DETERMINED
ENGINE007B_PRODUCT_OPTIMIZATION_ALLOWED=NO
ENGINE007C_STATUS=BLOCKED
PRODUCT_GAIN=NOT_MEASURED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
