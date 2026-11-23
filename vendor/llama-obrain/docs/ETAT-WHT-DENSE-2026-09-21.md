# WHT dense — état, ancrage, et mode d'emploi

**Branche** : `feat/wht-dense-matmul`, base `d225d66` (= `feat/vulkan-tq2g128`).
**Arbre** : `/mnt/obrain/wt-wht-dense`. **Build** : `build-vk` (Ninja, Release, Vulkan).

Le correctif de l'autre session (+29 lignes dans `ggml-vulkan.cpp` et `mul_mm_funcs.glsl`) a été
**appliqué ici** pour que la comparaison « avant/après » ne porte que sur UN changement.

## Le but, chiffré

Remplacer, dans `src/llama-graph.cpp:1482-1486`, l'op `ggml_wht` autonome par une **multiplication
dense** contre une matrice de Hadamard `H` 1024×1024 **constante** (fp16, ±1/32 — exacte).

| mesure d'avant (arbre principal, même base + même correctif) | valeur |
|---|---|
| segments du graphe | **338** |
| nœuds WHT | **168, tous sur CPU, 0 sur Vulkan** |
| s/passe GPU | **48,79** |
| s/passe CPU | 82,32 |
| PPL GPU / CPU | **4,0310 / 3,4135** (+18,1 %) |
| coût par segment ajouté | **74,3 ms** |

**Cible** : −336 segments, +16 % de FLOPs → **≈ 27-28 s/passe** et **PPL GPU ≈ 3,4135**.

## Les trois prérequis sont VÉRIFIÉS

1. `H[i][j] = popcount(i&j) paire ? +1/32 : −1/32` (`tests/test-obrain-rotation-ggml.cpp:44-55`).
   Valeurs **±1/32** = puissances de deux → **stockage fp16 sans perte**. Et `H` est **symétrique**.
2. Le chemin dense existe et est **validé à 0,000e+00** : `llama_mul_mat_hadamard`
   (`src/llama-impl.h:61-78`) = reshape → `ggml_mul_mat` → reshape.
3. Vulkan sait faire le matmul : `test-backend-ops -o MUL_MAT -b Vulkan0` →
   `m=16 n=16 k=1024 OK`, `m=16 n=1 k=1024 OK`.

## LE POINT BLOQUANT — et il est réel

La ligne de graphe est triviale. Ce qui ne l'est pas : **faire entrer la constante `H` dans le
graphe**. Le contexte de graphe de llama.cpp est créé en `no_alloc = true`
(`llama-context.cpp`) : un tenseur créé avec `ggml_new_tensor` n'a **pas** de données à la
construction, et rien ne le remplit.

**Les trois ancrages possibles, avec leur emplacement exact :**

| # | ancrage | où | coût |
|---|---|---|---|
| **A** | **Entrée de graphe** — miroir de `llama_kv_cache::build_input_k_rot` (`src/llama-kv-cache.cpp:1475-1494`, remplissage `:1980-1986`) | `llama-graph.h` (champ), `llama-graph.cpp` (création + `ggml_set_input`), et **le ramassage des entrées** (`llm_graph_result::set_inputs`) | ~100 lignes, **le remplissage doit être accroché à une structure d'entrée existante** (aucune n'est garantie présente pour les couches DeltaNet) |
| **B** | **Tampon persistant de `llama_context`** — un petit contexte ggml + un tampon backend de 2 Mio, `H` téléversé une fois, pointeur passé à `llm_graph_context` | `llama-context.cpp` + `llama-graph.h` | ~60 lignes ; repose sur le fait que le planificateur reconnaît un tenseur déjà porteur d'un tampon backend (à **vérifier**, non vérifié ici) |
| **C** | **Tenseur du modèle** créé au chargement | `llama-model.cpp` (`load_tensors`) | ~40 lignes, mais `llama_model` n'a **aucun motif existant** de tenseur créé hors GGUF (vérifié : `llama-model.cpp:2052-2054` est une **sonde de type de tampon**, pas une création) |

**Aucun des trois n'est une ligne**, et **A et C touchent des fichiers que l'autre session occupe**.
Je me suis arrêté là délibérément : une implémentation à moitié laissée dans un arbre partagé est
exactement ce que cette campagne reproche au portage actuel.

## Ce qui reste à faire, dans l'ordre

