# ENGINE-006B — FORENSIC STATIQUE DU NOYAU tq2_g128 MMVQ

Date : 2026-09-29. Machine : `spark-c578` (GB10, sm_121a, CUDA 13.2).
Artefacts : `.../engine-004-steady-state/RUN-001/qualification/e6b/` (`e6b-ptxas.log`, `e6b-sass.txt`)

---

## 0 — Résultat

L'instanciation exacte est identifiée, ses ressources compilateur sont mesurées, et le SASS de la
spécialisation cible est profilé. Trois faits se détachent :

1. **Aucun spill, occupation maximale atteignable** : 42 registres, 384 o de smem, 0 trafic de pile
   locale. L'hypothèse « pression registres / occupancy » est **écartée par mesure compilateur**.
2. **Le décodage ternaire domine la charge d'instructions** : sur le corps chaud, **~62 % des
   instructions sont de la manipulation de bits** (`LOP3` 67, `SHF` 34, `SEL` 32, `ISETP` 38) contre
   **8 `IDP.4A`** de produit scalaire réel.
3. **La quantification hors boucle reste le chemin dominant** : les 8 `LDG.E.U8` par thread chargent
   chacun **1 octet** — les octets de codes d'une même ligne sont contigus, donc un chargement
   64 bits est possible. `SCALAR_LOADS_WHERE_VECTOR_LOAD_POSSIBLE=YES_HYPOTHESIS`.

```
ENGINE006B_STATUS=STATIC_FORENSIC_DONE
ENGINE006_FIRST_OPTIMIZATION_AXIS=TQ2_G128_DECODE_STRENGTH_REDUCTION
ENGINE006_FIRST_OPTIMIZATION_AXIS_STATUS=SELECTED_PENDING_MICROBENCH_AND_FIDELITY_GATE
MEMORY_BOUND=NOT_PROVEN      COMPUTE_BOUND=NOT_PROVEN      OCCUPANCY_LIMITED=NOT_PROVEN
REGISTER_SPILLING_NOT_OBSERVED
```

---

## 1 — Gate de provenance (§3, §4)

```
CURRENT_LLAMA_CONTEXT_SHA256=c97c54db1867d8a1082b7953dae20d241c678155db70e5428268cc1745a4da73
CURRENT_LLAMA_GRAPH_SHA256  =092599de5bfc7202b58fe28fc8fe173be60404d5fb343675b983a060f3763387
DIFF_SHA256                 =b00e6a9029e8b5d589f58aba8e62f0b52c19a8a0ffe2b8babce3be25ec487305
DIFF_STAT                   =43 insertions, 0 suppression
```

Contenu, intégralement lu :

| fichier | greffe | classe |
|---|---|---|
| `src/llama-context.cpp` | `llama_exp1a4_component_restore()` : restauration d'UN composant mémoire depuis son sous-buffer | **ajout de symbole** |
| `src/llama-graph.cpp` | `cb()` : suffixe `_l%d` ajouté au nom du tenseur | **renommage** |

```
DIRTY_SOURCE_ORIGIN=KNOWN
DIRTY_SOURCE_RELEVANT_TO_ENGINE006B_RUNTIME=NO
ENGINE006B_BUILD_FROM_CURRENT_TREE_ALLOWED=YES
```

Preuves d'inactivité, chacune mécanique :

1. **Aucun appelant** de `llama_exp1a4_component_restore` dans tout le dépôt (recherche exhaustive).
2. **Aucun consommateur du nom** dans le chemin chaud : `mmvq.cu`, `vecdotq.cuh` et le dispatch de
   `ggml-cuda.cu` ne lisent jamais `src0->name`. Le renommage n'atteint ni la forme, ni le type, ni
   les strides, ni le contenu d'un tenseur.
3. **Découverte décisive** : ces deux greffes sont **présentes dans le DSO gelé**
   (`nm -D` retrouve `llama_exp1a4_component_restore` dans `frozen-runtime/libllama.so`). La baseline
   de **toutes** les mesures ENGINE-004/005 a donc été prise avec ces greffes : les écarter
   reviendrait à changer la référence, pas à la nettoyer.

Hashes des fichiers du chemin chaud, tous identiques aux vierges :

