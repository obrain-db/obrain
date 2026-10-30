# Proposition — chantier `obrain.search_surprise` (forme de recherche n°2)

**Date** : 2026-09-05 — branche `feat/zorder`, HEAD `570fbba7` — objectif « formes de
recherche » (goal 29a91d8c ; `search_delta` livré `c935b9d1`).
**Statut** : v1 EXÉCUTÉE (round 2, commit du chantier) selon le pivot §7 — méthode + CALL + tests 4/4 + portes vertes, détail dans le ledger.

---

## 1. Réalité du code découverte à la reconnaissance (ancre la sémantique)

L'intention d'origine (« résidu activation_réelle − P_prédite, spread × Predictor/
TopicRing ») ne correspond pas à une mécanique existante telle quelle :
- `thinker/predictor.rs` `Predictor` + `TopicRing` = **préfetch** de communautés
  chaudes (madvise) — pas un modèle probabiliste d'activation ;
- la machinerie « prédiction vs activation » vit en réalité dans **l'engram
  memory** :
  - `EngramManager::record_activation(nodes, prediction_error: f64)` — le résidu
    est FOURNI par l'appelant à chaque activation ;
  - `engram/formation.rs` : `surprise_accumulator += prediction_error`, et la
    formation d'engrammes est **gatée par la surprise** (`min_prediction_error`) —
  c'est déjà « l'inattendu qui devient mémoire » ;
  - `Engram` persiste : `strength`, `valence`, `precision`, `recall_count`,
    `recall_history`, `last_activated`, `fsrs_state` (prédiction FSRS de
    rappel), `predictive_model` (hopfield) ;
  - procédures existantes type : `obrain.engrams.list/inspect` (engine
    `cognitive_procedures.rs`), classées LECTURE via la liste d'autorisation de
    `query/plan.rs`.
- Gabarit d'intégration procédure additive LECTURE (miroir `search_delta`) :
  module `engine/src/database/search_surprise.rs` → dispatch dans
  `query/planner/lpg/mutation.rs` (`try_execute_*_procedure`) → autorisation
  read-only dans `query/plan.rs` → tests `engine/tests/search_surprise.rs`.

## 2. Sémantique proposée (v1 recommandée)

`obrain.search_surprise` = **« ce qui a surpris le modèle »** : les engrammes
(et leurs nœuds membres) dont l'activation récente s'écarte le plus de ce que
le modèle de rappel prédisait — un classement par résidu d'erreur de prédiction,
perspective interne de l'IA (complémentaire de `search_delta` qui est la vue
externe « ce qui a changé »).

Score de surprise par engramme (lecture seule de l'état stocké) :
- `actual` = force d'activation récente : `strength` (décroissance/décay) ou
  taux d'activation récent (`recall_history` dans un horizon), selon la mesure
  la plus fiable disponible ;
- `predicted` = probabilité de rappel prédite par l'état FSRS (`fsrs_state`) ou
  `precision` (moyenne historique) ;
- `surprise = actual − predicted` (signé) ; le classement expose les deux
  extrêmes : activé bien plus que prédit (positif) et bien moins (négatif —
  oubli inattendu).
- Sortie par nœud déduite de l'ensemble des engrammes le contenant (agrégation
  max/moyenne sur les membres, idem recall/weave existants).

Signature CALL (additive, LECTURE, feature-gatée) :
```gql
CALL obrain.search_surprise(limit: 20, signed: true)
YIELD node_id, engram_id, actual, predicted, surprise
ORDER BY abs(surprise) DESC
```
Paramètres optionnels : `limit` (défaut 20, cap 10 000), `signed` (true = les
deux extrêmes, false = |résidu| seulement).

