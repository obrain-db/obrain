# ENGINE-007B-F1A-Q0-N3septies — CAUSALITÉ DU CONTENU DES CAPTURES

Date : 2026-09-30. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7b-f1a-q0-n3septies/`

---

## 0 — Résultat

**L'inventaire complet montre que la contradiction est structurée, pas accidentelle. La cause n'est
pas établie.**

```
N3SEPTIES_STATUS=BLOCKED_CAPTURE_CONTENT_CAUSALITY_GATE
FIRST_DIVERGENCE_CAUSAL_ROOT=NOT_PROVEN
```

## 1 — Inventaire complet des deux époques (§1)

672 lignes de manifeste, **168 cibles par époque**, appariement par **nom exact de poids** (pas par
`TARGET_ID`) : **168 noms communs**.

| | époque 0 | époque 1 |
|---|---|---|
| activation toutes nulles | **25** | **0** |
| activation au moins une non nulle | 143 | 168 |
| base_output non nulle | 168 | 168 |
| **contradictions** (activation nulle ET base non nulle) | **25** | **0** |

```
EPOCH0_EPOCH1_TARGET_MATCHING=PASS
ACTIVATION_AND_OUTPUT_HASHES_VERIFIED=YES
ALL_EPOCH0_TARGETS_AUDITED=YES
ALL_EPOCH1_TARGETS_AUDITED=YES
```

**L'époque 1 est un contrôle positif** : aucune activation nulle, aucune contradiction.

## 2 — Les 25 cibles ne sont pas aléatoires

```
attn_output : blk.11 15 19 23 27 31 35 39 43 47 51 55   = LES 12 SUR 12  (famille E6)
attn_gate   : blk.9 12 14 18 22 26 30 34 38 42 46 50 54 = 13 SUR 36
ffn_up=0/48   ffn_gate=0/48   attn_q=0/12   attn_k=0/12
```

La nullité est **concentrée sur deux rôles**, avec une périodicité forte.

**Observation que je signale sans l'expliquer** : les 13 `attn_gate` incluent `blk.12` et `blk.14`,
qui **ne suivent pas** le pas de 4 des blocs d'attention complète (11, 15, 19…). Le motif des
`attn_gate` n'est donc **pas** simplement « les blocs d'attention complète ». **Je ne force pas une
règle qui ne colle pas.**

## 3 — Ce que cela change par rapport à N3sexies

N3sexies avait conclu, sur **une** cible (`blk.15.attn_output`), que l'activation d'époque 0 était
nulle. L'inventaire complet montre que c'est un **phénomène structuré** : 25 cibles, deux rôles.

```
Ce n est PAS un accident d une cible.
Ce n est PAS un probleme global de l epoque 0 (143 de ses 168 activations sont saines).
C est un probleme propre a CERTAINES PROJECTIONS, a l epoque 0.
```

Cela **affine** considérablement le champ : la piste « l'époque 0 est une passe préparatoire » est
**affaiblie** — une passe préparatoire n'aurait pas 143 activations parfaitement saines.

## 4 — Piste prioritaire pour la suite

`attn_output` et `attn_gate` sont les deux rôles dont les poids sont en famille **E6** et qui
reçoivent l'activation **repliée par la WHT** quand la rotation est active. Q0 tourne avec
`OBRAIN_NO_ROTATION=1`.

**À vérifier** : ces deux rôles empruntent-ils encore un chemin différent à la construction (le cache
`obrain_rot`) même rotation désactivée ? Et si oui, ce chemin produit-il un tenseur dont le buffer
n'est pas rempli à l'époque 0 ?

## 5 — Ce que je n'ai PAS fait, et ne déclare pas

```
etape B : verification des relations MUL_MAT sur les 25 cibles     NON EXECUTEE
etape C : chronologie du cycle de vie des buffers                   NON EXECUTEE
etape D : instrumentation post-calcul (POST_COMPUTE_CONTENT)        NON EXECUTEE
etape 6 : controle positif de reconstruction sur l epoque 1         NON EXECUTE
```

```
CAPTURE_CONTENT_VALIDITY_RULE=NOT_PROVEN
POST_COMPUTE_CONTENT_VERIFIED=NO
END_EPOCH_CONTENT_VERIFIED=NO
EPOCH1_NATIVE_REFERENCE_CONTROL=NOT_EXECUTED
```

**Je ne filtre PAS l'époque 0** pour obtenir une reconstruction plus favorable. Aucune cible n'a été
silencieusement écartée :

```
NO_REQUIRED_SUBMITTED_CAPTURE_SILENTLY_DROPPED=YES
```

## 6 — Rapport final (§10)

```
ENGINE007B_F1A_STATUS=BLOCKED_ACTIVATION_CAPTURE_GATE
F1A_Q0_N3SEPTIES_STATUS=BLOCKED_CAPTURE_CONTENT_CAUSALITY_GATE
EPOCH0_TARGET_COUNT=168
EPOCH0_ALL_ZERO_ACTIVATION_COUNT=25
EPOCH0_ZERO_INPUT_NONZERO_OUTPUT_COUNT=25
EPOCH1_TARGET_COUNT=168
EPOCH1_ALL_ZERO_ACTIVATION_COUNT=0
EPOCH1_ZERO_INPUT_NONZERO_OUTPUT_COUNT=0
EPOCH0_EPOCH1_TARGET_MATCHING=PASS
SAME_SUBMISSION_IDENTITY_PROVEN=NON_VERIFIE_EN_N3SEPTIES
SAME_MUL_MAT_RELATION_PROVEN=ACQUIS_EN_R7_POUR_L_ENSEMBLE_NON_REVERIFIE_POUR_LES_25
EPOCH0_CAPTURE_BUFFER_LIFETIME_TRACED=NO
EPOCH1_CAPTURE_BUFFER_LIFETIME_TRACED=NO
POST_COMPUTE_CONTENT_VERIFIED=NO
END_EPOCH_CONTENT_VERIFIED=NO
FIRST_DIVERGENCE_LAYER=RAW_ACTIVATION_CAPTURE_EPOCH0_ROLE_SPECIFIC
FIRST_DIVERGENCE_CAUSAL_ROOT=NOT_PROVEN
CAPTURE_CONTENT_VALIDITY_RULE=NOT_PROVEN
ZERO_ACTIVATION_ROLES=attn_output_12_sur_12__attn_gate_13_sur_36
ZERO_ACTIVATION_NOT_UNIFORM_ACROSS_EPOCH0=YES_143_ACTIVATIONS_SAINES
CORRECTED_CAPTURE_CONTENT_VERIFIED=NO
EPOCH1_NATIVE_REFERENCE_CONTROL=NOT_EXECUTED
NO_REQUIRED_SUBMITTED_CAPTURE_SILENTLY_DROPPED=YES
Q0_CAPTURE_GRAFT_NUMERICALLY_TRANSPARENT=YES
INDEPENDENT_ROW_OUTPUT_INDEXING_TEST=NOT_EXECUTED
REFERENCE_SAMPLE_TEMPORAL_PAIRING=PARTIAL
Q0_STATUS=NOT_PASSED
Q1_ALLOWED=NO
PRODUCT_MMVQ_SOURCE_MODIFIED=NO
MODEL_GGUF_MODIFIED=NO
SALIENCY_MEASURED=NO
QUALITY_ABLATION_MEASURED=NO
PRODUCT_GAIN=NOT_MEASURED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
ENGINE007_NEXT_STEP=TRACE_WHY_ATTN_OUTPUT_AND_ATTN_GATE_ARE_ZERO_IN_EPOCH0
```

## 7 — Livrables réellement créés

```
N3SEPTIES_STATUS.txt   inventaire des deux epoques, les 25 cibles, gates, piste
```

**Non créés** (étapes non exécutées) : `EPOCH0_TARGET_CONTENT_INVENTORY.csv`,
`EPOCH1_TARGET_CONTENT_INVENTORY.csv`, `EPOCH0_EPOCH1_TARGET_COMPARISON.csv`,
`ZERO_INPUT_NONZERO_OUTPUT_CONTRADICTIONS.csv`, `CAPTURE_CALCULATION_IDENTITY_PROOF.txt`,
`CAPTURE_BUFFER_LIFETIME_SOURCE_TRACE.txt`, `CAPTURE_BUFFER_REGION_MANIFEST.csv`,
`POST_COMPUTE_CONTENT_SNAPSHOTS.csv`, `END_EPOCH_CONTENT_SNAPSHOTS.csv`,
`EPOCH1_NATIVE_REFERENCE_CONTROL.csv`, `CAPTURE_CONTENT_NEGATIVE_CONTROL.txt`,
`N3SEPTIES_PATCH_PROVENANCE.txt`, `N3SEPTIES_BINARY_PROVENANCE.txt`,
`N3SEPTIES_CONFIG_PROVENANCE.txt`, `N3SEPTIES_FIRST_DIVERGENCE.txt`.

## 8 — La prochaine étape, et elle est étroite

```
1. les 25 cibles nulles sont attn_output (12/12) et attn_gate (13/36).
   -> lire le chemin de construction de ces DEUX roles dans build_lora_mm
      et voir s ils empruntent une branche differente des 4 autres roles.
