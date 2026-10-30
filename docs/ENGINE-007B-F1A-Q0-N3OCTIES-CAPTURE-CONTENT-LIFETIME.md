# ENGINE-007B-F1A-Q0-N3octies — DURÉE DE VIE DU CONTENU DES CAPTURES

Date : 2026-09-30. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7b-f1a-q0-n3octies/`

---

## 0 — Résultat

**Une hypothèse est réfutée, une autre survit sans être prouvée. La cause n'est pas établie.**

```
N3OCTIES_STATUS=BLOCKED_BUFFER_REUSE_CAUSALITY_GATE
FIRST_DIVERGENCE_CAUSAL_ROOT=NOT_PROVEN
BUFFER_REUSE_CAUSALITY=SUSPECTED_NOT_PROVEN
```

## 1 — Un indice mesuré, sur les données existantes

```
blk.12.attn_gate.weight    activation amax= 0.00000   base amax= 5.79812
blk.15.attn_output.weight  activation amax= 0.00000   base amax=11.04527
blk.16.attn_gate.weight    activation amax=11.04527   base amax= 4.43865
blk.20.ffn_up.weight       activation amax=11.04527   base amax= 3.79854
```

**Trois tenseurs sans rapport rapportent exactement `11.04527` en activation.** Or `11.04527` est le
`amax` de la **sortie** de `blk.15.attn_output`. C'est la signature d'une **région mémoire partagée**,
pas de trois coïncidences.

## 2 — Un indice que je refuse d'utiliser comme preuve

```
EPOQUE 0 : 131 activations egales au PREFIXE de la sortie d un AUTRE tenseur
EPOQUE 1 : 131 aussi
```

L'indice est réel mais il **ne discrimine pas** les deux époques. Un tenseur d'activation partagé par
plusieurs consommateurs légitimes explique une partie de ces égalités (même tenseur, mêmes valeurs).

**Je ne l'utilise donc pas comme preuve.** Je le conserve comme observation.

## 3 — H1 réfutée, H2 survivante

Votre §5 nomme deux hypothèses :

```
H1 : cur_capture est deja nul lorsqu il est consomme par MM_RAW
H2 : cur_capture est valide lors de sa consommation, mais son stockage est
     REUTILISE avant l extraction hote
```

**Discriminant disponible sans aucune instrumentation** : si H1 était vraie, la sortie native du
matmul serait nulle. Or les **25 sorties natives sont non nulles**.

```
H1_REFUTED_BY_NONZERO_NATIVE_OUTPUT=YES
H2_SURVIVING_NOT_PROVEN=YES
```

L'activation était donc **valide lors de sa consommation**. Ce qui a changé est survenu **après**.

## 4 — Ce que je n'ai PAS fait, et ne déclare pas

```
etape A : comparaison des chemins de construction des 4 temoins     NON EXECUTEE
etape B : lecture de la regle de l allocateur (GGML_OP_DUP)          NON EXECUTEE
          identification des REGIONS memoire (region_start/end)      NON FAITE
