# Optimisations du modèle ternaire — passe adversariale

**Date** : 2026-09-21. **Nature** : un même raisonnement, attaqué par plusieurs agents, puis pesé.
**Objet** : la liste des leviers d'optimisation du modèle ternaire `Qwen3.8-27B-OBRAIN-v3`
(12,51 Go, 27,32 G paramètres) — et la question de savoir lesquels tiennent.

**Régime des affirmations** : **MESURÉ** (exécution citée) · **CONSTATÉ** (fichier:ligne) ·
**CALCULÉ** (arithmétique) · **HYPOTHÈSE**. La colonne « vérifié par » dit qui a lu ou exécuté :
**humain** = relevé dans cette session, **agent** = rapporté par un sous-agent et **non reproduit**.

---

## 0 · La méthode, et pourquoi elle a mordu

Trois affirmations portées par l'auteur de la liste ont été confiées chacune à un agent **chargé de
les réfuter**, un quatrième a cherché les leviers absents, un cinquième a pesé le tout. Les agents
étaient en **lecture seule** : aucun binaire `llama-*`, aucun processus tué, rien écrit dans le
dépôt — la machine est partagée avec une autre session.

Résultat brut : **deux affirmations sur trois sont tombées**, et l'une d'elles a été réfutée par un
argument que son auteur n'avait pas vu. C'est le résultat utile de l'exercice.

---

## 1 · Ce qui a été réfuté

### 1.1 « Le décodage est borné par la bande passante » → **PARTIELLE**

