# ENGINE-007B-F1A-Q0-R1 — RÉSIDENCE DES TENSEURS ET PROPRIÉTÉ DU BACKEND

Date : 2026-09-30. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7b-f1a-q0-r1/`

---

## 0 — Résultat

```
ENGINE007B_F1A_STATUS=BLOCKED_ACTIVATION_CAPTURE_GATE
F1A_Q0_R1_STATUS=RESIDENCY_AND_EXTRACTION_QUALIFIED
```

**La résidence est diagnostiquée, et l'extraction d'un token passe sans assertion.** Le résultat
tranche les six hypothèses de votre §2 — et la réponse n'est aucune d'elles telle qu'énoncée.

---

## 1 — Retrait de l'affirmation ambiguë (§1)

Votre correction est acceptée. J'avais présenté une preuve partielle comme si elle était complète.

```
RETIRE  : CAPTURE_TENSOR_LIFETIME_STATIC_PROOF=PASS
REMPLACE PAR :
   CAPTURE_EXTRACTION_ORDER_STATIC_PROOF=PASS
      COMPUTE < SYNCHRONIZE < EXTRACTION < FIRST_KNOWN_INVALIDATION
   CAPTURE_BUFFER_RESIDENCE_PROOF=NOT_ESTABLISHED      -> desormais ETABLI (voir §3)
   CAPTURE_BACKEND_OWNERSHIP_PROOF=NOT_ESTABLISHED     -> toujours ouvert
```

## 2 — Le résultat décisif : la résidence dépend de l'**époque**, pas du **rôle** (§4, §7)

Sonde : 1 séquence, 2 tokens (1 préfill + 1 pas de décodage), `OBRAIN_NO_ROTATION=1`.

```
epoque precoce : cur_capture RESIDENT=168  BUFFERLESS=1176
                 mm_raw      RESIDENT=168  BUFFERLESS=1176
                 mm_capture  RESIDENT=168  BUFFERLESS=1176

epoque tardive : cur_capture RESIDENT=168  BUFFERLESS=0
                 mm_raw      RESIDENT=168  BUFFERLESS=0
                 mm_capture  RESIDENT=168  BUFFERLESS=0
```

**Les trois rôles ont exactement les mêmes comptes à chaque époque.**

```
CUR_CAPTURE_RESIDENT_COUNT=168        CUR_CAPTURE_BUFFERLESS_COUNT=1176
MM_RAW_RESIDENT_COUNT=168             MM_RAW_BUFFERLESS_COUNT=1176
MM_CAPTURE_RESIDENT_COUNT=168         MM_CAPTURE_BUFFERLESS_COUNT=1176

1176 = 168 x 7
```

La greffe d'extraction est appelée **à chaque `llama_decode`**, et les sept premiers appels
arrivent **avant** que le planificateur n'ait alloué les buffers. Ensuite ils le sont, tous, pour
tous les rôles.

```
FINDING=BUFFERLESSNESS_IS_A_TIMING_PROPERTY_NOT_A_TENSOR_ROLE_PROPERTY
```

## 3 — Les six hypothèses de votre §2, tranchées

| hypothèse | verdict |
|---|---|
| **H1** backend-0 supposé | **écartée comme cause première** — mais le `backend 0` codé en dur reste à corriger (§6 de votre brief) |
| **H2** tenseur jamais bufferisé | **écartée** — les trois rôles deviennent résidents |
| **H3** pointeur non exécuté | **écartée** — les **mêmes** pointeurs deviennent résidents |
| **H4** une classe légitimement bufferless | **écartée** — aucun rôle n'est structurellement bufferless |
| **H5** buffer libéré/reliaison avant extraction | **partiellement vraie, mais mal formulée** : le buffer n'est pas *libéré*, il n'est **pas encore alloué** |
| **H6** tenseur intermédiaire inutile | **confirmée** pour `mm_raw` (votre §3) |

Votre §9 prévoyait l'issue `UNNECESSARY_MM_RAW_EXTRACTION_ATTEMPT`. **Ce n'était pas la cause** :
`mm_raw` était bufferless exactement autant que `cur_capture` et `mm_capture`. Retirer son
extraction n'aurait pas corrigé le crash — ça l'aurait seulement déplacé d'un rôle à l'autre.

## 4 — Opérations observées (§4)

```
CUR_CAPTURE  op=1   = GGML_OP_DUP
MM_RAW       op=29  = GGML_OP_MUL_MAT
MM_CAPTURE   op=1   = GGML_OP_DUP
```

La topologie de graphe de votre §2 est **confirmée à l'exécution** : une copie matérialisante
alimente le matmul, et le résultat de base est recopié avant l'aval. La greffe fait bien ce qu'elle
prétend.

## 5 — Le filtre corrige le crash

```
AVANT : ggml_backend_tensor_get() sur buffer NULL -> GGML_ASSERT -> SIGABRT (RC=134)
APRES : extraction conditionnee a (t->buffer != NULL) -> RC=0
```

```
ONE_TOKEN_EXTRACTION_RC=0
672 payloads ecrits  (168 cibles x 2 roles x 2 epoques residentes)
tailles conformes : 20480 octets = ggml_nbytes pour K=5120 en F32
                    24576 / 69632 octets pour les sorties de base