```
mmvq.cu      2bf9a27028207120…     ggml-cuda.cu 960e38d680744e39…
qwen35.cpp   f0fd1a3c70958803…     vecdotq.cuh  a8ddd2223d81b20f…
```

## 2 — Instanciation exacte (§6)

```
TYPE                       GGML_TYPE_TQ2_G128 (43)
vec_dot                    vec_dot_tq2_g128_q8_1
VDR_TQ2_G128_Q8_1_MMVQ     1
ncols_dst                  1   (decode, s1=1)
has_fusion                 false   (aucune fusion active sur cette signature)
small_k                    false
qk (QK_G128)               128
qi                         128
QK8_1                      32
MMVQ_MAX_BATCH_SIZE_TQ2    49   (capacite etendue specifique au type)
device table               MMVQ_PARAMETERS_GENERIC (sm_121a n'est pas dans la fenetre Turing)
calc_nwarps(type, 1)       4
calc_rows_per_block(1)     1
```

Mangled name de la spécialisation cible :

```
_Z13mul_mat_vec_qIL9ggml_type43ELi1ELb0ELb0EEvPKvS2_PKi31ggml_cuda_mm_fusion_args_devicePfj5uint3jjjS7_jjjS7_jjjj
```

## 3 — Géométrie produit exacte (§7)

```
WEIGHT_SHAPE         5120 x 6144       (attn_gate.weight, 48 couches recurrentes)
ACTIVATION_SHAPE     6144 x 1
OUTPUT_SHAPE         5120 x 1

BLOCK_X=32  BLOCK_Y=4  BLOCK_Z=1        -> 128 threads = 4 warps
GRID_X=5120 GRID_Y=1 GRID_Z=1           -> 5120 blocs
WARPS_PER_BLOCK=4     ROWS_PER_BLOCK=1
OUTPUT_ROWS_PER_KERNEL=5120
BLOCK_COUNT_PER_MATMUL=5120
BLOCKS_PER_ROW_X=48   (6144/128)
TOTAL_THREADS_LAUNCHED=655 360
THREADS_DOING_USEFUL_WORK=655 360        (rows_per_block=1 : aucune ligne en trop)
BLOCK_TAIL_WASTE_PERCENT=0
ROW_TAIL_WASTE_PERCENT=0
```

`TAIL_WASTE=0` est **promu** : les dimensions s'alignent exactement (5120 = 40 x 128,
6144 = 48 x 128). Ce n'est pas une piste.

Chaque thread couvre exactement 1/32 d'une ligne, soit `48 x 34 / 32 = 51 octets` de poids.

## 4 — Décomposition du corps (§8, STATIC_ESTIMATE)

| catégorie | éléments SASS | par thread | statut |
|---|---|---|---|
| A. chargements poids tq2_g128 | `8 x LDG.E.U8.CONSTANT` (1 octet), `2 x LDG.E.U16` (échelle) | 4 tours | **mesuré (SASS)** |
| B. chargements activation q8_1 | `8 x LDG.E.CONSTANT` (32 bits) | 4 tours | **mesuré (SASS)** |
| C. échelle/métadonnées | `HFMA2` x3, `LDCU`/`LDC` (paramètres noyau) | — | **mesuré (SASS)** |
| D. décodage ternaire | `LOP3` 67, `SHF` 34, `SEL` 32, `ISETP` 38 | 4 tours | **mesuré (SASS)** |
| E. produit scalaire | `IDP.4A.S8.S8` x8 | 32 valeurs/tour | **mesuré (SASS)** |
| F. réduction | `SHFL.BFLY` x5 + `LDS` x3 + `STG` x1 | 1 fois | **mesuré (SASS)** |
| G. smem inter-warp | `tmp_shared`, 1 `__syncthreads()` | 1 fois | mesuré |
| H. écriture finale | `STG.E` x1 | 1 | mesuré |

Longueur du corps : **767 lignes de SASS**. Le décodage (D) représente **~62 %** des instructions
arithmétiques/entières, contre 8 instructions de dot product réel.

## 5 — Recherche des répétitions inutiles (§9)

