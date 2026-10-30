# FULL OBRAIN VIZ — feuille de route couche de restitution visuelle (2026-09-08)

> **Statut** : EN COURS — round 0 : cadrage + tissage dans le graphe
> (nœud `plan:/home/triviere/projects/banc-obrain/docs/FULL-OBRAIN-VIZ-ROADMAP-2026-09-08`
> + workstreams `ws:banc-obrain-viz/*`, arêtes `HAS_STEP`/`PREREQUISITE_OF`/`EVOLVES`/`TOUCHES`) ;
> **round 1 (08/09)** : P0 **v01 + v02 LIVRÉS** — crate `obrain-viz` opérationnel
> (IR + json-graph v1 + writers DOT/Mermaid + sanitizer partagé, 36 tests verts,
> clippy 0, commit `65103b31`) — rendu réel graphviz/mermaid reporté à la garde v06
> (binaires absents de l'environnement), structure verrouillée par golden files ;
> **round 2 (08/09)** : P0 **v03 LIVRÉ** — overlays stables `ricci`/`energy`/`roles`
> (mapping normalisé documenté → `color_hint`/`size_hint`, fraîcheur `stale` exposée,
> rapport + légende) + **rendu des hints** dans DOT (`fillcolor`/`color`/`penwidth`)
> et Mermaid (`style`/`linkStyle`), schéma json-graph v1 étendu (hints optionnels,
> golden `example.energy.*`) — 52 tests verts, clippy 0 ;
> **round 3 (08/09)** : P0 **v04 LIVRÉ** — CLI `obrain viz subgraph` + `obrain viz export`
> (extracteur partagé `viz_extract.rs` : résolution id|qn via GQL, BFS 1-3 hops borné
> deux directions, filtre edge-type, `truncated`/`dropped`, signaux persistés
> energy/ricci/weight/stale/curvature via `substrate_handle` + records, scc via
> `CALL obrain.strongly_connected_components`) + formats dot/mermaid/json-graph et
> `--overlay` branchés sur la lib — 9 tests d'extraction sur stores substrate réels,
> clippy 0 ;
> **round 4 (08/09)** : P0 **v05 LIVRÉ** — tools MCP `viz_subgraph`/`viz_export`
> (mêmes fonctions que la CLI, formats json|dot|mermaid, overlay), `node_curvature`
> (courbure + energy par id|qn), `viz_topo` (triangles CALLS non dirigés calculés en
> Rust sur le sous-graphe extrait — pas de table simpliciale lisible côté substrate,
> choix documenté : calcul local plutôt que nouvelle primitive ; hyperarêtes DISPATCH
> non couvertes, à étendre si besoin en V1) — 6 nouveaux tests bridge sur stores
> réels, catalogue +15 tests viz, clippy 0 ;
> **round 5 (08/09)** : P0 **v06 LIVRÉ → P0 EXIT** — garde automatisée
> `scripts/guard-viz.sh` (+ `scripts/viz_guard.py`, appelable en pre-push) :
> **11/11 PASS** sur corpus réel généré (déterminisme octet-à-octet, bien-formé sans
> arête pendante, ids uniques, stats cohérentes, DOT/Mermaid structurellement
> cohérents, troncature `limit=2 → truncated`, parité export_all == comptage store
> 86/86/84/84) ; bench chiffré export_all ~21 ms / subgraph 2-hops ~21 ms (médianes,
> corpus 86 nœuds — le bench 4 127 fonctions d'`engine_idx.obrain` se rejoue avec
> `VIZ_STORE=<store> scripts/guard-viz.sh --bench`) ; CLI viz ouvre désormais en
> auto substrate/legacy (flag artefact `--as` pour lever le conflit clap global).
> **round 6 (08/09)** : V1 **B + C LIVRÉS** — `obrain-viz/src/diff.rs` (diff structurel
> nœuds/arêtes, deltas alignés ImportJournal, déterministe) + CLI **`obrain viz compare
> <a> <b>`** (fichiers json-graph v1 ou stores) ; writer **GraphML** déterministe
> (`xml_escape` complet, clés déclarées) branché CLI (`--as graphml`) et tools MCP
> (`format: graphml`) + golden — 61 tests obrain-viz (54+7), 15 tests cli viz,
> garde non-régression 11/11 OK. **A (overlays volatils fsrs/engrams/pheromone)
> reste PLANNED** (recherche des canaux de données runtime au prochain round).
> **round 7 (08/09)** : V1 **A LIVRÉ** — overlays volatils : `Overlay::{Fsrs, Engrams,
> Pheromone}` + `is_volatile()` (exports volatils **horodatés** `generated_at` — le
> non-déterminisme temporel FSRS/évaporation est assumé et documenté) ; mappings
> normalisés (fsrs difficulté→couleur/stabilité→taille, engrams pastel par id,
> phéromone au max du sous-graphe → `pheromone_color`) ; lecture des signaux
> persistés en propriétés (`engram_id`/`fsrs_stability`/`fsrs_difficulty`) ; args
> CLI/MCP `fsrs|engrams|pheromone` — 65 tests lib, 16 tests cli viz, garde 11/11.
> Note : valeurs phéromones d'arêtes = canal annotations stigmergy à valider sur
> un store runtime réel (champ + mapping prêts, aucune donnée locale).
> **round 8 (08/09)** : V2 **D LIVRÉ** — rendu de l'arbre d'opérateurs EXPLAIN :
> `obrain-viz/src/plan.rs` (parse du texte indenté moteur, hints pushdown annotés
> conservés dans les libellés) → Subgraph d'opérateurs (ids `op_000…`, liens PLAN),
> rendu json-graph v1 / dot / mermaid / graphml via les writers existants ; CLI
> **`obrain viz explain <store> "EXPLAIN …"`** — démos réelles (Return/NodeScan,
> Filter avec hint, Expand/Join). Décision 9 résolue : l'explain moteur est un
> texte structuré → parse direct, aucun chantier moteur. 69 tests obrain-viz,
> 16 tests cli viz, garde 11/11, clippy 0.
> **round 9 (08/09)** : V3 **E LIVRÉ** — timeline : `TimelineStep`/`TimelineReport`
> (`diff.rs`) + CLI **`obrain viz timeline <snapshots…>`** (fichiers json-graph v1 ou
> stores, ordre = chronologie ; deltas alignés ImportJournal, échantillons bornés) —
> base d'animation d'évolution, démo réelle 4 snapshots → 3 étapes. 71 tests
> obrain-viz, 16 tests cli viz, garde 11/11, clippy 0. **Tous les workstreams du
> plan sont LIVRES (P0 + V1 A/B/C + V2 D + V3 E).**
> **round 10 (08/09) — V4 mise en service (vérification externe)** : **V4-A→F LIVRÉS**.
> V4-A : release `target/release/obrain` déployé → `~/.local/share/obrain-harness/bin/obrain`
> (backup `.bak-20260904`, intégrité md5 ✓, sonde bridge ✓) — `tools/list` expose
> `viz_subgraph/viz_export/node_curvature/viz_topo` et un `viz_subgraph`
> (`substitute_in_operator`, 5 nœuds/5 arêtes, json-graph v1) répond via le binaire
> déployé. V4-B : ouverture **READ-ONLY** (LOCK_SH) — `runtime-dev` vivant ouvert
> (322k nœuds / 1,1 M arêtes). V4-C : guard `VIZ_RENDER=1` → DOT→SVG réel
> (`@viz-js/viz` WASM) + parité nœuds/arêtes (3/3). V4-D : centre par **suffixe non
> ambigu** (candidats listés si ambigu). V4-E : overlays volatils en RO sur runtime
> (fsrs 0 stylé — aucune prop FSRS persistée sur ce store ; pipeline sain).
> V4-F : `limit` défaut **500**, `truncated` booléen toujours présent, json-graph v1
> **gelé**, hyperarêtes DISPATCH différées. **V4-G : G1 LIVRÉ (round 11)** — preuve
> d'usage consommable : `docs/examples/viz/{sub.json,sub.svg,viewer.html}`
> (sous-graphe réel `substitute_in_operator` 5 nœuds/5 arêtes, SVG Graphviz
> 4 901 octets — identique à la vérification externe — + vue interactive sans
> dépendance, clic → voisins). **V4-G2 (arbitrage opérateur 08/09 : « ne branche
> pas le GUI dsh — sers toi-même »)** : **G2a LIVRÉ** — `obrain viz serve`
> (repo banc-obrain, HTTP std-only zéro dépendance) : `GET /api/viz/subgraph?store=
> &center=&hops=&limit=&overlay=` → json-graph v1 (même impl que le tool bridge,
> ouverture RO) ; `GET /` → viewer interactif branché sur l'endpoint (mode
> fichier G1 conservé) ; garde étendue d'une étape serve (lancement/requête/
> kill, parité 3n/2e, `truncated` bool) → **13/13 PASS** (commit `0e82204e`).
> **G2b** (exposition ffs/Caddy) = décision opérateur (hors périmètre de ce
> repo). **V5** (`dashboard-ui`, DGX) = différé jusqu'à G2a stable.
> Reliquat V4-F : `truncated` est sous `stats` du json-graph v1 (schéma gelé) —
> exposé booléen à chaque point de service (stats, MCP top-level, endpoint serve).
> **Version du code** : HEAD (round 11, 08/09), branche `feat/llama-obrain-index-persist`.
> **Origine** : retour PO branchement obrain-db ↔ LLM (qwen 3.8 27b) — « les formes sont
> calculées, il manque les exports ». Retour analysé, hypothèse **vérifiée dans le code** (§1).
> **Règle d'or** : chaque étape garde une porte verte (tests ciblés + `cargo test` + rendu réel +
> garde anti-régression) ; une seule implémentation partagée CLI ↔ bridge MCP (parité garantie) ;
> stratégie mise à jour dans le graphe à chaque round (§7).

