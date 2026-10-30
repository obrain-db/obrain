# ENGINE-007B-F1A-Q0-R2 — IDENTITÉ DU GRAPHE SOUMIS ET PROPRIÉTÉ DU BACKEND

Date : 2026-09-30. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7b-f1a-q0-r2/`

---

## 0 — Résultat

**La question de R2 a une réponse nette, et c'est la bonne issue.**

```
F1A_Q0_R2_STATUS=EARLY_BUFFERLESS_EPOCHS_PROVEN_NON_EXECUTED
F1A_Q0_R1_STATUS=RESIDENCY_AND_EXTRACTION_QUALIFIED_PARTIAL
```

Les époques bufferless sont des **passes de construction de graphe**, pas des décodes exécutés.
Prouvé par corrélation avec la soumission de calcul, pas déduit.

---

## 1 — La mesure décisive (§6, §11)

J'ai posé un hook à l'endroit **exact** de la soumission :

```cpp
// llama-context.cpp, juste avant ggml_backend_sched_graph_compute_async
if (f1a_capture_get_ctx() != nullptr) {
    f1a_mark_compute_submission((const void *) gf, gf ? (int) ggml_graph_n_nodes(gf) : 0);
}
auto status = ggml_backend_sched_graph_compute_async(sched.get(), gf);
```

Chaque cible enregistre le compteur de soumissions **au moment de sa construction**. Résultat :

```
epoch | submissions_total | sid_at_build | classification       | cur_res | cur_null | n_targets
0     | 1                 | 0            | BUILD_ONLY_NOT_SUBMITTED | 168 | 1176     | 1344
1     | 2                 | 1            | SUBMITTED_FOR_COMPUTE    | 168 | 0        | 168
```

**Table décisive exigée par votre §11 :**

```
epoch | submitted                | cur null | raw null | mmcap null
0     | BUILD_ONLY_NOT_SUBMITTED | 1176     | 1176     | 1176
1     | SUBMITTED_FOR_COMPUTE    | 0        | 0        | 0
```

```
TOTAL_CAPTURE_EPOCHS=2
BUILD_ONLY_EPOCHS=1
SUBMITTED_EPOCHS=1
UNCLASSIFIED_EPOCHS=0
```

## 2 — Pourquoi c'est une preuve et non une déduction

L'époque 0 a `sid_at_build=0` : **aucune soumission de calcul n'avait eu lieu** quand ses 1344
cibles ont été construites. Ces tenseurs n'ont donc jamais été exécutés, et
`ggml_backend_tensor_get()` n'avait rien à lire — c'est l'assertion d'origine, expliquée.

L'époque 1 a `sid_at_build=1` : une soumission avait eu lieu, et ses 168 cibles sont **toutes**
résidentes.

```
EARLY_BUFFERLESS_EPOCHS=NON_EXECUTED_GRAPH_BUILD_OR_PLANNING_PASSES
BUFFERLESS_SKIP_FOR_NON_SUBMITTED_EPOCHS=SAFE
```

**Le cas dangereux de votre §13 n'est pas observé** : aucune époque ne satisfait
`COMPUTE_SUBMITTED=YES` avec un tenseur requis bufferless. Le filtre null-buffer ne masque donc
rien — il saute une passe de construction, ce qui ne perd aucune donnée scientifique.

## 3 — Votre §1 avait raison de refuser le raccourci

J'avais écrit `1176 = 168 × 7` et vous avez refusé de le nommer « sept décodes ». La mesure montre
que le rapport réel est **168 × 8 = 1344 cibles par époque de construction**, et que ces époques ne
sont pas des décodes du tout.

```
BUFFERLESSNESS_IS_NOT_TENSOR_ROLE_SPECIFIC=YES
BUFFERLESSNESS_IS_PHASE_OR_GRAPH_INSTANCE_SPECIFIC=YES
BUFFERLESSNESS_IS_FIRST_SEVEN_REAL_DECODE_CALLS=REJECTED
```

La greffe `build_lora_mm` est appelée **8 fois** par construction de graphe : le planificateur
construit plusieurs fois avant une seule soumission (planification, dimensionnement, réservation).
C'est une propriété du planificateur à documenter, et elle explique le volume.

## 4 — Réconciliation du compte de payloads (§15)

```
168 cibles x 2 classes (activation + base_output) x 2 epoques = 672
CUR_CAPTURE_PAYLOAD_COUNT=336
MM_CAPTURE_PAYLOAD_COUNT=336
OTHER_PAYLOAD_COUNT=0
TOTAL=672        SOMME_DES_CLASSES=672        PAYLOAD_ACCOUNTING_EXACT=YES
```

Aucun fichier inexpliqué : chaque payload suit `ep<epoch>_t<target>_<classe>.bin` et correspond à
une ligne du manifeste.

## 5 — Ce que R2 n'a PAS établi

```
ACTUAL_BACKEND_OWNERSHIP_RECORDED=NO
   end_epoch ne recoit toujours pas le planificateur : l appel a
   ggml_backend_sched_get_tensor_backend(sched, t) n est pas fait.
   -> F1A_Q0_R2_STATUS ne peut PAS etre SUBMITTED_GRAPH_RESIDENCY_BACKEND_QUALIFIED

