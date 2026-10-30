# ENGINE-006D — FAISABILITÉ DE LA VECTORISATION DES CHARGEMENTS DE POIDS

Date : 2026-09-29. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e6d/`

---

## 0 — Réponse

**L'axe est fermé par la géométrie du format, et il n'aurait de toute façon rien rapporté.**

1. **VEC64 et VEC32 sont interdits par l'alignement.** Le format fait **34 octets** par bloc de 128
   poids (32 de codes + 2 d'échelle). 34 mod 4 = 2 et 34 mod 8 = 2 : sur les 48 blocs d'une ligne,
   les adresses de fetch 8 octets ne sont **jamais** 8-alignées et les adresses 4 octets ne sont
   4-alignées que pour **la moitié** des blocs. L'alignement garanti du domaine de production est
   **2 octets**. Seul VEC16 est licite.

2. **VEC16 est licite mais ne réduit pas le chemin complet.** Dans le noyau entier :

```
BASE  : LDG=19 (U8=8, U16=2)  LOP3=67  SHF=34  IADD/LEA=61  SEL=32  lignes=777
VEC16 : LDG=15 (U8=0, U16=6)  LOP3=71  SHF=38  IADD/LEA=61  SEL=32  lignes=777
TOTAL_RELEVANT_INSTRUCTION_DELTA = 0
```

Les 4 instructions de chargement gagnées sont **exactement compensées** par 4 LOP3 et 4 SHF
supplémentaires. Le §15 impose `FULL_KERNEL_RELEVANT_INSTRUCTION_COUNT_REDUCED=YES` :
**gate non franchi**, VEC16 ne passe pas au microbench.

```
ENGINE006D_STATUS=CLOSED_AXIS_NOT_FEASIBLE_AND_NOT_PROFITABLE
NEXT_AXIS_WEIGHT_LOAD_VECTORIZATION=REJECTED_AT_STATIC_GATE
NEXT_AXIS_SELECTED=NO
```

---

## 1 — Freeze V1 (§1)

```
ENGINE006C_R2_STATUS=CLOSED_V1_REJECTED_FOR_PRODUCT
V1_LOCAL_MICROBENCH=WIN
V1_FULL_KERNEL_STRENGTH_REDUCTION=FALSE
COMPILER_ALREADY_LOWERS_V0_COMPARISON_EFFICIENTLY=SUPPORTED_BY_SASS
V1_REPLACEMENT_INCREASES_STATIC_INSTRUCTION_COUNT=YES
STATIC_V1_GAIN_MS_PER_TOKEN=0.2748        STATIC_V1_GAIN_TO_NOISE=1.77
V1_PRODUCT_GAIN=NOT_MEASURABLE
```

Les 0,2748 ms ne sont **pas** attribués causalement au SASS : l'écart est sous le seuil de
reproductibilité, et le SASS ne fait que documenter que V1 alourdit le noyau.

## 2 — Adresse exacte reconstituée (§4)

Le fetch, lu dans la source (`mmvq.cu` + `vecdotq.cuh`) :

```cpp
// mmvq.cu : qi = QI_G128 = 4, vdr = 1, nwarps = 4, rows_per_block = 1
kqs  = vdr * (tid % (qi/vdr))     -> tid % 4
kbx  = tid / (qi/vdr)             -> tid / 4        (boucle : kbx += 32)
vec_dot_tq2_g128_q8_1(vx, y, kbx_offset + i*stride_row_x + kbx, kqs)
   qs = bq->qs + 8*iqs            avec iqs = kqs
   lecture de qs[0..7]
```

Décomposition additive de l'adresse de base du fetch de 8 octets :

| terme | valeur | multiple de |
|---|---|---|
| base du tenseur | allouée par le backend CUDA | ≥ 256 (cudaMalloc) |
| row_offset | `row * 1632` | 8, 16 |
| group_offset | `kbx * 34`, kbx ∈ [0,47] | **2 seulement** |
| thread_offset | `8 * (tid % 4)` | 8 |
| intra_group_offset | 0 (les 8 octets sont lus d'un bloc) | — |

Résidus calculés sur **tout** le domaine (5120 lignes × 48 blocs × 128 threads) :

```
nb1 = 48*34 = 1632     1632 mod 2=0   mod 4=0   mod 8=0   mod 16=0
34                     34   mod 2=0   mod 4=2   mod 8=2   mod 16=2
8*(tid%4)              8    mod 2=0   mod 4=0   mod 8=0
offsets de fetch mod 8 atteignables : [0, 2, 4, 6]
   -> 8-aligne pour 12 blocs sur 48 seulement
