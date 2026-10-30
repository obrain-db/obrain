# ENGINE-007B-F1A-Q0-R6 — CLÔTURE DE L'INFRASTRUCTURE (reprise)

Date : 2026-09-30. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7b-f1a-q0-r6/`

---

## 0 — Je retire une affirmation fausse de mon rapport précédent

Dans le rapport R6 précédent, j'ai écrit :

```
PAYLOAD_SUBMISSION_ATTRIBUTION=OFF_BY_ONE_DOCUMENTED
« l etiquette SUBMISSION de chaque payload est decalee d une soumission »
```

**C'est faux, et je le retire.** En vérifiant la séquence réelle des événements :

```
[ligne 1285] f1a-sub submission=1
[ligne 5318] f1a-res RESIDENT ... bufferless=1176     <- end_epoch, APRES le decode #1
[ligne 5662] f1a-sub submission=2
[ligne 6167] f1a-res RESIDENT ... bufferless=0        <- end_epoch, APRES le decode #2
```

`end_epoch` s'exécute **après** le `llama_decode` qui vient de soumettre. Quand il tourne, la
soumission de ce décodage **est déjà comptée** :

```
decode #1 : f1a_ctr_submission = 1  -> epoque etiquetee sub=1   (correct)
decode #2 : f1a_ctr_submission = 2  -> epoque etiquetee sub=2   (correct)
```

Les tenseurs extraits viennent des données **de ce décodage**. L'étiquette est juste.

```
PAYLOAD_SUBMISSION_ATTRIBUTION=CORRECT_NO_OFFSET
HARNESS_DEFECT_51=WRONG_OFF_BY_ONE_ATTRIBUTION_CLAIM_IN_REPORT
   STATUS=RETRACTED_AFTER_MEASUREMENT
```

**Ce que j'ai fait de travers** : j'ai écrit une correction (`submission_id_at_build` →
`f1a_ctr_submission`) qui a produit un résultat, puis j'ai **interprété une étiquette `{1,2}` comme
un décalage** parce qu'elle ne ressemblait pas à ce que j'attendais (`{0,1}`). Je n'avais pas tracé
l'ordre des événements avant de conclure. C'est la même faute que dans les passes précédentes :
conclure depuis une forme plutôt que depuis une mesure.

## 1 — Ce que R6 a établi

| condition du §11 | état |
|---|---|
| `OUTPUT_DEPENDENT_PROVEN=156` | **YES** |
| `KV_SIDE_EFFECT_PROVEN=12` | **YES** |
| `KV_SIDE_EFFECT_UNRESOLVED=0` | **YES** |
| `TARGET_EFFECT_PATH_COMPLETENESS` | **PASS** |
| `CAPTURE_GRAPH_ROLE_RELATIONSHIPS_PROVEN` | **YES** (cur_bad=0, mm_bad=0, cap_bad=0) |
| `PAYLOAD_ACCOUNTING_EXACT` | **YES** |
| `CAPTURE_DISABLED_RUNTIME_PATH_INERT` | **YES** |
| `KV_SIDE_EFFECT_SINK_QUALIFIED` | **YES** |
| **`PAYLOAD_PROVENANCE_COMPLETE`** | **NO** — SHA256 manquant |
| `BACKEND_EXTRACTION_SEMANTICS_QUALIFIED` | **NO** |
| `SUBMITTED_TARGET_CAPTURE_COMPLETENESS` | non mesuré |

```
F1A_Q0_R6_STATUS=BLOCKED_PAYLOAD_PROVENANCE_GATE
Q0_FULL_RUN_ALLOWED=NO
```

## 2 — Le chemin d'écriture KV, identifié par lecture (§3)

```
OPERATION   llama_kv_cache::cpy_k
SOURCE      src/llama-kv-cache.cpp:1323
POSEE PAR   ggml_build_forward_expand(gf, mctx_cur->cpy_k(ctx0, k_cur, k_idxs, il))
            src/llama-graph.cpp:2879, 2975, 3040, 3140
```

Les quatre occurrences passent par `ggml_build_forward_expand` : **le nœud d'écriture appartient au
graphe soumis**, il n'est pas hors graphe. Et ce n'est **pas** un `GGML_OP_CPY` présumé — c'est la
fonction du fork qui construit l'écriture, comme votre §3 l'exigeait.

Cohérence architecturale : `full_attention_interval=4`, et les 12 `attn_k` sont aux couches 11, 15,
19, 23, 27, 31, 35, 39, 43, 47, 51, 55 — tous les 4 blocs. Cela **confirme** la classification sans
la fonder, comme vous l'avez demandé.

```
OUTPUT_DEPENDENT     156   ffn_up 48, ffn_gate 48, attn_gate 36, attn_q 12, attn_output 12
KV_SIDE_EFFECT        12   attn_k des couches 11..55 par pas de 4
UNEXPLAINED            0
```

## 3 — Payloads : comptes exacts, hashing insuffisant

```
672 payloads : 336 activation + 336 base_output
repartition : 336 sub=1 (ep=0) + 336 sub=2 (ep=1)
672 fichiers .bin
verificateur INDEPENDANT du chemin d ecriture :
   MISSING=0   UNEXPLAINED=0   DUPLICATE=0   HASH_MISMATCH=0
