# ENGINE-005I FINAL — CAMPAGNE R0/R1/R2 : RÉSULTAT

Date : 2026-09-29. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `/home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/qualification/e6/`

---

## 0 — Réponse

La question posée :

`DOES_MORE_OF_THIS_EXACT_NATIVE_MMVQ_WORK MAKE REAL TOKEN LATENCY WORSE?`

**OUI, de façon nette, monotone et reproductive :**

```
NATIVE_MMVQ_DUPLICATE_WORK_WALL_SENSITIVITY_MS_PER_REPEAT = 1,2591 ms
      (regression sur 6 runs : wall = 63,7314 + 1,2591 x extra_repeat_count)
PENTE / BRUIT = 9,6
DUPLICATE_WORK_LINEARITY_RATIO = 0,869
```

Donc :

```
ATTN_GATE_TQ2_MMVQ_CAUSAL_WALL_SENSITIVITY=MEASURED
TARGETED_MMVQ_WORK_IS_ON_THE_WALL_CRITICAL_PATH=SUPPORTED
NUMERICAL_PREFLIGHT=PASS                (probe bit a bit identique sur R0/R1/R2)
TARGET_SCOPE_GATE=PASS
```

Et la porte ENGINE-006 est satisfaite (§17) : **la cible peut être épinglée.** Ce qui suit distingue
soigneusement ce que la mesure **établit** de ce qu'elle **n'établit pas**.

---

## 1 — Preflight numérique : PASS (§6, §7, §8)

16 tokens, graphes actifs, même modèle / préfixe / positions / mode :

```
R0  probe = efa529aff901d4b934e8b1e29942ca305ea88f089a98d187018184874b0debf0
R1  probe = efa529aff901d4b934e8b1e29942ca305ea88f089a98d187018184874b0debf0
R2  probe = efa529aff901d4b934e8b1e29942ca305ea88f089a98d187018184874b0debf0
R0_R1_PROBE_LOGITS_BITWISE_MATCH=YES
R0_R2_PROBE_LOGITS_BITWISE_MATCH=YES
```

**Égalité bit à bit** sur les trois niveaux, avec 0 / 72 / 144 séquences supplémentaires. Le travail
dupliqué est donc **mathématiquement neutre**, exactement comme le gate d'idempotence de source le
prédisait.

Détail qui compte (leçon de `HARNESS_DEFECT_7`) : la sonde s'exécute dans un build qui **conserve la
tête de sortie normale** dans les trois variantes — elle calcule donc réellement ses logits. Aucun
état sérialisé/restauré n'était nécessaire ici.

## 2 — Scope gate : PASS (§5)

Fenêtre 8 tokens, compteurs lus après la fenêtre :

| variante | séquences natives ciblées | séquences supplémentaires | extra non-cible |
|---|---|---|---|
| R0 | 0 (non compté) | **0** | **0** |
| R1 | 72 | **72 = 1 × 72** | 168 |
| R2 | 72 | **144 = 2 × 72** | 168 |

```
NON_TARGET_ROLE_EXTRA_EXECUTIONS=168 CONSTANT sur les trois variantes
   = 72 (attn_gate q4_K, les 12 poids exclus par qtype)
   + 96 (autres roles en tq2_g128 : attn_q, ffn, ...)
```

Les 168 appels non-cibles restent **inchangés** entre variantes : le forçage ne les touche pas.
`NON_TARGET_ATTN_GATE_EXTRA_EXECUTIONS=0` et `NON_TARGET_ROLE_EXTRA_EXECUTIONS` **constant** ⇒
le scope est exact.

Réserve de compteur : `E6_TARGET_NATIVE_MMVQ_OPERATIONS` n'est incrémenté que lorsque la branche de
duplication est active, donc il vaut 0 en R0. Ce n'est pas un défaut de mesure mais une limite du
compteur — la vérification `extra = k × native` en R1/R2 (72/144) porte la preuve, et la **preuve
structurelle** demandée au §11 est dans le code : la séquence supplémentaire est un appel littéral
supplémentaire à `e6_native()` (quantification + noyau) dans le graphe construit, donc capturé par
CUDA Graph et rejoué à chaque replay.

## 3 — Campagne principale : ordre pré-enregistré respecté (§9)

`R0 R1 R2 R2 R1 R0`, un processus propre par run, 256 tokens, graphes actifs.

