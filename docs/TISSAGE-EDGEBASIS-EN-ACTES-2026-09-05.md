# Tissage EdgeBasis^* — en actes sur obrain (mémoire de l'agent)

**Date** : 2026-09-05 — branche `feat/zorder` — statut : DÉMONTRÉ (test
`crates/obrain-engine/tests/weave_ontology_demo.rs`, 2/2), aucune extension
moteur nécessaire pour représenter le vocabulaire.

Référence du vocabulaire : le « tissage » EdgeBasis^* (forme d'arête à 16
attributs, poids vectoriel ω à 8 dimensions, types de sommets, ~25 familles de
relations, hyperarêtes H_*, relief R_q, anti-déformation, clôture ouverte).
Ce doc = la traduction obrain + mes règles d'usage (ce que je m'engage à
respecter quand je structure MA propre mémoire dans le graphe).

## 1. Ce que le tissage EST déjà sur obrain (vérifié)

| Tissage | En obrain | Comment |
|---|---|---|
| Registre ouvert ℛ_t | `EdgeTypeRegistry` dynamique + `obrain.relationshipTypes` | types d'arêtes créés à la volée, colonnes de degrés typées par type |
| Types de sommets | Labels multi | choix de vocabulaire |
| Attributs épistémiques / ω vectoriel | propriétés typées par clé + colonnes f32 (energy/weight/ricci) | conventions de clés |
| Réification (arête → nœud v_e) | arêtes normales entre nœuds | une arête devient nœud puis reçoit SUPPORTED_BY… |
| Hyperarêtes H_* | réification ; machinery simplex/DISPATCH présente | H_claim/H_measurement = nœuds réifiés + membres |
| Causalité / saillance / contradiction / jumeaux | `search_causes/search_effects`, `search_surprise`, `search_conflicts`, `search_by_role` | formes de recherche 2-5 livrées |
| Temps | CDC horodaté, `createdAt` | valid/observed = props (conventions) |
| Clôture ouverte | native (aucun schéma fermé) | `RELATION_CANDIDATE` = conventions |

## 2. Les formes = mes opérations de lecture (recettes)

Sur un store (le mien de session ou n'importe quel .obrain) :

- **Reprendre** : `obrain.search_delta(since_ms)` — ce qui a changé depuis ma
  dernière vue (re-ancre après coupure) ;
- **Quoi relire** : `search_surprise(since_ms, k)` — nœuds dont l'activité
  s'écarte de l'attente (≫ degré) ;
- **Méfiance** : `search_conflicts(since_ms, k)` — propriétés qui oscillent
  A→B→A (claims qui se démentent) ;
- **Cause/effet** : `search_causes(id)` / `search_effects(id)` — conducteurs
  amont / conséquences aval ;
- **Twin** : `search_by_role(id, k)` — même rôle structurel dans une autre
  région/variante (contenu différent).

Démo rejouable : `cargo test -p obrain-engine --features cdc,algos --test weave_ontology_demo`
(un claim + 3 preuves dont une contradictoire + flip A→B→A + chaîne causale +
nouveau hub en rafale + deux étoiles jumelles : chaque motif est détecté par la
forme qui lui correspond).

## 3. Mes règles d'anti-déformation (engagement)

- Ne jamais fusionner : `strength ≠ confidence ≠ truth` ; `frequency ≠
  importance` ; `centrality ≠ validity` ; `similarity ≠ identity` ;
- Pas de `RELATED_TO` final ; `SIMILAR_TO` exige sa métrique ;
  `CAUSES` exige provenance/contexte ; `SAME_AS` jamais sur simple ressemblance ;
- Propagation d'inférence (SUPPORTS→PROVES, ANALOGOUS_TO…) interdite sans règle
  enregistrée avec prémices et `confidence ≤ min(premises)` ;
- Statut épistémique explicite (`OBSERVED/ASSERTED/DERIVED/HYPOTHETICAL/
  CONTESTED/REFUTED`) ; `EMERGES_FROM` reste HYPOTHETICAL sans ses 6 champs ;
- Preuves : les répétitions d'une même source comptent une fois ;
  `IndependentEvidence = DistinctOrigin ∧ ¬DerivedFrom ∧ ¬SharedHiddenSource` ;
- Les signaux sans source honnête = **NULL explicite**, jamais une valeur
  inventée (v. relief §4).

## 4. Relief R_q(v,t) — ce qui reste à faire (candidat phase 2)

Le relief vectoriel (E,C,S,B,T,G,N,A,U,X,D) est la philosophie même des formes
livrées, mais les dimensions « force de preuve E », « but G », « actionnabilité
A », « dépendance/circularité D » n'ont pas encore de signal moteur dédié.
Extension candidate (proposée, non implémentée — pas de sémantique inventée) :
une procédure LECTURE `CALL obrain.relief(node, ctx, θ…)` projetant les signaux
EXISTANTS (énergie/fraîcheur, surprise N, conflits X, causalité C, degré/
centralité B, rôle/forme S) pondérés par θ_q, avec NULL explicite pour les
dimensions absentes et pic = max local de voisinage (Peak_q). Priorité basse :
la valeur démontrée aujourd'hui est dans les cinq formes + ce test.

## 5. Pourquoi c'est utile POUR MOI (agent)

Le graphe n'est pas qu'un stockage : avec ces formes je peux m'auto-structurer —
lire mes propres traces (delta), repérer ce qui détonne (surprise), ce qui se
contredit (conflicts), enchaîner les causes (causes/effects), retrouver mes
doubles structurels (by_role) — et consigner mes règles épistémiques pour que la
mémoire reste non déformée à travers les reprises.
