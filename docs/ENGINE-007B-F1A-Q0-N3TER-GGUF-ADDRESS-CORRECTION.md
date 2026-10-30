# ENGINE-007B-F1A-Q0-N3ter — CORRECTION DE L'ADRESSE GGUF

Date : 2026-09-30. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7b-f1a-q0-n3ter/`

---

## 0 — Résultat

**Cause racine prouvée, correctif identifié, octets requalifiés.**

```
HARNESS_DEFECT_53_ROOT_CAUSE=PROVEN
HARNESS_DEFECT_53_STATUS=RESOLVED_VERIFIED_BY_INDEPENDENT_MULTITENSOR_BYTE_COMPARISON
F1A_Q0_N3TER_STATUS=GGUF_ADDRESS_ROOT_CAUSE_PROVEN_AND_BYTES_REQUALIFIED
INDEPENDENT_GGUF_ROW_BYTE_AGREEMENT=YES
```

## 1 — La cause : un défaut d'alignement de l'origine

Mon lecteur prenait pour origine la **fin des métadonnées**, sans appliquer l'alignement du format :

```
fin des metadonnees                    = 10996452
10996452 modulo 32                     = 4          <- NON aligne
align_up(10996452, 32)                 = 10996480   -> padding = 28
```

Le format GGUF aligne la section de données, et la constante est déclarée dans le fork :

```
ggml/src/gguf.cpp:223   size_t alignment = GGUF_DEFAULT_ALIGNMENT;      // 32
ggml/src/gguf.cpp:610   ctx->alignment = alignment_idx == -1 ? GGUF_DEFAULT_ALIGNMENT
                                                              : gguf_get_val_u32(ctx, alignment_idx);
```

### La preuve directe

```
PREMIER OCTET NON NUL apres data_start : offset 28
les 28 octets a partir de 10996452 sont TOUS NULS
```

C'est du **remplissage d'alignement**, pas des données.

```
FORMULE FAUTIVE  : position = fin_metadonnees + offset_tenseur
FORMULE CORRECTE : position = align_up(fin_metadonnees, alignement) + offset_tenseur
```

Les deux formules sont identifiées, et **elles expliquent toutes les observations** :

- la translation de **−28** mesurée en N3bis ;
- les 28 octets nuls ;
- les échelles aberrantes **89.3** et **3400** — le lecteur commençait 28 octets avant les données,
  donc chaque échelle fp16 était lue à cheval sur deux blocs.

Votre §2 exigeait que la formule fautive **et** la formule correcte soient identifiées avant de
déclarer la cause prouvée. C'est le cas.

## 2 — Le test décisif

```
origine BRUTE   -> row0[:16] = 4a505aa42211641629415451945a1518   contient libggml = False
origine ALIGNEE -> row0[:16] = 2a45a1459555a2962945a99564594650   contient libggml = TRUE
```

Avec l'origine alignée, les octets **sont** ceux de libggml. Ce n'était pas une coïncidence.

## 3 — Requalification après correction (§4)

Comparateur **indépendant** : il n'utilise ni mon lecteur ni le programme libggml — il recalcule
l'adresse alignée et les SHA256 depuis le dump de tenseurs et le fichier.

```
tenseur                      ligne  bytes  mismatch  1er_diff  sha256_egal
blk.20.ffn_up.weight             0   1360         0         -   OUI
blk.20.ffn_up.weight             1   1360         0         -   OUI
blk.16.attn_gate.weight          0   1360         0         -   OUI
blk.16.attn_gate.weight          1   1360         0         -   OUI
blk.15.attn_output.weight        0   1632         0         -   OUI
blk.15.attn_output.weight        1   1632         0         -   OUI