2. comparer, pour une MEME cible nulle en ep0 et saine en ep1, les adresses
   de tenseur, de buffer et les offsets internes.
   -> deux tenseurs peuvent partager un buffer sans partager la meme region.
3. si le chemin de construction est identique : mesurer POST_COMPUTE,
   avant l extraction, pour une cible nulle et une cible saine.
   -> l intervalle entre post-calcul et extraction devient le domaine du diagnostic.
```

## 9 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007_PRIMARY_LEVER=EPISODE_ALIGNED_STRUCTURED_WORK_ELIMINATION
ENGINE007B_STATUS=UNBLOCKED_FOR_STRUCTURED_EPISODE_FEASIBILITY_ONLY
ENGINE007B_F1A_STATUS=BLOCKED_ACTIVATION_CAPTURE_GATE
F1A_Q0_N3SEPTIES_STATUS=BLOCKED_CAPTURE_CONTENT_CAUSALITY_GATE
F1A_Q0_N3SEXIES_STATUS=DIVERGENCE_PROVEN_AT_RAW_ACTIVATION_EPOCH0
F1A_Q0_R7_STATUS=SUBMITTED_GRAPH_CAPTURE_INFRASTRUCTURE_QUALIFIED
FIRST_DIVERGENCE_LAYER=RAW_ACTIVATION_CAPTURE_EPOCH0_ROLE_SPECIFIC
FIRST_DIVERGENCE_CAUSAL_ROOT=NOT_PROVEN
CAPTURE_CONTENT_VALIDITY_RULE=NOT_PROVEN
EPOCH0_ZERO_ACTIVATIONS=25_SUR_168
EPOCH1_ZERO_ACTIVATIONS=0_SUR_168
ZERO_ACTIVATION_ROLES=attn_output_ET_attn_gate
EPOCH1_IS_A_POSITIVE_CONTROL=YES
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