1. Choisir l'ancrage (**B** est le moins intrusif et le plus contenu — à valider en 15 minutes : créer
   un tampon backend avec un tenseur F16 et voir si un graphe qui le référence reste sur Vulkan).
2. Mettre la ligne de graphe **derrière un interrupteur** (`OBRAIN_WHT_DENSE=1`) pour pouvoir mesurer
   les deux bras **dans le MÊME binaire** — c'est la seule façon de ne pas confondre l'effet du
   changement avec celui d'une reconstruction.
3. Reconstruire : `cmake --build build-vk -j16 --target llama-perplexity`.
4. Mesurer les trois chiffres, avec l'attribution complète (commande + sha256) comme dans
   `/mnt/obrain/mesures/` :
   - `GGML_SCHED_DEBUG=2 ... -v` → `sched_reserve: graph splits` : attendu **2**, contre 338 ;
   - temps par passe GPU : attendu **~27-28 s**, contre 48,79 ;
   - PPL GPU : attendu **≈ 3,4135**, contre 4,0310 — et le bras CPU doit rester à 3,4135.
5. Rapatrier : `git diff > /mnt/obrain/patches/wht-dense.patch`, puis appliquer dans l'arbre
   principal **sans toucher** aux +29 lignes de l'autre session (elles sont sauvegardées dans
   `/mnt/obrain/patches/`).

## Réserve

Le gain de **×1,8 sur le préfill** est **CALCULÉ**, pas mesuré : il suppose que le temps de la
rotation disparaît presque entièrement avec les segments. Si la mesure ne descend pas sous ~40 s, mon
modèle du coût par segment (74,3 ms) est faux et c'est **lui** qu'il faudra réviser — pas la cible.

---

# RÉSULTATS — 2026-09-21 (tour 8)

## Ce qui est acquis

| mesure | valeur |
|---|---|
| segments du graphe | **338 → 2** |
| s/passe GPU | **48,74 → 24,79 (fp16) / 25,22 (fp32)**, soit **×1,94 à ×1,97** |
| constante H | 1024×1024, allouée dans le tampon de la couche 0 (2 Mio fp16 / 4 Mio fp32) |
| transformée isolée | op ≡ produit dense **1,2e-06** à l'échelle réelle (5120 × 512) |
| op ≡ `obrain_rotate` | **0.000e+00** |
| arbre principal | **intact**, +29 lignes de l'autre session non touchées |

## LE FAIT MAJEUR — l'instrument de référence n'est pas reproductible

Trois exécutions aux arguments RIGOUREUSEMENT identiques :

| configuration | 3 essais | verdict |
|---|---|---|
| OP, `-ngl 99 -t 8` | 3,7805 / 3,8234 / 3,7632 | **varie** |
| OP, `-ngl 99 -t 1` | 3,3119 / 3,2762 / 3,2929 | **varie** |
| OP, `-ngl 0 -t 8` | 3,6726 / 3,6967 / 3,6782 | **varie** |
| **DENSE, `-ngl 99 -t 8`** | **3,2067 / 3,2067 / 3,2067** | **identique au bit** |

**Passer de 8 fils à 1 déplace la valeur OP de 3,80 à 3,29** : 15 % pour un changement de `-t`.

### Éliminés par lecture ou par mesure
- **les fils** : le non-déterminisme persiste à `-t 1`
- **l'appareil** : persiste à `-ngl 0`
- **la précision de H** : fp16 ≡ fp32 à 0,01 % (4,8567 contre 4,8560)
- **le noyau WHT lui-même** : `ggml_compute_forward_wht_f32` (`ops.cpp:1468`) parallélise par
  LIGNES (`row += nth`), chaque ligne étant séquentielle → déterministe par construction
- **l'aliasing en place** : `GGML_OP_WHT` est ABSENT de `ggml_op_can_inplace` (`ggml-alloc.c:22-51`)

### Ce qui reste
Le seul élément que le remède supprime et que l'op introduit : **les 338 segments de graphe et
leurs 336 traversées CPU↔GPU par passe**. C'est le dernier candidat.

### Conséquence, à porter partout
**Toute la table de descente du projet** (4,7407 · 4,0825 · 4,0723 · 4,0443 · 4,0803) a été produite
avec des modèles à poids repliés, donc par ce même chemin `ggml_wht`. **Aucune de ces valeurs n'est
garantie reproductible.** Idem pour le « +10,5 % » du portage et le « +18,1 % » de cette campagne.

