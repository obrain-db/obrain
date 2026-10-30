# ENGINE-007B-F1A-Q0-N5 — INDEXATION INDÉPENDANTE ET COUVERTURE TEMPORELLE

Date : 2026-09-30. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7b-f1a-q0-n5/`

---

## 0 — Résultat

**Deux gates fermés, un gate bloquant qui n'est pas numérique mais de provenance.**

```
INDEPENDENT_ROW_OUTPUT_INDEXING_TEST=PASS      <-- FERME
N5_EPOCH1_REFERENCE_VALIDATION=PASS            <-- FERME

FIRST_BLOCKING_GATE=REFERENCE_SAMPLE_TEMPORAL_PAIRING
F1A_Q0_N5_STATUS=BLOCKED_TEMPORAL_AND_ROLE_COVERAGE_GATE
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
```

## 1 — N5-A : test F32 indépendant d'indexation — PASS

Programme autonome `tools/n5-f32-row-output-test.cpp`, qui **ne réutilise ni N2, ni le lecteur
TQ2_G128, ni aucune fonction de sérialisation maison**.

### Forme réelle du vrai `ggml_mul_mat` (relevée, pas écrite en dur)

```
WEIGHT_NE=256,64,1,1        WEIGHT_NB=4,1024,65536,65536
ACTIVATION_NE=256,1,1,1     ACTIVATION_NB=4,1024,1024,1024
OUTPUT_NE=64,1,1,1          OUTPUT_NB=4,256,256,256
```

`out.ne[0] = M = weight.ne[1]`. C'est la convention, **établie par une expérience GPU**.

### Les sentinelles — erreur nulle partout

```
r=0   attendu=23.0     GPU=23.0     err=0.000e+00
r=1   attendu=379.0    GPU=379.0    err=0.000e+00
r=16  attendu=-353.0   GPU=-353.0   err=0.000e+00
r=32  attendu=-591.0   GPU=-591.0   err=0.000e+00
r=48  attendu=-461.0   GPU=-461.0   err=0.000e+00
r=62  attendu=-353.0   GPU=-353.0   err=0.000e+00
r=63  attendu=440.0    GPU=440.0    err=0.000e+00

TEST_ALL_ROWS=YES_PASS   (les 64 lignes, erreur nulle sur chacune)
```

Le motif de poids est **non symétrique et sans répétition** (`w=(k*7+r*13)%23-11`) : une
**permutation de lignes n'aurait pas passé**. Les valeurs sont des entiers, donc le test mesure
l'**indexation**, pas l'arrondi.

### Les deux mappings, vérifiés séparément

```
LOGICAL_ROW_TO_OUTPUT_MAPPING             = PASS
   la ligne logique r du poids correspond à out->data[r]
OUTPUT_LOGICAL_TO_SERIALIZED_BYTE_MAPPING = PASS
   out->nb[0] = 4 = sizeof(float) : l'élément logique i est à l'octet 4*i
```

Le second n'a **pas** été déduit du premier.

## 2 — N5-B : époque 1 — PASS

Référence **exactement** celle de N4, aucune règle modifiée :

```
d_fp32 = amax / 127.0f
q = roundf(x / d_fp32)          <- roundf, pas np.rint
stored_d = fp16(d_fp32)          <- pas d'inverse fp16
```

| tenseur | N | MAX_ABS | MEAN_ABS | REL_L2 | COSINE | BUDGET | verdict |
|---|---|---|---|---|---|---|---|
| `blk.20.ffn_up` | 17408 | 5.960e-07 | 5.940e-08 | 1.470e-07 | 1.0000000000 | 2.244e-02 | **PASS** |
| `blk.16.attn_gate` | 6144 | 7.153e-07 | 7.700e-08 | 1.371e-07 | 1.0000001192 | 2.660e-02 | **PASS** |
| `blk.15.attn_output` | 5120 | 4.768e-07 | 3.846e-08 | 1.609e-07 | 1.0000000000 | 2.403e-02 | **PASS** |

```
OUTPUT_VALUES_VALIDATED=28672
N5_EPOCH1_REFERENCE_VALIDATION=PASS
```

Budget appliqué **individuellement** par tenseur. L'époque 1 est qualifiée numériquement **au même
niveau que l'époque 0** (~1e-7). C'est une mesure **indépendante** : la référence a été réécrite
dans cette passe, et elle reproduit N4.

## 3 — N5-C : NON EXÉCUTÉ

**Je n'ai pas exécuté l'établissement de la chronologie.** Les quatre nombres restent non reliés :

```
LOGIT_DECODE_STEPS=9
OBSERVED_ACCEPTED_COMPUTE_SUBMISSIONS=4
CAPTURE_EPOCHS=2
CAPTURE_TARGETS_PER_EPOCH=168

