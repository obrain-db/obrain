# Un opérateur courbé, un spectre, des invariants prouvés

Statut : proposition structurelle (audit + refonte). Date : 2026-09-10.
Origine : lecture du papier OpenAI « Finite time blowup for Navier–Stokes »
(10/09/2026, https://cdn.openai.com/pdf/32d9f210-8b73-45e0-91bc-82a30aef8a9a/navier-stokes.pdf).
Méthode : trois audits indépendants en lecture seule de la couche cognitive (surface de
paramètres, composabilité des passes, provenance et auto-régulation), dont **toutes les
affirmations fortes ont été re-vérifiées dans le code** avant publication ici.

## 0. Ce qui se transpose vraiment (et ce qui ne se transpose pas)

**Ne se transpose pas.** Aucun théorème du papier n'est utilisable dans un moteur de
graphes : c'est de l'analyse EDP (auto-similarité anisotrope, intégration convexe,
amplification de couches à la Córdoba–Martínez-Zoroa). Le papier n'utilise **ni Ricci ni
transport optimal** : le rapprochement « Ricci flow ↔ Navier–Stokes » (image géométrique
d'Arnold) est un slogan, pas un outil. La courbure d'Ollivier-Ricci d'obrain n'est donc
pas concernée par la preuve elle-même.

**Se transpose — la structure de l'argument, en trois points :**

1. **Deux normes, deux rôles.** Le résultat est `sup_t ‖u‖_{L²} < ∞` *et*
   `limsup ‖u‖_{L∞} = ∞` : une garantie dans une norme n'est pas une garantie dans une
   autre. Toute revendication de stabilité doit nommer la norme où elle tient.
2. **Le signe du bilan gain/amortissement dépend de l'échelle.** Les impulsions croissent
   (elles pompent le cisaillement de fond) puis meurent (le raccourcissement de la longueur
   d'onde radiale fait grimper `k²`, donc la dissipation). Le régime n'existe que dans une
   *fenêtre* de rapports d'échelle (`0 < h < 1/100`). Un système sans axe d'échelle n'a pas
   de régime stable non trivial.
3. **Les corrections préservent l'invariant exactement.** Chaque passe de correction doit
   laisser `∇·u = 0` exact, sinon les erreurs s'accumulent au lieu de s'annuler.

Appliqués à obrain, ces trois points ont produit, dans l'ordre d'importance :

* **D9-D10** — l'étage cognitif n'est **pas câblé** (fleet non démarré, bus sans source,
  décroissances mortes) et, là où il tourne, les passes communiquent par **état partagé non
  déclaré** plutôt que par des contrats. C'est la découverte qui recadre tout le reste :
  la plupart des autres diagnostics sont **latents**, pas actifs.
* **D6** — *pourquoi* les clamps existent : avec un gain et une dissipation scalaires, tous
  les modes ont le même taux, donc il n'existe aucun point fixe non trivial. Les quatre
  clamps sont un **symptôme du même opérateur manquant**, pas des choix indépendants.
* **D2** — le seul régulateur global est une similitude : il ne peut, par construction,
  modifier aucune statistique invariante d'échelle. Et sa stabilité est déléguée au clamp,
  faute de garde `cT < 2`.
* **D11** — **la courbure persistée n'est pas une courbure.** Vérifié par un calcul exact et
  indépendant : la formule codée n'est ni l'Ollivier-Ricci ni la borne de Jost–Liu citée, et
  le seul test du dépôt porte sur l'unique graphe où la formule fausse donne le bon résultat.
  Rien ne la consomme par ailleurs.
* **D4** — l'énergie, primitive phare du moteur, est un **booléen** en backend de
  production, et sa décroissance n'a aucun appelant.
* **D8** — la seule violation d'invariant **non bornée et vivante** : la borne de budget
  synaptique, violée d'un facteur `2^(Δt/half_life)`.

Le point 3 du papier donne la refonte des passes (P3), le point 1 le « deux normes »
incarné par le couple masse/concentration (D8) et la doctrine de normalisation (P2).

---

# Partie I — Diagnostic (preuves vérifiées)

## D1. Quatorze lois de dissipation, quatorze constantes de temps, zéro invariant partagé

Chaque sous-système a sa propre loi, sa propre constante de temps et sa propre borne.
Aucune ne partage d'invariant avec une autre, et aucune n'est dérivée d'une autre. Le
tableau en liste douze ; deux de plus (co-change et utilité) vivent juste en dessous.

| # | Sous-système | Loi | Constante | Borne | Réf. |
|---|---|---|---|---|---|
| 1 | Énergie nœud | `E(t) = E₀·2^(−Δt/half_life)` | 24 h | clamp `[0,10]` puis Q1.15 → `[0,1]` | `energy.rs:83`, `:54`, `:390` |
| 2 | Énergie nœud (substrate) | boost additif puis clamp | `boost_on_mutation = 1.0` | `(cur+amt).clamp(0,max)` | `store.rs:9383` |
| 3 | Décroissance énergie | facteur multiplicatif par balayage | **aucun appelant de production** | — | `energy.rs:447` (cf. D4) |
| 4 | Engram (FSRS-5) | oubli en loi de puissance | **19 poids** `w[0..19]` | `[0,1]` | `engram/decay.rs:100` |
| 4b | Engram — loi **appliquée** à la force | `s ← s × 0.9` (plat) | `0.9`, purge si `s < 0.05` | — | `engram/manager.rs:331` |
| 5 | Homéostasie engram | `s ← c·s`, `c = 1 − rate·(moy − cible)` | `rate = 0.1`, cible `0.5` | clamp `[0,1]` | `engram/homeostasis.rs:113` |
| 6 | Stigmergie (phéromones) | évaporation multiplicative | `0.95` / balayage | seuil `1e-6` | `stigmergy.rs:272`, `homeostasis.rs:47` |
| 7 | Synapse (poids) | `w *= factor` ; LTP additif | `factor ∈ [0,1]` | `min(max_w)` clamp | `synapse.rs:167`, `:790` |
| 8 | Activation (spreading) | `propagé = parent·w·decay_factor` | `0.3` / saut | seuil + `max_activated_nodes` | `activation.rs`, `kernel_params.rs:11` |
| 9 | Diffusion de chaleur | combinaison convexe | `α = 0.4`, 4 itérations | aucune (principe du max) | `heat_diffusion.rs:22` |
| 10 | Fabric (risque) | somme pondérée | poids `RiskWeights` | normalisé par **max global** | `fabric.rs:526`, `:380` |
| 11 | Empreinte « spectrale » | projection aléatoire signée | 64 dims | L2 | `engram/spectral.rs:56` |

Deux lois de plus vivent hors de la couche cognitive avec la **même forme
exponentielle et une autre constante** : co-change (30 j, `co_change.rs:48`) et
utilité (30 j, `utility.rs:45`).

**Divergences de constantes pour un même concept** — preuve qu'aucune doctrine n'existe :
`propagation_decay = 0.3` (`kernel_params.rs:158`) contre `decay_factor = 0.5`
(`activation.rs:62`), deux défauts différents pour la même atténuation par saut. Et trois
paramètres du Consolidator (`decay_factor = 0.99`, `synapse_prune_threshold_u16`,
`engram_reinforce_delta_u16`, `thinker/consolidator.rs:53-55`) sont déclarés,
désérialisés et résolus (`thinker/config.rs:396-412`) mais **jamais consommés**.

**Le cas le plus révélateur** : la loi n° 4 (FSRS-5, 19 paramètres, oubli en loi de
puissance `R = (1 + Δt/9S)^{−1}`, `engram/decay.rs:206`) calcule une rétention qui pilote
l'**échéancier** (`fsrs_state.next_review`) — mais la force stockée, elle, décroît par un
facteur **plat** : `e.strength = (e.strength * 0.9).max(0.0)`
(`engram/manager.rs:331`). Le modèle à 19 paramètres n'est donc **pas** la loi de
décroissance de la force : c'est la loi n° 4b, un `×0.9` au tick.

**Conséquence structurelle :** l'« échelle de temps » d'obrain n'est pas une propriété du
système, c'est un couple `(cadence, facteur)`. La loi n° 3 prend un *facteur*, pas une
durée : le `half_life` de 24 h n'est donc pas honoré dans le chemin substrate, et
`effective_half_life` (`energy.rs:670`) — le seul crochet existant qui rendrait la
dissipation **dépendante de la structure** (degré du nœud) — n'a **aucun appelant de
production** (grep exhaustif : `energy.rs` + tests uniquement), et son coefficient
`structural_reinforcement_alpha` vaut `0.0` par défaut (`energy.rs:58`).

## D2. Le seul régulateur global est une similitude — donc aveugle par construction

`engram/homeostasis.rs:99-127` : le facteur `c` est **le même pour tous les engrams**.

```
c = 1 − rate·(moyenne − cible)   si moyenne > cible
c = 1 + rate·(cible − moyenne)   si moyenne < cible
s_i ← clamp(c · s_i, 0, 1)
```

Les deux branches sont **algébriquement identiques** : `1 − c(m−T) = 1 + c(T−m)`. Il n'y a
pas deux régimes, seulement `f = 1 + c(T − m)` — un contrôleur proportionnel sur un unique
scalaire.

**Théorème d'aveuglement.** Tant que le clamp est inactif, `s ↦ c·s` est une similitude
positive. Toute fonctionnelle **invariante d'échelle** est donc exactement préservée :
`max/moyenne`, coefficient de variation, Gini, entropie de la distribution normalisée,
kurtosis, et l'ordre des rangs. Preuve : ces fonctionnelles sont homogènes de degré 0.

**Le théorème n'est pas propre à l'homéostasie — il s'applique à la moitié de la boucle.**
L'évaporation des phéromones (`stigmergy.rs:248-262`, `:272-274`) et la décroissance
d'énergie à demi-vies uniformes (`α = 0` par défaut, `energy.rs:58`) sont **elles aussi**
des multiplications scalaires uniformes. Elles préservent donc exactement la distribution
normalisée `p_i = v_i/Σv_j`, donc **l'entropie de Shannon et le ratio `max/moyenne`** —
c'est-à-dire précisément les deux grandeurs dont `observe.rs` se sert comme indicateurs de
lock-in (`observe.rs:136-141`). **Un mécanisme ne peut pas réduire une quantité qu'il
préserve exactement par construction.** Ce n'est écrit nulle part dans le dépôt, et aucun
test ne l'énonce.

**La stabilité du régulateur n'est pas conçue : elle est déléguée au clamp.** La dynamique
de la moyenne se dérive exactement de `homeostasis.rs:113-124` :

```
m' = m·(1 + c(T − m)) = m + c·m·(T − m)
```

C'est la **discrétisation d'Euler explicite d'une logistique** `ṁ = c·m·(T−m)`. Points
fixes : `m = 0` (instable) et `m = T` (stable). Condition de stabilité *discrète* :
`|1 − cT| < 1`, soit **`0 < cT < 2`**. Aux défauts (`c = 0.1`, `T = 0.5`) la jacobienne vaut
`0.95` : convergence géométrique, erreur divisée par deux tous les ~13,5 balayages (≈ 13,5 h
à la cadence par défaut) — donc très lente, mais stable. **Aucune garde `cT < 2` n'existe**,
et le test à `c = 100, T = 0.99` (`homeostasis.rs:286-290`) a une jacobienne de `−98` : il ne
passe que **parce que le clamp rattrape la divergence**. La stabilité repose sur une
saturation numérique, pas sur une conception — et c'est exactement la situation décrite en
D6.

**Corollaire.** Le régulateur ne peut pas agir sur la concentration — la seule chose qui
compte pour une mémoire qui doit « oublier ce qui n'importe pas ». Exemple : un engram à
`1.0` et dix mille à `0.0` donnent `moyenne ≈ 0` ; le contrôleur *augmente* alors tout,
pousse celui du haut sur le clamp `1.0`, et laisse les autres où ils sont : le rapport
`max/moyenne` n'a pas bougé de `1 ε`. Quand le clamp est actif, le régulateur change bien
la forme — mais uniquement en écrasant la queue sur la borne, donc en **détruisant l'ordre**.

Le corollaire est plus violent que « aveugle » : dans le régime de forte concentration, le
contrôleur **protège l'extrémum**. Avec 1 engram à `1.0` et 999 à `0.0`, la moyenne vaut
`0.001 < 0.5`, donc le facteur vaut `1 + 0.1×(0.5 − 0.001) = 1.0499 > 1` : le contrôleur
*pousse vers le haut* l'engram déjà maximal, et seule la borne externe l'arrête. Un
régulateur qui renforce ce qu'il devrait contenir n'est pas un régulateur.

Deux défauts secondaires, tous deux vérifiables : le garde
`|moyenne − cible| < f64::EPSILON` (≈ 2.2e−16) ne se déclenche jamais ; et `scaling_rate`
n'a **aucune borne de type** (`homeostasis.rs:53` ; le test interne l'utilise à `100.0`).
Pour une moyenne très au-dessus de la cible le facteur passe alors **négatif**
(`1 − 100×0.5 = −49`) : toutes les forces deviennent négatives puis sont ramenées à `0.0`
par le clamp — non pas une divergence, mais un **effondrement global** du store, sans
garde-fou.

## D3. Chaque sous-système a choisi une norme différente, chacune avec une pathologie prouvée

| Choix | Où | Pathologie |
|---|---|---|
| moyenne (`L¹`) | homéostasie engram | aveugle à la forme (D2) |
| **max** (`L∞`) | fabric, `normalize(x,max)` `fabric.rs:516` | l'outlier **fixe l'échelle** : un nœud dont le pagerank explose écrase tous les autres vers 0 |
| clamp dur | énergie, synapse, engram | saturation : la queue perd son ordre (nœuds distincts → même score) |
| `L²` | empreinte « spectrale » | `engram/spectral.rs` n'a **aucun spectre** (projection aléatoire signée) |
| `1 − exp(−x/r)` | `energy_score` `energy.rs:647` | sature ; et le doc-comment promet un « **unbounded energy domain** » `energy.rs:644` que la colonne Q1.15 contredit |

Le point remarquable n'est pas qu'un choix soit mauvais : c'est qu'**obrain n'a pas de
doctrine de normalisation**. Les deux extrêmes du papier — la moyenne bornée et le sup qui
diverge — coexistent ici dans deux modules voisins, chacun avec sa pathologie, sans
arbitrage explicite.

**Et la référence de normalisation n'est même pas stable.** Deux défauts liés, tous deux
vérifiés :

* **Références hétérogènes entre nœuds.** `recalculate_all_risks` (`fabric.rs:353-367`)
  normalise toute la population par les maxima d'un instant `T`, tandis que
  `FabricListener::on_batch` (`:620-638`) normalise **seulement les nœuds touchés** par les
  maxima d'un instant `T'`. Deux nœuds dont `_cog_risk_score` a été écrit à des instants
  différents sont donc normalisés **contre des maxima différents** : les scores persistés ne
  sont pas mutuellement comparables. Scénario : un lot mute `A` (`max_mf = 5`) → `A` vaut
  `1.0` ; un second lot mute `Z` 20 fois (`max_mf = 20`) sans toucher `A` → `A` reste écrit
  contre `5`, `Z` contre `20`. Un `recalculate_all_risks()` ultérieur réécrit `A` deux fois
  plus bas **sans qu'aucune donnée de `A` ait changé**. C'est le symptôme exact de la
  normalisation par le max : le score d'un nœud dépend de ce que font les autres.
* **État de normalisation non persisté.** `recalculate_risk` ne persiste pas
  (`fabric.rs:332-350`) : dans `on_batch` l'ordre est `record_mutation_at` (qui persiste le
  score **d'avant** recalcul, `:627`) puis `recalculate_risk` (`:636`) ⇒ le
  `_cog_risk_score` sur disque est **systématiquement en retard d'un recalcul**. Et
  `last_mutated`/`staleness` ne sont pas persistés (`:153-171`), alors que le `FabricStore`
  est reconstruit vide à l'ouverture (`:135-141`) : **après chaque redémarrage, tous les
  nœuds paraissent fraîchement mutés** et `mutation_frequency` repart à `0`.

## D4. En backend de production, l'énergie est un booléen

Chaîne d'évidence, chaque maillon vérifié :

1. `EnergyConfig::default` : `boost_on_mutation = 1.0`, `default_energy = 1.0`,
   `max_energy = 10.0` (`energy.rs:52-58`).
2. Chemin substrate (`energy.rs:390`) : `max_q = max_energy.min(1.0)` → **`1.0`**.
   Le `10.0` du défaut est inatteignable.
3. `boost_node_energy_f32(id, amount, max)` (`store.rs:9383`) :
   `new = (cur + amount).clamp(0.0, max)` avec `amount = 1.0` → pour tout `cur ≥ 0`,
   `new = 1.0`.
4. Encodage `f32_to_q1_15` (`record.rs:519`) : `v.clamp(0,1) × 32768` → la colonne ne
   peut pas dépasser `1.0`.
5. `get_energy` en mode substrate **retourne la valeur brute** (pas de décroissance à la
   lecture, `energy.rs:321-325`) — et `EnergyStore::decay_all` **n'a aucun appelant de
   production** : les seuls sites `.decay_all(` sont `energy.rs:818`, `synapse.rs:1943`,
   `synapse.rs:2240` (tests) et `triggers.rs:561` (méthode *du TriggerEngine*, dans un
   test). `SubstrateStore::decay_all_energy` n'est appelé que depuis `energy.rs:450`.

**Cause racine identifiée — et c'est une croyance fausse partagée entre deux modules.** Le
Consolidator, seul Thinker censé appliquer la décroissance périodique, est un stub assumé
(`thinker/consolidator.rs:105-119`) : ses trois paramètres de décroissance ne sont jamais
consommés, et son commentaire justifie la no-op par « the decay path is exercised through
the cognitive EnergyStore bridge **that already runs on a separate timer** ». Ce timer
n'existe pas : `EnergyStore::decay_all` n'a aucun appelant de production (grep exhaustif sur
`crates/`). **Chaque module croit que l'autre applique la décroissance, donc personne ne
l'applique** — le motif d'invariant « appliqué nulle part » ; voir D7.

⇒ **Un nœud touché vaut `1.0`, définitivement.** Toute gradation est impossible : deux
nœuds boostés une fois et cent fois sont indiscernables. Puis
`energy_score(1.0, ref_energy = 1.0) = 1 − e^{−1} ≈ 0.6321` : **tous** les nœuds touchés
obtiennent le même score, pour toujours. Le module `search.rs:9` documente un classement
« Energy-weighted — recency via exponential decay » : la récence n'est pas délivrée dans le
chemin substrate.

Deux conséquences en cascade :

* Le `max_energy = 10.0` du défaut n'est vivant que dans le chemin *legacy* (sans
  substrate), où le plafond vaut `0.99995` au lieu de `0.6321` de score : **le même nœud
  logique a un plafond de score différent selon le backend.**
* L'encodage a un bit de marge perdu : `q1_15_to_f32(x) = x/32768` décode `[0,2)`, mais
  `f32_to_q1_15` clampe à `1.0` avant de multiplier — la moitié haute du code est
  inatteignable. Le garde `.min(65535.0)` montre que l'auteur a envisagé des valeurs > 1.

## D5. Le spectre et la courbure sont calculés, puis débranchés de la dynamique

obrain calcule deux objets géométriques coûteux. Aucun des deux ne gouverne le transport.

| Objet | Où | Ce qu'il calcule | Usage réel |
|---|---|---|---|
| Embedding spectral | `algorithms/spectral.rs:58` | vecteurs propres du laplacien **normalisé symétrique** `M = D^{-1/2}WD^{-1/2}`, itération de puissance + déflation | **features statiques** : facettes 0-1 de la cascade Hilbert (`hilbert_features.rs:13`) |
| Courbure Ollivier-Ricci | `substrate/geometric.rs` | `κ(u,v) = 1 − W₁(μ_u,μ_v)/d(u,v)` (borne Jost–Liu), persistée `ricci_u8` (stride 1/127.5) | **un seuil binaire** : coupe du BFS si `κ < threshold` (`context_loader.rs:266`) + drapeau « bottleneck » (`dreamer.rs:69`) |

Grep croisé : **aucune occurrence** de `curvature`/`ricci` dans `synapse.rs`,
`activation.rs`, `energy.rs`, `fabric.rs`, `engram/homeostasis.rs`, `engram/decay.rs`.
La courbure ne module ni une conductance, ni une décroissance, ni un poids de propagation.

C'est la perte structurelle centrale : les deux objets qui *devraient* gouverner la
dynamique (le spectre pour les échelles, la courbure pour la géométrie) sont calculés et
consommés ailleurs, tandis que la dynamique est pilotée par des scalaires uniformes.

Le motif n'est pas isolé — c'est un **patron récurrent** dans ce dépôt : un objet coûteux est
calculé puis consommé ailleurs que là où il compte, ou pas du tout.

| Objet calculé | Consommé où | Ce qui manque |
|---|---|---|
| Courbure `ricci_u8` (W₁, Jost–Liu) | seuil BFS + drapeau dreamer | aucune conductance, aucun poids de transport |
| Vecteurs propres du laplacien normalisé | facettes statiques 0-1 de Hilbert | aucun taux de décroissance par mode |
| FSRS-5 (19 paramètres, loi de puissance) | échéancier `next_review` | la **force** décroît par `×0.9` plat (`manager.rs:331`) |
| `effective_half_life` (modulation par degré) | exporté `lib.rs:190`, testé en proptest | **zéro appelant de production** ; `α = 0.0` par défaut |
| `decay_factor` du Consolidator | désérialisé, résolu | tick stub : jamais appliqué |
| Cascade `_hilbert_features` (64d) | calculée par les enrichers, persistée | **explicitement écartée au chargement** : `SKIP_ON_LOAD_PROP_KEYS = ["_st_embedding", "_kernel_embedding", "_hilbert_features"]` (`store.rs:196-200`), « never read by the hub runtime » |
| Colonne `substrate.hilbert` | écrite (`writer.rs:1680-1700`) | **jamais lue** : seul un `madvise` de plage (`store.rs:8933`), les valeurs sont sans effet |
| Permutation physique des slots | trie par `(centrality_cached, degré sortant)` (`hilbert.rs:329-346`) | ignore `_hilbert_features` **et** la colonne `hilbert` : le plan physique et le plan logique ne sont jamais réconciliés |
| `weighted_hilbert_distance` (la métrique de la cascade) | `:1537` | aucun appelant de production (seul `heat_diffusion`, lui-même sans appelant — D9) |

**Symptôme du même mal dans les constantes** : les poids FSRS `w[14]` et `w[15]` sont
documentés « short-term stability parameters » (`decay.rs:34`) mais utilisés comme
**facteurs de note** (`decay.rs:172-177`). Avec les défauts, cela donne
`grade_factor(Hard) = 2.27 > grade_factor(Good) = 1.0 > grade_factor(Easy) = 0.046`
(`decay.rs:56-59`) : « Hard » augmente donc **plus** la stabilité que « Good », et « Easy »
**moins** — l'inverse de la sémantique FSRS-5 citée en référence par le fichier lui-même
(`decay.rs:7-10`, où `hard_penalty < 1` et `easy_bonus > 1`). Une constante non dérivée d'un
principe est une constante qu'on ne peut pas vérifier.

## D6. Pourquoi les clamps existent : dichotomie du gain scalaire

Soit un champ `f` sur les nœuds, décomposé sur la base propre d'un opérateur de transport
`L` (laplacien du graphe) : `f = Σ_k ĝ_k φ_k`. Supposons l'amplification `γ` et la
dissipation `λ` **scalaires** (indépendantes du mode) — c'est le cas d'obrain : décroissance
multiplicative uniforme, LTP additif uniforme, gain d'activation uniforme par saut.

Alors `ĝ_k(t) = e^{(γ−λ)t} ĝ_k(0)` : **tous les modes ont le même taux**.

**Théorème (dichotomie).** Avec `γ`, `λ` indépendants du mode, la dynamique linéarisée n'a
que deux régimes : `γ > λ` → **tous** les modes croissent exponentiellement (⇒ un clamp
devient nécessaire) ; `γ < λ` → **tous** les modes s'éteignent (⇒ perte d'information).
Aucun régime intermédiaire ne peut préserver la structure pendant que le bruit meurt, car
il faudrait `γ_k > λ_k` pour certains `k` et `γ_k < λ_k` pour d'autres.

C'est la lecture exacte de la Figure 3(b) du papier : l'impulsion croît puis décroît
**parce que** le taux dépend de l'échelle. Transposé : les clamps durs (`clamp(0,10)`,
`clamp(0,1)`, `min(max_w)`, `tanh`) et le contrôleur de moyenne ne sont pas des choix de
conception indépendants — ce sont **quatre symptômes du même opérateur manquant**. Tant que
`λ` est scalaire, il n'existe pas de point fixe non trivial vers lequel réguler : il faut
une borne externe. On ne « corrige » donc pas la saturation en réglant les constantes.

## D7. L'invariant est appliqué au mauvais étage — donc contournable

Toutes les bornes d'obrain vivent dans la **couche Store**, jamais dans le **type de
donnée**. Or les primitives publiques écrivent directement le type :

* `NodeEnergy::boost_at` (`energy.rs:147`), `Synapse::reinforce_at` (`synapse.rs:143`),
  `Synapse::new` (`synapse.rs:91`), `Synapse::scale_weight` (`synapse.rs:166`) : aucune
  borne. Le clamp est appliqué par l'appelant (`energy.rs:413`, `synapse.rs:786`).
* **Réhydratation non clampée** : `get_synapse` (`synapse.rs:1123`) et `load_from_graph`
  (`synapse.rs:1401`) restaurent `weight: raw_weight` **sans** `clamp_weight`. Une valeur
  persistée hors borne (ancien `max_synapse_weight` plus permissif, propriété LPG
  `PROP_SYNAPSE_WEIGHT`) est restaurée telle quelle et survit jusqu'au prochain `reinforce`.
  La valeur en substrat reste bornée par Q0.16 (`synapse.rs:1380`) : **les deux backends ne
  garantissent donc pas le même invariant.**

C'est exactement le point 3 du papier, transposé : l'invariant (`∇·u = 0` là-bas) est
préservé *exactement à chaque étape*, ou il ne l'est pas du tout. Un invariant garanti par
la discipline des appelants n'est pas un invariant. La correction structurelle est de rendre
la borne **inconstructible à violer** : saturation dans le constructeur et dans le
constructeur de désérialisation, pas dans l'appelant.

## D8. Les vraies divergences non bornées — et l'instance exacte du « deux normes »

L'audit exhaustif de la couche cognitive donne une liste courte mais réelle de grandeurs
**sans aucune borne** :

| Grandeur | Preuve | Régime |
|---|---|---|
| Énergie cross-base | `boost_cross_base` : `*e += amount`, aucun clamp, **aucune décroissance nulle part** (`energy.rs:622-627`) | divergence illimitée |
| Phéromone stigmergique | `fetch_add(delta)` sans cap (`stigmergy.rs:196-210`) ; décroissance seulement multiplicative (`:248`) et le bruit anti-lock-in est **proportionnel** (`:312`) | diverge si le dépôt dépasse ~5 % par cycle |
| Précision β d'engram (Hopfield) | `precision: f64` sans clamp (`engram/types.rs:57`), consommée en `logits.push(betas[i] * dot)` puis softmax (`hopfield.rs:170`) | β → ∞ : winner-take-all |
| Magnitude d'activation par nœud | `*activation.entry(neighbor).or_insert(0.0) += propagated` (`activation.rs:417`) | bornée seulement *indirectement* par `max_hops`, `weight`, `decay_factor` ; le disjoncteur `max_activated_nodes` borne une **cardinalité**, pas une magnitude |

**Le cas de β est l'instance exacte du motif du papier, présente dans obrain aujourd'hui** :
l'agrégat est borné (le softmax somme à 1 — `hopfield.rs:175-181`) tandis que le
**coefficient de concentration** est non borné. Une masse de probabilité bornée peut donc se
concentrer entièrement sur un seul engram sans qu'aucune borne d'agrégat ne s'y oppose.
Aucun invariant de ce dépôt n'exprime cette distinction (masse vs concentration), ce qui est
précisément le point 1 : *une garantie dans une norme n'est pas une garantie dans une autre*.

Deux précisions d'honnêteté sur ce point :

* β est **par motif** (`hopfield.rs:54`, `:85-93`, utilisé `:170`), donc les logits
  `β_i⟨p_i,q⟩` ne sont pas le gradient d'une forme quadratique en `q` (sauf si tous les `β_i`
  sont égaux) : le fonctionnel d'énergie du Modern Hopfield à β unique **ne s'applique pas**
  à la variante implémentée. Borner β est donc une condition nécessaire à toute
  réclamation d'énergie, pas seulement une question de stabilité numérique.
* La revendication de capacité `2^{d/2}` (`hopfield.rs:12`) appartient au théorème de la
  règle de mise à jour complète `ξ ← P·softmax(βPᵀξ)` — qui **n'est jamais effectuée** :
  `hopfield.rs:183-199` trie et tronque au top-k en renvoyant les engrammes stockés. Sans le
  pas d'état `·P`, il n'y a ni dynamique, ni point fixe, ni fonctionnel d'énergie. C'est un
  **k-NN à pondération softmax**, pas un réseau attracteur.

**Cliquets et biais directionnel** — trois défauts de la même famille, chacun vérifié :

| Défaut | Preuve | Conséquence |
|---|---|---|
| Bruit anti-lock-in **unilatéralement négatif** | `((counter >> 33) as f64 / u32::MAX as f64) * 2.0 - 1.0` (`stigmergy.rs:311`) : `counter >> 33` tient sur 31 bits, donc le rapport est dans `[0, 0.5)` et le « bruit » dans `[−1, 0)` — le commentaire `:307` annonce `[-1.0, 1.0]` | le mécanisme ne peut que **réduire** les phéromones dominantes et n'affecte que la bande `v ≥ 0.9·max` : il ne peut jamais **promouvoir** un chemin non dominant. La revendication « encourages exploration » (`:291`) est insoutenable. Le test n'assert que `|Δ| ≤ 5 %` (`homeostasis.rs:493-497`), vrai aussi pour une perturbation unilatérale — il ne peut pas détecter le défaut |
| Porte de surprise **irréversible** | `surprise_accumulator += prediction_error` sans décroissance (`formation.rs:336`) et `reset_surprise()` appelé **uniquement par son propre test** (`formation.rs:548`) | dès la première activation avec `PE ≥ 0.3`, la porte de formation reste ouverte **définitivement** : un cliquet, l'inverse d'une homéostasie |
| Backend de phéromones **monotone croissant** | `flush()` écrit `backend ← backend + delta` (`stigmergy.rs:406-416`) ; `evaporate`/`inject_noise` n'opèrent que sur la carte en mémoire (`:248-317`), et `flush()` n'a **aucun appelant de production** | la persistance ne subit jamais ni évaporation ni bruit : le lock-in y croît structurellement, sans oubli possible |

**Un cas plus grave que l'absence de borne : une borne qui existe et qui est violée sans
limite.** `normalize_outgoing` (`synapse.rs:1019-1070`) est la normalisation compétitive qui
doit garantir `Σ poids sortants ≤ max_total_outgoing_weight` (défaut `100.0`,
`synapse.rs:60`). Elle calcule sa somme sur les poids **décrus** (`current_weight()`,
`:1046`) puis applique le facteur aux poids **bruts** (`scale_weight` → `self.weight *= f`,
`:1062`, `:168-170`). Or `current_weight() = raw · 2^(−Δt/half_life)` : la borne n'est donc
respectée qu'à `Δt = 0`, et l'écart croît comme `2^(Δt/half_life)` — **sans borne dans le
temps**. Contre-exemple : deux synapses `raw = 10`, `half_life = 1 s`, renforcées il y a
10 s → `Σ current ≈ 0.0195` ; avec `max_total = 0.01`, le facteur vaut `0.512` et les poids
bruts finissent à `5.12` chacun, soit `Σ raw = 10.24`, **1024× la borne annoncée**. C'est un
invariant documenté (`docs/cognitive/synapses.md:30`), violé en code exécutable — et non
latent, puisque `reinforce` l'appelle à chaque renforcement (`synapse.rs:801-802`).

À noter, pour ne pas surinterpréter : l'énergie de nœud, le poids synaptique et la force
d'engram **sont** bornés (clamps durs + colonnes à virgule fixe), et le poids synaptique a
une plage exploitable correcte (`raw/max_w` avec `max_w = 10`, `reinforce_amount = 0.2` :
~50 renforts avant saturation). La dégénérescence de D4 est propre à l'énergie, dont la
colonne stocke la valeur **brute** (`f32_to_q1_15`) au lieu d'être renormalisée — c'est un
écart d'implémentation, pas une doctrine.

## D9. L'étage cognitif est inerte : la boucle n'est pas câblée

Tout le diagnostic précédent supposait que les mécanismes s'exécutent. L'audit des
consommateurs montre qu'il n'en est rien, et à une échelle plus large qu'attendu. Trois
constats dominent :

1. **Le fleet de Thinkers n'est jamais démarré.** `thinker/mod.rs:75-78` affirme que
   `spawn_standard_fleet` est « the single entry point used by `SubstrateStore::open()` ».
   Aucun appel dans `obrain-substrate`, `obrain-engine` ni `obrain-cli` : le seul est
   `examples/rust/poc_cognitive_store.rs:111`, plus un test. **Aucun `ThinkerRuntime` ne
   tourne.**
2. **Le bus d'événements n'a aucune source.** Les 5 listeners cognitifs sont bien
   enregistrés (`engine.rs:541-670`) mais `MutationBus::publish` n'est appelé que depuis des
   bancs ; `set_post_commit_hook` (`session/mod.rs:467`) n'a **aucun appelant**. Les
   événements ne sont jamais publiés : la couche réactive entière (énergie, synapse, fabric,
   co-change, kernel) est **en attente d'un signal qui n'arrive pas**.
3. **`EngramManager::tick` n'est pas sur un timer.** Il n'est appelé que depuis le chemin
   d'écriture MCP (`mcp_server.rs:2593`, `:4795`), contrairement à ce qu'annonce
   `engram/manager.rs:297` (« Should be called on a timer ») ; `MemoryManager::start_periodic`
   (`memory.rs:561-580`), qui porterait le balayage horaire, n'a aucun appelant.

Et, dans ce qui reste atteignable, la moitié des éléments n'a pas de consommateur :

| Élément | Statut réel |
|---|---|
| `sweep_with_stigmergy` (évaporation + anti-lock-in) | appelé **uniquement par deux tests** de son propre fichier (`homeostasis.rs:435`, `:466`) |
| Feature `stigmergy` | déclarée (`obrain-cognitive/Cargo.toml:25`) mais **absente de tout groupe de features** ; `obrain-rag`, seul crate qui active les couches cognitives, active `engram, hopfield, energy, synapse` **sans** `stigmergy` |
| `meta_plasticity_factor()` (l'amplification ×2 « BCM ») | **jamais consommé** : aucun appelant hors tests |
| `HomeostasisSignal` (capteur `immune_fp_rate > 20 %` → `regulate_immune`) | **aucun consommateur**, seulement des ré-exports (`lib.rs:334`, `engram/mod.rs:82`) |
| `update_stigmergy_metrics` (capteurs entropie / ratio) | alimenté seulement par un test (`procedures.rs:551`) |
| `StigmergicEngine::flush()` (persistance des phéromones) | **aucun appelant de production** |
| `reconsolidate` | `#[allow(dead_code)]` (`recall.rs:394`), appelé par ses seuls tests |
| `mark_used` / `mark_rejected` (les seuls écrivains de `strength` hors decay) | **aucun appelant de production** (`manager.rs:268`, `:285`) |
| `mark_incident_edges_stale` / `refresh_stale_edges` | aucun appelant ⇒ `RICCI_STALE` n'est **jamais posé** ; seul `enrich --ricci` tourne, en recalcul complet |
| `decay_all_energy` / `decay_all_synapse` / `decay_all_coact` | aucun appelant de production |
| `ConsolidationEngine::consolidate`, `cristallization`, `distillation`, `kernel_bootstrap`, `kernel_adaptation`, `GdsRefreshScheduler` | aucun appelant |
| Tick du Consolidator | stub (`consolidator.rs:105-119`) — cf. D4 |

**Deux effondrements en cascade, qu'il faut nommer parce qu'ils changent la lecture des
autres diagnostics :**

* **La décroissance des engrams n'existe pas** (`is_due ≡ false`). `manager.rs:319-322`
  évalue `engram.fsrs_state.next_review`, mais `next_review` et `last_review` ne sont
  **jamais écrits** : `FsrsScheduler::review` les recopie tels quels (`decay.rs:150-151`,
  `:201-202`) et ils valent `None` par défaut (`types.rs:220-221`). Donc `is_due` est
  toujours faux, et ni le `×0.9` (`manager.rs:334`), ni la suppression à `< 0.05`
  (`:341`), ni `record_decay()` n'atteignent jamais l'exécution. **C'est une seconde
  décroissance morte, indépendante de celle de D4** — et la réponse à la question « la
  décroissance annule-t-elle la correction d'homéostasie ? » est : non, parce qu'elle n'a
  pas lieu.
* **Le régulateur de D2 est donc, en l'état, un no-op exact.** `Engram::new` fixe
  `strength = 0.5` (`engram/types.rs:102`), la cible vaut `0.5` (`homeostasis.rs:52`), et
  les seuls autres écrivains de `strength` sont inatteignables (ci-dessus). Toutes les
  forces valent exactement `0.5`, la moyenne égale la cible, et `synaptic_scaling` sort
  immédiatement. **Le théorème d'aveuglement de D2 est un défaut latent, pas un défaut
  actif** : il se manifestera intégralement dès qu'un seul écrivain de `strength` sera
  rebranché. C'est précisément pourquoi D9 doit être tranché avant tout réglage.

**Conséquence structurelle** : les **capteurs** (entropie, ratio `max/moyenne`,
`HomeostasisSignal`, méta-plasticité) et les **actionneurs** (scaling, évaporation, bruit)
existent mais ne sont **pas reliés**. Le seul couplage mesure→action réellement câblé est le
scaling synaptique sur la moyenne des forces — c'est-à-dire la régulation d'**un unique
scalaire global**, aveugle à la forme (D2) — et il est lui-même inerte. Le mot
« homéostasie » (`homeostasis.rs:1-5`, `:66-71`) décrit une **intention**, pas une propriété
du système en fonctionnement. `docs/cognitive/canonical-features-pipeline.md:66` décrit un
Consolidator actif et un Dreamer « periodically » déclenchant `refresh_all_ricci`
(`geometric.rs:273-276`) : les deux sont des stubs.

Corollaire méthodologique, qui vaut pour P3 : « la propriété est testée » n'équivaut pas à
« la propriété est exercée ». La quasi-totalité des éléments ci-dessus ont des tests verts et
zéro appelant de production.

## D10. Les passes sont couplées par un état et des clés partagés, non par une interface

Même en supposant D9 résolu, les passes ne composent pas par des contrats mais par des
**effets de bord partagés** — ce qui rend toute garantie locale illusoire.

| Couplage | Preuve | Effet |
|---|---|---|
| Drapeau `HILBERT_SORTED` entre le Warden et les enrichers | le Warden invalide le drapeau (`warden.rs:244`) puis **permute physiquement les slots** (`:255` → `store.rs:8614-8637`) ; l'état est partagé `store.rs:1413`, `:8723` | la sonde de couverture des enrichers échantillonne un **préfixe** de `node_ids()` (`hilbert_enricher.rs:147`) : un tick du Warden change la couverture **mesurée** sans qu'aucune donnée ne change → recalcul plein graphe déclenché par une autre passe |
| Clé `_cog_energy` ≡ `PROP_ENERGY` | `consolidation.rs:274` écrit `_cog_energy`, qui **est** `PROP_ENERGY` (`store_trait.rs:20`), relu par `energy.rs:348` et réhydraté par `load_from_graph` (`energy.rs:514-525`) | un nœud `Condensed` créé par la consolidation est réhydraté comme porteur d'une énergie réelle : deux passes écrivent la même grandeur avec des sémantiques différentes |
| Deux alias de `DERIVED_FROM` | `consolidation.rs:84` vs `provenance.rs:152` (ré-exports distincts `lib.rs:255` / `:263`) | une dérivation de consolidation est indiscernable d'une dérivation de provenance → P4 ne peut pas s'appuyer dessus |
| Deux écrivains de `_kernel_embedding` | `KernelEnricher` : `new_untrained(erased, 42)` (`kernel_enricher.rs:201`) ; `KernelListener` : `new_untrained(store, config.seed)` + `load_phi()` (`engine.rs:661-667`) ; `save_phi` sans appelant | même clé canonique, **projections différentes**, aucune priorité déclarée |
| `reinforce(a,b)` rescale des arêtes non renforcées | `synapse.rs:801-802` appelle `normalize_outgoing(source)` **et** `(target)` | un renforcement local modifie des arêtes étrangères : l'ordre `reinforce(A→B)` puis `reinforce(A→C)` ≠ l'ordre inverse |
| Ordre d'énumération → identifiants de communauté | `cognitive_init.rs:100-118` construit l'index dense **dans l'ordre de `all_node_ids()`** ; ces ids sont persistés puis relus par le Stage 7 et alimentent le Stage 9 | une permutation de slots change les ids de communauté, donc les arêtes COACT, donc `_hilbert_features` |

Et **deux écrivains non arbitrés d'un même état ne sont pas le pire cas** : le
dispatch réactif est explicitement **concurrent**. `obrain-reactive/src/scheduler.rs:333-400`
distribue le lot aux listeners « concurrently … in a separate spawned task via
`tokio::task::JoinSet` », sans ordre ni exclusion mutuelle. Cinq passes qui écrivent dans des
colonnes partagées traitent donc le même lot en parallèle, et l'ordre d'enregistrement
(`engine.rs:542, 596, 626, 645, 670`) **suggère faussement** une séquence. Aucun résultat
n'est reproductible par construction.

Le point 3 du papier s'applique ici dans sa forme la plus dure : l'invariant n'est pas
seulement contourné (D7), il est **partagé sans contrat**. Tant que les passes communiquent
par des clés et des drapeaux globaux, aucune déclaration d'invariant locale ne peut être
vraie.

## D11. La courbure persistée n'est pas une courbure, et rien ne la consomme

**La formule est fausse** — vérifié par calcul exact et indépendant (Annexe C) : la quantité
codée dans `ricci_u8` n'est ni l'Ollivier-Ricci, ni la borne de Jost–Liu citée. Trois
identités d'erreur : `K_n → 1 − κ`, arête pendante `→ +1/d` là où `κ = 0` exactement, arbre
`k`-régulier `→ κ/2`. Le seul test du dépôt porte sur `K₃`, **l'unique cas où la formule
fausse donne le bon résultat**.

Trois conséquences structurelles, indépendantes de la formule :

1. **Le signal est dominé par le degré, pas par la topologie.** Pour toute arête sans
   triangle, la valeur vaut `1/d_u + 1/d_v − 1`, négative dès que `d_u, d_v ≥ 3` : dans un
   graphe creux la quasi-totalité des arêtes est « négative » par arithmétique de degré. C'est
   l'explication du « 83 % de κ < 0 » rapporté par le ledger du dépôt — un artéfact, pas une
   découverte topologique.
2. **La classification du module est inversée sur les feuilles** : `geometric.rs:9-10` définit
   `κ < 0` comme « the only bridge between two otherwise disconnected regions » ; une arête
   pendante — exactement ce pont — reçoit `κ > 0` et serait donc classée « embedded inside a
   locally-dense cluster ».
3. **Rien ne la consomme.** Grep : **zéro** occurrence de `curvature`/`ricci` dans
   `obrain-engine/src`, `obrain-adapters/src` et `obrain-rag/src`. `RICCI_STALE` n'est
   **jamais posé** (`mark_incident_edges_stale` n'a aucun appelant) : le chemin incrémental
   n'existe pas, seul le recalcul complet par `enrich --ricci` tourne.
   `compute_all_node_curvatures` n'alimente que des `tracing::info!`
   (`cognitive_init.rs:292-297`, `converter.rs:2002-2039`). La porte κ du BFS n'a **qu'un
   mock de test** comme `CurvatureProvider` (`context_loader.rs:642-646`). Le Leiden pondéré
   par κ n'a aucun appelant de production (`cognitive_init.rs:73`), et le Leiden réel tourne
   avec des arêtes **non pondérées** (`w = 1.0`, `cognitive_init.rs:52-57`, `:125-126`).
   Le tick du Dreamer, censé déclencher le rafraîchissement, est un stub.
   Les seules surfaces consommatrices sont de la **restitution** : overlay de viz, outil MCP
   `node_curvature`, `enrich --ricci`.

⇒ Pour P1, cela signifie que la courbure n'est pas seulement à corriger : **il faut aussi
construire le chemin de consommation**, qui n'existe aujourd'hui à aucun étage.

## D12. La topologie au sens strict n'existe pas

* **Betti, caractéristique d'Euler, homologie, espace des cycles : aucun résultat** —
  grep sur `crates/` **et** `docs/` : zéro occurrence. Ce qui existe : composantes connexes
  (fortes et faibles), comptage de triangles, arbre couvrant minimal, détection de cycle
  **négatif** uniquement (`procedures.rs:212-234`).
* **La couche simpliciale est réelle, soignée, et jamais peuplée.** `SimplexRecord`
  (32 o, kinds `TRIANGLE|K4|K5|HYPEREDGE`, drapeau d'orientation), zone d'incidence de 16 o par
  slot, payload n-aire : c'est du travail sérieux. Mais la maintenance est gatée sur une
  configuration écrite **uniquement** par la CLI `obrain topo materialize`
  (`store.rs:10021-10026`, `:10446-10478`) — **aucun appel** dans `neo4j2obrain`,
  `obrain-migrate`, `obrain-indexer` : l'import ne peuple donc jamais la couche. Et rien ne la
  lit côté moteur (`grep simplex` dans `obrain-engine/src` → 0). Il n'y a d'ailleurs **ni
  opérateur de bord, ni complexe de chaînes, ni calcul de rang** : c'est une matérialisation
  de cliques, pas de l'homologie.
* **Le « Leiden » n'implémente pas la garantie qui le distingue de Louvain** : `refine(...)`
  prend `_gamma` en paramètre et **ne le lit jamais** (`leiden.rs:692`), sans BFS de
  γ-connexité — alors que la documentation du fichier l'annonce (`:12-14`, `:283-288`).

**Verdict de la couche géométrique/topologique** : un calcul réel, persisté proprement, dont
la formule est fausse au regard de sa propre référence et dont la sortie sert presque
exclusivement à la restitution. Ce n'est pas « décoratif » au sens où le coût est payé à
l'import (Stage 6) — c'est plus grave : c'est une métadonnée **plausible mais incorrecte**,
aujourd'hui sans consommateur pour la contredire.

---

# Partie II — Propositions

## P1. Un opérateur de transport unique, courbé, à spectre explicite

### FORMULE

Conductance d'arête dérivée de la courbure **déjà persistée** `κ_e` (et du poids `w_e`) :

```
c_e = w_e · exp(κ_e)                    κ_e ∈ [−1, 1]
L_κ f(u) = (1/d_u) Σ_v c_uv (f(u) − f(v))        d_u = Σ_v c_uv
∂_t f = −L_κ f + γ f − μ f
```

En base propre de `L_κ` (`L_κ φ_k = λ_k φ_k`, `0 = λ_1 ≤ λ_2 ≤ …`) :

```
ĝ_k' = (γ − μ − λ_k) ĝ_k
```

**Trois propriétés, chacune prouvable et chacune testable :**

1. **Conservation.** `L_κ` est à sommes de lignes nulles ⇒ `Σ_u ∂_t f|_A=0 = 0` : la
   diffusion **transporte**, elle ne détruit pas. Aujourd'hui obrain ne fait que détruire
   (`f ← c·f` scale la somme). « Décroissance » dans obrain = annihilation, pas transport.
2. **Principe du maximum.** Pour `γ = μ = 0` et `c_e ≥ 0`, `sup f` est non croissant
   (matrice stochastique) ⇒ **borne `L∞` dérivée, sans aucun clamp**, donc sans perte
   d'ordre. C'est l'opérateur déjà présent dans `heat_diffusion.rs` (combinaison convexe) —
   le seul du dépôt qui respecte déjà ce principe ; il suffit de le promouvoir.
3. **Taux certifié.** Si `κ_min = min_e κ_e > 0`, l'inégalité de Bakry–Émery (`CD(κ,∞)`)
   donne `λ_2 ≥ κ_min` : obrain **possède déjà** la constante qui certifie son taux de
   mélange, et ne l'utilise pas. Le seuil binaire de `context_loader` devient un effet
   continu : les arêtes à courbure négative (goulets) transportent lentement, les arêtes
   intra-cluster vite.

La **fenêtre gain/amortissement** du papier devient alors un critère calculable :
`λ(k)` croît avec `k`, donc `γ − μ − λ_k > 0` aux basses fréquences (la structure survit,
elle croît) et `< 0` aux hautes (le bruit meurt). Le régime souhaité n'est plus une
espérance, c'est une **inégalité sur le spectre**, vérifiable sur un graphe réel.

### MECANISME

1. **Un seul opérateur.** Extraire de `heat_diffusion.rs` le noyau de diffusion
   (voisinages + poids + combinaison convexe) en primitive partagée, et y ré-exprimer :
   décroissance énergie (n° 1, 3), décroissance synapse (n° 7), activation (n° 8),
   stigmergie (n° 6). Les lois FSRS (n° 4) et homéostasie (n° 5) restent des
   *régulations* au-dessus de l'opérateur, pas des lois concurrentes.
2. **Conductances courbées.** Lire `ricci_u8` dans les conductances (`c_e = w_e·exp(κ_e)`).
   Coût nul : la valeur est déjà dans `EdgeRecord`. `context_loader` garde son seuil pour
   compatibilité, mais la sélection devient graduée.
3. **Constantes dérivées.** Le `half_life` effectif devient `1/(γ − μ − λ_k)` par bande,
   donc un paramètre de *spectre* au lieu de `(cadence, facteur)`. Les balayages
   successifs deviennent des pas d'un même semi-groupe (`exp(−tL_κ)`), pas des lois
   distinctes.
4. **Règle de conception.** Interdiction d'un scalaire uniforme `(γ, λ)` : tout réglage
   doit être une fonction du mode, ou justifier que le spectre est plat.

### GAIN

* **11 lois → 1 opérateur + ses limites** ; les constantes de temps deviennent des
  valeurs propres, donc mesurables au lieu d'être choisies.
* **Invariants prouvés au lieu de clamps** : conservation de masse + principe du maximum
  ⇒ plus besoin de `clamp(0,10)`, `clamp(0,1)`, `min(max_w)` pour la stabilité.
* **La courbure est persistée par arête, à `O(deg)`** — c'est le seul des deux objets
  géométriques réellement disponible en `O(1)` à la lecture. **Mais sa formule est fausse
  (Annexe C) : elle doit être corrigée avant tout usage comme conducteur.**
* **Sélectivité** : aujourd'hui impossible (D6) ; demain réglable par `γ − λ_k`.

**Correction par rapport à la première version de ce document** : P1 prétendait « le spectre
est déjà calculé ». C'est faux, et la vérification exécutée l'a établi (Annexe A.1) : **aucune
valeur propre n'est calculée nulle part**, et au-delà de 10 000 nœuds les facettes dites
spectrales sont des features de degré (A.5). P1 n'est donc pas gratuite : elle doit produire
son propre spectre. Le seul actif réellement disponible est la courbure.

### TEST

Protocole sur un graphe réel (le brain lui-même), champ `f` = énergie ou poids synaptique.

1. Calculer `L_κ` (conductances courbées), sa base propre `{φ_k, λ_k}`, et projeter les
   états successifs : `ĝ_k(t)`.
2. Estimer le taux par mode : `ρ_k = (1/Δt)·log(ĝ_k(t+Δt)/ĝ_k(t))`.
3. **Mode scalaire** : régresser `ρ_k` sur `λ_k` (linéaire) et mesurer `R²`. Prédiction de
   la proposition : `ρ_k ≈ γ − μ − λ_k`, donc `R²` élevé et pente `−1`.
4. **Réfutation** (le test doit pouvoir tuer la proposition) :
   * si `ρ_k` est **constant** en `k` (dispersion < bruit de mesure), le modèle scalaire
     est adéquat et P1 est inutile ;
   * si `ρ_k` ne dépend pas de `κ`, la courbure n'apporte rien et P1 se réduit à un
     laplacien non courbé ;
   * si le principe du maximum est violé (`sup f` croît avec `γ = μ = 0`), le noyau
     n'est pas un opérateur de Markov et l'implémentation doit être revue avant toute suite.
5. **Validation** : `ρ_k` décroissant en `λ_k` avec `R² > 0.8` sur > 90 % des graphes du
   corpus ; et vérifier la conservation `|Σ f(t+Δt) − Σ f(t)| / Σ f < 1e-9` à `γ = μ = 0`.

### RISQUE

1. **Coût du spectre — devenu un prérequis, pas un simple coût.** La base propre complète est
   hors budget sur 10⁶ nœuds, et **les `λ_k` n'existent pas aujourd'hui** (Annexe A.1) : P1
   doit les produire. Mitigations : quotient de Rayleigh sur les vecteurs déjà obtenus
   (quasi gratuit si la base existe), estimation de `λ_2` seul (Fiedler), ou traitement par
   bandes.
2. **La base propre existante n'est pas fiable en l'état.** Elle est dégénérée dès qu'un nœud
   a un degré sortant nul (A.2) et **mathématiquement fausse** sur un graphe orienté non
   équilibré (A.3) : la déflation retire `D^{1/2}1`, qui n'est pas un vecteur propre de la
   matrice réellement utilisée. Toute réutilisation de `spectral.rs` pour P1 doit d'abord
   corriger cela (symétriser explicitement, ou passer au laplacien de marche aléatoire
   orienté), sans quoi P1 hériterait d'une base fausse. À noter : au-delà de 10 000 nœuds la
   question est masquée, puisque le code substitue un proxy de degré (A.5).
3. **La courbure n'est pas la bonne quantité — bloquant.** La formule codée n'est ni
   l'Ollivier–Ricci ni la borne de Jost–Liu citée (Annexe C, vérifié par W₁ exacte sur 11
   graphes, avec trois identités d'erreur : `K_n → 1 − κ`, arête pendante `→ +1/d` au lieu de
   `0`, arbre `k`-régulier `→ κ/2`). Utiliser `ricci_u8` comme conductance injecterait une
   géométrie fausse — pire qu'absente, parce que silencieusement plausible. **P1 doit donc
   corriger la formule d'abord** : soit le vrai Théorème 3 de Jost–Liu, soit un solveur W₁
   sur les arêtes qui comptent. Le pas de quantification (`1/255 ≈ 3.9e-3`) est en revanche
   acceptable, et `exp()` n'amplifie pas l'erreur de façon problématique.
4. **Cohérence des colonnes.** Le principe du maximum suppose `c_e ≥ 0` et une orientation
   traitée symétriquement ; les graphes d'obrain sont **orientés** et hétérogènes
   (poids, classes d'arêtes, `causal_boost`). Écrire la version symétrisée explicitement,
   sinon la conservation est fausse. Noter que la courbure, elle, est déjà calculée sur la
   version symétrisée (`Direction::Both`) : c'est un point d'appui, pas un obstacle.
5. **Migration.** Changer la sémantique de la colonne énergie est un changement de format :
   à versionner (la culture du dépôt le prévoit explicitement pour `ricci_u8`).
5. **Effet de bord sur les tests.** Les tests actuels encodent le comportement saturé
   (`energy.rs:800` commente « saturates at max_energy (clamped to 1.0 in Q1.15) ») : ils
   devront être réécrits, et ce sont eux qui documentent aujourd'hui la dégénérescence.

## P2. Doctrine de normalisation unique : saturation ordre-préservante + contrôle par distribution

### FORMULE

Deux substitutions indépendantes, chacune fermant un défaut prouvé en D2/D3/D4.

**(a) Saturation à la place du clamp.** Toute borne doit être une application
**strictement croissante et bornée** (`σ`), jamais un clamp :

```
σ(x) = x / (1 + x/ε)        (bornée par ε, inversible, strictement croissante)
```

Le clamp `x ↦ min(x, M)` n'est **pas injectif** : deux valeurs distinctes au-delà de `M`
deviennent identiques, et aucune transformation postérieure ne peut restaurer l'ordre.
La saturation, elle, préserve l'ordre strict : la queue du classement reste exploitable.

**(b) Contrôleur de distribution à la place de la similitude.** Remplacer
`s ← c·s` (homogène de degré 0 sur les statistiques qui comptent, D2) par un **transport
monotone 1-D** : `T` qui envoie la mesure empirique des forces vers la mesure cible.

```
T = F_cible^{-1} ∘ F_empirique        (réarrangement croissant, transport optimal 1-D)
```

`T` préserve l'ordre (comme la similitude) **et** contrôle toute la distribution :
cible de Gini, de queue, de `max/moyenne`. C'est la version correcte du « deux normes » du
papier : la norme agrégée et le sup sont pilotés par la même application, au lieu d'être
ignorés par l'une (homéostasie) ou subis par l'autre (normalisation par le max de
`fabric.rs`).

### MECANISME

1. `σ` remplace les clamps dans `energy.rs:413/416`, `synapse.rs:162/790`,
   `engram/homeostasis.rs:125`. La borne devient une **conséquence** de `σ`, plus une
   troncature.
2. **Ranger la colonne énergie.** `f32_to_q1_15` perd un bit : `q1_15_to_f32` décode `[0,2)`
   alors que l'encodage clampe à `1.0`. Corriger l'échelle (ou le clamp) restaure la plage
   pleine et débloque la chaîne de boosts **sans changement de format du slot** (à vérifier,
   sinon versionner).
3. `normalize(x, max)` (`fabric.rs:516`) : remplacer par une normalisation de **rang**
   (`x ↦ rang/(n−1)`), insensible aux outliers, ou par `σ` appliqué après un centrage
   robuste (médiane/MAD). Un nœud extrême ne doit plus fixer l'échelle de tous les autres.
4. Le contrôleur de distribution remplace `synaptic_scaling` ; il s'applique aussi bien
   aux énergies qu'aux poids synaptiques, ce qui unifie la régulation.
5. **Borne inconstructible à violer** (D7) : déplacer les bornes dans les constructeurs de
   type *et* dans la désérialisation (`Synapse::new`, `Synapse::reinforce_at`,
   `synapse.rs:1123`, `synapse.rs:1401`), plus dans les appelants. Une valeur hors borne ne
   doit pas pouvoir exister en mémoire, quel que soit le chemin d'écriture.
6. **Coefficient de concentration borné** (D8) : la précision β d'engram
   (`engram/types.rs:57`) doit être bornée ou normalisée, et l'invariant de *concentration*
   (entropie du softmax / masse maximale) doit être exposé comme une grandeur surveillée,
   pas seulement la masse.
7. **Test de conformité transverse** : un test qui échoue si un clamp dur est réintroduit
   dans la couche cognitive, et un test qui vérifie que toute fonction de normalisation
   est strictement monotone sur son domaine.

### GAIN

* Ordre préservé dans la queue ⇒ les classements (« hotspots », `get_risk_zones`,
  recherche pondérée par l'énergie) cessent d'être arbitraires entre nœuds saturés.
* Le contrôleur devient **capable d'agir** : c'est la différence entre « normaliser » et
  « multiplier par une constante ».
* Fin de la divergence inter-backends : une seule sémantique de plafond.

### TEST

1. **Réfutation directe du contrôleur actuel** (le test doit échouer sur le code
   d'aujourd'hui) : construire une population, appliquer `synaptic_scaling` jusqu'au point
   fixe, et mesurer Gini, `max/moyenne`, entropie normalisée. Prédiction : **variation
   < 1e-12 hors clamp** ⇒ le test échoue ⇒ D2 est confirmé par l'expérience, pas seulement
   par le calcul.
2. **Saturation** : compter les collisions d'ordre — le nombre de paires distinctes
   `(s_i, s_j)` rendues égales. Avec un clamp, les nœuds boostés depuis des historiques
   différents collisionnent en masse ; avec `σ`, zéro collision.
3. **Énergie** : après la correction de plage, vérifier que `n` boosts successifs donnent
   `n` valeurs distinctes (test de non-dégénérescence), et que `energy_score` couvre
   réellement `(0,1)` et non `{0, 0.6321}`.
4. **Normalisation robuste** : injecter un outlier de magnitude `10⁶` et mesurer l'écart-type
   des `n−1` autres scores avant/après changement. Le max-normalisé les écrase ; le
   rang-normalisé non.
5. **Contournement** (D7) : écrire une valeur hors borne via `Synapse::new` /
   `reinforce_at`, la persister, recharger — vérifier que la borne tient. **Le test doit
   échouer sur le code d'aujourd'hui** (`synapse.rs:1123/1401` restaurent la valeur brute).
6. **Concentration Hopfield** (D8) : mesurer l'entropie du softmax normalisée par `log n` en
   fonction de β. Prédiction : elle décroît vers 0 (winner-take-all) tandis que la masse
   reste 1. Réfutation : si l'entropie reste plate, β ne concentre pas et D8 est faux pour
   ce chemin.

### RISQUE

1. **Rupture de compatibilité de scores.** Les seuils existants (fabric, zones de risque)
   sont calés sur l'échelle actuelle ; une normalisation par rang change toutes les valeurs.
   Prévoir une période de double-écriture et un comparatif (`docs/PROPOSAL-COMPARATIF-*`).
2. **Le transport 1-D suppose un ordre total.** Il faut décider quoi faire des ex æquo
   (rangs moyens) et des nœuds non encore observés (pas de rang) : les exclure du
   réarrangement, sinon la mesure cible est biaisée par la population froide.
3. **Saturation ≠ immunité à la concentration.** `σ` borne, mais une fois plusieurs nœuds
   proches de `ε`, la résolution relative redevient faible. C'est pourquoi P2 est
   inséparable de P1 : la borne doit venir du spectre, pas seulement de la non-linéarité.
4. Le bit de marge de la colonne Q1.15 est peut-être **volontairement** réservé ; à
   confirmer avant de changer l'échelle d'encodage.

---

## P3. Les passes déclarent leur invariant et leur résidu

### FORMULE

Chaque passe `P` de la flotte (Thinkers + enrichers) déclare un triplet **explicite** :

```
I_P   : l'ensemble d'invariants préservés EXACTEMENT (à l'arrondi près)
R_P   : une fonctionnelle de résidu, R_P(P(x)) ≤ R_P(x)  (amélioration monotone)
P∘P = P  sur le résidu : R_P(P(P(x))) = 0 à la tolérance près (idempotence)
```

Plus un invariant **global** `I*` — celui de P1 (conservation de masse + principe du
maximum) — que **toutes** les passes doivent préserver. C'est le point 3 du papier
transposé : une correction qui ne préserve pas l'invariant exactement ne corrige pas, elle
accumule.

L'état actuel est partiellement bon, ce qui rend la proposition peu invasive : la passe
d'enrichissement CLI est **déjà** documentée « additive et idempotente »
(`obrain-cli/src/commands/enrich.rs:4`), et la passe Ricci a des tests qui vérifient
l'idempotence par la quantification (`substrate/geometric.rs:1673`, `:1732`, `:1783`). Mais le
verdict réel, passe par passe, est plus contrasté — et montre que la propriété n'est ni
déclarée, ni générale, ni exercée :

| Passe | Verdict | Preuve |
|---|---|---|
| `refresh_all_ricci` | **idempotent en valeur** | écriture conditionnelle `if cur.ricci_u8 != new \|\| stale` (`geometric.rs:357`) ; test `edges_updated == 0` (`:1723-1735`). Pas idempotent en **coût** : la 2ᵉ passe recalcule tout |
| `run_ldleiden_batch`, `synthesis`, `cristallization::find_stable` | **idempotents** | sans RNG / lecture pure (`consolidation.rs:198` revendique le déterminisme, et c'est faux ailleurs) |
| `HilbertEnricher`, `KernelEnricher` | **idempotents en valeur, pas en état** | skip si couverture ≥ 0.10 (`hilbert_enricher.rs:175-187`) mais compteurs croissants et horloge (`:164-214`) ; et le skip repose sur une sonde **biaisée** (D10) |
| `synaptic_scaling` | **non idempotent** | `m_{n+1} = m_n(1 + c(T − m_n))` n'est pas une involution : deux passes ≠ une |
| `SynapseStore::reinforce`, `decay_all` (×2) | **non idempotents** | additif au temps courant (`synapse.rs:145-149`) ; `decay(f)` deux fois = `f²`, et `0` est absorbant (`writer.rs:3256`) |
| `FabricListener::on_batch` | **non idempotent** | rejouer un lot double `mutation_frequency` et déplace le risque de **toute** la population (`fabric.rs:627`, `:204`) |
| `stage_derive_coact` (Stage 7 du pipeline) | **non idempotent** | `create_edge_with_props` **sans test de préexistence** (`pipeline.rs:1092-1101`) : chaque réexécution ajoute `K(K−1)` arêtes par communauté — et le Stage 9 énumère **tous** les types d'arêtes (`hilbert_features.rs:1114-1124`), donc `_hilbert_features` dépend de l'historique de réexécution |
| `consolidate`, `distill`/`inject`/`evaluate`, `adapt`, `bootstrap` | **non idempotents** | `create_node` inconditionnel (`consolidation.rs:266`) ; `SystemTime::now()` dans la valeur de sortie (`distillation.rs:297`) ; ordre d'itération `HashMap` (`:507-573`) |
| `Consolidator`, decay engram | **vacuement idempotents** — la passe n'existe pas | stub (`consolidator.rs:105-119`) ; `is_due ≡ false` (D9) |
| `Predictor` | **non idempotent, et le compteur est factice** | `communities_prefetched += top_k` incrémenté sans aucun prefetch (`predictor.rs:194`) |
| Rejeu WAL des kinds multiplicatifs | **non idempotent, contre la spec** | `docs/rfc/substrate/wal-spec.md:111-114` exige « Replay must be idempotent » ; `EnergyDecay`/`SynapseDecay` rejoués multiplient tous les slots (`replay.rs:571-582`, `:604-618`), et le code s'auto-contredit (`wal.rs:309-311` : « re-applying the same factor multiplies twice »). La vraie garantie est le déterminisme sous ordre, pas l'idempotence |

Trois « idempotences » ci-dessus ne sont donc que l'absence de la passe — la pire forme de
conformité, celle qui passe les tests sans rien faire.

### MECANISME

1. Étendre le trait `Thinker` (`thinker/mod.rs`) de deux méthodes à défaut neutre :
   `fn invariants(&self) -> &'static [Invariant]` et `fn residual(&self, &Store) -> f64`.
2. Le `ThinkerRuntime` (`thinker/runtime.rs`) vérifie `I_P` **après chaque tick** et consigne
   `R_P` avant/après dans `ThinkerTickReport` (`thinker/mod.rs:248`). Une passe qui dégrade
   son résidu ou casse son invariant est signalée, pas silencieuse.
3. Déclarer l'**ordre** de la flotte comme une dépendance explicite, et vérifier l'absence
   de cycle au démarrage (un cycle A↔B est structurellement suspect : chaque passe y lit ce
   que l'autre écrit). Deux cycles réels à traiter en priorité : le gate du Stage 6 (Ricci)
   dérivé de la sortie du Stage 7 (COACT), qui tourne **après**
   (`pipeline.rs:451-453` contre `:231`/`:245`) ; et le compteur fabric ↔ normalisation
   globale (`fabric.rs:204` → `:380` → `:550` → risque de **tous** les nœuds → `:204`).
4. **Sérialiser ou contracter le dispatch réactif.** `obrain-reactive/src/scheduler.rs:333-400`
   distribue le lot aux listeners **concurremment** via `JoinSet`, sans ordre ni exclusion :
   cinq passes écrivent dans des colonnes partagées en parallèle. Soit on déclare un ordre
   total et on exécute en séquence, soit on prouve que les colonnes écrites sont disjointes —
   mais l'état actuel n'est ni l'un ni l'autre, et l'ordre d'enregistrement
   (`engine.rs:542-670`) suggère faussement une séquence.
5. Pour les passes intrinsèquement non idempotentes (EMA d'affinité `affinity.rs:182`,
   compteurs de co-change `co_change.rs:106`, bruit anti-lock-in
   `homeostasis.rs:57` — `anti_lock_in_noise = 0.05`), déclarer un invariant plus faible
   (*résidu monotone*) et **semer le RNG** pour rendre la passe testable.
6. **Déclarer aussi la vivacité, pas seulement la correction.** D9 montre une douzaine
   d'éléments avec tests verts et zéro appelant de production : un contrat d'invariant doit
   donc distinguer « la propriété tient » de « la passe est exercée ». Concrètement : un test
   qui échoue si une passe déclarée `Active` n'est atteinte par aucun appelant de production
   (ou si sa feature n'est incluse dans aucun groupe de features, comme `stigmergy`
   aujourd'hui). Sans cela, P3 certifierait du code mort.
7. **Une clé, un écrivain.** D10 montre plusieurs grandeurs écrites par deux passes sous la
   même clé (`_cog_energy` ≡ `PROP_ENERGY`, `_kernel_embedding` avec deux Φ₀, deux alias
   `DERIVED_FROM`). Un invariant par passe ne veut rien dire si la grandeur n'a pas
   d'écrivain identifié : le contrat doit inclure la **propriété exclusive des clés**.

### GAIN

* La flotte passe de « nudges heuristiques » à **boucle de contrôle vérifiée** : on sait ce
  que chaque passe garantit.
* Les dépendances d'ordre cessent d'être implicites : aujourd'hui personne ne peut dire si
  le résultat dépend de l'ordre d'exécution.
* `R_P` donne un critère d'arrêt mesurable au lieu d'un nombre de ticks.

### TEST

1. **Idempotence** : exécuter chaque passe deux fois sur le même store ; mesurer
   `‖P(P(x)) − P(x)‖`. Réfutation de la déclaration si l'écart dépasse la tolérance pour une
   passe *déclarée* idempotente.
2. **Indépendance à l'ordre** : exécuter `n!` permutations (ou un échantillon aléatoire pour
   `n > 5`) et comparer les états finaux. Réfutation de l'indépendance si la variance dépasse
   la tolérance, avec identification de la paire de passes responsable.
3. **Reproductibilité du dispatch concurrent** : rejouer le même lot d'événements `N` fois
   avec les cinq listeners actifs et comparer les états finaux. Prédiction : états
   **différents** (D10). Réfutation si l'état est stable — auquel cas les colonnes écrites
   sont effectivement disjointes et le point 4 du MÉCANISME est inutile.
4. **Invariant global** : après la flotte complète, vérifier `I*` (masse conservée, `sup f`
   non croissant hors amplification) — c'est le test qui relie P3 à P1.
5. **Borne de budget synaptique** (D8) : construire le contre-exemple chiffré
   (`raw = 10`, `half_life = 1 s`, `Δt = 10 s`, `max_total = 0.01`) et vérifier
   `Σ raw ≤ max_total` après `normalize_outgoing`. **Le test doit échouer sur le code
   d'aujourd'hui** — c'est la mesure de la violation, pas une conjecture.

### RISQUE

1. **Coût.** Vérifier les invariants à chaque tick n'est pas gratuit sur 10⁶ nœuds ; prévoir
   un mode échantillonné (vérification sur un sous-ensemble) et une vérification complète en
   CI seulement.
2. **Fausses certitudes.** Un invariant mal spécifié est pire que pas d'invariant : mieux
   vaut déclarer `I_P` vide et l'assumer explicitement que déclarer une propriété fausse.
3. **Le bruit est un choix de conception** (`anti-lock_in_noise`), pas un bug : le rendre
   reproductible ne doit pas le supprimer.

## P4. Ordre causal, acteurs et chaînage : la provenance qui peut trancher une antériorité

### FORMULE

Ce que le système doit pouvoir répondre, pour toute affirmation `c` :

```
qui_a_dit_en_premier(c) = argmin_{e : hash(contenu(e)) = hash(c)} HLC(e)
depend_de(c)            = clôture transitive de DERIVED_FROM depuis l'événement de c
```

avec `HLC` une **horloge logique hybride** `(temps_physique, compteur, nœud)` totalement
ordonnée, et un **chaînage de hachage** (chaque événement hache ses prédécesseurs) rendant
l'ordre *prouvable* et non *asserté*.

L'état actuel ne permet aucun des deux, et pour une raison plus grave qu'un manque de
champs :

* **Le module de provenance est du code mort en production.** Son docstring affirme
  « callers do not need to explicitly record events — the `ProvenanceRecorder` is called
  internally by the cognitive engine subsystems » (`provenance.rs:8-10`). C'est faux :
  aucun appel de production n'existe (les seuls sont les tests unitaires du fichier et
  `tests/provenance_test.rs`, où le test *simule* l'écriture — « as the engine would do
  automatically », `tests/provenance_test.rs:29-33`). `record()` ne crée **ni nœud ni
  arête** : le module n'importe aucun store, et `HAS_COGNITIVE_EVENT` (`provenance.rs:149`)
  n'est utilisé nulle part ailleurs que dans les tests. Aucun `CognitiveEvent` n'a jamais
  été matérialisé dans un graphe obrain.
* **Registre en RAM, non persistable.** `DashMap<NodeId, Vec<CognitiveEvent>>`
  (`provenance.rs:184-189`), sans capacité, TTL ni compaction, et `CognitiveEvent` /
  `DerivedFromRecord` ne dérivent **ni `Serialize` ni `Deserialize`**
  (`provenance.rs:81`, `:159`) : la provenance ne survit pas à l'arrêt du processus.
* **Horloge murale seule, sans départage.** `timestamp` vient de `SystemTime::now()`
  (`provenance.rs:108`), l'ordre est `sort_by_key(|e| e.timestamp)` (`:303`, `:320`) : deux
  événements dans la même milliseconde ont un ordre arbitraire, et la dérive d'horloge entre
  deux écrivains est indétectable. L'`AtomicU64` `NEXT_EVENT_ID` (`:62`) repart à **1** à
  chaque démarrage et est en `Relaxed` — ce n'est pas une séquence durable.
* **Aucun acteur nulle part.** Grep `actor|author|asserted_by|attributed_to|created_by|
  agent_id|model_id|principal|user_id` sur `obrain-cognitive/src/` **et**
  `obrain-indexer/src/` → **0 occurrence** (contrôle positif fait). Le seul champ proche,
  `trigger_source: String` (`provenance.rs:93`), nomme le **sous-système interne**
  (`mutation_listener`, `manual_boost`, …), jamais la session, le compte, l'agent ou le
  modèle.
* **Le seul signal d'identité disponible est jeté.** L'indexeur lit l'historique git, mais
  `git_history.rs:93-94` fait `let _sha = parts.next();` et ne demande **jamais** `%an`/`%ae`
  : l'auteur du commit est disponible et délibérément écarté. Seul `%ct` (horodatage de
  commit) et les chemins de fichiers sont conservés.
* **`DERIVED_FROM` est un saut local, non inviolable.** Il n'existe réellement dans un store
  que pour la consolidation (`consolidation.rs:341-347`, condensé → original, un seul saut) ;
  dans `provenance.rs` ce n'est qu'un enregistrement mémoire lu à un saut (`:326-330`,
  `:350-359`). Aucune fermeture transitive, aucune signature, aucun `prev_hash`. Les deux
  mondes sont disjoints : la consolidation n'appelle jamais `record_derivation`.
* **Aucune entité « assertion datée ».** Grep `assertion|claim|belief|statement_id|fact_id`
  sur `obrain-cognitive/` → aucun résultat : il n'existe pas d'objet « affirmation » à
  ordonner. On ne peut donc pas poser la question, indépendamment de la provenance.

**Une brique existe pourtant déjà** : le substrate maintient une époque MVCC monotone
(`store.rs:1528`, avancée par `fetch_max` `:15621`, lecture « à une époque » `:12968`) et un
`VersionLog` append-only existe dans `obrain-common/src/temporal.rs` — mais il est
**désactivé par défaut** (`obrain-core/Cargo.toml:63`, `:70`) et la provenance ne le lit
jamais. C'est le point 1 du papier appliqué aux horloges : un temps de *visibilité* (l'époque)
n'est pas un temps *causal*, et confondre les deux produirait un ordre faux.

### MECANISME

1. Étendre `CognitiveEvent` (`provenance.rs:80`) de quatre champs : `actor: ActorId`,
   `hlc: Hlc`, `input_hashes: Vec<Hash>`, `prev_hash: Hash`.
2. `ActorId` doit distinguer humain / session / agent / modèle — c'est le champ qui manque
   pour toute question d'attribution.
3. `hlc` remplace le tri par horloge murale ; `prev_hash` chaîne le journal (append-only,
   déjà la forme du module) pour rendre l'antériorité vérifiable *a posteriori*.
4. Procédures `obrain.provenance.first(claim_hash)` et `obrain.provenance.descendants(id)`
   exposées comme les autres procédures cognitives (`obrain-cognitive/src/procedures.rs`).
5. Réutiliser le weave existant (`TOUCHES` sessions↔fichiers, déjà persisté) comme source
   d'`ActorId` plutôt que d'inventer un nouveau registre.

### GAIN

C'est la seule proposition de ce document qui ouvre une **capacité produit** et non une
correction : obrain devient le système capable de trancher « qui a établi quel lemme en
premier, à partir de quelle entrée, et qu'est-ce qui en dérive » — exactement la question
posée par la controverse de priorité autour du papier Navier–Stokes. Aujourd'hui la réponse
est : impossible (horloge murale, aucun acteur, aucune chaîne).

### TEST

1. **Antériorité sous dérive d'horloge** : deux écrivains dont les horloges physiques
   diffèrent de ±5 s écrivent le même contenu. Vérifier que `HLC` donne un ordre identique
   quel que soit l'ordre d'arrivée au store, et que l'ordre par horloge murale, lui, est
   faux. **Réfutation** : si l'ordre HLC dépend de la dérive physique, le mécanisme est mal
   implémenté.
2. **Concurrence réelle** : deux événements authentiquement simultanés doivent produire un
   **ex æquo explicite**, pas un ordre arbitraire. Réfutation si le résultat dépend du hasard
   d'itération.
3. **Altération** : modifier un événement passé et vérifier que la chaîne de hachage
   l'expose. Réfutation si le journal reste silencieux.

### RISQUE

1. **Coût de stockage.** Quatre champs par événement sur des millions d'événements : mesurer
   avant de généraliser, et prévoir un mode « provenance légère » (acteur + HLC, sans
   chaînage) pour les graphes volumineux.
2. **Immutabilité vs droit à l'effacement.** Un journal chaîné est en tension directe avec
   la suppression de données : prévoir des tombstones et un effacement cryptographique
   (jeter la clé, garder le hash) plutôt que de casser la chaîne.
3. **Le champ `actor` suppose un registre d'identités fiable.** `obrain-iam` existe
   (`crates/obrain-iam/`) : à vérifier avant de dupliquer un concept d'identité.
4. **Séparation des préoccupations.** P4 est indépendant de P1-P3 : c'est un changement de
   modèle de provenance, pas de dynamique. Le coupler à la refonte de l'opérateur serait une
   erreur de séquencement.

---

# Annexe — vérifications EXÉCUTÉES (mesuré, pas lu)

Méthode : test d'intégration jetable dans `crates/obrain-adapters/tests/`, exécuté via
`cargo test -p obrain-adapters --features algos` sur des graphes construits à la main, chaque
test exprimant une **prédiction falsifiable**. Le fichier de test a été supprimé après
mesure ; tous les chiffres ci-dessous sont des sorties réelles, pas des déductions.

## A.1 Le spectre d'obrain n'existe pas

Grep exhaustif (`eigenvalue|rayleigh|fiedler|spectral_gap|largest_eigen` sur `adapters`,
`cognitive`, `substrate`) : **aucune valeur propre n'est calculée nulle part**. Le seul
résultat trouvé est un commentaire de documentation dans `spectral.rs`. `spectral_eigenvectors`
produit des **vecteurs** propres, jamais les `λ_k`.

⇒ **Correction de P1** : la proposition ne peut pas se prévaloir d'un spectre « déjà payé ».
Ce qui existe est une base propre partielle, sans les taux. Le coût de P1 inclut donc le
calcul des `λ_k` (Rayleigh sur les vecteurs déjà obtenus, ou décomposition) — voir le RISQUE 1
de P1, qui devient un prérequis et non un simple coût.

## A.2 Un seul nœud de degré sortant nul effondre toute l'empreinte

Graphe triangle + 1 nœud puits → **1 seule valeur distincte (`0.5`)** sur 4 nœuds × 2 dims.
Contrôle sans le puits → 4 valeurs distinctes. Cause : le garde `min_deg < 1e-10` retourne
`vec![vec![0.5; n]; k]` (`spectral.rs:168-171`). Un nœud sans arête sortante est banal
(feuille, note, nœud document) : **l'empreinte spectrale est dégénérée par défaut**.

## A.3 La déflation est mathématiquement fausse sur graphe orienté non équilibré

Mesure exacte : `M·(D^{1/2}1) = (d_in/d_out)·(D^{1/2}1)` **composante par composante** —
ratios mesurés `[1.0, 0.5, 1.0, 1.5, 1.0, 1.0]`, identiques à `d_in/d_out`, étalement `1.0`.

Donc `t = D^{1/2}1` n'est vecteur propre de `M = D^{-1/2}WD^{-1/2}` que si le rapport
`d_in/d_out` est **constant** sur les nœuds. Le code déflate contre `t` : il retire une
direction qui n'est pas le mode trivial, et **la base extraite n'est pas celle du laplacien
normalisé dès que le graphe est orienté non équilibré** — soit, en pratique, toujours.
Origine du défaut : les degrés sont **sortants** (adjacence construite en `Outgoing`), alors
que le vecteur déflaté `D^{1/2}1` suppose une matrice symétrique.

## A.4 Sur graphe équilibré, l'implémentation est correcte

Deux triangles + pont, arêtes bidirectionnelles → dim0 trié `[0.0, 0.0, 0.138, 0.861, 1.0, 1.0]` :
les trois plus petites valeurs appartiennent au même bloc, le Fiedler **sépare** les blocs.
Le domaine de validité est donc précis : **petit graphe, équilibré**.

## A.5 Au-delà de 10 000 nœuds, les facettes « spectrales » sont des features de degré

Même graphe, `large_graph_threshold = 0` contre `= 1` : **8/8 nœuds** ont une facette 0
différente. Le chemin « grand graphe » (`hilbert_features.rs:376-402`) utilise
`compute_degree_approx` = `[rang(deg_total), rang(deg_moyen_des_voisins)]` (`:1111`).
⇒ Pour tout graphe > 10 000 nœuds, la ligne du tableau d'en-tête « Facette 0 = Spectral
eigvecs 1-2 — Global graph structure » (`hilbert_features.rs:13-14`) est **fausse en pratique**.

## A.6 La substitution est silencieuse — mais le garde existe

Étoile à 4 feuilles (les feuilles sont des puits) → facette 0 du chemin « spectral »
**strictement identique** à celle du chemin « degré » : centre `[1.0, 1.0]`, feuilles
`[0.375, 0.375]` — et `0.375 = 1.5/4` est exactement le rang moyen de 4 ex æquo dans
`rank_normalize`, signature de `compute_degree_approx`.

**Ici ma lecture initiale était trop pessimiste, et il faut le dire** : le code détecte la
dégénérescence (`is_degenerate`, écart-type ≈ 0, `hilbert_features.rs:148`) et substitue
l'approximation de degré (`:394-406`). C'est un point de rigueur à porter au crédit du code —
ma première prédiction (« la constante 0.5 survit dans la facette ») a été **réfutée par
l'exécution**. Ce qui reste vrai : la substitution n'est **ni journalisée ni exposée**, donc
un appelant ne peut pas savoir si la facette « spectrale » qu'il lit est spectrale ou de degré.

## A.7 Les 64 dimensions sont largement redondantes

Graphe anneau de 40 nœuds + cordes, 64 dims. Corrélations `|r| > 0.9` :
**161/224 paires de niveaux DANS une même facette**, contre **64/1792 ENTRE facettes** ;
**29 paires de colonnes strictement identiques**.

Cause structurelle : les 8 niveaux d'une facette sont les ordres 1..8 de la courbe de Hilbert
du **même** point 2D (niveau 0 = grille 2×2, soit 2 bits). La dimension effective est de
l'ordre de 2 par facette (≈ 16), **pas 64** — ce qui compte pour toute revendication de
capacité (index vectoriel, capacité Hopfield `2^{d/2}`).

## B. Spatial — vérifié : deux couches « Hilbert » qui ne se parlent pas

### B.1 La cascade 64d est un empilement de résolutions, pas un plongement spectral

Les 64 dimensions sont **8 facettes × 8 ordres** de la **même** courbe de Hilbert 2D
(`hilbert_encode_point`, `hilbert.rs:131-153` : `order = k+1`, grille `2^(k+1)×2^(k+1)`,
`H_k = d/(4^(k+1)−1)`). Il n'existe **aucune courbe de Hilbert 64-dimensionnelle** : le
niveau 0 est une grille 2×2 (**2 bits**). C'est la cause structurelle de la redondance mesurée
en A.7 (161/224 paires fortement corrélées *dans* une facette).

### B.2 La localité est **faible** — mesuré, et aucun test du dépôt ne l'asserte

Mesure (graphe à 4 anneaux de 8 nœuds + ponts, 36 arêtes, 64 dims, `FacetteWeights::uniform`) :

| Statistique | Valeur |
|---|---|
| distance moyenne des paires **adjacentes** | 2.185 |
| distance moyenne des paires **non adjacentes** | 2.567 |
| **ratio des moyennes** | **0.851** |
| fraction des arêtes sous la médiane des non-arêtes | 0.722 |
| **précision@36** (les 36 paires les plus proches sont-elles des arêtes ?) | **0.722** |

Verdict : la localité existe (**0.722 contre 0.5 au hasard**) mais elle est **faible** — un
encodage réellement préservant la localité donnerait un ratio très inférieur à 0.85 et une
précision proche de 1. Le vecteur 64d est donc un descripteur de rôle structurel, pas un
descripteur de proximité.

Point aggravant relevé par l'audit : **le seul test nommé « locality » du dépôt
(`hilbert_features.rs:1714-1740`) est non discriminant** — il compare deux cliques dont les
nœuds portent des **étiquettes différentes** (`["A"]` contre `["B"]`), et la facette 7 projette
un **hash d'étiquettes** : l'assertion est donc satisfaite par la facette 7 *indépendamment de
toute localité de graphe*. Le test passe, mais ne mesure pas ce qu'il annonce. C'est le
contre-exemple parfait du corollaire de D9 : test vert ≠ propriété exercée.

### B.3 `_hilbert_features` n'est **jamais chargé** par le runtime

`obrain-substrate/src/store.rs:196-200` :

```
SKIP_ON_LOAD_PROP_KEYS = &["_st_embedding", "_kernel_embedding", "_hilbert_features"];
```

avec le commentaire « same story — recomputed by the hilbert warden; **never read by the hub
runtime** ». La cascade 64d est donc **calculée, persistée, puis explicitement écartée au
chargement** par le runtime qui devrait la consommer. S'ajoute le fait que
`weighted_hilbert_distance` (`:1537`) n'a **aucun appelant de production** (seul
`heat_diffusion`, lui-même sans appelant — D9).

### B.4 Deux schémas « Hilbert » disjoints, et un troisième qui n'est pas spatial

| Couche | Ce qu'elle fait | Verdict |
|---|---|---|
| `hilbert_features` (adapters) | signature 64d, logique | **non chargée** au runtime (B.3) |
| `substrate/hilbert.rs` (permutation physique) | trie les **slots de nœuds** | clé de tri = **`(centrality_cached, degré sortant)`** (`hilbert.rs:329-346`), **pas** `_hilbert_features`, **pas** la colonne `hilbert` |
| colonne `substrate.hilbert` | écrite (`writer.rs:1680-1700`) | **jamais lue** : seul un `madvise` de plage (`store.rs:8933`) ; les valeurs sont sans effet |
| colonne `substrate.zorder` | index logique des fenêtres 2D | **réellement lue** (`store.rs:9139-9148`, `window_query_2d_sorted`) |
| « slots512 » | **packing de chaînes de propriétés** (7×512 o dans un carrier 4 Ko, `props_zone.rs:375-385`) | **pas un schéma spatial**, et **désactivé par défaut** (`OBRAIN_PROPS_SLOTS_512=1`) |

Le plan 2D physique (`centralité × degré`) et le plan 2D logique de la cascade (eigvec/degré,
puis degré/degré) **sont différents et ne sont jamais réconciliés**.

### B.5 Ce que la permutation physique fait bien — et ce qu'elle admet

À porter au crédit du code (vérifié) : bijection `old_to_new` validée avant toute écriture
(`slot_remap::validate_permutation`), slot 0 épinglé (sentinelle nulle), arêtes `src`/`dst`
réécrites, side-cars et zones engram enrôlés dans le remap, record `HilbertRepermuteV2`
fsyncé **avant** la première mutation, `msync` barrière, garde d'idempotence
(`HILBERT_SORTED`), fenêtre de rejeu vide exigée. Et surtout un **refus explicite** plutôt
qu'une corruption silencieuse quand des simplexes vivants existent
(`writer.rs:1464-1467`, `:2845-2849`) : les zones simpliciales référencent des slots de nœuds
et ne sont pas encore enrôlées dans le remap.

Ce que le code admet lui-même : « **not** transactionally crash-safe »
(`writer.rs:1439-1447`). Et un écart non tranché : les index de propriétés stockent un slot
brut (`props_index.rs:350-353`) sans epoch de remap, donc une permutation en cours de session
peut produire des faux négatifs — à trancher par un test d'intégration
(`create_property_index` → `repermuter_par_hilbert` → `find_nodes_in_range`), pas par lecture.

## C. Topologique — la formule de courbure est FAUSSE (vérifié par calcul exact)

**Ce paragraphe corrige une affirmation de la première version de ce document**, qui déclarait
« Jost–Liu est implémenté correctement ». C'était faux.

Méthode de vérification, indépendante du code obrain : la W₁ exacte est calculée par la
**dualité de Kantorovich–Rubinstein**, `W₁(μ,ν) = max{ Σ f(x)(μ(x) − ν(x)) : f 1-lipschitzienne }`,
par énumération exhaustive des `f` entières (optimum entier pour des distances entières), puis
`κ(u,v) = 1 − W₁(μ_u,μ_v)/d(u,v)` avec `μ_x` uniforme sur `N(x)` — c'est-à-dire **exactement la
convention que documente le module** (« uniform over `N(x)` in the simplest variant »,
`geometric.rs:29-31`). La méthode est validée par deux résultats publiés qu'elle reproduit :
l'arête pendante donne `κ = 0` (Jost–Liu, Remarque 3) et l'arbre `k`-régulier donne
`κ = −2(1 − 2/k)` (Jost–Liu, Proposition 2).

Résultats sur 11 graphes (vrai vs formule codée `1/du + 1/dv − 1 + T/min`) :

| Graphe | κ exact | formule codée | écart |
|---|---|---|---|
| chemin P4 (arête médiane) | 0.0000 | 0.0000 | 0 ✔ |
| cycle C4 | 0.0000 | 0.0000 | 0 ✔ |
| triangle K₃ | **0.5000** | **0.5000** | 0 ✔ |
| K₄ | 0.6667 | 0.3333 | −0.333 |
| K₅ | 0.7500 | 0.2500 | −0.500 |
| chemin P3 (arête pendante) | **0.0000** | **+0.5000** | +0.500 |
| étoile K₁,₂ (pendante) | **0.0000** | **+0.5000** | +0.500 |
| étoile K₁,₄ (pendante) | **0.0000** | **+0.2500** | +0.250 |
| arbre 3-régulier (arête interne) | −0.6667 | −0.3333 | +0.333 |
| barbell (pont) | −0.6667 | −0.3333 | +0.333 |
| grille 2×3 | **0.0000** | **−0.1667** | −0.167 |

**Trois identités exactes caractérisent l'erreur** (elles ne sont pas des coïncidences
numériques — chacune se démontre) :

1. **`K_n` : code `= 1 − κ_vrai`.** Vérifié jusqu'à `n = 8` (κ_vrai `= (n−2)/(n−1)`, code
   `= 1/(n−1)`, somme `= 1` exactement pour tout `n`). Le code retourne donc le **complément**
   de la vraie courbure. Les deux coïncident **uniquement à `n = 3`** — c'est-à-dire le seul
   graphe sur lequel le dépôt teste la courbure (`geometric.rs:1591-1610`, K₃ = 0.5). Le test
   valide la formule sur l'unique cas où la formule fausse donne le bon résultat.
2. **Arête pendante (`d_u = 1`) : code `= +1/d_v`, κ_vrai `= 0` exactement** (indépendamment
   reproduit de Jost–Liu Remarque 3). Le code **invente une courbure positive** sur toute arête
   de feuille. Or `geometric.rs:9-10` définit `κ < 0` comme « the only bridge between two
   otherwise disconnected regions » : une arête pendante est précisément ce pont, et elle est
   classée « `κ > 0` — embedded inside a locally-dense cluster ». **La classification est
   inversée sur les feuilles**, dans un graphe où les feuilles sont courantes.
3. **Arbre `k`-régulier (sans triangle) : code `= κ_vrai/2` exactement** (rapport 2.00 pour
   `k = 3,4,5,6`). La magnitude est systématiquement divisée par deux là où le terme de degré
   pur domine — c'est-à-dire sur la majorité des arêtes d'un graphe creux, et cela explique
   arithmétiquement le « 83 % de κ < 0 » rapporté par le ledger.

**Conséquences.**

* La documentation `geometric.rs:51` (« a provable lower bound on the true κ, never overshoots
  positive ») est **fausse dans les deux sens** : le code surestime sur les feuilles et la
  grille, sous-estime sur `K_n` et les arbres.
* Le nom est faux : ce n'est ni la vraie Ollivier-Ricci (aucun solveur W₁ dans le dépôt), ni
  le Théorème 2 ni le Théorème 3 de Jost–Liu (le Théorème 2 réel est
  `κ ≥ −2(1 − 1/dx − 1/dy)₊`).
* **Conséquence directe pour P1** : la quantité persistée dans `ricci_u8` ne peut pas servir
  de conducteur de transport en l'état. Ce n'est pas un problème de précision (le pas de
  quantification `1/255 ≈ 3.9e-3` est acceptable) mais de **formule**. P1 doit donc corriger
  la courbure **avant** de l'utiliser — étape que la première version de ce document n'avait
  pas identifiée.

## D. Ce qui reste solide dans la couche topologique et spatiale

Positif, vérifié, et à ne pas perdre de vue :

* **La plage du code est un théorème** (sur la quantité codée, pas sur la vraie κ) :
  `T ≤ d_u − 1` car `v ∈ N(u)` et `v ∉ N(v)`, donc `S ≤ 1/d_u + 1/d_v − 1 + (d_u−1)/d_u = 1/d_v`,
  soit `S ≤ 1/Δ` avec `Δ = max(d_u,d_v)`, et `S > −1`. Donc `S ∈ (−1, 1/2]` et le
  `.clamp(-1.0, 1.0)` (`geometric.rs:186`, `:609`) est **prouvablement inatteignable** : ce
  n'est pas une troncature qui masquerait un dépassement. (La vraie κ, elle, sort de cet
  intervalle : `−1.0` pour un arbre 4-régulier.)
* **La quantification est saine** : pas `1/127.5`, erreur d'arrondi `≤ 1/255 ≈ 3.9e-3`,
  saturation et jamais de wrap. La tolérance `±1e-2` des tests est 2,5× plus lâche que la
  borne réelle. Un point d'attention : `ricci_u8 == 0` sert de sentinelle « non calculé » dans
  `viz_extract.rs:238-240`, alors que 0 est aussi le niveau `−1.0` — ambiguïté atteignable pour
  `Δ > ~510` (il reste des bits libres dans `edge_flags`).
* **Le calcul est symétrisé correctement** : `Duration::Both` + `HashSet` dédupliqué
  (`geometric.rs:106-115`), μ_u uniforme, et les deux sens stockés reçoivent la même valeur.
  Le filtre `**n != src && **n != dst` (`:177`) est inoffensif mais sans objet (`u ∉ N(u)`).
* **La courbe de Hilbert est réelle et testée** (`hilbert.rs`) : conversion `xy↔d` avec test
  de roundtrip, test d'**adjacence** (`d` et `d+1` sont voisins en 2D — propriété
  définissante de la localité), test de déterminisme et de clamping. C'est du vrai Hilbert
  (rotations), **pas** du Morton — un Morton existe séparément dans
  `obrain-substrate/src/hilbert.rs:153-167`.