| run | variante | wall ms/token | TPS | extra ciblé | extra non-cible |
|---|---|---|---|---|---|
| 1 | R0 | 63,5711 | 15,7304 | 0 | 0 |
| 2 | R1 | 65,0234 | 15,3791 | 144 | 336 |
| 3 | R2 | 66,2030 | 15,1051 | 288 | 336 |
| 4 | R2 | 66,2375 | 15,0972 | 288 | 336 |
| 5 | R1 | 65,0751 | 15,3669 | 144 | 336 |
| 6 | R0 | 63,8330 | 15,6659 | 0 | 0 |

Statistiques par niveau :

```
R0  n=2  mean=63,7020  median=63,7020  sd=0,1309  min=63,5711  max=63,8330
R1  n=2  mean=65,0493  median=65,0493  sd=0,0259  min=65,0234  max=65,0751
R2  n=2  mean=66,2202  median=66,2202  sd=0,0172  min=66,2030  max=66,2375
```

Les deux runs **intra-niveau** sont quasi identiques (sd 0,017–0,026 ms, soit < 0,05 %), alors que la
dispersion de R0 est plus large (0,13 ms) — c'est le niveau de bruit à retenir.

## 4 — Analyse de pente (§13, §14)

```
DELTA_01 = R1_mean - R0_mean = 1,3472 ms/token
DELTA_12 = R2_mean - R1_mean = 1,1710 ms/token
DUPLICATE_WORK_LINEARITY_RATIO = DELTA_12 / DELTA_01 = 0,869

REGRESSION (n=6) : wall_ms_per_token = 63,7314 + 1,2591 × extra_repeat_count
residu max = 0,1603 ms
NATIVE_MMVQ_DUPLICATE_WORK_WALL_SENSITIVITY_MS_PER_REPEAT = 1,2591
```

Interprétation des critères (§14) : la pente est **positive**, **très supérieure au bruit** (rapport
9,6 contre un bruit de 0,13 ms), **reproductible** (deux runs par niveau à moins de 0,05 % l'un de
l'autre) et **approximativement monotone** (R0 < R1 < R2 strictement). Le ratio de linéarité de 0,87
est proche de 1 sans y être égal — c'est attendu, la duplication immédiate bénéficie d'une résidence
en cache chaude (§13), donc la pente marginale décroît légèrement du deuxième au troisième niveau.

**Normalisation par travail ajouté** — deux relevés de compteurs, deux fenêtres :

```
fenetre 8 jetons  : cible native 72, extra 72 en R1    ->  9 sequences cible par jeton
campagne 256 jetons : extra par jeton (compteur E6)   =  144
```

Le compteur est un compteur d'**appels au point de dispatch**, pas d'exécutions logiques par jeton
(la réserve `ONE_DISPATCH_SELECTION_EQUALS_ONE_LOGICAL_EXECUTION=NOT_PROVEN` de 005G reste valable) ;
les deux relevés sont donc publiés tels quels sans être moyennés. En prenant le relevé de la
campagne comme normalisation :

```
cout marginal par sequence dupliquee = 1,2591 ms / 144 = 0,0087 ms = 8,7 us
```

C'est l'ordre de grandeur attendu pour un GEMV quantifié de forme `5120 × 6144` à `N=1` (une
matrice 5120×6144 en q2 demande ~7,5 Mo de lecture de poids : à ~1 To/s utiles cela place le noyau
dans la dizaine de microsecondes). La cohérence de ce chiffre avec la forme du tenseur est un
contrôle de vraisemblance favorable — pas une preuve de causalité supplémentaire.

## 5 — Ce que la pente ÉTABLIT, et ce qu'elle n'établit pas (§13, §15)

**Établi** :

```
TARGETED_MMVQ_WORK_IS_ON_THE_WALL_CRITICAL_PATH=SUPPORTED
```
Ajouter du travail MMVQ natif sur cette signature **allonge réellement** la latence du jeton, de
façon mesurable et monotone. Ce travail n'est donc pas entièrement absorbé par le recouvrement.

**NON établi, et je ne le promus pas** :

```
ATTN_GATE_NATIVE_WALL_COST=NOT_DERIVABLE
```

La pente n'est **pas** le coût du premier appel natif. Trois raisons : la répétition immédiate
bénéficie de poids et d'activations **chauds en cache** ; la destination est la même (écriture
répétée au même endroit) ; et le pipeline peut absorber une partie du travail natif **d'origine**
que la duplication, elle, ne peut plus absorber (le travail supplémentaire arrive après que le
recouvrement disponible est déjà consommé). La quantité correcte est la **sensibilité du mur au
travail MMVQ supplémentaire** — pas le coût natif.

