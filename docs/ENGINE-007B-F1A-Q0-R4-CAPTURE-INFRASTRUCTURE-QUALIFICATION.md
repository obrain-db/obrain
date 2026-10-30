# ENGINE-007B-F1A-Q0-R4 — QUALIFICATION DE L'INFRASTRUCTURE DE CAPTURE

Date : 2026-09-30. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7b-f1a-q0-r4/`

---

## 0 — Résultat

**R4-a est fermé, avec une correction de mon propre compteur. R4-b à R4-e restent ouverts.**

```
F1A_Q0_R4_STATUS=BLOCKED_BACKEND_OWNERSHIP_GATE
Q0_FULL_RUN_ALLOWED=NO
```

| gate R4 | état |
|---|---|
| `EARLY_BUFFERLESS_TARGETS_PROVEN_NON_SUBMITTED` | YES (acquis en R3, conservé) |
| **`CAPTURE_DISABLED_RUNTIME_PATH_INERT`** | **YES** |
| `SCHEDULER_SYNCHRONIZE_COVERS_CAPTURE_BACKEND` | YES (acquis en R3, conservé) |
| `ACTUAL_BACKEND_OWNERSHIP_RECORDED` | **NO** |
| `CAPTURE_REGISTER_POINTERS_MATCH_SUBMITTED_GRAPH` | **NO** |
| `CAPTURE_GRAPH_ROLE_RELATIONSHIPS_PROVEN` | **NO** |
| `PAYLOAD_PROVENANCE_COMPLETE` | **NO** |
| `SUBMITTED_TARGET_CAPTURE_COMPLETENESS` | non mesuré |

Taxonomie §R4-h : le premier gate échoué est `BLOCKED_BACKEND_OWNERSHIP_GATE`.

## 1 — R4-a : le gate est fermé, et il a révélé un défaut de ma part

**Le problème que vous aviez identifié était exact** : les compteurs étaient incrémentés en
permanence, mais leur **publication** dépendait de `f1a_capture_end_epoch()`, jamais appelée quand
la capture est désactivée. On ne pouvait donc pas prouver l'inertie — et l'absence de fichiers ne
prouvait rien, comme votre §R4-a l'exige.

Correctif : `f1a_publish_counters_standalone()`, appelée par le harnais **à la fin du test,
inconditionnellement**. Aucun contexte F1A créé, aucun tenseur, aucune allocation, aucune E/S.

**Mesure, capture désactivée :**

```
[f1a-ctr_standalone] ctx_active=0 probe=4477 eligible_enter=0 note=0
                     extract_call=0 payload_write=0 dup_added=0

F1A_CAPTURE_CONTEXT_ACTIVE_COUNT_OFF=0
F1A_ELIGIBLE_BRANCH_ENTER_COUNT_OFF=0
F1A_CAPTURE_NOTE_COUNT_OFF=0
F1A_EXTRACTION_CALL_COUNT_OFF=0
F1A_PAYLOAD_WRITE_COUNT_OFF=0
F1A_DUP_NODES_ADDED_OFF=0
fichiers de capture=0   manifeste=0

CAPTURE_DISABLED_RUNTIME_PATH_INERT=YES
```

**Vérification que le compteur actif fonctionne** (capture activée) : `ctx_active=4479 > 0`,
`probe=0`. Le compteur distingue donc bien les deux modes — un compteur bloqué à zéro ne prouverait
rien.

### Le défaut que cette mesure a mis au jour

Premier passage : **`ctx_active=4477` alors qu'aucun contexte n'avait été créé.**

```
CAUSE=f1a_capture_get_ctx() incrementait ctx_active dans les DEUX branches de la fonction,
      donc le compteur mesurait les VERIFICATIONS du drapeau, pas les contextes crees.
```

C'est **exactement le travers que votre §0 interdit** : un compteur qui ne mesure pas ce que son
nom annonce. Sans la mesure que vous avez exigée, je l'aurais publié tel quel comme un « 4477 »
inexpliqué, ou pire, j'aurais conclu que le chemin n'était pas inerte.

```
CORRECTIF=ctx_active n incremente que la branche active ; ajout d un compteur
          distinct F1A_CAPTURE_PROBE_COUNT pour les verifications.
