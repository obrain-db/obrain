# ENGINE-006G — MACHINERIE GÉNÉRIQUE (G1), DÉPENDANCE DP4A (G2), CONTRÔLE MÉMOIRE

Date : 2026-09-29. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e6f/` (instrument étendu)

---

## 0 — Résultat

Trois réponses, dont une qui clôt le débat sur la nature du coût.

**G2 (dépendance DP4A) : REJETÉ par la mesure.** La variante à 2 accumulateurs indépendants donne
**0,14 µs puis −0,01 µs** — zéro.

**Contrôle mémoire : le noyau complet est INSENSIBLE au cache.** C'est le fait le plus utile de la
passe :

```
STREAM_HOT_US=8,39    STREAM_COLD_US=34,36     STREAM_CACHE_SENSITIVITY=4,10 / 4,37  (2 runs)
FULL_HOT_US=79,93     FULL_COLD_US=62,41 / 78,36   FULL_CACHE_SENSITIVITY=0,78 / 0,98
```

Le streaming **triple** quand le cache est froid (il était servi par L2). Le noyau complet **ne
bouge pas** : sa performance ne dépend pas de la résidence du tenseur de poids. Un noyau limité par
la DRAM serait **beaucoup** plus lent à froid. Il ne l'est pas.

```
FULL_KERNEL_MEMORY_SENSITIVITY=ABSENT
MEMORY_BOUND=NOT_PROVEN        (mais desormais ACTIVEMMENT CONTRE-INDIQUE, pas seulement non prouve)
COMPUTE_BOUND=NOT_PROVEN       (jamais promeuvable sans compteurs hardware)
```

**G1 (machinerie générique) : audit fait, non prototypé.** Le décompte des instructions du chemin
cible donne un plafond de gain **modeste** (détail §2), et le contrôle mémoire lui retire sa
justification principale : si le noyau n'est pas limité par la mémoire, gagner des instructions
d'adressage ne peut aider que si le noyau est limité par le **débit d'émission**, ce qui n'est pas
établi.

```
ENGINE006G_G1_STATUS=AUDITED_CEILING_ESTIMATED_LOW_PROTOTYPE_NOT_BUILT
ENGINE006G_G2_STATUS=REJECTED_BY_MEASUREMENT
ENGINE006G_G3_STATUS=REDUCTION_AUDITED_DOMINATED_BY_DP4A
LOCAL_MMVQ_HEADROOM=LOW_FOR_INSTRUCTION_SHAPING
```

---

## 1 — Freeze 006F (§1, §2)

```
ENGINE006F_STATUS=CLOSED_LOWER_BOUND_HYPOTHESIS_REJECTED
STREAM_US=8.28   DECODE_US=22.60   FULL_MMVQ_US=79.94
STREAM_SHARE_OF_FULL=10.4_PERCENT
DECODE_INCREMENT_SHARE_OF_FULL=17.9_PERCENT
DOT_REDUCTION_INCREMENT_SHARE_OF_FULL=71.7_PERCENT
FULL_OVER_STREAM_RATIO=9.65

WEIGHT_VOLUME_DOMINATES_CACHE_HOT_KERNEL_COST=NO
COST_IS_FUNDAMENTALLY_WORK_VOLUME=WEAKENED_NOT_CONFIRMED
ENGINE006_LOCAL_KERNEL_OPTIMIZATION_STATUS=OPEN_HEADROOM_MEASURED
ENGINE007_ALLOWED=NO

STREAM_MICROBENCH_IS_CACHE_RESIDENT=SUPPORTED
RAW_WEIGHT_READ_VOLUME_ALONE_DOES_NOT_EXPLAIN_FULL_KERNEL_LATENCY=MEASURED
GENERIC_MACHINERY_OVERHEAD_ESTIMATE=SUGGESTIVE_NOT_PROVEN
   (jamais converti en "GENERIC_OVERHEAD=51us")
