# ENGINE-007A-R10 — QUALIFICATION DE LA TABLE D'ARMES KBX

Date : 2026-09-29. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7a-r10/`

---

## 0 — Résultat

```
ENGINE007A_R10_STATUS=ARM_TABLE_QUALIFICATION_DONE_TIMING_PENDING
HARNESS_DEFECT_38=RESOLVED

ARM_TABLE_SHA256=e71b8358c80333f7526dc55c40bfcd5a92dd20fe99cfb1b6ba652bcd470a5ba7
ARM_TABLE_SELF_CHECK=PASS
ARM_TABLE_HOST_SIMULATION=PASS
GPU_KBX_QUALIFICATION=PASS
KBX48_NATIVE_KBX_SEQUENCE_MATCH_K_FULL=YES
KBX48_NATIVE_OUTPUT_MISMATCH=0          (etabli en R9A)
INPUT_SANITY_GATE=PASS
```

**Aucun timing n'a été exécuté dans cette passe**, conformément au §42.

Artefacts produits :

| fichier | sha256 (16 premiers) |
|---|---|
| `ARM_TABLE.txt` | `e71b8358c80333f7` |
| `HOST_SIMULATION.txt` | `563d8288acd06fbd` |
| `GPU_KBX_QUALIFICATION.txt` | `1e446b6818125c40` |

---

## 1 — Freeze R2-SANE (§1 à §5)

```
ENGINE007A_R2_SANE_STATUS=CLOSED_CONCLUSIVE_REPLICATED_ON_VALID_INPUTS
INPUT_SANITY_GATE=PASS        OUTPUT_SANITY_GATE=PASS
TQ2_BLOCK_COUNT=245760        FINITE_SCALE_COUNT=245760
NAN_SCALE_COUNT=0   INF_SCALE_COUNT=0   SUBNORMAL_SCALE_COUNT=0   ZERO_SCALE_COUNT=0
INVALID_FLOAT_METADATA_COUNT=0
KERNEL_DEVICE_CODE_PARITY=PASS_BY_COMPLETE_SASS_MANIFEST
CUBIN_IDENTITY=NOT_RECORDED
   -> je ne confonds pas une comparaison SASS complete avec un hash de cubin. Le paragraphe 1
      autorise explicitement cette distinction ; je la conserve telle quelle.

SANE_A100_MINUS_A0_US=2.9331   CI95=[2.8303,3.0359]   PERCENT=3.22   EFFECT_TO_NOISE=55.9
USEFUL_DP4A_WORK_IS_NOT_A_DOMINANT_WALL_COMPONENT=SUPPORTED_CLEAN_DATA
DP4A_EXECUTION_THROUGHPUT_LIMIT=REJECTED_BY_SANE_ARITHMETIC_REMOVAL_EXPERIMENT

A50_EVEN_MINUS_A50_ODD_US=-0.2585   -> A50_PLACEMENT_EFFECT=NOT_MATERIALLY_DETECTED
A25_A_MINUS_A25_B_US=-1.2520        -> A25_PLACEMENT_EFFECT=MATERIAL_AND_REPRODUCED
IDP_COUNT_ALONE_INSUFFICIENT=YES_CLEAN_DATA
ARITHMETIC_WORK_SITE_PLACEMENT_MATTERS=YES_CLEAN_DATA

A0_TO_A25=+0.7099  A25_TO_A50=+0.0470  A50_TO_A75=+0.7857  A75_TO_A100=+1.3905
ARITHMETIC_WORK_LINEAR_SCALING=REJECTED
ARITHMETIC_WORK_THRESHOLD_BEHAVIOR=SUGGESTED_BY_CLEAN_DATA_NOT_MECHANISTICALLY_EXPLAINED
   -> aucun cout par IDP n est fitte ni promu.

