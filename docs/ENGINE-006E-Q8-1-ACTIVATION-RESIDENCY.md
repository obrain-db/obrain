# ENGINE-006E — RÉSIDENCE DE L'ACTIVATION Q8_1 : AUDIT STATIQUE

Date : 2026-09-29. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : aucun nouveau binaire — audit dérivé de la source et du SASS de `qualification/e6b/`.

---

## 0 — Réponse

**L'axe est fermé sur le gate de redondance, avant toute implémentation.**

```
INTRA_BLOCK_Q8_1_LOAD_REDUNDANCY=YES        (32x en apparence)
INTRA_BLOCK_ACTIVATION_DUPLICATION=NO       (mais ZERO duplication entre warps)
POTENTIAL_LOAD_INSTRUCTION_REDUCTION_FROM_SHARED=0
Q8_1_SHARED_STAGING_AXIS=REJECTED_STATICALLY
```

Le point qui tranche, et je ne l'attendais pas sous cette forme : les **quatre warps d'un bloc
consomment des tranches d'activation DISJOINTES**. La redondance de 32× qui existe est
**intra-warp**, et le matériel la coalesce déjà. Un staging en mémoire partagée ne peut mutualiser
que de l'`INTRA_BLOCK_REUSE` **entre warps** — qui vaut exactement zéro ici.

```
ENGINE006E_STATUS=CLOSED_AXIS_REJECTED_AT_REDUNDANCY_GATE
NEXT_AXIS_Q8_1_ACTIVATION_RESIDENCY=REJECTED_AT_STATIC_GATE
NEXT_AXIS_SELECTED=NO
```

---

## 1 — Freeze 006D (§1)

```
ENGINE006D_STATUS=CLOSED_WEIGHT_LOAD_VECTORIZATION_REJECTED
MIN_GUARANTEED_WEIGHT_ALIGNMENT_BYTES=2
VEC64_ALIGNMENT_SAFE=NO      VEC32_ALIGNMENT_SAFE=NO      VEC16_ALIGNMENT_SAFE=YES
VEC16_FULL_KERNEL_RELEVANT_INSTRUCTION_DELTA=0
VEC16_MICROBENCH_GAIN=NOT_DETECTED
WEIGHT_LOAD_VECTORIZATION_PRODUCT_CAMPAIGN=NOT_RUN
CURRENT_WEIGHT_LOAD_FORM_IS_LOCALLY_PREFERRED_UNDER_EXISTING_34_BYTE_LAYOUT=SUPPORTED
U8_IS_THE_WIDEST_LEGAL_LOAD=NON_PUBLIE        (U16 est licite, conformement a la consigne)
```

## 2 — Qui traite quoi (§5) : le fait qui décide

Géométrie exacte, lue dans la source (`mmvq.cu` : `qi = QI_G128 = 4`, `vdr = 1`, `nwarps = 4`,
`rows_per_block = 1` ; `qk = 128`, `QK8_1 = 32`) :

```
kqs  = vdr * (tid % (qi/vdr))  = tid % 4        -> 4 tranches distinctes de 32 octets
kbx0 = tid / (qi/vdr)          = tid / 4        -> 0..31
blocks_per_iter = vdr * nwarps*warp_size / qi = 32
boucle : kbx, kbx+32  ->  kbx < 48  =>  48 blocs couverts
```

Consommation par warp (calculée sur les 128 threads) :

| warp | blocs kbx consommés | nb |
|---|---|---|
| 0 | 0–7 puis 32–39 | 16 |
| 1 | 8–15 puis 40–47 | 16 |
| 2 | 16–23 | 8 |
| 3 | 24–31 | 8 |

```
UNION_DES_BLOCS_CONSOMMES=48   (les 48 de la ligne : couverture complete)
INTERSECTION_ENTRE_WARPS=VIDE
INTRA_BLOCK_ACTIVATION_DUPLICATION=NO
```

Chaque warp lit **sa propre** partition de l'activation ; aucun octet n'est demandé par deux warps
du même bloc.

## 3 — Comptes demandés (§3)

```
Q8_1_UNIQUE_BYTES_PER_BLOCK  = 48 blocs x 36 octets = 1 728
Q8_1_UNIQUE_BYTES_PER_WARP   = 12 blocs x 36 octets =   432
Q8_1_LOADS_PER_OUTPUT_ROW    = 128 threads x 12 kbx  = 1 536 acces de tranche
Q8_1_LOADS_PER_WARP          =  32 threads x 12 kbx  =   384
Q8_1_LOADS_PER_BLOCK         = 128 threads x 12 kbx  = 1 536
octets demandes par le bloc  = 128 x 12 x 36 = 55 296
Q8_1_STATIC_REUSE_FACTOR (bloc) = 55 296 / 1 728 = 32 x
Q8_1_STATIC_REUSE_FACTOR (warp) = 13 824 /   432 = 32 x
```

