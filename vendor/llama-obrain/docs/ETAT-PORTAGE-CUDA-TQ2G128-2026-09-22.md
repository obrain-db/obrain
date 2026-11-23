# CUDA — `TQ2_G128` et la rotation WHT : le DGX est débloqué

**Date** : 2026-09-22. **Branche** : `feat/cuda-tq2g128` (dérivée de `feat/vulkan-tq2g128`).
**Arbre de travail** : `/home/triviere/projects/llama-obrain-cuda` (n3). **Clone de mesure** :
`dgx:~/projects/llama-obrain-cuda`.
**Machine** : DGX **`spark-c578`** — NVIDIA **GB10**, compute capability **12.1**, CUDA **13.2**,
20 cœurs, 121 Go unifiés, `CMAKE_CUDA_ARCHITECTURES=121`.

**Régime** : **MESURÉ** (exécution, commande citée) · **CONSTATÉ** (fichier:ligne) · **NON VÉRIFIÉ**.

---

## 1 · Le blocage, et le critère de recevabilité

`docs/TEST-DGX-WHT-DENSE.md` §0bis : `grep -rl TQ2_G128 ggml/src/ggml-cuda/` rendait **0**. Le modèle
ternaire retombait donc **intégralement sur le CPU du DGX**, et aucune mesure GPU n'avait de sens.

**Le critère fixé par ce document est tenu** : `ggml/src/ggml-cuda/` porte désormais le type, et
l'instrument le confirme (§3).

---

## 2 · Ce qui a été écrit — deux pièces, une garde

| # | fichier | quoi |
|---|---|---|
| 1 | `ggml-cuda/convert.cu` | `dequantize_block_tq2_g128` (un fil par octet de `qs`, 32 fils par bloc de 128 poids) + `dequantize_row_tq2_g128_cuda`, enregistré dans les **trois** tables (`bf16` / `fp16` / `f32`) |
| 2 | `ggml-cuda/ggml-cuda.cu` | le type entre dans la liste `MUL_MAT` de `supports_op` |
| 3 | `ggml-cuda/mmvq.cu` | **garde** : `ggml_cuda_should_use_mmvq` rend `false` pour `TQ2_G128` — **constatée nécessaire au banc** (§3) |
| 4 | `ggml-cuda/wht.cu` + `.cuh` | `GGML_OP_WHT` : un bloc CUDA par ligne, `block` fils, mémoire partagée, papillon `h = 1, 2, 4, …` puis normalisation `1/sqrt(block)` — sémantique copiée de `ggml_compute_forward_wht_f32` (`ggml-cpu/ops.cpp`), **cas général** (`block < ne[0]` compris) |
| 5 | `ggml-cuda/ggml-cuda.cu` | le cas dans le dispatch des opérations **et** dans `supports_op` |

**Sémantique ternaire**, copiée de la référence CPU `dequantize_row_tq2_g128` (`ggml-quants.c:2592`)
**et** du shader Vulkan déjà vérifié : le poids `i` vit dans les bits `2*(i%4)` de `qs[i/4]` ;
`0 → −1`, `1 → 0`, `2 → +1`, `3` replié sur `+1` ; **une seule échelle FP16** par groupe de 128.

**Chemin emprunté** : déquantification puis cuBLAS — celui que le document recommandait — parce que
`ggml_cuda_should_use_mmq` ne connaît pas le type.

---

## 3 · Ce qui est MESURÉ au niveau des opérations

Commande (sur le DGX, `build-cuda` reconstruit à chaque étape) :

```bash
build-cuda/bin/test-backend-ops -o MUL_MAT -p tq2_g128
build-cuda/bin/test-backend-ops -o WHT
```

| mesure | résultat |
|---|---|
| `MUL_MAT` × `tq2_g128` sur **CUDA0** | **15/15**, y compris `m=5120,n=1,k=5120`, `m=6144,n=1,k=6144`, `m=5120,n=32,k=5120`, `m=6144,n=8,k=6144`, `m=128,n=1,k=1280` |
| `WHT` sur **CUDA0** | **13/13**, y compris `ne=[1024,1,1,1] block=128` — le cas que Vulkan **refuse** (son shader ne sert qu'une ligne entière) |

**La garde MMVQ a été constatée avant d'être écrite**, et c'est le premier résultat du banc :

```
mmvq.cu:1140: fatal error, exit 134 (core dump)
pile : mul_mat_vec_q_switch_type -> ggml_cuda_mul_mat_vec_q -> ggml_backend_cuda_graph_compute
```

Cause, lue dans le code : `ggml_cuda_should_use_mmvq` se termine par
`default: return ne11 <= MMVQ_MAX_BATCH_SIZE;` — **permissif pour tout type quantifié** — alors que
`mul_mat_vec_q_switch_type` n'a pas de cas pour le type et **abandonne**. Le type était donc revendiqué
par un chemin incapable de le servir. Sans cette garde, le premier `MUL_MAT` à `n` petit **fait tomber le
binaire**.

---

## 4 · Ce qui est MESURÉ sur le modèle (le DGX répond enfin à la question du §5.2)

Modèle `Qwen3.8-27B-OBRAIN-v3.gguf` — **sha256 `1d7c9409…` vérifié identique sur n3 et sur le DGX**
après copie —, corpus `ppl.txt` **sha256 `9d16815e…` identique des deux côtés**.

```bash
GGML_SCHED_DEBUG=2 build-cuda/bin/llama-perplexity -v -m MODEL -f CORPUS -c 512 --chunks 1 -t 8 -ngl 99
build-cuda/bin/llama-perplexity -m MODEL -f CORPUS -c 512 --chunks 4 -t 8 -ngl 99   # 3 fois
build-cuda/bin/llama-perplexity -m MODEL -f CORPUS -c 512 --chunks 4 -t 8 -ngl 0    # témoin CPU
```

| question du document | attendu si le défaut se reproduit | **mesuré** |
|---|---|---|
| segments du graphe | ≈ 338 | **2** |
| nœuds WHT sur l'appareil | 0 (repli CPU) | **2 688 sur `CUDA`, 0 sur `CPU`** |
| déterminisme (3 essais) | trois listes `[1]` différentes | **`[1]3,8032` et `3,6183` ×3 — identiques** |
| écart GPU–CPU | ≈ +10,5 % | **3,6183 (GPU) contre 3,6361 (CPU) → 0,49 %** |

**Le défaut documenté ne se reproduit donc pas** : le graphe n'est pas fracturé, le résultat est
déterministe, et l'écart GPU–CPU retombe sous le demi-pour-cent. Le « +10,5 % » publié appartenait à un
état où la rotation **retombait sur le CPU** — ce que ces deux noyaux suppriment.

---

## 5 · Les débits, MESURÉS — et ce que le banc a trouvé

### 5.1 Le contexte, et il compte

Le DGX **n'est pas une machine libre** : quatre `llama-server` y tournent en permanence, dont **deux sur
le GPU** — le 27B `Q4_K_M` de la prod (port **9910**, 262 144 de contexte, `-ngl 99`) et un VL-8B
(port 9907) —, plus les embeddings en 9906 et un 1,7B sur CPU en 9908. **Tout chiffre GPU ci-dessous est
donc un plancher**, et la charge au moment de la mesure est donnée à chaque fois.

### 5.2 Les débits

Deux campagnes, **séparées parce qu'elles ne sont pas comparables** (c'est la règle du document, et elle
s'est vérifiée) :

| mesure | campagne A (12:37, `load` 0,07, **0** connexion prod) | campagne B (12:42-12:52, `load` 4,9 → 10,5, **1** connexion prod) |
|---|---|---|
| **GPU `pp512`** (`-ngl 99`) | **329,17 ± 0,00 t/s** | **311,66 ± 30,86 t/s** |
| **GPU `tg128`** (`-ngl 99`) | *arrêt du binaire* (défaut §5.3) | **1,86 ± 0,05 t/s** |
| **CPU `pp512`** (`-ngl 0`) | **372,89 ± 14,16 t/s** | **135,60 ± 3,27 t/s** |
| **CPU `tg128`** (`-ngl 0`) | 1,79 ± 0,68 t/s | 1,58 ± 0,50 t/s |

**Le préfill CPU varie de 2,75× entre les deux fenêtres** : sur cette machine, seuls les chiffres pris
dans la même fenêtre se comparent. C'est exactement ce que le document de test annonçait.

**Perplexité à 8 chunks** (protocole aligné sur n3), `-c 512 --chunks 8 -t 8` :

| | PPL | s/passe |
|---|---|---|
| **GPU CUDA** (`-ngl 99`) | **3,2012 ± 0,14520** | **3,51 s** (fenêtre calme) |
| **CPU** (`-ngl 0`) | **3,2079 ± 0,14591** | 6,86 s |
| *n3, Vulkan natif, même protocole* | *3,2088* | *23,95 s* |
| *n3, CPU pur, même protocole* | *3,2086* | *388,16 s* |

**Écart GPU–CPU sur le DGX : 0,0067, soit 0,21 %.** Et les deux moteurs **concordent** : 3,2012 (CUDA
DGX) contre 3,2088 (Vulkan n3) sur le même modèle et le même corpus — 0,008 d'écart, avec des chemins
de calcul différents.

### 5.3 Ce que le banc a trouvé : un SECOND chemin revendiquait le type

Le premier `llama-bench -ngl 99` a **fait le préfill puis est tombé** :

```
pp512 = 329,17 t/s          ← le préfill passe
mmvq.cu:1150: fatal error, exit 134
pile : ggml_cuda_try_fuse -> ggml_cuda_mul_mat_vec_q -> mul_mat_vec_q_switch_type
```

La garde précédente couvrait `ggml_cuda_mul_mat`, **pas la fusion** :
`ggml_cuda_should_fuse_mul_mat_vec_q` juge sur `ggml_is_quantized(src0->type)` — **même critère trop
large** — puis appelle `mul_mat_vec_q`, qui n'a pas de cas pour le type et abandonne. Le type était
revendiqué par un **second** chemin incapable de le servir. Corrigé (`66fa326`), et la perplexité est
**identique au chiffre près** après correction (3,2012), donc aucun effet sur la justesse.

**La leçon, pour la revue : « est quantifié » n'est PAS « a un noyau ».** Les deux endroits qui
décidaient ainsi sont maintenant explicites, et **tous les deux ont été constatés au banc avant d'être
corrigés**.

### 5.4 Le décodage est le point faible, et il est chiffré

| | préfill `pp512` | décodage `tg128` |
|---|---|---|
| **DGX, GPU CUDA** | **311–329 t/s** | **1,86 t/s** |
| DGX, CPU | 136–373 t/s | 1,58–1,79 t/s |
| **n3, 780M Vulkan natif** | 89,36 t/s | **4,62 t/s** |

**Le préfill GPU est excellent — ×3,5 à ×3,7 le 780M de n3.** Mais **le décodage est 2,5× PLUS LENT que
l'iGPU de n3**, et à peine meilleur que son propre CPU.

**La cause est dans le code, pas dans le silicium** : le type n'a **aucun noyau fusionné** (les deux
gardes du §5.3 le prouvent). Le chemin emprunté **déquantifie donc la matrice entière à chaque appel** :

