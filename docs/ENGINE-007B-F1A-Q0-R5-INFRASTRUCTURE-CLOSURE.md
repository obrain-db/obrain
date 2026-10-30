# ENGINE-007B-F1A-Q0-R5 — FERMETURE DE L'INFRASTRUCTURE (reprise)

Date : 2026-09-30. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7b-f1a-q0-r5/`

---

## 0 — Résultat

**Le problème d'atteignabilité est résolu à 156/168, et les 12 exceptions sont expliquées.**

```
F1A_Q0_R5_STATUS=DOWNSTREAM_REACHABILITY_QUALIFIED_WITH_DOCUMENTED_EXCEPTIONS
Q0_FULL_RUN_ALLOWED=NO
```

## 1 — J'ai cessé de deviner, et j'ai lu

Deux tentatives par propriété topologique avaient échoué (129 puis 153 échecs). Votre §C4 interdit
de deviner « les racines réellement utilisées ». J'ai donc **lu la source** :

```
src/llama-graph.cpp:1305-1337   ggml_set_output(t_logits) ; ggml_set_output(t_embd) ; ...
ggml/include/ggml.h:662         GGML_TENSOR_FLAG_OUTPUT = 2
                                « is an output for the GGML compute graph »
```

**La sortie est DÉCLARÉE par un drapeau, pas par une position dans le tableau.** Le snapshot fige
désormais `tensor->flags`, et l'atteignabilité se calcule en remontant `src[]` depuis **toutes** les
sorties déclarées, en une passe linéaire.

```
[f1a-out] sorties DECLAREES du graphe soumis = 3
[f1a-role] targets_soumis=168  cur_bad=0  mm_bad=0  cap_bad=0  down_bad=12
```

Progression de la même question, selon la méthode :

| méthode | échecs |
|---|---|
| deviner : dernier `mm_capture` | 129 |
| deviner : nœud non consommé | 153 |
| **lire le drapeau déclaré** | **12** |

## 2 — Les 12 exceptions sont identifiées, et ce ne sont pas des erreurs

```
12 blk.N.attn_k.weight
   blk.11, 15, 19, 23, 27, 31, 35, 39, 43, 47, 51, 55
```

**Tous les quatre blocs** — et `qwen35.full_attention_interval=4` dans les métadonnées du modèle.
Ce sont les **clés d'attention**, projetées puis **écrites dans le cache KV**, qui n'ont aucun
consommateur marqué dans le graphe de calcul. Leur destination est un effet de bord, pas une arête.

Les 156 autres atteignent une sortie déclarée :

```
48 ffn_up        48 ffn_gate        36 attn_gate        12 attn_q        12 attn_output
```

```
MM_CAPTURE_REACHES_LIVE_GRAPH_OUTPUT=YES_POUR_156_SUR_168
EXCEPTIONS=12_attn_k_written_to_KV_CACHE_WITHOUT_MARKED_GRAPH_CONSUMER
   -> ce ne sont PAS des defauts : c est la topologie reelle du modele
   -> aucune de ces 12 n est requise pour la qualification F1A (E5 et E6 sont
      couverts par ffn_up, ffn_gate, attn_gate, attn_q et attn_output)
```

## 3 — Statut des quatre gates R5

| gate | état |
|---|---|
| **R5-b** `ACTUAL_BACKEND_OWNERSHIP_RECORDED` | **YES** — `sched_backend=CUDA0`, `buffer_type=CUDA0`, 337 lignes |
| **R5-c** relations structurelles §C3 | **YES** — `cur_bad=0  mm_bad=0  cap_bad=0` (168/168) |
| **R5-c** atteignabilité §C4 | **156/168**, 12 exceptions documentées et expliquées |
| **R5-d** `PAYLOAD_PROVENANCE_COMPLETE` | **NON IMPLÉMENTÉ** |
| **R5-e** `SUBMITTED_TARGET_CAPTURE_COMPLETENESS` | **NON MESURÉ** |

Réserve maintenue sur R5-b : `ggml_backend_buffer_name()` renvoie le nom du **type** de buffer, pas
un propriétaire distinct. Je prouve que l'assignation et le buffer sont **tous deux CUDA0** et que
le buffer existe — pas une identité de propriétaire indépendante.

## 4 — Contrôles OFF / ON

```
OFF : ctx_active=0  probe=4477  eligible_enter=0  note=0  extract_call=0  payload_write=0
ON  : ctx_active=4479  note=1512  payload_write=672  dup_added=3024
```

```
CAPTURE_DISABLED_RUNTIME_PATH_INERT=YES
HARNESS_DEFECT_50_STATUS=RESOLVED_VERIFIED_BY_OFF_ON_DISCRIMINATION
   (verifie par la discrimination OFF/ON, pas par l ecriture du patch)