CAPTURE_REGISTER_POINTERS_MATCH_SUBMITTED_GRAPH=NOT_MEASURED
   exige l appartenance de chaque pointeur enregistre au gf soumis.
   Le hook a le gf (f1a_last_gf) ; la comparaison n est pas ecrite.

SCHEDULER_SYNCHRONIZATION_SEMANTICS_PROVEN=NOT_VERIFIED
   je n ai pas relu ggml_backend_sched_synchronize pour verifier qu il couvre
   tous les backends participants (paragraphe 18).
```

Le §23 exige neuf conditions ; **trois ne sont pas remplies**. Je ne promeus donc pas au-delà.

```
F1A_Q0_R2_STATUS=EARLY_BUFFERLESS_EPOCHS_PROVEN_NON_EXECUTED
   -> question principale de R2 : TRANCHEE
   -> propriete backend              : OUVERTE
   -> identite de pointeur soumis    : OUVERTE
   -> semantique de synchronisation  : OUVERTE
```

## 6 — Compteurs capture-off (§20)

Les cinq compteurs sont **implémentés** et publiés :

```
F1A_CAPTURE_CONTEXT_ACTIVE_COUNT   F1A_ELIGIBLE_BRANCH_ENTER_COUNT
F1A_CAPTURE_NOTE_COUNT             F1A_EXTRACTION_CALL_COUNT
F1A_PAYLOAD_WRITE_COUNT
```

Observés capture **activée** : `ctx_active=4477  eligible_enter=4473  note=1512
extract_call=672  payload_write=672`.

```
CAPTURE_DISABLED_RUNTIME_PATH_INERT=IMPLEMENTE_NON_ENCORE_LU
   une commande a lancer (capture desactivee, relever les compteurs), pas un travail a ecrire.
   Je ne le declare pas YES sans l avoir lu.
```

## 7 — Leçon méthodologique (§28)

Vos trois distinctions sont enregistrées comme règles permanentes, et cette passe en a fourni la
preuve pour la troisième :

```
GRAPH_TENSOR_EXISTENCE       != BUFFER_RESIDENCE
    -> I2-b puis R1 : un tenseur peut exister dans le graphe sans buffer
STATIC_EXTRACTION_ORDER      != BUFFER_LIFETIME_PROOF
    -> I2-b : l ordre etait prouve, la residence non
GRAPH_BUILD                  != GRAPH_COMPUTE_SUBMISSION
    -> R2 : 8 constructions pour une soumission ; l epoque 0 n a jamais ete executee