```

Les noms distinguent la soumission : `ep<epoch>_sub<N>_t<target>_c<capture>_<role>.bin`.

**Le défaut réel qui subsiste :**

```
PAYLOAD_HASH_ALGO=fnv64x4
   -> ce n est PAS un SHA256. Le paragraphe D2 exige un SHA256 calcule apres ecriture
      complete. NON SATISFAIT.
```

Le manifeste nomme l'algorithme pour que personne ne confonde les deux, mais cela ne remplace pas
la condition.

## 4 — Backend : distinction conservée (§6)

```
BACKEND_ASSIGNMENT_RECORDED=YES       SCHEDULER_ASSIGNED_BACKEND=CUDA0
BUFFER_TYPE_RECORDED=YES              BUFFER_TYPE=CUDA0
BUFFER_RESIDENT=YES                   337 lignes, tous les tenseurs requis
INDEPENDENT_BUFFER_OWNER_ID=NOT_EXPOSED
   -> ggml_backend_buffer_name() renvoie le nom du TYPE de buffer, pas un proprietaire
      distinct. Je ne demande pas a une API une propriete qu elle n expose pas, et je
      n annonce pas une propriete physique plus forte que ce qui est etabli.
BACKEND_EXTRACTION_SEMANTICS_QUALIFIED=NO
   -> le contrat des API utilisees n a pas ete relu pour etablir que l extraction hote
      est valide apres la synchronisation globale. NON FAIT.
```

## 5 — Preuves acquises, non rouvertes

Faute de contre-exemple : `CAPTURE_DISABLED_RUNTIME_PATH_INERT`,
`EARLY_BUFFERLESS_TARGETS_PROVEN_NON_SUBMITTED`, `SCHEDULER_SYNCHRONIZE_COVERS_CAPTURE_BACKEND`,
les trois compteurs de divergence de rôle à zéro, l'atteignabilité 156/168 et les 12 exceptions KV
expliquées.

## 6 — Rapport final (§14)

```
ENGINE007B_F1A_STATUS=BLOCKED_ACTIVATION_CAPTURE_GATE
F1A_Q0_R6_STATUS=BLOCKED_PAYLOAD_PROVENANCE_GATE
DIAGNOSTIC_BUILD=PASS
COMPUTE_SUBMISSION_COUNT=2
OUTPUT_DEPENDENT_TARGETS=156
OUTPUT_DEPENDENT_PROVEN=156
KV_SIDE_EFFECT_CANDIDATES=12
KV_SIDE_EFFECT_PROVEN=12
KV_SIDE_EFFECT_UNRESOLVED=0
KV_SIDE_EFFECT_SINK_QUALIFIED=YES
KV_WRITE_OP=llama_kv_cache::cpy_k
KV_WRITE_SOURCE=src/llama-kv-cache.cpp:1323
KV_WRITE_IN_SUBMITTED_GRAPH=YES
TARGET_EFFECT_PATH_COMPLETENESS=PASS
CAPTURE_GRAPH_ROLE_RELATIONSHIPS_PROVEN=YES
BACKEND_EXTRACTION_SEMANTICS_QUALIFIED=NO
INDEPENDENT_BUFFER_OWNER_ID=NOT_EXPOSED
PAYLOAD_PROVENANCE_COMPLETE=NO
PAYLOAD_ACCOUNTING_EXACT=YES
PAYLOAD_HASH_ALGO=fnv64x4_NOT_SHA256
TOTAL_PAYLOAD_COUNT=672
PAYLOAD_SUBMISSION_ATTRIBUTION=CORRECT_NO_OFFSET
SUBMITTED_TARGET_CAPTURE_COMPLETENESS=NOT_MEASURED
CAPTURE_DISABLED_RUNTIME_PATH_INERT=YES
NO_REQUIRED_SUBMITTED_TENSOR_SKIPPED=YES
PRODUCT_MMVQ_SOURCE_MODIFIED=NO
MODEL_GGUF_MODIFIED=NO
Q0_FULL_RUN_ALLOWED=NO
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
HARNESS_DEFECT_50_STATUS=RESOLVED_VERIFIED_BY_OFF_ON_DISCRIMINATION
HARNESS_DEFECT_51_STATUS=RETRACTED_AFTER_MEASUREMENT
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
ENGINE007_NEXT_STEP=R7_SHA256_BACKEND_EXTRACTION_SEMANTICS_AND_A_EQUALS_B_EQUALS_C
```

## 7 — Livrables réellement créés

```
KV_WRITE_SOURCE_TRACE.txt        identification par lecture + coherence architecturale
PAYLOAD_ACCOUNTING.txt           comptes exacts + retractation de l affirmation fausse
GRAPH_ROLE_RELATIONSHIPS.csv     169 lignes, colonne effect_class
TARGET_EFFECT_PATH.csv           156 OUTPUT_DEPENDENT + 12 KV_SIDE_EFFECT
PAYLOAD_PROVENANCE.csv           672 lignes (chemin, taille, hash, role)
BACKEND_OWNERSHIP_MANIFEST.csv   337 lignes
SUBMITTED_GRAPH_SNAPSHOTS.csv
RESIDENCY_MANIFEST.csv
EPOCH_CLASSIFICATION.csv
on.txt                           journal brut
```

**Non créés** : `DECLARED_OUTPUT_REACHABILITY.csv`, `KV_SIDE_EFFECT_PATHS.csv`,
`KV_SIDE_EFFECT_VALIDATION.txt`, `TARGET_EFFECT_PATH_COMPLETENESS.txt`,
`BACKEND_EXTRACTION_SEMANTICS.txt`, `SUBMISSION_CAPTURE_MANIFEST.csv`, `PAYLOAD_HASHES.txt`,
`SUBMITTED_TARGET_COMPLETENESS.txt`, `CAPTURE_OFF_RUNTIME_COUNTERS.txt`,
`R6_BUILD_PROVENANCE.txt`, `R6_PATCH_PROVENANCE.txt`, `CONFIG_PROVENANCE.txt`, `R6_STATUS.txt`.

## 8 — R7 : ce qui reste, sans rouvrir le reste

```
R7-a  SHA256 reel des payloads (le harnais peut le calculer apres ecriture, ou appeler
      sha256sum cote hote), et remplacer hash_algo.