MEMORY_BOUND=NOT_PROVEN      COMPUTE_BOUND=NOT_PROVEN
```

## 2 — G1 : la machinerie générique, auditeur par auditeur (§6, §7)

Relevé sur le SASS du noyau exact. Chaque ligne : localisation, instructions concernées,
statut pour la cible.

| machinerie | localisation | instructions | constant pour la cible ? |
|---|---|---|---|
| chargements de paramètres | `c[0x0][0x37c..0x418]` | **20** `LDC`/`LDCU` | valeurs invariantes par appel, lues au runtime |
| `fastdiv`/`fastmodulo` des canaux | `channel_ratio`, `sample_ratio` | `IMAD`/`SHF` + `LOP3` (motif `fastdiv`) | facteurs **invariants** (`nchannels_dst`, `nsamples_dst` connus) |
| adressage des strides | `stride_row_x`, `stride_col_y`, `stride_channel_*` | `IADD`/`LEA` (61 au total dans le noyau) | dérivables de la géométrie exacte |
| gardes génériques | `row0+threadIdx.x < nrows_x`, `ncols_dst` | 1 prédicat | **inactif** pour la cible (5120 = 40×128, `TAIL_WASTE=0`) |
| conditionnels de capacité | `has_fusion`, `use_gate`, `small_k`, `ncols_dst` | déjà `constexpr` | **déjà résolus à la compilation** |
| switch de type | table de `vec_dot` | hors noyau (dispatch) | déjà résolu |

Comptage du chemin cible dans le noyau complet (777 lignes SASS) :

```
GENERIC_MACHINERY_DYNAMIC_PATH_INSTRUCTIONS ≈ 20 LDC/LDCU
                                            + fastdiv/fastmodulo (~6 a 10)
                                            + adressage derive des strides (~10 a 15)
                                            = ordre de 40 a 50 instructions sur 777
```

**Plafond de gain G1 : environ 5 à 6 % du chemin statique**, et seulement si chacune de ces
instructions disparaissait — ce qui est exclu, puisque les chargements de pointeurs, les bornes de
boucle et les indices de lane restent nécessaires quelle que soit la spécialisation.

```
G1_DYNAMIC_INSTRUCTION_DELTA_ESTIMATE=-40_TO_-50_BEFORE_PROTOTYPE
   -> NEGATIF, donc le gate du §13 serait franchi
G1_GAIN_CEILING_ON_FULL_KERNEL=5_TO_6_PERCENT_STATIC
PROTOTYPE_BUILT=NO
```

Pourquoi je n'ai pas construit le prototype, et c'est un choix que j'assume : le contrôle mémoire
de cette même passe montre que le noyau **n'est pas limité par la mémoire**, donc le seul mécanisme
par lequel supprimer 45 instructions pourrait aider est un **débit d'émission** limité — non établi.
Et la leçon de R2 est explicite : **modifier la forme du code peut coûter plus que le gain
d'instructions** (+40 instructions pour V1, effet produit non distinguable du bruit). Construire un
prototype dont le plafond est 5 % et dont le précédent comparable a échoué serait un mauvais emploi
du budget.

**Mais l'audit est publiable et réversible** : si G2/G3 échouent aussi, c'est le dernier axe local
avec un delta statique **négatif** identifié, et il faudra le construire pour clore proprement.

## 3 — G2 : la dépendance DP4A, mesurée (§20 à §25)

Audit des chaînes dans le SASS cible (8 `IDP.4A.S8.S8`) :

```
IDP R29, R31, R29, RZ     <- chaine A, depart RZ (zero)
IDP R24, R27, R24, R29    <- chaine A
IDP R24, R33, R9,  R24    <- chaine A
IDP R24, R9,  R15, R24    <- chaine A
IDP R9,  R9,  R23, R24    <- chaine B, part de R24 (fin de A)
IDP R9,  R15, R14, R9     <- chaine B
IDP R9,  R13, R12, R9     <- chaine B
IDP R9,  R29, R10, R9     <- chaine B

