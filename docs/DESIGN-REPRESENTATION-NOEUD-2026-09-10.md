# Représentation par nœud — décisions de conception

Statut : **décisions** issues d'un panel de 5 analyses indépendantes puis d'une phase de tissage
croisé, arbitrées par des mesures exécutées. Date : 2026-09-10.
Document de diagnostic amont : `docs/PROPOSAL-OPERATEUR-SPECTRAL-COURBE-2026-09-10.md`.

**Méthode.** Cinq angles (théorie de la représentation · systèmes et coût · consommateurs et
fonction · topologies et robustesse · évaluation et méthode), puis cinq agents de tissage
recevant chacun les quatre autres angles et devant **réviser** leur position. Les désaccords
non tranchables par argument ont été **mesurés** : c'est ce qui a décidé D2 et D6, et ce qui a
réfuté deux recommandations de l'auteur du diagnostic (dont une de ce document, cf. §6). Les six
tables de mesures sont en §1 ; elles sont reproductibles et aucune n'est une opinion.

---

## 0. Le constat qui commande tout

Sur son **propre graphe de production** — 363 346 nœuds, 1 176 915 arêtes — **44,4 % des nœuds
n'ont aucune arête sortante** (161 384 puits, mesuré par anti-jointure). Le garde
`min_deg < 1e-10` de `spectral_embedding` se déclenche donc **nécessairement**, et les facettes 0-1
valent la constante `0.5` pour tous les nœuds ; `is_degenerate` détecte la facette constante et la
**remplace silencieusement** par des rangs de degré.

> **Les 16 premières des 64 dimensions ne sont pas spectrales en production : ce sont des rangs de
> degré, sur un graphe à queue lourde — c'est-à-dire le régime où cette quantité est
> *anti*-informative.**

La signature n'est ni trop grosse ni trop petite : elle encode la mauvaise chose, et elle le fait
dans le régime le plus défavorable. Les mesures le confirment : l'encodage actuel est **sous le
hasard** sur 3 des 5 topologies testées.

---

## 1. Ce qui est mesuré (reproductible, exécuté)

AUC = probabilité qu'une arête soit plus proche qu'une non-arête ; **0.5 = aucune information**,
en dessous = anti-information. `SE(AUC) ≈ 0.045` à n ≈ 80-200 : tout écart < 0.15 est du bruit.

**T1 — Généralité.** 64d actuel contre 16 coordonnées de repères farthest-point (sans étiquette) :

| Topologie | 64d actuel | **16 repères bruts** | gain |
|---|---|---|---|
| anneaux + 8 communautés (80 n) | 0.566 | **0.832** | +0.266 |
| scale-free, attachement préférentiel m=2 (200 n) | 0.479 | **0.831** | +0.171 |
| grille 2D 14×14 | 0.619 | **0.999** | +0.364 |
| arbre binaire (63 n) | **0.405** | 0.675 | +0.270 |
| biparti fichiers↔symboles (120 n) | 0.673 | 0.844 | +0.154 |

**T2 — Sans vérité terrain.** Repères FPS globaux : 0.832 / 0.738 (AUC globale / intra-communauté)
contre 0.904 / 0.801 avec les communautés **vraies** → **92 % du gain retenu**, et **meilleur** sur
P@88 (0.364 contre 0.091). Les repères n'ont besoin d'**aucune partition**.

**T3 — Budget égal : 16 composantes spectrales contre 16 repères.**

| Topologie | 64d actuel | 16 eigvecs | **16 repères** | repères + degré | degré seul |
|---|---|---|---|---|---|
| anneaux 80 n | .566 | .445 | **.980** | .857 | .500 |
| scale-free 200 n | .479 | **.218** | **.831** | **.239** | **.200** |
| grille 14×14 | .619 | .282 | **.999** | .978 | .629 |
| biparti 120 n | .673 | .500 | **.844** | .663 | .570 |