```
IS_SRC1_RELOADED_PER_OUTPUT_ROW?     NON au sens des octets lus : chaque ligne relit l'activation,
                                     mais elle est servie par le cache (576 octets au total).
POIDS_LUS_PAR_MATMUL=8 355 840 octets (8,0 Mo)   ACTIVATION=576 octets
RATIO_POIDS/ACTIVATION=14 507 : 1
IS_Q8_SCALE_RELOADED_PER_ROW?        oui, mais via L1 (36 octets par bloc q8_1)
IS_TQ2_METADATA_REDECODED_REPEATEDLY? NON : le decodage se fait une fois par octet de poids,
                                     sans redondance observable dans le SASS.
```

L'activation (576 octets) est **intégralement réutilisée 5120 fois** : elle tient en L1 et n'est pas
un poste de trafic. Le coût est dans les **poids**, nécessairement relus à chaque ligne.

## 6 — Audit mémoire (§10) et trafic calculé

Largeurs de chargement observées :

```
LDG.E.U8.CONSTANT   x8    (1 octet par chargement : les codes ternaires)
LDG.E.CONSTANT      x8    (32 bits : l'activation q8_1)
LDG.E.U16.CONSTANT  x2    (16 bits : les echelles fp16)
LDG.E               x1
STG.E               x1
LDS                 x3  (smem)
LDL/STL             0   (aucun trafic de pile locale)
```

`SCALAR_LOADS_WHERE_VECTOR_LOAD_POSSIBLE=YES_HYPOTHESIS` : dans une tranche de 32 valeurs, le thread
`tid % 128` lit `qs[8*(tid%128) .. +7]` — **8 octets contigus**. Le SASS les charge en 8 `U8`
séparés. Un `LDG.E.64` unique serait équivalent en données lues. La qualification reste une
**hypothèse** : mesurer l'effet demande une modification, pas une lecture de SASS.

Normalisation du coût mesuré (005I/006A : 1,1794 ms par repeat, 9 séquences ciblées par jeton) :

```
cout par sequence de matmul   = 131,0 us
cout par ligne de sortie      = 25,6 ns
debit effectif (poids+activ)  = 63,8 Go/s
```

Pour situer sans conclure : GB10 = 48 SM, 1536 threads/SM, 25,2 Mo de L2, bus 256 bits. Le tenseur de
poids de cette signature pèse 8,0 Mo, soit **moins que le L2 (25,2 Mo)** — un effet de cache réel est
donc possible entre couches. Le débit effectif de 63,8 Go/s est loin d'une saturation DRAM : cela
**n'exclut pas** une limite mémoire, mais ne la démontre pas non plus.

```
MEMORY_BOUND=NOT_PROVEN     COMPUTE_BOUND=NOT_PROVEN
Aucune metrique hardware disponible dans cette passe (NCU indisponible)
```

## 7 — Chemins de décodage (§11)

```
BITS/PACKING_PER_WEIGHT   2 bits (codes 0..3)
GROUP_SIZE                128 poids
BLOC_POIDS                34 octets = 2 (echelle fp16) + 32 (codes)
METADATA_PER_GROUP        1 echelle fp16
```

Mapping des codes, dans le code : `0 -> -1`, `1 -> 0`, `2 -> +1`, `3 -> +1` (replié), exprimé par
`(c > 2 ? 1 : c - 1)` répété **quatre fois par octet**, plus un `& 0xFF` et un décalage pour
l'empilement en int32.

Compilation SASS correspondante : par octet de poids, `SHF` (extraction des 4 codes), `ISETP`+`SEL`
par comparaison, `LOP3` pour le masquage et l'assemblage. Concentration mesurée :

```
LOP3=67   SHF=34   SEL=32   ISETP=38   IMAD=34   IADD/IADD3=50
```

Aucune table de correspondance n'est utilisée : le mapping est **recalculé arithmétiquement** à
chaque octet, à chaque ligne, à chaque couche.

## 8 — Réduction (§12)

```
DOT_PARTIALS_PER_THREAD   4 (un par tour de la boucle de blocs ; 4 valeurs empilees dans tmp)
DOT_PARTIALS_PER_WARP     128 (32 threads x 4)
MECANISME                 warp_reduce_sum (SHFL.BFLY x5) puis smem inter-warp + syncthreads
REDUCTION_SERIAL_STEPS    5 shuffles dans le warp, puis 3 lectures smem et 1 ecriture
```

La réduction est **déjà minimale** (une réduction butterfly standard). `tmp_shared` ne coûte que
384 octets. Ce n'est pas une piste.

## 9 — Registres, smem, spills (§13, §15)