etape C : chronologie I1 / I2 / I3                                   NON ETABLIE
etape D : experience de preservation ciblee                          NON MENEE
etape E : portee sur les 25 cibles                                   NON VERIFIEE
```

```
CAPTURE_BUFFER_REGIONS_IDENTIFIED=NO
CAPTURE_REGION_OVERLAP_COUNT=NON_MESURE
OVERLAP_AFTER_LAST_CONSUMER_COUNT=NON_MESURE
CAPTURE_CONTENT_LIFETIME_THROUGH_I1=VALIDE_PAR_REFUTATION_DE_H1
CAPTURE_CONTENT_LIFETIME_THROUGH_I2=NON_ETABLI
CAPTURE_CONTENT_LIFETIME_THROUGH_I3=NON_ETABLI
FIRST_UNPROVEN_LIFETIME_INTERVAL=I2_ET_I3
TARGETED_PRESERVATION_TEST_EXECUTED=NO
ALL_25_CONTRADICTIONS_EXPLAINED=NO
```

Votre §3 exige que le chevauchement soit **établi** et que son **effet causal** le soit aussi.
**Aucun des deux n'est fait.** Un `amax` identique est un indice, pas un chevauchement démontré.

## 5 — Une règle de validité que je dois corriger d'avance

```
RULE_DE_VALIDITE_TENUE_PAR_AMAX_SUP_0=NON_SUFFISANTE
```

143 activations d'époque 0 sont non nulles — cela ne garantit **pas** leur fidélité. Si H2 est vraie,
elles ont pu être écrasées elles aussi. Votre §6 le dit : la règle doit porter sur la **conservation
du contenu jusqu'à l'extraction**, pas sur `amax > 0`.

Je n'ai donc **aucune** base pour considérer les 143 comme fiables.

## 6 — Rapport final (§11)

```
ENGINE007B_F1A_STATUS=BLOCKED_ACTIVATION_CAPTURE_GATE
F1A_Q0_N3OCTIES_STATUS=BLOCKED_BUFFER_REUSE_CAUSALITY_GATE
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
ZERO_ACTIVATION_TARGETS_EPOCH0=25
ZERO_ACTIVATION_TARGETS_EPOCH1=0
SELECTED_TARGET_GRAPH_RELATIONS=ACQUIS_EN_R7_NON_REVERIFIE_SUR_LES_4_TEMOINS
ZERO_VS_HEALTHY_BUILD_PATH_DIFFERENCES=NON_MESUREES
CAPTURE_BUFFER_REGIONS_IDENTIFIED=NO
CAPTURE_REGION_OVERLAP_COUNT=NON_MESURE
OVERLAP_AFTER_LAST_CONSUMER_COUNT=NON_MESURE
CAPTURE_CONTENT_LIFETIME_THROUGH_I1=VALIDE_PAR_REFUTATION_DE_H1
CAPTURE_CONTENT_LIFETIME_THROUGH_I2=NON_ETABLI
CAPTURE_CONTENT_LIFETIME_THROUGH_I3=NON_ETABLI
FIRST_UNPROVEN_LIFETIME_INTERVAL=I2_ET_I3
TARGETED_PRESERVATION_TEST_EXECUTED=NO
PRESERVED_CAPTURE_CONTENT_VALID=NON_MESURE
DIAGNOSTIC_LOGITS_BITWISE_MATCH=NON_MESURE
FIRST_DIVERGENCE_CAUSAL_ROOT=NOT_PROVEN
BUFFER_REUSE_CAUSALITY=SUSPECTED_NOT_PROVEN
H1_REFUTED_BY_NONZERO_NATIVE_OUTPUT=YES
H2_SURVIVING_NOT_PROVEN=YES
ALL_25_CONTRADICTIONS_EXPLAINED=NO
ALL_REQUIRED_CAPTURE_CONTENT_PRESERVED=NO
ZERO_INPUT_NONZERO_OUTPUT_CONTRADICTION_COUNT=25
RULE_DE_VALIDITE_TENUE_PAR_AMAX_SUP_0=NON_SUFFISANTE
CAPTURE_DISABLED_RUNTIME_PATH_INERT=YES
SUBMITTED_TARGET_CAPTURE_COMPLETENESS=PASS_POUR_LA_STRUCTURE_NON_POUR_LE_CONTENU
Q0_CAPTURE_GRAFT_NUMERICALLY_TRANSPARENT=YES
INDEPENDENT_ROW_OUTPUT_INDEXING_TEST=NOT_EXECUTED
REFERENCE_SAMPLE_TEMPORAL_PAIRING=PARTIAL
Q0_NATIVE_OPERATOR_REFERENCE_VALIDATION=NOT_PASSED
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
PRODUCT_MMVQ_SOURCE_MODIFIED=NO
MODEL_GGUF_MODIFIED=NO
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
ENGINE007_NEXT_STEP=READ_ALLOCATOR_RULE_FOR_DUP_AND_COMPUTE_REGIONS
```

## 7 — Livrables réellement créés

```
N3OCTIES_STATUS.txt   indices, refutation de H1, ce qui reste non prouve, gates
```

**Non créés** : `N3OCTIES_SELECTED_TARGETS.csv`, `N3OCTIES_GRAPH_BUILD_PATH_COMPARISON.txt`,
`N3OCTIES_TARGET_GRAPH_RELATIONSHIPS.csv`, `N3OCTIES_ALLOCATOR_SOURCE_TRACE.txt`,
`N3OCTIES_BUFFER_REGIONS.csv`, `N3OCTIES_BUFFER_OVERLAPS.csv`,
`N3OCTIES_BUFFER_LIFETIME_PROOF.txt`, `N3OCTIES_EXTRACTION_CHRONOLOGY.txt`,
`N3OCTIES_PRESERVATION_SOURCE_CONTRACT.txt`, `N3OCTIES_TARGETED_PRESERVATION_TEST.csv`,
`N3OCTIES_CORRECTED_CONTENT_INVENTORY.csv`, `N3OCTIES_OFF_ON_REGRESSION.txt`,
`N3OCTIES_CAPTURE_COMPLETENESS.txt`, `N3OCTIES_FIRST_DIVERGENCE.txt`,
`N3OCTIES_PATCH_PROVENANCE.txt`, `N3OCTIES_BINARY_PROVENANCE.txt`,
`N3OCTIES_CONFIG_PROVENANCE.txt`.

## 8 — La prochaine étape, dans l'ordre

```
1. LIRE l allocateur du fork : comment traite-t-il GGML_OP_DUP et un tenseur qui
   n est pas marque GGML_TENSOR_FLAG_OUTPUT ?
   -> si la region d un DUP non-marque est reutilisable apres sa derniere
      consommation, H2 est mecaniquement plausible.
