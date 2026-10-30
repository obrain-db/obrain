# ENGINE-007B-F1A-Q0FWD-v1-RUN1 — PREMIÈRE EXÉCUTION QUALIFIÉE

Date : 2026-09-30. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../qualification/e7b-f1a-q0-forward-v1/run1/`

---

## 0 — Résultat

**Le run a bien eu lieu. Il s'arrête au gate de provenance temporelle, avant toute reconstruction.**

```
Q0_FORWARD_STATUS=BLOCKED
FIRST_BLOCKING_GATE=EVENT_LEDGER
BLOCKED_EVENT_LEDGER
Q1_FORWARD_ALLOWED=NO
```

**BLOCKED, pas FAIL.** Aucune mesure valide n'a enfreint le protocole. C'est l'**instrumentation
manquante** qui empêche d'établir la mesure scientifique — exactement la distinction que ton §32
demande de tenir.

## 1 — Préflight : les 4 autorités recheckées

```
PROTOCOL_SHA256_RECHECK=PASS             9a467c3a...a81a1b
TRAJECTORY_SHA256_RECHECK=PASS           dc7b55dc...a21096
HISTORICAL_SOURCE_TOKEN_SHA256_RECHECK=PASS  dc8d54a9...f44b
MODEL_SHA256_RECHECK=PASS                1d7c9409...b937  (recalcule sur 12.8 Go)
PREDECLARED_SENTINEL_SELECTION_SHA256_RECHECK=PASS
```

**Preuve que je n'ai pas retokenisé** : les 10 IDs extraits de la trajectoire gelée produisent
`tokens.txt` dont le SHA256 est **exactement** `dc8d54a9…f44b` — le hash historique du fichier
d'époque.

```
SOURCE_COMMIT=8447604
DIAGNOSTIC_BINARY_SHA256=e4b89428...cd7c
appels f1a_mark_output presents = 2
R1_TOKENS_GREEDY_USED_FOR_Q0FWD=NO
```

## 2 — RUN OFF puis ON

```
OFF : 9 pas, 8939536 octets, aucune inertie rompue, aucune ligne [f1a]
ON  : 9 pas, 672 payloads, 8 CSV de provenance
```

```
Q0FWD_CAPTURE_OFF_INERTNESS=PASS
LOGIT_BITWISE_MISMATCH_COUNT=0        <-- fichiers OFF et ON IDENTIQUES au bit pres
OFF_SHA256 = 53aa87fb70d101c7e98525e148f8c97b8441112a6728dacc28312c2dd5df0dd5
ON_SHA256  = 53aa87fb70d101c7e98525e148f8c97b8441112a6728dacc28312c2dd5df0dd5
Q0FWD_CAPTURE_GRAFT_NUMERICALLY_TRANSPARENT=YES
```

Sur l'inertie, je dois être précis : **le binaire ne publie pas de compteurs `F1A_*_COUNT`**. Ton
brief prévoit ce cas (« vérifier les équivalents réels ; ne crée pas de faux compteurs »). J'ai donc
utilisé les équivalents réels : absence de tout `.bin` écrit, absence de toute ligne `[f1a]` dans le
log OFF, logits produits normalement.

## 3 — Le gate bloquant, et ce qui manque exactement

```
A. Un ledger evenementiel produit PENDANT le run (9 EVENT_KINDS).
   0 occurrence de DECODE_BEGIN / GRAPH_READY / COMPUTE_SUBMIT / COMPUTE_COMPLETE /
   CAPTURE_BEGIN / CAPTURE_EXTRACT / CAPTURE_END / LOGITS_READ / DECODE_END.
B. Un DECODE_STEP associe a chaque soumission : AUCUN artefact ne porte la
   colonne decode_step. Sans elle, soumission <-> pas de decodage est INDECIDABLE.
```

Le protocole interdit de reconstruire le ledger depuis l'ordre des fichiers (§5 du brief préc. :
« ne plus reconstruire la chronologie après coup »). Je ne l'ai donc **pas** reconstruit — le CSV
existe avec son header et quatre lignes de constat.

## 4 — Une troisième difficulté que j'ai trouvée en chemin

La classification du runtime est **elle-même ambiguë** :

```
epoch 0 : submissions_total=1  max_submission_at_build=0  BUILD_ONLY_NOT_SUBMITTED
          n_targets=1344  cur_null=1176  raw_null=1176  mc_null=1176
epoch 1 : submissions_total=2  max_submission_at_build=1  SUBMITTED_FOR_COMPUTE
          n_targets=168   cur_null=0     raw_null=0     mc_null=0

SUBMITTED_GRAPH_SNAPSHOTS : submission_id 1 et 2
   gf=0xe1c9e1448030 (IDENTIQUE)   n_nodes=3991   fingerprint=a2ca58dde06fda7b (IDENTIQUE)