OLD_R2_RESULT=HISTORICAL_CONTAMINATED_BUT_SCIENTIFIC_CONCLUSION_REPLICATED_ON_SANE_DATA
OLD_CONTAMINATED_A100_MINUS_A0_US=3.4391 (3.65 %)   <- conserve a titre historique
ENGINE006_STATUS=CLOSED_NO_MATERIAL_RECOVERABLE_LOCAL_WASTE_FOUND   (non rouvert, paragraphe 6)
```

## 2 — Résolution de HARNESS_DEFECT_38 (§9 à §13)

La table fait autorité. Format sérialisé déterministe :

```
ARM_NAME|KERNEL_SYMBOL|MASK_HEX|KBX_COUNT|EXPECTED_KBX_SET|EXPECTED_DYNAMIC_KBX_COUNT|KBX_COUNT_PER_WARP|IS_REPLICATE|REPLICATE_OF
```

Mapping natif simulé : `kbx = tid/4 + 32*i`, 128 threads, 4 threads par bloc (`kqs = tid%4`), 48 blocs.

| arme | masque | blocs | kbx dynamiques | par warp | doublon |
|---|---|---|---|---|---|
| `K_FULL_NATIVE` | `0xffffffffffff` | 48 | 192 | 64/64/32/32 | — |
| `KBX48_NATIVE` | `0xffffffffffff` | 48 | 192 | 64/64/32/32 | — |
| `KBX36_SET_A` | `0xfffffffffeee` | 36 | 180 | 56/60/32/32 | non |
| `KBX24_SET_A` | `0x555555555555` | 24 | 96 | **32/32/16/16** | non |
| `KBX24_SET_B` | `0xaaaaaaaaaaaa` | 24 | 96 | **32/32/16/16** | non |
| `KBX12_SET_A` | `0x111111111111` | 12 | 48 | **16/16/8/8** | non |
| `KBX12_SET_B` | `0x222222222222` | 12 | 48 | **16/16/8/8** | non |
| `KBX12_SET_C` | `0x444444444444` | 12 | 48 | **16/16/8/8** | non |
| `KBX08_SET_A` | `0x041041041041` | 8 | 32 | **12/8/4/8** | non |
| `KBX08_SET_B` | `0x208208208208` | 8 | 32 | **12/12/4/4** | non |
| `KBX06_SET_A` | `0x010101010101` | 6 | 24 | 8/8/4/4 | non |
| `KBX06_SET_B` | `0x101010101010` | 6 | 24 | 8/8/4/4 | non |

```
ARM_COUNT=12
UNEXPECTED_DUPLICATE_CONFIG_COUNT=0
CARDINALITY_GATE=popcount(mask)==KBX_COUNT==size(EXPECTED_KBX_SET)_VERIFIE_POUR_LES_12_ARMES
```

`K_FULL_NATIVE` et `KBX48_NATIVE` partagent le même masque **volontairement** — ce sont deux noyaux
différents (natif contre boucle runtime), donc leurs configs canoniques diffèrent par
`kernel_symbol` et ne sont pas comptées comme doublons. C'est exactement la classe d'erreur qui
avait produit `P48`/`P48b` en R6 ; elle est fermée par construction.

## 3 — Simulation hôte (§14)

```
ARM_TABLE_HOST_SIMULATION=PASS
```

Pour chaque arme, la séquence ordonnée des couples `(thread, kbx)` a été générée par simulation du
mapping natif et comparée à la séquence attendue, sur les 128 threads. Toutes les armes :
`HOST_SEQUENCE_MATCH=YES`.

Bras complet, contrôle demandé au §16 :

```
KBX48_NATIVE_KBX_SEQUENCE_MATCH_K_FULL=YES
NATIVE_SEQUENCE_LENGTH=192   MASKED_SEQUENCE_LENGTH=192
```

## 4 — Qualification GPU (§15)

Noyau instrumenté (jamais chronométré) qui incrémente un compteur par `(thread, kbx)` effectivement
exécuté. Vérification à trois niveaux : ensemble observé, compte dynamique total, compte par bloc
contre le nombre natif de threads.

```
GPU_KBX_QUALIFICATION=PASS
```

Pour les 11 armes à masque : `SET_MATCH=YES`, `COUNT_MATCH=YES`, `PERBLOCK_MATCH=YES`.

Exemples de comptes observés, conformes à l'attendu : `KBX48_NATIVE=192`, `KBX36_SET_A=180`,
`KBX24_SET_A/B=96`, `KBX12_A/B/C=48`, `KBX08_A/B=32`, `KBX06_A/B=24`.

```
GPU_OBSERVED_KBX_SET==EXPECTED_KBX_SET=PASS_POUR_LES_11_ARMES_A_MASQUE
NO_PERFORMANCE_RESULT_FROM_QUALIFICATION_BUILD=YES
```

## 5 — Un point que je dois signaler avant la campagne (§24)

Le §24 exige que les paires à cardinalité égale soient **équilibrées entre warps**, faute de quoi un
écart de temps ne peut pas être attribué à l'emplacement seul.

```
KBX24_SET_A : 32/32/16/16      KBX24_SET_B : 32/32/16/16     -> EQUILIBRE
KBX12_SET_A : 16/16/8/8        KBX12_SET_B : 16/16/8/8       -> EQUILIBRE
KBX12_SET_C : 16/16/8/8                                     -> EQUILIBRE
KBX08_SET_A : 12/8/4/8         KBX08_SET_B : 12/12/4/4      -> NON EQUILIBRE (confond)
```

**`KBX08_SET_A` et `KBX08_SET_B` ne sont pas équilibrés entre warps** (12/8/4/8 contre 12/12/4/4).
Le §21 recommande la cardinalité 8 précisément parce que c'est là que R2-SANE a mesuré l'effet
d'emplacement le plus fort — mais ces deux masques-là introduiraient un déséquilibre de charge qui
interdirait l'attribution.

**Recommandation** : utiliser **`KBX12_SET_A` / `KBX12_SET_B` / `KBX12_SET_C`** comme contrôle
principal de même cardinalité — ils sont à trois ensembles de 12 blocs, strictement équilibrés
(16/16/8/8), et disjoints. `KBX24_SET_A`/`KBX24_SET_B` (32/32/16/16) servent de contrôle secondaire.
Je garde `KBX08` dans la table mais **je ne l'utiliserai pas comme contrôle primaire** sans
déclarer le confond.

```
KBX_SAME_CARDINALITY_PRIMARY_CONTROL=KBX12_SET_A_vs_KBX12_SET_B_vs_KBX12_SET_C
KBX_SAME_CARDINALITY_SECONDARY_CONTROL=KBX24_SET_A_vs_KBX24_SET_B
KBX08_WARP_BALANCE_CONFOUND=DOCUMENTED_NOT_USED_AS_PRIMARY_CONTROL
```

## 6 — Ce qui reste avant le timing (§41)

```
ARM_TABLE_SHA256=RECORDED                       OK
ARM_TABLE_SELF_CHECK=PASS                       OK
ARM_TABLE_HOST_SIMULATION=PASS                  OK
GPU_KBX_QUALIFICATION=PASS                      OK
HARNESS_DEFECT_38=RESOLVED                      OK
KBX48_NATIVE_KBX_SEQUENCE_MATCH_K_FULL=YES      OK
KBX48_NATIVE_OUTPUT_MISMATCH=0                  OK (R9A)
INPUT_SANITY_GATE=PASS                          OK
OUTPUT_SANITY_GATE=PASS                         OK

