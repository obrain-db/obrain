# Tester `WHT dense` sur le DGX — mode d'emploi (révision 2)

**Date** : 2026-09-21. **Révision 2** : corrige le §0, ajoute le blocage `TQ2_G128`/CUDA, rectifie l'empreinte
du modèle, les paramètres de compilation et les règles de cohabitation avec la prod. Les corrections
viennent d'une reconnaissance faite **sur le DGX lui-même** ; je les ai **vérifiées localement** une par
une (voir §0.3), et j'indique lesquelles.

**But** : reproduire sur le DGX (NVIDIA GB10, CUDA) ce qui a été mesuré sur n3 (AMD 780M, Vulkan), et
trancher : **le défaut du chemin `ggml_wht` existe-t-il sous CUDA, et le remède dense le supprime-t-il ?**

**Régime** : **MESURÉ** (exécution citée) · **CONSTATÉ** (fichier:ligne) · **CALCULÉ** · **NON VÉRIFIÉ**.

---

## 0 · Le chemin de code — ce que je m'étais trompé

### 0.1 Mon §0 précédent était faux, et mon test était cassé

J'avais écrit : *« le kit DGX ne contient pas le travail de ternarisation, aucun ancêtre commun »*.
**C'est faux.** J'avais comparé `llama-obrain-dgx` (l'arbre **amont llama.cpp**, qui n'est **pas** la
prod) avec `llama-obrain`, et surtout j'avais lancé :

```bash
git merge-base HEAD /home/triviere/projects/llama-obrain/HEAD   # ← ceci n'est PAS valide
```

Ce n'est **pas** une façon de comparer deux dépôts : `git merge-base` cherche un ancêtre **dans le
dépôt courant**, et un chemin de fichier n'y est pas un commit. **Mon « aucun ancêtre commun » ne
voulait rien dire.** Je le retire.

### 0.2 La réalité — **CONSTATÉ**, vérifié par moi

| | |
|---|---|
| **la prod DGX** | `dgx:~/projects/llama-obrain-fix`, branche **`fix/prod-unifiee`** |
| **la racine commune** | **`934ec562d`** — et ce commit **existe dans mon dépôt** (`git cat-file -t 934ec562d` → `commit`) |
| le serveur de prod | port **9910** |
| merge simulé de la prod avec `feat/wht-dense-matmul`, `feat/vulkan-tq2g128`, `integration/ternarisation-tq2g128` | **aucun conflit** |

**C'est donc un MERGE, pas un portage.** Le §0 de la révision 1 surestimait le travail.

### 0.3 La procédure, travaillée **dans un clone séparé**

```bash
# sur le DGX — ne PAS toucher llama-obrain-fix
git clone ~/projects/llama-obrain-fix ~/projects/llama-obrain-wht
cd ~/projects/llama-obrain-wht
git fetch triviere@192.168.1.103:projects/llama-obrain feat/wht-dense-matmul
git merge FETCH_HEAD
```

Le clone séparé est **la bonne pratique** : la prod continue de tourner sur son propre arbre et son
propre binaire, et rien de ce qu'on fait ici ne peut la toucher.

---

## 0bis · LE BLOCAGE QUE JE N'AVAIS PAS VU — `TQ2_G128` n'a aucun noyau CUDA

**CONSTATÉ, vérifié par moi** :

| chemin | fichiers mentionnant `TQ2_G128` |
|---|---|
| `ggml/src/ggml-cuda/` | **0** |
| `ggml/src/ggml-vulkan/` | 5 |
| `ggml/src/ggml-cpu/` | 2 |

**Conséquence directe, et elle invalide le plan de test de la révision 1** : le modèle v3 contient
**168 tenseurs `TQ2_G128`**. Sous CUDA, ces multiplications de matrices **basculeront sur le CPU**.

Autrement dit, mesurer le graphe sur DGX **sans noyau CUDA pour le type** mesurerait **le repli CPU du
ternaire** — et non l'effet de `ggml_wht`, ni celui du remède dense. **Le nombre de segments, le
déterminisme et les débits seraient tous pollués par ce repli**, et l'on ne pourrait rien conclure.

