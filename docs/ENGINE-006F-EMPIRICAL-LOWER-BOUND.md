# ENGINE-006F — PLANCHER EMPIRIQUE DE STREAMING : RÉSULTAT

Date : 2026-09-29. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e6f/`

---

## 0 — Réponse

**Le volume de poids n'est PAS le plancher de ce noyau.**

```
STREAM_US=8,28        (tous les octets de poids lus, aucun decodage)
DECODE_US=22,60       (STREAM + decodage ternaire exact)
FULL_MMVQ_US=79,94    (calcul MMVQ complet de production)

FULL_OVER_STREAM_RATIO=9,65
STREAM_EFFECTIVE_GB_S=1008,6
FULL_EFFECTIVE_GB_S=104,5
```

Lire tout le volume de poids du target prend **8,3 µs**. Le noyau complet en prend **79,9 µs**, soit
**9,65 fois plus**. Le décodage y ajoute 14,3 µs et le reste du calcul 57,3 µs.

```
WEIGHT_VOLUME_DOMINATES_LOWER_BOUND=NO
LOCAL_COMPUTE_OR_DECODE_HEADROOM_REMAINS=SUPPORTED
CAS=B  (le cas ou FULL est tres au-dessus de STREAM)
```

**Conséquence directe : la thèse « COST_IS_FUNDAMENTALLY_WORK_VOLUME » est AFFAIBLIE, pas
confirmée.** Le §12 prévoyait de fermer la ligne d'optimisation locale si le volume était confirmé.
Il ne l'est pas.

---

## 1 — Freeze 006E (§1, §2)

```
ENGINE006E_STATUS=CLOSED_Q8_1_RESIDENCY_REJECTED
INTRA_BLOCK_ACTIVATION_DUPLICATION=NO      INTER_WARP_Q8_1_OVERLAP=0
Q8_1_ADDRESS_GENERATION=HOISTED_OUT_OF_THE_KBX_LOOP
REDUNDANT_Q8_1_LOAD_INSTRUCTIONS_EXIST=NO
Q8_1_SHARED_STAGING_AXIS=REJECTED_STATICALLY
FULL_TQ2_G128_N1_SPECIALIZATION=AUDITED_NO_NEGATIVE_DELTA_DEMONSTRATED

TERNARY_STRENGTH_REDUCTION=REJECTED_PRODUCT
WEIGHT_LOAD_VECTORIZATION=REJECTED_STATIC_GATE
Q8_1_SHARED_RESIDENCY=REJECTED_STATIC_GATE
FULL_SPECIALIZATION=NO_NEGATIVE_SASS_DELTA_DEMONSTRATED

RELATIVE_OP_DOMINANCE_DOES_NOT_IMPLY_RECOVERABLE_LOCAL_WASTE=SUPPORTED
COST_IS_FUNDAMENTALLY_WORK_VOLUME=WEAKENED_BY_006F  (etait STRONGLY_SUPPORTED_NOT_YET_PROVEN)
```

## 2 — Dispositif (§4 à §7)

Trois kernels **séparément compilés**, mêmes lectures de poids, géométrie de production exacte
(5120 lignes × 48 blocs × 128 threads, 34 octets par bloc, aucun buffer synthétique) :

| kernel | contenu |
|---|---|
| **S0 STREAM** | lit les 8 octets de sa tranche par bloc, les combine par XOR, écrit un sink observable. Aucun décodage, aucun `dp4a`, aucune réduction de ligne |
| **S1 DECODE** | S0 + décodage ternaire exact, agrégé, sans produit scalaire q8_1 |
| **S2 FULL** | calcul MMVQ complet (décodage + `dp4a` + échelle + réduction warp) |

```
TARGET_WEIGHT_BYTES_READ=8 355 840   (layout physique reel, 34 octets par bloc)
TARGET_Q8_1_BYTES_READ=1 728
consumation du sink : verifiee (le noyau ne peut pas etre elimine) ; compilation et execution sans
rejet du compilateur
```

## 3 — Contrôle croisé remarquable : l'alignement 006D est confirmé à l'exécution

Premier jet de S0 en `uint32_t` :

```
CUDA_FAIL sync warm : misaligned address
```

C'est **exactement** la contrainte démontrée analytiquement en 006D : l'adresse vaut
`base + 34·kbx + 8·kqs`, et 34 mod 4 = 2, donc pour `kbx` impair l'adresse est 2 mod 4. Les lectures
32 bits sont donc illicites, comme annoncé. S0 est passé en `uint16` (4 lectures), seul largeur
garantie.

```
ALIGNMENT_CONSTRAINT_006D_CONFIRMED_AT_RUNTIME=YES
```

C'est une validation indépendante de l'audit 006D : la contrainte n'était pas théorique.

## 4 — Mesures (§8)

Deux exécutions, 20 kernels par mesure, warmup préalable :

| | exécution 1 | exécution 2 |
|---|---|---|
| `STREAM_US` | 8,2848 | 8,2496 |
| `DECODE_US` | 22,6000 | 22,5760 |
| `FULL_MMVQ_US` | 79,9360 | 79,8464 |
| `FULL_OVER_STREAM_RATIO` | 9,649 | 9,679 |

Reproductibilité à 0,4 % près. Décomposition :

```
DECODE_INCREMENT_US        = 14,3152    (STREAM -> DECODE : le decodage ternaire)
DOT_REDUCTION_INCREMENT_US = 57,3360    (DECODE -> FULL : dot-product + echelle + reduction)

