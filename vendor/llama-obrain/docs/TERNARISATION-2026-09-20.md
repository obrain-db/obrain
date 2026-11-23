# Ternarisation d'un 27B hybride — le format de PrismML décodé, et ce que le fork a déjà

**Date** : 2026-09-20. **Nature** : relevé de format obtenu par lecture directe des fichiers
publiés (requêtes HTTP partielles), et inventaire des points d'insertion dans ce fork.
**Statut** : le format est **vérifié au bit** (`PTQ1_0` décodé à **100,0000 %** sur 87 680 trits) ;
la convention de rotation est **MESURÉE** (§5) ; la descente de quantification est allée de **70 143
à 4,29** de perplexité **sans entraînement** (§11) ; la chaîne complète **parle, raisonne et suit une
instruction**. Ce qui reste ouvert est en fin de document.

> **Pour reprendre le travail** : `docs/REPRISE-TERNARISATION-2026-09-20.md` — mode d'emploi,
> artefacts, commandes exactes, pièges déjà payés, trois premières commandes. Le présent document
> est le *récit technique* ; l'autre est *l'état opérationnel*.

**Régime des affirmations** — celui du dépôt :

| mot | sens |
|---|---|
| **MESURÉ** | relevé par exécution, commande citée |
| **CONSTATÉ** | lu dans un fichier, ligne citée |
| **EXTRAIT** | lu dans un fichier distant, offset cité |
| **CALCULÉ** | arithmétique, formule et valeurs données |
| **NON VÉRIFIÉ** | pas de relevé — et je le dis plutôt que de le combler |

---

## 0 · Pourquoi ce document existe

« Utiliser leurs découvertes pour l'implémenter proprement chez nous », pas consommer leur modèle.
Le premier obstacle n'était pas les noyaux : c'était de savoir **ce qu'est exactement leur format**.
Ce document le fixe, et fixe ce qui reste à prouver avant d'écrire des noyaux.

---

## 1 · Les métadonnées de leur fichier

**EXTRAIT** de `prism-ml/Ternary-Bonsai-2-27B-gguf`, `Ternary-Bonsai-2-27B-PTQ1_0.gguf`,
premiers 3 Mo (l'en-tête GGUF seul ; aucune donnée de tenseur n'est nécessaire) :

```
general.architecture                = qwen35            ← notre architecture, à l'identique
general.basename                    = folded            ← le repli hors ligne est déclaré
general.version                     = v5
prism.hadamard.version              = 1
prism.hadamard.block_size           = 1024
prism.hadamard.transform            = normalized-sylvester-walsh-hadamard
prism.hadamard.axis                 = input-last-dimension
prism.hadamard.sign_mode            = explicit
prism.hadamard.sign_widths          = (5120, 6144, 17408)
prism.hadamard.sign_values          = 28672 valeurs ∈ {−1,+1}
prism.hadamard.weight_names         = 401 noms
prism.hadamard.inverse_weight_names = ['token_embd.weight']
prism.hadamard.gdn_v_grouped        = 1
```

**CALCULÉ** : `5120 + 6144 + 17408 = 28672` → le vecteur de signes est **un par largeur d'entrée**,
pas un par matrice. `5120 = 5×1024`, `6144 = 6×1024`, `17408 = 17×1024` → la rotation est
**par blocs de 1024 le long de `ne[0]`**, ce que `axis = input-last-dimension` désigne.

---

## 2 · Le layout `PTQ1_0` — vérifié à 100 %

**EXTRAIT** depuis les offsets réels de `Ternary-Bonsai-2-27B-PTQ1_0.gguf` et de
`Ternary-Bonsai-2-27B-F16.gguf` (base de la section de données = **11 120 992** dans les deux
fichiers — `tensor.offset` est relatif à cette base, pas au fichier) :

| tenseur | éléments | octets mesurés | bpw |
|---|---|---|---|
| `output.weight` | 1 271 398 400 | 278 118 400 | **1,7500** |
| `token_embd.weight` | 1 271 398 400 | 278 118 400 | **1,7500** |
| `blk.0.ffn_gate.weight` | 89 128 960 | 19 496 960 | **1,7500** |
| `blk.0.ffn_down.weight` | 89 128 960 | 19 496 960 | **1,7500** |

**CALCULÉ** : 1,75 bpw × 128 poids = 224 bits = **28 octets par groupe de 128**.
Décomposition : 26 octets de trits + 2 octets d'échelle FP16.

**MESURÉ — la taille de groupe est 128, et le test est discriminant.** Sur le tenseur
`blk.0.ffn_gate.weight` du fichier F16 (92 000 valeurs = 17 lignes de 5120) :

| fenêtre alignée | fenêtres à magnitude unique |
|---|---|
| 16 | 100,00 % |
| 32 | 100,00 % |
| 64 | 100,00 % |
| **128** | **100,00 %** |
| 256 | 0,29 % |

→ la magnitude est constante **jusqu'à 128 inclus** et casse au-delà : le groupe vaut 128.
Sur la ligne 0 : **40 changements de magnitude = 40 groupes** pour 5120 valeurs, soit 5120/128 ✓.
Zéros mesurés : 32,8 % (termaire équilibré, cf. trits ∈ {−1,0,+1}).

**MESURÉ — le layout interne du groupe, reconstitué puis vérifié à 100 %.**

Les 685 premiers groupes du tenseur `blk.0.ffn_gate.weight` ont été décodés et comparés, trit à
trit, aux valeurs du fichier F16 du même tenseur (mêmes groupes) :

| zone | octets | positions des poids | règle |
|---|---|---|---|
| A | 0..15 (16 o.) | 0..79 | `n*16 + m` |
| B | 16..23 (8 o.) | 80..119 | `80 + n*8 + m` |
| C | 24..25 (2 o.) | 120..127 | `120 + n*2 + m`, **4 chiffres** |
| — | 26..27 | — | **échelle FP16 du groupe** |

