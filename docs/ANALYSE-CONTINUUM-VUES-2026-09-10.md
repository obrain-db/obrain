# Analyse — continuum des vues et exigence « requêtes en ns »

Date : 2026-09-10 · Statut : **analyse**, décisions à prendre avec l'utilisateur

Proposition analysée : « la vue Cosmos doit avoir des boules de particules comme
la vue Monde ; la vue Monde est INTÉGRÉE au Cosmos (un zoom de caméra centré sur
le monde regardé) ; double-clic dans le monde → vue Graphe ; au bout de la vue
Graphe → les espèces de vecteurs / nœuds de vecteurs. Navigation pseudo-logique
de composition des vues. » + « toutes les requêtes doivent passer en ns ».

---

## 1 · Ce qui est en place (relevé, pas souvenir)

Quatre moteurs de rendu, **dix-huit projections au total** — bien plus que les
trois boutons de vue ne le laissent croire :

| Niveau | Composant | Projections réellement codées |
|---|---|---|
| Cosmos | `graph/CosmosWeave.tsx` | système solaire (foyer contextuel, orbites képlériennes, nappe d'affaissement, toile inter-mondes densifiée) |
| Atlas | `components/StageContinu.tsx` | `cosmos` / `monde` / `graphe` — **le même substrat, trois formes** |
| Monde | `components/StoreOverview.tsx` | `projets` / `modules` / `familles`, avec profondeur `crumb` et seuil θ commutable |
| Graphe | `graph/Graph3D.tsx` | 8 couches : `edges`, `tri` K3, `k4`, `hyper`, `contour` ∂, `spectral`, `synapse`, `labels` |
| Signaux (serveur) | `obrain_viz::Overlay` | 7 overlays : `ricci`, `energy`, `roles`, `fsrs`, `engrams`, `pheromone`, `tier` |

Autrement dit : **les projections existent déjà presque toutes**. Ce qui manque
n'est pas une projection de plus, c'est le **continu** entre elles — aujourd'hui
ce sont des boutons, alors que la proposition en fait des distances de caméra.

## 2 · Ce que la proposition change, concrètement

| Proposition | État actuel | Écart réel |
|---|---|---|
| Cosmos en **boules de particules** | Cosmos = sphères pleines + points pour les fibres ; Monde = nuage de particules par type | **Petit et payant** : c'est le même objet visuel aux deux niveaux, donc la transition cesse d'être un changement de nature |
| Monde **intégré** au Cosmos (zoom + centrage) | Deux composants distincts, deux scènes, deux caméras, deux états | **C'est le vrai chantier** : fusion des scènes (une seule scène, deux échelles) |
| Double-clic dans le monde → **Graphe** | StoreOverview a déjà `onDrill` (double-clic) mais vers un sous-espace, pas vers Graph3D | **Moyen** : recâbler la cible du double-clic sur l'échelle suivante |
| Bout du Graphe → **vecteurs / nœuds de vecteurs** | Le Graphe montre des nœuds et des arêtes ; les **signaux** (energy, engrams, ricci, tier) existent en surcouche de couleur seulement | **Moyen** : faire de l'espace des signaux un ÉTAGE, pas une coloration |
| « Navigation pseudo-logique » | 3 boutons + un toggle `morph` | **Petit** : déjà décrit dans `ADR-STAGE-CONTINU` (« les échelles ne sont pas des pages : ce sont des DISTANCES de caméra ») — jamais réalisé jusqu'au bout |

## 3 · Les projections que la proposition oublie (et qui fluidifieraient)

Relevées dans le code et les notes de conception, elles existent mais ne sont
pas des étages :

1. **L'axe du temps** (`docs/viz-projection-densite.md` §2.3). Les stores
   cognitifs ont un sens chronologique réel (`Session.createdAt`, 605 sessions
   mesurées). Aujourd'hui **aucune** vue ne projette le temps : c'est pourtant
   la seule projection qui explique « pourquoi ce graphe est comme ça ». À
   l'échelle la plus profonde, l'ordre chronologique serait plus lisible que la
   position topologique.
2. **L'espace des signaux comme étage, pas comme couleur** : `energy` (décroit
   avec le temps), `engrams`, `ricci` (bords structurels), `pheromone`, `tier`,
   `fsrs`, `roles`. Sept dimensions déjà calculées par le moteur, aplaties en
   teintes. C'est le candidat le plus direct aux « nœuds de vecteurs » : chaque
   nœud PORTE réellement un vecteur de signaux.
3. **L'espace vectoriel sémantique** (HNSW). Le moteur sait faire de la
   recherche vectorielle ; la viz ne la montre jamais. C'est la seule projection
   qui répondrait à « qu'est-ce qui RESSEMBLE à ceci » plutôt que « qu'est-ce
   qui est relié à ceci ».
4. **Les rôles structurels** (articulations, SCC) : le moteur les expose
   (`structural_roles`), le graphe les colorie sans les projeter.
5. **La toile inter-mondes** (le tissage) : c'est la seule projection qui relie
   les échelles — sans elle, « toutes les faces d'une même pièce » n'a pas de
   charpente visible entre les mondes.