Ce qui survit : la direction (à la fenêtre mesurée, moins d'octets = plus de débit) et l'arithmétique
du trafic par jeton.

Ce qui tombe :

| affirmation | ce que la mesure dit | vérifié par |
|---|---|---|
| « 60 Go/s ≈ plafond du 780M » | 60 Go/s = **67 %** du pic calculé — 2 canaux × 8 o × 5600 MT/s = **89,6 Go/s** | humain (`dmidecode -t 17` : 64 Go canal A + 16 Go canal B, DDR5-5600) |
| « aucune marge côté noyaux » | la référence `Q4_K_M` atteint **85,7 %** du même pic sur le même type d'APU | agent |
| « le portage a gagné ×1,68 sans retirer d'octet, donc le décode n'était pas borné par la bande passante » | **SOPHISME** — avant le portage les 23,1 % ternaires tournaient sur un noyau **scalaire CPU** (`ggml-cpu/quants.c:581`) : le goulot était le **calcul**, pas la DRAM ; lever un goulot de calcul ne dit rien du suivant | agent — et **c'était mon argument** ; il est rejeté |

**Défaut d'instrument, plus grave que l'affirmation** : la « borne 48-50 Go/s » qui circule dans les
documents du 2026-09-07 est obtenue par **la même formule** (`tok/s × taille du modèle`). Le corpus
affirme être au mur parce que le débit calculé ressemble au mur calculé. **Aucune mesure de bande
passante indépendante n'existe** dans le dépôt — ni STREAM, ni clpeak, ni memcpy. Le test de
saturation (à octets constants, faire varier ce qui n'est pas la bande passante) **n'a jamais été
fait**.

**Et la correction qui change la cible** : à la fenêtre de conception du modèle
(`context_length = 262144`, lu dans l'en-tête), le **cache KV coûte ~16-17 Go par jeton contre
11,87 Go de poids**. Les poids tombent à ~41 % du trafic, et supprimer **l'intégralité** du tenseur
ternaire ne rendrait que ~10 % de t/s. **Les octets à enlever sont ceux du KV, pas ceux des poids.**

### 1.2 « Emballer 5 trits par octet rend 19 % sans perte, gratuitement » → **PARTIELLE**

Ce qui survit, et je l'ai reproduit moi-même : les codes de 2 bits n'utilisent que **trois valeurs** —
sur **89 128 960 codes**, le quatrième vaut **0**. Le quantifieur l'explique
(`ggml-quants.c:2577` : `code = trit + 1`, trit ∈ {−1,0,+1}) et le validateur traite un code 3 comme
une corruption (l. 5794). Distribution : −1 **28,07 %**, 0 **43,85 %**, +1 **28,09 %**.
L'arithmétique tient : 3⁵ = 243 ≤ 256 ; 1,6 bpw utiles + 0,125 d'échelle = **1,725 bpw**, soit
**18,8 %** du ternaire, **exactement sans perte** (bijection sur la suite (trit, échelle fp16)).

Ce qui tombe :

- **L'échelle n'est pas 19 % du modèle mais 4,35 % du fichier** : le ternaire pèse 2,6926 Gio =
  23,1 % du fichier ; 18,8 % de cela = **0,507 Gio**. Le mot « gratuit » tombe aussi : il faut un
  nouveau type GGML, un nouveau groupe (640/1280, qui casse l'appariement actuel de deux groupes de
  128 par bloc `q8_K` de 256), un encodeur, un chargeur, un noyau, et refaire le shader Vulkan.
- **Ce n'est pas une idée neuve** : le format existe déjà dans l'arbre sous le nom `TQ1_0`
  (`ggml-common.h:290-294` : *« 5 elements per byte (3^5 = 243 < 256) »*) = **1,6875 bpw**, plus
  petit que les 1,725 revendiqués — et il a été **mesuré puis écarté** (erreur 0,008681 contre
  0,006233 ; produit scalaire 0,141345 contre 0,089486).
- **Le régime est à contre-sens** : le préfill est borné en **calcul** (6,46 t/s en ternaire contre
  66,92 t/s pour une `Q4_K_M` comparable). Ces octets n'achètent **aucun** token/s aujourd'hui.

### 1.3 « Sauter la lecture des poids multipliés par une activation nulle » → **RÉFUTÉE**

C'était la mienne, et je la soupçonnais déjà fausse. Elle est tombée sur **trois** clous, dont deux
que je n'avais pas vus :

1. **C'est SwiGLU, pas ReLU** (vérifié par moi) : `src/models/qwen35.cpp:495` construit
   `LLM_FFN_SILU` + `LLM_FFN_PAR` ; la branche empruntée est `ggml_swiglu_split`. Or
   `silu(x) = x/(1+exp(−x))` n'est **jamais exactement nul** hors `x = 0` ou débordement.
2. **Le grain de lecture n'est pas le poids, c'est le bloc à pas fixe** — argument **indépendant de
   la fonction d'activation**, et c'est le vrai clou. `QK_G128 = 128` poids dans 34 octets ; pour
   économiser **un octet** il faut **128 zéros consécutifs**. Mesuré : **0 bloc de 128 entièrement
   nul sur 88 233**. Même dans un monde ReLU idéal à 90 % de zéros, P = 0,9²⁵⁶ = 2 × 10⁻¹² par bloc.
3. **J'avais confondu les axes** : « les lignes de `ffn_down` » est l'axe *stridé* (dimension de
   sortie), pas l'axe indexé par l'activation. Et `ffn_down` est **Q4_K**, pas ternaire — la cible
   désignée n'était même pas le bon tenseur.

**Le remède serait plus cher que le mal** : une parcimonie explicite coûte indice + signe = 8 bits
par non-nul contre 2 bits à pas fixe ; le point d'équilibre est à **24 % de non-nuls**, or le modèle
est à **56,2 %** — la forme creuse **alourdirait** le fichier d'environ **+1,5 Gio**.

---

## 2 · Mes corrections aux agents

Une passe adversariale n'est pas infaillible ; deux des attaques ont elles-mêmes dû être corrigées.

**2.1 — Erreur de catégorie (attaque B).** L'agent a comparé le taux du packing (81,18 %) à ce que
`xz` atteint sur une tranche réelle (80,32 %) et conclu « le packing n'apporte rien ». **C'est faux** :
`xz` compresse le **fichier sur disque**. À l'inférence, un GGUF compressé doit être décompressé —
le noyau lit alors les octets **non compressés** depuis la DRAM, et le trafic par jeton est
**inchangé**. Le packing, lui, réduit les octets **lus par le noyau à chaque jeton**. Les deux ne
résolvent pas le même problème — disque contre bande passante — et **se composent** au lieu de
s'exclure. *(`xz` n'a pas d'accès aléatoire : il n'existe aucun chemin de décompression de poids dans
l'arbre — recherche de chargeur compressé : néant.)* Le fond de l'attaque B (le gain est petit, le
régime est à contre-sens) reste valable ; sa jambe comparative est retirée.

**2.2 — Ma réfutation du chemin fusionné, précisée.** J'avais écrit que le chemin fusionné ne peut
pas porter notre rotation parce que 5120 n'est pas une puissance de deux. C'est vrai, mais **la
raison exacte est plus simple et je l'ai lue** :

```
ggml_vk_fwht_pipeline_idx() :  case 64 / 128 / 256 / 512 ; default: return -1
ggml_vk_can_use_fwht()      :  if (idx < 0 || pipeline == nullptr) return false;   // refus SILENCIEUX
```

Notre rotation est en blocs de **`OBRAIN_WHT_BLOCK = 1024`** (`src/obrain-rotation.h:30`) — **1024
n'est pas dans la table**. Donc le remède nº 4 du document de portage (« brancher la rotation sur le
chemin fusionné déjà présent ») **ne peut pas fonctionner**, ni en 1024, ni en 5120. Le bon geste est
d'implémenter `GGML_OP_WHT` **par blocs** côté Vulkan à partir du `fwht.comp` existant.

---

## 3 · Ce que la passe a trouvé et que personne n'avait écrit

### 3.1 ★ 563 lignes de code de performance **mort** — vérifié par moi

`llama_kv_cache_project_hidden` — `src/llama-context.cpp:4838`, accolade fermante **l. 5400**, soit
**563 lignes**. Recherche dans tout le dépôt : **3 occurrences**, toutes non-appelantes
(un commentaire l. 4726, la définition, la déclaration `include/llama.h:1436`). **Zéro appelant.**

Ce que la fonction contient : un **GEMM CBLAS** avec déquantification CPU (`cblas_sgemm` l. 5117 et
5125), un cache global de poids déquantifiés (`g_dequant_cache`, l. 70), un `fopen("/tmp/…", "w")`
**non conditionnel**, et cette phrase, **l. 4956** :

```c
// This approach is proven correct and fast with CBLAS (~100ms vs 17s scalar).
```

**×170 revendiqué dans un commentaire, jamais encaissé.** Le chemin est **mort in-tree**, et — pire —
`llama_kv_cache_clear_dequant_cache` (l. 5402, déclarée `include/llama.h:1447`) est morte **aussi** :
le cache global n'a **aucun chemin d'invalidation**. Les deux défauts s'annulent aujourd'hui, et se
réveilleront le jour où quelqu'un branchera la première.

**Statut** : ★★★★★ en **information**, ★★☆☆☆ en **exploitation** — le « ×170 » est
**CONSTATÉ DANS UN COMMENTAIRE**, pas mesuré, et le chemin n'est pas branchable en l'état.

### 3.1bis ★★ MESURÉ — le défaut du portage **n'est pas là où le document le désigne**

Cinq mesures de perplexité, **même binaire** (`build-vk`, version `d225d66`), **même modèle**
(`Qwen3.8-27B-OBRAIN-v3.gguf`, sha256 `ca49bc00…`), **même corpus** (`/mnt/obrain/corpus/ppl.txt`,
sha256 `9d16815e…`), **mêmes drapeaux** (`-c 512 --chunks 8 -t 8`) : **seul `-ngl` change**.

| modèle | bras | perplexité | s/passe | écart GPU-CPU |
|---|---|---|---|---|
| **référence `Q4_K_M`** (aucun ternaire, aucune rotation) | GPU / CPU | **2,2257** ± 0,083 / **2,2189** ± 0,083 | 27,7 / 67,7 | **0,31 %** ✓ |
| `v3`, rotation **active** | GPU / CPU | **4,0310** ± 0,209 / **3,4135** ± 0,161 | 48,8 / 82,3 | **+18,1 %** ✗ |
| `v3`, rotation **désarmée** (`OBRAIN_NO_ROTATION=1`) | GPU / CPU | 183 661 / 183 051 | 23,8 / 64,1 | 0,33 % *(saturé)* |

**Trois faits, et une interprétation.**

1. **Le bras CPU reproduit la ligne de base publiée à quatre décimales** : 3,4135 contre 3,4136.
   La ligne de base contestée au §3.3 est donc bien **attribuable** — mon protocole est le bon, et le
   contrôle sur la référence reproduit lui aussi exactement le tableau du document (2,2257 / 2,2189).
2. **Le noyau ternaire est innocent** : `test-backend-ops -o MUL_MAT -b Vulkan0` rend **15/15 `OK`**
   sur `type_a=tq2_g128`, y compris aux dimensions réelles du modèle (m=5120,n=1,k=5120 ;
   m=6144,n=1,k=6144 ; m=5120,n=32,k=5120 ; m=6144,n=8,k=6144 ; m=128,n=1,k=1280).
3. **La rotation est numériquement juste** en isolation : `test-obrain-wht-device` rend
   **1,04e-09** de relatif — mais confirme que **Vulkan déclare l'op non supportée** et que
   **tous les nœuds WHT vont au CPU**.

**Interprétation (à porter comme telle)** : la divergence **disparaît** quand on retire la rotation et
**n'existe pas** sur un modèle sans ternaire. Le noyau et l'op étant justes *séparément*, le défaut
est dans **leur interaction — la traversée hôte↔périphérique** que la rotation impose (168 fois par
passe) — et **non** dans `dequant_tq2_g128.comp`.

**Conséquence directe** : le document `ETAT-PORTAGE-VULKAN-TQ2G128` désigne le shader de
déquantification comme « le suspect principal » et le chemin fp16 comme « ce que le modèle utilise
réellement ». **La mesure le contredit** — et c'est cohérent avec le fait que le correctif de +29
lignes (qui route vers le matmul quantifié) **n'a pas réparé la perplexité** : il corrigeait le
mauvais suspect. Le `3,7728` publié pour le GPU **n'est pas reproduit** (j'obtiens 4,0310) ; savoir si
le correctif a *dégradé* le résultat demande de reconstruire l'état d'avant, ce que je n'ai pas fait.

**Réserve honnête sur le point 3** : à PPL ≈ 183 000 la mesure est **saturée** (distribution quasi
uniforme, insensible aux petites erreurs), donc le bras « rotation désarmée » est un contrôle
**faible**. C'est le contrôle sur la **référence** (2,2 — non saturé) qui porte la conclusion, et lui
est solide.

### 3.1ter ★★★ MESURÉ — le mécanisme : **168 nœuds WHT coupent le graphe en 338 morceaux**

Instrument : `GGML_SCHED_DEBUG=2` **plus** `-v` (le tracé passe par le journal de débogage, filtré au
niveau par défaut — sans `-v` on n'obtient que 9 lignes). Même modèle, mêmes drapeaux, `-ngl 99`,
1 segment.

| grandeur | rotation **active** | rotation **désarmée** (`OBRAIN_NO_ROTATION=1`) |
|---|---|---|
| couches déchargées | **66/66 sur GPU** | **66/66 sur GPU** |
| nœuds du graphe | 3 855 | 3 687 |
| **segments du graphe** (`sched_reserve`) | **338** | **2** |
| **nœuds WHT** | **168 — dont 168/168 sur CPU, 0 sur Vulkan** | **0** |
| temps par passe, GPU | **48,79 s** | **23,84 s** |
| temps par passe, CPU | 82,32 s | 64,12 s |

**Les trois chiffres qui suffisent :**

1. **168 nœuds ajoutés → 336 segments ajoutés.** Le graphe passe de **2** à **338** segments, alors
   que seuls 168 nœuds supplémentaires apparaissent (3 687 → 3 855). Chaque WHT posé sur le CPU
   **coupe le graphe une fois** : le rapport est exactement 1 pour 1.
2. **La rotation coûte 24,95 s par passe sur GPU (51 % du temps)** et 18,20 s sur CPU (22 %). Elle
   coûte donc **37 % de plus en absolu sur le GPU que sur le CPU** — l'inverse exact de ce qu'un
   déchargement doit produire. C'est la signature de la traversée hôte↔périphérique.
3. **Aucun des 168 WHT n'atterrit sur Vulkan** : le backend déclare l'op non supportée, et
   `test-obrain-wht-device` l'avait déjà écrit (« Vulkan0 : non »).

**Conséquence chiffrée (CALCULÉ)** : le GPU ne rend aujourd'hui que **×1,69** face au CPU (48,79 contre
82,32 s). Ramenée sur l'appareil, la rotation est un `O(n log n)` négligeable devant les matmuls ;
le temps de passe devrait donc s'approcher des **~24 s** mesurés sans elle, soit **×2,0 sur le
préfill** — *et* la justesse retrouvée, puisque c'est cette même traversée qui produit les +18,1 %.

**Ce que coûte réellement la rotation — trois hypothèses mises à l'épreuve.** Le Δ mesuré est de
**24,95 s/passe** (GPU) et 18,20 s (CPU). Ce n'est **ni le calcul ni la bande passante** :

| hypothèse | calcul | part du Δ |
|---|---|---|
| ① le calcul du WHT — 168 × 1,751 ms × (2048/512) | **1,18 s** | **4,7 %** |
| ② la bande passante des transferts — 168 × 2 × 2048 × 5120 × 4 o = **13,12 Gio/passe**, à 50-90 Go/s | 0,16-0,28 s | **0,6-1,1 %** |
| ③ le **nombre de coupes du graphe** — 336 segments ajoutés | — | **74,3 ms par segment** (GPU), 54,2 (CPU) |

**Seule ③ rend compte du Δ.** Mon explication initiale (« la traversée hôte↔périphérique coûte cher »)
était **fausse de deux ordres de grandeur** : les octets transférés ne pèsent que 1 %. Ce qui coûte
est le **fractionnement** : 338 segments au lieu de 2, chacun payant synchronisation et ordonnancement.

**La règle de conception qui en découle, chiffrée** : *tout ce qui supprime des segments rapporte*,
même au prix de calcul. Deux voies, aucune n'exigeant de nouveau noyau :

- **(a) Compter la rotation avec une multiplication dense.** Remplacer l'op `ggml_wht` autonome par un
  `ggml_mul_mat` contre une matrice de Hadamard **matérialisée** (diagonale par blocs, 5120×5120) —
  sans le hint FWHT, donc empruntant le matmul Vulkan ordinaire. Coût : 5120² = **26,2 M MACs par
  rotation**, soit 168 × 26,2 M = **4,4 G MACs par jeton, +16 %** sur les ~27,3 G du modèle. Gain :
  **−336 segments**, soit ~24,9 s sur 48,79 s. **Attendu ≈ 27-28 s/passe, soit ×1,8 sur le préfill** —
  *et* la justesse, puisque c'est cette traversée qui produit les +18,1 %.
- **(b) La même chose en cinq multipliations 1024×1024** (la matrice est diagonale par blocs) :
  **5,2 M MACs par rotation, +3,2 %** seulement. À préférer si le découpage en blocs n'introduit pas
  lui-même de nouvelles coupes — ce qui reste à vérifier, et c'est exactement le piège que (a) évite.

Le calcul est **CALCULÉ**, pas mesuré : je n'ai pas écrit ce remède. Mais il transforme un problème
ouvert (« comment accélérer le GPU ? ») en une modification locale, bornée et chiffrée.

**Pourquoi le correctif de +29 lignes n'a rien réparé** : il visait le shader de déquantification, qui
n'est pas dans le chemin de la divergence. Le défaut est dans **le découpage du graphe**, pas dans un
noyau — et aucun test de noyau isolé ne peut le voir (les 15/15 de `test-backend-ops` et le 1,04e-09
de `test-obrain-wht-device` sont tous deux obtenus avec **tout sur le CPU**, condition qui n'est pas
celle du modèle).

### 3.1quater Spécification du remède — vérifiée sur pièces, **non implémentée**

Trois vérifications faites avant d'écrire une ligne de code, pour ne pas construire sur du sable :

1. **`H` est exactement représentable en fp16.** Sa construction est
   `H[i][j] = popcount(i & j) paire ? +1/32 : −1/32` (`tests/test-obrain-rotation-ggml.cpp:44-55`,
   Sylvester normalisée). Ses valeurs sont **±1/32, une puissance de deux** : le stockage fp16 est
   donc **sans perte**, et la rotation dense ne coûte **aucune précision**. Elle est de plus
   **symétrique** (`popcount(i&j) = popcount(j&i)`), donc `Hᵀ = H` et l'ordre des indices n'importe pas.
2. **Le chemin dense existe déjà et est validé.** `llama_mul_mat_hadamard()`
   (`src/llama-impl.h:61-78`) fait exactement *reshape → `ggml_mul_mat` → reshape*, et le test
   compare op ≡ matrice dense ≡ repli scalaire à **0,000e+00**. Le motif de tenseur-constante existe
   aussi : `llama_kv_cache::build_input_k_rot()` (`src/llama-kv-cache.cpp:1475-1494`) crée un
   tenseur de rotation déclaré en entrée, rempli par `set_input_k_rot()` (`:1980-1986`).
3. **Vulkan sait faire le matmul requis.** `test-backend-ops -o MUL_MAT -b Vulkan0` : les cas
   **fp16 avec `k=1024` passent** (`m=16 n=16 k=1024 OK`, `m=16 n=1 k=1024 OK`). Le prérequis de
   l'option (b) — un matmul 1024×1024 **sur l'appareil** — est donc satisfait.

**Le remède, en une phrase** : dans le crochet `src/llama-graph.cpp:1482-1486`, remplacer
`cur = ggml_wht(ctx0, cur, OBRAIN_WHT_BLOCK)` par un matmul dense contre un `H` **constant**
(1024×1024, fp16, ±1/32), après un `ggml_reshape_2d` qui découpe `ne[0]` en blocs de 1024.

**Ce qui n'est pas écrit, et pourquoi je m'arrête là** : la difficulté n'est pas la ligne de graphe,
c'est **l'acheminement de la constante `H` jusqu'au graphe**. Deux voies propres, toutes deux réelles :

- **(i) une entrée de graphe** — miroir exact de `build_input_k_rot` : nouveau champ dans
  `llm_graph_context`, tenseur créé avec `ggml_set_input`, rempli une fois par passe (4 Mio en fp16,
  négligeable devant 11,65 Gio). ~100 lignes réparties sur `llama-graph.h`, `llama-graph.cpp` et le
  ramassage des entrées — dans un **dépôt partagé où une autre session écrit**.
- **(ii) un tenseur du modèle** créé au chargement, ce qui évite le ramassage d'entrées mais touche
  `llama-model.cpp`.

**Ni l'une ni l'autre n'est une ligne**, et les deux modifient des fichiers que l'autre session
occupe ou côtoie. Écrire cela à moitié laisserait l'arbre sale — l'inverse de ce que la mesure exige.
La spécification ci-dessus est complète ; l'implémentation est un chantier à part entière, à mener
dans un **arbre dédié**, pas dans celui-ci.

**Ce qui est établi sans l'implémenter** : le coût (74,3 ms par segment), la cible (−336 segments,
attendu ≈ 27-28 s/passe), et le fait que **la voie est ouverte** — `H` exacte en fp16, chemin dense
déjà validé, matmul 1024×1024 déjà supporté par le backend.

