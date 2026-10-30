# FULL OBRAIN — Migration : retrait intégral de codebase-memory-mcp (2026-09-08)

**Objectif** : sortir complètement `codebase-memory-mcp` (binaire C) du harness DSH et ne
laisser qu'**obrain** (moteur Rust), avec une intégration au minimum équivalente.
Ce document est le plan d'exécution + contrat de parité. Statut de chaque phase en §6.

---

## 1. État vérifié (2026-09-08, preuves mesurées)

| Surface | Aujourd'hui | Statut |
|---|---|---|
| Moteur / stockage / requêtes | obrain Rust (substrate, `.obrain` mmap). Le C ne linke aucune lib obrain, n'ouvre aucun store | ✅ remplacé |
| Canal `bridge` du harness (GraphWriter / GraphInjector / compaction / skill-graph → `spawn(bin, ['bridge'])`) | `CBM_BRIDGE_BIN=/…/obrain` → **obrain parle déjà le protocole `codebase-memory-mcp bridge`** (alias de `mcp`, cf. `obrain --help`) | ✅ remplacé |
| Serveur MCP d'outils agents (namespace `mcp__codebase-memory-mcp__*`, 32 outils) | binaire **C** bare, résolu via PATH : prod `~/.local/bin` (unit `dsh-web.service`), dev `cbm-worktrees/ingest/build/c` (unit `dsh-dev-3083.service`) | ❌ reste |
| Ingestion live | `codebase-memory-mcp --cbm-daemon-internal` + `obrain-live-sync.service` (python → `cli import_sessions`) + `cbm-runtime-sync.{service,timer}` | ❌ reste |
| Config / déploiement | env posées par les unités systemd (`99-obrain-harness.conf`, `46-obrain-write.conf` prod ; `dsh-dev-3083.service` dev) ; défauts `binaryPath ?? "codebase-memory-mcp"` dans les packages | ❌ reste |

**Env vérifiés** (unités systemd user) :- PROD `dsh-web.service` + drop-ins : `DSH_GRAPH_READ=1`, `DSH_GRAPH_WRITER=1`,
  `CBM_BRIDGE_BIN=/home/triviere/.local/share/obrain-harness/bin/obrain`,
  `OBRAIN_STORE=…/runtime-prod.obrain`, `CBM_GRAPH_PROJECT=dsh-runtime`, `OBRAIN_WRITE=1` (drop-in 46),
  `PATH` avec `~/.local/bin` en tête (→ tout spawn `codebase-memory-mcp` résout le C installé).
- DEV `dsh-dev-3083.service` : `PATH` avec `cbm-worktrees/ingest/build/c` en tête (→ build C du worktree),
  `DSH_GRAPH_READ=1`, `DSH_GRAPH_WRITER=1`, lanceur `scripts/run-dev-3083-obrain.sh`
  (pré-chauffage `obrain bridge` RO sur `runtime-dev.obrain`).