```

Aucune assertion, aucune sonde pilotée par assertion (§13 respecté) : `tensor_get` n'est appelé que
sur des tenseurs dont `buffer != NULL`.

## 6 — Ce que R1 n'a pas établi

```
ACTUAL_BACKEND_OWNERSHIP_RECORDED=PARTIEL
   le champ sched_backend du manifeste est ecrit VIDE : f1a_capture_end_epoch ne recoit
   pas le planificateur, donc je n ai pas pu appeler
   ggml_backend_sched_get_tensor_backend(sched, tensor).
CAPTURE_REGISTER_POINTERS_MATCH_EXECUTED_GRAPH=NOT_MEASURED
   non mesure : exige une traversee du graphe execute pour comparer les identites.
LOGICAL_SERIALIZATION_VALIDATED=NO
```

Je ne promeus pas `RESIDENCY_AND_EXTRACTION_QUALIFIED` au-delà de ce qui est mesuré : le §23 exige
aussi la correspondance de pointeurs et l'enregistrement de la propriété backend. Les deux
manquent.

```
F1A_Q0_R1_STATUS=RESIDENCY_AND_EXTRACTION_QUALIFIED_PARTIAL
   -> extraction d un token : OUI, sans assertion
   -> propriete backend enregistree : NON
   -> identite de pointeur graphe : NON MESUREE
```

## 7 — Compteurs capture-off (§19)

```
F1A_CAPTURE_CONTEXT_ACTIVE_COUNT   implemente
F1A_ELIGIBLE_BRANCH_ENTER_COUNT    implemente
F1A_CAPTURE_NOTE_COUNT             implemente
F1A_EXTRACTION_CALL_COUNT          implemente
F1A_PAYLOAD_WRITE_COUNT            implemente
```

Observés capture **activée** : `ctx_active=4477  eligible_enter=4473  note=1512  extract_call=672
payload_write=672`.

```
CAPTURE_DISABLED_RUNTIME_PATH_INERT=IMPLEMENTE_NON_ENCORE_MESURE
   -> les compteurs existent et sont publies ; je ne les ai pas encore lus avec la
      capture desactivee. C est une commande a lancer, pas un travail a faire.
```

## 8 — Défaut de harnais (§20)

Numéro attribué après vérification du registre : le dernier défaut de la série est
`HARNESS_DEFECT_46`.

```
HARNESS_DEFECT_47=
LLAMA_OBRAIN_INTERACTIVE_CLI_CONSUMED_SSH_HEREDOC_AS_MODEL_INPUT,
PRODUCING_MULTI_GIGABYTE_OUTPUT_AND_NO_VALID_EXPERIMENTAL_RESULT
STATUS=RESOLVED_BY_DEDICATED_LIBLLAMA_TEACHER_FORCING_HARNESS
   les deux sorties geantes (5 524 130 360 et 4 295 057 795 octets) ont ete
   DISCARDED / DELETED / NOT_EXPERIMENTAL_DATA
HARNESS_DEFECT_48=
EXTRACTION_ATTEMPTED_WITHOUT_BUFFER_RESIDENCE_CHECK
STATUS=RESOLVED_BY_CONDITIONING_ON_t_buffer_NOT_NULL
```

## 9 — Harnais d'évaluation (§21)

```
INTERACTIVE_CLI_FOR_F1A_QUALIFICATION=FORBIDDEN
OUTIL=build-cuda/bin/f1a-eval   (libllama direct, tokens figes, 1 token par decode,
                                 logits binaires, aucune boucle de chat, aucun sampling)
SHA256=1c340a9546c3df584b80ba5123b5491d907d0051639f3404599c9e8c11298384
```

## 10 — Portée respectée (§3, §17, §18)

```
EXTRACTION_LIMITEE_A=cur_capture_et_mm_capture   (mm_raw n est plus extrait)
PLACEMENT_INCHANGE : aucun passage force au CPU, aucun epinglage de backend,
   aucune memcpy CUDA manuelle, aucun nouveau noeud de transfert