- 11,95 Gio lus **par jeton**, plus ~24 Gio écrits en F16 pour cuBLAS ;
- à 1,86 t/s, cela représente **≈ 67 Go/s de trafic mémoire soutenu** — le GB10 fait payer ce qu'il
  pourrait éviter : le chemin fusionné ne lirait que les 11,95 Gio du type, **trois fois moins**.

C'est la prochaine pièce, et elle est maintenant **chiffrée** : un noyau **MMVQ** (ou MMQ) pour
`TQ2_G128`, qui déquantifie dans le produit au lieu de le faire à côté. La référence existe côté CPU
(`ggml_vec_dot_tq2_g128_q8_K`), et le banc la validera : `test-backend-ops -p tq2_g128` couvre déjà
`n = 1 … 9`, soit précisément le décodage.

---

## 6 · Ce qui reste

1. **Noyau MMVQ / MMQ pour `TQ2_G128`** — le décodage. **C'est la priorité, et elle est chiffrée au §5.4.**
2. **Débits en fenêtre calme** sur le DGX, protocole du document (les chiffres du §5.2 sont des planchers,
   la machine servant deux modèles sur le GPU).
3. **Fusion** : cette branche dérive de `feat/vulkan-tq2g128` ; elle ne touche **que** `ggml-cuda/`, le
   test de backend et son document — fusion sans conflit attendu avec le travail Vulkan.

---

## 7 · Le décodage : de 1,86 à 10,92 t/s, et où sont les 60-80

### 7.1 La progression, chaque marche mesurée

| étape | commit | décodage `tg128` | preuve de non-régression |
|---|---|---|---|
| départ (déquantifier → F16 → cuBLAS) | `6012430` | **1,86 ± 0,05 t/s** | — |
| **noyau MMVQ** (`vec_dot_tq2_g128_q8_1`) | `73cc439` | **6,04 ± 0,43 t/s** | 15/15 au banc, perplexité 3,2012 |
| **fusion ouverte** (FFN denté en un noyau) | `b8ca15f` | **6,92 ± 0,13 t/s** | perplexité 3,2012 (identique) |
| **spéculation MTP** (head `mtp-Qwen3.8-27B-Q8_0`) | — (runtime) | **10,92 t/s** | 86 jetons acceptés / 122 proposés = **70,5 %**, longueur moyenne 3,10 |

**Total : ×5,9 sur le décodage**, sans toucher à un poids ni à la qualité (perplexité constante au chiffre près, 3,2012).

### 7.2 Le fait qui commande la suite : tout tourne à ~85 Go/s, la machine en fait 200

| | trafic par jeton | débit mesuré | trafic effectif |
|---|---|---|---|
| base MMVQ + fusion | 12,83 Go | 6,92 t/s | **≈ 89 Go/s** |
| base + MTP | ≈ 7,4 Go | 10,92 t/s | **≈ 81 Go/s** |
| *témoin machine* (Qwen3-1.7B Q8_0, `-ngl 99`) | 1,83 Go | 110,85 t/s | **≈ 203 Go/s** |

**Le noyau tourne donc à ~42 % de ce que la machine sait faire**, et c'est **la** porte : elle multiplie les deux bras à la fois. Passer le noyau au régime mémoire (~190 Go/s) donnerait ≈ **15-16 t/s** en base et ≈ **24 t/s** avec MTP — sans autre changement.

### 7.3 Le coût du brouillon, et pourquoi il limite la spéculation

Le head MTP est en **Q8_0 : 3,16 Go lus par pas de brouillon**. Avec une longueur moyenne de 3,10 jetons acceptés, cela fait **~10 Go de brouillon par passe** contre 12,83 Go de modèle principal — soit **+78 % de trafic** pour ×3,10 de jetons. D'où le gain observé de ×1,7 et non ×3,1.

**Corollaire** : un brouillon **moins lourd** (head MTP en 2-3 bits, ou un brouillon plus petit) rapproche le gain de la longueur acceptée. C'est la deuxième porte, et elle est indépendante de la première.

### 7.4 La route vers 60-80 t/s, chiffrée

| levier | état | effet attendu |
|---|---|---|
| 1. noyau MMVQ au régime mémoire | à faire (porte principale) | base 6,92 → **15-16 t/s** |
| 2. brouillon plus léger | à faire | gain de spéculation 1,7 → **2,5-3** |
| 3. les deux ensemble | — | **≈ 40-50 t/s** |
| 4. + longueur acceptée plus haute (prompt/réglages DFlash) | — | **60-80 t/s** |

⚠️ **Réserves, et elles comptent** : mesures prises sur une machine **partagée** (quatre `llama-server`, dont deux sur le GPU) — les valeurs absolues sont des **planchers**. En revanche, chaque comparaison ci-dessus oppose deux bras **de la même fenêtre de charge**, et la justesse (perplexité 3,2012, 15/15 au banc) est insensible à la charge.

---

## 8 · CORRECTION — le coût de la rotation n'était PAS 44 %, et la leçon de méthode

**Ce que j'avais annoncé** : « sans la rotation (`OBRAIN_NO_ROTATION=1`), le décodage passe de 6,37 à
11,40 t/s → le WHT coûte 44 % du décodage ».

**C'est faux**, et l'erreur est de méthode : les deux bras avaient été mesurés **à des heures
différentes**. Relancé en **A/B entrelacé dans la même fenêtre** (même binaire `f66c3c6`, mêmes
drapeaux, bras alternés, `-r 3` chacun, `load` 2,05 → 2,06) :

| bras | décodage `tg128` | préfill `pp512` |
|---|---|---|
| **rotation active** (avec le mémo) | **8,14 ± 2,97** t/s | 293,16 ± 19,25 t/s |
| **rotation désarmée** | **6,51 ± 0,20** t/s | 316,61 ± 24,97 t/s |
| **rotation active** (confirmation) | **6,89 ± 0,28** t/s | 299,94 ± 11,39 t/s |

