# Proposition — chantier `obrain.search_by_role` (forme de recherche n°3)

**Date** : 2026-09-05 — branche `feat/zorder`, HEAD `e921f98c` — objectif « formes de
recherche » (goal 29a91d8c ; `search_delta` `c935b9d1`, `search_surprise` `e921f98c`).
**Statut** : v1 EXÉCUTÉE (round 4, commit du chantier) — HKS 3 échelles + clustering, tests 6/6, bench daté dans le ledger.

---

## 1. Réalité du code (reconnaissance) — les primitives EXISTENT déjà

`crates/obrain-substrate/src/geometric.rs` (Ricci-Ollivier + chaleur) fournit,
tout compilé et testé :
- **`compute_hks_descriptors(store) -> Vec<[f32; 3]>`** (l.1217) — descripteur HKS
  **3 échelles par nœud** `[h(t_local), h(t_meso), h(t_global)]` (diagonale du noyau
  de chaleur estimée par Hutchinson, CSR unique + 3 diffusions) ; tests présents
  (`hks_distinguishes_clique_from_tree`, décroissance monotone) ;
- **`hks_distance(a, b) -> f32`** (l.1233) — distance entre descripteurs ;
- **`node_curvature(store, node) -> Option<f32>`** (l.631) — courbure Ricci moyenne
  pondérée des arêtes incidentes (dépend d'une passe de refresh : κ par défaut 0) ;
- dérivés : `heat_kernel_signature`, `CsrAdjacency`, `compute_forman`, `bottleneck_nodes`.

Le descripteur HKS est **invariant au contenu par construction** : il ne dépend que
de l'adjacence (aucun label, aucune propriété). C'est exactement la « signature
spectrale×HKS×ricci invariante au contenu » de l'intention — avec HKS réel, et la
courbure Ricci disponible en enrichissement optionnel.

## 2. Sémantique proposée (v1)

`search_by_role` = « quels nœuds jouent le MÊME RÔLE STRUCTUREL dans des régions
différentes du graphe ? » — indépendamment de leur contenu :

- descripteur de rôle du nœud `u` : `D(u) = hks_3scale[u]` (3 flottants) ;
- enrichissement optionnel v1.1 : `+ log2(1+degré)`, `+ node_curvature(u)` quand
  la courbure est disponible (colonne ricci non nulle), normalisés — gardé hors du
  score de base pour rester déterministe sans passe de refresh (une CALL est LECTURE) ;
- similarité : `hks_distance(D(u), D(v))` (distance géométrique 3D, plus petite =
  même rôle) ; agrégation top-K ;

Surfaces :
- `ObrainDB::search_by_role(node_id, limit, label: Option<&str>) -> Vec<RoleMatch>`
  (`RoleMatch { node_id, distance }`) ;
- `CALL obrain.search_by_role(node_id, limit, label) YIELD node_id, distance`
  — label optionnel = restreindre les candidats à un label (usage « twin dans
  l'autre composant ») ; par défaut tous les nœuds vivants (slot ≥ 1) sauf la source.

Propriétés : LECTURE pure (CSR + diffusions, aucune mutation — pas de `refresh`
ricci, pas d'écriture), additif, `SUBSTRATE_FORMAT_VERSION` inchangé.

## 3. Coût (à mesurer au bench)

- CSR + 3 diffusions HKS = coût dominant, documenté dans le code :
  ~112 ms pour 10⁶ arêtes (80 pas × 1,4 ms) sur la machine de référence — calculé
  sur une snapshot CSR en mémoire, sans scan store répété ;
- comparer les N candidats : O(N) distances 3D ;
- borne honnête à mettre au bench sur les stores réels (code corpus ~10⁶ ; runtime
  ~0,3 M) et à consigner daté. Sur très grands graphes, l'option `label` permet de
  restreindre les candidats (doc).

## 4. DoD (8 cases)

1. Module `engine/src/database/search_by_role.rs` (recalcul HKS une fois par CALL,
   filtrage candidats, tri) — additif strict, aucune mutation, aucune écriture.
2. `CALL obrain.search_by_role(node_id, limit, label)` enregistrée + classée LECTURE
   (`PROCEDURES_DE_LECTURE`) ; exécutable en read-only ; erreur explicite : nœud
   inconnu/hors registre, limit > cap 10 000.
3. Tests : (a) **jumeaux structurels cross-contenu** — deux K5 identiques en forme
   mais labels/props différents → le hub du K5₁ retrouve le hub du K5₂ en tête avec
   `distance ≈ 0`, et l'écarte d'un centre d'étoile (la clustering augmente la distance de rôle) ; (b) CALL +
   tri + `label` (candidats restreints) ; (c) read-only ; (d) nœud invalide → erreur.
4. Suites : engine lib défaut **680/680**, cdc+algos **724/724**, substrate
   **803/803**, core **870/870** — aucune modif de test existant.
5. Golden diff ∅ (4 surfaces — procédure nouvelle seulement).
6. Bench daté : temps de `search_by_role` sur code corpus + runtime (CSR+HKS+top-K),
   borne documentée ; zéro impact chemins existants (aucun hook).
7. Commit unique réversible + chiffres datés dans le ledger.
8. Doc : proposition + section ledger ; pas de déploiement prod.

## 5. Risques / décisions

- R1 : HKS 3 échelles seule (sans ricci) suffit-elle à séparer les rôles ? Les tests
  (clique/tree déjà distingués) + jumeaux K5 le prouvent pour la v1 ; ricci = v1.1
  documenté (nécessite un refresh → hors LECTURE).
- R2 : coût HKS global par CALL — accepté/borné (112 ms/10⁶ de référence), `label`
  optionnel pour restreindre ; bench réel requis.
- R3 : multi-composantes : HKS est local/diffusif — deux régions déconnectées
  jumelles restent comparables (HKS dépend du voisinage, pas de la connectivité
  globale) — validé par le test jumeaux.

## 6. Ordre d'exécution (round suivant, après validation)

1. module engine (CSR+HKS via substrate, filtres, tri) → 2. enregistrement CALL +
  LECTURE → 3. tests (jumeaux cross-contenu, label, read-only, erreurs) → 4. suites +
  bench datés → 5. ledger + commit unique → 6. proposition `search_conflicts`.