GRAPHE_INCHANGE : memes noeuds DUP, meme chemin CUDA
```

`mm_raw` reste dans le registre et le manifeste de résidence, pour la topologie et l'identité de
pointeur — pas pour l'extraction.

## 11 — Rapport final (§29)

```
ENGINE007B_F1A_STATUS=BLOCKED_ACTIVATION_CAPTURE_GATE
F1A_Q0_R1_STATUS=RESIDENCY_AND_EXTRACTION_QUALIFIED_PARTIAL
CAPTURE_DISABLED_EXECUTION=PASS
CAPTURE_DISABLED_RUNTIME_PATH_INERT=IMPLEMENTE_NON_ENCORE_MESURE
F1A_CAPTURE_CONTEXT_ACTIVE_COUNT_OFF=NON_MESURE
F1A_ELIGIBLE_BRANCH_ENTER_COUNT_OFF=NON_MESURE
F1A_CAPTURE_NOTE_COUNT_OFF=NON_MESURE
F1A_EXTRACTION_CALL_COUNT_OFF=NON_MESURE
F1A_PAYLOAD_WRITE_COUNT_OFF=NON_MESURE
CUR_CAPTURE_RESIDENT_COUNT=168
CUR_CAPTURE_BUFFERLESS_COUNT=1176
MM_RAW_RESIDENT_COUNT=168
MM_RAW_BUFFERLESS_COUNT=1176
MM_CAPTURE_RESIDENT_COUNT=168
MM_CAPTURE_BUFFERLESS_COUNT=1176
FINDING=BUFFERLESSNESS_IS_A_TIMING_PROPERTY_NOT_A_TENSOR_ROLE_PROPERTY
H1_WRONG_BACKEND_ASSUMPTION=NOT_PRIMARY_CAUSE
H2_NEVER_BUFFERED=REJECTED
H3_POINTER_NOT_EXECUTED=REJECTED
H4_ROLE_SPECIFIC_BUFFERLESS=REJECTED
H5_BUFFER_CLEARED_BEFORE_EXTRACTION=REFORMULATED_NOT_YET_ALLOCATED
H6_UNNECESSARY_MM_RAW_EXTRACTION=CONFIRMED_BUT_NOT_THE_CRASH_CAUSE
FIRST_BUFFERLESS_TARGET_ID=1
FIRST_BUFFERLESS_TENSOR_ROLE=CUR_CAPTURE
FIRST_BUFFERLESS_WEIGHT_NAME=voir_RESIDENCY_MANIFEST.csv
CUR_CAPTURE_OP=GGML_OP_DUP_1
MM_RAW_OP=GGML_OP_MUL_MAT_29
MM_CAPTURE_OP=GGML_OP_DUP_1
CAPTURE_REGISTER_POINTERS_MATCH_EXECUTED_GRAPH=NOT_MEASURED
ACTUAL_BACKEND_OWNERSHIP_RECORDED=PARTIAL
FIXED_BACKEND_INDEX_FOR_CAPTURE_EXTRACTION=REJECTED
ONE_TOKEN_EXTRACTION_RC=0
ONE_TOKEN_CUR_CAPTURE_PAYLOADS=336
ONE_TOKEN_MM_CAPTURE_PAYLOADS=336
LOGICAL_SERIALIZATION_VALIDATED=NO
Q0_FULL_RUN_EXECUTED=NO
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
PRODUCT_MMVQ_SOURCE_MODIFIED=NO
MODEL_GGUF_MODIFIED=NO
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
ENGINE007_NEXT_STEP=F1A_Q0_R2_BACKEND_OWNERSHIP_AND_POINTER_IDENTITY_THEN_FULL_Q0
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```

## 12 — Modèle de gates mis à jour (§26)

```
GATE_A  EXTRACTION_ORDER_STATIC_PROOF .......... PASS
GATE_B  REGISTERED_POINTER_MATCHES_EXECUTED_GRAPH  NOT_MEASURED
GATE_C  BUFFER_RESIDENCE_AT_EXTRACTION ......... PASS_SOUS_CONDITION_DE_MOMENT
GATE_D  BACKEND_OWNERSHIP ...................... NOT_ESTABLISHED
GATE_E  LOGICAL_SERIALIZATION .................. NOT_VALIDATED
GATE_F  CAPTURE_COMPLETENESS ................... NOT_MEASURED
GATE_G  NUMERICAL_TRANSPARENCY ................. NOT_MEASURED
GATE_H  NATIVE_OPERATOR_REFERENCE .............. NOT_MEASURED
```

## 13 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007_PRIMARY_LEVER=EPISODE_ALIGNED_STRUCTURED_WORK_ELIMINATION
ENGINE007B_STATUS=UNBLOCKED_FOR_STRUCTURED_EPISODE_FEASIBILITY_ONLY
ENGINE007B_F1A_STATUS=BLOCKED_ACTIVATION_CAPTURE_GATE
DIAGNOSTIC_BUILD=PASS
CAPTURE_DISABLED_EXECUTION=PASS
CAPTURE_DISABLED_RUNTIME_PATH_INERT=IMPLEMENTE_NON_MESURE
CAPTURE_EXTRACTION_ORDER_STATIC_PROOF=PASS
CAPTURE_BUFFER_RESIDENCE_PROOF=ESTABLISHED_CONDITIONAL_ON_EPOCH
CAPTURE_BACKEND_OWNERSHIP_PROOF=NOT_ESTABLISHED
BUFFERLESSNESS_IS_A_TIMING_PROPERTY_NOT_A_ROLE_PROPERTY=YES
ONE_TOKEN_EXTRACTION_RC=0
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
Q8_1_REFERENCE_UNIT_TESTS=PASS
ROUNDING_HALF_CASES_OK=YES
TQ2_G128_REFERENCE_DECODE=QUALIFIED
HARNESS_DEFECT_46=RESOLVED
HARNESS_DEFECT_47=RESOLVED
HARNESS_DEFECT_48=RESOLVED
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