```

## 8 — Rapport final (§30)

```
ENGINE007B_F1A_STATUS=BLOCKED_ACTIVATION_CAPTURE_GATE
F1A_Q0_R1_STATUS=RESIDENCY_AND_EXTRACTION_QUALIFIED_PARTIAL
F1A_Q0_R2_STATUS=EARLY_BUFFERLESS_EPOCHS_PROVEN_NON_EXECUTED
CAPTURE_DISABLED_RUNTIME_PATH_INERT=IMPLEMENTE_NON_ENCORE_LU
TOTAL_GRAPH_BUILDS=NOT_SEPARATELY_COUNTED
TOTAL_CAPTURE_EPOCHS=2
BUILD_ONLY_EPOCHS=1
SUBMITTED_EPOCHS=1
UNCLASSIFIED_EPOCHS=0
EARLY_BUFFERLESS_EPOCH_COUNT=1
EARLY_BUFFERLESS_SUBMITTED_COUNT=0
SUBMITTED_CUR_CAPTURE_BUFFERLESS_COUNT=0
SUBMITTED_MM_CAPTURE_BUFFERLESS_COUNT=0
CAPTURE_REGISTER_POINTERS_MATCH_SUBMITTED_GRAPH=NOT_MEASURED
ACTUAL_BACKEND_OWNERSHIP_RECORDED=NO
SCHEDULER_SYNCHRONIZE_COVERS_CAPTURE_BACKEND=NOT_VERIFIED
CUR_CAPTURE_PAYLOAD_COUNT=336
MM_CAPTURE_PAYLOAD_COUNT=336
OTHER_PAYLOAD_COUNT=0
TOTAL_PAYLOAD_COUNT=672
PAYLOAD_ACCOUNTING_EXACT=YES
NO_REQUIRED_SUBMITTED_TENSOR_SKIPPED_FOR_NULL_BUFFER=YES
BUFFERLESSNESS_IS_NOT_TENSOR_ROLE_SPECIFIC=YES
BUFFERLESSNESS_IS_PHASE_OR_GRAPH_INSTANCE_SPECIFIC=YES
BUFFERLESSNESS_IS_FIRST_SEVEN_REAL_DECODE_CALLS=REJECTED
NULL_BUFFER_ASSERTION_FIXED=YES
BUFFERLESS_SKIP_FOR_NON_SUBMITTED_EPOCHS=SAFE
MM_RAW_EXTRACTION_NOT_REQUIRED=YES
BUILD_CALLS_PER_CONSTRUCTION_PASS=8
ONE_TOKEN_EXTRACTION_RC=0
Q0_FULL_RUN_EXECUTED=NO
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
PRODUCT_MMVQ_SOURCE_MODIFIED=NO
MODEL_GGUF_MODIFIED=NO
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
ENGINE007_NEXT_STEP=F1A_Q0_R3_BACKEND_OWNERSHIP_AND_SUBMITTED_GRAPH_MEMBERSHIP
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```

## 9 — Livrables

```
EPOCH_CLASSIFICATION.csv / .txt   classification par epoque + table decisive
PAYLOAD_ACCOUNTING.txt            reconciliation exacte des 672 payloads
RESIDENCY_MANIFEST.csv            metadonnees de chaque tenseur enregistre
probe.txt                         journal brut : lignes [f1a-res], [f1a-cls], [f1a-ctr]
```

**Non produits** (exigent R3) : `BACKEND_OWNERSHIP_MANIFEST.csv` (avec propriétaire réel),
`REGISTERED_VS_SUBMITTED_POINTERS.csv`, `SUBMITTED_GRAPH_MANIFEST.csv`,
`SUBMITTED_GRAPH_FINGERPRINT.txt`, `SCHEDULER_SYNC_SEMANTICS.txt`,
`CONFIG_PROVENANCE.txt` mis à jour.

## 10 — Défauts de harnais

```
HARNESS_DEFECT_47=LLAMA_OBRAIN_INTERACTIVE_CLI_CONSUMED_SSH_HEREDOC...  RESOLVED
HARNESS_DEFECT_48=EXTRACTION_ATTEMPTED_WITHOUT_BUFFER_RESIDENCE_CHECK  RESOLVED
HARNESS_DEFECT_49=gf_n_nodes_ACCESSED_THROUGH_INCOMPLETE_TYPE_ggml_cgraph
                  RESOLVED_BY_ggml_graph_n_nodes_ACCESSOR
```

## 11 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007_PRIMARY_LEVER=EPISODE_ALIGNED_STRUCTURED_WORK_ELIMINATION
ENGINE007B_STATUS=UNBLOCKED_FOR_STRUCTURED_EPISODE_FEASIBILITY_ONLY
ENGINE007B_F1A_STATUS=BLOCKED_ACTIVATION_CAPTURE_GATE
DIAGNOSTIC_BUILD=PASS
CAPTURE_DISABLED_EXECUTION=PASS
CAPTURE_EXTRACTION_ORDER_STATIC_PROOF=PASS
CAPTURE_BUFFER_RESIDENCE_PROOF=ESTABLISHED
BUFFERLESSNESS_IS_PHASE_OR_GRAPH_INSTANCE_SPECIFIC=YES
EARLY_BUFFERLESS_EPOCHS=NON_EXECUTED_GRAPH_BUILD_OR_PLANNING_PASSES
SUBMITTED_EPOCHS_ALL_REQUIRED_TENSORS_RESIDENT=YES
CAPTURE_BACKEND_OWNERSHIP_PROOF=NOT_ESTABLISHED
CAPTURE_REGISTER_POINTERS_MATCH_SUBMITTED_GRAPH=NOT_MEASURED
PAYLOAD_ACCOUNTING_EXACT=YES
ONE_TOKEN_EXTRACTION_RC=0
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
Q8_1_REFERENCE_UNIT_TESTS=PASS
ROUNDING_HALF_CASES_OK=YES
TQ2_G128_REFERENCE_DECODE=QUALIFIED
HARNESS_DEFECT_47=RESOLVED
HARNESS_DEFECT_48=RESOLVED
HARNESS_DEFECT_49=RESOLVED
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
