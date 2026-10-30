# ENGINE-007B-F1A-Q0-N3sexies — QUALIFICATION DES ACTIVATIONS q8_1

Date : 2026-09-30. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7b-f1a-q0-n3sexies/`

---

## 0 — Résultat

**Première divergence démontrée. Elle est au niveau de l'activation brute, pas de q8_1.**

```
F1A_Q0_N3SEXIES_STATUS=DIVERGENCE_PROVEN_AT_RAW_ACTIVATION_EPOCH0
FIRST_DIVERGENCE_LAYER=RAW_ACTIVATION_CAPTURE_EPOCH0
FIRST_DIVERGENCE_LAYER_PROVEN=YES
```

## 1 — Les activations brutes (§2)

Lecture par les types et métadonnées réels du manifeste : `dtype=0` (F32), `ne=[K,1]`, `nb0=4`,
`contiguous=1`, taille d'octets = 4 × K. **Le type est lu dans le manifeste, pas déduit de la taille.**

```
blk.20.ffn_up.weight        K=5120 : amax=11.04527  non nulles=5120  nulles=0    fini=OUI
blk.15.attn_output.weight   K=6144 : amax= 0.00000  non nulles=0     nulles=6144 fini=OUI

tranches de 128 : E5  40 tranches, non nulles min=128 max=128, entierement nulles=0
                  E6  48 tranches, TOUTES entierement nulles