* **Le garde de repermutation est honnête** : refus explicite plutôt que corruption silencieuse
  quand des simplexes vivants existent (`writer.rs:2836-2855`), car ils référencent des slots
  de nœuds et ne sont pas enrôlés dans le remap. Attention : ce garde est **inerte en
  pratique**, la zone simpliciale n'étant jamais peuplée par l'import (voir ci-dessous).

**Deux sur-revendications documentaires à corriger** : l'en-tête de
`obrain-substrate/src/hilbert.rs:36-42` annonce « **Monotonicity guarantee (tested)** … bounded
by `O(n)` (not `O(n²)`) » alors que son propre test reconnaît le pire cas `O(n²)` et n'asserte
qu'une moyenne ; et `geometric.rs:59-64` parle d'un « 32 B EdgeRecord layout » alors que le
layout réel est 36 B (`record.rs:309`).

## E. Peut-on faire mieux que les 64 dimensions redondantes ? — mesuré

Protocole : graphe à **8 communautés évidentes** (8 anneaux de 10 nœuds + 8 ponts, 80 nœuds,
88 arêtes), `hilbert_features` avec le chemin spectral réel. Métrique principale : **AUC** =
probabilité qu'une arête soit plus proche qu'une non-arête (0.5 = aucune information), mesurée
**globalement** et **intra-communauté** — c'est le test dur, parce que séparer des communautés
est facile et n'est pas ce qu'on demande à un encodage local.