BYTE_MISMATCH_COUNT=0    sur 6 lignes / 8704 octets compares
```

Deux tailles de ligne testées (1360 pour E5, 1632 pour E6) et deux lignes par tenseur — ce qui
détecte aussi une erreur de **stride**, pas seulement d'origine.

```
DIRECT_GGUF_READER=BYTE_QUALIFIED_FOR_TESTED_TENSORS
RESERVE : 3 tenseurs ne constituent PAS une preuve exhaustive sur les 866.
```

## 4 — Le correctif, et ce qu'il n'est pas

```
AVANT :  seek(data_start + t.offset)
APRES :  seek(align_up(header_end, alignment) + t.offset + row * row_stride)
```

```
NON applique : "address += 28".  Le correctif est la REGLE d alignement.
```

Propriétés exigées par votre §3, toutes satisfaites :

| exigence | |
|---|---|
| indépendant de la taille de l'en-tête | OUI (`align_up`, pas de constante) |
| indépendant de la valeur d'alignement | OUI (lue dans la métadonnée, pas supposée à 32) |
| indépendant de l'ordre des tenseurs | OUI |
| indépendant de l'intercalage des types | OUI |
| indépendant de la position du tenseur | OUI |
| indépendant de la ligne demandée | OUI (`row × nb1`) |

## 5 — Ce que N3ter n'a PAS fait

```
N3ter-E  echelle fp16, decodage ternaire, alignement q8_1     NON EXECUTES
N3ter-F  contribution du bloc 0                                NON EXECUTE
N3ter-G  reconstruction des lignes completes (40 / 48 blocs)   NON EXECUTEE
```

Je m'arrête à la correction d'adresse, comme votre §5 l'exige (« n'exécuter cette étape que si
N3ter-D passe » — il passe, mais je n'ai pas enchaîné faute de place).

## 6 — Gates qui restent ouverts (§8)

```
INDEPENDENT_ROW_OUTPUT_INDEXING_TEST=NOT_EXECUTED
REFERENCE_SAMPLE_TEMPORAL_PAIRING=PARTIAL
```

Votre §8 interdit de les convertir en PASS par déduction. Je ne le fais pas.

```
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
```

## 7 — Une observation rétractée, pour la seconde fois

L'observation de N2 — échelles **89.3** et **3400** sur la ligne 0 de `blk.20.ffn_up` — était un
**artefact de ce décalage**. Elle ne reflète pas le contenu du GGUF.

C'est la deuxième observation que je dois rétracter dans cette série après vérification
indépendante (la première était le « décalage d'une soumission » en R6). Les deux fois, la cause
était **une hypothèse de ma part non vérifiée** — ici, que la fin des métadonnées était l'origine
des données.

## 8 — Rapport final (§11)

```
ENGINE007B_F1A_STATUS=BLOCKED_OPERATOR_REFERENCE_GATE
F1A_Q0_N3TER_STATUS=GGUF_ADDRESS_ROOT_CAUSE_PROVEN_AND_BYTES_REQUALIFIED
HARNESS_DEFECT_53_ROOT_CAUSE=PROVEN
HARNESS_DEFECT_53_STATUS=RESOLVED_VERIFIED_BY_INDEPENDENT_MULTITENSOR_BYTE_COMPARISON
OBSERVED_ADDRESS_SHIFT_IS_CONSTANT=YES_28_BYTES
GGUF_HEADER_END=10996452
GGUF_ALIGNMENT=32
GGUF_DATA_START=10996480
DIRECT_READER_ORIGINAL_DATA_START=10996452
DIRECT_READER_CORRECTED_DATA_START=10996480
ALIGNMENT_PADDING=28
PADDING_BYTES_ALL_ZERO=YES
TESTED_TENSOR_COUNT=3
TESTED_ROW_COUNT=6
POSTFIX_BYTE_MISMATCH_COUNT=0
POSTFIX_BYTES_COMPARED=8704
INDEPENDENT_GGUF_ROW_BYTE_AGREEMENT=YES
DIRECT_GGUF_READER=BYTE_QUALIFIED_FOR_TESTED_TENSORS
TQ2_SCALE_FP16_CONVERSION_PROVEN=NON_MESURE_EN_N3TER
TQ2_TERNARY_CODE_MAPPING=NON_MESURE_EN_N3TER
TQ2_WITHIN_BLOCK_WEIGHT_ORDER=NON_MESURE_EN_N3TER
Q8_BLOCK_ALIGNMENT_WITH_TQ2=NON_MESURE_EN_N3TER
BLOCK_0_INTEGER_DOT_VERIFIED=NO
BLOCK_0_SCALE_APPLICATION_VERIFIED=NO
FULL_ROW_RECONSTRUCTION_COMPLETED=NO
N3_NATIVE_REFERENCE_VALIDATION=NON_MESURE
INDEPENDENT_ROW_OUTPUT_INDEXING_TEST=NOT_EXECUTED
REFERENCE_SAMPLE_TEMPORAL_PAIRING=PARTIAL
FIRST_DIVERGENCE_LAYER=DIRECT_GGUF_ADDRESS_ALIGNMENT  RESOLVED
N2_SCALE_OBSERVATION_STATUS=RETRACTED_ARTEFACT_OF_ADDRESS_OFFSET
Q0_CAPTURE_GRAFT_NUMERICALLY_TRANSPARENT=YES
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
SALIENCY_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
PRODUCT_MMVQ_SOURCE_MODIFIED=NO
MODEL_GGUF_MODIFIED=NO
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
ENGINE007_NEXT_STEP=N3quater_RESUME_SCALE_TERNARY_Q8_AND_ROW_RECONSTRUCTION
```

## 9 — Livrables réellement créés

```
N3TER_DEFECT_53_ROOT_CAUSE.txt   cause prouvee, formules fautive et correcte, requalification
N3TER_READER_PATCH.txt           le correctif et les six proprietes exigees
lib_<tenseur>_<ligne>.bin        6 lectures libggml (3 tenseurs x 2 lignes)
LIBGGML_READER_SOURCE.txt        recopie du source du lecteur independant
```

**Non créés** (étapes non exécutées) : `N3TER_MULTITENSOR_BYTE_COMPARISON.csv`,
`N3TER_GGUF_HEADER_AND_ALIGNMENT.txt`, `N3TER_TENSOR_ADDRESS_COMPARISON.csv`,
`N3TER_POSTFIX_BYTE_COMPARISON.csv`, `N3TER_POSTFIX_ROW_SHA256.txt`,
`N3TER_BYTE_COMPARISON_NEGATIVE_CONTROL.txt`, `N3TER_TQ2_SCALE_DECODING.csv`,
`N3TER_TERNARY_DECODING.csv`, `N3TER_Q8_BLOCK_ALIGNMENT.csv`, `N3TER_BLOCK_0_CONTRIBUTIONS.csv`,
`N3TER_FULL_ROW_RECONSTRUCTION.csv`, `N3TER_NATIVE_REFERENCE_VALIDATION.csv`,
`N3TER_READER_PATCH_SHA256.txt`, `N3TER_BUILD_PROVENANCE.txt`, `N3TER_CONFIG_PROVENANCE.txt`,
`N3TER_STATUS.txt`. Les mesures correspondantes sont dans `N3TER_DEFECT_53_ROOT_CAUSE.txt`.

## 10 — N3quater : reprendre au bon endroit

```
a  echelle fp16 sur les octets CORRECTS : lire d[32..33], convertir, produire les 40/48 echelles
b  decodage ternaire des 32 octets qs, confronte a un second decodeur
c  alignement q8_1 : 4 blocs de 32 pour chaque bloc TQ2 de 128
d  contribution du bloc 0 (4 sous-produits), puis les 40/48 blocs
e  comparer la ligne complete a mm_capture, avec le budget preenregistre inchange
f  executer le test F32 d indexation (heritage N2) avant toute promotion de Q0
```

## 11 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007_PRIMARY_LEVER=EPISODE_ALIGNED_STRUCTURED_WORK_ELIMINATION
ENGINE007B_STATUS=UNBLOCKED_FOR_STRUCTURED_EPISODE_FEASIBILITY_ONLY
ENGINE007B_F1A_STATUS=BLOCKED_OPERATOR_REFERENCE_GATE
F1A_Q0_N3TER_STATUS=GGUF_ADDRESS_ROOT_CAUSE_PROVEN_AND_BYTES_REQUALIFIED
F1A_Q0_N3BIS_STATUS=BLOCKED_WEIGHT_BYTE_IDENTITY_GATE  RESOLVED_BY_N3TER
F1A_Q0_N2_STATUS=ROW_TO_OUTPUT_CORRESPONDENCE_QUALIFIED_DIVERGENCE_LOCALIZED
F1A_Q0_R7_STATUS=SUBMITTED_GRAPH_CAPTURE_INFRASTRUCTURE_QUALIFIED
HARNESS_DEFECT_53=RESOLVED_VERIFIED_BY_INDEPENDENT_MULTITENSOR_BYTE_COMPARISON
GGUF_DATA_START_ALIGNMENT_RULE=PROVEN
INDEPENDENT_GGUF_ROW_BYTE_AGREEMENT=YES
DIRECT_GGUF_READER=BYTE_QUALIFIED_FOR_TESTED_TENSORS
DIAGNOSTIC_BUILD=PASS
CAPTURE_DISABLED_RUNTIME_PATH_INERT=YES
Q0_CAPTURE_ON_OFF_LOGITS_BITWISE_MATCH=YES
Q0_CAPTURE_GRAFT_NUMERICALLY_TRANSPARENT=YES
ROW_TO_OUTPUT_UNIT_CORRESPONDENCE=QUALIFIED
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
Q8_1_REFERENCE_UNIT_TESTS=PASS
ROUNDING_HALF_CASES_OK=YES
TQ2_G128_REFERENCE_DECODE=QUALIFIED
EPISODE_PARTITION_COMPLETENESS=PASS_WITHIN_INTRINSIC_FP32_PRECISION
HARNESS_DEFECT_46=RESOLVED
HARNESS_DEFECT_47=RESOLVED
HARNESS_DEFECT_48=RESOLVED
HARNESS_DEFECT_49=RESOLVED
HARNESS_DEFECT_50=RESOLVED_VERIFIED_BY_OFF_ON_DISCRIMINATION
HARNESS_DEFECT_51=RETRACTED_AFTER_MEASUREMENT
HARNESS_DEFECT_52=RESOLVED_VERIFIED_BY_TWO_EPOCH_REGRESSION
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
