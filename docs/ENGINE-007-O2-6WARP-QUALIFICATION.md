# ENGINE-007-O2 — QUALIFICATION DE LA REDISTRIBUTION DENSE SUR 6 WARPS

Date : 2026-09-29. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e7-o2/`

---

## 0 — Résultat

**Tous les gates de qualification passent. Aucun chronométrage n'a été exécuté.**

```
ENGINE007O2_STATUS=QUALIFIED_TIMING_PENDING
TIMING_EXECUTED=NO
```

| | natif | O2 |
|---|---|---|
| threads par bloc | 128 | **192** |
| warps physiques | 4 | **6** |
| épisodes par warp | **{2,2,1,1}** | **{1,1,1,1,1,1}** |
| kbx logiques | 0..47 | 0..47 |
| kbx dynamiques | 192 | 192 |
| multiplicité par kbx | 4 | 4 |
| registres | 38 | 38 |
| spills | 0 | **0** |
| warps résidents/SM (théorique) | **48** | **48** |
| occupation théorique | 100 % | **100 %** |
| **fidélité bit à bit** | — | **0 mismatch sur 5 graines** |

```
HOST_KBX_SET_MATCH=YES                HOST_KBX_MULTIPLICITY_MATCH=YES
GPU_MAPPING_QUALIFICATION=PASS
SAME_LOGICAL_KBX_SET=YES              SAME_DYNAMIC_KBX_EXECUTION_COUNT=YES
WEIGHT_ADDRESS_SET_IDENTICAL=YES      Q8_ADDRESS_SET_IDENTICAL=YES      SCALE_ADDRESS_SET_IDENTICAL=YES
SAME_IDP_WORK=YES                     SAME_TQ2_DECODE_WORK=YES
INPUT_SANITY_GATE=PASS                OUTPUT_SANITY_GATE=PASS
BITWISE_KERNEL_OUTPUT_MATCH=YES       NATIVE_SPILLS=0   O2_SPILLS=0
UNEXPECTED_ARITHMETIC_CHANGE=NO       UNEXPECTED_MEMORY_WORK_CHANGE=NO
```

Le point remarquable : **l'occupation théorique ne baisse pas**. 12 blocs × 4 warps = 48 comme
8 blocs × 6 warps = 48. Il n'y a donc **pas de confond d'occupation** qui viendrait brouiller
l'interprétation du futur chronométrage.

```
O2_OCCUPANCY_CONFOUND=NO
```

---

## 1 — Provenance (§5)

```
REPO=/home/triviere/projects/llama-obrain-cuda
GIT_HEAD=8447604f7f0c3b48a77be46de2466a5b23e27826
GIT_BRANCH=feat/cuda-tq2g128
ARBRE_SALE_CONNU=oui (build-cuda/ : fichiers de progression CMake supprimes) -> NON reclasse en defaut
SHA256 ggml/src/ggml-cuda/mmvq.cu      = 2bf9a27028207120f476ab543264f0f5ec40ceceb4058989efcf1a84fdeee037
SHA256 ggml/src/ggml-cuda/ggml-cuda.cu = 960e38d680744e39723d241d4742856c1cb5336de13d007aa7a61debd5a67ada
SHA256 ggml/src/ggml-cuda/common.cuh   = 4210eb862ea61eef15f4963e0b4b947618b1330c12a79bedd29610ccc4c5d5e2
SHA256 ggml/src/ggml-cuda/vendors/cuda.h = e7847065d5d741f892d2f1f7aaaa8f418a41e3b983d1e6f09010055d4d1c38d1
NVCC=Build cuda_13.2.r13.2/compiler.37953736_0      CUDA=13.2
GPU=NVIDIA GB10   DRIVER=580.173.02   COMPUTE_CAP=12.1

Q4N_MODEL_FAMILY=Qwen3.8     Q4N_MODEL_VARIANT=27B
Q4N_LLAMA_CPP_ARCH_ENUM=LLM_ARCH_QWEN35      Q4N_LLAMA_CPP_ARCH_ID=qwen35
LLAMA_CPP_ARCH_ID_IS_MODEL_VERSION=NO
ACTIVE_MODEL_IS_QWEN3_8_27B=YES              QWEN3_5_USED_FOR_EXECUTION=NO