part du decodage          : 14,32 / 79,94 = 17,9 %
part du dot-product+reduc : 57,34 / 79,94 = 71,7 %
part du streaming pur     :  8,28 / 79,94 = 10,4 %
```

Ce ne sont pas nécessairement des composantes additives du noyau produit (unités de code
différentes), mais l'ordre de grandeur est sans ambiguïté.

## 5 — Bande passante effective (§9)

Comparaison à un plafond mesuré **sur le même GPU**, pas à une valeur marketing :

```
LOCAL_DEVICE_STREAMING_CEILING_GB_S=255,8   (lecture pure, 512 Mio x10, uint4, 1024 blocs)
TARGET_STREAM_EFFECTIVE_GB_S=1008,6          (S0)
STREAM_TO_DEVICE_CEILING_RATIO=3,94
FULL_EFFECTIVE_GB_S=104,5
FULL_TO_DEVICE_CEILING_RATIO=0,41
```

Deux lectures importantes, et il faut les donner toutes les deux :

- **S0 dépasse le plafond DRAM d'un facteur 3,9** — donc les 8,36 Mo du tenseur sont **servis par le
  cache** (L2 = 25,2 Mo, mesuré en 006B), pas par la DRAM. Le streaming n'est pas seulement « déjà
  bon » : il est **plus rapide que la DRAM**.
- **FULL tourne à 104,5 Go/s, soit 41 % du plafond DRAM** — donc le noyau complet **n'atteint pas**
  la bande passante mémoire disponible. Il n'est pas limité par le débit mémoire.

```
ROOFLINE_CLASSIFICATION=NOT_AVAILABLE      (aucun compteur hardware, conformement au §11)
EMPIRICAL_STREAMING_LOWER_BOUND_COMPARISON=OBTAINED
MEMORY_BOUND=NOT_PROVEN                    (mais desormais fortement AFFAIBLI)
```

## 6 — Un écart supplémentaire, signalé sans être surinterprété

Comparaison du microbench `S2 FULL` avec la mesure **produit** de 006A :

```
S2_FULL_MICROBENCH_US = 79,94
PRODUCTION_INFERRED_US = 131,0    (1,1794 ms/token de sensibilite, 9 sequences cible par jeton)
RATIO_MICROBENCH_OVER_PRODUCTION = 1,64
```

Le `S2` de ce microbench est une boucle interne simplifiée : il ne contient ni `fastdiv`/`fastmodulo`
des canaux, ni l'adressage général des strides, ni le chargement des 20 paramètres de noyau, ni la
machine de garde du noyau générique. L'écart de **1,64×** suggère donc que la **machinerie générique
du noyau produit** coûte environ **40 % de plus que la boucle interne seule**.

**Je ne surinterprète pas cet écart** : les unités de code ne sont pas identiques (le microbench a
une boucle par ligne, la production a 4 warps par ligne), et `PRODUCTION_INFERRED_US` est dérivé
d'une sensibilité à la duplication, pas d'une mesure directe. Mais c'est un indice convergent avec
le §2 : il reste de la matière au-dessus de la boucle interne.

```
DEBUG_MICROBENCH_OVER_PRODUCTION_ATTRIBUTION=NOT_ESTABLISHED
GENERIC_MACHINERY_OVERHEAD_ESTIMATE=SUGGESTIVE_NOT_PROVEN
```

## 7 — Ce que cela change (§10, §12)

Le §12 prévoyait de fermer la ligne locale si le volume était confirmé. **Il ne l'est pas**, donc :

```
ENGINE006_LOCAL_KERNEL_OPTIMIZATION_STATUS=RESTE_OUVERTE
   (et NON "EXHAUSTED_NO_MATERIAL_RECOVERABLE_WASTE_FOUND")