## 0. Pourquoi (décision actée, non négociable)

Obrain calcule et **persiste** déjà les formes (courbure, énergie, engrams, pheromones) ; il ne
sait pas les **restituer** (aucun export exploitable par un LLM, un agent ou un markdown). La
couche de restitution est le pont entre le store cognitif et ses consommateurs :

- **Bridge MCP (LLM)** : `viz_subgraph` → « un appel = un sous-graphe rendable » (borné,
  token-friendly, affichable). La granularité sous-graphe est la bonne pour un agent.
- **CLI + exports** : DOT/Mermaid d'abord (rendu immédiat dans n'importe quel markdown/artifact),
  puis graphml/json-graph pour l'interop.
- **Différenciation obrain** : les overlays cognitifs (ricci/energy/fsrs/pheromone/engrams/roles)
  attachés comme attributs standard (weight, color_hint, size_hint) — ce que ni Neo4j ni Kuzu ne
  peuvent copier. **Les prioriser.**
- **Invariant d'implémentation** : la lib de restitution est unique ; CLI et tools MCP l'appellent
  (aucune divergence de comptes possible entre les deux surfaces).

## 1. État vérifié à HEAD (preuves fichier:ligne)

### Déjà calculé et persisté (les « formes »)
| Signal | Preuve code | Support |
|---|---|---|
| Ricci-Ollivier per-edge (Jost-Liu) + batch | `crates/obrain-substrate/src/geometric.rs` : `compute_ricci_fast` :132, `refresh_all_ricci` :277, `refresh_ricci_for_nodes` :439 | `EdgeRecord.ricci_u8` (Q8, `record.rs` :299-347), flag `RICCI_STALE` |
| `node_curvature` (agrégation nodale) | `geometric.rs` :631 (`compute_all_node_curvatures` :668) | lit le persisté ; exige ricci frais |
| Forman-Ricci (fast path combinatoire) | `geometric.rs` :979+ ; HKS + résistance effective exportées `lib.rs` :91-96 | calcul |
| Energy nœuds (Q1.15) + décroissance/boost | `NodeRecord.energy` (`record.rs`), `replay.rs` :435, `window_procedures.rs` :16/:64 (`CALL obrain.window_2d_energy`) | persisté |
| FSRS-5 (stabilité/difficulté engrams) | `obrain-cognitive/src/engram/decay.rs` (algo FSRS-5, 19 poids), `manager.rs` (scheduler), exposé `procedures.rs` :95-130 (`fsrs_stability`, `fsrs_difficulty`) | engrams (sous-ensemble de nœuds) |
| Pheromones stigmergy (4 trails, évaporation, anti-lock-in) | `obrain-cognitive/src/stigmergy.rs` (`PheromoneMap` :162, evaporate :264, flush `EdgeAnnotator` :399) ; `engram/mod.rs` Layer 2 | en mémoire + annotations arêtes (`__ann:pheromone_*`) |
| Engrams Layer 0 (Hebbian) | `obrain-cognitive/src/engram/mod.rs`, `manager.rs` | état engram |
| Rôles structurels (scc / articulation) | déjà exposé côté MCP : `obrain-cli/src/mcp_server.rs` :528 (`structural_roles`), :3401 | graphe CALLS |
| Refresh CLI | `obrain-cli/src/main.rs` :154-163 + `commands/enrich.rs` (`enrich --ricci`) | passe idempotente |

