# GPU — la base MESURÉE, et le vrai verrou

**Date** : 2026-09-21. **Branche** : `integration/ternarisation-tq2g128` (`c69a752`).
**Machines** : n3 (192.168.1.103) et **n2 (192.168.1.102)** — même APU (Ryzen 7 8845HS +
**Radeon 780M**, `RADV PHOENIX`), n2 avec **109 Go** de RAM contre 62 sur n3.

**Régime** : **MESURÉ** (exécution, commande citée) · **CONSTATÉ** (fichier cité) · **CALCULÉ** ·
**NON VÉRIFIÉ**.

---

## 1 · Ce que la note de reprise affirmait, et ce que la mesure dit

La note dit : *« le binaire Vulkan sert le modèle ternaire à 2,01 tok/s contre 1,90 en CPU pur →
tout reste sur le CPU »*. **C'est faux en partie**, et la nuance change la cible :

**MESURÉ** — `llama-bench` sur n3, modèle `Qwen3.8-27B-OBRAIN-v3.gguf` :

```bash
build-vk/bin/llama-bench -m /mnt/obrain/models/Qwen3.8-27B-OBRAIN-v3.gguf -ngl 0  -t 8 -p 128 -n 64 -r 1
build-vk/bin/llama-bench -m /mnt/obrain/models/Qwen3.8-27B-OBRAIN-v3.gguf -ngl 99 -t 8 -p 128 -n 64 -r 1
```

| backend | préfill (pp128) | décodage (tg64) |
|---|---|---|
| CPU pur (`-ngl 0`) | 6,00 t/s | 1,77 t/s |
| **Vulkan (`-ngl 99`)** | 6,46 t/s | **2,85 t/s** |

**Vulkan accélère déjà le décodage de 61 %.** La raison : les tenseurs NON ternaires du modèle
(`token_embd` Q4_K, `output` Q6_K, `attn_v` Q4_K, `ffn_down` Q4_K, et les couches de bord) **sont**
déchargeables — seuls les tenseurs `TQ2_G128` ne le sont pas, faute de noyau.

**Et le périphérique est bien là** (`--list-devices`) :
`Vulkan0: AMD Radeon Graphics (RADV PHOENIX)` — 48 492 Mio sur n3, **72 636 Mio sur n2**.

---

## 2 · Le vrai verrou n'est pas le décodage : c'est le PRÉFILL

**CONSTATÉ** — `ggml_vec_dot_tq2_g128_q8_K` (`ggml/src/ggml-cpu/quants.c:581`) ne contient
**aucune intrinsèque** : c'est un noyau scalaire de référence, bloc par bloc.

**CALCULÉ** — ce que cela coûte pour un prompt, au préfill mesuré :

| contexte | à 6,00 t/s (CPU) | à 6,46 t/s (Vulkan) |
|---|---|---|
| 4k | 11,4 min | 10,6 min |
| 32k | 1,52 h | 1,41 h |
| 128k | 6,07 h | 5,64 h |
| **256k** | **12,1 h** | **11,3 h** |

**Un prompt de 256k demanderait donc plus de onze heures.** Ce n'est pas « lent » : c'est
inutilisable. Le décodage (2,85 t/s) est un confort à côté.

**MESURÉ — et voici l'ordre de grandeur de la correction, sur la MÊME machine et le MÊME moteur**
(smoke test de n2, modèle déjà présent là-bas) :

| modèle sur le 780M de n2 | taille | préfill (pp64) | décodage (tg32) |
|---|---|---|---|
| **référence `Q4_K_M`** | 17,66 Gio | **66,92 t/s** | 4,05 t/s |
| **ternaire `v3`** | 11,64 Gio | **6,46 t/s** | 2,85 t/s |

**Un facteur 10,4 sur le préfill, à modèle comparable en taille.** La référence est **entièrement**
déchargeable (Q4_K est supporté par Vulkan) ; le ternaire retombe sur le CPU. L'écart **est** le
noyau manquant.

**CALCULÉ** : au régime de la référence, un prompt de 256k passerait de **11,3 h à ~65 min**.

---

## 3 · Deux chemins existent déjà dans le code, et l'un est orphelin

**CONSTATÉ — le shader WHT Vulkan existe et il est câblé.** `vulkan-shaders/fwht.comp` (115 lignes,
commit `934ec56`, nôtre) alimente `pipeline_fwht_f32`, et `ggml_vk_can_use_fwht()`
(`ggml-vulkan.cpp:9397`) le déclenche **à l'intérieur du matmul** :