**Il faut donc, avant toute mesure, au moins un noyau CUDA minimal** : **déquantification `TQ2_G128`
vers `f16`, puis cuBLAS**. C'est la voie la plus courte — pas besoin d'un noyau fusionné.

**Autre piste, qui ne lève pas ce blocage mais vit au même endroit** : implémenter `GGML_OP_WHT` en
CUDA (§6). Les deux pièces touchent `ggml-cuda/` et peuvent être faites ensemble.

**Le critère de recevabilité est simple** : quand `grep -rl TQ2_G128 ggml/src/ggml-cuda/` ne rend plus
0 fichier, la mesure a un sens. Avant, non.

> ### MISE À JOUR DU 2026-09-22 — le blocage est LEVÉ
> Les **deux pièces** sont écrites et mesurées sur le DGX, sur la branche **`feat/cuda-tq2g128`**
> (document complet : `docs/ETAT-PORTAGE-CUDA-TQ2G128-2026-09-22.md`) :
>
> - **déquantification `TQ2_G128`** (`convert.cu`) → `test-backend-ops -p tq2_g128` : **15/15** sur CUDA0 ;
> - **`GGML_OP_WHT`** (`wht.cu`, papillon en mémoire partagée, **cas général** `block < ne[0]` compris)
>   → `test-backend-ops -o WHT` : **13/13** sur CUDA0.
>
> **Et la question du §5.2 a sa réponse** : sur le modèle `v3` (sha256 `1d7c9409…`, identique à n3),
> corpus `ppl.txt` (`9d16815e…`), **segments = 2** (et non 338), **2 688 / 2 688 nœuds WHT sur CUDA**,
> **déterminisme 3× identique**, et **GPU 3,6183 contre CPU 3,6361, soit 0,49 %** — le « +10,5 % »
> publié appartenait à l'état où la rotation retombait sur le CPU.
>
> ⚠️ **Les débits du §5.4 ne sont toujours pas mesurés** : la prod tourne sur la même machine, et ce
> document exige une fenêtre calme. Les sections 5.3 et 5.4 restent donc **sans objet** : le bras
> `OBRAIN_WHT_DENSE` qu'elles posaient a été retiré du code (voir `docs/SPEC-WHT-VULKAN-NATIF.md` §3).

---

## 1 · L'accès au DGX

| test (fait depuis n3) | résultat |
|---|---|
| `ping 192.168.1.7` | **répond** |
| port 22 | **OUVERT** |
| `ssh dgx-selfd` (sans commande) | `selfd-ssh-gate: refusé — aucune commande` |
| `ssh dgx-selfd "uname -a"` | `selfd-ssh-gate: refusé — forme inattendue (mots entre apostrophes attendus)` |
| `ssh triviere@192.168.1.7` (direct) | `Permission denied (publickey,password)` |

La clé est **restreinte à une commande forcée `selfd-ssh-gate`** (commentaire du `~/.ssh/config`).
**Je n'ai pas sondé son vocabulaire par essais successifs** — deviner les commandes d'une passerelle
restreinte sur une machine distante n'est pas une méthode acceptable.

**Note d'accès** : `obrain-db` **14c790b3** (l'accès SSH) — lisible depuis n3.
**Note technique** : `obrain-db` **82637562**.

**Correction de registre** : `MACHINES-CONNUES-2026-09-15.md` donne **`dgx .107`** — **faux**.
L'adresse est **`192.168.1.7`**.

---

## 2 · Ce qui est établi sur n3 — **et sur quel artefact**

### 2.1 ⚠️ La faille d'artefact — je dois la déclarer