2. CALCULER les regions memoire reelles des 4 temoins a chaque epoque
   (buffer + offset interne), et chercher un chevauchement avec un tenseur
   produit PLUS TARD dans la meme soumission.
   -> c est l etape que le brief exige et que je n ai pas faite.
3. seulement alors : experience de preservation ciblee, si le mecanisme est confirme.
```

**Un point d'interet** : l'époque 0 est celle qui contenait les 1176 cibles **bufferless**. Elle est
donc l'époque où le planificateur alloue le plus tardivement — cohérent avec un contenu extrait trop
tôt dans le cycle de vie. Cela renforce H2 sans la prouver.

## 9 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007_PRIMARY_LEVER=EPISODE_ALIGNED_STRUCTURED_WORK_ELIMINATION
ENGINE007B_STATUS=UNBLOCKED_FOR_STRUCTURED_EPISODE_FEASIBILITY_ONLY
ENGINE007B_F1A_STATUS=BLOCKED_ACTIVATION_CAPTURE_GATE
F1A_Q0_N3OCTIES_STATUS=BLOCKED_BUFFER_REUSE_CAUSALITY_GATE
F1A_Q0_N3SEPTIES_STATUS=BLOCKED_CAPTURE_CONTENT_CAUSALITY_GATE
F1A_Q0_R7_STATUS=SUBMITTED_GRAPH_CAPTURE_INFRASTRUCTURE_QUALIFIED
BUFFER_REUSE_CAUSALITY=SUSPECTED_NOT_PROVEN
H1_REFUTED_BY_NONZERO_NATIVE_OUTPUT=YES
H2_SURVIVING_NOT_PROVEN=YES
FIRST_DIVERGENCE_CAUSAL_ROOT=NOT_PROVEN
CAPTURE_CONTENT_VALIDITY_RULE=NOT_PROVEN
RULE_DE_VALIDITE_TENUE_PAR_AMAX_SUP_0=NON_SUFFISANTE
EPOCH0_ZERO_ACTIVATIONS=25_SUR_168
EPOCH1_ZERO_ACTIVATIONS=0_SUR_168
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