### 3.2 ★ Le correctif Vulkan est écrit, compilé… et **jamais mesuré**

**CONSTATÉ par moi** : l'arbre de travail porte **+29 lignes non commitées** (`ggml-vulkan.cpp` +5,
`mul_mm_funcs.glsl` +24), et `case GGML_TYPE_TQ2_G128` y est présent **6 fois** (l. 7140, 7214, 7282,
7375, 7446, 17268) — dont celle du `switch` de `ggml_vk_get_mul_mat_mat_pipeline` que le document de
portage disait avoir *revertée*.

**Rapporté par agent, non reproduit** : la chaîne d'horodatages montre le `.spv` recompilé à
03:52:02 après l'édition de 03:51:57, un `llama-perplexity` à 03:52:37 et la bibliothèque reliée à
03:54:52 — mais **aucun journal postérieur ne l'évalue** (le seul est un vidage de mémoire). Autrement
dit : le `+10,5 % de perplexité` publié a peut-être été mesuré **avant** le correctif.

### 3.3 La ligne de base : le bras CPU **reproduit**, le bras GPU **non** — mesuré

**Correction de ce document par lui-même.** Une première rédaction (sur la foi d'un agent) concluait
que la ligne de base était *inattribuable*, parce qu'aucun journal présent sur la machine ne porte
3,4136 :

| journal | valeur | ligne de commande conservée |
|---|---|---|
| `ppl-native2.log` | 4,2059 ± 0,229 (8 segments) | **non** |
| `ppl-vk.log` | 4,8764 | **non** |
| `ppl-vk-cpu.log` | 4,3603 | **non** |
| `ppl-native.log` | 405 706 (exécution cassée) | **non** |
| `ppl-fix.log` | *(aucune PPL — vidage de mémoire)* | **non** |

**La mesure refait la conclusion.** En rejouant le protocole avec une attribution complète, le bras
**CPU reproduit 3,4135 contre 3,4136 publié** — soit quatre décimales. La ligne de base CPU est donc
**parfaitement attribuable**, et mon protocole est bien le bon (§3.1bis).

Ce qui ne se reproduit pas, c'est le **bras GPU** : le document publie **3,7728** (+10,5 %), j'obtiens
**4,0310** (+18,1 %). Le modèle, le corpus et les drapeaux étant identiques — puisque le CPU tombe
juste —, l'écart ne peut venir que de l'**état du binaire** : le document est daté 03:46:28, le
correctif de +29 lignes a été écrit de 03:48 à 03:52. **Inférence (haute confiance, non prouvée) : le
correctif a *dégradé* le résultat GPU de +10,5 % à +18,1 %**, en corrigeant un suspect qui n'était pas
le bon. La prouver demanderait de reconstruire l'état d'avant — **je ne l'ai pas fait**.

Le reste du constat tient : **aucun** de ces journaux ne conserve sa ligne de commande, et aucun ne
porte le `sha256` du modèle. C'est un défaut d'hygiène réel, et il a coûté ici une conclusion fausse
— la mienne.

### 3.4 Le placement mémoire n'est pas ce que l'on croyait

**Vérifié par moi** : `dmidecode` donne deux barrettes **asymétriques** — 64 Go (canal A) + 16 Go
(canal B), DDR5-5600. **Rapporté par agent, non reproduit** : `numactl --hardware` donnerait **deux
nœuds** — nœud 0 = 16 092 Mo **avec tous les CPU**, nœud 1 = 48 124 Mo **sans aucun CPU**, distances
10/20. Ce n'est donc pas un problème d'entrelacement de barrettes mais une topologie **NPS2 avec un
nœud mémoire pur** — et le modèle (11,65 Gio) **ne peut pas tenir** sur les 16 Go du nœud 0.

**Le projet n'a jamais écrit `--numa` ni `numactl` une seule fois.** Le mécanisme proposé par l'agent
doit être réécrit ; le levier, lui, reste entier et se teste en trois commandes.

---

## 4 · Les leviers, ordonnés — et leur statut

| # | levier | gain | statut | coût |
|---|---|---|---|---|
| **0** | **✅ FAIT — mesurer le correctif déjà compilé** | **négatif, et instructif** : le GPU reste à **+18,1 %** du CPU ; le noyau ternaire est innocenté (15/15) ; le défaut est **à la rotation**, pas au shader de déquantification | **MESURÉ** (§3.1bis) | fait |
| **1** | **Rendre la rotation calculable sur Vulkan** (`GGML_OP_WHT` par blocs, ou recâblage) | **c'est le seul défaut de justesse identifié** : sans lui, aucun gain GPU n'est utilisable | CONSTATÉ (mécanisme) / **localisé par mesure** (§3.1bis) | élevé — mais désormais **ciblé** |
| **2** | **Sauver les 29 lignes non commitées** | ce n'est pas un gain, c'est un **risque** : une autre session écrit dans ce dépôt | ✅ **fait** — patch + copies + sha256 dans `/mnt/obrain/patches/` | fait |
| **3** | **KV quantifié** `-ctk q8_0 -ctv q4_0` (+ `-fa on`) | **×1,54 au décodage à 256k** (65 536 → 26 624 o/position) ; **nul à contexte court** | CALCULÉ ; autorisé par la liste blanche Vulkan | 2 commandes, **aucune ligne de code** |
| **4** | **Placement mémoire** (`--membind`, `--interleave`) | magnitude **inconnue**, signe inconnu | HYPOTHÈSE | 3 commandes |
| **5** | **SIMD pour le noyau ternaire** (le CPU reste le chemin de repli obligé tant que la rotation y tombe) | attaque le préfill, borné en **calcul** | CONSTATÉ (absent : `grep tq2_g128 ggml/src/ggml-cpu/arch/` = 0) | élevé |
| **6** | **Recodage base 3 / entropique des trits** | 4,1-4,4 % du fichier ; plancher 1,6754 bpw si l'on vise l'entropie mesurée (H = 1,5506) | CALCULÉ, sans perte | **élevé** — à faire en dernier |
| **7** | **`llama_kv_cache_project_hidden`** | ×170 revendiqué dans un commentaire | CONSTATÉ / jamais mesuré | 563 lignes à auditer |

**Ce que le levier 0 change dans l'ordre** : le shader de déquantification **sort de la liste** — il
n'était pas le suspect, et le correctif qui le visait a laissé la perplexité fausse (voire pire). Le
premier poste est désormais **la rotation**, parce que c'est le seul endroit mesuré où le GPU et le
CPU divergent.

**Écartés, avec leur raison** : le SIMD du noyau ternaire (absent partout —
`grep tq2_g128 ggml/src/ggml-cpu/arch/` = 0 — mais ne concerne que le repli CPU) ; l'imatrix pondérée
(morte **et** rejetée par la mesure : PPL 83,09 → 175,65 — **ne pas « réparer »**) ; les 40 shaders
Vulkan « non référencés » et les `#if 0` de `spacemit` (faux positifs, non compilés sur x86-64) ;
l'état récurrent SSM (144 Mio constants, mais **114 fois** plus petit que le KV à 256k — ce n'est pas
le levier du contexte long, contrairement à ce que je supposais).