### Déjà en CLI, pas encore en bridge (lecture)
| Capacité | Preuve | Cible |
|---|---|---|
| Topologie (triangles/cliques/hyperarêtes) | `obrain-cli/src/commands/topo.rs` (Materialize/List/Stats/Hilbert) | tools bridge lecture (P0-v05) |
| Journal d'import en deltas | `commands/import_journal.rs` :139 `ImportJournal { added_nodes, removed_nodes, added_edges, removed_edges }` | socle de `viz_timeline` (V1-B) |

### Absent (le trou que la roadmap comble)
- **Aucun export** DOT / Mermaid / GraphML / json-graph dans les crates (grep `viz_subgraph` /
  `viz_export` / `overlay` / `graphml` sur `crates/` → 0 hit fonctionnel ; seuls des blocs mermaid
  statiques dans `docs/`). Le constat PO « formes calculées, exports manquants » est **exact**.
- **Aucune borne/statistique** de sous-graphe côté requêtes (pas de notion `truncated`).

## 2. Porte P0 — Fondations (séquentiel, prérequis de tout)

- **v01 — Crate `obrain-viz` (lib pure) + IR + json-graph v1** : ✅ **LIVRÉ (round 1, `65103b31`)** —
  nouveau member du workspace (`crates/obrain-viz` : `ir.rs`, `json_graph.rs`, `sanitize.rs`) ;
  IR interne `{nodes:[{id, qn, label, file_path, signaux?}], edges:[{src, dst, type, weight?, ricci?}]}`
  avec **id déterministe et stable** entre deux runs (base du diff/golden) ; sérialisation
  **json-graph v1** (schéma maison versionné = source de vérité diff/timeline/interop) + en-tête
  `{schema, stats:{node_count, edge_count, truncated}, generated_at}` ; validation bien-formé
  (ids uniques, aucune arête pendante). Porte : tests unitaires IR + golden json + déterminisme
  octet-à-octet — **verts (33 tests + golden)**.