ENGINE007A_R10_TIMING_ALLOWED=YES
```

Le §42 interdisait le timing dans cette passe : il n'y en a pas eu. La campagne est autorisée pour
la passe suivante.

## 7 — Protocole prévu (§28, §29, §33)

```
CANONICAL_MICROBENCH_TIMING_PROTOCOL=K2048_WARMUP3_LATIN_MULTI_PROCESS
KERNELS_PER_BATCH=2048   WARMUP_BATCHES=3   ONE_EVENT_PAIR_PER_BATCH
FRESH_PROCESS_PER_CAMPAIGN   PROCESS_COUNT>=5
LATIN_POSITION_BALANCING   NEUTRAL_WORK_BETWEEN_ARMS
PROCESS_LEVEL_MEAN_PRIMARY_UNIT   PAIRED_CONTRASTS

PREMIERE ANALYSE=KBX_SAME_CARDINALITY_GATE (KBX12 A/B/C, puis KBX24 A/B)
   -> si ce gate ECHOUE : KBX_COUNT_ALONE_INSUFFICIENT=YES, AUCUNE regression
      wall ~ count n est construite (paragraphe 30).
   -> si ce gate PASSE : monotonie, puis seulement alors regression et pente.

GATE PREALABLE=KBX48_NATIVE vs K_FULL_NATIVE (paragraphe 18)
   FULL_RUNTIME_MINUS_NATIVE_US ; si l ecart est mesurable malgre la fidelite bitwise :
   RUNTIME_LOOP_CODEGEN_TIMING_EFFECT=MEASURED et aucune pente native n est forcee.

LE NOYAU EST IDENTIQUE POUR TOUS LES BRAS A MASQUE :
CODEGEN_CONSTANT_ACROSS_PARTIAL_ARMS=YES
ADDITIONAL_KEEPALIVE=NONE   ANTI_HOIST_EXPRESSION=NONE   ZERO_VALUED_GUARD=NONE
```

`R4_HISTORICAL_FULL_EXPLANATION_REFERENCE=5.2808_US_PER_MISSING_PASS` reste une pré-inscription
historique, non une attente courante (§32).

## 8 — Défauts et statuts

```
HARNESS_DEFECT_37=RESOLVED_ROOT_CAUSE_INVALID_NAN_SYNTHETIC_INPUT
HARNESS_DEFECT_38=RESOLVED_BY_AUTHORITATIVE_ARM_TABLE
HARNESS_DEFECT_40=SYNTHETIC_TQ2_G128_GENERATOR_FILLED_FP16_SCALE_WITH_ARBITRARY_BITS
   STATUS=RESOLVED_R9A
   -> le numero 40 etait bien le suivant non utilise dans le ledger local ; il est confirme.
