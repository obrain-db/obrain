# ENGINE-007B-F1A-Q0-R3 — PROVENANCE DE GRAPHE ET QUALIFICATION DE L'EXTRACTION

Date : 2026-09-30. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7b-f1a-q0-r3/`

---

## 0 — Résultat

**R3 a produit la preuve qui manquait à R2. Mais trois gates du §12 restent ouverts, et je ne les
franchis pas.**

```
F1A_Q0_R3_STATUS=EARLY_BUFFERLESS_TARGETS_PROVEN_NON_SUBMITTED
Q0_FULL_RUN_ALLOWED=NO
```

| gate du §12 | état |
|---|---|
| `EARLY_BUFFERLESS_TARGETS_PROVEN_NON_SUBMITTED` | **YES** — c'est la preuve centrale |
| `ALL_BUILD_INSTANCES_CLASSIFIED` | YES |
| `NO_REQUIRED_SUBMITTED_TENSOR_SKIPPED` | YES |
| `SCHEDULER_SYNCHRONIZE_COVERS_CAPTURE_BACKEND` | **YES** — prouvé par la source |
| `PAYLOAD_ACCOUNTING_EXACT` | YES |
| `ALL_SUBMITTED_CUR/MM_CAPTURE_RESIDENT` | YES |
| `CAPTURE_REGISTER_POINTERS_MATCH_SUBMITTED_GRAPH` | **partiel** — 0 contre-exemple, mais la correspondance par rôle n'est pas écrite |
| `ACTUAL_BACKEND_OWNERSHIP_RECORDED` | **NO** |
| `CAPTURE_DISABLED_RUNTIME_PATH_INERT` | **NO** — publication des compteurs absente |
| `PAYLOAD_PROVENANCE_COMPLETE` | **NO** — SHA256 par payload non écrit |

## 1 — L'instantané figé, et ce qu'il montre (§2)

Le hook R2 ne gardait qu'un compteur et un dernier pointeur. R3 le remplace par un **instantané figé
des nœuds**, pris au moment de l'appel, avec son résultat :

```
[f1a-sub] submission=1 gf=0xe38d4ac4d030 nodes=3991 rc=0 fp=3f93adcb1129607b
[f1a-sub] submission=2 gf=0xe38d4ac4d030 nodes=3991 rc=0 fp=3f93adcb1129607b
```

Deux points qui comptent :

- **`rc=0`** : ce sont des soumissions **acceptées**, pas seulement tentées (votre §2).
- **Même pointeur, même nombre de nœuds, même empreinte FNV-1a sur (pointeur, op, sources)** : c'est
  **le même graphe réutilisé** pour les deux pas de décodage. Je ne conclus pas « deux tokens »
  sans le mesurer — c'est ce que la mesure dit.

## 2 — La preuve centrale : les 1176 cibles bufferless sont absentes des graphes soumis (§4)

```
[f1a-prov] RESIDENT_ET_SOUMIS=504   BUFFERLESS_ET_SOUMIS=0
[f1a-prov] RESIDENT_ET_SOUMIS=504   BUFFERLESS_ET_SOUMIS=0
```

```
BUFFERLESS_TARGETS=1176
BUFFERLESS_TARGETS_IN_SUBMITTED_GRAPHS=0
RESIDENT_TARGETS_IN_SUBMITTED_GRAPHS=504