Conséquence pratique pour ENGINE-006 : la sensibilité causale justifie d'attaquer cette signature,
mais elle **ne chiffre pas** le gain attendu d'une optimisation. Le gain réel devra être mesuré
après modification, pas déduit de cette pente.

## 6 — Endpoints (§10, §12)

Les compteurs de mode graphe demandés au §10 (`PROPERTIES_CHANGED_TRUE`, `CAPTURE_BEGIN`,
`INSTANTIATE`, `EXEC_UPDATE`, `LAUNCH`, `NO_UPDATE_LAUNCH`) ne sont **pas** instrumentés dans ce
runtime : je ne les publie donc pas. Les endpoints disponibles et publiés sont les endpoints
primaires du protocole RUN-003 :

```
BACKEND_ACTIVE_INTERVAL_SUM_MS   (via E5E_BACKEND_MS_PER_TOKEN, publie par run)
BACKEND_EQUIVALENT_TPS           (E5E_TPS, publie par run)
WALL_MS_PER_TOKEN                (metrique principale)
GREEDY_CONTINUOUS_WINDOW_MS / END_TO_END_GREEDY_TPS=NOT_MEASURED
      (le workload est teacher-forced, non greedy autoregressif — a ne pas confondre)
```

Preuve que les séquences supplémentaires sont **bien exécutées au replay** (et non seulement
construites) : le wall augmente de 1,26 ms par repeat de façon monotone. Si le travail ajouté
n'était capturé que dans le graphe sans être rejoué, la paroi serait restée plate comme en 005F-R2.

## 7 — Réserve sur le buffer temporaire (§4)

```
EXTRA_REPEAT_TEMP_BUFFER_SEMANTICS=REUSE_SAME_POOL_ALLOCATION_FULLY_REWRITTEN
TEMP_BUFFER_CONTENT_BEFORE_QUANTIZE=IRRELEVANT
```

La répétition réutilise la **même** allocation `src1_q8_1` du pool, exactement comme le chemin natif
le fait pour son unique usage : pas de scratch persistant, pas d'aliasing inter-répétition, pas de
synchronisation ajoutée, pas de changement de durée de vie. C'est la propriété qui rend la
duplication sémantiquement fidèle, et elle découle du gate D (le tampon est intégralement réécrit
par `quantize_row_q8_1_cuda` avant toute lecture).

## 8 — Gate ENGINE-006 (§17)

```
TARGET_MMVQ_REEXECUTION_SEMANTICALLY_IDEMPOTENT=SOURCE_PROVEN
NUMERICAL_PREFLIGHT=PASS
TARGET_SCOPE_GATE=PASS
ATTN_GATE_TQ2_MMVQ_CAUSAL_WALL_SENSITIVITY=MEASURED
```

Les trois conditions du §17 sont réunies. Cible :

```
ENGINE006_TARGET=recurrent attn_gate.weight / tq2_g128 / decode s1=1 / MMVQ
ENGINE006_TARGET_POPULATION=36 poids tq2_g128 (les 12 q4_K explicitement hors cible)
ENGINE006_KERNEL_MODIFICATION_ALLOWED=YES
ENGINE006_ACTION=OPTIMIZE_CURRENT_MMVQ_FOR_EXACT_TQ2_G128_DECODE_SIGNATURE
```

La nature d'ENGINE-006 est **d'améliorer le MMVQ existant** pour cette signature exacte, **pas** de
basculer vers MMQ (structurellement indisponible sur ce domaine, 005H).

Deux réserves à porter dans l'ouverture d'ENGINE-006 :

1. La sensibilité mesurée est celle du travail **supplémentaire** ; le gain d'une optimisation devra
   être mesuré, pas déduit (§5).
2. Aucun axe d'optimisation ne doit être choisi avant un profiling dédié de la signature
   (`memory access`, `dequantization`, `vectorization`, `register pressure`, `warp layout`,
   `reduction strategy`, `fusion`) — le gate de sensibilité ne désigne pas la cause du coût.

## 9 — Provenance