**Proposition de continuum à six étages**, chacun étant une distance de caméra
et non une page :

```
1. Toile        les 6 mondes + leurs fibres de preuve (existe)
2. Monde        une boule de particules = une base (particules à porter depuis
                StoreOverview ; zoom+centrage pour la rejoindre)
3. Organisation projets / modules / familles (existe : StoreOverview)
4. Graphe       nœuds + arêtes + K3/K4/hyper/∂ (existe : Graph3D)
5. Signaux      chaque nœud projette son VECTEUR de signaux
                (energy, engrams, ricci, tier, pheromone) — à créer
6. Sémantique   similarité vectorielle HNSW, ou axe du temps pour les stores
                cognitifs — à créer
```

Les étages 1→4 existent déjà ; il manque leur **continuité** (une seule scène,
un seul état, des distances) et les étages 5→6.

## 4 · « Toutes les requêtes en ns » — mesuré, et ce que ça implique

Mesures du **temps moteur** (hors HTTP), sur `runtime-dev` (357 732 nœuds,
1 170 210 arêtes) et `engine_idx` (6 587 nœuds) :

| Opération | Latence moteur | Nature |
|---|---|---|
| `count(r)` toutes arêtes | **32 µs** | compteur maintenu |
| `count(n)` tous nœuds, 357 k | **49 µs** | compteur maintenu |
| `count(n:Session)` | **51 µs** | compteur par label |
| `count(n:Message)` | **100 µs** | compteur par label |
| lookup par `qualified_name` | **159 µs** | index |
| lire 10 `qualified_name` | **168 µs** | lecture |
| lire 10 nœuds | **318 µs** | lecture |
| `count(r:PROVES)` | **26 ms** | **scan** (pas de compteur par type !) |
| GROUP BY label (10 groupes) | **152 ms** | **scan complet** |
| GROUP BY type d'arête (5 groupes) | **2 099 ms** | **scan complet** (2,3 M arêtes) |

**Conclusion honnête : rien n'est en ns aujourd'hui.** Le plancher mesuré est
**26 300 ns** (26 µs) — déjà 4 ordres de grandeur au-dessus de la nanoseconde —
et les requêtes dont la viz dépend pour se dessiner sont à **152 ms et 2,1 s**,
soit 8 à 9 ordres.

Mais la mesure dit aussi **pourquoi**, et c'est réparable :

- ce qui est **compté** est en µs : le moteur tient des compteurs (nœuds,
  arêtes, nœuds par label) ;
- ce qui est **agrégé** est en ms/s : le planificateur **scanne** au lieu de
  lire les compteurs. `count(r:PROVES)` = 26 ms contre `count(r)` = 32 µs la
  même milliseconde **prouve** qu'il n'existe pas de compteur par type d'arête
  — alors que le substrate entretient un **registre de degrés typés**
  (`scan_typed_edge_weights`, hydraté à chaque `create_edge`) qui pourrait le
  fournir.

**Chemin réaliste vers le « ns », par ordre de rendement :**

1. **Compteur par type d'arête** → `count(r:T)` de 26 ms à ~µs ; le GROUP BY
   par type de 2,1 s à ~µs (nb de types × lecture de compteur).
2. **GROUP BY label servi par les compteurs par label** → 152 ms à ~µs
   (≤ 64 labels, un registre de bitset existe déjà).
3. **Cache d'agrégats durable par store** (déjà en place pour `modules`,
   `overview`, `arcs` : `disk_save`/`disk_load`) → première visite µs après
   échauffement, ce qui est déjà le cas.
4. La **nanoseconde au sens strict** (< 1 µs, soit ~30 cycles) n'est atteignable
   que pour des lectures de compteur en mémoire, pas pour une requête
   arbitraire : il faut le dire. L'objectif utile est **sub-microseconde sur les
   requêtes de viz** (compteurs + index), pas la ns sur toute requête.

## 5 · Ce que je propose de faire, dans l'ordre

1. **Boules de particules dans le Cosmos** (petit, visible, et c'est le
   préalable à la continuité : même primitive aux étages 1 et 2).
2. **Fusion Cosmos + Monde** : une seule scène, l'étage Monde étant une distance
   de caméra ; le double-clic descend d'un étage (Monde → Organisation → Graphe).
3. **Compteurs par type d'arête + GROUP BY servi par compteurs** : c'est le
   levier qui fait passer les requêtes de viz de la seconde à la microseconde.
   Prérequis de la fluidité : on ne peut pas naviguer en continu sur des
   requêtes à 2 s.
4. **Étage « Signaux »** : projeter le vecteur de signaux par nœud (energy,
   engrams, ricci, tier) — les données existent déjà côté moteur.
5. **Étage « Sémantique / temps »** : similarité vectorielle pour les stores de
   code, axe chronologique pour les stores cognitifs.

Les points 1 et 3 sont indépendants et immédiatement utiles ; le point 2 est le
chantier structurant ; les points 4 et 5 ouvrent la profondeur que tu décris.