- **v02 — Writers DOT + Mermaid** : ✅ **LIVRÉ (round 1, `65103b31`)** — sanitizer d'ids **partagé**
  (qn/chemins `/`, `-`, `.`, non-ASCII, `"`, `\n`) ; tests d'échappement hostiles ; golden files.
  Porte : sortie parsable **et rendu réel** (`dot -Tsvg`, moteur mermaid) sur échantillon — parsabilité
  + échappements **verts (golden)**, rendu réel binaire reporté à la garde v06 (graphviz/mermaid absents
  de l'environnement round 1).
- **v03 — Overlays stables** : ✅ **LIVRÉ (round 2)** — module `overlay.rs` :
  ricci (per-edge `ricci_u8`, **fraîcheur `RICCI_STALE` exposée** : compteur +
  `edge.stale`), energy (nœuds Q1.15), roles (`scc_id`) ; **fonction de
  normalisation documentée par overlay** (ricci = divergence autour de 0, energy = rampe
  froide→chaude, roles = pastel déterministe par scc) → `color_hint`/`size_hint`
  (attributs standard, schéma json-graph v1 étendu) + rapport + légende ; **rendu hints
  writers** : DOT `fillcolor`/`color`/`penwidth`, Mermaid `style`/`linkStyle`
  (déterministes, indexés). Porte : mapping testé + golden — **verts
  (`example.energy.*`, 52 tests, clippy 0)**. Rendu binaire réel toujours reporté v06.
- **v04 — CLI `viz`** : ✅ **LIVRÉ (round 3)** — `obrain viz subgraph <store> <center>`
  (`--hops 1-3` défaut 2, `--limit` → `truncated`/`dropped`, `--edge-type` répétable,
  `--outbound-only`/`--inbound-only`, `--no-signals`) + `obrain viz export <store>`
  (`--format json|dot|mermaid`, `--overlay ricci|energy|roles`). Extracteur partagé
  `obrain-cli/src/viz_extract.rs` (résolution id|qn GQL, BFS borné deux directions avec
  direction réelle des arêtes préservée, signaux persistés energy/ricci/weight/stale/
  curvature via `substrate_handle` + records `read_node`/`read_edge`, scc via
  `CALL obrain.strongly_connected_components`) ; parité des comptes vérifiée par test
  `export_all` = comptage store. Porte : commandes documentées + **tests d'intégration
  sur stores substrate réels (9 tests verts, clippy 0)**. « sur la sortie de
  viz_subgraph » (mode `--subgraph-file`) ajouté avec v05/v06 si besoin.
- **v05 — Bridge MCP (lecture)** : ✅ **LIVRÉ (round 4)** — tools `viz_subgraph`/`viz_export`
  (mêmes fonctions que CLI via `crate::viz_extract` — parité par construction ; formats
  json-graph v1 | dot | mermaid ; `--overlay` ricci|energy|roles avec `overlay_report`),
  **`node_curvature`** (courbure nodale + energy par id|qn), **`viz_topo`** (triangles CALLS
  non dirigés par nœud autour d'un centre, calculés en Rust sur le sous-graphe extrait ;
  top 20 + incidences). Décision tracée : les tables simpliciales (triangles matérialisés,
  hyperarêtes) **ne sont pas lisibles** côté substrate (note `cliques_of/hyperedges_of` du
  code) → lecture par **calcul local** plutôt que nouvelle primitive substrate ; hyperarêtes
  DISPATCH non couvertes (extension V1 si besoin). Porte : appels MCP réels sur store
  indexé — **verts (6 tests bridge + catalogue, 15 tests viz au total, clippy 0)**.
- **v06 — DoD automatisé** : ✅ **LIVRÉ (round 5) → P0 EXIT** — `scripts/guard-viz.sh`
  (+ `viz_guard.py`) : **11/11 PASS** sur corpus réel généré — déterminisme octet-à-octet,
  bien-formé (aucune arête pendante, ids uniques), stats cohérentes, DOT/Mermaid
  structurellement cohérents avec le sous-graphe, troncature (`--limit 2 → truncated`),
  **parité export_all == comptage store** (86 nœuds / 84 arêtes des deux côtés). Bench
  chiffré (corpus 86 nœuds, médianes sur 3 runs) : **export_all json ~21 ms, subgraph
  2-hops ~21 ms** ; le bench 4 127 fonctions (`engine_idx.obrain`, machine du PO) se
  rejoue : `VIZ_STORE=<store> scripts/guard-viz.sh --bench`. Spéc vivante : goldens
  `crates/obrain-viz/tests/golden/example.{dot,mmd}` + schéma `example.json` (rendus
  binaires graphviz/mermaid toujours indisponibles dans cet environnement — verrouillage
  structurel par goldens + garde). Rendu binaire réel à rejouer où dot/mmdc existent.

## 3. Workstreams (après P0) — vagues

| WS | Domaine | Aujourd'hui | Cible | Dépend de | Porte de sortie |
|---|---|---|---|---|---|
| **P0 — Fondations** | crate obrain-viz, json-graph v1, DOT/Mermaid, overlays stables, CLI viz, bridge lecture | rien | v01→v06 | — | guard-viz vert + rendus réels + bench chiffré |
| **A — Overlays volatils** (V1) | fsrs (stability/difficulty), engrams, pheromones (stigmergy) | engrams/pheromones/fsrs dans le cognitive | normalisation + `metadata.generated_at` ; **non-déterminisme temporel assumé et documenté** (décroissance FSRS/évaporation) | P0 | golden figés par ts ; légende ; 2 exports espacés ≠ mais expliqués |
| **B — viz_timeline + compare** (V1) | journal d'import daté + diff | `ImportJournal` (deltas bruts non datés) | deltas datés **par re-dérivation** de snapshots json-graph v1 (pas de nouveau mécanisme) ; `viz compare --format json-graph` entre deux variantes | P0 (json-graph) | diff correct sur 2 stores réels ; comptes exacts |
| **C — Formats interop** (V1) | graphml + évent. cytoscape/graphology | — | GraphML standard (échappement XML testé) | P0 | relecture GraphML (networkx) identique |
| **D — explain rendu** (V2) | arbre d'opérateurs du planner | `query` CLI existe ; structure explain interne à vérifier (Décision 9) | `query --explain --format dot\|mermaid` + hints pushdown annotés | P0 (writers) + décision 9 | rendu du plan sur requêtes réelles |
| **E — Animation évolution** (V3, optionnelle) | flux de deltas → animation | B | animation à partir des snapshots B | B | démo ; **hors périmètre si la valeur n'est pas confirmée** |

### Vagues de parallélisation
- **V0 (1 agent)** : P0 v01 → v06. Rien d'autre ne démarre avant.
- **V1 (2-3 agents)** : **A**, **B**, **C** en parallèle (modules disjoints : overlays / timeline /
  formats ; tous sur la lib P0).
- **V2 (1 agent)** : **D** (dépend de la décision 9 côté moteur).
- **V3 (1 agent)** : **E** — démo, gated sur la valeur.

## 4. Stratégie de test (ne rien casser)

1. **Logique pure séparée** : la lib `obrain-viz` ne dépend que des types substrate (lecture) —
   testable sans CLI, sans MCP, sans binding.
2. **Déterminisme d'abord** : input constant → sortie octet-à-octet identique (base de tout le
   reste : golden, diff, cache). Sauf overlays volatils (A) : `generated_at` explicite.