---

## 5 · Le plan de mesure

**Hygiène désormais acquise** : toute mesure est lancée avec **la commande en tête de fichier** et le
**`sha256` du modèle et du corpus**. Les cinq mesures du §3.1bis sont archivées dans
`/mnt/obrain/mesures/` avec cette attribution, et la ligne de base CPU **reproduit** (3,4135 contre
3,4136 publié). Le reste du défaut d'hygiène tient : les anciens journaux ne portent ni commande ni
empreinte, et publient `-r 1` — à `-r 1`, tout écart inférieur à 5 % est ininterprétable.

0. **✅ FAIT — la justesse du portage.** GPU 4,0310 contre CPU 3,4135, même binaire, mêmes drapeaux.
   Le noyau ternaire est innocenté (15/15) ; la référence donne 0,31 % d'écart. **Le défaut est à la
   rotation.**
1. **Localiser précisément le croisement hôte↔périphérique.** Le test isolé (`test-obrain-wht-device`)
   place **tout** sur le CPU : il ne reproduit donc pas la condition réelle. Instrument à écrire :
   journaliser le placement des nœuds WHT quand `-ngl 99` est actif, et comparer la sortie de
   `ggml_wht` côté CPU à ce que le GPU attend — c'est le seul endroit où les deux chemins divergent
   aujourd'hui. *Attendu* : identifier si la faute est la copie, les dimensions, ou la contiguïté.
   *Invalidant* : si le placement est correct et la copie exacte, l'écart vient d'ailleurs — et il
   faudra rouvrir le dossier du matmul.