```cpp
if (ggml_get_op_params_i32(dst, 1) != GGML_HINT_SRC0_IS_HADAMARD) return false;
```

**CONSTATÉ — le chemin fusionné côté llama existe… et personne ne l'appelle.**
`llama_mul_mat_hadamard()` (`src/llama-impl.h:57-75`) pose exactement ce hint
(`ggml_mul_mat_set_hint(res, GGML_HINT_SRC0_IS_HADAMARD)`) — et `grep -rn mul_mat_set_hint src/`
ne trouve **que sa propre définition**. C'est du **code mort**.

**CONSTATÉ — le crochet du graphe prend l'AUTRE chemin** (`src/llama-graph.cpp:1482-1486`) :
il insère une op `ggml_wht` **autonome**, que Vulkan **ne supporte pas** — d'où le repli CPU et
l'aller-retour hôte↔périphérique que la note redoutait. Le remède est donc déjà écrit, il n'est
simplement pas branché.

**La note de reprise est donc en retard sur le code** : *« un shader `GGML_OP_WHT` à écrire »* — il
est écrit.

---

## 4 · Le portage à faire, et ce qu'il coûte

**CONSTATÉ — il n'y a AUCUN patron ternaire à recopier dans Vulkan** : `TQ1_0` et `TQ2_0` font
**0 occurrence** dans `ggml-vulkan.cpp`, comme `TQ2_G128`. Les shaders de déquantification existent
pour `q4_k`, `q5_k`, `q6_k`, `mxfp4`… — **aucun pour un ternaire**.

Le travail, en quatre pièces :

| # | pièce | fichier |
|---|---|---|
| 1 | déquantification TQ2_G128 (34 octets → 128 poids, codes 2 bits + échelle FP16) | `vulkan-shaders/dequant_*.comp` + `dequant_funcs.glsl` |
| 2 | déclaration du type dans le générateur de shaders | `vulkan-shaders-gen.cpp` |
| 3 | capacité de `MUL_MAT` / `MUL_MAT_VEC` pour le type (les `switch` vers `ggml-vulkan.cpp:7249-7289`) | `ggml-vulkan.cpp` |
| 4 | brancher la rotation sur le chemin **fusionné** déjà présent (§3) | `src/llama-graph.cpp` |

**NON VÉRIFIÉ** : le gain du portage lui-même. Le facteur 10,4 du §2 est l'écart **entre deux
modèles**, pas la mesure du portage — c'est une **borne supérieure** de ce qu'on peut espérer,
mesurée sur la même machine. La mesure directe viendra quand le type sera supporté.

---

## 5 · n2 : état, et ce qu'il reste à faire

**MESURÉ / CONSTATÉ :**

- n2 joint, même APU, **109 Go de RAM**, GPU vu par Vulkan (**72 636 Mio**).
- **Un second NVMe de 953,9 Go (`nvme0n1p1`, ext4) existe et n'est PAS monté** ; `/mnt/data`
  (root, vide) l'attend ; `/etc/fstab` ne le mentionne pas.
- **Le montage exige un mot de passe** : `udisksctl` refusé par polkit
  (`NotAuthorizedCanObtain` — pas d'agent d'authentification sans TTY), et `sudo` demande un mot de
  passe. **Je ne peux pas le monter moi-même.**
- **17 Go libres** sur le seul volume monté — le modèle ternaire (12,5 Go) y tiendrait de justesse,
  sans marge.
- **n2 ne peut pas CONSTRUIRE** : `glslc` et les en-têtes Vulkan sont absents, et l'installation
  demande `sudo`. **Mais il peut EXÉCUTER** : le binaire Vulkan de n3 lui a été livré
  (`rsync`, 69 Mo) et **il tourne** — c'est ce qu'établit le smoke test du §2.

**La commande qui débloque (une seule, et c'est la tienne) :**

```bash
ssh triviere@192.168.1.102
sudo mount /dev/nvme0n1p1 /mnt/data && sudo chown triviere:triviere /mnt/data
# puis, si tu veux qu'il survive au redémarrage :
echo "/dev/nvme0n1p1 /mnt/data ext4 defaults 0 2" | sudo tee -a /etc/fstab
```

Ensuite je copie le modèle ternaire, les sources de la branche et les artefacts de mesure sur n2, et
n2 devient le nœud de run pendant que n3 garde son harness.
