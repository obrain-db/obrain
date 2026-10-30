# Ce qu'apporte obrain — et usages de ce type de base

**Date** : 2026-09-05 — branche `feat/zorder`, HEAD `e7bfb9eb`. Document de
positionnement fondé sur les fonctionnalités RÉELLES du dépôt (commits,
procédures, benchs datés du ledger) — aucune promesse hors code.

---

## 1. Qu'est-ce qu'obrain (en une phrase)

Une **base de données graphe native en Rust** qui combine, sur un même
stockage mmap+WAL : le modèle LPG/GQL (nœuds/arêtes typées/propriétés), des
**compteurs et index O(1)**, des **signaux structurels et géométriques**
(degrés typés, fenêtres Z/2D, communautés, courbure de Ricci, signatures
spectrales/HKS, motifs topologiques), un **moteur cognitif-mémoire**
(engrammes, énergie, synapses, FSRS), une **capture de changement (CDC)** et
des **formes de recherche pour l'IA** — le tout additif, sans version de
format, avec ingestion native très rapide.

## 2. Les piliers concrets (avec preuves mesurées du ledger)

1. **Graphe natif typé & stockage durable**
   - Requêtes GQL/Cypher (`MATCH`, `CALL`), arêtes dirigées typées, props ;
   - zones mmap + WAL, verrous partagés RO / writer unique shared-write
     (lecture 0,15 ms pendant écrivain actif), compaction, replay, remap de
     slots Z-order, `SUBSTRATE_FORMAT_VERSION` stable (additif strict) ;
   - **ouverture ~0,07 s / 35 Mo** (warm) après correction CDC lazy.
2. **Lecture O(1) et index**
   - `nodeCountByLabel` ~0,13 µs, `edgeCountByType` ~0,18 µs, `count(n)`
     ~46 µs, lookup `qualified_name` ~43 µs ; index BM25/vector/hybrid
     (recall@10 = 0,9994, recall@100 = 1,0 sur 1 M).
3. **Relief structurel & géométrique**
   - colonnes de **degrés typées** par arête (pré-filtre d'expansion, auto-
     guérison des sidecars périmés — incident 09-01/09-05 clos) ;
   - **fenêtres 2D Z** (`CALL obrain.window_2d[_energy]` : hubs/énergie,
     zruns 80 µs vs scan 270 µs), tri spectral ;
   - **communautés** (`CALL obrain.louvain/leiden/label_propagation`),
     **courbure de Ricci** par arête persistée (bottlenecks, Forman),
     **HKS 3 échelles** par nœud (rôles structurels), triangles/cliques ;
   - jump 2-hop, fenêtres 2D par plages Z (localité Hilbert/Z).
4. **Moteur cognitif / mémoire**
   - engrammes formés par co-activation **gatée par la surprise**
     (`prediction_error`), FSRS (stabilité/difficulté), précision, spectral
     hopfield ; `obrain.engrams.list/inspect`, `cognitive.metrics`,
     `cognitive.spread` ; énergie & synapses persistées en colonnes.
5. **Changement (CDC) & journaux**
   - journal CDC persistant borné (fenêtre 100 k) : `search_delta` (diff
     graphe), historique par entité, « rien de nouveau » en O(1) ;
   - journaux de session en zones substrate (lecture par fenêtres),
     capture live multi-process (writer shared + lecteurs RO).
6. **Ingestion native très rapide**
   - **445 493 nœuds / 1 670 005 arêtes en 0,263 s** (wall 0,41 s, 334 Mo)
     vs kuzu LOAD 0,5 s pour le même corpus — QC exact, requêtes O(1)
     ensuite.
7. **Formes de recherche IA (5/5 livrées)**
   - `search_delta` (ce qui a changé), `search_surprise` (ce qui s'active
     contre les attentes), `search_by_role` (même rôle structurel ailleurs),
     `search_conflicts` (états qui se contredisent), `search_causes/
     search_effects` (spread directionnel causal) — toutes LECTURE pure,
     callables en read-only, additifs.

## 3. Positionnement (différenciation honnête)

| | GDB classiques (Neo4j/Memgraph…) | GDB embarquées (kuzu/…) | Bases vectorielles | obrain |
|---|---|---|---|---|
| Graphe + GQL | oui | oui | non | **oui (Rust natif, zones mmap)** |
| Ingestion massive | lente (CSV/HTTP) | rapide | n/a | **0,26 s / 1,67 M (natif)** |
| Lecture O(1)/index topo | partiel | partiel | non | oui (µs, label/type/typage) |
| Fenêtres spatiales Z/2D | non | non | non | **oui (hubs/énergie/plages Z)** |
| Géométrie (Ricci, HKS, roles) | add-ons | non | non | **natif (colonnes persistées)** |
| Communautés | algorithmes | partiel | non | oui (CALL leiden/louvain/lp) |
| CDC/audit | plugin | limité | non | **natif (fenêtre bornée)** |
| Mémoire cognitive (engrammes/FSRS/énergie) | non | non | approche | **intégré** |
| Recherches « IA » (delta/surprise/rôle/conflits/causalité) | non | non | RAG simple | **5 formes LECTURE** |

## 4. Usages de ce type de base

1. **Analyse et navigation de code à grande échelle** : graphe de symboles
   (File/Function/… + CALLS/IMPORTS), hubs par fenêtres, lookup O(1), rôles
   structurels (couplage par courbure/bottlenecks), recherche par rôle de
   jumeaux structurels entre modules/langages.
2. **Base pour IA agentique** : mémoire de session (messages, actions,
   `TOUCHES`/`PART_OF`), recontextualisation par diff (`search_delta`) et par
   surprise d'activité, engrammes (rappel cognitif par weave), signaux
   d'énergie/valence pour orienter les décisions.
3. **RAG structurel-sémantique** : hybridation BM25 + vecteurs + topologie
   (topo-rerank mesuré), formes spectrales ; pas seulement « le plus
   similaire » mais « le plus pertinent structurellement ».
4. **Audit & évolution** : « qu'est-ce qui a changé depuis ma dernière vue »,
   historique par entité, détection de dissonance d'états, causalité dirigée
   (causes/effets) sur le graphe d'activité.
5. **Analyse de réseaux multi-domaines** (dépendances, risques, couplage) :
   communautés, fenêtres 2D par communauté, courbure (bottlenecks), cliques
   topologiques.
6. **Stockage runtime d'applications** : capture live de flux d'événements
   en zones (journaux de session), lecteurs RO concurrents pendant un writer
   unique, restart/compaction sûrs — le tout dans un seul fichier
   d'empreinte maîtrisée (RSS 35 Mo à l'ouverture, gates mémoire vertes).

## 5. Limites assumées (pour rester honnête)

- CDC = fenêtre bornée (100 k) — pas un historique infini ;
- détection de conflits = temporelle d'état (pas d'analyse sémantique NLP) ;
- communautés persistées post-import = non (contiguïté à la création) —
   calcul à la demande via CALL ;
- « causalité » = structurelle dirigée amortie (pas d'inférence temporelle
   formelle) — extensions v1.1 documentées dans les propositions.