3. **Parité par requête store** : les comptes d'export sont recalculés par comptage store, jamais
   dérivés du parcours — toute divergence = bug d'extraction.
4. **Échappement hostile** : labels avec `"`, `\n`, accents, `/`, `-` → DOT et Mermaid restent
   valides (tests dédiés, pas de cas « en principe ça marche »).
5. **Rendu réel comme preuve** : chaque format validé par un rendu effectif (graphviz/mermaid) sur
   échantillon + golden files en CI ; le rendu humain reste la validation ponctuelle.
6. **Anti-régression chiffrée** : bench P0-v06 = garde-fou obligatoire de chaque WS suivant.

## 5. DoD renforcé + garde (au-delà du « non vide » du PO)

Un export est « fait » quand :
- **rendu réel validé** (Graphviz + Mermaid live) sur un échantillon représentatif ;
- **parité nœuds/arêtes** avec le store (recalcul par requête, voir §4.3) ;
- **bien formé** : pas d'arête pendante (chaque src/dst ∈ nodes), ids uniques, pas de doublon ;
- **déterminisme** octet-à-octet à entrée constante (overlays volatils exceptés, datés) ;
- **bornes** : 3-hops sur le nœud le plus connecté du store de référence → `truncated: true`,
  latence p95 et mémoire bornées (writer streaming pour store entier) ;
