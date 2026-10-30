# Capacités agent — « ce que je peux faire, branché à obrain »

**Date** : 2026-09-05 — branche `feat/zorder`, HEAD `85a44d98` (worktree propre).
**But** : inventaire, à destination de l'agent (et de toute reprise), de CE QUE
LE MOTEUR OBTRAIN PEUT FAIRE pour moi — formes de recherche, comparaison
multi-variantes, ingestion, écriture/observation, continuité, vérification.
Sources : `docs/ETAT-2026-09-05-2015.md`, `docs/ledger-ameliorations-2026-09-04.md`,
`docs/GARANTIES-NON-REGRESSION.md`, propositions `docs/PROPOSAL-*`, code engine.

---

## 1. Ré-ancre en 30 s (après perte de contexte)

0. **PERDU ? Lis d'abord `docs/RECONSTRUCTION-AGENT-2026-09-05.md`** (protocole L0/L1/L2).
1. `git log --oneline -30 && git status` — HEAD actuel, propre attendu.
2. `docs/ETAT-2026-09-05-2015.md` (état réel consolidé + procédure §8) puis ce fichier.
3. Goals de session du jour : formes de recherche **5/5 complete** ; comparatif
   multi-variantes **complete** ; les chantiers de cette session (degrés
   typés `6678330c`, CDC lazy `c29ce045`, surprise `e921f98c`) sont ancêtres du HEAD.

## 2. INTERROGER le graphe (lecture seule — CALL / méthodes engine)

Les 5 formes de recherche (module + `CALL obrain.*` classée LECTURE, exécutable
en read-only) — à utiliser selon le besoin agent :

| Forme | `CALL` | Usage agent type |
|---|---|---|
| diff CDC | `obrain.search_delta(since_ms, only_nodes, only_edges)` YIELD entity_id, kind, timestamp | **re-contextualisation** : « ce qui a changé depuis ma dernière vue » (rep rendre après coupure) |
| surprise d'activité | `obrain.search_surprise(since_ms, limit, signed)` YIELD node_id, activity, degree, expected, surprise | **saillance** : quels nœuds s'activent ≪/≫ leur degré ne le prédit (quoi relire/scruter) |
| rôle structurel | `obrain.search_by_role(node_id, limit, label?)` YIELD node_id, distance | **jumeaux cross-contenu** (HKS 3 échelles + re-rank clustering) : même rôle dans une autre variante/branche |
| dissonance | `obrain.search_conflicts(since_ms, limit)` YIELD node_id, property, reversals | **contradictions** : valeur qui oscille A→B→A (claims qui se démentent) |
| causalité dirigée | `obrain.search_causes(node_id, limit, depth)` / `search_effects(...)` YIELD node_id, level, score | **influence** : conducteurs amont / conséquences aval (BFS directionnel amorti γ=0,5) |
| relief contextuel | `obrain.relief(since_ms, limit)` YIELD node_id, relief, novelty, recency, centrality, conflict | **projection R_q** du tissage §XXIII sur signaux existants (N=surprise, T=fraîcheur CDC, B=degré, X=conflits en pénalité) ; dims E/S/C/G/A/U/D = NULL documenté |

Autres LECTURES moteur : fenêtres 2D (`obrain.window_2d[_energy]`, hubs par
communauté/Z), procédures cognitives (`obrain.cognitive.metrics`,
`obrain.engrams.list/inspect`), `obrain.procedures()` (catalogue), schéma
`obrain.labels / relationshipTypes / propertyKeys`, GQL complet
(MATCH/OPTIONAL/agrégats/CALL…).

## 3. ANALYSER les variantes (comparaison multi-bases)

- `obrain compare <A.obrain> <B.obrain>` — diff STRUCTUREL entre variantes
  d'un même dépôt (branches/worktrees) : identité par `(label, qualified_name)`,
  `--prefix-a/--prefix-b` pour normaliser le préfixe projet CBM ; modes
  (structurel rapide ms, complet), sémantique ENSEMBLE (dédup arêtes
  parallèles) + Jaccard sur uniques, résultat MÉMORISÉ par paire (rejeu 0 ms,
  invalidation par mtime) — démo réelle 2 worktrees d'obrain : Jaccard nœuds
  0,888 / arêtes 0,921 en 493 ms. Chiffres : `docs/PROPOSAL-COMPARATIF-MULTI-VARIANTES-2026-09-05.md`.