```

```
RAW_ACTIVATION_SERIALIZATION_VERIFIED=YES
RAW_ACTIVATION_FINITE=YES
```

## 2 — Le fait décisif : l'époque 0 est nulle, l'époque 1 est saine

Pour la **même cible** `blk.15.attn_output.weight`, lue par les chemins déclarés du manifeste :

| époque | target | kind | valeurs | amax | non nulles |
|---|---|---|---|---|---|
| **ep0** | 1202 | activation | 6144 | **0.00000** | **0** |
| ep0 | 1202 | base_output | 5120 | 11.04527 | 5120 |
| **ep1** | 26 | activation | 6144 | **0.02709** | **6144** |
| ep1 | 26 | base_output | 5120 | 11.71043 | 5120 |

**L'activation de l'époque 0 est entièrement nulle ; celle de l'époque 1 est saine.** Dans les deux
cas la sortie native du matmul est non nulle.

## 3 — La contradiction arithmétique

Un matmul de base, sans terme constant, avec une activation **toute nulle**, **doit** produire une
sortie toute nulle. Or la sortie native de la même cible est non nulle (`amax = 11.04527`).

```
C EST UN CONTRE-EXEMPLE.
```

Votre §2 demande alors de réexaminer le domaine, l'identité, la durée de vie ou la lecture des
captures — plutôt que de modifier le quantificateur. C'est ce que je fais.

## 4 — Ce que cela rétracte

La reconstruction E6 nulle que j'ai mesurée en N3quater et N3quinquies portait sur **l'époque 0**,
donc sur une activation nulle. **Ma valeur de référence nulle était donc correcte** : somme de
produits dont un opérande est nul.

```
LE ZERO E6 N EST PAS UN DEFAUT DE MA REFERENCE NI DU DECODAGE TERNAIRE.
C EST UNE PROPRIETE DE LA CAPTURE D EPOQUE 0.
```

C'est la **troisième rétractation** de cette série, et elle invalide l'axe principal de deux passes
(N3quater, N3quinquies). Chaque fois, la cause était une donnée que je n'avais pas vérifiée avant de
bâtir un raisonnement dessus.

## 5 — Piste prioritaire pour la suite

L'époque 0 est **aussi** celle qui contenait les 1176 cibles bufferless (établi en R3). Ses 168
cibles résidantes pourraient avoir été extraites **avant que le producteur du tenseur d'activation
n'ait écrit ses valeurs** — buffer alloué, contenu pas encore produit.

Cela réconcilierait deux faits que j'avais traités séparément :

- R3 : « les cibles de l'époque 0 sont résidentes puisqu'elles ont un buffer » ;
- N3sexies : « leur contenu est nul ».

**Un buffer résident n'est pas un buffer rempli.** C'est la même distinction que R1 m'avait apprise
entre existence du tenseur et résidence du buffer, poussée d'un cran plus loin.

## 6 — Ce que N3sexies n'a pas fait

```
Q8 audit (160 blocs E5 / 192 blocs E6)                     NON EXECUTE
tests synthetiques q8_1                                    NON EXECUTES
alignement q8_1 / TQ2                                       NON MESURE
produits entiers E6                                         NON CALCULES
accumulation E6                                             NON TRACEE
```

Votre §2 dit : « Si E6 est entièrement nulle, ne pas conclure immédiatement à un défaut de q8_1.
Comparer alors cette observation à la sortie native de la même cible. » C'est ce que j'ai fait, et
cela a **arrêté la descente** avant q8_1 — correctement, puisque la contradiction est en amont.

## 7 — Rapport final (§13)

```
ENGINE007B_F1A_STATUS=BLOCKED_ACTIVATION_CAPTURE_GATE
F1A_Q0_N3SEXIES_STATUS=DIVERGENCE_PROVEN_AT_RAW_ACTIVATION_EPOCH0
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
CURRENT_FORK_ENCODER_DECODER_CONSISTENCY=PASS
ACTUAL_GGUF_ENCODER_PROVENANCE=UNKNOWN
E5_SAMPLE_PAIRING=PASS
E6_SAMPLE_PAIRING=PASS
RAW_ACTIVATION_SERIALIZATION_VERIFIED=YES
RAW_ACTIVATION_FINITE=YES
E5_RAW_NONZERO_COUNT=5120
E6_RAW_NONZERO_COUNT=0_POUR_EPOCH0__6144_POUR_EPOCH1
RAW_ACTIVATION_ALL_ZERO_EPOCH0=YES
RAW_ACTIVATION_ALL_ZERO_EPOCH1=NO
E6_NATIVE_OUTPUT_NONZERO_WHILE_ACTIVATION_ZERO=YES_CONTRADICTION
E5_Q8_BLOCKS_PROCESSED=NON_MESURE
E6_Q8_BLOCKS_PROCESSED=NON_MESURE
E5_Q8_NONZERO_BLOCK_COUNT=NON_MESURE
E6_Q8_NONZERO_BLOCK_COUNT=NON_MESURE
Q8_REFERENCE_UNIT_TESTS=PASS_ACQUIS_NON_REJoue
Q8_SYNTHETIC_NONZERO_CONTROL=NON_EXECUTE
Q8_BLOCK_ALIGNMENT_WITH_TQ2=NON_MESURE
E6_NONZERO_INTEGER_DOT_COUNT=NON_MESURE
E6_NONZERO_FP32_CONTRIBUTION_COUNT=NON_MESURE
E6_SUM_ABS_FP32_CONTRIBUTIONS=NON_MESURE
E6_FINAL_ACCUMULATOR=0.0_POUR_EPOCH0
E6_RETURNED_REFERENCE_VALUE=0.0_POUR_EPOCH0
E6_NATIVE_MM_CAPTURE_VALUE=amax_11.04527
BLOCK_0_INTEGER_DOT_VERIFIED=NO
BLOCK_0_SCALE_APPLICATION_VERIFIED=NO
FIRST_DIVERGENCE_LAYER=RAW_ACTIVATION_CAPTURE_EPOCH0
FIRST_DIVERGENCE_LAYER_PROVEN=YES
N3SEXIES_NATIVE_REFERENCE_VALIDATION=FAIL
EPOCH0_RESIDENT_ACTIVATION_CONTENT=ZERO
EPOCH1_RESIDENT_ACTIVATION_CONTENT=VALID
INDEPENDENT_ROW_OUTPUT_INDEXING_TEST=NOT_EXECUTED
REFERENCE_SAMPLE_TEMPORAL_PAIRING=PARTIAL
Q0_CAPTURE_GRAFT_NUMERICALLY_TRANSPARENT=YES
PRODUCT_MMVQ_SOURCE_MODIFIED=NO
MODEL_GGUF_MODIFIED=NO
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
ENGINE007_NEXT_STEP=TEST_WHETHER_EPOCH0_RESIDENT_ACTIVATIONS_ARE_SYSTEMATICALLY_ZERO
```

## 8 — Livrables réellement créés

```
N3SEXIES_STATUS.txt   activations brutes E5/E6, contradiction, divergence prouvee, pistes
```

**Non créés** (étapes non exécutées) : `N3SEXIES_SOURCE_Q8_PATH.txt`, `N3SEXIES_SAMPLE_PAIRING.csv`,
`N3SEXIES_RAW_ACTIVATION_E5.csv`, `N3SEXIES_RAW_ACTIVATION_E6.csv`, `N3SEXIES_Q8_BLOCKS_E5.csv`,
`N3SEXIES_Q8_BLOCKS_E6.csv`, `N3SEXIES_Q8_SYNTHETIC_TESTS.txt`, `N3SEXIES_Q8_TQ2_ALIGNMENT.csv`,
`N3SEXIES_E6_INTEGER_DOTS.csv`, `N3SEXIES_E6_BLOCK_CONTRIBUTIONS.csv`,
`N3SEXIES_E6_ACCUMULATOR_TRACE.txt`, `N3SEXIES_E5_CONTROL.csv`, `N3SEXIES_FIRST_DIVERGENCE.txt`,
`N3SEXIES_REFERENCE_PATCH.txt`, `N3SEXIES_REFERENCE_PROVENANCE.txt`,
`N3SEXIES_CONFIG_PROVENANCE.txt`.

## 9 — La prochaine étape, et elle est étroite

```
1. pour TOUTES les cibles de l epoque 0, compter les valeurs non nulles de l activation
   -> l epoque 0 est-elle SYSTEMATIQUEMENT nulle en activation ?