Mesuré par `ptxas -v`, **mêmes flags que le build de production** (`-O3 -use_fast_math
-gencode=arch=compute_121a,code=sm_121a`, 317 fonctions compilées) :

```
SPECIALISATION CIBLE  mul_mat_vec_q<tq2_g128, 1, false, false>
  REGISTERS_PER_THREAD=42
  STATIC_SHARED_MEMORY=384 octets
  STACK_FRAME=0        SPILL_STORES=0        SPILL_LOADS=0
  BARRIERS=1

VARIANTE AVEC FUSION  mul_mat_vec_q<tq2_g128, 1, true, false>
  REGISTERS_PER_THREAD=48    STATIC_SHARED_MEMORY=768 octets    SPILLS=0
```

Comparaison interne, même forme, même `ncols_dst=1`, mêmes flags :

| type | registres | smem | spills |
|---|---|---|---|
| **tq2_g128 (43)** | **42** | 384 o | 0 |
| q4_K (12) | 56 | 384 o | 0 |
| q6_K (14) | 48 | 384 o | 0 |

`tq2_g128` est le **plus économe** des trois. Sur 317 fonctions compilées, seules 6 présentent des
spills (8 à 72 octets) — la cible n'en fait pas partie.

```
REGISTER_SPILLING_NOT_OBSERVED
```

Arithmétique d'occupation (faits machine : 48 SM, 65 536 registres/SM, 1536 threads/SM) :

```
blocs/SM limités par les registres : 65536 / (128 x 42) = 12,1 -> 12
blocs/SM limités par les threads   : 1536 / 128         = 12
OCCUPANCY_THEORIQUE=100 %
vagues de blocs = 5120 / (48 x 12) = 8,9
```

**La pression registres ne limite rien.** `HIGH_REGISTER_COUNT => OCCUPANCY_LIMITED` est donc
explicitement rejeté pour cette spécialisation.

## 10 — Chemins de décodage et branches runtime (§18)

```
branches runtime dans le corps          : 3 BRA (boucle de blocs, garde finale, sortie)
conditionnels resolus a la compilation  : has_fusion, use_gate, ncols_dst, small_k
switch qtype                            : resolu au dispatch (hors noyau)
bounds checks                           : 1 (garde d'ecriture finale)
tail handling                           : aucun (TAIL_WASTE=0)
IS_IT_CONSTANT_FOR_ENGINE006_TARGET     : oui pour has_fusion/use_gate/ncols_dst/small_k (deja
                                          constexpr) ; le reste est de l'adressage dependant
                                          des strides, dynamique mais invariant par appel
```

## 11 — Table des hypothèses (§20)

| Hypothèse | preuve statique | impact potentiel | statut |
|---|---|---|---|
| pression registres | ptxas : 42 reg, 0 spill, 100 % d'occupation atteignable | nul | **NO** |
| spills | SASS : 0 `LDL`/`STL` | nul | **NO** |
| gaspillage de queue | géométrie exacte : 5120=40x128, 6144=48x128 | nul | **MEASURED_STATIC (0 %)** |
| gaspillage de réduction | 5 SHFL + 3 LDS, butterfly standard | faible | **NO** |
| rechargements redondants d'activation | 576 o réutilisés 5120x, servis par L1 | faible | **NO** |
| métadonnées tq2 redécodées | décodage une fois par octet, sans redondance | nul | **NO** |
| **décodage ternaire cher** | **SASS : 62 % des instructions en LOP3/SHF/SEL/ISETP vs 8 IDP** | **le plus élevé des candidats** | **SUPPORTED** |
| **chargements poids non vectorisés** | **8 x LDG.E.U8 pour 8 octets contigus** | non chiffré | **CANDIDATE (hypothèse)** |
| branches runtime génériques supprimables | 4 paramètres déjà `constexpr` | faible | **NO** |
| limite bande passante mémoire | aucune métrique HW ; 63,8 Go/s effectifs | inconnu | **NOT_PROVEN** |
| limite calcul | aucune métrique HW | inconnu | **NOT_PROVEN** |
| limite d'occupation | 100 % atteignable par calcul | nul | **NO** |

## 12 — Axe retenu et justification (§21, §22)

```
ENGINE006_FIRST_OPTIMIZATION_AXIS=TQ2_G128_DECODE_STRENGTH_REDUCTION
```