**T4 — Budget contre mécanisme** (mêmes 16 repères, trois encodages) :

| Topologie | **i — 16 coords brutes (16d)** | ii — 8 facettes × 2 repères → Hilbert (64d) | iii — cascade actuelle (64d) |
|---|---|---|---|
| anneaux 80 n | **0.988** | 0.787 | 0.566 |
| scale-free 200 n | **0.839** | 0.508 | 0.477 |
| grille 14×14 | **0.999** | 0.921 | 0.618 |
| taux d'ex æquo | 0.001-0.002 | 0.000 | 0.000 |

**T5 — Position authentique ou rôle déguisé ?** Corrélation de rang des 16 coordonnées avec le
degré, et AUC restreinte par quantile de degré :

| Topologie | \|ρ\| ≥ 0.7 | max \|ρ\| | AUC q≤2 % | 2-10 % | 10-50 % | q≥50 % |
|---|---|---|---|---|---|---|
| anneaux 80 n | **0 / 16** | 0.41 | 1.000 | 1.000 | 0.991 | 0.984 |
| scale-free 300 n | **0 / 16** | 0.36 | 0.952 | 0.937 | 0.862 | 0.839 |
| grille 16×16 | **0 / 16** | 0.55 | 0.998 | 1.000 | 1.000 | 0.999 |
| biparti 150 n | **0 / 16** | 0.62 | 0.860 | 0.882 | 0.864 | 0.909 |

**T6 — Production.** 363 346 nœuds · 1 176 915 arêtes · **161 384 puits (44,4 %)** · 16 296 `File`
· 37 730 `Function` · 39 % des `Function` n'appellent jamais (graphe quasi-biparti, queue lourde).

---

## 2. LES DÉCISIONS

### D1 — Séparer position et rôle ; le rôle sort de l'espace métrique

**Mesure décisive** : ajouter le degré au vecteur de repères fait **s'effondrer** l'encodage —
scale-free **0.831 → 0.239** (Δ 0.592 ≫ 2·SE 0.09), anneaux 0.980 → 0.857, biparti 0.844 → 0.663.
Le degré seul donne **0.200** en scale-free : c'est le mécanisme de l'anti-information de la
cascade actuelle (biais de taille des arêtes — une coordonnée monotone en degré rend les arêtes
systématiquement *plus loin* que les non-arêtes).

**Raison structurelle, pas numérique** : la similarité de rôle est **invariante** à l'isomorphisme
de composante ; la proximité de position y est **sensible par définition**. Formulé en jauge : une
métrique n'existe qu'à l'isométrie près du groupe d'automorphismes de la structure qui l'engendre.
**Aucun plongement unique ne peut être les deux.**

**Rôle conservé hors L2, en colonnes typées, avec sa propre clé d'invalidation.** Nuance tranchée
par le panel contre la proposition initiale : le rôle **reste persisté** — `search_by_role` le fait
déjà (`substrate.hks_cache`, HKS 3 échelles `[f32;3]`, clé `edge_hw`, **778 → 88 ms** avec parité
exacte). Le recalculer ferait passer le chemin froid de 88 à 778 ms pour rien. La règle est
**une clé d'invalidation par vue**, pas « persister ou recalculer ».

*Test falsifiable* : Spearman(coord_i, quantile de degré) sur les 16 coordonnées, plus AUC par
quantile. Si ≥ 12/16 ont |ρ| ≥ 0.7 **ou** si l'AUC décroît fortement en queue → la « position » est
du rôle déguisé et D1 est requalifié. **Mesuré (T5) : 0/16 à |ρ| ≥ 0.7 et AUC ≥ 0.84 dans tous les
quantiles → D1 tient.**

### D2 — `pos` = 16 coordonnées brutes, **sans encodage Hilbert**, sans `rank_normalize` global

C'est la décision la plus fortement étayée, et celle qui a départagé deux théories concurrentes
(le mal est-il le *budget* ou le *mécanisme* ?). **T4 tranche : le budget.**