**La rotation ne coûte rien de mesurable** : le bras désarmé est **plus lent ou égal**. Les 44 %
étaient la dérive de la charge entre deux fenêtres, pas la transformée.

**La leçon vaut pour tout ce document** : sur cette machine **partagée** (quatre `llama-server`, dont
deux sur le GPU), une mesure isolée ne vaut rien — l'écart entre fenêtres (±30 à 45 %) dépasse tous
les effets que je cherche. **Deux bras ne se comparent QUE s'ils sont entrelacés dans la même
fenêtre** (ce que le §7.1 avait fait pour le noyau MMVQ, et pas pour la fusion ni pour la rotation).

**Ce qui survit à la correction, et ce qui tombe :**

| résultat | statut |
|---|---|
| noyau MMVQ : 1,86 → 6,04 t/s | **tient** — ×3,2, très au-delà de la dérive |
| MTP : 6,48 → 10,92 t/s | **tient** — les deux bras mesurés **dans la même session serveur** |
| fusion : 6,04 → 6,92 t/s (+14 %) | **à revérifier** — dans le bruit de la machine |
| rotation : 44 % du décodage | **RETIRÉ** — artefact de charge |

**Le mémo de rotation (`f66c3c6`) est conservé** : il est juste (résultat identique, perplexité 3,2012)
et il supprime 1 080 nœuds redondants par passe (3 024 → 1 944). Mais **son effet sur le débit n'est pas
démontrable ici** : dans le bruit de la machine. Il reste parce qu'il enlève du travail inutile, pas
parce qu'il rapporte.

**Et le noyau MMVQ, lui, est à 6 % d'un noyau mature** (mesuré à l'op : 149 Go/s contre 159 Go/s pour
`q4_0` aux mêmes formes, §5.4). **La porte n'est donc pas là** : ce qui reste des 145 ms par jeton est
ailleurs (attention, KV, quantifications d'activations, 1 944 nœuds WHT, lancements), et **c'est cette
répartition qu'il faut mesurer avant de toucher à autre chose** — avec des bras entrelacés.

---

## 9 · LE BUDGET D'UN JETON, mesuré — et la porte est enfin nommée

`nsys` ne rapporte **aucun noyau** sur cette plateforme (CUPTI muet, même avec
`--cuda-graph-trace=node`), et `perf` ne couvre qu'une opération à la fois. J'ai donc ajouté
l'instrument qui manquait : **`GGML_CUDA_OP_TIMING=1`** (deux événements CUDA par opération, **aucune
synchronisation par nœud**, une seule en fin de graphe). Il imprime le temps GPU cumulé par type
d'opération. À lire comme un **classement** : il exige `GGML_CUDA_DISABLE_GRAPHS=1`, donc chaque
opération paie son lancement.

```
GGML_CUDA_OP_TIMING=1 GGML_CUDA_DISABLE_GRAPHS=1 llama-bench -m v3 -ngl 99 -p 0 -n 32 -r 1
tg32 = 6,67 t/s
[obrain] BUDGET GPU PAR OPERATION — 2028,6 ms cumulées, 30 750 opérations
  MUL_MAT           1704,7 ms   84,0 %   9537 appels   178,7 us
  GET_ROWS            93,2 ms    4,6 %   3201          29,1 us
  ADD                 34,8 ms    1,7 %   3168          11,0 us
  FLASH_ATTN_EXT      34,4 ms    1,7 %    528          65,2 us
  WHT                 33,3 ms    1,6 %   3564           9,3 us
  SET_ROWS            28,0 ms    1,4 %   1056          26,5 us
  ROPE                18,7 ms    0,9 %   1056          17,7 us
  L2_NORM             17,9 ms    0,9 %   3168           5,6 us
  CPY / UNARY / CONCAT / SCALE / CONT : le reste, sous 1 % chacun
```

**Trois conclusions, et la première est une réhabilitation :**

1. **La rotation fait 1,6 %** du temps GPU. Le §8 avait raison de retirer les « 44 % » : la
   transformée n'est pas le sujet, et le mémo du §8 n'était donc pas la porte non plus.
2. **Les matmuls font 84 %.** C'est là, et nulle part ailleurs.
3. Le reste (attention, KV, normalisations, rope) **tient sous 5 % à lui tous seul** : la piste
   « c'est l'attention ou le SSM qui coûte » est morte.

**Les graphes CUDA valent ~4 %** (différences appariées, A/B entrelacé : +0,38 et +0,19 t/s) : le
décodage **n'est pas** dominé par les lancements.

### 9.1 Où est la marge, alors que le noyau est à 94 % d'un noyau mature ?

Parce que ces 94 % sont mesurés **aux formes du banc** (`m=4096, k=14336, n=8`), pas aux formes du
modèle en décodage. Dans le modèle :

| | trafic par jeton | temps de matmul | **débit effectif** |
|---|---|---|---|
| dans le modèle (n=1) | 12,83 Go | ≈ 126 ms (84 % de 150 ms) | **≈ 102 Go/s** |
| au banc (`perf`, n=8) | — | — | **149 Go/s** |
| `q4_0`, même banc | — | — | 159 Go/s |

**Le noyau perd ~30 % de son efficacité sur les formes réelles du décodage** (n=1, k=5120/6144,
m=5120/6144/17408), là où le banc mesure n=4..8. **C'est la porte, et elle est mesurable** : il faut
mesurer l'opération aux formes exactes du décodage, identifier celles qui décrochent, et les régler
(`nwarps`, `rows_per_cuda_block`, paramètres par type).

### 9.2 La leçon de méthode, complétée

Le §8 disait : n'interpréter que des bras de la **même fenêtre**. La mesure des graphes ajoute une
seconde règle : **la machine dérive de 11 % en six minutes** (7,18 → 6,80 → 6,63 → 6,44 t/s dans une
seule séquence entrelacée). Donc : **différences APPARIÉES** entre bras alternés, jamais une moyenne
de mesures dispersées.

---

## 10 · LA CONTENTION : toutes les mesures de debit de la session etaient faussees (facteur 2,2)

**Le fait, et il renverse la lecture de tout ce document.** Une mesure faite pendant que la prod du DGX
etait **au repos** (load 0,58, **0 connexion** sur les quatre `llama-server`) :

| mesure | sous contention (toute la session) | **fenetre propre** |
|---|---|---|
| decode `tg32` / `tg128` | 6,4 - 8,1 t/s | **15,64 / 15,76 t/s** |
| prefill `pp512` | 293 - 337 t/s | **664,94 +/- 10,18 t/s** |
| MTP, bras de base | 6,48 t/s | **15,73 t/s** |
| **MTP, avec brouillon** | 10,92 t/s | **30,23 t/s** (86/122 acceptes, longueur 3,10) |
| perplexite 8 chunks (qualite) | 3,2012 | **3,2012** (inchangee) |

**Facteur 2,2 sur le decodage, 2 sur le prefill.** Le DGX heberge la prod sur LE MEME GPU et la MEME
LPDDR5X (le 27B `Q4_K_M` en 256k de contexte sur 9910, plus un VL-8B sur 9907) : quand elle streame ses
poids, elle prend la bande passante, et mes mesures prenaient le reste.

**Consequences, et la premiere est severe :**

1. **Les debits publies dans ce document avant ce paragraphe ne decrivent pas le moteur** — ils
   decrivent le moteur SOUS CONTENTION. Les *comparaisons* appariees restent valables (meme fenetre),
   les *valeurs absolues* non.
2. **Le « trou de 80 ms par jeton » n'a jamais existe.** Il venait de la contention. Il n'y a donc
   **rien a fusionner** : le projet de regie (fusion des ~385 petites operations, paquet QKV/gate-up)
   n'a plus d'objet — je le retire de la feuille de route.
3. **Le decode de base est AU PLAFOND de la machine** : 12,83 Go par jeton a 15,7 t/s, soit **202 Go/s**,
   exactement la bande passante pratique mesuree (203 Go/s). Deux chemins de calcul independants
   (61 ms de noyaux ; 203/12,83) donnaient 15,8-16,4 t/s : c'est ce qui se mesure.

### 10.1 Le modele est devenu PREDICTIF

Avec les octets par jeton accepte, le debit se calcule — et il colle a la mesure :

| chemin | octets par jeton emis | debit prevu | **mesure** |
|---|---|---|---|
| base | 12,83 Go | 203/12,83 = **15,8 t/s** | **15,64 t/s** |
| MTP (brouillon Q8_0, longueur 3,10) | (12,83 + ~3x3,16)/3,10 = **7,2 Go** | 203/7,2 = **28 t/s** | **30,23 t/s** |