R12_ARM_TABLE_SHA256=e829d189c74ab2623b7eef9e15bc454ff887125347760934fcd54990e0e6bb94
```

**Aucun fichier du dépôt produit n'a été modifié.** La variante O2 vit dans un microbench autonome
(`e7-o2.cu`), conformément au §6 qui exige une variante **compilée séparément** et au §28 qui exige
un patch déclaré. Le patch est une déclaration de la transformation, pas une mutation du dépôt.

```
O2.patch      sha256=8ada1e0b9ccd602331f324d566d0d6027937108b6ae45073659b8c08dbb135e9
e7-o2.cu      sha256=ae2a4dbaa5183961...
```

## 2 — Géométrie et mapping (§8, §10)

**Natif** — `kbx = tid/4 + 32*i`, 128 threads, 4 warps :

```
warp0 -> G0 (kbx 0..7) + G4 (kbx 32..39)
warp1 -> G1 (kbx 8..15) + G5 (kbx 40..47)
warp2 -> G2 (kbx 16..23)
warp3 -> G3 (kbx 24..31)
EPISODES_PER_WARP={2,2,1,1}
```

**O2** — `kbx = 8*warp + lane/4`, 192 threads, 6 warps :

```
warp0 -> G0   warp1 -> G1   warp2 -> G2   warp3 -> G3   warp4 -> G4   warp5 -> G5
EPISODES_PER_WARP={1,1,1,1,1,1}
```

```
O2_ONLY_INTENDED_TOPOLOGY_CHANGE=EPISODE_TO_PHYSICAL_WARP_ASSIGNMENT
```

**Autre différence structurelle introduite, et elle doit être déclarée** : le partage de l'unité de
réduction. En natif, 4 threads consécutifs (`tid/4` constant) traitent un même kbx ; ce groupement
par 4 est **préservé** en O2 (`lane/4`), donc le calcul de `sumi` est identique. En natif, un thread
traite **1 à 2** corps ; en O2, exactement **1**. C'est la conséquence directe de la redistribution.

## 3 — Preuve hôte (§9)

Séquences ordonnées `(thread, warp, lane, episode, kbx)` générées pour les deux géométries :

```
HOST_MAPPING_NATIVE.txt   195 lignes (header + 192)
HOST_MAPPING_O2.txt       195 lignes
HOST_MAPPING_COMPARE.txt
```

```
NATIVE_LOGICAL_KBX_SET=0..47          O2_LOGICAL_KBX_SET=0..47
NATIVE_DYNAMIC_KBX_COUNT=192          O2_DYNAMIC_KBX_COUNT=192
NATIVE_PER_KBX_MULTIPLICITY=4 uniforme   O2_PER_KBX_MULTIPLICITY=4 uniforme
NATIVE_EPISODES_PER_WARP={2,2,1,1}    O2_EPISODES_PER_WARP={1,1,1,1,1,1}
NATIVE_ACTIVE_PHYSICAL_WARPS=4        O2_ACTIVE_PHYSICAL_WARPS=6

EXECUTION_UNITS_IDENTIFIQUES=YES
   -> l unite elementaire (kbx, kqs) est un ENSEMBLE IDENTIQUE dans les deux variantes,
      avec multiplicite 1 partout. C est l identite du travail effectue.

HOST_KBX_SET_MATCH=YES
HOST_KBX_MULTIPLICITY_MATCH=YES
```

## 4 — Qualification GPU du mapping (§11)

Noyau instrumenté séparé, **jamais chronométré** :

```
NATIF  DYNAMIC=192 EPISODES=6 PER_KBX_MULT=4 (min=max) WARPS=4
       EPI_LANES=32,32,32,32,32,32
       EPISODE->WARPS : G0:{0} G1:{1} G2:{2} G3:{3} G4:{0} G5:{1}

O2     DYNAMIC=192 EPISODES=6 PER_KBX_MULT=4 (min=max) WARPS=6
       EPI_LANES=32,32,32,32,32,32
       EPISODE->WARPS : G0:{0} G1:{1} G2:{2} G3:{3} G4:{4} G5:{5}

GPU_MAPPING_QUALIFICATION=PASS
```

Le noyau observé confirme exactement la simulation hôte : **seule l'affectation épisode → warp
physique change**, et chaque épisode garde ses 32 lanes.

## 5 — Équivalence d'adresse (§12)

```
BASE      rowbase = w + row*48*34            (identique)
POIDS     rowbase + kbx*34 + 0               (32 octets de codes)
ECHELLE   rowbase + kbx*34 + 32              (uint16_t d)
Q8        y + kbx*sizeof(block_q8_1)         (table partagee, indexee par kbx seul)