- `obrain enrich --ricci` : courbure persistée post-import — **REVERT sans
  gain** (`b37e264d`, écritures WAL dominantes) : documenté, `node_curvature`
  reste disponible quand la courbure est rafraîchie.

## 4. INGESTER & suivre l'historique (Rust, pas de JS)

- `obrain journal-import <…>` : journal d'import (manifest + diff →
  `substrate.import-journal`) — capture des changements A/D d'un dépôt, diff
  incrémental, compare post-import ~23 ms sans rescan. RFC `e07eb047`.
- `obrain native-import` : ingestion zone-direct d'un corpus binaire.
- Bridge live : `import_events` (GraphWriter → substrate) + **CDC×GQL**
  (`be1592e4`) : les créations passant par le chemin GQL/executor sont aussi
  observées (boucle mémoire agent complète).
- `repair-degrees` (CLI) + **auto-guérison moteur** des colonnes de degrés
  typées (`6678330c`) : plus jamais de 0 silencieux sur `MATCH (a)-[:T]->(b)`.

## 5. ÉCRIRE / MUTER

- GQL : CREATE/MERGE/SET/DELETE via session (le writer bridge = `import_events`
  MERGE — mêmes chemins), binding NAPI (`createNode/createEdge`, prop vectorielles),
  `obrain shell`, `obrain query`.
- Contrainte partagée : **un seul writer OBRAIN_WRITE=1 par store** (WAL
  sérialisé) ; lecteurs RO partagés qui rejouent le WAL à l'ouverture.

## 6. OPS & continuité

- `obrain continuity [--stale-after-min]` : sentinelle — exit 0 si le journal
  est récent, 1 si le store est FIGÉ (écrivain absent) ; JSON stable (timer ops).
- Sauvegarde/rollback : `backup` (+ manifeste de déploiement avec sha),
  `wal` (replay/truncate), `validate`, `checkpoint`/`compact`.
- État déploiement 2026-09-05 : **PROD dsh-web** sur binaire validé
  (continuity C3, tag `v0.0.1-zorder-2026-09-05-continuity`, manifeste +
  rollback) ; **DEV (R6) différé** — ne pas redémarrer le service qui héberge
  la session ; aletheia n2 injoignable (à re-tester).

## 7. VÉRIFIER (non-régression, rejouable)

- Rapide : `cargo test -p obrain-core --lib` · `-p obrain-substrate --lib`
  · `-p obrain-engine --lib` · `-p obrain-cli --bin obrain` (compteurs de
  référence : engine 680 · cdc+algos 724 · core 870 · substrate ~804).
- Surfaces : `cargo test -p obrain-engine --tests --features cdc,algos`
  (formes search_*, fenêtres, CDC) ; inventaire complet :
  `cargo test -p obrain-engine --tests --no-fail-fast` (+ `--features cdc,algos,ai`).
- Filets d'exactitude (oracles indépendants) : compare structurel==complet
  (40 graines), search_conflicts (30), search_surprise (10, égalité <1e-6),
  fenêtres 2D zruns==scan==oracle (250 requêtes), parités de caches.
- Bench : `scripts/bench-verify/verify-imports-compare.sh` + verify-o1/engine.

## 8. Bonnes pratiques (héritées des chantiers)

- Un chantier = proposition (design + DoD 8 cases) → tests → suites → bench
  daté → ledger → **un commit réversible** ; format `SUBSTRATE_FORMAT_VERSION`
  inchangé ; additif strict ; pas de déploiement prod sans demande ; logique
  en RUST (CLI/moteur), pas de scripts JS.
- Règle de ce dépôt : toute mesure datée/rejouable, portes rejouées avant
  « done », aucun écart/bug laissé (inventaire 0 rouge, cf. GARANTIES).