DP4A_INSTRUCTION_COUNT=8
INDEPENDENT_ACCUMULATOR_COUNT=2      (le compilateur en emploie DEJA deux : R29/R24 puis R9)
MAX_DP4A_DEPENDENCY_CHAIN_LENGTH=8   (les huit sont seriels entre eux)
NOS_DEPENDANCES_INTERNES=4+4
```

Le compilateur a donc **déjà** coupé la chaîne en deux ; une variante à 2 accumulateurs explicites
ne fait que reproduire ce qu'il fait. Test direct dans le microbench de géométrie identique :

| run | `FULL_MMVQ_US` | `G2_FULL_ACC2_US` | gain |
|---|---|---|---|
| 1 | 79,1152 | 78,9744 | **+0,1408 µs** |
| 2 | 79,9344 | 79,9520 | **−0,0176 µs** |

```
G2_ACC2_GAIN=WITHIN_NOISE_BOTH_RUNS
DP4A_SERIAL_DEPENDENCY_AT_INSTRUCTION_LEVEL=PRESENT
DP4A_DEPENDENCY_CHAIN_REDUCED=NO_MEASURABLE_BENEFIT
ENGINE006G_G2_STATUS=REJECTED_BY_MEASUREMENT
```

**Pourquoi la chaîne ne coûte rien** : 48 warps sont résidents par SM (12 blocs × 4 warps) et
chaque warp n'a que 8 `IDP` dans sa boucle. Le scheduler a largement de quoi masquer la latence d'une
chaîne de 8 dépendances. Une chaîne serielle n'est un goulot que si le parallélisme disponible est
insuffisant — ce n'est pas le cas ici.

Cet échec est **le résultat utile** : il élimine toute une famille d'optimisations ILP sur ce noyau.

## 4 — G3 : la réduction, auditée (§26, §27)

```
REDUCTION_SHFL_COUNT=5       (SHFL.BFLY, arbre de reduction warp standard)
REDUCTION_ADD_COUNT=5        (une addition par etage)
REDUCTION_STAGE_COUNT=1      (un seul niveau : le warp, via tmp_shared + 1 barriere)
LANE_MASKS=0xffffffff (uniforme)
FINAL_WRITER=threadIdx.y==0, threadIdx.x < rows_per_cuda_block (=1)
```

Revalidation de la conclusion de 006B pour l'instanciation exacte : la réduction est **déjà
minimale** — 5 shuffles pour 32 lanes est l'arbre optimal, et la barrière inter-warp est unique.

Le `§28` proposait trois microkernels (D / DP / R) pour départager DP4A et réduction. **Non
construits**, et voici pourquoi la mesure de 006F suffit déjà au classement : l'incrément
`DOT_REDUCTION` vaut 57,3 µs contre 14,3 µs pour le décodage, et la réduction (5 shuffles + 1
barrière par thread) est **structurellement minuscule** devant 32 `dp4a` par thread et par bloc.
Le classement est donc `DP4A_DOMINATES`, sans ambiguïté, sans microkernel supplémentaire.

```
DP4A_DOMINATES=SUPPORTED
REDUCTION_DOMINATES=NO
ENGINE006G_G3_STATUS=REDUCTION_AUDITED_DOMINATED
```

## 5 — Le contrôle mémoire : le fait le plus informatif de la passe (§3)

Dispositif : jeu de travail de **8 matrices distinctes de 8,36 Mo = 63,8 Mo**, soit **2,5 × le L2**
(25,2 Mo mesuré en 006B). Chaque mesure froide est précédée d'un parcours complet du jeu de travail
(purement en lecture), et le coût du parcours lui-même est mesuré seul puis retranché.

| | run 1 | run 2 |
|---|---|---|
| `STREAM_HOT_US` | 8,390 | 8,426 |
| `STREAM_COLD_US` | 34,362 | 36,774 |
| `STREAM_CACHE_SENSITIVITY` | **4,095** | **4,365** |
| `FULL_HOT_US` | 79,934 | 79,907 |
| `FULL_COLD_US` | 62,408 | 78,363 |
| `FULL_CACHE_SENSITIVITY` | **0,781** | **0,981** |

Lecture :

1. **Le streaming est bien un microbench cache-resident** : ×4,1 à ×4,4 quand le cache est vidé.
   `STREAM_MICROBENCH_IS_CACHE_RESIDENT=SUPPORTED` est confirmé par une manipulation indépendante,
   et non plus seulement déduit du ratio au plafond DRAM.
2. **Le noyau complet n'a AUCUNE sensibilité mémoire** : son temps ne change pas (voire diminue, la
   mesure froide étant bruitée par la soustraction du coût de flush — réserve déclarée).
   `FULL_KERNEL_MEMORY_SENSITIVITY=ABSENT`.
3. Conséquence : la question « le noyau complet est-il memory-bound ? » reçoit enfin une réponse
   **empirique et négative**. Elle reste `NOT_PROVEN` au sens strict du §11 (aucun compteur
   hardware), mais elle n'est plus seulement non prouvée : elle est **contredite par une expérience**.

```
FULL_KERNEL_MEMORY_SENSITIVITY=ABSENT
STREAM_MICROBENCH_IS_CACHE_RESIDENT=SUPPORTED_CONFIRMED_BY_CONTROL
MEMORY_BOUND=NOT_PROVEN_STRICT / EMPIRICALLY_CONTRADICTED
COMPUTE_BOUND=NOT_PROVEN
      -> l'insensibilite memoire POINTE vers une limite de calcul, mais un noyau peut aussi etre
         limite par la latence des dependances, par l'occupation, ou par le debit d'emission.
         Aucune de ces trois n'est departagee sans compteurs hardware.
