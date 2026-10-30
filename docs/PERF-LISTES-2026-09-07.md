# Arc latence endpoints « list » — diagnostic & correctifs (2026-09-07)

**Contexte** : après redémarrage/montée, `list`, `session.list`, `workspace.list`,
`subagent.list`, `session.models`, `auth.status` semblaient lents malgré une base
« microsecondes ». Verdict : le moteur est µs ; la latence venait de l'épaisseur
(bridge + queries + fold JS + cycle de vie des process).

## Décomposition mesurée (runtime-dev, 227-260 sessions)
- count(n:Session) 0,03-0,06 ms (µs) ; SELECT qn seul 1,6 ms ; + 7 props GQL ~18 ms ;
- sessions_list GQL avec `OR … EXISTS {(:Message)-[:PART_OF]->(s)}` : ~27 ms ET
  OMETTAIT les 69 subagents avec messages (le bras EXISTS d'un OR n'était jamais
  évalué — faux silencieux) ;
- open store au (re)spawn bridge (auto-fermeture 5 s, respawn par requête) : ~11 ms ;
- build reverse-PART_OF (1er appel) : ~18 ms.

## Correctifs livrés (obrain, worktree propre)
- `sessions_list` : omission corrigée (210), OR-EXISTS supprimé du bridge (branche
  owner incluse) → lecteur 100 % substrate (batch props) → **mémo-cache writer**
  (invalidation à chaque import_events) : **0,6-1 ms/appel** (27→21→9→0,6 ms).
  Commits `6f9a8140` `8b428b9b` `a5926a9d` `7fe03110`.
- Moteur : `EXISTS` sous `OR` désormais **supporté** par UNION-DISTINCT des
  disjonctifs (chaque branche passe par semi/anti-join) — réel : 226 = 157+69.
  Commit `b9e5ebbb` (suite de la garde `a83ca001`). `NOT EXISTS` (AND) déjà OK.
- Dette documentée/tests : `planner_exists_under_or.rs` (OR-EXISTS union dédup,
  NOT-EXISTS).

## Déploiement (binaire partagé, swaps atomiques avec backup)
- Installé sur `~/.local/share/obrain-harness/bin/obrain` (hash `3ad1c768`) ;
  backups : `obrain.bak-sessionsfix-20260907-095735`, `obrain.bak-orexists-20260907-100925`.
- Writer dev déjà sur le nouveau binaire (10:11) ; writer prod reprendra au respawn.

## Actions restantes (nécessitent un restart du service dev — à ton signal)
1. **Bridge chaud** : `OBRAIN_IDLE_MS=300000` ajouté à
   `harness-worktrees/graph-writer/scripts/run-dev-3083-obrain.sh` (commit
   `73017ea86d`) — évite open (~11 ms) + build reverse-map + perte du cache à
   chaque respawn ; actif au prochain boot du process.
2. Le rebuild/restart du service chargera aussi la source apiproxy actuelle :
   `session.list` y est déjà mémoïsé (`cachedSessionListMetadata`, WeakMap par
   `seq` — fold une fois par plage d'événements, cf. commentaire GC 63 %).
3. Profil fin des endpoints (auth.status/workspace.list/…) une fois le writer
   chaud, si un point reste > 5 ms.

## Formes de recherche (rappel objectif)
`search_delta`, `search_surprise`, `search_by_role`, `search_conflicts`,
`search_causality` (+ `search_relief`) : livrées, CALL LECTURE, tests verts
(conflicts 6, causality 6, by_role 7, surprise 4, delta 5 sous cdc+algos).

## Complément 2026-09-07 (15:40) — `session.models` 1,5 s : cause racine trouvée et corrigée

**Symptôme** : `session.models` ~1,5 s à CHAQUE appel (mesuré HTTP : 1,49-1,53 s, chaud
compris) ; `llm.models` (même catalogue) = 2 ms → le catalogue n'était pas en cause ;
`session.history` aussi ~1,5 s.

**Décomposition (bridge engine, tool `session_load`)**
- `session_load(events_only)` exécutait la requête messages
  `MATCH (m:Message)-[:PART_OF]->(s:Session {qn}) … ORDER BY m.seq` **inconditionnellement** ;
- ce pattern coûte **~1,45 s même sans résultat** (session inconnue mesurée à 1 452 ms ;
  requête isolée = 1 462 ms à résultat vide) — scan planifié sur les nœuds Message du
  store (ancre non poussée dans le plan, LIMIT non exploitable à vide) ;
- les consommateurs harness de `events_only` (readRequestHeader / readRecent / loadInternal,
  `session-persistence-graph`) ne lisent que `events` (journal) — jamais `messages`.

**Correctif (obrain)** : requête `messages` désormais exécutée uniquement hors
`events_only` (le nœud Session meta reste fourni, µs). Mesuré (bridge, runtime-dev) :
grande session events_only 4 000 : **1 535 ms → 75 ms** (froid) / 18 ms (chaud) ;
session inconnue : **~1 450 ms → 5 ms**. → `session.models`/`session.history` retombent
à ~20-80 ms par appel (cache header chaud ensuite).

**Résidu documenté (chantier planner moteur, hors périmètre de ce correctif)** : le
pattern `(m:Message)-[:PART_OF]->(s:Session {…}) ORDER BY …` reste ~1,4 s même à
résultat vide (ancre de session non poussée avant le scan/expand des Messages) — à
traiter côté planner (push-down de l'ancre + early-exit) pour les chemins non-events
(loadByNodes/history graphe).

## Déploiement 2026-09-07 (10:46) — correctifs session_load actifs sur le binaire partagé

- Swap `~/.local/share/obrain-harness/bin/obrain` → `f15cee63` (commits `c13aedb3`
  + `3d8553c4`) ; backup `obrain.bak-sessionmodels-20260907-104631`.
- Vérifié sur le store runtime-dev (nouveau binaire) : `session_load` events_only
  grande session (limite 1000) **45 ms** (incl. open 11 ms) ; session inconnue
  **4,6 ms** ; requête messages forme ancre-d'abord **17 ms**.
- Les bridges RO (session_load) respawnent avec le nouveau binaire (idle 5 s) —
  pas de restart requis pour la partie moteur. Le writer (import) n'est pas
  concerné par ce correctif.
- Côté harness : `readRequestHeader` page 4000→1000 commité (`38da1b7163`, worktree
  graph-writer) — actif au prochain (re)chargement du service web (hoste cette
  session → restart à ton signal).
- Suite possible « µs à froid » : mémo du catalogue `buildModelCatalog` (clé config,
  invalidation) — non implémenté (risque de fraîcheur/acceptabilité) ; à décider.