`Q8_1_STATIC_REUSE_FACTOR` est bien un **facteur logique** de demandes, pas une estimation de trafic
DRAM (§3, §7).

## 4 — Niveaux de réutilisation (§4) : le partage qui compte

| niveau | facteur | qui le résout déjà |
|---|---|---|
| `INTRA_THREAD_REUSE` | 1 | rien à faire : chaque thread lit sa tranche une fois |
| **`INTRA_WARP_REUSE`** | **32 ×** | **le matériel (coalescence des 32 lanes)** |
| `INTRA_BLOCK_REUSE` | 32 × **total, mais 0 entre warps** | les warps sont déjà disjoints |
| `INTER_BLOCK_REUSE` | 5 120 blocs × 128 threads sur 1 728 octets | L1/L2 |

Le shared memory ne peut mutualiser que de l'`INTRA_BLOCK_REUSE` **entre warps**. Ici cette
quantité est **nulle**. Les 32× qui existent sont intra-warp et déjà servis par la coalescence
matérielle : un staging ne les supprimerait pas, il **ajouterait** des `STS`/`LDS`/`BAR` par-dessus
le même accès global.

## 5 — Attribution des chargements (§6)

Comptage **attribué**, pas global, sur le noyau exact (SASS de `e6b-sass.txt`) :

```
LDG.E.U8.CONSTANT    x 8    poids tq2_g128 (8 octets par thread)
LDG.E.CONSTANT       x 8    activation q8_1 (qs, 8 mots de 32 bits = 32 octets)
LDG.E.U16.CONSTANT   x 2    echelle fp16 du poids (bq->d)
LDG.E                x 1    (sortie / divers)
LDC/LDCU             x 20   parametres de noyau (lectures constantes, pas des donnees)

Q8_1_LDG_INSTRUCTION_COUNT=8
Q8_1_LOAD_WIDTHS=32_BITS (8 x LDG.E)
Q8_1_BYTES_PER_THREAD_PER_BLOCK=32 (plus le champ ds, atteint par les offsets +-0x10 du meme motif)
```

Forme des adresses observées : trois bases (`R32`, `R20`, `R30`) avec des offsets
`−0x10, −0x8, −0x6, −0x7, −0x5, −0x4, −0x4, −0x3, −0x2, −0x1, 0, 0x4, 0x8, 0xc, 0x10, 0x20` — le
compilateur a **hisssé** les huit lectures d'une tranche hors de la boucle, ce qui est la raison
mécanique pour laquelle les 8 instructions servent les 12 blocs kbx et non 96.

```
REDUNDANT_LOAD_INSTRUCTIONS_EXIST=NO       (au sens des instructions : une seule serie de 8 par thread)
DRAM_BYTES_DUPLICATED=NOT_PROVEN           (conforme au §7 : aucune metrique de cache disponible)
```

C'est le point clé que le §3 espérait : il n'y a **rien à réduire** du côté des instructions de
chargement d'activation — elles ont déjà été hissées et servent toute la boucle.

## 6 — Faisabilité du staging partagé (§8, §9)

```
SHARED_BYTES_PER_BLOCK_REQUIRED = 1 728 octets (deja multiple de 16, aucun padding necessaire)
CURRENT_STATIC_SHARED_MEMORY    =   384 octets (tmp_shared pour la reduction inter-warp, ptxas 006B)
PROPOSED_TOTAL_SHARED_MEMORY_PER_BLOCK = 1 728 octets
   (les 384 octets de tmp_shared restent necessaires : la reduction inter-warp ne disparait pas)
```

Occupancy, avec les limites GB10 mesurées (48 SM ; 65 536 registres/SM ; 1 536 threads/SM ;
102 400 octets de smem/SM ; 42 registres/thread) :

```
BLOCKS_PER_SM_LIMIT_FROM_THREADS          = 1536 / 128        = 12
BLOCKS_PER_SM_LIMIT_FROM_REGISTERS        = 65536 / (128*42)  = 12
BLOCKS_PER_SM_LIMIT_FROM_SHARED_MEMORY    = 102400 / 384      = 266   (avant)
                                          = 102400 / 1728     =  59   (apres)

THEORETICAL_ACTIVE_BLOCK_LIMIT_BEFORE = 12
THEORETICAL_ACTIVE_BLOCK_LIMIT_AFTER  = 12
SHARED_MEMORY_OCCUPANCY_REGRESSION=NO
```

