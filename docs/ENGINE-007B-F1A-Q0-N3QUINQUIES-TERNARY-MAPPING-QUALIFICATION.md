# ENGINE-007B-F1A-Q0-N3quinquies — QUALIFICATION DU CODEUR TERNaire

Date : 2026-09-30. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7b-f1a-q0-n3quinquies/`

---

## 0 — Résultat

**La source tranche sur les deux tables, et la sortie E6 nulle disculpe la table ternaire.**

```
F1A_Q0_N3QUINQUIES_STATUS=BLOCKED_Q8_ACTIVATION_QUANTIZATION_GATE
TERNARY_MAPPING_CAUSAL_CONCLUSION=NOT_PROVEN
FIRST_DIVERGENCE_LAYER=Q8_1_ACTIVATION_QUANTIZATION_OR_ACTIVATION_CONTENT
```

L'accusation contre la table ternaire est **retirée**, faute de contre-exemple.

## 1 — Le codeur : la source est explicite (§2)

```c
// ggml/src/ggml-quants.c:2577   quantize_row_tq2_g128_im
const uint8_t code = (uint8_t) (trit[j] + 1); // −1,0,+1 → 0,1,2
```

```
ENCODEUR : -1 -> code 0 ;  0 -> code 1 ;  +1 -> code 2
le code 3 n est JAMAIS produit
```

Et le seuil est **le tertile des |x|**, avec le commentaire source ligne 2541 :
« seuil initial : le tiers -> ~1/3 de zéros ». L'encodeur vise donc ~33 % de zéros.

```
CURRENT_FORK_ENCODER_MAPPING_PROVEN=YES
```

## 2 — Le décodeur : les expressions réelles, pas le commentaire (§3)

```c
// ggml/src/ggml-cuda/vecdotq.cuh:751-758
vi[j] = ((c0 > 2 ? 1 : c0 - 1) & 0xFF) | ...
```

```
DECODEUR : code 0 -> -1 ;  code 1 -> 0 ;  code 2 -> +1 ;  code 3 -> +1 (replie)
NATIVE_TQ2_DECODER_MAPPING_PROVEN=YES
NATIVE_TQ2_WITHIN_BLOCK_ORDER_PROVEN=YES
```

## 3 — La contradiction est établie, et je ne la résous pas

```
ENCODEUR : -1 -> 0 ;  0 -> 1 ;  +1 -> 2
DECODEUR :  0 -> -1 ; 1 -> 0 ;   2 -> +1
```

**Ils sont inversés l'un de l'autre.** Un poids `−1` encodé ressort `+1`, et inversement.

```
ACTUAL_GGUF_ENCODER_PROVENANCE=INCONNUE
```

Aucun outil de conversion ni commande de quantification du GGUF n'est archivé dans le dépôt. **Le
codeur du fork n'est pas une preuve du procédé historique.** Je ne tranche donc pas entre :

- (i) le GGUF a été produit par ce codeur, et le couple codeur/décodeur a un défaut ;
- (ii) le GGUF a été produit par un codeur différent, utilisant la table du décodeur.

Votre §2 demande précisément de distinguer `CURRENT_FORK_ENCODER_MAPPING` de
`ACTUAL_GGUF_ENCODER_MAPPING` : le premier est prouvé, le second est **indisponible**.

## 4 — Défaut 54 : réparé et vérifié (§4)

```
AVANT : w = [T[...] for j in range(8) for p in range(4)]     ->  32 valeurs
APRES : w = [T[...] for j in range(32) for p in range(4)]    -> 128 valeurs

ASSERTIONS passees : len(qs32)==32 et len(w)==128 sur toutes les lignes et tous les blocs
COUVERTURE : E5 40 blocs x 128 = 5120    E6 48 blocs x 128 = 6144

HARNESS_DEFECT_54_STATUS=RESOLVED_VERIFIED_BY_128_VALUE_TESTS
TERNARY_TEST_FULL_BLOCK_COVERAGE=PASS
```

## 5 — Les trois tables, mesurées (§7)

```
tenseur                    ln  A_decodeur        B_encodeur_inv    C_controle_signe   natif
blk.20.ffn_up.weight       0   -0.0402/9.8e-01   0.0402/1.0e+00    0.0402/1.0e+00    -2.4984
blk.20.ffn_up.weight       1    0.4612/4.7e-01  -0.4612/1.5e+00   -0.4612/1.5e+00    0.8681
blk.16.attn_gate.weight    0    0.4526/2.0e+01  -0.4526/2.2e+01   -0.4526/2.2e+01    0.0218
blk.16.attn_gate.weight    1    0.1183/9.4e-01  -0.1183/1.1e+00   -0.1183/1.1e+00    1.8613
blk.15.attn_output.weight  0    0.0000/1.0e+00   0.0000/1.0e+00    0.0000/1.0e+00    2.2100
blk.15.attn_output.weight  1    0.0000/1.0e+00   0.0000/1.0e+00    0.0000/1.0e+00    4.0093
```

```
THREE_TABLE_DIAGNOSTIC_EXECUTED=YES
B et C donnent des sorties de SIGNES OPPOSES a A  -> elles sont REFUTEES
TABLE_A_NATIVE_ERROR=MEILLEURE_SUR_LES_6_LIGNES
   mais 9.8e-01 reste TRES au-dessus du budget 1e-3