* Les 16 coordonnées brutes (16 dims) **écrasent** la même information encodée par la cascade
  Hilbert (64 dims) : scale-free **0.839 contre 0.508**, anneaux 0.988 contre 0.787, grille 0.999
  contre 0.921.
* Les taux d'ex æquo sont négligeables (≤ 0.002) : le gain **n'est pas** un artefact de départage.
* L'encodage Hilbert **hiérarchique** (`niveau_{k+1} ≈ g/4 + niveau_k/4`) fait que les 8 niveaux
  d'une facette portent **une** information échantillonnée 8 fois — ce qui explique, au chiffre
  près, la participation 3,3/64 et les 29 paires de colonnes identiques de la cascade actuelle.

⇒ **16 f32, une coordonnée par repère, distances normalisées par la profondeur max atteinte.**
Pas de `rank_normalize` global (qui force une marginale uniforme et fabrique un ordre total
arbitraire sur les facettes catégorielles), pas de courbe de Hilbert. Quatre fois moins de mémoire
(64 o/nœud contre 256 o) **et** deux fois plus de discrimination.

### D3 — Repères par farthest-point sampling déterministe, **sans aucune partition**

**T2** : 92 % du gain retenu sans étiquette, meilleur P@88. Cela **supprime** toute dépendance aux
communautés — et donc au Leiden du projet, dont le `refine` **ignore son paramètre `_gamma`** et
dont la variante pondérée par courbure n'a aucun appelant. On retire une dépendance fragile et la
fuite méthodologique en même temps.

À retenir du panel : **stratifier** par communautés (comme strates, pas comme sélecteur), exclure
le top 1 % de degré (hubs), et tester avant tout multi-échelle la borne LMR (D5). Le danger réel
n'est pas la partition, c'est l'**ordre d'énumération** (`seed1 = node_ids[0]`).

### D4 — Refuser la PCA globale comme mécanisme de production

La compression mesurée est gratuite (PCA-16 ≡ 64d, N ≡ L), **mais une PCA globale rend chaque ligne
dépendante de toutes les autres** : ni incrémentale, ni recalculable localement, ni stable après
insertion — et sa base n'est pas fixée (même argument de jauge qu'en D1). **Refusée** par 4 agents
sur 5, pour trois raisons convergentes (non-incrémentale, jauge non fixée, non localisable).

Ce que la mesure valide est un **budget de coordonnées fixe et local** (16 repères), pas une
projection globale.

### D5 — La décroissance en `n` est un test **obligatoire** avant toute généralisation

Borne LMR : à dimension fixe, la résolution en distance se sature quand le diamètre croît, donc
**l'AUC locale doit décroître en `n`**. Seuil retenu par le panel : **AUC(8000) ≤ AUC(80) − 0.09**
(≈ 2·SE). Test : même générateur, même graine, n = 80 / 800 / 8000, **courbe** et non point.
*Non mesuré à ce jour — c'est la première chose à exécuter.*

### D6 — Deux métriques, et la seconde ne peut pas être remplacée par la première

Le projet possède **déjà** un instrument sur la base réelle : `docs/rfc/zorder-hilbert-dimension-base.md`
§5ter mesure le gain `zruns/scan` **par quantile de degré**, et le profil est **non monotone** —
q = 50 % → **0,95×** (perte), q = 2 % → **4,31×**, q = 0,5 % → **6,91×**. Une AUC, invariante par
transformation monotone des scores, **ne peut pas voir** cette structure.

* **Métrique de conception** (locale, dans le dépôt) : profil d'AUC par quantile de degré,
  ≥ 5 quantiles, **plus** le taux d'ex æquo et `AUC_max = 1 − ½·P(ex æquo)`. Jamais d'AUC, de
  `P@k` ou de MAP sans publier `k/|V|`.
  *Leçon chiffrée de cette étude* : j'ai d'abord utilisé `k` = nombre d'arêtes, donc `k > |V|` —
  métrique **invalide**, et c'est ce qui expliquait une contradiction apparente (0.722 puis 0.000).