- **bench de taille** sur `engine_idx.obrain` (**4 127 fonctions**) + latence p95 chiffrés, seuils
  durs posés dans la garde ;
- `scripts/guard-viz.sh` (pré-push, pattern `pre-push.sh` existant) : viz_subgraph 2-hops sur un
  nœud connu → **non vide ET bien formé ET déterministe** (le « non vide » seul peut cacher un
  parcours partiel — insuffisant), + comptes + échappement + truncation.

## 6. Décisions à trancher (bloquantes, à arbitrer au cadrage P0)

1. **Sémantique `limit`/`truncated`** : borne sur arêtes d'abord ? ordre du BFS (reco : tri par
   weight décroissant, sinon déterministe par qn) ? `visited`/`dropped` dans les stats.
2. **Schéma json-graph v1** : valider le format minimal versionné `{schema, nodes, edges,
   metadata}` — source de vérité du diff (B), du timeline (B) et de l'interop (C).
3. **Périmètre overlays v1** : stables (ricci/energy/roles) en P0 ; volatils (fsrs/engrams/
   pheromone) en V1-A avec normalisation + horodatage.
4. **Fraîcheur ricci** : exposer la stale-ness (`edge.stale` + compteur) **sans** recompute à la
   volée côté bridge ; `enrich --ricci` reste la passe de refresh.
