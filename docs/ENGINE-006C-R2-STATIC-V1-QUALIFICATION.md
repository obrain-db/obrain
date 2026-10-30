# ENGINE-006C-R2 — QUALIFICATION PRODUIT STATIQUE DE V1 : REJET CONFIRMÉ

Date : 2026-09-29. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/r2/`

---

## 0 — Résultat

Le commutateur runtime est supprimé, deux variantes **compilées séparément** depuis la même baseline
sont comparées, et V1 **ne gagne toujours pas** :

```
BASE-REBUILD  mean = 64,1135 ms/token  (sd 0,1557)
STATIC-V1     mean = 63,8386 ms/token  (sd 0,0766)

STATIC_V1_GAIN_MS_PER_TOKEN = +0,2748 ms
gain / bruit                = 1,77          <- SOUS LE SEUIL D'UN GATE (exige > bruit, reproduit)
STATIC_V1_BACKEND_TPS_SPEEDUP = 1,0043      (+0,43 %)
```

```
V1_STATIC_PRODUCT_TRANSLATION=NO_MEASURABLE_GAIN
ENGINE006_V1_STATUS=REJECTED_FOR_PRODUCT
MICROBENCH_SPEEDUP_DOES_NOT_TRANSLATE_TO_PRODUCT=YES
```

La fidélité est parfaite sur les 6 runs (séquence `daa762b7…` et sonde `e61e28c6…` identiques), mais
le gain n'est **pas distinguable du bruit d'exécution**.

---

## 1 — Correction de statut (§1, §2)

```
V1_LOCAL_MICROBENCH=WIN                                     (le microbench A bien selectionne V1)
V1_RUNTIME_SWITCH_PRODUCT_IMPLEMENTATION=REJECTED
V1_TRANSFORMATION_PRODUCT_STATUS=NOT_YET_ISOLATED_FROM_CODEGEN_EFFECT   -> leve par R2
V1_STATIC_PRODUCT_IMPLEMENTATION=MEASURED_NO_SIGNIFICANT_GAIN
PATCHED_SOURCE_SHAPE_OVERHEAD_MS_PER_TOKEN=+1.1312
PATCHED_SOURCE_SHAPE_OVERHEAD_CAUSE=NOT_ESTABLISHED
```

`ENGINE006_FIRST_OPTIMIZATION_AXIS=REJECTED_BY_MICROBENCH` est **retiré** : le microbench n'a pas
rejeté V1, c'est le produit qui la rejette.

## 2 — Gate de scope (§3, §4, §5) : **CAS B**

Une seule occurrence de l'expression de mapping dans tout `ggml-cuda` :

```
vecdotq.cuh:752   vi[j] = ((c0 > 2 ? 1 : c0 - 1) & 0xFF) | ...
   dans vec_dot_tq2_g128_q8_1 (vecdotq.cuh:732)
   atteinte par mmvq.cu:13  case GGML_TYPE_TQ2_G128 -> vec_dot_tq2_g128_q8_1