L'occupation **ne régresserait pas** (le facteur limitant reste les threads). Ce n'est donc pas
l'occupation qui condamne l'idée — c'est le §11 : il n'y a aucune redondance inter-warp à
mutualiser.

## 7 — Verdict et gates franchis / non franchis (§10 à §13)

```
S1 (staging complet de l activation)   : NON PROTOTYPE
S2 (staging par tuile)                 : NON PROTOTYPE
COOPERATIVE_LOAD_INSTRUCTIONS          : non estimable utilement (aucun gain attendu)
SHARED_STORE_INSTRUCTIONS              : +>
SHARED_LOAD_INSTRUCTIONS               : +>
BARRIER_COUNT                          : +1 minimum par tuile
ADDED_ADDRESS_ARITHMETIC               : +>
FULL_KERNEL_RELEVANT_INSTRUCTION_PATH_IMPROVED=NO_PAR_CONSTRUCTION
BASE_REGISTERS=42   CANDIDATE_REGISTERS=NON_MESURE (pas de candidat)
BASE_SPILLS=0       CANDIDATE_SPILLS=NON_MESURE
```

Le §11 avait prévu exactement ce cas : *« Si chaque activation byte n'est déjà chargé qu'une fois
par bloc : `Q8_1_SHARED_STAGING_AXIS=REJECTED_STATICALLY` »*. Le compte par warp montre même mieux :
chaque octet est chargé une fois **par warp**, et les warps ne se recouvrent pas.

Aucune campagne microbench, aucun prototype, aucun build produit n'a été lancé — les gates
statiques ont tranché.

## 8 — Audit de la spécialisation complète (§20, §21)

Le §22 demandait cet audit en parallèle. Relevé sur le SASS du noyau exact :

```
branches runtime dans le corps          : 3 BRA (boucle kbx, garde finale, sortie)
conditionnels resolus a la compilation  : has_fusion, use_gate, ncols_dst, small_k  (deja constexpr)
bounds checks                           : 1 (garde d ecriture finale)
tail handling                           : aucun (TAIL_WASTE=0, mesure 006B)
switch qtype                            : resolu au dispatch, hors noyau
address arithmetic dynamique            : oui (strides, fastdiv/fastmodulo des canaux) -- facteurs
                                          invariants par appel mais pas par compilation
instructions de parametres              : 20 LDC/LDCU
```

Réserves honnêtes :

- Les 20 `LDC/LDCU` sont des lectures de paramètres de noyau : une spécialisation les remplacerait
  par des constantes immédiates. Gain **borné à ~20 instructions sur 777**, soit ~2,6 %.
- Les `fastdiv`/`fastmodulo` des canaux sont recalculés alors que leurs facteurs sont invariants par
  appel. Gain non chiffré.
- Mais la mesure R2 a montré que **modifier la forme du code peut coûter plus que le gain
  d'instructions** (+40 instructions pour V1, et un effet produit non distinguable du bruit).

```
FULL_TQ2_G128_N1_SPECIALIZATION=AUDITED_NO_NEGATIVE_DELTA_DEMONSTRATED
NEXT_AXIS_SPECIALIZATION_SELECTED=NO
```

## 9 — Où en est le compte des axes

| axe | statut |
|---|---|
| `TQ2_G128_DECODE_STRENGTH_REDUCTION` (V1) | **REJETÉ** — produit non mesurable, SASS +40 instructions |
| `TQ2_G128_WEIGHT_LOAD_VECTORIZATION` | **REJETÉ** — alignement 2 octets, delta d'instructions nul |
| `Q8_1_ACTIVATION_RESIDENCY_AND_REUSE` | **REJETÉ** — zéro redondance inter-warp à mutualiser |
| `FULL_TQ2_G128_N1_SPECIALIZATION` | **AUDITÉ, non sélectionné** — aucun delta négatif démontré |

Trois axes fermés sur des mesures, aucun ouvert. Le constat qui se dégage, et qu'il faut regarder en
face : **le noyau n'a pas d'inefficacité locale évidente**. Chacun des trois candidats s'est révélé
soit forcé par le format (largeurs de chargement), soit déjà optimalement hissé (chargements), soit
sans redondance exploitable (activation). Le §26 de 006B est en train de se confirmer :

```
RELATIVE_OP_DOMINANCE != CRITICAL_PATH_SENSITIVITY
```