* **Métrique d'acceptation en production** : `g(q)` du RFC, avec un **plancher bilatéral** :
  gain(q=2 %) ≥ 4,31× **et** gain(q=50 %) ≥ 0,95×. Un axe qui gagne en tête **et** perd dans le
  corps est **refusé**.

Seuils chiffrés retenus : gain AUC ≥ **0.03** en moyenne sur le banc ; scale-free doit battre le
témoin « degré seul » de **≥ 0.15** ; **aucune** coordonnée à contribution négative > 0.03 ;
participation ratio ≥ **0.375·d** (le 16-eigvecs, avec 16 vraies directions, fait 0.218 — le nombre
de dimensions n'est pas l'information) ; **0** paire de colonnes identiques ; ex æquo ≤ 0.10 ;
quantile de MAP minimal ≥ **0.10** ; corps ≥ **3×** l'actuel (mesuré 4,5×).

### D7 — Dégénérescences : trois corrections, dont une risque déjà active

1. **`d = ∞` doit être distinct de `0` et du repère.** Aujourd'hui `unwrap_or(&0)` encode un nœud
   **non atteint** comme « à distance 0 de la graine » — avec 44 % de puits et un BFS sortant seul,
   des composantes entières sont à l'origine. C'est le vrai silence, pas l'effondrement (que
   `is_degenerate` traite).
2. **Supprimer la normalisation par maximum global** (`max_d1`) : un maximum global rend l'ajout
   d'une arête au diamètre capable de rescale **tous** les nœuds — aucune invalidation
   incrémentale n'est correcte avant cette correction.
3. **Contrôle de bonne dimension insuffisant** : il ne détecte pas le mélange de slots (toutes les
   dimensions restent valides).

### D8 — Un producteur unique, écriture différentielle, fraîcheur par tampon

Deux producteurs coexistent sans contrat partagé (`hilbert_enricher.rs:202` et
`neo4j2obrain/src/pipeline.rs:1255`, ce dernier découvert par `graph_query` seul). La sonde de
couverture échantillonne un **préfixe**, alors que les slots sont triés par (communauté, Hilbert) :
le préfixe est une communauté, et il suffit que 10 % de ce préfixe soit couvert pour que le tick
devienne un **no-op définitif**. `hilbert_features_incremental` n'a **aucun appelant** et fait
quatre passes plein graphe (son en-tête annonce `O(|changed|·deg)`). `feature_epoch` existe et
fonctionne mais n'est **pas** bumpée par l'écriture de propriétés, et ne survit pas à une
réouverture.

**Décisions** : `ensure_features(store, stamp)` comme producteur unique ; réécriture
**différentielle** (ne persister que ce qui change, comparaison bit à bit) ; gate
`(couverture, stamp)` où `stamp = {feature_epoch, dims, landmark_seed, semantic_order_hash}` ; le
tampon dans une **zone nommée** (le `MetaHeader` fait 96 o pleins et sa carte est figée) ; ordre
canonique par **clé sémantique**, pas par position de slot.

**Persistance** : `Value::Vector` est déjà routé vers la colonne dense `substrate.veccol.*` et
enrôlé dans `SlotRemappable` — ce chantier est **déjà livré**, ce n'est pas une proposition. Le
goulot est **l'écriture**, pas le calcul : 626 845 écritures × 438 ns pour le sidecar HKS, alors que
16 BFS coûtent ≈ 10 s. Corollaire : **ne créer aucune colonne nouvelle** (une colonne non enrôlée
rejouerait le risque de remap, D9).

### D9 — Ne PAS geler : livrer le test *golden* du remap et un consommateur **dans le dépôt**

La proposition « geler tant que le seul consommateur de classement est hors checkout » a été
**refusée par 4 agents sur 5**, pour des raisons convergentes :