## État
- Branche `feat/wht-dense-matmul`, arbre `/mnt/obrain/wt-wht-dense`
- Patch : `/mnt/obrain/patches/wht-dense-MES-SEULEMODIF.patch` (61 lignes, 3 fichiers)
- **NON rapatrié** : justesse non établie faute de référence reproductible
- Interrupteurs : `OBRAIN_WHT_DENSE=<pourcentage>` (balayage 0-100), `OBRAIN_WHT_TRACE=1` (trace)

> ### MISE À JOUR DU 2026-09-22 — le produit dense est RETIRÉ
> La transformée native (`GGML_OP_WHT` sur Vulkan, `docs/SPEC-WHT-VULKAN-NATIF.md`) **le remplace et
> le surpasse** : préfill 88,26 contre 82,73, décodage 4,76 contre 2,38 — sans sa pénalité de 45 %.
> Le retrait a été fait **ici** (`9b9505a`) puis **dans l'arbre principal** (rapatrié par `7b47999`,
> retiré par le commit « Suppression du code mort : le patch dense »), avec vérification avant/après
> (§3 de la spécification : PPL 3,8161 des deux côtés, binaire sans trace du patch).
> **`OBRAIN_WHT_DENSE`, `OBRAIN_H_CPU` et `OBRAIN_WHT_TRACE` n'existent plus** — cette trace et le
> balayage par pourcentage avaient été introduits **ici** (`b5b45c7`) et sont partis avec le patch
> dense (`9b9505a`). Seul `OBRAIN_NO_ROTATION` subsiste. Les lignes ci-dessus restent comme trace de la
> campagne qui les a produites, pas comme mode d'emploi.
>
> **Et la transformée native a rejoint l'arbre principal** (`b6767df`, cherry-pick de `3142d88`) : les
> deux fichiers concernés y sont désormais **identiques au bit** à ceux de cette branche, et les cinq
> critères y ont été **re-mesurés** — segments 2, 2 688 nœuds WHT tous sur Vulkan, préfill 89,36 t/s
> (×2,08), décodage 4,62 (intact), déterminisme `3,6372` ×3, contrôle négatif 168 976. Détail et
> commandes : `docs/SPEC-WHT-VULKAN-NATIF.md` §4bis.

## LE CONTRÔLE QUI INNOCENTE LE MOTEUR (tour 9)

Référence `Q4_K_M` — **aucun tenseur replié, aucun `ggml_wht`** — GPU, 3 essais identiques :

| modèle | 3 essais | verdict |
|---|---|---|
| **référence Q4_K_M** | **2,2257 / 2,2257 / 2,2257** | **DÉTERMINISTE** |
| ternaire v3, produit dense | **3,2067 / 3,2067 / 3,2067** | **DÉTERMINISTE** |
| ternaire v3, `ggml_wht` | 3,2762 → 4,0310 selon l'exécution | **NE SE REPRODUIT PAS** |

**Le moteur, le backend Vulkan, le corpus, le protocole et la précision sont donc tous innocentés.**
Le non-déterminisme est **propre au chemin `ggml_wht`** — et à lui seul.

### Ce que cela établit
1. Les valeurs OP (3,28 à 4,03) sont **gonflées par un défaut intermittent**, pas dispersées autour
   d'une vraie valeur : la plus basse observée (3,2762) est encore au-dessus du dense (3,2067).
2. Le produit dense **supprime ce défaut** en même temps qu'il double le débit.
3. **Corollaire lourd** : la table de descente du projet a été mesurée par le chemin défaillant.
   Ses valeurs sont donc **gonflées et non reproductibles**.

### Hypothèses éliminées (toutes, sauf une)
fils (`-t 1` varie) · appareil (`-ngl 0` varie) · précision de H (fp16 ≡ fp32) · le noyau WHT
(déterministe par lecture : `ops.cpp:1468`, parallélisation par lignes) · l'aliasing en place
(`GGML_OP_WHT` absent de `ggml_op_can_inplace`) · **le moteur** (ce tour).
Reste : **les 338 segments et leurs traversées CPU↔GPU** — le seul élément que le remède supprime.