**Le fichier `/mnt/obrain/models/Qwen3.8-27B-OBRAIN-v3.gguf` a été RÉÉCRIT le 21/09 à 16:58.**
C'est un lien physique (`liens 2`) vers `balayage/combo.gguf`, et l'empreinte actuelle est
**`1d7c9409ae7f2c7f6071c8ad…`**, taille **12 844 231 424 o** *(mesurée par moi, conforme à l'annonce)*.

**Mes mesures ne portent donc pas toutes sur le même fichier** :

| plage | fichier | ma mesure |
|---|---|---|
| 14:35 → 16:00 | **ancien** (12 510 038 784 o, `ca49bc00…`) | perplexité, segments, déterminisme |
| 16:44 → 16:48 | **ancien** | campagne 5 cibles (×1,81 préfill, TTFT ×18,5) |
| **17:07 → 17:46** | **nouveau** (12,84 Go) | campagnes 30 exécutions |
| **20:53 → 20:58** | **nouveau** | **banc `llama-bench` (×1,92 préfill, −45 % décodage)** |

**Les résultats structurels tiennent** — le nombre de segments et le déterminisme dépendent du **graphe**,
pas des poids. Et le banc est **interne** (les deux bras, même session, même fichier). Mais **la
perplexité et le banc ne portent pas sur le même artefact**, et je le dis plutôt que de le laisser croire.

**À faire sur n3** : rejouer la perplexité sur le fichier actuel si l'on veut un couple cohérent.

### 2.2 Le défaut — **MESURÉ** sur n3

| configuration | backends | **segments** | déterminisme |
|---|---|---|---|
| binaire CPU seul (`build/`) | **1** | **1** | **stable** (4,0748) |
| `build-vk`, Vulkan désactivé | **1** | **1** | **stable** (4,0748 ×2) |
| `build-vk`, **OP** `-ngl 99` | plusieurs | **338** | **instable** (6,10 / 6,38 / 12,57) |
| `build-vk`, **OP** `-ngl 0` | plusieurs | **1 196** | **instable** (4,57 / 4,99) |
| **DENSE** | plusieurs | **2** | **stable** (3,8116 ×3) |

**Cause** : `GGML_OP_WHT` n'est pas supportée par Vulkan. Dans un planificateur **multi-backend**, elle
force un placement qui **coupe le graphe à chaque nœud** — **1 196 segments même à `-ngl 0`**. Ce n'est
pas le transfert (copies hôte→hôte à `-ngl 0`, corruption persistante) : c'est **le fractionnement**.

**Éliminés un par un** : fils (`-t 1`) · appareil (`-ngl 0`) · précision de `H` (fp16 ≡ fp32) · le noyau
WHT (`ops.cpp:1468`, parallélisé par lignes) · l'aliasing en place (`GGML_OP_WHT` absent de
`ggml_op_can_inplace`) · le moteur (référence Q4_K_M stable).

### 2.3 Le remède et son coût — **MESURÉ** (sur le fichier **actuel**)

| | **OP** | **DENSE** | |
|---|---|---|---|
| préfill `pp512` | 43,02 ± 0,05 t/s | **82,73 ± 0,60 t/s** | **×1,92** |
| décodage `tg128` | **4,30 ± 0,78 t/s** | **2,38 ± 0,08 t/s** | **−45 %** |
| TTFT (chat réel) | 450,9 ms | **24,3 ms** | ×18,5 |

**Cause de la pénalité** : **168 petits matmuls à n = 5 par jeton**, mauvaise forme pour RADV. **Ce
n'est pas les FLOPs** (déjà optimaux : 5,2 M MACs par rotation, +3,2 %).

---

## 3 · Prérequis sur le DGX — paramètres corrigés

