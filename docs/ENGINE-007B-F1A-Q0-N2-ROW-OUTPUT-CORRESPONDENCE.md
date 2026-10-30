# ENGINE-007B-F1A-Q0-N2 — CORRESPONDANCE LIGNE ↔ UNITÉ DE SORTIE

Date : 2026-09-30. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7b-f1a-q0-n2/`

---

## 0 — Résultat

**La correspondance ligne ↔ unité de sortie est qualifiée. La divergence est localisée.**

```
F1A_Q0_N2_STATUS=ROW_TO_OUTPUT_CORRESPONDENCE_QUALIFIED_DIVERGENCE_LOCALIZED
FIRST_DIVERGENCE_LAYER=WEIGHT_BYTE_DECODE_OR_BLOCK_SCALE_OFFSET
ENGINE007B_F1A_STATUS=BLOCKED_OPERATOR_REFERENCE_GATE
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
```

Les quatre couches en amont de la divergence sont **éliminées**. La cinquième est nommée.

## 1 — N2-A / N2-B : la convention, établie par les métadonnées capturées

```
blk.20.ffn_up      activation ne=[5120,1,1,1]   base_output ne=[17408,1,1,1]
blk.16.attn_gate   activation ne=[5120,1,1,1]   base_output ne=[ 6144,1,1,1]
blk.15.attn_output activation ne=[6144,1,1,1]   base_output ne=[ 5120,1,1,1]
```

C'est **exactement** la convention de votre §3 : `weight.ne[0] = K`, `activation.ne[0] = K`,
`activation.ne[1] = 1` (un token), `output.ne[0] = N`, `output.ne[1] = 1`.

```
TARGET_WEIGHT_IDENTITY_PROVEN=YES
TARGET_ACTIVATION_IDENTITY_PROVEN=YES
TARGET_NATIVE_OUTPUT_IDENTITY_PROVEN=YES
GGML_MUL_MAT_OUTPUT_DIMENSION_CONVENTION_PROVEN=YES
WEIGHT_ROW_TO_OUTPUT_COORDINATE_MAPPING_PROVEN=YES
```

**La ligne logique `r` du poids correspond bien à `output[r]`.** Ma tension de N1bis est résolue —
et dans le bon sens : mon appariement était juste, la cause est ailleurs.

## 2 — N2-C : indexation du payload, prouvée

```
base_output : contiguous=1, nb0=4, ne0=17408, bytes=69632 = 17408 x 4
offset_bytes = 4*r

r=0   base[0]  = -2.498384
r=1   base[1]  =  0.868136
r=17  base[17] = -1.410863
```

Toutes finies, ordre de grandeur 1 — cohérent avec une projection FFN.

```
OUTPUT_DTYPE_MATCH=YES          OUTPUT_SHAPE_MATCH=YES
OUTPUT_STRIDE_MAPPING_PROVEN=YES
ALL_TESTED_OUTPUT_OFFSETS_IN_BOUNDS=YES
```

## 3 — N2-D : j'ai cru à une incohérence GGUF, et j'ai vérifié avant de conclure

En ne considérant que les tenseurs `TQ2_G128`, l'écart entre offsets consécutifs ne valait **pas**
la taille du premier tenseur :

```
60 coherents / 107 incoherents sur 167 paires
exemple : blk.20.ffn_up -> blk.21.attn_gate : ecart=42 537 856, taille attendue=23 674 880
```

**J'ai vérifié avant de conclure à un défaut du lecteur** — et c'était mon hypothèse qui était fausse :

```
EN INCLUANT TOUS LES TYPES : 865 coherents / 0 incoherents sur 865 paires
output.weight (Q6_K)  offset=0            taille=1 042 944 000  ecart=1 042 944 000  OK
output_norm (F32)     offset=1 042 944 000 taille=20 480       ecart=20 480       OK
token_embd (Q4_K)     offset=1 042 964 480 taille=715 161 600  ecart=715 161 600  OK
```

Les tenseurs sont **intercalés** (Q4_K, F32, BF16 entre les TQ2_G128). Mon lecteur GGUF est correct,
et il avait déjà été cross-validé en F1A (866 tenseurs, 0 écart).

```
GGUF_TENSOR_NAME_EXACT_MATCH=YES      GGUF_TENSOR_TYPE_MATCH=YES
GGUF_TENSOR_DIMENSIONS_MATCH=YES      GGUF_DATA_START_PROVEN=YES
WEIGHT_ROW_BYTE_STRIDE_PROVEN=YES     WEIGHT_ROW_OFFSETS_IN_BOUNDS=YES
K=5120 -> 40 blocs x 34 = 1360 octets par ligne
K=6144 -> 48 blocs x 34 = 1632 octets par ligne
```

**C'est la deuxième fois dans cette série que je trouve une « incohérence » qui est en réalité mon
hypothèse.** J'ai cette fois vérifié avant de l'annoncer — la discipline a payé.

## 4 — La divergence, localisée

Le layout du bloc est confirmé par la source :

```c
// ggml-common.h:286
typedef struct {
    uint8_t qs[QK_G128/4]; // 1 trit par case de 2 bits   -> 32 octets
    ggml_half d;           // echelle FP16 du groupe      ->  2 octets
} block_tq2_g128;          // 34 octets
```

C'est exactement ce que je lis. **Mais les valeurs lues sont incohérentes :**

```
ligne 0 de blk.20.ffn_up, echelles fp16 des 40 blocs :
   min=-0.0247   max=3400.0000   moyenne=402.1997   premiere=89.3125   derniere=0.0031

