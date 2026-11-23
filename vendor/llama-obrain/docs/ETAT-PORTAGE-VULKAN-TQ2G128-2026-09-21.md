# Portage Vulkan de `TQ2_G128` — état et diagnostics

**Date** : 2026-09-21. **Branche** : `feat/vulkan-tq2g128` (base `integration/ternarisation-tq2g128`),
dernier commit `697c297`. **Nature** : note d'état. **Le portage est RAPIDE et FAUX.**
**Il n'est pas livrable, et la branche déployable n'a pas bougé.**

**Régime** : **MESURÉ** (exécution citée) · **CONSTATÉ** (fichier cité) · **CALCULÉ**.

---

## 1 · Ce qui marche — MESURÉ

`llama-bench`, modèle `Qwen3.8-27B-OBRAIN-v3.gguf`, `-ngl 99` :

| | avant portage | **après portage** | gain |
|---|---|---|---|
| préfill `pp128` | 6,46 t/s | **40,58 t/s** | **×6,3** |
| décodage `tg64` | 2,85 t/s | **4,80 t/s** | **×1,68** |

C'est **la première fois qu'un type ternaire est porté dans ce backend Vulkan** : `TQ1_0` et
`TQ2_0` y font aussi 0 occurrence — il n'existait **aucun patron** à recopier.

## 2 · Ce qui ne marche pas — MESURÉ, et statistiquement établi

Perplexité, **même build, même corpus, 8 segments (4 096 jetons)** :

| | perplexité |
|---|---|
| CPU (`-ngl 0`) | **3,4136 ± 0,159** |
| GPU (`-ngl 99`) | **3,7728 ± 0,188** |

**+10,5 %, et les intervalles ne se recouvrent pas.**

## 3 · Le contrôle qui accuse le portage, et non le backend — MESURÉ

La **référence** `Q4_K_M` (aucun tenseur ternaire), même build, même corpus :

| | GPU | CPU |
|---|---|---|
| référence `Q4_K_M` | 2,2257 ± 0,083 | 2,2189 ± 0,083 → **écart 0,3 %** |
| ternaire `v3` | 3,7728 ± 0,188 | 3,4136 ± 0,159 → **écart 10,5 %** |

**Le backend Vulkan n'est donc pas intrinsèquement lossy** : sur la référence, GPU et CPU donnent
la même chose. L'écart n'apparaît **qu'avec les tenseurs ternaires** → le défaut est dans le portage.