```

## 5 — Rapport final (§4)

```
ENGINE007B_F1A_STATUS=BLOCKED_ACTIVATION_CAPTURE_GATE
F1A_Q0_R5_STATUS=DOWNSTREAM_REACHABILITY_QUALIFIED_WITH_DOCUMENTED_EXCEPTIONS
DIAGNOSTIC_BUILD=PASS
CAPTURE_DISABLED_RUNTIME_PATH_INERT=YES
EARLY_BUFFERLESS_TARGETS_PROVEN_NON_SUBMITTED=YES
COMPUTE_SUBMISSION_COUNT=2
ACTUAL_BACKEND_OWNERSHIP_RECORDED=YES
SCHEDULER_BACKEND_AND_BUFFER_OWNER_CONSISTENT=YES_SUR_NOM_ET_TYPE
SCHEDULER_SYNCHRONIZE_COVERS_CAPTURE_BACKEND=YES
CAPTURE_REGISTER_POINTERS_MATCH_SUBMITTED_GRAPH=YES
CAPTURE_GRAPH_ROLE_RELATIONSHIPS_PROVEN=PARTIAL_156_SUR_168_AVEC_EXCEPTIONS_DOCUMENTEES
CUR_CAPTURE_ROLE_MISMATCH_COUNT=0
MUL_MAT_ROLE_MISMATCH_COUNT=0
MM_CAPTURE_ROLE_MISMATCH_COUNT=0
DOWNSTREAM_REACHABILITY_FAILURE_COUNT=12
DOWNSTREAM_REACHABILITY_FAILURES_ARE=attn_k_written_to_KV_cache
SUBMITTED_GRAPH_DECLARED_OUTPUT_COUNT=3
TARGET_INVOCATION_COUNT=168_par_epoque_soumise
UNIQUE_CUR_CAPTURE_COUNT=NON_MESURE
UNIQUE_MM_CAPTURE_COUNT=NON_MESURE
TOTAL_PAYLOAD_COUNT=672
PAYLOAD_PROVENANCE_COMPLETE=NO
PAYLOAD_ACCOUNTING_EXACT=YES_POUR_R3
SUBMITTED_TARGET_CAPTURE_COMPLETENESS=NOT_MEASURED
NO_REQUIRED_SUBMITTED_TENSOR_SKIPPED=YES
HARNESS_DEFECT_50_STATUS=RESOLVED_VERIFIED_BY_OFF_ON_DISCRIMINATION
PRODUCT_MMVQ_SOURCE_MODIFIED=NO
MODEL_GGUF_MODIFIED=NO
Q0_FULL_RUN_ALLOWED=NO
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
ENGINE007_NEXT_STEP=F1A_Q0_R6_PAYLOAD_PROVENANCE_AND_TARGET_COMPLETENESS
```

## 6 — Leçon de méthode

Trois tentatives sur la même question :

```
deviner par la forme du graphe  -> 129 echecs
deviner par une autre forme     -> 153 echecs  (pire)
lire la declaration explicite   ->  12 echecs, tous expliques
```

Le fait que la deuxième devinette soit **pire** que la première aurait dû me faire conclure plus tôt
que le problème était ma méthode, pas mon paramétrage. Une troisième devinette aurait encore échoué :
c'est la **lecture de la source** qui a tranché.

## 7 — Livrables

```
BACKEND_OWNERSHIP_MANIFEST.csv       337 lignes (168 cibles x 2 roles)
GRAPH_ROLE_RELATIONSHIPS.csv         169 lignes, une par cible soumise
SCHEDULER_BACKEND_CONSISTENCY.txt     coherence + reserve sur « proprietaire »
DOWNSTREAM_REACHABILITY_PROOF.txt    les 12 exceptions et leur explication
CAPTURE_OFF_RUNTIME_COUNTERS.txt     les cinq zeros
on.txt / off.txt                     journaux bruts
```

**Non créés** : `SUBMISSION_CAPTURE_MANIFEST.csv`, `PAYLOAD_PROVENANCE.csv`, `PAYLOAD_HASHES.txt`,
`PAYLOAD_ACCOUNTING.txt`, `SUBMITTED_TARGET_COMPLETENESS.txt`, `SUBMITTED_GRAPH_EDGES.csv`,
`R5_BUILD_PROVENANCE.txt`, `R5_PATCH_PROVENANCE.txt`, `R5_STATUS.txt`, `CONFIG_PROVENANCE.txt`.

## 8 — Ce qui reste pour fermer R5

```
R5-d  SHA256 par payload + COMPUTE_SUBMISSION_ID + HARNESS_DECODE_STEP + nom de fichier
      distinguant les soumissions, puis controle independant (existence, taille, hash)
      -> PAYLOAD_PROVENANCE_COMPLETE
R5-e  A == B == C par identifiants de cible
      -> SUBMITTED_TARGET_CAPTURE_COMPLETENESS
```

Ces deux points ne sont pas écrits. `Q0` complet et `Q1` restent interdits.

## 9 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007_PRIMARY_LEVER=EPISODE_ALIGNED_STRUCTURED_WORK_ELIMINATION
ENGINE007B_STATUS=UNBLOCKED_FOR_STRUCTURED_EPISODE_FEASIBILITY_ONLY
ENGINE007B_F1A_STATUS=BLOCKED_ACTIVATION_CAPTURE_GATE
DIAGNOSTIC_BUILD=PASS
CAPTURE_DISABLED_RUNTIME_PATH_INERT=YES
EARLY_BUFFERLESS_TARGETS_PROVEN_NON_SUBMITTED=YES
ACTUAL_BACKEND_OWNERSHIP_RECORDED=YES
SCHEDULER_SYNCHRONIZE_COVERS_CAPTURE_BACKEND=YES
CAPTURE_REGISTER_POINTERS_MATCH_SUBMITTED_GRAPH=YES
CUR_CAPTURE_ROLE_MISMATCH_COUNT=0
MUL_MAT_ROLE_MISMATCH_COUNT=0
MM_CAPTURE_ROLE_MISMATCH_COUNT=0
DOWNSTREAM_REACHABILITY=156_SUR_168_EXCEPTIONS_DOCUMENTEES
SUBMITTED_GRAPH_DECLARED_OUTPUT_COUNT=3
PAYLOAD_PROVENANCE_COMPLETE=NO
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
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