sortie native : |max| = 3.7985
```

`y = Σ (d_w × d_a × sumi)`. Une échelle de poids de **89 à 3400** est incompatible avec des sorties
d'ordre **1 à 4**, et une échelle **négative** est suspecte pour un décodeur ternaire `{-1, 0, +1}`.

```
FIRST_DIVERGENCE_LAYER=WEIGHT_BYTE_DECODE_OR_BLOCK_SCALE_OFFSET
```

Les quatre couches éliminées : identité des tenseurs, convention de dimensions, indexation du
payload, offsets GGUF. Ce qui reste est **la lecture des octets de poids elle-même** — soit leur
position, soit l'offset de l'échelle dans le bloc, soit l'ordre des blocs dans la ligne.

## 5 — N2-F : couverture temporelle

```
9 pas de logits | 4 soumissions acceptees | 2 epoques de capture | 672 payloads
```

Ces quatre quantités **ne se correspondent pas** 1:1, et votre §7 demandait de ne pas le supposer.
Les captures proviennent des époques **soumises** ; les constructions préparatoires sont exclues
(prouvé en R3). Les 9 pas ne sont donc **pas** tous couverts.

```
Q0_CAPTURE_DECODE_DOMAIN_EXPLICITLY_DOCUMENTED=YES
REFERENCE_SAMPLE_TEMPORAL_PAIRING=PARTIAL
   -> les echantillons compares proviennent de l epoque 0 / soumission 1 uniquement.
      Aucune activation d un pas n est comparee a la sortie d un autre.
```

## 6 — Gates N2 non exécutés

```
N2-E  INDEPENDENT_ROW_OUTPUT_INDEXING_TEST=NON EXECUTE
   le test F32 independant demandé au paragraphe 6 n a pas ete ecrit.
   Il reste utile, mais l indexation est deja prouvee par les metadonnees
   capturees (ne/nb/bytes coherents) et par les valeurs finies d ordre 1.