| Encodage | dims | AUC globale | **AUC intra** | P@88 | P@352 |
|---|---|---|---|---|---|
| A — cascade actuelle | 64 | 0.566 | 0.576 | 0.000 | 0.000 |
| B — facettes 2D brutes | 16 | **0.454** | 0.412 | 0.000 | 0.000 |
| C — 2 niveaux les plus fins | 16 | 0.558 | 0.546 | 0.000 | 0.000 |
| D — A projeté en **PCA 16d** | 16 | **0.566** | **0.576** | 0.000 | 0.000 |
| A sans la facette BFS-depth | 56 | 0.527 | 0.546 | 0.000 | 0.000 |
| **F — A + eigvecs 5-8** | 80 | **0.704** | 0.510 | 0.000 | 0.034 |
| **G — F projeté en PCA 16d** | 16 | **0.704** | 0.510 | 0.000 | 0.034 |

Spectre de A : **ratio de participation 3.3 sur 64**, **95 % de la variance en 4 dimensions**.
Spectre de F : 4.8 sur 80.

### E.1 Oui — et on peut gagner 4× en mémoire sans rien perdre (prouvé)

**D a exactement les métriques de A** (0.566 / 0.576 / 0.000 / 0.000), et **G a exactement
celles de F** (0.704 / 0.510 / 0.000 / 0.034). Autrement dit : **16 dimensions reproduisent
intégralement le comportement de 64 — et de 80.** La colonne et l'index vectoriel peuvent
passer de 64 à 16 dims (256 B → 64 B par nœud ; 256 Mo → 64 Mo à un million de nœuds) avec une
perte **indétectable sur cette métrique**. C'est un gain gratuit, sans risque algorithmique —
il ne reste qu'un coût de migration de format.