```

**`submissions_total` n'est pas le nombre de soumissions du run** : il vaut 2 pour l'époque 1 seule
et 1 pour l'époque 0. Et les deux époques pointent **la même `gf` et le même fingerprint** — donc soit
réutilisation de graphe, soit une seule soumission observée deux fois.

```
OBSERVED_COMPUTE_SUBMISSION_COUNT=AMBIGU
```

Je ne le fixe pas. Et je n'applique pas la règle prospective de sélection des soumissions à un compte
que je ne peux pas établir — ton §13 dit « ne fixe pas le nombre avant observation du ledger ».

## 5 — Ce que je n'ai PAS exécuté, et pourquoi

```
SUBMISSION_CLASSIFICATION.csv          pas de decode_step par soumission
CAPTURE_COMPLETENESS.csv               pas d identifiant de soumission scientifique etabli
TEMPORAL_PAIRING.csv                   REFERENCE_SAMPLE_TEMPORAL_PAIRING reste PARTIAL
SENTINEL_VALIDATION.csv                le protocole exige pairing COMPLETE avant toute reconstruction
FULL_NATIVE_REFERENCE_VALIDATION.csv   idem
```

Le protocole est explicite : « Sinon arrêt avant reconstruction numérique. » **Aucune sentinelle,
aucune exhaustive.** Je ne convertis pas un BLOCKED de provenance en une mesure numérique.

Ce qui existe malgré tout et servira : `EPOCH_CLASSIFICATION.csv`, `SUBMITTED_GRAPH_SNAPSHOTS.csv`,
`PAYLOAD_PROVENANCE.csv` (qui porte déjà `epoch`, `submission_id`, `target_id`, `capture_id`,
`weight_name`, `role` et un hash de payload).

## 6 — Rapport final (§38)

```
ENGINE007B_F1A_STATUS=BLOCKED_EVENT_LEDGER
Q0_FORWARD_PROTOCOL_VERSION=v1
Q0_FORWARD_PROTOCOL_SHA256=9a467c3a3262709c80dafe4d4855d1477627e7f761498d434c8587d7b2a81a1b
Q0FWD_TOKEN_TRAJECTORY_SHA256=dc7b55dc5e87f7fcc50d2b2af15878cad7d6e5361b8a01836b10a248e7a21096
HISTORICAL_TOKEN_SOURCE_SHA256=dc8d54a929cfeb42646b099cc3e0af61b8d8a13e5234d748259e225c9d86f44b
Q0_FORWARD_PROTOCOL_FROZEN=YES
Q0FWD_TRAJECTORY_FROZEN=YES
Q0FWD_RUN_ALLOWED=YES
MODEL_SHA256_RECHECK=PASS
PROTOCOL_SHA256_RECHECK=PASS
TRAJECTORY_SHA256_RECHECK=PASS
HISTORICAL_SOURCE_TOKEN_SHA256_RECHECK=PASS
R1_TOKENS_GREEDY_USED_FOR_Q0FWD=NO
Q0FWD_CAPTURE_OFF_INERTNESS=PASS
OBSERVED_COMPUTE_SUBMISSION_COUNT=AMBIGU
REQUIRED_SCIENTIFIC_SUBMISSION_COUNT=NON_ETABLI
TOTAL_REQUIRED_TARGET_SAMPLES=NON_ETABLI
TOTAL_CAPTURED_TARGET_SAMPLES=672_PAYLOADS_SUR_2_EPOQUES
SUBMITTED_TARGET_CAPTURE_COMPLETENESS=NON_ETABLI
PAYLOAD_PROVENANCE_COMPLETE=PARTIAL (8 CSV produits, sans decode_step)
ZERO_INPUT_NONZERO_OUTPUT_CONTRADICTION_COUNT=NON_MESURE
CAPTURE_CONTENT_VALIDITY=NON_ETABLI
REFERENCE_SAMPLE_TEMPORAL_PAIRING=PARTIAL
LOGIT_BITWISE_MISMATCH_COUNT=0
Q0FWD_CAPTURE_GRAFT_NUMERICALLY_TRANSPARENT=YES
INDEPENDENT_ROW_OUTPUT_INDEXING_TEST=PASS
OUTPUT_INDEXING_CODE_UNCHANGED=YES
PREDECLARED_SENTINEL_SELECTION_SHA256_RECHECK=PASS
ROLE_COVERAGE_COUNT=6_SELECTION_FIGEE_NON_UTILISEE
Q0FWD_SENTINEL_TARGET_SAMPLE_COUNT=0
Q0FWD_SENTINEL_FAILED_SAMPLE_COUNT=0
Q0FWD_SENTINEL_SUBSET_VALIDATION=NOT_EXECUTED
TOTAL_NUMERICALLY_RECONSTRUCTED_TARGET_SAMPLES=0
FAILED_TARGET_SAMPLE_COUNT=0
Q0FWD_FULL_NATIVE_REFERENCE_VALIDATION=NOT_EXECUTED
MISSING_REQUIRED_SUBMISSION_COUNT=NON_ETABLI
MISSING_REQUIRED_TARGET_SAMPLE_COUNT=NON_ETABLI
Q0FWD_TEMPORAL_COVERAGE=NOT_ESTABLISHED
NATIVE_Q8_INTERMEDIATE_COMPARISON=SOURCE_QUALIFIED_NOT_DIRECTLY_OBSERVED
Q0_FORWARD_STATUS=BLOCKED
Q1_FORWARD_ALLOWED=NO
FIRST_BLOCKING_GATE=EVENT_LEDGER
FIRST_FAILURE=NONE
Q0_PREDECLARED_STATUS=NOT_EVALUABLE_NO_PREDECLARED_PROTOCOL
Q0_HISTORICAL_STATUS=NOT_PASSED
Q1_HISTORICAL_ALLOWED=NO
HARNESS_DEFECT_56_STATUS=RESOLVED_VERIFIED_BY_TARGETED_PRESERVATION_AND_BITWISE_TRANSPARENCY
PRODUCT_MMVQ_SOURCE_MODIFIED=NO
MODEL_GGUF_MODIFIED=NO
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
ENGINE007_NEXT_STEP=INSTRUMENT_EVENT_LEDGER_AND_DECODE_STEP_THEN_RERUN_Q0FWD_V1
```

## 7 — Livrables réellement créés

```
Q0FWD_RUN1_PREFLIGHT.txt                 4 rechecks + provenance du build
Q0FWD_RUN1_OFF_CONFIG.txt                configuration OFF + inertie
Q0FWD_RUN1_OFF_LOG.txt                   journal OFF
Q0FWD_RUN1_OFF_LOGITS.bin                8939536 octets
Q0FWD_RUN1_ON_CONFIG.txt                 configuration ON
Q0FWD_RUN1_ON_LOG.txt                    journal ON
Q0FWD_RUN1_ON_LOGITS.bin                 8939536 octets, identique a OFF
Q0FWD_RUN1_LOGIT_BITWISE_COMPARISON.csv  9 pas + TOTAL, mismatch=0
Q0FWD_RUN1_EVENT_LEDGER.csv              header + constat d absence
Q0FWD_RUN1_FIRST_FAILURE.txt             le gate, ce qui manque, l ambiguite
Q0FWD_RUN1_STATUS.txt                    synthese et ledger
tokens.txt                               10 IDs, SHA256 historique reproduit
capon/                                   672 payloads + 8 CSV de provenance
```

**Non créés** : `Q0FWD_RUN1_OFF_STEP_LOGITS.csv`, `Q0FWD_RUN1_SUBMISSION_CLASSIFICATION.csv`,
`Q0FWD_RUN1_CAPTURE_COMPLETENESS.csv`, `Q0FWD_RUN1_PAYLOAD_PROVENANCE.csv`,
`Q0FWD_RUN1_TEMPORAL_PAIRING.csv`, `Q0FWD_RUN1_SENTINEL_VALIDATION.csv`,
`Q0FWD_RUN1_FULL_NATIVE_REFERENCE_VALIDATION.csv`, `Q0FWD_RUN1_ROLE_SUMMARY.csv`,
`Q0FWD_RUN1_SUBMISSION_SUMMARY.csv`.

## 8 — Le test minimal pour fermer ce gate

**Un seul ajout, dans la greffe diagnostique** (pas dans le code produit) :

```
1. un compteur de pas de decodage, incremente a chaque llama_decode de f1a-eval,
   expose a f1a-capture (le harnais sait deja quel pas il execute, l.77) ;
2. une ligne de ledger ecrite A LA VOLEE a chaque evenement, avec au minimum
   decode_step + submission_id + event_kind + target_count ;
3. le decode_step propage dans GRAPH_CAPTURE_MANIFEST.csv et PAYLOAD_PROVENANCE.csv
   (la colonne n existe pas aujourd hui).
```

Le point 1 est le seul vraiment structurant : sans lui, `decode_step` n'existe nulle part, et
`submission <-> pas` reste indécidable quel que soit le post-traitement.

**Après quoi** : reconstruire le binaire, refaire le préflight, relancer OFF/ON, et only then
enchaîner pairing → sentinelles → exhaustif.

`Q0_FORWARD_STATUS=BLOCKED`, `Q1_FORWARD_ALLOWED=NO`, aucune sentinelle exécutée, protocole et
trajectoire intacts.