PLUMBING_FAILURE_COUNT=28   PLUMBING_FAILURE_29=NOT_ASSIGNED
```

Errata historique (§39), inchangés :

```
R7/R7B/R8 : NUMERICAL_NARRATIVE_RETRACTED, STRUCTURAL_SASS_FACTS_RETAINED
R4/R5/R6  : INVALID_SYNTHETIC_INPUT_PRESENT
R5        : ALSO_STRUCTURE_GATE_FAIL
R6        : OLD_TIMING_QUARANTINED_BY_ARM_TABLE_DEFECT
R2        : CLEAN_REPLICATION_COMPLETED
```

## 9 — Provenance

```
Runtime baseline   : libllama 3db83675… / libggml-cuda abafbf0d…  (restaures, SHA verifies)
Arbre llama-obrain-cuda : les 4 fichiers du chemin chaud aux SHA vierges
Fichiers sales     : les 3 greffes connues, inchangees
MODIFICATIONS DE SOURCE DANS CETTE PASSE : AUCUNE (microbench autonome)
Instrument         : qualification/e7a-r10/e7a-r10.cu + bin/e7a-r10
   Deux noyaux (natif, runtime a masque) plus un noyau de qualification, UN binaire, UNE session.
AUCUN TIMING EXECUTE DANS CETTE PASSE
```

## 10 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007A_R2_SANE_STATUS=CLOSED_CONCLUSIVE_REPLICATED_ON_VALID_INPUTS
ENGINE007A_R10_STATUS=ARM_TABLE_QUALIFICATION_DONE_TIMING_PENDING
ENGINE007A_R10_TIMING_ALLOWED=YES
ARM_TABLE_VERSION=1
ARM_COUNT=12
ARM_TABLE_SHA256=e71b8358c80333f7526dc55c40bfcd5a92dd20fe99cfb1b6ba652bcd470a5ba7
HOST_SIMULATION_SHA256=563d8288acd06fbd5bf643f4d1b0f6ce36388751758fbbcec17260bde5d077f4
GPU_QUALIFICATION_SHA256=1e446b6818125c407cfd5e1d278bef856ed0a73b70fb43f49e05b7e9d8142778
ARM_TABLE_SELF_CHECK=PASS
ARM_TABLE_HOST_SIMULATION=PASS
GPU_KBX_QUALIFICATION=PASS
UNEXPECTED_DUPLICATE_CONFIG_COUNT=0
CARDINALITY_GATE=PASS
KBX48_NATIVE_KBX_SEQUENCE_MATCH_K_FULL=YES
KBX48_NATIVE_OUTPUT_MISMATCH=0
NATIVE_SEQUENCE_LENGTH=192
INPUT_SANITY_GATE=PASS
OUTPUT_SANITY_GATE=PASS
KERNEL_DEVICE_CODE_PARITY=PASS_BY_COMPLETE_SASS_MANIFEST
CUBIN_IDENTITY=NOT_RECORDED
CODEGEN_CONSTANT_ACROSS_PARTIAL_ARMS=YES
ADDITIONAL_KEEPALIVE=NONE
ANTI_HOIST_EXPRESSION=NONE
ZERO_VALUED_GUARD=NONE
KBX_SAME_CARDINALITY_PRIMARY_CONTROL=KBX12_SET_A_vs_KBX12_SET_B_vs_KBX12_SET_C
KBX_SAME_CARDINALITY_SECONDARY_CONTROL=KBX24_SET_A_vs_KBX24_SET_B
KBX08_WARP_BALANCE_CONFOUND=DOCUMENTED_NOT_USED_AS_PRIMARY_CONTROL
R10_KERNEL_TIMING_NOT_YET_RUN=YES
R4_HISTORICAL_FULL_EXPLANATION_REFERENCE=5.2808_US_PER_MISSING_PASS
KBX_DYNAMIC_BODY_MULTIPLICITY_HYPOTHESIS=READY_FOR_VALID_TEST
MAJOR_FIXED_FLOOR_REMAINS_UNEXPLAINED=YES
HARNESS_DEFECT_37=RESOLVED
HARNESS_DEFECT_38=RESOLVED
HARNESS_DEFECT_40=RESOLVED_R9A
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
ENGINE007B_STATUS=BLOCKED
ENGINE007C_STATUS=BLOCKED
ENGINE007_PRIMARY_LEVER=NOT_IDENTIFIED
ENGINE006_STATUS=CLOSED_NO_MATERIAL_RECOVERABLE_LOCAL_WASTE_FOUND
COMPUTE_BOUND=NOT_PROVEN
ISSUE_THROUGHPUT_LIMITED=NOT_PROVEN
```
