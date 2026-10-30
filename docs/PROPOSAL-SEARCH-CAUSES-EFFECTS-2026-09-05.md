# Proposition — chantier `obrain.search_causes` / `obrain.search_effects` (forme de recherche n°5)

**Date** : 2026-09-05 — branche `feat/zorder`, HEAD `98861a47` — objectif « formes de
recherche » (goal 29a91d8c ; livrés : search_delta `c935b9d1`, search_surprise
`e921f98c`, search_by_role `2bfb03bd`, search_conflicts `98861a47`).
**Statut** : v1 EXÉCUTÉE (round 8 — clôture 5/5, commit du chantier) — tests 6/6, bench daté dans le ledger.

---

## 1. Réalité du code (reconnaissance)

- Arêtes **dirigées** (src→dst) par type : `GraphStore::edges_from(node,
  Direction::{Outgoing|Incoming})` (core traits.rs:159) — les deux sens sont
  lisibles en lecture pure ; `SubstrateStore::writer()` permet de lire
  poids/`is_tombstoned` si besoin (non requis en v1 : poids uniforme).
- Pas de modèle causal formel existant ; le « spread directionnel causal »
  s'appuie donc sur la topologie dirigée : **causes** = propagation AMONT le
  long des arêtes entrantes (BFS inverse) ; **effets** = propagation AVAL le
  long des arêtes sortantes (BFS avant).
- Aucune ontologie temporelle requise pour la v1 (gating temporel CDC =
  extension v1.1 documentée).

## 2. Sémantique proposée (v1)

Propagation d'influence amortie sur le graphe dirigé, déterministe et LECTURE :

- `search_effects(node_id, limit, depth=3)` : à partir de l'ancre, BFS avant
  (arêtes sortantes) ; chaque nœud découvert au niveau `ℓ` reçoit un score
  `score(v) = Σ_{u déjà scoré, u→v} score(u) × γ`, γ = 0,5 (amortissement par
  niveau ; ancre = 1.0) — un nœud atteignable par plusieurs chemins s'accumule.
- `search_causes(node_id, limit, depth=3)` : idem en BFS **inverse** (arêtes
  entrantes) — les conducteurs amont.
- Visited multi-niveaux borné (frontière plafonnée, `limit` + cap d'expansion)
  → terminaison garantie même sur graphes cycliques ; exclusion de l'ancre.
- Sortie `(node_id, level, score)` triée par score décroissant.

Lecture pure substrate (BFS sur `edges_from`), additif, pas d'écriture.

Surfaces :
- `ObrainDB::search_causes(node_id, limit, depth)` / `search_effects(...)`
  → `Vec<CauseRow { node_id, level, score }>` ;
- `CALL obrain.search_causes(node_id, limit, depth)` et
  `CALL obrain.search_effects(node_id, limit, depth)` YIELD node_id, level,
  score — procédures GQL classées LECTURE.

## 3. Coût

BFS borné : O(frontière cumulée ≤ cap × depth), chaque niveau lève des
`edges_from` par nœud — typiquement bien sous un scan complet ; borne
documentée au bench. Aucun hook sur les chemins existants.

## 4. DoD (8 cases)

1. Module `engine/src/database/search_causality.rs` (méthodes + 2 procédures) —
   additif strict, aucune mutation.
2. `CALL obrain.search_causes/search_effects` enregistrées + classées LECTURE ;
   read-only OK ; erreurs : nœud hors registre/isolé (rendu vide ou erreur
   documentée), depth plafonné (≤ 6), limit cap 10 000.
3. Tests : (a) chaîne A→B→C : `effects(A)` = B(ℓ1) puis C(ℓ2), scores décroissants ;
   (b) `causes(C)` = B puis A (directionnalité vérifiée : effects(A) ≠ causes(A)) ;
   (c) étoile : effets du centre = feuilles à score égal amorti ; (d) cycle →
   terminaison (visité, pas de boucle infinie) ; (e) depth/limit ; (f) CALL + read-only ;
   (g) nœud invalide.
4. Suites : engine 680/724, substrate 803, core 870, search_* précédentes vertes.
5. Golden diff ∅.
6. Bench daté : procédure sur copie runtime (depth 2-3) — coût mesuré, frontière
   cumulée rapportée.
7. Commit unique réversible + ledger.
8. Doc (proposition § statut) + ledger ; pas de déploiement prod.

## 5. Risques / limites (documentées)

- R1 : causalité STRUCTURELLE dirigée (graphe) avec amortissement — PAS une
  inférence causale temporelle formelle ; le gating temporel CDC (cause
  antérieure à l'effet) est une extension v1.1 si demandée.
- R2 : arêtes multi-types confondues en v1 (poids uniforme par arête) — un
  filtre `edge_types` peut être ajouté en v1.1.
- R3 : cycles → amortissement multi-niveaux borné (pas de renforcement en
  boucle), comportement déterministe.

## 6. Ordre d'exécution (round suivant — clôture)

1. module engine (BFS avant/inverse amorti) → 2. enregistrements CALL + LECTURE →
3. tests → 4. suites + bench daté → 5. ledger + commit → 6. bilan final 5/5 +
   clôture du goal 29a91d8c.