Vérification des six critères du §21 :

| critère | vérification |
|---|---|
| 1. appuyé par preuve statique | oui — 62 % des instructions, mesuré en SASS |
| 2. changement isolable | oui — la fonction `vec_dot_tq2_g128_q8_1` seule |
| 3. cible exacte seulement | oui — cette `vec_dot` n'est atteinte que par `tq2_g128` |
| 4. sémantique numérique préservée | oui — mapping identique `{-1,0,+1}`, mêmes `dp4a` |
| 5. réversible | oui — un `#if`, aucun autre appelant |
| 6. agit sur le corps dominant | oui — c'est le corps du noyau |

Ce qui **n'est pas** retenu, et pourquoi : réduire la pression registres (aucun spill) ; la réduction
(déjà minimale) ; la queue (nulle) ; la fusion avec la quantification (`INTERACTION=-0,0022 ms` en
006A, aucune marge) ; les chargements vectorisés (**hypothèse**, pas encore étayée par une mesure —
elle pourra être le deuxième axe si le premier ne donne rien).

**Aucun gain théorique n'est promu** :

```
EXPECTED_SPEEDUP=NOT_PROMOTED_UNTIL_MEASURED
```

Une remarque de prudence sur la lecture du SASS : une instruction de manipulation de bits coûte
généralement 1 cycle entier, et le noyau dispose de beaucoup de parallélisme (65 536 threads en vol
par SM) pour masquer ces latences. « 62 % des instructions » **ne prouve pas** que 62 % du temps y
passe. Ce qui est établi, c'est que c'est le seul poste massif et modifiable identifié — et que tous
les autres candidats ont été écartés par mesure.

## 13 — Chemin immédiat

1. **Microbench de décodage isolé** (`-1/0/+1` par `SEL` vs par table) sur la géométrie exacte,
   pour transformer l'hypothèse `SUPPORTED` en mesure avant de toucher au produit.
2. **Première modification ENGINE-006C** (`§23, §24`) : chemin spécifique
   `if (exact target) { decode optimise } else { inchange }`, compteurs
   `OPTIMIZED_PATH_SELECTED` / `FALLBACK_PATH_SELECTED` / `NON_TARGET_PATH_CHANGED=0`, puis gate
   de fidélité `BITWISE_LOGITS_MATCH`.
3. **Benchmark produit uniquement** (`§25`) : greedy normal, CUDA Graph ON, 256 jetons, comparaison
   contre ENGINE-004 (15,7983 / 15,6330 TPS). Pas le benchmark duplicate-work.

## 14 — Provenance

```
Runtime baseline   : libllama 3db83675… / libggml-cuda abafbf0d…   (restaurés, SHA vérifiés)
Arbre llama-obrain-cuda : mmvq.cu 2bf9a270… , ggml-cuda.cu 960e38d6… , qwen35.cpp f0fd1a3c… ,
                          vecdotq.cuh a8ddd222…   (identiques aux vierges)
Greffes connues    : src/llama-context.cpp et src/llama-graph.cpp (EXP1A4A / ENGINE-005C),
                     presentes AUSSI dans le DSO gele -> baseline des mesures coherente
AUCUNE MODIFICATION DE SOURCE dans cette passe (lecture, compilation de lecture, desassemblage)
Artefacts : qualification/e6b/{e6b-ptxas.log, e6b-sass.txt, e6b-compile.sh}
```

Défauts de harnais consignés dans cette passe (aucun effet sur les chiffres publiés, tous issus de
la compilation et du désassemblage) :

```
HARNESS_DEFECT_12=NVCC_COMPILE_SCRIPT_MANGLED_GENERATE_CODE_FLAG
HARNESS_DEFECT_13=DEVICE_PROPERTY_PROBE_WITHOUT_CUDA_RUNTIME_LINK
```

## 15 — Addendum : fermeture du gate de provenance (à lire avant ENGINE-006C)

Le gate du §3/§4 est désormais **fermé**, et la closure a corrigé deux points de ma première lecture.

### 15.1 Ce que la première lecture avait faux

J'avais écrit « aucun appelant » de `llama_exp1a4_component_restore`. **C'est inexact** : le
consommateur existe, dans un **troisième fichier source modifié** que le brief ne mentionnait pas :

