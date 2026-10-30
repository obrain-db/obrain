# ENGINE-007B-F1A-Q0-N6 — PROVENANCE TEMPORELLE ET COUVERTURE DES RÔLES

Date : 2026-09-30. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7b-f1a-q0-n6/`

---

## 0 — Résultat

**Le protocole Q0 préenregistré n'existe pas. Je m'arrête sur cette ambiguïté.**

```
Q0_PREDECLARED_PROTOCOL_FOUND=NO
Q0_PREDECLARED_PROTOCOL_PROVENANCE=FAIL
REQUIRED_Q0_POPULATION_DEFINED=NO
Q0_TEMPORAL_POPULATION_SPECIFICATION=AMBIGUOUS_PREEXISTING_PROTOCOL

F1A_Q0_N6_STATUS=BLOCKED_Q0_PROTOCOL_PROVENANCE_GATE
FIRST_BLOCKING_GATE=Q0_PREDECLARED_PROTOCOL_PROVENANCE
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
```

## 1 — La recherche, et ce qu'elle a trouvé (§5)

```
docs/                               aucune definition de Q0 anterieure aux resultats
git log -S "Q0" -- docs/            les occurrences anciennes portent sur les NIVEAUX DE
                                    CONFORMITE GQL (docs/user-guide/gql/*), pas l engine-007
git log --diff-filter=A docs/*Q0*   aucun ajout de document Q0 dans l historique
qualification/                      les Q0_*.txt existants sont des SORTIES de passe
                                    (Q0_STATUS=, Q0_ON_LOGITS.bin), pas une definition de population
grep Q0_REQUIRED|Q0_PROTOCOL|PROTOCOLE Q0
  sur banc-obrain et exp-artifacts  UNE SEULE occurrence : mon propre rapport N5, ecrit aujourd hui
```

Les rapports ENGINE-007B sont **non suivis par git** (`git status` → `??`), donc ils ne constituent
pas non plus une trace versionnée antérieure.

**Aucun artefact consultable n'a défini Q0 avant les résultats.**

## 2 — Conséquence : je ne peux pas extraire la population

```
Q0_REQUIRED_RUNS              NON DEFINI PAR PROTOCOLE
Q0_REQUIRED_DECODE_STEPS      NON DEFINI PAR PROTOCOLE
Q0_REQUIRED_SUBMISSIONS       NON DEFINI PAR PROTOCOLE
Q0_REQUIRED_CAPTURE_EPOCHS    NON DEFINI PAR PROTOCOLE
Q0_REQUIRED_TARGETS           NON DEFINI PAR PROTOCOLE
Q0_REQUIRED_ROLES             formule dans le brief N5 (6 roles), pas dans un protocole
Q0_REQUIRED_NUMERIC_SCOPE     budget formule dans les briefs, pas dans un protocole
```

Votre §6 et §32 l'exigent : **je ne réécris pas rétroactivement une règle pour la qualifier de
préenregistrée.** Je laisse ces champs explicitement non définis.

### Ce que les briefs ont réellement dit, séparément

```
PREEXISTING_Q0_REQUIREMENTS (formules dans les briefs, pas dans un protocole)
   budget : RELATIVE_L2 <= 1e-3 ; COSINE >= 0.9999 ;
            MAX_ABS <= 1e-2 * max_abs(native_mm_base)
   NATIVE_Q8_INTERMEDIATE_COMPARISON=SOURCE_QUALIFIED_NOT_DIRECTLY_OBSERVED
   Q0_STATUS=NOT_PASSED, Q1_ALLOWED=NO tant que les gates ne sont pas fermes
   N4 : 3 vecteurs complets d epoque 0 valides
   N5 : 3 vecteurs complets d epoque 1 valides
   le brief N5 exigeait les 6 roles (ROLE_COVERAGE_COUNT=6)
   le brief N6 exige une selection preenregistree AVANT les mesures
```

Ces exigences sont **réelles** — mais elles ne définissent **pas** les quatre champs ci-dessus.
**Les 168 cibles, les 2 époques et les 9 pas sont des constats de captures, pas des exigences
écrites.** C'est exactement la distinction que votre §10 et §15 demandent de tenir.

## 3 — Ce que j'ai fait quand même : le préenregistrement prospectif

Votre §17 demande de figer la sélection des 6 rôles **avant** toute mesure d'erreur. Je l'ai fait.

```
N6_ROLE_STRATIFIED_SELECTION.csv
N6_ROLE_STRATIFIED_SELECTION.sha256
SHA256 = 448808715eea67d416be356d5f03e28b9675d15b05acb510e202a0f263697798
```

Règle déterministe, appliquée sur la liste **triée des couches réellement présentes** dans
l'inventaire (168 tenseurs) :

| rôle | occurrences réelles | retenues | couches |
|---|---|---|---|
| `ffn_gate` | 48 | 5 | 8, 20, 32, 44, 55 |
| `ffn_up` | 48 | 5 | 8, 20, 32, 44, 55 |
| `attn_gate` | 36 | 5 | 8, 20, 32, 44, 54 |
| `attn_q` | 12 | 5 | 11, 23, 35, 47, 55 |
| `attn_k` | 12 | 5 | 11, 23, 35, 47, 55 |
| `attn_output` | 12 | 5 | 11, 23, 35, 47, 55 |

```
ROLE_COVERAGE_COUNT=6
TOTAL_TENSORS=30
```

Stratification *premier / quart / milieu / trois-quarts / dernier* sur chaque liste.

**Ceci est PROSPECTIF, pas historique.** C'est un `PROPOSED_FORWARD_Q0_COMPLETION_CRITERIA`. Il est
enregistré avant toute mesure N6, donc valide pour la suite de N6 — mais **il ne répare pas l'absence
de protocole historique**, et je ne le présenterai pas comme tel.

## 4 — Ce que je n'ai PAS exécuté, et pourquoi

```
N6-B (ledger des soumissions)     NON EXECUTE
N6-B2 (9 pas <-> soumissions)     NON EXECUTE
N6-B3 (2 epoques <-> soumissions) NON EXECUTE
N6-C (couverture vs Q0)           NON EXECUTABLE sans population definie
N6-E (roles manquants)            NON EXECUTE
N6-F / 168 cibles                 NON EXECUTE
```

**Le brief est explicite** : « Le premier objectif n'est pas de produire encore plus de sorties à
1e-7. Il est de prouver exactement à quels calculs correspondent les sorties déjà validées. » Sans
population Q0 définie, je ne sais pas **quelles époques ni quelles cibles** valider — et le brief
interdit de choisir après avoir vu les erreurs.

Lancer N6-E maintenant produirait 30 tenseurs à 1e-7 sans que cela ferme un gate.

## 5 — Rapport final (§31)

```
ENGINE007B_F1A_STATUS=BLOCKED_OPERATOR_REFERENCE_GATE
F1A_Q0_N6_STATUS=BLOCKED_Q0_PROTOCOL_PROVENANCE_GATE

Q0_PREDECLARED_PROTOCOL_FOUND=NO
Q0_PREDECLARED_PROTOCOL_PROVENANCE=FAIL
REQUIRED_Q0_POPULATION_DEFINED=NO
Q0_TEMPORAL_POPULATION_SPECIFICATION=AMBIGUOUS_PREEXISTING_PROTOCOL

LOGIT_DECODE_STEPS=9
OBSERVED_COMPUTE_SUBMISSIONS=4
CAPTURE_EPOCHS=2
SUBMISSION_EVENT_LEDGER_COMPLETE=NO
SUBMISSION_DECODE_STEP_MAPPING=NON_ETABLI
CAPTURE_EPOCH_SUBMISSION_MAPPING=NON_ETABLI
REFERENCE_SAMPLE_TEMPORAL_PAIRING=PARTIAL

ALL_REQUIRED_SUBMISSIONS_CAPTURED=NON_ETABLI
ALL_REQUIRED_TARGETS_CAPTURED=NON_ETABLI

INDEPENDENT_ROW_OUTPUT_INDEXING_TEST=PASS
N5_EPOCH1_REFERENCE_VALIDATION=PASS

ROLE_COVERAGE_COUNT=6
PREDECLARED_SELECTION_HASH_VERIFIED=YES
FFN_GATE_REFERENCE_VALIDATION=NOT_EXECUTED
ATTN_Q_REFERENCE_VALIDATION=NOT_EXECUTED
ATTN_K_REFERENCE_VALIDATION=NOT_EXECUTED
N6_ROLE_STRATIFIED_REFERENCE_VALIDATION=NOT_EXECUTED
FULL_168_TARGET_NATIVE_REFERENCE_VALIDATION=NOT_EXECUTED

NATIVE_OPERATOR_REFERENCE_VALIDATION=PASS_FOR_3_TENSORS_2_EPOCHS_ONLY
CAPTURE_INFRASTRUCTURE_QUALIFIED=YES
CAPTURE_CONTENT_VALIDITY_QUALIFIED=YES
CAPTURE_GRAFT_NUMERICALLY_TRANSPARENT=YES
NATIVE_Q8_INTERMEDIATE_COMPARISON=SOURCE_QUALIFIED_NOT_DIRECTLY_OBSERVED

N3NONIES_VS_N4_OLD_METRIC_DIFFERENCE=NOT_FULLY_RECONSTRUCTED

Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
FIRST_BLOCKING_GATE=Q0_PREDECLARED_PROTOCOL_PROVENANCE

PRODUCT_MMVQ_SOURCE_MODIFIED=NO
MODEL_GGUF_MODIFIED=NO
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
ENGINE007_NEXT_STEP=OPERATOR_MUST_PREREGISTER_Q0_POPULATION_OR_AUTHORIZE_FORWARD_CRITERIA
```

## 6 — Livrables réellement créés

```
N6_STATUS.txt                        la recherche, le constat, l arret, le ledger
N6_Q0_PROTOCOL_PROVENANCE.txt        les quatre champs laisses explicitement non definis
N6_ROLE_STRATIFIED_SELECTION.csv     6 roles, 30 tenseurs, 5 couches par role
N6_ROLE_STRATIFIED_SELECTION.sha256  empreinte figee
```

**Non créés** : `N6_Q0_PREDECLARED_REQUIREMENTS.txt`, `N6_SUBMISSION_EVENT_LEDGER.csv`,
`N6_DECODE_STEP_SUBMISSION_MAPPING.csv`, `N6_CAPTURE_EPOCH_SUBMISSION_MAPPING.csv`,
`N6_Q0_POPULATION_COVERAGE.csv`, `N6_TEMPORAL_PAIRING_PROOF.txt`, `N6_FFN_GATE_VALIDATION.csv`,
`N6_ATTN_Q_VALIDATION.csv`, `N6_ATTN_K_VALIDATION.csv`, `N6_ROLE_STRATIFIED_VALIDATION.csv`,
`N6_FULL_168_TARGET_VALIDATION.csv`, `N6_COSINE_PRECISION_CHECK.txt`,
`N6_N3NONIES_N4_PROVENANCE_RECONCILIATION.txt`, `N6_FIRST_BLOCKING_GATE.txt`,
`N6_SOURCE_PROVENANCE.txt`, `N6_BUILD_PROVENANCE.txt`, `N6_CONFIG_PROVENANCE.txt`.

Le cosinus `1.0000001192` (§3) n'a pas été retraité : `N6_COSINE_PRECISION_CHECK.txt` n'existe pas.
Je note que la valeur est un artefact de la métrique, pas un défaut des vecteurs — les composantes
sont cohérentes à 7e-07.

## 7 — Ce qu'il te faut décider

Le gate bloquant est **une décision, pas une mesure** :

- **Option A** — tu fournis / autorises le protocole Q0 : quels runs, quels pas de décodage, quelles
  soumissions, quelles époques, quelles cibles, quels rôles. Alors N6-B à N6-F deviennent exécutables
  et je les enchaîne.
- **Option B** — tu autorises explicitement les `PROPOSED_FORWARD_Q0_COMPLETION_CRITERIA` comme
  politique de qualification pour la suite, en sachant qu'ils ne sont **pas** historiques. Q0 restera
  dans ce cas qualifié **selon des critères forgés après coup**, et le rapport doit le dire.

Tant que ce choix n'est pas fait :

```
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
```

## 8 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007B_F1A_STATUS=BLOCKED_OPERATOR_REFERENCE_GATE
F1A_Q0_N6_STATUS=BLOCKED_Q0_PROTOCOL_PROVENANCE_GATE
Q0_PREDECLARED_PROTOCOL_FOUND=NO
Q0_TEMPORAL_POPULATION_SPECIFICATION=AMBIGUOUS_PREEXISTING_PROTOCOL
INDEPENDENT_ROW_OUTPUT_INDEXING_TEST=PASS
N5_EPOCH1_REFERENCE_VALIDATION=PASS
ROLE_COVERAGE_COUNT=6
PREDECLARED_SELECTION_HASH_VERIFIED=YES
REFERENCE_SAMPLE_TEMPORAL_PAIRING=PARTIAL
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
HARNESS_DEFECT_56=RESOLVED
HARNESS_DEFECT_57=RESOLVED
PRODUCT_MMVQ_SOURCE_MODIFIED=NO
MODEL_GGUF_MODIFIED=NO
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