Corollaire à corriger dans les revendications : une capacité Hopfield `2^{d/2}` se lit sur le
**rang effectif** (≈ 3-5 ici), pas sur le nombre nominal de dimensions.

### E.2 Oui — et il y a un vrai déficit d'information à combler (mesuré)

L'AUC globale de 0.566 sur un graphe à **8 communautés évidentes** est proche du hasard : la
cascade actuelle n'encode presque pas la proximité. Ajouter **deux facettes issues des
eigenvectors d'ordre supérieur (5-6 et 7-8)** fait passer l'AUC globale de **0.566 à 0.704**
(+0.138) et P@352 de 0.000 à 0.034. L'ajout coûte 16 dims — et **ne coûte rien après PCA**
(G ≡ F, 16 dims). C'est la démonstration que le manque n'est pas seulement de la redondance :
c'est un manque d'information, corrigeable.

À porter au crédit de l'encodage existant : la cascade **améliore** l'information par rapport
aux coordonnées brutes (A = 0.566 contre B = **0.454**, ce dernier étant *anti*-informatif —
les arêtes y sont systématiquement plus loin que les non-arêtes). La non-linéarité de Hilbert
apporte donc quelque chose ; ce n'est pas elle qu'il faut jeter.

### E.3 Non — le cas intra-communauté résiste, et c'est structurel