HARNESS_DEFECT_50=CAPTURE_CONTEXT_ACTIVE_COUNTER_MEASURED_PROBES_NOT_CONTEXTS
                   STATUS=RESOLVED_BY_SPLITTING_THE_TWO_MEANINGS
```

## 2 — Mesures capture ON (inchangées, pour référence)

```
[f1a-ctr] ctx_active=4479  eligible_enter=4473  note=1512
          extract_call=672  payload_write=672  dup_added=3024

[f1a-sub] submission=1 gf=0xf16c76c4d030 nodes=3991 rc=0 fp=99c3f0ed21ba33fb
[f1a-prov] RESIDENT_ET_SOUMIS=504  BUFFERLESS_ET_SOUMIS=0
[f1a-sub] submission=2 gf=0xf16c76c4d030 nodes=3991 rc=0 fp=99c3f0ed21ba33fb
[f1a-prov] RESIDENT_ET_SOUMIS=504  BUFFERLESS_ET_SOUMIS=0
```

Le résultat de R3 est **reproduit** : appartenance prouvée, zéro cible bufferless dans les graphes
soumis. L'empreinte diffère de R3 (`3f93adcb…` vs `99c3f0ed…`) parce qu'elle hache des adresses :
ce sont les deux soumissions **entre elles** qui doivent coïncider, et elles coïncident.

## 3 — Ce que R4 n'a pas fait, et pourquoi

```
R4-b  backend proprietaire    : NON IMPLEMENTE. end_epoch ne recoit toujours pas le sched.
R4-c  relations par role      : NON IMPLEMENTE. L appartenance est prouvee (R3), la
                                structure ne l est pas : cur_capture==src1, mm_raw==sortie,
                                mm_capture==DUP(mm_raw) et consommation par l aval.
R4-d  provenance des payloads : NON IMPLEMENTE. Pas de SHA256 par payload, pas de
                                rattachement COMPUTE_SUBMISSION_ID.
