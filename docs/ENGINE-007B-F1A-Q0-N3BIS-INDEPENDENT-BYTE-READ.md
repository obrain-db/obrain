# ENGINE-007B-F1A-Q0-N3bis — LECTURE INDÉPENDANTE DES OCTETS PAR LIBGGML

Date : 2026-09-30. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7b-f1a-q0-n3bis/`

---

## 0 — Résultat

**Le gate a fait exactement son travail : il a innocenté libggml et désigné MON lecteur.**

```
INDEPENDENT_GGUF_ROW_BYTE_AGREEMENT=NO
N3BIS_STATUS=BLOCKED_WEIGHT_BYTE_IDENTITY_GATE
DIRECT_READER_IS_WRONG_BY_28_BYTES=YES
LIBGGML_READER_ROW0_BYTES=QUALIFIED
```

## 1 — Le lecteur indépendant fonctionne (§2, cas A/B/C)

```
LIBGGML_LOAD_STATUS=OK
TENSOR_NAME=blk.20.ffn_up.weight
TENSOR_TYPE=tq2_g128 (43)
NE0=5120  NE1=17408  NE2=1  NE3=1
NB0=34    NB1=1360   NB2=23674880  NB3=23674880
GGML_NBYTES=23674880
ROW_BYTES=1360
DATA_POINTER_NON_NULL=YES
BLOCKS_PER_ROW=40
EXPECTED_ROW_BYTES_FROM_METADATA=1360
READ_PATH=tensor_data_plus_row_stride
```

Une erreur de compilation corrigée : `p.ctx` est un `ggml_context**` (paramètre de sortie), pas un
`ggml_context*`.

## 2 — La comparaison, et ce qu'elle révèle vraiment

```
DIRECT_ROW_BYTE_COUNT=1360      LIBGGML_ROW_BYTE_COUNT=1360
DIRECT_ROW_SHA256  = a9358acc6fe1c6eb6e0a1c4532ea7b55b71e87c23bac3a9a6aeccd286f24a93b
LIBGGML_ROW_SHA256 = d9a81d7f4929b93f8321c0870d7fff7a29386f4c0528aaf94c171ab91f00bc5e

COMPARED_BYTE_COUNT=1360
BYTE_MISMATCH_COUNT=1334
FIRST_DIFFERING_BYTE_OFFSET=0
DIRECT_BYTE=0x4a    LIBGGML_BYTE=0x2a
```

**1334 octets sur 1360 diffèrent — mais ce ne sont pas des données différentes.**

```
les 32 premiers octets de LIBGGML apparaissent dans DIRECT a l offset 28
les 32 premiers octets de DIRECT n apparaissent NULLE PART dans LIBGGML
MEILLEUR DECALAGE DIRECT->LIBGGML : offset = -28   concordance = 1.0000
```

**Concordance de 100,00 % sur les 1332 octets recouvrants.** Les deux lecteurs lisent **le même
flux, décalé de 28 octets**.

Le `BYTE_MISMATCH_COUNT=1334` est une mesure correcte (comparaison à offset égal) mais une
**conclusion trompeuse** : sans l'analyse de corrélation, j'aurais pu annoncer « les deux lecteurs
ne concordent pas » et chercher une corruption inexistante.

## 3 — Lequel est au bon endroit : verdict par la physique des données

```
DIRECT  : echelles des 40 blocs : min=-0.02473  max=3400.00000  moyenne=402.19968
LIBGGML : echelles des 40 blocs : min= 0.01036  max=   0.01524  moyenne=  0.01225
```

Les échelles libggml sont **serrées, positives, d'ordre 10⁻²** — exactement ce qu'on attend d'une
échelle de poids ternaires dont la sortie native est d'ordre **3.8**.

Les échelles de mon lecteur direct sont **négatives et jusqu'à 3400** — incompatibles avec des
sorties d'ordre 1 à 4.

```
C EST MON LECTEUR DIRECT QUI EST DECALE DE 28 OCTETS.
LIBGGML_READER_ROW0_BYTES=QUALIFIED
DIRECT_READER_IS_WRONG_BY_28_BYTES=YES
```

## 4 — Conséquence sur le diagnostic N2 : une observation est rétractée

L'observation de N2 :

```
ligne 0 de blk.20.ffn_up : echelles min=-0.0247 max=3400.0 moyenne=402.2 premiere=89.3125
```

**était un artefact de mon décalage de 28 octets. Elle ne reflète pas le contenu du GGUF.**

Votre §3 de N3 demandait de ne pas corriger un offset parce qu'une échelle alternative semble plus
plausible, ni de déclarer une échelle négative invalide sans vérifier. J'ai respecté les deux
consignes — et la vérification indépendante a montré que **l'échelle n'était ni invalide ni à
corriger : c'est mon adresse qui était fausse.**

```
TQ2_SCALE_FP16_CONVERSION=EN_COURS_DE_QUALIFICATION_SUR_OCTETS_CORRECTS
   l echelle du bloc 0 lue par libggml vaut 0.01232 -- plausible, non encore qualifiee.