**Chronologie réelle (processus) :** chaque web (prod 3082, dev 3083) spawn :
`obrain bridge` (engine, long-lived) **et** `codebase-memory-mcp` bare (serveur MCP d'outils) ;
+ `--cbm-daemon-internal` (daemon d'écriture/ingestion, orphelin sous systemd) ;
+ enfants `cli import_sessions` de `obrain-live-sync.service`.

**Décision utilisateur** : « Pas CBM, on est full obrain » — ép. 98/98bis `aletheia/docs/decisions-log.md`,
marqueur `CBM_OBSOLETE` ; bascule prod full obrain-db le 2026-09-04
(`banc-obrain/docs/CHECKPOINT-ZORDER-2026-09-04.md`).

---

## 2. Contrat de parité — les 32 outils du namespace `mcp__codebase-memory-mcp__*`

Légende statut cible : **N** = déjà natif dans `obrain bridge` · **Q** = exprimable via `query`
(toutes syntaxes + CALL) ou wrapper fin Rust · **P** = portage nécessaire côté Rust ·
**I** = ingestion à nativer (CLI/bridge) · **X** = décision (abandonner ou ré-implémenter).

| # | Outil | Statut cible | Note |
|---|---|---|---|
| 1 | search_graph | N | bridge Rust `mcp_server.rs` |
| 2 | get_graph_schema | N | idem |
| 3 | context_build | N | idem |
| 4 | import_events | N | idem |
| 5 | runtime_search | N | idem |
| 6 | session_load | N | idem |
| 7 | sessions_list | N | idem |
| 8 | query_graph | Q | wrapper → `execute_language` (GQL/Cypher) |
| 9 | trace_path | Q | chemin multi-sauts : API moteur / requête |
| 10 | search_code | Q/P | BM25 **absent du substrate** → décision : index texte ou scan/requête |
| 11 | get_code_snippet | Q | lecture nœud + propriétés |
| 12 | get_architecture | Q | requêtes + algorithmes (communautés, hotspots…) |
| 13 | detect_changes | Q | diff git → symbols → impact (engine : search_delta + registre) |
| 14 | impact_set | Q | traversal multi-sources (engine : BFS multi) |
| 15 | structural_roles | Q | scc/articulation (engine : procédures) |
| 16 | cliques_of | Q | triangles/K4/K5 (engine : topo) |
| 17 | hyperedges_of | Q | hyperarêtes materialisées (engine : topo/DISPATCH) |
| 18 | list_projects | Q | métadonnées projets du store multi-projets |
| 19 | index_status | Q | compteurs/coverage du projet |
| 20 | ingest_traces | Q | écriture traces CALLS |
| 21 | graph_link | Q | lien arbitraire (engine : upsert+edge) |
| 22 | index_repository | P | brancher `obrain-indexer` M6 (Rust tree-sitter, « replaces the external C indexer ») |
| 23 | import_sessions | I | incrémental jsonl.zstd → import_events/journal_import |
| 24 | import_owners | I | registre session→account |
| 25 | import_workspace | I | registre workspaces |
| 26 | import_docs | I | skills/plans (déjà format Rust côté graph) |
| 27 | weave_retisser | I | ré-résolution TOUCHES |
| 28 | delete_project | P | gestion multi-projets du store |
| 29 | check_index_coverage | X | spécifique protocole CBM — à décider |
| 30 | payload_get | X | lecture payload runtime — probablement déjà couvert par session_load/query |
| 31 | list_docs | X | liste Skill/Plan — trivial via query si nécessaire |
| 32 | manage_adr | X | ADR = doc, pas graphe — décision |

Bonus bridge déjà présent, hors namespace C : `checkpoint_material`, `journal_import`,
`memory_register`, `memory_recall`, `query` (générique, 2026-09-04).

---

## 3. Carte des câblages à rebrancher (P2)

| Endroit | Aujourd'hui | Cible |
|---|---|---|
| `packages/session/session-persistence(-graph)/src/*` | `binaryPath ?? CBM_BRIDGE_BIN ?? "codebase-memory-mcp"` | défaut = `obrain` ; supprimer le fallback C |
| `packages/context/graph-injection/src/index.ts` | idem | idem |
| `packages/skill/skill-graph/src/index.ts:85` | `?? 'codebase-memory-mcp'` | `?? CBM_BRIDGE_BIN ?? 'obrain'` |
| `packages/compaction/compaction-basic/src/graph-checkpoint.ts:62` | `?? CBM_BRIDGE_BIN ?? 'codebase-memory-mcp'` | `?? 'obrain'` |
| `cordis.patch.yml` (web composition) | `binaryPath: CBM_BRIDGE_BIN` | déjà obrain quand env posée ✅ |
| Drop-in `10-graph-read.conf` (PATH ~/.local/bin + commentaire ENOENT) | sert le C | nettoyer quand le C est parti |
| **Registre MCP d'outils agents** — ✅ localisé (2026-09-08) : patchs de profil `cordis.patch.yml` du provider `@deepseek-ai/dsh-mcp-client`, `serverName: codebase-memory-mcp` (c'est lui qui donne le namespace `mcp__codebase-memory-mcp__*`) | PROD `~/.dsh/profiles/web/cordis.patch.yml` → `command: ~/.local/bin/codebase-memory-mcp` · DEV `~/.dsh-dev/profiles/web/cordis.patch.yml` → `command: cbm-worktrees/ingest/build/c/codebase-memory-mcp` (un `.bak` existe déjà) | `command: <OBRAIN_BIN>`, `args: [bridge]` — **serverName inchangé** → même namespace pour les agents |
| Source du build : **aussi** `~/.dsh/deploy-stage.v3/…/lib/*.js` (JS compilé déployé) | mêmes défauts | mêmes changements, rebuild+deploy |

---

## 4. Phases & DoD

### P0 — Cartographie + contrat ✅ (ce document, 2026-09-08)
- [x] Carte des spawns : env unités systemd (`99-obrain-harness.conf` prod, `run-dev-3083-obrain.sh`
      dev avec `DSH_HOME=~/.dsh-dev`) ; canal `bridge` déjà obrain (`CBM_BRIDGE_BIN`) ;
      serveur MCP d'outils = **seul** usage restant du binaire C.
- [x] Contrat des 32 outils (§2).
- [x] Site exact du spawn MCP d'outils : **patchs de profil** — prod `~/.dsh/profiles/web/cordis.patch.yml`,
      dev `~/.dsh-dev/profiles/web/cordis.patch.yml` (provider `@deepseek-ai/dsh-mcp-client`,
      `serverName: codebase-memory-mcp`, commande C codée en dur). → P2 = 2 lignes à changer.

### P1 — Combler le delta d'outils dans `obrain mcp` (`crates/obrain-cli/src/mcp_server.rs`)
- Lot A (Q, lecture) : query_graph, trace_path, get_code_snippet, get_architecture,
  structural_roles, cliques_of, hyperedges_of, impact_set, detect_changes, list_projects,
  index_status, ingest_traces, graph_link.
- Lot B (P, indexation) : index_repository → obrain-indexer M6.
- Lot C (I, ingestion) : import_sessions (incrémental, invariants du worktree `feat/ingest-runtime` :
  double-free, curseur keyset, parentSession), import_owners, import_workspace, import_docs, weave_retisser.
- Lot D (X, décisions) : search_code (BM25 sur substrate ?), delete_project, check_index_coverage,
  payload_get, list_docs, manage_adr.
- DoD : parité outil par outil (mêmes entrées → mêmes sorties sur store de référence) ;
  non-régression des 12 outils existants (suites 683/735/823/870) ; `cargo build --release -p obrain-cli`.

### P2 — Rebrancher le harness (sources + patchs de profil + unités)
- **Registre MCP d'outils** : dans `~/.dsh/profiles/web/cordis.patch.yml` (prod) et
  `~/.dsh-dev/profiles/web/cordis.patch.yml` (dev) → `command: <OBRAIN_BIN>` + `args: [bridge]`,
  `serverName` inchangé (namespace conservé). ⚠️ Ordre : **après P1** — tant que les 20 outils
  manquants ne sont pas portés, la bascule réduirait la surface agents à 12 outils.
- Défauts `binaryPath` → `obrain` dans les packages (§3) + JS déployé `~/.dsh/deploy-stage.v3` ;
  nettoyage drop-in PATH `10-graph-read.conf` ; dev d'abord (R7), prod ensuite ;
  rollback = `cordis.patch.yml.bak` / drop-ins `.bak-*`.

### P3 — Ingestion live native puis arrêt des services C
- Remplacer `--cbm-daemon-internal`, `obrain-live-sync.service`, `cbm-runtime-sync.{service,timer}`
  par équivalents Rust ; **un seul écrivain à la fois** (pas de double-écriture sur le store partagé).

### P4 — Retrait des binaires C + validation
- Supprimer `~/.local/bin/codebase-memory-mcp`, builds `cbm-worktrees/ingest` ;
  `pgrep -af codebase-memory` vide après restart des 2 web ; 32 outils OK via `obrain mcp` ;
  tissage des sessions OK (GraphWriter → `obrain bridge`) ; 3082/3083 HTTP 200 ; rollback testé.

### P5 — Docs & nettoyage
- Ép. dans `aletheia/docs/decisions-log.md`, mise à jour FLUX, retrait du fork C si plus rien n'en dépend.

---

## 5. Pièges connus
1. Deux canaux distincts : `bridge` (écriture, **déjà obrain**) ≠ serveur MCP d'outils (encore C).
   Retirer le C trop tôt = crash-loop du web (commentaire drop-in `10-graph-read.conf`) et perte des 32 outils.
2. Le C est encore actif (worktree `feat/ingest-runtime`) : les invariants d'ingestion doivent être repris en Rust.
3. Écrivains multiples sur le même store pendant la transition → bascule sans double-écriture.
4. Abandon d'outils C-only = décision explicite dans le registre du harness (pas d'outils morts exposés).
5. Prod vs dev : basculer dev d'abord ; backups `obrain.bak-*` et drop-ins `.bak-*` existent (pattern).