| pièce | valeur **corrigée** | vérification |
|---|---|---|
| GPU | **NVIDIA GB10**, compute capability **12.1** | `nvidia-smi -L` |
| CUDA | **13.2** installé | `nvcc --version` |
| **architecture de compilation** | **`-DCMAKE_CUDA_ARCHITECTURES=121`** — **ni 89, ni 90** | — |
| modèles | **il n'y a PAS de `/mnt/obrain`** sur le DGX — utiliser **`~/projects/models`** (511 Go libres) | `df -h` |
| transfert | **`rsync` lancé DEPUIS le DGX**, qui atteint n3 directement | — |
| **le modèle** | `Qwen3.8-27B-OBRAIN-v3.gguf` · **12 844 231 424 o** · sha256 **`1d7c9409ae7f2c7f6071c8ad…`** | `sha256sum` |
| **le corpus** | `ppl.txt` · 58 743 o · sha256 `9d16815ecd5c1e1a…` | `sha256sum` |
| **le code** | §0 — un **merge**, dans un **clone séparé** | — |
| **⚠️ le noyau CUDA pour `TQ2_G128`** | **N'EXISTE PAS** — §0bis. **Sans lui, aucune mesure n'a de sens.** | `grep -rl TQ2_G128 ggml/src/ggml-cuda/` |

Transfert (depuis le DGX) :

```bash
mkdir -p ~/projects/models
rsync -avP triviere@192.168.1.103:/mnt/obrain/models/Qwen3.8-27B-OBRAIN-v3.gguf ~/projects/models/
rsync -avP triviere@192.168.1.103:/mnt/obrain/corpus/ppl.txt                ~/projects/models/
sha256sum ~/projects/models/Qwen3.8-27B-OBRAIN-v3.gguf   # attendu 1d7c9409…
```

**Méthode, apprise à nos dépens** : **la ligne de commande et le sha256 en tête de chaque journal.**
Trois mesures de cette campagne ont dû être refaites faute de l'avoir fait.

---

## 4 · Compilation — avec les contraintes de la prod

```bash
cd ~/projects/llama-obrain-wht
cmake -S . -B build-cuda -G Ninja \
  -DGGML_CUDA=ON -DGGML_CUDA_FA=ON \
  -DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=ON -DLLAMA_CURL=OFF \
  -DCMAKE_CUDA_ARCHITECTURES=121
nice -n 19 cmake --build build-cuda -j4 --target llama-perplexity llama-bench llama-server
```

⚠️ **`-j4` et `nice`, pas `-j$(nproc)`** : la prod tourne sur la même machine.

---

## 5 · Les mesures

### 5.1 Préalable — lever le blocage §0bis

Tant que `grep -rl TQ2_G128 ggml/src/ggml-cuda/` rend **0**, **ne pas mesurer** : les chiffres
porteraient sur le repli CPU du ternaire.

### 5.2 Le défaut existe-t-il sous CUDA ? — **le cœur du test**

```bash
M=~/projects/models/Qwen3.8-27B-OBRAIN-v3.gguf
C=~/projects/models/ppl.txt

# a) OP, 3 exécutions IDENTIQUES
for i in 1 2 3; do
  LD_LIBRARY_PATH=build-cuda/bin build-cuda/bin/llama-perplexity \
    -m "$M" -f "$C" -c 512 --chunks 4 -t 8 -ngl 99 2>&1 \
    | grep -oE "^\[[0-9]+\][0-9.,]+|Final estimate: PPL = [0-9.]+"
done
```

**Attendu si le défaut se reproduit** : les trois listes `[1]` **diffèrent** (n3 : 6,38 / 6,10 / 12,57).
**Si les trois sont identiques** : le défaut est **propre à Vulkan/RADV** — résultat majeur.

```bash
# b) compter les segments — c'est cette mesure qui NOMME la cause
GGML_SCHED_DEBUG=2 LD_LIBRARY_PATH=build-cuda/bin build-cuda/bin/llama-perplexity -v \
  -m "$M" -f "$C" -c 512 --chunks 1 -t 8 -ngl 99 2>&1 \
  | grep -oE "graph splits = [0-9]+|graph nodes = [0-9]+" | sort -u
```

⚠️ **`-v` est OBLIGATOIRE** : sans lui le journal est filtré et l'on n'obtient **que 9 lignes** (payé sur n3).
Attendu : **≈ 338** si le défaut se reproduit, **1-2** sinon.

### 5.3 Le remède corrige-t-il ? — ⚠️ **CE BRAS N'EXISTE PLUS (2026-09-22)**