R7-b  BACKEND_EXTRACTION_SEMANTICS : relire le contrat de ggml_backend_tensor_get et de
      ggml_backend_sched_synchronize pour etablir que l extraction hote est valide.
R7-c  A == B == C par identifiants de cible, pour chaque soumission acceptee.
```

## 9 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007_PRIMARY_LEVER=EPISODE_ALIGNED_STRUCTURED_WORK_ELIMINATION
ENGINE007B_STATUS=UNBLOCKED_FOR_STRUCTURED_EPISODE_FEASIBILITY_ONLY
ENGINE007B_F1A_STATUS=BLOCKED_ACTIVATION_CAPTURE_GATE
DIAGNOSTIC_BUILD=PASS
CAPTURE_DISABLED_RUNTIME_PATH_INERT=YES
EARLY_BUFFERLESS_TARGETS_PROVEN_NON_SUBMITTED=YES
OUTPUT_DEPENDENT_PROVEN=156
KV_SIDE_EFFECT_PROVEN=12
KV_SIDE_EFFECT_UNRESOLVED=0
KV_SIDE_EFFECT_SINK_QUALIFIED=YES
TARGET_EFFECT_PATH_COMPLETENESS=PASS
CAPTURE_REGISTER_POINTERS_MATCH_SUBMITTED_GRAPH=YES
CUR_CAPTURE_ROLE_MISMATCH_COUNT=0
MUL_MAT_ROLE_MISMATCH_COUNT=0
MM_CAPTURE_ROLE_MISMATCH_COUNT=0
BACKEND_ASSIGNMENT_RECORDED=YES
INDEPENDENT_BUFFER_OWNER_ID=NOT_EXPOSED
BACKEND_EXTRACTION_SEMANTICS_QUALIFIED=NO
SCHEDULER_SYNCHRONIZE_COVERS_CAPTURE_BACKEND=YES
PAYLOAD_ACCOUNTING_EXACT=YES
PAYLOAD_PROVENANCE_COMPLETE=NO
PAYLOAD_SUBMISSION_ATTRIBUTION=CORRECT_NO_OFFSET
SUBMITTED_TARGET_CAPTURE_COMPLETENESS=NOT_MEASURED
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
Q8_1_REFERENCE_UNIT_TESTS=PASS
ROUNDING_HALF_CASES_OK=YES
TQ2_G128_REFERENCE_DECODE=QUALIFIED
HARNESS_DEFECT_46=RESOLVED
HARNESS_DEFECT_47=RESOLVED
HARNESS_DEFECT_48=RESOLVED
HARNESS_DEFECT_49=RESOLVED
HARNESS_DEFECT_50=RESOLVED_VERIFIED_BY_OFF_ON_DISCRIMINATION
HARNESS_DEFECT_51=RETRACTED_AFTER_MEASUREMENT
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
