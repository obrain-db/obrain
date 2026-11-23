# `GGML_OP_WHT` natif sur Vulkan — spécification d'implémentation

**Date** : 2026-09-21. **But** : que la transformée tourne **sur l'appareil** au lieu de retomber sur le
CPU. C'est la pièce qui répare la **cause** — le fractionnement du graphe — au lieu du symptôme.

**Branche** : `feat/wht-dense-matmul` (arbre `/mnt/obrain/wt-wht-dense`). **Fichiers** : `ggml-vulkan.cpp`
et `src/llama-graph.cpp`. **Aucune ligne dans `ggml-cuda/`** (voir §6).

---

## 1 · L'idée, en une phrase

`ggml_vk_fwht` (`ggml-vulkan.cpp:9434`) normalise par `1/sqrt(src->ne[0])` et transforme **chaque ligne
de largeur `ne[0]`**. Or notre rotation est **par blocs de 1024** sur des tenseurs de largeur 5120/6144.

**Si l'on reshape en `[1024, N]` avant l'op, alors `ne[0] = 1024 = bloc`** — et le pipeline FWHT
**existant** convient tel quel. C'est exactement ce que fait déjà le crochet dense, sauf qu'il appelle
un `ggml_mul_mat` là où il faut appeler l'op.

**Conséquence** : pas de nouveau shader, pas de nouvelle op, pas de matmul — donc **pas de pénalité de
décodage**.

---

## 2 · Les six points d'édition — tous vérifiés par lecture

### `ggml/src/ggml-vulkan/ggml-vulkan.cpp`

| # | où | quoi |
|---|---|---|
| **1** | ligne **918** : `vk_pipeline pipeline_fwht_f32[4];` | → **`[5]`** |
| **2** | lignes **~5364** et **~5372** : les deux boucles `for (uint32_t n : {64, 128, 256, 512})` | → **`{64, 128, 256, 512, 1024}`** |
| **3** | `ggml_vk_fwht_pipeline_idx` (ligne **9398**) : le `switch (n)` | ajouter **`case 1024: return 4;`** |
| **4** | `supports_op` : le `switch (op->op)` | ajouter un `case GGML_OP_WHT:` → **`true`** si `src[0]->type == F32`, `ggml_is_contiguous(src[0])`, et `ggml_vk_fwht_pipeline_idx(op->src[0]->ne[0]) >= 0` |
| **5** | `build_graph` : le `switch` des ops (modèle : `case GGML_OP_CUMSUM: ggml_vk_cumsum(ctx, compute_ctx, src0, node); break;`) | ajouter **`case GGML_OP_WHT: ggml_vk_fwht(ctx, compute_ctx, src0, node); break;`** |

**Le point 2 est sans risque**, et c'était le seul doute : `n` est une **constante de spécialisation**,
pas une taille de groupe de travail — la signature (ligne 2557) est
`(…, uint32_t parameter_count, std::array<uint32_t,3> wg_denoms, std::vector<uint32_t> specialization_constants, …)`
et l'appel passe `{1,1,1}` comme `wg_denoms` et `{device->subgroup_size, n}` comme
**constantes de spécialisation**. Ajouter 1024 crée donc simplement une variante du shader.

### `src/llama-graph.cpp` — le crochet

| # | quoi |
|---|---|
| **6** | dans `build_lora_mm`, **avant l'op**, reshapes en `[1024, ggml_nelements(cur)/1024]` ; après, `ggml_reshape_4d` vers la forme d'origine |

Concrètement, remplacer la ligne actuelle :

```c
cur = ggml_wht(ctx0, ggml_is_contiguous(cur) ? cur : ggml_cont(ctx0, cur), OBRAIN_WHT_BLOCK);
```

par :

```c
if (!ggml_is_contiguous(cur)) { cur = ggml_cont(ctx0, cur); }
ggml_tensor * x2 = ggml_reshape_2d(ctx0, cur, OBRAIN_WHT_BLOCK,
                                   ggml_nelements(cur)/OBRAIN_WHT_BLOCK);
ggml_tensor * y2 = ggml_wht(ctx0, x2, OBRAIN_WHT_BLOCK);   // ne[0] == bloc -> une transformée par ligne
cur = ggml_reshape_4d(ctx0, y2, cur->ne[0], cur->ne[1], cur->ne[2], cur->ne[3]);
```