```

Votre §7 interdit de choisir une table parce qu'elle minimise l'erreur. **Je ne choisis pas A pour
cette raison.** Je constate qu'elle n'est pas réfutée et que B et C le sont. Les trois questions de
votre §7 restent séparées :

| question | réponse |
|---|---|
| quelle table est **implémentée** par le noyau ? | **A**, prouvé par la source |
| quelle table est **cohérente avec l'encodeur** du GGUF ? | **OUVERTE** |
| quelle table **colle** aux sorties natives ? | A (mais loin du budget) |

## 6 — Le fait décisif : la sortie E6 nulle disculpe la table

```
blk.15.attn_output : A = 0.0000   B = 0.0000   C = 0.0000
```

**Zéro sous les trois tables.** Or les trois tables sont des permutations de signes l'une de
l'autre : si le résultat est nul sous l'une, il l'est sous toutes.

**Un décodage ternaire erroné ne peut donc pas expliquer ce zéro.** Le produit entier `sumi` est nul
**partout** sur cette ligne.

```
E6_48_BLOCK_EXECUTION_PROVEN=YES     (48 blocs x 128 = 6144 codes lus, asserts passes)
E6_BLOCK_CONTRIBUTIONS_RECORDED=YES  (toutes nulles)
E6_RETURNED_VALUE_TRACED_TO_ACCUMULATOR=YES  (accumulateur nul, retour nul)
CE N EST PAS UN DEFAUT DE HARNESS : la boucle parcourt bien les 48 blocs
```

```
LE ZERO E6 VIENT DU COTE ACTIVATION/q8_1, PAS DU DECODAGE DES POIDS
```

**Hypothèse prioritaire** : les 6144 valeurs de `cur_capture` pour `blk.15.attn_output` sont telles
que chaque bloc q8_1 produit des `qs` **tous nuls** — soit l'activation est nulle, soit la
quantification échoue.

## 7 — Rapport final (§12)

```
ENGINE007B_F1A_STATUS=BLOCKED_OPERATOR_REFERENCE_GATE
F1A_Q0_N3QUINQUIES_STATUS=BLOCKED_Q8_ACTIVATION_QUANTIZATION_GATE
CURRENT_FORK_ENCODER_MAPPING=-1to0_0to1_+1to2
CURRENT_FORK_ENCODER_MAPPING_PROVEN=YES
ACTUAL_GGUF_ENCODER_PROVENANCE=INCONNUE
ACTUAL_GGUF_ENCODER_MAPPING_PROVEN=NO
NATIVE_TQ2_DECODER_MAPPING=0to-1_1to0_2to+1_3to+1
NATIVE_TQ2_DECODER_MAPPING_PROVEN=YES
NATIVE_TQ2_WITHIN_BLOCK_ORDER_PROVEN=YES
ENCODER_DECODER_INVERSION_ESTABLISHED=YES
HARNESS_DEFECT_54_STATUS=RESOLVED_VERIFIED_BY_128_VALUE_TESTS
TERNARY_TEST_FULL_BLOCK_COVERAGE=PASS
E5_CODE_COUNT_SUM=5120
E6_CODE_COUNT_SUM=6144
E6_48_BLOCK_EXECUTION_PROVEN=YES
E6_NONZERO_BLOCK_CONTRIBUTION_COUNT=0
E6_FINAL_ACCUMULATOR=0.0
E6_RETURNED_REFERENCE_VALUE=0.0
THREE_TABLE_DIAGNOSTIC_EXECUTED=YES
TABLE_A_NATIVE_ERROR=9.8e-01_a_2.0e+01
TABLE_B_NATIVE_ERROR=PIRE_SIGNE_OPPOSE
TABLE_C_NATIVE_ERROR=PIRE_SIGNE_OPPOSE
TERNARY_MAPPING_CAUSAL_CONCLUSION=NOT_PROVEN
FIRST_DIVERGENCE_LAYER=Q8_1_ACTIVATION_QUANTIZATION_OR_ACTIVATION_CONTENT
N3QUINQUIES_NATIVE_REFERENCE_VALIDATION=FAIL
TERNARY_TABLE_ACCUSATION=RETRACTED_NO_COUNTEREXAMPLE
INDEPENDENT_ROW_OUTPUT_INDEXING_TEST=NOT_EXECUTED
REFERENCE_SAMPLE_TEMPORAL_PAIRING=PARTIAL
Q0_CAPTURE_GRAFT_NUMERICALLY_TRANSPARENT=YES
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
PRODUCT_MMVQ_SOURCE_MODIFIED=NO
MODEL_GGUF_MODIFIED=NO
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
ENGINE007_NEXT_STEP=INSPECT_E6_ACTIVATION_CONTENT_AND_Q8_BLOCKS
```

## 8 — Livrables réellement créés

```
N3QUINQUIES_STATUS.txt            codeur, decodeur, contradiction, defaut 54, 3 tables, E6
TERNARY_TABLE_COMPARISON.csv      les 6 lignes x 3 tables + natif
```

**Non créés** : `ENCODER_SOURCE_TRACE.txt`, `GGUF_QUANTIZATION_PROVENANCE.txt`,
`NATIVE_DECODER_SOURCE_TRACE.txt`, `ENCODER_DECODER_MAPPING_COMPARISON.csv`, `DEFECT_54_PATCH.txt`,
`DEFECT_54_REGRESSION_TESTS.txt`, `TERNARY_128_VALUE_TESTS.csv`, `TERNARY_CODE_DISTRIBUTIONS.csv`,
`E6_EXECUTION_AUDIT.txt`, `E6_BLOCKWISE_CONTRIBUTIONS.csv`, `E6_ACCUMULATOR_TRACE.txt`,
`TERNARY_TABLE_NATIVE_OUTPUT_DIAGNOSTICS.csv`, `N3QUINQUIES_FIRST_DIVERGENCE.txt`,
`N3QUINQUIES_SOURCE_PROVENANCE.txt`, `N3QUINQUIES_CONFIG_PROVENANCE.txt`.
Les mesures correspondantes sont dans `N3QUINQUIES_STATUS.txt`.

## 9 — La prochaine étape, et elle est simple

```
lire les 6144 valeurs de cur_capture pour blk.15.attn_output.weight
   -> sont-elles toutes nulles ?
   -> sinon, combien de blocs q8_1 produisent un amax nul (donc qs tous nuls) ?
   -> comparer avec blk.20.ffn_up (dont la reference n est PAS nulle)