SUBMISSION_DECODE_STEP_MAPPING=NON_MESURE
CAPTURE_EPOCH_SUBMISSION_MAPPING=NON_MESURE
REQUIRED_Q0_POPULATION_DEFINED=NO
Q0_TEMPORAL_POPULATION_SPECIFICATION=AMBIGUOUS
```

Je **ne les assimile pas**. Le protocole Q0 préenregistré n'a pas été retrouvé et lu dans cette
passe, donc je ne peux pas dire quelle population il exige.

## 4 — N5-D : NON EXÉCUTÉ

```
ROLE_COVERAGE_COUNT=3       (ffn_up, attn_gate, attn_output)
ROLES_MISSING=ffn_gate, attn_q, attn_k
PREDECLARED_SELECTION_HASH_VERIFIED=NO
N5_ROLE_STRATIFIED_REFERENCE_VALIDATION=NOT_EXECUTED
N5_FULL_REQUIRED_POPULATION_VALIDATION=NOT_EXECUTED
FULL_168_TARGET_NATIVE_REFERENCE_VALIDATION=NOT_EXECUTED
```

Les 3 tenseurs ne couvrent **pas** les 6 rôles, et **pas** les 168 cibles. Je ne convertis pas
`3_TENSORS_PASS` en `ALL_168_PASS` — c'est exactement ce que votre §16 interdit.

## 5 — §19 : l'écart N3nonies / N4

```
N3NONIES_VS_N4_OLD_METRIC_DIFFERENCE=NOT_FULLY_RECONSTRUCTED
```

`N3nonies` donnait `rel=0.535` sur `blk.20.ffn_up` ligne 0 ; `N4` donne `2.06e-7`. Entre les deux
il y a le **correctif de préservation** (défaut 56) : les captures d'époque 0 de N3nonies étaient
prises **avant** `ggml_set_output`, celles de N4 **après**.

C'est l'explication la plus directe, **mais je ne l'ai pas confirmée par les SHA256 des payloads
utilisés dans chaque passe**. N4 reste valide car N5-B le **reproduit** avec une référence réécrite
indépendamment.

## 6 — Rapport final (§23)

```
ENGINE007B_F1A_STATUS=BLOCKED_OPERATOR_REFERENCE_GATE
F1A_Q0_N5_STATUS=BLOCKED_TEMPORAL_AND_ROLE_COVERAGE_GATE

N4_CURRENT_REFERENCE_PROVENANCE=REWRITTEN_INDEPENDENTLY_IN_N5_AND_REPRODUCED
N4_NUMERIC_RESULTS_REPRODUCIBLE=YES_EPOCH0_AND_EPOCH1_BOTH_AT_1e-7

N3NONIES_VS_N4_OLD_METRIC_DIFFERENCE=NOT_FULLY_RECONSTRUCTED

INDEPENDENT_F32_MUL_MAT_TEST=PASS
INDEPENDENT_F32_GPU_EXECUTION=PASS
F32_ROW_OUTPUT_CORRESPONDENCE=PASS
F32_OUTPUT_SERIALIZATION=PASS
REAL_TQ2_CAPTURE_INDEXING=PARTIAL (formes confirmees par reconstruction, offsets non testes separement)
INDEPENDENT_ROW_OUTPUT_INDEXING_TEST=PASS

N5_EPOCH1_FFN_UP_VALIDATION=PASS
N5_EPOCH1_ATTN_GATE_VALIDATION=PASS
N5_EPOCH1_ATTN_OUTPUT_VALIDATION=PASS
N5_EPOCH1_REFERENCE_VALIDATION=PASS

LOGIT_DECODE_STEPS=9
OBSERVED_COMPUTE_SUBMISSIONS=4
CAPTURE_EPOCHS=2
SUBMISSION_DECODE_STEP_MAPPING=NON_MESURE
CAPTURE_EPOCH_SUBMISSION_MAPPING=NON_MESURE
REQUIRED_Q0_POPULATION_DEFINED=NO
ALL_REQUIRED_SUBMISSIONS_CAPTURED=NON_ETABLI
ALL_REQUIRED_TARGETS_CAPTURED=NON_ETABLI
REFERENCE_SAMPLE_TEMPORAL_PAIRING=PARTIAL

ROLE_COVERAGE_COUNT=3
PREDECLARED_SELECTION_HASH_VERIFIED=NO
N5_ROLE_STRATIFIED_REFERENCE_VALIDATION=NOT_EXECUTED
N5_FULL_REQUIRED_POPULATION_VALIDATION=NOT_EXECUTED
FULL_168_TARGET_NATIVE_REFERENCE_VALIDATION=NOT_EXECUTED