WEIGHT_ADDRESS_SET_IDENTICAL=YES
SCALE_ADDRESS_SET_IDENTICAL=YES
Q8_ADDRESS_SET_IDENTICAL=YES
   -> l adresse ne depend que de (row, kbx). Comme l ensemble des kbx et leur multiplicite par
      thread sont identiques, l ensemble des adresses atteintes est identique, element par element.
```

## 6 — Fidélité bit à bit (§16, §26)

Cinq graines synthétiques déterministes, échelles fp16 finies normales non nulles :

| graine | sanity entrée | NaN échelle | Inf échelle | zéro échelle | **mismatch** | NaN sortie | Inf sortie |
|---|---|---|---|---|---|---|---|
| 0 | PASS | 0 | 0 | 0 | **0** | 0 | 0 |
| 1 | PASS | 0 | 0 | 0 | **0** | 0 | 0 |
| 2 | PASS | 0 | 0 | 0 | **0** | 0 | 0 |
| 3 | PASS | 0 | 0 | 0 | **0** | 0 | 0 |
| 4 | PASS | 0 | 0 | 0 | **0** | 0 | 0 |

```
BITWISE_OUTPUT_MISMATCH_COUNTS_BY_SEED=0,0,0,0,0
BITWISE_KERNEL_OUTPUT_MATCH=YES
OUTPUT_SANITY_GATE=PASS
AUCUN EPSILON, AUCUNE TOLERANCE, AUCUN COMPARATEUR RELACHE
```

**C'est le point qui méritait un soin particulier (§17).** Passer de 4 à 6 warps change la
topologie de réduction, et le risque était de réassocier l'accumulation flottante. La réduction a
donc été **généralisée sur le nombre de warps** — même structure séquentielle `for i in 0..NWARP-1`
dans les deux cas — de sorte que seules les **valeurs** des partiels changent, jamais l'**ordre** des
additions. Le résultat à 0 mismatch sur 5 graines le confirme.

```
REDUCTION_SEMANTICS_IDENTICAL_PAR_CONSTRUCTION=YES
FLOAT_ACCUMULATION_ORDER_CHANGED=NO
```

## 7 — Manifeste SASS et classification de l'écart (§19, §20)

| opérande | natif | O2 | delta | classification |
|---|---|---|---|---|
| `TOTAL_SASS_LINES` | 2113 | 2097 | **−16** | atténuation du contrôle de boucle |
| `LDG.E.U8` (poids) | 32 | 32 | 0 | — |
| `LDG.E.S8` (activations) | 32 | 32 | 0 | — |
| `LDG.E.U16` (échelle) | 1 | 1 | 0 | — |
| `IDP.4A` | **32** | **32** | 0 | **travail arithmétique identique** |
| `LOP3` | 258 | 258 | 0 | décodage identique |
| `SHF` | 103 | 102 | −1 | contrôle |
| `IMAD` | 69 | 69 | 0 | — |
| `IADD` | 169 | 162 | **−7** | contrôle de boucle (une seule itération en O2) |
| `LEA` | 35 | 35 | 0 | — |
| `I2F` | 1 | 1 | 0 | frontière entier→flottant identique |
| `FMUL` | 0 | 0 | 0 | — |
| `FFMA` | 1 | 1 | 0 | — |
| `FADD` | **9** | **11** | **+2** | **soutien de réduction (6 partiels au lieu de 4)** |
| `SHFL` | 5 | 5 | 0 | réduction warp identique |
| `BAR` | 1 | 1 | 0 | — |
| `STG` | 1 | 1 | 0 | — |
| `BSSY` / `BSYNC` | 1 / 1 | 1 / 1 | 0 | topologie de contrôle identique |

```
EXPECTED_GEOMETRY_CONTROL    = -16 lignes (boucle), -7 IADD, -1 SHF
EXPECTED_REDUCTION_SUPPORT   = +2 FADD
UNEXPECTED_ARITHMETIC_CHANGE=NO
UNEXPECTED_MEMORY_WORK_CHANGE=NO
```

Le `+2 FADD` est **exactement** le coût de la sommation des 6 partiels au lieu de 4, et le
`−16` lignes vient de la disparition de la seconde itération de boucle. Tous les compteurs de
travail réel — 32 `IDP.4A`, 32 chargements de poids, 32 d'activations, 258 `LOP3` — sont
**strictement identiques**. C'est la signature attendue d'une redistribution pure.

## 8 — Ressources et occupation (§21, §22, §23)

```
NATIVE_THREADS_PER_BLOCK=128              O2_THREADS_PER_BLOCK=192
NATIVE_REGISTERS=38                       O2_REGISTERS=38
NATIVE_SPILLS=0                           O2_SPILLS=0
NATIVE_STATIC_SHARED=16                   O2_STATIC_SHARED=24
NATIVE_DYNAMIC_SHARED=0                   O2_DYNAMIC_SHARED=0