```
Runtime baseline   : libllama 3db83675… / libggml-cuda abafbf0d…   (restaurés, SHA vérifiés)
Arbre llama-obrain-cuda : mmvq.cu (2bf9a270…, identique a la sauvegarde d'origine, git propre)
                          ggml-cuda.cu 960e38d6… , qwen35.cpp f0fd1a3c…   (identiques aux vierges)
Runtime instrumente : build-e5i (source vierge + census + cible de graphe + duplication, sans evenement CUDA)
Instrument conservé : source-e6/mmvq-patched.cu et source-e6/apply-dup.py
Sequence rejouee   : qualification/run003/run003-tokens.txt (256 tokens teacher-forced)
Harnais : bin/e6i (fenetre --tokens configurable, rapport e6 avant la sonde)
```

Défauts de harnais rencontrés et corrigés dans la passe (consignés) :

```
HARNESS_DEFECT_9=E6_REPORT_PRINTED_AFTER_PROBE_NOT_BEFORE
HARNESS_DEFECT_10=TARGET_POINTER_NOT_STABLE_ACROSS_PROCESSES_ASLR
```
Le premier masquait les compteurs en mode fenêtre courte ; le second a imposé l'identification de
cible par **rôle + qtype** (ce qui correspond exactement à la définition de cible : 36 poids, les 12
q4_K exclus par leur qtype 12). Aucun effet sur les chiffres de la campagne, qui utilisent tous la
cible par rôle + qtype.

## 10 — Ledger

```
ENGINE004_STATUS=CLOSED_BASELINE
ENGINE005D_STATUS=CLOSED_PROVENANCE
ENGINE005G_STATUS=CLOSED_ROLE_KEYED_RUNTIME_DISPATCH
ENGINE005H_STATUS=CLOSED_DISPATCH_DOMAIN
ENGINE005I_STATUS=CLOSED_SENSITIVITY_MEASURED
DISPATCH_DECISION_DOMAIN_GRAPH_MODE_INDEPENDENT=YES
ATTN_GATE_DISPATCH_GRAPH_MODE_INDEPENDENT=YES
SIGNATURES_GRAPH_OFF=1738   SIGNATURES_GRAPH_ON=1738   MAPPING_DIVERGENT=0
TARGET_MMVQ_REEXECUTION_SEMANTICALLY_IDEMPOTENT=SOURCE_PROVEN
DUPLICATE_WORK_EXPERIMENT_ALLOWED=YES
EXTRA_REPEAT_TEMP_BUFFER_SEMANTICS=REUSE_SAME_POOL_ALLOCATION_FULLY_REWRITTEN
TEMP_BUFFER_CONTENT_BEFORE_QUANTIZE=IRRELEVANT
NUMERICAL_PREFLIGHT=PASS
R0_R1_PROBE_LOGITS_BITWISE_MATCH=YES
R0_R2_PROBE_LOGITS_BITWISE_MATCH=YES
TARGET_SCOPE_GATE=PASS
NON_TARGET_ATTN_GATE_EXTRA_EXECUTIONS=0
NON_TARGET_ROLE_EXTRA_EXECUTIONS=168_CONSTANT_ACROSS_VARIANTS
R0_MEAN_MS_PER_TOKEN=63.7020   R0_SD=0.1309
R1_MEAN_MS_PER_TOKEN=65.0493   R1_SD=0.0259
R2_MEAN_MS_PER_TOKEN=66.2202   R2_SD=0.0172
DELTA_01=1.3472   DELTA_12=1.1710
DUPLICATE_WORK_LINEARITY_RATIO=0.869
NATIVE_MMVQ_DUPLICATE_WORK_WALL_SENSITIVITY_MS_PER_REPEAT=1.2591
SLOPE_TO_NOISE_RATIO=9.6
ATTN_GATE_TQ2_MMVQ_CAUSAL_WALL_SENSITIVITY=MEASURED
TARGETED_MMVQ_WORK_IS_ON_THE_WALL_CRITICAL_PATH=SUPPORTED
ATTN_GATE_NATIVE_WALL_COST=NOT_DERIVABLE_FROM_SLOPE
ENGINE006_TARGET=recurrent_attn_gate.weight/tq2_g128/decode_s1_1/MMVQ
ENGINE006_TARGET_POPULATION=36_TQ2_WEIGHTS
ENGINE006_KERNEL_MODIFICATION_ALLOWED=YES
ENGINE006_ACTION=OPTIMIZE_CURRENT_MMVQ_FOR_EXACT_TQ2_G128_DECODE_SIGNATURE
ENGINE006_OPTIMIZATION_AXIS=NOT_YET_CHOSEN_PENDING_DEDICATED_PROFILING
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