### 10.2 La specification qui en decoule, et elle est chiffree

Pour atteindre 60-80 t/s : **203/60 = 3,38 Go** et **203/80 = 2,54 Go par jeton accepte**. Or le
brouillon actuel est le poste gaspille : **Q8_0, 3,16 Go lus par pas**, soit ~9,5 Go par passe de
speculation pour 3,10 jetons acceptes.

| levier, sans toucher aux poids du modele principal | octets/jeton accepte | debit attendu |
|---|---|---|
| **brouillon ternaire ou 2-4 bits** (3,16 -> ~0,8 Go par pas) | ~4,9 Go | **~41 t/s** |
| **+ acceptation plus longue** (3,10 -> 5, DFlash / n-gram / prefixe) | ~3,2 Go | **~63 t/s** |
| artefact structure <= 3 Go actifs/jeton (MoE, butterfly+low-rank) | **~3,0 Go** | **60-80 t/s** |

**Les deux premiers leviers ne changent AUCUN poids du modele principal** : le brouillon ne fait que
proposer, et le modele principal verifie chaque jeton accepte. La qualite est donc inchangee par
construction — et l'invariant de perplexite 3,2012 le confirme a chaque etape.

### 10.3 Lecon de methode, definitive

Trois regles, payees chacune par une mesure fausse :

1. **Toute mesure de debit doit enregistrer l'etat de la prod** (load ET connexions). Sans cela, un
   facteur 2,2 se glisse dans les chiffres — et il ne se voit pas.
2. **Seules les comparaisons APPARIEES dans la meme fenetre veulent dire quelque chose** (la machine
   derive de 11 % en six minutes, et de 2,2x selon que la prod streame ou non).
3. **Un instrument qui rend zero ou qui plante n'est pas une mesure** : les deux instruments essayes
   dans cette session ont ete retires, avec leur histoire ecrite ici.

---

## 11 · LE BROUILLON MTP : trois bras mesures, et le ternaire perd

Objectif, celui du §10 : minimiser `B_eff = (B_verify + k*B_draft)/E[jetons acceptes]`. Quatre heads
fabriques, quatre bras mesures **dans la meme fenetre**, etat de la machine enregistre a chaque bras.

### 11.1 Les artefacts

| fichier | type | taille |
|---|---|---|
| `mtp-Qwen3.8-27B-Q8_0.gguf` | Q8_0 (reference d'origine) | **2,95 Go** |
| `mtp-Qwen3.8-27B-Q4_K_M.gguf` | Q4_K_M (requantifie) | **1,56 Go** |
| `mtp-Qwen3.8-27B-TQ2G128.gguf` | TQ2_G128 ternaire replie | **0,74 Go** |
| `mtp-TQ2-SORTIE-Q4.gguf` | corps ternaire + sortie Q4_K | 1,24 Go |

Le head est ternarisable : ses largeurs `k` sont 5120, 6144, 10240, 17408 -- **toutes multiples de
1024**, donc le contrat de repli (`GGML_ASSERT(cur->ne[0] % OBRAIN_WHT_BLOCK == 0)` dans
`build_lora_mm`) est respecte. Les deux tenseurs a `k = 256` sont des vecteurs de normalisation 1-D,
qui ne passent jamais par une projection lineaire.

**Aucune source bf16 du head n'existe** (le cache HF du depot de base ne contient que 12 Ko de
metadonnees) : Q4 et TQ2 sont donc **doublement quantifies** (Q8_0 -> cible). Leurs acceptations
mesurees sont des **bornes basses**.

### 11.2 Le tableau definitif (fenetre propre : load 0,14-1,24, prod = 0)

| bras | octets | t/s | proposes / acceptes | acceptation | **x base** |
|---|---|---|---|---|---|
| base (sans brouillon) | -- | **15,90** | -- | -- | 1,000 |
| Q8 | 2,95 Go | 29,82 | 122 / 86 | 70,5 % | x1,876 |
| **Q4_K_M** | **1,56 Go** | **33,20** | 125 / 85 | **68,0 %** | **x2,089** |
| Q4 (second passage) | 1,56 Go | **33,20** | 125 / 85 | 68,0 % | x2,089 |
| TQ2_G128 ternaire | 0,74 Go | 5,36 | 345 / **10** | **2,9 %** | x0,79 |
| TQ2 corps + sortie Q4_K | 1,24 Go | 24,37 | 167 / 71 | 42,5 % | x1,53 |

(Les deux derniers bras sont mesures dans une autre fenetre -- le base y etait anormalement bas a
7,79 t/s -- donc seules leurs **acceptations** se lisent, pas leurs debits.)

**Reproductibilite : les deux passages du bras Q4 donnent 33,20 t/s et 125/85 au chiffre pres.**

### 11.3 Ce que la mesure dit

1. **La quantifier, oui ; la ternariser, non.** Le Q4 garde 68,0 % d'acceptation pour 1,56 Go et
   donne **+31 % sur le Q8** (29,82 -> 33,20). Le ternaire, a 0,74 Go, **effondre l'acceptation** :
   **2,9 %** (345 proposes, 10 acceptes). Meme en gardant la projection de sortie a Q4_K -- la sortie
   pesant 1,27 Go des 2,95 Go du head, c'est le premier suspect -- l'acceptation ne remonte qu'a
   **42,5 %**, et le debit retombe a 24,37 t/s.
2. **Le mecanisme reste CORRECT** -- la parite le prouve : les quatre bras produisent **le meme texte
   sur 536 caracteres** (sur 553 a 564). La divergence n'arrive qu'a la fin de la generation, et pour
   tous les bras de brouillon au meme point, donc elle ne vient pas du ternaire mais des **numeriques
   de la verification par lot** (le verify passe a n > 1, alors que le decode sequentiel est a n = 1 :
   les logits different au dernier bit, et un argmax glouton peut bifurquer). Le temoin de qualite
   n'est donc pas « identique au bit », c'est « identique jusqu'au point ou les numeriques de lot
   divergent » -- et ce point doit etre surveille, pas suppose.
3. **Les octets du brouillon ne sont PAS le terme dominant.** Diviser par deux le brouillon
   (2,95 -> 1,56 Go) ne rapporte que +11 % (29,82 -> 33,20). Le terme dominant reste
   `B_verify / E[acceptes]` = 12,83 Go par jeton accepte, et c'est **l'acceptation qui commande**.
4. Le debit de base confirme le plafond : 12,83 x 15,90 = **204 Go/s**, exactement le §10.

### 11.4 La suite, dans l'ordre que la mesure impose

1. **Allonger l'acceptation** (`--draft-max`, n-gram / prompt-lookup, un second prompt type) : c'est
   le seul levier dont le rendement est demontre. A brouillon Q4 constant, passer d'environ 2,4 a 5
   jetons acceptes par passe ferait tomber `B_eff` de 6,14 a ~4,2 Go, soit **~48 t/s**.
2. **Un brouillon plus leger pourra revenir par la suite** -- mais a 2-4 bits *gardant l'acceptation*,
   ce que la double quantification Q8 -> TQ2 ne permet pas de conclure. Il faudrait la source bf16 du
   head.
3. **Ne pas toucher aux poids du modele principal** : les trois premiers leviers n'en changent aucun,
   et la qualite y est inchangee par construction (le principal verifie chaque jeton accepte).

### 11.5 Deux regles de methode de plus

- **Verifier qu'un port est libre avant d'y lancer un serveur de banc.** Le port 9933 etait tenu par un
  service `python` etranger (pid 2405) : les requetes du banc ont recu `{"detail":"Not Found"}` -- le
  tableau est sorti a 0,00 t/s partout, ce qui *ressemble* a une mesure et n'en est pas une.
- **Enregistrer l'etat de la machine a chaque bras** (load + connexions prod) : un bras de base a
  7,79 t/s au lieu de 15,90 rend la fenetre entiere suspecte, et il faut alors ne lire que les
  *ratios* intra-fenetre.

---

## 12 · LA COURBE draft-max : ou est l'optimum, et le goulot qui la borne

Instrumentation d'abord (§12.1), puis 28 bras mesures : 4 familles de prompts x (base + draft-max 2, 3, 4,
6, 8, 12), head brouillon **Q4_K_M (1,56 Go)**, etat de la machine releve a chaque bras. Le drapeau du
fork est `--spec-draft-n-max` (`--draft-max` a ete **retire**).

### 12.1 L'instrumentation, et une erreur qu'elle a attrapee toute seule