NATIVE_MAX_ACTIVE_BLOCKS_PER_SM=12        O2_MAX_ACTIVE_BLOCKS_PER_SM=8
NATIVE_RESIDENT_WARPS_PER_SM=48           O2_RESIDENT_WARPS_PER_SM=48
NATIVE_THEORETICAL_OCCUPANCY=100,00 %     O2_THEORETICAL_OCCUPANCY=100,00 %

SM_COUNT=48   MAX_THREADS_PER_SM=1536   MAX_WARPS_PER_SM=48   REGS_PER_SM=65536

O2_OCCUPANCY_CONFOUND=NO
```

C'est une propriété **théorique** de lancement et de ressource, calculée par l'API d'occupation
CUDA, **pas** une occupation mesurée à l'exécution — les compteurs matériels sont indisponibles
(R13). Le §22 demande explicitement de ne pas confondre les deux.

Le fait que 12×4 = 8×6 = 48 est une bonne fortune : le chronométrage à venir comparera **la
géométrie des épisodes à capacité de warps résidents égale**.

## 9 — Grille et intégrité du protocole (§24, §25)

```
O2_GRID_X=5120          (= natif)
SAME_OUTPUT_ROW_COUNT=YES     SAME_BLOCK_COUNT=5120
PAS_DE_BLOCS_MULTI-LIGNES, PAS_DE_BLOCS_PERSISTANTS, PAS_DE_LANCEMENT_COOPERATIF
   -> le paragraphe 25 est respecte : O2 isole la redistribution d episodes dans un bloc, rien d autre.
```

## 10 — Plan de chronométrage : pré-enregistré, NON exécuté (§32)

```
PROTOCOLE=K2048_WARMUP3_LATIN_MULTI_PROCESS
   KERNELS_PER_BATCH=2048   WARMUP_BATCHES=3   ONE_EVENT_PAIR_PER_BATCH
   PROCESSUS_NEUFS>=5   ROTATION_LATINE   CONTRASTES_APPARIES
   UNITE_STATISTIQUE=moyenne_par_processus
BRAS=K_FULL_NATIVE (128 threads)  vs  O2_6WARP (192 threads)
   -> deux noyaux COMPILES SEPAREMENT, pas de commutateur runtime
CONTRASTE_PRIMAIRE=O2_6WARP_MINUS_NATIVE_US
ANCRES_A_INCLURE=seulement si elles restent comparables ; le binaire O2 est autonome
CORRECTION_DU_DIAGNOSTIC_DE_POSITION=utiliser l effet intra-bras ou sur les contrastes apparies
   (HARNESS_DEFECT_43), pas la plage des moyennes par position
```

Interprétation pré-enregistrée (§33) :

| résultat | promotion |
|---|---|
| gain substantiel | `DENSE_EPISODE_REDISTRIBUTION=MATERIAL_KERNEL_LEVER` |
| gain faible (~1-3 %) | `PHYSICAL_WARP_REUSE_PENALTY=REAL_BUT_LIMITED_AT_FULL_GEOMETRY` |
| aucun gain | `R12_E2_SAME_VS_DIFFERENT_EFFECT=DOES_NOT_GENERALIZE_TO_FULL_SIX_EPISODE_GEOMETRY` |
| régression | `SIX_WARP_GEOMETRY=REJECTED` |

```
TIMING_EXECUTED=NO
PRODUCT_GAIN=NOT_MEASURED
TOKENS_PER_SECOND_GAIN=NOT_MEASURED
END_TO_END_GAIN=NOT_MEASURED
```

## 11 — Défauts de harnais de cette passe

```
HARNESS_DEFECT_45=SYMBOL_EXTRACTION_TRUNCATED_FOR_k_o2_6warp
   -> comme en R7 : la premiere extraction du nom mange a rendu un SASS vide (27 lignes) pour O2.
      Detecte immediatement, extraction corrigee, aucune conclusion tiree du releve vide.
   STATUS=RESOLVED