2. **KV quantifié.** Qualité à `-c 4096 --chunks 4 -fa on`, témoin `-ctk f16 -ctv f16` contre
   `-ctk q8_0 -ctv q4_0` ; débit à `-p 32768 -r 3`. *Invalidant* : dégradation de PPL au-delà de
   quelques pour cent (essayer alors `-ctv q8_0`). **Indépendant du défaut** — c'est le seul levier du
   dossier qui ne dépende ni de la rotation ni des noyaux ternaires.
3. **Placement.** Trois bras, mêmes drapeaux : défaut, `numactl --interleave=all`,
   `numactl --membind=1`. *Invalidant* : trois valeurs identiques à la dispersion près.
4. **Ampleur du repli de rotation**, une fois (1) compris : `OBRAIN_NO_ROTATION=1` des deux côtés, en
   **ne lisant que le temps**. Le bras GPU passe déjà de 48,8 à **23,8 s** et le CPU de 82,3 à
   **64,1 s** — un premier ordre de grandeur est là.

---

## 6 · Réserve

- **Le débit n'a pas été mesuré.** Les cinq mesures du §3.1bis sont des **perplexités** ; les chiffres
  de débit du dossier (6,46 / 40,58 / 2,85 / 4,80 t/s) **n'ont pas été reproduits** — seuls les temps
  par passe le sont (48,8 contre 82,3 s/passe, soit ×1,69 sur les bras de perplexité).