5. **curvature vs ricci** : deux champs distincts — `ricci` per-edge sur `edges[]`,
   `curvature` (agrégation nodale) sur `nodes[]`.
6. **Collision multi-overlay** (`color_hint`/`size_hint`) : ordre de priorité fixe, ou multiplex
   (color d'un overlay, size d'un autre).
7. **Crate dédié `obrain-viz`** (lib) + câblage CLI/MCP — valider l'approche mono-implémentation
   (pas de sérialisation dans substrate : crate bas niveau).
8. **Timeline sans nouveau mécanisme** : deltas datés re-dérivés par diff de snapshots json-graph
   v1 (le journal `ImportJournal` documente déjà added/removed à l'import).
9. **explain (D)** : vérifier si l'explain actuel est structuré en arbre ; sinon structuration
   préalable côté moteur — D ne démarre pas avant cette réponse.
10. **Store de référence** : `engine_idx.obrain` (4 127 fonctions) pour bench et garde + un corpus
    secondaire dense pour les bornes de volume.

### Résolutions actées (round 10, V4 — vérification externe)
1. **`limit`/`truncated`** : défaut `limit = 500` (CLI + MCP) ; `truncated` **toujours booléen**
   dans `stats` du json-graph v1 (jamais `None`).
2. **json-graph v1** : **gelé tel quel** — validé par le premier rendu SVG réel (parité 5/5) et les goldens.
3. **Suffixe centre** : repli `ENDS WITH '.x'` non ambigu (listes candidats si ambigu) — live `substitute_in_operator`.
4. **Ouverture** : READ-ONLY (LOCK_SH) pour toute la couche viz — stores vivants lisibles sans toucher l'écrivain.
5. **Hyperarêtes DISPATCH** : différées (tables simpliciales non exposées côté substrate ; calcul local documenté).

## 7. Engagement d'exécution autonome (protocole de mise à jour dans le graphe)

- Ordre : P0 → vagues V1/V2/V3 ; portes vertes obligatoires (§4/§5).
- **Chaque round** : mise à jour du doc en tête (statut) + des nœuds Task du graphe
  (`ws:banc-obrain-viz/*`) : `status` (PLANNED/PARTIAL/LIVRE), `wave`, `notes` (preuves
  commit + fichier:ligne), `updated` (YYYY-MM-DD), props `v01`… `v06` pour P0 ;
  tâches terminées marquées LIVRE avec preuve.
- **Tissage** : le plan reste ancré (EVOLVES → projet `home.triviere.projects.banc_obrain`,
  HAS_STEP → tasks, TOUCHES → fichiers) ; vérifier les voisins du plan après chaque modification
  structurelle majeure (requête graphe).
- Règle d'or : pas de commit sans tests ciblés + `cargo test -p obrain-viz` (+ crates touchés) +
  `guard-viz.sh` vert + rendu réel pour tout nouveau format.
- Traçabilité : preuves `fichier:ligne` dans `notes` des tasks, jamais de statut LIVRE sans preuve.