```

## 5 — Ce qui n'est PAS qualifié par cette passe (§5 du brief)

```
DIRECT_GGUF_READER_ROW0_BYTES=QUALIFIED      (par le lecteur libggml)
TQ2_SCALE_FP16_CONVERSION                    NON QUALIFIEE
TQ2_TERNARY_DECODING                         NON QUALIFIE
Q8_1_BLOCK_ALIGNMENT                         NON QUALIFIE
NATIVE_GPU_OPERAND_BYTES                     NON QUALIFIE
NATIVE_OPERATOR_REFERENCE                    NON QUALIFIE
INDEPENDENT_ROW_OUTPUT_INDEXING_TEST         NOT_EXECUTED
REFERENCE_SAMPLE_TEMPORAL_PAIRING            PARTIAL
```

## 6 — Un point que je n'ai pas résolu, et que je signale

J'ai cherché l'origine exacte du décalage de 28 octets en relisant plusieurs tenseurs par les deux
chemins, mais ma recherche par balayage n'a pas trouvé les lignes dans la fenêtre explorée
(3 Mo depuis le début du fichier), et je n'ai donc **pas** caractérisé la règle. Je ne peux pas
encore dire si 28 est **constant** ou **variable selon le tenseur**.

Ce que je sais : `data_start` et les offsets relatifs de ma table produisent une adresse décalée.
Le fait que les 865 transitions d'offsets soient cohérentes signifie que **la table est interne
cohérente** — mais cela ne garantit pas que `data_start` soit la bonne origine absolue.

## 7 — Rapport final (§8)

```
ENGINE007B_F1A_STATUS=BLOCKED_OPERATOR_REFERENCE_GATE
F1A_Q0_N3BIS_STATUS=BLOCKED_WEIGHT_BYTE_IDENTITY_GATE
INDEPENDENT_LIBGGML_READER_BUILD=PASS
INDEPENDENT_LIBGGML_READER_EXECUTION=PASS
MODEL_SHA256=1d7c9409ae7f2c7f6071c8ad32d2481f2c09da94fc2aec3f63a4d44777e2b937
TENSOR_NAME=blk.20.ffn_up.weight
TENSOR_TYPE=tq2_g128_43
TENSOR_NE=[5120,17408,1,1]
TENSOR_NB=[34,1360,23674880,23674880]
DIRECT_ROW_BYTE_COUNT=1360
LIBGGML_ROW_BYTE_COUNT=1360
DIRECT_ROW_SHA256=a9358acc6fe1c6eb6e0a1c4532ea7b55b71e87c23bac3a9a6aeccd286f24a93b
LIBGGML_ROW_SHA256=d9a81d7f4929b93f8321c0870d7fff7a29386f4c0528aaf94c171ab91f00bc5e
BYTE_MISMATCH_COUNT=1334
FIRST_DIFFERING_BYTE_OFFSET=0
BEST_ALIGNMENT_OFFSET=-28
BEST_ALIGNMENT_AGREEMENT=1.0000
INDEPENDENT_GGUF_ROW_BYTE_AGREEMENT=NO
DIRECT_READER_IS_WRONG_BY_28_BYTES=YES
LIBGGML_READER_ROW0_BYTES=QUALIFIED
DIRECT_SCALES=ABSURDES_min_-0.02473_max_3400.0
LIBGGML_SCALES=PLAUSIBLES_min_0.01036_max_0.01524
N2_SCALE_OBSERVATION_STATUS=RETRACTED_ARTEFACT_OF_ADDRESS_OFFSET
FIRST_DIVERGENCE_LAYER=DIRECT_GGUF_ADDRESS_COMPUTATION
INDEPENDENT_ROW_OUTPUT_INDEXING_TEST=NOT_EXECUTED
REFERENCE_SAMPLE_TEMPORAL_PAIRING=PARTIAL
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
SALIENCY_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
PRODUCT_MMVQ_SOURCE_MODIFIED=NO
MODEL_GGUF_MODIFIED=NO
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
ENGINE007_NEXT_STEP=N3TER_CHARACTERIZE_THEN_FIX_DIRECT_ADDRESSING_AND_REQUALIFY
```

## 8 — Livrables réellement créés

```
blk20_ffn_up_row0_libggml.bin    1360 octets, lus par libggml depuis t->data
blk20_ffn_up_row0_direct.bin     1360 octets, lus par mon chemin d adressage
LIBGGML_READER_SOURCE.txt        le source du lecteur independant
LIBGGML_READER_BUILD.txt         hashes source/binaire, API utilisee, resultat du build
TENSOR_METADATA.txt              ne, nb, nbytes, pointeur data, chemin de lecture
ROW0_FIRST_BLOCK_HEX.txt         qs[0..31] et d[32..33] des deux lectures
ROW0_BYTE_COMPARISON.txt         le verdict et son raisonnement
```

**Non créés** : `ROW0_SHA256.txt` (les deux SHA256 sont publiés dans
`ROW0_BYTE_COMPARISON.txt`), `COMPARISON_NEGATIVE_CONTROL.txt` (le contrôle négatif a été exécuté
et a détecté la copie altérée, avec le fichier de référence intact — résultat consigné dans le
journal, pas dans un fichier séparé), `N3BIS_STATUS.txt` (le statut est dans
`ROW0_BYTE_COMPARISON.txt` et dans ce rapport).

## 9 — La prochaine étape, précise

```
N3ter-a  caracteriser le decalage : lire la ligne 0 de PLUSIEURS tenseurs par les deux
         chemins, et voir si 28 est constant ou depend du tenseur.
         -> si constant : c est data_start qui est faux de 28.
         -> si variable : la regle d alignement par tenseur n est pas celle que j applique.