- **Le risque principal n'est pas arithmétique, il est épistémique** : la matière des agents vient de
  **lecture de code**, et le fait le plus spectaculaire (« ×170 ») est un **commentaire**. Un dossier
  où la preuve est statique finit par prendre la cohérence pour une validation. **Cette passe en a
  donné la démonstration directe** : un agent concluait que la ligne de base était inattribuable, et
  j'ai reproduit cette conclusion *dans ce document* avant que la mesure ne la contredise (§3.3).
- **L'axe le plus solide du dossier est aussi le moins utile** : la distribution des trits est le seul
  chiffre reproduit indépendamment, et c'est le seul sans enjeu de performance.
- **Une autre session écrit dans ce dépôt en ce moment.** Les 29 lignes non commitées qui portent le
  seul gain de performance identifié peuvent disparaître dans un `git checkout`.

---

## A8 · DÉCOUVERTE MAJEURE — l'instrument de perplexité du projet n'est pas reproductible

**Trouvé en vérifiant mon propre travail au 8ᵉ tour, pas en cherchant.** Trois exécutions aux
arguments **rigoureusement identiques** :

| modèle | 3 essais | verdict |
|---|---|---|
| **référence `Q4_K_M`** — aucun poids replié, aucun `ggml_wht` | **2,2257 / 2,2257 / 2,2257** | **déterministe** |
| ternaire `v3`, rotation par **produit dense** | **3,2067 / 3,2067 / 3,2067** | **déterministe** |
| ternaire `v3`, rotation par **`ggml_wht`** | **3,2762 → 4,0310** selon l'exécution | **ne se reproduit pas** |

