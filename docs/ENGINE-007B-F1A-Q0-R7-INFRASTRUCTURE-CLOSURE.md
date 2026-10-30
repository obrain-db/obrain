# ENGINE-007B-F1A-Q0-R7 — CLÔTURE DES TROIS DERNIERS GATES

Date : 2026-09-30. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7b-f1a-q0-r7/`

---

## 0 — Résultat

**Les trois gates sont fermés.**

```
F1A_Q0_R7_STATUS=SUBMITTED_GRAPH_CAPTURE_INFRASTRUCTURE_QUALIFIED
Q0_FULL_RUN_ALLOWED=YES
```

Le statut général **reste** :

```
ENGINE007B_F1A_STATUS=BLOCKED_ACTIVATION_CAPTURE_GATE
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
```

Une qualification d'infrastructure n'est pas une qualification numérique. Je ne les confonds pas.

## 1 — Chronologie gelée (§1)

```
[1285] f1a-sub submission=1
[5318] f1a-res RESIDENT ... bufferless=1176     <- end_epoch, APRES le decode #1
[5662] f1a-sub submission=2
[6167] f1a-res RESIDENT ... bufferless=0        <- end_epoch, APRES le decode #2
```

```
PAYLOAD_SUBMISSION_ATTRIBUTION=CORRECT_NO_OFFSET
SUBMISSION_ID_OFF_BY_ONE_COUNT=0
```

Aucun compteur n'a été décrémenté, aucun registre de soumissions n'a été ajouté. Le mécanisme
d'attribution est **inchangé** — c'est mon interprétation de R6 qui était fausse, pas le code.

## 2 — R7-a : SHA256 réel (§2)

Le §2 autorise le calcul par le harnais après extraction. Aucune cryptographie n'entre dans le
noyau CUDA. Les 672 payloads R6 ont été repris, copiés, et leur SHA256 recalculé depuis les
fichiers.

```
PAYLOAD_HASH_ALGO=SHA256
PAYLOAD_SHA256_LENGTH=64_HEX_CHARACTERS
TOTAL_PAYLOAD_COUNT=672
MISSING_REQUIRED_PAYLOAD_COUNT=0
PAYLOAD_SIZE_MISMATCH_COUNT=0
PAYLOAD_HASH_MISMATCH_COUNT=0
PAYLOAD_SHA256_VERIFIED=YES
```

**Contrôle négatif** — un validateur qui répond toujours OK ne prouve rien :

```
copie d un payload, UN OCTET modifie (data[0] ^= 0x01)
  DIVERGENCE DETECTEE = OUI
  CONTROLE_NEGATIF=PASS
  payload de qualification intact apres le test : OUI
```

Le payload original de qualification **n'a jamais été touché** : le test opère sur une copie, et son
intégrité est revérifiée après coup.

`fnv64x4` reste utilisé là où un condensé non cryptographique suffit — les empreintes de structure
de graphe (`f1a_sub_snap::fingerprint`). Il n'est plus présenté comme un SHA256.

## 3 — R7-b : contrat d'extraction backend (§4)

Les cinq garanties, chacune étayée par la source ou par la mesure :

| garantie | preuve |
|---|---|
| **B1** buffer valide à l'extraction | 337/337 tenseurs requis ont `buffer_ptr != NULL` ; le code **skippe** tout buffer nul |
| **B2** `tensor_get` applicable au type observé | `ggml_backend_tensor_get` prend le **tenseur**, dérive le backend de `t->buffer`, délègue à `buft->iface.get_tensor` — aucun backend supposé n'est passé |
| **B3** octets de la plage demandée | appel `tensor_get(t, buf, 0, ggml_nbytes(t))` ; `written_bytes == ggml_nbytes(t)` pour les 672, `SIZE_MISMATCH=0` |
| **B4** synchronisation couvrante | `sched_synchronize` boucle sur `sched->n_backends` (prouvé par source en R3) ; CUDA0 est dans ce périmètre |
| **B5** extraction avant invalidation | preuve statique R3 : `COMPUTE < SYNCHRONIZE < EXTRACTION < première invalidation` |

```
BACKEND_EXTRACTION_SEMANTICS_QUALIFIED=YES
INDEPENDENT_BUFFER_OWNER_ID=NOT_EXPOSED
```

Je ne transforme pas `ggml_backend_buffer_name()` en preuve d'une identité de propriétaire physique :
elle renvoie le nom du **type** de buffer. Le gate porte sur la validité du transfert hôte, pas sur un
identifiant que l'API n'expose pas.

## 4 — R7-c : A == B == C (§5)

Comparaison **par identifiants** `(epoch, target_id)`, pas par cardinalité :

```
A_COUNT=168    B_COUNT=168    C_COUNT=168