EARLY_BUFFERLESS_TARGETS_PROVEN_NON_SUBMITTED=YES
```

**Aucun contre-exemple.** Votre §4 prévoyait l'arrêt si une cible bufferless appartenait à un graphe
soumis : ce cas n'existe pas. La bufferlessness est donc bien une propriété des passes de
construction, établie par **appartenance au graphe soumis** et non plus par un compteur de
soumissions.

C'est la définition opérationnelle que votre §2 demandait de substituer à `f1a_last_gf`.

## 3 — Sémantique de la synchronisation (§7) : prouvée par la source

```cpp
// ggml/src/ggml-backend.cpp:1904
void ggml_backend_sched_synchronize(ggml_backend_sched_t sched) {
    GGML_ASSERT(sched);
    for (int i = 0; i < sched->n_backends; i++) {
        ggml_backend_synchronize(sched->backends[i]);
    }
    ...
}
```

La fonction **boucle sur tous les backends participants**. Aucun n'est oublié.

```
SCHEDULER_SYNCHRONIZE_COVERS_CAPTURE_BACKEND=YES
PREUVE=source_ggml-backend.cpp:1906_boucle_sur_sched_n_backends
```

Et un `tensor_get` réussi n'est **pas** utilisé comme preuve (§7) : c'est la lecture de source qui
l'établit.

## 4 — Contrôle négatif capture off (§10) : la mesure n'est pas faite

```
capture desactivee : RC=0, fichiers de capture = 0
lignes [f1a-ctr] dans la sortie : 0
```

**Mais l'absence de fichiers ne prouve pas l'inertie** — votre §10 l'interdit explicitement. Et les
cinq compteurs **ne sont pas publiés** :

```
F1A_CAPTURE_CONTEXT_ACTIVE_COUNT_OFF=NON_MESURE   (idem pour les quatre autres)
```

**Cause** : les compteurs sont incrémentés, mais leur **publication** est déclenchée par
`f1a_capture_end_epoch()`, qui n'est jamais appelée quand la capture est désactivée. Sans capture,
rien ne les imprime.

```
CAPTURE_DISABLED_RUNTIME_PATH_INERT=NON_MESURE
```

Le correctif est petit (publier depuis le destructeur ou la fin du harnais), mais je ne le déclare
pas fait sans l'avoir lu.

## 5 — Ce que R3 n'a pas établi

```
CAPTURE_REGISTER_POINTERS_MATCH_SUBMITTED_GRAPH=PARTIEL
   0 contre-exemple, et 504 tenseurs residents tous presents dans le graphe soumis.
   MAIS la correspondance par ROLE n est pas ecrite : je n ai pas verifie
   cur_capture == src1 du MUL_MAT, mm_raw == sortie du MUL_MAT,
   mm_capture == DUP(mm_raw) et consomme par l aval (paragraphe 3).
   L appartenance est prouvee ; la STRUCTURE ne l est pas.

ACTUAL_BACKEND_OWNERSHIP_RECORDED=NO
   end_epoch ne recoit toujours pas le sched : ggml_backend_sched_get_tensor_backend
   n est pas appelee. Le champ sched_backend du manifeste est ecrit vide.

PAYLOAD_PROVENANCE_COMPLETE=NO
   les payloads portent ep<epoch>_t<target>_<classe>.bin mais pas de SHA256 enregistre,
   et la colonne COMPUTE_SUBMISSION_ID n est pas rattachee a chaque payload (paragraphe 9).