2. comparer avec l epoque 1 pour les MÊMES cibles (memes noms de poids)
   -> si ep0 nulle / ep1 saine partout : le defaut est propre a l epoque 0
3. si oui : l extraction de l epoque 0 lit des buffers alloues mais pas encore remplis.
   -> corrige : n extraire que les epoques ou le producteur a effectivement ecrit,
      c est-a-dire celles dont les soumissions sont confirmees ET dont le contenu
      n est pas nul.
4. alors seulement : relancer la reconstruction sur l epoque 1 et reprendre q8_1.
```

**Le point 3 est un défaut de mon instrumentation, pas du modèle.** Je ne l'ai pas encore prouvé —
je l'ai identifié comme piste prioritaire.

## 10 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007_PRIMARY_LEVER=EPISODE_ALIGNED_STRUCTURED_WORK_ELIMINATION
ENGINE007B_STATUS=UNBLOCKED_FOR_STRUCTURED_EPISODE_FEASIBILITY_ONLY
ENGINE007B_F1A_STATUS=BLOCKED_ACTIVATION_CAPTURE_GATE
F1A_Q0_N3SEXIES_STATUS=DIVERGENCE_PROVEN_AT_RAW_ACTIVATION_EPOCH0
F1A_Q0_N3QUINQUIES_STATUS=BLOCKED_Q8_ACTIVATION_QUANTIZATION_GATE
F1A_Q0_N3QUATER_STATUS=BLOCKED_TQ2_TERNARY_DECODE_GATE  ACCUSATION_RETIREE
F1A_Q0_R7_STATUS=SUBMITTED_GRAPH_CAPTURE_INFRASTRUCTURE_QUALIFIED
FIRST_DIVERGENCE_LAYER=RAW_ACTIVATION_CAPTURE_EPOCH0
FIRST_DIVERGENCE_LAYER_PROVEN=YES
TERNARY_TABLE_ACCUSATION=RETRACTED
E6_ZERO_REFERENCE=EXPLIQUE_PAR_UNE_ACTIVATION_NULLE_EPOCH0
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
