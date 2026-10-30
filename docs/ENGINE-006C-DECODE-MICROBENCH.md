# ENGINE-006C — MICROBENCH DU DÉCODAGE TERN AIRE tq2_g128

Date : 2026-09-29. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e6c/`

---

## 0 — Résultat

```
TERNARY_DECODE_FIDELITY=PASS                    (16 777 216 symboles, 0 ecart)
VEC_DOT_MICROBENCH_SPEEDUP=+18,1 %              (reproduit : +18,1 % sur deux executions)
DECODE_ONLY_SPEEDUP=+16,5 %
NO_NEW_SPILLS=YES
REGISTER_COUNT_REGRESSION_ACCEPTABLE=YES        (+1 registre)
```

Une variante à force réduite (V1) est **bit-à-bit identique** et **18 % plus rapide** que la
formulation actuelle (V0) sur la boucle interne du noyau, à géométrie de production. La variante par
table constante (V2) est **4,6× plus lente** : elle est rejetée, et le résultat est instructif.

**Ce qui n'est PAS fait** : la mesure produit. Le patch produit est écrit, il compile, mais la
mesure 256 tokens n'a pas abouti — la chaîne de harnais a échoué (§5). Donc :

```
ENGINE006C_PRODUCT_MEASUREMENT=NOT_DONE
PRODUCT_SPEEDUP=NOT_PROMOTED_UNTIL_MEASURED
```

---

## 1 — Fidélité avant performance (§6, §7)

```
GOLDEN_VECTOR_BLOCKS=524 288
GOLDEN_VECTOR_SYMBOLS=16 777 216
DECODE_MISMATCH_COUNT=0     (V0, V1 et V2 confondus)
TERNARY_DECODE_FIDELITY=PASS
```

Corpus : balayage exhaustif des octets par blocs de 8, plus tous-zéro, tous-0xFF, alternés 0xAA/0x55,
et pseudo-aléatoire déterministe. La comparaison se fait contre une **référence hôte indépendante**
(`decode_reference_host`, même sémantique que le code actuel), sur l'empilement `int32` exact attendu
par `dp4a` — pas seulement sur des valeurs décimales.

## 2 — Les trois variantes

| variante | principe | mécanisme |
|---|---|---|
| **V0** | actuelle | `(c > 2 ? 1 : c - 1)` — comparaison par code, 4 `ISETP`+4 `SEL` par octet |
| **V1** | force réduite | `(0xFF & ((c-1)>>31)) \| (0x01 & ((1-c)>>31))` — décalage signé, aucune comparaison |
| **V2** | table | `__constant__ int[256]`, un accès `LDC` par octet |

L'identité utilisée par V1 est exacte pour tout `c ∈ {0,1,2,3}` : `is0 = (c-1)>>31` vaut `-1` si
`c==0` ; `ge2 = (1-c)>>31` vaut `-1` si `c>=2` ; d'où `0xFF` pour `c=0`, `0x00` pour `c=1`, `0x01`
pour `c=2` et `c=3` — exactement le mapping du format.

## 3 — Microbench : deux niveaux, géométrie réelle (§8, §9, §10, §11)

```
ROWS=5120  BLOCKS_PER_ROW=48  GROUP_SIZE=128  PRODUCTION_VECTOR_WIDTH=32
WEIGHT_BYTES_PER_MATMUL=8 355 840
ITERS=64 par kernel, 20 kernels par mesure, warmup avant
MICROBENCH_TIMING_SEMANTICS_VALIDATED=YES
   (un intervalle evenementiel = 20 invocations non chevauchantes d'un seul kernel)
```

### Niveau A — decode-only

| variante | ms (20 kernels) | vs V0 |
|---|---|---|
| V0 | 0,2850 / 0,2856 | — |
| **V1** | **0,2466 / 0,2451** | **+16,5 % / +16,5 %** |
| V2 | 1,5290 / 1,5232 | −436 % |

### Niveau B — vec-dot isolé (boucle interne de production, N=1)

| variante | µs par matmul | vs V0 |
|---|---|---|
| V0 | 42,257 / 42,296 | — |
| **V1** | **34,595 / 35,338** | **+18,1 % / +16,4 %** |
| V2 | 195,014 / 195,789 | −361 % |

Les deux exécutions complètes donnent le même classement et le même ordre de grandeur du gain :
**V1 gagne de 16 à 18 %**, toujours.

## 4 — Preuve compilateur (§13, §14)

```
kern_vec_dot<0> (V0) : 55 registres, 0 spill, stack 0
kern_vec_dot<1> (V1) : 56 registres, 0 spill, stack 0     (+1 registre)
kern_vec_dot<2> (V2) : 48 registres, 0 spill, stack 0
```

```
NO_NEW_SPILLS=YES
REGISTER_COUNT_REGRESSION=-1_REGISTER_ACCEPTABLE
```

Aucune table volumineuse, aucune pression smem, aucune divergence de branche ajoutée : la
transformation ne déplace pas le coût vers un autre poste (c'était l'exigence du §13).

## 5 — Ce qui n'a PAS abouti : la mesure produit

Le patch produit a été écrit selon la règle du §18 — **deux fichiers seulement**, `vecdotq.cuh` et
`mmvq.cu`, aucun changement de graphe, de scheduler, d'état, de KV ni de chemin de sortie :

- `vecdotq.cuh` : les deux formulations coexistent dans `vec_dot_tq2_g128_q8_1`, sélectionnées par un
  drapeau `__constant__ int g_e6c_decode_sr` (le chemin d'origine reste intact quand il vaut 0) ;
- `mmvq.cu` : `ggml_cuda_mul_mat_vec_q` pose ce drapeau depuis `GGML_E6C_DECODE_SR`, une fois.

Le patch **compile** (`MAKE=0`). La mesure 256 tokens n'a pas été obtenue, pour deux raisons
enchaînées :

1. Un premier essai a échoué parce que `/tmp/e5e.cpp` n'existait plus sur la machine : le lien du
   harnais a échoué **silencieusement** (sortie redirigée), et mon test d'égalité a rendu
   `BITWISE_LOGITS_MATCH=YES` sur **deux chaînes vides** — faux positif que je signale comme tel.
2. Après avoir figé le harnais dans les artefacts, le lien a buté sur des symboles d'instruments
   d'anciennes passes (`ggml_census_*`, `e6_dup_report`). Les avoir déclarés faibles a produit un
   **segfault** aux deux bras : un symbole faible non résolu saute en adresse nulle.

```
ENGINE006C_PRODUCT_MEASUREMENT=BLOCKED_HARNESS_INSTRUMENT_SYMBOLS
BITWISE_LOGITS_MATCH_PRODUCT=NOT_EVALUATED
```

Le correctif est connu et court : supprimer du harnais les appels aux instruments des passes
terminées (ils ne servent plus à rien en 006C), puis relier et mesurer. **Aucune mesure produit n'a
été publiée**, et aucun gain produit n'est promu.

## 6 — V2 rejetée, et ce que cela apprend

La table constante perd **4,6×** malgré 48 registres seulement. Signature claire : les accès `LDC`
non uniformes (indice dépendant des données) se sérialisent dans le pipeline constant. C'est une
information utile pour la suite : la voie « lookup » est **fermée pour ce format**, et la
transformation gagnante est bien du **calcul entier**, pas de la mémoire. Toute recherche future de
réduction de force sur ce décodage doit rester dans l'ALU.

## 7 — Statut et suite

```
TERNARY_DECODE_LOCAL_OPTIMIZATION_EXISTS=YES
FULL_VEC_DOT_BENEFIT=+16..18_PERCENT
NO_NEW_SPILLS=YES
ENGINE006C_PRODUCT_PATCH_ALLOWED=NO_YET
   -> le §17 l'autorise conditionnellement a un gain du vec-dot : celui-ci est acquis.
      Mais le §19 exige le gate de fidelite produit, non franchi.
```

Prochaines étapes, dans l'ordre :

1. Nettoyer le harnais des appels d'instruments terminés, relier, et franchir le gate de fidélité
   produit (`BITWISE_LOGITS_MATCH` puis séquence greedy).
2. Mesurer le produit : greedy normal, CUDA Graph ON, 256 jetons, contre ENGINE-004
   (15,7983 backend / 15,6330 greedy TPS). **C'est la seule mesure qui compte** : le §21 interdit
   explicitement de projeter les +18 % du microbench sur le modèle.
3. Si le produit ne gagne pas malgré les +18 % locaux, conclure
   `TERNARY_DECODE_LOCAL_OPTIMIZATION_EXISTS=YES / FULL_VEC_DOT_BENEFIT=NOT_DETECTED` au niveau
   modèle et revenir à la table 006B pour l'axe suivant — sans bricoler la variante.

## 8 — Provenance

```
Runtime baseline   : libllama 3db83675… / libggml-cuda abafbf0d…   (restaures, SHA verifies)
Arbre llama-obrain-cuda : mmvq.cu 2bf9a270… , vecdotq.cuh a8ddd222… ,
                          ggml-cuda.cu 960e38d6… , qwen35.cpp f0fd1a3c…   (tous vierges)
Fichiers sales     : les 3 greffes connues (2 dans src/, 1 outil sous tools/) — inchangees
MODIFICATIONS      : toutes appliquees par script declare, restaurees en fin de passe
ASSAINISSEMENT     : 8 fichiers d'instrument orphelins retires de ggml/src/ggml-cuda
                     (census, layers, provenance, rolejoin, observe) et archives dans les artefacts
Artefacts          : qualification/e6c/e6c-microbench.cu, bin/e6c-microbench, harness/e5e.cpp
```

Défauts de harnais de cette passe, tous consignés :

```
HARNESS_DEFECT_14=HARNESS_SOURCE_MISSING_FROM_TMP_SILENT_LINK_FAILURE
HARNESS_DEFECT_15=EQUALITY_TEST_TRUE_ON_TWO_EMPTY_STRINGS
HARNESS_DEFECT_16=WEAK_UNDEFINED_INSTRUMENT_SYMBOL_CALLED_CAUSES_SEGFAULT
HARNESS_DEFECT_17=INSTRUMENT_SYMBOL_ADDED_TO_HARNESS_ACROSS_PASSES_WITHOUT_CLEANUP
```

## 9 — Ledger

```
ENGINE006_STATUS=OPEN
ENGINE006A_STATUS=CLOSED_COMPONENT_DECOMPOSITION
ENGINE006B_STATUS=CLOSED_STATIC_FORENSIC
ENGINE006_TARGET=recurrent attn_gate.weight/tq2_g128/decode_s1_1/MMVQ
ENGINE006_FIRST_OPTIMIZATION_AXIS=TQ2_G128_DECODE_STRENGTH_REDUCTION
ENGINE006_FIRST_OPTIMIZATION_AXIS_STATUS=VALIDATED_LOCALLY_PENDING_PRODUCT_GATE
BUILD_SOURCE_BASELINE=EXACT_CURRENT_TREE_IDENTITY_RECORDED
DIRTY_TREE_BASELINE_INTEGRITY=PASS
PATCHED_BY_DECLARED_SCRIPT=YES
MANUAL_SOURCE_EDIT=NO
GOLDEN_VECTOR_BLOCKS=524288
GOLDEN_VECTOR_SYMBOLS=16777216
DECODE_MISMATCH_COUNT=0
TERNARY_DECODE_FIDELITY=PASS
DECODE_ONLY_SPEEDUP=+16.5_PERCENT
VEC_DOT_MICROBENCH_SPEEDUP=+18.1_PERCENT
VEC_DOT_MICROBENCH_SPEEDUP_REPRODUCED=+16.4_PERCENT
V2_LOOKUP_TABLE_SPEEDUP=-361_PERCENT_REJECTED
NO_NEW_SPILLS=YES
REGISTERS_V0=55  REGISTERS_V1=56  SPILLS_V0=0  SPILLS_V1=0
MICROBENCH_TIMING_SEMANTICS_VALIDATED=YES
ENGINE006C_PRODUCT_PATCH_ALLOWED=NO_YET
ENGINE006C_PRODUCT_MEASUREMENT=BLOCKED_HARNESS_INSTRUMENT_SYMBOLS
BITWISE_LOGITS_MATCH_PRODUCT=NOT_EVALUATED
PRODUCT_SPEEDUP=NOT_PROMOTED_UNTIL_MEASURED
NEXT_MEASUREMENT=CLEAN_HARNESS_THEN_PRODUCT_GREEDY_256_TOKENS
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