```

**Réserve méthodologique déclarée** : les mesures froides intègrent une soustraction du coût de
flush, et les deux runs donnent 62,4 et 78,4 µs — une dispersion de 25 %. La conclusion « pas de
sensibilité mémoire » repose donc sur le fait que le froid **n'est pas plus lent** que le chaud
(ratio ≤ 1), pas sur une valeur fine. Si le noyau était limité par la DRAM, le froid serait
**plusieurs fois** plus lent (le streaming l'est, ×4). Il ne l'est pas.

## 6 — Ce que cela change pour la nature du coût

Trois mesures convergent maintenant :

| question | réponse | source |
|---|---|---|
| le volume de poids explique-t-il le coût ? | **non** (ratio 9,65) | 006F |
| le noyau est-il limité par la mémoire ? | **non, empiriquement** | 006G §5 |
| la chaîne de dépendance DP4A coûte-t-elle ? | **non** (gain nul) | 006G §3 |
| la réduction coûte-t-elle ? | **non** (5 shuffles, dominée) | 006G §4 |
| la machinerie générique est-elle un gros poste ? | **plafond ~5-6 %** | 006G §2 |
| le décodage ternaire coûte-t-il ? | 17,9 % du microbench, **V1 a échoué** | 006F / 006C-R2 |

Le portrait qui se dégage : le coût est dans **l'exécution de 5,24 millions de MAC par matmul à N=1**,
c'est-à-dire dans le travail arithmétique lui-même, sans gaspillage structurel identifiable par les
moyens dont je dispose. Et les 30 matrices `attn_gate` pèsent 5,24 M de MAC chacune — à N=1, ce
**rapport calcul/mémoire est intrinsèquement défavorable**, ce qui explique pourquoi le streaming
paraît bon marché et le calcul cher.

```
COST_IS_ARITHMETIC_WORK_ITSELF=SUPPORTED_BY_CONVERGENT_EVIDENCE
   (et non plus "work volume" au sens de volume de donnees)
LOCAL_MMVQ_HEADROOM=LOW_FOR_INSTRUCTION_SHAPING
   (quatre mecanismes de gaspillage local elimines par la mesure)
```

## 7 — Pourquoi je ne promus toujours pas COMPUTE_BOUND

Le §31 fixe le critère d'arrêt d'ENGINE-006 : soit G1+G2/G3 ne trouvent **aucun** headroom local
mesurable, soit une optimisation réussit. Nous sommes dans le premier cas **en apparence** — mais la
règle que tu as imposée dès le §11 de 006F interdit de convertir une absence de sensibilité en
classification matérielle :

```
MEMORY_BOUND=NOT_PROVEN         (jamais "NO")
COMPUTE_BOUND=NOT_PROVEN        (jamais "YES")
OCCUPANCY_LIMITED=NOT_PROVEN
ROOFLINE_CLASSIFICATION=NOT_AVAILABLE
```

Un noyau peut être limité par la **latence** d'émission, par l'**occupation**, par la **taille du
problème** (5120 blocs de 128 threads = 655 360 threads pour 48 SM, soit ~8,9 vagues — peu de
parallélisme par SM au regard de la latence DRAM), ou par une **limite de débit d'instruction**.
Ces hypothèses sont maintenant les seules vivantes, et aucune ne se départage sans compteurs.

Le candidat le plus intéressant que la mesure n'a pas encore touché : **la géométrie elle-même**.
`BLOCKS_PER_SM_LIMIT=12` par threads et par registres, soit 48 warps résidents — mais 5120 blocs
pour 48 SM font ~107 blocs par SM, traités en vagues. Une géométrie différente (plus de lignes par
bloc, moins de blocs) changerait le rapport travail/parallélisme **sans toucher une seule instruction
du corps**. C'est le seul levier restant que les quatre échecs n'ont pas invalidé.

## 8 — Provenance

```
Runtime baseline   : libllama 3db83675… / libggml-cuda abafbf0d…  (restaures, SHA verifies)
Arbre llama-obrain-cuda : les 4 fichiers du chemin chaud aux SHA vierges
Fichiers sales     : les 3 greffes connues, inchangees
MODIFICATIONS DE SOURCE DANS CETTE PASSE : AUCUNE
Instrument etendu   : qualification/e6f/e6f-lower-bound.cu (+ k_full_acc2, + controle hot/cold)
```

Défauts de harnais de cette passe :

```
HARNESS_DEFECT_24=FLUSH_HOT_COLD_APPELAIT_K_FULL_SUR_UN_BUFFER_PLAT
   -> "illegal memory access" ; le flush est desormais purement en lecture (k_stream)