```
 M src/llama-context.cpp        greffe EXP1A4A : ajout du symbole
 M src/llama-graph.cpp          greffe ENGINE-005C : suffixe _l%d sur le nom des tenseurs
 M tools/obrain/pstate-exp0a.cpp  CONSOMMATEUR : +655 lignes, declare et utilise
                                  llama_exp1a4_component_restore
```

Ce troisième fichier n'apparaissait pas dans `git status` filtré sur `src/` : il est sous `tools/`.
Les trois modifications forment donc une **chaîne expérimentale cohérente et voulue**
(`PSTATE_EXP1A3_IDENTITY_STATE_SET_CAUSAL`, donc `EXP1A4A`), pas des résidus.

L'outil `pstate-exp0a` est un **binaire autonome** : il n'entre pas dans `libllama.so` /
`libggml-cuda.so`, et aucun de nos runtimes de mesure ne le construit
(`ls build-e5i/bin | grep pstate` → vide). Il consomme la greffe sans que la greffe entre dans le
runtime mesuré.

### 15.2 La preuve mécanique qui manquait, faite

Le seul risque réel de la greffe `ENGINE-005C` était qu'un consommateur lise les noms de tenseurs
dans le chemin chaud. Vérifié fichier par fichier :

```
ggml/src/ggml-cuda/mmvq.cu        lectures du nom = 0
ggml/src/ggml-cuda/vecdotq.cuh    lectures du nom = 0
ggml/src/ggml-cuda/mmvq.cuh       lectures du nom = 0
ggml/src/ggml-cuda/quantize.cu    lectures du nom = 0
ggml/src/ggml-cuda/ggml-cuda.cu   aucune occurrence de ggml_get_name / src0->name
```

Le renommage n'atteint donc **ni la sélection de chemin, ni le corps du noyau, ni la
quantification**. Il est confiné au `cb()` du constructeur de graphe.

### 15.3 La référence (le point qui tranche)

```
strings frozen-runtime/libllama.so | grep -c "_l%d"        -> 6
nm -D frozen-runtime/libllama.so | grep -c exp1a4          -> 1
```

Les deux greffes sont **présentes dans le DSO gelé**, c'est-à-dire dans la référence de **toutes**
les mesures ENGINE-004/005/006. Les retirer ne « nettoierait » pas la baseline : cela en créerait une
autre, non mesurée. Les conserver est la seule option cohérente avec les chiffres publiés.

### 15.4 Verdict et règle de build retenue

```
DIRTY_SOURCE_ORIGIN=KNOWN
DIRTY_SOURCE_RELEVANT_TO_ENGINE006B_RUNTIME=NO
FROZEN_DSO_CONTAINS_BOTH_GRAFTS=YES
BUILD_SOURCE_BASELINE=EXACT_CURRENT_TREE_IDENTITY_RECORDED
BUILD_SOURCE_BASELINE_ISOLATION=NOT_REQUIRED_FOR_006C
```

Justification du choix, contre l'intuition du §4 : construire depuis un arbre isolé à `HEAD`
**sans** les deux greffes produirait un binaire qui ne correspond plus à la baseline DSO gelée. Le
§4 pose `BUILD_PROVENANCE_BASELINE > REPOSITORY_NOMINAL_BASELINE` : ici la **provenance** est
justement l'arbre courant plus ses trois fichiers sales, identifiés, classés et inertes sur le
chemin chaud. L'isolation n'est donc pas requise — mais elle est **remplacée par une identité
enregistrée**, pour que le prochain build soit reproductible sans ambiguïté.

Manifeste permanent (copie dans le dépôt et dans les artefacts) :

```
/home/triviere/projects/llama-obrain-cuda/ENGINE-006B-BUILD-BASELINE.txt
/home/triviere/exp-artifacts/engine-004-steacy-state/RUN-001/qualification/e6b/BUILD-BASELINE-MANIFEST.txt
```

Il contient : `GIT_HEAD=8447604f7f0c3b48a77be46de2466a5b23e27826`, le sujet du commit, la liste
exacte des fichiers modifiés et non suivis, les SHA256 des quatre fichiers du chemin chaud, la
classification des trois fichiers sales, et les répertoires de build avec leur rôle.