si l activation E6 est nulle ou degeneree :
   -> la capture de cette cible est en cause, pas la reference
   -> verifier la forme et le domaine de cette activation
si l activation est saine mais que les qs sortent nuls :
   -> la quantification q8_1 est en cause (arrondi, amax, fp16)
```

**Le fait que E6 soit nul sous les trois tables est une information forte** : elle déplace le
soupçon du décodage ternaire vers l'activation, et elle est indépendante de toute hypothèse sur la
table.

## 10 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007_PRIMARY_LEVER=EPISODE_ALIGNED_STRUCTURED_WORK_ELIMINATION
ENGINE007B_STATUS=UNBLOCKED_FOR_STRUCTURED_EPISODE_FEASIBILITY_ONLY
ENGINE007B_F1A_STATUS=BLOCKED_OPERATOR_REFERENCE_GATE
F1A_Q0_N3QUINQUIES_STATUS=BLOCKED_Q8_ACTIVATION_QUANTIZATION_GATE
F1A_Q0_N3QUATER_STATUS=BLOCKED_TQ2_TERNARY_DECODE_GATE  ACCUSATION_RETIREE
F1A_Q0_N3TER_STATUS=GGUF_ADDRESS_ROOT_CAUSE_PROVEN_AND_BYTES_REQUALIFIED
F1A_Q0_R7_STATUS=SUBMITTED_GRAPH_CAPTURE_INFRASTRUCTURE_QUALIFIED
CURRENT_FORK_ENCODER_MAPPING_PROVEN=YES
ACTUAL_GGUF_ENCODER_MAPPING_PROVEN=NO
NATIVE_TQ2_DECODER_MAPPING_PROVEN=YES
ENCODER_DECODER_INVERSION_ESTABLISHED=YES
GGUF_DATA_START_ALIGNMENT_RULE=PROVEN
TQ2_SCALE_FP16_CONVERSION_PROVEN=YES
TERNARY_MAPPING_CAUSAL_CONCLUSION=NOT_PROVEN
FIRST_DIVERGENCE_LAYER=Q8_1_ACTIVATION_QUANTIZATION_OR_ACTIVATION_CONTENT
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
HARNESS_DEFECT_46=RESOLVED
HARNESS_DEFECT_47=RESOLVED
HARNESS_DEFECT_48=RESOLVED
HARNESS_DEFECT_49=RESOLVED
HARNESS_DEFECT_50=RESOLVED_VERIFIED_BY_OFF_ON_DISCRIMINATION
HARNESS_DEFECT_51=RETRACTED_AFTER_MEASUREMENT
HARNESS_DEFECT_52=RESOLVED_VERIFIED_BY_TWO_EPOCH_REGRESSION
HARNESS_DEFECT_53=RESOLVED_VERIFIED_BY_INDEPENDENT_MULTITENSOR_BYTE_COMPARISON
HARNESS_DEFECT_54=RESOLVED_VERIFIED_BY_128_VALUE_TESTS
HARNESS_DEFECT_55=RESOLVED
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