offsets de bloc mod 4 : [0, 2]      -> 4-alignement NON garanti
offsets de bloc mod 2 : [0]         -> 2-alignement GARANTI

MIN_GUARANTEED_ALIGNMENT_BYTES=2
```

**Le 8-alignement n'est pas garanti**, et il ne vient pas du pointeur de base : il vient des
**34 octets par bloc**. Aucune lecture 64 bits scalaire n'est donc licite ici.

## 3 — Gate de borne (§5)

```
LAST_VALID_BYTE_INDEX        = 8 355 840
MAX_8BYTE_FETCH_START        = 8 355 830   (derniere ligne 5119, bloc 47, tid%4=3 -> offset 24)
MAX_8BYTE_FETCH_LAST_BYTE    = 8 355 837
WIDE_LOAD_CROSSES_OBJECT_BOUNDARY (largeur 8) = NO    (marge restante : 3 octets)
```

La borne **n'est pas** le facteur limitant : c'est l'alignement. À noter que les 32 octets de `qs`
d'un bloc sont suivis immédiatement des 2 octets d'échelle `d` du même bloc : un fetch qui
déborderait de 2 octets mordrait sur l'échelle suivante — la marge de 3 octets ne protège que la
toute dernière ligne, pas les frontières internes. C'est une seconde raison de ne pas élargir sans
preuve.

## 4 — Trois candidats, un seul licite (§6, §7)

| variante | largeur | alignement requis | garanti sur tout le domaine | verdict |
|---|---|---|---|---|
| **VEC16** | 2 × 4 octets → 4 × u16 | 2 | **OUI** | licite |
| VEC32 | 8 octets → 2 × u32 | 4 | NON (34 mod 4 = 2) | **interdit** |
| VEC64 | 8 octets → 1 × u64 | 8 | NON (34 mod 8 = 2) | **interdit** |

Conformément au §8, le mapping octet-exact utilisé pour VEC16 est explicite et little-endian :

```
octet 2j   = (w[j] >> 0) & 0xFF
octet 2j+1 = (w[j] >> 8) & 0xFF        pour j = 0..3
```

## 5 — Test de largeurs : microbench (§10, §14)

Trois formulations compilées séparément, mêmes flags, microbench sur 1 M de tranches de 8 octets :

| variante | LDG | LOP3 | SHF | lignes SASS | ms (20 kernels) |
|---|---|---|---|---|---|
| A — 8 × u8 (actuelle) | 8 (U8) | 64 | 24 | 545 | 0,4935 |
| **B — 4 × u16** | **4 (U16)** | 72 | 28 | 561 | **0,4929** |
| C — tranche de 8 octets | 4 (U16) | 64 | 24 | 561 | 0,4925 |

```
LOAD_INSTRUCTION_COUNT_REDUCED_A_TO_B=8_TO_4      (MEASURED_FROM_SASS)
MICROBENCH_RUNTIME_DELTA=0,1_PERCENT              (dans le bruit)
GLOBAL_MEMORY_TRANSACTION_COUNT_FROM_8_U8_LOADS=NOT_MEASURED
MEMORY_TRANSACTIONS_REDUCED=NON_MESURE              (conforme au §10)
```

Diviser par deux les instructions de chargement **ne change rien au temps d'exécution** dans ce
microbench — signe que ce noyau n'est pas limité par les instructions de chargement.

**Deux pièges évités**, signalés comme tels :

- Le premier jet ne compilait pas (`kern<V>` avec `V` variable de boucle) : les trois instanciations
  sont maintenant explicites. Aucune mesure n'a été tirée d'un binaire absent.
- Les empreintes SASS initiales étaient **vides** parce que le nom mangled recherché était faux :
  je ne les ai pas interprétées comme un résultat.

## 6 — VEC16 dans le noyau complet (§14, §15)

Patch déclaré (une fonction, 8 lectures u8 → 4 lectures u16), compilé dans `build-r2-v1`, SASS du
noyau exact `mul_mat_vec_q<tq2_g128, 1, false, false>` :

| | LDG | U8 | U16 | LOP3 | SHF | IADD/LEA | SEL | lignes |
|---|---|---|---|---|---|---|---|---|
| **BASE** | 19 | 8 | 2 | 67 | 34 | 61 | 32 | **777** |
| **VEC16** | **15** | 0 | 6 | 71 | 38 | 61 | 32 | **777** |

```
TOTAL_RELEVANT_INSTRUCTION_DELTA=0
FULL_KERNEL_RELEVANT_INSTRUCTION_COUNT_REDUCED=NO
```

Le §14 l'avait anticipé mot pour mot : *« Si la lecture large réduit les LDG mais ajoute autant de
PRMT/SHF/LOP3, elle est rejetée statiquement. »* Ici ce sont 4 LOP3 et 4 SHF — le compte est
exactement nul. **VEC16 est rejetée au gate statique, sans microbench produit.**

## 7 — Portée de la modification (§18)

Comme pour V1, le point de modification est `vec_dot_tq2_g128_q8_1`, donc :

```
IMPLEMENTATION_SCOPE=ALL_TQ2_G128_MMVQ (168 poids, 6 roles)
   attn_gate 36 | ffn_gate 48 | ffn_up 48 | attn_q 12 | attn_k 12 | attn_output 12