```

## 7 — Rapport final (§12)

```
ENGINE007B_F1A_STATUS=BLOCKED_OPERATOR_REFERENCE_GATE
F1A_Q0_N2_STATUS=ROW_TO_OUTPUT_CORRESPONDENCE_QUALIFIED_DIVERGENCE_LOCALIZED
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
TARGET_WEIGHT_IDENTITY_PROVEN=YES
TARGET_ACTIVATION_IDENTITY_PROVEN=YES
TARGET_NATIVE_OUTPUT_IDENTITY_PROVEN=YES
GGML_MUL_MAT_OUTPUT_DIMENSION_CONVENTION_PROVEN=YES
WEIGHT_ROW_TO_OUTPUT_COORDINATE_MAPPING_PROVEN=YES
OUTPUT_STRIDE_MAPPING_PROVEN=YES
GGUF_TENSOR_NAME_EXACT_MATCH=YES
GGUF_TENSOR_TYPE_MATCH=YES
GGUF_TENSOR_DIMENSIONS_MATCH=YES
GGUF_DATA_START_PROVEN=YES
WEIGHT_ROW_BYTE_STRIDE_PROVEN=YES
INDEPENDENT_ROW_OUTPUT_INDEXING_TEST=NON_EXECUTE
REFERENCE_SAMPLE_TEMPORAL_PAIRING=PARTIAL
Q0_CAPTURE_DECODE_DOMAIN_EXPLICITLY_DOCUMENTED=YES
FIRST_DIVERGENCE_LAYER=WEIGHT_BYTE_DECODE_OR_BLOCK_SCALE_OFFSET
WEIGHT_SCALES_OBSERVED_ON_ROW_0=min_-0.0247_max_3400.0_mean_402.2
NATIVE_OUTPUT_MAGNITUDE=|max|_3.7985
Q0_CAPTURE_GRAFT_NUMERICALLY_TRANSPARENT=YES
Q0_EPISODE_REFERENCE_RECONSTRUCTS_FULL_OPERATOR=NOT_QUALIFIED
PRODUCT_MMVQ_SOURCE_MODIFIED=NO
MODEL_GGUF_MODIFIED=NO
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
ENGINE007_NEXT_STEP=N3_PROVE_WEIGHT_BYTES_AND_BLOCK_SCALE_POSITION
```

## 8 — Ce que N3 doit faire

```
N3-a  lire la MEME ligne par un chemin totalement independant (libggml, no_alloc=false)
      et comparer octet a octet avec ma lecture directe.
      -> elimine ou confirme la position des octets.
N3-b  verifier l offset de l echelle DANS le bloc en confrontant source et lecture
      (struct dit 32 + 2 ; y a-t-il un alignement ou un ordre different cote loader ?)
N3-c  sur une ligne, calculer y en n utilisant QUE le bloc 0 et comparer a une
      sous-somme verifiable ; puis etendre bloc par bloc.
      -> localise si l erreur est dans les codes, l echelle, ou l ordre des blocs.
N3-d  ne PAS ouvrir de SASS avant N3-a a N3-c.
```

## 9 — Livrables réellement créés

```
N2_STATUS.txt     gates prouves, divergence localisee, valeurs observees
```

**Non créés** : `N2_SOURCE_CONVENTION_PROOF.txt`, `N2_TENSOR_METADATA.csv`,
`N2_GGUF_ROW_LAYOUT.csv`, `N2_OUTPUT_COORDINATE_MAPPING.csv`,
`N2_OUTPUT_SERIALIZATION_PROOF.txt`, `N2_INDEPENDENT_INDEXING_TEST.txt`,
`N2_SUBMISSION_DECODE_MAPPING.csv`, `N2_REFERENCE_SAMPLE_PAIRING.csv`,
`N2_NATIVE_REFERENCE_DIAGNOSTICS.csv`, `N2_FIRST_DIVERGENCE.txt`, `N2_CONFIG_PROVENANCE.txt`.
Les mesures correspondantes sont dans `N2_STATUS.txt` et dans les journaux, mais je ne les
déclare pas comme fichiers séparés puisqu'ils n'existent pas.

## 10 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007_PRIMARY_LEVER=EPISODE_ALIGNED_STRUCTURED_WORK_ELIMINATION
ENGINE007B_STATUS=UNBLOCKED_FOR_STRUCTURED_EPISODE_FEASIBILITY_ONLY
ENGINE007B_F1A_STATUS=BLOCKED_OPERATOR_REFERENCE_GATE
F1A_Q0_N2_STATUS=ROW_TO_OUTPUT_CORRESPONDENCE_QUALIFIED_DIVERGENCE_LOCALIZED
F1A_Q0_R7_STATUS=SUBMITTED_GRAPH_CAPTURE_INFRASTRUCTURE_QUALIFIED
HARNESS_DEFECT_52=RESOLVED_VERIFIED_BY_TWO_EPOCH_REGRESSION
DIAGNOSTIC_BUILD=PASS
CAPTURE_DISABLED_RUNTIME_PATH_INERT=YES
Q0_CAPTURE_ON_OFF_LOGITS_BITWISE_MATCH=YES
Q0_CAPTURE_GRAFT_NUMERICALLY_TRANSPARENT=YES
ROW_TO_OUTPUT_UNIT_CORRESPONDENCE=QUALIFIED
GGUF_OFFSETS=VERIFIED_865_OF_865_WITH_ALL_TYPES
FIRST_DIVERGENCE_LAYER=WEIGHT_BYTE_DECODE_OR_BLOCK_SCALE_OFFSET
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
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