Une **règle de build** est fixée pour ENGINE-006C : toute mesure doit être prise avec un runtime dont
`mmvq.cu` et `vecdotq.cuh` sont **exactement** aux SHA consignés ici (`2bf9a270…`, `a8ddd222…`), et
toute modification doit être appliquée par script patch déclaré, jamais à la main.

## 16 — Ledger

```
ENGINE006A_STATUS=CLOSED_COMPONENT_DECOMPOSITION
TARGET_SCOPE_GATE=PASS
QUANTIZE_DUPLICATE_WALL_SENSITIVITY=NOT_DETECTED
MMVQ_KERNEL_DUPLICATE_WALL_SENSITIVITY_MS_PER_TOKEN=1.1794
FULL_PATH_DUPLICATE_WALL_SENSITIVITY_MS_PER_TOKEN=1.1917
INTERACTION_MS=-0.0022
ENGINE006_PRIMARY_AXIS=MMVQ_KERNEL
FUSION_WITH_Q8_1_QUANTIZATION_PRIORITY=NOT_SUPPORTED_BY_CURRENT_MEASUREMENT
FUSION_CAN_NEVER_HELP=NOT_PROMOTED
ENGINE006_TARGET=recurrent attn_gate.weight/tq2_g128/decode_s1_1/MMVQ
ENGINE006_KERNEL_MODIFICATION_ALLOWED=YES
DIRTY_SOURCE_ORIGIN=KNOWN
DIRTY_SOURCE_RELEVANT_TO_ENGINE006B_RUNTIME=NO
DIRTY_SOURCE_COUNT=3
DIRTY_SOURCE_2=src/llama-context.cpp_graft_EXP1A4A
DIRTY_SOURCE_3=src/llama-graph.cpp_graft_ENGINE-005C
DIRTY_SOURCE_4=tools/obrain/pstate-exp0a.cpp_CONSUMER_NOT_IN_RUNTIME
FROZEN_DSO_CONTAINS_BOTH_GRAFTS=YES
HOT_PATH_TENSOR_NAME_READS=0
BUILD_SOURCE_BASELINE=EXACT_CURRENT_TREE_IDENTITY_RECORDED
BUILD_SOURCE_BASELINE_MANIFEST=ENGINE-006B-BUILD-BASELINE.txt
ENGINE006B_BUILD_FROM_CURRENT_TREE_ALLOWED=YES
ENGINE006B_STATUS=STATIC_FORENSIC_DONE
KERNEL_SPECIALIZATION_IDENTIFIED=YES
GRID_X=5120  GRID_Y=1  GRID_Z=1
BLOCK_X=32  BLOCK_Y=4  BLOCK_Z=1  WARPS_PER_BLOCK=4  ROWS_PER_BLOCK=1
VDR=1  QK=128  QI=128  NCOLS_DST=1  HAS_FUSION=false  SMALL_K=false
SASS_INSTRUCTION_LINES=767
SASS_IDP_4A_COUNT=8
SASS_LDG_U8_COUNT=8
SASS_LOP3_SHF_SEL_ISETP=171
DECODE_INSTRUCTION_SHARE=APPROX_62_PERCENT
REGISTERS_PER_THREAD=42
STATIC_SHARED_MEMORY=384
STACK_FRAME=0
SPILL_STORES=0
SPILL_LOADS=0
REGISTER_SPILLING_NOT_OBSERVED
OCCUPANCY_THEORIQUE_ATTEIGNABLE=100_PERCENT
TAIL_WASTE=0
SCALAR_LOADS_WHERE_VECTOR_LOAD_POSSIBLE=YES_HYPOTHESIS
MEMORY_BOUND=NOT_PROVEN
COMPUTE_BOUND=NOT_PROVEN
OCCUPANCY_LIMITED=NOT_PROVEN
ENGINE006_FIRST_OPTIMIZATION_AXIS=TQ2_G128_DECODE_STRENGTH_REDUCTION
ENGINE006_FIRST_OPTIMIZATION_AXIS_STATUS=SELECTED_PENDING_MICROBENCH_AND_FIDELITY_GATE
EXPECTED_SPEEDUP=NOT_PROMOTED_UNTIL_MEASURED
NEXT_MEASUREMENT=TQ2_DECODE_MICROBENCH_THEN_ENGINE006C_TARGETED_PATCH
PLUMBING_FAILURE_COUNT=28
PLUMBING_FAILURE_29=NOT_ASSIGNED
```