```

Aucune condition de rôle : la table de types route **tous** les appels `tq2_g128`.

```
V1_APPLIES_TO_ATTN_GATE_TQ2_ONLY=NO
V1_APPLIES_TO_ALL_TQ2_G128_MMVQ=YES
ENGINE006_IMPLEMENTATION_SCOPE=BROADER_THAN_CAUSAL_TARGET
```

Rôles et poids touchés par V1 :

| rôle | poids tq2_g128 |
|---|---|
| `ffn_gate.weight` | 48 |
| `ffn_up.weight` | 48 |
| **`attn_gate.weight`** (la cible causale) | **36** |
| `attn_k.weight` | 12 |
| `attn_output.weight` | 12 |
| `attn_q.weight` | 12 |
| **total** | **168** |

```
V1_AFFECTED_WEIGHT_COUNT=168
V1_AFFECTED_ROLES=attn_gate,attn_q,attn_k,attn_output,ffn_gate,ffn_up
V1_AFFECTED_DISPATCH_SIGNATURES=6_roles_x_s1_1_MMVQ_plus_les_signatures_non_decode
```

Donc 006C-R1 mesurait en réalité :

```
GENERIC_TQ2_G128_MMVQ_STRENGTH_REDUCTION   (et non attn_gate seul)
```

C'est une correction d'attribution, pas une invalidation : le changement était fidèle et le résultat
reste interprétable — mais il ne renseigne **pas** spécifiquement sur la cible causale `attn_gate`.

Conformément au §4 : `BITWISE_LOGITS_MATCH_PRODUCT=YES` prouve `NUMERICAL_FIDELITY=PASS`, **pas** le
scope. Le scope vient de la source, ci-dessus.

## 3 — Patch statique (§6, §7)

Deux variantes compilées séparément depuis la baseline enregistrée, **aucun commutateur runtime** :

```
BASE-REBUILD : source vierge, aucun drapeau, aucune branche, aucun getenv, aucune alternative
STATIC-V1    : l'expression V0 remplacee en place par V1, et rien d'autre
```

```
BASE_SOURCE_SHA256     = a8ddd2223d81b20fcc09ba46…   (vecdotq.cuh vierge)
STATIC_V1_SOURCE_SHA256= c97c319afdc4ed3d51ecbd57…   (vecdotq.cuh patche)
STATIC_V1_PATCH_SHA256 = 776010622df18bc8c482d191…   (diff unifie)
STATIC_V1_CHANGED_FUNCTION_COUNT=1                   (vec_dot_tq2_g128_q8_1 uniquement)
STATIC_V1_CHANGED_LINE_COUNT=20
PATCH_REVERSE_ROUNDTRIP=PASS                         (inversion textuelle exacte verifiee)
if_flag_constant_selector_in_static_v1=0            (aucun des trois)
```

## 4 — Parité de compilation (§8)

Deux arborescences de build distinctes, même commande cmake, mêmes flags :

```
COMPILE_CONFIGURATION_PARITY=PASS    (CUDA_FLAGS et CUDA_DEFINES identiques, diff vide)
BASE     DSO = 18a515bcfc3d3f3c0c9a3e4b5d656dfa…
STATIC-V1 DSO = 95961025f3334960ec33727bd3c2a892…
```

## 5 — Preuve compilateur : V1 produit **plus** d'instructions

SASS du noyau exact `mul_mat_vec_q<tq2_g128, 1, false, false>` dans les deux builds :

| | LOP3 | SHF | SEL | ISETP | IDP.4A | LDG | lignes |
|---|---|---|---|---|---|---|---|
| **BASE** | 67 | 34 | **32** | 38 | 8 | 19 | 777 |
| **STATIC-V1** | **83** | 34 | **56** | 38 | 8 | 19 | 777 |

```
STATIC_V1_LOP3_DELTA=+16
STATIC_V1_SEL_DELTA=+24
STATIC_V1_INSTRUCTION_DELTA=+40_SUR_LA_FENETRE_DU_NOYAU
```

**V1 n'allège pas le décodage : elle l'alourdit de 40 instructions.** Le microbench mesurait une
fonction isolée où le compilateur disposait de toute la marge de registres et d'ordonnancement ; dans
le noyau complet, `(c-1)>>31` puis `&` puis `|` coûte davantage que le `ISETP`+`SEL` d'origine que le
compilateur sait déjà fusionner.

C'est l'explication mécanique du résultat produit, et elle est **mesurée**, pas supposée.

## 6 — Campagne (§11)

Ordre fixé avant observation : `BASE V1 V1 BASE BASE V1`. Un processus neuf par run, greedy normal,
CUDA Graph ON, 256 jetons.

| run | bras | DSO | ms/token | backend TPS | séquence | sonde |
|---|---|---|---|---|---|---|
| 1 | BASE | 18a515bc… | 64,1472 | 15,5891 | daa762b7… | e61e28c6… |
| 2 | V1 | 95961025… | 63,9151 | 15,6458 | daa762b7… | e61e28c6… |
| 3 | V1 | 95961025… | 63,8669 | 15,6576 | daa762b7… | e61e28c6… |
| 4 | BASE | 18a515bc… | 64,2850 | 15,5557 | daa762b7… | e61e28c6… |
| 5 | BASE | 18a515bc… | 63,9082 | 15,6474 | daa762b7… | e61e28c6… |
| 6 | V1 | 95961025… | 63,7339 | 15,6902 | daa762b7… | e61e28c6… |

```
PRODUCT_FIDELITY=PASS                    (6/6 sequences et 6/6 sondes identiques)
STATIC_V1_GAIN_MS_PER_TOKEN=+0.2748      (gain/bruit = 1.77, NON REPRODUCTIBLE au seuil exige)
STATIC_V1_BACKEND_TPS_SPEEDUP=1.0043
RUNTIME_SWITCH_CODEGEN_INTERFERENCE=NOT_SUPPORTED
   (le gain statique 0.2748 ms n'est PAS significativement superieur au gain interne 0.1011 ms
    observe en 006C-R1 : l'intervalle de confiance au bruit observe les recouvre)
```

## 7 — Sur l'ordre de grandeur (§14)

006C-R1 avait déjà révélé l'échelle réelle : **+18 % au microbench → ~0,1 à 0,3 ms/token au
produit**, soit ~0,4 à 1,7 % de paroi. Ce n'est pas un échec scientifique : c'est la mesure qui
montre qu'une réduction de 62 % → 60 % d'instructions dans une boucle **latency-bound** ne rapporte
presque rien. Cette leçon est now établie sur deux protocoles indépendants (switch runtime et build
statique).

```
V1_PRODUCT_IMPACT=SMALL
ENGINE006_V1_STATUS=REJECTED_FOR_PRODUCT
```

## 8 — Porte 1 (BASE-REBUILD vs FROZEN) : parité **non** établie

```
FROZEN       : 63,8685 ms/token   (reference)
BASE-REBUILD : 64,1678 ms/token   (1 run)
ecart        : +0,30 ms/token
```

Un seul run de chaque côté : l'écart est du même ordre que le bruit inter-runs observé ensuite
(sd 0,15 ms sur BASE-REBUILD). Je **ne** promus donc pas :

```
BUILD_REPRODUCTION_PERFORMANCE_PARITY=NOT_ESTABLISHED
```

Conséquence sur l'interprétation, énoncée explicitement : l'A/B BASE vs V1 est valide comme
comparaison **contemporaine** (même arbre, même config, même harnais, même jour), mais une incertitude
de ±0,15 ms entre DSO reconstruites existe, et le gain mesuré de 0,27 ms est du même ordre. C'est
précisément pourquoi le gate du §16 n'est pas franchi.

## 9 — Suite (§17 à §20)

```
NEXT_AXIS=TQ2_G128_WEIGHT_LOAD_VECTORIZATION
WEIGHT_LOAD_VECTORIZATION_CAUSE=HYPOTHESIS
NEXT_AXIS_SELECTED=NO
ENGINE006_EXPERIMENTAL_PRODUCT_VARIANTS=SEPARATELY_COMPILED   (regle permanente)
ENGINE006_METHOD_RULE_1=NO_RUNTIME_SWITCH_FOR_KERNEL_PRODUCT_AB
```

Audit exigé avant toute modification (§18), dont j'ai déjà la matière :

```
8_BYTES_CONTIGUOUS=YES                 (qs[8*(tid%128) .. +7], verifie en source)
ALIGNMENT_AT_EVERY_ACCESS=NOT_ESTABLISHED
CROSS_BOUNDARY_SAFETY=NOT_ESTABLISHED
ENDIANNESS_ASSUMPTIONS=NOT_ESTABLISHED
COMPILER_ALREADY_COALESCES=UNKNOWN
EIGHT_SCALAR_INSTRUCTIONS_IMPLY_EIGHT_TRANSACTIONS=NO
```

Le §18 a raison sur le fond : **huit instructions scalaires ne prouvent pas huit transactions**. Sur
une carte à secteur de 32 octets et à coalescence matérielle, les 8 `LDG.E.U8` d'un même thread
peuvent très bien produire une seule transaction. L'audit d'alignement devra donc précéder toute
hypothèse de gain — et vu la leçon de R2, l'attente de gain produit doit rester **basse**.

## 10 — Statut des défauts antérieurs (§21)

```
HARNESS_DEFECT_14..17=RESOLVED     AFFECTS_PROMOTED_RESULT=NO
HARNESS_DEFECT_18=PREROLL_STOP_CRITERION_RECONSTRUCTED_NOT_CONFIRMED   (ouvert, cf. §12)
HARNESS_DEFECT_19=TARGET_SELECTION_COUNTER_NOT_POSED                   (ouvert)
```

## 11 — Séquence et freeze (§12)

```
E6CR1_SEQUENCE_SHA256=daa762b74378aa09d7d187c508130446b6ca9131d1811d7b42966732275ecbac
RUN003_SEQUENCE_SHA256=210b7eb55ff64a10c1d51af5391edc21b8d4545d297817c332d5635842590231
RUN003_SEMANTICS_REPRODUCED=NO
CONTEMPORANEOUS_CAUSAL_COMPARISON=VALID
REPLACES_ENGINE004_FREEZE=NO
```

## 12 — Provenance

```
Runtime baseline   : libllama 3db83675… / libggml-cuda abafbf0d…  (restaures, SHA verifies)
Arbre llama-obrain-cuda : les 4 fichiers du chemin chaud aux SHA vierges
Fichiers sales     : les 3 greffes connues, inchangees
Builds             : build-r2-base (18a515bc…), build-r2-v1 (95961025…), configs identiques
Patches            : apply-static-v1.py (declare, roundtrip inverse PASS)
Artefacts          : r2/, source-r2/, qualification/e6b/r2-{base,v1}-sass.txt
```

## 13 — Ledger

```
ENGINE006C_R1_STATUS=CLOSED_RUNTIME_SWITCH_VARIANT_REJECTED
V1_LOCAL_MICROBENCH=WIN
V1_RUNTIME_SWITCH_PRODUCT_IMPLEMENTATION=REJECTED
V1_STATIC_PRODUCT_IMPLEMENTATION=MEASURED_NO_SIGNIFICANT_GAIN
PATCH_RUNTIME_MECHANISM_OVERHEAD_MS_PER_TOKEN=1.1312
PATCHED_SOURCE_SHAPE_OVERHEAD_CAUSE=NOT_ESTABLISHED
V1_INTERNAL_GAIN_WITHIN_PATCHED_BUILD_MS_PER_TOKEN=0.1011
ENGINE006_IMPLEMENTATION_SCOPE=BROADER_THAN_CAUSAL_TARGET
V1_APPLIES_TO_ATTN_GATE_TQ2_ONLY=NO
V1_APPLIES_TO_ALL_TQ2_G128_MMVQ=YES
V1_AFFECTED_WEIGHT_COUNT=168
V1_AFFECTED_ROLES=attn_gate,attn_q,attn_k,attn_output,ffn_gate,ffn_up
BASE_SOURCE_SHA256=a8ddd2223d81b20fcc09ba46
STATIC_V1_SOURCE_SHA256=c97c319afdc4ed3d51ecbd57
STATIC_V1_PATCH_SHA256=776010622df18bc8c482d191
STATIC_V1_CHANGED_FUNCTION_COUNT=1
PATCH_REVERSE_ROUNDTRIP=PASS
COMPILE_CONFIGURATION_PARITY=PASS
STATIC_V1_LOP3_DELTA=+16
STATIC_V1_SEL_DELTA=+24
STATIC_V1_INSTRUCTION_DELTA=+40
BASE_MEAN_MS_PER_TOKEN=64.1135
STATIC_V1_MEAN_MS_PER_TOKEN=63.8386
STATIC_V1_GAIN_MS_PER_TOKEN=0.2748
STATIC_V1_GAIN_TO_NOISE_RATIO=1.77
STATIC_V1_BACKEND_TPS_SPEEDUP=1.0043
V1_STATIC_PRODUCT_TRANSLATION=NO_MEASURABLE_GAIN
MICROBENCH_SPEEDUP_DOES_NOT_TRANSLATE_TO_PRODUCT=YES
ENGINE006_V1_STATUS=REJECTED_FOR_PRODUCT
ENGINE006_FIRST_OPTIMIZATION_AXIS=REJECTED_BY_PRODUCT_MEASUREMENT
RUNTIME_SWITCH_CODEGEN_INTERFERENCE=NOT_SUPPORTED
BUILD_REPRODUCTION_PERFORMANCE_PARITY=NOT_ESTABLISHED
PRODUCT_FIDELITY=PASS
BITWISE_LOGITS_MATCH_PRODUCT=YES
NEXT_AXIS_WEIGHT_LOAD_VECTORIZATION=QUEUED_NOT_STARTED
NEXT_AXIS_SELECTED=NO
ENGINE006_EXPERIMENTAL_PRODUCT_VARIANTS=SEPARATELY_COMPILED
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