`SPEC-METRICS` decompose chaque passe : jetons proposes / acceptes, temps du **brouillon**, temps de la
**verification**, temps total, et imprime en fin de run la moyenne, p50/p90/p95/max de la longueur
acceptee, l'histogramme des passes (0/1/2/3/4+ acceptes) et les temps **par jeton accepte**.

Premiere validation, et elle a mis le doigt sur une faute de mesure : la verification sortait a
**1,73 ms** par passe alors que le decode d'un seul jeton coute ~63 ms. La cause : `llama_decode` rend
la main apres l'**enfilement**, pas apres l'execution — le GPU continue en asynchrone et l'attente se
paie au premier echantillonnage des logits. Correctif : la verification s'ouvre avant le decode et se
**clot apres `common_sampler_sample_and_accept_n`**. Sans cela, 75 ms par passe restaient dans un
« reste » inexplicable.

**A retenir : sur ce moteur, un `llama_decode` chronometre autour de son appel ne mesure rien du GPU.**

### 12.2 La courbe (fenetres propres : prod = 0 sauf mention)

| famille | base | **dm optimal** | t/s | x base | longueur | acceptation |
|---|---|---|---|---|---|---|
| naturel | 15,90 | **4** | **35,70** | **x2,245** | 2,50 | 63,4 % |
| code | 15,83 | **4** | **35,21** | **x2,225** | 2,43 | 62,1 % |
| conversation | 15,78 | **4** | **34,46** | **x2,184** | 2,41 | 60,5 % |
| repetition | 15,79 | **2** | **37,17** | **x2,354** | 3,10 | 88,9 % |

Detail du naturel (brouillon et verification, par jeton accepte) :

| dm | t/s | longueur | brouillon | **verification** | passes a 0/1/2/3/4+ |
|---|---|---|---|---|---|
| 2 | 29,95 | 1,52 | 6,15 ms | 49,09 ms | 8 / 8 / 34 / 0 / 0 |
| 3 | 33,12 | 2,02 | 6,81 ms | 38,65 ms | 7 / 7 / 6 / 22 / 0 |
| **4** | **35,70** | 2,50 | 7,36 ms | 31,65 ms | 5 / 7 / 3 / 7 / 14 |
| 6 | 33,48 | 2,82 | 9,90 ms | 30,39 ms | 5 / 7 / 4 / 6 / 11 |
| 8 | 15,37 | 3,88 | 11,03 ms | **71,43 ms** | 1 / 7 / 2 / 5 / 11 |
| 12 | 14,32 | 3,88 | 14,36 ms | **74,10 ms** | 1 / 6 / 2 / 7 / 10 |

**Trois enseignements :**

1. **L'optimum est structurel, pas propre au prompt** : dm = 4 pour trois familles, dm = 2 pour la
   repetition (qui accepte deja 3,1 jetons avec 2 propositions, 88,9 %). Le plateau 3-6 est large.
2. **La moyenne cache tout.** A dm=2, 16 passes sur 50 acceptent 0 ou 1 jeton ; a dm=4, 14 passes
   acceptent 4+. C'est pour cela que la distribution est instrumentee et pas seulement la moyenne.
3. **Le goulot a un nom : `MMVQ_MAX_BATCH_SIZE = 8`.** Le lot de verification compte `propose + 1`
   jetons : 7 a dm=6 (verify 30,39 ms) et **9 a dm=8** (verify 71,43 ms). `ggml-cuda.cu:1795` bascule
   du chemin vectoriel MMVQ au chemin tensor-core MMQ des que le lot depasse 8 — et a ces formes
   (n = 9-13 pour m jusqu'a 17408), le chemin MMQ est **2,3 a 3 fois plus lent**. Le saut de 30 a 100 ms
   par jeton accepte n'est pas une histoire de bande passante : c'est un changement de noyau.

### 12.3 Le modele de cout NE se valide PAS tel quel, et c'est le prochain poste a identifier

Avec les octets naifs (verification 12,83 Go, brouillon 1,56 Go par pas), la bande passante impliquee
par les bons bras monte a **271-315 Go/s** — au-dessus du plafond mesure (204 Go/s). Le modele
surestime donc les octets, ou sous-estime le plafond. En faisant l'hypothese que **le pas de brouillon
coute ~0,29 Go au lieu de 1,56** (projection de sortie partagee avec le modele principal, ou pas lue a
chaque pas), les trois familles retombent a **200-202 Go/s**, c'est-a-dire exactement le plafond.

**Regle du mandat appliquee : quand la prediction diverge fortement, on ARRETE l'optimisation et on
identifie le poste de cout.** Le poste a identifier est donc le cout REEL d'un pas de brouillon (les
1,56 Go du fichier ne sont manifestement pas tous lus par pas) — a mesurer avant toute autre decision.

### 12.4 Reponse factuelle a la question posee

**Peut-on depasser durablement ~40 t/s en augmentant seulement le nombre de jetons acceptes par
verification, sans toucher aux poids du modele principal ?**

**Non, pas dans l'etat actuel du moteur** : la courbe culmine a **35,7 t/s (x2,25)** puis **recule**.
Deux raisons, toutes deux mesurees : (a) au-dela de 6 propositions, l'acceptation marginale chute
(63 % -> 48 % -> 34 %) alors que le brouillon continue de couter ; (b) surtout, des que le lot de
verification depasse 8 jetons, le noyau change et la verification triple.

**En revanche, la meme mesure montre ou est la marge** : si le lot de verification restait sur le
chemin vectoriel au-dela de 8 jetons (instancier MMVQ pour 9-16), la courbe continuerait de monter
au-dela de dm=8, puisque le brouillon y coute deja 11-14 ms par jeton accepte et que la longueur
acceptee, elle, monte jusqu'a 3,9-7,5. C'est un changement d'**instantiation de noyau**, pas de
poids : la prochaine experience propre.

---

## 13 · CORRECTION DU §12.3 : le denominateur etait le jeton COMMIS, pas le jeton accepte

Le §12.3 concluait que le modele de cout surestimait les octets et soupconnait que le pas de brouillon ne
lise que ~0,29 Go au lieu de 1,56. **C'est faux, et l'erreur etait dans mon denominateur.**

