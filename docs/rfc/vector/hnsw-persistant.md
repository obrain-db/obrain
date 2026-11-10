# RFC — Index vectoriel persistant à dimension native (HNSW sous substrat)

**Statut** : Brouillon (draft) — à valider avant tout code.
**Auteur** : session obrain (rédigé avec l'assistant, relu par le mainteneur).
**Date** : 2026-11-10.
**Cible** : obrain v0.0.2 (pas v0.0.1).
**Porte** : voir « Critères d'acceptation ».

## Problème

Le substrat (`obrain-substrate`) ne sait pas indexer des vecteurs à dimension
native :

- Le niveau L2 du retrieval par paliers est fixé à **384 dimensions**
  (`obrain_substrate::L2_DIM`, `crates/obrain-substrate/src/tiers.rs`).
- `SubstrateTieredIndex::new` **panique** si la dimension diffère de `L2_DIM`.
- `create_vector_index` **refuse** toute dimension supérieure à 384
  (`crates/obrain-engine/src/database/index.rs`, lignes 205 à 241).

Le cas d'usage qui le révèle : PO (orchestrateur) indexe des embeddings E5
multilingues de **768 dimensions** (notes, fichiers, fonctions) pour le
retrieval du chat. Obrain ne peut pas les accueillir sans projection.

Mesure (harnais `vecbench`, données réelles de PO, 8 131 notes et 24 390
fichiers, 768-d, 200 requêtes, vérité terrain par force brute ; contrôle
brute-contre-brute = 1,0000) :

| N | recall@10 obrain (projection 384-d) | recall@10 Neo4j |
|---|---|---|
| 8 131 (notes) | 0,630 | 0,9365 |
| 24 390 (fichiers) | 0,767 | 0,9510 |

**Ce que cette mesure ne prouve pas** : la latence à recall égal (le cascade
n'atteint pas le recall de Neo4j, donc aucune comparaison de vitesse n'est
légitime). Le 0,365 cité dans `docs/LIMITES-v0.0.1.md` provient de vecteurs
aléatoires et n'est pas représentatif.

### Ce qui existe déjà (vérifié dans `origin/main` d7d2a669)

- `obrain-core` possède un `HnswIndex` (`crates/obrain-core/src/index/vector/hnsw.rs`).
- Sa sérialisation plate existe et est testée en aller-retour :
  `export_flat` (ligne 1259), `from_flat` (1321), `from_flat_raw` (1402).
- **Aucun appelant hors de `hnsw.rs` ne l'utilise** sous le substrat. Il
  n'existe donc pas aujourd'hui de persistance de cet index sous le substrat.

## Proposition

Rendre l'index **dérivé**, pas journalisé.

1. **Source de vérité** : les vecteurs restent des propriétés `Value::Vector`
   journalisées dans le WAL, comme aujourd'hui. Aucun nouvel opcode WAL.
2. **Index = cache reconstructible** : le graphe HNSW est sauvegardé en
   instantané (`export_flat`) au checkpoint.
3. **Ouverture** : on recharge l'instantané puis on réinsère les vecteurs
   écrits après son horodatage (repérés par le LSN du checkpoint).
4. **Repli** : si l'instantané est absent ou ne passe pas sa somme de contrôle,
   on reconstruit l'index depuis les propriétés. Pire cas : une ouverture lente,
   jamais une perte ni une corruption.
5. **Suppressions** : pierres tombales filtrées à la lecture, reconstruction
   périodique quand leur proportion dépasse un seuil.
6. **Dimension paramétrée** : la dimension est un paramètre de
   `create_vector_index` (768 et 1024 sont des cas de test), pas une constante
   de compilation. `L2_DIM` reste la largeur du niveau de paliers existant ; le
   nouvel index n'y est pas branché.

**Statut de chaque point** : (1), (2) et (4) sont **déduits** de
l'architecture existante, non prototypés. (3) suppose que le LSN du checkpoint
est disponible à l'ouverture : **à vérifier dans le code**.

## Alternatives

| Option | Description | Verdict |
|---|---|---|
| A. Réparer obrain (ce RFC) | HNSW persistant, dérivé, à dimension native | **Retenue**, sous réserve de la porte |
| B. Index vecteur local côté PO | candle + HNSW embarqué dans PO, obrain ne porte que nœuds, arêtes, propriétés | Rejetée : deux moteurs à sauvegarder et à garder cohérents ; PO perd le retrieval partagé avec obrain |
| C. Neo4j pour les vecteurs seulement | garde la dépendance lourde | Rejetée : annule la simplification qui motive la migration |
| D. Projeter à 384-d et garder le cascade | aucun code nouveau | Rejetée : recall 0,63 mesuré, 3 résultats sur 10 faux |

## Impact

- **Fichiers touchés (estimation, non exhaustive)** : `crates/obrain-core/src/index/vector/hnsw.rs`
  (branchement des primitives plates), `crates/obrain-engine/src/database/index.rs`
  (`create_vector_index` pour dimension quelconque), `crates/obrain-engine/src/database/search.rs`
  (`vector_search` sur l'index persistant), `crates/obrain-substrate` (écriture et
  relecture de l'instantané en zone).
- **Format** : un fichier d'instantané versionné ; un changement de format
  d'instantané n'impose **aucune** migration de données (il est reconstructible).
- **Rupture d'API** : aucune prévue. `vector_search` garde sa signature.
- **Performance** : la reconstruction est O(N log N) ; à mesurer (pas de chiffre
  avant la première prototype).
- **Dépendances** : aucune nouvelle.

## Critères d'acceptation (la porte)

Tous doivent passer pour fusionner et publier en v0.0.2 :

1. **Recall@10 ≥ 0,95** à **25 000 vecteurs en 768 dimensions**, mesuré
   contre force brute, sur des vecteurs **structurés synthétiques** (jeu
   reproductible, dans le dépôt).
2. **Survie à la réouverture** : un index construit, la base fermée puis
   rouverte, retourne le même classement top-10 sur un jeu de requêtes fixe.
3. **Repli** : instantané supprimé ou corrompu → reconstruction, résultats
   identiques à la sauvegarde (même classement).
4. **Dimension paramétrée** : mêmes tests à 384 et 768 dimensions.
5. **Latence** : p95 mesuré et publié, sans seuil imposé à ce stade (la
   comparaison à Neo4j n'est pas un critère : elle n'est valable qu'à recall égal).
6. **Juge sur données réelles** : `vecbench` (PO, données privées, **local
   seulement**) passe à 0,95 avant que PO ne bascule. Ce juge n'entre jamais
   dans le dépôt public.

## Plan de migration

1. Ce RFC validé.
2. Prototype derrière un drapeau de compilation, mesure du recall sur le jeu
   synthétique.
3. Critère 1 franchi → intégration complète, critères 2 à 5.
4. Publication v0.0.2 sur crates.io (après la porte de release de v0.0.1).
5. PO épingle la v0.0.2 et passe `vecbench` (critère 6) avant de basculer.

## Questions ouvertes

- Le LSN de checkpoint est-il lisible à l'ouverture sans scanner le WAL ?
  (conditionne le point 3 de la proposition.)
- Quelle proportion de pierres tombales déclenche la reconstruction ? (seuil à
  fixer par mesure.)
- Faut-il garder `HnswIndex` sur `LpgStore` (l'ancien chemin) ou le retirer ?
  Proposition : le laisser le temps de la v0.0.2, puis le retirer.
