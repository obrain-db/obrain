# ENGINE-007B-F1A-Q0-N3 — OCTETS DE POIDS ET ARITHMÉTIQUE PAR BLOC

Date : 2026-09-30. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7b-f1a-q0-n3/`

---

## 0 — Résultat

**Le gate qui décide n'a pas été exécuté. Je m'arrête là, sans promouvoir la suspicion de N2.**

```
N3_STATUS=BLOCKED_ON_INDEPENDENT_BYTE_READ_NOT_EXECUTED
FIRST_DIVERGENCE_LAYER=NOT_PROVEN_EN_N3
ENGINE007B_F1A_STATUS=BLOCKED_OPERATOR_REFERENCE_GATE
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
```

Votre §0 dit : « Ne pas promouvoir la première divergence suspectée au rang de cause prouvée sans
comparaison indépendante. » C'est précisément ce que N3 devait produire — la lecture indépendante
par libggml — et **je ne l'ai pas écrite**. La suspicion de N2 reste donc une suspicion.

## 1 — Ce que N3 a établi par la source

### A3 : le poids n'est pas déquantifié avant le matmul

```
ggml-cuda.cu:1769   if (ggml_cuda_should_use_mmvq(src0->type, cc, ne11)) {
ggml-cuda.cu:1770       ggml_cuda_mul_mat_vec_q(ctx, src0, src1, nullptr, dst);
ggml-cuda.cu:1718   bool use_mul_mat_vec_q = ggml_is_quantized(src0->type) && ...
```

Le noyau MMVQ consomme **directement** les octets `TQ2_G128` quantifiés. Il n'y a **pas** de
déquantification du poids avant le produit.

```
NATIVE_WEIGHT_OPERAND_BYTE_COMPARISON=NOT_EXTRACTED
   -> l operande natif est comparable octet a octet EN PRINCIPE, mais je ne l ai pas extrait.
```

### N3-b : layout du bloc, prouvé par assertion de compilation

```
ggml-common.h:286   typedef struct { uint8_t qs[QK_G128/4]; ggml_half d; } block_tq2_g128;
ggml-common.h:287   static_assert(sizeof(block_tq2_g128) == sizeof(ggml_half) + QK_G128/4, ...)

TQ2_BLOCK_SIZE_PROVEN=YES   34 octets
TQ2_SCALE_BYTE_OFFSET_PROVEN=YES   offset 32
```

Le `static_assert` garantit l'absence de remplissage : `qs[0..31]` à l'offset 0, `d` aux offsets
32-33. Ce n'est pas une supposition, c'est une contrainte vérifiée à la compilation.

### N2-C précisé : la population des 865 transitions

```
GGUF : 866 tenseurs.  865 transitions = 866 - 1.
-> la population est l ENSEMBLE COMPLET des tenseurs, tries par offset, TOUS TYPES CONFONDUS.
OFFSET_TRANSITION_MISMATCHES=0 sur 865.
```

Mon hypothèse de contiguïté des seuls `TQ2_G128` était fausse (60 cohérents / 107 incohérents dans
ce sous-ensemble). C'est bien la vérification tous-types qui tranche.

## 2 — Le gate bloquant

```
INDEPENDENT_GGUF_ROW_BYTE_AGREEMENT=NON_MESURE
```

Le §2/A2 demande un **programme diagnostique distinct** utilisant le chargeur GGUF de libggml,
lisant les octets de la même ligne par un chemin indépendant de mon calcul d'adresse.

**Sans lui, je ne peux pas trancher entre :**

- (i) mes octets sont mal lus — position erronée, décalage, mauvais tenseur ;
- (ii) ces valeurs sont **réellement présentes dans le fichier**, et l'incohérence est ailleurs.

C'est exactement la question que le gate doit fermer, et je ne l'ai pas fermée.

## 3 — La question ouverte, énoncée précisément

```
ligne 0 de blk.20.ffn_up.weight, echelles fp16 des 40 blocs :
   min = -0.0247     max = 3400.0     moyenne = 402.2     premiere = 89.3125
sortie native : |max| = 3.7985
```

Votre §3 dit deux choses que je respecte :

- « Ne pas corriger un offset parce qu'une échelle alternative semble numériquement plus plausible » ;
- « Ne pas déclarer une échelle négative invalide sans ce contrôle » (ce que le quantificateur
  autorise).

**Je n'ai fait ni l'un ni l'autre.** Je n'ai pas déplacé l'offset, et je n'ai pas déclaré l'échelle
négative invalide. La question reste ouverte.

## 4 — Non exécuté, et déclaré tel

```
A1  tête GGUF, alignement, bornes de fichier  : partiellement fait (data_start et offsets)
    GGUF_ALIGNMENT VERIFIE EXPLICITEMENT     : NON
A2  lecture independante par libggml          : NON ECRITE
A3  extraction de l operande natif            : NON FAITE
N3-c decodage ternaire confronte a un 2e decodeur : NON FAIT
N3-d blocs q8_1 alignes avec les 128 du TQ2   : NON FAIT
N3-e contribution isolee du bloc 0            : NON FAIT
N3-f ligne complete, 40 ou 48 blocs           : NON FAIT
N3-9 test F32 d indexation (heritage N2)      : NON FAIT
N3-9 couverture temporelle                    : PARTIELLE (heritage N2)
```

## 5 — Rapport final (§12)

```
ENGINE007B_F1A_STATUS=BLOCKED_OPERATOR_REFERENCE_GATE
F1A_Q0_N3_STATUS=BLOCKED_ON_INDEPENDENT_BYTE_READ_NOT_EXECUTED
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
INDEPENDENT_GGUF_ROW_BYTE_AGREEMENT=NON_MESURE
NATIVE_WEIGHT_OPERAND_BYTE_COMPARISON=NOT_EXTRACTED
NATIVE_WEIGHT_OPERAND_IS_STILL_QUANTIZED=YES_PROUVE_PAR_LA_SOURCE
TQ2_BLOCK_SIZE_PROVEN=YES_34
TQ2_SCALE_BYTE_OFFSET_PROVEN=YES_32
TQ2_SCALE_FP16_CONVERSION_PROVEN=NON_MESURE
TQ2_TERNARY_CODE_MAPPING=PASS_ACQUIS_S2BIS_NON_RECONFRONTE
TQ2_WITHIN_BLOCK_WEIGHT_ORDER=PASS_ACQUIS_S2BIS_NON_RECONFRONTE
Q8_BLOCK_ALIGNMENT_WITH_TQ2=NON_MESURE
BLOCK_0_INTEGER_DOT_VERIFIED=NO
BLOCK_0_SCALE_APPLICATION_VERIFIED=NO
FULL_ROW_RECONSTRUCTION_COMPLETED=NO
FIRST_DIVERGENCE_LAYER=NOT_PROVEN
GGUF_OFFSET_TRANSITIONS=865_OF_865_ALL_TYPES
NATIVE_WEIGHT_OPERAND_IS_DEQUANTIZED_BEFORE_MATMUL=NO
Q0_CAPTURE_GRAFT_NUMERICALLY_TRANSPARENT=YES
PRODUCT_MMVQ_SOURCE_MODIFIED=NO
MODEL_GGUF_MODIFIED=NO
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
ENGINE007_NEXT_STEP=N3_BIS_WRITE_LIBGGML_INDEPENDENT_ROW_READER
```

## 6 — Livrables réellement créés

```
N3_STATUS.txt    ce qui est prouve par la source, ce qui reste ouvert, le gate bloquant
```

**Non créés** (tous les autres fichiers du §11) : `N3_DIRECT_GGUF_READER_PROVENANCE.txt`,
`N3_LIBGGML_READER_PROVENANCE.txt`, `N3_TENSOR_ADDRESS_MANIFEST.csv`,
`N3_WEIGHT_ROW_BYTE_COMPARISON.csv`, `N3_WEIGHT_ROW_HASHES.txt`, `N3_TQ2_BLOCK_LAYOUT_PROOF.txt`,
`N3_TQ2_RAW_BLOCKS.csv`, `N3_TQ2_SCALE_DECODING.csv`, `N3_TERNARY_DECODING.csv`,
`N3_Q8_BLOCK_ALIGNMENT.csv`, `N3_Q8_QUANTIZATION_DIAGNOSTICS.csv`, `N3_BLOCK_0_INTEGER_DOT.csv`,
`N3_BLOCKWISE_CONTRIBUTIONS.csv`, `N3_NATIVE_REFERENCE_COMPARISON.csv`,
`N3_INDEPENDENT_INDEXING_TEST.txt`, `N3_TEMPORAL_COVERAGE.txt`, `N3_FIRST_DIVERGENCE.txt`,
`N3_CONFIG_PROVENANCE.txt`.

## 7 — N3-bis : la seule chose à faire, et elle est petite

```
1. ecrire un programme distinct qui charge le GGUF via libggml (no_alloc=false),
   recupere le tenseur par son nom exact, lit ne/nb/type/nbytes/data,
   et extrait les 1360 octets de la ligne 0 par ggml_backend_tensor_get ou memcpy
   depuis data + r*nb[1].
   -> chemin INDEPENDANT de mon calcul d adresse.
2. comparer octet a octet avec ma lecture directe, publier le premier octet divergent.
3. si les octets concordent : les echelles 89.3 et 3400 sont REELLES dans le fichier,
   et la divergence est ailleurs (decodage, alignement q8_1, ou reduction).
4. si les octets divergent : ma lecture est fausse, corriger l adresse.
```

Tant que cette comparaison n'existe pas, tout raisonnement sur l'arithmétique est suspendu — et je
ne le poursuis pas.

## 8 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007_PRIMARY_LEVER=EPISODE_ALIGNED_STRUCTURED_WORK_ELIMINATION
ENGINE007B_STATUS=UNBLOCKED_FOR_STRUCTURED_EPISODE_FEASIBILITY_ONLY
ENGINE007B_F1A_STATUS=BLOCKED_OPERATOR_REFERENCE_GATE
F1A_Q0_N2_STATUS=ROW_TO_OUTPUT_CORRESPONDENCE_QUALIFIED_DIVERGENCE_LOCALIZED
F1A_Q0_N3_STATUS=BLOCKED_ON_INDEPENDENT_BYTE_READ_NOT_EXECUTED
F1A_Q0_R7_STATUS=SUBMITTED_GRAPH_CAPTURE_INFRASTRUCTURE_QUALIFIED
HARNESS_DEFECT_52=RESOLVED_VERIFIED_BY_TWO_EPOCH_REGRESSION
DIAGNOSTIC_BUILD=PASS
CAPTURE_DISABLED_RUNTIME_PATH_INERT=YES
Q0_CAPTURE_ON_OFF_LOGITS_BITWISE_MATCH=YES
Q0_CAPTURE_GRAFT_NUMERICALLY_TRANSPARENT=YES
ROW_TO_OUTPUT_UNIT_CORRESPONDENCE=QUALIFIED
GGUF_OFFSETS=VERIFIED_865_OF_865_ALL_TYPES
NATIVE_WEIGHT_OPERAND_IS_STILL_QUANTIZED=YES
INDEPENDENT_GGUF_ROW_BYTE_AGREEMENT=NOT_MEASURED
FIRST_DIVERGENCE_LAYER=NOT_PROVEN
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