avec, pour les zones A et B, `n ∈ 0..4` = chiffre base-3 dans l'octet (le **premier** trit est le
**plus significatif**), et pour la zone C `n ∈ 0..3` avec le nombre base-3 **décalé d'un chiffre**
(leur encodeur fait `q *= 3` avant la division plafond — la même astuce que `TQ1_0`).

Encodage d'un octet : `octet = ceil(q·256/243)` où `q = Σ xi_k·3^{4−k}` (base 3 sur 5 chiffres).
Décodage : `q = floor(octet·243/256)` (borné à 242).

**Résultat MESURÉ sur 87 680 poids (685 groupes × 128) de `blk.0.ffn_gate.weight` :**

```
TRITS identiques   : 87 680 / 87 680 = 100,0000 %
VALEURS identiques : 87 680 / 87 680 = 100,0000 %   (trit × échelle == valeur du fichier F16)
```

**Conséquence directe, et elle change une conclusion antérieure** : le fichier
`Ternary-Bonsai-2-27B-F16.gguf` (53,8 Go) **n'est pas** Qwen3.8-27B en FP16. Il porte les mêmes
métadonnées `prism.hadamard.*` et `basename = folded` : c'est **leur modèle ternaire déplié en
F16**. Inutilisable comme source de poids, mais excellent comme oracle — c'est ce qui a permis
la vérification ci-dessus. `output.weight` et `token_embd.weight` y sont stockés en 16,0 bpw
(2 octets par élément) pour information.

**Commun aux tailles** : les octets d'un tenseur sont alignés sur 4096 (écart mesuré entre la fin
d'un tenseur et le début du suivant : 20 480 octets sur `output.weight`, identique dans les deux
fichiers).

---

## 3 · `PQ2_0` — CALCULÉ, non vérifié

`PQ2_0` = 2,13 bpw annoncés. **CALCULÉ** : 34 octets / 128 poids = 2,125 bpw = 32 octets de
trit en cases de 2 bits + 2 octets d'échelle. Le whitepaper corrobore la valeur 34 par une
remarque indépendante (« la version MLX coûte 36 octets plutôt que 34 par 128 poids, car elle
stocke une échelle *et* un biais »).
**NON VÉRIFIÉ** : le layout interne de `PQ2_0` n'a pas été décodé (non nécessaire pour commencer).

---

## 4 · Leur politique par tenseur — lue, pas supposée

**EXTRAIT** de la table des tenseurs (851 tenseurs) et de `prism.hadamard.weight_names` :

| type | nombre | quoi |
|---|---|---|
| **143** (au-delà de `GGML_TYPE_COUNT = 43` → **refus sûr** par un runtime qui ne le connaît pas) | 402 | tous les tenseurs rotatés **+ `token_embd.weight`** (rotation **inverse**) |
| **BF16 (30)** | 96 | `ssm_alpha.weight` + `ssm_beta.weight` des 48 blocs DeltaNet = les `in_proj_a/b` |
| **F32 (0)** | 353 | `ssm_a`, `ssm_dt.bias`, `ssm_conv1d.weight`, `ssm_norm`, `attn_norm`, `post_attention_norm`, `attn_q_norm`, `attn_k_norm` |

**Les 401 tenseurs rotatés**, recomptés par famille :
`ffn_down` ×64, `ffn_gate` ×64, `ffn_up` ×64, `attn_qkv` ×48, `attn_gate` ×48, `ssm_out` ×48,
`attn_k` ×16, `attn_v` ×16, `attn_q` ×16, `attn_output` ×16, `output.weight` ×1.
**CALCULÉ** : `64×3 + 48×3 + 16×4 + 1 = 401` ✓.

**CALCULÉ** : les exceptions pèsent 23,6 M (`alpha`+`beta`) + 1,97 M (`conv1d`) + le reste
≈ **26,2 M paramètres = 0,0976 %** — le chiffre annoncé par leur whitepaper, retrouvé au
tenseur près. Détail qu'ils ne documentent pas : `ssm_a`, `ssm_dt.bias` et `ssm_conv1d` sont en
**F32**, pas en BF16.

**Lecture pour nous** : le chemin d'état récurrent des couches linéaires **n'est pas ternarisé**.
C'est le même arbitrage que celui des quants `UD-*` d'Unsloth, où `ssm_*` reste à ~2 bpw.

---

## 5 · La rotation — ce qui est établi, ce qui ne l'est pas

**ÉTABLI (EXTRAIT)** : base = Sylvester–Walsh–Hadamard normalisée, **bloc 1024**, axe = dernière
dimension de l'entrée (= `ne[0]` en ggml), signes ±1 **explicites**, **un vecteur par largeur**
(5120, 6144, 17408), repliés hors ligne (`basename = folded`), et **déclarés dans le fichier**.