`attn_gate` représente 7 % du temps MUL_MAT relatif et sa sensibilité murale à la duplication est
mesurée (1,18 ms/repeat) — mais cela ne signifie pas que son code contient un gaspillage récupérable.
Les trois audits convergent vers : **le coût est dans le volume de travail, pas dans sa forme.**

## 10 — Provenance

```
Runtime baseline   : libllama 3db83675… / libggml-cuda abafbf0d…  (restaures, SHA verifies)
Arbre llama-obrain-cuda : les 4 fichiers du chemin chaud aux SHA vierges
Fichiers sales     : les 3 greffes connues, inchangees
MODIFICATIONS DE SOURCE DANS CETTE PASSE : AUCUNE
Sources de l audit : mmvq.cu, vecdotq.cuh, ggml-common.h (lecture), SASS de e6b-sass.txt
AUCUN BUILD, AUCUN PROTOTYPE, AUCUNE CAMPAGNE
```

## 11 — Ledger

```
ENGINE006_STATUS=OPEN
ENGINE006C_R2_STATUS=CLOSED_V1_REJECTED_FOR_PRODUCT
ENGINE006D_STATUS=CLOSED_WEIGHT_LOAD_VECTORIZATION_REJECTED
ENGINE006E_STATUS=CLOSED_AXIS_REJECTED_AT_REDUNDANCY_GATE
NEXT_AXIS=Q8_1_ACTIVATION_RESIDENCY_AND_REUSE
NEXT_AXIS_Q8_1_ACTIVATION_RESIDENCY=REJECTED_AT_STATIC_GATE
NEXT_AXIS_SELECTED=NO
Q8_1_UNIQUE_BYTES_PER_BLOCK=1728
Q8_1_UNIQUE_BYTES_PER_WARP=432
Q8_1_LOADS_PER_OUTPUT_ROW=1536
Q8_1_LOADS_PER_WARP=384
Q8_1_LOADS_PER_BLOCK=1536
Q8_1_STATIC_REUSE_FACTOR=32
INTRA_THREAD_REUSE=1
INTRA_WARP_REUSE=32
INTRA_BLOCK_REUSE=TOTAL_32_BUT_ZERO_BETWEEN_WARPS
INTER_BLOCK_REUSE=5120_BLOCKS_ON_1728_BYTES
INTRA_BLOCK_ACTIVATION_DUPLICATION=NO
WARPS_CONSUME_DISJOINT_ACTIVATION_PARTITIONS=YES
INTRA_BLOCK_Q8_1_LOAD_REDUNDANCY=NO_ACTIONABLE
POTENTIAL_LOAD_INSTRUCTION_REDUCTION_FROM_SHARED=0
Q8_1_SHARED_STAGING_AXIS=REJECTED_STATICALLY
Q8_1_LDG_INSTRUCTION_COUNT=8
Q8_1_LOAD_WIDTHS=32_BITS
Q8_1_ADDRESS_GENERATION=HOISTED_OUT_OF_THE_KBX_LOOP
REDUNDANT_LOAD_INSTRUCTIONS_EXIST=NO
DRAM_BYTES_DUPLICATED=NOT_PROVEN
SHARED_BYTES_PER_BLOCK_REQUIRED=1728
CURRENT_STATIC_SHARED_MEMORY=384
PROPOSED_TOTAL_SHARED_MEMORY_PER_BLOCK=1728
BLOCKS_PER_SM_LIMIT_FROM_THREADS=12
BLOCKS_PER_SM_LIMIT_FROM_REGISTERS=12
BLOCKS_PER_SM_LIMIT_FROM_SMEM_BEFORE=266
BLOCKS_PER_SM_LIMIT_FROM_SMEM_AFTER=59
THEORETICAL_ACTIVE_BLOCK_LIMIT_BEFORE=12
THEORETICAL_ACTIVE_BLOCK_LIMIT_AFTER=12
SHARED_MEMORY_OCCUPANCY_REGRESSION=NO
FULL_TQ2_G128_N1_SPECIALIZATION=AUDITED_NO_NEGATIVE_DELTA_DEMONSTRATED
GENERIC_PARAMETER_LOADS=LDC_LDCU_COUNT_20
MEMORY_BOUND=NOT_PROVEN
COMPUTE_BOUND=NOT_PROVEN
OCCUPANCY_LIMITED=NOT_PROVEN
RELATIVE_OP_DOMINANCE_DOES_NOT_IMPLY_RECOVERABLE_WASTE=SUPPORTED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