> Le produit dense a été **retiré du code** (branche `feat/wht-dense-matmul`, commit `9b9505a`, puis
> arbre principal) : **`OBRAIN_WHT_DENSE` n'est plus lu**, et la ligne `H dense 1024x1024` ne peut plus
> apparaître. Ce qui le remplace est la **transformée native** (`GGML_OP_WHT` sur l'appareil) — mais
> elle n'existe **que sur Vulkan** (`docs/SPEC-WHT-VULKAN-NATIF.md`) : sous CUDA, §0bis reste le
> blocage, et le remède n'y a pas de forme. **La seule mesure de ce §5 qui reste exécutable est
> §5.2b** (compter les segments) — c'est elle qui nomme la cause.

*Historique, conservé pour la trace — cette commande n'a plus cours :*

```bash
# sans objet : OBRAIN_WHT_DENSE n'est plus lu par le code
OBRAIN_WHT_DENSE=100 LD_LIBRARY_PATH=build-cuda/bin build-cuda/bin/llama-perplexity \
  -m "$M" -f "$C" -c 512 --chunks 4 -t 8 -ngl 99 2>&1 \
  | grep -oE "^\[[0-9]+\][0-9.,]+|Final estimate: PPL = [0-9.]+|H dense 1024x1024"
```

Attendu, **quand ce bras existait** : la ligne `H dense 1024x1024` apparaissait, les segments tombaient
à **2**, et les trois exécutions donnaient **le même** `[1]`.

### 5.4 Le coût sous cuBLAS — **la question ouverte**, désormais sans bras `DENSE`

> Sans objet aujourd'hui : le bras `DENSE` **et son interrupteur n'existent plus**. Les références n3
> ci-dessous se rapportent au produit dense **retiré** ; elles restent dans l'histoire de la campagne
> (`docs/ETAT-WHT-DENSE-2026-09-21.md`), pas dans une procédure exécutable.

*Historique, conservé pour la trace — cette boucle n'a plus cours :*

```bash
for ARM in OP DENSE; do   # le bras DENSE n'existe plus
  if [ "$ARM" = "DENSE" ]; then export OBRAIN_WHT_DENSE=100; else unset OBRAIN_WHT_DENSE; fi
  echo "== $ARM =="
  LD_LIBRARY_PATH=build-cuda/bin build-cuda/bin/llama-bench \
    -m "$M" -ngl 99 -p 512 -n 128 -r 5 2>&1 | grep -E "pp512|tg128"
done
```

**Références n3** : préfill 43,02 → **82,73** t/s (×1,92) · décodage **4,30 → 2,38** t/s (−45 %).

⚠️ **À faire dans une fenêtre calme** : *« les débits mesurés pendant que la prod tourne ne seront pas
comparables à n3 »*. **Le déterminisme (§5.2) se teste tout de suite** ; **les débits, non.**

---

## 6 · Critères de décision — écrits d'avance

| observation | conclusion |
|---|---|
| OP non déterministe **et** ≈338 segments sous CUDA | le défaut est dans **l'op + le planificateur multi-backend**, pas dans Vulkan → **porter le remède partout** (CUDA, Metal) |
| OP **déterministe** sous CUDA | le défaut est **propre à Vulkan/RADV** → ne pas activer le remède sous CUDA sans raison |
| DENSE : décodage **≥ 4,0 t/s** | **feu vert** — le remède gagne sur les deux tableaux → l'activer par défaut |
| DENSE : décodage **< 3,0 t/s** | la pénalité n'est pas propre à RADV → la voie est **`GGML_OP_WHT` natif** (§7) |

**Jamais sur une seule exécution.** Toute affirmation de non-déterminisme exige **trois essais
identiques** — l'erreur qui a coûté le plus cher ici : un écart 2687/2688 annoncé « trouvé », démenti
par le contrôle à trois essais.

---

## 7 · La voie native — **CALCULÉ**

Implémenter `GGML_OP_WHT` **sur l'appareil** (CUDA et Vulkan) : la seule voie qui donne **les deux** —
préfill ×1,92 (les coupes disparaissent) **et décodage inchangé** (FWHT en O(n log n), aucun matmul).

- **Vulkan** : `vulkan-shaders/fwht.comp` **existe** (115 lignes), mais `ggml_vk_fwht_pipeline_idx` ne
  connaît que **64/128/256/512** alors que notre bloc vaut **1024** (`src/obrain-rotation.h:30`).
- **CUDA** : **rien** — 0 occurrence.

**Les deux pièces (`TQ2_G128` et l'op) vivent au même endroit** : `ggml-cuda/`. À faire ensemble.

**Chiffre qui validera** : décodage **≈ 4,3 t/s** sans perdre le **×1,92** du préfill.

---

## 8 · Règles de cohabitation avec la prod — **impératives**

| | |
|---|---|
| **thermique** | à 00:45 le GPU était à **95 % et 78 °C** et le **plafond d'horloge était inactif** — c'est la configuration du **power-off du 19/09**. Lancer **`~/dgx-anti-poweroff.sh` (sudo) AVANT**. |
| **mémoire** | **22 Go disponibles sur 121**. Le modèle de test fait **12,8 Go**, en plus du **27B de prod** (262k de contexte, 16 Go de cache), du **VL-8B**, du **1.7B** et de **bge-m3**. **Surveiller `free`** — un OOM tuerait la prod. |
| **binaire de prod** | **ne JAMAIS toucher `llama-obrain-fix/build-fix`** |
| **compilation** | `nice` + **`-j4`** |
| **processus** | **tuer par PID uniquement** — jamais `pkill -f llama-server` (il a tué mon serveur en pleine 4ᵉ question sur n3, tout a été perdu) |
| **ports** | serveur de test sur un port libre, **hors 9905-9910 et 9921-9934** |
| **débits** | non comparables pendant que la prod tourne → **fenêtre calme** |

---

## 9 · Pièges de mesure déjà payés

| piège | correctif |
|---|---|
| `GGML_SCHED_DEBUG=2` sans `-v` | 9 lignes, aucun tracé → **ajouter `-v`** |
| `2>&1 > fichier` | stderr sur le terminal, tracé perdu → **`> fichier 2>&1`** |
| `echo $!` après `cd … && nohup … &` | capture le PID d'un sous-shell → **deux serveurs ont fui et tenu 32 Go** ; vérifier avec `ss -ltnp` |
| conclure sur **une** exécution | faux verdict → **trois essais minimum** |
| mesure sans sa ligne de commande | refaite trois fois → **commande + sha256 en tête de journal** |
| `-r 1` | écart < 5 % ininterprétable → **`-r 3` minimum**, publier la dispersion |
| comparer deux builds | confondu (le backend CPU diffère) → **comparer dans le même binaire** |

---

## 10 · Ce qu'il faut renvoyer

1. `grep -rl TQ2_G128 ggml/src/ggml-cuda/` — **le blocage est-il levé ?**
2. Les **trois** listes `[1]` par bras, avec la commande et le sha256 du modèle.
3. Le **nombre de segments** pour OP et DENSE, `-ngl 99` **et** `-ngl 0`.
4. Le tableau `pp512` / `tg128` des deux bras, `-r 5`, avec dispersions.
5. La ligne `[obrain] H dense 1024x1024`.
6. `nvidia-smi -L`, `nvcc --version`, et l'état thermique au moment du test.

---

## 11 · Ce que ce document n'établit pas

- **Je n'ai jamais exécuté sur le DGX ni sur le M4.** Tout est **MESURÉ sur n3** ou **CONSTATÉ par
  lecture**. La compatibilité DGX est **déduite** — solide, mais **pas une exécution**.
- **Le chemin d'accès est résolu par tes notes, pas par moi** (§1).
- **Le noyau CUDA pour `TQ2_G128` n'existe pas** (§0bis) : **c'est le véritable obstacle**, pas le GPU.
- **Mes mesures n3 ne portent pas toutes sur le même fichier de modèle** (§2.1).