**Piège de méthode, écarté au passage** : la build Vulkan et la build CPU n'ont **pas le même backend
CPU** (la première calcule le même travail en **44 s contre 233 s** — jeu d'instructions différent).
Comparer les deux builds est donc **confondu** ; seule une comparaison **dans la même build** vaut.

## 4 · Ce que `test-backend-ops` dit, et qui ne colle pas encore — MESURÉ

Type ajouté à `all_types` (`tests/test-backend-ops.cpp`) et **cinq cas ajoutés aux dimensions réelles
du modèle** (la suite générique s'arrête à `k=256`, soit deux blocs — insuffisant pour une erreur
d'indiçage qui n'apparaîtrait qu'aux indices élevés) :

| cas | verdict |
|---|---|
| `MUL_MAT` `m=5120,n=1,k=5120` (40 blocs) | **OK** |
| `MUL_MAT` `m=6144,n=1,k=6144` (48 blocs) | **OK** |
| `MUL_MAT` `m=5120,n=32,k=5120` | **OK** |
| `MUL_MAT` `m=6144,n=8,k=6144` | **OK** |
| `MUL_MAT` `m=128,n=1,k=1280` | **OK** |
| **15/15 au total** | **le noyau de multiplication est juste** |
| `MUL_MAT_ID` | **ÉCHEC, erreur 0,319** — mais sans effet ici : le modèle est **DENSE** (CONSTATÉ : le GGUF ne porte **aucun** paramètre d'expert, seulement `feed_forward_length`), donc `MUL_MAT_ID` n'est pas emprunté |

## 5 · Où le défaut se trouve — CONSTATÉ

Le chemin réellement emprunté par le modèle, lu dans le code (`ggml-vulkan.cpp:8672-8732`) :

```cpp
vk_matmul_pipeline mmp = ggml_vk_get_mul_mat_mat_pipeline(ctx, src0->type, ...);
const bool qx_needs_dequant = mmp == nullptr || x_non_contig;   // <- vrai pour nous
...
GGML_ASSERT(!qx_needs_dequant || to_fp16_vk_0 != nullptr);
```

`TQ2_G128` **n'est pas dans le `switch (src0_type)`** de `ggml_vk_get_mul_mat_mat_pipeline`
(ligne 7209) → elle rend `nullptr` → **`qx_needs_dequant = true`** → **les poids sont déquantifiés
en fp16 par `dequant_tq2_g128.comp`**, puis un matmul fp16 est exécuté.

**Deux pièces sont donc candidates, et une seule est en cause :**

1. **le matmul quantifié direct** — validé 15/15 par le test, mais **non emprunté** par le modèle ;
2. **`dequant_tq2_g128.comp`** (chemin `to_fp16`) — **c'est ce que le modèle utilise réellement**, et
   c'est le suspect principal. Mon indiçage y traite **2 octets par fil** (32 octets par bloc contre
   16 pour Q1_0, dont j'ai copié la structure) : c'est la partie la moins éprouvée du portage.

**Tentative de correction, et son échec instructif** : ajouter le type au `switch` fait **planter**
(`segfault`) — la table `pipeline_dequant_mul_mat_mat[type]` n'est pas peuplée pour lui. Le chemin
« propre » demande donc **aussi** la création de ces pipelines. Modification **revertée** : l'état
actuel ne plante pas.

## 6 · Ce qu'il reste à faire, dans l'ordre

1. **Corriger `dequant_tq2_g128.comp`** — ou le remplacer par une écriture indexée sur `QUANT_K`
   plutôt que sur la structure copiée de Q1_0. C'est le suspect principal et le chemin réellement
   emprunté.
2. **Ou peupler `pipeline_dequant_mul_mat_mat`** pour le type, et brancher le matmul quantifié —
   chemin validé par le test, mais il faut créer ses pipelines (sinon segfault).
3. **Re-mesurer la perplexité** (8 segments) après correction : la cible est **3,41 ± 0,16**.
4. Puis seulement : la rotation sur GPU (étape 3 de l'objectif), et 256k (étape 4).

**Instrument à réutiliser** : `build-vk/bin/test-backend-ops -o MUL_MAT -b Vulkan0 -p "tq2_g128"`
(15 cas, dont les dimensions du modèle). Ce test a **innocenté le mul_mat** — c'est ce qui a
redirigé le diagnostic vers le shader de déquantification, et c'est un gain de temps réel.

---

## 7 · MISE À JOUR DU 2026-09-22 — le shader n'est plus le suspect, et c'est mesuré

**MESURÉ**, sur l'arbre principal (`feat/vulkan-tq2g128`), `build-vk` reconstruit, l'instrument du §6 :

```
LD_LIBRARY_PATH=build-vk/bin build-vk/bin/test-backend-ops -o MUL_MAT -b Vulkan0 -p "tq2_g128"
→ 15/15 tests passed, Vulkan0 OK
```

**Les 15 cas passent**, y compris ceux qui portent les **formes réelles du modèle** — `k=5120`, `k=6144`,
`n=32`, `n=8`, `k=1280` — et ceux qui balaient les petits `n` (1 à 9). Le backend Vulkan **sert donc
bien** le type (il ne se contente pas de le refuser : un refus s'afficherait `not supported`), et sa
déquantification donne le **même résultat que la référence CPU** à la tolérance du test.

**Ce que cela change** : l'étape 1 du §6 (« corriger `dequant_tq2_g128.comp` — le suspect principal »)
**n'est plus un suspect**. Le correctif rapatrié par `7b47999` (« +29 lignes, écrites par la session
parallèle ») **tient** au niveau de l'op. Le diagnostic doit donc chercher ailleurs s'il reste un écart —
ce qui est cohérent avec le §5 du `CHALLENGE-OPTIMISATIONS` : l'écart de perplexité publié était mesuré
sur le chemin **non déterministe** `ggml_wht`, et il a été invalidé.

**Ce qui reste ouvert, et il faut le dire** : l'étape 3 du §6, **re-mesurer la perplexité à 8 chunks**
(cible `3,41 ± 0,16`), n'est **pas** faite ici. Ce qui est mesuré depuis la transformée native est
`3,8519` à `-c 512 --chunks 1 -ngl 99` (et `3,8161` sur `build/`, CPU pur) : **ces deux chiffres ne se
comparent pas** à la cible, qui portait sur 8 chunks.

### Et la perplexité à 8 chunks, elle, est mesurée

`-c 512 --chunks 8 -t 8`, modèle `v3`, corpus `ppl.txt`, **les deux chemins dans la même fenêtre** :

| chemin | par passe | `[1] … [8]` | **PPL finale** |
|---|---|---|---|
| **Vulkan, transformée native** (`-ngl 99`) | **23,95 s** | 3,8519 · 4,0811 · 4,3646 · 3,6372 · 3,2568 · 3,1975 · 3,2664 · 3,2088 | **3,2088 ± 0,14617** |
| **CPU pur** (`-ngl 0`), témoin | 388,16 s | 3,8161 · 4,0748 · 4,3651 · 3,6392 · 3,2577 · 3,1991 · 3,2671 · 3,2086 | **3,2086 ± 0,14557** |

**Écart GPU–CPU : 0,0002.** Le chemin Vulkan **reproduit la référence CPU** à la quatrième décimale, et
il est **16,2× plus rapide par passe**. Le `[1]` du premier chunk — **3,8519**, la valeur de la
conversation d'origine — tombe exactement là où il avait été consigné.

**La cible, elle, est maintenant identifiée — et son artefact a disparu.** Les journaux de mesures
tranchent :

| | campagne du 2026-09-21 | aujourd'hui |
|---|---|---|
| **modèle** | v3 sha256 **`ca49bc00…`** (`/mnt/obrain/mesures/eq.log`, `ppl-fix-gpu.log`) | v3 sha256 **`1d7c9409…`**, mtime **16:58** |
| **corpus** | `ppl.txt`, sha256 `9d16815e…` | **le même** |
| **drapeaux** | `-c 512 --chunks 8 -t 8` | **les mêmes** |

Le **protocole est identique** ; seul le **fichier modèle** a changé — et il a changé **après** la
campagne (journal de 14:12, mtime du fichier 16:58). La v3 sur disque est aujourd'hui **liée en dur** à
`balayage/combo.gguf` (même inode), et **aucun autre fichier de cette taille n'existe** sur n3 :
l'artefact de la campagne **n'est plus là**. Sa ligne de base (`3,4136`) n'est donc **pas re-mesurable**
ici, et l'écart de 0,2 avec les `3,2086` d'aujourd'hui est un **changement de modèle**, pas de chemin.
Une cible sans son artefact n'est pas une cible — c'est le même constat que la table des pièges de la
SPEC au §5.

**Non-régression vérifiée le même jour** : `test-backend-ops -o MUL_MAT_HADAMARD` → **6/6** (le chemin
fusionné FWHT est intact après le durcissement de `supports_op` décrit dans
`docs/SPEC-WHT-VULKAN-NATIF.md` §4bis).