N3ter-b  corriger UNIQUEMENT le calcul d adresse du lecteur direct, puis requalifier.
N3ter-c  alors seulement : decoder l echelle fp16 sur les octets CORRECTS,
         puis le ternaire, puis les blocs q8_1, puis le bloc 0, puis la ligne complete.
```

**Le fait que libggml donne des échelles physiquement plausibles est une bonne nouvelle** : la
chaîne de référence n'était pas fausse par nature, elle travaillait sur les mauvais octets.

## 10 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007_PRIMARY_LEVER=EPISODE_ALIGNED_STRUCTURED_WORK_ELIMINATION
ENGINE007B_STATUS=UNBLOCKED_FOR_STRUCTURED_EPISODE_FEASIBILITY_ONLY
ENGINE007B_F1A_STATUS=BLOCKED_OPERATOR_REFERENCE_GATE
F1A_Q0_N3BIS_STATUS=BLOCKED_WEIGHT_BYTE_IDENTITY_GATE
F1A_Q0_N3_STATUS=BLOCKED_ON_INDEPENDENT_BYTE_READ_NOT_EXECUTED
F1A_Q0_N2_STATUS=ROW_TO_OUTPUT_CORRESPONDENCE_QUALIFIED_DIVERGENCE_LOCALIZED
F1A_Q0_R7_STATUS=SUBMITTED_GRAPH_CAPTURE_INFRASTRUCTURE_QUALIFIED
INDEPENDENT_GGUF_ROW_BYTE_AGREEMENT=NO
DIRECT_READER_IS_WRONG_BY_28_BYTES=YES
LIBGGML_READER_ROW0_BYTES=QUALIFIED
N2_SCALE_OBSERVATION=RETRACTED_ARTEFACT
FIRST_DIVERGENCE_LAYER=DIRECT_GGUF_ADDRESS_COMPUTATION
HARNESS_DEFECT_53=DIRECT_GGUF_READER_ADDRESS_OFFSET_28_BYTES  OPEN
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
