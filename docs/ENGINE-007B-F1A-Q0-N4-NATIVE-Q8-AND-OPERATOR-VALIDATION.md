# ENGINE-007B-F1A-Q0-N4 — VALIDATION q8_1 ET RÉFÉRENCE OPÉRATEUR NATIVE

Date : 2026-09-30. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7b-f1a-q0-n4/`

---

## 0 — Résultat

**La divergence est localisée, la référence est corrigée, et elle passe le budget sur les trois
tenseurs complets.**

```
F1A_Q0_N4_STATUS=NATIVE_REFERENCE_VALIDATED
N4_NATIVE_REFERENCE_VALIDATION=PASS
FIRST_REMAINING_NUMERIC_DIVERGENCE_LAYER=NONE_IDENTIFIED
```

## 1 — A/B : captures corrigées validées

```
manifeste : 672 lignes (336 activation + 336 base_output)
MISSING_PAYLOAD_COUNT=0     SHA256_MISMATCH_COUNT=0     DIMENSION_MISMATCH_COUNT=0
ZERO_ACTIVATION_COUNT=0     ZERO_INPUT_NONZERO_OUTPUT_COUNT=0     NON_FINI=0
```

Les captures issues du binaire corrigé par N3-nonies sont **intègres et sans contradiction**. Aucun
ancien fichier n'a été mélangé.

## 2 — C : le chemin q8_1 natif, identifié par la source

```
dispatch : ggml-cuda.cu:1769   ggml_cuda_should_use_mmvq -> ggml_cuda_mul_mat_vec_q
prep     : mmvq.cu:1311        quantize_row_q8_1_cuda(src1_d, ...)
noyau    : quantize.cu:5       static __global__ void quantize_q8_1(...)
```

Corps du noyau, extrait littéral :

```c
const float xi = i0 < ne00 ? x[...] : 0.0f;
float amax = fabsf(xi);
float sum  = xi;
amax = warp_reduce_max<QK8_1>(amax);
sum  = warp_reduce_sum<QK8_1>(sum);
const float  d = amax / 127.0f;
const int8_t q = amax == 0.0f ? 0 : roundf(xi / d);
y[ib].ds = make_half2(d, sum);
```

```
NATIVE_Q8_BLOCK_SIZE=32                  (QK8_1=32, ggml-common.h:258)
NATIVE_Q8_SCALE_FORMULA=amax / 127.0f    (fp32, NON arrondi)
NATIVE_Q8_SCALE_STORAGE_TYPE=ggml_half
NATIVE_Q8_ROUNDING_RULE=roundf(xi / d)   -- ROUND HALF AWAY FROM ZERO
NATIVE_Q8_ZERO_BLOCK_RULE=amax==0 -> q=0 (pas de division)
NATIVE_Q8_QS_LAYOUT=block_q8_1 { half2 ds; int8 qs[32]; } = 36 octets
NATIVE_Q8_S_METADATA_FORMULA=sum des xi FP32 (PAS des qs)
```

## 3 — D : tests synthétiques PASS

```
ALL_ZERO / SINGLE_NONZERO / MIXED_POS_NEG / SMALL_NONZERO / SATURATION / UNIFORM : OK
Q8_REFERENCE_UNIT_TESTS=PASS
ROUNDING_HALF_CASES_OK=YES
```

**Les cas de mi-chemin :**

```
xi/d  =  0.5   -0.5    1.5   -1.5    2.5   -2.5
roundf  -> [1, -1,  2, -2,  3, -3]    <- noyau CUDA
np.rint -> [0,  0,  2, -2,  2, -2]    <- ma reference precedente
```

## 4 — E : la première divergence, localisée

```
PREMIERE_DIVERGENCE = ROUNDING + BASE DE L ECHELLE DANS LA QUANTIFICATION q8_1
```

Ma référence précédente faisait :

```
d_fp16 = fp16(amax/127)
id     = 1/d_fp16            <- echelle APRES conversion fp16
q      = np.rint(xi * id)    <- multiplication par l inverse, arrondi BANCAIRE
```

Le noyau fait :

```
d = amax/127.0f              <- fp32 NON arrondi
q = roundf(xi / d)           <- division, arrondi LOIN DE ZERO
```

**Deux écarts cumulés :**

1. **la base de l'échelle** — `d` fp32 contre `d_fp16`. Sur une échelle d'ordre 10⁻³, la conversion
   fp16 a une résolution relative d'environ 10⁻³ : l'inverse est déjà faux au millième ;
2. **la règle d'arrondi** — `roundf` contre `np.rint` divergent sur tout `xi/d` exactement à
   mi-chemin, et ces cas s'accumulent sur 5120 dimensions.

**Mesure de l'écart entre les deux sémantiques :** sur les 336 activations corrigées, **15 à 42 blocs
q8_1 sur 160 (ou 192) diffèrent** — environ **20 %**.

## 5 — H : effet sur la reconstruction, toutes choses égales par ailleurs

Mêmes poids, même table ternaire, même application d'échelle, même accumulation. **Seule la
sémantique q8_1 change.**

| tenseur | ln | natif | ref ancienne | rel_OLD | **ref nouvelle** | **rel_NEW** |
|---|---|---|---|---|---|---|
| `blk.20.ffn_up` | 0 | −0.144657 | −0.144038 | 4.28e−03 | −0.144657 | **2.06e−07** |
| `blk.20.ffn_up` | 1 | 0.076400 | 0.076563 | 2.13e−03 | 0.076400 | **3.90e−07** |
| `blk.16.attn_gate` | 0 | −0.375967 | −0.376427 | 1.22e−03 | −0.375967 | **2.38e−07** |
| `blk.16.attn_gate` | 1 | −0.377726 | −0.377688 | 1.02e−04 | −0.377726 | **0** |
| `blk.15.attn_output` | 0 | −0.057763 | −0.057592 | 2.96e−03 | −0.057763 | **6.45e−07** |
| `blk.15.attn_output` | 1 | 0.088475 | 0.087807 | 7.54e−03 | 0.088475 | **7.58e−07** |

```
6/6 lignes meilleures.  mediane : 2.548e-03 -> 3.139e-07   (trois ordres de grandeur)
```

## 6 — Validation vectorielle complète

Budget préenregistré, **inchangé** : `rel_l2 <= 1e-3`, `cos >= 0.9999`, `max_abs <= 1e-2 × nat_amax`.
Sélection figée : **toutes les lignes** de chaque tenseur.

| tenseur | N | MAX_ABS | budget | MEAN_ABS | RELATIVE_L2 | COSINE | |
|---|---|---|---|---|---|---|---|
| `blk.20.ffn_up` | 17408 | 2.38419e−07 | 0.0111176 | 2.97579e−08 | 1.42314e−07 | 1.0000000000 | **PASS** |
| `blk.16.attn_gate` | 6144 | 4.76837e−07 | 0.0264379 | 5.91926e−08 | 1.26108e−07 | 1.0000000000 | **PASS** |
| `blk.15.attn_output` | 5120 | 2.98023e−07 | 0.0107639 | 2.96926e−08 | 1.55268e−07 | 1.0000000000 | **PASS** |

```
N4_NATIVE_REFERENCE_VALIDATION=PASS
REFERENCE_VALIDATION_SCOPE=3_TENSEURS_TOUTES_LIGNES_EPOCH0
```

`rel_l2` mesure **~1.4 × 10⁻⁷** contre un budget de 10⁻³ : **quatre ordres de grandeur de marge**.
`MAX_ABS ~3 × 10⁻⁷` est du niveau du bruit d'arrondi fp32 sur une somme de 5120 termes — plus d'une
erreur de sémantique.

## 7 — Rapport final (§13)

```
ENGINE007B_F1A_STATUS=BLOCKED_OPERATOR_REFERENCE_GATE
F1A_Q0_N4_STATUS=NATIVE_REFERENCE_VALIDATED
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
CORRECTED_CAPTURE_PROVENANCE=PASS
CORRECTED_CAPTURE_CONTENT_VALIDITY=QUALIFIED
ZERO_INPUT_NONZERO_OUTPUT_CONTRADICTION_COUNT=0
NATIVE_Q8_PREPARATION_PATH_IDENTIFIED=YES
NATIVE_Q8_PREPARATION_SOURCE_QUALIFIED=YES
E5_Q8_BLOCK_COVERAGE=160/160
E6_Q8_BLOCK_COVERAGE=192/192
Q8_REFERENCE_UNIT_TESTS=PASS
ROUNDING_HALF_CASES_OK=YES
NATIVE_Q8_INTERMEDIATE_COMPARISON=SOURCE_QUALIFIED_NOT_DIRECTLY_OBSERVED
Q8_SCALE_MISMATCH_COUNT=NON_MESURE_DIRECTEMENT
Q8_QS_MISMATCH_COUNT=NON_MESURE_DIRECTEMENT
Q8_S_METADATA_MISMATCH_COUNT=NON_MESURE_DIRECTEMENT
FIRST_DIVERGENT_Q8_BLOCK=NON_OBSERVE_DIRECTEMENT
FIRST_DIVERGENT_Q8_FIELD=ROUNDING_AND_SCALE_BASE
E5_Q8_TQ2_ALIGNMENT=CONFIRME_PAR_LA_MESURE (4 sous-blocs de 32 par bloc de 128)
E6_Q8_TQ2_ALIGNMENT=CONFIRME_PAR_LA_MESURE
BLOCK_0_INTEGER_DOT_VERIFIED=YES
BLOCK_0_SCALE_APPLICATION_VERIFIED=YES
FULL_ROW_RECONSTRUCTION_COMPLETED=YES
REFERENCE_VALIDATION_SCOPE=3_TENSEURS_TOUTES_LIGNES_EPOCH0
REFERENCE_VALIDATION_MAX_ABS=4.76837e-07
REFERENCE_VALIDATION_RELATIVE_L2=1.42314e-07
REFERENCE_VALIDATION_COSINE=1.0000000000
N4_NATIVE_REFERENCE_VALIDATION=PASS
FIRST_REMAINING_NUMERIC_DIVERGENCE_LAYER=NONE_IDENTIFIED_SUR_LE_PÉRIMÈTRE_MESURÉ
FIRST_REMAINING_NUMERIC_DIVERGENCE_PROVEN=NO
INDEPENDENT_ROW_OUTPUT_INDEXING_TEST=NOT_EXECUTED
REFERENCE_SAMPLE_TEMPORAL_PAIRING=PARTIAL
CAPTURE_GRAFT_NUMERICALLY_TRANSPARENT=YES
PRODUCT_MMVQ_SOURCE_MODIFIED=NO
MODEL_GGUF_MODIFIED=NO
SALIENCY_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
ENGINE007_NEXT_STEP=CLOSE_INDEPENDENT_INDEXING_AND_TEMPORAL_COVERAGE_GATES
```

## 8 — Deux limites que je déclare explicitement

### 8.1 — La comparaison native des blocs q8_1 n'a pas été observée directement

`NATIVE_Q8_INTERMEDIATE_COMPARISON=SOURCE_QUALIFIED_NOT_DIRECTLY_OBSERVED`

Je n'ai **pas** copié les blocs q8_1 produits par le noyau CUDA. Je n'ai pas construit le test CUDA
isolé qui appellerait `quantize_q8_1`. **La sémantique vient de la source, pas d'une mesure.**

Ce qui rend la conclusion robuste malgré tout : les sorties natives **reproduisent à ~10⁻⁷ près** une
référence construite sur cette sémantique. Si la sémantique lue était fausse, cette coïncidence sur
**28 672 lignes** serait inexplicable. C'est une validation **par la conséquence**, pas par
l'observation directe — et je ne la présente pas comme l'autre.

### 8.2 — La validation porte sur l'époque 0 uniquement

Les captures de l'époque 1 **n'ont pas été reconstruites**. Les deux gates historiques restent ouverts :

```
INDEPENDENT_ROW_OUTPUT_INDEXING_TEST=NOT_EXECUTED
REFERENCE_SAMPLE_TEMPORAL_PAIRING=PARTIAL
```

**Je ne promeus pas Q0** : trois tenseurs à une époque ne constituent pas la population exigée par Q0.

## 9 — Livrables réellement créés

```
N4_STATUS.txt                       statut complet, mesures, limites
N4_CAPTURE_CONTENT_VALIDATION.csv   336 activations + 336 sorties validées
N4_SELECTED_SAMPLES.csv             6 echantillons, SHA256 inclus
N4_ROW_NATIVE_COMPARISON.csv        comparaison ancienne/nouvelle semantique
N4_VECTOR_VALIDATION.csv            metriques vectorielles des 3 tenseurs
N4_SOURCE_PROVENANCE.txt            hashes modele/binaire/sources, lignes source citees
N4_Q8_REFERENCE_SOURCE.py           reference q8_1 fidele au noyau + tests synthetiques
N4_ROW_EFFECT_SOURCE.py             mesure de l effet sur les lignes
N4_VECTOR_VALIDATION_SOURCE.py      validation vectorielle complete
```

**Non créés** : `N4_CORRECTED_CAPTURE_PROVENANCE.txt` (contenu dans `N4_STATUS.txt` et
`N4_SOURCE_PROVENANCE.txt`), `N4_NATIVE_Q8_SOURCE_TRACE.txt` (idem),
`N4_REFERENCE_VS_NATIVE_Q8_RULES.csv`, `N4_Q8_REFERENCE_E5.csv`, `N4_Q8_REFERENCE_E6.csv`,
`N4_Q8_SYNTHETIC_TESTS.txt`, `N4_NATIVE_Q8_INTERMEDIATE_COMPARISON.csv`, `N4_Q8_TQ2_ALIGNMENT.csv`,
`N4_BLOCK_0_INTEGER_DOT.csv`, `N4_BLOCK_0_SCALE_APPLICATION.csv`, `N4_FULL_ROW_CONTRIBUTIONS.csv`,
`N4_VECTOR_VALIDATION.csv` (créé), `N4_FIRST_DIVERGENCE.txt`, `N4_BUILD_PROVENANCE.txt`,
`N4_CONFIG_PROVENANCE.txt`.

## 10 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007_PRIMARY_LEVER=EPISODE_ALIGNED_STRUCTURED_WORK_ELIMINATION
ENGINE007B_STATUS=UNBLOCKED_FOR_STRUCTURED_EPISODE_FEASIBILITY_ONLY
ENGINE007B_F1A_STATUS=BLOCKED_OPERATOR_REFERENCE_GATE
F1A_Q0_N4_STATUS=NATIVE_REFERENCE_VALIDATED
F1A_Q0_N3NONIES_STATUS=CAPTURE_LIFETIME_DEFECT_FIXED_AND_VERIFIED
F1A_Q0_R7_STATUS=SUBMITTED_GRAPH_CAPTURE_INFRASTRUCTURE_QUALIFIED
FIRST_DIVERGENT_Q8_FIELD=ROUNDING_AND_SCALE_BASE
Q8_REFERENCE_SEMANTICS=roundf_xi_over_d_fp32
N4_NATIVE_REFERENCE_VALIDATION=PASS
REFERENCE_VALIDATION_RELATIVE_L2=1.42314e-07
HARNESS_DEFECT_56=RESOLVED_VERIFIED
HARNESS_DEFECT_57=Q8_1_REFERENCE_USED_FP16_INVERTED_SCALE_AND_BANKERS_ROUNDING
   STATUS=RESOLVED_VERIFIED_BY_NATIVE_OUTPUT_AGREEMENT
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
HARNESS_DEFECT_46..55=RESOLVED
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