J'avais divise par les jetons **acceptes du brouillon** (`accepted_draft_tokens`), alors que le debit
compte les jetons **commis** : chaque passe de verification commet aussi **un** jeton produit par le
modele principal lui-meme (la position 0 est toujours commise, quelle que soit l'issue du brouillon).

```
committed_output_tokens = accepted_draft_tokens + verify_rounds

B_eff = (verify_rounds * B_main + proposed_draft_tokens * B_draft) / committed_output_tokens
```

Controle par les temps sur le bras naturel dm=4 : `90 x (7,36 + 31,65) ms = 3,511 s` contre
`126 / 35,70 = 3,529 s` de debit mesure — les deux ferment.

**Recalcul des douze bras en dm <= 6 :**

| famille | dm=2 | dm=3 | dm=4 | dm=6 |
|---|---|---|---|---|
| naturel | 189,6 | 191,4 | **193,6** | 192,9 |
| code | 188,6 | 189,4 | **194,3** | 194,9 |
| conversation | 190,1 | 189,1 | **192,6** | 193,8 |

(Go/s impliques ; plafond mesure ~203-204)

**Le modele de cout est VALIDE** — il n'y a aucune projection partagee a chercher. Les bras dm >= 8
restent a 39-97 Go/s : ce n'est pas une question d'octets, c'est le changement de noyau MMVQ -> MMQ,
deja nomme, a caracteriser dans un micro-banc aux formes reelles.

**Les compteurs sont desormais nommes et imprimes separement** (`SPEC-COST`) :
`proposed_draft_tokens`, `accepted_draft_tokens`, `verify_rounds`, `verifier_committed_tokens`,
`committed_output_tokens`, plus la taille du lot de verification (`proposed + 1`) avec son maximum —
c'est elle qui decide de la passerelle a 8. Confondre « jeton accepte » et « jeton produit »
sous-estimait le denominateur d'environ 40 % : assez pour inventer une bande passante au-dessus du
plafond et partir chercher un faux mystere.

**Regle ajoutee au banc** : un bras de base sous 12 t/s signifie fenetre chargee -> le script de mesure
**s'arrete** au lieu de produire des chiffres. Il l'a fait de lui-meme sur le premier essai du bord
(base 5,98 t/s, prod=2).

---

## 14 · PROTOCOLE DU MICRO-BANC MMVQ / MMQ (plan de reference, non encore execute)

But : repondre a `T_MMVQ(n)` contre `T_MMQ(n)` pour `n = 7, 8, 9, 10, 12, 16` aux **formes reelles de
verification**, et savoir si la limite `MMVQ_MAX_BATCH_SIZE = 8` est une limite de PERFORMANCE ou
aussi une hypothese de VALIDITE.

### 14.1 Trois executables immuables, trois repertoires independants

`GGML_CUDA_FORCE_MMQ` est une **macro de compilation** (`#ifdef`, ggml-cuda.cu:5213), pas une variable
d'environnement : on obtient donc trois binaires figes, et **aucun comportement ne change au milieu des
mesures**. Trois repertoires de build **separes**, jamais recycles avec des drapeaux differents (une
recompilation partielle ou un objet en cache serait une contamination silencieuse) ; le hash de chaque
binaire est note dans le rapport.

| build | drapeau | ce qu'il donne |
|---|---|---|
| **A** | aucun | MMVQ pour n <= 8, MMQ pour n >= 9 (le comportement nominal) |
| **B** | `-DGGML_CUDA_FORCE_MMQ` | `T_MMQ(n)` pour **tous** les n |
| **C** | `MMVQ_MAX_BATCH_SIZE = 16` **en local, documente, reverte apres** | `T_MMVQ(n)` pour n = 9..16, la ou le chemin n'existe pas encore |

Le build C ne doit pas etre livre : c'est une experience, et elle se termine par un revert explicite.

### 14.2 Justesse : comparaison TENSORIELLE, pas seulement la perplexite

La perplexite 3,2012 et la suite TQ2 (15/15) sont necessaires mais **insuffisantes** : elles peuvent
masquer une erreur locale. Pour chaque vraie forme et chaque `n` en 9, 10, 12, 16, on compare
directement les sorties sur les memes entrees :

```
max | Y_MMVQ - Y_MMQ |        et        || Y_MMVQ - Y_MMQ ||_2 / || Y_MMQ ||_2
```

C'est ce qui dira si la constante 8 etait une frontiere de vitesse ou une frontiere de validite.

### 14.3 Ressources : le seuil brutal peut venir d'un registre de trop

`ptxas -v` est indispensable mais le nombre de registres seul ne suffit pas. Par instanciation
(`ncols_dst`), on releve :

* registres par fil ;
* shared memory **statique** et **dynamique** ;
* spills / local memory (`ptxas` le dit : « spill stores/loads ») ;
* fils par bloc ;
* occupancy **theorique** et, si accessible, **obtenue**.

Le noyau stocke `float tmp[ncols_dst][rows_per_cuda_block]` : la pression registre croit
**lineairement** avec n. Mais la rupture reelle peut etre brutale — passer de 96 a 112 registres peut
faire tomber de deux blocs residents a un seul. C'est exactement le genre de seuil qui expliquerait
pourquoi 8 a ete choisi.

### 14.4 La table visee, et les trois scenarios

| n | MMVQ | MMQ | gain | regs | occupancy | erreur relative |
|---|---|---|---|---|---|---|
| 7 | | | | | | |
| 8 | | | | | | |
| 9 | | | | | | |
| 10 | | | | | | |
| 12 | | | | | | |
| 16 | | | | | | |

* **MMVQ rapide et stable jusqu'a 16** -> la limite 8 est vraisemblablement conservatrice, l'extension
  merite un vrai patch.
* **MMVQ gagne a 9-10 puis s'effondre a 12-16** -> domaine intermediaire specialise a exploiter : le
  scenario le plus interessant, parce qu'il **suffit** au speculative decoding. On n'a pas besoin que
  MMVQ tienne jusqu'a 16 ; s'il tient jusqu'a 10 ou 12, des brouillons plus longs deviennent
  possibles sans tomber dans le precipice x2,3-3 du MMQ.
* **MMVQ perd immediatement apres 8, spill massif, occupancy cassee** -> la constante 8 est fondee et
  il faut un **troisieme kernel**, pas une constante plus grande.

### 14.5 Etat de l'attente (au moment ou ce plan est ecrit)

Fenetre chargee en continu (`prod = 2`, load ~1,1) : **aucune mesure du bord n'a ete publiee**. Le
guetteur sonde toutes les 2 minutes et ne lance la mesure que si `prod = 0` et `load < 2` ; le script
de mesure, lui, **s'arrete** si le bras de base descend sous 12 t/s. Il l'a deja fait une fois
(base 5,98 t/s) au lieu de produire des chiffres.

Verifie au passage : les quatre serveurs actifs sont la **production legitime** (bge-m3, Qwen3-1.7B sur
CPU, 27B `Q4_K_M`, VL-8B) — **aucun serveur de banc orphelin**, la fenetre depend donc de la charge
reelle et non de mes restes.

---

## 15 · LE BORD 8/9 EST ETABLI : banc apparie bidirectionnel, 24 bras en fenetre propre

Banc relance apres le redemarrage de la machine, **dans une fenetre enfin propre** : `prod = 0` sur les
24 bras, load 0,2-0,6, base stable a **16,06-16,32 t/s** (dispersion < 2 %). 3 blocs, ordres **A, B, A**
(l'ordre B remonte dm10 -> dm5), 4 bras `dm` + 12 bases encadrantes.

### 15.1 Les bras, tels que mesures

| bloc | ordre | bras | t/s | prod | base encadrante | **R** | ecart des bases |
|---|---|---|---|---|---|---|---|
| 1 | A | dm=5 | 34,00 | 0->0 | 16,32 / 16,27 | **2,086** | 0,3 % |
| 1 | A | dm=7 | 31,72 | 0->0 | 16,27 / 16,12 | **1,958** | 0,9 % |
| 1 | A | dm=9 | 15,95 | 0->0 | 16,12 / 16,09 | **0,991** | 0,2 % |
| 1 | A | dm=10 | 15,63 | 0->0 | 16,09 / 16,13 | **0,970** | 0,2 % |
| 2 | B | dm=5 | 33,68 | 0->0 | 16,16 / 16,09 | **2,089** | 0,5 % |
| 2 | B | dm=7 | 32,65 | 0->0 | 16,11 / 16,16 | **2,024** | 0,3 % |
| 2 | B | dm=9 | 16,28 | 0->0 | 16,13 / 16,11 | **1,010** | 0,1 % |
| 2 | B | dm=10 | 15,64 | 0->0 | 16,09 / 16,13 | **0,971** | 0,2 % |
| 3 | A | dm=5 | 33,73 | 0->0 | 16,09 / 16,06 | **2,099** | 0,2 % |
| 3 | A | dm=7 | 32,53 | 0->0 | 16,06 / 16,06 | **2,026** | 0,0 % |
| 3 | A | dm=9 | 16,31 | 0->0 | 16,06 / 16,11 | **1,014** | 0,3 % |

**Les onze triplets sont VALIDES** (aucun rejet au seuil de 15 % ; le pire ecart de bases est 0,9 %).

### 15.2 Le temoin direct : la verification elle-meme

Metriques par bras, **identiques d'un bloc a l'autre** (reproductibilite au centieme) :

| dm | proposes/passe | **lot de verification** | chemin | longueur acceptee | brouillon | **verification** | R |
|---|---|---|---|---|---|---|---|
| 5 | 4,92 | **5,9** | **MMVQ** | 2,50 | 9,13 ms | **32,24 ms** | **2,09** |
| 7 | 6,84 | **7,8** | **MMVQ** | 2,94 | 10,91 ms | **30,15 ms** | **2,00** |
| 9 | 8,96 | **10,0** | **MMQ** | 4,08 | 9,91 ms | **67,04 ms** | **1,00** |
| 10 | 9,69 | **10,7** | **MMQ** | 3,88 | 11,61 ms | **68,67 ms** | **0,97** |

**La rupture est dans la verification, pas dans le brouillon** : le brouillon reste a 9-12 ms par jeton
commis d'un bout a l'autre, tandis que la verification **passe de 30,15 a 67,04 ms** exactement quand le
lot franchit 8 (7,8 -> 10,0). Facteur **2,2** sur le seul poste qui compte.

Et la consequence sur le debit est brutale : **au-dela du seuil, la speculation ne rapporte plus rien**
(R = 1,00) et devient meme legerement negative (R = 0,97, dm=10) — le gain de longueur acceptee (2,9 ->
4,1) ne compense pas le doublement du cout de verification.

### 15.3 Verdict factuel demande

**La rupture a 8/9 est REPRODUCTIBLE** :
* dans les **trois blocs** (R = 2,086 / 2,089 / 2,099 pour dm=5 ; 1,958 / 2,024 / 2,026 pour dm=7 ;
  0,991 / 1,010 / 1,014 pour dm=9) ;
* dans les **deux sens temporels** (blocs 1 et 3 en ordre A descendant, bloc 2 en ordre B remontant) ;
* avec les bases encadrantes a **moins de 1 %** l'une de l'autre (aucun triplet rejete) ;
* et avec un **mecanisme identifie** : `MMVQ_MAX_BATCH_SIZE = 8` (ggml-cuda.cu:1795), le lot de
  verification valant `proposes + 1` — 7,8 a dm=7, 10,0 a dm=9.

**Donc : le diagnostic MMVQ -> MMQ est etabli, et le micro-banc A/B/C (§14) est la suite justifiee.**

### 15.4 Lecons operationnelles de cette sequence

- **Un redemarrage nettoie `/tmp`** : les resultats partiels du premier banc ont ete perdus avec lui.
  Desormais les mesures qui comptent sont ecrites dans le depot (`docs/`), pas seulement dans `/tmp`.
- **La fenetre propre vaut un facteur 2,2** sur les debits et rend les bras trois fois plus rapides
  (16-22 s contre 60-75 s sous contention) : c'est la seule configuration ou un banc peut durer.
- **Le binaire du DGX etait un commit en retard** (`960f23f` au lieu de `6145f11`) : les journaux
  portaient l'ancienne etiquette (`JETON ACCEPTE`) sans les compteurs nommes. Les valeurs restent
  exploitables, mais la prochaine fois je **verifie le commit du binaire** avant de lancer un banc.

---

## 16 · REGLE GENERALE : une mesure CUDA est INVALIDE tant que le point de synchronisation n'est pas identifie

La meme classe de bug a coute **deux tables** dans cette campagne. Il faut donc l'inscrire comme regle, pas
comme anecdote.

| occurrence | ce qui a ete chronometre | ce qui se mesurait vraiment |
|---|---|---|
| `llama_decode` (serveur, §12.1) | la verification d'une passe de speculation | l'**enfilement** : 1,73 ms annonces pour un decode qui en coute ~63 |
| `ggml_backend_graph_compute` (micro-banc, §15.5) | une operation MUL_MAT | l'**enfilement** : 0,04 us pour lire 8 Mo, soit des centaines de To/s |

**Le chronometrage correct, partout (A, B, C, chaud comme streaming) :**

```
synchronize -> t0 -> graph_compute -> synchronize -> t1
```

Les deux erreurs ont la meme signature : un chiffre **physiquement impossible** (au-dessus du pic de la
machine, 273 Go/s) ou **incoherent avec une mesure independante** (67 ms end-to-end contre 1,73 ms au
micro-banc). C'est cette signature — et non l'intuition — qui les a fait sortir.

**Regles qui en decoulent :**

1. **Toute mesure CUDA est invalide tant que le point de synchronisation n'est pas explicitement
   identifie.** Une API asynchrone chronometree comme synchrone ne mesure pas un noyau : elle mesure une
   file d'attente.
2. **Un chiffre physiquement impossible doit arreter l'interpretation**, pas etre explique. On recalcule
   le debit implique et on le compare au pic de la machine : s'il le depasse, la mesure est fausse.
3. **Un instrument qui n'est juste que parfois ne vaut rien.** Les nombres du regime chaud etaient
   plausibles « par accident » (des synchronisations fortuites) : c'est plus dangereux qu'un chiffre
   visiblement faux, parce que rien ne donne l'alerte. La table a donc ete jetee **en entier**.
4. **Verifier une mesure contre une autre mesure**, jamais contre une attente : le croisement
   end-to-end / micro-banc, et le fait que A et C coincident aux lots <= 8 (les deux prennent MMVQ),
   sont les seuls temoins qui vaillent.

Corollaire operationnel deja applique : les dumps de justesse portent **forme + n + lot + etiquette +
occurrence**, et les entrees calculees cote CPU de deux builds sont comparees **bit a bit** avant toute
comparaison de sorties — un ecrasement silencieux ou une difference d'entree produirait sinon une
conclusion sur des donnees qui n'ont jamais ete les memes.

---

## 17 · MICRO-BANC, REGIME STREAMING CERTIFIE : le gain n'est PAS un artefact de cache

Le regime streaming mesurait des microsecondes impossibles parce que le cas ne respectait pas la
semantique de `eval_perf` : celui-ci duplique le **noeud terminal** `n_runs` fois et rapporte
`total/total_runs`. Un graphe a N matmuls **chaines par des additions** n'a pas de noeud terminal
duplicable : le « us/run » n'etait pas le temps d'un graphe complet, et le diviser par N fabriquait
des chiffres a des dizaines de To/s.

**Correctif** : un cas peut prendre la main via `perf_independent()`. Le cas streaming construit,
alloue, echauffe, **synchronise**, execute le graphe COMPLET R fois, **synchronise**, et publie ses
chiffres. `eval_perf` reste inchange pour les autres cas, dont le regime chaud.

### 17.1 Le controle qui certifie le compteur

| R | q_proj n=10 (us par graphe) | gate_up n=16 | k_v n=16 |
|---|---|---|---|
| 1 | 19 623 | 18 923 | 5 549 |
| 2 | 18 778 | 19 467 | 3 844 |
| 4 | 18 674 | 19 120 | 4 074 |

**Stable a ~5 %** — donc le compteur de repetitions est juste (si R=2 avait donne la moitie de R=1, il
aurait encore ete faux). Et le controle physique tient : 255 Mo / 18,7 ms = **13,6 Go/s**, tres en
dessous du pic de 273 Go/s.

### 17.2 Les trois builds, en regime streaming (Go/s effectifs)

| forme | n | A (nominal) | **B (MMQ partout)** | **C (MMVQ jusqu'a 16)** |
|---|---|---|---|---|
| q_proj | 8 | 121,9 | **13,3** | 120,3 |
| q_proj | 10 | 13,6 (MMQ) | 13,2 | **84,0** |
| q_proj | 16 | 13,2 (MMQ) | 13,2 | **49,9** |
| gate/up | 8 | 145,9 | **13,8** | 147,2 |
| gate/up | 10 | 13,8 (MMQ) | 13,0 | **95,4** |
| gate/up | 16 | 13,8 (MMQ) | 13,7 | **54,1** |
| k/v | 8 | 51,8 | **16,9** | 43,2 |
| k/v | 10 | 17,3 (MMQ) | 16,7 | **38,8** |
| k/v | 16 | 16,1 (MMQ) | 16,1 | **29,8** |

*(working set : 255 Mo pour q_proj/o_proj, 248 Mo pour gate/up et down, 85 Mo pour k/v ; N = 32, 11, 64)*

### 17.3 Ce que ca dit, en ratios

**S_stream = T_MMQ / T_MMVQ**, a graphe strictement identique :

| forme | n=8 | n=10 | n=16 |
|---|---|---|---|
| q_proj | **x9,1** | x6,2 | x3,8 |
| o_proj | **x9,8** | x6,1 | x3,8 |
| gate/up | **x10,5** | x7,1 | x3,9 |
| down | **x10,4** | x6,4 | x4,0 |
| k/v | x1,2 | x2,3 | x1,9 |

**Le gain se reproduit dans le regime streaming** : x3,8 a x10,5 sur les quatre grandes formes, avec
k/v a nouveau la forme la plus faible (x1,2 a x2,3). Consequence double :

1. **Ce n'est PAS un artefact de cache chaud.** Le regime qui compte pour le decodage speculatif —
   des tampons distincts, un volume qui depasse les caches — donne les memes conclusions que le
   regime chaud, du meme ordre de grandeur.
2. **MMQ ne delivre que 13-17 Go/s** sur ces formes aux petits lots, contre 40-147 Go/s pour MMVQ.
   C'est ce qui explique mecaniquement l'effondrement end-to-end du §15 (67 ms de verification par
   jeton commis a dm=9/10 contre 30 ms a dm=7) : le moteur choisit le mauvais noyau exactement pour les
   lots que la speculation fabrique.

### 17.4 Deux points encore ouverts

- **Justesse tensorielle** : les vidages ont produit **0 fichier** sur cette passe (filtre `-p MUL_MAT`
  sans effet observable) : a diagnostiquer avant toute conclusion sur la correction de MMVQ a 9-16.
- **Ressources par `ncols_dst`** : le journal `ptxas -v` de la premiere passe a ete perdu ; il faut le
  refaire avec `-Xptxas -v` et demangler les noms pour attribuer registres/shared/occupancy a chaque
  instantiation 9-16.

---

## 18 · LES DEUX GATES SONT FERMES : justesse et ressources

### 18.1 Gate 1 — justesse MMVQ au-dela de 8 : CERTIFIEE

Vingt cas (5 formes x n = 9, 10, 12, 16), un appel precis par cas, graine fixe, fichiers de vidage
etiquetes par **nom de backend** (la premiere passe comparait deux references CPU entre elles a cause
d'une etiquette supposee — d'ou un `max_abs` de zero parfaitement trompeur).

* **20/20 PASSENT sur les deux builds** contre la **reference CPU** (`1/1 tests passed`) : MMVQ a 9-16
  calcule la bonne chose ;
* **entrees CPU IDENTIQUES** dans les 20 cas (la graine rend la comparaison possible) ;
* ecart MMVQ contre MMQ, sur entrees identiques :

| forme | rel_l2 (n=9 / 10 / 12 / 16) | max_abs (n=16) |
|---|---|---|
| q_proj | 4,58e-3 / 4,58e-3 / 4,55e-3 / 4,53e-3 | 1,59 |
| o_proj | 4,75e-3 / 4,65e-3 / 4,66e-3 / 4,62e-3 | 0,72 |
| gate/up | 4,60e-3 / 4,59e-3 / 4,57e-3 / 4,56e-3 | 1,59 |
| down | 6,03e-3 / 5,95e-3 / 5,90e-3 / 5,99e-3 | 18,49 |
| k/v | 4,54e-3 / 4,49e-3 / 4,31e-3 / 4,27e-3 | 1,59 |

**Ecart relatif ~0,5 %, stable avec n, et AUCUNE anomalie apres n = 8** : la valeur ne saute pas au
passage de la frontiere. C'etait exactement la question posee — et la reponse est que MMVQ au-dela de
8 est numeriquement sain.

### 18.2 Gate 2 — ressources par instantiation : l'hypothese est INVALIDEE

Lues dans le **binaire** (`cuobjdump --dump-resource-usage`), la ou le journal de compilation etait
incomplet (308 compilations mais 253 lignes de ressources). Le nom mangle encode type et lot
(`IL9ggml_type43ELi9ELb0ELb0EE`), donc l'attribution est certaine.

| n | 1 | 2 | 4 | 6 | **8** | **9** | **10** | **12** | 14 | **16** |
|---|---|---|---|---|---|---|---|---|---|---|
| registres | 93 | 54 | 98 | 122 | **146** | **137** | **149** | **172** | 193 | **213** |
| shared | 4096 | 0 | 4096 | 2560 | 3072 | 0 | 0 | 0 | 0 | 0 |
| pile (spills) | **0** | 0 | 0 | 0 | **0** | 0 | 0 | 0 | 0 | 0 |
| blocs/SM (128 f) | 5 | 9 | 5 | 4 | 3 | 3 | 3 | 2 | 2 | 2 |

**Aucun spill a aucune taille** (STACK = 0 partout), croissance des registres **progressive**, et
occupancy qui decroit doucement (12 -> 5 -> 3 -> 2 blocs/SM). Surtout : **n = 9 consomme MOINS de
registres que n = 8 (137 contre 146)** — il n'y a **aucune cassure** a la frontiere.

**Conclusion : la constante 8 n'est justifiee ni par les ressources ni par la justesse.** Elle ne
correspond qu'a un choix historique, et le chemin vectoriel reste sain (registres moderes, zero spill)
jusqu'a 16 sur ce type.

### 18.3 Courbe de crossover streaming, complete (n = 8, 9, 10, 12, 16)

Temps par graphe (R = 4, apres echauffement complet) et rapport A/C (= MMQ / MMVQ) :

| forme | n=8 | n=9 | n=10 | n=12 | n=16 |
|---|---|---|---|---|---|
| q_proj | 1,0 | **7,6** | 6,6 | 5,4 | 3,8 |
| o_proj | 1,0 | **7,2** | 6,9 | 5,1 | 3,9 |
| gate/up | 1,1 | **8,1** | 7,5 | 5,8 | 4,1 |
| down | 1,0 | **7,0** | 6,7 | 5,4 | 4,0 |
| k/v | 1,0 | 3,3 | 3,0 | 2,4 | 2,0 |

Le rapport vaut **~1,0 a n = 8** (les deux builds prennent MMVQ : controle fonctionnel du dispatch),
puis **7 a 8 a n = 9**, et decroit lentement. **k/v reste la forme faible** (2,0 a 3,3) mais ne perd
jamais : conformement a la consigne, elle reste **"a determiner"** — la marge y est trop faible pour
trancher sans repetitions supplementaires, alors que les quatre grandes formes sont franches.

### 18.4 Ce que la mesure autorise a proposer

Un **domaine MMVQ derive des mesures**, non une constante globale : les quatre projections
(`q_proj`, `o_proj`, `gate/up`, `down`) supportent MMVQ jusqu'a **n = 16** sans perte de justesse ni
de ressources ; `k/v` demande des repetitions avant d'etre classee. La prediction a falsifier reste
celle du §15 : faire tomber la verification de dm=9/10 de **~67-69 ms a ~30 ms par jeton commis**, et
ramener R au-dessus de 1.

---

## 19 · VALIDATION END-TO-END A vs C : la discontinuite disparait

Fenetre propre (`prod = 0` sur les douze bras, load 0,6-1,4), bras **apparies** (A et C adjacents pour
chaque valeur de dm), meme prompt, meme graine (`temperature 0`), meme brouillon Q4_K_M. Deux binaires
immuables : **A** nominal (`sha 5e21a653`) et **C** MMVQ etendu a 16 (`sha 1cc69c0f`).

| bras | A t/s | C t/s | **A/base** | **C/base** | C verify ms/jeton commis | C lot moy/max | longueur acceptee (A / C) |
|---|---|---|---|---|---|---|---|
| base | 16,08 | 16,11 | 1,00 | 1,00 | -- | -- | -- |
| **dm=7** | 32,35 | 32,62 | **2,01** | **2,03** | 22,34 | 7,8 / 8 | 2,94 / 2,94 |
| **dm=9** | **16,04** | **32,18** | **1,00** | **2,00** | **22,36** | 9,6 / 10 | 3,85 / 3,85 |
| **dm=10** | 15,45 | 27,81 | 0,96 | **1,73** | 25,61 | 10,5 / 11 | 3,50 / 3,50 |
| **dm=12** | 15,11 | 24,02 | 0,94 | **1,49** | 29,28 | 12,1 / 13 | 3,50 / 3,50 |
| dm=16 | 17,40 | 17,09 | 1,08 | 1,06 | 46,68 | 16,4 / **17** | 5,05 / 5,05 |

### 19.1 Les trois conditions de la prediction sont remplies

1. **Chute massive du verify** : a dm=9, C mesure **22,36 ms par jeton commis**, contre **67,04 ms**
   mesures sur A dans la campagne du §15 — facteur **3**. La prediction « 67-69 ms -> ~30 ms » est
   satisfaite, et depassee.
2. **Retour du gain speculatif** : A a dm=9 vaut **x1,00** (16,04 t/s, exactement le debit de base,
   c'est-a-dire aucun gain) ; C a dm=9 vaut **x2,00** (32,18 t/s, le meme gain qu'a dm=7).
3. **Aucune regression d'acceptation** : a chaque dm, la longueur acceptee est **identique** entre A et
   C (2,94 / 3,85 / 3,50 / 3,50 / 5,05). Seuls les noyaux de verification changent : ce qui est accepte
   ne change pas.

**Le resultat central : la discontinuite dm7 -> dm9 a disparu.** Dans A, passer de 7 a 9 propositions
faisait tomber le debit de x2,01 a x1,00 ; dans C, dm=9 conserve x2,00 et dm=10 x1,73. Le precipice
n'existe plus.

### 19.2 Deux details qui comptent pour le patch

* **dm=16 retombe** (x1,06) : le lot vaut `proposes + 1`, donc 17 avec une seule proposition de plus
  que 16 — et 17 depasse `MMVQ_MAX_BATCH_SIZE = 16` du build C. Le seuil du patch doit donc se lire en
  **taille de lot**, pas en nombre de propositions.
* **Le binaire A n'avait pas l'instrumentation `SPEC-COST`** (je n'avais reconstruit que
  `test-backend-ops` dans l'arbre nominal) : ses colonnes `verify`/`lot` sont donc vides, et la
  comparaison sur ce point s'appuie sur la campagne du §15 (67,04 ms). Une re-mesure appairee avec
  l'instrumentation a jour est a faire pour figer le tableau definitif.