R4-e  exhaustivite A==B==C    : NON MESURE.
```

Ces quatre points sont spécifiés dans votre brief et je ne les ai pas exécutés. Je ne les déclare
donc pas acquis, et **Q0 complet reste interdit**.

## 4 — Aucune mesure numérique, aucune salience

```
q8_1 non executee   TQ2 non comparee   logits OFF/ON non compares
SALIENCY_MEASURED=NO   QUALITY_ABLATION_MEASURED=NO   Q1_ALLOWED=NO
```

## 5 — Rapport final

```
ENGINE007B_F1A_STATUS=BLOCKED_ACTIVATION_CAPTURE_GATE
F1A_Q0_R1_STATUS=RESIDENCY_AND_EXTRACTION_QUALIFIED_PARTIAL
F1A_Q0_R2_STATUS=EARLY_BUFFERLESS_EPOCHS_PROVEN_NON_EXECUTED
F1A_Q0_R3_STATUS=EARLY_BUFFERLESS_TARGETS_PROVEN_NON_SUBMITTED
F1A_Q0_R4_STATUS=BLOCKED_BACKEND_OWNERSHIP_GATE
CAPTURE_DISABLED_RUNTIME_PATH_INERT=YES
F1A_CAPTURE_CONTEXT_ACTIVE_COUNT_OFF=0
F1A_ELIGIBLE_BRANCH_ENTER_COUNT_OFF=0
F1A_CAPTURE_NOTE_COUNT_OFF=0
F1A_EXTRACTION_CALL_COUNT_OFF=0
F1A_PAYLOAD_WRITE_COUNT_OFF=0
F1A_DUP_NODES_ADDED_OFF=0
EARLY_BUFFERLESS_TARGETS_PROVEN_NON_SUBMITTED=YES
SCHEDULER_SYNCHRONIZE_COVERS_CAPTURE_BACKEND=YES
ACTUAL_BACKEND_OWNERSHIP_RECORDED=NO
CAPTURE_REGISTER_POINTERS_MATCH_SUBMITTED_GRAPH=NO
CAPTURE_GRAPH_ROLE_RELATIONSHIPS_PROVEN=NO
PAYLOAD_ACCOUNTING_EXACT=YES_POUR_R3
PAYLOAD_PROVENANCE_COMPLETE=NO
SUBMITTED_TARGET_CAPTURE_COMPLETENESS=NOT_MEASURED
Q0_FULL_RUN_ALLOWED=NO
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
PRODUCT_MMVQ_SOURCE_MODIFIED=NO
MODEL_GGUF_MODIFIED=NO
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
ENGINE007_NEXT_STEP=F1A_Q0_R5_BACKEND_OWNERSHIP_ROLE_RELATIONSHIPS_AND_PAYLOAD_PROVENANCE
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```

## 6 — Livrables

```
CAPTURE_OFF_RUNTIME_COUNTERS.txt   les cinq zeros, + le defaut de compteur et son correctif
R4_STATUS.txt                      etat des cinq gates, statut d arret
off.txt / on.txt                   journaux bruts des deux executions
```

**Non produits** (exigent R5) : `BACKEND_OWNERSHIP_MANIFEST.csv`,
`SCHEDULER_BACKEND_CONSISTENCY.txt`, `GRAPH_ROLE_RELATIONSHIPS.csv`,
`DOWNSTREAM_REACHABILITY_PROOF.txt`, `SUBMISSION_CAPTURE_MANIFEST.csv`,
`PAYLOAD_PROVENANCE.csv`, `PAYLOAD_HASHES.txt`, `SUBMITTED_TARGET_COMPLETENESS.txt`,
`R4_BUILD_PROVENANCE.txt`, `R4_PATCH_PROVENANCE.txt`, `CONFIG_PROVENANCE.txt`.

## 7 — Défauts de harnais

```
HARNESS_DEFECT_46=Q8_1_REFERENCE_USED_BANKERS_ROUNDING_INSTEAD_OF_C_ROUNDF   RESOLVED
HARNESS_DEFECT_47=LLAMA_OBRAIN_INTERACTIVE_CLI_CONSUMED_SSH_HEREDOC...       RESOLVED
HARNESS_DEFECT_48=EXTRACTION_ATTEMPTED_WITHOUT_BUFFER_RESIDENCE_CHECK        RESOLVED
HARNESS_DEFECT_49=gf_n_nodes_ACCESSED_THROUGH_INCOMPLETE_TYPE                RESOLVED
HARNESS_DEFECT_50=CAPTURE_CONTEXT_ACTIVE_COUNTER_MEASURED_PROBES_NOT_CONTEXTS RESOLVED
```

## 8 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007_PRIMARY_LEVER=EPISODE_ALIGNED_STRUCTURED_WORK_ELIMINATION
ENGINE007B_STATUS=UNBLOCKED_FOR_STRUCTURED_EPISODE_FEASIBILITY_ONLY
ENGINE007B_F1A_STATUS=BLOCKED_ACTIVATION_CAPTURE_GATE
DIAGNOSTIC_BUILD=PASS
CAPTURE_DISABLED_EXECUTION=PASS
CAPTURE_DISABLED_RUNTIME_PATH_INERT=YES
CAPTURE_EXTRACTION_ORDER_STATIC_PROOF=PASS
CAPTURE_BUFFER_RESIDENCE_PROOF=ESTABLISHED
EARLY_BUFFERLESS_TARGETS_PROVEN_NON_SUBMITTED=YES
SCHEDULER_SYNCHRONIZE_COVERS_CAPTURE_BACKEND=YES
CAPTURE_BACKEND_OWNERSHIP_PROOF=NOT_ESTABLISHED
CAPTURE_GRAPH_ROLE_RELATIONSHIPS_PROVEN=NO
PAYLOAD_ACCOUNTING_EXACT=YES
PAYLOAD_PROVENANCE_COMPLETE=NO
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
Q8_1_REFERENCE_UNIT_TESTS=PASS
ROUNDING_HALF_CASES_OK=YES
TQ2_G128_REFERENCE_DECODE=QUALIFIED
HARNESS_DEFECT_46=RESOLVED
HARNESS_DEFECT_47=RESOLVED
HARNESS_DEFECT_48=RESOLVED
HARNESS_DEFECT_49=RESOLVED
HARNESS_DEFECT_50=RESOLVED
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