**Pourquoi c'est équivalent** : pour tout tenseur dont `ne[0]` est multiple de 1024, l'aplatissement en
`[1024, N]` découpe **exactement** les mêmes blocs que l'op par blocs — chaque ligne de `x2` **est** un
bloc. La sortie est identique au bit (mesuré : `op ≡ produit dense` à **1,2e-06** à l'échelle réelle).

**Et cela reste correct sur CPU** : `ggml_compute_forward_wht_f32` (`ops.cpp:1468`) traite `ne00 == ne0`,
donc avec `ne[0] = 1024` il fait une transformée pleine largeur par ligne — soit, par ligne de `x2`, un
bloc. Identique.

---

## 3 · Le remède dense devient inutile — et il est RETIRÉ (2026-09-22)

Le patch `OBRAIN_WHT_DENSE` (produit dense) **doit être retiré** une fois celui-ci validé : il n'était
que le **révélateur** de la cause, et il coûte 45 % du décodage.

**C'est fait**, dans la branche de validation **puis** dans l'arbre principal :

| arbre | commit | ce qui est retiré |
|---|---|---|
| `feat/wht-dense-matmul` | `9b9505a` | `g_obrain_h_rot`, `obrain_graph_set_h_rot`, `obrain_ctx_h_rot`, la branche dense du crochet, `OBRAIN_WHT_DENSE` / `OBRAIN_H_CPU` |
| **arbre principal** (`feat/vulkan-tq2g128`, rapatriement `7b47999`) | « Suppression du code mort : le patch dense » | les mêmes, **plus** le commentaire orphelin qui les annonçait |

**Vérification avant/après, arbre principal** — `build/` CPU pur (`GGML_VULKAN=OFF`), modèle `v3`
(sha256 `1d7c9409…`), corpus `/mnt/obrain/corpus/ppl.txt` (sha256 `9d16815e…`),
`-c 512 --chunks 1 -t 8 -ngl 0` :

| état du binaire | témoin dans `libllama.so` | PPL |
|---|---|---|
| **avant** | 1 chaîne `OBRAIN_WHT_DENSE`, symbole exporté | **3,8161** |
| **après** | **0** chaîne, **0** symbole | **3,8161** |

Trois exécutions identiques à quatre décimales : **l'instrument CPU est déterministe**, donc l'égalité
avant/après est concluante. **Le choix du CPU n'est pas un détail** : dans l'arbre principal, le chemin
Vulkan reste celui de `ggml_wht` (338 segments), dont la perplexité **n'est pas reproductible**
(3,2762 à 4,0310 selon l'exécution) — c'est le défaut que la transformée native supprime, et elle vit
sur `feat/wht-dense-matmul`, pas encore ici.

**Ce qui reste, et ce n'est que cela** : `OBRAIN_NO_ROTATION` (la contre-épreuve du §4). ⚠️ La trace
`OBRAIN_WHT_TRACE`, annoncée « à conserver » par la révision précédente de ce document, **n'existe plus
nulle part** — mais elle a bel et bien existé : introduite par `b5b45c7` (`feat/wht-dense-matmul`, avec
le balayage `OBRAIN_WHT_DENSE=<pourcentage>`), emportée avec le patch dense par `9b9505a`. L'arbre
principal n'a jamais reçu cette variante : son crochet (rapatrié par `7b47999`) n'avait que
l'interrupteur binaire, sans trace ni balayage.

---

## 4 · Critères de validation — écrits d'avance

Le binaire `build-vk`, `-ngl 99`, sur `v3` (sha256 **`1d7c9409ae7f2f2c7f6071c8ad…`**, 12 844 231 424 o) :

| mesure | attendu | avant |
|---|---|---|
| **segments du graphe** (`GGML_SCHED_DEBUG=2` **+ `-v`**) | **2** | 338 |
| **nœuds WHT sur Vulkan** | **168 / 168** | 0 / 168 |
| **préfill `pp512`** | **≈ 82,7 t/s** | 43,02 |
| **décodage `tg128`** | **≈ 4,3 t/s** *(inchangé)* | 4,30 |
| **déterminisme** (3 essais identiques) | **identique** | 6,10 / 6,38 / 12,57 |

**Le chiffre qui décide** : **décodage ≈ 4,3 t/s SANS perdre le ×1,92 du préfill.** Si on l'obtient,
c'est gagné sur les deux tableaux et le patch peut être activé par défaut.

**Et le contrôle négatif** : `OBRAIN_NO_ROTATION=1` doit donner ≈ 183 000 de perplexité (le modèle
détruit) — sinon l'op ne fait rien.

---

## 4bis · La vérification faite sur l'ARBRE PRINCIPAL (2026-09-22)

La transformée native a été transférée dans l'arbre principal par cherry-pick de `3142d88`
(**`b6767df`**, auteur conservé). Après le transfert, `src/llama-graph.cpp` et
`ggml/src/ggml-vulkan/ggml-vulkan.cpp` sont **identiques au bit** à ceux de `feat/wht-dense-matmul`
(`git diff feat/wht-dense-matmul -- <ces deux fichiers>` → **vide**).

**Commande** : `build-vk/bin/llama-perplexity` (reconstruit, `-ngl 99`), modèle `v3`
(sha256 `1d7c9409…`), corpus `/mnt/obrain/corpus/ppl.txt` (sha256 `9d16815e…`), `-t 8`.

| critère (§4) | attendu | avant | **mesuré, `b6767df`** |
|---|---|---|---|
| **segments du graphe** (`GGML_SCHED_DEBUG=2 -v`) | **2** | 338 | **2** ✅ |
| **nœuds WHT sur Vulkan** | tous sur l'appareil | **0 / 168** | **2 688 nœuds, 2 688 sur `[Vulkan]`, 0 sur `[CPU]`** ✅ (5 376 mentions, le compte de la branche) |
| **préfill `pp512`** | ≈ 82,7 t/s | 43,02 ± 0,05 | **89,36 ± 0,31 t/s** (×2,08) ✅ |
| **décodage `tg128`** | ≈ 4,3 t/s *(inchangé)* | 4,30 ± 0,78 | **4,62 ± 1,57 t/s** ✅ |
| **déterminisme** (3 essais) | identique | 6,10 / 6,38 / 12,57 | **`[1]3,8519` · `3,6372` ×3** ✅ |
| **contrôle négatif** `OBRAIN_NO_ROTATION=1` | ≈ 183 000 | — | **168 976,72** ✅ (même ordre : l'op agit) |

**Et le chiffre qui referme la question ouverte** : à `-c 512 --chunks 1 -ngl 99`, la perplexité vaut
**3,8519** — exactement la valeur consignée lors de la vérification du retrait du patch dense sur la
branche. Le chiffre de la conversation perdue est donc **reproduit ici**, sur l'arbre principal, et il
se lit désormais sur un graphe **non fracturé**. (À ne pas confondre avec le **3,8161** de la
vérification avant/après du retrait : celle-là tournait sur `build/`, **CPU pur**, seul instrument
comparable tant que le chemin Vulkan était le `ggml_wht` non déterministe.)

⚠️ **Réserve, et elle est réelle** : les **débits** ont été mesurés **sous charge** (la prod tourne sur
la même machine, `load average` 1,2–1,6). Ils sont donc **conservateurs** et **non comparables** à une
fenêtre calme — mais ils suffisent à trancher le troc : **le préfill double sans que le décodage
recule**. Les critères qui, eux, ne dépendent pas de la charge (segments, placements, déterminisme)
sont mesurés.

**Et sur CPU, le transfert ne change rien** — le point qu'il fallait vérifier, puisque le §2 l'annonçait
par le raisonnement seulement : `build/` (CPU pur, `-ngl 0`, mêmes modèle et corpus,
`-c 512 --chunks 1 -t 8`) donne **3,8161**, la valeur exacte mesurée **avant** le transfert. Le RESHAPE
`[1024, N]` découpe les mêmes blocs : le chemin CPU reste identique.

**Et la justesse est désormais testée, elle ne l'était pas.** L'op `GGML_OP_WHT` n'avait **aucun** cas
dans `test-backend-ops` : la suite ne couvrait FWHT que par le chemin **fusionné** (`MUL_MAT` + hint
HADAMARD), et seulement jusqu'à 512. Le test ajouté (`test_wht`, 13 cas : les cinq largeurs servies ×
1 et 4 lignes, les formes du crochet, et un bloc **plus petit** que la ligne) donne :

- **12 cas sur 12 justes** sur Vulkan aux largeurs servies — `1024` comprise, celle de la rotation ;
- **1 échec avant correction** : `WHT(ne=[1024,1,1,1], block=128)`, **ERR = 1 876** ;

**la faute trouvée** : le shader transforme **une ligne de largeur `ne[0]`** — pipeline *et*
normalisation `1/sqrt(ne[0])` déduits de `ne[0]` **seul** — et ne lit jamais le paramètre de bloc. Le
CPU, lui, découpe la ligne en `ne[0]/block` blocs (normalisation `1/sqrt(block)`). `supports_op` se
déclarait donc capable d'un cas qu'il **sert faux, en silence**. Le graphe du modèle ne tombe pas
dedans (le crochet reshape en `[1024, N]`, donc `block == ne[0]`), mais l'op est publique et son
contrat autorise `block < ne[0]`.

**Correction** : `supports_op` exige `ggml_get_wht_block(op) == op->src[0]->ne[0]`. Le cas fautif
devient un `not supported` — repli CPU, ce qui est honnête. **Après** : `12/12` comparés sur Vulkan
plus un refus explicite, suite **verte** ; et le remède est **intact** (segments 2, 2 688 nœuds WHT
tous sur Vulkan, 0 sur CPU, PPL `3,8519` inchangée).

---

## 5 · Pièges — ceux qui ont déjà coûté

| piège | correctif |
|---|---|
| `GGML_SCHED_DEBUG=2` sans `-v` | **9 lignes, aucun tracé** → ajouter `-v` |
| `void ggml_vk_create_pipeline(...)` introuvable | c'est **`ggml_vk_create_pipeline_func`** (ligne 2557) |
| `env -fa on …` | `env` intercepte `-fa` → passer les drapeaux **après le binaire** |
| conclure sur **une** exécution | **trois essais** — un « trouvé » 2687/2688 a été démenti par le contrôle |
| mesure sans sa ligne de commande | **commande + sha256 en tête de journal** |
| `2>&1 > fichier` | `> fichier 2>&1` |
| `supports_op` qui juge sur `ne[0]` alors que l'op porte un **bloc** | exiger `ggml_get_wht_block(op) == ne[0]` : sinon le backend accepte l'op et rend un résultat **faux en silence** (ERR 1,876 mesuré) |
| croire une op « testée » parce que son chemin fusionné l'est | `test-backend-ops -o WHT` : `GGML_OP_WHT` n'avait **aucun** cas avant le §4bis |
| un test qui n'éprouve que la forme du **repli hors ligne** (bloc < ligne) et en conclut « pas de noyau » | éprouver la forme que le **modèle** emploie — reshape en `[1024, N]`, donc `ne[0] == bloc` — c'est la seule que le shader sert (`test-obrain-wht-device`, §4bis) |

---

## 6 · Et le DGX ?

**`GGML_OP_WHT` n'existe pas dans `ggml-cuda/`** (0 occurrence) — mais **ce travail-là ne s'y transpose
pas directement** : le shader FWHT n'a pas d'équivalent CUDA. Il faudrait écrire un noyau de papillon.
**Et il y a pire** : `TQ2_G128` non plus (**0 fichier**) → le modèle ternaire tournerait sur le CPU du
DGX. **Voir `docs/TEST-DGX-WHT-DENSE.md` §0bis.**

**Le présent travail sert d'abord n3** — et il est le préalable à toute mesure, DGX compris : sans lui,
l'instrument n'est pas fiable.

---

## 7 · État de cette spécification

**CONSTATÉ** : les six points d'édition, leurs numéros de ligne, la signature de création des pipelines,
la sémantique de `ggml_vk_fwht` et de `ggml_compute_forward_wht_f32`.

**MISE À JOUR DU 2026-09-22** : cette spécification **a été écrite et validée** sur la branche
`feat/wht-dense-matmul` (`3142d88` : les cinq critères du §4 tenus, `b189760` : validation en contexte
réel, `9b9505a` : retrait du produit dense). Elle a ensuite été **transférée dans l'arbre principal**
(`b6767df`) et **re-vérifiée là** : voir le **§4bis**. Le texte du §7 ci-dessous est laissé tel
qu'écrit.

**NON ÉCRIT, NON COMPILÉ, NON MESURÉ.** Je m'arrête à la spécification par contrainte de contexte, pas
par difficulté — et je l'écris plutôt que de livrer un demi-patch invérifiable. Les six éditions sont
bornées, leurs emplacements sont nommés, et les cinq chiffres qui les valideront sont fixés au §4.