* Le plan de position a un consommateur **structurel dans le dépôt** : les zones `Hilbert`/`Zorder`
  et le prefetch `madvise` le consomment déjà.
* Des consommateurs in-repo existent (`search_by_role` ; `memory_recall` via le serveur MCP).
* Geler **conserve un passif** : l'actuel est sous le hasard sur 3/5.
* La couche **coûte déjà** (réécriture pleine graphe, jamais relue jusqu'au bout).

**Risque critique associé, que personne n'avait soulevé** : `bulk_sort_by_hilbert` et
`compact_community` **permutent physiquement les slots**. Tout side-car indexé par slot non enrôlé
fait que **le vecteur de A devient celui de B** — sans erreur, sans CRC invalide. La télémétrie du
projet contient déjà un écart ×10 attribué à une confusion slot/id. **Aucune AUC ne mesure ce
risque.**

*Test golden, à écrire avant tout le reste* : écrire les features → `bulk_sort_by_hilbert` →
exiger `max|Δ| = 0` par **clé sémantique**. Il **échoue aujourd'hui** (la facette 4 utilise un max
global, l'initialisation de l'itération de puissance est indexée par **position**, et le
partitionnement n'a pas de graine).

---

## 3. Fermeture des voies : ce que chaque piste donne, mesuré

Le §5 laissait six voies ouvertes. Elles ont été parcourues jusqu'au bout par la mesure. Trois
sont **fermées** (dont deux contre l'attente), trois **ne sont pas fermées** et portent un gain
chiffré.

### 3.1 Voie « position » — FERMÉE, validée

Généralité 5/5 (T1), sans vérité terrain (T2), position authentique et non du rôle déguisé
(T5 : 0/16 coordonnées à |ρ| ≥ 0.7 avec le degré, AUC ≥ 0.84 dans tous les quantiles).
Gain sur l'existant : **+0.15 à +0.36 d'AUC**, et suppression d'un signal anti-informatif
(l'actuel est **sous le hasard** sur 3 topologies sur 5).

### 3.2 Voie « nombre de repères » — NON FERMÉE : il reste du gain, et il est bon marché

Balayage `k = 2 → 64` (une coordonnée par repère) :

| k | anneaux | **scale-free** | grille | ex æquo (scale-free) |
|---|---|---|---|---|
| 2 | 0.917 | 0.564 | 0.914 | 0.076 |
| 8 | 0.979 | 0.783 | 0.998 | 0.013 |
| **16** | **0.986** | **0.837** | **0.999** | 0.002 |
| 32 | 0.994 | 0.868 | 0.998 | 0.0003 |
| 64 | 0.992 | **0.897** | 0.999 | 0.0001 |

* Les structures régulières **saturent dès k = 4-16** (grille 0.994 dès k = 4).
* Les graphes à **queue lourde continuent de gagner** : 0.837 → 0.897 de k = 16 à 64, soit
  **+0.060**. Le taux d'ex æquo chute dans le même temps (0.002 → 0.0001), donc une partie du
  gain vient de la résolution des ex æquo.
* Coût : **un BFS par repère**, linéaire, négligeable devant les ~1200 produits matrice-vecteur du
  chemin spectral actuel.

⇒ **Le budget de repères ne doit pas être figé à 16 : il doit s'adapter au graphe** (≥ 32-64 sur
une topologie à queue lourde). C'est la voie la plus simple à exploiter, et elle n'apparaissait
pas dans le §4.

### 3.3 Voie « montée à l'échelle » (borne LMR) — partiellement fermée, non bloquante

16 repères, même générateur, négatifs échantillonnés :

| générateur | n = 200 | n = 1000 | n = 5000 | « degré seul » |
|---|---|---|---|---|
| communautés plantées | 0.995 | 0.997 | **0.999** | 0.499-0.500 |
| scale-free m = 2 | 0.837 | 0.822 | **0.797** | 0.199-0.211 |

La borne LMR **mord, mais lentement** : −0.040 d'AUC sur un facteur 25 de taille, uniquement sur
la topologie à queue lourde. Sur les communautés plantées, l'AUC **augmente** légèrement avec n.
Conclusion : ce n'est **pas** le risque dominant dans la plage testée — mais combiné à 6.2, la
prescription est cohérente : *plus le graphe est grand et de diamètre élevé, plus il faut de
repères*. Le témoin « degré seul » reste anti-informatif (0.20) à toutes les échelles, ce qui
confirme le diagnostic D1.

### 3.4 Voie « placement stratifié » — FERMÉE, sans gain (et une recommandation du panel réfutée)

| placement (scale-free 300) | AUC |
|---|---|
| FPS pur | 0.856 |
| FPS stratifié par communautés détectées | 0.856 |
| FPS **en excluant le top 1 % de degré** | **0.844** |

Exclure les hubs — recommandé par l'agent topologies — **dégrade** légèrement (−0.012). Réserve
d'honnêteté : ma propagation d'étiquettes a encore trouvé **une seule** communauté, donc le bras
« stratifié » est retombé sur FPS et **ne teste pas** un bon partitionnement ; la voie est close
faute de gain démontré, pas démontrée inutile.

### 3.5 Voie « couche de paires » — NON FERMÉE : elle paie là où le vecteur est faible

Scores de pair seuls et fusionnés par rangs avec la position (AUC, taux d'ex æquo entre parenthèses) :

| | anneaux | **scale-free** | grille |
|---|---|---|---|
| position 16d seule | 0.986 | **0.839** | 0.999 |
| voisins communs / Adamic-Adar / allocation | 0.482 (96 % ex æquo) | 0.530-0.532 (72 %) | 0.483 (97 %) |
| **fusion rangs position + paire** | 0.993 | **0.926** | 0.984 |

Lecture : pris seuls, les scores de pair paraissent inutiles — mais avec 72 à 97 % d'ex æquo, la
métrique **ne peut pas les voir** (le plafond tombe à 0.52-0.64). Ce qu'ils apportent est réel :
les paires qui partagent un voisin sont massivement des arêtes, et la fusion les remonte. Résultat :
**scale-free 0.839 → 0.926 (+0.087)**, la topologie la plus réaliste et la plus difficile, pour un
coût **local** `O(deg²)` — c'est-à-dire la seule voie qui **dépasse la borne du vecteur par nœud**
(D1).

### 3.6 Voie « rappel d'engrams » (3ᵉ besoin) — FERMÉE, et c'est le plus grand gain mesuré

Tâche : retrouver un engram **différent mais positionnellement proche** (ensembles de nœuds
**disjoints** : même communauté contre communauté différente, 300 paires) :

| encodage des indices | AUC | **recall@1** |
|---|---|---|
| **identité — hachage de `NodeId`, ce qu'obrain fait** | **0.442** | **0.020** |
| recouvrement Jaccard (voie directe) | 0.500 | 1.000 *(artefact d'ex æquo)* |
| **agrégat de position sur les membres** | **0.948** | **0.860** |

* **Obrain ne sait pas faire de généralisation positionnelle au rappel** : 2 % de recall@1, et une
  AUC de 0.442, **sous le hasard**. C'est structurel — l'indice est un hachage d'identité, donc
  deux engrams sans nœud commun sont orthogonaux par construction.
* L'agrégat de position sur les membres atteint **86 % de recall@1** : **+84 points**, le plus grand
  écart mesuré de toute cette étude.
* Le 1.000 du Jaccard est un **artefact de ma procédure** (tous les scores à zéro → tri stable →
  le positif placé en premier reste premier). Il est donc **non informatif** sur des ensembles
  disjoints, ce qui est précisément le cas visé.

### 3.7 Classement des voies par gain mesuré

| Rang | Voie | Gain mesuré | État |
|---|---|---|---|
| 1 | **Agrégat de position pour le rappel d'engrams** | recall@1 **2 % → 86 %** | non exploitée, coût quasi nul (les features existent déjà) |
| 2 | **Couche de paires fusionnée** | AUC **+0.087** sur scale-free | non exploitée, coût local `O(deg²)` |
| 3 | **Budget de repères adaptatif** | AUC **+0.060** (16 → 64) sur scale-free | non exploitée, coût linéaire (1 BFS/repère) |
| 4 | **Position à la place de la cascade** | AUC **+0.15 à +0.36**, 4× moins de mémoire | décidée (D2, D3) |
| 5 | Placement stratifié / exclusion des hubs | **aucun gain** (−0.012) | fermée |
| 6 | Compression PCA | aucun gain de qualité (4× de mémoire seule) | refusée en production (D4) |

### 3.8 Ce qui reste hors de portée

* **Le consommateur de classement réel est hors checkout** (`CompositeEmbedding` 532d, où
  `_hilbert_features` pèse 0.15) : l'effet produit des rangs 1-4 ne peut pas être vérifié ici.
* **Tout est mesuré sur des graphes synthétiques ≤ 5000 nœuds.** Le graphe de production fait
  363 346 nœuds avec 44 % de puits : la validation à cette échelle reste à faire, et la voie 6.3
  indique qu'il faut **plus** de repères, pas moins.
* **Le test *golden* du remap n'est pas écrit** — c'est une action, pas une voie de recherche.

---

## 4. Plan d'exécution (ordonné par rapport risque/gain)

Les trois premiers points sont les **voies non fermées du §3** : ils portent le gain le plus élevé
et n'ont pas été décidés plus haut parce qu'ils n'étaient pas connus au moment des décisions.

1. **Agrégat de position pour le rappel d'engrams** (§3.6) — gain mesuré **le plus élevé** de
   l'étude (recall@1 2 % → 86 %), coût quasi nul puisque les features existent déjà. C'est un
   changement de la fonction d'encodage des indices (agréger la vue position sur les membres de
   l'engram) et non de la couche de features. *Banc* : `crates/obrain-rag/tests/membench_recall.rs`,
   engrams à ensembles **disjoints** mais positionnellement proches, recall@10.
2. **Couche de paires fusionnée** (§3.5) — **+0.087 d'AUC** sur scale-free, la topologie la plus
   réaliste et la plus difficile. Coût local `O(deg²)`. C'est la seule voie qui dépasse la borne du
   vecteur par nœud (D1) ; elle a un précédent dans le dépôt (`search_by_role.rs:246-254`).
   *Banc* : AUC par quantile de degré **et** témoin « degré seul », avec publication du taux
   d'ex æquo — sans lui, les scores de pair paraissent inutiles alors qu'ils sont décisifs.
3. **Budget de repères adaptatif** (§3.2) — **+0.060 d'AUC** sur scale-free en passant de 16 à 64,
   pour un coût **linéaire** (un BFS par repère). Le budget doit dépendre de la topologie et du
   diamètre, pas être figé à 16.
4. **Test *golden* du remap** (D9) — il échoue aujourd'hui ; c'est le seul mode de défaillance
   *silencieux et corrupteur* identifié, et il est indépendant de toute refonte. **À faire avant
   les points 1-3**, parce qu'une permutation de slots invalide silencieusement tout vecteur de
   position — y compris ceux que ces voies exploitent.
5. **Supprimer le max global** de la facette 4 et distinguer `d = ∞` (D7) — prérequis à toute
   fraîcheur incrémentale correcte.
6. **`pos` = 16 coordonnées brutes de repères FPS** (D2, D3), en gardant le format d'écriture
   existant (veccol), avec un **témoin « degré seul »** dans le banc : sans ce témoin, aucune
   mesure n'est interprétable. Budget initial dérivé du point 3, pas figé à 16.
7. **Producteur unique + écriture différentielle + gate par `stamp`** (D8).
8. **Consommateur `search_by_pos`** dans le dépôt (procédure de lecture + banc), pour que la vue
   position ait un lecteur terminal observable.
9. **Rôle** : promouvoir le sidecar HKS existant en colonne nommée à clé `edge_hw` (D1) — pas de
   nouvelle colonne, pas de recalcul.
10. **Retirer les tests trompeurs** : `hilbert_features.rs:1713-1740` (test « locality »
    tautologique — il compare deux cliques d'étiquettes différentes alors que la facette 7 hache
    les étiquettes), `:1811-1930` (gate mesuré par `eprintln`, unilatéral, rien de persisté),
    `:1994-2035` (idempotence qui passe si la passe n'écrit rien), `:1646-1661` (ne vérifie que
    dimensions et plage) ; `geometric.rs:1592-1610` (courbure testée sur `K₃`, l'unique graphe où la
    formule fausse donne le bon résultat).

---

## 5. Ce qui reste ouvert et non tranché

* **LMR (D5)** — non mesuré. À dimension fixe, l'AUC locale *doit* décroître en `n` ; personne n'a
  vérifié le régime dans lequel obrain opère (10⁵-10⁶ nœuds).
* **Nombre et échelle des repères** — 16 a été choisi pour remplir 8 facettes. Un balayage de `k`
  (8/16/32/64) et un placement multi-échelle restent à faire, **après** LMR.
* **Le compromis du multi-échelle contre la borne LMR** : l'agent topologies recommande un
  placement stratifié multi-échelle, l'agent théorie exige de tester LMR avant — les deux sont
  compatibles mais l'ordre compte.
* **Fuite méthodologique résiduelle** : T5 montre une position authentique (0/16 à |ρ| ≥ 0.7), mais
  le maximum observé atteint 0.62 sur le graphe biparti — zone « non concluante » du protocole.
  À re-mesurer sur le graphe de production.
* **Le seul consommateur de classement réel est hors checkout** (`CompositeEmbedding` 532d, où
  `_hilbert_features` pèse 0.15 contre 0.55 pour l'embedding textuel) : les poids des blocs, et
  donc l'effet réel de D2 sur le produit, **n'ont pas pu être vérifiés ici**.
* **Le rappel d'engrams n'est pas touché par ce document** : il compare des **ensembles**
  (recouvrement d'identité) et demande un **agrégateur ensemble→vecteur**, pas une meilleure
  signature par nœud. Ses défauts propres sont ailleurs : signatures in-process, matrice de motifs
  reconstruite à chaque rappel, pondération des indices jetée.

## 6. Corrections assumées (traçabilité)

Trois affirmations de ce travail se sont révélées fausses et sont corrigées ici :

1. **« Garder un petit budget de rôle à côté des repères »** → **réfuté par la mesure** (D1 :
   0.831 → 0.239). Le rôle doit être dans un espace séparé, pas concaténé.
2. **`P@k` avec `k` = nombre d'arêtes** → **invalide** (`k > |V|`) ; c'est ce qui a produit une
   contradiction apparente entre deux de mes mesures (0.722 puis 0.000).
3. **`rank_normalize` déclaré non déterministe** (objection d'un agent contre un autre) → **tranché
   en faveur de « déterministe »** : la détection d'ex æquo attribue le **même rang moyen** à tout
   le groupe, donc l'ordre d'itération du `HashMap` ne change aucune valeur. Le non-déterminisme
   réel est en amont, dans la position au sein de `node_ids`.

Et deux corrections factuelles : « écrite puis écartée » est **inexact depuis T16.7** (colonne
`veccol`, enrôlée dans le remap) — le fait exact est « persistée, jamais hydratée par le runtime »,
et le défaut n'est pas l'absence de persistance mais un **invariant de lecture qui n'est qu'un
commentaire** ; enfin le second producteur est
`crates/neo4j2obrain/src/pipeline.rs:271`, pas un fichier d'`obrain-engine`.