Périmètre d'exécution : le moteur cognitif ARMÉ (feature `cognitive-engram`,
même prérequis que `obrain.engrams.*`) ; sur un moteur sans engrammes, retour
vide explicite (pas d'erreur).

## 3. DoD (8 cases)

1. Module `engine/src/database/search_surprise.rs` (+ score dans
   `obrain-cognitive` si le calcul y vit : fonction pure sur `Engram`/store) —
   additif strict, aucune mutation.
2. Procédure `CALL obrain.search_surprise(...)` enregistrée + classée LECTURE
   (liste d'autorisation `query/plan.rs`) ; exécutable en session read-only.
3. Tests de non-régression : méthode moteur + CALL (tri par |résidu|, signe,
   agrégation nœud, moteur vide → vide, read-only) — ≥ 4 tests, gabarit
   `search_delta` (feature `cdc`?? non — feature cognitive-engram/algos).
4. Suites : engine lib défaut + cdc+algos + cognitive ; substrate 802 ;
   core 870 — toutes vertes, aucun test existant modifié.
5. Golden diff ∅ (4 surfaces GQL/NAPI/CLI/bridge inchangées — procédure
   nouvelle seulement).
6. Bench daté : requête surprise sur un moteur synthétique (N engrammes) et
   sur runtime réel — coût ≤ coût de `engrams.list`-like (lecture seule de
   l'état mémoire) ; zéro latence sur les chemins existants (aucun hook chaud).
7. Un commit réversible (un par recherche), chiffres datés dans le ledger.
8. Doc : section user-guide + ledger (mesures) ; aucun déploiement prod.

## 4. Bench (plan)

- Synthétique : `record_activation` avec erreurs de prédiction contrôlées
  (graines) → formation d'engrammes → top-K surprise = graines attendues
  (recall exact) ; timing de la procédure (µs/ms) avant/après (rien à comparer
  avant : procédure nouvelle — on borne le coût vs `engrams.list`).
- Réel : runtime dev (engrammes formés en prod selon commentaire formation :
  ~1 k/tick) — mesure datée.

## 5. Risques / questions ouvertes

- **R1 (sémantique)** : « actual » mesuré par `strength` courante vs taux
  d'activation récent — la valeur la plus honnête sera tranchée à
  l'implémentation sur les données réelles (les deux exposées si possible).
- **R2 (persistance)** : `surprise_accumulator` de formation est volatile
  (pré-engramme, en mémoire) — v1 porte sur l'état ENGRAmme persistant
  (fsrs/precision/strength), pas sur l'accumulateur éphémère ; documenté.
- **R3 (portée live)** : comme `obrain.engrams.*`, la procédure lit le moteur
  cognitif ARMÉ de la session (pas un scan de store froid) ; sur moteur non
  armé → vide explicite.
- **Alternatives écartées (documentées)** : (a) « surprise = activité CDC par
  nœud vs attente Poisson/degré » — testable sur store froid mais sans lien
  avec un modèle prédictif interne (moins fidèle à l'intention) ; (b) étendre
  `Predictor/TopicRing` en modèle probabiliste — chantier Thinker séparé.
  Option (a) reste disponible si la v1 engramme est jugée trop « interne ».

## 6. Ordre d'exécution (après validation)

1. Score pur (obrain-cognitive, testé) → 2. module engine + CALL + autorisation
   LECTURE → 3. tests méthode/CALL/read-only → 4. suites complètes + bench →
   5. ledger + doc + commit unique réversible → 6. proposition `search_by_role`.

---

## 7. PIVOT v1 (2026-09-05, décision d'implémentation fondée sur le code réel)

La reconnaissance fine a montré que la v1 « engramme » (FSRS `retention_probability`
/ `PredictiveModel` hopfield) est **creuse sur les données réelles** : la plupart
des engrammes n'ont jamais de `last_review`/`predictive_model` → la procédure
rendrait vide en prod (mauvaise recherche). `strength` est quasi-statique
(formation/reconsolidation) — pas un signal d'activation suivi.

⇒ **v1 implémentée = variante (a) de la proposition** : surprise d'ACTIVITÉ au
niveau graphe, complémentaire de `search_delta` (qui liste tout) — « les
changements qui contredisent une attente apprise » :
- **actual** : `activity(node)` = nb d'événements CDC le touchant dans la fenêtre
  `[since_ms, now]` (create/update du nœud ; chaque extrémité d'une arête) ;
- **predicted** : attente proportionnelle au degré courant du nœud dans le graphe
  (modèle nul : l'activité suit la connectivité) — `expected(node) =
  total_window × (deg(node)+1) / (Σdeg + N)` (lissage +1, Poisson) ;
- **surprise signée** : `(activity − expected) / sqrt(expected + ε)` (z approx.) —
  positif = s'active plus que son degré ne le prédit, négatif = s'efface ;
- agrégation par nœud (les arêtes comptent pour leurs deux extrémités).

Données : fenêtre CDC persistée (mêmes bornes/gabarit que `search_delta`,
feature `cdc`, LECTURE pure). Déterministe, testable sur graphe synthétique,
non-dégénérée sur stores runtime (toute activité récente produit un rang).

La v1 « engramme interne » reste documentée (annexe §2) comme variante future si
un jour les engrammes portent systématiquement un état FSRS suivi.