HARNESS_DEFECT_25=MESURE_FROIDE_INTEGRE_LE_COUT_DE_FLUSH
   -> resolu par soustraction du flush mesure seul ; dispersion residuelle 25 % declaree
```

## 9 — Ledger

```
ENGINE006_STATUS=OPEN
ENGINE006C_R2_STATUS=CLOSED_V1_REJECTED
ENGINE006D_STATUS=CLOSED_WEIGHT_LOAD_VECTOR_REJECTED
ENGINE006E_STATUS=CLOSED_Q8_1_RESIDENCY_REJECTED
ENGINE006F_STATUS=CLOSED_LOWER_BOUND_HYPOTHESIS_REJECTED
ENGINE006G_STATUS=PARTIAL_G1_AUDITED_G2_REJECTED_G3_AUDITED
WEIGHT_VOLUME_DOMINATES_CACHE_HOT_KERNEL_COST=NO
COST_IS_FUNDAMENTALLY_WORK_VOLUME=WEAKENED_NOT_CONFIRMED
COST_IS_ARITHMETIC_WORK_ITSELF=SUPPORTED_BY_CONVERGENT_EVIDENCE
LOCAL_KERNEL_HEADROOM=MEASURED_LOW
GENERIC_MACHINERY_OVERHEAD=SUGGESTIVE_NOT_PROVEN
STREAM_MICROBENCH_IS_CACHE_RESIDENT=SUPPORTED_CONFIRMED_BY_CONTROL
STREAM_CACHE_SENSITIVITY=4.10_TO_4.37
FULL_CACHE_SENSITIVITY=0.78_TO_0.98
FULL_KERNEL_MEMORY_SENSITIVITY=ABSENT
MEMORY_BOUND=NOT_PROVEN_EMPIRICALLY_CONTRADICTED
COMPUTE_BOUND=NOT_PROVEN
OCCUPANCY_LIMITED=NOT_PROVEN
ENGINE006G_G1_STATUS=AUDITED_CEILING_ESTIMATED_LOW_PROTOTYPE_NOT_BUILT
G1_DYNAMIC_INSTRUCTION_DELTA_ESTIMATE=-40_TO_-50
G1_GAIN_CEILING_ON_FULL_KERNEL=5_TO_6_PERCENT_STATIC
ENGINE006G_G2_STATUS=REJECTED_BY_MEASUREMENT
DP4A_INSTRUCTION_COUNT=8
INDEPENDENT_ACCUMULATOR_COUNT=2_ALREADY_EMITTED_BY_COMPILER
MAX_DP4A_DEPENDENCY_CHAIN_LENGTH=8
G2_ACC2_GAIN_US=+0.1408_THEN_-0.0176
DP4A_DEPENDENCY_CHAIN_REDUCED=NO_MEASURABLE_BENEFIT
ENGINE006G_G3_STATUS=REDUCTION_AUDITED_DOMINATED
REDUCTION_SHFL_COUNT=5
REDUCTION_ADD_COUNT=5
REDUCTION_STAGE_COUNT=1
DP4A_DOMINATES=SUPPORTED
REDUCTION_DOMINATES=NO
LOCAL_MMVQ_HEADROOM=LOW_FOR_INSTRUCTION_SHAPING
NEXT_AXIS_CANDIDATE=KERNEL_GEOMETRY_AND_PARALLELISM_SHAPE_ONLY
ENGINE007_ALLOWED=NO
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