Le non-déterminisme persiste à `-t 1`, à `-ngl 0`, en fp16 comme en fp32. Il **disparaît** avec un
modèle sans repli. Il est donc **propre au chemin `ggml_wht`**.

**Hypothèses éliminées, chacune par lecture ou par mesure :** les fils · l'appareil · la précision de
`H` · le noyau WHT lui-même (`ggml_compute_forward_wht_f32`, `ops.cpp:1468`, parallélise **par
lignes**, séquentiel dans chaque ligne → déterministe par construction) · l'aliasing en place
(`GGML_OP_WHT` **absent** de `ggml_op_can_inplace`, `ggml-alloc.c:22-51`) · le moteur Vulkan (contrôle
sur la référence). **Reste : les 338 segments de graphe et leurs 336 traversées CPU↔GPU par passe.**

**Conséquence, et elle dépasse cette campagne :** la table de descente du projet (4,7407 · 4,0825 ·
4,0723 · 4,0443 · 4,0803) a été mesurée avec des modèles à **poids repliés**, donc par ce même chemin.
**Aucune de ces valeurs n'est garantie reproductible.** Il en va de même du « +10,5 % » du document
`ETAT-PORTAGE-VULKAN-TQ2G128` et du « +18,1 % » de la présente campagne — ces deux écarts sont
maintenant **invalidés**.

**La direction de l'erreur est établie** : la plus basse valeur OP jamais observée (3,2762) reste
**au-dessus** du dense stable (3,2067) — le défaut **gonfle** la perplexité, il ne la disperse pas
autour d'une vraie valeur.

**Remède rapatrié** : le produit dense (`OBRAIN_WHT_DENSE`, **inerte par défaut**) supprime le défaut
en même temps qu'il double le débit. Détail complet : `docs/ETAT-WHT-DENSE-2026-09-21.md`.

*(2026-09-22 — ce produit dense a depuis été **RETIRÉ**, remplacé par la transformée native : voir
`docs/SPEC-WHT-VULKAN-NATIF.md` §3. L'interrupteur `OBRAIN_WHT_DENSE` n'est plus lu par le code. Les
chiffres ci-dessus restent ceux de la campagne.)*