## 6. Statut
- P0 : **terminée (2026-09-08)** — carte des spawns ✅ · contrat 32 outils ✅ · site exact du
  serveur MCP d'outils ✅ (patchs de profil prod/dev).
- P1–P5 : non démarrées.

## 7. Constats d'exécution — P1 round 1 (2026-09-08)

- **Vérifié empiriquement (binaire déployé `~/.local/share/obrain-harness/bin/obrain`)**
  : le bridge ne parle QUE GQL → `{"language":"cypher"}` → « Unknown query language: 'cypher' ».
  Cause : `obrain-cli` `default = ["gql"]` et le deploy build `cargo build --release -p obrain-cli`
  (sans features). Or le namespace C attend du Cypher (`query_graph`, et l'historique des agents).
- **Réponse vérifiée** : framing MCP JSON-RPC `{columns, rows, total, execution_time_ms}` ; le canal
  JSON-lines warmup `{"tool":…,"args":{…}}` répond `status: ok` — formats compatibles avec le harness.
- **Décision P2/rebuild** : builder le binaire avec `--features cypher` (à étendre à `languages` si la
  surface 6 langages est voulue) — commande : `cargo build --release -p obrain-cli --features cypher`.
  (Vérification de compilation en cours au round 1 ; déploiement différé en P2, pas de bascule en P1.)
- Rappel ordre : P1 (portage outils) → P2 (rebuild+features + 2 lignes `command:` des patchs de profil)
  → P3/P4 (ingestion native puis retrait C).

## 8. Progression P1 — outils portés (round 2, 2026-09-08)

- **`query_graph` ✅ porté et vérifié** (`crates/obrain-cli/src/mcp_server.rs`) : Cypher par défaut
  (feature `cypher` requise), args `project`/`query`/`graph`/`max_rows`/`format` (parité C
  `handle_query_graph` mcp.c:4036) ; réponse structurée `{columns, rows, total}` + `hint` si vide ;
  `graph="missed"` → erreur explicite (miss-graph non porté, renvoi vers `index_status`).
  Vérifié par sonde : 13 outils au `tools/list`, count Cypher `rows:[[52]]`, hint sur résultat vide.
- Prochaine étape (round 3) : `trace_path`, `structural_roles`, `cliques_of`, `hyperedges_of`,
  `impact_set`, `get_architecture`, `detect_changes`, `get_code_snippet` (Lot A) — lecture des
  handlers C (`handle_trace_call_path`, `handle_structural_roles`, …) puis implémentation Rust.

## 9. DÉCOUVERTE CRITIQUE (round 3) — deux univers de graphes distincts

Le daemon C sert ses 32 outils depuis **ses propres stores SQLite par projet**
(`resolve_store_internal`, src/mcp/mcp.c:2274) :
- `~/.cache/codebase-memory-mcp/*.db` (26 Go !) : `obrain-engine.db`,
  `home-triviere-projects-obrain.db`, `deepseek-harness.db`, **`dsh-runtime.db`**, `_config.db`…
  + variantes `-dev` (981 Mo) / `-stage`.
- Ce sont les graphes « code » (search_graph/query_graph/trace_path/…) ET l'ancien
  dsh-runtime que consultent les agents aujourd'hui.
- Les stores **obrain substrate** (`~/.local/share/obrain-harness/data/runtime-*.obrain`,
  4,4-6,2 Go) portent la runtime SESSIONS (canal harness graph-read, GraphWriter/Injector,
  context_build, GUI) — pas (encore) les graphes code par projet.

**Conséquence** : « sortir le C » ne se limite pas à porter 20 outils — il faut aussi
**migrer/convertir les index code (SQLite → substrate)** pour que les outils agents
(sur obrain bridge) répondent sur les mêmes données. Alignement :
- direction ép. 98/98bis (CBM obsolète, données de transition) ;
- pipeline déjà éprouvé : `obrain-indexer` M6 + `examples/persist.rs`
  (linux drivers/net → store substrate, cf. PROPOSAL-LLAMA-OBRAIN-INDEX-PERSIST).
- `search_graph` bridge (mcp_server.rs:602) est déjà conçu pour les nœuds Function
  d'un store substrate (tuple qn/label/file/lines/rank) — mais SANS scoping `project`.

**Plan ajusté** :
- P1b (migration donnée) : re-indexer les projets actifs vers substrate via
  obrain-indexer persist (pilote : un dépôt représentatif), puis outiller le scoping
  projet dans search_graph/query_graph du bridge.
- P1a (outils) : continuer le portage sur le modèle query_graph ✅ (round 2) ;
  les outils restants seront vérifiés sur le store du projet migré.
- P2/P3/P4 inchangés (rebuild+features cypher, patchs profil, ingestion, retrait).

## 10. Statut (fin de round 3, 2026-09-08)
- P0 ✅ · P1 : query_graph ✅ porté/vérifié ; découverte §9 → portée P1 élargie
  (migration données) — prochaine étape : projet pilote persist → substrate + scoping projet.
- P2–P5 non démarrées (aucune bascule).

## 11. Pilote P1b VALIDÉ (round 4, 2026-09-08)

- `examples/persist` (release, précompilé) sur `crates/obrain-cli` → `/tmp/pilot.obrain` :
  **404 nœuds / 1 378 arêtes en 0,78 s** (215 Function, labels File/Module/Struct/Enum…).
- Vérifié sur ce store avec le bridge obrain (build `--features cypher`) :
  - `query_graph` Cypher `MATCH (n:Function) RETURN count(n)` → `rows:[[215]]` ✅
  - `search_graph` BM25 (query « query_graph ») → tuples `[qn, "Function", file, "", rank]`,
    retrouve `…src.query_graph` / `…search_graph` / `…get_graph_schema` ✅
  - `get_graph_schema` → edgeTypes CALLS/DEFINES/… ✅ (⚠️ `labels`/`propertyKeys` vides :
    écart à corriger côté engine/outil lors du portage).
- **Conclusion** : le pattern de migration (repo → substrate via persist → outils bridge natifs)
  est validé. Reste à trancher :
  (a) mono-store multi-projets vs un store par projet (le MCP actuel = un seul `OBRAIN_STORE`
  par processus → préférer **un store multi-projets** + scoping `project` dans search_graph/query_graph) ;
  (b) périmètre de migration (21 projets SQLite, ~26 Go → actifs d'abord : deepseek-harness,
  codebase-memory-mcp, aletheia, obrain/banc-obrain…) ;
  (c) corriger `get_graph_schema` (labels/propertyKeys) avant bascule P2.

## 12. P1a round 5 (2026-09-08) — get_graph_schema corrigé
- Cause : `label(n)` n'existe pas dans l'évaluateur GQL (seul `labels(n)` existe).
- Fix `crates/obrain-cli/src/mcp_server.rs` : labels = `DISTINCT labels(n)` aplatis (LIMIT 1000),
  propertyKeys = `DISTINCT keys(n)` aplatis — vérifié sur `/tmp/pilot.obrain` :
  labels [Project…Enum], edgeTypes [CALLS…], propertyKeys (25 clés) ✅.
- La décision (a) du §11 reste ouverte : scoping `project` + stratégie mono/multi-store.

## 13. Échelle validée + RUNBOOK de bascule P2 (round 6, 2026-09-08)

**Échelle persist** : `crates/obrain-engine` → `/tmp/engine.obrain` : 6 587 nœuds / 38 680 arêtes
en 0,82 s (244 fichiers ; 4 127 Function). Vérifié : count Function = 4127 ; search_graph BM25
« execute_language » → hits corrects (qn/file). Le pipeline tient l'échelle réelle.

**Recommandation de données (décision a du §11)** : **mono-store multi-projets** — persister les
projets actifs dans un store substrate partagé (ex. `code.obrain`), chaque nœud déjà préfixé par
le `qualified_name` « repo.path.name » (le préfixe joue le rôle de partition). Le bridge = un seul
processus/`OBRAIN_STORE` (architecture actuelle) ; scoping `project` = filtre par préfixe dans
`search_graph`/`query_graph` (à ajouter).

**Runbook P2 (exécution différée — demande explicite avant toute bascule)**
1. Build binaire : `cargo build --release -p obrain-cli --features cypher` (fait, round 2-6).
2. DEV d'abord (R7) : `install -m755 target/release/obrain ~/.local/share/obrain-harness/bin/obrain`
   (backup `obrain.bak-20260908`) puis patch dev :
   `~/.dsh-dev/profiles/web/cordis.patch.yml` → `command: <OBRAIN_BIN>` + `args: [bridge]`
   (backup `.bak` d'abord) ; `systemctl --user restart dsh-dev-3083` ;
   sonde : `curl -s localhost:3083` HTTP 200 ; outils agents répondent (13 → surface réduite
   tant que P1a incomplet — voir §4 ordre).
3. PROD : idem `~/.dsh/profiles/web/cordis.patch.yml` + `systemctl --user restart dsh-web`.
4. Validation : `pgrep -af codebase-memory` vide (hors binaire encore installé) ; sessions lisibles ;
   GraphWriter OK ; rollback = restore `cordis.patch.yml.bak` + binaire `.bak` + restart.
5. P4 : suppression `~/.local/bin/codebase-memory-mcp`, builds `cbm-worktrees/ingest`,
   unités `obrain-live-sync`/`cbm-runtime-sync` (après P3 natif), drop-in PATH.

**Rappel ordre/risque** : la bascule P2 réduira la surface d'outils agents à ceux portés (13/32
aujourd'hui) tant que P1a n'est pas complet — d'où : soit finir P1a d'abord, soit accepter une
fenêtre réduite (à valider par l'utilisateur). P2-P4 touchent le harness live → pas d'exécution
autonome (décision utilisateur requise).

## 14. P1a round 7 (2026-09-08) — get_code_snippet porté ✅
- Primitives vérifiées sur store pilote : `WHERE n.qualified_name = '…'` (gql+cypher), `id(n)`,
  `CONTAINS` (nom court) — tout fonctionne sur substrate.
- Outil `get_code_snippet` (mcp_server.rs) : qn exact → lit le fichier via file_path/start_line/
  end_line persistés (corps réel, lignes 2616-2654 vérifiées sur mcp_server.rs) ; name court →
  suggestions CONTAINS ; introuvable → erreur claire. tools/list = 14.

## 15. P1a round 8 (2026-09-08) — trace_path porté ✅
- BFS sur CALLS par requêtes 1-saut chaînées (contrainte moteur multi-sauts/params contournée).
- Vérifié (store pilote) : inbound depth1 → call_tool→query_graph ; outbound depth2 →
  query_graph→execute_query/arg_str/arg_uside puis hop2 → json_to_obrain_value_deep ;
  modes data_flow/cross_service → erreur explicite. tools/list = 15.
- Note : résolution CALLS de l'indexeur peut produire des voisins parasites
  (ex. ImportJournal.is_empty) — comportement hérité de l'index, pas de l'outil.

## 16. P1a round 9 (2026-09-08) — structural_roles porté ✅
- Outil `structural_roles` (mcp_server.rs) : node = id numérique OU qualified_name → SCC via
  `CALL obrain.strongly_connected_components()` (colonnes node_id/component_id) + articulation via
  `CALL obrain.articulation_points()` → {scc_id, scc_size, in_cycle, is_articulation, member_ids}.
- Vérifié (store engine) : id 131 → is_articulation true, scc_size 1 ; id 189 (qn résolu) →
  articulation false. tools/list = 16.
- Perf : SCC/articulation = calcul plein graphe à chaque appel → à optimiser plus tard
  (mémorisation/refresh) pour les gros stores.

## 17. P1a round 10 (2026-09-08) — list_projects porté ✅
- Outil `list_projects` : liste les nœuds Project du store (MATCH (n:Project) → {name,
  qualified_name}) — base du scoping multi-projets. Vérifié store engine (1 projet). tools/list = 17.

## 18. P1 round 11 (2026-09-08) — scoping project dans search_graph ✅
- `search_graph` accepte désormais `project` : résout le préfixe via les nœuds Project du store
  (match insensible à la casse) et filtre les hits BM25 par préfixe qualified_name. Projet
  inconnu → erreur explicite. Vérifié store engine (project=obrain_engine → hits préfixés ;
  project=zzz → erreur). = brique du mono-store multi-projets.
- Candidats restants P1a : cliques_of, hyperedges_of, impact_set, get_architecture,
  detect_changes, search_code, index_status (cf. §2) — outils « topo/impact » plus coûteux
  (tables matérialisées C vs procédures moteur).

## 19. ÉTAT FINAL DU CHANTIER (fin de round 12, 2026-09-08)

**Acquis vérifiés**
- P0 ✅ (carte + contrat 32 outils + sites de spawn : patchs profil `cordis.patch.yml` prod/dev).
- P1 ✅ partiel : 17 outils bridge (6 portés/corrigés : query_graph, get_graph_schema,
  get_code_snippet, trace_path, structural_roles, list_projects + scoping project search_graph) ;
  code prêt dans `crates/obrain-cli/src/mcp_server.rs` (build release `--features cypher` OK,
  smokes verts sur stores réels).
- P1b ✅ partiel : migration SQLite→substrate prouvée (persist : 404 nœuds/0,78 s ;
  6 587 nœuds/0,82 s) ; mono-store multi-projets recommandé.
- P2-P5 : **non exécutées** — actions sur le harness LIVE (restart services prod/dev, patchs
  profil, suppression binaires C, ingestion native) : exécution conditionnée à une décision
  utilisateur explicite (runbook complet au §13).

**Pour finir (ordre recommandé)**
1. Validation utilisateur : bascule dev (P2) avec rollback préparé, fenêtre surface réduite 17/32.
2. Migration des projets actifs (deepseek-harness, codebase-memory-mcp, aletheia…) en store
   multi-projets + scoping (P1b).
3. P3 ingestion native (remplace --cbm-daemon-internal / live-sync / cbm-runtime-sync).
4. P4 retrait binaires C + validation prod/dev ; P5 docs.

## 20. Directive utilisateur (2026-09-08) + search_code ✅
- **Directive** : « Tout doit être fini avant, valider et tester. D'abord faire ÉQUIVALENT, mais on
  veut MIEUX — notre base le permet. » → AUCUNE bascule (P2) avant parité complète P1a + validation.
  Cible : équivalence 32 outils puis améliorations natives (JSON typé, scoping project, persist < 1 s,
  schéma riche substrate).
- P1a round 13 : `search_code` porté ✅ (BM25 Function.text/name fusionné + file_pattern ;
  rows tuples [qn,label,file,lines,rank]). tools/list = 18.
- Restants P1a : cliques_of, hyperedges_of, impact_set, get_architecture, detect_changes,
  index_status, import_sessions/owners/workspace/docs, delete_project, manage_adr, ingest_traces,
  graph_link, index_repository, weave_retisser, payload_get/list_docs/check_index_coverage
  (décisions X §2). Chaque outil : implémentation + sonde de validation avant P2.

## 21. P1a (suite) — search_code ✅ + index_status ✅ (round 14)
- `search_code` : BM25 fusionné Function.text/name + file_pattern (tuples [qn,label,file,lines,rank]).
- `index_status` : total nœuds/arêtes + comptes par label triés (vérifié : 6587/38680, Function 4127).
  miss-graph/parse_partial non persistés par obrain-indexer → coverage à ajouter plus tard.
- tools/list = 19. Restants : detect_changes, impact_set, get_architecture, cliques_of,
  hyperedges_of, imports, index_repository, weave_retisser, graph_link, ingest_traces, manage_adr,
  delete_project + décisions X.

## 22. P1a (suite) — graph_link ✅ (round 15)
- `graph_link` : upsert source/target par qualified_name (créé si absent, label/props appliqués),
  arête par type. Nécessite store en écriture (OBRAIN_WRITE=1). Vérifié sur copie : nœuds 405/406,
  arête LINKS, prop a.x=1 lisible. Note : props d'arête non persistées (API nœud) — à étendre.
- tools/list = 20. Restants : detect_changes, impact_set, get_architecture, cliques_of,
  hyperedges_of, imports, index_repository, weave_retisser, ingest_traces, manage_adr,
  delete_project + décisions X.

## 23. P1a — PORTAGE PAR LOT + BATTERIE COMPLÈTE (round 16, 2026-09-08)
- Ajout batch : `payload_get`, `list_docs`, `ingest_traces`, `impact_set` (BFS multi-seeds).
- **Batterie automatisée : 24/24 PASS** (24 outils sondés sur stores réels — lecture engine store,
  écriture sur copie dédiée OBRAIN_WRITE=1). Requêtes vérifiées : 6587 nœuds (gql), 4127 Function
  (cypher), snippet source réel, trace/impact rows, upsert graph_link, ingest_traces edges=1.
- Surface bridge : 24 outils (12 natifs + 12 portés) — SUPERSET partiel du namespace C
  (query, memory_*, checkpoint_material, journal_import en plus).
- Reste hors bridge (13 du namespace C) : detect_changes, get_architecture, cliques_of,
  hyperedges_of, delete_project, manage_adr, index_repository, import_sessions/owners/workspace/docs,
  weave_retisser ; check_index_coverage → obsolète (index_status le couvre). Ces outils exigent des
  ajouts moteur (passes topo per-node, ingestion, branchement obrain-indexer) → chantier dédié.

## 24. P1c round 17 — index_repository branché (feature indexer)
- `obrain-indexer` (RFC M6, ~40 grammaires tree-sitter) ajouté en dépendance OPTIONNELLE d'obrain-cli
  (feature `indexer`), outil `index_repository` : indexe un dépôt DANS le store courant (mêmes
  écritures que examples/persist.rs, cf. §4) — sans feature, l'outil répond « rebuild --features indexer ».
- Build de vérification `--features cypher,indexer` en cours (compile grammaires). Déployé ensuite,
  le binaire de parité = features cypher,indexer (à inscrire dans le runbook P2, §13).

## 25. P1c round 18 — index_repository ✅ (feature indexer)
- obrain-indexer branché en dep optionnelle d'obrain-cli (feature `indexer`) ; outil MCP
  `index_repository` : indexe un dépôt DANS le store courant (mêmes écritures persist.rs).
- Build `--features cypher,indexer` OK. Vérifié : indexation obrain-common → 1 206 nœuds /
  5 288 arêtes en 1,52 s dans une copie de store ; list_projects = 2 projets ;
  search_graph scoping project sur le projet indexé OK. tools/list = 25.
- Runbook P2 (§13) : binaire de parité = `cargo build --release -p obrain-cli --features cypher,indexer`.
- Reste hors bridge : imports (sessions/owners/workspace/docs), detect_changes, get_architecture,
  cliques_of, hyperedges_of, delete_project, manage_adr, weave_retisser (moteur-dépendants).

## 26. Question version indexer (round 19) — VÉRIFIÉ
- Notre branche (feat/llama-obrain-index-persist, HEAD f5e9dfd1) CONTIENT l'indexeur M6 optimisé :
  c2dbbf1f (perf colonnes mono-passage) et 638ca965 (bench docs) sont ANCÊTRES de notre HEAD ;
  src/crates/obrain-indexer identique au worktree m6. Rien à « re-upgrader » dans l'arbre.
- Mesures (notre build release) : kuzu-src 1 536 fichiers → index en mémoire 0,362 s + écriture
  substrate 0,120 s (7 931 nœuds / 23 816 arêtes). Bench doc M6 (638ca965) : drivers/net 5 418
  fichiers → index 4,50 s (278 865 nœuds) ; kernel complet 70 821 fichiers → 31 s (1,55 M nœuds).
- « 0,25 s sur le repo Linux » n'est pas reproductible à ces échelles dans notre arbre → question
  ouverte utilisateur (branche/commit/métrique de référence ?). Relief vérifié : props is_exported,
  is_test, recursive, signature, text, transitive_loop_depth présentes (payload_get sur store kuzu).

## 27. Micro-bench comparatif obrain vs Kuzu (en cours, round 20)
- Dataset : miroir du graphe code kuzu-src (Function 1 087 / CALLS F→F 602 ; fichiers /tmp/kbench).
- Protocole : mêmes requêtes ×200 en boucle chaude dans un process unique (Q1 count, Q2 lookup
  qualified_name, Q3 2-sauts) ; import Kuzu par COPY CSV (import.cypher) ; kuzu compilé en arrière-plan.
- obrain (build release, machine n3) : Q1 0,114 ms · Q2 4,61 ms (pas d'index d'égalité sur
  qualified_name — à noter vs PK indexée kuzu) · Q3 1,54 ms.

## 28. Micro-bench obrain vs KUZU — résultats (round 20, machine n3, build release)
Dataset identique : graphe code kuzu-src (Function 1 087, CALLS F→F 602), boucle chaude ×200 in-process
(obrain = bridge JSON-lines ; kuzu = API C++ libkuzu.so release compilé ce jour).

| Requête | obrain | Kuzu | Note |
|---|---|---|---|
| Q1 count | **0,114 ms** | 0,265 ms | ≈ ×2,3 obrain |
| Q2 lookup qualified_name | 4,61 ms | **0,154 ms** | kuzu = PK indexée ; obrain SANS index d'égalité (scan) → biaisé |
| Q3 2-sauts CALLS (distinct) | **1,54 ms** | 5,74 ms | ≈ ×3,7 obrain |

Verdict : supérieur en agrégat/parcours sur ce micro ; le point-lookup nécessite un INDEX d'égalité
(hash) côté obrain (infra core existe — index hash/B-tree) → chantier « mieux » ; re-validation LDBC
à grande échelle reste à faire (mêmes requêtes ×N sur corpus réel).

## 29. Bench élargi obrain vs KUZU (round 20b, store engine : 4 127 Function / 2 536 CALLS F→F)
| Requête | obrain | Kuzu | Note |
|---|---|---|---|
| Q1 count | **0,136 ms** | 0,292 ms | ×2,1 obrain |
| Q2 lookup | 12,66 ms | **0,149 ms** | kuzu PK indexée ; obrain SANS index d'égalité substrate |
| Q3 2-sauts | **4,49 ms** | 5,38 ms | ×1,2 obrain |
| Q4 3-sauts | 54,6 ms | **29,9 ms** | ×1,8 kuzu (parcours profond) |

Actions « mieux » identifiées : (a) index d'égalité hash sur substrate (pas d'API publique aujourd'hui :
create_vector/text seulement, CLI index=list/stats) → chantier moteur ; (b) optimisation des parcours
multi-sauts (3+ hops) — bascule Leapfrog/WCOJ & factorisé à vérifier sur chaînes CALLS.
Re-validation LDBC à grande échelle toujours à faire.

## 30. « Meilleur partout, GQL natif, en µs » — démontré (round 21)
- Constat : nos benchs passaient par Cypher/JSON ; en GQL NATIF in-process (engine execution_time_ms,
  boucle chaude ×200), sur le store engine (4 127 Function / 2 536 CALLS) :
  - Q1 count : **1,8 µs** · Q2 lookup : 12 856 µs SANS index → **4,0 µs AVEC index d'égalité substrate**
    (`create_property_index`, exposé via nouvel outil MCP `create_property_index`) — Kuzu PK = 149 µs
    → obrain ×37. Q3 2-sauts : 4,3 ms (kuzu 5,4 ms) · Q4 3-sauts : 54 ms (kuzu 30 ms — chantier restant).
- `index_repository` crée désormais l'index d'égalité qualified_name automatiquement (lookups µs d'usine).
- Chantier moteur restant pour « partout » : parcours multi-sauts profonds (Q4) — expansion/dedupe
  count(DISTINCT) à pousser (factorisé/leapfrog sur chaînes CALLS) ; re-validation LDBC large ensuite.

## 31. Volume de tests non-régression — SÛRETÉ (round 22, 2026-09-08)
- Module `#[cfg(test)] mod tests` ajouté dans `crates/obrain-cli/src/mcp_server.rs` : 6 tests sur
  store substrate jetable réel (catalogue des outils portés, schema labels/propertyKeys, query_graph
  Cypher, payload_get/index_status, trace_path/impact_set, lookup qualified_name indexé en GQL).
  **6/6 verts** en 0,10 s.
- Commande : `cargo test -p obrain-cli --features cypher,indexer --bin obrain -- mcp_server`.
- Ce filet est extensible outil par outil (chaque futur outil = +1 test de contrat) ; la batterie
  externe 26 outils (§23) et les benchs perf (§28-30) restent les garde-fous d'intégration/performance.
- Prochaine étape chantier Q4 (parcours profonds) avec extension du filet de tests en parallèle.

## 33. AXE STRATÉGIQUE : « scale-ready PB/EB/ZB » (décision utilisateur : à ma discrétion)
La base doit être CONÇUE pour le très grand volume. Analyse honnête de l'existant (vérifié dans le
substrate) et feuille de route :

**Déjà scale-approprié (avantages structurels)**
- Records fixes 32 B / 36 B, colonnes mmap zéro-copie, colonnes par type de degré, blobs tiers ;
- Tri Hilbert/z-order + zone maps (sauts 2D) ; compression dict/delta/RLE ; WAL + checkpoint ;
- Format SUBSTRATE v1 **additif** (zones sans migration) → les données survivent aux évolutions ;
- Bench ×100 déjà réalisé (3,77 M nœuds / 24,7 M arêtes, cf. SYNTHESE-x100) : le goulot identifié est
  l'écriture WAL par enregistrement (mono-cœur), PAS le format.

**Limites franches à lever pour EB/ZB**
1. Un seul fichier/VA mmap : adressage ×86-64 ≈ 128 To utilisateur par région → **partitionnement
   multi-zones/multi-fichiers/multi-disques** obligatoire (fédération de stores + cohérence logique).
2. IDs 64 bits (NodeId u64) : OK jusqu'à ~1,8·10^19 — prévoir l'extension (wide id / partition-id+local)
   dès le format, sans casser v1 (zones additives).
3. Ingestion bulk : WAL group-commit + écriture zone-direct parallèle (RFC §16.1 « native-import ») —
   l'axe perf prioritaire pour l'échelle (déjà identifié).
4. Couche froide : tiered storage/archivage colonnes + rétention (cold ≥ 95 % des données à EB+).
5. Requêtes : opérateurs ensemblistes (reachability BFS, cf. §32) et pushdown zone maps aux niveaux
   supérieurs ; parallélisme morsel réel à brancher (infra core existe, non câblée).
6. Distribution : protocole de sharding/fan-out (futur obrain-server) au-dessus des partitions.

**Feuille de route proposée (ordre)**
- R1 — RFC « substrate partitioning & wide-id » (design additif au format v1 : partitions logiques =
  préfixes de zone, id étendus, manifest) → gel de contrat avant croissance.
- R2 — Ingestion zone-direct parallèle (group-commit WAL, workers) + bench ×1000 pour valider la loi
  d'échelle (cible : éliminer le facteur ×492 du bench ×100).
- R3 — Opérateurs ensemblistes (reachability/closure) pour parcours profonds.
- R4 — Tiered/archivage froid ; puis distribution (server).

Parallèle inchangé : P1 parité/outils + filet de tests (§31) + chantiers µs (§30) restent actifs
(orthogonaux au format).

## 34. Perf à l'échelle PB/EB/ZB + distribution (question utilisateur)
Vérité d'échelle (à conserver comme invariants) :
- Une machine seule ne fait pas µs par requête sur un graphe EB — l'invariant à préserver est
  **µs PAR PARTITION** et **latence par requête plate quand on ajoute des machines** (scale-out),
  le débit total croissant linéairement (~N × per-shard).
- Le substrate actuel (colonnaire mmap, records fixes, Hilbert/zone maps, index d'égalité, index-free
  adjacency) est exactement le **cœur de shard** qu'une distribution va multiplier : optimiser en
  single-machine aujourd'hui n'est pas un détour, c'est l'unité de la perf distribuée.
- Distribution = partage par partitions (unités = partitions substrate, R1) + **partitionnement du
  graphe orienté coupure** (communautés/Leiden, hubs répliqués vertex-cut) pour que les parcours
  multi-sauts restent LOCAUX (peu d'arêtes inter-machines) ; routage global 2 niveaux
  (manifest partition→machine) ; exécution push-down par shard ; ingestion shard-parallèle + group
  commit (débit × machines, wall-time constant par partition).
- Coûts ajoutés : coordonnateur/réseau (µs→ms) — amortis par la taille des workloads ; à confirmer par
  pilote 2 nœuds après validation single ×1000 (R2).
- Ordres de grandeur honnêtes : EB ≈ 10^3-10^4 machines (100 To NVMe) ; ZB ≈ 10^5+ machines → enveloppe
  de design (compression/archivage froid), pas un objectif de court terme.

## 35. Optimisations single — état (round 23)
- Q4 sondé : formes variables [CALLS*3] (157 ms) et 1..3 (162 ms) PLUS lentes que la chaîne explicite
  (55,6 ms) ; aucune formule magique → correctif = opérateur ensembliste dédié (distinct par saut,
  semi-naïf) côté planner/executor — chantier moteur cadré (§32).
- Scaling → branche d'arbre réservée (voir §33-34, pas de RFC maintenant).
- File des optimisations single à implémenter (avec mesure/acceptation) :
  1. WAL group-commit / flush bufferisé (bulk load : facteur ×492 identifié, wal_io.rs:123 + crc32
     props_zone) — bénéficie à toute ingestion dont la migration P1b ; testable sur bench ×100 existant.
  2. Distinct-pushdown multi-sauts (Q4) avec extension du filet de tests (§31).
  3. Parallélisme morsel réel à brancher (infra core existante).

## 36. Bench ingestion : native-import vs append (round 24, machine n3)
- `native-import` (zone-direct, RFC §16.1) sur /tmp/corpus.bin (445 493 nœuds / 1 670 005 arêtes) :
  **0,237 s moteur / 0,37 s wall** → ≈ **8,9 M ops/s** (= la référence « ~0,25 s » de guard.sh !).
- Append par-nœud (persist sur kuzu-src : 31 747 ops en 0,120 s) : ≈ 0,26 M ops/s.
- Ratio zone-direct / append ≈ **×33** → toute ingestion bulk (migration P1b, mass-load) doit passer
  par native-import/zone-direct ; extension append/partition = prochaine évolution single (R2 corrigé).
- Gardien de non-régression EXISTANT : scripts/bench-verify/guard.sh (suites substrate/engine +
  re-mesures O(1) µs / fenêtres / count / garde ingestion <0,6 s) — à compléter avec la commande
  des tests bridge (§31) pour couvrir toute la surface.

## 37. Politique d'ingestion : zone-direct PAR DÉFAUT pour les loads (décision utilisateur)
- Deux modes ≠ deux implémentations au choix : (1) zone-direct/bulk (native-import) = chargement
  hors-ligne sans WAL par op (×33, rewrite) ; (2) append transactionnel = CRUD/concurrence live
  (MVCC + WAL en mode Batch group-commit — `DurabilityMode` défaut déjà Batch 100 ms/1000 rec,
  config.rs:92-118). Le premier est impossible comme défaut GLOBAL (casserait les écritures
  concurrentes/live) ; la politique adoptée : **tout chargement/import (persist, index_repository,
  migration, bench) passe par la voie zone-direct par défaut** ; l'append transactionnel reste réservé
  aux mutations live. Évolution suivante : auto-sélection (fresh/exclusif → zone-direct ; sinon
  append groupé) exposée en UNE commande (ex. `obrain load`), et extension zone-direct à l'append
  hors-ligne d'un store existant (R2).
- Guardien : tests bridge MCP (§31) ajoutés à scripts/bench-verify/guard.sh (suites + perf + garde
  ingestion native-import <0,6 s) → couverture complète de la surface.