CAPTURE_INFRASTRUCTURE_QUALIFIED=YES
CAPTURE_CONTENT_VALIDITY_QUALIFIED=YES
CAPTURE_GRAFT_NUMERICALLY_TRANSPARENT=YES
NATIVE_Q8_INTERMEDIATE_COMPARISON=SOURCE_QUALIFIED_NOT_DIRECTLY_OBSERVED

Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO

PRODUCT_MMVQ_SOURCE_MODIFIED=NO
MODEL_GGUF_MODIFIED=NO
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
ENGINE007_NEXT_STEP=CLOSE_TEMPORAL_AND_ROLE_COVERAGE_GATES
```

## 7 — Test minimal pour fermer le gate bloquant

C'est un gate de **provenance**, pas de numérique.

```
1. RETROUVER et LIRE le protocole Q0 preenregistre ; en extraire
   Q0_REQUIRED_DECODE_STEPS / Q0_REQUIRED_SUBMISSIONS /
   Q0_REQUIRED_CAPTURE_EPOCHS / Q0_REQUIRED_TARGETS.
2. LIRE LES EVENEMENTS de soumission du harnais (pas l ordre des fichiers ecrits)
   et etablir 9 pas <-> 4 soumissions <-> 2 epoques.
3. Si les 2 epoques captees couvrent la population requise :
      REFERENCE_SAMPLE_TEMPORAL_PAIRING=COMPLETE
   Sinon : nommer la capture complementaire exacte a executer.
```

Puis, pour les rôles, produire `N5_ROLE_STRATIFIED_SELECTION.csv` **avant** de calculer la moindre
erreur, et valider `ffn_gate`, `attn_q`, `attn_k` — les trois rôles manquants.

## 8 — Livrables réellement créés

```
N5_STATUS.txt                       gates fermes, gates ouverts, decision, test minimal
N5_F32_SYNTHETIC_TEST_SOURCE.cpp    le programme autonome
N5_F32_SYNTHETIC_TEST_BUILD.txt     build, 2 erreurs corrigees
N5_F32_SYNTHETIC_TEST_RESULTS.csv   serialisation reelle + 64 lignes + verdicts
N5_REAL_CAPTURE_INDEXING_PROOF.txt  preuve d indexation, mappings separes
N5_EPOCH1_FFN_UP_VALIDATION.csv     17408 lignes
N5_EPOCH1_ATTN_GATE_VALIDATION.csv  6144 lignes
N5_EPOCH1_ATTN_OUTPUT_VALIDATION.csv 5120 lignes
N5_EPOCH1_VECTOR_SUMMARY.csv        metriques par tenseur
N5_EPOCH1_VECTOR_SUMMARY.txt        lecture du resume
N5_EPOCH1_RUN.txt                   journal d execution
N5_SOURCE_PROVENANCE.txt            provenance de la reference et du test
```

**Non créés** (étapes non exécutées) : `N5_N4_PROVENANCE_RECONCILIATION.txt`,
`N5_SUBMISSION_EVENT_LEDGER.csv`, `N5_DECODE_STEP_SUBMISSION_MAPPING.csv`,
`N5_CAPTURE_EPOCH_SUBMISSION_MAPPING.csv`, `N5_REQUIRED_Q0_POPULATION.txt`,
`N5_TEMPORAL_COVERAGE.csv`, `N5_ROLE_STRATIFIED_SELECTION.csv`,
`N5_ROLE_STRATIFIED_SELECTION.sha256`, `N5_ROLE_STRATIFIED_VALIDATION.csv`,
`N5_FULL_168_TARGET_VALIDATION.csv`, `N5_FIRST_BLOCKING_GATE.txt`,
`N5_BUILD_PROVENANCE.txt`, `N5_CONFIG_PROVENANCE.txt`.

## 9 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007B_F1A_STATUS=BLOCKED_OPERATOR_REFERENCE_GATE
F1A_Q0_N5_STATUS=BLOCKED_TEMPORAL_AND_ROLE_COVERAGE_GATE
INDEPENDENT_ROW_OUTPUT_INDEXING_TEST=PASS
N5_EPOCH1_REFERENCE_VALIDATION=PASS
REFERENCE_SAMPLE_TEMPORAL_PAIRING=PARTIAL
ROLE_COVERAGE_COUNT=3
N5_FULL_REQUIRED_POPULATION_VALIDATION=NOT_EXECUTED
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
HARNESS_DEFECT_56=RESOLVED_VERIFIED_BY_TARGETED_PRESERVATION_AND_BITWISE_TRANSPARENCY
HARNESS_DEFECT_57=RESOLVED
PRODUCT_MMVQ_SOURCE_MODIFIED=NO
MODEL_GGUF_MODIFIED=NO
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