```

## 6 — Payloads

```
168 cibles x 2 classes x 2 epoques = 672
CUR_CAPTURE=336   MM_CAPTURE=336   OTHER=0
PAYLOAD_ACCOUNTING_EXACT=YES
UNEXPLAINED_PAYLOAD_COUNT=0
```

Aucun payload n'est attribué à une construction non soumise — c'est garanti par la mesure du §2 :
les époques non soumises ne produisent aucun payload (leurs tenseurs sont sautés faute de buffer).

## 7 — Aucune référence numérique (§11)

```
q8_1 non executee   TQ2 non comparee   logits OFF/ON non compares
aucun classement d episode
SALIENCY_MEASURED=NO
```

## 8 — Rapport final

```
ENGINE007B_F1A_STATUS=BLOCKED_ACTIVATION_CAPTURE_GATE
F1A_Q0_R1_STATUS=RESIDENCY_AND_EXTRACTION_QUALIFIED_PARTIAL
F1A_Q0_R2_STATUS=EARLY_BUFFERLESS_EPOCHS_PROVEN_NON_EXECUTED
F1A_Q0_R3_STATUS=EARLY_BUFFERLESS_TARGETS_PROVEN_NON_SUBMITTED
Q0_FULL_RUN_ALLOWED=NO
TOTAL_COMPUTE_SUBMISSIONS=2
SUBMISSION_RESULTS=rc=0_pour_les_deux
SUBMITTED_GRAPH_NODES=3991
SUBMITTED_GRAPH_FINGERPRINT=3f93adcb1129607b_pour_les_deux
BUFFERLESS_TARGETS=1176
BUFFERLESS_TARGETS_IN_SUBMITTED_GRAPHS=0
RESIDENT_TARGETS_IN_SUBMITTED_GRAPHS=504
EARLY_BUFFERLESS_TARGETS_PROVEN_NON_SUBMITTED=YES
ALL_BUILD_INSTANCES_CLASSIFIED=YES
NO_REQUIRED_SUBMITTED_TENSOR_SKIPPED=YES
ALL_SUBMITTED_CUR_CAPTURE_RESIDENT=YES
ALL_SUBMITTED_MM_CAPTURE_RESIDENT=YES
CAPTURE_REGISTER_POINTERS_MATCH_SUBMITTED_GRAPH=PARTIAL_MEMBERSHIP_PROVEN_ROLE_STRUCTURE_NOT
ACTUAL_BACKEND_OWNERSHIP_RECORDED=NO
SCHEDULER_SYNCHRONIZE_COVERS_CAPTURE_BACKEND=YES
SCHEDULER_SYNC_PROOF=SOURCE_ggml-backend.cpp:1906
PAYLOAD_ACCOUNTING_EXACT=YES
PAYLOAD_PROVENANCE_COMPLETE=NO
CAPTURE_DISABLED_RUNTIME_PATH_INERT=NON_MESURE
F1A_CAPTURE_CONTEXT_ACTIVE_COUNT_OFF=NON_MESURE
F1A_ELIGIBLE_BRANCH_ENTER_COUNT_OFF=NON_MESURE
F1A_CAPTURE_NOTE_COUNT_OFF=NON_MESURE
F1A_EXTRACTION_CALL_COUNT_OFF=NON_MESURE
F1A_PAYLOAD_WRITE_COUNT_OFF=NON_MESURE
PRODUCT_MMVQ_SOURCE_MODIFIED=NO
MODEL_GGUF_MODIFIED=NO
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
ENGINE007_NEXT_STEP=F1A_Q0_R4_ROLE_STRUCTURE_BACKEND_OWNERSHIP_AND_OFF_COUNTERS
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```

## 9 — Livrables

```
SUBMITTED_GRAPH_SNAPSHOTS.csv / .txt   instantanes figes : 2 soumissions, empreintes
REGISTERED_VS_SUBMITTED_POINTERS.txt   preuve centrale : 0 bufferless dans les graphes soumis
SCHEDULER_SYNC_SEMANTICS.txt           preuve par la source
CAPTURE_OFF_RUNTIME_COUNTERS.txt       controle negatif : mesure absente, dit tel quel
EPOCH_CLASSIFICATION.csv               classification par epoque
RESIDENCY_MANIFEST.csv                 metadonnees + appartenance par tenseur
on.txt / off.txt                       journaux bruts des deux executions
```

**Non produits** : `BACKEND_OWNERSHIP_MANIFEST.csv`, `PAYLOAD_PROVENANCE.csv`,
`BUILD_INSTANCE_MANIFEST.csv`, `COMPUTE_SUBMISSION_MANIFEST.csv` (détail par nœud),
`R3_PATCH_PROVENANCE.txt`, `CONFIG_PROVENANCE.txt`.

## 10 — R4 : les quatre choses qui restent

```
R4-a  publier les cinq compteurs quand la capture est desactivee
         -> CAPTURE_DISABLED_RUNTIME_PATH_INERT
R4-b  passer le sched a end_epoch, appeler ggml_backend_sched_get_tensor_backend,
      enregistrer SCHEDULER_ASSIGNED_BACKEND / BUFFER_TYPE / BUFFER_OWNER
         -> ACTUAL_BACKEND_OWNERSHIP_RECORDED
R4-c  ecrire la correspondance PAR ROLE : cur_capture == src1, mm_raw == sortie,
      mm_capture == DUP(mm_raw) et consomme par l aval
         -> CAPTURE_REGISTER_POINTERS_MATCH_SUBMITTED_GRAPH
R4-d  SHA256 par payload + rattachement COMPUTE_SUBMISSION_ID
         -> PAYLOAD_PROVENANCE_COMPLETE
```

Alors seulement Q0 complet (OFF/ON, fidélité numérique, complétude, reconstruction de la sortie
CUDA) devient autorisé.

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
EARLY_BUFFERLESS_TARGETS_PROVEN_NON_SUBMITTED=YES
SUBMITTED_GRAPHS_ACCEPTED=YES_rc_0
SCHEDULER_SYNCHRONIZE_COVERS_CAPTURE_BACKEND=YES
CAPTURE_REGISTER_POINTERS_MATCH_SUBMITTED_GRAPH=PARTIAL
CAPTURE_BACKEND_OWNERSHIP_PROOF=NOT_ESTABLISHED
PAYLOAD_ACCOUNTING_EXACT=YES
PAYLOAD_PROVENANCE_COMPLETE=NO
CAPTURE_DISABLED_RUNTIME_PATH_INERT=NON_MESURE
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
Q8_1_REFERENCE_UNIT_TESTS=PASS
ROUNDING_HALF_CASES_OK=YES
TQ2_G128_REFERENCE_DECODE=QUALIFIED
HARNESS_DEFECT_46=RESOLVED
HARNESS_DEFECT_47=RESOLVED
HARNESS_DEFECT_48=RESOLVED
HARNESS_DEFECT_49=RESOLVED
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