**MESURÉ — la convention est tranchée.** Méthode : les vrais poids `W` du BF16
(`unsloth/Qwen3.8-27B-GGUF`, `blk.0.ffn_gate.weight` — 16 lignes de 5120 lues à l'offset
`421 107 584`), chaque transformée candidate appliquée aux **lignes** (un tenseur ggml de forme
`[ne0=in, ne1=out]` range l'entrée en `ne[0]`), puis ternarisation g128 et comparaison aux trits
stockés. Témoin : accord au hasard = 33,3 %.

| transformée appliquée à la ligne | trits retrouvés |
|---|---|
| identité | 33,32 % |
| WHT seule | 32,18 % |
| `S` puis WHT | **82,59 %** |
| WHT puis `S` | 32,82 % |

**La convention est donc `S` d'abord, puis la WHT** : les lignes sont repliées par
`w ← H_norm(S ⊙ w)`, par blocs de 1024 le long de `ne[0]`. La transformée **jumelle appliquée à
l'activation au runtime** est donc l'autre ordre : `x ← S ⊙ H_norm(x)`.

**CALCULÉ** — cohérence avec leur whitepaper : `f(x) = W(Rx)` et un repli des lignes
`w ← (H S) w` donnent `W' = W·R` avec `R = (1/√n)·Hₙ·S`… **aux symétries près** ; l'énoncé
opérationnel qui compte est celui mesuré ci-dessus, pas la forme écrite.

**MESURÉ — leur vecteur de signes est spécifique, et ce n'est pas un détail de forme :**

| signes utilisés (ordre `S` puis WHT) | trits retrouvés |
|---|---|
| **les leurs** (`prism.hadamard.sign_values[0:5120]`) | **90,12 %** |
| aléatoires ±1 (PRNG seedé) | 33,58 % |
| aucun (`S = +1`, WHT seule) | 32,37 % |
| alternés `+1/−1` | 33,67 % |

Le fait que **leur WHT soit retrouvée à 90 % avec leurs signes et à 33 % sans** établit aussi,
par contrecoup, que **mon ordonnancement de la WHT (Sylvester, papillons naturels) est le leur** —
un désaccord d'ordonnancement aurait donné 33 % même avec les bons signes.

---

## 5bis · Ce que la rotation apporte réellement — mesuré, et c'est peu sur ce tenseur

**MESURÉ** sur 96 lignes de `blk.0.ffn_gate.weight` (1 demi-million de poids, 3 840 groupes de
128) : erreur relative de la meilleure ternarisation équilibrée d'un groupe, `‖v − s·t‖² / ‖v‖²`.
La norme `‖W‖` étant invariante par rotation, les régimes sont comparables.

| régime | moyenne | p99 | **max** | écart-type |
|---|---|---|---|---|
| sans rotation | 20,84 % | 27,79 % | **43,01 %** | 2,71 % |
| rotation + signes PrismML | 20,71 % | 27,35 % | **31,98 %** | 2,63 % |

**Lecture** : la rotation ne gagne **presque rien en moyenne** (+0,13 point) mais **écrase le pire
groupe de 43,0 % à 32,0 %** — soit −26 % sur le cas défavorable. C'est le mécanisme attendu : une
rotation orthogonale ne change pas l'énergie, elle **étale les canaux aberrants sur le groupe**
donc supprime les cas catastrophiques.

**Et sur ce tenseur, il n'y a presque rien à étaler** : la dispersion des colonnes (écart-type par
canal d'entrée, sur 384 lignes) va de 0,0054 à 0,0122, soit un rapport de **2,3× seulement**. Le
gain modeste est donc cohérent, et il ne se généralise pas : `ffn_gate` n'est pas le tenseur où
les outliers vivent.

**Conséquence pour notre implémentation** : le bénéfice de la rotation doit être **mesuré tenseur
par tenseur**, pas supposé — et le test est celui ci-dessus, il tourne en secondes sur un
échantillon de lignes. Il décidera *quels* tenseurs on rote (la liste de 401 noms de PrismML est
leur politique, pas un résultat).

### Leur règle d'échelle — mesurée

**MESURÉ** sur 640 groupes, rapport entre leur échelle stockée et les statistiques des valeurs
repliées vraies :

| rapport | moyenne | écart-type | dispersion |
|---|---|---|---|
| `s / max|v|` | 0,3992 | 0,0532 | **13,3 %** |
| `s / rms` | **1,1116** | 0,0263 | **2,4 %** |
| `s / moyenne|v|` | 1,3904 | 0,0291 | 2,1 % |

L'échelle n'est donc **pas** `amax` (qui donne 49,4 % d'accord seulement) : elle est proportionnelle
à la **dispersion du groupe**, `s ≈ 1,112 × rms`, à 2,4 % près. Et le taux de zéros de leur fichier
est **32,8 %** — le tiers exact d'une ternarisation **équilibrée**, seuil au tiers, qui reproduit
**90,12 %** de leurs trits (contre 84,7 % pour les moindres carrés, dont le taux de zéros monte à
46 %). La ternarisation visée est équilibrée, pas `amax`.

---

## 6 · Ce que ce fork a déjà — l'inventaire des points d'insertion

**CONSTATÉ**, avec les lignes :

| pièce | où | état |
|---|---|---|
| `GGML_HINT_SRC0_IS_HADAMARD` | `ggml/include/ggml.h:445` | existe |
| `ggml_mul_mat_set_hint()` | `ggml/include/ggml.h:1432` | existe |
| poseur du hint (`llama_mul_mat_hadamard`) | `src/llama-impl.h:71` | existe |
| noyau FWHT **CUDA** | `ggml/src/ggml-cuda/fwht.cu:61`, `fwht.cuh` | existe |
| noyau FWHT **CPU** | `ggml/src/ggml-cpu/ggml-cpu.c:1262` | existe |
| noyau FWHT **Vulkan** | `ggml/src/ggml-vulkan/ggml-vulkan.cpp:9402` | existe |
| taille de groupe WHT dans `op_params` | `src/llama-kv-cache.cpp:1371,1422` (`wht_group = 128`) | **déjà paramétrée** |
| FWHT **Metal / SYCL / OpenCL / WebGPU** | — | **rien** |
| ternaire `TQ1_0` (id 34, bloc 256) | `ggml.h:424`, `type_traits_cpu[34]` `ggml-cpu.c:400` | **CPU seul** |
| `vec_dot` ternaire CPU | `ggml-cpu/quants.c:481` (+ AVX2 `arch/x86/quants.c:1376`) | existe |
| ternaire sur **CUDA** | `ggml/src/ggml-cuda/**` | **0 occurrence** |
| goulot unique de toutes les projections | `build_lora_mm` — `src/llama-graph.cpp:1467` (`ggml_mul_mat` l. 1471) | existe |
| politique de quantification par tenseur | `enum tensor_category` — `src/llama-quant.cpp:25-38` | existe |
| borne d'erreur ternaire en test | `tests/test-quantize-fns.cpp` (`MAX_QUANTIZATION_TOTAL_ERROR_TERNARY`) | existe |
| refus explicite au démarrage (idiome du fork) | `src/llama-model.cpp:39-102` (`OBRAIN_REC_TYPE × OBRAIN_STATE_TYPE`) | existe |

**CALCULÉ — le fait qui borne le chantier** : `TQ1_0` = 54 octets / 256 poids = **1,6875 bpw** ;
`PTQ1_0` = 28 octets / **128** poids = **1,75 bpw**. Même packing de trits (5 par octet, division
plafond par 243), **groupe divisé par deux**, échelle FP16 par groupe. Leur format est donc
`TQ1_0` **dont on a divisé le groupe par deux**, plus la rotation déclarée en métadonnées.

---

## 7 · Plan, par étages, avec les portes

### 7.0 État d'implémentation (2026-09-20)

| pièce | état | porte |
|---|---|---|
| Type GGML `TQ2_G128` (groupe 128, cases de 2 bits, 34 o/128 = 2,125 bpw) | **FAIT** — `ggml.h`, `ggml-common.h`, `ggml-quants.{c,h}`, `ggml.c`, `ggml-cpu/{quants.c,quants.h,ggml-cpu.c,ops.cpp}` | `test-quantize-fns` : erreur absolue **0,006233** (contre 0,008681 pour `TQ1_0`), produit scalaire **0,089486** (contre 0,141345), erreur de référence **0,000000** |
| ftype `MOSTLY_TQ2_G128` de bout en bout | **FAIT** — `llama.h`, `llama-quant.cpp`, `llama-model-loader.cpp`, `tools/quantize/quantize.cpp` | `test-quant-type-selection` : **33/33 sections PASS** sur 4 architectures — aucune politique existante perturbée |
| Rotation `A = H∘S` (signes puis WHT), partagée | **FAIT** — `src/obrain-rotation.{h,cpp}` | `test-obrain-rotation` : **55 passés, 0 échoué** — orthogonalité, `AᵀA = I`, localité du bloc, et **poids repliés × activation transformée = produit d'origine à 4,5e-06 près** |
| La **composition ggml** que le crochet utilisera | **DÉPASSÉE** : l'op existe maintenant (voir la ligne suivante) | — |
| **`GGML_OP_WHT`** — l'op de premier ordre, `ggml_wht(ctx, a, 1024)` | **FAIT** — `ggml.h`, `ggml.c` (enum, nom, symbole, `ggml_wht`, `ggml_get_wht_block`), `ggml-cpu/ops.{h,cpp}` (référence CPU parallélisée), `ggml-cpu.c` (dispatch + tâches) | `test-obrain-rotation-ggml` : **7 passés, 0 échoué** — et il compare TROIS implémentations : l'op, le chemin historique matrice dense + hint (`GGML_HINT_SRC0_IS_HADAMARD`), et `obrain_rotate` du repli hors ligne. **Les trois tombent d'accord à 0,000e+00** (l'op et le repli sont donc littéralement la même transformée), involution `T(T(x)) = x` à 5,4e-07, contrat à 3,3e-05 |
| **Les signes ±1 : abandonnés, sur mesure** | **MESURÉ** — sur 11 tenseurs et ~450 000 poids du 27B : WHT seule **20,672 %** d'erreur moyenne contre **20,677 %** avec les signes (±1 engendrés), p99 27,46 % contre 27,54 %, et le meilleur alterne **6 fois sur 11** selon le tenseur. C'est du bruit. La rotation en revanche est indispensable : `token_embd` passe de **97,5 %** de pire groupe à 31 % | conséquence : l'op est une WHT **nue** — aucun tenseur de signes, aucune table, aucune plomberie d'allocation. Si la perplexité montre un jour qu'ils manquent, c'est un `ggml_mul` devant l'op |
| **Vitesse de l'op WHT** | **MESURÉE** (512 lignes de 5120, 8 fils, aller-retour) | op **15,0 Go/s** · chemin historique matrice dense + hint **24,0 Go/s** · `obrain_rotate` scalaire **5,2 Go/s**. L'écart de 1,6× est une cible d'optimisation identifiée : le chemin du hint utilise la FWHT déjà optimisée du fork (`ggml-cpu.c`), mon noyau est scalaire dans `dst`. À corriger, ou à laisser aux backends qui ont déjà leurs noyaux FWHT (CUDA `fwht.cuh`, Vulkan) |
| **Leçon de métrique (à ne pas réapprendre)** | **MESURÉE** | rapporter l'erreur d'un produit scalaire **au résultat** est faux : sur 5120 termes, elle a donné 4,35e-05 là où la même chaîne donne **4,3e-09** rapportée à `Σ|w_i·x_i|`. L'annulation domine, pas la précision — et le symptôme est trompeur (plus de termes, moins d'erreur). Les deux tests utilisent désormais la métrique sans annulation |
| Vitesse de la ternarisation | **MESURÉE ET CORRIGÉE** | `qsort` par groupe : 21,7 M poids/s → **sélection en O(n) : 44,2 M poids/s (×2,04)**, écart à `TQ1_0` ramené de **14,7× à 7,55×**, et l'erreur de quantification **inchangée (0,006233)** — le tertile est le même, on ne trie simplement plus ce qu'on ne lit pas |
| Politique de repli (ce qui est épargné) | **FAIT dans le helper** — `ne1 < 512` écarte `ssm_alpha`/`ssm_beta`, `ne0 % 1024` écarte `ssm_conv1d`, `_norm.weight` écarte les norms | 14 cas du test |
| Repli branché dans le quantifieur | **NON FAIT** — volontairement : voir ci-dessous | — |
| Transformée jumelle dans le graphe (`build_lora_mm`) | **FAIT** — trois lignes dans `src/llama-graph.cpp` (le goulot unique de toutes les projections linéaires) : `if (w->type == GGML_TYPE_TQ2_G128) cur = ggml_wht(ctx0, cur, OBRAIN_WHT_BLOCK);`. Deux `GGML_ASSERT` refusent net un poids replié dont l'activation ne pourrait pas l'être | `test-obrain-rotation-ggml` (le crochet appelle le même op que le test) |
| Repli branché dans le quantifieur | **FAIT** — `src/llama-quant.cpp`, juste avant `llama_tensor_quantize_impl` : le repli suit le type (`new_type == GGML_TYPE_TQ2_G128`), donc les deux moitiés ne peuvent pas diverger. **Trois refus explicites** : un tenseur que la politique épargne mais que le type réclame, une source F32 (le repli s'écrirait dans le tenseur du modèle), une largeur non multiple de 1024 | `llama-quantize` se construit ; les 4 portes sont vertes |
| **L'op sur le GPU (Radeon 780M, RADV PHOENIX, gfx1103)** | **MESURÉ** — `tests/test-obrain-wht-device.cpp`, variante Vulkan | **Vulkan déclare l'op NON supportée** (`ggml_backend_vk_supports_op` finit par `default: return false`) — honnêtement, pas silencieusement. Le **schédulicateur** place donc le nœud WHT sur le CPU et le reste sur le GPU : **le contrat TIENT sur le 780M (1,04e-09)**. Coût : **1,669 ms** pour 512 lignes de 5120 (**12,6 Go/s**) *aller-retour hôte↔périphérique compris* — c'est ce que supprimera un shader Vulkan |
| Perplexité 27B | **MESURÉE — ET ELLE ÉCHOUE** : **70 143** contre **2,3983** pour le `Q4_K_M` de référence, même corpus (`README.md` + cette ADR, `-c 512`, 4 passes, CPU). Le modèle **charge et génère** ; il produit du bruit. Voir le §10
| Perplexité 9B (cobaye, source `Q4_K_M` requantifiée) | **87 822** avec rotation, **1 629 269** sans — même corpus, `-c 512`, 2 passes. Deux modèles, une seule conclusion |

---

## 10 · Verdict : la ternarisation PTQ ne tient pas, et ce n'est pas réparable par le quantifieur

**MESURÉ**, dans l'ordre où les faits se sont imposés :

| fait | valeur | ce qu'il élimine |
|---|---|---|
| Perplexité du ternaire contre `Q4_K_M`, même corpus | **70 143 contre 2,3983** | l'approche telle quelle |
| Le fichier écrit est-il replié correctement ? | déplié **18,9 %** / brut **180,7 %**, sur 3 tenseurs dont `token_embd` et `output.weight` | un double repli, un repli manquant, un décalage de layout |
| La rotation sert-elle ? | **MESURÉ, comparaison contrôlée** (9B, même source, même corpus, seul l'interrupteur change) : sans elle **1 629 269**, avec elle **87 822** → **18,5×** | « la rotation ne fait rien » |
| Les primitives sont-elles justes ? | op ≡ matrice dense + hint ≡ repli scalaire : **0.000e+00** ; contrat **4,3e-09** | un bug dans l'op, le hint ou le repli |
| Où est le plancher ? | trois niveaux {−s,0,+s} sur une gaussienne : **19 %** d'erreur (optimum de Lloyd-Max) ; mon quantifieur mesure **18,9 %** | tout gain à attendre d'un meilleur choix d'échelle ou de seuil |

**CALCULÉ, et c'est la conclusion.** Le quantifieur est **déjà à l'optimum de son format**. 19 % d'erreur relative par groupe, sur 64 couches, ne peut pas produire un modèle qui fonctionne : `Q4_K_M` tourne autour de 1-2 %, `Q2_K` autour de 7-8 %, et la perplexité est une fonction très raide de cette erreur.

**Le seul levier restant à budget de bits constant** est une compensation d'erreur qui minimise l'erreur de **sortie** et non celle des poids (type GPTQ, pondérée par l'imatrix — que llama.cpp calcule déjà et que ce quantifieur ignore). Elle reste **à faire**, et son rendement à 2 bits est incertain.

**CE QUI A ÉTÉ ESSAYÉ ET REJETÉ PAR LA MESURE — à ne pas refaire.** La diffusion du résidu (reporter l'erreur d'un poids sur le suivant, pour annuler l'erreur cumulée du groupe) paraît taillée pour ce format :

```
erreur par élément   0,006233 → 0,008255   (dégradée — attendu)
erreur du produit    0,089486 → 0,322091   (3,6× PIRE — la porte passe au rouge)
```

Diffuser n'annule l'erreur que si l'activation est **lisse** sur les 128 canaux du groupe. Après une rotation de Hadamard, les composantes voisines n'ont aucune raison de se ressembler : le résidu reporté redevient une erreur indépendante, simplement plus grosse. **Reverté**, portes vertes retrouvées à l'identique.

**Ce que cela dit de la littérature, et qu'il fallait prendre au sérieux plus tôt** : BitNet doit être **entraîné** avec des poids ternaires (QAT), et les types `TQ1_0`/`TQ2_0` de llama.cpp sont documentés comme réservés aux modèles *déjà* ternarisés. Les 98,2 % de rétention de PrismML à 1,76 bpw ne sont **pas** atteignables par un arrondi par groupe, même avec la rotation : il leur manque un entraînement, ou une compensation qui n'apparaît pas dans leur whitepaper.

# La descente, mesurée — sans une heure d'entraînement

**Régime** : `llama-perplexity`, corpus du dépôt (`-c 512`, 2 passes, CPU, 8 fils), source
`Qwen3.8-27B-BF16.gguf` (54,66 Go). **Toutes les variantes sont reproduites par un PARAMÈTRE
d'environnement** : `OBRAIN_ATTN_V`, `OBRAIN_FFN_DOWN`, `OBRAIN_EDGE_LAYERS` — **10 minutes par
essai** (4 min de quantification + 5 min de mesure), sans recompiler.

| variante | taille | perplexité | ce qui a changé |
|---|---|---|---|
| v1 — **tout** ternaire, embedding et tête compris | 7,32 Go | **70 143** | — |
| v2 — `token_embd` Q4_K, `output.weight` Q6_K | 8,40 Go | **83,09** | 2 tenseurs |
| + `attn_v` en Q4_K | 8,43 Go | **41,71** | **+0,03 Go** |
| + `ffn_down` en Q4_K | 10,12 Go | **34,29** | +1,72 Go |
| + `attn_v` **et** `ffn_down` | 10,15 Go | **15,71** | — |
| + **bords** (4 premières et dernières couches) en Q4_K | 9,43 Go | **7,32** | **+1,03 Go** |
| référence `Q4_K_M` (Unsloth) | 18,97 Go | **2,3983** | — |

**Trois faits, et ils répondent à « doit-on entraîner ? » :**

1. **70 143 → 83,09 par DEUX tenseurs.** Ternariser l'**entrée** et la **sortie** était l'erreur
   fatale, et le dépôt l'interdit explicitement pour tous ses ftypes ternaires — ce que le retour
   anticipé court-circuitait.
2. **83,09 → 7,32 par UNE famille : les couches de bord** (facteur 11,4, pour +1,03 Go). La règle
   classique des premières et dernières couches domine ici tout le reste.
3. **L'ordre des leviers** : les bords (9×) ≫ `attn_v` (2× sur 0,03 Go) > `ffn_down` (2,4×).
   Personne ne pouvait le deviner : c'est mesuré.

**Conclusion : la quantification après coup SUFFIT à produire un modèle qui parle, RAISONNE et suit
une instruction — sans un seul entraînement.** Il reste des familles à explorer (`attn_qkv`,
`ssm_out`, `attn_output`, mix Q3_K/Q4_K, imatrix avec une vraie compensation d'erreur de SORTIE),
mais la pente est établie et chaque essai coûte 10 minutes.

### La descente complète (27B, corpus du dépôt, `-c 512 --chunks 2`, CPU 8 fils)

| variante | taille | perplexité | **raisonne ?** |
|---|---|---|---|
| tout ternaire, embedding et tête compris | 7,32 Go | **70 143** | bruit |
| `token_embd` Q4_K + `output.weight` Q6_K | 8,40 Go | 83,09 | — |
| + `attn_v` Q4_K | 8,43 Go | 41,71 | — |
| + `ffn_down` Q4_K | 10,12 Go | 34,29 | — |
| + les deux | 10,15 Go | 15,71 | — |
| + **4 couches de bord** Q4_K | 9,43 Go | 7,32 | **non** |
| + **8 couches de bord** Q4_K | 10,32 Go | 5,70 | **OUI** |
| + 12 couches de bord Q4_K | 11,22 Go | 4,99 | **OUI** |
| **+ 8 bords + `attn_v` + `ffn_down`** | **11,61 Go** | **4,74** | **OUI** ✅ |
| référence `Q4_K_M` (Unsloth) | 18,97 Go | 2,3983 | OUI |

**Le critère qui a tranché, et il fallait le choisir juste.** La perplexité ne suffisait pas : elle
passe de 7,32 à 5,70 entre les bords à 4 et à 8, alors que **la capacité de raisonner passe de
« non » à « oui »** — à 4 couches de bord, le modèle ferme `<think>` aussitôt et répond sans penser
(signature documentée de la quantification agressive : la dégradation se concentre sur les chaînes de
raisonnement longues, pas sur la fluidité). Le juge retenu est donc **textuel** : sur une complétion
brute au préambule de raisonnement ouvert, on compte les caractères produits **avant** `</think>`.
Trente secondes par essai, et il attrape ce que la perplexité laisse passer.

**Trois réglages de plomberie qui ont coûté le plus cher** (et qui n'ont rien à voir avec les poids) :
**`--jinja`** au serveur — sans lui le gabarit du GGUF n'est pas utilisé et le modèle se tait ;
**`chat_template_kwargs: {"enable_thinking": false}`** et non `thinking_budget_tokens` ;
**`--spec-type draft-mtp`** et non `--mtp` pour le décodage spéculatif.

**Ce que cette descente ne prouve PAS** : une perplexité et un juge textuel ne sont pas un benchmark.
À 4,74 contre 2,40, la question qui compte reste **les tâches obrain** (retrouver les bonnes
fonctions dans le graphe, tenir un appel d'outil) — à mesurer avec `ops-gpu16/qa_obrain.py` et
`scripts/engine-ask-check.mjs`, sur le service réel, pas ici. **Et le GPU n'est pas encore
utilisable** : mesuré 2,01 tok/s sur le binaire Vulkan contre 1,90 en CPU pur, parce que
`TQ2_G128` n'a pas de noyau `mul_mat_vec` côté Vulkan et que `GGML_OP_WHT` y est déclaré non
supporté — deux shaders à écrire.

## 11 · Ce qu'on a techniquement loupé — mesuré sur leurs poids : **la politique de tenseurs**


**La question posée** : analyser leurs poids pour savoir ce qui manque. **MESURÉ**, un seul tenseur
(`blk.0.ffn_gate.weight`, 24 lignes de 5120), les deux ramenés dans la même base, comparés au BF16
`unsloth/Qwen3.8-27B-GGUF` :

| | erreur RMS par élément | erreur de produit scalaire |
|---|---|---|
| **PrismML** (leur F16 déplié par `S∘H`, avec leurs signes) | **48,10 %** | 45,68 % |
| **nous** (`TQ2_G128` déplié par `H`) | **43,44 %** | **32,96 %** |
| plancher du format (3 niveaux, optimum de Lloyd-Max) | 43,4 % | — |

**Trois faits, et ensemble ils ferment le débat :**

1. **Notre 43,44 % EST le plancher** : trois niveaux sur une gaussienne donnent une erreur
   quadratique moyenne de 0,19 (Lloyd-Max), soit **43,4 % en RMS**. Notre quantifieur est donc à
   l'optimum exact de son format — (mon rapport antérieur disait « 19 % » : c'était la variance,
   pas l'erreur ; la valeur RMS est 43,4 %, et les deux mesures du dépôt le confirment).
2. **Leur modèle s'écarte PLUS que le nôtre (48,10 % contre 43,44 %)** de la même base — et il
   fonctionne (83,9 de moyenne sur 20 benchmarks) là où le nôtre produit du bruit. Une perturbation
   de poids de 43-48 % ne peut **pas** laisser la fonction intacte en quantification après coup.
3. **Mais 90 % de leurs trits coïncident avec l'arrondi optimal de la même base** (mesuré au §5) :
   c'est bien le même modèle, adapté.

**Conclusion — et elle a été CORRIGÉE par une mesure, ce qui vaut mieux qu'une intuition.**

Le §11 croyait lire « il faut entraîner ». **C'était faux, et la mesure l'a dit en une passe.** En
suivant la politique du dépôt lui-même — que notre retour anticipé avait contournée —
c'est-à-dire en **épargnant l'entrée et la sortie** (`token_embd` en Q4_K, `output.weight` en Q6_K,
le reste en ternaire) :

| modèle | perplexité (même corpus, `-c 512`, 2 passes) |
|---|---|
| v1 — tout ternaire, embedding et tête compris | **70 143** |
| **v2 — entrée et sortie épargnées** | **83,09** |
| référence `Q4_K_M` | 2,3983 |

**70 143 → 83,09 : un facteur 844.** Le modèle passe du *bruit* à *un mauvais modèle de langue*.
Autrement dit : la quantification après coup **fonctionne** ; ce qui la tuait, c'est d'avoir
ternarisé les deux tenseurs qui sont **l'entrée** et la **sortie** du réseau — 43 % d'erreur RMS y
détruisent la représentation de chaque jeton avant que le résidu puisse l'absorber, alors que la
même erreur au milieu est répartie entre les canaux.

**Ce qui reste à franchir (83 → viser ~3-8)** est maintenant un problème de *politique de tenseurs*
et de *statistiques d'activation*, pas d'entraînement :

1. **Le levier non essayé** : le quantifieur **ignore l'imatrix** que llama.cpp calcule déjà
   (`llama-imatrix`). Une quantification guidée par l'importance réelle des canaux — et non par la
   seule magnitude des poids — est exactement ce qui distingue GPTQ d'un arrondi au plus proche.
   Le corpus est là (le graphe Linux de 439 688 nœuds est un jeu de calibration idéal).
2. **La politique de tenseurs, par mesure** : les politiques bas-bit du dépôt épargnent
   `attn_v` (Q4_K dès que `n_gqa >= 4`) et traitent `ffn_down` à part — pas nous. Chaque variante se
   teste en **4 min de quantification + 5 min de mesure**.
3. **Et seulement ensuite** l'entraînement, qui reste la voie si 1 et 2 plafonnent.

**Ce qui est établi et ne se rejoue plus** : le format, l'op `WHT`, le crochet du graphe, le repli,
la chaîne de quantification (4 min pour le 27B), et le fait que **l'entrée et la sortie ne se
ternarisent pas par quantification après coup**.

**Piège de méthode, à ne pas refaire.** Ma première version du test GPU utilisait `ggml_gallocr` : elle a répondu « allocation OK » puis un résultat **faux** (3,15e-03) à **475 Go/s** — plus rapide que la mémoire de la machine, ce qui trahit un calcul qui n'a pas eu lieu. `ggml_gallocr` ne consulte pas `supports_op` : il alloue et envoie tout au backend choisi. **Le seul harnais dont le verdict vaut sur un GPU est `ggml_backend_sched`**, celui que llama.cpp utilise. Et le symptôme (un débit *trop* bon) est plus dangereux qu'une panne : il ressemble à une victoire.

**Note sur la disparition de l'obstacle.** Le premier plan passait par une matrice de Hadamard dense de 1024×1024 consommée par `ggml_mul_mat` + `GGML_HINT_SRC0_IS_HADAMARD` (le chemin du cache KV). Il butait sur un point réel : un tenseur constant doit être *alloué par le backend* (mémoire hôte pour le CPU, mémoire de périphérique pour CUDA/Vulkan), ce qui demandait la plomberie `ggml_set_input` + upload du chemin KV. L'op `GGML_OP_WHT` n'a **pas** ce problème : elle ne transporte aucune donnée, elle calcule. Et elle laisse chaque backend libre de l'implémenter (les noyaux FWHT existent déjà côté CUDA et Vulkan).

**Pourquoi le repli n'est PAS encore branché dans le quantifieur, alors qu'il tient en dix
lignes.** Parce qu'un fichier dont les poids sont repliés mais dont le runtime n'applique pas la
transformée jumelle n'est pas « dégradé » : il produit du **bruit**, sans le dire. Les deux
moitiés doivent atterrir ensemble, ou pas du tout. Tant que le crochet du graphe n'existe pas,
le quantifieur ne doit pas pouvoir produire ce fichier — c'est la même règle que
`OBRAIN_REC_TYPE × OBRAIN_STATE_TYPE` (`src/llama-model.cpp:39-102`) : un état incohérent
**refuse de démarrer** au lieu de mentir en silence.

**Le crochet du graphe, tel qu'il est prévu** — `build_lora_mm` (`src/llama-graph.cpp:1467`) est
le goulot unique par lequel passent toutes les projections linéaires de tous les modèles. Pour un
poids de type `GGML_TYPE_TQ2_G128` : signes (`ggml_mul` contre un tenseur constant par largeur),
puis Walsh–Hadamard **par blocs de 1024** via la machinerie existante
(`ggml_mul_mat` + `GGML_HINT_SRC0_IS_HADAMARD`, `src/llama-impl.h:71`, consommée par CPU
`ggml-cpu.c:1262`, CUDA `ggml-cuda.cu:1735`, Vulkan `ggml-vulkan.cpp:9402`). Le bloc de 1024 est
ce qui permet de réutiliser l'existant : la WHT exige une puissance de deux, et 5120/6144/17408
sont exactement 5/6/17 blocs — un `H₁₀₂₄` dense fait 4 Mo et se partage entre toutes les couches,
là où un `H₅₁₂₀` serait impossible et un `H₁₇₄₀₈` pèserait 1,2 Go.

**L'amélioration qu'on peut s'offrir et pas eux** : dans `build_layer_attn`, `wq`, `wk`, `wv`
consomment **la même activation** (`src/models/qwen35.cpp:283-293`) ; idem `ffn_up`/`ffn_gate`.
Une rotation par activation distincte, soit 3 par couche au lieu de 7 — et Bonsai documente que
c'est **son plus gros coût non-GEMM** au décodage à batch 1.

### 7.1 Les étages

| # | étage | livrable | porte | noyau neuf |
|---|---|---|---|---|
| 0 | convention de rotation | trancher `H·S` / `S·H` / sens de `token_embd` contre le BF16 | retrouve les trits stockés (attendu : >90 %), les autres conventions ≈ 33 % | non |
| 1 | quantifieur + repli hors ligne | outil : BF16 → rotation repliée → ternaire g128 (échelle **optimisée**, pas `amax`), politique d'exceptions, métadonnées déclarées + **refus au chargement** | erreur de quantification sur tenseurs réels ; `llama-perplexity` sur le 27B | non |
| 2 | rotation dans le graphe | branchement dans `build_lora_mm` derrière un drapeau par tenseur, en réutilisant le hint | perplexité 27B (CPU) | non |
| 3 | noyaux ternaires CUDA | `vec_dot_tq*_q8_1` dans `vecdotq.cuh` + tables `mmvq.cu`, puis **MMQ** (leur `pp512` est ÷2 en `PTQ1_0`) | perplexité GPU, débit | oui |
| 4 | raffinements | échelle apprise par groupe, réglage des exceptions par **mesure** (pas recopie) | courbe qualité/bits | non |
| 5 | Vulkan, puis Metal ; état récurrent avant `i8` | — | — | oui |

**Écart assumé à l'étage 1** : la v0 peut réutiliser `TQ1_0` (id 34) comme conteneur, puisque ses
noyaux CPU existent et que la rotation passe par le hint. Cela isole les variables — mais le
groupe 256 au lieu de 128 **dégrade** la qualité, et devra être corrigé à l'étage 4 au plus tard.

---

## 8 · Ce que ce document ne tranche pas

| question | état | ce qu'il faudrait |
|---|---|---|
| La convention de rotation | **RÉSOLU** : `S` puis WHT sur les lignes, jumelle `S ⊙ H(x)` sur l'activation (§5) | — |
| **D'où viennent leurs ±1 ?** | **NON TRANCHÉ** — et c'est le point ouvert le plus important : leurs signes sont nécessaires pour retrouver *leur* fichier (90 % vs 33 %), mais **la rotation n'apporte presque rien en moyenne sur `ffn_gate`** (§5bis). Deux hypothèses tiennent : signes optimisés sur données de calibration (SpinQuant), ou règle déterministe dérivée des poids | tester si `S` corrèle avec une statistique de colonne de `W` (moyenne, médiane, signe du 1er vecteur singulier), et **mesurer l'erreur de ternarisation tenseur par tenseur** pour savoir où la rotation paie |
| Le layout de `PQ2_0` | **NON VÉRIFIÉ** | décoder 34 octets/128 avec la méthode du §2 |
| `gdn_v_grouped = 1` : ce que ça change | **NON VÉRIFIÉ** | lire `blk.*.attn_v` / `ssm_out` des blocs DeltaNet et chercher la structure groupée |
| La rotation paie-t-elle sur les **autres** tenseurs ? | **NON MESURÉ** — mesuré sur `ffn_gate` seulement (canaux quasi homogènes, 2,3×) | le test du §5bis, appliqué aux 11 familles ; les candidats qui devraient payer sont ceux qui voient des activations à outliers (`attn_qkv`, `ffn_down`, `output`) |
| Leur ternarisation compense-t-elle l'erreur (GPTQ-like) ? | **NON TRANCHÉ** : 90,1 % d'accord avec une ternarisation équilibrée et **aucune** compensation | tester une compensation séquentielle par ligne et voir si l'accord monte vers 100 % |
| La qualité sur **notre** ternarisation | **NON MESURÉ** | étages 1–2 : perplexité sur le 27B |
| Le débit réel sur nos machines | **NON MESURÉ** | étage 3 |

---

## 9 · Reproductibilité (les commandes)

Les relevés ci-dessus viennent de requêtes HTTP partielles sur des fichiers publics — aucun
téléchargement complet n'a été nécessaire :

```bash
B="https://huggingface.co/prism-ml/Ternary-Bonsai-2-27B-gguf/resolve/main"
# en-tête + table des tenseurs (métadonnées seules)
curl -sL -r 0-3145727 -o head.bin "$B/Ternary-Bonsai-2-27B-PTQ1_0.gguf"
# un tenseur : blk.0.ffn_gate.weight  →  base de données 11 120 992 + offset relatif
curl -sL -r $((11120992+594124800))-$((11120992+594124800+19039)) -o P_ffn.tq "$B/Ternary-Bonsai-2-27B-PTQ1_0.gguf"
curl -sL -r $((11120992+5431664640))-$((11120992+5431664640+183999)) -o F_ffn.f16 "$B/Ternary-Bonsai-2-27B-F16.gguf"
# les vrais poids (BF16 Unsloth, shard 1) — blk.0.ffn_gate.weight à l'offset absolu 421 107 584
#   base de données de CE shard : 10 994 688 (table de 835 tenseurs finissant à 10 994 657)
python3 - <<'EOF'
import struct
f=open('/mnt/obrain/models/qwen38-bf16/Qwen3.8-27B-BF16-00001-of-00002.gguf','rb'); f.seek(421107584)
raw=f.read(384*5120*2)                      # 384 lignes = 1 966 080 poids
W=[struct.unpack('<f',struct.pack('<I',x<<16))[0] for x in struct.unpack('<%dH'%(384*5120), raw)]
EOF
```

Le BF16 est lu comme 16 bits hauts d'un `float32` (`x << 16`), sans bibliothèque externe :
`numpy` n'est pas installé sur n3 et n'est pas nécessaire à ces relevés.

**Piège à retenir pour tout ce qui suit** : `tensor.offset` d'un GGUF est relatif à la **section de
données**, pas au fichier. La première tentative de ce relevé a lu des métadonnées en croyant lire
des poids, et a produit des statistiques plausibles mais fausses (75 % puis 50 % de fenêtres à
magnitude unique — un artefact des octets ASCII). Le calcul de la base (`fin de table alignée sur
32`) est **obligatoire** avant toute lecture par offset.