**L'AUC intra-communauté reste ≈ 0.5 (hasard) dans toutes les variantes**, y compris après
ajout d'eigvecs d'ordre supérieur (elle *baisse* même à 0.510 : les vecteurs d'ordre supérieur
résolvent entre communautés, pas à l'intérieur). La cause n'est pas l'encodage mais les
**entrées** : six des huit facettes sont **constantes à l'échelle d'une communauté**
(communauté, centralité, degré, labels, co-usage) — mesuré : 1 facette constant *même sur un
graphe où les labels varient*. Seule la facette 4 (profondeur BFS depuis 2 graines globales)
varie à l'intérieur, et elle varie **monotonement le long des chemins** : deux nœuds de même
profondeur (souvent non adjacents) sont à distance nulle dans cette facette, tandis qu'une
arête est à distance `1/9`. **L'adjacence est donc systématiquement pénalisée** — d'où
P@88 = 0.000 alors que le ratio des distances moyennes, lui, reste favorable (0.926).

C'est une limite de principe, pas un défaut d'implémentation : **un vecteur par nœud encode un
rôle, jamais une adjacence** — l'adjacence est une propriété de *paire*. Deux nœuds de même
rôle sont indiscernables dans *tout* encodage par nœud, si ingénieux soit-il.

**Ce qui marcherait** : une coordonnée qui varie **à l'échelle de la communauté** — donc des
graines **placées dans les communautés** (PageRank personnalisé, ou BFS multi-graines à
l'échelle locale) au lieu de 2 graines globales, et non des eigenvectors globaux d'ordre
supérieur. Le protocole est prêt : il suffit de remplacer la facette 4 et de mesurer l'AUC
intra, qui est aujourd'hui à 0.5 et qu'**aucun test du dépôt ne mesure**.

### E.4 Recommandation en trois étapes, par rapport gain/risque

1. **Tronquer à 16 dims** (PCA ou sélection des niveaux utiles). Gain 4×, perte mesurée nulle.
   Prérequis : accepter une migration de format de la colonne `_hilbert_features` — qui n'est
   de toute façon **pas chargée** par le runtime (B.3), donc le risque réel est faible.
2. **Ajouter les facettes eigvecs 5-8** : +0.138 d'AUC globale, coût nul après PCA.
3. **Remplacer la facette de profondeur BFS par des graines locales**, et **instituer l'AUC
   intra-communauté comme métrique d'acceptation** : tant qu'aucun test ne la mesure, aucune
   de ces trois étapes ne peut être validée ni réfutée.

## F. Réallouer le budget de 64 dims plutôt que le tronquer — mesuré, et c'est mieux

Même protocole et même métrique qu'en annexe E (80 nœuds, 8 communautés, AUC globale et
intra-communauté). L'idée : puisque 4 à 16 dims suffisent, **les 48 dims restantes sont un
budget libre** — autant les dépenser en information nouvelle qu'en résolutions imbriquées.
On garde donc le **format 64d** (l'index vectoriel, la colonne et les consommateurs ne
changent pas) et on remplace des facettes.

Facettes de remplacement testées : **repères locaux** = distances BFS à 4 ancres *par
communauté* (positions 0, 2, 5, 7 de chaque anneau), normalisées en rang, puis encodées par
la même cascade Hilbert — 2 repères par facette, donc 8 dims par facette comme les autres.

| Variante (toutes à 64d sauf mention) | AUC globale | **AUC intra** | P@88 | P@352 |
|---|---|---|---|---|
| A — cascade actuelle | 0.566 | 0.576 | 0.000 | 0.000 |
| H — facette 4 → repères | 0.581 | 0.598 | 0.000 | 0.003 |
| J — facettes 4,5 → repères | 0.593 | 0.537 | 0.000 | 0.003 |
| K — facettes 4-7 → repères | 0.668 | 0.618 | 0.000 | 0.009 |
| **L — facettes 0,1,4,5 → repères** | **0.772** | **0.745** | **0.102** | 0.074 |
| **M — 8 facettes de repères seules** | **0.858** | **0.770** | 0.057 | 0.091 |
| **N — L compressé en 16 dims** | **0.772** | **0.745** | **0.102** | 0.074 |
| **O — M compressé en 16 dims** | **0.858** | **0.770** | 0.057 | 0.091 |

### F.1 Le gain est double, et il est décisif

* **Réallouer paie** : +0.206 d'AUC globale et **+0.170 d'AUC intra** (L contre A). C'est la
  première fois que la discrimination intra-communauté quitte franchement le hasard
  (0.576 → 0.745) et que P@88 cesse d'être nul.
* **Remplacer entièrement paie encore plus** : M, où *toutes* les facettes de rôle sont
  remplacées par des repères, atteint **0.858** d'AUC globale (**+0.292**) et **0.770** intra
  (**+0.194**). Les facettes de rôle (communauté, centralité, degré, labels) n'apportaient
  donc pas seulement peu : elles **noyaient** l'information de proximité.
* **La compression est gratuite, encore** : N ≡ L et O ≡ M, chiffre pour chiffre, avec un
  **ratio de participation de 2.2 sur 64** pour M.

⇒ **16 dimensions de distances à des repères (AUC 0.858) écrasent les 64 dimensions
actuelles (AUC 0.566).** Le contenant n'a jamais été le problème : le contenu l'était.

### F.2 Coût : c'est aussi moins cher à calculer

L'encodage à repères coûte **16 parcours BFS** en `O(V+E)`, contre **deux appels à
`spectral_embedding`** (2 puis 4 vecteurs propres, ~200 itérations de puissance + déflation
chacun) — soit de l'ordre de **1200 produits matrice-vecteur** dans le chemin actuel. Le
remplacement est donc **plus simple, plus déterministe (pas d'itération) et
vraisemblablement plus rapide**, en plus d'être bien meilleur. Il supprime au passage la
dépendance aux trois défauts spectraux documentés en A.1-A.3 (pas de valeurs propres, effondrement
sur un puits, déflation fausse sur graphe orienté) : un BFS ne connaît pas ces problèmes.

### F.3 Ce que ce résultat ne prouve pas — à lire avant de généraliser

1. **Les repères ont été placés en utilisant les communautés *de vérité terrain*** du graphe
   de test. C'est la faiblesse méthodologique principale : la mesure suppose une partition
   disponible. En production il faut la fournir (le Leiden de l'import la calcule, ou un
   échantillonnage « farthest-point » sans étiquette) — et la qualité du résultat dépendra de
   la qualité de cette partition. **À re-mesurer avec des repères choisis sans vérité
   terrain avant toute décision.**
2. **Un seul type de graphe testé.** Les distances à des repères sont *particulièrement*
   adaptées à une structure d'anneaux + communautés plantées — c'est presque le cas d'école
   du plongement par repères (landmark MDS). Sur un graphe sans communautés nettes, ou à
   structure en échelle, le gain sera différent. Un banc sur 3-4 topologies réelles
   (brain, code, social) est nécessaire.
3. **L'AUC intra plafonne à 0.77, pas à 1.** Un plongement métrique ne peut pas rendre
   l'adjacence parfaitement séparable : sur un anneau, deux nœuds à distance 2 sont proches
   eux aussi, et P@88 = 0.057 signifie que les arêtes ne constituent encore que 5,7 % des 88
   paires les plus proches. Pour aller plus loin il faudrait des caractéristiques de **paire**
   (recouvrement de voisinage, Adamic-Adar, similarité structurelle), pas un vecteur par
   nœud — c'est une limite de principe (E.3).

### F.4 Recommandation révisée (elle remplace celle de E.4)

1. **Remplacer les facettes de rôle par des facettes de repères locaux** — d'abord en gardant
   64d, avec la métrique **AUC intra** comme critère d'acceptation. Gain mesuré +0.29 d'AUC
   globale, +0.19 intra, et un coût de calcul *inférieur*.
2. **Puis compresser à 16 dims** : perte mesurée **nulle** (N ≡ L, O ≡ M), 4× de mémoire
   gagnés. L'étape 1 de E.4 (« tronquer à 16 dims ») est donc subsumée : elle ne sert à rien
   tant que le contenu n'a pas changé.
3. **Choisir les repères sans vérité terrain** et **re-mesurer** avant de figer — c'est le
   seul point du document où la mesure repose sur une hypothèse que la production ne fournit
   pas gratuitement.

## G. Les repères survivent-ils sans vérité terrain ? — oui, mesuré

La réserve n° 1 de l'annexe F était la **fuite méthodologique** : les repères y étaient placés
en utilisant les communautés *vraies*, ce que la production ne fournit pas. Test décisif,
même graphe et même métrique (16 repères → 8 facettes → 64d, puis compression PCA-16) :

| Stratégie de placement | dims | AUC globale | **AUC intra** | P@88 | P@352 |
|---|---|---|---|---|---|
| A — cascade actuelle (référence) | 64 | 0.566 | 0.576 | 0.000 | 0.000 |
| 1 — vérité terrain (fuite, borne haute) | 64 | 0.904 | 0.801 | 0.091 | 0.159 |
| **2 — farthest-point global (AUCUNE étiquette)** | 64 | **0.832** | **0.738** | **0.364** | 0.142 |
| 3 — communautés par propagation d'étiquettes | 64 | 0.795 | 0.687 | 0.023 | 0.136 |
| 4 — mixte (8 FPS + 8 par communauté détectée) | 64 | 0.835 | 0.723 | 0.341 | 0.125 |
| **2bis — FPS compressé en 16 dims** | **16** | **0.832** | **0.738** | **0.364** | 0.142 |

### G.1 La fuite ne coûtait presque rien — et le sans-étiquette gagne sur un critère

Le farthest-point sampling **sans aucune étiquette** retient **92 % du gain** de la borne avec
vérité terrain (0.832 contre 0.904 en AUC globale ; 0.738 contre 0.801 en intra), et il la
**bat nettement sur P@88** : **0.364 contre 0.091**. Mon cadrage « borne haute » était donc
trop généreux : étaler les repères sur tout le graphe sert mieux le classement de tête que de
les concentrer par communauté.

**Conséquence de conception, et elle simplifie tout : les repères n'ont besoin d'aucune
partition.** Pas de communautés, pas de Leiden — dont l'audit topologique a montré par
ailleurs que le `refine` **ignore son paramètre `_gamma`** et que la variante pondérée par
courbure n'a aucun appelant. On supprime donc une dépendance fragile **et** la fuite en même
temps.

### G.2 La compression reste gratuite — pour la troisième fois

**2bis ≡ 2, chiffre pour chiffre** (0.832 / 0.738 / 0.364 / 0.142), et 1bis ≈ 1 (l'AUC intra
monte même de 0.801 à 0.804). La compression PCA-16 ne coûte rien non plus sur les variantes à
repères.

### G.3 Le design qui en sort, chiffré

> **16 dimensions = 8 facettes × 2 repères**, distances BFS normalisées en rang puis encodées
> par la cascade Hilbert **déjà en place**, repères choisis par farthest-point sampling
> (déterministe, sans étiquette).

| | aujourd'hui (64d) | proposition (16d) | écart |
|---|---|---|---|
| AUC globale | 0.566 | **0.832** | **+0.266** |
| AUC intra-communauté | 0.576 | **0.738** | **+0.162** |
| P@88 | 0.000 | **0.364** | — |
| mémoire / nœud | 256 o | **64 o** | **×4** |
| calcul | 2 × ~200 itérations de puissance + déflation (≈ 1200 produits matrice-vecteur), non déterministe en pratique (déflation fausse sur graphe orienté) | **16 BFS**, déterministe | plus simple et moins cher |
| dépendances | communautés implicites, vecteurs propres, valeurs propres (jamais calculées), garde anti-dégénérescence silencieuse | aucune | — |

### G.4 Réserves restantes, honnêtement

1. **Une seule topologie** (anneaux + communautés plantées) : c'est le cas le plus favorable au
   plongement par repères. Le banc multi-topologies reste à faire — c'est l'objet de l'audit
   « topologies » lancé en parallèle.
2. **La variante 3 n'est pas un test valable** de « communautés détectées » : ma propagation
   d'étiquettes s'est effondrée sur **1 seule communauté** (pureté 0.125), donc elle a
   pratiquement placé 2 repères dans tout le graphe. Cela n'affaiblit pas la conclusion — FPS
   seul suffit — mais cela ne démontre rien sur les communautés détectées non plus.
3. **Le nombre et l'échelle des repères ne sont pas optimisés** : 16 a été choisi pour remplir
   exactement 8 facettes. Un balayage de k (8, 16, 32, 64) et un placement **multi-échelle**
   (grossier + fin) sont les prochains leviers évidents.
4. **L'AUC intra plafonne à 0.74** : la limite de principe de E.3 tient — un vecteur par nœud
   ne peut pas rendre l'adjacence parfaitement séparable.

## H. Banc multi-topologies — la proposition gagne partout, et l'actuel est pire que le hasard

Réserve n° 1 restante de l'annexe G : une seule topologie. Banc de 5 topologies, même métrique
(AUC = P(arête plus proche que non-arête) ; 0.5 = hasard ; P@k = précision au rang k = nb
d'arêtes). A = cascade actuelle 64d ; B = **16d de repères farthest-point, sans étiquette**.

| Topologie | \|V\| | \|E\| | A — AUC / P@k | **B — AUC / P@k** | gain |
|---|---|---|---|---|---|
| anneaux + 8 communautés | 80 | 88 | 0.566 / 0.000 | **0.832 / 0.364** | **+0.266** |
| **scale-free** (attachement préférentiel m=2) | 80 | 157 | **0.381** / 0.019 | **0.552** / 0.115 | +0.171 |
| grille 2D 10×10 | 100 | 180 | 0.543 / 0.000 | **0.907 / 0.406** | **+0.364** |
| arbre binaire équilibré | 63 | 62 | **0.405** / 0.000 | **0.675** / 0.000 | +0.270 |
| biparti fichiers↔symboles | 60 | 60 | 0.634 / 0.000 | **0.788 / 0.267** | +0.154 |

### H.1 La proposition gagne sur les cinq — la réserve de généralité tombe

Gain positif sur **5 topologies sur 5** (+0.154 à +0.364), y compris sur les structures où les
repères ne sont pas avantagés par construction. Ce n'est plus un résultat de cas d'école.
P@k reste à 0.000 sur l'arbre, où les distances par niveaux créent des ex æquo massifs — c'est
le seul point où B ne débloque pas le classement de tête.

### H.2 Mais le constat le plus grave est sur l'existant : **il est pire que le hasard**

**AUC 0.381 sur scale-free, 0.405 sur un arbre, 0.566 sur le cas le plus favorable.** Une AUC
inférieure à 0.5 signifie que **les arêtes y sont systématiquement plus LOIN que les
non-arêtes** : la cascade actuelle n'est pas neutre, elle est **anti-informative**. Or le
scale-free est la topologie la plus courante pour un graphe de connaissances (loi de puissance
des degrés). Autrement dit : sur un graphe réel typique, la signature de 64 dimensions que
obrain calcule, persiste et indexe **désinforme**.

Cela rejoint la mesure de l'annexe E (les facettes 2D brutes donnaient 0.454, également sous
le hasard) : la non-linéarité de Hilbert rattrape une partie du problème (0.454 → 0.566) mais
ne renverse pas le signe. **Ce n'est pas un problème de dimension ni de redondance : c'est un
problème de contenu, et il est plus grave que je ne l'avais écrit en E.2.**

### H.3 Ce que la proposition ne résout pas

La AUC de B sur **scale-free reste à 0.552**, à peine au-dessus du hasard : un plongement à
16 distances de repères ne capture pas la structure en loi de puissance, parce que les
distances de graphe y sont écrasées par les hubs (limitation connue des plongements métriques
sur graphes à queue lourde). Deux leviers, non testés ici :

1. **Repères pondérés par le degré / multi-échelle** (repères de rangs de degré différents,
   distances normalisées par le degré, ou distances en marches aléatoires plutôt qu'en sauts).
2. **Compléter par un terme de rôle explicite** (degré, coefficient de clustering, rang de
   PageRank) — non pas pour remplacer les repères, mais parce que sur scale-free le *rôle*
   porte une information que la *position* ne porte pas.

   ⚠️ **Cette seconde recommandation est RÉFUTÉE par la mesure — voir I.2.** Je l'ai écrite
   avant de tester la concaténation : mesurer « ancres + degré » dans le même espace L2 fait
   **s'effondrer** l'encodage (.831 → **.239** en scale-free). Le rôle doit vivre en **colonnes
   typées séparées**, jamais concaténé dans le vecteur. Ce paragraphe est conservé pour la
   traçabilité du raisonnement, pas comme recommandation.

C'est le seul point du document où je recommande explicitement de **ne pas remplacer les
facettes de rôle par des repères seuls**, mais de garder un petit budget de rôle à côté —
renversement partiel de la conclusion de F.1, imposé par la mesure.

## I. Budget égal, séparation position/rôle, et le mécanisme de l'anti-information

Trois critiques ont été portées contre les annexes F-H par un panel de cinq analyses
indépendantes. Les trois se sont révélées **fondées**, et les corriger change la conception.

### I.1 La comparaison « repères > spectral » était biaisée en faveur des repères

En F/H je comparais **8 facettes d'ancres (16 repères)** à un chemin spectral qui n'utilisait
que **4 vecteurs propres** (2 + 4). À budget égal — 16 composantes propres contre 16 ancres, une
coordonnée par repère — la mesure est sans appel :

| topologie | 64d actuel | 16 eigvecs | **16 ancres** | ancres + degré | **degré seul (1d)** |
|---|---|---|---|---|---|
| anneaux 80 n | .566 | .445 | **.980** | .857 | .500 |
| **scale-free 200 n** | .479 | **.218** | **.831** | **.239** | **.200** |
| grille 14×14 | .619 | .282 | **.999** | .978 | .629 |
| biparti 120 n | .673 | .500 | **.844** | .663 | .570 |

La conclusion tient — et **l'écart est bien plus grand** que ce que j'avais mesuré : les repères
écrasent le spectral (.831-.999 contre .218-.500). **16 vecteurs propres purs sont pires que le
hasard** (.218 en scale-free, .282 en grille), et valent exactement `.500` en biparti : c'est la
signature de l'effondrement sur la constante documenté en A.2 — un seul nœud de degré sortant
nul suffit à détruire toute la base propre.

### I.2 Ma recommandation H.3 est RÉFUTÉE : le rôle ne doit pas cohabiter dans le vecteur

En H.3 j'écrivais qu'il fallait « garder un petit budget de rôle **à côté** » des repères. La
mesure dit l'inverse : **concaténer le degré au vecteur de repères détruit l'encodage**.

* scale-free : .831 → **.239** (effondrement sous le hasard)
* anneaux : .980 → .857 · biparti : .844 → .663 · grille : .999 → .978

**Correction** : le rôle ne doit pas être concaténé dans l'espace L2, même avec un petit poids.
La raison est structurelle, pas numérique : la similarité de rôle est **invariante** à
l'isomorphisme de composante, la proximité de position y est **sensible par définition** — aucun
plongement unique ne peut être les deux. Le rôle doit vivre en **colonnes typées séparées**
(communauté, degré, centralité, label), et si le degré devient une coordonnée il doit être
**inversé** (distance-au-hub, pas « hub-ness »).

### I.3 Le mécanisme de l'anti-information est identifié, et il est confirmé

Troisième critique : ce n'est pas la redondance qui rend la cascade anti-informative, c'est le
**biais de taille des arêtes** — dès qu'une coordonnée est monotone en (degré, PageRank),
`E|r(u)−r(v)| | arête > | non-arête`, parce que les arêtes touchent préférentiellement les rangs
extrêmes alors que la masse des non-arêtes est faite de paires de rangs voisins.

**Prédiction** : plus la queue de degré est lourde, plus l'AUC chute. **Test** : le score
« degré seul » donne **.200** en scale-free et .629 en grille — il reproduit exactement l'ordre
de mes AUC de cascade (.479 < .619). Le mécanisme est confirmé quantitativement. Cela **remplace**
mon explication d'origine (H.2), qui invoquait la seule redondance.

### I.4 Trois corrections méthodologiques à retenir

1. **Puissance statistique** : `SE(AUC) ≈ .045` pour n ≈ 80-200 (88 positifs, ~3080 négatifs).
   **Tous les écarts inférieurs à .15 dans mes tableaux E/F sont du bruit** — les micro-écarts
   H/J/K/L (0.581 / 0.593 / 0.668) ne sont pas interprétables. Seul A→B (Δ ≈ .27-.36) l'est.
2. **`P@k` était invalide** : j'avais pris `k` = nombre d'arêtes, soit `k > |V|` (88 sur 80
   nœuds, 352 sur 80). Toute métrique `@k` doit publier `k/|V|` et contraindre `k ≤ 0,05·|V|`.
   L'AUC reste la seule métrique valide de mes tableaux.
3. **La compression par PCA est mesurément gratuite, mais disqualifiée comme mécanisme de
   production** : une PCA **globale** rend chaque ligne dépendante de toutes les autres — donc ni
   incrémentale, ni recalculable localement, ni stable après insertion. Ce que la mesure valide
   est un **codage par rang local** (16 distances à des repères), pas une projection globale.
   Les annexes E et F recommandaient « compresser via PCA-16 » : à remplacer par un **budget de
   coordonnées fixe et local**.

### I.5 Une correction factuelle supplémentaire

La formule « écrite puis écartée » (D5, B.3) est **inexacte depuis T16.7** : `Value::Vector` est
routé vers la colonne dense mmap `substrate.veccol.*` (`store.rs:17962`, `:18018-18023`), et
`SKIP_ON_LOAD_PROP_KEYS` ne concerne plus que les bases bincode héritées. La donnée est
**persistée, jamais hydratée** — et `KernelEnricher` (`kernel_enricher.rs:26`) est un lecteur
réel en processus. Le diagnostic D9 reste vrai sur le fond (aucun consommateur de *classement*
dans ce dépôt), mais le vocabulaire doit être corrigé.

## J. Ce que dit le graphe de PRODUCTION — vérifié par requête

Mesuré par moi-même sur le graphe réel du projet (`graph_query`, project
`home.triviere.projects.banc_obrain`), et non plus sur un graphe synthétique :

```
MATCH (n) RETURN count(n)                → 363 346 nœuds
MATCH ()-[e]->() RETURN count(e)         → 1 176 915 arêtes
MATCH (n) WHERE NOT (n)-->() RETURN count(n)  → 161 384 puits
MATCH (f:File) RETURN count(f)           → 16 296 fichiers
```

### J.1 Les facettes 0-1 sont mortes en production — 16 dimensions sur 64

**161 384 nœuds sans aucune arête sortante, soit 44,4 %.** Le garde
`min_deg < 1e-10` de `spectral_embedding` (`spectral.rs:168-171`) se déclenche donc
**nécessairement** — il suffit d'**un** nœud puits — et les facettes 0-1 valent la constante
`0.5` pour **tous** les nœuds. Le garde `is_degenerate` (`hilbert_features.rs:146-170`) détecte
alors la facette constante et la **remplace silencieusement** par `compute_degree_approx`
(`:392-401`).

⇒ **En production, les 16 premières dimensions sur 64 ne sont pas spectrales : ce sont des rangs
de degré** (rang du degré total, rang du degré moyen des voisins), et la substitution n'est ni
journalisée ni exposée. Le seuil des 10 000 nœuds n'est même pas en cause : c'est le taux de
puits qui déclenche.

### J.2 Le graphe réel est exactement le régime défavorable

16 296 `File` contre 37 730 `Function`, 225 418 `DEFINES`, 555 339 `CALLS`, et **39 % des
`Function` n'appellent jamais** : le graphe est quasi-biparti (fichiers ↔ symboles) et à queue
de degré lourde. C'est précisément le régime où la mesure I.3 montre que les coordonnées
monotones en degré sont **anti-informatives** (degré seul : AUC **0.200** en scale-free).

**Synthèse de production** : la signature de 64 dimensions que obrain calcule et persiste est,
sur son propre graphe, composée à 25 % de rangs de degré (anti-informatifs) et, pour le reste, de
facettes de rôle constantes à l'échelle d'une communauté. **Ce n'est pas un problème de
redondance : c'est un problème de contenu, dans le régime précis où le contenu est le plus
nuisible.**

### J.3 Deux défauts d'encodage trouvés sur le chemin réel

1. **« Inatteignable » ≡ « graine ».** La facette 4 mappe un nœud non atteint par le BFS sur la
   profondeur `0` (`hilbert_features.rs:797`, `unwrap_or(&0)`) : avec 44 % de puits et un BFS
   sortant seul, toute une composante est encodée « à distance 0 de la graine ». Le sentinelle
   d'inatteignable doit être distincte de `0` **et** du repère.
2. **Normalisation par max non locale.** `max_d1` (`:791`) est un maximum global (diamètre) :
   l'ajout d'une arête qui étend le diamètre **rescale tous les nœuds d'un coup**. Toute
   stratégie de fraîcheur incrémentale par époque est donc fausse par construction sur cette
   facette — un point à réconcilier avec la recommandation de fraîcheur par époque (I.5).

### J.4 Un risque critique que personne n'avait soulevé : le remap renomme le plan

`bulk_sort_by_hilbert` et `compact_community` **permutent physiquement les slots**. Tout
side-car indexé par slot qui n'est pas enrôlé dans `SlotRemapRegistry` fait que **le vecteur de
A devient celui de B** — sans erreur, sans CRC invalide, avec des données fausses. Ce n'est pas
hypothétique : la télémétrie du projet contient déjà un écart ×10 attribué à une confusion
slot/id (`session-5acdcc77…`). Le contrôle de bonne dimension (`hilbert_features.rs:291-298`)
**ne détecte pas** ce mode de défaillance, puisqu'un mélange de slots conserve toutes les
dimensions. **Aucune AUC ne mesure ce risque** : il faut un test *golden* (écrire les features,
permuter, exiger que `feature(id sémantique)` soit inchangée).

---

## Ordre d'exécution recommandé

1. **D9** — décider, pour chacun des éléments sans appelant de production, s'il est
   **activé** ou **supprimé**. Le fleet n'est pas démarré, le bus n'a pas de source, le tick
   n'est pas sur un timer, et la décroissance des engrams ne s'exécute jamais (`is_due ≡
   false`). C'est un choix de produit, pas une tâche technique — mais il conditionne tout le
   reste : régler un régulateur qui ne tourne pas ne sert à rien, et la plupart des autres
   diagnostics sont aujourd'hui **latents**.
2. **`normalize_outgoing`** (D8) — la seule violation d'invariant **non bornée et vivante**
   identifiée : la borne `Σ poids sortants ≤ max_total` est violée d'un facteur
   `2^(Δt/half_life)`. Corriger en calculant somme et facteur sur **la même** grandeur
   (brute ou décrue), et rafraîchir `last_reinforced` dans `decay_all` pour supprimer la
   double décroissance (F2). Quelques lignes, effet mesurable immédiatement.
3. **D4** — corriger la dégénérescence de l'énergie : soit un appelant de décroissance réel
   (implémenter le tick du Consolidator, ou câbler `EnergyStore::decay_all` sur un timer),
   soit assumer que l'énergie n'a pas de décroissance et retirer le paramètre. La primitive
   phare redevient informative, et la croyance fausse entre deux modules disparaît.
4. **D10** — lever les couplages par état partagé : une clé, un écrivain (`_cog_energy` ≡
   `PROP_ENERGY`, deux Φ₀ pour `_kernel_embedding`, deux alias `DERIVED_FROM`), et sortir la
   sonde de couverture de l'ordre physique des slots (échantillon aléatoire seedé au lieu de
   `take(sample_n)`, `hilbert_enricher.rs:147`).
5. **Gardes numériques immédiates** (chacune quelques lignes) : `0 < cT < 2` sur le
   contrôleur (D2) ; correction du décalage `>> 33` du bruit pour rétablir la symétrie
   annoncée, ou suppression de la revendication d'exploration (D8) ; borne sur β (D8).
6. **D7** — remonter les bornes dans les constructeurs de type et dans la réhydratation.
   Indépendant de tout le reste, mécanique, et il supprime une classe entière de
   contournements (dont une divergence inter-backends).
7. **Test 1 de P2** — écrire le test qui échoue sur `synaptic_scaling`. Il chiffre D2 et
   protège la refonte du contrôleur.
8. **P1 étapes 1-2** — noyau de diffusion partagé + conductances courbées, d'abord en
   lecture seule (double écriture de `f`), puis bascule. **Prérequis bloquant : corriger la
   formule de courbure (D11 / Annexe C) avant de l'utiliser comme conducteur.** Le test de
   référence est déjà écrit dans l'annexe : la W₁ exacte par dualité de Kantorovich–Rubinstein
   sur `K₄` (2/3), une arête pendante (0) et un arbre 3-régulier (−2/3) — trois valeurs que la
   formule actuelle rate, dont deux avec inversion de signe ou de moitié.
9. **Test 3 de P1** — la régression `ρ_k` sur `λ_k` décide si P1 vaut la peine d'aller plus
   loin. Si `R²` est faible, s'arrêter là : le diagnostic D6 resterait vrai mais la
   paramétrisation spectrale n'apporterait rien.
10. **P3** — déclarer invariants, résidus et vivacité passe par passe, et trancher le
    dispatch concurrent. À faire *avant* la bascule de P1 : sans contrat d'invariant, la
    migration de l'opérateur n'est pas vérifiable.
11. **P4** — indépendant de tout le reste, à séquencer selon la priorité produit. C'est la
    seule proposition qui ouvre une capacité nouvelle plutôt que de réparer l'existant, et sa
    première étape est triviale : **ne plus jeter l'auteur git** (`git_history.rs:94`).
12. **Encodage spatial** (Annexes E et F) — indépendant des points 1-11, et le meilleur
    rapport effort/gain du document après la courbure. Ordre mesuré : **(a)** remplacer les
    facettes de rôle (communauté, centralité, degré, labels, profondeur BFS globale) par des
    **facettes de repères locaux** — distances BFS à des ancres communautaires, encodées par
    la même cascade Hilbert : **+0.29 d'AUC globale, +0.19 intra-communauté, et un coût de
    calcul inférieur** (16 BFS au lieu de ~1200 produits matrice-vecteur) ; **(b)** puis
    compresser à 16 dims — perte mesurée **nulle**, 4× de mémoire gagnés. Prérequis de
    validation : choisir les repères **sans vérité terrain** (l'annexe F les place avec les
    communautés vraies) et instituer **l'AUC intra-communauté comme métrique d'acceptation** —
    elle vaut ≈ 0.5 aujourd'hui et aucun test du dépôt ne la mesure.

## Ce que ce document ne prétend pas

* Aucun résultat du papier Navier–Stokes n'est utilisé ; seul son **schéma d'argument**
  (deux normes, fenêtre d'échelle, préservation exacte de l'invariant) est transposé.
* Les diagnostics D1-D12 sont vérifiés dans le code à la date du 2026-09-10 ; les numéros de
  ligne peuvent dériver.* **La portée de D9/D10 est limitée à ce checkout.** `obrain-hub` et `obrain-chat` en sont
  absents (`docs/rfc/substrate/migration-audit.md:96`) : si `spawn_standard_fleet` y est
  appelé, le fleet et les listeners tournent réellement là-bas et les risques correspondants
  deviennent actifs. **Indéterminé depuis ce dépôt** — et c'est la première chose à vérifier
  avant d'agir sur D9.
* **La plupart des diagnostics sont donc latents, pas actifs** : D2 (le contrôleur de
  moyenne sort immédiatement, toutes les forces valant `0.5`), la décroissance d'énergie
  (D4), la décroissance d'engram, l'évaporation stigmergique et le bruit ne s'exécutent pas
  aujourd'hui. Les exceptions **vivantes** identifiées sont : `normalize_outgoing` (D8, la
  borne est violée à chaque renforcement), `refresh_all_ricci` via `enrich --ricci`, le
  Stage 7 du pipeline en réexécution, et `EngramManager::tick` sur le chemin d'écriture MCP.
  Les propositions P1-P4 sont donc, pour l'essentiel, des **préalables** : elles fixent ce
  qu'il faudra garantir le jour où les passes seront rebranchées.
* **Les magnitudes d'obrain sont bornées** (clamps durs + colonnes à virgule fixe) : la thèse
  « rien ne borne les maxima » serait fausse, et c'est la raison pour laquelle le document
  ne la soutient pas. Ce qui manque n'est pas une borne, c'est un *opérateur* : des bornes
  dures détruisent l'ordre de la queue, une normalisation par le max fait dépendre l'échelle
  d'un seul outlier, et aucun mécanisme ne pilote la concentration. Les clamps sont la
  conséquence (D6), pas la cause.
* **Ce qui a été exécuté, et ce qui ne l'a pas été.** Les annexes A et C sont **mesurées** :
  un test d'intégration jetable a été compilé et exécuté
  (`cargo test -p obrain-adapters --features algos`) sur des graphes construits à la main, puis
  supprimé ; et la courbure exacte a été calculée par un script Python indépendant du code
  obrain (dualité de Kantorovich–Rubinstein, énumération exhaustive — une cinquantaine de
  lignes, reproductible sans dépendance). En revanche **D1-D10 et D12 ne sont pas mesurés** :
  ce sont des lectures de code et des dérivations, sans exécution ni banc. Les valeurs
  déployées des configs ne sont pas connues non plus.
* **Les valeurs réellement déployées ne sont pas connues** : `HomeostasisConfig`,
  `EnergyConfig` et `FsrsConfig` sont surchargeables, et l'analyse porte sur les `Default`
  du code. La condition de stabilité `cT < 2` (D2) et l'inégalité d'inversion de FSRS
  peuvent changer si l'exploitant fournit une configuration différente.
* **La conformité numérique à FSRS-5 n'a pas été testée** : le document constate l'ordre des
  facteurs de note tel qu'implémenté et l'incohérence entre commentaire et usage ; il
  n'affirme pas que les 19 poids sont faux.
* P1 n'est pas démontré empiriquement : le test 3 est précisément là pour pouvoir la
  réfuter.