A_MINUS_B=VIDE      B_MINUS_A=VIDE
A_MINUS_C=VIDE      C_MINUS_A=VIDE
A == B == C : OUI

MISSING_TARGET_COUNT=0
UNEXPECTED_TARGET_COUNT=0
SUBMITTED_TARGET_CAPTURE_COMPLETENESS=PASS
```

- **A** — cibles du graphe soumis : 168, les constructions non soumises exclues (`sub_at_build > 0`).
- **B** — A + relations locales correctes (`DUP → MUL_MAT → DUP`, `src1 == cur_capture`,
  `src0 == mm_raw`) **et** chemin d'effet qualifié : 156 `OUTPUT_DEPENDENT` + 12 `KV_SIDE_EFFECT`.
- **C** — A + captures vérifiées : `activation` **et** `base_output` présents, résidence, extraction,
  taille et SHA256 vérifiés.

## 5 — Comptabilité des payloads (§3)

```
TOTAL_PAYLOAD_COUNT=672
par role         : activation 336, base_output 336
par soumission   : 336 sub=1, 336 sub=2
fichiers disque  : 672
PAYLOAD_ACCOUNTING_EXACT=YES
PAYLOAD_PROVENANCE_COMPLETE=YES
```

Le nombre attendu est **dérivé du manifeste de cette exécution**, pas de la formule
`nombre_de_cibles × 2`. Les 672 fichiers sont conservés comme référence de comptabilité historique.

**Précision honnête sur ce qui a été refait et ce qui ne l'a pas été** : R7 est un post-traitement
hôte. Le binaire diagnostique est **inchangé depuis R6**, et l'exécution du modèle n'a pas été
rejouée. Le SHA256 est donc calculé sur les fichiers existants, pas sur une nouvelle extraction.
Cela satisfait le §2 (« le calcul peut être réalisé par le harnais après extraction »), mais ce n'est
pas une ré-exécution.

## 6 — Rapport final (§10)

```
ENGINE007B_F1A_STATUS=BLOCKED_ACTIVATION_CAPTURE_GATE
F1A_Q0_R7_STATUS=SUBMITTED_GRAPH_CAPTURE_INFRASTRUCTURE_QUALIFIED
DIAGNOSTIC_BUILD=PASS
DIAGNOSTIC_BINARY_SHA256=INCHANGE_DEPUIS_R6
CAPTURE_PATCH_SHA256=INCHANGE_DEPUIS_R6
PAYLOAD_SUBMISSION_ATTRIBUTION=CORRECT_NO_OFFSET
SUBMISSION_ID_OFF_BY_ONE_COUNT=0
PAYLOAD_HASH_ALGO=SHA256
PAYLOAD_SHA256_VERIFIED=YES
TOTAL_PAYLOAD_COUNT=672
PAYLOAD_HASH_MISMATCH_COUNT=0
PAYLOAD_SIZE_MISMATCH_COUNT=0
PAYLOAD_PROVENANCE_COMPLETE=YES
PAYLOAD_ACCOUNTING_EXACT=YES
BACKEND_EXTRACTION_SEMANTICS_QUALIFIED=YES
INDEPENDENT_BUFFER_OWNER_ID=NOT_EXPOSED
A_COUNT=168
B_COUNT=168
C_COUNT=168
A_MINUS_B=VIDE
B_MINUS_A=VIDE
A_MINUS_C=VIDE
C_MINUS_A=VIDE
SUBMITTED_TARGET_CAPTURE_COMPLETENESS=PASS
CAPTURE_DISABLED_RUNTIME_PATH_INERT=YES
NO_REQUIRED_SUBMITTED_TENSOR_SKIPPED=YES
HARNESS_DEFECT_51_STATUS=RETRACTED_AFTER_MEASUREMENT
Q0_FULL_RUN_ALLOWED=YES
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
PRODUCT_MMVQ_SOURCE_MODIFIED=NO
MODEL_GGUF_MODIFIED=NO
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
ENGINE007_NEXT_STEP=Q0_FULL_NUMERICAL_TRANSPARENCY_AND_NATIVE_OPERATOR_REFERENCE
```

## 7 — Livrables réellement créés

```
CHRONOLOGY_PROOF_RETAINED.txt        ordre des evenements + retractation du defaut 51
PAYLOAD_PROVENANCE.csv               672 lignes, hash_algo=SHA256, digest reel
PAYLOAD_HASH_VERIFICATION.txt        verificateur independant, 0 divergence
PAYLOAD_HASH_NEGATIVE_CONTROL.txt    copie alteree detectee, original intact
PAYLOAD_ACCOUNTING.txt               comptes + provenance
BACKEND_EXTRACTION_SOURCE_CONTRACT.txt   B1..B5, chacun etaye
BACKEND_EXTRACTION_VALIDATION.txt
SUBMITTED_TARGET_SET_A.csv           168 identifiants
SUBMITTED_TARGET_SET_B.csv           168 identifiants
SUBMITTED_TARGET_SET_C.csv           168 identifiants
SUBMITTED_TARGET_SET_COMPARISON.txt  comparaison par identifiants
R7_STATUS.txt
capout/*.bin                         672 payloads avec SHA256 verifie
```

**Non créés** : `PAYLOAD_SHA256.txt` (le digest est dans `PAYLOAD_PROVENANCE.csv`, colonne
`payload_hash`), `CAPTURE_OFF_RUNTIME_COUNTERS.txt` (le contrôle OFF n'a **pas** été rejoué : R7 ne
modifie pas le chemin d'instrumentation, donc je m'appuie sur R4/R6 sans contre-exemple),
`R7_BUILD_PROVENANCE.txt` et `R7_PATCH_PROVENANCE.txt` (aucune source modifiée), `CONFIG_PROVENANCE.txt`.

## 8 — Ce que cette clôture ne dit pas

Elle dit que **l'infrastructure de capture est qualifiée**. Elle ne dit **rien** sur la fidélité
numérique de la référence ni sur l'intérêt d'une élimination d'épisodes :

```
Q0_STATUS=NOT_PASSED
   -> transparence numerique OFF/ON non mesuree
   -> reference q8_1/TQ2 contre la sortie CUDA native non mesuree
Q1_ALLOWED=NO
   -> le domaine POST-WHT n est pas qualifie
SALIENCY_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
```

## 9 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007_PRIMARY_LEVER=EPISODE_ALIGNED_STRUCTURED_WORK_ELIMINATION
ENGINE007B_STATUS=UNBLOCKED_FOR_STRUCTURED_EPISODE_FEASIBILITY_ONLY
ENGINE007B_F1A_STATUS=BLOCKED_ACTIVATION_CAPTURE_GATE
F1A_Q0_R7_STATUS=SUBMITTED_GRAPH_CAPTURE_INFRASTRUCTURE_QUALIFIED
Q0_FULL_RUN_ALLOWED=YES
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
BUFFER_TYPE_RECORDED=YES
INDEPENDENT_BUFFER_OWNER_ID=NOT_EXPOSED
BACKEND_EXTRACTION_SEMANTICS_QUALIFIED=YES
SCHEDULER_SYNCHRONIZE_COVERS_CAPTURE_BACKEND=YES
PAYLOAD_HASH_ALGO=SHA256
PAYLOAD_SHA256_VERIFIED=YES
PAYLOAD_HASH_MISMATCH_COUNT=0
PAYLOAD_SIZE_MISMATCH_COUNT=0
CONTROLE_NEGATIF=PASS
PAYLOAD_ACCOUNTING_EXACT=YES
PAYLOAD_PROVENANCE_COMPLETE=YES
PAYLOAD_SUBMISSION_ATTRIBUTION=CORRECT_NO_OFFSET
SUBMITTED_TARGET_CAPTURE_COMPLETENESS=PASS
A_COUNT=168
B_COUNT=168
C_COUNT=168
NO_REQUIRED_SUBMITTED_TENSOR_SKIPPED=YES
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