NON_TARGET_KERNEL_BEHAVIOUR_UNCHANGED=YES
```

## 8 — Ce que cette passe établit

```
MIN_GUARANTEED_ALIGNMENT_BYTES=2
WIDE_LOAD_64BIT_ALIGNMENT_SAFE=NO
WIDE_LOAD_32BIT_ALIGNMENT_SAFE=NO
ONLY_LEGAL_WIDTH=16_BIT
8_SCALAR_U8_LOADS_ARE_THE_MAXIMUM_LEGAL_WIDTH=SUPPORTED
   (l'alignement garanti vaut 2 : huit lectures d'un octet sont la forme la plus large qui soit
    licite sans changer le format)
SCALAR_WEIGHT_LOAD_INSTRUCTION_COUNT_IS_LAYOUT_FORCED=SUPPORTED
COMPILER_ALREADY_COALESCES=NOT_PROVEN   (aucune metrique hardware)
MEMORY_BOUND=NOT_PROVEN
```

**Conséquence pour la suite** : les « 8 × `LDG.E.U8` » de 006B ne sont **pas** une inefficacité du
code mais une **propriété du format de quantification** (34 octets par bloc, donc alignement 2). Les
réduire exigerait de changer le format — ce qui sort du périmètre d'ENGINE-006, et invaliderait le
modèle à 2,125 bpw.

## 9 — Suite (§22)

Le §22 prévoit exactement ce cas :

```
SCALAR_WEIGHT_LOAD_FORM_NOT_A_PRODUCT_BOTTLENECK=SUPPORTED
NEXT_AXIS_WEIGHT_LOAD_VECTORIZATION=REJECTED_AT_STATIC_GATE
NEXT_AXIS_SELECTED=NO
```

Passer au candidat statique suivant **sans insister sur les loads**. Les deux axes ouverts restants
de la table 006B, honnêtement qualifiés :

1. **branches runtime génériques** — classé `NO` en 006B (les paramètres `ncols_dst`, `has_fusion`,
   `use_gate`, `small_k` sont déjà `constexpr`). Reste non testé : l'effet d'**une spécialisation
   complète** sur l'allocation de registres et l'ordonnancement — c'est précisément ce que R2 a
   montré pouvoir coûter cher dans l'autre sens (le code ajouté a alourdi le noyau).
2. **forme des chargements de l'activation q8_1** — non auditée à ce jour. Contrairement aux poids,
   l'activation est **réutilisée 5120 fois** et sa mise en cache (L1/smem) pourrait être un levier
   structurel : le vecteur q8_1 de 48 blocs × 36 octets = 1728 octets tient en smem.
   `ACTIVATION_RESIDENCY_STRATEGY=AUDIT_NOT_DONE`.

Aucun de ces deux n'est sélectionné. Le §17 de 006C-R2 reste la règle : `NEXT_AXIS_SELECTED=NO`
jusqu'à ce qu'un audit statique désigne un candidat avec un delta d'instructions **négatif** mesuré.

## 10 — Build-parity issue (§23) : maintenue visible

```
BUILD_REPRODUCTION_PERFORMANCE_PARITY=NOT_ESTABLISHED
PRIMARY_CAUSAL_COMPARISON_006D=BASE_REBUILD_vs_VECTORIZED_REBUILD
HISTORICAL_FREEZE_AS_MAIN_CONTROL=NO
```

Le problème reste ouvert et documenté. Il n'a pas affecté cette passe, qui se juge sur le **SASS** et
non sur le wall clock : le delta d'instructions nul est une propriété du code compilé, pas d'une
campagne de mesure.

## 11 — Provenance

```
Runtime baseline   : libllama 3db83675… / libggml-cuda abafbf0d…  (restaures, SHA verifies)
Arbre llama-obrain-cuda : les 4 fichiers du chemin chaud aux SHA vierges
Fichiers sales     : les 3 greffes connues, inchangees
Builds             : build-r2-base (BASE), build-r2-v1 (VEC16) — parite de configuration
Patchs             : apply-vec16.py (declare), restaure en fin de passe
Artefacts          : qualification/e6d/{e6d-widths.cu, apply-vec16.py, e6d-full-vec16.txt}
AUCUNE CAMPAGNE PRODUIT lancee : le gate statique a tranche avant.
```

Défauts de harnais de cette passe :

```
HARNESS_DEFECT_20=TEMPLATE_PARAMETER_NON_CONSTANT_IN_LAUNCH        (corrige, aucune mesure tiree)
HARNESS_DEFECT_21=EMPTY_SASS_FROM_WRONG_MANGLED_NAME               (corrige, non interprete)
```

## 12 — Ledger

```
ENGINE006_STATUS=OPEN
ENGINE006C_R2_STATUS=CLOSED_V1_REJECTED_FOR_PRODUCT
ENGINE006_V1_STATUS=REJECTED
V1_FULL_KERNEL_STATIC_INSTRUCTION_REDUCTION=NO
NEXT_AXIS=TQ2_G128_WEIGHT_LOAD_VECTORIZATION
ENGINE006D_STATUS=CLOSED_AXIS_NOT_FEASIBLE_AND_NOT_PROFITABLE
NEXT_AXIS_WEIGHT_LOAD_VECTORIZATION=REJECTED_AT_STATIC_GATE
NEXT_AXIS_SELECTED=NO
ALIGNMENT_AT_EVERY_ACCESS=MEASURED
CROSS_BOUNDARY_SAFETY=MEASURED_NO_CROSSING_AT_WIDTH_8
ENDIANNESS_MAPPING=DEFINED_LITTLE_ENDIAN_OCTET_EXACT
MIN_GUARANTEED_ALIGNMENT_BYTES=2
WIDE_LOAD_64BIT_ALIGNMENT_SAFE=NO
WIDE_LOAD_32BIT_ALIGNMENT_SAFE=NO
ONLY_LEGAL_WIDTH=16_BIT
VEC16_LOAD_INSTRUCTIONS=8_TO_4
VEC16_FULL_KERNEL_TOTAL_INSTRUCTION_DELTA=0
VEC16_LOP3_DELTA=+4
VEC16_SHF_DELTA=+4
FULL_KERNEL_RELEVANT_INSTRUCTION_COUNT_REDUCED=NO
TERNARY_DECODE_FIDELITY_NOT_REACHED=GATE_STATIQUE_AVANT_MICROBENCH
SCALAR_WEIGHT_LOAD_FORM_NOT_A_PRODUCT_BOTTLENECK=SUPPORTED
SCALAR_WEIGHT_LOAD_INSTRUCTION_COUNT_IS_LAYOUT_FORCED=SUPPORTED
GLOBAL_MEMORY_TRANSACTION_COUNT_FROM_8_U8_LOADS=NOT_MEASURED
MEMORY_BOUND=NOT_PROVEN
COMPUTE_BOUND=NOT_PROVEN
OCCUPANCY_LIMITED=NOT_PROVEN
BUILD_REPRODUCTION_PERFORMANCE_PARITY=NOT_ESTABLISHED
NEXT_CANDIDATE_AUDIT=ACTIVATION_Q8_1_RESIDENCY_OR_FULL_SPECIALIZATION
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