PLUMBING_FAILURE_29=NON_ASSIGNE
```

## 12 — Nettoyage de /tmp (demande de l'utilisateur, avant tout redémarrage)

30 fichiers de travail ont été rapatriés de `/tmp` du nœud vers
`qualification/_tmp-archive/` (6,9 Mo) : journaux `ptxas`/édition de liens, extraits SASS
intermédiaires de R4/R5/R6/R7, et deux scripts d'analyse. Une fois la vérification faite,
**tous mes fichiers `/tmp` ont été supprimés** : les sources `.cu` de toutes les passes étaient
déjà archivées dans leurs répertoires de qualification respectifs, et les binaires dans `bin/`.

```
FICHIERS_RAPATRIES=30      TAILLE=6,9 Mo      DESTINATION=qualification/_tmp-archive/
FICHIERS_SUPPRIMES_DE_TMP=mes fichiers e7a-*, r2*, r4*, r5*, r6*, r7*, r9*, r11*, r13*
AUCUN_FICHIER_TIERS_TOUCHE=oui (verifie : tous les fichiers recents etaient les miens)
SOURCE_006I_MANQUANT_RETROUVE=qualification/e6i/e6i-spec.cu (archives sous un autre nom)
```

## 13 — Ledger

```
ENGINE007_STATUS=OPEN
ENGINE007A_R12_STATUS=CLOSED_ACTIVE_WARP_EPISODE_COUNT_PRIMARY_WALL_VARIABLE
ENGINE007A_R13_STATUS=BLOCKED_MISSING_HARDWARE_COUNTER_PERMISSION
ENGINE007_PRIMARY_LEVER=WARP_EPISODE_LEVEL_STRUCTURED_EXECUTION
ENGINE007B_STATUS=UNBLOCKED_FOR_STRUCTURED_EPISODE_FEASIBILITY_ONLY
ENGINE007B_PRODUCT_OPTIMIZATION_ALLOWED=NO
ENGINE007C_STATUS=BLOCKED

ENGINE007O2_STATUS=QUALIFIED_TIMING_PENDING
BASELINE_PROVENANCE=GIT_8447604f_branch_feat_cuda-tq2g128_arbre_sale_connu
PATCH_SHA256=8ada1e0b9ccd602331f324d566d0d6027937108b6ae45073659b8c08dbb135e9
NATIVE_THREADS_PER_BLOCK=128
O2_THREADS_PER_BLOCK=192
NATIVE_PHYSICAL_WARPS=4
O2_PHYSICAL_WARPS=6
NATIVE_EPISODE_MAPPING={2,2,1,1}
O2_EPISODE_MAPPING={1,1,1,1,1,1}
SAME_LOGICAL_KBX_SET=YES
SAME_DYNAMIC_KBX_EXECUTION_COUNT=YES
HOST_MAPPING_GATE=PASS
GPU_MAPPING_GATE=PASS
WEIGHT_ADDRESS_SET_IDENTICAL=YES
Q8_ADDRESS_SET_IDENTICAL=YES
SCALE_ADDRESS_SET_IDENTICAL=YES
SAME_IDP_WORK=YES
SAME_TQ2_DECODE_WORK=YES
BITWISE_OUTPUT_MISMATCH_COUNTS_BY_SEED=0,0,0,0,0
BITWISE_KERNEL_OUTPUT_MATCH=YES
NATIVE_REGISTERS=38
O2_REGISTERS=38
NATIVE_SPILLS=0
O2_SPILLS=0
NATIVE_MAX_ACTIVE_BLOCKS_PER_SM=12
O2_MAX_ACTIVE_BLOCKS_PER_SM=8
NATIVE_RESIDENT_WARPS_PER_SM=48
O2_RESIDENT_WARPS_PER_SM=48
O2_OCCUPANCY_CONFOUND=NO
UNEXPECTED_ARITHMETIC_CHANGE=NO
UNEXPECTED_MEMORY_WORK_CHANGE=NO
REDUCTION_ORDER_PRESERVED_BY_CONSTRUCTION=YES
TIMING_EXECUTED=NO
PRODUCT_GAIN=NOT_MEASURED
NEXT_STEP_IF_QUALIFIED=O2_NATIVE_VS_6WARP_CANONICAL_TIMING
HARNESS_DEFECT_45=RESOLVED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