ENGINE006_STATUS=RESTE_OPEN
ENGINE007_ALLOWED=NO          (le §12 ne s'applique pas)
```

Le tableau des trois axes locaux fermés reste valable — **mais il ne prouvait pas ce qu'on croyait** :
il prouvait que les trois transformations *essayées* étaient neutres ou négatives, pas que le noyau
est proche de son plancher. La mesure de plancher montre le contraire : il reste **un facteur 9,65
entre le streaming et le calcul complet**.

Deux directions se rouvrent, dans l'ordre que le §3 impose (**prouver un delta SASS négatif
d'abord**) :

1. **La machinerie générique du noyau** (§6 ci-dessus) : `fastdiv`/`fastmodulo` recalculés pour des
   facteurs invariants par appel, adressage des strides, 20 lectures de paramètres. Le gain borné des
   seuls `LDC` était ~2,6 % (006E) ; mais l'écart microbench/production suggère que la facture totale
   pourrait être bien supérieure. C'est le candidat à auditer **en premier**, avec la contrainte de
   R2 : mesurer le delta SASS **complet**, pas la propreté de la source.
2. **La part dot-product + réduction (71,7 %)** : c'est le poste dominant mesuré. Aucune
   transformation locale ne l'a attaqué jusqu'ici — les trois axes fermés portaient sur le décodage
   (17,9 %) et les chargements (10,4 %). Le `dp4a` et la réduction sont des candidats **non
   explorés**.

## 8 — Pistes structurelles (§14 à §19) : statut inchangé

```
ENGINE007_ALLOWED=NO
TERNARY_SPARSITY_AUDIT=NOT_STARTED
LOW_RANK_STRUCTURED_AUDIT=NOT_STARTED
DIMENSION_REDUCTION_AUDIT=NOT_STARTED
SPECULATIVE_DECODING_HISTORY=x1.93_TO_x2.09_NOT_REINTEGRATED
   -> ne pas convertir cet historique en resultat : il n'est pas revalide sur le runtime actuel
```

Ces pistes restent hors périmètre tant qu'ENGINE-006 n'a pas épuisé ce que la mesure de plancher
vient de rouvrir.

## 9 — Provenance

```
Runtime baseline   : libllama 3db83675… / libggml-cuda abafbf0d…  (restaures, SHA verifies)
Arbre llama-obrain-cuda : les 4 fichiers du chemin chaud aux SHA vierges
Fichiers sales     : les 3 greffes connues, inchangees
MODIFICATIONS DE SOURCE DANS CETTE PASSE : AUCUNE
Artefact           : qualification/e6f/e6f-lower-bound.cu + bin/e6f-lower-bound
Plafond local      : sonde dediee (lecture pure uint4), meme GPU, meme session
```

Défauts de harnais de cette passe, consignés :

```
HARNESS_DEFECT_22=SONDE_LOWER_BOUND_ECRITE_EN_LARGES_GREFFES_SED_FRAGILES
   -> trois echecs successifs (helper declare apres usage, chemin de copie fautif) ; le fichier a
      ete reecrit entierement. Aucune mesure n'a ete tiree d un binaire absent.
HARNESS_DEFECT_23=S0_EN_UINT32_INVALIDE_PAR_ALIGNEMENT
   -> revele par "misaligned address" ; corrige en uint16 ; c est en realite une CONFIRMATION de
      la contrainte 006D et non un defaut de mesure.
```

## 10 — Ledger

```
ENGINE006_STATUS=OPEN
ENGINE006C_R2_STATUS=CLOSED_V1_REJECTED
ENGINE006D_STATUS=CLOSED_WEIGHT_LOAD_VECTOR_REJECTED
ENGINE006E_STATUS=CLOSED_Q8_1_RESIDENCY_REJECTED
ENGINE006F_STATUS=CLOSED_EMPIRICAL_LOWER_BOUND_OBTAINED
LOCAL_MMVQ_RECOVERABLE_WASTE=NOT_EXHAUSTED_CONTRARY_TO_EXPECTATION
COST_IS_FUNDAMENTALLY_WORK_VOLUME=WEAKENED_NOT_CONFIRMED
WEIGHT_VOLUME_DOMINATES_LOWER_BOUND=NO
LOCAL_COMPUTE_OR_DECODE_HEADROOM_REMAINS=SUPPORTED
CAS=B
TARGET_WEIGHT_BYTES_READ=8355840
TARGET_Q8_1_BYTES_READ=1728
STREAM_US=8.2848
DECODE_US=22.6000
FULL_MMVQ_US=79.9360
DECODE_INCREMENT_US=14.3152
DOT_REDUCTION_INCREMENT_US=57.3360
STREAM_EFFECTIVE_GB_S=1008.57
FULL_EFFECTIVE_GB_S=104.53
FULL_OVER_STREAM_RATIO=9.649
STREAM_TO_DEVICE_CEILING_RATIO=3.94
LOCAL_DEVICE_STREAMING_CEILING_GB_S=255.8
FULL_TO_DEVICE_CEILING_RATIO=0.41
WEIGHT_TENSOR_SERVED_FROM_CACHE_NOT_DRAM=SUPPORTED
MICROBENCH_OVER_PRODUCTION_RATIO=1.64
GENERIC_MACHINERY_OVERHEAD_ESTIMATE=SUGGESTIVE_NOT_PROVEN
ALIGNMENT_CONSTRAINT_006D_CONFIRMED_AT_RUNTIME=YES
ROOFLINE_CLASSIFICATION=NOT_AVAILABLE
MEMORY_BOUND=NOT_PROVEN_WEAKENED
COMPUTE_BOUND=NOT_PROVEN
OCCUPANCY_LIMITED=NOT_PROVEN
ENGINE006_LOCAL_KERNEL_OPTIMIZATION_STATUS=OPEN_HEADROOM_MEASURED
ENGINE007_ALLOWED=NO
NEXT_AXIS_CANDIDATE=GENERIC_KERNEL_MACHINERY_AND_DOT_REDUCTION_PATH
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
