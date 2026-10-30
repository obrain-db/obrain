# Cartographie d'obrain — 10 vagues × 5 agents

> Orchestration multi-agents (workflow) : **50 agents** en 10 vagues de 5, chacun en lecture seule,
> produisant une tranche structurée (aire, synthèse, composants, relations, constats, trous).
> Généré le 2026-10-03 00:44. Racine : `/home/triviere/projects/banc-obrain`
> (branche `consolidation/tronc-2026-09-15-plus-fusion`, HEAD `456d3794`).
> Données brutes : [`docs/cartographie-obrain/raw/`](raw/) (50 JSON).

## Vue d'ensemble (synthèse de tête)

**obrain** est « The Graph Database That Learns » — une base de graphe en **Rust** qui apprend,
structurée en **~24 crates** en couches strictes :

1. **Fondations** — `obrain-common` (types/IDs/Value, mémoires, MVCC) → `obrain-core` (modèle LPG/RDF,
   index, exécution vectorisée, statistiques, compression) → `obrain-substrate` (backend de stockage
   mmap+WAL « topology-as-storage », records Pod à taille fixe NodeRecord 40B / EdgeRecord 36B, pages 4KiB)
   → `obrain-engine` (ObrainDB, sessions, transactions, pipeline GQL Translator→Binder→Optimizer→Planner→Executor)
   → `obrain` (façade pure, un seul `lib.rs`, ré-exporte tout).
2. **Cognitif** — `obrain-cognitive` (energy, synapse, scar, fabric, engrams 6 couches, spreading activation)
   et `obrain-rag` (pipeline RAG hybride : TF-IDF + Hopfield spectral + énergie, sans modèle d'embedding neuronal).
3. **Ingestion/persistance** — `obrain-indexer` (tree-sitter → CodeGraph, remplaçant natif de l'indexeur C),
   `obrain-migrate` (migration in-place du format substrate), `obrain-journal` (reçus signés SHA-256+Ed25519),
   `neo4j2obrain` (stream Neo4j bolt → substrate + pipeline cognitif 10 étapes).
4. **Sécurité / self** — `obrain-iam` (ORN + politiques, défaut Deny), `obrain-tls` (rustls, aucun repli clair),
   `obrain-coffre` (AES-256-GCM, hiérarchie KEK/DEK/HKDF), `obrain-garde` (pure std), `obrain-selfd` (micro-harness),
   `obrain-selflink`, `obrain-aveugle` (porteur sans clé).
5. **Interfaces** — `obrain-cli`, `obrain-httpmin`, `obrain-reactive`, `obrain-adapters` (parseurs GQL/Cypher/SPARQL/…),
   `obrain-viz`, `bindings` (napi/ffi/wasm).

Autour du cœur : **bindings multi-langages** (Python, Node/TS, Go, C#/.NET, Dart, WASM, MCP), **~200 docs**
(ADR, PROPOSAL, DESIGN, ENGINE-* qualification), **~50 worktrees/branches** (feat/*, perf/*, integration/*,
fix/*, docs/*, consolidation/*, fusion/*), **~100 scripts** (acceptation, bench, deploy, enrich), et un
**dashboard Vue/Vite** (`packages/obrain-dashboard`).

## Sommaire

1. [Vague 1 — V1 Coeur moteur](#vague-1--v1-coeur-moteur)
2. [Vague 2 — V2 Bindings](#vague-2--v2-bindings)
3. [Vague 3 — V3 Docs](#vague-3--v3-docs)
4. [Vague 4 — V4 Ramifications](#vague-4--v4-ramifications)
5. [Vague 5 — V5 Scripts](#vague-5--v5-scripts)
6. [Vague 6 — V6 Qualité](#vague-6--v6-qualité)
7. [Vague 7 — V7 Cognitif](#vague-7--v7-cognitif)
8. [Vague 8 — V8 API](#vague-8--v8-api)
9. [Vague 9 — V9 Dashboard](#vague-9--v9-dashboard)
10. [Vague 10 — V10 Synthèse](#vague-10--v10-synthèse)

## Table récapitulative (C=composants, R=relations, F=constats, G=trous)

| # | Vague | Agent | Tranche | C | R | F | G |
|---:|---|---|---|---:|---:|---:|---:|
| 1.1 | 1 | — | coeur-stockage-requetes (crates obrain, obrain-core, obrain- | 46 | 14 | 9 | 7 |
| 1.2 | 1 | — | Couche cognitive (obrain-cognitive + obrain-rag) — primitive | 32 | 10 | 9 | 7 |
| 1.3 | 1 | — | Ingestion / indexation / persistance (obrain-indexer, obrain | 39 | 14 | 10 | 9 |
| 1.4 | 1 | — | Sécurité + auto-orchestration (IAM, TLS, coffre, garde, self | 43 | 18 | 10 | 11 |
| 1.5 | 1 | — | Interfaces & intégrations — CLI, HTTP minimal, pipeline réac | 39 | 14 | 10 | 10 |
| 2.1 | 2 | — | Bindings Python (crates/bindings/python + packages/obrain-cl | 18 | 13 | 8 | 8 |
| 2.2 | 2 | — | Bindings Node/TypeScript + CLI npm + interface JS du dashboa | 22 | 12 | 9 | 8 |
| 2.3 | 2 | — | Bindings Go, C#/.NET, Dart (tranche interfaces multi-langage | 19 | 10 | 9 | 7 |
| 2.4 | 2 | — | Bindings WASM (crates/bindings/wasm) + chaîne de build wasm3 | 27 | 12 | 11 | 10 |
| 2.5 | 2 | — | MCP server + napi/FFI (crates/bindings, obrain-cli mcp bridg | 25 | 13 | 10 | 9 |
| 3.1 | 3 | — | docs/ADR-*.md — chaîne des décisions d'architecture (8 fichi | 53 | 13 | 15 | 13 |
| 3.2 | 3 | — | docs/PROPOSAL-*.md — 11 propositions (formes de recherche, r | 34 | 12 | 12 | 10 |
| 3.3 | 3 | — | Conception représentation-nœud, slots512, pont mémoire zero- | 34 | 12 | 11 | 11 |
| 3.4 | 3 | — | États, consolidations et jalons (docs/CONSOLIDATION-*, ETAT- | 28 | 15 | 13 | 14 |
| 3.5 | 3 | — | docs/ — sous-répertoires structurés (architecture, getting-s | 42 | 17 | 9 | 10 |
| 4.1 | 4 | — | perf/* worktrees (banc-forme, banc-obrain-agregats, banc-obr | 30 | 8 | 10 | 7 |
| 4.2 | 4 | — | feat/principal-natif (j01-j05) + zero-copy + orbites + prove | 33 | 12 | 10 | 10 |
| 4.3 | 4 | — | Branches IAM — lignée feat/iam-* (obrain-iam + obrain-cli) | 23 | 10 | 8 | 9 |
| 4.4 | 4 | — | integration/fusion/consolidation | 24 | 11 | 8 | 8 |
| 4.5 | 4 | — | branches-diverses (docs principal-natif, fix MCP/engrams/sel | 28 | 10 | 8 | 7 |
| 5.1 | 5 | — | scripts/acceptation-*.sh + scripts/verif-*.sh + check-all.sh | 27 | 12 | 9 | 8 |
| 5.2 | 5 | — | scripts-bench-perf — dispositif de mesure de performance (ba | 25 | 11 | 11 | 10 |
| 5.3 | 5 | — | scripts/deploy-*.sh + ui-*.mjs + viz (déploiement, UI, dashb | 24 | 15 | 8 | 8 |
| 5.4 | 5 | — | scripts-enrich-index — pipeline d'enrichissement et d'indexa | 14 | 12 | 8 | 7 |
| 5.5 | 5 | — | scripts-*-check.mjs + gql-conformance.py + engine-etat.py (g | 23 | 13 | 8 | 8 |
| 6.1 | 6 | — | Tests golden et garantie de non-régression | 14 | 9 | 8 | 7 |
| 6.2 | 6 | — | qualification/ (ENGINE-004→007, programme de qualification f | 22 | 9 | 10 | 12 |
| 6.3 | 6 | — | examples/ + docs/examples/ (exemples de l'API publique obrai | 33 | 12 | 11 | 10 |
| 6.4 | 6 | — | Tests dans crates/ (tests d'intégration, tests unitaires #[t | 26 | 10 | 11 | 9 |
| 6.5 | 6 | — | CI & Bench & Makefile | 25 | 12 | 9 | 8 |
| 7.1 | 7 | — | Primitives cognitives obrain-cognitive (energy / synapse / s | 24 | 10 | 9 | 7 |
| 7.2 | 7 | — | Procédures cognitives GQL — CALL obrain.cognitive.* / obrain | 13 | 10 | 8 | 8 |
| 7.3 | 7 | — | Sections cognitives des docs — README (Cognitive Primitives  | 37 | 11 | 11 | 13 |
| 7.4 | 7 | — | Recherche & rank fusion (feat/recherche-couplee, search_delt | 18 | 10 | 7 | 8 |
| 7.5 | 7 | — | Algorithmes GDS (Graph Data Science) | 22 | 9 | 8 | 7 |
| 8.1 | 8 | — | Dialecte GQL (ISO/IEC 39075:2024) | 23 | 10 | 12 | 7 |
| 8.2 | 8 | — | HTTP/REST | 18 | 10 | 9 | 7 |
| 8.3 | 8 | — | Transport WebSocket + modèle réactif (obrain-reactive, feat/ | 20 | 9 | 8 | 8 |
| 8.4 | 8 | — | Serveur MCP — pont obrain (bridge) + surface selfd + intégra | 13 | 11 | 10 | 8 |
| 8.5 | 8 | — | Surfaces CLI + IAM auth (obrain-cli, packages/obrain-cli-*,  | 23 | 9 | 10 | 9 |
| 9.1 | 9 | — | packages/obrain-dashboard/src/ — front-end web du dashboard  | 42 | 16 | 11 | 10 |
| 9.2 | 9 | — | obrain-viz (couche de restitution visuelle) + dashboard-viz  | 29 | 14 | 8 | 9 |
| 9.3 | 9 | — | scripts/ui-* — contrat UI, promotion de version du dashboard | 22 | 10 | 9 | 8 |
| 9.4 | 9 | — | Horloge WASM et front statique (docs/WASM-HORLOGE, scripts/b | 28 | 13 | 12 | 10 |
| 9.5 | 9 | — | Docs UX / Dashboard obrain (8432) | 35 | 12 | 8 | 9 |
| 10.1 | 10 | — | Roadmap & phases restantes (roadmap.md, plan d'améliorations | 21 | 12 | 8 | 9 |
| 10.2 | 10 | — | Écosystème, manifeste et releases | 24 | 9 | 7 | 7 |
| 10.3 | 10 | — | Invariants transverses sécurité/perf (IAM, zero-copy, slots5 | 29 | 14 | 12 | 10 |
| 10.4 | 10 | — | Gaps et dette (tranche v10-4) — TODO/FIXME/XXX, stubs, code  | 22 | 10 | 11 | 15 |
| 10.5 | 10 | — | nomenclature-coherence | 18 | 11 | 10 | 8 |

---

## Vague 1 — V1 Coeur moteur

### 1.1 coeur-stockage-requetes (crates obrain, obrain-core, obrain-engine, obrain-substrate, obrain-common)

_Fichier : `raw/v1-1-coeur-stockage-requetes.json` — 46 composants · 14 relations · 9 constats · 7 trous_


**Synthèse.** La tranche couvre le cœur du moteur de graphe Obrain : les cinq crates de base forment une pile en couches strictes — obrain-common (types/allocateurs/MVCC, feuille sans dépendance interne), obrain-core (modèle LPG+RDF, trait GraphStore, opérateurs d'exécution vectorisés, index HNSW/BM25, stockage compressé), obrain-substrate (le backend de stockage unique « topology-as-storage » : enregistrements Pod à taille fixe mappés en mmap, pages 4 KiB, WAL natif CRC32, embeddings tiered), obrain-engine (ObrainDB, session, transaction MVCC, catalogue, et la pipeline complète translator→binder→optimizer→planner→executor), et obrain (façade publique qui re-exporte tout). Le modèle de stockage est remarquable : un nœud est un NodeRecord de 40 octets (v3) dont les arêtes sortantes forment une liste chaînée via un offset U48, les propriétés vivent dans des chaînes de pages 4 KiB à CRC32, et la durabilité repose sur un WAL double (propriétés scalaires inline + pages). Le cutover T17 (2026-04-23) a retiré le format mono-fichier .obrain au profit de substrate comme backend unique, tout en conservant le LpgStore in-memory legacy pour la migration et l'ombre cognitive. La pipeline de requêtes consomme les parseurs GQL/Cypher/SPARQL d'obrain-adapters (AST) puis les traduit en opérateurs physiques implémentés dans obrain-core, exécutés contre le trait objet-sûr GraphStore que SubstrateStore implémente. Les cinq crates portent #![deny(unsafe_code)] (forbid dans obrain), seul obrain-common/src/types/flat.rs est explicitement allow(unsafe_code).


**Composants clés.**
- crates/obrain/src/lib.rs — façade publique : re-export ObrainDB/Session/VERSION/GraphStore + NodeId/EdgeId/Value + allocateur global jemalloc/mimalloc
- crates/obrain-common/src/types/mod.rs — types publics NodeId, EdgeId, Value, PropertyKey, LogicalType, Timestamp, PropertyMap
- crates/obrain-common/src/types/value.rs — Value (enum dynamique) + HashableValue/OrderableValue/OrderedFloat64
- crates/obrain-common/src/types/id.rs — wrappers u64 NodeId/EdgeId/LabelId/PropertyKeyId/TransactionId/EpochId
- crates/obrain-common/src/memory/ — arena.rs / bump.rs / pool.rs / buffer/ (BufferManager, grants, régions)
- crates/obrain-common/src/mvcc.rs — VersionChain / Version / VersionInfo (isolation snapshot)
- crates/obrain-common/src/collections.rs — alias FxHashMap/FxHashSet/indexmap (hachage foldhash)
- crates/obrain-common/src/utils/error.rs — Error/Result partagés
- crates/obrain-core/src/graph/traits.rs — GraphStore / GraphStoreMut (contrat objet-sûr consommé par tout le moteur)
- crates/obrain-core/src/graph/lpg/node.rs — Node (labels SmallVec + PropertyMap)
- crates/obrain-core/src/graph/lpg/edge.rs — Edge (src/dst/edge_type + PropertyMap)
- crates/obrain-core/src/graph/rdf/ — RdfStore / Term / Triple (feature rdf)
- crates/obrain-core/src/execution/operators/ — 40+ opérateurs physiques (scan, filter, expand, join, aggregate, sort, variable_length_expand, shortest_path)
- crates/obrain-core/src/execution/chunk.rs + vector.rs — DataChunk / DataVector (exécution vectorisée colonnes)
- crates/obrain-core/src/execution/parallel/ — exécution morsel-driven (rayon/crossbeam)
- crates/obrain-core/src/execution/spill/ — external sort / spill disque hors mémoire
- crates/obrain-core/src/index/adjacency.rs — ChunkedAdjacency (liste d'adjacence chunkée)
- crates/obrain-core/src/index/vector/hnsw.rs — index HNSW (ANN)
- crates/obrain-core/src/index/text/inverted_index.rs — index inversé BM25
- crates/obrain-core/src/storage/ — dictionary / bitpack / delta / runlength / succinct (rank-select, Elias-Fano, wavelet)
- crates/obrain-core/src/change_tracker.rs — ChangeTracker / GraphEvent / GraphDiff
- crates/obrain-core/src/subscription.rs — SubscriptionManager / EventFilter
- crates/obrain-core/src/cache/second_chance.rs — LRU second-chance
- crates/obrain-engine/src/database/mod.rs — ObrainDB (new_in_memory / open / open_substrate / open_read_only)
- crates/obrain-engine/src/query/mod.rs — pipeline translator→binder→optimizer→planner→executor
- crates/obrain-engine/src/query/translators/gql/mod.rs — traducteur GQL (AST obrain_adapters::query::gql)
- crates/obrain-engine/src/query/translators/cypher.rs — traducteur Cypher
- crates/obrain-engine/src/query/planner/lpg/ — plan physique LPG (scan/expand/filter/mutation/typed_degree_rewrite)
- crates/obrain-engine/src/query/optimizer/ — cardinality / cost / join_order
- crates/obrain-engine/src/query/executor/ — Executor + procedure_call / user_procedure
- crates/obrain-engine/src/session/mod.rs — Session (resolve_store, vues zéro-copie prop_str_view)
- crates/obrain-engine/src/transaction/ — TransactionManager + mvcc + prepared + write_tracker
- crates/obrain-engine/src/catalog/mod.rs — Catalog / CatalogError / IndexDefinition / IndexType
- crates/obrain-engine/src/config.rs — Config / AccessMode / DurabilityMode / GraphModel / StorageFormat
- crates/obrain-engine/src/admin.rs — AdminService (stats, backup, inspection, validation)
- crates/obrain-engine/src/cdc.rs — CdcLog (change data capture, feature cdc)
- crates/obrain-substrate/src/record.rs — NodeRecord(40 B)/EdgeRecord(36 B)/U48/SimplexRecord (Pod, repr(C))
- crates/obrain-substrate/src/page.rs — PropertyPage 4 KiB + header CRC32 + slot directory
- crates/obrain-substrate/src/wal.rs — WalRecord / WalPayload / WalKind (framing 28 B, CRC32, LSN)
- crates/obrain-substrate/src/meta.rs — MetaHeader 96 B + SUBSTRATE_FORMAT_VERSION=3 + meta_flags
- crates/obrain-substrate/src/file.rs — SubstrateFile + Zone enum (substrate.nodes/edges/props/tier0-2/simplex_dir/engram_members)
- crates/obrain-substrate/src/store.rs — SubstrateStore (impl GraphStore) — plus gros fichier du dépôt (~1.6 MB)
- crates/obrain-substrate/src/writer.rs — Writer (écritures + compaction, 199 KB)
- crates/obrain-substrate/src/dict.rs — registres internés (labels/edge-types/property-keys) + sidecar substrate.dict
- crates/obrain-substrate/src/props_zone.rs — PropsZone v2 (pages partagées, PROPS_V2_FILENAME)
- crates/obrain-substrate/src/tiers.rs — Tier0/Tier1/Tier2 (embeddings binaire + f16, hamming/cosine)

**Relations / dépendances.**
- obrain → obrain-engine (re-export ObrainDB, Session, GraphStore, VERSION) et → obrain-common (re-export NodeId/EdgeId/Value)
- obrain-engine → obrain-common (types), obrain-core (default-features=false), obrain-substrate (inconditionnel), obrain-adapters (parseurs + LpgWal)
- obrain-substrate → obrain-common + obrain-core (default-features=false, consomme le trait GraphStore)
- obrain-core → obrain-common (seule dépendance directe)
- obrain-common → aucune dépendance interne (crate feuille)
- impl GraphStore for SubstrateStore (obrain-substrate/src/store.rs) — le substrat implémente le trait défini dans obrain-core
- ObrainDB.store = Arc<dyn GraphStoreMut> (effacé) + handle concret SubstrateStore conservé pour les colonnes cognitives (energy/synapse/scar)
- Session::resolve_store → active_store()/external_store (routage du store actif par graphe nommé)
- obrain-engine/query/translators/gql → obrain_adapters::query::gql::ast ; cypher → obrain_adapters::query::cypher::ast (le parsing vit dans adapters)
- obrain-engine/query/executor/procedure_call → dispatch CALL (procedures.rs feature algos, cognitive_procedures feature cognitive)
- obrain-core/execution/operators → trait GraphStore (node_ids/neighbors/find_nodes_by_property/index_seek)
- SubstrateFile::zone → Zone::Nodes/Edges/Props/Tier0-2/SimplexDir/EngramMembers (un fichier mmap par zone)
- WAL : WalWriter/WalReader → replay_from (rejeu au boot) ; merge_wal_external_edges pour les arêtes inter-bases
- obrain-engine/database/mod.rs → crud.rs/index.rs/search.rs/query.rs/admin.rs (opérations réparties en sous-modules)

**Points notables.**
- Topology-as-storage : les arêtes sortantes sont une liste chaînée par offset U48 (NodeRecord.first_edge_off), pas une matrice d'adjacence ; les propriétés sont des chaînes de pages 4 KiB à CRC32.
- SubstrateStore est concentré dans un fichier unique store.rs de ~1.6 MB (plus writer.rs 199 KB) — toute l'impl GraphStore du backend de production.
- NodeRecord est passé de 32 à 40 octets en v3 (colonne tenant_id + _reserve), EdgeRecord à 36 octets ; SUBSTRATE_FORMAT_VERSION=3.
- Les 5 crates portent #![deny(unsafe_code)] (forbid dans obrain) ; seule exception explicite obrain-common/src/types/flat.rs #[allow(unsafe_code)].
- Cutover T17 (2026-04-23) : le format mono-fichier .obrain est retiré, substrate devient l'unique backend ; LpgStore in-memory survit en legacy pour obrain-migrate et l'ombre cognitive.
- Le trait GraphStore est très large avec des dizaines de hooks par défaut (no-op) qui documentent le cutover substrate (graphes nommés, arêtes externes, degree typé, MVCC).
- WAL à double journalisation : propriétés scalaires inline (PropSetInline/EdgePropSetInline/PropRemoveInline) + pages props ; plus arêtes inter-bases externes (ExternalEdgeInsert/Delete).
- L'exécution dispose de deux moteurs : opérateurs vectorisés (DataChunk/DataVector) + parallélisme morsel, et une couche de requêtes cognitives CALL obrain.cognitive.* feature-gated.
- obrain-engine/tests contient 168 fichiers de tests d'intégration (conformité, coverage, bancs, cognitive, cdc, arêtes inter-bases).

**Trous / incohérences.**
- Doc obsolète : la carte de modules de crates/obrain-substrate/src/lib.rs annonce « NodeRecord (32 B) and EdgeRecord (32 B) » alors que record.rs impose 40 B / 36 B (v3, T17f).
- Incohérence interne à record.rs : le titre de section dit encore « NodeRecord — 32 B, aligned to 8 » en contradiction avec l'invariant const assert size_of == 40.
- crates/obrain-engine/src/config.rs définit StorageFormat (Auto/WalDirectory/SingleFile) qui référence le format mono-fichier .obrain retiré au T17 — surface potentiellement morte ou documentaire.
- LpgStore (graph/lpg) est toujours présent dans obrain-core mais plus ré-exporté à la racine — code legacy conservé uniquement pour obrain-migrate et les sidecars cognitifs.
- Substrate ne supporte pas les graphes nommés/multi-graphes (documenté « RFC séparé »), mais la surface GraphStoreMut correspondante existe par défaut no-op (risque de confusion sémantique).
- Trois fichiers substrate ont des permissions rw------- (instantane.rs, unite.rs, synapse_incidence.rs) contrairement à leurs pairs rw-rw-r-- — anomalie d'umask à vérifier.
- Les tests de conformité GQL connus-défauts sont masqués derrière la feature « defauts-connus » (non compilée par défaut) : défauts moteur documentés mais jamais exécutés en suite verte.



### 1.2 Couche cognitive (obrain-cognitive + obrain-rag) — primitives energy/synapse/scar/fabric/engram et scoring RAG

_Fichier : `raw/v1-2-couche-cognitive.json` — 32 composants · 10 relations · 9 constats · 7 trous_


**Synthèse.** La couche cognitive d'obrain est scindée en deux crates : obrain-cognitive (primitives biomimétiques de bas niveau) et obrain-rag (retrieval-augmented generation au-dessus de ces primitives). obrain-cognitive expose ~45 modules derrière des feature flags (défaut vide), organisés autour d'écouteurs MutationListener branchés sur le MutationBus d'obrain-reactive : EnergyListener, SynapseListener, FabricListener, CoChangeDetector, etc. Les quatre primitives cœur suivent le même schéma de décroissance exponentielle (2^(-Δt/half_life)) : energy (activation des nœuds), synapse (apprentissage Hebbien « fire together, wire together »), scar (mémoire persistante des erreurs), fabric (métriques composées de risque). Le système engram ajoute six couches biomimétiques (Hebbian+FSRS, immuno, stigmergie, Hopfield moderne + codage prédictif, épigénétique, homéostasie) au-dessus de ces primitives. obrain-rag est un pipeline RAG « schema-agnostic » en trois étapes (recall d'engrams par Hopfield → spreading activation à travers les synapses → construction de contexte), avec une boucle de feedback Hebbienne facultative. L'architecture a migré (T6) d'une persistance legacy write-through via propriétés de graphe vers une persistance en colonnes substrate (WAL-first, mmap) : NodeRecord.energy (u16 Q1.15), EdgeRecord.weight_u16 (Q0.16) et NodeRecord.scar_util_affinity (5 bits). Le scoring RAG combine un index inversé TF-IDF (sans mots vides, IDF calculé à la volée), un rappel Hopfield/spectral, l'énergie de spreading activation et un réordonnancement MMR diversité-aware.


**Composants clés.**
- crates/obrain-cognitive/src/lib.rs — point d'entrée, ~45 modules feature-gated + re-exports (EnergyStore, SynapseStore, ScarStore, FabricStore, EngramStore, RecallEngine, ...)
- crates/obrain-cognitive/src/energy.rs — EnergyConfig, NodeEnergy (décroissance E(t)=E0×2^(-Δt/half_life)), EnergyStore (DashMap + colonne substrate u16 Q1.15), EnergyListener, energy_score(), effective_half_life()
- crates/obrain-cognitive/src/synapse.rs — SynapseConfig, Synapse, SynapseStore (normalisation Hebbienne compétitive, index inversé node_to_keys), SynapseListener, SynapseConsolidator (SegQueue + task tokio), CrossBaseNodeId/Synapse
- crates/obrain-cognitive/src/activation.rs — spreading activation BFS : SpreadConfig, spread(), spread_single(), ActivationMap, causal_boost (vs associatif)
- crates/obrain-cognitive/src/fabric.rs — FabricScore, FabricStore, RiskWeights (pagerank .25 / mutation .25 / annotation_gap .20 / knowledge_gap 0 / betweenness .15 / scar .15), compute_risk_score(), FabricListener
- crates/obrain-cognitive/src/scar.rs — ScarConfig, Scar, ScarReason (Rollback/Error/Invalidation/ConstraintViolation/Truncation/Repetition/UserCorrection/Custom), ScarStore (champ scar 5 bits), add_scar/heal/boost_scar/partial_heal
- crates/obrain-cognitive/src/engram/mod.rs — arborescence du système engram (6 couches biomimétiques + homéostasie), feature flags engram/hopfield/immune/epigenetic/stigmergy
- crates/obrain-cognitive/src/engram/types.rs — Engram (ensemble, spectral_signature, strength, valence, precision β, recall_history, source_episodes, horizon), EngramId, EpisodeId
- crates/obrain-cognitive/src/engram/formation.rs — HebbianWithSurprise, CoActivationDetector, EngramFormationTrigger, FormationConfig (min_episodes=3, min_overlap=0.5, min_prediction_error=0.3)
- crates/obrain-cognitive/src/engram/decay.rs — FsrsConfig, FsrsScheduler (Free Spaced Repetition Scheduler), ReviewRating
- crates/obrain-cognitive/src/engram/recall.rs — RecallEngine (double voie directe + Hopfield), WarmupSelector (MMR), poids_entier()/ECHELLE_POIDS=1024, cache PatternMatrix invalidation
- crates/obrain-cognitive/src/engram/hopfield.rs — Modern Hopfield : retrieve = softmax(β×P^T×q)×P, PatternMatrix, max_marginal_relevance(), softmax_compete(), bayesian_update(), PredictionErrorCalculator
- crates/obrain-cognitive/src/engram/homeostasis.rs — HomeostasisConfig, HomeostasisEngine (scaling synaptique, anti-lock-in)
- crates/obrain-cognitive/src/engram/spectral.rs — SpectralEncoder (signature spectrale des engrams pour la similarité vectorielle)
- crates/obrain-cognitive/src/engram/store.rs — EngramStore (persistance des traces mémoire)
- crates/obrain-cognitive/src/engram/manager.rs — EngramManager (coordonnateur : formation, decay, recall, homeostasis, metrics)
- crates/obrain-cognitive/src/engram/crystallization.rs — CrystallizationDetector, crystallize(), generate_summary() (matérialisation de notes), LABEL_CRYSTALLIZED_NOTE
- crates/obrain-cognitive/src/engram/substrate_adapter.rs — SubstrateVectorIndexAdapter (pont index vectoriel ↔ substrat)
- crates/obrain-cognitive/src/engram/traits.rs — CognitiveStorage, VectorIndex, InMemoryVectorIndex, CognitiveNode/Edge, EdgeAnnotator, QueryObserver
- crates/obrain-cognitive/src/engine.rs — CognitiveEngine / CognitiveEngineBuilder (assemblage des sous-systèmes)
- crates/obrain-cognitive/src/search.rs — SearchPipeline, Reranker, SearchWeights, SearchResult (scoring de recherche)
- crates/obrain-cognitive/src/procedures.rs — procédures d'introspection GQL (engrams.list/inspect/forget, cognitive.metrics)
- crates/obrain-rag/src/lib.rs — RAG « Graph-Augmented Retrieval via Engrams », 3 étapes + boucle feedback, schema-agnostic
- crates/obrain-rag/src/retriever.rs — EngramRetriever + InvertedIndex (TF-IDF à la volée, dampening de cardinalité de label, matching bidirectionnel), tokenize_text(), retrieve() en 4 étapes
- crates/obrain-rag/src/ranking.rs — rank_nodes(), rank_with_diversity() (MMR : W_SCORE .6 / W_RICHNESS .25 / W_DIVERSITY .15 / REDUNDANCY_PENALTY .30), text_richness()
- crates/obrain-rag/src/context.rs — GraphContextBuilder (formatage markdown, filtrage noise_properties, lookup de noms de relations)
- crates/obrain-rag/src/budget.rs — estimate_tokens(), select_within_budget() (greedy par score)
- crates/obrain-rag/src/feedback.rs — CognitiveFeedback, find_mentioned_nodes(), FALLBACK_CAP=32 (Plan 69e59065 T2)
- crates/obrain-rag/src/pipeline.rs — RagPipeline (query → retrieve → rank → budget → format, + feedback)
- crates/obrain-rag/src/config.rs — RagConfig (presets fast/balanced/thorough, validate(), noise_properties)
- crates/obrain-rag/src/traits.rs — Retriever, ContextBuilder, FeedbackSink, RetrievedNode, RetrievalSource, RagContext, RetrievalResult
- docs/cognitive/energy.md, synapses.md, fabric.md, engrams.md, rag.md, overview.md, canonical-features-pipeline.md — documentation de la couche

**Relations / dépendances.**
- obrain-rag dépend de obrain-cognitive (features engram, hopfield, energy, synapse via feature 'cognitive'), de obrain-core (GraphStore) et obrain-common (NodeId)
- obrain-cognitive dépend de obrain-common, obrain-core, obrain-reactive (MutationEvent/MutationListener), + obrain-adapters / obrain-iam / obrain-substrate en optionnel
- EnergyListener / SynapseListener / FabricListener / CoChangeDetector implémentent obrain_reactive::MutationListener et sont dispatchés par le MutationBus via un Scheduler
- EngramRetriever::retrieve() appelle RecallEngine::recall() (obrain_cognitive::engram) puis spread() (obrain_cognitive::activation) via SynapseActivationSource adossé au SynapseStore
- CognitiveFeedback::feedback() appelle EnergyStore::boost() (energy) et SynapseStore::reinforce() (synapse) — boucle Hebbienne response-aware
- FabricStore::set_scar_intensity() consomme ScarStore::cumulative_intensity() ; le risque composite compute_risk_score() intègre le scar via RiskWeights.scar
- La persistance substrate route : EnergyStore→boost_node_energy_f32 (u16 Q1.15), SynapseStore→reinforce_edge_synapse_f32 (Q0.16), ScarStore→set_node_scar_field_f32 (5 bits) — toutes WAL-first
- SubstrateVectorIndexAdapter (engram/substrate_adapter.rs) relie le VectorIndex des engrams au SubstrateStore
- RecallEngine fusionne deux voies : rappel direct (overlap d'ensemble) pondéré direct_weight + Hopfield softmax(β×P^T×q)×P, avec cache PatternMatrix invalidé sur EngramStore::revision
- GraphContextBuilder appelle rank_with_diversity() (ranking) puis select_within_budget() (budget) avant formatage

**Points notables.**
- « embeddings » au sens neural n'existe pas dans obrain-rag : la recherche hybride est vectorielle PAR LE GRAPHE — signature spectrale (SpectralEncoder) + Hopfield (vecteur) combinée à l'énergie de spreading activation + index inversé TF-IDF ; aucune inférence d'embedding externe
- Migration de persistance majeure (T6) : chaque store a un mode legacy (propriétés graphe write-through) ET un mode substrate (colonne WAL-first mmap) — double chemin encore compilé, legacy qualifié « test-only »
- Les synapses substrate sont écrites dans des slots SANS adjacence relationnelle (create_synapse_slot) — invisibles aux jointures MATCH, lisibles via la colonne de poids (29 943 synapses mesurées hors graphe relationnel)
- SynapseStore::reinforce() ne fait plus qu'UNE écriture durable synchrone (colonne poids) ; les métadonnées (epoch, count) passent par une SegQueue lock-free drainée par un SynapseConsolidator (tick 30s) + drain SIGTERM (Plan 69e59065 T3)
- Plan 69e59065 T2 : FALLBACK_CAP=32 dans feedback.rs plafonne le renforcement C(N,2) — calibré par feedback_scaling_bench.rs (N=100 → 4 950 reinforces → 86.59 ms > gate 50 ms, gotcha 49938809)
- Le tokenizer RAG a retiré les mots vides (commit e9ab991d) : tout mot ≥2 caractères est un terme, la pondération se fait en aval par TF-IDF + dampening de cardinalité de label ; les accents sont conservés tels quels
- Le risque fabric est un poids composé normalisé (pas de collapse multiplicatif) ; mais knowledge_gap a un poids PAR DÉFAUT de 0.0 — la contribution « trous structurels » est inactive par défaut (opt-in via RiskWeights::new)
- RecallEngine cache la PatternMatrix Hopfield (parking_lot::Mutex) et l'invalide sur EngramStore::revision — la reconstruction coûtait 79-85% du chemin de recall (4.8 ms à n=2000)
- obrain-cognitive est #![deny(unsafe_code)] et wasm32-compatible (web_time::Instant/SystemTime avec feature serde) ; le ConsolidatorRuntime encapsule un runtime tokio mono-thread dédié nommé 'synapse-consolidator'

**Trous / incohérences.**
- FabricStore n'a PAS de mode substrate (contrairement à EnergyStore/SynapseStore/ScarStore) — il reste en legacy write-through propriétés graphe ; incohérence dans la migration T6
- knowledge_gap (gap-detection) pèse 0.0 par défaut dans RiskWeights : la détection de trous structurels est calculée mais jamais injectée dans le risque sans activation manuelle
- L'objectif mentionne des « embeddings » : aucun modèle d'embedding ni code d'inférence vectorielle n'existe dans obrain-rag — la dimension vectorielle repose uniquement sur SpectralEncoder + InMemoryVectorIndex (pas d'embedding sémantique externe)
- Double implémentation legacy/substrate maintenue dans energy.rs, synapse.rs, scar.rs — code legacy conservé « for test parity » mais toujours compilé (risque de divergence et de maintenance)
- obrain-rag/src/traits.rs n'a pas été inspecté en profondeur (traits RetrievalSource/RetrievedNode consommés partout) ; examples/rag_demo.rs et tests/membench_recall.rs non ouverts
- Le module engram (pub mod engram) est compilé sans feature (traits/types toujours disponibles) alors que procedures.rs est gaté par feature 'engram' — frontière de feature à clarifier
- FabricListener::on_event recalcule le risque uniquement dans on_batch (on_event n'incrémente que mutation_frequency sans recalcul de risque) — asymétrie possible entre chemins event/batch



### 1.3 Ingestion / indexation / persistance (obrain-indexer, obrain-migrate, obrain-journal, neo4j2obrain)

_Fichier : `raw/v1-3-ingestion-persistance.json` — 39 composants · 14 relations · 10 constats · 9 trous_


**Synthèse.** La tranche couvre quatre crates qui forment le pipeline d'ingestion→persistance du graphe obrain. obrain-indexer (RFC M6) remplace l'indexeur C codebase-memory-mcp : il découvre les fichiers d'un dépôt, les parse via tree-sitter (8 langages prioritaires + framework spec + pack de 371 grammaires), résout les appels par une chaîne de confiance et produit un CodeGraph en mémoire (nœuds Project/Function/…, arêtes CALLS/DEFINES/IMPORTS/…) ; l'écriture substrate en bulk est encore hors du crate (seul examples/persist.rs l'effectue via obrain-engine). obrain-migrate est le convertisseur one-shot legacy .obrain → substrate, découpé en phases rejouables (StoreInit→Nodes→Edges→Cognitive→Tiers→CognitiveInit→Done) avec checkpoint JSON et plusieurs modes in-place (--finalize, --checkpoint, --finalize-v2, --upgrade-edges-v2). obrain-journal n'est pas un journal CDC/événements au sens classique : c'est une chaîne de hachage signée Ed25519 de reçus (empreintes), destinée à la garantie de confidentialité/audit (ADR-2026-09-15-GARANTIE-CONFIDENTIALITE-CALCUL). neo4j2obrain est l'importeur streaming Neo4j (bolt) → substrate qui exécute un pipeline cognitif inline (embeddings, tiers L0/L1/L2, LDleiden, PageRank, Ricci, COACT, engrammes, puis _hilbert_features/_kernel_embedding) pour que la base soit cognitivement vivante à l'ouverture. Les notions « slots512 » (packing de chaînes de propriétés 7×512 o) et « zorder/Hilbert » (courbe de Hilbert, permutation physique des slots) vivent dans obrain-substrate et ne touchent ces crates que par effet de bord (obrain-migrate saute _hilbert_features/_kernel_embedding).


**Composants clés.**
- crates/obrain-indexer/src/lib.rs — entrée du crate, déclaration des 17 modules, INDEXER_CONTRACT_VERSION=1, réexports CodeGraph/Edge/Node/NodeKind/EdgeKind
- crates/obrain-indexer/src/index.rs — orchestration index_repository(): pipeline 2 phases rayon (structure+extract puis resolve), FQN, contexte git, passes metrics, historisation
- crates/obrain-indexer/src/discovery.rs — découverte des fichiers + politique d'exclusion comptée et motivée (Raison::Versionnage/Dependances/SortieDeBuild/…, budget 5 Mo)
- crates/obrain-indexer/src/schema.rs — contrat cible: 28 NodeKind (Project…Constructor) et 40 EdgeKind (CALLS/DEFINES/IMPORTS/…/CROSS_GRPC_CALLS) miroir du contrat C
- crates/obrain-indexer/src/graph.rs — CodeGraph en mémoire: buffers nœuds/arêtes + index by_qn/by_id/by_kind + colonnes de métriques + top-caches + compteurs booléens
- crates/obrain-indexer/src/parse.rs — trait Language (« une grammaire = un module »), FileExtraction (definitions/calls/imports/bindings/struct_fields), LanguageRegistry
- crates/obrain-indexer/src/resolve.rs — SymbolRegistry + chaîne de confiance (import_map 0.95 … suffix 0.55) + passe « récepteur » (self/type/import/binding/fn_return/champ)
- crates/obrain-indexer/src/lang/mod.rs — build_registry(): 8 langages prioritaires + spec_langs::register + pack_langs::register
- crates/obrain-indexer/src/lang/rust.rs — grammaire Rust de référence (impl/traits/modules, tests/benches isolés, super::/crate::)
- crates/obrain-indexer/src/lang/python.rs — grammaire Python (fonctions, classes, décorateurs, imports)
- crates/obrain-indexer/src/lang/c.rs — grammaire C (fonctions, structs, includes)
- crates/obrain-indexer/src/lang/cpp.rs — grammaire C++ (classes, méthodes, héritage, templates)
- crates/obrain-indexer/src/lang/go.rs — grammaire Go (majuscule = exporté, packages, méthodes)
- crates/obrain-indexer/src/lang/java.rs — grammaire Java (classes, interfaces, méthodes)
- crates/obrain-indexer/src/lang/javascript.rs — grammaire JavaScript (fonctions, classes, require/import)
- crates/obrain-indexer/src/lang/typescript.rs — grammaire TypeScript (interfaces, types, décorateurs)
- crates/obrain-indexer/src/spec_engine.rs — moteur de grammaires pilotées par spec JSON (885 lignes, calcul de métriques par branch types)
- crates/obrain-indexer/src/pack_langs.rs — enregistrement des langages manquants via tree-sitter-language-pack
- crates/obrain-indexer/src/metrics.rs — passes complexity_pass/tests_pass (transitive_loop_depth, récursif, arêtes TESTS)
- crates/obrain-indexer/src/git_history.rs — analyse git log --name-only → change_count/last_modified + arêtes FILE_CHANGES_WITH (couplage de co-évolution)
- crates/obrain-indexer/src/git_ctx.rs — capture du contexte git (branche, base_sha/head_sha, worktree) → nœud Branch + HAS_BRANCH
- crates/obrain-indexer/examples/persist.rs — SEULE écriture substrate du graphe M6 via obrain_engine::ObrainDB::open_substrate + create_property_index("qualified_name")
- crates/obrain-indexer/Cargo.toml — dépend de obrain-engine (features gql/storage/parallel/wal/spill/mmap/regex) + ~51 grammaires tree-sitter
- crates/obrain-migrate/src/main.rs — CLI clap: modes migrate + --finalize + --checkpoint + --finalize-v2 + --upgrade-edges-v2 + --dry-run/--no-backup + filtres de propriétés
- crates/obrain-migrate/src/converter.rs — cœur (2778 lignes): migrate() + phase_nodes/phase_edges/phase_cognitive/phase_tiers/phase_cognitive_init + finalize/finalize_v2/upgrade_edges_v2/checkpoint
- crates/obrain-migrate/src/checkpoint.rs — Phase (StoreInit→Done) + Checkpoint JSON atomique .migrate-checkpoint.json (resume idempotent)
- crates/obrain-migrate/src/legacy_reader.rs — open_legacy(): dispatch fichier/répertoire, mais les deux chemins bail() (lecteur legacy retiré au cutover T17)
- crates/obrain-journal/src/lib.rs — doc du contrat: chaîne/signature/reçu, ce qui est prouvé et ce qui ne l'est pas
- crates/obrain-journal/src/journal.rs — Journal: recevoir()/verifier()/verifier_signatures()/accorde_avec()/sequences_manquantes + sérialisation binaire MAGIC OBJRNL01
- crates/obrain-journal/src/cle.rs — CleJournal: clé Ed25519 (engendrer/depuis_octets/publique/signer/effacer), zeroize du secret
- crates/obrain-journal/src/entree.rs — Entree à champs fixes (journal/sequence/horodatage/quoi/precedent/signature), empreinte SHA-256 à domaine, empreinte_de/empreinte_du_journal/genese
- crates/obrain-journal/src/erreur.rs — ErreurJournal: SignatureInvalide/ChaineRompee/TeteDifferente/JournalEtranger/SequenceAbsente/ElementDifferent
- crates/neo4j2obrain/src/lib.rs — cible bibliothèque + init_runtime() (souscripteur tracing + pool rayon idempotents), réexport run/PipelineOptions/DEFAULT_TEXT_KEYS
- crates/neo4j2obrain/src/main.rs — CLI: --neo4j-url/--user/--password/--out/--cognitive/--enrich-only/--embeddings-model/--st-model-path/--text-keys
- crates/neo4j2obrain/src/pipeline.rs — orchestration run(): 10 étapes (bolt→embeddings→tiers→LDleiden→PageRank→Ricci→COACT→engrams→hilbert→kernel) + probe_stage_coverage + open_or_create_substrate
- crates/neo4j2obrain/src/bolt_reader.rs — étape 1: streaming Neo4j bolt par fenêtres (comptage source, pre-dimensionnement id_map, traduction Value via value_from_bolt)
- crates/neo4j2obrain/src/embedder.rs — étape 2: chargement SentenceTransformer (.onnx + vocab.txt) aligné sur le runtime obrain-hub
- crates/neo4j2obrain/src/sentence_transformer.rs — inference ONNX WordPiece 384-dim thread-local (load_with_threads, intra_threads=1 par worker rayon)
- crates/neo4j2obrain/src/cognitive_init.rs — étapes 4-6: run_ldleiden_batch (+variante ricci_boost), PageRank (damping 0.85), refresh_all_ricci/compute_all_node_curvatures

**Relations / dépendances.**
- obrain-indexer → obrain-engine (features gql/storage/parallel/wal/spill/mmap/regex) + obrain-common + tree-sitter (51 grammaires listées dans Cargo.toml)
- obrain-indexer::index_repository → discover() → lang::build_registry() → lang.extract() (rayon) → SymbolRegistry → resolve() → metrics::complexity_pass/tests_pass → git_history::analyze → CodeGraph
- obrain-indexer/examples/persist.rs → obrain_engine::ObrainDB::open_substrate + create_node/set_node_property/create_edge/set_edge_property + create_property_index("qualified_name")
- obrain-cli → obrain-indexer (feature `indexer`, active par défaut — crates/obrain-cli/Cargo.toml:101)
- obrain-migrate → obrain-core (feature tiered-storage) + obrain-common + obrain-substrate (SubstrateStore, PropertiesStreamingWriter, BLOB_COLUMN_THRESHOLD_BYTES)
- obrain-migrate::converter::migrate → phase_nodes → phase_edges → phase_cognitive → phase_tiers → phase_cognitive_init, checkpointées par checkpoint::Phase (PartialOrd = ordre d'exécution)
- obrain-migrate écrit substrate.obrain dans --out via SubstrateStore + PropertiesStreamingWriter (remplace set_node_property pour borner le RSS)
- obrain-journal → ed25519-dalek + sha2 + getrandom + zeroize + hex (aucune dépendance obrain-*) ; consommé par obrain-iam et obrain-aveugle (workspace)
- neo4j2obrain → obrain-core + obrain-common + obrain-substrate + obrain-cognitive (feature community, optional) + obrain-adapters (feature algos) + neo4rs/tokio/chrono (optional) + ort (optional)
- neo4j2obrain::pipeline::run → stage_bolt_stream → stage_fill_embeddings → stage_build_tiers → cognitive_init::run_ldleiden_batch/PageRank/Ricci → stage_derive_coact → stage_seed_engrams → stage_fill_hilbert_features → stage_fill_kernel_embeddings
- neo4j2obrain --enrich-only consomme les bases produites par obrain-migrate (structure sans embeddings/tiers/communautés/engrammes)
- obrain-migrate::converter saute _st_embedding/_kernel_embedding/_hilbert_features (is_embedding_key) — les colonnes cognitives sont réémises en tant que colonnes substrate (cog_keys _cog_energy/_cog_scar/_cog_utility/_cog_affinity/_syn_weight/_community_id)
- slots512 / zorder / Hilbert : implémentés dans obrain-substrate (props_zone.rs packing 7×512o, hilbert.rs + bulk_sort_by_hilbert + window_query_2d), non dans les 4 crates cibles
- obrain-migrate et neo4j2obrain sont des binaires feuilles (aucun dépendant in-tree hors membership workspace) ; obrain-indexer et obrain-journal sont des bibliothèques réutilisées

**Points notables.**
- Décalage doc/code: crates/obrain-indexer/src/lib.rs annonce encore « Squelette initial (RFC en cours de validation) » alors que index.rs est un pipeline complet parallélisé (1064 lignes) avec résolution, métriques et historique git.
- Le crate indexeur ne persiste PAS en substrate: index_repository() retourne un CodeGraph en mémoire ; seule examples/persist.rs écrit réellement le graphe, via obrain_engine::ObrainDB::open_substrate (et non obrain-substrate directement).
- obrain-migrate/src/legacy_reader.rs est intégralement retiré: open_legacy_dir() et open_legacy_file() bail() tous deux — la feature `legacy-read` est donc du code mort et l'outil ne peut plus convertir un .obrain legacy.
- obrain-journal n'est pas un journal CDC/événements: c'est une chaîne de hachage signée Ed25519 de reçus d'empreintes (audit de confidentialité), sans persistance de contenu — il ne stocke que des SHA-256.
- neo4j2obrain compilé sans features (`cargo build -p neo4j2obrain`) NE SAIT ni lire Neo4j (pas de neo4j-bolt) ni calculer d'embeddings (pas de embed) — c'est un enrichisseur de substrat silencieux, documenté dans lib.rs.
- Le pipeline neo4j2obrain compte en réalité 10 étapes, pas 8 comme le dit l'en-tête de main.rs : les étapes 9 (_hilbert_features 64-72d) et 10 (_kernel_embedding 80d) ont été ajoutées en T17l.
- Chemin macOS codé en dur par défaut dans neo4j2obrain/main.rs: st_model_path/st_vocab_path pointent vers /Users/triviere/projects/obrain/obrain-chat/models/… (portabilité cassée hors macOS/ce compte).
- Stage 7 (COACT) est non idempotent: create_edge_with_props sans test de préexistence (pipeline.rs:1092-1101), donc _hilbert_features dépend de l'historique des ré-exécutions (documenté dans docs/PROPOSAL-OPERATEUR-SPECTRAL-COURBE-2026-09-10.md).
- « slots512 » est un packing de chaînes de propriétés (7×512 octets dans un carrier 4 Ko, props_zone.rs) désactivé par défaut (OBRAIN_PROPS_SLOTS_512=1), pas un schéma spatial ; « zorder/Hilbert » (feat/zorder) est la courbe de Hilbert + permutation physique des slots dans obrain-substrate.
- obrain-migrate gère finement la mémoire des gros imports: drop du scope legacy avant flush, PropertiesStreamingWriter, filtres --skip-prop/--max-prop-bytes/--skip-all-props pour éviter l'OOM (megalaw ~7 Go).

**Trous / incohérences.**
- Écriture substrate bulk de l'indexeur absente du crate (RFC M6 §5 point 4 non réalisé) — la persistance passe par examples/persist.rs seulement.
- Feature `legacy-read` d'obrain-migrate est un trompe-l'œil: les deux lecteurs legacy bail() → aucune migration legacy réelle possible avec ce binaire.
- obrain-journal n'a pas de contrat de persistance disque (nom de fichier/rotation/répertoire) — seuls vers_octets()/depuis_octets() existent, l'intégration au chemin d'écriture du moteur reste externe.
- Horodatage du journal déclaré par l'opérateur (non attesté) — la chaîne prouve l'ordre mais pas la date (limite documentée).
- Chemin de modèle ONNX/vocab codé en dur (/Users/triviere/…) dans neo4j2obrain/main.rs — à externaliser.
- Stage 7 (COACT) non idempotent: ré-exécuter le pipeline ajoute des arêtes en double.
- Le périmètre demandé « migration (slots512, zorder) » n'appartient pas aux 4 crates : ces primitives vivent dans obrain-substrate ; obrain-migrate ne fait que sauter les colonnes _hilbert_features/_kernel_embedding.
- Les types de propriétés Neo4j non supportés (Geo, Date) sont silencieusement abandonnés (warn une fois) dans bolt_reader.rs — pas de comptage des pertes.
- obrain-indexer/src/lib.rs (en-tête « squelette ») et index.rs (complet) divergent : la documentation d'en-tête est périmée.



### 1.4 Sécurité + auto-orchestration (IAM, TLS, coffre, garde, self, selflink, aveugle)

_Fichier : `raw/v1-4-securite-self-orchestration.json` — 43 composants · 18 relations · 10 constats · 11 trous_


**Synthèse.** La tranche couvre la chaîne complète de confidentialité et d'auto-orchestration d'obrain : de l'identité (obrain-iam) au transport chiffré (obrain-tls, étape 0 de l'ADR), à la confidentialité structurelle du calcul (obrain-coffre, clés par espace), au transport aveugle (obrain-aveugle, étape 3 / variante V-B), en passant par l'arrêt maîtrisé des générations (obrain-garde) et l'action à distance (obrain-selfd / obrain-selflink). La séparation de doctrine est nette : l'IAM rend un verdict (Allow/Deny, évalué par policy.rs et posé par Garde dans garde.rs), tandis que le coffre remplace le verdict par l'absence de clé — « une politique qui a tort ne le dit pas ; une clé absente ne peut pas avoir tort ». Deux primitives partagées portent la garantie : obrain-journal (chaînage + signature) est consommé par RegistreAudit (audit_chaine.rs) et par le Porteur aveugle (porteur.rs), ce qui rend l'histoire d'audit et le routage non reniables. La garde d'entrée IAM adopte une posture fail-closed (OBRAIN_IAM=1) et bruyante quand non armée, avec rotation de session, révocation réelle et expiration vérifiée (correctifs du 2026-09-13). obrain-garde (crate, sans aucune dépendance) centralise l'arrêt de génération (budget total, répétition, client parti, index d'appel aberrant) pour les boucles de selfd. selfd est un micro-harness binaire ~750 Ko à deux dépendances qui agit sur la machine (exec/read/write/list, politique figée au démarrage, --allow-exec refusé par défaut, anti-évasion de racine par canonicalisation) et s'ancre sur le self lu via /api/viz/weave, tandis que selflink est le client HTTP minimal symétrique (lecture seule) du même moteur.


**Composants clés.**
- crates/obrain-iam/src/lib.rs — façade IAM : re-exporte Orn, Garde, IamStore, AuthProvider, AuthenticatedSession, model
- crates/obrain-iam/src/orn.rs — Obrain Resource Name (orn:obrain:service:account:type/path), wildcards */**, substitution ${tenant}
- crates/obrain-iam/src/model.rs — User/Role/Policy/Credential/AuditEvent, labels (:IamUser…), arêtes HAS_ROLE/HAS_POLICY/HAS_CREDENTIAL/PERFORMED
- crates/obrain-iam/src/policy.rs — evaluate() : Deny > Allow > ImplicitDeny, action_prefix et resource wildcard matching
- crates/obrain-iam/src/store.rs — IamStore : CRUD graphe-backed sur __system, évalue l'accès, journalise (1302 lignes)
- crates/obrain-iam/src/provider.rs — trait AuthProvider + ObrainIamProvider : authenticate (Password/SessionToken/ApiKey/Internal/OidcToken), create_session, revoke, federation_token, assume_role
- crates/obrain-iam/src/garde.rs — Garde (garde d'entrée) : connecter/authentifier/controler/deconnecter/renouveler, posture fail-closed, variables OBRAIN_IAM*
- crates/obrain-iam/src/session_ext.rs — AuthenticatedSession : check_access/check_read/check_write, tenant scope, compteur d'accès
- crates/obrain-iam/src/audit_chaine.rs — RegistreAudit : audit chaîné+signé via obrain_journal, empreinte_evenement (verdict couvert)
- crates/obrain-iam/src/gdpr.rs — GdprManager : consentement, effacement (certificat), rétention TTL, export (portabilité)
- crates/obrain-iam/src/admin.rs — Administration : gestion des comptes partagée CLI/serveur, plafond, anti-verrouillage, traçage (1918 lignes)
- crates/obrain-iam/src/error.rs — IamError (InvalidOrn, AccessDenied, PermissionDenied, ResourceNotFound/Exists…)
- crates/obrain-iam/tests/la_cle_prime_sur_le_droit.rs — frontière : un Allow de la garde n'ouvre pas un espace scellé
- crates/obrain-iam/tests/audit_chaine.rs — l'audit chaîné confronté au reçu
- crates/obrain-tls/src/lib.rs — étape 0 ADR : transport chiffré des surfaces, sans repli en clair par construction
- crates/obrain-tls/src/config.rs — ConfigTls (certificat+clé obligatoires), fournisseur ring, Debug sans clé
- crates/obrain-tls/src/ecoute.rs — EcouteTls/FluxTls (Read+Write), délai de poignée de main
- crates/obrain-tls/src/erreur.rs — ErreurTls (avant_toute_ecoute, CleEtCertificatIncoherents…)
- crates/obrain-coffre/src/lib.rs — confidentialité structurelle : clé = autorisation, garanties G1-G5
- crates/obrain-coffre/src/cle.rs — hiérarchie de clés : Argon2id (plancher vérifié) → KEK → DEK → sous-clés HKDF par usage
- crates/obrain-coffre/src/coffre.rs — Coffre : scellement par blocs AEAD, sceau d'en-tête, IdCoffre tiré au sort anti-greffe
- crates/obrain-coffre/src/enveloppe.rs — Enveloppe : bourrage à 5 classes fixes (256→65536), G3
- crates/obrain-coffre/src/jeton.rs — Jeton de capacité (Portee) : action opaque, taille fixe 130 octets
- crates/obrain-coffre/src/depot.rs — ecrire_enveloppe/lire_enveloppe (persistance disque)
- crates/obrain-coffre/src/aleatoire.rs, enveloppe.rs, erreur.rs — entropie OS, erreurs CoffreError (CleAbsente est_une_impossibilite)
- crates/obrain-garde/src/lib.rs — la garde d'une génération : pure, sans E/S, sans dépendance
- crates/obrain-garde/src/garde.rs — Garde/Motif/Reglages/Veille : budget total, client_parti, prose, index_appel_valide
- crates/obrain-garde/src/repetition.rs — Repetition : détection de bloc/ligne répétés, prudente (<600 chars ⇒ ne conclut pas)
- crates/obrain-aveugle/src/lib.rs — étape 3 ADR, variante V-B : le porteur transporte sans pouvoir lire
- crates/obrain-aveugle/src/adresse.rs — Adresse 16 octets tirés au sort (jamais dérivée d'un nom)
- crates/obrain-aveugle/src/message.rs — Charge scellée, sceller/ouvrir (détournement détecté), taille_conforme
- crates/obrain-aveugle/src/porteur.rs — Porteur : dépôt/retrait, anti-rejeu, quota, journal de transitions signé
- crates/obrain-selfd/src/main.rs — selfd : CLI serve/mcp/stdio/attacher/--call, politique figée au démarrage
- crates/obrain-selfd/src/agent.rs — derouler() : boucle harness ancrée sur le self, bornes, Evenement (Texte/Acte/Rend/Fin/Borne/Arret)
- crates/obrain-selfd/src/soi.rs — Socle : lit le self (/api/viz/weave, /api/engine/memoire, /api/query), lecture seule
- crates/obrain-selfd/src/politique.rs — Politique : racine, exec_autorise (défaut false), resoudre() anti-évasion par canonicalisation
- crates/obrain-selfd/src/outils.rs — exec/read/write/list : pas de shell implicite, sortie drainée, délai = délai
- crates/obrain-selfd/src/protocole.rs — Etat partagé + JSON-RPC : analyser/executer_outil/repondre, un seul traiter pour 3 surfaces
- crates/obrain-selfd/src/identite.rs — Identite persistée (node:<hôte>-<hex>), stable à travers redémarrages
- crates/obrain-selfd/src/mcp.rs, attache.rs, chat.rs, ecoute.rs, serveur.rs — surfaces MCP stdio, branchement moteur, chat SSE, HTTP, socket Unix
- crates/obrain-selflink/src/main.rs — selflink : info/stores/mondes/hubs/self/memoire/query/ask/sante/login
- crates/obrain-selflink/src/engine.rs — Moteur : client HTTP minimal, resoudre_store() (jamais deviner), demander() SSE
- docs/ADR-2026-09-15-GARANTIE-CONFIDENTIALITE-CALCUL.md — ADR de référence : étapes 0 (TLS), 3 (aveugle), §4.1 (audit signé), variante V-B

**Relations / dépendances.**
- obrain-iam → obrain-core (GraphStoreMut), obrain-common (Value/NodeId), obrain-journal (RegistreAudit chaîné+signé), bcrypt/sha2/rand
- obrain-iam Garde::nouvelle → IamStore::new + ObrainIamProvider (provider = Arc<dyn AuthProvider>, emetteur concret pour set_user_password/revoquer_jeton)
- obrain-iam garde::Garde::controler → provider.authorize → store.evaluate_access → policy::evaluate (Deny>Allow>ImplicitDeny)
- obrain-iam ObrainIamProvider::authenticate_by_token_hash → lit EXPIRES_TS (expiration VÉRIFIÉE), remonte User -[:HAS_CREDENTIAL]-> Credential
- obrain-iam audit_chaine::RegistreAudit → obrain_journal::{Journal,CleJournal,Entree,empreinte_de} (DOMAINE obrain-iam/v1/audit)
- obrain-iam [dev] → obrain-substrate (SubstrateStore), obrain-coffre (test la_cle_prime_sur_le_droit : verdict IAM ≠ clé coffre)
- obrain-engine → obrain-cognitive → obrain-iam (feature optionnelle 'iam'), obrain-cli → obrain-iam (feature 'iam' = cognitive-iam)
- obrain-cli → obrain-garde (la boucle chat du moteur traverse la garde d'arrêt)
- obrain-selfd → obrain-httpmin (client SSE supervise), obrain-garde (Garde/Reglages/index_appel_valide), anyhow, serde_json — 2 dépendances seulement
- obrain-selflink → obrain-httpmin (get/post_json/lire_sse), anyhow, serde_json — 2 dépendances seulement
- obrain-aveugle → obrain-coffre (CleEspace, Enveloppe, ClasseTaille), obrain-journal (journal de transitions), getrandom, hex
- obrain-aveugle Porteur : ne détient AUCUNE CleEspace (propriété de type), journalise chaque transition via Journal::recevoir
- obrain-aveugle message::sceller/ouvrir → obrain_coffre::Enveloppe::sceller_dans/desceller (destination scellée dedans, détournement détecté)
- obrain-tls → rustls (fournisseur ring, with_no_client_auth), rustls-pemfile — aucun dépendant interne (étape 0 isolée)
- obrain-coffre : CleEspace → sous_cle(Usage) par HKDF-SHA256 (coffre/entete-coffre/enveloppe/jeton), aucune dépendance interne
- selfd : 4 surfaces (socket/MCP/stdio/CLI) appellent le même protocole::executer_outil ; agent::derouler → outils::exec/read/write/list + soi::requete
- selfd attache::boucle ↔ moteur 8432 avec jeton IAM (SELFD_MOTEUR_TOKEN), distinct du jeton de surface locale
- selflink Moteur ↔ serveur viz : /api/info, /api/stores, /api/worlds, /api/query, /api/engine/memoire, /api/engine/chat, /api/auth/login

**Points notables.**
- Séparation de doctrine IAM vs coffre explicite et testée : l'IAM rend un verdict (policy.rs), le coffre n'a pas de verdict — la clé EST l'autorisation (lib.rs coffre + test la_cle_prime_sur_le_droit.rs)
- L'audit IAM était décrit « immutable » sans chaînage ni signature ; audit_chaine.rs le branche sur obrain-journal (ADR §4.1 réserve n°1), l'empreinte couvre le VERDICT (Deny→Allow = attaque la plus rentable) et chaque champ est préfixé par sa longueur (anti-ambiguïté de concaténation)
- Quatre défauts de sécurité réels corrigés et tracés au 2026-09-13 : jeton PRÉVISIBLE (DefaultHasher+compteur → entropie 256 bits), expiration annoncée mais JAMAIS vérifiée (EXPIRES_TS), révocation no-op (déconnexion laissait le jeton vivant), hash de jeton SipHash 64 bits sans sel → SHA-256
- obrain-garde est un crate SANS AUCUNE dépendance (pur std) : « une garde qui tirerait tokio/anyhow/HTTP ne serait pas adoptable par les boucles qui en ont le plus besoin » — appelable depuis selfd (2 dépendances)
- Le coffre lie l'identifiant du coffre (16 octets tirés au sort) dans l'AAD de chaque bloc : un test a trouvé que deux coffres de même espace/forme avaient un en-tête identique octet pour octet, permettant la greffe de blocs (G4)
- Attaque par abaissement des paramètres Argon2id fermée à deux étages : plancher vérifié AVANT dérivation (deriver_kek), et paramètres couverts par l'AAD (modification au-dessus du plancher casse l'authentification)
- Le Porteur aveugle est aveugle PAR TYPE : aucun champ/argument/méthode ne lui donne accès à CleEspace ; il impose quand même forme/non-rejeu/quota sans lire, et journalise ses transitions en signé (routage non reniable)
- TLS : aucun repli en clair possible par construction (ConfigTls obligatoire, pas de par_defaut/sans_tls) ; contrepartie assumée : ring ⇒ pas de post-quantique, pas de mTLS, pas de rotation à chaud
- selfd : politique fixée au démarrage et jamais négociée (« l'absence de décision vaut refus »), --allow-exec refusé par défaut, anti-évasion par canonicalisation du chemin (.., symlink, absolu détourné), pas de shell implicite
- selflink expose trois référentiels de « mondes » divergents (graphe / presets /api/stores / registre /api/worlds) et refuse de deviner un --store nu (un nom nu est résolu relatif au cwd du serveur et lit une base vide SANS RIEN DIRE)

**Trous / incohérences.**
- Aucune route de remise de jeton dans le serveur : les jetons se créent hors bande (create_user/create_credential ou AuthProvider::create_session) — armer la garde suppose de fabriquer le premier jeton à la main (garde.rs § CE QUI MANQUE)
- OIDC/JWT non implémenté : AuthRequest::OidcToken rend InvalidParameter « requires T1.7 (JWT/Ed25519) » (provider.rs)
- Federation token : « For now we trust the caller and just record them » — pas de validation d'intersection des scoped_actions ; assume_role : le scoping de rôle « sera appliqué par AuthenticatedSession en T1.4 » (pas encore)
- TLS : pas de mTLS, pas de sélection SNI (un seul certificat), pas de rotation/ACME/révocation (rechargement au redémarrage), pas de post-quantique, fuite de métadonnées (G3) hors périmètre
- Coffre : la longueur exacte du contenu reste lisible dans l'en-tête en clair (mesuré par attaques_externes.rs) — bourrage du coffre = format v2 + gonflement ; la substitution d'un coffre ENTIER n'est détectable que via desceller_avec_id
- Jeton de capacité : pas d'anti-rejeu (relisibilité jusqu'à échéance), horloge fournie par l'appelant (un opérateur qui recule son horloge ressuscite un jeton expiré) — parade côté client hors crate
- Aveugle : les métadonnées (qui parle à qui, quand, volume) restent visibles ; adresses stables ⇒ corrélation temporelle possible ; seulement 5 classes de taille (256→65536) ; ne force pas l'adoption (l'engine peut servir en clair)
- Les surfaces n'adoptent pas encore TLS/audit/aveugle : « les surfaces doivent adopter » (lib.rs tls/aveugle, audit_chaine.rs) — le code fournit le chemin mais ne l'impose pas
- selfd/selflink ne peuvent pas ÉCRIRE dans le self : aucune route HTTP d'écriture côté moteur (consolidate/import_events/memory_register passent par le bridge MCP stdio local) — les notes restent locales
- Collision de nommage : deux « garde » distinctes coexistent (obrain-iam::garde::Garde = contrôle d'accès ; obrain-garde::Garde = arrêt de génération) — risque de confusion lors d'audits croisés
- gdpr.rs utilise un now_iso() qui est un COMPTEUR AtomicU64 (format « 2026-01-01T00:00:NNZ »), pas une horloge réelle — incohérent avec l'expiration par époque (maintenant_ts) ailleurs dans la même crate



### 1.5 Interfaces & intégrations — CLI, HTTP minimal, pipeline réactif, adaptateurs, restitution viz, bindings FFI/napi/wasm

_Fichier : `raw/v1-5-interfaces-integrations.json` — 39 composants · 14 relations · 10 constats · 10 trous_


**Synthèse.** La tranche des surfaces d'entrée/sortie d'obrain est organisée autour de six crates qui enveloppent l'engine. `obrain-cli` est le binaire `obrain` (~40 sous-commandes clap) : admin, query one-shot, REPL rustyline, pont MCP JSON-RPC stdio, daemons HTTP std-only (`viz serve`, `harness serve`), pont LLM vers un llama-server OpenAI-compatible, et un client HTTP minimal maison. `obrain-httpmin` est un client HTTP std-only GET/POST extrait pour dédupliquer `selflink`/`selfd`, mais le CLI garde encore sa propre copie (`src/http_min.rs`). `obrain-reactive` fournit le substrat événementiel (MutationBus tokio::broadcast, Scheduler, InstrumentedStore, trait MutationListener) à coût nul quand inutilisé et compatible WASM. `obrain-adapters` est la couche d'intégration : parsers GQL/Cypher/SPARQL/Gremlin/GraphQL/SQL-PGQ, stockage WAL, et registre de plugins/algorithmes. `obrain-viz` est une lib pure (sans dépendance substrate) qui transforme l'IR Subgraph en json-graph v1/v2, DOT, Mermaid, GraphML, avec overlays cognitifs (ricci/energy/roles) et diff/timeline. `bindings/` expose le moteur en 7 langages : C FFI (base), Node (napi), Python (pyo3 + bridges NetworkX/solvOR/pandas/polars), WASM (wasm-bindgen), Go/Dart/C# (tous empilés sur la couche C).


**Composants clés.**
- crates/obrain-cli/src/main.rs — binaire `obrain`, struct `Cli` (clap), enum `Commands` (~40 sous-commandes : info/query/shell/init/compact/repair*/enrich/resonance/continuity/compare/validate/index/backup/data/wal/topo/jump/journal/viz/iam/memoire/entretien/harness-serve/mcp/bridge/…), `OutputFormat` auto/table/json/csv, `ExitCode`, allocateurs jemalloc/mimalloc exclusifs
- crates/obrain-cli/src/commands/mod.rs — `open_existing()` + 28 modules de commandes (backup, compact, compare, data, enrich, iam, index, info, init, journal, jump, memoire, native_import, query, repair, resonance, schema, stats, topo, validate, version, viz, wal, …)
- crates/obrain-cli/src/commands/query.rs — exécution one-shot (--query/--file/--stdin, --param k=v, --lang, --timing, --read-only, --slots)
- crates/obrain-cli/src/repl.rs — REPL rustyline `obrain shell` (historique, transactions, :help/:quit/:timing)
- crates/obrain-cli/src/engine.rs — pont LLM : extrait le voisinage réel via extract_subgraph, journalise l'activation, interroge un serveur OpenAI-compatible (llama-server 127.0.0.1:9901), lit reasoning_content puis content
- crates/obrain-cli/src/mcp_server.rs — mode bridge MCP JSON-RPC stdio (initialize/tools/list/tools/call) : search_graph, runtime_search, sessions_list (mémo-cache réservé OBRAIN_WRITE=1), context_build, import_events, get_graph_schema
- crates/obrain-cli/src/harness_serve.rs — daemon HTTP std-only lecture seule (T1) : /api/harness/health, /api/harness/sessions
- crates/obrain-cli/src/viz_serve.rs — `viz serve` HTTP std-only : GET /api/viz/subgraph, /api/stores, gzip, ETag, cache de requêtes, instrumentation de phases OBRAIN_SURFACE_PHASES
- crates/obrain-cli/src/viz_ws.rs — primitives WebSocket RFC6455 std-only (handshake SHA-1 + base64, frames texte non masquées, close)
- crates/obrain-cli/src/viz_extract.rs — extracteur de sous-graphes substrate (BFS borné, hops 0..=8, signaux energy/ricci/weight/stale, rôle SCC), source unique CLI + MCP viz_subgraph
- crates/obrain-cli/src/http_min.rs — client HTTP minimal std-only du CLI (POST/GET, Content-Length ou chunked, pas de TLS/redirection) — DOUBLON de obrain-httpmin
- crates/obrain-cli/src/output/mod.rs + output/formatter.rs — sorties table (comfy-table)/JSON/CSV, auto-détection TTY, NO_COLOR, format_bytes/format_duration_ms
- crates/obrain-cli/src/commands/viz.rs — `obrain viz subgraph|export` (json-graph v1 / DOT / Mermaid / GraphML, overlays ricci/energy/roles/fsrs/engrams/pheromone)
- crates/obrain-httpmin/src/lib.rs — client HTTP minimal std-only partagé : parse_url, echapper (RFC3986), avec_params, Reponse::json, envoyer (Content-Length | chunked | corps-à-EOF), DELAI_DEFAUT 180s
- crates/obrain-reactive/src/lib.rs — façade : MutationBus, ReactiveError, MutationEvent/MutationBatch, MutationListener/TenantFilteredListener, Scheduler/BatchConfig, InstrumentedStore
- crates/obrain-reactive/src/bus.rs — MutationBus (tokio::broadcast, capacité 16384, publish < 5µs sans abonnés)
- crates/obrain-reactive/src/event.rs — MutationEvent, MutationBatch, EventContext (tenant_id/principal_arn/session_id), NodeSnapshot, EdgeSnapshot
- crates/obrain-reactive/src/listener.rs — trait MutationListener (on_batch/on_event), TenantFilteredListener
- crates/obrain-reactive/src/scheduler.rs — Scheduler (tokio::task natif, dispatch synchrone WASM, batching max_batch_size 100 / max_delay 50ms)
- crates/obrain-reactive/src/store.rs — InstrumentedStore<S: GraphStoreMut> : intercepte les mutations, buffer pending, drain_pending → MutationBatch au commit
- crates/obrain-adapters/src/lib.rs — couche d'intégration : plugins, query, storage
- crates/obrain-adapters/src/query/ — parsers lexer/parser/ast pour gql, cypher, sparql, gremlin, graphql, sql_pgq + keywords
- crates/obrain-adapters/src/plugins/ — PluginRegistry, traits Plugin/Algorithm/UserDefinedFunction, registre_global, installer_udfs_dans_le_moteur, algorithms/ (~25 fichiers : centrality, community, spectral, shortest_path, kernel, heat_diffusion, mst, ego_graph, …)
- crates/obrain-adapters/src/storage/wal/ — WalManager, WalConfig, DurabilityMode (Sync/Batch/Adaptive/NoSync), async_log, flusher, record, recovery, typed
- crates/obrain-viz/src/lib.rs — façade : Subgraph, formats json-graph/dot/mermaid/graphml, overlays, diff, VizError/VizResult
- crates/obrain-viz/src/ir.rs — IR Subgraph/VizNode/VizEdge/Stats + validation (validate_edges/validate_ids), sérialisation f32 courte (ryu)
- crates/obrain-viz/src/json_graph.rs + json_graph_v2.rs — schémas `obrain/json-graph@1` et @2 (JsonGraphDoc/JsonGraphDocV2)
- crates/obrain-viz/src/dot.rs + mermaid.rs + graphml.rs — writers DOT (DotOptions/to_dot), Mermaid (to_mermaid), GraphML (to_graphml)
- crates/obrain-viz/src/overlay.rs — Overlay ricci/energy/roles/fsrs/engrams/pheromone, color_ramp/pastel_for_id/ricci_color, stables vs volatils
- crates/obrain-viz/src/diff.rs — SubgraphDiff, TimelineReport, timeline_subgraphs, diff_subgraphs
- crates/bindings/common/src/ — abstractions partagées des bindings (entity, error, json) consommées par C/Node/Python/WASM
- crates/bindings/c/src/ + c/obrain.h — FFI C (cdylib+staticlib) : ObrainStatus, obrain_open*/execute*/execute_cypher/gremlin/graphql/sparql, gestion mémoire obrain_free_*
- crates/bindings/node/src/ + node/index.js + node/index.d.ts — bindings napi (napi9/async) : ObrainDB, JsNode/JsEdge, QueryResult, simd_support/version, index.js auto-généré NAPI-RS
- crates/bindings/python/src/ + python/obrain/__init__.py — bindings pyo3 : PyObrainDB, PyNode/PyEdge, PyQueryResult, to_pandas/to_polars, as_networkx/as_solvor, cognitive (PyCognitiveEngine/PyGDS)
- crates/bindings/python/src/bridges/ — PyAlgorithms, PyNetworkXAdapter, PySolvORAdapter
- crates/bindings/wasm/src/ — bindings wasm-bindgen : Database (in-memory), execute, exportSnapshot/importSnapshot, cognitive_browser (energy/synapse/search), types, utils
- crates/bindings/go/ — bindings Go via CGO (-lobrain_c) : Database, Node, Edge, Transaction, types, vector, admin, index, errors, obrain.go
- crates/bindings/dart/ — bindings Dart via FFI : ObrainDB, database/transaction/types/value/error, ffi/bindings + ffi/loader
- crates/bindings/csharp/ — bindings C# via P/Invoke : ObrainDB, ObrainException, QueryResult, Transaction, ValueConverter, Native/NativeMethods, Native/Handles

**Relations / dépendances.**
- obrain-cli → obrain-engine (features storage/parallel/regex/text-index/hybrid-search/cognitive/cognitive-engram/algos/cdc) + obrain-core + obrain-substrate (repair ouvre le fichier substrat sans store) + obrain-common + obrain-garde + obrain-rag
- obrain-cli → obrain-viz (viz subgraph/export) ; viz_extract.rs → obrain-viz::Subgraph/VizNode/VizEdge + obrain-substrate::SubstrateStore (une seule implémentation partagée CLI ↔ MCP viz_subgraph)
- obrain-cli → obrain-adapters (feature algos : Louvain/Leiden pour `topo hilbert`) ; → obrain-indexer (optional), obrain-iam (optional, feature iam)
- obrain-cli/src/http_min.rs ≈ obrain-httpmin/src/lib.rs — deux implémentations quasi identiques du même client HTTP minimal (dette de déduplication)
- obrain-reactive → obrain-core (default-features=false) + obrain-common ; obrain-substrate en dev-dependency ; tokio sync (WASM) vs tokio full (natif)
- obrain-adapters → obrain-core + obrain-common + obrain-substrate (cycle-clear : substrate ne dépend pas d'adapters)
- obrain-viz : lib pure, dépendances serde/serde_json/thiserror uniquement — aucune dépendance substrate (les extracteurs vivent côté CLI/bridge)
- bindings/common → obrain-core + obrain-common ; consommé par bindings/c, bindings/node, bindings/python, bindings/wasm
- bindings/c → obrain-engine + obrain-core + bindings/common ; bindings/go (CGO), bindings/dart (FFI), bindings/csharp (P/Invoke) s'empilent tous sur obrain.h
- bindings/node → napi + obrain-engine + obrain-adapters + bindings/common
- bindings/python → pyo3 + pyo3-async-runtimes + obrain-engine + obrain-adapters + bindings/common + obrain-cognitive (optional)
- bindings/wasm → wasm-bindgen + obrain-engine (gql, regex-lite) + obrain-reactive (optional) + obrain-cognitive (optional) + bindings/common (default-features=false)
- obrain-cli (mcp_server, harness_serve, viz_serve, viz_ws, http_min) : tout le transport réseau est std-only, zéro dépendance HTTP dans le binaire
- viz_serve.rs → viz_extract.rs → obrain_viz : même chaîne que le tool MCP viz_subgraph (parité des comptes par construction)

**Points notables.**
- Règle architecturale stricte tenue : aucune dépendance HTTP/TLS dans obrain-cli — le serveur viz, le harness serve, le client http_min et le WebSocket sont tous écrits en std::net pur (TcpListener/TcpStream), avec des implémentations maison de SHA-1 (RFC3174) et du handshake RFC6455
- obrain-httpmin existe explicitement « pour ne pas avoir deux clients » (dédupliquer selflink/selfd), mais obrain-cli/src/http_min.rs conserve encore sa propre copie ~120 lignes du même client — la déduplication visée n'est pas achevée côté CLI
- obrain-viz est une lib pure sans dépendance substrate : elle ne connaît que l'IR Subgraph, garantissant déterminisme octet-à-octet, validation (arêtes pendantes/ids dupliqués) et testabilité ; la sérialisation f32 courte (ryu) économise ~9% de charge mesurée
- obrain-reactive est à coût nul quand inutilisé (< 5µs/publish sans abonnés) et double-cible natif/WASM via web-time et tokio conditionnel (sync sur wasm32)
- obrain-adapters/storage/file (ancien format monofichier .obrain) et l'ancien MemoryBackend sont retirés du chemin actif mais les sources restent sur disque « pending final deletion » — code mort assumé
- Les bindings forment 3 couches : C FFI (obrain.h) comme socle unique, puis Go (CGO), Dart (FFI) et C# (P/Invoke) empilés dessus, tandis que Node/Python/WASM attaquent l'engine en direct (napi/pyo3/wasm-bindgen)
- Le binding Python est le plus riche : ponts écosystème NetworkX/solvOR + conversions pandas/polars + couche cognitive (PyCognitiveEngine, PyGDS) — 6 dialectes testés en conformité (spec/ per dialecte)
- obrain-cli a deux mini-daemons HTTP std-only (viz_serve en lecture seule, harness_serve en lecture seule T1) et un pont MCP stdio — trois surfaces de service pour un même store, toutes respectant l'invariant mono-écrivain WAL
- viz_serve.rs instrumente ses phases (OBRAIN_SURFACE_PHASES) en 11 postes exclusifs (lecture socket, routage, params, cache, moteur, écriture…) inertes par défaut, preuve d'un soin de mesure sur la latence de la surface
- Le pont MCP met en cache sessions_list uniquement dans le process écrivain (OBRAIN_WRITE=1), car c'est lui qui voit toutes les mutations import_events — pas de staleness possible ; les process lecture seule ne cachent pas

**Trous / incohérences.**
- obrain-cli/src/http_min.rs duplique obrain-httpmin/src/lib.rs : le CLI ne consomme pas encore le crate partagé (dette de déduplication, risque de divergence documenté par le crate lui-même)
- crates/obrain-adapters/src/storage/file/ (format/header/manager/toc) est du code mort laissé sur disque en attente de suppression après le cutover substrate — à confirmer qu'aucun chemin ne le référence encore
- crates/obrain-cli/src/viz_weave.rs : module présent dans main.rs mais non inspecté — son rôle exact (tissage viz ?) reste à clarifier par rapport à viz_serve/viz_extract
- Go/Dart/C# n'ont pas de bindings natifs directs vers l'engine : ils passent tous par la couche C FFI (obrain.h), ce qui peut limiter fonctionnalités/performances vs Node/Python/WASM — à vérifier si les features AI/cognitive y sont exposées
- obrain-httpmin n'a ni répertoire tests/ ni tests unitaires visibles dans src/lib.rs — la couverture du parsing chunked/Content-Length est à confirmer
- bindings/wasm : le profil par défaut `browser` n'active que gql + regex-lite (pas cypher ni ai) — la surface WASM complète (full = languages+ai) n'est pas le défaut
- obrain-viz maintient deux schémas json-graph (v1 `obrain/json-graph@1` et v2 `@2` avec EdgeColumns) — lequel fait autorité pour l'interop et la timeline n'est pas tranché à la lecture des en-têtes
- crates/obrain-cli/src/remote.rs : rôle non clarifié (requêtes distantes ? client vers un service ?) — relation avec http_min et harness_serve à tracer
- La couverture des outils MCP du mcp_server.rs (search_graph, runtime_search, sessions_list, context_build, import_events, get_graph_schema) est-elle exhaustive vs ce que le harness appelle réellement ? — le commentaire l'affirme mais la liste exacte des outils du SDK n'est pas confrontée ici
- bindings/common/src/entity.rs : la sémantique exacte d'extraction des entités depuis les résultats de requête n'est pas tracée dans cette passe (consommé par les 4 bindings natifs)




## Vague 2 — V2 Bindings

### 2.1 Bindings Python (crates/bindings/python + packages/obrain-cli-python + examples/python + sections Python du README)

_Fichier : `raw/v2-1-bindings-python.json` — 18 composants · 13 relations · 8 constats · 8 trous_


**Synthèse.** Le binding Python d'obrain est une extension native PyO3 (crate `obrain-python`, lib `obrain`, crate-type cdylib) embarquée dans crates/bindings/python/, packagée via maturin sous le nom PyPI `obrain` (version 0.0.1, requires-python >=3.12) et publiée sous Apache-2.0. L'API principale tourne autour de `ObrainDB` (in-memory ou persistant via `Config::persistent`/`open`/`open_read_only`), qui expose l'exécution GQL/Cypher/SPARQL/Gremlin/GraphQL/SQL-PGQ, du CRUD nœud/arête, des transactions ACID (context manager avec auto-commit/rollback), des index de propriétés, la recherche vectorielle HNSW/MMR, la recherche texte BM25, le search hybride RRF, le CDC, l'embedding ONNX et l'export pandas/polars. Le module Rust se décompose en database.rs (~2900 lignes, `PyObrainDB`/`PyTransaction`/`AsyncQueryResult`), query.rs (`PyQueryResult`/`PyQueryBuilder`), types.rs (`PyValue` + conversion bidirectionnelle de types), graph.rs (`PyNode`/`PyEdge`), error.rs, bridges/ (algorithms/networkx/solvor), cognitive.rs (CognitiveEngine + GDS + CognitiveSearch) et quantization.rs (SQ/PQ/BQ). Les fonctionnalités sont entièrement feature-gated dans Cargo.toml (`embedded` par défaut, `full` pour tout) et propagées via maturin avec `features = ["pyo3/extension-module", "full"]`. Le package `obrain-cli-python` (packages/obrain-cli-python/) est un simple lanceur hatchling (console script `obrain`) qui localise puis exécute un binaire obrain pré-compilé (bundlé / venv bin / PATH). Les 4 exemples Python (examples/python/*.py) sont des notebooks marimo (fraude, vector search, NetworkX, visualisation), et la suite de tests compte 88 fichiers test_*.py couvrant les specs GQL/Cypher/GraphQL/Gremlin.


**Composants clés.**
- crates/bindings/python/Cargo.toml — manifeste du crate `obrain-python` (lib `obrain`, cdylib, features miroir de l'engine: gql/cypher/sparql/gremlin/graphql/sql-pgq/vector-index/text-index/hybrid-search/cdc/embed/algos/cognitive/full)
- crates/bindings/python/pyproject.toml — build maturin, nom PyPI `obrain` 0.0.1, requires-python >=3.12, features full, module-name obrain, dep solvor>=0.6.1
- crates/bindings/python/src/lib.rs — `#[pymodule] obrain`: enregistre PyObrainDB/Node/Edge/QueryResult/AsyncQueryResult/Value + bridges algos + cognitive + quantization + fonctions simd_support/vector + __version__
- crates/bindings/python/src/database.rs — PyObrainDB (execute*, CRUD, index, vector/text/hybrid search, embedding, admin/stats/schema/validate/WAL, import_df, nodes_df/edges_df, CDC) + PyTransaction + PyDatabaseStats + AsyncQueryResult (~2900 lignes)
- crates/bindings/python/src/query.rs — PyQueryResult (colonnes/rows/nodes/edges, to_pandas/to_polars/to_list/scalar, metrics) + PyQueryBuilder (param)
- crates/bindings/python/src/types.rs — PyValue + conversion bidirectionnelle Python<->Value (Null/Bool/Int64/Float64/String/List/Map/Vector/Bytes/Timestamp/Date/Time/Duration/ZonedDatetime/Path) + fn vector()
- crates/bindings/python/src/graph.rs — PyNode (id/labels/get/has_label/properties) et PyEdge (id/edge_type/source_id/target_id/get/properties)
- crates/bindings/python/src/error.rs — PyObrainError -> ValueError/RuntimeError via obrain_bindings_common::error::classify_error
- crates/bindings/python/src/bridges/mod.rs + algorithms.rs (824 l) + networkx.rs (547 l) + solvor.rs (501 l) — db.algorithms / db.as_networkx() / db.as_solvor()
- crates/bindings/python/src/cognitive.rs (683 l) — PyCognitiveEngine (energy/synapse/scar/fabric), PyGDS (pagerank/louvain/leiden/betweenness/similarity/project), PyCognitiveSearch
- crates/bindings/python/src/quantization.rs (448 l) — PyQuantizationType + PyScalarQuantizer + PyProductQuantizer + PyBinaryQuantizer
- crates/bindings/python/python/obrain/__init__.py + py.typed — ré-export ObrainDB/Node/Edge/QueryResult/Value/__version__/simd_support/vector depuis obrain.obrain
- crates/bindings/python/tests/ — conftest.py (fixtures db/node_ids/social_graph/pattern_graph, markers gql/cypher/gremlin/graphql/sparql), fixtures/, bases/, lpg/{cypher,gql,graphql,gremlin}/spec/ (88 test_*.py)
- crates/bindings/python/README.md — doc install/quickstart/API (Database, Query Languages, CRUD, Transactions, QueryResult, Vector Search)
- crates/bindings/common/ — entity.rs (extract_entities), error.rs (ErrorCategory/classify_error), json.rs — partagés par les bindings (dont Python)
- packages/obrain-cli-python/ — pyproject.toml (hatchling, console script obrain), README.md, obrain_cli/__init__.py (launcher: _find_binary bundlé/venv bin/PATH), obrain_cli/__main__.py
- examples/python/fraud_detection.py, vector_search.py, networkx_integration.py, graph_visualization.py — notebooks marimo (`marimo run`)
- README.md (racine) — sections Python (l.86-106, 505-509) + écosystème anywidget-graph/anywidget-vector

**Relations / dépendances.**
- crates/bindings/python/src/*.rs -> obrain-engine, obrain-core, obrain-common, obrain-adapters, obrain-cognitive (optionnel), obrain-bindings-common (Cargo.toml deps)
- PyObrainDB::execute_language_impl -> ObrainDB::execute_language + obrain_bindings_common::entity::extract_entities -> PyQueryResult
- PyObrainDB::create_node/create_edge/set_*_property/get_node/get_edge -> ObrainDB (engine) et store() (nodes_by_label, get_nodes_properties_batch, get_node_property_batch)
- PyObrainDB::vector_search/mmm_search/batch_vector_search -> db.vector_search/mmr_search/batch_vector_search (engine) ; hybrid_search -> obrain_core::index::text::FusionMethod
- PyObrainDB::register_embedding_model -> obrain_engine::embedding::OnnxEmbeddingModel::from_files
- PyTransaction::new -> obrain_engine::session::Session + transaction::IsolationLevel (ReadCommitted/Serializable/snapshot)
- PyValue::from_py/to_py -> obrain_common::types::Value (conversion bi-directionnelle des types)
- bridges/algorithms.rs -> obrain_adapters::plugins::algorithms (bfs/bfs_layers/dfs/dijkstra/...) ; networkx.rs/solvor.rs -> import réseau NetworkX/solvOR via pyo3
- cognitive.rs -> obrain_cognitive::{EnergyStore, SynapseStore, scar::ScarStore, fabric::FabricStore} + obrain_adapters::plugins::algorithms::louvain (GDS)
- quantization.rs -> obrain_core::index::vector::{BinaryQuantizer, ProductQuantizer, ScalarQuantizer, QuantizationType}
- python/obrain/__init__.py -> import obrain.obrain (module natif compilé) et ré-export partiel
- packages/obrain-cli-python/obrain_cli -> subprocess.run(binaire obrain) ; pyproject [project.scripts] obrain = obrain_cli:main
- tests/conftest.py -> import obrain (extension compilée) + tests.fixtures.datasets/generators ; tests lpg spec couvrent les langages feature-gated

**Points notables.**
- GDS `leiden()` est en réalité un alias de `louvain()`: cognitive.rs l.408-411 appelle `algorithms::louvain` alors que la doc annonce 'Louvain as the base with refinement pass' — pas d'implémentation de Leiden dédiée.
- Surface d'API volontairement retirée mais conservée sous forme de stubs qui lèvent des erreurs: `save()`, `to_memory()`, `open_in_memory()` (commentaires 'T17 final cutover 2026-04-23', passage au substrate par répertoire).
- Le module Python enregistre ~20 classes/fonctions au niveau natif (Algorithms, GDS, CognitiveEngine, quantizers, Transaction, DatabaseStats...) mais __init__.py n'en ré-exporte que 7 — le reste n'est accessible que via `obrain.obrain.*` ou getattr.
- README.md du crate documente `from obrain import IsolationLevel` + `IsolationLevel.SERIALIZABLE` et `result.execution_time` (secondes) alors que l'API réelle utilise une string (`begin_transaction("serializable")`) et `execution_time_ms` (millisecondes).
- La conversion de types est ambigüe pour les listes de floats: `[1, 2, 3]` (int) devient List, `[0.1, 0.2]` devient Vector — distinction intentionnelle documentée dans types.rs l.147-149.
- AsyncQueryResult stocke `column_types` marqué `#[allow(dead_code)]` (réservé pour un futur accès typé) et ne fait pas d'extraction de nœuds/arêtes en contexte async.
- Incohérence de versionnage: lib `obrain` 0.0.1 (pyproject) vs `obrain_cli.__version__` = "0.4.4" vs badge README 'python 3.12+' — et requires-python diverge (>=3.12 pour la lib, >=3.9 pour le CLI).
- Le search hybride et le vector search (HNSW, MMR, quantisation SQ/PQ/BQ) sont exposés très en profondeur côté Python, dépassant la simple couverture GQL.

**Trous / incohérences.**
- Pas de stubs .pyi générés (seul py.typed présent): les IDE s'appuient sur les docstrings Rust via maturin, mais aucune `__init__.pyi`/`*.pyi` n'est commitée.
- `IsolationLevel` référencé dans README.md (l.83-84) n'existe pas comme classe/enum exportée — l'API prend une chaîne; documentation en décalage avec l'implémentation.
- README.md l.96 documente `result.execution_time` (secondes) alors que le getter réel est `execution_time_ms` (Option[f64] en millisecondes).
- PyGDS::leiden() n'implémente pas Leiden (appelle louvain) — soit un bug de dénomination, soit un algorithme manquant.
- Stubs morts conservés (`save`/`to_memory`/`open_in_memory`) qui lèvent systématiquement des erreurs — code mort à supprimer ou à documenter comme breaking change.
- __init__.py ne ré-exporte pas Algorithms/GDS/CognitiveEngine/quantizers/Transaction/DatabaseStats enregistrés côté natif — surface publique incomplète et incohérente.
- Dépendance `solvor>=0.6.1` déclarée obligatoire dans pyproject.toml alors que `as_solvor()` est feature-gated (algos/full) — dépendance dure pour une fonctionnalité optionnelle.
- Aucun script de build/wheel Python dans scripts/ (pas de scripts/*python*, *maturin*, *wheel*); le tableau des exemples du README racine ne liste que des exemples Rust, omettant les 4 notebooks marimo de examples/python/.



### 2.2 Bindings Node/TypeScript + CLI npm + interface JS du dashboard

_Fichier : `raw/v2-2-bindings-node-ts.json` — 22 composants · 12 relations · 9 constats · 8 trous_


**Synthèse.** La surface Node/TypeScript d'obrain se divise en DEUX paquets npm distincts et un client navigateur. (1) @obrain-db/js (crates/bindings/node/) est le binding natif napi-rs v3 (crate Rust `obrain-node`, cdylib) qui enveloppe obrain_engine::database::ObrainDB derrière un Arc<RwLock> et expose une API asynchrone (execute/executeCypher/executeSql/executeGremlin/executeGraphql/executeSparql), le CRUD nœud/arête, les transactions, la recherche vectorielle HNSW, le text-search BM25, le hybride, le CDC et l'embedding ONNX — le tout via index.js auto-généré (loader multi-plateformes) et index.d.ts (types). (2) @obrain-db/cli (packages/obrain-cli-npm/) est un lanceur mince (bin/obrain.js) qui résout le binaire obrain-cli Rust plateforme-spécifique parmi des optionalDependencies et le spawn avec execFileSync, sans logique métier. (3) Le dashboard Vue/React (packages/obrain-dashboard/) N'UTILISE PAS le binding napi : son interface d'appel à obrain est un client HTTP/WebSocket REST (src/lib/api.ts) qui interroge le serveur « viz » (/api/stores, /api/viz/subgraph avec streaming WS + repli HTTP, /api/query, /api/functions, /api/viz/weave, /api/worlds, /api/engine/ask…). Le tronc documente Node.js/TypeScript uniquement dans le README racine (installation `npm install @obrain-db/js`, exemple `ObrainDB.create()`/`execute()`), sans section dédiée au CLI. L'ensemble est en Apache-2.0 côté npm mais porte des incohérences de licence, d'URL de dépôt et de version entre le workspace Rust (0.0.1) et les paquets publiés (0.5.24).


**Composants clés.**
- crates/bindings/node/package.json — manifeste @obrain-db/js v0.5.24, main=index.js, types=index.d.ts, 6 optionalDependencies plateformes, engines node>=20
- crates/bindings/node/index.js — loader napi-rs auto-généré (586 lignes) : détection musl/glibc, résolution de ~15 plateformes vers les binaires .node, repli WASI
- crates/bindings/node/index.d.ts — déclarations TypeScript auto-générées : ObrainDB, JsNode, JsEdge, QueryResult, Transaction, simdSupport(), version()
- crates/bindings/node/Cargo.toml — crate obrain-node (cdylib), deps obrain-engine/core/common/adapters + obrain-bindings-common, napi 3 (napi9/async/serde-json/error_anyhow), features gql/cypher/sparql/gremlin/graphql/sql-pgq/vector-index/text-index/hybrid-search/cdc/embed/algos/rdf, profils embedded/full
- crates/bindings/node/build.rs — napi_build::setup() (génère les symboles C napi)
- crates/bindings/node/src/lib.rs — module racine : #[macro_use] napi_derive, simd_support() -> obrain_core::index::vector::simd_support(), version() -> CARGO_PKG_VERSION
- crates/bindings/node/src/database.rs — JsObrainDB (js_name ObrainDB) : create/open/open_read_only, execute_language_impl, CRUD nœud/arête, windowQuery2d(+Sorted), createEdgesBatch(Fast)/bulkImport, nodeCountByLabel/edgeCountByType, createPropertyIndex, checkpoint, vector/text/hybrid search, CDC history, embed ONNX (1274 lignes)
- crates/bindings/node/src/graph.rs — JsNode / JsEdge exposés à JS (getters id/labels/edgeType, get(), properties(), hasLabel)
- crates/bindings/node/src/query.rs — QueryResult : getters columns/length/executionTimeMs/rowsScanned, get()/toArray()/scalar()/nodes()/edges()/rows()
- crates/bindings/node/src/transaction.rs — Transaction : execute/commit/rollback/isActive + variantes par langage, session obrain_engine::session::Session, support `using` (explicit resource management Node 22+)
- crates/bindings/node/src/types.rs — conversion JS<->Value (table de correspondance null/bool/number/string/Array/Object/Buffer/Date/BigInt/Float32Array), js_to_value/value_to_js
- crates/bindings/node/src/error.rs — NodeObrainError (thiserror) -> napi::Error (InvalidArg vs GenericFailure)
- crates/bindings/node/__test__/index.spec.mjs — suite vitest (exports, cycle de vie, CRUD, requêtes) importée depuis ../index.js
- crates/bindings/node/npm/linux-x64-gnu/package.json (et 5 autres) — paquets plateformes @obrain-db/js-* publiant le binaire .node
- crates/bindings/wasm/package.json — @obrain-db/wasm (binding WASM séparé, non utilisé par le dashboard)
- packages/obrain-cli-npm/package.json — manifeste @obrain-db/cli v0.0.1, bin obrain, optionalDependencies @obrain-db/cli-{linux-x64,linux-arm64,darwin-x64,darwin-arm64,win32-x64}, engines node>=18
- packages/obrain-cli-npm/bin/obrain.js — launcher : table PLATFORMS platform/arch -> pkg+bin, findBinary() (optional dep -> adjacent -> PATH), execFileSync avec stdio inherit
- packages/obrain-cli-npm/platform-template/package.json.tmpl — gabarit des paquets binaires @obrain-db/cli-{{PLATFORM}} (os/cpu/files {{BINARY}})
- packages/obrain-cli-npm/README.md — doc d'installation/usage du CLI npm (info/stats/validate/query/shell/init)
- packages/obrain-dashboard/src/lib/api.ts — client REST/WS du dashboard vers le serveur obrain (viz) : getCached/get, streamSubgraph (WebSocket), fetchSubgraph, searchFunctions, resoudreAncres, runQuery (/api/query), fetchWeave, fetchModuleArcs, fetchSpacePage, fetchLabelPairs, fetchEntretien, fetchActivation, askEngine, addWorld/fetchWorlds, fetchRamifications
- packages/obrain-dashboard/src/app/store.ts — état jotai (storesAtom, activeStoreAtom, centersAtom, queryTextAtom/queryLangAtom/queryResultAtom, subjectAtom, ancrerAtom, faceAtom…) ; QueryResult local {columns, rows, error, execution_time_ms}
- README.md (racine) — sections Node.js (ligne ~108) et Node.js/TypeScript (ligne ~511) : `npm install @obrain-db/js` et exemple ObrainDB.create()/execute()/close()

**Relations / dépendances.**
- crates/bindings/node/src/database.rs -> obrain_engine::database::ObrainDB (wrap Arc<RwLock<ObrainDB>>) ; exécution via db.execute_language(query, language, params)
- crates/bindings/node/src/lib.rs::simd_support() -> obrain_core::index::vector::simd_support()
- crates/bindings/node/Cargo.toml -> obrain-engine, obrain-core, obrain-common, obrain-adapters, obrain-bindings-common (crates/bindings/common)
- crates/bindings/node/src/database.rs -> crate::graph::{JsEdge,JsNode}, crate::query::QueryResult, crate::transaction::Transaction, crate::types, crate::error::NodeObrainError
- crates/bindings/node/src/database.rs::convert_json_params/json_to_value -> obrain_bindings_common::json::{json_params_to_map, json_to_value}
- crates/bindings/node/src/database.rs::extract_entities -> obrain_bindings_common::entity::extract_entities
- crates/bindings/node/index.js -> require() des paquets @obrain-db/js-{platform} (optionalDependencies) pour charger le binaire .node
- packages/obrain-cli-npm/bin/obrain.js -> spawn du binaire obrain/obrain.exe des paquets @obrain-db/cli-{platform} (optionalDependencies), repli cargo install obrain-cli
- packages/obrain-dashboard/src/lib/api.ts -> fetch()/WebSocket() vers les endpoints HTTP du serveur obrain (aucun import de @obrain-db/js ni de @obrain-db/wasm)
- packages/obrain-dashboard/src/app/App.tsx, Inspector.tsx, StoreOverview.tsx, StageContinu.tsx, Graph3D.tsx, CosmosWeave.tsx, EngineAsk.tsx, WorldAdd.tsx, useActivation.ts, fractale.ts, fratrie.ts, SubjectLine.tsx, CenterSearch.tsx, CommandBar.tsx, FacetBar.tsx, ChatPleinEcran.tsx -> importent depuis src/lib/api.ts
- packages/obrain-dashboard/src/app/store.ts -> import type { JsonGraph } from ../lib/api ; runQuery (lib/api) alimente queryResultAtom
- README.md racine -> cite @obrain-db/js (npm), @obrain-db/wasm, @obrain-db/web (badges) ; la table des bindings mentionne Node.js/TypeScript (napi-rs)

**Points notables.**
- Le dashboard n'utilise PAS le binding natif @obrain-db/js ni WASM : son interface JS à obrain est un client HTTP/WebSocket REST pur (src/lib/api.ts) contre le serveur « viz » — la frontière réelle côté navigateur est donc le réseau, pas napi-rs.
- Deux surfaces npm coexistent et ne se parlent pas : @obrain-db/js (binding embarqué napi-rs, v0.5.24) et @obrain-db/cli (lanceur de binaire Rust, v0.0.1) — le CLI ne passe pas par le binding JS, il spawn l'exécutable obrain-cli compilé.
- Désaccord de version : le workspace Cargo.toml est à 0.0.1, mais les paquets npm (node index.js/d.ts/package.json) sont figés à 0.5.24 ; or version() retourne env!(CARGO_PKG_VERSION) = 0.0.1, donc un appel JS version() renverra 0.0.1 alors que npm a installé 0.5.24.
- index.d.ts déclare inconditionnellement les méthodes sous feature-gates Rust (executeSql, executeGremlin, executeGraphql, executeSparql, mmrSearch, CDC, embed…) alors que database.rs les compile sous #[cfg(feature=...)] ; seuls les builds --features full (script `napi build --features full`) les exposent tous.
- Le loader index.js (auto-généré napi-rs) gère ~15 plateformes (android, freebsd, linux-arm/loong64/riscv64/ppc64/s390x, openharmony, win32-ia32/arm64, darwin-universal…), mais package.json ne liste que 6 optionalDependencies — les autres cibles ne sont pas publiées.
- Incohérence de licence : packages/obrain-cli-npm/platform-template/package.json.tmpl porte AGPL-3.0-or-later alors que le package CLI principal, le README CLI et le binding node sont Apache-2.0.
- Incohérence d'URL de dépôt : le template CLI pointe github.com/ObrainDB/obrain tandis que partout ailleurs (node, README, cli package.json) c'est github.com/this-rs/obrain.
- Les commentaires .d.ts mélangent anglais et français (ex. windowQuery2dSorted « Fenêtre 2D ACCÉLÉRÉE (zruns) », createEdgesBatchFast « BULK LOAD (sans WAL/CDC…) ») — traces des ajouts récents de la lignée substrate/zorder non harmonisés.
- Le QueryResult JS expose rows() comme « all rows as array of arrays » mais aussi toArray() comme « objects » ; la distinction sémantique entre les deux (clés de colonnes vs positions) est documentée de façon ambiguë.

**Trous / incohérences.**
- Le champ `files` de packages/obrain-cli-npm/package.json (et du binding node) référence LICENSE, mais AUCUN fichier LICENSE n'existe dans packages/obrain-cli-npm/ ni crates/bindings/node/ (vérifié par ls).
- index.js référence un repli WASI (obrain.wasi.cjs et @obrain-db/js-wasm32-wasi) mais aucun obrain.wasi.cjs n'est présent et wasm32-wasi n'est pas une cible du package — branche morte du gabarit napi-rs.
- Aucune config de publication (workflow CI, .npmrc de publish, pipeline napi artifacts) n'a été trouvée à côté des paquets plateformes node/cli — la chaîne de release n'est pas cartographiée dans cette tranche.
- executeSql() mappe le langage 'sql' (execute_language_impl('sql', …)) alors que le README du binding annonce « SQL/PGQ (SQL:2023 GRAPH_TABLE) » — l'étiquette feature s'appelle sql-pgq mais la clé runtime est 'sql'.
- fetchLabelPairs() (dashboard) porte un bug documenté et non corrigé : `labels(a)[0]`/`labels(b)[0]` dans un GROUP BY recopie le label de la cible dans la source (le cross-label est quasi vide) — signalé en commentaire, à reprendre (cf. docs/ADR-COSMOS-TOILE-2026-09-10.md).
- App.tsx.bak-avant-fix (package obrain-dashboard) reste dans l'arbre source — fichier de sauvegarde mort, non compilé, à purger.
- Les fichiers dist/ de packages/obrain-dashboard (dizaines de bundles index-*.js/.map commités) cohabitent avec src/ — artefacts de build versionnés, source potentielle de confusion sur l'interface réellement servie.
- Question ouverte : aucune route /api/viz/subgraph/stream n'est visiblement définie côté serveur dans cette tranche (seul le client WebSocket la consomme) — l'implémentation serveur du streaming WS n'a pas été vérifiée ici.



### 2.3 Bindings Go, C#/.NET, Dart (tranche interfaces multi-langages)

_Fichier : `raw/v2-3-bindings-go-csharp-dart.json` — 19 composants · 10 relations · 9 constats · 7 trous_


**Synthèse.** Les trois bindings Go, C#/.NET et Dart existent réellement et de façon substantielle sous crates/bindings/ : Go (13 fichiers, ~1870 lignes, CGO), C# (.NET 8 P/Invoke via LibraryImport, ~2273 lignes, solution Obrain.sln) et Dart (dart:ffi, ~2632 lignes, package obrain 0.5.24). Tous trois s'appuient sur une unique couche native commune, le crate obrain-c (crates/bindings/c, cdylib+staticlib, ~80 fonctions C exportées depuis src/database.rs), lui-même aidé du crate partagé obrain-bindings-common (crates/bindings/common). Le README principal annonce correctement ces trois bindings (sections Go/C#/Dart lignes 517-539, et ligne 431 « Go (CGO), C# (.NET 8 P/Invoke), Dart (dart:ffi) »), ainsi que les instructions d'installation go get / dotnet add package / pubspec. Les surfaces d'API se répondent (execute/executeWithParams/executeCypher/Sparql/Gremlin/GraphQL/SQL, CRUD node/edge, transactions ACID, recherche vectorielle k-NN+MMR, index de propriétés). La documentation mkdocs ne recense toutefois que l'API Go (docs/api/go/index.md) et C (docs/api/c/index.md) : C# et Dart ne sont documentés que par leur README.md interne au package. Plusieurs incohérences ont été relevées : dérive entre les deux copies d'en-tête C (go/obrain.h vs c/obrain.h), README de obrain-c décrivant une signature d'API obsolète (out-params + codes de statut) qui n'existe pas dans le code réel, et un numéro de version Dart (0.5.24) prétendant « match workspace release » alors que le workspace Cargo est en 0.0.1. Aucune bibliothèque native n'est embarquée dans le dépôt (runtimes/ C# absents, aucun .so/.dylib/.dll dans dart/), la compilation manuelle via `cargo build -p obrain-c` étant requise avant test.


**Composants clés.**
- crates/bindings/go/ — bindings Go complets (13 fichiers) : go.mod (module github.com/this-rs/obrain/crates/bindings/go), obrain.go, node.go, edge.go, transaction.go, vector.go, index.go, admin.go, types.go, errors.go, obrain_test.go, README.md
- crates/bindings/go/obrain.h — en-tête C copié pour CGO (158 lignes), diverge de c/obrain.h
- crates/bindings/go/obrain.go — Database (OpenInMemory/Open/Execute/ExecuteWithParams/ExecuteCypher/Gremlin/GraphQL/SPARQL/SQL, NodeCount/EdgeCount), cgo LDFLAGS -lobrain_c
- crates/bindings/go/node.go / edge.go / transaction.go / vector.go / index.go — CRUD nœuds, arêtes, transactions (BeginTransactionWithIsolation), recherche vectorielle, index de propriétés
- crates/bindings/csharp/Obrain.sln — solution VS (2 projets : Obrain, Obrain.Tests)
- crates/bindings/csharp/src/Obrain/Obrain.csproj — net8.0, AllowUnsafeBlocks, PackageId Obrain v0.0.1, bundling conditionnel runtimes/win-x64|linux-x64|linux-arm64|osx-x64|osx-arm64
- crates/bindings/csharp/src/Obrain/ObrainDB.cs — handle principal IDisposable/IAsyncDisposable (Memory/Open/Execute*/BeginTransaction/CreateNode/CreateEdge/VectorSearch/MmrSearch/Info/Save)
- crates/bindings/csharp/src/Obrain/Native/NativeMethods.cs — P/Invoke LibraryImport (AOT-safe) de toutes les fonctions obrain_* exportées
- crates/bindings/csharp/src/Obrain/{Transaction,QueryResult,ValueConverter,ObrainException}.cs + Native/Handles.cs — transaction, résultat, conversion de valeurs, exceptions, SafeHandle
- crates/bindings/csharp/tests/Obrain.Tests/ — xunit : DatabaseTests, CrudTests, QueryTests, TransactionTests
- crates/bindings/dart/pubspec.yaml — package obrain 0.5.24, sdk >=3.0.0, deps ffi ^2.1.0
- crates/bindings/dart/lib/obrain.dart — entrée publique (export database/error/transaction/types/value)
- crates/bindings/dart/lib/src/database.dart — ObrainDB (memory/open/execute*/beginTransaction/CRUD/vector, NativeFinalizer)
- crates/bindings/dart/lib/src/ffi/bindings.dart + ffi/loader.dart — bindings dart:ffi statiques + chargement plateforme (obrain_c.dll/libobrain_c.dylib/libobrain_c.so)
- crates/bindings/dart/lib/src/{transaction,value,types,error}.dart + example/example.dart + test/ — transaction, encodeur de paramètres/valeurs, types, hiérarchie d'exceptions scellée, exemple et tests
- crates/bindings/c/ — crate obrain-c (cdylib+staticlib) : src/database.rs (~80 fonctions C : open_memory/open/open_read_only/execute*/CRUD/vector/batch/transaction/info/wal), types.rs, error.rs, obrain.h, build.rs (header maintenu manuellement)
- crates/bindings/common/ — crate obrain-bindings-common : abstractions partagées error/json/entity utilisées par obrain-c
- docs/api/go/index.md — page API Go dans la doc mkdocs (Database/Query Languages/CRUD/Transactions/Vector Search)
- README.md (racine) — sections Go (517-527), C# / .NET (529-533), Dart (535-539) + badge Go et ligne 431 des bindings multi-langages

**Relations / dépendances.**
- go (CGO) → c/obrain.h (copie go/obrain.h) → libobrain_c via #cgo LDFLAGS: -lobrain_c
- csharp (P/Invoke LibraryImport) → obrain_c (LibName="obrain_c") → même ABI C que go/dart
- dart (dart:ffi lookupFunction) → obrain_c (chargé via loadNativeLibrary) → même ABI C
- obrain-c (crate) dépend de obrain-engine, obrain-core, obrain-common, obrain-bindings-common (Cargo.toml de c/)
- obrain-bindings-common dépend de obrain-engine (default-features=false) et obrain-common
- features partagées : c/Cargo.toml et common/Cargo.toml propagent gql/cypher/sparql/gremlin/graphql/sql-pgq/vector-index/text-index/hybrid-search/cdc/algos/rdf + profils embedded/full vers obrain-engine
- csharp/src/Obrain/Obrain.csproj référence README.md parent (PackageReadmeFile) et bundling conditionnel des runtimes natives
- csharp/tests/Obrain.Tests/Obrain.Tests.csproj référence src/Obrain/Obrain.csproj et copie conditionnelle obrain_c.dll/libobrain_c.so/libobrain_c.dylib
- docs/api/go/index.md ↔ go/README.md : contenu quasi identique (quick start, API) ; mkdocs.yml référence api/go/index.md et api/c/index.md
- go/obrain.h ↔ c/obrain.h : deux copies du même en-tête, mais en divergence (cf. findings)

**Points notables.**
- Dérive d'en-tête : go/obrain.h contient obrain_execute_sql (ligne 70) mais pas obrain_open_read_only ; c/obrain.h contient obrain_open_read_only (ligne 58) mais pas obrain_execute_sql. La source Rust de référence c/src/database.rs contient LES DEUX — chaque copie d'en-tête est donc périmée dans une direction différente.
- README obsolète de obrain-c : c/README.md documente une API à out-params + codes de statut (obrain_open_memory(&db) != OBRAIN_OK, obrain_execute(db, query, &result)) qui n'existe pas dans c/src/database.rs ; le vrai ABI retourne des pointeurs (obrain_open_memory() -> *mut ObrainDatabase). Go/C#/Dart suivent tous le vrai ABI, pas le README.
- Incohérence de version : dart/pubspec.yaml = 0.5.24 et dart/CHANGELOG.md prétend « Version bump to match workspace release », mais Cargo.toml [workspace.package] version = 0.0.1. Le C# (NuGet Version 0.0.1) correspond au workspace, pas le Dart.
- C# et Dart n'ont PAS de page API dans docs/api/ ni dans la nav mkdocs.yml (seuls api/go/index.md et api/c/index.md sont câblés, lignes 340-345) : leur documentation repose uniquement sur leur README.md interne.
- Aucune bibliothèque native embarquée : les runtimes/win-x64|linux-x64|... référencés par Obrain.csproj sont absents (condition Exists), et aucun .so/.dylib/.dll dans dart/ — le build `cargo build -p obrain-c` + copie manuelle est obligatoire avant test (ce qui est bien documenté dans chaque README).
- Le Go binding expose ExecuteSQL (obrain_execute_sql) mais n'expose PAS OpenReadOnly (obrain_open_read_only), alors que l'ABI C le fournit — surface Go légèrement plus étroite que le C.
- Les trois bindings couvrent la même surface fonctionnelle (GQL/Cypher/Gremlin/GraphQL/SPARQL/SQL, CRUD, transactions ACID à isolation configurable, recherche vectorielle k-NN + MMR) ; le C# est le plus riche côté API publique (méthodes Async/await + MmrSearch + Save + DropVectorIndex/RebuildVectorIndex).
- Le C# utilise la source generation LibraryImport (.NET 8, AOT-safe) plutôt que l'ancien DllImport, et le Dart utilise NativeFinalizer pour le nettoyage natif automatique — choix modernes cohérents.
- build.rs de obrain-c indique que l'en-tête est « maintained manually » (cbindgen ne supporte pas encore #[unsafe(no_mangle)] en Rust 2024) — cause probable de la dérive go/obrain.h vs c/obrain.h.

**Trous / incohérences.**
- Header drift non résolu entre go/obrain.h et c/obrain.h (obrain_execute_sql absent de c/obrain.h ; obrain_open_read_only absent de go/obrain.h) — pas de génération automatisée pour garder les copies synchronisées.
- c/README.md documente un ABI obsolète (out-params + statut) qui ne correspond pas au code réel — documentation trompeuse pour tout nouvel intégrateur C.
- Pas de docs/api/csharp/ ni docs/api/dart/ dans docs/api/ ni dans mkdocs.yml : C# et Dart absents de la documentation API publiée, alors que Python/Node/Go/WASM/C y figurent.
- Version Dart 0.5.24 vs workspace 0.0.1 — le CHANGELOG Dart (« match workspace release ») est factuellement faux ; le mécanisme de versioning Dart n'est pas raccordé au workspace Rust.
- Chemins de publication externes non vérifiables en lecture seule : github.com/this-rs/obrain/crates/bindings/go, pkg.go.dev, NuGet « Obrain », pub.dev « obrain » — présence réelle des packages publiés non confirmée dans ce dépôt.
- Bundling natif incomplet : les répertoires runtimes/ du csproj C# et toute .so/.dylib/.dll Dart sont absents, donc les packages NuGet/pub ne sont pas autonomes en l'état (build natif + copie manuelle requis).
- Le binding Go omet obrain_open_read_only (ouverture en lecture seule) pourtant disponible dans l'ABI C — trou de parité de fonctionnalité entre Go et C#/C.



### 2.4 Bindings WASM (crates/bindings/wasm) + chaîne de build wasm32 + « horloge » WASM

_Fichier : `raw/v2-4-bindings-wasm.json` — 27 composants · 12 relations · 11 constats · 10 trous_


**Synthèse.** Le livrable wasm d'obrain est une bibliothèque de liaison (pas un binaire CLI) : le crate `obrain-wasm` (crates/bindings/wasm, publié en `@obrain-db/wasm`) embarque le moteur en mémoire via wasm-bindgen, cible `wasm32-unknown-unknown`, profil `minimal-size` (opt-level=z, lto=fat, panic=abort). La compilation est pilotée par scripts/build-wasm.sh (build + garde horloge + wasm-bindgen --target web) et ses déclinaisons .ps1 et build-wasm-all.* (variantes ai/lite) ; scripts/verify-wasm.sh ajoute l'exécution de la suite de tests wasm. L'« horloge » WASM documente un correctif du 2026-09-17 : std::time::{Instant,SystemTime}::now() paniquent sur wasm32-unknown-unknown (« time not implemented », trap unreachable), et le module livré liait ce panic ; le remède est la substitution de web-time (horloge hôte JS performance.now()) dans les 7 crates du graphe wasm, gardée par un grep de la chaîne fautive dans le binaire. La levée du blocage d'horloge a révélé un second blocage : Config::in_memory() était en réalité un substrat fichier dans un répertoire temporaire (std::env::temp_dir() → « no filesystem on this platform »), corrigé par un vrai substrat RAM (ZoneFile Mmap|Ram, RamDir, SubstrateStore::open_memory) choisi uniquement sur wasm32. La suite de tests wasm (tests/web.rs, 22 tests) était morte (ne compilait plus depuis le retrait d'export_snapshot/import_snapshot au commit b0f5bd11) et a été réparée puis rendue verte (22/22). Le chantier est mesuré bout en bout sous Node (init → new Database → INSERT → MATCH → importLpg) et sans régression native (8 854 tests passés), mais le module n'a jamais été exécuté dans un vrai navigateur et le mode mémoire ne persiste ni ne checkpointe rien.


**Composants clés.**
- crates/bindings/wasm/Cargo.toml — crate obrain-wasm, crate-type cdylib+rlib, features browser/gql/cypher/rdf/sparql/gremlin/graphql/sql-pgq/vector-index/text-index/hybrid-search/languages/ai/cognitive/full
- crates/bindings/wasm/src/lib.rs — export #![forbid(unsafe_code)], struct Database { inner: ObrainDB }, ~20 méthodes wasm_bindgen (execute, executeRaw, executeWithLanguage(+Params), executeCypher/Gremlin/Graphql/Sparql/Sql, importLpg, importRdf, importRows, memoryUsage, textSearch, hybridSearch, vectorSearch, mmrSearch, createVectorIndex, version…)
- crates/bindings/wasm/src/types.rs — conversions Rust Value → JsValue (value_to_js, row_to_js_object)
- crates/bindings/wasm/src/utils.rs — set_panic_hook via console_error_panic_hook
- crates/bindings/wasm/src/cognitive_browser.rs — cognitive_demo() (energy/synapse/spreading activation) derrière feature cognitive
- crates/bindings/wasm/tests/web.rs — 22 tests wasm-bindgen-test (cible wasm32), réparés le 2026-09-17 (roundtrip entre deux bases, js_error_message)
- crates/bindings/wasm/pkg/ — artefact livré régénéré : obrain_wasm_bg.wasm (2,6 Mo), obrain_wasm.js (38 Ko), obrain_wasm.d.ts (242 l.)
- crates/bindings/wasm/obrain_wasm.js — stub chargeur commité qui throw « WASM binary not found » (remplacé par wasm-pack)
- crates/bindings/wasm/obrain_wasm.d.ts — déclarations TypeScript de référence (185 l., encore sur l'ancienne API snapshot)
- crates/bindings/wasm/package.json + package-lite.json — métadonnées npm @obrain-db/wasm / @obrain-db/wasm-lite (version 0.5.24)
- crates/bindings/wasm/README.md — docs package (usage, tailles, statut, support runtimes)
- crates/bindings/common/ — crate obrain-bindings-common (json.rs json_params_to_map/json_to_value, entity.rs, error.rs) partagé par les bindings
- scripts/build-wasm.sh — build wasm32 + GARDE HORLOGE (grep 'time not implemented') + wasm-bindgen + rapport tailles (seuils 300/400 Ko gzip)
- scripts/build-wasm-all.sh — variantes ai (pkg/) et lite (pkg-lite/), écrit les package.json @obrain-db/wasm et wasm-lite
- scripts/verify-wasm.sh — vérification bout en bout : construit + garde horloge + exécute la suite wasm (wasm-bindgen-test-runner), résolution de la bonne version de wasm-bindgen dans Cargo.lock
- scripts/build-wasm.ps1 / scripts/build-wasm-all.ps1 — équivalents PowerShell (Windows), SANS garde horloge
- docs/WASM-HORLOGE-2026-09-17.md — note de correctif horloge (501 lignes, 6 sections + 6bis…6septies), mesures avant/après et garde
- docs/ADR-2026-09-17-SUBSTRAT-EN-MEMOIRE.md — décision du substrat en mémoire (165 lignes)
- docs/api/wasm/index.md — référence API @obrain-db/wasm (encore sur exportSnapshot/importSnapshot et ~660 Ko gzip)
- crates/obrain-substrate/src/file.rs — ZoneFile à deux modes (Mmap|Ram), RamZone + RamDir, SubstrateFile::open_memory()
- crates/obrain-substrate/src/wal_io.rs — WalWriter/WalMedium (fichier ou tampon), open_memory()
- crates/obrain-substrate/src/writer.rs — Writer::new choisit le WAL selon is_memory()
- crates/obrain-substrate/src/store.rs — SubstrateStore::open_memory()
- crates/obrain-engine/src/database/mod.rs:944-953 — cfg(target_arch=wasm32) → open_memory(), sinon open_tempfile()
- crates/obrain-substrate/benches/memoire_vs_fichier.rs + BANCS.md — banc mémoire vs fichier (ouverture 128×, création nœuds 7,7×)
- Cargo.toml [profile.minimal-size] — opt-level=z, lto=fat, codegen-units=1, strip=true, panic=abort
- web-time (dépendance) — déclaré dans obrain-common, obrain-core, obrain-substrate, obrain-engine, obrain-adapters, obrain-cognitive (features serde), obrain-reactive

**Relations / dépendances.**
- obrain-wasm → obrain-engine (features gql + regex-lite ; optional core/reactive/cognitive) via Cargo.toml
- obrain-wasm → obrain-bindings-common (json_params_to_map, json_to_value) pour la conversion JsValue ↔ Value
- obrain-wasm → wasm-bindgen 0.2 (0.2.128 dans Cargo.lock) + serde-wasm-bindgen 0.6 + js-sys 0.3 + console_error_panic_hook 0.1
- obrain-wasm (feature cognitive) → obrain-cognitive (energy/synapse/search) + obrain-reactive
- Database::new() → ObrainDB::new_in_memory() → with_config (config.path=None) → SubstrateStore::open_memory() [wasm32] / open_tempfile() [natif]
- SubstrateStore::open_memory → SubstrateFile::open_memory → ZoneFile(Ram) + WalWriter(WalMedium) + Writer::new(is_memory)
- scripts/build-wasm.sh → cargo build --target wasm32-unknown-unknown --profile minimal-size -p obrain-wasm → wasm-bindgen --target web --out-dir pkg
- scripts/build-wasm-all.sh → scripts/build-wasm.sh --features ai (pkg/) puis --out-dir pkg-lite (pkg-lite/)
- scripts/verify-wasm.sh → build + garde horloge (grep 'time not implemented') + CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner cargo test -p obrain-wasm --test web
- web-time remplace std::time::{Instant,SystemTime} dans les 7 crates du graphe wasm (common/core/substrate/engine/adapters/cognitive/reactive)
- getrandom 0.4 features wasm_js (target wasm32) fournit l'entropie aléatoire au lieu de std
- obrain-engine → obrain-substrate : le moteur ne connaît le mode RAM que via l'API publique SubstrateStore::open_memory() (API ZoneFile inchangée)

**Points notables.**
- Le 'mode mémoire' d'Obrain était historiquement un substrat FICHIER dans un répertoire temporaire (open_tempfile → std::env::temp_dir()), ce qui le rendait impossible sur wasm32-unknown-unknown ; le chantier du 2026-09-17 l'a transformé en vrai substrat RAM (ZoneFile Mmap|Ram + RamDir registre partagé par nom).
- Le défaut d'horloge était invisible à la compilation : le module se construisait (exit 0) et se chargeait, mais trappait à l'exécution (RuntimeError: unreachable) dès le premier constructeur — mesuré sur l'artefact livré pkg/ du 2026-09-03.
- La garde horloge repose sur un détecteur binaire fiable : un panic std::time est LIÉ dès qu'un crate l'appelle (même chemin jamais emprunté), donc `strings -a …wasm | grep 'time not implemented'` suffit à prouver l'absence du chemin fautif.
- La suite de tests wasm (tests/web.rs, 22 tests) était MORTE : elle ne compilait plus depuis le commit b0f5bd11 (retrait d'export_snapshot/import_snapshot + JsError sans Display) — c'est précisément la garde qui aurait attrapé le défaut d'horloge.
- Il existe deux copies de la glue JS dans le dépôt : le stub commité obrain_wasm.js (qui throw « WASM binary not found ») et l'artefact réel pkg/obrain_wasm.js généré ; les .d.ts divergents (185 vs 242 lignes).
- Exigence de version exacte de wasm-bindgen : Cargo.lock fige 0.2.128, le CLI du PATH est 0.2.114 (refuse le module) ; verify-wasm.sh résout la bonne version dans ~/.cache/.wasm-pack/.
- Leçon de méthode consignée : `cargo build … | tail -12` sans pipefail rapporte le code de sortie de tail, pas de cargo — un échec de compilation rapportait 0 ; toutes les mesures ont été reprises avec set -o pipefail.
- Divergence de tailles documentées : README du crate dit 513/531 Ko gzip, docs/api/wasm/index.md dit ~660 Ko, et le pkg/ régénéré mesure 863 Ko gzip (au-dessus du seuil 400 Ko du script) — écart non isolé.
- Pas de binaire obrain wasm : le livrable wasm32 est exclusivement la bibliothèque de liaison obrain-wasm ; le CLI obrain reste natif (x86_64, rustc 1.91.1).
- Le profil minimal-size (opt-level=z, lto=fat, codegen-units=1, strip, panic=abort) est le profil de livraison wasm, distinct de release.
- build-wasm.ps1 et build-wasm-all.ps1 n'implémentent PAS la garde horloge présente dans les scripts .sh — asymétrie Windows/Linux.

**Trous / incohérences.**
- Le module n'a jamais été exécuté dans un vrai navigateur (Chrome/Firefox/Safari) : seules des exécutions Node ont été mesurées.
- Le mode mémoire ne persiste RIEN et n'est PAS checkpointable : SubstrateStore::checkpoint() échoue (flush + std::process::id() panique sur wasm32) ; aucun snapshot natif substrat n'existe (chantier T17b non réalisé).
- Les colonnes blob (blob_column.rs:647-648) écrivent encore sur path() et n'ont pas été exercées en mode RAM : elles échoueraient proprement.
- docs/api/wasm/index.md est périmée : documente exportSnapshot()/importSnapshot() (surface retirée par b0f5bd11) et une taille ~660 Ko gzip contredite par la mesure (863 Ko).
- crates/bindings/wasm/README.md est périmée : coche « Snapshot export/import for IndexedDB persistence » et tailles 513/531 Ko, sans mention du substrat RAM ni de la garde horloge.
- crates/bindings/wasm/obrain_wasm.d.ts (stub commité) référence encore exportSnapshot/importSnapshot, en divergence avec le .d.ts réel de pkg/.
- build-wasm-all.sh / .ps1 écrivent des package.json version 0.0.0, alors que package.json/package-lite.json portent la version 0.5.24 — incohérence de versionnement npm.
- Le support Deno/Cloudflare Workers est annoncé « supported/untested » sans exécution réelle mesurée.
- Les branches meta.rs/checkpoint.rs renvoient toujours 0 pour les champs temporels (created_at/last_checkpoint) sur wasm — décision antérieure non rouverte, une horloge Date.now() serait disponible si un lecteur en dépendait.
- La contribution propre du substrat en mémoire à la croissance de taille (2 991 939 o vs 863 Ko gzip) n'a pas été isolée par différence avec l'artefact antérieur.



### 2.5 MCP server + napi/FFI (crates/bindings, obrain-cli mcp bridge, docs M6)

_Fichier : `raw/v2-5-mcp-napi-ffi.json` — 25 composants · 13 relations · 10 constats · 9 trous_


**Synthèse.** La tranche MCP+FFI d'obrain se déploie en deux surfaces distinctes : d'une part le pont MCP embarqué dans le binaire CLI (crates/obrain-cli/src/mcp_server.rs, 8001 lignes / ~350 Ko, le plus gros module du crate, 33 outils JSON-RPC stdio au protocole 2024-11-05), d'autre part le package externe obrain-mcp (PyPI/uv, 16 outils query/CRUD/vecteur/algorithmes) documenté dans docs/ecosystem/obrain-mcp.md mais non présent dans le dépôt. Le pont embarqué est un serveur JSON-RPC 2.0 complet (initialize, tools/list, tools/call, ping, notifications/initialized) qui accepte aussi le dialecte bridge brut {"tool":..., "args":...} du harness codebase-memory, et passe chaque appel par une garde IAM (GardeIam::controler, OBRAIN_IAM_TOKEN, détection d'écriture via gql_ecrivante) avant tout outil. Les bindings multi-langages reposent sur deux cœurs : napi-rs v3 pour Node (crates/bindings/node, paquet @obrain-db/js 0.5.24, 6 cibles plateforme + index.js auto-généré) et une FFI C manuelle (crates/bindings/c, obrain.h écrit à la main car cbindgen ne gère pas #[unsafe(no_mangle)] Rust 2024) consommée par Go (CGO -lobrain_c), C# (LibraryImport net8.0) et Dart (dart:ffi). Le crate partagé obrain-bindings-common (entity/error/json) mutualise l'extraction d'entités et la classification d'erreurs entre Python/Node/C/WASM. Les trois docs M6 documentent l'indexeur tree-sitter (159 langages) qui alimente l'outil MCP index_repository et sa vérification de parité face à l'indexeur C de référence, avec 100 % des méthodes et 99,97 % des fonctions Rust retrouvés.


**Composants clés.**
- README.md (section '## MCP Server', lignes 350-364 ; table Architecture ligne 399 ; ligne 567) — entrée marketing MCP, annonce `npm install -g @obrain-db/mcp`
- crates/obrain-cli/src/mcp_server.rs (8001 lignes) — serveur MCP JSON-RPC stdio 'obrain mcp' / 'obrain bridge', 33 outils, garde IAM, cache sessions_list
- crates/obrain-cli/src/main.rs — dispatch Commands::Mcp / Commands::Bridge -> mcp_server::run, log stderr en mode MCP
- crates/obrain-cli/Cargo.toml — features (indexer/iam/ai/algos), deps obrain-engine/obrain-indexer/obrain-rag/obrain-garde/obrain-viz/obrain-adapters
- crates/obrain-cli/src/worlds.rs — réutilise mcp_server::index_repository (chemin d'indexation unique)
- crates/obrain-cli/src/viz_serve.rs — GardeIam, gql_ecrivante, jeton_du_pont partagés avec le pont MCP
- crates/obrain-cli/src/viz_extract.rs — extracteurs pour le futur tool viz_subgraph (v05)
- crates/obrain-cli/src/outils.rs — format search_code '<label>.text' partagé avec le bridge
- crates/bindings/node/Cargo.toml — napi-rs v3 (napi9/async/serde-json/error_anyhow), crate-type cdylib
- crates/bindings/node/src/lib.rs + database.rs (1274 l) + query.rs (203 l) + transaction.rs (253 l) + graph.rs (179 l) + types.rs (288 l) + error.rs (53 l)
- crates/bindings/node/index.js + index.d.ts — loader NAPI-RS auto-généré (détection musl, fallback WASI, multi-arch) et typages TS
- crates/bindings/node/package.json — @obrain-db/js 0.5.24, 6 cibles napi + 6 optionalDependencies, build `napi build --features full`
- crates/bindings/node/__test__/index.spec.mjs — suite vitest (lifecycle, CRUD, query, transactions)
- crates/bindings/node/npm/*/package.json — 6 paquets plateforme (darwin-x64/arm64, linux-x64-gnu, linux-arm64-gnu/musl, win32-x64-msvc)
- crates/bindings/c/obrain.h + src/lib.rs + src/database.rs (1939 l) + src/types.rs + src/error.rs — FFI C (ObrainStatus, ~60 fonctions obrain_*, obrain_last_error thread-local, Arc<RwLock<ObrainDB>>)
- crates/bindings/c/build.rs — commentaire : cbindgen non utilisé (pas de support #[unsafe(no_mangle)] Rust 2024)
- crates/bindings/common/src/lib.rs + entity.rs (198 l) + error.rs (74 l) + json.rs (190 l) — obrain-bindings-common (RawNode/RawEdge, classification d'erreurs)
- crates/bindings/go/obrain.go + obrain.h + go.mod — CGO, #cgo LDFLAGS -lobrain_c, consommation de la FFI C
- crates/bindings/csharp/src/Obrain/Native/NativeMethods.cs + Obrain.sln — P/Invoke LibraryImport net8.0 (AOT-safe) sur obrain_c
- crates/bindings/dart/lib/src/ffi/loader.dart + bindings.dart — dart:ffi charge libobrain_c.so/.dylib/.dll
- crates/bindings/python/ (PyO3) et crates/bindings/wasm/ (wasm-bindgen) — siblings du même écosystème bindings
- docs/ecosystem/obrain-mcp.md — produit externe obrain-mcp (github.com/this-rs/obrain-mcp, PyPI), 16 outils, transports stdio/SSE/HTTP
- docs/m6-delegation-plan-159-langages.md — plan indexeur tree-sitter 159 langages (banc-obrain-m6, HEAD b416c423)
- docs/m6-vs-c-multilang-verification.md — parité M6 vs indexeur C sur 11 langages (corpus /tmp/m6-multilang)
- docs/m6-vs-c-verification-2026-09-06.md — parité fine Rust (8 474 méthodes 100 %, 11 641/11 644 fonctions 99,97 %)

**Relations / dépendances.**
- mcp_server.rs -> obrain_engine::ObrainDB (Session::execute_language -> QueryResult serialisé {columns, rows, total})
- mcp_server.rs -> obrain_indexer::index_repository (feature 'indexer') via l'outil index_repository ; worlds.rs réutilise ce même chemin
- mcp_server.rs -> obrain_rag::traits::RetrievedNode (budget jetons de l'injection context_build/checkpoint_material)
- mcp_server.rs -> crate::viz_serve::{GardeIam, gql_ecrivante, jeton_du_pont} (garde IAM partagée pont/serveur HTTP)
- mcp_server.rs -> obrain_common::types::Value (json_to_obrain_value / to_json_value / prop / prop_str)
- crates/obrain-cli/src/main.rs : Commands::Mcp(args) et Commands::Bridge(args) -> mcp_server::run(args)
- crates/bindings/node/src/lib.rs -> modules database/query/transaction/graph/types/error ; napi_derive #[napi]
- crates/bindings/node -> obrain-engine, obrain-core, obrain-common, obrain-adapters, obrain-bindings-common
- crates/bindings/node/index.js (auto NAPI-RS) -> require() des paquets plateforme @obrain-db/js-* + .node locaux
- crates/bindings/c/src/database.rs -> obrain-engine/obrain-core/obrain-common + obrain-bindings-common ; exports #[unsafe(no_mangle)] extern "C" obrain_*
- crates/bindings/go -> CGO LDFLAGS -lobrain_c (FFI C) ; csharp -> LibraryImport "obrain_c" ; dart -> dart:ffi libobrain_c.*
- crates/bindings/common -> obrain-engine (default-features=false) + obrain-common (mutualise entity/error/json pour Python/Node/C/WASM)
- docs m6-* -> crates/obrain-indexer (lang_specs.rs 161 LangSpec, spec_engine.rs, grammars/lotN, tests/lang) et l'outil MCP index_repository

**Points notables.**
- Le pont MCP embarqué (obrain mcp) est le plus gros module de obrain-cli : 8001 lignes, 33 outils, un serveur JSON-RPC 2.0 complet + un dialecte bridge brut {"tool":..., "args":...} normalisé en tools/call
- Garde IAM systématique avant tout outil : GardeIam.controler(jeton, ecriture, espace) avec OBRAIN_IAM_TOKEN, et détection d'écriture combinant outil_ecrivant() + gql_ecrivante() pour query/query_graph
- Lecture par défaut (verrou partagé LOCK_SH) car le harness respawn le process à chaque requête ; l'écrivain (GraphWriter) porte OBRAIN_WRITE=1 et seul lui sert le mémo-cache sessions_list
- Échéance de requête unifiée OBRAIN_QUERY_TIMEOUT_MS (défaut 30 s au pont) pour ne plus être le seul chemin non borné — commentaire avouant 6 lignes dupliquées entre 4 points d'entrée (serveur/CLI/REPL/pont)
- Contrat de réponse miroir du moteur C : structuredContent (payload objet) + content[0].text (JSON) + isError, avec plafond d'octets par réponse et seuil d'alerte
- Deux produits MCP distincts coexistent : le pont Rust embarqué (33 outils, parité codebase-memory) vs obrain-mcp externe (PyPI/uv, 16 outils query/CRUD/vecteur/algorithmes, transports stdio/SSE/HTTP) — non clarifié dans le README
- napi-rs v3 (napi9) : paquet @obrain-db/js 0.5.24, 6 cibles + optionalDependencies, loader index.js auto-généré (détection musl, fallback WASI, ~15 architectures)
- FFI C écrite à la main (obrain.h) car cbindgen ne supporte pas #[unsafe(no_mangle)] en édition Rust 2024 ; ~60 fonctions obrain_*, codes ObrainStatus, obrain_last_error thread-local, handle Arc<RwLock<ObrainDB>> partageable entre threads
- Trois consommateurs natifs de la même FFI C : Go (CGO -lobrain_c), C# (.NET 8 LibraryImport AOT-safe), Dart (dart:ffi) ; Python (PyO3) et WASM (wasm-bindgen) sont des bindings séparés
- L'outil MCP index_repository repose sur l'indexeur tree-sitter M6 (159 langages) vérifié en parité vs indexeur C : 100 % méthodes, 99,97 % fonctions Rust, superset sur la majorité des 11 langages testés

**Trous / incohérences.**
- DÉRIVE DE VERSION DU PONT : le checkout principal (banc-obrain HEAD 456d3794) expose 33 outils, mais le pont déployé qui sert l'agent (mcp__parc__ / mcp__codebase-memory-mcp__) expose detect_gaps, spread, topo_snapshot, topo_diff (+ index_commits) — présents uniquement dans des worktrees plus récents (wt-indexeur-commits feat/indexeur-commits f02e5b1d : 40 outils ; wt-iam-audit/wt-iam-tronc/wt-projets-moteur : 39 ; banc-obrain-fix-ecrivain : 39)
- README incohérent : la commande `npm install -g @obrain-db/mcp` (ligne 355) n'a aucun paquet correspondant dans le dépôt (seul @obrain-db/js existe en packages/node) ; le pont réel est le binaire Rust `obrain mcp`, pas un paquet npm
- Le produit obrain-mcp externe (16 outils, github.com/this-rs/obrain-mcp, PyPI) est une surface distincte du pont embarqué — aucun des 3 docs de périmètre ni le README n'articule ce partage
- mcp_server.rs avoue une dette : les 6 lignes de l'échéance de requête sont dupliquées entre 4 points d'entrée et 'à consolider dans un module partagé une fois stabilisés'
- obrain.h maintenu manuellement (cbindgen inutilisable avec #[unsafe(no_mangle)] Rust 2024) — risque de désynchronisation en-tête/implémentation à chaque ajout de fonction FFI
- index_status note explicitement que miss-graph/parse_partial ne sont pas persistés par obrain-indexer ; la vérification M6 s'appuie sur une base SQLite C de référence (tmp-m6-ref-arbre.db) hors dépôt
- Les docs M6 référencent des branches/worktrees et commits (feat/m6-code-graph, banc-obrain-m6, HEAD b416c423, hash 8f531963, 25614779, 128d069b…) absents de la branche tronc courante consolidation/tronc-2026-09-15-plus-fusion
- Le loader index.js auto-généré référence ~15 cibles (android, freebsd, loong64, riscv64, s390x, ppc64, openharmony, WASI, darwin-universal…) alors que package.json ne déclare que 6 optionalDependencies — boilerplate napi-rs standard, mais la couverture publiée est en retard sur le loader
- Pas de crate MCP dédié : le serveur MCP vit dans obrain-cli (fichier de 350 Ko mêlant transport JSON-RPC, 33 outils, garde IAM, ingestion, injection et métriques) — contrairement à la ligne 'Bindings & Integrations' de l'architecture README qui le présente comme un composant autonome




## Vague 3 — V3 Docs

### 3.1 docs/ADR-*.md — chaîne des décisions d'architecture (8 fichiers, 7 thèmes)

_Fichier : `raw/v3-1-adr.json` — 53 composants · 13 relations · 15 constats · 13 trous_


**Synthèse.** Les 8 ADR couvrent 7 thèmes : chaîne IAM + contrôle de l'explosion des liens, garantie de confidentialité du calcul, échelle multi-niveaux du dashboard, substrat en mémoire (WASM), couche de décision, Cosmos-toile (base synaptique durable de l'agent) et stage continu (projection 3D unique). Deux fichiers IAM sont des doublons quasi-identiques ne différant que par la casse du nom (le majuscule ajoute la section « Le canal »). L'ADR IAM (2026-09-13) transforme une bibliothèque sans appelant (obrain-iam : 5 395 lignes, 95 tests, 0 point d'application) en garde fail-closed armée sur 3 points d'entrée, et fixe 7 décisions de planification mesurées (retournement de motif, pushdown de filtre, budget cartésien, échéances armées, garde unique, défaut bruyant, capacité constatée). L'ADR confidentialité (2026-09-15) pose que « personne ne peut cacher un calcul à celui qui exécute le moteur » et propose V-B (self aveugle) comme plancher, avec l'étape 0 (TLS) déjà mesurée à 16/16 via la crate obrain-tls mais non encore branchée sur run_serve(). L'ADR substrat mémoire (2026-09-17) insère un mode RAM dans la couche zone de obrain-substrate (ZoneBacking::Ram, RamDir), activé seulement sur wasm32, faisant passer la suite wasm de 1/21 à 22/22 et divisant l'ouverture par 128. L'ADR échelle (2026-09-16) consolide une autorité unique palierDe(d,R) dans echelle.ts (22 cas) et la loi densifier/approfondir dans densite.ts (18 cas), en signalant 2 défauts ouverts. L'ADR Cosmos (2026-09-10) transforme la vue en système solaire + toile synaptique fondée sur 4 preuves réelles (P1 entité partagée, P2 pont de vécu, P3 plan traversant, P4 flux de types), persistée durablement. L'ADR couche de décision (2026-09-22) est un plan d'exécution outillé (134 nœuds / 187 arêtes dans le graphe) pour exposer obrain.decide / obrain.settle avec calibration Beta postérieure, mais avec 0/6 critères d'acceptation atteints, 0/14 tests écrits et la Phase 0 (oracle) comme porte.


**Composants clés.**
- docs/ADR-STAGE-CONTINU.md — stage continu : une seule projection 3D à échelles continues (caméra = distance, jamais de swap de page)
- docs/ADR-COSMOS-TOILE-2026-09-10.md — Cosmos système solaire + toile synaptique durable (accepté, implémenté)
- docs/ADR-2026-09-13-chaine-iam-et-explosion-des-liens.md — chaîne IAM + explosion des liens (DOUBLON, casse minuscule)
- docs/ADR-2026-09-13-CHAINE-IAM-ET-EXPLOSION-DES-LIENS.md — chaîne IAM + explosion des liens (DOUBLON, casse majuscule, ajoute « Le canal »)
- docs/ADR-2026-09-15-GARANTIE-CONFIDENTIALITE-CALCUL.md — garantie de confidentialité du calcul (PROPOSÉ, variante V-A/V-B non tranchée)
- docs/ADR-2026-09-16-ECHELLE-MULTI-NIVEAUX.md — échelle multi-niveaux du dashboard (loi d'échelle unique)
- docs/ADR-2026-09-17-SUBSTRAT-EN-MEMOIRE.md — substrat en mémoire (APPLIQUÉ ET MESURÉ)
- docs/ADR-2026-09-22-COUCHE-DECISION.md — couche de décision : faire d'Obrain son propre modèle (PROPOSÉ ET PROJETÉ)
- crates/obrain-iam/src/garde.rs — Garde IAM (authentifier + check_graph_access, fail-closed ImplicitDeny)
- crates/obrain-iam/src/admin.rs — espaces, rôles, politiques bornées à (tenant, espace), actions graph:read/write, iam:audit/admin
- crates/obrain-iam/src/provider.rs — bcrypt (DEFAULT_COST), jetons 256 bits, empreinte SHA-256
- crates/obrain-iam/src/model.rs — AuditEvent (principal, action, resource, result, timestamp)
- crates/obrain-tls/Cargo.toml + src/{config,ecoute,erreur,lib}.rs — transport TLS 1.3 (FluxTls Read+Write), non branché sur run_serve()
- crates/obrain-tls/examples/serveur_tls.rs — relais de capture témoin (référencé à tort comme examples/serveur_tls.rs)
- crates/obrain-cli/src/viz_weave.rs — preuves P1-P4, agrégation, persistance et relecture de la base synaptique
- crates/obrain-cli/src/viz_serve.rs — serveur HTTP main, /api/viz/weave, /api/viz/subgraph, foyers contextuels
- crates/obrain-cli/src/commands/viz.rs — sous-commandes viz weave / viz serve (--synapse-store)
- crates/obrain-substrate/src/file.rs — ZoneFile bimode (ZoneBacking::Mmap | Ram), RamZone, registre RamDir
- crates/obrain-substrate/src/wal_io.rs — WalMedium::{File, Memoire}
- crates/obrain-substrate/src/store.rs — SubstrateStore::open_memory()
- crates/obrain-substrate/src/page.rs + props_zone.rs — seal_crc32 (intégrité seule, aucune confidentialité)
- crates/obrain-engine/src/database/mod.rs — routage open_memory() sur wasm32, câblage doublé with_config (DEC-C6)
- crates/obrain-engine/src/config.rs — Config::in_memory() (path: None)
- crates/obrain-engine/src/cognitive_procedures.rs — poids constants de obrain.cognitive.search, point d'extension CALL, PROCEDURES_DE_LECTURE
- crates/obrain-engine/src/cognitive_udfs.rs — obrain.risk = churn × pagerank × gap × betweenness
- crates/obrain-engine/src/database/search_causality.rs — search_causes/search_effects, BFS amorti γ=0,5
- crates/obrain-cognitive/src/tenant.rs — isolation cognitive par tenant (DashMap distinctes)
- crates/obrain-cognitive/src/intent.rs — classification d'intention sans embeddings/LLM
- crates/obrain-cognitive/src/engram/hopfield.rs — PredictiveModel, observe (Welford), bayesian_update, softmax_compete β par motif
- crates/obrain-cognitive/src/episodic.rs — registre Stimulus→Process→Outcome→Validation→Lesson
- crates/obrain-cognitive/src/synapse.rs — SegQueue<MetadataDelta>, drain consolidateur/SIGTERM (DEC-C5)
- crates/obrain-cognitive/src/kernel.rs — MutationBus → recompute_affected
- crates/obrain-cognitive/src/activation.rs — EdgeClass::Causal vs Associative, causal_boost
- crates/obrain-cognitive/src/store_trait.rs — write-through DashMap + persist_node_f64
- crates/obrain-rag/src/feedback.rs — renforcement des synapses co-mentionnées (écho auto-référentiel PI6)
- crates/obrain-garde/src/repetition.rs — coupe-circuit de répétition (référencé sans chemin complet par l'ADR échelle)
- crates/obrain-core/src/index/vector/config.rs — HNSW à dimension fixe (contrainte DEC-C4 §4)
- crates/obrain-common/src/types/value.rs — as_vector() n'accepte que Value::Vector (PI1)
- packages/obrain-dashboard/src/graph/CosmosWeave.tsx — système solaire, nappe 46×46, toile, layout
- packages/obrain-dashboard/src/components/StageContinu.tsx — stage 3D unique (référencé à tort sous graph/)
- packages/obrain-dashboard/src/graph/echelle.ts — palierDe(d,R), distancePourT, bandes (22 cas) + echelle.test.mjs
- packages/obrain-dashboard/src/graph/densite.ts — loi densifier/approfondir (18 cas)
- packages/obrain-dashboard/src/lib/api.ts — fetchWeave + avertissement fetchLabelPairs (forme 2 expressions fautive)
- scripts/verif-chaine-iam.sh — vérification 13 étapes port 8435 (verdict + code de sortie)
- scripts/acceptation-tls.sh — critère tcpdump exécutable (16 vérifications, 0 échec)
- scripts/weave.sh + scripts/vizserve.sh — orchestration tissage exclusif / serveur viz
- scripts/cosmos-capture.mjs + scripts/check-weave-contract.mjs — capture CDP réelle + garde-fou contrat sans navigateur
- scripts/couche-decision.gql + scripts/couche-decision-ingest.sh — projection idempotente du plan couche-décision
- docs/ADMINISTRATION-IAM-2026-09-13.md — défaut delete_policy/delete_role (iam_status jamais lu)
- docs/ECHELLE-MULTI-NIVEAUX-REPRISE-2026-09-16.md — document de reprise détaillé de l'échelle
- docs/WASM-HORLOGE-2026-09-17.md — comptes et commandes du substrat mémoire
- docs/APPORT-OBRAIN-USAGES.md — limite « causalité = structurelle dirigée amortie »
- docs/user-guide/vector-search/basics.md — enseigne le bug PI1 (coercition vector() omise)

**Relations / dépendances.**
- ADR-2026-09-13 (IAM) → crates/obrain-iam/{garde,admin,provider,model}.rs : garde unique appliquée sur viz serve, obrain query et le pont MCP
- ADR-2026-09-13 (IAM) → crates/obrain-cli/src/viz_serve.rs + scripts/verif-chaine-iam.sh : chaîne fermée connexion/renouvellement/déconnexion/audit vérifiée en 13 étapes
- ADR-2026-09-13 D1/D2 → planificateur crates/obrain-engine (retournement de motif edges_from(b, Incoming), pushdown de filtre des deux côtés de jointure)
- ADR-2026-09-13 D7 → substrat : has_backward_adjacency() constatée par échantillon déterministe (64 destinations)
- ADR-2026-09-15 (confidentialité) → crates/obrain-tls (étape 0) + scripts/acceptation-tls.sh ; dépend de obrain-iam (garde) et obrain-substrate (CRC32 seul, pas de chiffrement au repos)
- ADR-2026-09-15 étape 0 → crates/obrain-cli/src/viz_serve.rs (run_serve) : branchement encore ouvert, serveur réel en clair
- ADR-2026-09-16 (échelle) → packages/obrain-dashboard/src/graph/echelle.ts importé par CosmosWeave.tsx ; densite.ts ; crates/obrain-garde/src/repetition.rs (coupe-circuit, budget BUDGET_QUESTION_S=300)
- ADR-2026-09-17 (substrat mémoire) → crates/obrain-substrate/{file,wal_io,store}.rs routé par crates/obrain-engine/src/{config,database/mod}.rs, consommé par crates/bindings/wasm
- ADR-2026-09-22 (couche décision) → obrain-cognitive (engram/hopfield, episodic, synapse, kernel, store_trait) + obrain-engine (cognitive_procedures, cognitive_udfs, search_causality) + obrain-rag/feedback.rs (PI6)
- ADR-2026-09-22 projection → scripts/couche-decision.gql (MERGE idempotent) joué par scripts/couche-decision-ingest.sh, écrit via le pont MCP parc (query_graph accepte les écritures)
- ADR-2026-09-22 contraintes → 34 CodeRef ancrés ANCRE_DANS des symboles réels indexés (projet obrain, files_scanned 1885)
- ADR-2026-09-10 (Cosmos) → crates/obrain-cli/src/viz_weave.rs (preuves P1-P4) + viz_serve.rs (/api/viz/weave) + CosmosWeave.tsx + scripts/weave.sh
- ADR-2026-09-09 (stage continu) → remplace CosmosView (SVG+swaps) par StageContinu.tsx (Three.js) monté en permanence ; CosmosWeave rend StageContinu en repli

**Points notables.**
- DOUBLON : ADR-2026-09-13-chaine-iam-et-explosion-des-liens.md (minuscule) et ADR-2026-09-13-CHAINE-IAM-ET-EXPLOSION-DES-LIENS.md (majuscule) sont le même ADR ; seule différence : le majuscule ajoute la section « Le canal » et reformule quelques lignes (« mesuré sur runtime-dev »).
- L'IAM était une bibliothèque sans appelant : 5 395 lignes, 9 modules, 95 tests verts, zéro point d'application ; feature iam absente de default et ai ; le serveur servait sans identité et rien ne le disait.
- « Aucun TLS dans tout le workspace » (2026-09-15) : grep rustls/native-tls/openssl/tls sur tous les Cargo.toml ne rend rien ; serveur HTTP/1.1 écrit à la main sur std::net::TcpListener.
- Cinq défauts IAM corrigés, chacun mesuré : jeton déterministe (DefaultHasher+compteur), révocation décorative (magasin RO), expiration jamais vérifiée, horodatage fictif (now_iso = compteur), empreinte faible (SipHash-64 sans sel → SHA-256).
- has_backward_adjacency() renvoyait true en dur : sur chaîne entrante incomplète, le retournement (D1) rendrait moins de lignes en silence ; désormais constatée par échantillon déterministe.
- Le produit cartésien MATCH (a)-[r]->(b), (c) tuait le serveur sans message (~5·10¹¹ paires) ; aucun opérateur ne consultait memory_limit/spill_path.
- L'ADR couche-décision est auto-porteur : le plan vit dans le graphe (134 nœuds / 187 arêtes), ses comptes se revérifient par GQL (MATCH ... WHERE x.chantier='couche-decision'), et le document a été corrigé 5 fois par vérification, jamais par relecture.
- État réel du chantier couche-décision : 26 étapes a_faire, 0/6 critères d'acceptation, 0/14 tests écrits — aucune ligne implémentée ; la Phase 0 (oracle à vérité observable) est la porte.
- PI1 (piège le plus dangereux) : un littéral [0.9,...] se parse en Value::List, as_vector() ne l'accepte pas, WHERE null > 0.99 est faux → résultat plausible et vide ; la doc user-guide/vector-search/basics.md enseigne le bug.
- Bugs moteur relevés et figés en contraintes : STARTS WITH et CONTAINS inutilisables (0 ligne ou filtre ignoré), une instruction par invocation, INSERT d'arête non idempotent, count(x) après OPTIONAL MATCH compte les lignes.
- La preuve P4 (flux de types) a failli être construite sur une lecture fausse : GROUP BY sur deux expressions labels(...) recopie la cible dans la source (Function→Function partout).
- Durabilité synaptique vérifiée : la base survit aux redémarrages, le renforcement s'accumule (×7 après sept tissages), mais SET exige l'incrément en tête (binding périmé sinon) et open_shared_write perd les propriétés post-flush.
- Le mode RAM coûte ~0 en lecture et divise l'ouverture par 128 (6,5 ms → 51 µs) et la création de nœuds par 7,7 — un premier banc annonçait à tort la RAM 28-52 % plus lente (bruit : contrôle à blanc à 6,4 % de dérive).
- Le checkpoint wasm a été travaillé puis reverté : fusionner_index_* nomme ses fichiers avec std::process::id() qui panique sur wasm32 (« no pids ») → suite 22/22 → 1/22.
- La causalité d'Obrain est structurelle amortie (γ=0,5), pas une inférence formelle ; l'ADR décision la maintient hors de l'estimateur et exige un registre d'intervention (acted, source, SUIT_DE) avant d'accumuler.

**Trous / incohérences.**
- Doublon à résorber : fusionner ou supprimer l'un des deux ADR IAM (minuscule vs majuscule) ; le majuscule est la version la plus complète (section « Le canal »).
- Chemin imprécis dans ADR-2026-09-15 §11 : « examples/serveur_tls.rs » n'existe pas à la racine ; le fichier réel est crates/obrain-tls/examples/serveur_tls.rs.
- Chemin imprécis dans ADR-2026-09-16 : « repetition.rs » cité sans crate ; le fichier réel est crates/obrain-garde/src/repetition.rs (et non obrain-engine).
- StageContinu.tsx cité comme packages/obrain-dashboard/src/graph/StageContinu.tsx dans le périmètre ADR ; le fichier réel est packages/obrain-dashboard/src/components/StageContinu.tsx.
- CosmosView.tsx est absent (retiré, conforme à ADR-STAGE-CONTINU « l'ancien CosmosView est RETIRÉ ») — à confirmer qu'aucune référence morte ne subsiste.
- ADR confidentialité reste PROPOSÉ : la variante du self (V-A scellé vs V-B aveugle) n'est pas tranchée ; les coûts TEE/FHE/MPC sont NON MESURÉS.
- Branchement TLS sur run_serve() encore ouvert : la crate obrain-tls vit « à côté » du serveur ; le serveur réel sert toujours en clair (sans ce branchement, §1.2 reste vrai).
- Couche de décision : 0/6 critères d'acceptation, 0/14 tests, aucune ligne implémentée ; le registre d'intervention (P4-S5 : acted, source, SUIT_DE) doit être posé AVANT d'accumuler sous peine de perte sèche.
- Mode RAM : pas de persistance/snapshot natif (T17b non réglé), prouvé en Node mais pas dans un navigateur, coût en octets NON MESURÉ (pas de THP/madvise, tampons doublant à la réallocation).
- fetchLabelPairs (dashboard, vue Monde) utilise encore la forme à deux expressions labels(...) fautive : same[s] capte presque tout, cross quasi vide — reprise à faire (avertissement posé en tête de fonction).
- Bug moteur ouvert : agrégation GROUP BY sur deux expressions labels(...) (colonnes liées au même emplacement) et jointure (a:Label)-[r]->(b) vide sur self-demo pourtant peuplé.
- Réserves de sécurité nommées : jetons hachés SHA-256 sans sel (bcrypt/argon2 « in production »), audit non chaîné ni signé (magasin appartenant à l'opérateur), clair sur disque (hexdump du .obrain rend le graphe).
- La capacité has_backward_adjacency est vérifiée par échantillon déterministe (64 destinations), non exhaustive : un cas d'échec hors échantillon passerait inaperçu.



### 3.2 docs/PROPOSAL-*.md — 11 propositions (formes de recherche, relief, comparatif, CDC-GQL, froid, persistance indexer, audit spectral/courbure)

_Fichier : `raw/v3-2-proposals.json` — 34 composants · 12 relations · 12 constats · 10 trous_


**Synthèse.** Les 11 PROPOSAL couvrent deux vagues : d'abord les cinq formes de recherche GQL (search_delta/surprise/by_role/conflicts/causes-effects), toutes marquées EXÉCUTÉ avec commits et tests datés, puis les chantiers d'infrastructure (import relief via enrich --ricci, comparatif multi-variantes via obrain compare, CDC branché sur le chemin d'écriture GQL réel, ouverture/CDC à froid ~1 ms, optimisation by_role 776 ms → ~1-2 ms par enrich --hks, persistance obrain-indexer → substrate). Le point culminant est PROPOSAL-OPERATEUR-SPECTRAL-COURBE (1782 lignes), un audit structurel en lecture seule qui pose 12 diagnostics D1-D12 sur la couche cognitive et 4 propositions P1-P4 : la découverte maîtresse est que l'étage cognitif n'est pas câblé (fleet non démarré, bus sans source, tick hors timer, décroissance engram morte car is_due ≡ false) et que la courbure persistée ricci_u8 n'est ni l'Ollivier-Ricci ni la borne de Jost-Liu (formule fausse, seul test sur K3 où la formule fausse donne par accident le bon résultat, et aucun consommateur). La méthode est constante : reconnaissance du code réel avant design, DoD en 8 cases, benches datés consignés au ledger, commit unique réversible, zéro script JS (principe « obrain = Rust »), pas de déploiement prod. Les propositions récentes généralisent un patron additif « enrich-then-scan » (enrich --ricci, --hks, sidecars counts_cache/cdc_tail) pour sortir les calculs lourds du chemin de requête. L'audit spectral conclut que la plupart des diagnostics sont latents (non actifs) et recommande de trancher D9 avant tout réglage, puis de corriger la formule de courbure avant d'en faire une conductance.


**Composants clés.**
- docs/PROPOSAL-SEARCH-SURPRISE-2026-09-05.md — search_surprise (forme n°2, pivot CDC)
- docs/PROPOSAL-SEARCH-BY-ROLE-2026-09-05.md — search_by_role (forme n°3, HKS 3 échelles)
- docs/PROPOSAL-SEARCH-CONFLICTS-2026-09-05.md — search_conflicts (forme n°4, oscillation CDC)
- docs/PROPOSAL-SEARCH-CAUSES-EFFECTS-2026-09-05.md — search_causes/effects (forme n°5, BFS amorti)
- docs/PROPOSAL-IMPORT-RELIEF-2026-09-05.md — enrich --ricci (relief import)
- docs/PROPOSAL-COMPARATIF-MULTI-VARIANTES-2026-09-05.md — obrain compare (multi-variantes)
- docs/PROPOSAL-CDC-GQL-2026-09-05.md — CDC branché sur le chemin GQL
- docs/PROPOSAL-FROID-2026-09-06.md — ouverture + cdc_tail ~1 ms à froid
- docs/PROPOSAL-OPTIM-BY-ROLE-2026-09-06.md — by_role 776 ms → enrich --hks
- docs/PROPOSAL-LLAMA-OBRAIN-INDEX-PERSIST-2026-09-07.md — obrain-indexer → substrate
- docs/PROPOSAL-OPERATEUR-SPECTRAL-COURBE-2026-09-10.md — audit D1-D12 + P1-P4
- crates/obrain-substrate/src/geometric.rs — compute_hks_descriptors, node_curvature, refresh_all_ricci
- crates/obrain-substrate/src/store.rs — mutation_observer, create_node/create_edge, HILBERT_SORTED
- crates/obrain-substrate/src/mutation_observer.rs — trait StoreMutationObserver
- crates/obrain-engine/src/database/crud.rs — record_create_node/update (CDC), anti-doublon
- crates/obrain-engine/src/database/cdc.rs — CdcLog, changes_since_timestamp
- crates/obrain-engine/src/database/search_by_role.rs — module by_role (CALL)
- crates/obrain-engine/src/database/search_conflicts.rs — module conflicts (CALL)
- crates/obrain-engine/src/database/search_causality.rs — causes/effects (CALL)
- crates/obrain-engine/src/database/search_surprise.rs — module surprise (CALL)
- crates/obrain-core/src/execution/operators/merge.rs — MergeOperator/MergeEdgeOperator (chemin GQL)
- crates/obrain-cognitive/src/engram/manager.rs — EngramManager::tick, strength ×0.9, is_due
- crates/obrain-cognitive/src/engram/decay.rs — FSRS-5 (19 poids), FsrsScheduler
- crates/obrain-cognitive/src/engram/homeostasis.rs — synaptic_scaling, c=1+c(T-m)
- crates/obrain-cognitive/src/energy.rs — half_life, clamp Q1.15, decay sans appelant
- crates/obrain-cognitive/src/synapse.rs — reinforce, normalize_outgoing (violation D8)
- crates/obrain-cognitive/src/provenance.rs — ProvenanceRecorder (code mort), DashMap RAM
- crates/obrain-cognitive/src/thinker/mod.rs — spawn_standard_fleet (jamais appelé), trait Thinker
- crates/obrain-reactive/src/scheduler.rs — dispatch concurrent JoinSet (D10)
- crates/obrain-substrate/src/hilbert_enricher.rs — sonde biaisée, _hilbert_features
- crates/obrain-substrate/src/pipeline.rs — stage_derive_coact non idempotent (D12/P3)
- crates/obrain-indexer/src/index.rs — IndexOutput.graph en mémoire (M6)
- crates/obrain-indexer/src/git_history.rs — auteur git écarté (P4)
- crates/obrain-cli/src/commands/enrich.rs — sous-commande enrich --ricci/--hks

**Relations / dépendances.**
- search_* (delta/surprise/by_role/conflicts/causes-effects) = modules engine/src/database/*.rs enregistrés en procédures CALL, classées LECTURE via la liste d'autorisation query/plan.rs, dispatchées par query/planner/lpg/mutation.rs (try_execute_*_procedure)
- search_by_role → geometric.rs compute_hks_descriptors + CsrAdjacency (substrate) ; optim révisée via enrich --hks persisté puis scan de colonnes
- search_conflicts / search_surprise (pivot) / search_delta → fenêtre CdcLog (cdc.rs) alimentée par crud.rs record_update/record_create_node
- CDC-GQL : substrate mutation_observer (StoreMutationObserver) → engine CdcMutationAdapter (cfg feature=cdc) → CdcLog::record ; anti-doublon crud par drapeau cdc_observer_active
- chemin GQL réel = session.execute → MergeOperator/MergeEdgeOperator → SubstrateStore::create_node_with_props/create_edge_with_props (avant : aucun CDC)
- import relief : obrain enrich --ricci → geometric.rs refresh_all_ricci → ricci_u8 persisté dans EdgeRecord (update_edge + WAL) ; débloque node_curvature
- comparatif : obrain compare A B, identité nœuds par (label, qualified_name), Jaccard nœuds/arêtes ; orchestration par variante = 1 store par db codebase-memory
- froid : from_substrate → rebuild_counts_only (recount O(E)) + replay WAL ; sidecars substrate.counts_cache (CNT1) et substrate.cdc_tail (CDCT)
- persistance : obrain-indexer IndexOutput.graph (mémoire) → binaire persist → store substrate lu par obrain bridge RO → search_graph/BM25
- audit spectral : 14 lois de dissipation (D1) réparties energy/synapse/engram/stigmergie/activation/heat_diffusion/fabric/spectral + co_change/utility ; passes couplées par clés partagées (_cog_energy≡PROP_ENERGY, _kernel_embedding, DERIVED_FROM) et dispatch concurrent (D10)
- P1 ré-exprime décroissance/activation/stigmergie en un opérateur L_κ de conductances c_e=w_e·exp(κ_e) ; P2 remplace clamps par saturation σ(x)=x/(1+x/ε) + transport 1-D ; P3 étend trait Thinker de invariants()/residual() ; P4 ajoute ActorId+HLC+prev_hash à CognitiveEvent
- pipeline cognitif : Stage 6 (Ricci) gated par la sortie du Stage 7 (COACT), exécuté dans le désordre (pipeline.rs) ; couche simpliciale peuplée uniquement via CLI obrain topo materialize (jamais par neo4j2obrain/migrate/indexer)

**Points notables.**
- D11 (audit) : ricci_u8 n'est ni l'Ollivier-Ricci ni la borne de Jost-Liu — 3 identités d'erreur (K_n→1-κ, arête pendante→+1/d au lieu de 0, arbre k-régulier→κ/2) ; seul test sur K3, l'unique cas où la formule fausse est juste ; zéro consommateur (grep curvature/ricci = 0 dans engine/adapters/rag)
- D9 : l'étage cognitif est inerte — spawn_standard_fleet n'appelé que par examples/rust/poc_cognitive_store.rs:111 + 1 test ; MutationBus::publish sans source ; EngramManager::tick uniquement sur le chemin d'écriture MCP ; MemoryManager::start_periodic sans appelant
- La décroissance des engrammes n'existe pas : next_review/last_review jamais écrits (FsrsScheduler::review les recopie) → is_due ≡ false → ni ×0.9, ni purge <0.05, ni record_decay ne s'exécutent
- D4 : l'énergie (primitive phare) est un booléen en backend prod — clamp Q1.15 rend {0, 0.6321} ; decay_all_energy sans appelant ; effective_half_life (crochet structurel) jamais consommé, structural_reinforcement_alpha = 0.0
- D1 : 14 lois de dissipation avec 14 constantes de temps, zéro invariant partagé ; la loi FSRS-5 à 19 paramètres n'est PAS la loi appliquée — la force décroît par un ×0.9 plat (manager.rs:331)
- search_surprise a pivoté : la v1 « engramme » (FSRS retention_probability/hopfield) jugée creuse sur données réelles → v1 implémentée = variante (a), surprise d'activité CDC vs modèle nul proportionnel au degré (z-score)
- search_conflicts est une détection temporelle d'oscillation d'état (A→B→A via reversals CDC), pas une contradiction sémantique — NLP explicitement hors périmètre
- optim by_role a été révisée en cours de chantier : le parallélisme plafonne ~50-100 ms (×50 insuffisant) → architecture enrich-then-scan (enrich --hks persiste 3 flottants/nœud, by_role devient scan mmap 1,7 Mo → ≤2 ms)
- 3 paramètres du Consolidator (decay_factor=0.99, synapse_prune_threshold_u16, engram_reinforce_delta_u16) sont déclarés/désérialisés/résolus mais jamais consommés ; feature stigmergy déclarée mais absente de tout groupe de features
- D8 : la seule violation d'invariant non bornée et vivante est normalize_outgoing — borne Σ poids ≤ max_total violée d'un facteur 2^(Δt/half_life) (ex. raw=10, half_life=1s, Δt=10s, max_total=0.01)
- P4 : le module provenance.rs est du code mort (record() ne crée ni nœud ni arête, DashMap en RAM non Serialize) ; git_history.rs:93-94 lit l'historique git mais jette délibérément l'auteur (let _sha = parts.next()) — aucune entité actor n'existe (grep actor|author|model_id = 0)
- Annexes A et C sont mesurées (test d'intégration jetable obrain-adapters + script Python W₁ Kantorovich–Rubinstein) ; D1-D10 et D12 ne sont que des lectures de code — le document l'assume explicitement

**Trous / incohérences.**
- Portée D9/D10 limitée à ce checkout : obrain-hub et obrain-chat absents — si spawn_standard_fleet y est appelé, fleet et listeners tournent réellement là-bas (indéterminé depuis ce dépôt, première chose à vérifier avant d'agir)
- Valeurs déployées inconnues : HomeostasisConfig/EnergyConfig/FsrsConfig surchargeables, analyse sur les Default ; la condition cT<2 (D2) peut changer selon la config exploitant
- D1-D10 et D12 non mesurés (lectures/dérivations sans exécution ni banc) — seuls A et C sont vérifiés exécutés ; conformité numérique FSRS-5 non testée
- CDC sur import code (chemin natif zone-direct bulk) n'émet pas d'événements — relief CDC réservé aux bases vivantes, hors v1 (documenté, pas un bug)
- Couche simpliciale (SimplexRecord, homologie) jamais peuplée par l'import (uniquement CLI topo materialize) et jamais lue côté moteur — Betti/homologie absents (zéro occurrence)
- Leiden : refine() prend _gamma sans le lire (leiden.rs:692), sans BFS de γ-connexité — la garantie qui distingue Leiden de Louvain n'est pas implémentée
- normalize(x, max) de fabric.rs:516 fait dépendre l'échelle de tous d'un seul outlier (max global) ; régression ρ_k sur λ_k (Test 3 de P1) non encore exécutée — P1 non démontré empiriquement
- Le bit de marge de la colonne Q1.15 est peut-être volontairement réservé — à confirmer avant de corriger l'échelle d'encodage (P2, risque non tranché)
- search_conflicts sparse par nature (fenêtre runtime réelle = retour vide) — difficile de prouver la valeur sur données réelles ; search_causes/effects confondent tous les types d'arêtes (poids uniforme, filtre edge_types reporté v1.1)
- Rejeu WAL des kinds multiplicatifs non idempotent contre la spec (wal-spec.md:111-114 « Replay must be idempotent ») — garantie réelle = déterminisme sous ordre, pas idempotence (auto-contradiction wal.rs:309-311)



### 3.3 Conception représentation-nœud, slots512, pont mémoire zero-copy, tissage périmètre, plans j5..j9 (docs/DESIGN-*, docs/PLAN-*, docs/plan-j*, docs/plan-r1, PORTE-POST-DEPLOIEMENT)

_Fichier : `raw/v3-3-design-plan.json` — 34 composants · 12 relations · 11 constats · 11 trous_


**Synthèse.** La tranche cartographie la refonte de la représentation des nœuds (position vs rôle), le format de stockage PropsZone en slots de 512 octets, un pont mémoire zero-copy entre un moteur d'inférence embarqué et obrain, et la lignée de chantiers 'NATIF-J5..J9' qui migrent les consommateurs vers le format substrate v3 (SUBSTRATE_FORMAT_VERSION=3, meta.rs:20). DESIGN-REPRESENTATION-NOEUD tranche 9 décisions (D1-D9) par mesures reproductibles : 44,4 % des nœuds du graphe de production sont des puits sans arête sortante, l'encodage Hilbert 64d actuel est sous le hasard sur 3 topologies sur 5, et la position retenue = 16 coordonnées brutes de repères farthest-point (sans Hilbert, sans rank_normalize, sans PCA), le rôle étant séparé hors L2 en colonnes typées (clé edge_hw, HKS 3 échelles). DESIGN-PROPSZONE-SLOTS-512 remplace l'unité d'allocation 4K/RMW-4K par des unités de 512 o logées 7 par carrier 4K (magic 0xC5A2_50C8, UnitéAddr 48 bits à bit 47 discriminateur), sans réécriture des bases existantes. DESIGN-PONT-MEMOIRE-ZERO-COPY expose StrView<'store>/FactView qui empruntent les octets du heap mmap sous garde de lecture (0,223 µs/lecture = 7,4× vs chemin JSON, zéro unsafe) pour tokeniser/seeder sans copie. Le tissage périmètre (PLAN-TISSAGE-PERIMETRE, PLAN-AMELIORATIONS-PROTEGEES) gèle les invariants I1-I12 multi-writer et le protocole C1-C10 de livraison sans régression. Les plans j5..j9 industrialisent successivement : bascule v3 + runbook (j5), index ordonné par valeur props.ord.idx.v2 pour ORDER BY/min-max O(k) (j6), garde d'époque anti-index-périmé (j8), allègement payload gzip+JSON compact (j7), transport WebSocket streaming maison (j9) ; plan-r1 répare la corruption R1 (15 nœuds à chaînes d'adjacence cycliques) par reconstruction des chaînes depuis les EdgeRecord intacts.


**Composants clés.**
- docs/DESIGN-REPRESENTATION-NOEUD-2026-09-10.md — décisions D1-D9 position/rôle, repères FPS 16d, producteur unique ensure_features, test golden remap
- docs/DESIGN-PROPSZONE-SLOTS-512-2026-09-06.md — unités 512 o, carrier 4K (7 slots), UnitéAddr 48 bits, magic 0xC5A2_50C8
- docs/DESIGN-PONT-MEMOIRE-ZERO-COPY-2026-09-10.md — StrView/FactView/ResultBuf, contrat de lifetimes, PoC 0,223 µs/lecture
- docs/PLAN-FLIP-SLOTS512-2026-09-06.md — bascule prod du format slots512 (garde SUBSTRATE_FORMAT_VERSION 1→2, QC parité par base)
- docs/PLAN-TISSAGE-PERIMETRE-2026-09-06.md — carte unique du périmètre multi-writer + contrat bridge, invariants I1-I12
- docs/PLAN-AMELIORATIONS-PROTEGEES-2026-09-04.md — protocole C1-C10, phases P1a..P5b, ledger de mesures
- docs/plan-j5.md — contrat de tests consommateurs v3 (jalons A/B/C/D, compteurs de slots)
- docs/plan-j5-bascule.md — runbook de bascule v3 + POST-MORTEM prod + réserves R1/R2/R3
- docs/plan-j6.md — index ordonné de propriété (props.ord.idx.v2, min/max O(1), ORDER BY LIMIT k)
- docs/plan-j7.md — allègement payload viz (gzip flate2 + JSON compact)
- docs/plan-j8.md — garde ORDER BY/min-max consciente de l'époque de l'index (ancres wal/pages)
- docs/plan-j9.md — transport vivant viz : WebSocket streaming maison (handshake RFC6455 std-only)
- docs/plan-j9-protobuf.md — blueprint Phase C protobuf (.proto viz_stream.proto, différé)
- docs/plan-r1.md — réparation R1 (arêtes cycliques), rebuild_edge_chains + CLI repair-edge-chains
- docs/PORTE-POST-DEPLOIEMENT-2026-09-05.md — Porte 5 continuité d'écriture (obrain continuity, seuil STALE)
- crates/obrain-substrate/src/props_zone.rs — PropsZone v2, read_heap_page/read_heap, walk_chain, encode/decode_page_id
- crates/obrain-substrate/src/page.rs — magics NODE/EDGE (page.rs:16/21), header 4K, HeapPage::read_at
- crates/obrain-substrate/src/file.rs — garde format_version (file.rs:939/1280-1316), est_zone_props_v2_nom
- crates/obrain-substrate/src/meta.rs — SUBSTRATE_FORMAT_VERSION = 3 (meta.rs:20), MetaHeader
- crates/obrain-substrate/src/props_index.rs — PropsIndexZone (props.idx.v2), EntreeIndex cle12+slot, candidats_ordonnes
- crates/obrain-substrate/src/props_ord_index.rs — zone props.ord.idx.v2 triée par valeur, ancres (ancre_wal, ancre_pages)
- crates/obrain-substrate/src/store.rs — node_prop_str_view, walk_outgoing_chain, rebuild_edge_chains, splice_lock
- crates/obrain-core/src/execution/operators/sort.rs — bounded_top_k, SortOperator::with_limit(k)
- crates/obrain-engine/src/database/mod.rs — ObrainDB::rebuild_edge_chains, délégation substrate_handle
- crates/obrain-cli/src/mcp_server.rs — prop_str, runtime_search, search_graph (contrat bridge CBM)
- crates/obrain-cli/src/viz_serve.rs — respond_cache, app_dir (CARGO_MANIFEST_DIR), endpoints /api/viz/subgraph,/api/query
- crates/obrain-cli/src/viz_ws.rs — primitives WebSocket maison (handshake SHA-1+base64, send_text/send_binary)
- crates/obrain-substrate/tests/zero_copy_views.rs — 9 tests d'intégration StrView (zone/blob/LWW/tombstone/goldens LE)
- crates/obrain-substrate/tests/contrat_j5_props_truncate.rs — contre-épreuve BUG-PROPS-TRUNCATE (3 cycles open/close)
- crates/obrain-engine/tests/orderby_epoque_j8.rs — garde d'époque (nouveau min inséré/supprimé sous écriture)
- crates/obrain/benches/zero_copy.rs — bench criterion execute+JSON vs get_node_property vs prop_str_view
- crates/obrain/examples/seeding_zero_copy.rs — démo seeding 480 tokens depuis StrView sans String intermédiaire
- packages/obrain-dashboard/src/lib/api.ts — client WS + repli HTTP (J9), types GraphNode/VizNode
- scripts/verifier-deploiement.sh — Porte 5 continuité (writer ≥1 + continuity exit 0)

**Relations / dépendances.**
- DESIGN-PROPSZONE-SLOTS-512 → crates/obrain-substrate/src/props_zone.rs + page.rs : UnitéAddr remplace encode_page_id/decode_page_id (props_zone.rs:97-105), 7 slots par carrier 4K
- PLAN-FLIP-SLOTS512 → file.rs/meta.rs : bascule SUBSTRATE_FORMAT_VERSION 1→2 (refus ouverture file.rs:939), puis v3 (meta.rs:20) sur la lignée principal-natif
- DESIGN-PONT-MEMOIRE-ZERO-COPY : SubstrateStore::node_prop_str_view → blobcol → PropsZone v2 (LWW/tombstone) → DashMap héritée ; Session::prop_str_view (obrain-engine) → réexport crate faîtière obrain
- StrView::Zone porte MappedRwLockReadGuard sur le heap mmap de PropsZone ; StrView::Blob porte Arc (parking_lot arc_lock) sur BlobColumnWriter — aucun unsafe ajouté
- DESIGN-REPRESENTATION-NOEUD D8 : deux producteurs concurrents hilbert_enricher.rs:202 et neo4j2obrain/src/pipeline.rs:1255, remplacés par ensure_features(store, stamp) unique
- plan-j6 : props_ord_index.rs (zone props.ord.idx.v2 triée par valeur) alimente bounded_top_k/SortOperator::with_limit(k) de obrain-core/src/execution/operators/sort.rs
- plan-j8 : store.rs pk_ord_valeur_pret re-confronte les ancres (ancre_wal, ancre_pages) à chaque requête ; garde project.rs try_plan_order_by_index + aggregate.rs try_plan_minmax_shortcut
- plan-j9 : viz_serve.rs api_subgraph refactoré en prepare_subgraph → PreparedSubgraph ; viz_ws.rs streame canon.nodes/canon.edges ; api.ts (dashboard) repli fetch HTTP+gzip
- plan-r1 : crates/obrain-cli/src/commands/repair.rs (obrain repair-edge-chains --apply) → ObrainDB::rebuild_edge_chains (engine) → SubstrateStore::rebuild_edge_chains (substrate, splice_lock)
- PORTE-POST-DEPLOIEMENT : obrain continuity --stale-after-min → scripts/verifier-deploiement.sh (Porte 5), surveille writer pgrep -f "obrain bridge"
- PLAN-TISSAGE-PERIMETRE : invariants I1-I12 fichés sur file.rs, multi_writer_invariants.rs, multi_writer_wal.rs, write_tracker.rs ; contrat bridge = contrat canal cbm (M4)
- DESIGN-REPRESENTATION-NOEUD D9 : bulk_sort_by_hilbert + compact_community permutent physiquement les slots → tout side-car indexé par slot non enrôlé corrompt silencieusement (vecteur de A devient celui de B)

**Points notables.**
- Le graphe de production (363 346 nœuds, 1 176 915 arêtes) a 44,4 % de puits (161 384 sans arête sortante) : le garde min_deg < 1e-10 de spectral_embedding se déclenche nécessairement et les 16 premières des 64 dimensions sont des rangs de degré anti-informatifs
- 16 coordonnées brutes de repères farthest-point (16 f32, 64 o/nœud) écrasent la cascade Hilbert 64d : scale-free 0.839 vs 0.508, anneaux 0.988 vs 0.787, grille 0.999 vs 0.921 — et 4× moins de mémoire
- Le plus grand gain mesuré de l'étude n'est pas la position : l'agrégat de position pour le rappel d'engrams passe recall@1 de 2 % à 86 % (+84 points), car l'indice actuel (hachage de NodeId) rend deux engrams disjoints orthogonaux par construction
- La couche de paires fusionnée (voisins communs/Adamic-Adar) fait +0.087 d'AUC sur scale-free (0.839→0.926), la seule voie qui dépasse la borne du vecteur par nœud — mais 72-97 % d'ex æquo masquent son apport aux métriques AUC
- Le pont zero-copy atteint 0,223 µs/lecture (168 ns sur DGX GB10 aarch64), soit 7,4× vs le chemin execute+JSON+re-parse, avec 0 bloc unsafe et 1 doc-test compile_fail garantissant que la vue ne survit pas au store
- La bascule v3 (j5) migre 15 stores en <11 s cumulées (runtime-dev 12G en 10,6 s) et fait chuter obrain_voisinage de 4160 ms à 74 ms ; la migration ne réécrit que la zone nodes (32→40 octets, rename atomique, méta bumpée en dernier = point de commit idempotent)
- plan-j8 révèle un bug de justesse : has_pending_writes() n'est armé que par les transactions explicites, jamais par les écritures auto-commit (TransactionId::SYSTEM, mode des bridges writers) — un index ordonné périmé rendait un ancien min ou une ligne fantôme ; corrigé par garde d'époque sur les ancres (wal, pages)
- La réserve R1 (plan-r1) : 15 nœuds à chaînes d'adjacence CYCLIQUES (incident dedup du 08/09, avant le splice_lock) explosent la traversée sortante ×16 (20,9M vs 1,31M arêtes) — les records sont intacts, seuls les pointeurs next_from/next_to sont faux ; reconstruction depuis les records (1 348 records + 761 têtes, soit 0,1 %)
- Le POST-MORTEM j5 documente un piège de déploiement : viz_serve.rs::app_dir() résout le dashboard via env!("CARGO_MANIFEST_DIR") figé À LA COMPILATION — après échange du binaire, /app renvoyait 404 pendant 30 min jusqu'à copie du dist au chemin du worktree de build
- Le serveur viz reste délibérément std-only/synchrone (thread-par-connexion) : pour J9, tokio-tungstenite est rejeté et le WebSocket (handshake RFC6455 + frames texte) est réimplémenté maison en ~120 lignes (SHA-1+base64 inline), zéro dépendance
- Le binaire v1 déployé mentait sur les counts (compteurs de manifeste rassis : 424 258 annoncés vs 424 320 énumérés) et rendait 0 arête sortante sur dsh-runtime (degrés rassis) — le v3 est auto-cohérent (count = énumération = schéma)

**Trous / incohérences.**
- Le chemin littéral docs/PORTE-POST-DEPLOIEMENT n'existe pas : le fichier réel est docs/PORTE-POST-DEPLOIEMENT-2026-09-05.md (nom suffixé par date)
- DESIGN-REPRESENTATION-NOEUD cite crates/obrain-core/src/operators/sort.rs, mais le chemin réel de bounded_top_k est crates/obrain-core/src/execution/operators/sort.rs (répertoire execution/ intermédiaire)
- La borne LMR (D5, décroissance de l'AUC en n) est explicitement NON mesurée — 'c'est la première chose à exécuter' ; le régime obrain (10⁵-10⁶ nœuds) reste non validé
- Le consommateur de classement réel (CompositeEmbedding 532d, où _hilbert_features pèse 0.15) est HORS checkout : l'effet produit de D2 sur les rangs 1-4 ne peut pas être vérifié dans le dépôt
- Aucune compaction de chaînes (tombstones accumulés, chaînes append-only) n'est traitée : reportée indéfiniment dans DESIGN-PROPSZONE-SLOTS-512 (§7) et PLAN-FLIP-SLOTS512 (hors périmètre)
- Le test golden du remap (D9) n'est pas écrit à la date du document alors qu'il est désigné comme le seul mode de défaillance silencieux et corrupteur — action prioritaire non encore réalisée
- P5b (C6 : mémoire de sessions cognitive) reste EN ATTENTE dans PLAN-AMELIORATIONS-PROTEGEES — design doc référencé mais implémentation = round dédié non engagé
- La copie à chaud de plan-r1 porte 30 nœuds 'husk' (label sans propriété joignable, props écrites après le dernier flush perdues) — sans rapport avec les 15 nœuds corrompus mais non résolus sur la copie
- Consommateurs non couverts par la bascule j5 (à la charge de l'orchestrateur) : les 3 viz dev wami :8433/:8435/:8448, tmn (non trouvé), les 3 bridges MCP du chat (config à basculer à la main), et la boucle chat LLM complète jamais exercée
- Le surcoût résiduel de l'étage LECTURE DE PROPRIÉTÉS (~2× serveur, ~0,7 µs/lecture en plus) reste non tranché en plan-j5-bascule R3 (candidats : stride 40 o, est_visible_pour, vintage du binaire v1)
- plan-j9 Phase C (protobuf) et plan-j9 Phase B (deltas) sont différés derrière la Phase A WS ; la négociation enc=pb n'est qu'un blueprint à exécuter en J10 (prost/prost-build absents de Cargo.lock)



### 3.4 États, consolidations et jalons (docs/CONSOLIDATION-*, ETAT-*, CHECKPOINT-*, INSTANTANE-*, CONSTAT-*, DIAG-*, RAPATRIEMENT-NANO)

_Fichier : `raw/v3-4-etats-consolidation.json` — 28 composants · 15 relations · 13 constats · 14 trous_


**Synthèse.** Tranche des documents d'état, de consolidation et de jalons du dépôt obrain (14 fichiers, du 2026-09-04 au 2026-09-17) qui retracent l'historique d'état d'un moteur de graphe Rust en consolidation permanente : du checkpoint feat/zorder taggé v0.0.1-zorder (09-04) jusqu'à l'instantané exact bit-à-bit OBRSNAP1 (09-17). Chaque jalon est ancré sur des commits, shas et comptes de tests réels (substrate 798→812 tests, engine 678→683) et sur des mesures datées (native-import 0,27 s pour 445 k/1,67 M, overview ×276 à froid, CONTAINS ×41 via trigrammes). Les documents révèlent une lignée qui se resserre de 34 branches non fusionnées vers 2, avec deux régressions silencieuses retrouvées par merge-base/cargo check plutôt que par relecture, et une cause racine structurelle : deux lignées divergentes dont le tronc récent (342a61a1) ne compile qu'avec des fichiers non suivis. Le fil moteur relie le blindage anti-perte (graph-writer acquitté C1/C2), le fix de remap des chaînes PropsZone v2 (d7f1a303 puis auto-guérison au rejeu 57e185f1) et l'émergence du format d'instantané (v1→v2→v3, round-trip prouvé). Des résidus restent ouverts : la compaction des pages de propriétés (T17d, 97,4 % de l'instantané), compact_community non couvert, et plusieurs mesures non faites.


**Composants clés.**
- docs/CHECKPOINT-ZORDER-2026-09-04.md
- docs/CHECKPOINT-DEPLOIEMENT-2026-09-05.md
- docs/DIAG-NON-PERSISTANCE-2026-09-05.md
- docs/ETAT-2026-09-05-2015.md
- docs/CONSTAT-REMAP-PROPS-DETACHEMENT-2026-09-06.md
- docs/ETAT-OPTIM-FROID-2026-09-06.md
- docs/ETAT-DASHBOARD-2026-09-09.md
- docs/CONSOLIDATION-OBRAIN-2026-09-10.md
- docs/ETAT-COSMOS-2026-09-10.md
- docs/CONSOLIDATION-2026-09-13.md
- docs/RAPATRIEMENT-NANO-2026-09-15.md
- docs/ETAT-CONSOLIDE-2026-09-15.md
- docs/CONSOLIDATION-2026-09-16.md
- docs/INSTANTANE-2026-09-17.md
- crates/obrain-substrate/src/instantane.rs
- crates/obrain-substrate/src/props_zone.rs
- crates/obrain-substrate/src/record.rs
- crates/obrain-substrate/src/page.rs
- crates/obrain-substrate/src/store.rs
- crates/obrain-substrate/src/meta.rs
- crates/obrain-engine/src/database/admin.rs
- crates/obrain-cli/src/viz_serve.rs
- crates/obrain-cli/src/commands/native_import.rs
- packages/session/session-persistence/src/graph-writer.ts
- packages/session/session-persistence/src/coordinator.ts
- packages/obrain-dashboard/src/graph/CosmosWeave.tsx
- scripts/bench-verify/
- scripts/acceptation-toutes.sh

**Relations / dépendances.**
- docs/CONSOLIDATION-2026-09-13.md → docs/CONSOLIDATION-OBRAIN-2026-09-10.md : reprend la branche integration/zero-copy (merge 4889f5a7) et l'empile dans consolidation/2026-09-13
- docs/CONSOLIDATION-2026-09-16.md → docs/CONSOLIDATION-2026-09-13.md : part de consolidation/tronc-release-2026-09-15 et rapatrie le socle perf/chemin-nanoseconde + 34 branches
- docs/RAPATRIEMENT-NANO-2026-09-15.md → docs/ETAT-CONSOLIDE-2026-09-15.md : label_counts() rapatrié puis intégré à fusion/tronc-plus-j6-mes-correctifs
- docs/CONSTAT-REMAP-PROPS-DETACHEMENT-2026-09-06.md → crates/obrain-substrate/src/props_zone.rs : rafraichir_owners_de_chaine re-stampe les chaînes v2 après slot_remap
- docs/DIAG-NON-PERSISTANCE-2026-09-05.md → packages/session/session-persistence/src/graph-writer.ts : canal acquitté + auto-guérison (C1) ; → coordinator.ts re-push JSONL→graphe (C2)
- docs/RAPATRIEMENT-NANO-2026-09-15.md → crates/obrain-engine/src/database/admin.rs : label_counts() = all_labels() + load atomique par étiquette
- docs/INSTANTANE-2026-09-17.md → crates/obrain-substrate/src/instantane.rs : exporter/restaurer/stats, format OBRSNAP1, CRC32 par zone
- crates/obrain-substrate/src/instantane.rs → crates/obrain-substrate/src/record.rs : lit les strides NodeRecord 40 B / 36 B / SimplexRecord 32 B
- docs/INSTANTANE-2026-09-17.md → crates/obrain-substrate/src/props_zone.rs : chaîne de pages liées first_prop_off/next_page, PAGE_SIZE=4096, curseur monotone (T17d)
- docs/CHECKPOINT-ZORDER-2026-09-04.md → scripts/bench-verify/ : kit de vérification commité, sorties docs/bench-verify/2026-09-04-*.txt
- docs/ETAT-CONSOLIDE-2026-09-15.md → crates/obrain-cli/src/viz_serve.rs : api_overview réécrite en opérations nommées (0,684 ms à froid)
- docs/ETAT-DASHBOARD-2026-09-09.md → packages/obrain-dashboard : React 18 + Jotai + Three.js, servi par obrain viz serve
- branches : feat/zorder (5ae779d2) → perf/chemin-nanoseconde (8c980863) → consolidation/2026-09-13 → consolidation/tronc-2026-09-15-plus-fusion (HEAD 456d3794)
- deux lignées : tronc e9826f3b (socle perf) vs tronc 342a61a1 (feat/mcp-next-step-hints, non auto-cohérent sans fichiers non suivis)
- dépôts : banc-obrain (integration/zero-copy) et llama-obrain (integration/inject-2026-09-10 = production 87c279b)

**Points notables.**
- L'ordre chronologique des jalons est strictement monotone et daté : 09-04 (zorder) → 09-05 (déploiement + diag persistance) → 09-06 (remap props + froid) → 09-09/10 (dashboard + cosmos + consolidation 2 dépôts) → 09-13/15/16 (consolidations) → 09-17 (instantané)
- La prod harness a été basculée dès le 09-04 sur runtime-full.obrain (256 757 nœuds / 252 146 arêtes en 34 s) — source de vérité dsh-runtime.db
- DIAG-NON-PERSISTANCE documente une base qui a 'menti' ~1 h : journal figé à 20:13:08, 148 327 événements rejoués au retour du writer (21:21:33)
- CONSTAT-REMAP-PROPS est le seul doc à porter un SUIVI 2 : fix live d7f1a303 puis auto-guérison au rejeu 57e185f1 (fenêtre power-loss fermée)
- CONSOLIDATION-OBRAIN prouve que master de llama-obrain est 6 commits EN RETARD sur la production (:9910) — quiconque repart de master réintroduit le DoS
- Les 3 tests cross-type (operators_filter) échouent à l'identique sur main (2c6c5bab), ~150 commits avant zero-copy — préexistence prouvée, pas supposée
- Deux régressions silencieuses dans la lignée servie : clé de cache ayant perdu la GÉNÉRATION du moteur, et disparition de sans_mesure/from_cache (restaurées ff3f463a)
- La cause racine du 09-13 : le tronc 342a61a1 appelle ancrage.rs/engine.rs/viz_weave.rs mais ces fichiers sont NON SUIVIS — un git clone à ce commit ne compile pas
- RAPATRIEMENT-NANO mesure un gain ×276 à froid (165,4 ms → 0,6 ms) et révèle que labels(n)[0] dépend de l'ordre d'insertion, pas de la sémantique du nœud
- INSTANTANE révèle que substrate.props.v2 = 97,4 % de l'instantané, à cause de ~1 page de 4 Kio par nœud porteur (curseur monotone, compaction différée à T17d)
- L'instantané v2 gagne −40,3 % (14 078 652 → 8 408 183 octets) uniquement en découpant les zéros de queue de grow_to — sans perte, prouvé bit-à-bit
- Trois incidents de méthode consignés : git add non re-stagé (feat/orbites commité cassé), git commit --amend sur le mauvais commit, et pkill -f qui matche sa propre ligne de commande
- Quatre branches fermées par git merge -s ours sur preuve (git cherry = 0 commit non équivalent), dont feat/recherche-couplee déjà absorbée (fuse_results/RRF présents)

**Trous / incohérences.**
- Résidu 1 du CONSTAT-REMAP : compact_community (writer) permute aussi les slots mais n'a aucun driver store-level identifié — non couvert par le refresh
- Le repro simulé une_prop_ecrite_apres_un_remap_de_slot_ne_perd_pas_la_chaine reste #[ignore] (passerait au vert si la garde d'append devenait remap-aware)
- ETAT-COSMOS : base abîmée sur disque — 30 191 décodages de propriétés en échec sur 241 nœuds (WAL frame invalid: StringRef out of range), récupérés en mémoire seulement
- ETAT-COSMOS : 6 mondes sur 12 n'ont aucune fibre (aucun qualified_name partagé) — la moitié de la carte est vide de trajectoires
- ETAT-OPTIM-FROID : bug pré-existant seek_relative + BufReader — cdc_persist::load() s'arrête ~19,9 Mo / 174 k frames au lieu d'atteindre used (1,66 M frames)
- INSTANTANE : le WAL (journal de rejeu) n'est pas transporté par l'instantané — seules les zones + méta + dictionnaire le sont
- INSTANTANE : en mode mémoire, node_count/edge_count de la méta valent 0 (non flushés) — l'instantané compte des SLOTS (26 315) pas des entités vivantes (2 000)
- RAPATRIEMENT-NANO : edge_counts_by_type() balaie O(#arêtes) et avait été mesuré faux d'un facteur 2 dans la lignée NANO — non exposé délibérément
- ETAT-CONSOLIDE : les assertions /api/query (GROUP BY label 89 ms, GROUP BY type d'arête 455 ms, CONTAINS 'session-' 954 ms) restent des balayages, non réécrites
- CONSOLIDATION-09-16 : 3 assertions de fratrie-check.mjs échouent sur le 8432 pour raisons de données (aucun qn __file__, phase Cosmos sur store runtime) + 1 instable
- CONSOLIDATION-09-16 : le déploiement --release du tronc consolidé au 8432 n'a pas été fait (le service tournait encore sur la lignée d'avant)
- CONSOLIDATION-09-16 : le worktree de la lignée consolidée vit dans /tmp/tpc-j6 (volatil) — pas de worktree durable
- INSTANTANE : NON MESURÉ — aucun bytes/nœud d'instantané n'existait avant, coût des reconstructeurs de colonnes latérales (degrés/Hilbert/communautés) non mesuré
- sauvegarde/worktree-principal-2026-09-13 (226 fichiers) : son contenu propre = stores parasites (substrate.*, engine_idx/substrate.*) + CosmosWeave.tsx.bak — archivé, sort à trancher (jamais fusionner)



### 3.5 docs/ — sous-répertoires structurés (architecture, getting-started, user-guide, tutorials, cognitive, algorithms, api, rfc, ecosystem, contributing, includes, overrides, examples, golden, bench, bench-verify, perf, dashboard-viz, mermaid, javascripts, assets, hooks)

_Fichier : `raw/v3-5-docs-structures.json` — 42 composants · 17 relations · 9 constats · 10 trous_


**Synthèse.** La tranche cartographiée couvre 22 sous-répertoires structurés de docs/ (hors fichiers plats racine), pilotés par mkdocs.yml (379 lignes, thème Material, site_name 'Obrain', repo this-rs/obrain) dont la nav référence la majorité des dossiers pédagogiques : getting-started, cognitive, user-guide, tutorials, api, algorithms, ecosystem, contributing, architecture. Le cœur éditorial est user-guide/ (74 fichiers, 12 sous-dossiers par langage de requête GQL/Cypher/Gremlin/GraphQL/SPARQL/SQL-PGQ et par binding Python/Rust), suivi de architecture/ (12 fichiers, modèle de stockage columnar/adjacency/zone-maps, moteur d'exécution vectorisé, transactions MVCC) et de cognitive/ (7 fichiers : energy, synapses, engrams, fabric, rag + un pipeline canonique _hilbert/_kernel/_st_embedding). Une famille 'opérationnelle' non publiée dans la nav (rfc/ 18 fichiers, mermaid/ 28 diagrammes .mmd, golden/, bench/, bench-verify/, perf/, dashboard-viz/) documente le travail de conception et de mesure en cours. La machinerie MkDocs repose sur des hooks (copy_markdown.py qui copie CHANGELOG.md racine vers docs/ et tous les .md vers site/md/), des snippets (includes/abbreviations.md), un override de thème (overrides/main.html) et des assets (logo-full/logo-small/playground.png). Des incohérences existent entre la nav déclarée et le contenu réel (favicon manquant, document orphelin, RFC hors nav), et des doublons de fichiers apparaissent (principal-natif v1/v2, deux ADR quasi identiques à la racine).


**Composants clés.**
- docs/mkdocs.yml — configuration MkDocs centrale (nav 379 lignes, thème Material, hooks, plugins marimo/minify/search/git-revision-date)
- docs/index.md — page d'accueil 'The graph database that learns' (302 lignes, hero, quickstart Python/Rust, grille cognitive vs traditionnel)
- docs/architecture/index.md — vue d'ensemble modulaire (mermaid UI/engine/adapters/core/common) + design principles
- docs/architecture/overview.md — architecture haut niveau, principes, flux de requête
- docs/architecture/crates.md — responsabilités des crates
- docs/architecture/storage/index.md + adjacency.md + columnar.md + compression.md + zone-maps.md — modèle de stockage
- docs/architecture/execution/index.md — moteur d'exécution vectorisé push-based et parallélisme morsel-driven
- docs/architecture/optimization/index.md — optimisation cost-based, join ordering, estimation de cardinalité
- docs/architecture/memory/index.md — buffer manager, allocateurs arena, spill-to-disk
- docs/architecture/transactions/index.md — MVCC, snapshot isolation, détection de conflits
- docs/getting-started/index.md — guide d'entrée (prérequis par langage)
- docs/getting-started/installation.md + quickstart.md + first-graph.md + configuration.md + cli.md + migration.md + security.md + troubleshooting.md + glossary.md
- docs/user-guide/index.md + transactions.md — index du guide utilisateur et transactions
- docs/user-guide/gql/ — 16 fichiers (basic-queries, patterns, filtering, expressions, aggregations, paths, mutations, composite-queries, functions-{string,numeric,temporal,element}, types, schema, transactions, conformance)
- docs/user-guide/cypher/ — 7 fichiers (basic-queries, patterns, filtering, aggregations, paths, mutations)
- docs/user-guide/gremlin/ + graphql/ + sparql/ + sql-pgq/ — guides par langage de requête
- docs/user-guide/data-model/ — nodes, edges, properties, lpg-vs-rdf, gql-vs-sparql
- docs/user-guide/vector-search/ — basics, hnsw-index, quantization, python-api
- docs/user-guide/python/ + rust/ — bindings embarqués (database, nodes, edges, transactions, results)
- docs/user-guide/extending/ — plugins, storage-backends, custom-functions, rust-extensions
- docs/user-guide/persistence/ — in-memory, persistent, wal
- docs/tutorials/ — social-network, knowledge-graph, recommendations, fraud-detection + examples/ (graph-visualization, vector-search, fraud-detection-example, networkx-integration)
- docs/cognitive/overview.md + energy.md + synapses.md + engrams.md + fabric.md + rag.md — couche cognitive
- docs/cognitive/canonical-features-pipeline.md — pipeline _hilbert_features/_kernel_embedding/_st_embedding (document orphelin, hors nav)
- docs/algorithms/ — centrality, community, link-prediction, similarity, metrics, path-finding
- docs/api/ — index + python/ rust/ node/ go/ wasm/ c/ (référence API multi-langages)
- docs/rfc/principal-natif.md + principal-natif-v2.md + m6-code-graph-natif.md + synapse-incidence.md + zorder-hilbert-dimension-base.md + diff-incremental-variantes.md
- docs/rfc/substrate/ — 12 fichiers (format-spec, wal-spec, multi-writer, zorder-superposition, tier-persistence, double-journalisation-wal-props, propszone-pages-partagees, migration-audit, cognitive-quality*)
- docs/ecosystem/ — obrain-server, obrain-web, obrain-langchain, obrain-llamaindex, obrain-mcp, obrain-memory, graph-bench, anywidget-{graph,vector}, performance
- docs/contributing/ — setup, code-style, testing, pull-requests
- docs/includes/abbreviations.md — snippet d'abréviations auto-appliqué (pymdownx.snippets)
- docs/overrides/main.html — override de thème (bannière announce commentée)
- docs/hooks/copy_markdown.py — hook pre/post build (sync CHANGELOG.md, copie .md vers site/md/)
- docs/examples/viz/ — viewer.html, viewer3d.html, sub.json, sub.svg, README-demo.md + vendor/{three.min.js, OrbitControls.js}
- docs/golden/2026-09-04/ — golden files (bridge-sonde.jsonl, cli-schema.json, cli-stats.json, cli-info.json, gql-counts.jsonl, 2026-09-04-engine.txt, 2026-09-04-o1.txt)
- docs/bench/ — bench-queries-complet-2026-09-07.md, linux-kernel-2026-09-07.md, linux-kernel-RC-2026-09-07.md
- docs/bench-verify/ — README.md, VERIFY-GAINS.md, CHIFFRES-AMELIORATIONS.md + sorties .txt de bench
- docs/perf/ — ENDPOINTS.md, endpoints.json, chat-ancrage.png
- docs/dashboard-viz/ — GRAPH.md, perimeter-8432.md, CHARGE-SUBGRAPH-2026-09-15.md, PAYLOAD-json-graph-2026-09-15.md, VERIFICATION-2026-09-15.md
- docs/mermaid/ — 28 diagrammes .mmd (banc-obrain-*.mmd, engine-*.mmd, moteur-*.mmd, obrain-*.mmd, harness-obrain.mmd, vite-rs-devtools.mmd)
- docs/javascripts/mathjax.js — chargement MathJax pour pymdownx.arithmatex
- docs/assets/ — logo-full.png, logo-small.png, playground.png

**Relations / dépendances.**
- mkdocs.yml → docs/index.md, getting-started/*, cognitive/*, user-guide/*, tutorials/*, architecture/*, api/*, algorithms/*, ecosystem/*, contributing/* (nav hiérarchique)
- mkdocs.yml theme.custom_dir → docs/overrides/ (main.html)
- mkdocs.yml extra_css → docs/stylesheets/extra.css ; extra_javascript → docs/javascripts/mathjax.js
- mkdocs.yml hooks → docs/hooks/copy_markdown.py (on_pre_build / on_post_build)
- hooks/copy_markdown.py → ../CHANGELOG.md (racine) copié vers docs/changelog.md ; docs/llms.txt → site/ ; tous les .md → site/md/ (exclut includes/ et overrides/)
- mkdocs.yml pymdownx.snippets auto_append → docs/includes/abbreviations.md (injecté dans toutes les pages)
- docs/architecture/index.md → architecture/overview.md, crates.md, storage/, execution/, optimization/, memory/, transactions/
- docs/getting-started/index.md → user-guide/index.md, tutorials/index.md, api/index.md (what's next)
- docs/cognitive/canonical-features-pipeline.md → crates obrain-adapters (hilbert_features, KernelManager), obrain-engine (embedding::onnx), obrain-chat (retrieval), zones substrate (veccol/blobcol/props.v2/tier*/synapse/engram)
- docs/rfc/principal-natif.md → crates/obrain-core (base_registry.rs AccessContext), obrain-engine (session/mod.rs), obrain-iam (lib.rs, session_ext.rs)
- docs/rfc/substrate/format-spec.md → obrain-substrate (format binaire mmap, NodeRecord/EdgeRecord/PropertyPage/StringHeap/tier0-2/hilbert/zorder/wal)
- docs/dashboard-viz/GRAPH.md → packages/obrain-dashboard (StoreOverview.tsx, api.ts, viz_serve.rs, vizserve.sh) + perimeter-8432.md
- docs/examples/viz/viewer*.html → examples/viz/sub.json + sub.svg + vendor/three.min.js + vendor/OrbitControls.js (viewers web auto-suffisants)
- docs/golden/2026-09-04/ ↔ docs/bench-verify/*.txt (mêmes noms engine/o1 : golden de référence vs sorties de vérification)
- docs/ecosystem/obrain-*.md → packages/obrain-cli-npm, obrain-cli-python, obrain-dashboard (produits de l'écosystème)
- docs/user-guide/gql/* ↔ docs/algorithms/* et docs/api/* (GQL comme langage par défaut, lié aux algos GDS et à la référence API)
- docs/rfc/principal-natif.md ↔ principal-natif-v2.md (v1 vs v2 du même RFC — relation de version/succession)

**Points notables.**
- user-guide/ est de loin le dossier le plus dense (74 fichiers) : il documente 6 langages de requête (GQL, Cypher, Gremlin, GraphQL, SPARQL, SQL/PGQ) et 2 bindings embarqués (Python, Rust), confirmant la stratégie multi-langage du produit.
- docs/cognitive/canonical-features-pipeline.md est un document récent (Sep 5) qui décrit le pipeline unique de features cognitives (_hilbert_features 64d, _kernel_embedding 80d, _st_embedding 384d) mais n'est PAS référencé dans la nav mkdocs.yml — document orphelin.
- Le dossier rfc/ (18 fichiers, dont 12 dans rfc/substrate/) est entièrement absent de la nav mkdocs.yml : la documentation de conception (format binaire substrate, multi-writer, zorder) n'est pas publiée sur le site.
- mermaid/ contient 28 diagrammes .mmd (banc-obrain-*, engine-007-*, moteur-*) générés autour du 30 Sep/2 Oct, documentant l'architecture des crates et le moteur, mais ils sont hors nav et hors rendu HTML (fichiers sources uniquement).
- examples/viz/vendor/ embarque three.min.js (603 Ko) et OrbitControls.js (26 Ko) : des bibliothèques tierces vendées directement dans l'arbre de documentation pour les viewers 2D/3D locaux.
- docs/ contient 383 fichiers .md au total, mais la nav ne couvre qu'une partie : les dossiers opérationnels (bench, bench-verify, perf, golden, dashboard-viz) servent de journal de mesure/qualification plutôt que de documentation publiée.
- rfc/substrate/format-spec.md annonce 'Target crate: obrain-substrate (new, workspace member of lab/grafeo)' — une référence à un workspace lab/grafeo qui n'apparaît pas dans la liste officielle des crates/ (~24 crates).
- Deux fichiers racine quasi identiques coexistent : ADR-2026-09-13-chaine-iam-et-explosion-des-liens.md et ADR-2026-09-13-CHAINE-IAM-ET-EXPLOSION-DES-LIENS.md (mêmes dates, casse différente) — doublon probable à la racine de docs/.
- Le hook copy_markdown.py expose un mécanisme 'llms.txt' et une copie de tous les .md vers site/md/ destinés à la consommation LLM — une chaîne d'ingestion pour agents documentée dans l'outillage du site.

**Trous / incohérences.**
- mkdocs.yml référence theme.favicon 'assets/favicon.ico' mais ce fichier n'existe pas (assets/ ne contient que logo-full.png, logo-small.png, playground.png) — favicon cassé au build.
- docs/cognitive/canonical-features-pipeline.md est orphelin : présent sur disque (Sep 5) mais absent de la nav mkdocs.yml.
- rfc/ (18 fichiers) est entièrement hors nav mkdocs.yml — aucun lien de publication vers la documentation de conception.
- mermaid/ (28 .mmd), golden/, bench/, bench-verify/, perf/, dashboard-viz/ sont hors nav — documents de travail non publiés, sans porte d'entrée depuis le site.
- Doublon probable à la racine docs/ : ADR-2026-09-13-chaine-iam-et-explosion-des-liens.md vs ADR-2026-09-13-CHAINE-IAM-ET-EXPLOSION-DES-LIENS.md (même sujet, casse différente).
- Doublon de RFC : principal-natif.md (26 Ko) et principal-natif-v2.md (30 Ko) — la relation v1→v2 n'est pas explicitée par la nav (rfc non publié).
- rfc/substrate/format-spec.md référence un workspace 'lab/grafeo' absent de la liste crates/ officielle — incohérence potentielle de provenance du crate obrain-substrate.
- overrides/main.html contient un bloc announce entièrement commenté (code mort, aucune bannière active).
- examples/viz/ mélange contenu de démo et bibliothèques tierces vendées (three.min.js 603 Ko, OrbitControls.js) dans l'arbre docs/ — poids et provenance non tracés.
- La nav référence comparisons/ et roadmap.md (existants) mais ces éléments sortent du périmètre des 22 dossiers cartographiés ; la frontière entre docs publiés (nav) et docs opérationnels (hors nav) n'est documentée nulle part.




## Vague 4 — V4 Ramifications

### 4.1 perf/* worktrees (banc-forme, banc-obrain-agregats, banc-obrain-cout, banc-obrain-gql, banc-obrain-heap, banc-obrain-nano, banc-obrain-ns, banc-obrain-scan, banc-obrain-voisinage, banc-repli, banc-surface, banc-plan, /mnt/obrain/wt-perf-index, /mnt/obrain/wt-perf-indexeur)

_Fichier : `raw/v4-1-perf-worktrees.json` — 30 composants · 8 relations · 10 constats · 7 trous_


**Synthèse.** Quatorze worktrees de branche perf/* cartographiés (12 dans /home/triviere/projects/ + 2 dans /mnt/obrain/), tous ancrés sur des lignées de la branche de consolidation du dépôt principal (consolidation/tronc-2026-09-15-plus-fusion @ 456d3794) et rédigés en messages de commit français. Ils forment quatre lignées emboîtées : (1) cache-de-requêtes/garde-adaptative (base 26215d42) portant banc-forme, banc-repli, banc-surface, banc-plan ; (2) projection/compteurs/repair-props (8b051962 → cbab7bb0 → 57fcdfff) portant banc-obrain-gql, banc-obrain-agregats, banc-obrain-heap, banc-obrain-nano ; (3) cout/scan/voisinage (b5259757 → 92984209 → 22f3c4d8 → 4e7f6f2d) portant banc-obrain-cout, banc-obrain-scan, banc-obrain-voisinage ; (4) la lignée indexeur T27 (empreinte de dépôt) portant wt-perf-indexeur puis wt-perf-index. Chaque branche porte une optimisation micro-mesurée : l'intention `contient(prop, aiguille)` sans texte GQL (banc-forme), la réconciliation O(1) des compteurs du substrat (agregats), le banc de plage `STARTS WITH` non exécuté (cout), la projection de lot consommée plutôt que recopiée (gql), le message de réparation heap (heap), les opérations nommées de l'overview (nano), les index par défaut au data-load (ns), le scan plafonné `node_ids_capped` (scan), le ré-ancrage du voisinage entrant (voisinage), le rejeu de fenêtre trigrammes évité sur fichier muet (repli), la ventilation HTTP et la date du store en mémoire (surface), le planificateur qui cesse de lire les données (plan), l'écriture atomique de ligne de metrics (wt-perf-index) et l'empreinte de dépôt u128 (wt-perf-indexeur). Les gains annoncés vont de 341 ms → 14 µs (planification) à 375 001 slots → 0 (voisinage entrant), 1 293 s → ~60 s (indexation noyau Linux) et 77,1 ms → opération nommée (histogramme de labels).


**Composants clés.**
- /home/triviere/projects/banc-forme/crates/obrain-engine/src/database/contient.rs — ObrainDB::contient(prop, aiguille) : intention sans texte, deux routes (index `noeuds_par_predicat_texte` ou balayage vérifié), contrat QueryResult identique à GQL
- /home/triviere/projects/banc-forme/crates/obrain-engine/tests/contient_intention.rs — comptes comparés à GQL (égalité des deux routes)
- /home/triviere/projects/banc-forme/crates/obrain-engine/examples/bench_intention_contient.rs — banc de l'intention contient
- /home/triviere/projects/banc-obrain-agregats/crates/obrain-substrate/src/store.rs — compteurs atomiques (total_live_nodes), double fuite bouchée, invariant prouvé, réconciliation à open()
- /home/triviere/projects/banc-obrain-agregats/crates/obrain-rag/src/context.rs — ajustements du contexte RAG liés aux compteurs
- /home/triviere/projects/banc-obrain-cout/crates/obrain-engine/benches/plage_prefixe.rs — banc STARTS WITH (prefix_range → intervalle [p, haute(p)) vs CONTAINS témoin)
- /home/triviere/projects/banc-obrain-cout/crates/obrain-engine/Cargo.toml — ajout de la cible bench
- /home/triviere/projects/banc-obrain-gql/crates/obrain-core/src/execution/operators/project.rs — ProjectOperator : le lot de propriétés est consommé (Option::take) au lieu d'être recopié
- /home/triviere/projects/banc-obrain-heap/crates/obrain-cli/src/commands/repair.rs — raconter_props : message expliquant pourquoi --apply ne touche pas les entrées ensevelies
- /home/triviere/projects/banc-obrain-nano/crates/obrain-cli/src/viz_serve.rs — overview : 3 requêtes GQL remplacées par des opérations nommées (label_counts())
- /home/triviere/projects/banc-obrain-ns/crates/obrain-cli/src/commands/data.rs — data load construit les index par défaut (qualified_name, name), flag --no-index, jamais d'index vide
- /home/triviere/projects/banc-obrain-ns/crates/obrain-cli/src/commands/index.rs — rebuild_property_indexes()
- /home/triviere/projects/banc-obrain-ns/crates/obrain-cli/src/main.rs — branchement des commandes index
- /home/triviere/projects/banc-obrain-scan/crates/obrain-core/src/execution/operators/scan.rs — ScanOperator::load_batch via nodes_by_label_capped / node_ids_capped
- /home/triviere/projects/banc-obrain-scan/crates/obrain-core/src/graph/traits.rs — GraphStore::node_ids_capped (plafond de lignes)
- /home/triviere/projects/banc-obrain-scan/crates/obrain-substrate/src/compteurs.rs — compteurs plafonnés du substrat
- /home/triviere/projects/banc-obrain-scan/crates/obrain-substrate/src/writer.rs — compteurs au fil de l'écriture
- /home/triviere/projects/banc-obrain-scan/crates/obrain-substrate/tests/scan_plafonne.rs + crates/obrain-engine/tests/scan_plafonne.rs — garde qui empêche le scan de mentir
- /home/triviere/projects/banc-obrain-voisinage/crates/obrain-engine/src/query/planner/lpg/filter.rs — try_plan_reancrage_entrant (Expand inverse, ré-ancrage sur la cible indexée)
- /home/triviere/projects/banc-obrain-voisinage/crates/obrain-engine/tests/voisinage_entrant.rs — contrat d'égalité voisinage entrant
- /home/triviere/projects/banc-repli/crates/obrain-substrate/src/store.rs — completer_trigrammes_par_fenetre : rejeu de fenêtre évité sur fichier MUET
- /home/triviere/projects/banc-surface/crates/obrain-cli/src/viz_serve.rs — vent::Phases (OBRAIN_SURFACE_PHASES=1), ventilation d'UN appel, store_mtime en mémoire, qcache par (store, clé)
- /home/triviere/projects/banc-plan/crates/obrain-engine/src/query/planner/lpg/filter.rs — le planificateur cesse de lire les données pour décider (341 ms → 14 µs)
- /home/triviere/projects/banc-plan/crates/obrain-engine/src/session/mod.rs — SessionConfig/plan froid, accès store au moment du plan
- /home/triviere/projects/banc-plan/crates/obrain-engine/examples/banc_plan_froid.rs + tests/plan_materialisation_plafonnee.rs
- /mnt/obrain/wt-perf-index/crates/obrain-cli/src/mcp_server.rs — metrics_append : une ligne = une écriture (write_all) pour ne plus entrelacer les morceaux
- /mnt/obrain/wt-perf-indexeur/crates/obrain-indexer/src/index.rs — empreinte_depot(root, opts) -> u128, IndexOutcome::empreinte_depot
- /mnt/obrain/wt-perf-indexeur/crates/obrain-indexer/src/cache_extraction.rs — cache d'extraction par empreinte de contenu
- /mnt/obrain/wt-perf-indexeur/crates/obrain-indexer/tests/empreinte_depot.rs — empreinte de dépôt sans indexer
- /mnt/obrain/wt-perf-indexeur/crates/obrain-indexer/src/index.rs — tables internes en foldhash + complexity_pass sur tableaux denses (commits antérieurs)

**Relations / dépendances.**
- Lignée cache-requêtes/garde-adaptative (base 26215d42, 307b623a, 3e4908b9, 41f78060) → portée par banc-forme, banc-repli, banc-surface, banc-plan
- banc-obrain-gql (cbab7bb0 projection) → repris dans banc-obrain-agregats (57fcdfff contient cbab7bb0) → repris dans banc-obrain-nano (8c980863 contient 57fcdfff + cbab7bb0)
- Lignée repair-props (8b051962, d5ed6493) partagée par banc-obrain-gql, banc-obrain-agregats, banc-obrain-heap
- banc-obrain-cout (b5259757…92984209) → banc-obrain-scan (22f3c4d8 sur 92984209) → banc-obrain-voisinage (4e7f6f2d sur 00b4ad77 scan plafonné + cout)
- Lignée indexeur T27 : wt-perf-indexeur (ad8da840 empreinte dépôt) est l'ancêtre direct de wt-perf-index (fed43e7f == ad8da840), qui ajoute pont MCP/écriture anticipée/import en lot/metrics_append
- banc-obrain-ns est sa propre lignée trigrammes/CONTAINS (da2d6e7f → c2aa9229) sur obrain-cli + obrain-engine
- Tous les worktrees pointent vers le tronc consolidation/tronc-2026-09-15-plus-fusion @ 456d3794 du dépôt principal banc-obrain
- obrain-cli::viz_serve (nano, surface) et obrain-cli::commands (ns, heap) dépendent tous d'obrain-engine (planner) et obrain-substrate (store/compteurs/trigrammes)

**Points notables.**
- banc-plan : le planificateur cesse de lire les données pour décider — 341 ms → 14 µs de planification (plan froid)
- banc-obrain-voisinage : ré-ancrage du voisinage entrant sur la cible indexée — 375 001 slots scannés → 0
- banc-obrain-cout : banc 'conçu pour être lu sous charge — et NON EXÉCUTÉ' (résultat = rapport entre bras entrelacés, jamais valeur absolue)
- banc-surface : ventilation d'UN appel HTTP armée par OBRAIN_SURFACE_PHASES=1, avec postes lecture/route/params/mtime/generation/qcache
- banc-surface : la date du store (store_mtime) est gardée en mémoire au lieu d'être relue à chaque appel (cache qcache par (store, clé))
- wt-perf-index : writeln! non tamponné entrelaçait les morceaux de lignes JSON entre écrivains concurrents → remplacé par un seul write_all
- wt-perf-indexeur : empreinte de dépôt u128 pour savoir SANS indexer qu'une réindexation ne changerait rien (T27 : 1 293 s → ~60 s sur le noyau Linux)
- banc-obrain-nano : divergence sémantique assumée — labels(n)[0] (première étiquette) vs label_counts() (chaque étiquette), équivalents en mono-étiquette seulement
- banc-forme : ObrainDB::contient rend un QueryResult strictement identique à GQL (columns=['n'], rows de Value::Int64) — aucune forme nouvelle
- banc-obrain-ns : data load construit les index par défaut n°3 (qualified_name, name), --no-index les débranche, jamais d'index vide

**Trous / incohérences.**
- rev-list --count origin/main..HEAD est vide sur les 12 worktrees /home/triviere/projects : la référence origin/main n'existe pas dans ces worktrees → divergence exacte vs tronc non mesurable directement (tronc réel = consolidation/tronc-2026-09-15-plus-fusion @ 456d3794)
- banc-obrain-cout : le banc plage_prefixe.rs est écrit mais jamais exécuté (aucun résultat mesuré enregistré)
- Duplication de logique compteurs entre banc-obrain-agregats (store.rs) et banc-obrain-scan (compteurs.rs/writer.rs) — à réconcilier lors de la fusion
- banc-obrain-nano : divergence sémantique label_counts() vs labels(n)[0] non résolue pour les nœuds multi-étiquettes (testé, assumé, mais risque de non-équivalence)
- Les chiffres 'mesurés' (825 ms, 14,4 ms, 77,1 ms, 12,8 ms) sont portés en commentaires/bancs, sans artefacts de bench reproductibles pour surface et nano
- wt-perf-indexeur et wt-perf-index partagent le même commit racine T27 (ad8da840 == fed43e7f) → risque de double-maintenance de l'empreinte de dépôt
- Pas de documentation de synthèse consolidant les 14 branches perf/* en un seul récit d'optimisation (les gains sont épars dans les messages de commit)



### 4.2 feat/principal-natif (j01-j05) + zero-copy + orbites + provenance + selflink + recherche-couplee + m6-code-graph + orderby-epoque-j8

_Fichier : `raw/v4-2-feat-natif-zero-copy.json` — 33 composants · 12 relations · 10 constats · 10 trous_


**Synthèse.** La lignee « principal natif » fait descendre l'identite tenant dans le substrat obrain, pilote par deux RFC (docs/rfc/principal-natif.md v1 puis principal-natif-v2.md). Le v2 casse trois decisions du v1 : le filtre tenant vit dans les primitives mmap (meme boucle que TOMBSTONED), pas dans noeud_visible_pour ; une SEULE rupture de format 32→40 B livre ensemble le drapeau ALLOCATED et la colonne tenant_id ; le Principal est un canal type immuable, pas l'AccessContext mutable. L'implementation j01→j04 est une chaine lineaire propre (jalon 0 machinerie de migration versionnee, jalon 1 ALLOCATED + format 1→2, jalon 2 tenant_id + format 2→3, jalon 3 PrincipalSubstrat + avec_principal + filtre/cascade, jalon 4 sondes + mesure de phases + consolidation tronc-complet), mais j05 diverge depuis j03 et non depuis j04. Les worktrees j06-j09 ne sont PAS la suite de principal-natif : ils portent une lignee parallele « index ordonne » (index-ordonne-j6 → payload-viz-j7 → orderby-epoque-j8 → transport-ws-j9). La lignee zero-copy (StrView/FactView/ResultBuf/arene) offre un pont memoire ~7,4x sans copie vers les moteurs d'inference. Orbites derive les echelles de navigation de l'arbre de prefixes qualified_name ; provenance-aretes preserve la provenance des appels a l'indexation ; selflink/selfd installe un micro-harness par nœud ; recherche-couplee apporte recherche couplee + engrams positionnels + ancrage ; m6-code-graph remplace l'indexeur C externe par un indexeur natif substrate.


**Composants clés.**
- docs/rfc/principal-natif.md — RFC v1 « isolation native : le principal descend dans le substrat » (481 lignes, branche docs/principal-natif, worktree banc-obrain-rfc)
- docs/rfc/principal-natif-v2.md — RFC v2 « identite tenant descend dans le substrat », corrige 3/5 decisions du v1 (548 lignes, docs/principal-natif-v2, banc-obrain-rfc2)
- docs/rfc/m6-code-graph-natif.md — RFC M6 : indexeur de code natif obrain remplacant codebase-memory C (245 lignes, etat brouillon)
- docs/DESIGN-PONT-MEMOIRE-ZERO-COPY-2026-09-10.md — design du pont memoire zero-copy O1 (178 lignes)
- crates/obrain-substrate/src/migration.rs — machinerie de migration versionnee (jalon 0/1/2) : table(), migration_1_vers_2, ecrire_version_meta
- crates/obrain-substrate/src/record.rs — NodeRecord 32→40 B (tenant_id + _reserve), node_flags::ALLOCATED (bit 6), module tenant::{PUBLIC, SENTINELLE_NON_MIGRE, visible}
- crates/obrain-substrate/src/meta.rs — SUBSTRATE_FORMAT_VERSION 1→3, meta_flags::MONO_TENANT_PUBLIC
- crates/obrain-substrate/src/principal.rs — PrincipalSubstrat (tenant only, immuable via OnceLock, refuse la sentinelle)
- crates/obrain-engine/src/database/mod.rs — ObrainDB::avec_principal(tenant_id) (passthrough, point d'entree jalon 4)
- crates/obrain-substrate/src/store.rs — SubstrateStore::avec_principal, portee_tenant(), filtre tenant::visible, cascade aretes extremites_dans_la_portee, compteurs filtres
- crates/obrain-substrate/src/warden.rs — heuristique trous basculee sur ALLOCATED (jalon 1)
- crates/obrain-substrate/src/writer.rs — pose ALLOCATED sur tout record ecrit (jalon 1)
- crates/obrain-substrate/src/replay.rs + wal.rs — tenant_id transporte par le WAL (jalon 2)
- crates/obrain-substrate/tests/trous_allocated.rs, tenant_colonne.rs, filtre_principal.rs — tests de migration/isolation
- crates/obrain-engine/tests/principal_docking.rs — contrat de branchement du principal (jalon 3)
- crates/obrain-engine/examples/banc_open_ro_j4.rs, cree_store_melange_j4.rs, sonde_edges_j4.rs, sonde_j4.rs — sondes J4
- crates/obrain-engine/src/session/mod.rs — mesure de phases OBRAIN_PHASES (jalon 4)
- scripts/contrat-j5-consommateurs.sh — batterie consommateurs v3 (jalons B/C du plan-j5)
- crates/obrain-substrate/src/view.rs — StrView (Zone/Blob/Shared), brique zero-copy
- crates/obrain-substrate/src/result_buf.rs — ResultBuf, arene reutilisable (offset + String recyclee)
- crates/obrain-substrate/src/blob_column.rs + blob_column_registry.rs — gardes Arc des colonnes blob
- crates/obrain/benches/zero_copy.rs + zero_copy_suite.rs — benchs criterion (7,4x mesure)
- crates/obrain/examples/seeding_zero_copy.rs — demo tokenisation depuis la vue (480 tokens)
- crates/obrain-cli/src/orbites.rs — derivation des echelles depuis l'arbre de prefixes qualified_name (633 lignes)
- crates/obrain-indexer/src/discovery.rs — provenance-aretes : exclusion avec RAISON, artéfacts sans hub (348 lignes)
- crates/obrain-cli/src/mcp_server.rs — preservation de la provenance des appels a l'import (provenance-aretes)
- crates/obrain-selfd/* (main.rs, agent.rs, chat.rs, ecoute.rs, identite.rs, mcp.rs, outils.rs, politique.rs, protocole.rs, serveur.rs, soi.rs) — micro-harness Rust par nœud
- crates/obrain-selflink/src/engine.rs + main.rs — contact minimal avec le moteur, sans binaire ni base
- crates/obrain-httpmin/src/lib.rs — HTTP minimal pour selfd/selflink
- crates/obrain-cli/src/ancrage.rs, hubs.rs, outils.rs, memoire.rs — ancrage store + outil obrain_recherche (recherche-couplee)
- crates/obrain-cognitive/src/engram/position.rs + position_substrate.rs — engrams positionnels (1563 + 548 lignes, recherche-couplee)
- crates/obrain-indexer/src/graph.rs + examples/queries.rs + examples/bench_queries.rs — indexeur M6 natif (queries agent + bench)
- packages/obrain-dashboard/src/graph/CosmosWeave.tsx — rendu cosmos (recherche-couplee)

**Relations / dépendances.**
- j01 (4facbce5) → j02 (3b440588) → j03 (a8894383) → j04 (a8df6fd7) : chaine lineaire verifiee par merge-base --is-ancestor
- j05 (9a3f954b) diverge depuis j03 (merge-base = a8894383), PAS depuis j04 — la chaine principal « j04→j05 » est rompue
- j06 (index-ordonne-j6) → j07 (payload-viz-j7) → j08 (orderby-epoque-j8) → j09 (transport-ws-j9) : lignee « index ordonne » distincte
- docs/principal-natif (RFC v1) → docs/principal-natif-v2 (RFC v2, remplace v1, 3 decisions cassees) ; base commune afbb5cfc
- afbb5cfc (base RFC) N'EST PAS un ancetre de feat/principal-natif-j01 — l'implementation part de main (release v0.0.1-m6-zorder 2c6c5bab)
- feat/recherche-couplee (afbb5cfc) → CLI importe dans j01 (« alignement complet sur le CLI de feat/recherche-couplee »)
- RFC v1 decision 2 (filtre dans noeud_visible_pour) CORRIGEE en v2 decision A3 (filtre dans primitives mmap node_ids/nodes_by_label_capped)
- avec_principal → portee_tenant() → tenant::visible(rec.tenant_id, portee) → cascade aretes via extremites_dans_la_portee / degre_filtre_par_portee
- feat/zero-copy-views (83c53b28) et feat/zero-copy-suite (0b29991d) divergent depuis e6ea350e (merge-base) — vues vs suite
- feat/zero-copy-suite (0b29991d) → integration/zero-copy (af0490a2) : consolidation zero-copy
- feat/m6-code-graph derive de feat/zorder (b64f4928, selon RFC) ; indexeur natif substrate remplace codebase-memory C
- obrain-selfd + obrain-selflink + obrain-httpmin : trois crates ajoutes sur feat/selflink, branchés sur le harness

**Points notables.**
- Le RFC v2 renverse la decision centrale du v1 : le tenant se teste dans la MEME boucle mmap que TOMBSTONED, car node_ids/nodes_by_label_capped ne passent pas par noeud_visible_pour
- Le drapeau ALLOCATED (bit 6 de flags) regle DEUX bugs en un seul bump de format : le comptage des trous (10 créés / 132 lisibles) et la fuite trou=public qu'aurait creee tenant_id=0 + zero-remplissage
- 40 ∤ 4096 : le passage 32→40 B force NODES_PER_PAGE=512 et NODE_ALLOC_PAGE_BYTES=20480 (5 pages OS, zero slack), au lieu de la page OS 4 KiB
- Zero-copy : StrView tient la garde de lecture de la zone (RwLockReadGuard::try_map / read_arc) — le contrat de lifetime est porte par le borrow checker (doc-test compile_fail), zero unsafe ajoute
- Mesure zero-copy : vue 223 ns vs execute+JSON 1646 ns (7,4x x86_64), 168 ns vs 651 ns (3,9x aarch64 DGX GB10) — la vue est le plancher sur les deux archs
- Orbites : rapatrier 1,55 M de chaines en projection brute (3,7 s) coute MOINS cher que d'agreger un seul rang (5-8 s) — le GROUP BY interdit le raccourci du planificateur
- j04 est une consolidation geante (170 fichiers, +54854 lignes) qui merge feat/tronc-complet-2026-09-13, pas du travail « principal » pur ; j05 efface 54690 lignes (hygiene dist + scripts)
- provenance-aretes : chaque exclusion d'indexation porte sa RAISON, et les artéfacts cessent de fabriquer des hubs — la provenance des CALLS n'est plus jetée a l'import
- selflink introduit un micro-harness RUST (obrain-selfd) par nœud, parce que le harness Node ne peut pas démarrer partout
- Le nom « feat/principal-natif-j01..j9 » est trompeur : seuls j01-j05 existent ; j06-j09 sont des branches distinctes (index ordonne / payload viz / orderby epoque / transport WS)

**Trous / incohérences.**
- feat/principal-natif-j6..j9 N'EXISTENT PAS — les worktrees banc-obrain-j6..j9 portent index-ordonne-j6, payload-viz-j7, orderby-epoque-j8, transport-ws-j9 (lignee parallele)
- j05 diverge depuis j03 et non j04 : les sondes J4 (banc_open_ro_j4.rs…) sont ABSENTES de la lignee j05 ; le chainage nominal j01→j05 est incohérent avec le nom des branches
- Le RFC v2 declare « derivee de HEAD afbb5cfc » mais feat/principal-natif-j01 descend de main/zorder (2c6c5bab), afbb5cfc n'etant pas un ancetre — divergence terrain/documentation
- Jalon 4 (session_as / Principal{reader,tenant,roles} immuable par session) N'EST PAS implemente : seul PrincipalSubstrat (tenant) + avec_principal par HANDLE existe ; la multiplexion par session est reportee
- Jalon 5 (consommateurs this-rs / obrain-tmn / viz:8432) non realise dans ces branches ; this-rs n'est pas un depot git et obrain-tmn pointe vers ~/projects/lab/grafeo-pillars (inexistant)
- Le dictionnaire de tenants (substrate.tenantcat / internement Uuid↔u32, RFC §5.1) n'est pas implemente dans j02/j03 — la colonne tenant_id existe mais la table de resolution manque
- Le re-tenanting par lots (M2 du v1) et le durcissement fail-closed (M3) ne sont pas visibles dans j01-j05
- feat/zero-copy-views (prop views) et feat/zero-copy-suite (edge views + ResultBuf) divergent depuis e6ea350e et ne sont pas fusionnees entre elles (seul le pont « suite » part en integration/zero-copy)
- RFC m6-code-graph-natif est marque « brouillon — sections a completer » : exploration de l'indexeur C inachevee, schema cible partiellement documente
- Le crate obrain-substrate dans banc-obrain-natif ne contient que 16 crates au total (vs ~24 annonces) — la surface du worktree j01 est reduite par rapport au tronc



### 4.3 Branches IAM — lignée feat/iam-* (obrain-iam + obrain-cli)

_Fichier : `raw/v4-3-iam-branches.json` — 23 composants · 10 relations · 8 constats · 9 trous_


**Synthèse.** La lignée IAM se déploie sur deux baselines parallèles : la release tronc (b6c6da2c « LA FAMILLE 7… », non ancêtre de HEAD) porte feat/iam-mint-endpoint puis feat/iam-groupes, tandis que la branche principale (HEAD 456d3794) porte feat/iam-groupes-tronc (rebase de groupes sur main) puis feat/iam-audit-borne (audit borné). feat/iam-mint-endpoint ajoute Garde::emettre_service et le point de remise de jeton POST /api/iam/token ; feat/iam-groupes ajoute le modèle Group (User-[:MEMBER_OF]->Group-[:HAS_ROLE]->Role), Garde::controler_ressource sur un ORN quelconque, les mots de passe argon2id (PHC, sel par compte) et le cache des politiques effectives par principal. feat/iam-audit-borne couronne la chaîne avec l'audit borné (lectures autorisées agrégées, purge de rétention signée) et constitue aussi la pointe de integration/projets. La branche fix/iam-pont-lecture (6cbb4d30) vérifie les identités au pont MCP sans écrire dans iam.obrain via Garde::depuis_env_lecteur et IamStore::sans_audit ; elle a été fusionnée dans integration/t27 et est ancêtre des 4 branches IAM mais PAS de HEAD. Le point de fourche entre main et tronc-release est 20fc3d38. La bibliothèque obrain-iam (crates/obrain-iam, 12 modules) reste la source unique de la règle, appliquée en opt-in (`--features iam`) aux surfaces viz_serve.rs et mcp_server.rs, et administrée hors bande par la sous-commande `obrain iam` (commands/iam.rs).


**Composants clés.**
- crates/obrain-iam/src/garde.rs — Garde : authentifier/controler/controler_action/controler_ressource/emettre_service/renouveler/connecter/deconnecter/audit/depuis_env_lecteur/agrege/purger_audit
- crates/obrain-iam/src/admin.rs — Administration (noyau) : plafonnee_par_l_acteur, verrouillage du dernier administrateur, trace des gestes
- crates/obrain-iam/src/model.rs — User, Role, Group, Policy, AuditEvent, EDGE_MEMBER_OF, EDGE_HAS_ROLE (Group absent du HEAD)
- crates/obrain-iam/src/store.rs — IamStore : CRUD identités, evaluate_access, sans_audit, log_audit_event
- crates/obrain-iam/src/provider.rs — ObrainIamProvider/AuthProvider, argon2id PHC, empreintes SHA-256
- crates/obrain-iam/src/audit_chaine.rs — chaîne d'audit signée (Journal, recu, empreinte_evenement)
- crates/obrain-iam/src/orn.rs — Orn (parsing, wildcard, substitution de variables)
- crates/obrain-iam/src/policy.rs — moteur d'évaluation allow/deny/implicit-deny (fail-closed)
- crates/obrain-iam/src/session_ext.rs — AuthenticatedSession (intégration session)
- crates/obrain-iam/src/gdpr.rs — export/effacement RGPD (horodatage gelé préexistant)
- crates/obrain-iam/src/error.rs — IamError / IamResult
- crates/obrain-iam/src/lib.rs — façade de la crate, ré-exports publics
- crates/obrain-iam/tests/audit_chaine.rs — tests de la chaîne d'audit
- crates/obrain-iam/tests/la_cle_prime_sur_le_droit.rs — tests de la chaîne IAM (garde)
- crates/obrain-cli/src/viz_serve.rs — endpoints HTTP /api/auth/*, /api/iam/*, /api/iam/token (mint), page /login et /iam
- crates/obrain-cli/src/mcp_server.rs — pont MCP, point de dispatch unique de call_tool()
- crates/obrain-cli/src/commands/iam.rs — sous-commande CLI `obrain iam` (hors bande)
- crates/obrain-cli/tests/iam_ressource_et_404.rs — tests d'intégration HTTP ressource/404
- scripts/verif-chaine-iam.sh — porte de vérification de la chaîne IAM de bout en bout
- scripts/acceptation-iam.sh — porte d'acceptation CLI + HTTP de l'administration IAM
- docs/IAM-BRANCHE-2026-09-13.md — revue de la chaîne IAM branchée (garde, armement, maillons)
- docs/ADMINISTRATION-IAM-2026-09-13.md — administration des comptes (noyau, garanties, pièges)
- docs/ADR-2026-09-13-CHAINE-IAM-ET-EXPLOSION-DES-LIENS.md — ADR chaîne IAM et contrôle de l'explosion des liens

**Relations / dépendances.**
- obrain-cli --features iam -> obrain-engine/cognitive-iam -> obrain-cognitive/iam -> obrain-iam (chaîne de feature opt-in)
- viz_serve.rs::handle() et mcp_server.rs::call_tool() appellent Garde::authentifier + check_graph_access/controler_action (garde unique, fail-closed)
- commands/iam.rs et viz_serve.rs (/api/iam/*) partagent le même noyau Administration (obrain_iam::admin) — une règle, deux surfaces
- feat/iam-groupes-tronc (569f8a08) est le REBASE de feat/iam-groupes (e068e458) sur HEAD 456d3794 (base tronc b6c6da2c abandonnée)
- feat/iam-audit-borne (574d03f7) = feat/iam-groupes-tronc + commit « audit borné » ; il est aussi la pointe de integration/projets
- feat/iam-groupes (e068e458) descend de feat/iam-mint-endpoint (a399665b), lui-même basé sur tronc-release b6c6da2c
- fix/iam-pont-lecture (6cbb4d30) a été fusionné dans integration/t27 (commit 9fed71eb) et est ancêtre des 4 branches IAM, mais absent de HEAD 456d3794
- Point de fourche main/tronc-release = 20fc3d38 (ancêtre de HEAD, non-ancêtre de b6c6da2c)
- IamStore::evaluate_access collecte les politiques en passant par les rôles (node_to_role/node_to_policy filtrent status=deleted)
- Garde::emettre_service (mint) est consommé par la route POST /api/iam/token de viz_serve.rs

**Points notables.**
- Deux baselines parallèles : mint/groupes vivent sur tronc-release (b6c6da2c), groupes-tronc/audit-borne sur main (456d3794) — même jeu de features rebasé deux fois
- Le HEAD principal (456d3794) ne contient NI Group/MEMBER_OF, NI argon2id, NI emettre_service, NI controler_ressource, NI depuis_env_lecteur, NI l'audit borné : tout vit encore sur les branches IAM
- fix/iam-pont-lecture n'est pas dans HEAD mais est déjà fusionné dans integration/t27 et présent dans les 4 branches IAM
- feat/iam-audit-borne cumule double rôle : branche de feature et pointe de integration/projets
- La bibliothèque IAM est complète et testée mais n'a aucun appelant hors d'elle-même sans --features iam (opt-in expresse, jamais imposée)
- fail-closed : PolicyDecision sans « abstention », pas de politique => ImplicitDeny => refus (inverse du défaut Abstain du framework this, 72 tests)
- docs/IAM-BRANCHE-2026-09-13.md documente encore examples/amorcer_iam.rs alors que ce fichier a disparu du HEAD (remplacé par obrain iam amorcer)
- Deux ADR quasi-identiques ne différant que par la casse : ADR-2026-09-13-chaine-iam-et-explosion-des-liens.md et ADR-2026-09-13-CHAINE-IAM-ET-EXPLOSION-DES-LIENS.md

**Trous / incohérences.**
- examples/amorcer_iam.rs référencé par la doc IAM-BRANCHE mais absent du HEAD (supersédé par la sous-commande obrain iam amorcer)
- ADR dupliquée en deux fichiers ne différant que par la casse (chaine-iam-et-explosion-des-liens vs CHAINE-IAM-ET-EXPLOSION-DES-LIENS)
- feat/iam-mint-endpoint et feat/iam-groupes (baseline tronc b6c6da2c) ne sont pas fusionnés dans main — risque de divergence avec groupes-tronc
- fix/iam-pont-lecture absent de HEAD 456d3794 (seulement integration/t27 et branches IAM)
- Group/MEMBER_OF/HAS_ROLE, argon2id, emettre_service, controler_ressource et audit borné absents du HEAD principal
- gdpr.rs : horodatage gelé (erased_at/exported_at = 2026-01-01) et principal=cible au lieu d'acteur — défaut préexistant non corrigé
- revoke_session dépend de la visibilité de delete_node dans le substrat (écrit une expiration passée + tente la suppression)
- Pas de MFA, pas de renouvellement automatique client, pas de renommage de compte (clé d'ORN), pas d'énumération des sessions d'une identité
- feat/iam-audit-borne est à la fois feature et pointe de integration/projets — son état de merge vers main reste à confirmer



### 4.4 integration/fusion/consolidation

_Fichier : `raw/v4-4-integration-fusion-consolidation.json` — 24 composants · 11 relations · 8 constats · 8 trous_


**Synthèse.** La tranche integration/fusion/consolidation est un DAG à 12 branches réparties sur 3 filesystems (/home/triviere/projects, /mnt/obrain, /tmp). Le socle le plus profond est integration/zero-copy (af0490a2), ancêtre de presque tout — sauf de fusion/tronc-plus-j6 et fusion/tronc-plus-j6-mes-correctifs, qui vivent sur une lignée distincte privée de ce socle. La branche de tête réelle est integration/projets (574d03f7, worktree wt-projets-moteur) : elle est 101 commits devant le HEAD du dépôt principal, et la chaîne t27 ⊂ n3-tronc ⊂ projets (147/139 commits d'écart) montre une intégration progressive du travail IAM/selfd/flotte. Le HEAD du dépôt principal, consolidation/tronc-2026-09-15-plus-fusion (456d3794), est une fusion « tissage » qui absorbe consolidation/2026-09-13, consolidation/tronc-complet-2026-09-13, fusion/j4-consolide, fusion/j4-final, fusion/tronc-plus-j6 et ses correctifs, mais il est lui-même DÉPASSÉ par les branches integration (t27, n3-tronc, projets). consolidation/tronc-release-2026-09-15 (89ba8841) diverge du HEAD au commit « sonde d'auto-connaissance de la flotte » (20fc3d38) et n'a jamais été fusionné : ses correctifs « WAL O_APPEND » et « course entre tests » sont orphelins. Les chantiers j4 et tronc-complet sont des jumeaux divergents : fusion/j4-final (11376f67) et consolidation/tronc-complet-2026-09-13 (e0fc7fd5) rejouent tous deux « index de trigrammes » et « mesure de phases » avec des SHAs et horodatages différents (~2h45 d'écart).


**Composants clés.**
- crates/obrain-iam/src/garde.rs (+1132 lignes sur integration/projets) — garde de classification lecture/écriture, cœur de l'autorité IAM
- crates/obrain-iam/src/admin.rs, store.rs, policy.rs, provider.rs, audit_chaine.rs, model.rs, gdpr.rs, orn.rs — administration, politiques effectives, audit chaîné
- crates/obrain-substrate/src/props_zone.rs (+263), store.rs (+547), wal_io.rs, replay.rs, file.rs, compteurs.rs, writer.rs — reprojection de zone props, WAL, ingestion en lot
- crates/obrain-substrate/tests/reprojection_zone.rs, ingestion_props_en_lot.rs, scan_plafonne.rs, tenant_colonne.rs, trous_allocated.rs, operators_filter.rs
- crates/obrain-selfd/src/soi.rs (+382), politique.rs (+306), attache.rs, mcp.rs, outils.rs (+186), protocole.rs, ecoute.rs, main.rs — le « self » / flotte auto-orchestrée
- crates/obrain-selfd/tests/annonce.rs (+811), capacites.rs (+651) — annonce de capacités et self par compte
- crates/obrain-indexer/src/lib.rs, metrics.rs, parse.rs, resolve.rs, spec_engine.rs (+ tests cache_extraction.rs, empreinte_depot.rs)
- crates/obrain-viz/src/overlay.rs (+51), lib.rs — overlays de rendu
- crates/obrain-cli/src/viz_serve.rs (+223) — serveur de visualisation CLI
- packages/obrain-dashboard/src/graph/flotte.ts (1233 lignes) + flotte.test.mjs (664) — logique de la flotte (branches integration/*, absent du HEAD)
- packages/obrain-dashboard/src/graph/PanneauFlotte.tsx (907), PanneauGens.tsx (351), PanneauMonde.tsx (58), ChatPleinEcran.tsx, CosmosWeave.tsx
- packages/obrain-dashboard/src/lib/resonance.ts (90) + app/App.tsx, app/store.ts, styles/_layout.scss (+381)
- deploy/systemd/selfd.service (+130), selfd.attacher.service (+81), obrain-machines.timer (+38), obrain-viz.service.d-cache-persistant.conf (+82)
- scripts/acceptation-flotte.sh (+815), acceptation-harness.sh (+606), acceptation-self-par-compte.sh (+649), acceptation-portabilite.sh (+299), acceptation-8432.sh
- scripts/selfd-empaqueter.sh (+473), selfd-installer.sh (+532), machines-appliquer.sh (+316), machines-appliquer-comptes.sh (+120), memoire-appliquer-comptes.sh (+403)
- scripts/porte-atelier.sh (+530), verifier-integration-tissage.sh (+290), version-check.sh (+190), banc-interface-authentifiee.sh (+119)
- scripts/ui-chat-porte.mjs, ui-gens-porte.mjs (+209), ui-monde-porte.mjs (+186), ui-mobile-porte.mjs, ui-capacites.mjs (+110), ui-lire-ecran.mjs (+140), ui-mobile-capture.mjs (+169), ui-bascule.sh, ui-deploy.sh, ui-contrat.json, ui-verifier.sh
- scripts/projection-check.mjs, protocole-check.mjs, world-add-check.mjs, endpoints-check.mjs, fratrie-check.mjs, echelle-multi-niveaux-check.mjs, views-controls-check.mjs, flotte-auto-connaissance.sh
- docs/ACCEPTATION-FLOTTE-2026-09-16.md (656), DEVELOPPEMENT-SELF-PAR-COMPTE-2026-09-16.md (849), ETAT-BINAIRE-SELFD-2026-09-16.md (1070) — absents du HEAD, présents sur integration/*
- docs/FLOTTE-EN-SERVICE-2026-09-16.md (291), UI-FLOTTE-8432-2026-09-16.md (627), PROTOCOLE-VERSION.md (82)
- docs/rfc/substrate/format-spec.md (+15) — spec de format modifiée par le chantier trigrammes
- spike/node-distant/drive.mjs (185), cordis.yml (119), measure.mjs (81), mount-harness-client.mjs (69) — nœud distant / micro-harness
- docs de consolidation/2026-09-13 (commits docs) : CAUSE RACINE (deux lignées), surface de conflit réelle, verdict de vérification, consolidation des 16 chantiers
- docs du HEAD (TISSAGE) : LE TISSAGE EST VÉRIFIÉ, TISSAGE SECOND VOLET, savoir du multi-échelle devenu document de reprise

**Relations / dépendances.**
- integration/zero-copy (af0490a2) est ancêtre de t27, 2026-09-13, tronc-complet, tronc-release, j4-consolide, j4-final et du HEAD — socle quasi-universel
- integration/t27 (9fed71eb) ⊂ integration/n3-tronc-20260920 (d97f5ecb) ⊂ integration/projets (574d03f7) — chaîne linéaire, t27..projets = 147 commits
- consolidation/tronc-2026-09-15-plus-fusion (HEAD 456d3794) ⊂ integration/n3-tronc (ahead 93) et ⊂ integration/projets (ahead 101) — le dépôt principal est en retard
- integration/n3-tronc-20260920 est la fusion de t27 et du HEAD (merge-base t27/HEAD = 20fc3d38) + le travail n3 « chaîne armable / jeton porte son client / audit chaîné »
- fusion/j4-consolide (4ff85c8d) ⊂ fusion/j4-final (11376f67), et tous deux ⊂ HEAD — le chantier « principal j4 » rejoint le tronc
- consolidation/tronc-complet-2026-09-13 (e0fc7fd5) et fusion/j4-consolide partagent le merge-base 5032e107 (« Merge feat/tronc-complet-2026-09-13 ») puis divergent
- consolidation/tronc-complet-2026-09-13 ⊂ integration/t27 et ⊂ consolidation/tronc-release-2026-09-15 — socle de la lignée serveur
- consolidation/tronc-release-2026-09-15 (89ba8841) et le HEAD divergent à 20fc3d38 ; tronc-release n'est ancêtre de RIEN d'autre (aucun descendant fusionné)
- fusion/tronc-plus-j6 (b1fb10e2) ⊂ HEAD, mais ne contient PAS zero-copy — lignée hors socle
- fusion/tronc-plus-j6-mes-correctifs (4da722ae) a divergé de tronc-plus-j6 : 21 commits en avant, 17 en arrière (branche « mes correctifs » non rebasée)
- worktrees : /home/triviere/projects/banc-obrain→HEAD plus-fusion, banc-obrain-consolidation→2026-09-13, banc-obrain-consolide→tronc-complet, banc-obrain-integ→zero-copy, wt-projets-moteur→projets, /mnt/obrain/wt-integration→t27, /mnt/obrain/wt-integration-n3→n3-tronc, /tmp/fusion-j4→j4-consolide, /tmp/tpc-j6→mes-correctifs

**Points notables.**
- Le HEAD du dépôt principal (456d3794, consolidation/tronc-2026-09-15-plus-fusion) est 101 commits DERRIÈRE integration/projets : le worktree de référence n'est pas la pointe du travail
- Deux paires de commits jumeaux à SHAs/horodatages différents : « index de trigrammes » = 4f97c7c1 (j4-final, 16:24) vs 496e64bb (tronc-complet, 13:41) ; « mesure de phases » = 11376f67 (16:24) vs e0fc7fd5 (13:50) — même messages, mêmes fichiers, rejoués ~2h45 plus tard
- fusion/tronc-plus-j6 ne descend pas de integration/zero-copy : c'est la seule lignée (avec mes-correctifs) privée du socle zero-copy, donc potentiellement privée de ses correctifs substrate
- consolidation/tronc-release-2026-09-15 est une impasse : ses correctifs « LE WAL S'OUVRAIT SANS O_APPEND » et « LA SUITE DIT ENFIN LA VÉRITÉ (course entre tests) » ne sont ancêtres d'aucune branche vivante
- fusion/tronc-plus-j6-mes-correctifs n'est PAS une branche posée sur tronc-plus-j6 : les deux ont divergé (21 vs 17 commits uniques), malgré le nom « mes correctifs »
- Le travail « flotte » (flotte.ts 1233 lignes, PanneauFlotte.tsx 907, resonance.ts) n'existe QUE sur integration/projets et n3-tronc : les fichiers sont absents du checkout HEAD
- integration/t27 est un pont de lecture IAM qui vérifie les identités « sans écrire dans iam.obrain » — abstraction d'écriture en lecture seule, thème central de l'audit multi-axe
- Le worktree banc-obrain-release est sur feat/data-load-bulk, pas sur consolidation/tronc-release-2026-09-15 : nom de répertoire trompeur

**Trous / incohérences.**
- Aucun worktree pour fusion/j4-final (11376f67), fusion/tronc-plus-j6 (b1fb10e2) ni consolidation/tronc-release-2026-09-15 (89ba8841) — trois branches sans checkout matériel
- Le correctif WAL O_APPEND de tronc-release semble perdu : personne ne le porte vers HEAD ni vers integration/* (à confirmer par diff de wal_io.rs)
- Quelle version de « index de trigrammes »/« mesure de phases » fait foi — j4-final (16:24) ou tronc-complet (13:41) ? Duplication non résolue par fusion
- fusion/tronc-plus-j6 hors socle zero-copy : quels correctifs substrate lui manquent-ils réellement ? Aucun merge-base avec zero-copy
- La raison de la divergence de fusion/tronc-plus-j6-mes-correctifs (21/17) n'est documentée dans aucun message de commit consulté
- Le HEAD est en retard de 93-101 commits : qui est censé re-fusionner integration/projets vers le tronc, et pourquoi le travail « tissage » du HEAD n'est-il pas dans projets ?
- Les 3 docs flotte (ACCEPTATION/DEVELOPPEMENT/ETAT-BINAIRE-SELFD 2026-09-16) n'ont pas de répertoire docs/ dédié à la consolidation — leur date (09-16) précède la lignée n3 (09-20)
- spike/node-distant (cordis.yml, drive.mjs) est présent sur integration/* et tronc-release mais supprimé côté tronc-plus-j6/HEAD : statut réel de ce spike (mort ou à rapatrier) ?



### 4.5 branches-diverses (docs principal-natif, fix MCP/engrams/selfd, feat indexeur/zorder/data-load, harness, reparation, union, /tmp/banc-head)

_Fichier : `raw/v4-5-branches-diverses.json` — 28 composants · 10 relations · 8 constats · 7 trous_


**Synthèse.** Cette tranche cartographie 20 branches résiduelles réparties sur le dépôt principal banc-obrain (HEAD consolidation/tronc-2026-09-15-plus-fusion @ 456d3794), un clone de déploiement séparé banc-obrain-release (origin = banc-obrain), et des worktrees isolés. Elles se groupent en neuf thèmes : (1) la conception 'principal natif' (RFC v1/v2, docs/rfc/principal-natif*.md, dérivées de afbb5cfc, qui documentent AccessContext dans obrain-core/src/execution/base_registry.rs:246) ; (2) le chantier substrate 'slots 512' de PropsZone (feat/slots512-wiring, feat/zorder, feat/release-m6-zorder, crates/obrain-substrate/src/props_zone.rs + unite.rs, porte OBRAIN_PROPS_SLOTS_512) ; (3) le bridge MCP/selfd (écrivain unique, exec arrière-plan, arrêt d'exec, auto-install Claude, next-step hints) ; (4) l'indexeur (commits git, contenu incrémental, index ordonné par valeur j6) ; (5) data-load bulk + dashboard canvas (group-commit WAL, couches Monde/Graphe) ; (6) planner/journal ch8b-engrams (pushdown SourceCap, WAL merkle POC) ; (7) réparation d'arêtes (repair-edge-chains, étiquettes Self) ; (8) harness/portabilité (porte-atelier, selfd-empaqueter) ; (9) dashboard nav/rendu (union). Plusieurs branches sans worktree n'existent que dans l'object store git (inspectables via git show), et deux anomalies de nommage ont été relevées (feat/llama-obrain-index-persist sans code llama, feat/mcp-next-step-hints aux pointes divergentes).


**Composants clés.**
- docs/rfc/principal-natif.md — worktree banc-obrain-rfc, branche docs/principal-natif (57353aac)
- docs/rfc/principal-natif-v2.md — worktree banc-obrain-rfc2, branche docs/principal-natif-v2 (6b3a4d50)
- crates/obrain-core/src/execution/base_registry.rs (AccessContext, :246) — cité par les RFC principal-natif
- crates/obrain-engine/src/session/mod.rs (access_context Mutex<Option<Arc<AccessContext>>>, :124)
- crates/obrain-substrate/src/props_zone.rs — feat/slots512-wiring (porte OBRAIN_PROPS_SLOTS_512, ecrire_debut_chainee_pack)
- crates/obrain-substrate/src/unite.rs — feat/slots512-wiring (codec UnitéAddr + SlotPage + CarrierControl)
- crates/obrain-substrate/examples/cogcheck_slots512.rs — feat/zorder (contrôle cognitif sous migration slots)
- crates/obrain-substrate/src/props_ord_index.rs — banc-obrain-j6, feat/index-ordonne-j6 (index ordonné par valeur)
- crates/obrain-selfd/src/mcp.rs — fix/ecrivain-unique-bridge (écrivain unique) + feat/selfd-arret (cancel exec)
- crates/obrain-selfd/src/outils.rs — fix/selfd-exec-arriere-plan (exec non bloquant pipes) + feat/selfd-arret
- crates/obrain-selfd/src/protocole.rs — feat/selfd-arret (notifications/cancelled)
- crates/obrain-cli/src/commands/mcp_install.rs — /mnt/obrain/wt-mac-darwin, feat/mcp-install-claude
- crates/obrain-cli/src/mcp_guide.md — feat/mcp-install-claude (guide embarqué)
- crates/obrain-cli/src/mcp_server.rs — feat/mcp-install-claude + feat/indexeur-commits (outil index_commits)
- crates/obrain-indexer/src/commits.rs — wt-indexeur-commits (Commit, PARENT, TOUCHES, AT_COMMIT)
- crates/obrain-indexer/src/schema.rs + index.rs + lib.rs — wt-indexeur-commits
- crates/obrain-indexer/tests/commits_git.rs — wt-indexeur-commits
- crates/obrain-cli/src/commands/repair.rs — banc-obrain-r1 (repair-edge-chains, :451-523)
- crates/obrain-cli/src/main.rs — banc-obrain-r1 (enum RepairEdgeChains :181, :565)
- crates/obrain-cli/examples/reparer_etiquettes.rs — worktrees/obrain-self (reparation/self-etiquettes-20260916)
- crates/obrain-engine/src/query/planner/lpg/mod.rs — fix/ch8b-engrams (pushdown SourceCap)
- crates/obrain-engine/src/query/optimizer/cardinality.rs — fix/ch8b-engrams (corrélation inter-MATCH)
- crates/obrain-engine/src/query/binder.rs + processor.rs — fix/ch8b-engrams
- packages/obrain-dashboard/src/components/StoreOverview.tsx — feat/llama-obrain-index-persist (vue Monde vide)
- packages/obrain-dashboard/.gitignore — /tmp/union-nav, union/nav-et-rendu (dist symlink)
- scripts/porte-atelier.sh — banc-obrain-tronc, harness/continuite-dans-contrat-2026-09-15
- scripts/selfd-empaqueter.sh — banc-obrain-tronc, harness/continuite-dans-contrat-2026-09-15
- scripts/verifier-integration-tissage.sh — worktrees/obrain-self (reparation/self-etiquettes-20260916)

**Relations / dépendances.**
- docs/principal-natif-v2 REMPLACE docs/principal-natif (v1 57353aac) — les deux dérivent du même parent afbb5cfc (8eb2aec1 → 9ca6f852 → afbb5cfc)
- feat/zorder contient l'historique complet de feat/slots512-wiring (re-applique la porte OBRAIN_PROPS_SLOTS_512 via 8a016604) — zorder = câblage env complet par-dessus slots512-wiring
- feat/release-m6-zorder = release candidate m6 : merge de feat/m6-code-graph (50f10d9a) + search_by_role + reverse-adjacence PART_OF + sim_conversations
- feat/indexeur-commits (f02e5b1d) s'empile sur feat/indexeur-contenu (6f84b3e1 = parent direct) — +2 commits (histoire git, outil index_commits)
- feat/selfd-arret (66430ded) s'empile sur fix/selfd-exec-arriere-plan (6e007731 = parent direct)
- feat/data-load-bulk (d71aa5dc) est la continuation de feat/llama-obrain-index-persist (126c766d) — +4 commits (expansion IN, résolution centres, group-commit WAL, fix Number)
- banc-obrain-release (clone séparé) a pour remote origin = /home/triviere/projects/banc-obrain — ses origin/* mirror les branches du dépôt principal
- fix/ecrivain-unique-bridge intègre integration/t27 (merge 9fed71eb, 063b8c26, 014983ca) — T27 = indexeur empreinte/foldhash/import lot
- /tmp/banc-head (8eb2aec1) est l'ancêtre commun des branches docs/principal-natif(-v2) et de la lignée feat/mcp-next-step-hints
- d47ca592 (commit 'next-step hints') est atteignable depuis le dépôt principal et contenu dans consolidation/tronc, docs/principal-natif(-v2), feat/iam-*

**Points notables.**
- feat/llama-obrain-index-persist porte un nom trompeur : aucun code llama/index-persist — tout le contenu est le dashboard canvas (Monde/Graphe, FacetBar, CanvasBoundary, moteur de densité)
- feat/mcp-next-step-hints a DES pointes divergentes : dépôt principal (1fa57572, lignée cognitive 'STARTS WITH via index'/trois natures) vs clone banc-obrain-release (d47ca592, le vrai commit next-step-hints basé sur data-load-bulk)
- banc-obrain-release est un CLONE indépendant (pas un worktree) — son origin/HEAD pointe vers feat/mcp-next-step-hints (d47ca592), pas vers la branche locale courante feat/data-load-bulk
- La famille zorder compte 4 arbres : feat/zorder, feat/slots512-wiring, feat/release-m6-zorder + le worktree détaché banc-obrain-v0001-zorder (a80e3b9b)
- Les RFC principal-natif sont remarquablement tracés : chaque affirmation est citée fichier:ligne via grep/sed (ex. AccessContext base_registry.rs:246, access_context session/mod.rs:124)
- reparation/self-etiquettes-20260916 : l'étiquette Self SE DÉPLACE (pas de suppression) — prouvé par 42 907 arêtes, corrige un WAL ouvert sans O_APPEND (2 corruptions/2 jours)
- /tmp/banc-head est un instantané détaché décrit 'checkpoint-montee-20260907-233-g8eb2aec1' (233 commits après checkpoint-montee-20260907), arbre complet mais non branché
- feat/data-load-bulk réalise x3-4 sur 50k+50k via group-commit WAL et corrige la perte silencieuse des propriétés {Number:...} en Null

**Trous / incohérences.**
- Branches sans worktree (fix/ch8b-engrams, fix/ecrivain-unique-bridge, feat/slots512-wiring, feat/zorder, feat/release-m6-zorder, feat/llama-obrain-index-persist) : fichiers inspectables uniquement via git show <branche>:<chemin>, non extraits sur disque
- Nom de branche demandé 'harness/continuite-dans-contrat' : la branche réelle est harness/continuite-dans-contrat-2026-09-15 (suffixe daté) — même chose pour union/nav-et-rendu-20260915
- Divergence de pointe feat/mcp-next-step-hints (principal 1fa57572 vs clone d47ca592) — non réconciliée, risque de confusion pour un futur merge
- feat/llama-obrain-index-persist : fonctionnalité 'llama index persist' introuvable — nom orphelin, contenu = dashboard canvas
- Les origin/* du clone banc-obrain-release peuvent être STALE par rapport au dépôt principal (ex. origin/feat/mcp-next-step-hints = d47ca592 vs pointe locale principale 1fa57572)
- banc-obrain-v0001-zorder (detached a80e3b9b) hors périmètre mais apparenté au thème zorder — non cartographié ici, doublon potentiel
- Aucun test de bout en bout vérifié par exécution (lecture seule) — les suites tests citées (commits_git.rs, cogcheck) sont signalées mais non exécutées




## Vague 5 — V5 Scripts

### 5.1 scripts/acceptation-*.sh + scripts/verif-*.sh + check-all.sh + smoke-backend.sh + echecs-connus.txt (portes d'acceptation et registre des échecs connus)

_Fichier : `raw/v5-1-scripts-acceptation.json` — 27 composants · 12 relations · 9 constats · 8 trous_


**Synthèse.** La tranche couvre 16 fichiers : onze portes d'acceptation (acceptation-*.sh), deux vérifications (verif-*.sh), un orchestreur (acceptation-toutes.sh qui enchaîne 10 portes), un smoke backend (smoke-backend.sh), un check dashboard-graph (check-all.sh) et le registre des échecs connus (echecs-connus.txt). Chaque porte est un point d'entrée autonome : elle crée ses stores dans /tmp, prend un port libre (refus explicite de 8432/8433, les services utilisateur), lance son propre `viz serve` et nettoie à la sortie. Le principe récurrent est la falsifiabilité par des outils indépendants du code testé (grep, dd, hexdump, openssl s_client, curl, strings) plutôt que par des assertions Rust écrites par la même main. Les portes couvrent la sécurité de bout en bout : transport chiffré (TLS), confidentialité du calcul (coffre), journal de calcul chaîné/signé (journal), administration IAM (iam), selfs par compte (self-par-compte, chat-self), catalogue des selfs, résonance entre selfs, et ajout de monde GitHub enrichi par un compte non-admin. acceptation-8432.sh est la plus large : builds (cargo --features ai + tsc/vite), instance isolée 8433, protocole HTTP, endpoints, projection multi-dimension et fratrie via Chrome CDP. Le registre echecs-connus.txt ne contient qu'une entrée (un test substrat `des_ancres_divergentes_ne_rendent_jamais_un_resultat_ampute`), consommé par bench-regression.sh pour qu'un banc durablement rouge ne masque pas un nouvel échec. Une constante documentée trois fois est la classe « IAM NON COMPILÉ / réglage inerte » : un binaire construit sans la feature ou un env armé mais jamais lu par le binaire déployé (verif-env-deployees.sh le détecte via `strings`).


**Composants clés.**
- scripts/acceptation-toutes.sh — orchestreur qui lance 10 portes en séquence, journaux dans /tmp/portes-*, verdict par porte
- scripts/acceptation-8432.sh — porte du dashboard-graph : builds cargo/tsc/vite, instance isolée 8433, protocole, endpoints, projection, fratrie (Chrome CDP)
- scripts/acceptation-iam.sh — administration IAM : CLI hors bande + serveur armé, plafond d'espace, mutations POST-only, verrouillage du dernier admin
- scripts/acceptation-chat-self.sh — le chat annonce le self de l'APPELANT dans le flux SSE (self_qn), deux comptes → deux selfs/journaux
- scripts/acceptation-coffre.sh — confidentialité du calcul : scellé/desceller, grep/hexdump, octet modifié détecté, clé 0600, forbid(unsafe_code)
- scripts/acceptation-journal.sh — journal de calcul chaîné/signé : empreintes seules, dd altère, troncature démasquée par la tête client
- scripts/acceptation-monde-utilisateur.sh — un compte non-admin ajoute un dépôt GitHub, parcours clone→indexation→enrichissement→courbure
- scripts/acceptation-resonance.sh — interconnexion de selfs : --lier/--delier, aucun partage par défaut, le secret privé ne traverse jamais HTTP
- scripts/acceptation-self-par-compte.sh — deux comptes → deux selfs distincts, deux journaux distincts, anonyme → 401
- scripts/acceptation-selfs-catalogue.sh — catalogue /api/engine/selfs : qui existe, lecture mémoire, attribution du monde, sans écriture
- scripts/acceptation-tls.sh — étape 0 transport chiffré : TLS 1.3, capture relais, témoin --clair, grep ne trouve ni jeton ni secret
- scripts/verif-chaine-iam.sh — chaîne d'authentification : login→cookie→whoami→refresh/rotation→audit→logout→refus
- scripts/verif-env-deployees.sh — confronte les OBRAIN_* armés (/proc/<pid>/environ ou systemctl cat) aux littéraux lus par le binaire (strings)
- scripts/check-all.sh — vérification complète dashboard-graph : smoke backend + e2e-dashboard.mjs + statut serveur
- scripts/smoke-backend.sh — disponibilité + chaud < 50 ms + cohérence GQL sur runtime-dev et linux-full3
- scripts/echecs-connus.txt — registre des échecs connus (format nom | date | attribution), 1 entrée
- scripts/bench-regression.sh — consommateur du registre : un échec connu ne rend pas le banc rouge
- scripts/protocole-check.mjs — traversée de chemin, compression, keep-alive, HEAD, Date, ETag/304, SSE (appelé par acceptation-8432.sh)
- scripts/endpoints-check.mjs — présence/code/latence chaud et froid des endpoints (appelé par acceptation-8432.sh)
- scripts/projection-check.mjs — chaîne multi-dimension requête→axes→sélection→graphe (appelé par acceptation-8432.sh)
- scripts/fratrie-check.mjs — frères et sœurs à la sélection, se connecte à un CDP déjà ouvert (appelé par acceptation-8432.sh)
- scripts/e2e-dashboard.mjs — parcours E2E interface (appelé par check-all.sh)
- scripts/vizserve-isole.sh — lance une instance isolée sur port dédié sans tuer l'instance existante
- crates/obrain-coffre — noyau de scellement (examples/sceller.rs, tests/porte_confidentialite.rs, tests/attaques_externes.rs)
- crates/obrain-journal — journal de calcul chaîné/signé (examples/journal.rs)
- crates/obrain-tls — noyau TLS (examples/serveur_tls.rs, tests/donnees/{ca.pem,cert-de-test.pem,cle-de-test.pem})
- crates/obrain-iam — identités et politiques (example amorcer_iam, tests/la_cle_prime_sur_le_droit.rs)

**Relations / dépendances.**
- acceptation-toutes.sh → lance 10 portes (coffre, journal, tls, iam, selfs-catalogue, self-par-compte, chat-self, resonance, monde-utilisateur, 8432) via OBRAIN_BIN=$REPO/target/release/obrain et timeout DELAI=1800
- acceptation-toutes.sh → lit ./target/release/obrain (cargo build --release -p obrain-cli --features ai,iam requis)
- acceptation-8432.sh → node scripts/protocole-check.mjs, endpoints-check.mjs, projection-check.mjs, fratrie-check.mjs ; scripts/vizserve-isole.sh --port 8433 ; lance chrome headless CDP sur 9248
- check-all.sh → bash scripts/vizserve.sh, bash scripts/smoke-backend.sh, node scripts/e2e-dashboard.mjs
- acceptation-coffre.sh → cargo build -p obrain-coffre --example sceller ; cargo test -p obrain-coffre --test porte_confidentialite ; exécute target/debug/examples/sceller
- acceptation-journal.sh → cargo build -p obrain-journal --example journal ; exécute target/debug/examples/journal (ecrire/verifier/tete)
- acceptation-tls.sh → cargo build -p obrain-tls --example serveur_tls ; exécute target/debug/examples/serveur_tls (servir/relais) ; lit crates/obrain-tls/tests/donnees/*
- acceptation-iam.sh / verif-chaine-iam.sh / monde-utilisateur / chat-self / self-par-compte / resonance / selfs-catalogue → `$BIN viz serve` avec OBRAIN_IAM=1 OBRAIN_IAM_STORE=... OBRAIN_IAM_ACCOUNT=obrain + `iam amorcer`
- acceptation-iam.sh → sonde la sous-commande `iam utilisateurs` (et non `nm -C`) car nm produit des faux positifs (mesuré 2026-09-14)
- verif-chaine-iam.sh → exemple amorcer_iam (ou cargo run --example amorcer_iam), policies Allow graph:read
- bench-regression.sh → lit scripts/echecs-connus.txt (coupe sur '|', field 1 = nom de test, grep du nom dans le log)
- echecs-connus.txt → référence le test crates/obrain-substrate/src/store.rs:31682 des_ancres_divergentes_ne_rendent_jamais_un_resultat_ampute (documents docs/PERF-REQUETES-LENTES-2026-09-13.md)

**Points notables.**
- Chaque porte documente explicitement « ce qu'elle NE prouve PAS » (coffre: lecture du clair pendant le calcul ; journal: complétude et honnêteté du calcul ; tls: pas de tcpdump réel faute de droits) — honnêteté de périmètre systématique
- Principe de falsifiabilité : les vérifications sont faites par des outils externes au code (grep, dd, hexdump, openssl s_client, curl, strings) pour ne pas qu'une assertion Rust se juge elle-même
- Garde de port : toutes les portes IAM refusent 8432/8433 (services utilisateur) et prennent un port libre dédié, stores et cookies dans /tmp nettoyés en sortie
- acceptation-8432.sh démarre lui-même Chrome CDP (port 9248) car fratrie-check.mjs ne lance aucun navigateur — correction documentée d'un faux échec ECONNREFUSED qui accusait le produit
- La détection IAM du binaire a été corrigée : `nm -C` passait au vert sur un binaire sans la feature (mesuré 2026-09-14) ; la sonde fiable est la commande réelle `iam utilisateurs` qui répond « NON COMPILÉ »
- Le registre echecs-connus.txt a une entrée unique datée 2026-09-13, avec en tête une règle éditoriale stricte : on n'y inscrit que ce qui est compris et attribué, un échec inexpliqué doit rester ROUGE
- verif-env-deployees.sh documente la classe « réglage armé mais inerte » (OBRAIN_* posé dans systemd mais jamais lu par le binaire déployé), déjà documentée 3× (« IAM NON COMPILÉ »)
- Des variables d'inspection GARDER_JOURNAL=1 / GARDER_MONDE=1 permettent de conserver les artefacts (base enrichie, magasin IAM, journaux) pour rejouer la lecture à la main
- acceptation-resonance.sh vérifie des permissions réelles (cache résonance 0700, fichier de consentement 0600) et qu'un self ne peut pas s'abonner à lui-même

**Trous / incohérences.**
- echecs-connus.txt ne contient qu'UNE entrée alors que le dépôt compte ~24 crates et 10+ portes d'acceptation : registre très mince, sans outil de détection des entrées périmées (un échec corrigé y reste à jamais)
- La colonne « attribution » d'echecs-connus.txt est du texte libre anonyme (« autre session ») alors que l'en-tête exige une attribution — pas de lien vers un compte/propriétaire
- check-all.sh et smoke-backend.sh codent en dur l'adresse http://192.168.1.103:8432 (IP machine) — non paramétrable par variable d'environnement, donc fragile hors de cette machine
- smoke-backend.sh code en dur des stores (/tmp/linux-full3.obrain, runtime-dev.obrain) et masque les échecs curl par `|| true` sur la phase de chauffe — un store absent est silencieusement ignoré
- acceptation-toutes.sh fige la liste des 10 portes dans un tableau PORTES (pas d'auto-découverte de acceptation-*.sh) : ajouter une porte exige d'éditer l'orchestrateur
- Incohérence de sonde IAM entre deux scripts : acceptation-iam.sh rejette `nm -C` (faux positifs documentés) mais verif-chaine-iam.sh (ligne 56) utilise encore `nm -C ... | grep obrain_iam`
- acceptation-8432.sh exige `cargo build --release` + `npx tsc -b` + `npx vite build` à chaque passe (lent) ; seul `--sans-build` contourne — pas de cache incrémental pour un usage CI répété
- La porte TLS ne peut pas capturer le vrai réseau (pas de tcpdump sans droits) et substitue le relais de serveur_tls.rs : la preuve « sur le fil » est un proxy du trafic, pas le chemin réseau réel (limite assumée mais non résolue)



### 5.2 scripts-bench-perf — dispositif de mesure de performance (bancs, gardes, corpus, ligne de base)

_Fichier : `raw/v5-2-scripts-bench-perf.json` — 25 composants · 11 relations · 11 constats · 10 trous_


**Synthèse.** La tranche cartographiée est le dispositif complet de mesure de performance d'obrain : un banc à seuils central (scripts/requetes-lentes.sh) qui rejoue un corpus de 18 requêtes RÉELLES extraites des journaux MCP des 12-13/09/2026, plus une famille de bancs satellites, de gardes DoD et de rapports. La doctrine méthodologique est partout la même : mesurer au minimum sur plusieurs tours (min/médiane, jamais une passe unique), distinguer froid/chaud (cache serveur durable vs calcul réel), et surtout distinguer le temps REQUÊTE du chronomètre moteur du temps MURAL du processus (ouverture + requête). Le cache durable du serveur (clé = store + texte + mtime) est contourné par des nonces/espaces de fin, sinon on relit un souvenir au lieu de mesurer. Le banc rend DEUX verdicts (seuil objectif strict, et régression au-delà de max(seuil, base × TOLERANCE)) car un banc qui clignote au seuil ne détecte rien ; il refuse de mesurer un binaire périmé (code 3) et un store tenu par un autre processus (code 4, avec RO=1 comme sortie de secours). Les gardes (msgpath_guard.py, viz_guard.py) figent les DoD (ouverture ≤30/50 ms, runtime_search <50 ms, déterminisme octet-à-octet, parité export_all==comptage). Les rapports (docs/PERF-*.md, docs/perf/ENDPOINTS.md) documentent des dizaines d'A/B mesurés et l'important registre des optimisations « écrites, mesurées, puis RETIRÉES ». Le dispositif conclut que les 14 requêtes du corpus réel passent sous 5 ms en processus long, mais que 4 formes (V1, F7, F1, F3) restent à leur plancher structurel ou en attente d'une déclaration d'index ordonné qui n'existe pas encore.


**Composants clés.**
- scripts/requetes-lentes.sh — banc à seuils central (corpus réel 18 requêtes, min/médiane + temps mural, double verdict, gardes binaire périmé/store tenu)
- scripts/banc-requetes-lentes.baseline — ligne de base des minima par requête, avec en-tête #store qui nomme son store
- scripts/corpus-requetes.txt — les 18 requêtes du corpus (C1-C14, F1/F3/F7, V1), format « libellé|requête », extraites du banc
- scripts/bench-regression.sh — harnais de non-régression à 8 axes (lib tests, DB open, RAM, PO vs Neo4j, planificateur, corpus réel, chemin chat, endpoints viz serve)
- scripts/bench-serveur.sh — banc par le serveur viz sur le store VIVANT (execution_time_ms, cache déjoué par espaces de fin)
- scripts/bench-filtre-propriete.sh — banc du coût par cellule de propriété (ns/ligne), mode froid vs régime stationnaire via obrain shell
- scripts/bench-message-path.sh — banc du chemin chat (bridge + runtime_search + context_build), rapporte sans sanctionner
- scripts/bench-compare.sh — comparateur critcmp qui poste les régressions en commentaire de PR (CI)
- scripts/bench_boot.rs — banc de démarrage (WAL replay vs mmap epoch files), exécutable cargo -Zscript
- scripts/perf-endpoints.mjs — mesure consolidée endpoints + opérations moteur → écrit docs/perf/endpoints.json et docs/perf/ENDPOINTS.md
- scripts/vitesse-8432.mjs — mesure clé NEUVE vs clé REJOUÉE pour distinguer calcul réel du cache, avec --compare d'artefacts
- scripts/requetes-latence-check.mjs — latences des requêtes d'outils du chat (execution_time_ms, budget 50 ms, médiane via --repetitions)
- scripts/msgpath_guard.py — garde DoD du chemin message (ouverture propre ≤30 ms, après écritures ≤50 ms, runtime_search chaud <50 ms)
- scripts/guard-msgpath.sh — wrapper shell du point d'entrée msgpath_guard.py
- scripts/viz_guard.py — garde DoD de la couche viz (déterminisme, parité export_all==comptage, bornes, bench p50/p95)
- scripts/guard-viz.sh — wrapper shell du point d'entrée viz_guard.py
- scripts/smoke-backend.sh — smoke des endpoints viz serve (chaud < 50 ms), appelé tel quel par l'axe 7
- scripts/echecs-connus.txt — registre des échecs connus (nommés, datés, attribués) qui ne rendent pas le banc rouge
- scripts/equiv-resultats.sh — vérification d'équivalence des résultats (comptes complets balayage ⇄ index)
- docs/perf/ENDPOINTS.md + docs/perf/endpoints.json — artefact consolidé des coûts endpoints/moteur (26 s → µs, six ordres de grandeur)
- docs/PERF-LISTES-2026-09-07.md — diagnostic latence des endpoints « list » (sessions_list, session_load, mémo-cache)
- docs/PERF-TRIGRAMMES-2026-09-13.md — l'index de trigrammes CONTAINS (sélectivité exacte, seuil de rareté 4096, doctrine des deux silences)
- docs/PERF-REQUETES-LENTES-2026-09-13.md — rapport principal (2268 lignes) : 11 leviers posés, A/B, pièges d'instrument, registre des retraits
- crates/obrain-engine/examples/bench_texte_gql.rs — banc de la REQUÊTE en processus long (avec/sans index, OBRAIN_BANC_VALEURS=1)
- crates/obrain-substrate/examples/bench_trigrammes.rs — banc du PRÉDICAT (index vs balayage, ligne par ligne)

**Relations / dépendances.**
- scripts/requetes-lentes.sh lit scripts/banc-requetes-lentes.baseline (verdict de régression) et exporte scripts/corpus-requetes.txt (corpus identique par construction)
- scripts/bench-regression.sh → appelle scripts/requetes-lentes.sh (axe 5), enveloppe scripts/bench-message-path.sh d'un seuil (axe 6), appelle scripts/smoke-backend.sh tel quel (axe 7), et lance target/release/examples/bench_texte_gql (axe 8 store vivant)
- scripts/bench-regression.sh → lit scripts/echecs-connus.txt pour ne pas rendre rouge un échec déjà connu
- scripts/requetes-latence-check.mjs importe scripts/session-iam.mjs (session armée) et interroge /api/query du serveur 8432
- scripts/vitesse-8432.mjs interroge /api/query, /api/viz/subgraph, /api/viz/weave, /api/viz/modules, /api/viz/space, /api/viz/arcs, /api/functions du serveur 8432
- scripts/perf-endpoints.mjs écrit docs/perf/endpoints.json et docs/perf/ENDPOINTS.md ; partage le nonce d'espace de fin avec vitesse-8432.mjs et bench-serveur.sh
- scripts/guard-msgpath.sh → exec python3 scripts/msgpath_guard.py ; scripts/guard-viz.sh → exec python3 scripts/viz_guard.py
- scripts/msgpath_guard.py pilote le binaire obrain bridge (spawn-court JSON-RPC) et refuse les stores vivants (~/.local/share/obrain-harness/data)
- scripts/viz_guard.py pilote obrain viz subgraph/export/serve et vérifie la parité avec obrain query count
- docs/PERF-*.md référencent crates/obrain-substrate/src/props_trigram_index.rs, props_index.rs, crates/obrain-engine/src/query/planner/lpg/filter.rs, crates/obrain-core/src/graph/traits.rs
- scripts/bench-filtre-propriete.sh et bench-serveur.sh partagent le même corpus MCP 2026-09-12 et la même métrique ns/ligne ou execution_time_ms

**Points notables.**
- Le cache durable du serveur (clé = store + texte + mtime) fausse toute mesure : contournement systématique par espaces de fin ou nonces aléatoires, sinon on relit un souvenir (execution_time_ms d'une exécution antérieure)
- Double verdict (seuil objectif strict vs régression base × TOLERANCE 1,5) : C9 clignotait 47,8/67 ms selon la charge — confondre objectif et régression rendrait la porte inutilisable en pré-commit
- La porte affiche désormais DEUX colonnes (temps requête du moteur ET temps mural du processus) car l'ouverture du store vivant a coûté jusqu'à 1 400 ms, invisible au chronomètre moteur
- Garde anti-instrument : refus de mesurer un binaire périmé (code 3, deux fois le chantier a conclu sur du vide) et un store tenu (code 4, RO=1 comme verrou partagé)
- La clé d'index ordonné est tronquée à 11 octets (LARGEUR_CLE/encoder_cle_ordonnee) : tous les qualified_name partagent 'home.trivie', donc l'index rend tous les candidats et retombe en balayage
- Les quatre formes hors-seuil ont un statut précis : V1 (43 correspondances vivent tard dans l'ordre des créneaux) et F7 (négation, logique ternaire) sont des PLANCHERS PROUVÉS ; F1/F3 attendent une déclaration 'ordered' qui n'existe pas dans le catalogue
- La vérification des VALEURS (OBRAIN_BANC_VALEURS=1) a attrapé une « réponse rapide et fausse » : l'index de trigrammes en mémoire rendait 0 au lieu de 30 825 sur le store vivant (faux négatif intermittent sous écrivain concurrent)
- Registre méthodique des « écrit, mesuré, puis RETIRÉ » : négation par différence (5,9× mais faux positif), vérification paresseuse, _mm_prefetch, tampon réutilisé, dédoublonnage, garde adaptative de la série ordonnée (empirait F3 229→569 ms)
- Le rafraîchissement des index du store vivant a été RETIRÉ de production : gain mesuré (14→5 lentes) mais justesse non garantie — « une réponse rapide et fausse est le pire résultat possible »
- Plancher mesuré : aucune opération en nanosecondes, le plancher est ~10 µs (count) et le plafond 26 s (subgraph hops=2 froid) — six ordres de grandeur d'écart
- 65 535 engrams rechargeables = plafond fonctionnel (allocateur u16 épuisé par les imports neo4j), pas un ralentissement : 63,4 ms par ouverture, coût qui croît avec la zone

**Trous / incohérences.**
- bench/GUARD-SPEC.md référencé par scripts/guard-msgpath.sh (ligne 2) mais ABSENT du dépôt — référence morte
- bench/bench_msg_path.py référencé par scripts/guard-msgpath.sh (ligne 10) mais ABSENT (aucun fichier bench_msg_path.py trouvé)
- obrain index list rend [] alors que la déclaration est active et persistée (défaut d'outillage signalé dans PERF-REQUETES-LENTES §C12) : un utilisateur croit n'avoir aucun index
- L'index de trigrammes EN MÉMOIRE rend encore un faux négatif sur le store vivant quand tous les trigrammes sont communs (cause exacte non localisée, hypothèse non testée laissée pour le tour suivant)
- F1/F3 (plages ordonnées) : aucune ligne 'ordered' dans le catalogue d'index — c'est une FONCTIONNALITÉ manquante, pas un interrupteur, donc non résolvable par un geste d'exploitation
- La construction en mémoire des trigrammes est OPT-IN par défaut (u32::MAX) — choix de JUSTESSE documenté, mais un levier de perf désactivé sur les processus jetables
- scripts/bench_boot.rs est un .rs exécutable cargo -Zscript sous scripts/ alors que les autres bancs Rust vivent dans crates/*/examples/ — incohérence de localisation
- Le relevé « processus long, store vivant » des tours 44/46/51 a été pris avec un index en mémoire suspect : les TEMPS restent valables mais les VALEURS ne sont pas toutes garanties (réserve assumée)
- Le froid du viz (space/weave) est attribué au handler (agrégation sur les membres, N mondes × ouverture) mais non corrigé — hors périmètre du chantier perf
- docs/perf/chat-ancrage.png (335 Ko) est un artefact binaire orphelin dans le répertoire des mesures consolidées, sans source de génération documentée



### 5.3 scripts/deploy-*.sh + ui-*.mjs + viz (déploiement, UI, dashboard)

_Fichier : `raw/v5-3-scripts-deploy-ui-viz.json` — 24 composants · 15 relations · 8 constats · 8 trous_


**Synthèse.** La tranche couvre le cycle « build → promouvoir → servir → vérifier » du dashboard obrain (port 8432) et le déploiement du checkpoint harness (binaires obrain). Le pivot UI est scripts/ui-deploy.sh : il bâtit (tsc+vite) ou prend un dist, mesure les capacités via scripts/ui-capacites.mjs sur la fermeture atteignable des assets (et non sur le dossier-cimetière), applique la porte scripts/ui-contrat.json (bloquantes markdown/katex, suivies fratrie/ws-client/projection/ramifications/admin-comptes/descente-fractale), gèle une version nommée en lecture seule, bascule atomiquement `current`, puis vérifie par HTTP via scripts/ui-verifier.sh ; l'invariant central est « jamais en arrière » (une capacité perdue refuse la promotion sauf --accepter-perte). Le service est lancé par scripts/vizserve.sh ou l'unit systemd scripts/obrain-viz.service, qui arment l'IAM, persistent l'index de trigrammes, activent la réécriture préfixe→intervalle et préchauffent les caches disque des agrégats (/api/viz/*). Une instance isolée (scripts/vizserve-isole.sh, port 8433) existe pour mesurer sans tuer le service partagé. Les sondes qualité mobile sont des portes headless-Chrome (scripts/ui-mobile-porte.mjs, scripts/ui-chat-porte.mjs, scripts/ui-lire-ecran.mjs, scripts/ui-mobile-capture.mjs) qui mesurent géométrie réelle, chevauchements et lisibilité. Le reste couvre la sync dev→prod des stores (scripts/sync-store-dev-prod.sh), l'auto-connaissance de la flotte (scripts/flotte-auto-connaissance.sh), l'export public du monorepo (scripts/export-public.sh) et la copie markdown mkdocs (scripts/copy_docs_md.py).


**Composants clés.**
- scripts/deploy-harness-obrain.sh — fait du checkpoint obrain la référence du harness (backup unique + remplacement atomique + sonde bridge + rollback cp)
- scripts/deploy-prod-first-check.sh — pré-vol prod-first LECTURE SEULE (binaire, bridge tools/list, stores prod/dev, confs systemd)
- scripts/ui-deploy.sh — orchestrateur de promotion UI : verrou flock, build tsc+vite, porte de capacités, version gelée, bascule atomique, vérification
- scripts/ui-bascule.sh — bascule délibérée vers une version déjà promue (verrou + réécriture etat.json + journal + vérif)
- scripts/ui-verifier.sh — comparaison par HTTP (sha256) de l'index.html et des assets servis vs version courante
- scripts/ui-contrat.json — contrat UI servie sur 8432 : bloquantes (markdown, katex) et suivies (7 capacités)
- scripts/ui-capacites.mjs — mesure des capacités d'un dist sur la fermeture atteignable depuis index.html (marqueurs compilés)
- scripts/ui-mobile-porte.mjs — porte tactile : débordement, cibles ≥44px, chevauchements, texte <11px sur 3 tailles
- scripts/ui-chat-porte.mjs — porte conversation : champ de saisie visible/actif/non recouvert, envoi réel, indépendance du canvas
- scripts/ui-lire-ecran.mjs — carte mesurée de l'écran (boîtes, polices, récit vertical, chevauchements) sans capture d'image
- scripts/ui-mobile-capture.mjs — capture PNG mobile + rapport JSON des états (accueil, chat, commandes cosmos)
- scripts/vizserve.sh — lance/relance le dashboard (arrêt robuste, entretien-bases, armement IAM, trigrammes, préchauffage agrégats)
- scripts/vizserve-isole.sh — seconde instance sur 8433 (écoute locale, arrêt par PID du port, --synapse-store)
- scripts/vizdev.sh — dashboard en mode DEV (OBRAIN_DEV=1, hot reload) sur 192.168.1.103:8432
- scripts/obrain-viz.service — unit systemd utilisateur : PATH pnpm, OBRAIN_PREFIXE_INTERVALLE=0, IAM armée, ExecStart viz serve
- scripts/sync-store-dev-prod.sh — copie reflink dev→prod avec --plan/--apply, détachement systemd-run, rollback .bak
- scripts/flotte-auto-connaissance.sh — sonde lecture seule de l'auto-déclaration des machines (SSH, /api/harness/health, requête graphe)
- scripts/export-public.sh — génère l'arbre public depuis le monorepo privé (rsync, stubs, scrub manifests, gates cargo/clippy/test)
- scripts/copy_docs_md.py — copie les .md de docs/ vers site/md pour consommation LLM (mkdocs)
- scripts/entretien-bases.sh — entretien appelé par vizserve.sh pendant la fenêtre d'exclusivité (degrés + repair)
- scripts/guard-viz.sh — garde DoD viz (wrapper de scripts/viz_guard.py, appelé par pre-push.sh)
- crates/obrain-cli/src/commands/viz.rs + viz_serve.rs + viz_ws.rs — implémentation de `obrain viz serve` (HTTP /app + /api/viz/*, dist résolu par OBRAIN_UI_DIR ou CARGO_MANIFEST_DIR)
- crates/obrain-viz/src/* — couche de restitution (ir, json_graph, dot, mermaid, overlay) consommée par viz serve
- packages/obrain-dashboard/ — front React/TSX servi sous /app (src/app, src/components, src/graph, src/lib/api.ts)

**Relations / dépendances.**
- vizserve.sh → `./target/release/obrain viz serve` → crates/obrain-cli/src/commands/viz.rs (VizCommands::Serve) → viz_serve.rs (sert /app + /api/viz/{modules,overview,arcs,ramifications,weave})
- vizserve.sh → scripts/entretien-bases.sh (appelé pendant la fenêtre entre arrêt et relance ; OBRAIN_SANS_ENTRETIEN=1 / OBRAIN_REPAIR_APPLY=1)
- vizserve.sh et obrain-viz.service → armement IAM via OBRAIN_IAM / OBRAIN_IAM_ACCOUNT / OBRAIN_IAM_STORE (magasin dédié iam.obrain)
- vizserve.sh → préchauffage /api/viz/modules + /api/viz/overview (parallèle) puis /api/viz/arcs (séquentiel borné, ex-OOM)
- ui-deploy.sh → ui-capacites.mjs (mesure capacités) + ui-contrat.json (porte bloquantes/suivies) → ui-verifier.sh (vérification HTTP finale)
- ui-bascule.sh → ui-verifier.sh (re-vérifie après bascule délibérée)
- ui-deploy.sh → dé-couplage : renomme le dist du worktree et le remplace par un lien → $UI_DIR/current (path figé CARGO_MANIFEST_DIR/../../packages/obrain-dashboard/dist, override OBRAIN_UI_DIR)
- sync-store-dev-prod.sh → `$BIN bridge` (sondes count Session/Message) → systemd-run --user (détachement) → systemctl stop/start dsh-web + dsh-dev-3083
- deploy-harness-obrain.sh → `$BIN bridge` (initialize + tools/call sessions_list) sur copie RO du store ; écrit dans ~/.local/share/obrain-harness/bin
- deploy-prod-first-check.sh → `$BIN bridge` tools/list + vérification stores runtime-{prod,dev}.obrain et confs systemd dsh-web
- flotte-auto-connaissance.sh → SSH BatchMode + curl http://ip:8090/api/harness/health + `obrain query` sur runtime-dev.obrain (nœuds machine:/harness:)
- export-public.sh → rsync (exclusions cognitive/rag/bench) + stubs PRIVATE_FILES + cargo check/clippy/test gates sur l'arbre exporté
- copy_docs_md.py → parse mkdocs.yml → copie docs/*.md vers site/md + llms.txt
- guard-viz.sh → exec python3 scripts/viz_guard.py (logique de garde), appelé par scripts/pre-push.sh
- ui-mobile-porte.mjs / ui-chat-porte.mjs / ui-lire-ecran.mjs / ui-mobile-capture.mjs → spawn Chrome headless (ms-playwright chromium-1234) via CDP WebSocket (ports 9250/9251/9253/9254)

**Points notables.**
- ui-capacites.mjs ne mesure pas le dossier assets (un « cimetière » de bundles périmés a fait mentir la mesure le 15/09) mais la FERMETURE ATTEIGNABLE depuis index.html, en suivant les citations de chunks dynamiques
- Invariant « jamais en arrière » : ui-deploy.sh refuse de promouvoir une version qui a perdu une capacité suivie que la version servie possédait, sauf --accepter-perte=<capacite> explicite et journalisé
- ui-contrat.json porte un méta-commentaire « Modifier ce fichier est un ACTE explicite » — le contrat est le lieu unique de décision de ce que l'utilisateur ne doit jamais reperdre
- vizserve.sh contient un historique opérationnel dense : OOM des trois agrégats arcs parallèles (1,5 M nœuds/4,8 M arêtes), piège pgrep -f qui matche sa propre ligne de commande (contourné par obrain vi[z] serve), persistance d'index de trigrammes (CONTAINS 59,6 ms → 0,55 ms)
- obrain-viz.service éteint délibérément OBRAIN_PREFIXE_INTERVALLE=0 (mesure A/B : réécriture STARTS WITH→intervalle 15-20× plus lente sur LIMIT bas) alors que vizserve.sh l'active par défaut — deux postures opposées entre service et script manuel
- Les 5 scripts ui-*.mjs sont des « portes » de qualité automatisées headless-Chrome qui remplacent l'inspection visuelle par des mesures géométriques reproductibles (chevauchements, seuil tactile 44 px, police <11 px)
- sync-store-dev-prod.sh s'auto-détache via systemd-run --user --wait --pipe pour survivre à l'arrêt du service qui l'héberge (sinon --apply se suicide à l'étape 1)
- export-public.sh définit la frontière public/privé : obrain-cognitive et obrain-rag sont privés ; les fichiers mixtes sont STUBBÉS (pas supprimés) pour que cargo fmt reste résolvable, et les features cognitifs restent déclarées mais vides (cfg jamais-vrai)

**Trous / incohérences.**
- PRESETS dupliqués dans vizdev.sh, vizserve.sh, vizserve-isole.sh et obrain-viz.service (mêmes chemins de stores, drift possible entre instance manuelle et service) ; vizserve-isole.sh omet les presets Linux et garde un sous-ensemble
- ui-verifier.sh : LIENS_ATTENDUS câblé en dur sur /home/triviere/projects/banc-obrain-j5/packages/obrain-dashboard/dist, worktree qui n'est pas le checkout courant (consolidation/tronc-2026-09-15-plus-fusion)
- deploy-harness-obrain.sh : BAK (obrain.bak-20260904) et TAG (v0.0.1-zorder-2026-09-04) codés en dur avec dates figées — le backup unique ne se re-fait jamais, rollback vers un état figé
- export-public.sh : `sed -i ''` est une syntaxe BSD/macOS (script écrit pour Homebrew/rustup) — portabilité Linux non garantie sans adaptation
- flotte-auto-connaissance.sh : liste MACHINES codée en dur (n1/n2/n3/dgx/m4), pas de découverte ; la sonde interroge le port 8090 (/api/harness/health) alors que le reste de la tranche sert 8432/3082/3083 — port harness non unifié
- Aucun test/CI visible dans le périmètre pour les scripts shell eux-mêmes (ui-*.sh, vizserve.sh, sync-store) : seuls guard-viz.sh (via pre-push.sh) et les gates cargo d'export-public.sh sont vérifiés
- copy_docs_md.py : le parsing de mkdocs.yml est naïf (regex ligne par ligne, pas de gestion de blocs/anchors) et l'exclusion par sous-chaîne `in path_str` est fragile ; dépend de mkdocs build préalable (site/ absent → warning, pas d'erreur)
- vizdev.sh n'arrête pas proprement l'ancienne instance (fuser -k 8432) mais ne gère ni l'attente de libération du port ni l'armement IAM/trigrammes que vizserve.sh et le service appliquent — posture dev incohérente avec la prod



### 5.4 scripts-enrich-index — pipeline d'enrichissement et d'indexation du graphe

_Fichier : `raw/v5-4-scripts-enrich-index.json` — 14 composants · 12 relations · 8 constats · 7 trous_


**Synthèse.** Cette tranche couvre les 14 scripts d'enrichissement, d'indexation et d'entretien du graphe obrain. Elle se décompose en quatre familles : (1) l'enrichissement topologique de la courbure de Ricci (`enrich-ricci-maintain.sh`, `enrich-ricci-window.sh`) sur 8 bases servies par le dashboard, où l'invariant mono-écrivain oblige à tuer les détenteurs de verrou exclusif PAR PID avant toute passe ; (2) l'indexation et le transfert de corpus : `index-code-projects.sh` (index_repository via le pont MCP), les convertisseurs Node `.mjs` qui s'appuient sur le binaire N-API `/tmp/rtprobe.node` (`runtime-to-substrate.mjs`, `code-index-to-substrate.mjs`, `code-corpus-to-binary.mjs`) pour peupler les stores substrate depuis les SQLite cbm ; (3) l'entretien et la mémoire : `entretien-bases.sh` (colonnes de degrés + diagnostic des enregistrements), `memoire.sh` / `memoire-appliquer-comptes.sh` (application journalisée du Self), `backfill-sessions.sh` (réparation des métadonnées de session perdues) ; (4) la projection de chantier `couche-decision.gql` + son pilote `couche-decision-ingest.sh`, qui encode l'ADR-2026-09-22 en 21 instructions GQL rejouables. Le fil conducteur est l'invariant mono-écrivain du substrat : tout script d'écriture arrête proprement les détenteurs, vérifie l'absence de verrou, et repart en lecture si un autre processus tient la base. Deux auxiliaires complètent l'ensemble : `session-iam.mjs` (ouverture de session authentifiée pour les bancs) et `cosmos-capture.mjs` (vérification visuelle WebGL par CDP), qui ne peuplent pas le graphe mais le surveillent.


**Composants clés.**
- scripts/enrich-ricci-maintain.sh — passe continue de `obrain enrich --ricci` sur 8 bases, saute les stores verrouillés (fuser sur substrate.meta), flock mono-instance, idempotent
- scripts/enrich-ricci-window.sh — fenêtre de maintenance : arrêt des détenteurs PAR PID, enrich avec reprises, relance garantie du dashboard par `trap restart_viz EXIT INT TERM`
- scripts/index-code-projects.sh — insère les projets CODE (index_repository) dans runtime-prod/runtime-dev via `obrain bridge`, détaché systemd-run, arrêt/relance dsh-web + dsh-dev-3083
- scripts/runtime-to-substrate.mjs — convertit un SQLite cbm runtime (nodes/edges/runtime_payloads) en store substrate, reconstruit les arêtes IN_SESSION session→message
- scripts/code-corpus-to-binary.mjs — génère le corpus binaire little-endian pour `obrain native-import` (RFC §16.1)
- scripts/code-index-to-substrate.mjs — importe les index de code cbm (un SQLite par projet) dans un store substrate unique, batch createEdgesBatchFast 8192, journal-import détaché
- scripts/backfill-sessions.sh — rejoue les en-têtes session.jsonl.zstd via import_events (origin/parentSession) + pose source_path en GQL, simulation par défaut (--appliquer)
- scripts/memoire.sh — applique le journal de mémoire du chat dans self-demo.obrain (MERGE idempotent), arrêt propre du viz serve, contrôle des détenteurs via /proc/<pid>/fd
- scripts/memoire-appliquer-comptes.sh — applique le journal Self compte par compte dans selfs/<compte>.obrain, découverte des comptes par le journal (pas l'annuaire IAM)
- scripts/session-iam.mjs — ouvre une session authentifiée (POST /api/auth/login) et rend le cookie ; sonde /api/stores pour détecter serveur ouvert (200) vs armé (401)
- scripts/entretien-bases.sh — maintenance pendant la fenêtre de redémarrage : colonnes de degrés (repair-degrees, en place) + diagnostic/application `obrain repair` + acquittement `obrain entretien`
- scripts/cosmos-capture.mjs — capture de la vue Cosmos via Chrome CDP (127.0.0.1:9248), sonde WebGL, mesure pixel via ffmpeg (contenu/luminosité/centre de lumière)
- scripts/couche-decision.gql — projection GQL du chantier « couche de décision » (source docs/ADR-2026-09-22-COUCHE-DECISION.md) : 21 instructions, labels Chantier/Todo/Gate/Constraint/Pitfall/Test/Risk/CodeRef/Artefact, 121 arêtes
- scripts/couche-decision-ingest.sh — pilote qui joue couche-decision.gql une instruction par invocation (parseur « entire query must be consumed »), --store explicite obligatoire, MERGE idempotent

**Relations / dépendances.**
- enrich-ricci-maintain.sh / enrich-ricci-window.sh → `obrain enrich --ricci <store>` + fuser sur substrate.meta pour détecter les verrous exclusifs
- enrich-ricci-window.sh → scripts/vizserve.sh (relance du dashboard sur :8432 via `trap restart_viz EXIT INT TERM`)
- index-code-projects.sh → `obrain bridge` (outils MCP index_repository + list_projects) + systemd-run --user (détachement) + systemctl dsh-web.service / dsh-dev-3083.service
- runtime-to-substrate.mjs / code-index-to-substrate.mjs / enrich-live-dev-to-store.mjs → require('/tmp/rtprobe.node') (binding N-API ObrainDB) + node:sqlite DatabaseSync
- code-corpus-to-binary.mjs → consommé par `obrain native-import` (format binaire RFC §16.1)
- code-index-to-substrate.mjs → spawn détaché de `obrain journal-import <dest> --prefix` (journal d'import RFC diff-incremental-variantes)
- memoire.sh / memoire-appliquer-comptes.sh → `obrain memoire --self <qn> [--store] [--appliquer]` (journal append-only → MERGE dans la base Self)
- entretien-bases.sh → `obrain repair-degrees`, `obrain repair [--apply]`, `obrain entretien [--accorder]` ; appelé par scripts/vizserve.sh entre arrêt et relance
- backfill-sessions.sh → `obrain mcp` (import_events, sessions_list) + `obrain query` (SET source_path en GQL) ; arrête dsh-web/dsh-dev-3083 pour le verrou exclusif
- couche-decision-ingest.sh → `obrain query <store> <instruction>` ligne par ligne sur couche-decision.gql
- couche-decision.gql → docs/ADR-2026-09-22-COUCHE-DECISION.md (source de vérité), et ancres vers crates/obrain-engine/src/query/plan.rs, crates/obrain-cognitive/src/synapse.rs, hopfield.rs, store_trait.rs, intent.rs
- session-iam.mjs → endpoints HTTP /api/stores (sonde) et /api/auth/login (session), identifiants depuis options / OBRAIN_IAM_USER+PASSWORD / data/iam.obrain.identifiants.txt

**Points notables.**
- L'invariant mono-écrivain du substrat structure TOUT le périmètre : memoire.sh, backfill-sessions.sh, entretien-bases.sh et enrich-ricci-window.sh arrêtent les détenteurs puis vérifient l'absence de verrou avant toute écriture exclusive, et documentent explicitement la raison (flock partagé en lecture, refus de promotion en verrou exclusif).
- enrich-ricci-window.sh documente un piège réel de pkill : `pkill -f 'obrain viz serve'` matche sa propre ligne de commande quand le motif y figure — le script se tire dessus (constaté deux fois le 2026-09-10) ; d'où l'arrêt PAR PID lu depuis fuser, jamais par motif.
- Tous les convertisseurs .mjs reposent sur /tmp/rtprobe.node, un binaire N-API de 9,8 Mo posé dans /tmp — dépendance éphémère non versionnée, absente du dépôt, donc non reproductible depuis le checkout seul.
- couche-decision.gql encode une liste dense de limites MESURÉES du parseur GQL : une seule instruction par invocation, INSERT d'arête non idempotent, `order` mot réservé, concaténation de chaînes rendant 0, STARTS WITH/CONTAINS inutilisables, projection d'une propriété absente éliminant la ligne (C14–C21).
- couche-decision.gql durcit 21 contraintes (C1–C21) dont C20/C21 découvertes « après la première projection », alors que l'en-tête annonce encore « 19 contraintes » — dérive documentaire mineure entre le commentaire et le corps.
- Les scripts hardcodent des chemins et adresses absolus non portables : /home/triviere/projects/banc-obrain, l'IP 192.168.1.103:8432, /tmp/linux-full3.obrain, /tmp/obrain-viz-cache — exécutables uniquement sur cette machine.
- enrich-live-dev-to-store.mjs est un script d'enrichissement (copie de props de session runtime-dev → runtime-full-v2) mais n'est PAS capté par le glob enrich-*.sh ; il cible runtime-full-v2.obrain, absente de la liste des 8 bases de enrich-ricci-*.sh.
- cosmos-capture.mjs est le seul script du périmètre dédié à la VÉRIFICATION VISUELLE (screenshot CDP + mesure de pixels ffmpeg) plutôt qu'au peuplement du graphe — il surveille la scène WebGL Cosmos, pas le substrat.

**Trous / incohérences.**
- /tmp/rtprobe.node est un binaire N-API non versionné : où est son code source et comment le reconstruire ? (aucune référence trouvée dans le périmètre ni dans scripts/).
- Incohérence de nommage des stores : backfill-sessions.sh vise runtime.obrain en prod alors que enrich-ricci-*.sh et index-code-projects.sh utilisent runtime-prod.obrain — deux noms pour (probablement) la même base.
- code-index-to-substrate.mjs ligne 27 : dans la branche NO_PROPS, `ids.set(..., createNodeNoWal(lab))` stocke la valeur de retour sans `.id`, alors que la branche normale stocke `createNode(...).id` — asymétrie suspecte si createNodeNoWal rend un objet nœud.
- enrich-live-dev-to-store.mjs cible runtime-full-v2.obrain, base absente de la liste BASES de enrich-ricci-maintain/window.sh — cible orpheline ou legacy à éclaircir.
- couche-decision.gql bloque si le projet « obrain » n'a pas été indexé dans le store cible (bloc §20 « 1 885 fichiers scannés ») ; l'ingestion des ancres réelles échoue silencieusement sans ce prérequis, non contrôlé par le pilote.
- session-iam.mjs ne couvre pas le cas d'un cookie absent sur réponse 200 (POST login direct) et lit un fichier d'identifiants en clair — aucune rotation ni test unitaire visible dans le périmètre.
- Les deux passe de entretien-bases.sh (degrés puis diagnostic) ont été fusionnées « base par base » après qu'un agent concurrent a regaspillé la fenêtre libre — preuve que ces scripts dépendent d'une discipline d'exécution externe non verrouillée par un mécanisme global.



### 5.5 scripts-*-check.mjs + gql-conformance.py + engine-etat.py (garde-fous de vérification vivante du dashboard, du moteur et du dialecte GQL)

_Fichier : `raw/v5-5-scripts-mjs-py.json` — 23 composants · 13 relations · 8 constats · 8 trous_


**Synthèse.** Cette tranche regroupe ~22 sondes/contrôles (Node .mjs, Python) qui vérifient le dashboard obrain et le moteur contre le service VIVANT, majoritairement via le protocole CDP de Chrome (WebSocket) et ffmpeg pour mesurer de vrais pixels. Leur philosophie commune : ne jamais croire un champ qui peut être undefined/NaN, mesurer l'effet réel (luminance, contenu de scène, requêtes réseau observées) plutôt que le calcul, et dire explicitement ce qu'on ne peut pas mesurer (SKIP, identité IAM, erreurs connues). Le « contrat » central est celui du WEAVE (scripts/check-weave-contract.mjs) : tous les champs lus par CosmosWeave.tsx existent/typés, aucun NaN dans masse/orbite/taille/affaissement de nappe, curseur de densification ≥2 paliers monotones, fibres classables. Deux familles de gardes structurent le reste : (a) les NATURES (natures-check.mjs : tout type réellement présent déclaré dans natures.ts, couleurs séparées ≥33, table cohérente avec les listes en dur des vues) et (b) la FRATRIE/fractale/vues (fratrie-check.mjs exige une règle de parenté unique window.__fratrie.parentDe, la fractale-check.mjs une descente récursive réelle, vues-check.mjs un canvas WebGL sain et un bundle non périmé). Les scripts chat-* (ancrage, reprise protocole SSE, reprise vraie UI) vérifient que le chat lit le store du sujet et qu'une coupure de flux ne fait jamais afficher « flux interrompu ». Enfin gql-conformance.py suit la conformité ISO/IEC 39075:2024 (annotation // ISO: dans les tests .rs croisée avec docs/user-guide/gql/conformance.md) et tools/engine-etat.py génère un diagramme Mermaid d'état dérivé d'artefacts distants (ssh dgx). L'ensemble instrumente le dashboard via des hooks de test exposés (window.__etages, window.__fratrie, window.__net, window.__fractale, window.__pickSweep).


**Composants clés.**
- scripts/check-weave-contract.mjs — contrat de la toile Cosmos (champs, NaN, nappe, densification, fibres)
- scripts/natures-check.mjs — garde des NATURES : 27 types déclarés, couleurs séparées (seuil 33), accord vues/table
- scripts/fratrie-check.mjs — 5 contrôles de parenté (règle unique, sélection→fratrie, troncature, clic frère, index local) — 1062 lignes
- scripts/fractale-check.mjs — descente fractale Atlas (sous-groupes réels, profondeur, tokens valides)
- scripts/vues-check.mjs — vues Monde/Cosmos : routes non bloquées + bundle servi à jour + canvas WebGL
- scripts/endpoints-check.mjs — balayage complet des endpoints (froid/chaud, SSE, identité IAM)
- scripts/emphasis-check.mjs — effet visuel de sélection mesuré en luminance (figé, différentiel)
- scripts/world-add-check.mjs — ajout d'un monde GitHub depuis l'interface, enrichi (clone→indexation→enrichissement)
- scripts/activation-live-check.mjs — l'engine consulté par un autre processus allume le graphe en direct
- scripts/mobile-panels-check.mjs — menus rétractables sur mobile (390×844, repli mémorisé/localStorage)
- scripts/views-controls-check.mjs — grammaire de contrôles commune aux 3 vues + continuité de sujet
- scripts/e2e-dashboard.mjs — E2E headless (navigation Monde, dive, carte, historique, multi-store)
- scripts/facetbar-monde.mjs — la barre de facettes (fratrie) se monte dans l'état par défaut Monde
- scripts/chat-ancrage-check.mjs — le chat lit le store du sujet visé (this-rs/this), pas runtime-dev
- scripts/chat-reprise-check.mjs — reprise d'un flux SSE coupé (protocole, --tuer tue obrain viz serve)
- scripts/chat-reprise-ui-check.mjs — reprise en vraie interface : « flux interrompu » jamais affiché
- scripts/continuum-check.mjs — descente continue sans trou + traversée des échelles à la molette seule
- scripts/projection-check.mjs — projection multi-axes : clic ancrable→/api/viz/subgraph, cache, rafale, borne 20000
- scripts/echelle-multi-niveaux-check.mjs — 7 exigences du canevas multi-échelles (4 crans, polymorphisme, fluence)
- scripts/gql-conformance.py — conformité GQL ISO/IEC 39075:2024 (report/dialect/validate)
- tools/engine-etat.py — génère docs/mermaid/engine-007-etat.mmd depuis les artefacts de passes (ssh dgx)
- scripts/session-iam.mjs — helper d'ouverture de session IAM (importé par chat-reprise-check et requetes-latence-check)
- scripts/cosmos-capture.mjs — helper de capture CDP référencé par fractale-check

**Relations / dépendances.**
- check-weave-contract.mjs → packages/obrain-dashboard/src/graph/CosmosWeave.tsx (champs/maths lus par le composant) et GET /api/viz/weave
- natures-check.mjs → packages/obrain-dashboard/src/graph/natures.ts (table NATURES) + GET /api/stores + /api/viz/overview (jamais /api/viz/signatures, tronqué à 10 types)
- fratrie-check.mjs → window.__fratrie.parentDe (src/graph/fratrie.ts) + window.__etages.filtre + window.__pickSweep + /api/viz/subgraph, /api/viz/modules, /api/functions
- projection-check.mjs → /api/query (GQL) + /api/viz/subgraph (et stream WS) + instrumentation window.__net/window.__t0 ; cible /api/viz/modules
- chat-reprise-check.mjs → import session-iam.mjs ; /api/engine/chat (SSE événements texte/think/fin/erreur) ; pgrep/kill 'obrain vi[z] serve'
- chat-ancrage-check.mjs → /api/engine/sante + bouton .chat-flottant + textarea .chat-pied
- echelle-multi-niveaux-check.mjs → window.__etages + /api/viz/space (comptage Network) + src/graph/densite.ts (loi du redessin)
- vues-check.mjs → /api/viz/status, /api/stores, /api/worlds, /api/viz/signatures, /api/viz/overview, /api/viz/weave, /api/info + bundle /app/assets/index-*.js
- endpoints-check.mjs → 25+ routes (api/stores, info, viz/*, functions, query, worlds, engine/*, entretien, actifs statiques) avec seuils par route
- gql-conformance.py → docs/user-guide/gql/conformance.md (Annexe D) + crates/obrain-engine/tests/*.rs (annotations // ISO:) + Cargo.toml (version) → docs/gql-dialect.json
- tools/engine-etat.py → ssh dgx → /home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/qualification/e7b-f1a-q0-*/*.txt + POST /lint et /update sur http://127.0.0.1:7777 (mermaid.ffs.dev)
- Tous les contrôles CDP → Chrome headless (swiftshader) sur 127.0.0.1:9248/9251/9253 + ffmpeg (luminance/contenu de scène) ; défaut BASE = http://192.168.1.103:8432
- endpoints-check / echelle / chat-reprise partagent le motif IAM : OBRAIN_TOKEN/OBRAIN_COOKIE → SKIP explicite des 401 (jamais un faux positif ni un faux négatif)

**Points notables.**
- Le dashboard expose des HOOKS DE TEST dédiés (window.__etages, window.__fratrie.parentDe, window.__pickSweep, window.__net, window.__fractale) que les sondes lisent — un contrat d'instrumentation implicite entre UI et contrôles.
- Plusieurs contrôles mesurent des PIXELS RÉELS (ffmpeg luminance/ratio de contenu) plutôt que des états DOM, car les scènes 'morph' bougent de 17 à 36 % en permanence ; l'effet de sélection (emphasis) n'est mesurable qu'une fois la scène FIGÉE (bouton ⏸).
- fratrie-check.mjs (62 Ko, 1062 lignes) est de loin le plus gros : il recopie la règle de parenté en heuristique de ciblage, résout les ancres en qn (resoudreAncres) et chronomètre l'index local à 160/8000/200000 nœuds (l'ancien Math.min(...a) levait RangeError à 200000).
- endpoints-check.mjs distingue froid/chaud par route, traite le SSE (lecture du premier événement), et évite le faux positif 'error' sur les actifs statiques (un bundle JS contient forcément la chaîne "error").
- L'échelle-multi-niveaux-check.mjs annonce 'CINQ EXIGENCES' en en-tête mais en énumère 7 (numérotation 1-7) — dérive de rédaction entre titre et corps.
- gql-conformance.py est 100 % hors-ligne (scan de fichiers .rs et .md) : report, dialect (génère docs/gql-dialect.json) et validate (CI, exit code) — aucun appel réseau.
- tools/engine-etat.py est un générateur hybride : il lit des artefacts distants par ssh (dgx), reconstruit un flowchart Mermaid, puis LINTE et publie via l'API mermaid.ffs.dev (POST /lint, /update, /create).
- Le motif anti-faux-négatif est systématique : les contrôles attendent la donnée qu'ils mesurent (bornes de sondage) au lieu de supposer sa présence (hops=2 coûte 12,8 s au premier appel).

**Trous / incohérences.**
- scripts/engine-etat.py est ABSENT (le périmètre le mentionne) ; seul tools/engine-etat.py existe — incohérence de chemin entre l'énoncé et le dépôt.
- tools/engine-etat.py : sous-graphe DEF mal construit — le bloc chunk (lignes 199-205) jette le texte accumulé et émet des nœuds d<num> à libellé vide, et ne purge jamais le reliquat <4 ; le nœud dlist 'defauts 46 a 56' est codé en dur (potentiellement périmé).
- tools/engine-etat.py : RUN, OUT, API et le gate 'Q0_STATUS=NOT_PASSED / opérateur non qualifié' sont codés en dur (lignes 20-23, 150-157) — non paramétrable, non reproductible hors machine dgx.
- La plupart des sondes ont pour BASE par défaut l'IP privée 192.168.1.103:8432 (non portable, aucun repli auto sur 127.0.0.1) ; facetbar-monde.mjs pointe 8433 et CDP 9423 (instance isolée).
- gql-conformance.py ne scanne que crates/obrain-engine/tests : des annotations // ISO: dans d'autres crates seraient ignorées par le rapport de couverture.
- activation-live-check.mjs porte les permissions 600 (rw-------) et mesure un allumage de monde par diff de pixels, mais la propriété centrale (avant→après de la ligne 'Engine:') ne vérifie pas que le MONDE précis consulté s'allume (le diff est global/informatif).
- continuum-check.mjs : la passe molette réutilise une variable t2 fermée sur un txt() antérieur (ligne 199-206) — mélange de relevés clavier/molette dans la même fonction pct().
- e2e-dashboard.mjs et chat-reprise-ui-check.mjs dépendent d'un Chromium Playwright à chemin codé en dur (chromium-1234) ; absence = SKIP silencieux (exit 0), ce qui peut masquer une non-vérification en CI.




## Vague 6 — V6 Qualité

### 6.1 Tests golden et garantie de non-régression

_Fichier : `raw/v6-1-golden-tests.json` — 14 composants · 9 relations · 8 constats · 7 trous_


**Synthèse.** La garantie de non-régression d'obrain repose sur trois mécanismes complémentaires, tous réels et inspectables. Le premier est la matrice de portes documentée dans docs/GARANTIES-NON-REGRESSION.md : une chaîne de commandes rejouables (cargo test par crate, puis --tests avec features cdc/algos/ai, puis --all-features --workspace en CI) assortie d'un inventaire rouge daté au ledger. Le second est le test golden octet-à-octet de la couche de visualisation : crates/obrain-viz/tests/golden.rs compare la sortie des writers (json/dot/mermaid/graphml) à des fichiers de référence figés dans crates/obrain-viz/tests/golden/, régénérables via l'exemple print_golden.rs. Le troisième est le registre des échecs connus scripts/echecs-connus.txt, lu par scripts/bench-regression.sh, qui distingue un échec NOMÉ-DATÉ-ATTRIBUÉ (toléré, affiché) d'un nouvel échec (rouge) — principe « un banc durablement rouge n'apprend plus rien ». La couche substrate fige elle aussi des octets little-endian par des tests golden (zero_copy_views.rs) et verrouille le déterminisme du read-path Top-K par tri (sim desc, offset asc) documenté dans store.rs. docs/golden/2026-09-04/ conserve des artefacts historiques (compteurs, latences, schémas CLI, sondes bridge) qui servent de référence de régression datée plutôt que de diff automatisé. L'hermétisme (tests indépendants de l'environnement du poste) est tenu par code via GardeIam::desarmer_pour_les_tests() et un audit env::set_var/remove_var documenté dans le même fichier de garanties.


**Composants clés.**
- docs/GARANTIES-NON-REGRESSION.md — matrice de portes (bench/features/exactitude), règles d'hermétisme (IAM désarmé, env_remove, OBRAIN_PERF_GATE), inventaire rouge daté 2026-09-05 → 2026-09-17
- crates/obrain-viz/tests/golden.rs — 7 tests anti-régression de schéma comparant les octets de sortie aux golden files (json/dot/mermaid/graphml + variantes overlay energy)
- crates/obrain-viz/tests/golden/ — golden files de référence : example.{json,dot,mmd,graphml} + example.energy.{json,dot,mmd}
- crates/obrain-viz/examples/print_golden.rs — régénérateur des golden files (cargo run -p obrain-viz --example print_golden), échantillon sample() dupliqué volontairement avec tests/golden.rs
- scripts/echecs-connus.txt — registre des échecs connus (format nom_du_test | date | attribution) ; entrée unique : des_ancres_divergentes_ne_rendent_jamais_un_resultat_ampute
- scripts/bench-regression.sh — harnais de régression 8 axes ; lit scripts/echecs-connus.txt (lignes 72-103) pour ne pas rendre le banc rouge sur un échec connu
- scripts/viz_guard.py — garde DoD de la couche viz : déterminisme octet-à-octet (sha256), structure, bornes limit/truncated, parité export_all == comptage store, bench
- scripts/guard-viz.sh — wrapper shell de scripts/viz_guard.py (point d'entrée documenté dans pre-push.sh)
- crates/obrain-substrate/tests/zero_copy_views.rs — goldens little-endian : golden_le_prefixe_longueur_du_heap, golden_le_entree_index_blob (représentation disque FIGÉE octet à octet)
- crates/obrain-substrate/tests/slot_remap_sidecars.rs — t4_golden_vector_top_k_avant_apres : parité des scores cosinus avant/après repermutation des slots
- crates/obrain-substrate/tests/zero_copy_suite.rs — vue_arete_survit_a_la_reouverture (formats v2 little-endian, « goldens O1 »)
- crates/obrain-substrate/src/store.rs — contrat de déterminisme du read-path Top-K (tri sim desc, offset asc) « que le golden test du read-path verrouille » (ligne ~14522)
- docs/golden/2026-09-04/ — artefacts historiques datés : 2026-09-04-engine.txt, 2026-09-04-o1.txt, cli-info.json, cli-schema.json, cli-stats.json, gql-counts.jsonl, bridge-sonde.jsonl
- docs/FULL-OBRAIN-VIZ-ROADMAP-2026-09-08.md — spéc vivante des goldens viz (json-graph v1 gelé, échappements hostiles, overlays volatils figés par ts)

**Relations / dépendances.**
- crates/obrain-viz/tests/golden.rs ← lit → crates/obrain-viz/tests/golden/example.* via fn golden(name) et assert_same() (assert_eq! octets)
- crates/obrain-viz/examples/print_golden.rs → écrit → crates/obrain-viz/tests/golden/example.* (régénération après évolution volontaire du schéma)
- crates/obrain-viz/tests/golden.rs ↔ crates/obrain-viz/examples/print_golden.rs : échantillon sample() dupliqué volontairement, les deux copies doivent rester identiques
- scripts/guard-viz.sh → exec → scripts/viz_guard.py (la logique vit en python, le wrapper garde le point d'entrée shell)
- scripts/bench-regression.sh → lit → scripts/echecs-connus.txt (registre des échecs connus, boucle while IFS= read + cut -d'|' -f1)
- crates/obrain-substrate/tests/zero_copy_views.rs → fige → représentation little-endian de HeapPage::append et BlobSlotEntry (bytemuck::bytes_of / pod_read_unaligned)
- crates/obrain-substrate/tests/slot_remap_sidecars.rs → appelle → store.vector_top_k (obrain-substrate/src/store.rs) et recompute cosine() pour parité
- docs/GARANTIES-NON-REGRESSION.md → documente → commandes cargo test des crates (obrain-core, obrain-substrate, obrain-engine, obrain-cli) et .github/workflows/ci.yml
- docs/FULL-OBRAIN-VIZ-ROADMAP-2026-09-08.md → référence → crates/obrain-viz/tests/golden/ et scripts/guard-viz.sh (§5 garde DoD)

**Points notables.**
- Deux familles de « golden » coexistent : (a) golden files octet-à-octet pour le schéma de sortie viz (obrain-viz) et (b) goldens de représentation binaire disque (substrate little-endian) — la première est diff automatisé, la seconde est figée par assertion d'octets en dur
- docs/golden/2026-09-04/ n'est PAS un diff automatisé : ce sont des artefacts historiques (sorties de requêtes/perf/CLI sur un store code-all3 de 445 493 nœuds / 1 670 005 arêtes) conservés comme référence datée
- Le mécanisme echecs-connus.txt est une philosophie explicite : un banc durablement rouge « n'apprend plus rien », donc un échec nommé-daté-attribué est toléré et AFFICHÉ mais ne rend pas le banc rouge ; un échec inexpliqué reste rouge
- L'échantillon golden viz est hostile par conception : ids avec ':', '/', '-', '.', libellés avec guillemet et saut de ligne, signaux energy/curvature/scc/ricci/stale — pour verrouiller les échappements writers dès maintenant
- La duplication volontaire de sample() entre tests/golden.rs et examples/print_golden.rs est un point de fragilité assumé et documenté (« les garder identiques »)
- Le golden ratio apparaît aussi comme CONSTANTE déterministe (seed SplitMix64 0x9E37_79B9_7F4A_7C15 dans tiers.rs, vecteur initial spectral 0.618033988749895) — sans rapport avec les golden files, mais nommé 'golden'
- docs/GARANTIES-NON-REGRESSION.md documente des faux positifs d'audit corrigés : le motif set_var|remove_var désignait à tort offset_var/remove_variable (5 fichiers innocents) — le motif exact est env::set_var/env::remove_var
- L'inventaire rouge daté montre un progrès mesurable : 8 769 · 26 · 14 au départ → 8 838 · 0 · 0 le 2026-09-17 (cargo test --workspace --no-fail-fast en environnement propre)

**Trous / incohérences.**
- docs/golden/2026-09-04/ ne référence aucun script qui le consomme ou le compare automatiquement : rôle exact de ces artefacts dans la chaîne de non-régression non établi (référence manuelle ?)
- Aucun mécanisme de golden diff ne semble exister pour la sortie GQL/query (contrairement à viz et substrate) : gql_spec_compliance.rs est cité dans les garanties mais non inspecté ici (hors périmètre grep 'golden')
- La duplication manuelle de sample() (golden.rs ↔ print_golden.rs) est fragile : une divergence entre les deux copies produirait un test vert mais un golden régénéré incohérent — aucun test ne vérifie leur identité
- scripts/echecs-connus.txt ne contient qu'UNE entrée, alors que le fichier de garanties décrit plusieurs flake/gardes de temps (annulation_bout_en_bout, import_bulk_400k...) : la couverture du registre semble partielle
- Les goldens substrate little-endian sont des assertions d'octets en dur dans le code (pas de fichiers golden séparés) — un portage big-endian casserait les tests, ce qui est le but, mais la « vérité » de référence est dispersée dans le code de test
- print_golden.rs, guard-viz.sh et les golden files ont des permissions incohérentes (-rw------- pour print_golden.rs et GARANTIES, -rwx--x--x pour guard-viz.sh) : non exécutable en lecture pour d'autres utilisateurs, possiblement accidentel
- Le read-path Top-K cite « le golden test du read-path » (store.rs ligne 14522) mais le test golden correspondant n'a pas été localisé nommément dans crates/obrain-substrate/tests/ par le grep 'golden' (seul slot_remap_sidecars.rs contient vector_top_k golden)



### 6.2 qualification/ (ENGINE-004→007, programme de qualification forensique du moteur d'inférence)

_Fichier : `raw/v6-2-qualification.json` — 22 composants · 9 relations · 10 constats · 12 trous_


**Synthèse.** La tranche 'qualification' ne qualifie pas le produit obrain (base de graphe Rust) mais un moteur d'inférence dérivé de llama.cpp optimisé sur GPU NVIDIA GB10 (sm_121a, CUDA 13.2, machine spark-c578), autour des quantifications TQ2_G128 et q8_1. Le processus est une chaîne de passes ENGINE-004→005→006→007A→007B-F1A, chacune clôturée par un ledger de statuts UPPER_SNAKE_CASE figés (PROUVE/RETIRE/OUVERT/PARTIEL/FINAL) et un registre de défauts de harnais numérotés. La synthèse est un graphe Mermaid unique (VISION-ENGINE.mmd, 339 lignes) qui consolide 73 rapports en 18 sphères de fragments ; le répertoire qualification/ ne contient que cette carte et ses 18 fragments .txt, les artefacts de mesure vivant hors dépôt dans /home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/. Les runs RUN-001/002/003 établissent le baseline gelé (libllama/libggml-cuda vérifiés par SHA, TPS=15.7983, séquence 210b7eb5…) ; les gates Q0/Q1 contrôlent la qualification numérique de la greffe de capture (transparence bit-à-bit + référence native sous budget 1e-3) et Q8 désigne la sémantique de quantification d'activation q8_1. Le verdict dominant est l'honnêteté méthodologique : V1 gagne +18% au microbench mais est rejetée au produit (−1,6%), et Q0 reste NOT_PASSED / Q1_ALLOWED=NO malgré une validation native finale à rel_l2 1.42e-07.


**Composants clés.**
- qualification/obrain-vision-mermaid/VISION-ENGINE.mmd — graphe de synthèse maître (339 lignes, 18 sphères, classes PROUVE/RETIRE/OUVERT/PARTIEL/FINAL)
- qualification/obrain-vision-mermaid/fragment_defauts.txt — registre des défauts de harnais 46→57, #58 jamais attribué
- qualification/obrain-vision-mermaid/fragment_retractations.txt — 9 rétractations N2→N4 (échelle fp16 inversée, arrondi bancaire, defaut 57)
- qualification/obrain-vision-mermaid/fragment_e004.txt — observateur ENGINE-004, transition cache_k_l3 1024→1280 à l'ordinal 224
- qualification/obrain-vision-mermaid/fragment_e005a.txt — paroi GPU MUL_MAT 86.2→91.7%, CUPTI/ncu indisponibles sur GB10
- qualification/obrain-vision-mermaid/fragment_e005f.txt — cible epinglée attn_gate.weight tq2_g128, pente murale 1.2591 ms
- qualification/obrain-vision-mermaid/fragment_e006.txt — noyau MMVQ, 42 registres/0 spill, V1 rejetée au produit
- qualification/obrain-vision-mermaid/fragment_e007a1.txt / e007a2.txt / e007a3.txt — série 007A R1→R13 (dp4a, NaN du générateur, table d'armes)
- qualification/obrain-vision-mermaid/fragment_e007b.txt — modèle cible Qwen3.8-27B-OBRAIN-v3, granularité épisode 1024, O2 +0.8268 µs
- qualification/obrain-vision-mermaid/fragment_f1a1.txt / f1a2.txt — géométrie hétérogène E5/E6, capture liée au graphe
- qualification/obrain-vision-mermaid/fragment_r1r4.txt / r5r7.txt — résidence bufferless vs soumis, SHA256 réel sur 672 payloads
- qualification/obrain-vision-mermaid/fragment_n1n2.txt / n3addr.txt / n3tern.txt / n3life.txt — chaîne numérique N1→N4, décallage GGUF 28 octets, durée de vie des captures
- docs/ENGINE-006C-R1-PRODUCT-QUALIFICATION.md — V1 rejetée au produit (gain net −1.0301 ms/token)
- docs/ENGINE-006C-R2-STATIC-V1-QUALIFICATION.md — V1 statique re-confirmée rejetée (+40 instructions SASS LOP3/SEL)
- docs/ENGINE-006C-DECODE-MICROBENCH.md — microbench décodage ternaire tq2_g128 (+18.1% local)
- docs/ENGINE-005I-GRAPH-ON-QUALIFICATION.md — qualification graph-on du dispatch (MMVQ vs MMQ)
- docs/ENGINE-007-O2-6WARP-QUALIFICATION.md — redistribution dense 6 warps, fidélité bit-à-bit
- docs/ENGINE-007A-R10-ARM-TABLE-QUALIFICATION.md — table d'armes auto-vérifiée (defaut 38)
- docs/ENGINE-007B-F1A-Q0*-QUALIFICATION.md (21 fichiers, ~35 docs Q0 au total) — R1→R7, N1→N4, passes de qualification numérique
- docs/ENGINE-007B-F1A-Q0FWD-v1-PROSPECTIVE-QUALIFICATION.md — protocole prospectif gelé (SHA 9a467c3a…), run bloqué faute de trajectoire figée
- docs/ENGINE-004-CLOTURE-RUN003.md — clôture baseline ENGINE-004, RUN-003, TPS 15.7983
- engine-004-aprime/recovery-01/…/ENGINE008_RENDERER_QUALIFICATION_REPORT.md — rapports de qualification du renderer (hors chaîne GPU)

**Relations / dépendances.**
- VISION-ENGINE.mmd importe/consolide 18 fragment_*.txt (une sphère par fichier), chacun citant ses rapports docs/ENGINE-*.md sources
- ENGINE-004 (RUN-001/002/003) → ENGINE-005 (paroi GPU) → ENGINE-006 (noyau MMVQ) → ENGINE-007A (dp4a/armes) → ENGINE-007B-F1A (capture + Q0/Q1/Q8)
- ENGINE-006C-R1 (switch runtime) → ENGINE-006C-R2 (build statique) : R2 corrige l'attribution de R1 (V1 touche 168 poids / 6 rôles, pas attn_gate seul)
- fragment_r1r4 → fragment_r5r7 → fragment_n1n2 → fragment_n3addr → fragment_n3tern → fragment_n3life → fragment_retractations (chaîne de découverte N)
- vecdotq.cuh:732/752 (vec_dot_tq2_g128_q8_1) → mmvq.cu:13 (dispatch GGML_TYPE_TQ2_G128) → ggml-quants.c:2577 (quantize_row_tq2_g128_im, codeur ternaire)
- ggml-alloc.c:690 (GGML_TENSOR_FLAG_OUTPUT) → defaut 56 (captures libérées faute de ggml_set_output) → 336 activations corrompues
- Q0 (gate numérique) → Q1_ALLOWED (autorise le domaine post-WHT) ; Q8 = sémantique q8_1 validée en N4 (défaut 57)
- RUN-003 (baseline gelé, SHA séquence 210b7eb5…) → harnais e6c-r1 (séquence locale daa762b7…) : sémantique RUN-003 NON reproduite (HARNESS_DEFECT_18)
- docs ENGINE-007B-F1A-Q0* : chaque rapport de passe porte un ledger reprenant PLUMBING_FAILURE_COUNT=28 / PLUMBING_FAILURE_29=NOT_ASSIGNED

**Points notables.**
- V1 gagne +18% au microbench ternaire mais est REJETÉE au produit : gain net −1.0301 ms/token (1,6% plus lent) ; R2 montre que V1 alourdit le noyau de +40 instructions SASS (LOP3 +16, SEL +24) — le compilateur fusionne déjà ISETP+SEL
- La campagne produit R1 prouve bit-à-bit que patch et V1 sont numériquement neutres (séquence daa762b7…, logits e61e28c6… identiques sur les 3 bras), condition nécessaire pour interpréter la performance
- Le registre des défauts de harnais s'arrête à 57 (#58 jamais attribué) et le compteur PLUMBING_FAILURE reste bloqué à 28 (#29 jamais assigné) dans toute la série
- Le même défaut de référence q8_1 est enregistré deux fois (46 en S2bis-I1, 57 en N4) avec une composante d'échelle fp16 inversée en plus — double numérotation non tranchée
- Défaut 56 : les 336 captures d'activation des deux époques étaient TOUTES corrompues (pas seulement les 25 visibles) faute de ggml_set_output/GGML_TENSOR_FLAG_OUTPUT ; rend sans valeur les reconstructions N1bis/N3quater/N3quinquies
- Défaut 57 : la référence q8_1 utilisait une échelle fp16 inversée + arrondi bancaire np.rint au lieu de roundf(xi/d) fp32 ; corrigée, elle valide 3 tenseurs à rel_l2 1.42314e-07 (marge de 4 ordres de grandeur vs budget 1e-3)
- Q0 reste NOT_PASSED et Q1_ALLOWED=NO jusqu'à la fin, même après validation native : 3 tenseurs à une seule époque ne constituent pas la population exigée, et le protocole Q0 pré-déclaré n'a jamais existé (Q0_PREDECLARED_PROTOCOL_FOUND=NO)
- Q0FWD-v1 : protocole prospectif écrit, hashé (SHA 9a467c3a…) et gelé AVANT toute mesure, mais le run est bloqué car la trajectoire teacher-forced des 9 pas n'est pas figée — exemple d'honnêteté de gel vérifiée par horodatage
- CUPTI et ncu sont indisponibles sur GB10/CUDA 13.2 : l'attribution GPU passe par paires d'événements CUDA et provenance de construction (64 ancres sur 64)
- Le lecteur GGUF direct était décalé de 28 octets (alignement 32 : fin des métadonnées 10996452 → origine alignée 10996480), ce qui produisait des échelles absurdes (3400.0) ; cause racine prouvée (défaut 53)

**Trous / incohérences.**
- harness/e6c-r1-harness.cpp, bin/e6cr1-frozen, bin/e6cr1-patched et tous les artefacts de mesure (r1/, r2/, e5i/, e7-o2/…) sont cités dans les docs mais absents du dépôt — ils vivent dans /home/triviere/exp-artifacts/engine-004-steady-state/RUN-001/ (hors git, provenance par SHA/horodatage uniquement)
- Ambiguïté RUN-001 : désigne à la fois le répertoire artefact racine engine-004-steady-state/RUN-001/ et un run baseline de la série RUN-001/002/003
- PLUMBING_FAILURE_29 et défaut de harnais #58 jamais attribués — deux numéros orphelins dans deux séries de comptage distinctes
- Double numérotation défaut 46/57 (même défaut de référence q8_1) non tranchée, et statut contradictoire du défaut 56 (RESOLVED_VERIFIED vs OPEN) dans le même fichier N3nonies
- Q0_PREDECLARED_PROTOCOL_FOUND=NO et Q0FWD_PROTOCOL_GIT_COMMIT=NOT_AVAILABLE : la qualification historique n'a pas de protocole pré-déclaré et le protocole prospectif est hors contrôle de version
- Les blocs q8_1 produits par le noyau CUDA n'ont jamais été copiés/comparés directement (NATIVE_Q8_INTERMEDIATE_COMPARISON=SOURCE_QUALIFIED_NOT_DIRECTLY_OBSERVED) : validation par la conséquence (28672 lignes), pas par observation directe
- Saillance, ablation de qualité et gain produit (tokens/s) jamais mesurés sur toute la chaîne — chaque ledger porte SALIENCY_MEASURED=NO, QUALITY_ABLATION_MEASURED=NO, PRODUCT_GAIN=NOT_MEASURED
- Provenance du quantiseur ayant produit le GGUF inconnue, aucun outil de quantification archivé (retract_gguf_encoder_provenance OUVERT)
- Classification matérielle du noyau (MEMORY_BOUND / COMPUTE_BOUND / occupancy-limited) jamais prouvée — engine-006 la déclare OUVERT
- Le writer (couple région mémoire / écrivain) qui écrasait les captures est non identifié ; R13 bloqué faute de permission compteurs matériels (RmProfilingAdminOnly=1, ERR_NVGPUCTRPERM)
- Sémantique de pré-roll RUN-003 non reproduite (HARNESS_DEFECT_18 PREROLL_STOP_CRITERION_RECONSTRUCTED_NOT_CONFIRMED) : comparaison historique avec le baseline 15.7983 TPS non applicable au harnais e6c-r1
- Aucun script de qualification ENGINE dédié trouvé sous scripts/ (les 28 occurrences de 'qualif' y sont des scripts viz/bench/enrich sans rapport) : la chaîne de qualification est documentée mais non automatisée en script



### 6.3 examples/ + docs/examples/ (exemples de l'API publique obrain)

_Fichier : `raw/v6-3-examples.json` — 33 composants · 12 relations · 11 constats · 10 trous_


**Synthèse.** La tranche examples/ contient 24 fichiers d'exemples : 20 binaires Rust (crate workspace obrain-examples) et 4 notebooks Python marimo, plus docs/examples/viz/ qui est une démo de restitution web (json-graph v1, DOT, Mermaid, three.js) plutôt qu'un exemple de langage. Les exemples Rust 'pédagogiques' (basic, transactions, parameterized, vector_search, algorithms, persistence, multi_language, social_network, knowledge_base, iot_stream) s'appuient sur l'API umbrella `obrain::ObrainDB` : session.execute() GQL, execute_with_params(), execute_cypher()/execute_sql()/execute_language(), transactions/savepoints, index HNSW et procédures CALL obrain.pagerank/connected_components/louvain/degree_centrality/betweenness_centrality/cognitive.spread. Un second groupe (bench_load, bench_head2head, bench_cognitive, bench_croise, spread_pure, poc_cognitive_store, poc_engrams, wal_merkle) descend dans les crates internes obrain_engine/obrain_substrate/obrain_cognitive/obrain_core et sert au benchmarking et aux POC cognitifs, pas à l'apprentissage de l'API publique. Les notebooks Python utilisent un paquet pip `obrain` (>=0.5.0) avec une surface API différente (db.create_node/create_edge/execute, db.algorithms.pagerank/louvain/dijkstra, db.as_networkx(), db.detailed_stats()) et des widgets marimo/anywidget-graph/anywidget-vector. Le sous-dossier docs/examples/viz illustre la chaîne `obrain viz subgraph` (export json-graph v1 → DOT → SVG → viewer HTML 2D/3D) sur un sous-graphe réel du moteur (substitute_in_operator). Le périmètre ne couvre QUE Rust et Python : aucun exemple Node/Go/C#/Dart/WASM, et aucun fichier GQL autonome (le GQL n'existe qu'en littéraux de chaîne Rust).


**Composants clés.**
- examples/rust/Cargo.toml — crate workspace obrain-examples : 19 cibles [[bin]], 6 features (default/storage/full/cognitive-full/enrichment/engram), deps vers obrain, obrain-engine, obrain-cognitive, obrain-common, obrain-core, obrain-reactive, obrain-substrate, sha2
- examples/rust/basic.rs — bin basic : ObrainDB::new_in_memory, session.execute (INSERT/MATCH/RETURN/WHERE/ORDER BY), result.iter(), .scalar(), multi-hop MATCH
- examples/rust/transactions.rs — bin transactions : begin_transaction/commit/rollback/savepoint/rollback_to_savepoint
- examples/rust/parameterized.rs — bin parameterized : execute_with_params, HashMap<String, Value>, syntaxe $name anti-injection
- examples/rust/vector_search.rs — bin vector_search : create_node/set_node_property, Value::Vector, create_vector_index (HNSW cosine), vector_search (k-NN)
- examples/rust/algorithms.rs — bin algorithms : CALL obrain.pagerank/connected_components/louvain/degree_centrality, NodeId::from, db.get_node
- examples/rust/persistence.rs — bin persistence (feature storage) : ObrainDB::open/close, WAL, note de retraite export_snapshot/import_snapshot (T17)
- examples/rust/multi_language.rs — bin multi_language (feature full) : execute_cypher, execute_sql (GRAPH_TABLE/PGQ), execute_language, node_count/edge_count
- examples/rust/social_network.rs — bin social_network : modèle cognitif simulé (energy, synapse, SET, Louvain, PageRank, betweenness_centrality, fabric simplifié)
- examples/rust/knowledge_base.rs — bin knowledge_base : renforcement synaptique simulé, recherche multi-signaux (energy×pagerank×relevance), consolidation des nœuds stales
- examples/rust/iot_stream.rs — bin iot_stream : co-change, SET ... CASE WHEN, agrégats COUNT/AVG, stagnation, betweenness, louvain, score de risque
- examples/rust/migrate_v2.rs — bin migrate-v2 (feature storage) : RETIRÉ, affiche l'erreur et exit(1), pointe vers obrain-migrate
- examples/rust/bench_load.rs — bin bench-load (feature storage) : mesure open-time, COUNT, labels(n)/type(r) — docstring .obrain périmé
- examples/rust/bench_head2head.rs — bin bench-head2head (feature storage) : Config::read_only/ObrainDB::with_config, create_text_index/text_search (BM25), 7 opérations type bench_db.c
- examples/rust/bench_cognitive.rs — bin bench-cognitive (features storage+cognitive-full) : ObrainDB (obrain_engine), cognitive_engine(), CALL obrain.cognitive.spread causal/synapse
- examples/rust/bench_croise.rs — bin bench-croise (features storage+cognitive-full) : structure réelle par base (ToolCall/PRODUCED/ToolResult vs Function/CALLS)
- examples/rust/spread_pure.rs — bin spread-pure (feature storage) : mesure du noyau spread via obrain_engine::ObrainDB + CALL obrain.cognitive.spread
- examples/rust/poc_cognitive_store.rs — bin poc-cognitive-store (features storage+cognitive-full+enrichment) : HilbertEnricher, spawn_standard_fleet, obrain_core::graph::GraphStore, SubstrateStore
- examples/rust/poc_engrams.rs — bin poc-engrams (features storage+cognitive-full+engram) : cognitive_engine().engram_manager(), record_activation/tick/recall, CALL obrain.engrams.list
- examples/rust/wal_merkle.rs — bin wal-merkle : chaîne SHA256 de racines Merkle sur le WAL substrate via obrain_substrate::wal_io::WalReader
- examples/rust/README.md — table des 7 exemples principaux + commandes cargo run (ne documente PAS les 19 binaires)
- examples/python/README.md — quick start marimo/uv, dépendances (obrain>=0.5.0, anywidget-graph, anywidget-vector, marimo, numpy, networkx, matplotlib)
- examples/python/graph_visualization.py — notebook marimo : db.create_node/create_edge, execute, result.nodes()/edges(), db.algorithms.pagerank/louvain/dijkstra, anywidget-graph
- examples/python/vector_search.py — notebook marimo : embeddings 128D, hybrid search (filtre + cosinus), PCA 3D, anywidget-vector, db.detailed_stats()/schema()
- examples/python/fraud_detection.py — notebook marimo : détection d'anneaux (cycles), mule accounts, burst, OPTIONAL MATCH/WITH, db.algorithms.pagerank, set_node_property
- examples/python/networkx_integration.py — notebook marimo : db.as_networkx(directed)/to_networkx(), nx.betweenness_centrality/clustering, matplotlib, comparaison PageRank
- docs/examples/viz/README-demo.md — sous-graphe réel substitute_in_operator (1 hop), DOT + Mermaid + commandes obrain viz subgraph
- docs/examples/viz/sub.json — json-graph v1 (5 nœuds / 5 arêtes, signaux energy/curvature/ricci/weight), centre substitute_in_operator
- docs/examples/viz/sub.svg — rendu Graphviz 16.0.0 du sous-graphe
- docs/examples/viz/viewer.html — vue interactive 2D sans dépendance (SVG, clic→voisins), mode fichier sub.json ou /api/viz/subgraph
- docs/examples/viz/viewer3d.html — vue 3D three.js (InstancedMesh, LineSegments vertexColors, picking raycast, K3/K4/hyperarêtes)
- docs/examples/viz/vendor/three.min.js — three.js minifié (603 Ko) pour viewer3d.html
- docs/examples/viz/vendor/OrbitControls.js — contrôles caméra three.js pour viewer3d.html

**Relations / dépendances.**
- examples/rust (crate obrain-examples) est membre du workspace racine (/home/triviere/projects/banc-obrain/Cargo.toml, ligne 32)
- Tous les binaires pédagogiques dépendent de l'umbrella crates/obrain (path ../../crates/obrain, features embedded/storage/tiered-storage/full) — re-export de ObrainDB, Value, NodeId, Config
- basic/transactions/parameterized/vector_search/algorithms/persistence/multi_language/social_network/knowledge_base/iot_stream/bench_load/bench_head2head importent `obrain::ObrainDB` (API publique)
- spread_pure, bench_cognitive, bench_croise, poc_engrams importent `obrain_engine::ObrainDB` (crate interne) au lieu de l'umbrella
- wal_merkle dépend de obrain_substrate::wal_io::WalReader (accès direct au WAL substrate)
- poc_cognitive_store dépend de obrain_cognitive::thinker (HilbertEnricher, spawn_standard_fleet), obrain_common::types, obrain_core::graph::GraphStore, obrain_substrate::SubstrateStore
- Les procédures GQL CALL obrain.pagerank/connected_components/louvain/degree_centrality/betweenness_centrality/cognitive.spread/engrams.list sont la surface algorithmique/cognitive publique du moteur, invoquée depuis les .rs
- Les features du Cargo.toml d'exemples délèguent : storage→obrain/storage+tiered-storage, cognitive-full→obrain-engine/cognitive-full+storage+gql+cypher, enrichment→obrain-cognitive/enrichment+thinker, engram→obrain-cognitive/engram+thinker+obrain-engine/cognitive-engram
- Les notebooks Python dépendent du paquet pip `obrain` (>=0.5.0) + marimo 0.19.x + anywidget-graph + anywidget-vector + numpy (+ networkx/matplotlib optionnels)
- docs/examples/viz/sub.json est produit par la CLI `obrain viz subgraph <store> substitute_in_operator --hops 1 --as json` (couche de restitution obrain-viz)
- viewer.html/viewer3d.html consomment sub.json (json-graph v1) ; viewer3d.html charge vendor/three.min.js + OrbitControls.js
- README-demo.md récapitule la chaîne de restitution : json-graph v1 → DOT → SVG (@viz-js/viz WASM) → Mermaid

**Points notables.**
- Surface API Python ≠ Rust : le Python expose db.create_node/create_edge/execute + db.algorithms.pagerank/louvain/dijkstra + db.as_networkx()/detailed_stats()/schema(), tandis que le Rust passe par session.execute() GQL + procédures CALL obrain.* — deux idiomes d'accès différents à la même base
- migrate_v2.rs est un binaire mort encore déclaré dans Cargo.toml : il ne fait qu'imprimer un message de retraite (T17 substrate cutover) et exit(1)
- bench_load.rs (docstring + usage '<file.obrain>') et bench_head2head.rs mentionnent encore le format mono-fichier .obrain, décommissionné par la bascule substrate — documentation périmée
- persistence.rs documente explicitement la retraite des API export_snapshot/import_snapshot (backup = cp -r/tar/rsync du répertoire)
- README.md (rust) ne liste que 7 exemples alors que Cargo.toml déclare 19 cibles [[bin]] : bench_*, poc_*, spread_pure, wal_merkle ne sont pas documentés
- 5 fichiers ont des permissions 0600 (-rw-------) au lieu de 664 : bench_cognitive.rs, bench_croise.rs, bench_head2head.rs, poc_cognitive_store.rs, poc_engrams.rs — anomalie de mode inexpliquée
- wal_merkle.rs et spread_pure.rs sont des outils de diagnostic interne (mesure du noyau spread, chaîne Merkle on-chain aletheia) et non des exemples pédagogiques d'API publique
- docs/examples/viz n'est PAS un exemple de code mais une démo de restitution web : json-graph v1 (sub.json), SVG Graphviz, viewer 2D vanilla et viewer 3D three.js optimisé (InstancedMesh, 1 draw call par couche)
- Le GQL n'existe qu'en littéraux de chaîne Rust — aucun fichier .gql autonome dans examples/
- multi_language.rs prouve le triple dialecte GQL/Cypher/SQL-PGQ (GRAPH_TABLE) + dispatch dynamique execute_language() sur la même base
- Aucun exemple Node/Go/C#/Dart/WASM alors que le contexte annonce des bindings multi-langages : seuls rust/ et python/ existent

**Trous / incohérences.**
- Absence d'exemples Node.js, Go, C#, Dart et WASM dans examples/ — le périmètre annoncé (Rust, Python, Node, GQL) n'est couvert qu'à moitié (pas de Node, pas de GQL autonome)
- Aucun fichier GQL autonome (.gql) : le GQL est noyé dans les chaînes Rust, pas de corpus d'exemples requête par requête
- migrate_v2.rs : code mort encore câblé en cible [[bin]] (required-features storage) — à retirer ou à transformer en doc
- bench_load.rs et bench_head2head.rs : références périmées au format mono-fichier .obrain (usage <file.obrain>) alors que ObrainDB::open attend désormais un répertoire substrate
- examples/rust/README.md obsolète : 7 exemples documentés vs 19 binaires déclarés, matrice de features incomplète (enrichment/engram non expliqués)
- Dérive API Python vs Rust non documentée : db.algorithms.* programmatique (Python) vs CALL obrain.* (Rust) — pas de table de correspondance des deux surfaces
- Version du paquet Python `obrain` non vérifiée (README exige >=0.5.0) : à confirmer que db.as_networkx/detailed_stats/algorithms existent bien dans le binding publié (risque de doc en avance sur le paquet)
- Anomalie de permissions 0600 sur 5 fichiers bench/poc — cause inconnue (accident de chmod ?)
- docs/examples/ ne contient qu'un sous-dossier viz/ : aucun exemple de prose/API pour Rust ou Python côté documentation
- Les binaires bench_*/poc_* utilisent des crates internes (obrain_engine, obrain_substrate, obrain_cognitive, obrain_core) sans contrat de stabilité — frontière entre 'exemple d'API publique' et 'outil interne' floue



### 6.4 Tests dans crates/ (tests d'intégration, tests unitaires #[test]/#[tokio::test]/#[rstest], fixtures, benchmarks)

_Fichier : `raw/v6-4-tests-crates.json` — 26 composants · 10 relations · 11 constats · 9 trous_


**Synthèse.** La tranche couvre l'intégralité du dispositif de test des ~24 crates Rust du dépôt obrain, soit environ 10 300 occurrences d'attributs #[test] réparties en 298 fichiers de tests d'intégration (répertoires tests/) et des centaines de modules `#[cfg(test)]` inline dans src/. Le cœur des tests est concentré dans quatre crates : obrain-engine (~3 812 #[test] dont 39 #[tokio::test], 167 fichiers tests/), obrain-substrate (~1 363 #[test], 53 fichiers), obrain-cognitive (~1 229 #[test] dont 118 #[tokio::test], 51 fichiers) et obrain-core (~1 137 #[test] dont 11 #[tokio::test], 117 modules cfg(test) inline pour seulement 2 fichiers tests/), alors que la façade obrain et obrain-selflink ne portent AUCUN test. Le framework dominant est le #[test] std de Rust, complété par #[tokio::test] pour les tests async (engine, cognitive, reactive, core, adapters), par proptest! pour la propriété-bornée (obrain-cognitive/tests/scoring_proptest.rs, et inline dans obrain-substrate/src/store.rs et simd.rs) et par criterion pour les benches (8 crates possèdent un répertoire benches/, obrain-substrate en tête avec 14 fichiers) ; en revanche rstest, insta et assert_cmd sont ABSENTS malgré la mention de rstest dans le périmètre demandé. Une signature remarquable est la suite de conformité GQL d'obrain-engine (gql_02..gql_17 + gql_spec_compliance.rs + spec_compliance.rs) gouvernée par une feature cargo `defauts-connus` qui compile délibérément des tests ROUGES documentant les écarts connus à la spécification. Les tests sont massivement rédigés en français et portent des préfixes de traçabilité (t5_/t9_/t17*, j5/j5b, seam_*, coverage_*, verif_*adversarial, contrat_*, banc_*) qui encodent des numéros d'épiques, des vagues de revue adverse et des contrats de non-régression. Les bindings multi-langages ont leurs propres harnais (pytest pour Python, vitest pour Node avec un seul spec __test__/index.spec.mjs, test natif Go, test Dart, xUnit C#) mais les bindings csharp/dart/go sont présents sur disque sans figurer dans les membres du workspace Cargo.


**Composants clés.**
- crates/obrain-engine/tests/ — 167 fichiers de tests d'intégration, la plus grosse suite du dépôt (gql_*, t5_*, t9_*, t17*, seam_*, coverage_*, verif_*, contrat_*, banc_*)
- crates/obrain-engine/tests/gql_spec_compliance.rs — conformité GQL (0.5.13) via ObrainDB::new_in_memory() + session.execute()
- crates/obrain-engine/tests/gql_02_comparaison.rs … gql_17_cycles.rs — suite de conformité GQL numérotée (comparaison, nullité, chaînes, nombres, agrégats, tri, pagination, types, écriture, temporel, chemins, cycles)
- crates/obrain-engine/tests/rev2_adversarial.rs + revue_adversariale_annulation.rs + verif_adversariale.rs — harnais de revue adverse (vagues de croisement de correctifs)
- crates/obrain-engine/tests/contrat_j5b_planificateur.rs — contrat natif de non-régression (garde-fou OBRAIN_CONTAINS_GARDE, repli scan vs index)
- crates/obrain-engine/tests/fixtures/snapshot_v1.bin — fixture binaire orpheline (jamais référencée dans un .rs)
- crates/obrain-engine/benches/ — 5 fichiers criterion (dont aretes_inter_bases_bench.rs, megalaw_bench.rs en tests/)
- crates/obrain-cognitive/tests/ — 51 fichiers (energy, synapse, scar, fabric, memory, consolidation, distillation, tenant, ldleiden, thinkers_concurrent)
- crates/obrain-cognitive/tests/scoring_proptest.rs — proptest! vérifiant energy_score/synapse_score ∈ [0,1] et monotonicité (feature energy+synapse+fabric)
- crates/obrain-substrate/tests/ — 53 fichiers (operators_*, crash_safety, wal_*, simplex, zero_copy, vector_top_k_exact, fragmentation_soak)
- crates/obrain-substrate/src/store.rs + src/simd.rs — proptest inline dans le code source (propriété-bornée embarquée)
- crates/obrain-substrate/benches/ — 14 fichiers criterion (le plus gros banc du dépôt)
- crates/obrain-core/tests/ — 2 fichiers seulement, l'essentiel étant 117 modules #[cfg(test)] inline dans src/
- crates/obrain-common/src/ — 318 #[test] répartis dans 30 modules #[cfg(test)] inline (aucun répertoire tests/)
- crates/obrain-adapters/tests/sparql_parser_tests.rs + s5_turboquant_spike.rs — parseurs GQL/Cypher/SPARQL ; ~1 229 #[test] dont 78 modules cfg(test) inline
- crates/obrain-cli/tests/ — 5 fichiers (harness_context.rs, harness_serve.rs, commands.rs, self_par_utilisateur.rs, viz_space_pagination.rs) spawnant le binaire via CARGO_BIN_EXE_obrain
- crates/obrain-cli/tests/harness_context.rs — T2 harness natif : POST in-process /api/harness/context vs contrat context_build
- crates/obrain-reactive/tests/ — 7 fichiers (bus, event, listener, mutation_bus, scheduler, store) dont 49 #[tokio::test]
- crates/obrain-coffre/tests/, obrain-iam/tests/, obrain-indexer/tests/, obrain-tls/tests/, obrain-journal/tests/, obrain-migrate/tests/, obrain-rag/tests/, obrain-viz/tests/, obrain-aveugle/tests/ — suites moyennes (2-7 fichiers chacune)
- crates/obrain-garde/src — 18 #[test] inline (crate pure sans dépendance, testée unitairement)
- crates/neo4j2obrain/tests/ + src (9 #[test]) — tests du connecteur Bolt (feature neo4j-bolt)
- crates/bindings/python/tests/ — harnais pytest (conftest.py, fixtures/generators.py, rdf/sparql + rdf/graphql, lpg/gql)
- crates/bindings/node/__test__/index.spec.mjs — unique spec vitest (package.json: "test": "vitest run")
- crates/bindings/c/src/database.rs + crates/bindings/common/src/{error,json,entity}.rs + crates/bindings/wasm/src/lib.rs — tests Rust inline des bindings (83 #[test], wasm-bindgen-test)
- crates/bindings/go/obrain_test.go, crates/bindings/dart/test/*.dart, crates/bindings/csharp/tests/Obrain.Tests — harnais natifs Go/Dart/C#
- Cargo.toml (racine) — [workspace.dependencies] inclut criterion/proptest/tempfile partagés ; members n'incluent que bindings python/node/c/wasm/common (pas csharp/dart/go)

**Relations / dépendances.**
- obrain-engine/tests/* → obrain_engine::ObrainDB (new_in_memory/open) + obrain_common::types::Value + session.execute() — l'API publique de test de bout en bout
- obrain-engine/tests → crates/obrain-adapters (parseurs GQL/Cypher/SPARQL/Gremlin/GraphQL/SQL-PGQ) via le pipeline translator→binder→optimizer→planner
- obrain-substrate/tests/operators_* → obrain-core (opérateurs physiques relocalisés T17 W4.p4, ex. operators_filter, operators_parallel_source) — feature obrain-core/regex+parallel activée en dev-dependency
- obrain-engine/tests/gql_*.rs ↔ feature cargo `defauts-connus` (crates/obrain-engine/Cargo.toml:80) — tests volontairement rouges derrière #[cfg(feature = "defauts-connus")]
- obrain-cognitive/tests/* → obrain_cognitive::{energy_score, synapse_score, effective_half_life} + features energy/synapse/fabric gating (scoring_proptest.rs: #![cfg(all(...))])
- obrain-cli/tests/* → binaire obrain via env!("CARGO_BIN_EXE_obrain") + tempfile::tempdir() — e2e HTTP/TCP sans crate assert_cmd
- criterion (benches/) réutilise les mêmes API ObrainDB/GraphStore que les tests — 8 crates avec benches/ (substrate 14, engine 5, cognitive 2, obrain 2, adapters/common/core/reactive 1)
- tempfile est le dev-dependency universel de tous les tests d'intégration (workspace.dependencies) — isolation par répertoire temporaire
- proptest partagé workspace → consommé réellement par obrain-cognitive (tests/) et obrain-substrate (src/store.rs, src/simd.rs) ; déclaré mais inutilisé dans obrain-iam
- bindings/python/tests/conftest.py → import obrain (pytest.skip si non installé) — les tests Python dépendent du wheel installé, pas du build cargo direct

**Points notables.**
- Le volume est énorme et très déséquilibré : ~10 300 #[test], mais 4 crates (engine, substrate, cognitive, core) concentrent ~75% ; la façade obrain et obrain-selflink ont 0 test, obrain-selfd seulement 2.
- rstest est mentionné dans le périmètre demandé mais ABSENT de tout le dépôt (0 occurrence) ; insta (snapshot testing) est également absent — les occurrences 'insta' sont des faux positifs (instance/instantiate/snapshot).
- La feature cargo `defauts-connus` d'obrain-engine est une pratique originale : elle compile des tests qui ÉCHOUENT délibérément pour tenir la « liste vivante » des écarts GQL à la spécification (cargo test --features defauts-connus => rouge légitime).
- Les tests sont massivement rédigés en français (noms de fichiers, doc-comments, messages d'assertion) — banc_* = micro-benchmarks vivant dans tests/, contrat_* = contrats de non-régression, verif_*adversarial = revue adverse.
- Les noms de fichiers encodent une traçabilité d'épiques : t5_* (WAL/perf), t9_* (planner rewrite), t17* (substrate/cutover), j5/j5b (orchestrateur), seam_*, coverage_* — un historique d'itérations visible dans la structure des tests.
- 20 tests #[ignore] (perf gates, benchs, outils de diagnostic comme t17j_t4_diagnose.rs, ldleiden_perf_gate.rs, prefetch_page_faults.rs).
- obrain-cli teste le vrai binaire en le spawnant (Command::new(env!("CARGO_BIN_EXE_obrain"))) pour des routes HTTP comme /api/harness/context, sans assert_cmd — une dépendance de test au binaire compilé.
- obrain-cognitive est la crate la plus async des tests : 118 #[tokio::test] (memory manager, consolidateurs, tenants, thinkers_concurrent) contre 0 pour common/substrate (sync).
- obrain-substrate embarqua proptest directement dans src/store.rs et src/simd.rs (pas seulement tests/), une propriété-bornée en ligne de code.
- Le Node binding a un seul fichier de test (__test__/index.spec.mjs) pour un package publié en 6 plateformes npm (darwin/linux/win32 × arm64/x64) — couverture très fine côté JS.
- Les bindings csharp/dart/go existent sur disque avec leurs tests natifs mais ne sont PAS membres du workspace Cargo (members = python, node, c, wasm, common) — une divergence entre fichiers et manifeste.

**Trous / incohérences.**
- crates/obrain-engine/tests/fixtures/snapshot_v1.bin — fixture binaire jamais référencée par aucun fichier .rs (code mort / orpheline).
- obrain (façade publique) n'a AUCUN test, pas même un smoke test de re-export ; obrain-selflink a 0 test et obrain-selfd 2 tests seulement.
- rstest et insta sont absents du dépôt alors que rstest est attendu par le périmètre demandé — aucune assertion de snapshot/cas paramétré n'est outillée.
- proptest est déclaré en dev-dependency d'obrain-iam mais jamais utilisé (0 occurrence proptest!/proptest:: dans obrain-iam).
- Les bindings csharp/dart/go ne sont pas dans les members du Cargo.toml racine — leur build/test échappe au `cargo test` global (testés hors workspace).
- La distinction test/bench est floue : de nombreux micro-benchmarks vivent dans tests/ (banc_*, *_bench.rs, *_perf_gate.rs, graph_benchmarks.rs) plutôt que dans benches/ criterion.
- Le Node binding n'a qu'un unique spec vitest (index.spec.mjs) — aucune couverture unitaire des APIs napi par module.
- Aucun test avec framework de snapshot de sortie (pas de .snap / golden files) malgré la volumétrie des suites GQL — les régressions de sortie sont vérifiées par assertions manuelles.
- Pas de script d'orchestration global de test dans scripts/ (seuls bench-verify, verify-wasm.sh, verif-chaine-iam.sh existent) — pas de runner unique consolidant les 298 fichiers d'intégration + bindings.



### 6.5 CI & Bench & Makefile

_Fichier : `raw/v6-5-ci-bench-makefile.json` — 25 composants · 12 relations · 9 constats · 8 trous_


**Synthèse.** Le pipeline CI principal (.github/workflows/ci.yml) orchestre 12 jobs : rust-check (fmt + clippy -D warnings + doc --all-features + build examples + validation des annotations ISO GQL), rust-test (matrice ubuntu/windows/macos, debug + release --all-features), miri (nightly-2026-02-11 épinglé sur obrain-common avec -Zmiri-strict-provenance), coverage (cargo-llvm-cov → Codecov), python-test (3 OS × Python 3.12/3.13/3.14), node-test (Node 20/22/24), go-test, csharp-test, dart-test, wasm-build (wasm32-unknown-unknown, gql seul), benchmark-pr (Criterion --save-baseline + critcmp, commentaire de PR) et supply-chain (cargo-deny --all-features). Trois profils de features sont documentés en tête du fichier : full (--all-features), WASM (default-features=false + gql) et défaut (bindings). Le harnais de bench est double : d'une part les benches Criterion internes aux crates (arena_bench/index_bench/query_bench/cognitive_bench + ~25 benches supplémentaires dont ceux d'obrain-substrate), d'autre part bench/substrate, un workspace Cargo autonome (hors du workspace principal) qui pilote les gates du RFC Substrate via des binaires dédiés (retrieval-gate, t16-validation, reverse-traversal-bench, real-retrieval-bench) et le Makefile racine qui les orchestre (bench-baseline/substrate/compare). Deux autres familles complètent le banc : bench/echelle (allocateurs, requêtes, load à échelle, en zsh) et scripts/bench-verify (re-mesure anti-régression sur stores réels). Le lint local est couvert par scripts/ci-local.sh et scripts/ci-clean.sh (variantes PowerShell ci-local.ps1/ci-clean.ps1) ainsi qu'un hook pre-push.sh (fmt + clippy) qui n'est pas installé. codecov.yml définit des seuils (projet 90%/2%, patch 90%/5%) et un suivi par composant dont les chemins sont périmés (crates/common vs crates/obrain-common). deny.toml verrouille licences, advisories, bans et sources (RUSTSEC-2025-0141 ignoré pour bincode 2.0).


**Composants clés.**
- .github/workflows/ci.yml — pipeline CI principal (12 jobs, 3 profils de features documentés en en-tête)
- .github/workflows/docs.yml — build mkdocs --strict + déploiement GitHub Pages + validation PR
- .github/workflows/pypi.yml — build/publish wheels Python (linux/musllinux/windows/macos, x86_64+aarch64) via maturin
- .github/workflows/release.yml — release multi-langages : binaires natifs, crates.io, Go, npm (node+wasm+cli), NuGet, checksums SHA256
- .github/workflows/publish.yml — publication du binding Dart sur pub.dev
- .github/workflows/retrieval-gate.yml — gate T9 (recall@10/recall@100/p95 sur 1M nœuds, N=1M) dédié à la stack retrieval substrate
- .github/workflows/update-obrain-web.yml — PR automatisée de mise à jour du repo obrain-web après release
- Makefile — orchestration bench/substrate (baseline, substrate, compare, smoke, clean) + variables PO_DB/MEGALAW_DB
- codecov.yml — seuils de couverture (projet 90%/2%, patch 90%/5%), ignore-list, suivi par composant
- deny.toml — config cargo-deny (advisories, licences allow-list, bans, sources deny)
- scripts/ci-local.sh — contrôle CI local (fmt/clippy/doc/test debug+release/python) avec mode --quick
- scripts/ci-clean.sh — CI 'propre' mimant GitHub Actions (venv neufs, 3 versions Python) avec --skip-rust/--python
- scripts/ci-local.ps1 — variante PowerShell du contrôle CI local
- scripts/ci-clean.ps1 — variante PowerShell du CI propre
- scripts/pre-push.sh — hook pre-push (blocage push upstream GrafeoDB + cargo fmt + clippy -D warnings)
- scripts/check-all.sh — smoke backend + E2E du chantier dashboard-graph (pas un check CI/lint)
- scripts/bench_boot.rs — bench de boot (replay WAL vs fichiers epoch mmap) via cargo +nightly -Zscript
- scripts/bench-compare.sh — compare deux baselines Criterion via critcmp et poste un commentaire de PR (seuil de régression)
- scripts/build-wasm.sh — build wasm-bindgen (cibles bundler, gql seul ou --features full)
- scripts/gql-conformance.py — validation des annotations // ISO: des tests contre docs/user-guide/gql/conformance.md
- bench/substrate/ — workspace Cargo autonome (Cargo.toml, src/lib.rs, bench_baseline.rs, bench_substrate.rs, bins retrieval_gate/t16_validation/reverse_traversal_bench/real_retrieval_bench, benches retrieval/activation/crud/ldleiden)
- bench/echelle/ — bancs d'échelle zsh/python (bench_allocateurs.sh, bench_requetes.sh, duel_load_allocateurs.sh, load_garde.sh, verif_index_persiste.sh, gen_xn.py) + resultats/
- scripts/bench-verify/ — batterie de re-mesure anti-régression (perf-snapshot.sh, semantic-resolution.sh, surface-tools.sh, verify-o1.mjs, verify-engine.mjs, combo-pipeline.mjs, guard.sh, metrics-report.py, ...)
- crates/obrain-common/benches/arena_bench.rs, crates/obrain-core/benches/index_bench.rs, crates/obrain-engine/benches/query_bench.rs, crates/obrain-cognitive/benches/cognitive_bench.rs — benches Criterion référencés par benchmark-pr
- crates/obrain-substrate/benches/ — ~12 benches Criterion (hks_refresh, read_path, tiered_scan, ricci_batch, geodesic, spreading_activation, effective_resistance, memoire_vs_fichier, zorder_analysis, forman_batch, heat_step, hopfield_recall, decay_all, vec_write)

**Relations / dépendances.**
- ci.yml (job benchmark-pr) → scripts/bench-compare.sh (critcmp + gh pr comment)
- ci.yml (jobs rust-check/wasm-build) → scripts/gql-conformance.py + scripts/build-wasm.sh
- ci.yml (job supply-chain) → deny.toml (EmbarkStudios/cargo-deny-action@v2 --all-features)
- ci.yml (job coverage) → codecov.yml (cargo llvm-cov --lcov → codecov-action@v5)
- Makefile (cibles bench-*) → bench/substrate/src/bench_baseline.rs et bench_substrate.rs (bins bench-baseline/bench-substrate)
- retrieval-gate.yml → bench/substrate/src/bin/retrieval_gate.rs (cargo build --release --bin retrieval-gate)
- bench/substrate/Cargo.toml → dépendances path : obrain-substrate, obrain-cognitive (feature community), obrain-core, obrain-common
- bench/substrate/src/lib.rs → définit BenchResult/Target/Backend/Measurement/RssSampler consommés par les runners et gates
- scripts/ci-local.sh + ci-clean.sh → dupliquent les commandes cargo fmt/clippy/doc/test du job rust-check/rust-test de ci.yml
- release.yml (publish-wasm) → scripts/build-wasm.sh --target bundler --features full et crates/bindings/wasm/pkg
- bench/substrate/README.md → spec des gates T9 (rss_anon_peak ≤1 GiB, open_duration ≤100 ms, recall@10 ≥99 %) reprises par le Makefile
- docs.yml → docs/requirements.txt (uv pip install) puis mkdocs build --strict

**Points notables.**
- ci.yml documente explicitement 3 profils de features en commentaire d'en-tête (full / WASM minimal / défaut bindings), et la feature 'embed' (ONNX ~17 MB) n'est jamais testée (opt-in seulement).
- Le job benchmark-pr élimine la variance matérielle en exécutant base ET PR sur le même runner (checkout du sha base puis head), et exécute le bench base EN PREMIER pour que l'échauffement thermique profite au run PR (rendant les régressions plus faciles à détecter).
- Le job miri épingle nightly-2026-02-11 à cause d'une régression cargo (rust-lang/miri#4855) et ne teste que obrain-common avec -Zmiri-strict-provenance.
- bench/substrate est un workspace Cargo AUTONOME (hors du workspace principal) pour isoler criterion/sysinfo/clap des builds de release — mais son Cargo.toml garde un commentaire NOTE obsolète affirmant que les deps path sont 'wired in T3... les runners sont des stubs', alors que obrain-substrate/obrain-cognitive/obrain-core/obrain-common sont déjà câblés.
- Le Makefile et bench/substrate portent encore l'ancien nom 'lab/grafeo'/'Grafeo', et pre-push.sh bloque tout push vers GrafeoDB/grafeo (fork → obrain) par sécurité de fuite d'information.
- scripts/check-all.sh porte un nom trompeur : ce n'est pas un check CI/lint mais un smoke + E2E du chantier dashboard-graph (vizserve, smoke-backend.sh, e2e-dashboard.mjs, endpoint 192.168.1.103:8432).
- Le job benchmark-pr garde le bench cognitif derrière un test d'existence de fichier ([ -f crates/obrain-cognitive/benches/cognitive_bench.rs ]) — il peut être silencieusement sauté sur un commit base antérieur à la création du crate.
- retrieval-gate.yml épingle rustc '1.85' (aligné sur rust-version du Cargo.toml de bench/substrate) alors que ci.yml utilise 'stable' — divergence de toolchain entre les deux workflows.
- scripts/bench-verify/ contient une batterie de re-mesure anti-régression autonome (guard.sh, perf-snapshot.sh, verify-o1.mjs) distincte du CI GitHub, orientée stores réels et machine n3.

**Trous / incohérences.**
- codecov.yml component_management pointe vers crates/common, crates/core, crates/adapters, crates/engine — chemins INEXISTANTS (les vrais crates sont crates/obrain-common, obrain-core, obrain-adapters, obrain-engine) ; le suivi par composant est donc inopérant.
- Makefile : cible bench-compare = simple stub TBD (T9) qui liste des JSON et affiche '(compare binary TBD)' — aucune sortie de gate n'est réellement produite côté Makefile (la vraie gate est dans retrieval-gate.yml/bin).
- scripts/pre-push.sh n'est PAS installé comme hook (.git/hooks/pre-push absent) — le blocage anti-upstream et le lint pre-push sont donc inactifs localement.
- scripts/ci-local.ps1 et scripts/ci-clean.ps1 existent mais aucune référence dans les docs/README du périmètre CI (scripts orphelins, vraisemblablement Windows-only).
- bench/substrate/Cargo.toml contient un commentaire NOTE périmé affirmant que les runners sont des stubs (deps 'wired in T3') alors qu'ils sont déjà câblés — documentation en retard sur le code.
- Le bench cognitif du job benchmark-pr peut être silencieusement omis (test [ -f ]) sans signaler de skip explicite dans le commentaire de PR.
- bench/substrate a un dossier target/ présent sur disque (build artifacts, fingerprints) mais non versionné (git ls-files = 0) — polluant local seulement, pas un bug VCS.
- bench/echelle/ est en zsh (#!/bin/zsh) alors que le reste du banc CI est bash — portabilité limitée, non couvert par le CI GitHub.




## Vague 7 — V7 Cognitif

### 7.1 Primitives cognitives obrain-cognitive (energy / synapse / scar / fabric / engrams)

_Fichier : `raw/v7-1-primitives-cognitives.json` — 24 composants · 10 relations · 9 constats · 7 trous_


**Synthèse.** obrain-cognitive est le crate qui matérialise « The Graph Database That Learns » : chaque sous-système cognitif implémente MutationListener (obrain-reactive) et réagit de façon asynchrone aux mutations du graphe via la MutationBus/Scheduler. Cinq primitives centrales cohabitent : l'energy (énergie nodale à décroissance exponentielle E(t)=E0·2^(-Δt/half_life), boostée à chaque mutation/requête, avec renforcement structurel par degré), la synapse (apprentissage hebbien « fire together, wire together » avec poids à décroissance exponentielle et plafond compétitif), le scar (mémoire persistante des erreurs — rollback, erreur, troncature LLM — dont l'intensité guérit exponentiellement et nourrit le risque), le fabric (métriques auto-calculées par nœud : fréquence de mutation, densité d'annotation, staleness, risk_score composite, pagerank/betweenness/community via GDS refresh) et l'engram (trace mnésique consolidée émergente, architecture biomimétique en 6 couches + homéostasie). Toutes les métriques sont persistées en write-through via des propriétés préfixées `_cog_*` (PROP_ENERGY, PROP_SYNAPSE_WEIGHT, PROP_SCAR_INTENSITY, PROP_FABRIC_RISK...). L'engram est le sommet d'abstraction : il consolide la co-activation (couche 0 Hebbian+FSRS), l'immunité, la stigmergie, le Hopfield moderne + codage prédictif, l'épigénétique, et s'expose en GQL via des procédures d'introspection (engrams.list/inspect/forget, cognitive.metrics). Le crate est strictement `#![deny(unsafe_code)]`, WASM-compatible (web-time), et découpé en 3 niveaux de features de convenance (cognitive → cognitive-fabric → cognitive-full).


**Composants clés.**
- crates/obrain-cognitive/src/lib.rs — point d'entrée, arborescence des modules feature-gated, re-exports (355 lignes)
- crates/obrain-cognitive/Cargo.toml — features energy/synapse/fabric/scar/engram/hopfield/immune/stigmergy/epigenetic + groupes de convenance
- crates/obrain-cognitive/src/energy.rs — EnergyConfig/NodeEnergy/EnergyListener/EnergyStore/energy_score/effective_half_life (872 l.)
- crates/obrain-cognitive/src/synapse.rs — Synapse/SynapseConfig/SynapseListener/SynapseStore/synapse_score/mutation_frequency_score + SynapseConsolidator (2323 l.)
- crates/obrain-cognitive/src/activation.rs — ActivationMap/ActivationSource/SpreadConfig/spread/spread_single (spreading activation)
- crates/obrain-cognitive/src/scar.rs — Scar/ScarConfig/ScarReason/ScarStore (718 l.)
- crates/obrain-cognitive/src/fabric.rs — FabricScore/FabricListener/FabricStore/RiskWeights (647 l.)
- crates/obrain-cognitive/src/gds_refresh.rs — GdsRefreshScheduler (PageRank/Betweenness/Louvain) alimentant le fabric
- crates/obrain-cognitive/src/store_trait.rs — constantes PROP_* de persistance write-through (_cog_energy, _cog_synapse_weight, _cog_scar_intensity, _cog_risk_score...)
- crates/obrain-cognitive/src/engram/mod.rs — documentation des 6 couches biomimétiques + feature flags engram (119 l.)
- crates/obrain-cognitive/src/engram/types.rs — Engram/EngramId/EpisodeId/EngramHorizon/FsrsState/PredictionError/RecallEvent (240 l.)
- crates/obrain-cognitive/src/engram/store.rs — EngramStore cache DashMap + write-through CognitiveStorage + backend substrate (engram_members/engram_bitset)
- crates/obrain-cognitive/src/engram/formation.rs — HebbianWithSurprise/FormationConfig/EngramFormationTrigger/CoActivationDetector (843 l.)
- crates/obrain-cognitive/src/engram/decay.rs — FsrsScheduler/FsrsConfig/ReviewRating (spaced repetition)
- crates/obrain-cognitive/src/engram/recall.rs — RecallEngine/WarmupSelector/ActivatedEngram/DetailLevel
- crates/obrain-cognitive/src/engram/hopfield.rs — PatternMatrix/hopfield_retrieve/max_marginal_relevance/softmax_compete/PredictiveModel (57346 o.)
- crates/obrain-cognitive/src/engram/homeostasis.rs — HomeostasisEngine/HomeostasisConfig (synaptic scaling, anti-lock-in)
- crates/obrain-cognitive/src/engram/observe.rs — CognitiveMetrics/EngramMetricsCollector/HomeostasisSignal
- crates/obrain-cognitive/src/engram/crystallization.rs — CrystallizationDetector/crystallize/generate_summary (engram → note)
- crates/obrain-cognitive/src/engram/manager.rs — EngramManager coordinateur unifié des sous-systèmes
- crates/obrain-cognitive/src/engram/position.rs + position_substrate.rs — signatures positionnelles µs (PosCentroid/PosIndex/SubstratePositionProvider)
- crates/obrain-cognitive/src/engram/spectral.rs — SpectralEncoder (signature spectrale pour similarité/Hopfield)
- crates/obrain-cognitive/src/procedures.rs — procédures GQL d'introspection (engrams.list/inspect/forget, cognitive.metrics)
- crates/obrain-cognitive/src/config.rs — CognitiveConfig/KernelConfigToml

**Relations / dépendances.**
- energy/synapse/fabric/co_change implémentent MutationListener (obrain-reactive) → réagissent aux MutationEvent du MutationBus
- activation.spread consume les synapses (poids hebbiens) pour propager l'activation entre nœuds
- scar.scar_intensity est poussé dans fabric.FabricScore via FabricStore::set_scar_intensity → pondère risk_score (poids scar=0.15 par défaut)
- fabric ← gds_refresh.GdsRefreshScheduler (pagerank, betweenness, community_id) et ← gap_detection (knowledge_gap, distinct de annotation_density)
- engram couche 0 (formation HebbianWithSurprise) prolonge le principe de co-activation des synapses, modulé par l'erreur de prédiction
- engram.store (feature substrate) miroite vers obrain-substrate : tables side-column substrate.engram_members et substrate.engram_bitset (Bloom 64 bits)
- procedures.rs interroge EngramStore/EngramMetricsCollector pour exposer engrams.list/inspect/forget et cognitive.metrics au dialecte GQL
- store_trait.rs fournit les PROP_* partagés par energy/synapse/scar/fabric pour la persistance write-through sur GraphStore
- energy.effective_half_life = base·(1+α·ln(1+degree)) — couplage structurel énergie/degré (structural_reinforcement_alpha)
- lib.rs re-exporte les primitives : EnergyListener, SynapseListener, Scar, FabricListener, EngramManager... sous leurs gates respectifs

**Points notables.**
- synapse.rs est de loin le plus gros fichier (2323 lignes, ~98 Ko) et porte un SynapseConsolidator (plan 69e59065 T3) drainant pending_metadata_writes via crossbeam::SegQueue, cadence 30s, contrainte de crash-safety aa932b40
- ScarReason encode des motifs spécifiques LLM (Truncation, Repetition, UserCorrection) — la couche cognitive sert donc aussi à mémoriser la qualité des sorties LLM, pas seulement les rollbacks BDD
- L'engram est le seul module « toujours disponible » (non gate au niveau mod), mais ses sous-systèmes restent feature-gated ; position.rs est volontairement feature-free (dépend seulement de obrain_common)
- engram.store expose trois primitives substrate additionnelles form/recall/members avec un bitset Bloom monotone-union (bits périmés = faux positifs résolus par la table members)
- Le crate est `#![deny(unsafe_code)]` et WASM-compatible (web_time::Instant/SystemTime au lieu de std::time)
- Propriétés dépréciées détectées : PROP_FABRIC_CHURN → PROP_FABRIC_MUTATION_FREQ, PROP_FABRIC_DENSITY → PROP_FABRIC_ANNOTATION_DENSITY (traces de renommage)
- engram décrit une architecture en 6 couches biomimétiques (Hebbian+FSRS, immuno, stigmergie, Hopfield+codage prédictif, épigénétique) + homéostasie — le tout coordonné par EngramManager
- engrams.forget implémente explicitement le RGPD « droit à l'oubli » (procédures.rs, avec cascade de relations)
- knowledge_gap (liens manquants, sémantiquement proches mais non connectés) est explicitement distingué de annotation_density (notes manquantes) dans fabric.rs

**Trous / incohérences.**
- Cargo.toml classe `scar` sous le commentaire `# Future subsystems` (lignes 69-70) alors que scar.rs est entièrement implémenté et re-exporté — commentaire obsolète/incohérent
- engram/mod.rs documente des features `engram-immune`/`engram-stigmergy`/`engram-hopfield`/`engram-epigenetic`/`engram-full` qui n'existent PAS dans Cargo.toml (noms réels : immune, stigmergy, hopfield, epigenetic ; pas de engram-full) — doc désynchronisée des flags
- src/thinker/kernel_enricher.rs porte des permissions 0600 (rw-------), cas unique dans le crate — à clarifier (volontaire pour kernel secret ? ou accident)
- structural_reinforcement_alpha est exposé dans EnergyConfig et effective_half_life existe, mais son branchement réel dans EnergyListener (calcul du degré au moment du decay) n'a pas été vérifié en profondeur
- Le crate est documenté « ~24 crates » mais obrain-cognitive agrège déjà >40 modules : la frontière énergie/synapse/fabric vs engram (couche 0) est conceptuellement floue (double mécanisme de co-activation)
- Aucun test d'intégration visible dans src/tests dédié au couplage scar→fabric.risk_score au niveau crate (seuls des tests unitaires internes ont été repérés)
- La procédure cognitive.metrics agrège EngramMetricsCollector mais la couverture des primitives non-engram (energy/synapse/fabric/scar) dans ces métriques GQL reste à confirmer



### 7.2 Procédures cognitives GQL — CALL obrain.cognitive.* / obrain.engrams.*

_Fichier : `raw/v7-2-procedures-cognitives.json` — 13 composants · 10 relations · 8 constats · 8 trous_


**Synthèse.** Le point d'entrée unique des procédures cognitives est try_execute_cognitive_procedure dans crates/obrain-engine/src/cognitive_procedures.rs, un dispatcher matché sur le nom résolu qui délègue à 11 exécuteurs. Sept procédures obrain.cognitive.* (spread, distill, search, detect_gaps, synthesize, reinforce, bootstrap) forment la couche « développée » : spread lit via les synapses (seule procédure cognitive prouvée lecture seule et inscrite dans PROCEDURES_DE_LECTURE), reinforce et bootstrap écrivent dans le SynapseStore, et detect_gaps/synthesize sont des primitives génériques sans forme de sortie taillée pour un consommateur. Quatre procédures de niveau 2 (obrain.engrams.list/inspect/forget et obrain.cognitive.metrics) exposent l'introspection des engrams et les métriques cognitives, déléguées à crates/obrain-cognitive/src/procedures.rs (dont le droit à l'oubli RGPD via engrams.forget). L'exécution est déclenchée par plan_call_procedure dans crates/obrain-engine/src/query/planner/lpg/mutation.rs (famille « cognitive », gatée par la feature `cognitive`), la classification lecture/écriture étant faite par procedure_mute et PROCEDURES_DE_LECTURE dans crates/obrain-engine/src/query/plan.rs. Trois UDF compagnes (obrain.energy, obrain.risk, obrain.synapses) dans cognitive_udfs.rs complètent l'exposition, enregistrées dans le PluginRegistry. Le chantier « couche de décision » (docs/ADR-2026-09-22-COUCHE-DECISION.md + scripts/couche-decision.gql) spécifie deux procédures futures — obrain.decide (lecture) et obrain.settle (écriture différée) — qui n'existent pas encore dans le code. docs/gql-dialect.json ne catalogue que les fonctionnalités génériques GQL de procédures (GP01/GP02/GP04), sans signature des obrain.cognitive.*.


**Composants clés.**
- crates/obrain-engine/src/cognitive_procedures.rs — dispatcher try_execute_cognitive_procedure + 11 exécuteurs (spread, distill, search, detect_gaps, synthesize, reinforce, bootstrap, engrams.list/inspect/forget, cognitive.metrics)
- crates/obrain-cognitive/src/procedures.rs — implémentations Level 2 des engrams (engrams_list/inspect/forget, cognitive_metrics) + yield_columns() + ProcedureResult
- crates/obrain-engine/src/cognitive_udfs.rs — UDF obrain.energy(node), obrain.risk(node), obrain.synapses(node) + register_cognitive_udfs
- crates/obrain-engine/src/query/planner/lpg/mutation.rs — plan_call_procedure : ordre de résolution des 6 familles, dispatch cognitif gaté `cognitive` (l.864-875)
- crates/obrain-engine/src/query/plan.rs — PROCEDURES_DE_LECTURE (l.320-394) et procedure_mute (l.579-605) : classification lecture/écriture des CALL
- crates/obrain-engine/tests/cognitive_query_test.rs — tests d'intégration des procédures cognitives via l'engine
- crates/obrain-engine/tests/cognitive_engram_procedures_test.rs — tests des procédures engrams
- crates/obrain-engine/tests/procedure_qui_ecrit.rs — discipline anti-vacuité sur PROCEDURES_DE_LECTURE (témoin reinforce/engrams.forget)
- docs/cognitive/engrams.md — documentation des procédures engrams.list/inspect/forget et cognitive.metrics
- docs/ADR-2026-09-22-COUCHE-DECISION.md — spécification des procédures futures obrain.decide (lecture) et obrain.settle (écriture différée)
- scripts/couche-decision.gql — projection GQL du chantier couche-decision (ancrages vers cognitive_procedures.rs, plan.rs, synapse.rs)
- crates/obrain-cli/src/mcp_server.rs — exposition des CALL obrain.cognitive.* via l'outil MCP `query`
- docs/gql-dialect.json — conformité GQL : fonctionnalités procédures GP01/GP02/GP04 (support), GP05/GP17/GP18 (non supportées)

**Relations / dépendances.**
- mutation.rs:866-875 → cognitive_procedures.rs::try_execute_cognitive_procedure (gaté `#[cfg(feature = "cognitive")]`)
- cognitive_procedures.rs → obrain_cognitive::CognitiveEngine (trait) : synapse_store(), energy_store(), fabric_store(), engram_store(), vector_index(), engram_metrics()
- cognitive_procedures.rs → obrain_adapters::plugins::AlgorithmResult (type de retour des exécuteurs)
- cognitive_procedures.rs → obrain_core::graph::GraphStore (accès structurel read-only pour spread causal, detect_gaps, synthesize, bootstrap)
- obrain-cognitive/src/procedures.rs (yield_columns) ↔ cognitive_procedures.rs (colonnes de repli) : double source de vérité pour les mêmes colonnes
- plan.rs::PROCEDURES_DE_LECTURE liste uniquement « obrain.cognitive.spread »/« cognitive.spread » comme lecture cognitive ; reinforce/engrams.forget écrivains restent exclus
- cognitive_udfs.rs → obrain_adapters::plugins::PluginRegistry (register_udf) lors de l'init de la base
- couche-decision.gql ancre les symboles réels : try_execute_cognitive_procedure (cogproc-rs-41), execute_reinforce (cogproc-rs-228), poids search 0.3/0.3/0.3/0.1 (cogproc-rs-445)
- mcp_server.rs expose obrain.cognitive.* / obrain.engrams.* via l'outil `query` (CALL inclus) côté serveur MCP
- ADR-2026-09-22-COUCHE-DECISION.md DEC-P3/P4 → obrain.decide / obrain.settle (spécifiés, non implémentés) ; DEC-C2 → PROCEDURES_DE_LECTURE plan.rs:320-351

**Points notables.**
- Onze procédures CALL effectivement dispatchées, mais la doc d'en-tête de cognitive_procedures.rs n'en liste que 5 (spread, distill, detect_gaps, synthesize, reinforce) — search, bootstrap, metrics et les 3 engrams sont omis de ce commentaire.
- Seule obrain.cognitive.spread est PROUVÉE lecture seule (bench + lecture de code) et figure dans PROCEDURES_DE_LECTURE ; toutes les autres cognitives sont sur-approximées en écriture, par conception (CognitiveEngine est un trait à mutabilité intérieure).
- obrain.cognitive.search est un squelette non câblé : vector_candidates est un Vec::new() codé en dur avec un commentaire « simulate », donc la recherche renvoie un résultat vide malgré tout l'échafaudage SearchPipeline/SearchWeights (HNSW non branché au niveau CALL).
- obrain.cognitive.bootstrap (2026-08-31) crée une synapse par arête du graphe en une passe (bootstrap_batch, mesuré 407-749 ms pour 29 994 synapses) pour rendre une base importée « cognitive » — sa doc YIELD source/target/weight est contredite par le code qui renvoie une colonne `count`.
- obrain.engrams.forget implémente le droit à l'oubli RGPD (cascade engram + signature spectrale + rappels), mais le chemin CALL n'utilise que la variante sans cascade immune (immune=None), rendant engrams_forget_with_cascade mort au niveau CALL.
- Les procédures futures obrain.decide (lecture) et obrain.settle (écriture différée par SegQueue) sont spécifiées en détail (ADR + couche-decision.gql : contraintes C2/C5/C7/C12, tests T6-T11) mais ABSENTES du dispatcher — le chantier P3/P4 est en statut todo, acceptation « non atteint ».
- obrain.cognitive.synthesize n'embarque aucun LLM (NoopSummarizer + texte structuré tronqué au budget) — choix assumé : pas de client HTTP/clé API dans le moteur de stockage.
- docs/gql-dialect.json ne référence AUCUNE signature obrain.cognitive.* : il ne couvre que la syntaxe générique des procédures (GP01/GP02/GP04 supportées, GP05/GP17/GP18 non).

**Trous / incohérences.**
- obrain.decide et obrain.settle sont référencés dans l'ADR et couche-decision.gql mais absents de cognitive_procedures.rs — non implémentés (DEC-P3/P4 en todo).
- Incohérence de nommage : l'ADR/couche-decision nomment les futures procédures obrain.decide/obrain.settle (sans segment « cognitive. »), alors que toutes les procédures cognitives existantes vivent sous obrain.cognitive.*.
- obrain.cognitive.search : vector_candidates codé en dur à vide (Vec::new(), commentaire « simulate ») — l'intégration vectorielle HNSW n'est pas branchée, la procédure renvoie 0 ligne.
- Doc d'en-tête obsolète : la signature YIELD de detect_gaps y est décrite comme « kind, source, target, score, energy, reason » alors que le code émet « kind, node_a, node_b, score, detail ».
- obrain.cognitive.bootstrap : option direction 'both' documentée mais non implémentée (seul le parcours sortant est collecté) ; résultat renvoyé = colonne unique `count`, pas source/target/weight comme documenté.
- Double source de vérité pour les colonnes YIELD : obrain-cognitive/src/procedures.rs::yield_columns duplique les listes codées en dur dans les replis « sous-système absent » de cognitive_procedures.rs (risque de divergence, le code le reconnaît déjà pour le registre).
- engrams_forget_with_cascade (cascade détecteurs immune) est défini et testé dans obrain-cognitive mais jamais atteint depuis le chemin CALL (execute_engrams_forget appelle la variante non-cascade) — cascade immune morte au niveau GQL.
- gql-dialect.json n'a pas de registre machine des signatures obrain.cognitive.* : aucun catalogue formel des arguments/YIELD de ces procédures hors commentaires Rust.



### 7.3 Sections cognitives des docs — README (Cognitive Primitives / CALL Procedures / Cognitive Procedures / UDFs), docs/cognitive/, docs/algorithms/, PROPOSAL-OPERATEUR-SPECTRAL-COURBE, continuity-spectral-spatial.md, viz-projection-densite.md

_Fichier : `raw/v7-3-docs-cognitif-algo.json` — 37 composants · 11 relations · 11 constats · 13 trous_


**Synthèse.** Le modèle cognitif documenté d'obrain repose sur quatre primitives biomimétiques à décroissance exponentielle E(t)=E0·2^(-Δt/half_life) — energy (activation des nœuds), synapse (apprentissage Hebbien « fire together, wire together » + normalisation compétitive), scar (mémoire persistante des erreurs qui nourrit le risque) et fabric (score de risque composite 0-1 pondéré pagerank/mutation/annotation_gap/betweenness/scar) — surmontées du système engram à 7 couches (Hebbian+FSRS, immunitaire, stigmergie, Hopfield moderne + codage prédictif, épigénétique, homéostasie) et d'un pipeline RAG schema-agnostic. Ces primitives sont exposées en requête via les procédures CALL obrain.cognitive.* (spread/distill/detect_gaps/synthesize), obrain.engrams.* (list/inspect/forget/metrics) et les UDF obrain.energy/risk/synapses, aux côtés de ~22 algorithmes GDS (pagerank, betweenness, louvain, leiden, dijkstra, scc, k_core, mst, max_flow...) accessibles en CALL obrain.*. Le pipeline canonique de features (canonical-features-pipeline.md) produit trois vecteurs persistés — _hilbert_features 64d (topologie), _kernel_embedding 80d (dépend de Hilbert), _st_embedding 384d (ONNX MiniLM texte) — fusionnés en CompositeEmbedding 532d pour la récupération hub/chat. L'audit PROPOSAL-OPERATEUR-SPECTRAL-COURBE (D1-D12, chaque affirmation revérifiée dans le code) renverse ce récit : l'étage cognitif est inerte (fleet de Thinkers jamais démarré, MutationBus sans source, décroissances sans appelant), la courbure persistée ricci_u8 est une formule fausse (ni Ollivier-Ricci ni Jost-Liu), l'énergie en backend substrate est un booléen (toujours 1.0), et la signature Hilbert 64d est anti-informative (AUC sous le hasard sur graphe réel). Les propositions P1-P4 refondent le modèle : un opérateur de transport courbé unique à spectre explicite (P1), une doctrine de normalisation unique (saturation + transport 1-D, P2), des contrats d'invariants/résidus/idempotence par passe (P3) et une provenance causale HLC chaînée (P4). continuity-spectral-spatial.md et viz-projection-densite.md rattachent enfin ce modèle à la reconstruction de contexte du harness (signature spectrale + localité Hilbert) et à la projection viz multi-échelle (macro/meso/micro).


**Composants clés.**
- README.md — sections « Cognitive Primitives » (energy/synapse/scar/fabric, formule risk, poids 0.25/0.25/0.20/0.15/0.15), « GDS Algorithms », « CALL Procedures Reference », « Cognitive Procedures », « Cognitive UDFs »
- docs/cognitive/overview.md — les 7 couches cognitives (mermaid Foundation→L0→L1→L2→L34→L5→Homéostasie) + table des feature flags (energy/synapse/fabric/co-change/engram/immune/stigmergy/epigenetic/cognitive-full)
- docs/cognitive/energy.md — modèle E(t)=E0·2^(-Δt/half_life), reinforcement structurel (half-life eff par degré), EnergyConfig, list_low_energy
- docs/cognitive/synapses.md — Hebbian co-activation, normalisation compétitive, spreading activation BFS (decay_factor, circuit breaker), SynapseConfig/SpreadConfig
- docs/cognitive/engrams.md — formation (CoActivation/PredictionError/HebbianWithSurprise), Hopfield recall, FSRS-5 (19 poids), crystallization, horizons (working/short/long-term)
- docs/cognitive/fabric.md — FabricScore (mutation_frequency/annotation_density/staleness/pagerank/betweenness/scar_intensity/community_id/risk_score), formule risk, RiskWeights, write-through _cog_*
- docs/cognitive/rag.md — pipeline 5 étapes (Text-to-Cues → Engram Recall → Spreading Activation → Context Building → Feedback) + presets fast/balanced/thorough
- docs/cognitive/canonical-features-pipeline.md — 3 features canoniques (_hilbert_features 64d / _kernel_embedding 80d / _st_embedding 384d), stages neo4j2obrain 1-10, CompositeEmbedding 532d
- docs/algorithms/index.md — 22 algorithmes GDS + procédures CALL, API db.algorithms(), adapter NetworkX
- docs/algorithms/centrality.md — pagerank, betweenness, closeness, degree, eigenvector
- docs/algorithms/community.md — louvain, label_propagation, connected/strongly/weakly components, triangle count
- docs/algorithms/path-finding.md — shortest_path, dijkstra, bfs/dfs, sssp (LDBC), floyd-warshall
- docs/algorithms/metrics.md — transitivity, triangle count, density, degree distribution
- docs/algorithms/link-prediction.md — common neighbors, jaccard, adamic-adar, preferential attachment (via NetworkX)
- docs/algorithms/similarity.md — jaccard/cosine/common-neighbors via NetworkX
- docs/PROPOSAL-OPERATEUR-SPECTRAL-COURBE-2026-09-10.md — audit D1-D12 (lois de dissipation, similarité aveugle, normalisation, énergie booléenne, courbure fausse, clamps, étage inerte, couplages) + propositions P1-P4 + annexes mesurées A-J
- docs/continuity-spectral-spatial.md — reconstruction spectrale (forme) + spatiale (Hilbert) du contexte au-delà de la fenêtre, lien CH6/harness, weave TOUCHES 16 889 arêtes
- docs/viz-projection-densite.md — projection multi-échelle macro/meso/micro, sens des arêtes CALLS, échantillonnage structurel, expansion delta
- crates/obrain-cognitive/src/energy.rs — EnergyStore/EnergyListener, décroissance, effective_half_life (sans appelant production)
- crates/obrain-cognitive/src/synapse.rs — SynapseStore/SynapseListener, reinforce, normalize_outgoing (borne violée 2^(Δt/half_life))
- crates/obrain-cognitive/src/scar.rs — ScarConfig/ScarStore, intensité I0·2^(-Δt/half_life), PROP_SCAR_INTENSITY
- crates/obrain-cognitive/src/fabric.rs — FabricStore/FabricListener/RiskWeights, recalculate_risk, normalisation par max global
- crates/obrain-cognitive/src/engram/ — decay.rs (FSRS-5), hopfield.rs, formation.rs, homeostasis.rs, manager.rs, spectral.rs (SpectralEncoder), crystallization.rs
- crates/obrain-cognitive/src/procedures.rs — obrain.engrams.list/inspect/forget + obrain.cognitive.metrics
- crates/obrain-cognitive/src/provenance.rs — CognitiveEvent/DerivedFromRecord, horloge murale seule, aucun appelant production
- crates/obrain-cognitive/src/thinker/ — runtime.rs, warden.rs, hilbert_enricher.rs, kernel_enricher.rs, consolidator.rs (stub), dreamer.rs, predictor.rs
- crates/obrain-cognitive/src/stigmergy.rs — phéromones, évaporation multiplicative, bruit anti-lock-in asymétrique (>>33)
- crates/obrain-cognitive/src/lib.rs — exports feature-gated (~45 modules) : energy/synapse/fabric/co_change/scar/engram/thinker/procedures/provenance...
- crates/obrain-reactive/src/scheduler.rs — dispatch concurrent des listeners via JoinSet (pas d'ordre ni d'exclusion)
- crates/obrain-adapters/src/plugins/algorithms/spectral.rs — spectral_embedding (laplacien normalisé, eigvecs, jamais de λ_k)
- crates/obrain-adapters/src/plugins/algorithms/hilbert.rs + hilbert_features.rs — cascade 64d (8 facettes × 8 ordres), facettes 0-1 « spectrales », substitution degré >10k nœuds
- crates/obrain-adapters/src/plugins/algorithms/heat_diffusion.rs — noyau de diffusion (combinaison convexe), seul opérateur respectant le principe du maximum
- crates/obrain-adapters/src/plugins/algorithms/kernel_manager.rs + kernel_math.rs — KernelManager, _kernel_embedding 80d (Φ₀)
- crates/obrain-substrate/src/geometric.rs — courbure ricci_u8 (formule 1/du+1/dv-1+T/min, fausse vs Jost-Liu), refresh_all_ricci
- crates/obrain-substrate/src/hilbert.rs — permutation physique des slots (tri centrality_cached × degré sortant), colonne hilbert jamais lue
- crates/obrain-rag/src/ — pipeline.rs, retriever.rs, feedback.rs, ranking.rs, budget.rs, context.rs (RagPipeline, CognitiveFeedback, GraphContextBuilder)
- crates/neo4j2obrain/ — CLI d'enrichissement canonique (stages 1-10), --enrich-only idempotent par stage

**Relations / dépendances.**
- README.md « Cognitive Primitives » détaille energy/synapse/scar/fabric qui sont chacun un fichier docs/cognitive/*.md (energy.md, synapses.md, fabric.md) ; scar n'a pas de doc dédiée mais un crates/obrain-cognitive/src/scar.rs
- docs/cognitive/overview.md (7 couches) → docs/cognitive/engrams.md (couches L0 Hebbian+FSRS et L3+4 Hopfield+codage prédictif) ; overview référence energy/synapses/engrams/fabric/rag en « Next Steps »
- docs/cognitive/fabric.md : risk_score ← signaux energy/synapse/scar + GdsRefreshScheduler (PageRank/Louvain/Betweenness) ; persistance write-through _cog_risk/_cog_mutation_freq/_cog_annotation_density
- docs/cognitive/rag.md : Text-to-Cues → Engram Recall (hopfield_retrieve + MMR) → Spreading Activation (synapses BFS) → Context Building → Feedback (reinforce synapses + boost energy)
- docs/cognitive/canonical-features-pipeline.md : _kernel_embedding dépend de _hilbert_features ; _st_embedding via ONNX MiniLM ; CompositeEmbedding = concat(L2 kernel 80 ⊕ hilbert 64 ⊕ st 384 ⊕ degré 4) → 532d
- docs/algorithms/* ↔ crates/obrain-adapters/src/plugins/algorithms/* : pagerank/betweenness/louvain/etc. sont des procédures CALL obrain.* implémentées côté adapters
- PROPOSAL-OPERATEUR-SPECTRAL-COURBE cite des fichiers:ligne vérifiés : energy.rs:83/390/447, synapse.rs:1019-1070 (normalize_outgoing), fabric.rs:526, geometric.rs, spectral.rs:168, hilbert_features.rs, homeostasis.rs:99-127, provenance.rs:8-10
- obrain-cognitive/src/lib.rs exporte les modules ; engine.rs enregistre les 5 listeners cognitifs ; obrain-reactive/src/scheduler.rs les dispatch concurremment (D10)
- docs/continuity-spectral-spatial.md → crates/obrain-adapters/src/plugins/algorithms/spectral.rs + crates/obrain-cognitive/src/engram/spectral.rs + hilbert_features.rs (chaîne spectral+spatial pour memory_recall/context_build)
- docs/viz-projection-densite.md → energy/synapses = densité d'intérêt, ricci<0 = bords (∂ contour) ; dépend des procédures MCP node_curvature/viz_subgraph (restitution de la courbure)
- PROPOSAL P1 s'appuie sur heat_diffusion.rs (seul noyau respectant le principe du maximum) et exige de corriger geometric.rs avant d'utiliser ricci_u8 comme conductance

**Points notables.**
- Contradiction majeure docs-vs-code : README et docs/cognitive décrivent une boucle réactive active, mais l'audit D9 (vérifié) établit que spawn_standard_fleet n'a aucun appelant hors exemple/test, MutationBus::publish n'est appelé que par des bancs, et EnergyStore::decay_all/decay_all_synapse n'ont aucun appelant de production — l'étage cognitif est latent, pas actif
- D4 : en backend substrate, l'énergie est un booléen — boost_on_mutation=1.0, clamp à max_q=min(10.0,1.0)=1.0, encodage f32_to_q1_15 → tout nœud touché vaut 1.0 pour toujours ; energy_score(1.0)=0.6321 uniforme ; la récence annoncée (search.rs) n'est pas délivrée
- D11 + Annexe C : la courbure persistée ricci_u8 n'est ni Ollivier-Ricci ni la borne Jost-Liu citée — vérifié par W₁ exacte (Kantorovich-Rubinstein) sur 11 graphes : K_n → 1-κ, arête pendante → +1/d (au lieu de 0), arbre k-régulier → κ/2 ; le seul test porte sur K₃, unique cas où la formule fausse est juste
- D2 : l'homéostasie (s ← c·s) est une similitude positive qui préserve exactement les statistiques d'échelle (Gini, max/moyenne, entropie) qu'elle est censée réguler — aveugle par construction ; et elle est en pratique un no-op (toutes les forces valent 0.5 = cible)
- D8 : normalize_outgoing calcule sa somme sur les poids décrus mais scale les poids bruts → la borne Σ ≤ max_total_outgoing_weight est violée d'un facteur 2^(Δt/half_life), vivant à chaque reinforce (contre-exemple 1024× la borne)
- A.1 : aucune valeur propre n'est calculée nulle part (grep eigenvalue/rayleigh/fiedler vide) ; spectral_embedding produit des vecteurs propres, jamais les λ_k — la « base spectrale » de P1 n'est pas gratuite
- Annexes H/I/J : la signature _hilbert_features 64d est anti-informative (AUC 0.381 scale-free, 0.405 arbre, sous le hasard 0.5) ; en production 44.4% des nœuds sont des puits → les 16 premières dims sont des rangs de degré substitués silencieusement ; des repères farthest-point 16d atteignent AUC 0.832 (+0.266)
- D12 : la couche simpliciale (SimplexRecord TRIANGLE/K4/K5/HYPEREDGE) est réelle mais jamais peuplée par l'import ; le Leiden n'implémente pas la γ-connexité (refine ignore _gamma) ; aucun Betti/homologie
- FSRS-5 : le modèle à 19 paramètres pilote l'échéancier next_review, mais la force stockée décroît par un facteur plat ×0.9 (manager.rs:331) ; et grade_factor(Hard)=2.27 > grade_factor(Easy)=0.046, l'inverse de la sémantique FSRS-5 citée
- P4 (provenance) : le module est du code mort — record() ne crée ni nœud ni arête, aucun CognitiveEvent matérialisé, horloge murale seule, zéro occurrence actor/author ; l'indexeur jette l'auteur git (let _sha = parts.next(), jamais %an/%ae)
- canonical-features-pipeline.md affirme Stages 9-10 ✓ T17l, mais la note I.5 corrige : SKIP_ON_LOAD_PROP_KEYS ne concerne plus que les bases bincode héritées depuis T16.7 (Value::Vector → veccol) — la donnée est persistée mais jamais hydratée/consommée pour le classement

**Trous / incohérences.**
- Écart de portée D9/D10 : le checkout banc-obrain n'inclut pas obrain-hub ni obrain-chat — si spawn_standard_fleet y est appelé, fleet/listeners tournent réellement là-bas et les diagnostics deviennent actifs (indéterminé depuis ce dépôt)
- docs/cognitive/scar.md absent alors que README « Cognitive Primitives » documente scar et que crates/obrain-cognitive/src/scar.rs existe (déséquilibre doc vs code)
- README « GDS Algorithms » annonce ~28 procédures mais docs/algorithms/index.md dit « All 22 algorithms » — décompte incohérent entre les deux sources (aucun inventaire unique de référence)
- Le pipeline réactif README (InstrumentedStore → MutationBus → Scheduler → Listeners) contredit D9 (bus sans source, set_post_commit_hook sans appelant) : aucun des deux ne documente l'état réellement câblé
- FSRS-5 : conformité numérique aux 19 poids jamais testée ; la doc (decay.rs:34) nomme w[14..17] « short-term stability » mais le code les utilise en facteurs de note — non réconcilié
- A.3 : la déflation spectrale est mathématiquement fausse sur graphe orienté non équilibré (d_in/d_out non constant) — base propre invalide en pratique ; non corrigé
- B.3/J.4 : _hilbert_features jamais consommé pour le classement, colonne substrate.hilbert écrite mais jamais lue ; et le remap physique des slots peut mélanger slot/id sans CRC invalide (risque ×10 déjà observé en télémétrie session-5acdcc77) — aucun test golden feature(id sémantique)
- Le seul test « locality » (hilbert_features.rs:1714-1740) est non discriminant (satisfait par la facette hash de labels, pas par la localité de graphe) — test vert ≠ propriété exercée
- AUC intra-communauté ≈ 0.5 (hasard) dans toutes les variantes et aucun test du dépôt ne la mesure — métrique d'acceptation manquante pour tout changement d'encodage spatial
- Contrôleur homeostasie : aucune garde 0 < cT < 2 ; scaling_rate sans borne de type (testé à 100 → facteur négatif, effondrement global du store) ; garde |moy-cible| < f64::EPSILON jamais déclenchée
- stigmergy : feature déclarée dans Cargo.toml mais absente de tout groupe de features ; obrain-rag active engram/hopfield/energy/synapse SANS stigmergy ; flush() sans appelant production → la persistance phéromone n'oublie jamais
- Valeurs réellement déployées des configs (EnergyConfig/HomeostasisConfig/FsrsConfig) non connues — l'audit porte sur les Default du code ; la condition cT<2 peut changer en production
- continuity-spectral-spatial.md date du 2026-09-04 et annonce un rappel ~0.48 ms « à re-mesurer sur la base full » — mesure périmée non rafraîchie



### 7.4 Recherche & rank fusion (feat/recherche-couplee, search_delta/surprise/by-role, rank fusion RRF, feat/zorder)

_Fichier : `raw/v7-4-recherche-rankfusion.json` — 18 composants · 10 relations · 7 constats · 8 trous_


**Synthèse.** La tranche « recherche & rank fusion » d'obrain se déploie sur deux axes. (1) Les « formes de recherche » GQL, livrées comme procédures LECTURE additives dans crates/obrain-engine/src/database/ : search_delta (diff CDC graphe), search_surprise (écart activité observée vs attendue par le degré), search_by_role (rôle structurel via descripteurs HKS 3 échelles), search_conflicts (reversals A→B→A dans la fenêtre CDC), search_causes/search_effects (BFS avant/inverse amorti), et le composite search_relief/obrain.relief (N/T/B/X) — documentées par 5 PROPOSAL-SEARCH-*. (2) Le rank fusion, porté par la primitive générique RRF (Reciprocal Rank Fusion, k=60 par défaut) + Weighted dans crates/obrain-core/src/index/text/fusion.rs (fuse_results/fuse_rrf/fuse_weighted), consommée par ObrainDB::hybrid_search (BM25+vecteur, crates/obrain-engine/src/database/search.rs) et par l'outil CLI obrain_recherche (chantier feat/recherche-couplee, crates/obrain-cli/src/outils.rs). Le chantier feat/zorder (ordre Z/Morton + superposition hilbert/zorder, fenêtres 2D window_query_2d) relève de la recherche spatiale de fenêtres et enrichit le même catalogue moteur, taggé v0.0.1-zorder-2026-09-04 puis fusionné dans feat/release-m6-zorder. feat/recherche-couplee est déjà absorbé dans le tronc (CONSOLIDATION-2026-09-16 : fuse_results 3×, hybrid_search 2×, RRF 4×) et le worktree banc-obrain-recherche est périmé face au tronc. Un troisième étage, SearchPipeline (crates/obrain-cognitive/src/search.rs), fusionne 4 signaux cognitifs (similarité, énergie, topologie, synapse) par somme pondérée + trait Reranker optionnel — distinct du RRF générique.


**Composants clés.**
- crates/obrain-core/src/index/text/fusion.rs — enum FusionMethod (Rrf{ k }, Weighted{ weights }) + fuse_results/fuse_rrf/fuse_weighted (RRF k=60 par défaut)
- crates/obrain-engine/src/database/search.rs — ObrainDB::text_search, text_search_federated, vector_search, hybrid_search (fusion via fuse_results)
- crates/obrain-engine/src/database/search_delta.rs — search_delta (CDC diff graphe, fenêtre chaude CdcLog, feature cdc)
- crates/obrain-engine/src/database/search_surprise.rs — search_surprise (z=(activité−attendue)/sqrt(attendue+ε) sur fenêtre CDC)
- crates/obrain-engine/src/database/search_by_role.rs — search_by_role (descripteur HKS 3 échelles + hks_distance)
- crates/obrain-engine/src/database/search_conflicts.rs — search_conflicts (reversals A→B→A par couple (nœud,propriété), CDC)
- crates/obrain-engine/src/database/search_causality.rs — search_causes/search_effects (BFS inverse/avant amorti γ=0.5)
- crates/obrain-engine/src/database/search_relief.rs — search_relief/obrain.relief (θ_N·N+θ_T·T+θ_B·B−θ_X·X, recompose surprise+conflicts+degré+recence)
- crates/obrain-cli/src/outils.rs — outil chat obrain_recherche (BM25 multi-index + fusion RRF k=60, fn recherche), tests recherche_sur_store_reel
- crates/obrain-cognitive/src/search.rs — SearchPipeline (4 signaux similarity/energy/topology/synapse, SearchWeights, trait Reranker/NoopReranker)
- crates/obrain-engine/src/query/plan.rs — PROCEDURES_DE_LECTURE (liste d'autorisation read-only : obrain.search_*/relief)
- crates/obrain-substrate/src/hilbert.rs — zorder primitives : grid_coords_from_features, zorder_key_2d, zorder_key_from_features, zorder_window_ranges
- crates/obrain-substrate/src/store.rs — SubstrateStore::window_query_2d / window_query_2d_sorted (scan exact vs voie zruns accélérée)
- crates/obrain-substrate/src/geometric.rs — compute_hks_descriptors (HKS 3 échelles), hks_distance, node_curvature (support de search_by_role)
- docs/PROPOSAL-SEARCH-SURPRISE-2026-09-05.md, PROPOSAL-SEARCH-BY-ROLE-2026-09-05.md, PROPOSAL-SEARCH-CONFLICTS-2026-09-05.md, PROPOSAL-SEARCH-CAUSES-EFFECTS-2026-09-05.md (4 propositions + pivot v1 documentés)
- docs/zorder-RAPPORT-FINAL.md, docs/CHECKPOINT-ZORDER-2026-09-04.md, docs/RELEASE-RC-M6-ZORDER-2026-09-07.md, docs/rfc/substrate/zorder-superposition.md (chantier zorder)
- worktree /home/triviere/projects/banc-obrain-recherche (branche feat/recherche-couplee, HEAD 669f626e) — commit 058d8142 ajoute obrain_recherche à outils.rs
- worktree /home/triviere/projects/banc-obrain-v0001-zorder (detached HEAD a80e3b9b) — checkpoint zorder kit de vérification

**Relations / dépendances.**
- fusion.rs::fuse_results ← appelée par engine search.rs::hybrid_search (BM25+vecteur) et par cli outils.rs::recherche (obrain_recherche, sources BM25 multi-index)
- FusionMethod::Rrf (k=60) est le défaut de FusionMethod (impl Default) ; FusionMethod::Weighted normalise min-max puis somme pondérée
- bindings : crates/bindings/python/src/database.rs (hybrid_search fusion 'rrf'|'weighted') et crates/bindings/wasm/src/lib.rs exposent le même RRF
- search_relief.rs compose compute_search_surprise + compute_search_conflicts + degrés substrate (fenêtre CDC bornée CDC_RELOAD_CAP)
- search_delta/search_surprise/search_conflicts dépendent du CDC (crate::cdc::CdcLog, ChangeEvent, changes_since_timestamp) — feature 'cdc'
- search_by_role dépend de obrain-substrate::geometric.rs (compute_hks_descriptors/hks_distance, CSR + 3 diffusions)
- dispatch GQL : query/planner/lpg/mutation.rs try_execute_*_procedure + query/plan.rs PROCEDURES_DE_LECTURE (read-only)
- obrain_recherche (outils.rs) référence le couple bridge <label>.text/.name déjà éprouvé par mcp_server.rs::search_code ; create_text_index à la demande
- zorder : hilbert.rs (zorder_key_2d, zorder_window_ranges) → store.rs window_query_2d / window_query_2d_sorted → engine ObrainDB::window_query_2d → NAPI windowQuery2d
- feat/recherche-couplee est absorbé dans le tronc (docs/CONSOLIDATION-2026-09-16.md ligne 155) ; feat/zorder taggé v0.0.1-zorder-2026-09-04 puis fusionné dans feat/release-m6-zorder (merge 50f10d9a)

**Points notables.**
- La fusion par rang RRF est la primitive UNIQUE de rank fusion, paramètre k=60 codé en dur comme défaut partout (fuse_results, hybrid_search, obrain_recherche) — aucune variante type 'reciprocal rank fusion amélioré' ni MMR n'est branchée sur la fusion hybride du moteur
- Le pivot documenté de search_surprise : la v1 « engramme interne » (FSRS/hopfield) était creuse sur données réelles → bascule vers la surprise d'ACTIVITÉ CDC (variante (a) de la proposition), mesurable et non dégénérée
- obrain_recherche fusionne les scores BM25 de plusieurs index (label,propriété) par RRF, mais n'embarque PAS d'embedder : un index vectoriel présent est seulement ANNONCÉ, jamais fusionné (pas de simulacre)
- search_by_role s'appuie sur des primitives déjà compilées/testées (HKS 3 échelles dans geometric.rs, courbure Ricci en enrichissement optionnel hors LECTURE)
- search_relief (obrain.relief) est une 6e forme composée, antérieure aux 5 propositions listées, qui réutilise surprise+conflicts+degré plutôt qu'un calcul dédié — jamais documentée par une PROPOSAL-SEARCH-RELIEF
- Le chantier zorder a été validé à l'échelle (fill Z = 41-46% du coût hkey, fenêtres 2D zruns ×1.7→×131 sous crossover ~10-50% d'aire, localité Z non dégradée vs Hilbert) et déployé comme binaire de référence harness (CHECKPOINT-ZORDER)
- La requête de obrain_recherche n'entre dans AUCUNE chaîne GQL (elle va au tokenizer BM25 tel quel) — surface anti-injection explicite, contrairement à obrain_query

**Trous / incohérences.**
- Aucun PROPOSAL-SEARCH-DELTA : search_delta a été livré en premier (c935b9d1) AVANT la série des 5 propositions — seuls le code (search_delta.rs) et les tests existent, la proposition manque
- search_relief/obrain.relief n'a pas de doc de proposition (seul PROPOSAL-IMPORT-RELIEF existe, sujet différent) — la 6e forme est orpheline côté documentation
- docs/comparisons/ ne contient AUCUN comparatif de rank fusion (seulement cypher/graphql/gremlin/sparql-vs-gql) — la doc rank fusion vit dans docs/user-guide/vector-search/index.md, docs/roadmap.md, docs/ecosystem/obrain-server.md, docs/llms.txt
- Le worktree banc-obrain-v0001-zorder est en detached HEAD a80e3b9b alors que le CHECKPOINT-ZORDER documente le tag v0.0.1-zorder-2026-09-04 (HEAD 5ae779d2) — divergence entre l'état documenté et le worktree ; aucune branche feat/zorder n'est actuellement checkoutée
- Le worktree banc-obrain-recherche (feat/recherche-couplee) est périmé : diff vs tronc = 350 fichiers, 102 565 suppressions (tronc Sep 30 vs recherche Sep 14) — branche déjà absorbée, worktree orphelin
- search_delta.rs porte un attribut #[cfg(feature = "cdc")] dupliqué sur deux lignes consécutives (l.91-92) — résidu de lint inoffensif mais signe d'édition rapide
- La voie vectorielle de obrain_recherche est morte dans le binaire CLI (feature 'embed' ONNX/HF jamais câblée au CLI) : fusion hybride BM25+vecteur inaccessible côté chat, malgré hybrid_search compilé
- Coût HKS de search_by_role annoncé ~112 ms/10⁶ arêtes par CALL (recalcul CSR + 3 diffusions à chaque appel) — borné par doc mais sans cache ni preuve de bench sur store réel dans la proposition



### 7.5 Algorithmes GDS (Graph Data Science)

_Fichier : `raw/v7-5-gds-algorithmes.json` — 22 composants · 9 relations · 8 constats · 7 trous_


**Synthèse.** La bibliothèque GDS d'obrain est documentée dans README.md (section « GDS Algorithms », lignes 234-288) avec 34 procédures CALL réparties en 6 familles (centrality, community, shortest paths, traversal/structure, clustering/similarity, network flow/MST). L'implémentation réelle est concentrée dans crates/obrain-adapters/src/plugins/algorithms/ : 26 fichiers .rs (~25 700 lignes) qui exposent à la fois des fonctions libres (pagerank, dijkstra, kruskal, articulation_points, …) et des wrappers structs implémentant le trait GraphAlgorithm (traits.rs) pour être enregistrables comme procédures. L'enregistrement CALL est fait dans crates/obrain-engine/src/procedures.rs via BuiltinProcedures::new() qui instancie ~28 Arc<dyn GraphAlgorithm>. Une seconde implémentation de Leiden (batch Traag-Waltman + variante incrémentale LDleiden) vit dans crates/obrain-cognitive/src/community/, déterministe et sans RNG, distincte du leiden de obrain-adapters. Le pont cognitif est assuré par crates/obrain-cognitive/src/gds_refresh.rs, un scheduler qui recale pagerank/betweenness/louvain et injecte les métriques dans la FabricStore (risk_score). Les bindings Python (crates/bindings/python/src/bridges/algorithms.rs, networkx.rs) et la doc docs/algorithms/ (7 fichiers .md) complètent l'écosystème. Des écarts existent entre le README et le code enregistré (procédures annoncées mais absentes du registre).


**Composants clés.**
- README.md (section GDS Algorithms lignes 234-288, tables des 34 procédures + CALL Procedures Reference lignes 290-310)
- crates/obrain-adapters/src/plugins/algorithms/mod.rs (hub de ré-export : 26 modules, ~50 pub use)
- crates/obrain-adapters/src/plugins/algorithms/traits.rs (trait GraphAlgorithm/ParallelGraphAlgorithm, Control, TraversalEvent, MinScored)
- crates/obrain-adapters/src/plugins/algorithms/centrality.rs (degree_centrality, pagerank, betweenness_centrality, closeness_centrality, hits — 1624 lignes)
- crates/obrain-adapters/src/plugins/algorithms/community.rs (label_propagation, louvain, leiden — 1792 lignes)
- crates/obrain-adapters/src/plugins/algorithms/components.rs (connected_components, strongly_connected_components, topological_sort, UnionFind)
- crates/obrain-adapters/src/plugins/algorithms/shortest_path.rs (dijkstra, dijkstra_path, astar, bellman_ford, floyd_warshall — 1265 lignes)
- crates/obrain-adapters/src/plugins/algorithms/traversal.rs (bfs, bfs_layers, bfs_with_visitor, dfs, dfs_all, dfs_with_visitor)
- crates/obrain-adapters/src/plugins/algorithms/structure.rs (articulation_points, bridges, k_core, kcore_decomposition)
- crates/obrain-adapters/src/plugins/algorithms/clustering.rs (triangle_count, clustering_coefficient, global/local_clustering_coefficient)
- crates/obrain-adapters/src/plugins/algorithms/similarity.rs (jaccard, cosine_similarity, adamic_adar, overlap_coefficient, resource_allocation, top_k_similar)
- crates/obrain-adapters/src/plugins/algorithms/mst.rs (kruskal, prim)
- crates/obrain-adapters/src/plugins/algorithms/flow.rs (max_flow Ford-Fulkerson/push-relabel, min_cost_max_flow)
- crates/obrain-adapters/src/plugins/algorithms/relevance.rs (personalized_pagerank, extract_subgraph — PPR)
- crates/obrain-adapters/src/plugins/algorithms/{spectral.rs, heat_diffusion.rs, hilbert.rs, hilbert_features.rs, kernel*.rs, contraction.rs, projection.rs, ego_graph.rs, stable_communities.rs} (extensions Rust-API : embedding, noyau, contraction, projection, ego-graph)
- crates/obrain-engine/src/procedures.rs (BuiltinProcedures::new() registre CALL obrain.*, resolve_name, output_columns_for)
- crates/obrain-cognitive/src/community/leiden.rs (leiden_batch, Graph, Partition, LeidenConfig, modularity)
- crates/obrain-cognitive/src/community/ldleiden.rs (LDleiden incrémental : on_edge_add/reweight/remove, refine_in_place)
- crates/obrain-cognitive/src/gds_refresh.rs (GdsRefreshScheduler, GdsRefreshConfig — recalage fabric feature-gated 'gds-refresh')
- crates/obrain-engine/tests/gds_integration_e2e.rs + crates/obrain-adapters/benches/gds_bench.rs (tests e2e + benches pagerank/louvain/leiden/betweenness)
- crates/bindings/python/src/bridges/algorithms.rs + crates/bindings/python/src/bridges/networkx.rs (pont Python)
- docs/algorithms/{index.md, centrality.md, community.md, path-finding.md, link-prediction.md, similarity.md, metrics.md}

**Relations / dépendances.**
- crates/obrain-engine/src/procedures.rs -> crates/obrain-adapters/src/plugins/algorithms/mod.rs : BuiltinProcedures::new() importe et enregistre les wrappers *Algorithm (PageRankAlgorithm, LeidenAlgorithm, DijkstraAlgorithm, KruskalAlgorithm, ArticulationPointsAlgorithm, …) en Arc<dyn GraphAlgorithm>
- crates/obrain-adapters/src/plugins/algorithms/mod.rs -> traits.rs : ré-export du trait GraphAlgorithm et des types communs (Control, TraversalEvent, MinScored)
- chaque fichier algorithms/*.rs -> obrain_core::graph::GraphStore : les fonctions libres prennent &dyn GraphStore comme entrée de données
- crates/obrain-cognitive/src/gds_refresh.rs -> obrain_adapters::plugins::algorithms::{pagerank, betweenness_centrality, louvain} : le scheduler recale et injecte dans FabricStore.set_gds_metrics() puis recalculate_all_risks()
- crates/obrain-cognitive/src/community/leiden.rs + ldleiden.rs : implémentation Leiden indépendante (module communautaire T10) SANS dépendance obrain-adapters, portant NodeRecord.community_id u32 via Writer::update_community_batch
- crates/bindings/python/src/bridges/algorithms.rs -> crates/obrain-adapters/src/plugins/algorithms : exposition des algorithmes aux bindings Python
- crates/obrain-engine/tests/gds_integration_e2e.rs -> projection + hits + leiden + khop + similarity : validation du pipeline GDS complet en e2e
- README.md (tables GDS) <-> crates/obrain-engine/src/procedures.rs : correspondance partielle entre les 34 procédures documentées et le registre réel
- crates/obrain-cognitive/src/fabric.rs <- gds_refresh.rs : les métriques pagerank/betweenness/community alimentent le risk_score composite (README lignes 220-232)

**Points notables.**
- A* (astar) est implémenté comme fonction générique libre `astar<H>` dans shortest_path.rs mais n'a NI wrapper struct `AstarAlgorithm` NI enregistrement dans BuiltinProcedures : la procédure `obrain.astar()` annoncée par le README (ligne 259) n'existe pas
- jaccard, cosine_similarity, adamic_adar sont des fonctions libres regroupées derrière un unique NodeSimilarityAlgorithm (nom enregistré 'similarity', paramètre metric) et non des procédures CALL séparées comme le suggère le README (lignes 278-280)
- Il existe DEUX implémentations de Leiden : adapters/community.rs::leiden (procédure CALL 'leiden') et cognitive/community/leiden.rs::leiden_batch + ldleiden.rs (LDleiden incrémental, déterministe byte-for-byte, sans RNG, calibration Newman-Girvan sur karate/football)
- La procédure k-core est enregistrée sous le nom 'kcore' (structure.rs ligne 484) et non 'k_core' comme annoncé par le README (obrain.k_core(), ligne 271)
- gds_refresh est feature-gated derrière le feature 'gds-refresh' ; sans lui, refresh_stub() ne fait rien (no-op)
- Des algorithmes entiers sont Rust-API uniquement, non exposés en CALL : spectral_embedding, heat_kernel_diffusion, hilbert/hilbert_features, kernel (IrreducibleKernel), contraction, projection, personalized_pagerank, stable_communities (seul khop est enregistré pour l'ego-graph)
- triangle_count existe en fonction libre et en colonne de sortie de clustering_coefficient, mais pas de procédure `obrain.triangle_count()` autonome
- Le module hilbert_manager a été supprimé (note T17 W2c dans mod.rs lignes 49-51) comme code mort sans appelant de production

**Trous / incohérences.**
- Écart README/code : `obrain.astar()` documenté mais aucun AstarAlgorithm n'existe (free function non enregistrée) — procédure manquante ou doc erronée
- Écart README/code : `obrain.jaccard()`, `obrain.cosine_similarity()`, `obrain.adamic_adar()` documentés comme procédures distinctes mais regroupés sous la procédure unique 'similarity'
- Écart README/code : `obrain.k_core()` vs nom réel enregistré 'kcore' (et 'kcore_decomposition' existe en plus)
- Duplication architecturale : deux implémentations Leiden (obrain-adapters vs obrain-cognitive) sans lien apparent — question de consolidation ou de divergence assumée (batch CALL vs incrémental cognitive)
- `obrain.astar()`/`obrain.triangle_count()` : fonctions implémentées mais non exposées en procédures CALL autonomes — frontière README « full GDS library » vs registre réel à réconcilier
- Le README ne liste pas les extensions réellement présentes (spectral, heat_diffusion, hilbert_features, kernel, contraction, projection, personalized_pagerank, stable_communities) — documentation GDS incomplète par rapport au code
- gds_refresh.rs dépend du feature 'gds-refresh' : vérifier si ce feature est activé par défaut dans Cargo.toml (sinon le recalage fabric est inopérant en build standard)




## Vague 8 — V8 API

### 8.1 Dialecte GQL (ISO/IEC 39075:2024)

_Fichier : `raw/v8-1-gql-dialect.json` — 23 composants · 10 relations · 12 constats · 7 trous_


**Synthèse.** Le dialecte GQL d'obrain est implémenté en deux étages : un parseur écrit à la main (aucune grammaire externe pest/nom/lalrpop/g4) dans crates/obrain-adapters/src/query/gql/ (lexer.rs 1191 lignes, parser.rs 10080 lignes/382 Ko, ast.rs 1387 lignes) et un traducteur AST→LogicalPlan dans crates/obrain-engine/src/query/translators/gql/ (mod.rs 3697 lignes + pattern.rs/expression.rs/aggregate.rs). La conformité à ISO/IEC 39075:2024 est déclarée par une matrice machine-readable docs/gql-dialect.json (151 features : 126 supported, 1 partial, 24 not_supported, seulement 30 testées), générée depuis docs/user-guide/gql/conformance.md par scripts/gql-conformance.py (modes report/dialect/validate). Le moteur accepte sept langages de requête (GQL, Cypher, Gremlin, GraphQL, SQL/PGQ, SPARQL, GraphQL-RDF) qui convergent tous vers le même IR LogicalPlan (crates/obrain-engine/src/query/plan.rs) puis le même planificateur LPG, GQL n'étant que l'un d'eux. Au-delà du standard, obrain étend GQL par des primitives cognitives appelables en CALL (obrain.cognitive.spread/distill/search/detect_gaps/synthesize/reinforce/bootstrap, obrain.engrams.*, obrain.cognitive.metrics), des UDF cognitives (obrain.energy/risk/synapses) et une batterie d'algorithmes GDS (obrain.pagerank, leiden, louvain, betweenness_centrality, dijkstra, scc, articulation_points, etc.). Le parseur supporte aussi un sucre maison V4 (`USING BASE alias AS "slug"`), la composition de requêtes (UNION/EXCEPT/INTERSECT/OTHERWISE/NEXT), les commandes de session/transaction (USE GRAPH, SESSION SET/RESET, START TRANSACTION avec niveaux d'isolation, COMMIT/ROLLBACK/SAVEPOINT), et le DDL schéma (CREATE NODE/EDGE/GRAPH TYPE, INDEX, CONSTRAINT, CREATE PROCEDURE). La pipeline d'exécution documentée dans processor.rs est parse→translate→bind→optimize→plan→execute, avec une frontière écriture/lecture fondée sur l'analyse structurelle du plan optimisé plutôt que sur le texte de la requête.


**Composants clés.**
- docs/gql-dialect.json — matrice de conformité machine-readable (151 features, generated 2026-03-14, dialect ObrainDB 0.5.22)
- docs/user-guide/gql/conformance.md — source de vérité humaine de la conformité ISO (sections Minimum Conformance, Core Language, Annex D)
- docs/user-guide/gql/ — 17 guides GQL (index, basic-queries, patterns, paths, expressions, filtering, functions-{element,numeric,string,temporal}, mutations, schema, transactions, types, composite-queries, aggregations)
- scripts/gql-conformance.py — outil de suivi conformité (report / dialect→génère gql-dialect.json / validate→CI), parse les tables Annex D et les annotations // ISO:
- crates/obrain-adapters/src/query/gql/mod.rs — point d'entrée gql::parse(query)→Result<Statement>
- crates/obrain-adapters/src/query/gql/lexer.rs (1191 lignes) — lexer manuel, enum TokenKind (mots-clés, opérateurs -> <- -- ~ |+| ? etc., ponctuation, Parameter, Eof)
- crates/obrain-adapters/src/query/gql/parser.rs (10080 lignes / 382 Ko) — parseur à descente récursive, dispatch parse_single_statement + ~60 méthodes parse_*
- crates/obrain-adapters/src/query/gql/ast.rs (1387 lignes) — AST : Statement, QueryStatement, DataModificationStatement, SchemaStatement, CallStatement, SessionCommand, CompositeOp, TransactionIsolationLevel
- crates/obrain-adapters/src/query/keywords.rs — CommonKeyword partagé entre lexers GQL/Cypher/SQL-PGQ (from_uppercase, is_keyword)
- crates/obrain-adapters/src/query/ — 6 dialectes parsés (gql/, cypher/, gremlin/, graphql/, sparql/, sql_pgq/) + ast.rs de chaque
- crates/obrain-engine/src/query/translators/gql/mod.rs (3697 lignes) — GqlTranslator AST→LogicalPlan, GqlTranslationResult (Plan/SessionCommand/SchemaCommand)
- crates/obrain-engine/src/query/translators/gql/pattern.rs (882 lignes) — traduction des motifs de chemin/étiquettes/modes de chemin
- crates/obrain-engine/src/query/translators/gql/expression.rs (352 lignes) — traduction des expressions/prédicats
- crates/obrain-engine/src/query/translators/gql/aggregate.rs (346 lignes) — traduction des agrégats/GROUP BY/HAVING
- crates/obrain-engine/src/query/translators/mod.rs — hub multi-langages (gql, cypher, sparql, gremlin, graphql, sql_pgq, graphql_rdf) vers LogicalPlan commun
- crates/obrain-engine/src/query/plan.rs (2841 lignes) — IR LogicalPlan (MatchOp, ExpandOp, CallProcedureOp, CreateNodeOp, MergeOp, ShortestPathOp, PathMode, etc.)
- crates/obrain-engine/src/query/processor.rs (1541 lignes) — QueryLanguage enum (7 langages) + pipeline parse→translate→bind→optimize→plan→execute + frontière écriture/lecture
- crates/obrain-engine/src/query/planner/lpg/*.rs (~21 Ko) — planificateur LPG (scan, expand, filter, join, aggregate, project, mutation, typed_degree_rewrite, reachable_pushdown)
- crates/obrain-engine/src/procedures.rs (909 lignes) — catalogue procédures GDS (obrain.pagerank, hits, leiden, louvain, dijkstra, scc, articulation_points, similarity, subgraph.khop…)
- crates/obrain-engine/src/cognitive_procedures.rs (1025 lignes) — CALL obrain.cognitive.* / obrain.engrams.* dispatché via try_execute_cognitive_procedure
- crates/obrain-engine/src/cognitive_udfs.rs (172 lignes) — UDF cognitives obrain.energy/obrain.risk/obrain.synapses via register_cognitive_udfs
- crates/obrain-engine/tests/gql_spec_compliance.rs (69 annotations // ISO:) + 18 tests gql_*.rs / spec_compliance.rs — conformance par tests
- docs/PROPOSAL-CDC-GQL-2026-09-05.md — proposition liant CDC et GQL

**Relations / dépendances.**
- scripts/gql-conformance.py (mode dialect) → génère docs/gql-dialect.json depuis docs/user-guide/gql/conformance.md (tables Annex D + annotations // ISO: des tests)
- gql::parse (obrain-adapters/src/query/gql/mod.rs) → Parser::parse (parser.rs) → Statement/AST (ast.rs), via Lexer (lexer.rs) qui consomme CommonKeyword (keywords.rs)
- obrain-engine translate_full (translators/gql/mod.rs) → GqlTranslator → LogicalPlan (plan.rs), en s'appuyant sur pattern.rs/expression.rs/aggregate.rs
- processor.rs process() → translate_lpg() → Binder → Optimizer → Planner (planner/lpg/) → exécution ; la frontière mute/lecture lit optimized_plan.root.mutations_avec(catalog)
- translators/mod.rs regroupe gql/cypher/sparql/gremlin/graphql/sql_pgq/graphql_rdf : tous produisent le même LogicalPlan, GQL n'est qu'un dialecte parmi 7
- CALL procédure → CallStatement (parser.rs parse_call_statement) → CallProcedureOp (translators/gql/mod.rs) → procedures.rs (GDS) ou cognitive_procedures.rs (obrain.cognitive.*)
- cognitive_udfs.rs register_cognitive_udfs → PluginRegistry (obrain-adapters) → UDF obrain.energy/risk/synapses consommables dans les expressions GQL
- planner/lpg/mutation.rs:867 appelle crate::cognitive_procedures::try_execute_cognitive_procedure pendant la planification des mutations (reinforce/bootstrap en ligne)
- docs/user-guide/gql/*.md ↔ conformance.md ↔ gql-dialect.json ↔ scripts/gql-conformance.py forment la boucle de documentation/conformité/vérification CI
- obrain-engine Cargo.toml feature gql (ainsi que obrain, obrain-cli, obrain-indexer, obrain-adapters) gâtent le module gql derrière un feature flag

**Points notables.**
- Le parseur GQL est un descente-récursive entièrement manuscrit de 10 080 lignes (382 Ko) — aucune grammaire générée (pas de pest/nom/lalrpop/antlr .g4 dans tout crates/)
- Conformité déclarée : 126/151 features supportées, 1 partial, 24 non supportées — mais seules 30 sont couvertes par des tests (ratio testé/supporté ≈ 24%)
- G048 « Parenthesized path: subpath variable declaration » est l'unique feature 'partial' ET apparaît aussi dans la liste 'tested' — double-statut apparent dans la matrice
- Extensions cognitives au dialecte : CALL obrain.cognitive.{spread,distill,search,detect_gaps,synthesize,reinforce,bootstrap} + obrain.engrams.{list,inspect,forget} + obrain.cognitive.metrics
- UDF cognitives obrain.energy(node), obrain.risk(node) (churn×pagerank×knowledge_gap×betweenness), obrain.synapses(node) (liste de maps Hebbiennes) via obrain-cognitive
- Sucre maison V4 dans le parseur : préambule `USING BASE alias AS "slug"` résolu dans ref(alias,…) et IN alias (parser.rs base_aliases, correctif parseur-queue)
- Composition de requêtes étendue au-delà du standard : UNION [ALL|DISTINCT], EXCEPT [ALL], INTERSECT [ALL], OTHERWISE (fallback si gauche vide) et NEXT (composition linéaire)
- Support transactionnel ISO Section 19 : START TRANSACTION [READ ONLY|READ WRITE] [ISOLATION LEVEL ReadCommitted|SnapshotIsolation|Serializable], SAVEPOINT/ROLLBACK TO/RELEASE
- Frontière écriture/lecture fondée sur l'analyse structurelle du plan optimisé (mutations_avec), corrigeant l'ancienne heuristique textuelle (upper.contains("SET")) — cf. commentaire processor.rs:373-394
- correctif parseur-queue : fermeture de classe du parseur — l'entrée doit être intégralement consommée, les ';' finaux tolérés comme jetons Error (pas de TokenKind dédié au terminateur)
- Le terminateur ';' n'a pas de jeton lexer propre : il est traité comme TokenKind::Error avec texte ';' puis sauté (commentaire parser.rs:337-344)
- Catalogue GDS riche exposé en CALL : 28+ algorithmes (pagerank, hits, betweenness/closeness/degree_centrality, bfs/dfs, connected_components, scc, dijkstra/sssp, bellman_ford, floyd_warshall, clustering_coefficient, label_propagation, louvain/leiden, kruskal/prim, max_flow, articulation_points, bridges, k_core, similarity.topk, subgraph.khop)

**Trous / incohérences.**
- Seules 30 des 126 features supportées ont un test annoté // ISO: (gql_spec_compliance.rs) — 96 features supportées sans couverture de test déclarée
- 24 features not_supported incluent des éléments structurels majeurs : GE01/GE02 (graph/binding table reference value expressions), GE04/GE05 (graph/binding table parameters), GV45 (record types), GV60/GV61/GV65 (graph reference & binding table reference & dynamic union types), GE09 (horizontal aggregation), GT03 (multi-graphes dans une transaction), GP17/GP18 (binding variable definition block, catalog/data mixing)
- Incohérence de statut : G048 listé à la fois comme 'partial' et 'tested' — la matrice n'explique pas comment une feature partielle est « testée »
- Le traducteur graphql est un fichier unique (translators/graphql.rs) alors que gql/ est un répertoire multi-fichiers — asymétrie structurelle non documentée
- Les messages d'erreur du dispatch parse_single_statement sont incohérents : la branche identifiant mentionne SAVEPOINT, la branche non-identifiant l'omet (parser.rs:458-467)
- docs/gql-dialect.json est figé au 2026-03-14 (dialect 0.5.22) : la version semble périmée par rapport au HEAD 456d3794, l'outil de regénération n'a pas de garantie d'être exécuté en CI à chaque commit
- Le worktree perf/gql-projection (banc-obrain-gql, cbab7bb0) suggère un chantier d'optimisation de projection GQL encore non fusionné dans la lignée principale



### 8.2 HTTP/REST

_Fichier : `raw/v8-2-http-rest.json` — 18 composants · 10 relations · 9 constats · 7 trous_


**Synthèse.** La couche HTTP/REST d'obrain est entièrement écrite à la main en std::net (aucun framework, aucune dépendance réseau : ni tokio, ni hyper, ni TLS côté serveur), autour d'un serveur HTTP/1.1 monolithique de ~9 637 lignes, crates/obrain-cli/src/viz_serve.rs, qui sert le dashboard et la quasi-totalité des routes /api/* sur :8432. Un second serveur minimal en lecture seule, crates/obrain-cli/src/harness_serve.rs (233 lignes), sert les contrats /api/harness/* sur :8090 en in-process. Le crate obrain-httpmin est un CLIENT HTTP minimal (std uniquement, ~560 lignes) partagé par selflink (qui interroge le moteur) et selfd (qui interroge un serveur OpenAI-compatible), né pour supprimer deux copies divergentes ; il descend de crates/obrain-cli/src/http_min.rs. Le routage est un unique match sur le chemin (lignes 3131-3252 de viz_serve.rs) couvrant statiques (/app, /viewer2d, /login, /iam, /sitemap.xml, /__dev/version), la lecture (/api/query, /api/functions, /api/info, /api/stores), la projection viz (/api/viz/{status,overview,modules,signatures,orbites,arcs,ramifications,space,weave,activation,subgraph,subgraph/stream}), les mondes, le moteur (/api/engine/{sante,memoire,selfs,resonance,entretien,ask,chat}), l'authentification (/api/auth/*), l'administration IAM (~17 sous-routes /api/iam/*) et le canal machine /api/remote/*. Le modèle d'exécution est un thread par requête avec Connection: close, plus compression gzip (NATIF-J7) et un upgrade WebSocket pour /api/viz/subgraph/stream. La garde d'accès GardeIam (viz_serve.rs) enveloppe obrain_iam::Garde pour un point de décision unique partagé entre serveur HTTP, pont MCP et CLI. L'audit 2026-09-15 atteste 27 endpoints sondés, zéro route morte, et 4 points chauds > 100 ms ; le script scripts/endpoints-check.mjs rejoue ce balayage à froid puis à chaud avec gestion explicite SSE, identité et heuristique « error ».


**Composants clés.**
- crates/obrain-httpmin/src/lib.rs — client HTTP minimal std-only (parse_url, echapper, get, post_json, lire_sse, post_sse_supervise)
- crates/obrain-httpmin/Cargo.toml — dépendances : anyhow + serde_json uniquement ; « CE CRATE EXISTE POUR NE PAS AVOIR DEUX CLIENTS »
- crates/obrain-cli/src/viz_serve.rs — serveur HTTP/1.1 principal :8432 (9 637 lignes), routage lignes 3131-3252, run_serve ligne 1327
- crates/obrain-cli/src/harness_serve.rs — daemon harness natif :8090 lecture seule, routes /api/harness/health|sessions|context
- crates/obrain-cli/src/http_min.rs — ancêtre du client httpmin (~120 lignes), « obrain-cli n'embarque aucune dépendance HTTP »
- crates/obrain-cli/src/main.rs — câblage CLI : mod viz_serve / mod harness_serve, Commands::Viz → viz::run, Commands::HarnessServe → harness_serve::run
- crates/obrain-cli/src/mcp_server.rs — partage GardeIam (etat/est_armee) et fonctions sessions_list / context_build appelées par harness_serve
- crates/obrain-selfd/src/ecoute.rs — surface HTTP du nœud selfd (page + /api/status, /api/chat), jeton obligatoire, comparaison en temps constant
- crates/obrain-selfd/src/chat.rs — consommateur obrain-httpmin (chat OpenAI-compatible)
- crates/obrain-selflink/src/engine.rs — consommateur obrain-httpmin (interroge le moteur via /api/*)
- crates/obrain-garde/src/garde.rs — garde de classification lecture/écriture réutilisée par selfd/ecoute
- crates/obrain-tls/examples/serveur_tls.rs — serveur TLS témoin étape 0, même dessin que viz_serve.rs (servir/traiter sur S: Read+Write)
- docs/audit-endpoints-2026-09-15.md — inventaire 27 endpoints sondés, mesures froid/chaud, verdict zéro route morte
- scripts/endpoints-check.mjs — balayage complet des endpoints (verdict OK/VIDE/ERREUR/ABSENT/LENT/SKIP, SSE, identité, statiques)
- docs/api/index.md — référence API multi-langages (Python/Node/Go/C#/Dart/WASM/Rust), PAS la doc HTTP/REST
- docs/perf/ENDPOINTS.md — coûts consolidés des endpoints (froid/médiane/max, cache par (store,centre,sauts,limite))
- docs/dashboard-viz/perimeter-8432.md — inventaire des endpoints :8432 issu du code + mesures à chaud (contrat ≤ ~1 ms)
- packages/obrain-dashboard/src — client SPA (React/Vite) qui consomme /api/viz/*, /api/stores, /api/query

**Relations / dépendances.**
- viz_serve.rs (serveur) ↔ http_min.rs / obrain-httpmin (client) : symétrie std::net ; le serveur répond Content-Length ou chunked, le client décode les deux
- obrain-httpmin ← obrain-cli/src/http_min.rs : refactorisation/partage, élimine deux clients divergents (selflink et selfd en avaient chacun une copie)
- obrain-selflink/src/engine.rs → obrain-httpmin → moteur (self, mémoire, requêtes, conversation)
- obrain-selfd/src/chat.rs (et agent.rs, soi.rs, attache.rs) → obrain-httpmin → serveur OpenAI-compatible (llama-server :9901, POST /v1/chat/completions)
- viz_serve.rs::GardeIam → obrain_iam::Garde : enveloppe mince, un seul endroit décide (HTTP + pont MCP + CLI), feature-gated
- harness_serve.rs → ObrainDB::open_read_only (verrou partagé) → mcp_server::sessions_list / mcp_server::context_build (in-process, sans respawn de bridge)
- main.rs → viz_serve::run_serve (TcpListener + un thread par requête) et harness_serve::run (TcpListener + boucle séquentielle)
- scripts/endpoints-check.mjs → BASE par défaut http://192.168.1.103:8432 → sonde viz_serve.rs ; identité via OBRAIN_TOKEN/OBRAIN_COOKIE
- packages/obrain-dashboard/src → /api/stores, /api/query, /api/functions, /api/viz/{overview,modules,arcs,space,subgraph,status}
- obrain-tls/examples/serveur_tls.rs → contrat d'intégration de run_serve : poignée de main TLS puis même boucle traiter(S: Read+Write)

**Points notables.**
- Zéro framework HTTP : tout est std::net/TcpStream écrit à la main (viz_serve, harness_serve, http_min, httpmin, ecoute, serveur_tls) — le parti pris « pas de dépendance réseau pour joindre un service local » est tenu jusqu'au serveur
- docs/api/ ne documente PAS l'API HTTP/REST : c'est la référence des bindings multi-langages (Python/Node/Go/C#/Dart/WASM/Rust) ; les endpoints HTTP vivent dans docs/dashboard-viz/perimeter-8432.md et docs/perf/ENDPOINTS.md
- Modèle un thread par requête avec Connection: close (pas de keep-alive, pas de pool) ; la compression gzip (NATIF-J7) cible le coût dominant = transfert de payload JSON (~30x)
- L'audit 2026-09-15 conclut « zéro route morte » sur 27 endpoints sondés et identifie 4 points chauds > 100 ms : /api/functions (758 ms, trigrammes à la demande), /api/viz/space (1 513 ms, projection par appel), /api/viz/weave (144 ms), /api/harness/context (110 ms vs contrat 50 ms)
- Faille de traversal /app/* fermée et documentée : chemin décodé + assaini via chemin_sous, réponse 404 (pas 403) pour ne pas confirmer l'existence d'un fichier hors dossier
- /api/viz/subgraph/stream est un WebSocket (upgrade) négocié, non couvert par la sonde HTTP paramétrée (contrat testé ailleurs, handshake 0,9 ms)
- La garde IAM (GardeIam) a été écrasée le 2026-09-13 par une réécriture concurrente de viz_serve.rs puis reposée — leçon notée sur les fichiers partagés qui peuvent disparaître
- harness_serve est volontairement lecture seule (mode:ro) en T1, coexistant avec l'écrivain dsh sans violer l'invariant WAL
- Le serveur selfd refuse de démarrer son écouteur HTTP sans jeton (--token / $SELFD_TOKEN) et compare le jeton en temps constant ; écoute 127.0.0.1 par défaut

**Trous / incohérences.**
- docs/api/ (périmètre nommé) ne couvre pas les endpoints HTTP/REST — mismatch de périmètre ; l'inventaire HTTP n'a pas de doc API dédiée consolidée
- Pas de liste d'endpoints unique et exhaustive côté code : le match de routage (viz_serve.rs ~40+ routes) déborde la liste de scripts/endpoints-check.mjs (27) — /api/viz/orbites, /api/viz/ramifications, /api/engine/selfs, /api/engine/resonance, /api/auth/audit, /api/iam/* (17) et /api/remote/* (6) ne sont pas sondées par le script
- scripts/endpoints-check.mjs durcit BASE (192.168.1.103:8432) et STORE (/home/triviere/.local/share/obrain-harness/data/runtime-dev.obrain) — dépendance machine-spécifique
- Aucun TLS dans le serveur réel (seulement l'exemple témoin obrain-tls) ; le gateway :8144 mentionné dans l'audit (IAM armé) n'a pas été localisé dans cette tranche
- Asymétrie SSE : /api/engine/chat en GET (contrainte EventSource) côté serveur, mais selfd consomme le modèle en POST SSE (/v1/chat/completions) — deux conventions de flux coexistent
- harness_serve est figé en T1 lecture seule : pas de routes d'écriture, le chantier T2/NS (index d'égalité, /api/harness/context < 50 ms) est signalé non résolu dans le code inspecté
- Code mort potentiel : le client historique http_min.rs coexiste encore avec obrain-httpmin dans obrain-cli, sans qu'il soit établi ici s'il est encore utilisé ailleurs que comme ancêtre



### 8.3 Transport WebSocket + modèle réactif (obrain-reactive, feat/transport-ws-j9)

_Fichier : `raw/v8-3-websocket-reactive.json` — 20 composants · 9 relations · 8 constats · 8 trous_


**Synthèse.** obrain comporte DEUX notions de « push » disjointes. (1) Le substrat réactif en-process, la crate obrain-reactive (crates/obrain-reactive/), qui transforme chaque mutation de graphe en événements publiés sur un tokio::broadcast : InstrumentedStore (wrapper transparent de GraphStoreMut) tamponne les MutationEvent, le commit hook les draine en MutationBatch, le MutationBus les diffuse, le Scheduler les agrège par BatchConfig (100 events / 50 ms) et les dispatche en parallèle (JoinSet) vers des MutationListener. Les consommateurs réels sont les sous-systèmes cognitifs de obrain-cognitive (EnergyListener, SynapseListener, FabricListener, CoChangeDetector, KernelListener, StigmergicMutationListener, EpisodeRecorder), câblés par CognitiveEngineBuilder::build(scheduler) depuis obrain-engine/database/mod.rs. (2) Le transport WebSocket, porté par la branche feat/transport-ws-j9 (worktree /home/triviere/projects/banc-obrain-j9), qui streame le subgraph du viz par CHUNKS au lieu d'un corps JSON unique : primitives RFC6455 std-only maison dans crates/obrain-cli/src/viz_ws.rs (SHA-1 + base64 inline, zéro dépendance nouvelle, clause anti-enlisement), endpoint /api/viz/subgraph/stream dans viz_serve.rs (frames begin/nodes*/edges*/end + ordre par importance structurelle), mode live/deltas piloté par l'époque = mtime du store (sondage 400 ms), et client dashboard (api.ts) avec repli HTTP transparent. Les deux mondes ne communiquent PAS : le live WS poll le mtime du fichier au lieu de s'abonner au MutationBus. La Phase C protobuf est différée (prost absent de Cargo.lock).


**Composants clés.**
- crates/obrain-reactive/src/lib.rs — façade : doc d'architecture (InstrumentedStore→MutationBus→Scheduler→Listeners), exports, deny(unsafe_code)
- crates/obrain-reactive/src/bus.rs — MutationBus : wrapper tokio::broadcast (capacité 16 384), publish/publish_batch/subscribe, compteurs atomiques, zéro-coût sans abonné
- crates/obrain-reactive/src/event.rs — MutationEvent (NodeCreated/Updated/Deleted, EdgeCreated/Updated/Deleted), NodeSnapshot/EdgeSnapshot, MutationBatch, EventContext (tenant/principal/session, from_wami)
- crates/obrain-reactive/src/store.rs — InstrumentedStore<S: GraphStoreMut> : intercepte les écritures, tamponne les événements, drain_pending/clear_pending
- crates/obrain-reactive/src/scheduler.rs — Scheduler natif (tokio::task + batching + dispatch concurrent JoinSet) vs WASM (synchrone), BatchConfig
- crates/obrain-reactive/src/listener.rs — trait MutationListener (on_event/on_batch/accepts/accepts_context) + TenantFilteredListener
- crates/obrain-reactive/src/error.rs — ReactiveError (BusCapacityExceeded, ListenerError, SendError) + Result
- crates/obrain-reactive/tests/ + benches/bus_bench.rs — suites event/scheduler/listener/mutation_bus/store/bus_extra + bench
- crates/obrain-engine/src/database/mod.rs — crée MutationBus + Scheduler, construit le CognitiveEngine avec le scheduler (feature cognitive)
- crates/obrain-engine/src/session/mod.rs — post_commit_hook (ligne ~3806) + set_post_commit_hook (~573), point d'injection réactif
- crates/obrain-cognitive/src/engine.rs — CognitiveEngineBuilder::build : register_listener Energy/Synapse/Fabric/CoChange/Kernel (~542-670)
- crates/obrain-cognitive/src/{energy,synapse,fabric,co_change,kernel,stigmergy,episodic}.rs — implémenteurs de MutationListener (boost énergie, co-activation, métriques fabric, détection co-change, embeddings kernel, stigmergie, épisodes)
- crates/obrain-cli/src/viz_ws.rs — primitives WS std-only : sha1/base64 inline, is_websocket_upgrade, handshake 101, send_text/send_close, vecteurs RFC6455
- crates/obrain-cli/src/viz_serve.rs — api_subgraph_stream, prepare_subgraph, stream_subgraph, importance_order, compute_delta, live_loop, envoyer_delta (constantes WS_CHUNK/WS_LIVE_POLL/WS_HEARTBEAT/WS_LIVE_MAX/WS_RESNAPSHOT_DELTA_MAX)
- crates/obrain-cli/src/main.rs — mod viz_ws / viz_serve / viz_extract / viz_weave
- packages/obrain-dashboard/src/lib/api.ts — streamSubgraph + fetchSubgraph (repli HTTP), parse des frames begin/nodes/edges/end, wsUrl ws/wss
- docs/plan-j9.md — NATIF-J9 : doctrine, Phases A/B/C, décision archi (WS maison std-only, identité par construction, repli)
- docs/plan-j9-protobuf.md — Phase C différée : schéma .proto Frame/Node/Edge/Delta, négociation Sec-WebSocket-Protocol/enc=pb
- scripts/guard-msgpath.sh — garde DoD du chemin message, délègue à scripts/msgpath_guard.py (copie jetable, PASS/FAIL)
- scripts/msgpath_guard.py — rejoue bridge MCP/spawn-court sur copie store, gates ouverture propre/sale, runtime_search chaud, intégrité

**Relations / dépendances.**
- obrain-reactive → obrain-common (types NodeId/EdgeId/PropertyKey/Value) et obrain-core (traits GraphStore/GraphStoreMut, graph::lpg::Node/Edge) via Cargo.toml
- obrain-engine (features reactive/cognitive, dep optionnelle obrain-reactive) → crée MutationBus + Scheduler et passe le scheduler au CognitiveEngineBuilder (database/mod.rs ~766 et ~1059)
- obrain-engine/session commit → post_commit_hook(&dyn GraphStoreMut) → censé drainer les événements de l'InstrumentedStore (doc, feature=reactive)
- obrain-cognitive (dep obrain-reactive) → implémente MutationListener (7 listeners) et les enregistre via CognitiveEngineBuilder::build(&scheduler) (engine.rs ~542-670)
- obrain-cli/viz_ws → obrain_viz (Subgraph::canonical/stats, JSON_GRAPH_SCHEMA_V1) via subgraph_for_stream/stream_subgraph ; aucune dépendance tokio-tungstenite/tungstenite
- obrain-cli/viz_serve route GET /api/viz/subgraph/stream → viz_ws::handshake → stream_subgraph (snapshot) puis live_loop (mode live=1)
- packages/obrain-dashboard api.ts → endpoint /api/viz/subgraph/stream (WebSocket) avec repli fetch('/api/viz/subgraph') HTTP+gzip
- scripts/guard-msgpath.sh → python3 scripts/msgpath_guard.py (référence bench/GUARD-SPEC.md, absente)
- compute_delta (viz_serve) réimplémente la diff node/edge localement au lieu d'appeler obrain_viz::diff_subgraphs (diff.rs), pourtant mentionné comme réutilisable dans plan-j9.md

**Points notables.**
- Le handshake WebSocket est implémenté MAISON en std-only (SHA-1 + base64 inline, ~120 lignes) avec vecteurs RFC6455 — refus explicite de tokio-tungstenite pour ne pas introduire un runtime async dans un serveur synchrone thread-par-connexion (clause anti-enlisement).
- Deux systèmes « push » disjoints : obrain-reactive (bus mutation in-process, tokio) et le transport WS du viz (serveur→client). Le live WS ne s'abonne PAS au MutationBus : il sonde le mtime du store toutes les 400 ms (epoch = mtime u128).
- InstrumentedStore, la SOURCE d'événements du pipeline réactif (interception de GraphStoreMut), n'est instanciée nulle part en production : seul obrain-substrate/src/store.rs le mentionne en commentaire. La chaîne MutationBus→Scheduler→Listeners est câblée, mais la capture des mutations par wrapper semble latente/non branchée.
- Le stream est ordonné par IMPORTANCE STRUCTURELLE (degré décroissant dans le sous-graphe, tie-break energy desc puis id) : le squelette/hubs peint avant les détails, tout en préservant l'identité (permutation, union == corps HTTP).
- Backpressure WS : au-delà de WS_RESNAPSHOT_DELTA_MAX=4096 changements, le serveur re-snapshot complet au lieu de pousser un delta énorme ; abonnement borné à 300 s + heartbeat 15 s.
- Phase C protobuf différée (prost/prost-build absents de Cargo.lock) : le schéma .proto Frame/Node/Edge/Delta est posé (docs/plan-j9-protobuf.md) mais non compilé.
- Le client dashboard parse les frames live/delta/epoch/heartbeat/resnapshot/bye en no-op (default) : le mode live=1 n'est pas encore consommé par l'UI actuelle, qui n'utilise que le snapshot one-shot.
- Le Scheduler natif est à spawn paresseux (premier register_listener) et zéro-coût sans listener ; sur WASM, Scheduler est synchrone (pas de tâche de fond).

**Trous / incohérences.**
- InstrumentedStore n'est jamais construit dans obrain-engine (aucun `InstrumentedStore::new` hors tests/doc) : la source d'événements du pipeline réactif est-elle réellement branchée en production, ou la capture repose-t-elle sur un autre mécanisme non trouvé ?
- Le live/delta WS est du polling (mtime toutes les 400 ms) et non de l'événementiel branché sur le MutationBus : une mutation qui ne modifie pas le mtime, ou plusieurs commits dans la même fenêtre, peuvent être fusionnés/ratés ; le lien entre l'epoch=mtime et l'epoch de transaction réel n'est pas documenté.
- epoch = mtime du fichier store (u128) est un proxy grossier de fraîcheur (pas un epoch MVCC) ; risque de collision/latence entre écriture et détection.
- docs/plan-j9.md annonce une réutilisation de obrain_viz::diff_subgraphs pour les deltas, mais viz_serve.rs réimplémente compute_delta localement (divergence doc/code).
- scripts/guard-msgpath.sh référence bench/GUARD-SPEC.md qui n'existe pas (ls bench/GUARD-SPEC.md → absent).
- La Phase C protobuf est différée (prost hors Cargo.lock) : pas d'encodeur/décodeur binaire ni de négociation enc=pb implémentés.
- Le mode live (abonnement vivant + deltas) est implémenté côté serveur mais non consommé par l'UI dashboard (api.ts ignore les frames live/delta) : fonctionnalité orpheline à ce stade.
- Le bench bus_bench.rs existe mais la métrique « <5 us sans abonné » n'est attestée que par doc/commentaires, pas par un test chiffré visible.



### 8.4 Serveur MCP — pont obrain (bridge) + surface selfd + intégration Claude

_Fichier : `raw/v8-4-mcp-server.json` — 13 composants · 11 relations · 10 constats · 8 trous_


**Synthèse.** Le serveur MCP principal est `obrain mcp` / `obrain bridge` dans crates/obrain-cli/src/mcp_server.rs (8001 lignes) : un pont JSON-RPC 2.0 sur stdin/stdout qui expose 35 outils au harness DeepSeek (client MCP standard `@modelcontextprotocol` SDK / StdioClientTransport), et qui double le protocole `codebase-memory-mcp bridge` du moteur C (`{"tool","args"}` normalisé en `tools/call`). Le protocole ne sert QUE des outils : `initialize` annonce `capabilities.tools.listChanged=false`, `tools/list` renvoie le catalogue, `tools/call` route vers l'API moteur ObrainDB — aucune ressource (`resources/list`) ni prompt (`prompts/list`) n'est implémenté. Le contrat de réponse miroir du moteur C est `structuredContent` = payload JSON objet (l'autorité pour le client), `content[0].text` = JSON sérialisé, `isError` sur échec. Chaque appel passe d'abord la garde IAM (GardeIam via `OBRAIN_IAM_TOKEN`) qui classe lecture/écriture (outil_ecrivant + gql_ecrivante) et publie son état dans la réponse `initialize` (champs `iam`/`iam_armee`). Les 35 outils couvrent la lecture/analyse du graphe (search_graph, query, query_graph, trace_path, impact_set, viz_*…), le runtime (sessions_list, context_build, checkpoint_material, session_load…), l'écriture (import_events, graph_link, index_repository, memory_register…) et la parité codebase-memory (search_code, payload_get, list_docs…). Un second serveur MCP indépendant vit dans crates/obrain-selfd (surface machine : exec/read/write/list/node_chat/node_run/node_status, transport stdio ou ssh). L'intégration Claude est portée par la branche NON FUSIONNÉE feat/mcp-install-claude (commande `obrain mcp-install` + guide embarqué `mcp_guide.md` servi dans `initialize.instructions`), tandis que feat/mcp-next-step-hints (hints `next_step_hint()` annexés aux réponses) est, elle, déjà fusionnée dans HEAD.


**Composants clés.**
- crates/obrain-cli/src/mcp_server.rs — serveur MCP bridge (8001 lignes) : boucle JSON-RPC (run/handle_line), tools_list() 35 outils, call_tool() routeur + garde IAM, next_step_hint(), outil_ecrivant(), échéance OBRAIN_QUERY_TIMEOUT_MS, mémo-cache sessions_list (process écrivain), AutoOpt metrics (OBRAIN_METRICS_FILE)
- crates/obrain-cli/src/main.rs — câblage CLI : mod mcp_server (l.33), Commands::Mcp (l.369) et Commands::Bridge alias (l.371-373), dispatch run() (l.739-740), log_to_stderr pour Mcp (l.650)
- crates/obrain-cli/src/harness_serve.rs — réutilise mcp_server::sessions_list (l.176) et mcp_server::context_build (l.199) depuis le serveur HTTP harness
- crates/obrain-cli/src/worlds.rs — réutilise mcp_server::index_repository (l.578) : l'indexeur du serveur MCP partagé avec l'outil `worlds`
- crates/obrain-selfd/src/mcp.rs — surface MCP selfd (250 lignes) : outils exec/read/write/list/node_chat/node_run/node_status, boucle JSON-RPC une trame par ligne, version 2025-06-18
- crates/obrain-selfd/src/protocole.rs — analyser(ligne), executer_outil(etat, methode, args, secret), Etat::nouveau, repondre — protocole partagé par les modes serve/socket/mcp de selfd
- crates/obrain-selfd/src/main.rs — sous-commande `selfd mcp` (l.58), mode "mcp" sans verrou ni jeton (l.363-449), appel mcp::boucle(&mut etat, None) (l.449)
- crates/obrain-cli/src/commands/mcp_install.rs — (branche feat/mcp-install-claude, NON fusionnée) auto-installation du binaire comme serveur MCP Claude (1293 lignes) : sonde de format binaire (Mach-O/ELF/PE), fusion idempotente de claude_desktop_config.json, mcp-uninstall/mcp-status, 7 tests unitaires + 38 tests e2e HOME isolé
- crates/obrain-cli/src/mcp_guide.md — (branche feat/mcp-install-claude) guide 7 sections embarqué via include_str! et servi dans initialize.instructions (3697 caractères)
- crates/obrain-cli/src/commands/mod.rs — enregistre le module mcp_install (branche feat/mcp-install-claude)
- crates/obrain-indexer/src/schema.rs, lang_specs.rs, docs.rs, metrics.rs, tests_detect.rs — parité du schéma et de l'analyse de l'ancien indexeur C codebase-memory-mcp (répliquée par le serveur MCP)
- crates/obrain-engine/src/database/text_index_persist.rs — log de démarrage "obrain-mcp: store ouvert" partagé avec le pont
- README.md — section « MCP Server » (l.350-364) : expose queries/algorithms/cognitive, npm install -g @obrain-db/mcp, renvoi vers github.com/this-rs/obrain-mcp

**Relations / dépendances.**
- mcp_server.rs::run → obrain_engine::ObrainDB (ouvre le store en lecture partagée par défaut, OBRAIN_STORE ou --store, verrou LOCK_SH pour les respawns bridge du harness)
- mcp_server.rs::call_tool → viz_serve::{GardeIam, gql_ecrivante, jeton_du_pont} — la garde IAM passe AVANT tout outil (jeton depuis OBRAIN_IAM_TOKEN, espace = radical du store)
- mcp_server.rs::handle_line("initialize") → viz_serve::GardeIam::partagee().etat() / est_armee() — publie l'état d'accès dans la réponse initialize (champs iam/iam_armee)
- main.rs Commands::Mcp / Commands::Bridge → mcp_server::run (même point d'entrée, Bridge = alias du protocole codebase-memory-mcp bridge)
- harness_serve.rs → mcp_server::sessions_list / context_build (réutilisation des implémentations du bridge par le serveur HTTP)
- worlds.rs → mcp_server::index_repository (l'outil `worlds` réutilise l'indexeur du serveur MCP)
- selfd/mcp.rs::boucle → protocole::analyser + protocole::executer_outil (Etats : exec/read/write/list/chat/run/status mappés par methode_native)
- selfd/main.rs (mode mcp) → selfd/mcp.rs::boucle(&mut etat, None) — pas de verrou ni jeton : le lanceur (config harness / ssh) EST la frontière de confiance
- mcp_server.rs::tools_list() ↔ mcp_server.rs::call_tool() — parité 35/35 vérifiée (noms identiques), la doc en-tête du module ne cite que 6 outils représentatifs
- obrain-indexer/* → parité codebase-memory-mcp (schéma OUVERT répliqué, lang_specs/docs/metrics/tests_detect issus de internal/cbm du moteur C)
- feat/mcp-install-claude → main.rs (+23/+11 lignes) : sous-commandes mcp-install/mcp-uninstall/mcp-status/mcp-guide ; mcp_server.rs (+85) : initialize.instructions = include_str!("mcp_guide.md") + OBRAIN_IDLE_MS=0

**Points notables.**
- mcp_server.rs fait 8001 lignes : le bridge Rust est un miroir fonctionnel du moteur C codebase-memory-mcp, y compris le format de trame bridge non-JSONRPC {"tool","args"} normalisé en tools/call pour le GraphWriter/GraphInjector du harness.
- Le protocole MCP n'implémente AUCUNE ressource ni prompt : initialize renvoie uniquement capabilities.tools.{listChanged:false}, seuls tools/list et tools/call sont servis — l'intégration « Claude » de la branche ajoute en revanche initialize.instructions (guide embarqué).
- feat/mcp-install-claude est NON FUSIONNÉE dans HEAD (branche courante consolidation/tronc-2026-09-15-plus-fusion, HEAD 456d3794) : mcp_install.rs, mcp_guide.md et le champ instructions n'existent PAS sur HEAD — la commande `obrain mcp-install` et l'installation Claude Desktop sont absentes du tronc.
- feat/mcp-next-step-hints est FUSIONNÉE : next_step_hint() (l.1103-1141) annexe un champ `hint` impératif aux réponses de SUCCÈS des outils principaux (query, query_graph, search_graph, search_code, runtime_search, get_graph_schema, index_status) — additif strict, jamais sur erreur, jamais par-dessus un hint posé par l'outil.
- La garde IAM publie son état dans initialize (champs iam/iam_armee) et chaque refus d'outil garde sa structure (champs à plat error/outil/acces/espace/http/iam) au lieu d'aplatir en une phrase — correction documentée « UN REFUS GARDE SA STRUCTURE » (l.560-577).
- sessions_list possède un mémo-cache (SESSIONS_LIST_GEN/CACHE) réservé au process écrivain (OBRAIN_WRITE=1), qui voit toutes les mutations ; les process RO ne cachent pas (point de vue d'un instant).
- La doc d'en-tête de mcp_server.rs (l.10-15) ne cite que 6 outils représentatifs alors que tools_list() en sert 35 ; le commentaire « P1 parité 31 outils » des commits est désormais obsolète (35 au HEAD).
- Le commentaire « DÉCISIONS ENREGISTRÉES EN CODE » (l.1036-1075) documente 6 outils de l'ancien serveur C NON servis (detect_changes, get_architecture, cliques_of/hyperedges_of, delete_project, weave_retisser, check_index_coverage) avec l'équivalent bridge de contournement.
- La branche feat/mcp-install-claude documente trois pannes réelles du premier branchement : (1) OBRAIN_IDLE_MS=5000 par défaut tuait le serveur après 5 s d'inactivité (fix OBRAIN_IDLE_MS=0 pour Desktop + `claude mcp add`) ; (2) initialize ne servait aucun instructions (fix : mcp_guide.md embarqué) ; (3) graph_link jetait les propriétés d'arête alors que create_edge_with_props existe depuis toujours.
- README.md (l.355) renvoie vers `npm install -g @obrain-db/mcp` et github.com/this-rs/obrain-mcp — des artefacts EXTERNES absents de ce dépôt ; le serveur réel est le sous-processus `obrain mcp`/`obrain bridge` de obrain-cli, pas un paquet npm dans packages/.

**Trous / incohérences.**
- feat/mcp-install-claude non fusionnée : la commande obrain mcp-install, mcp-uninstall, mcp-status, mcp-guide et le champ initialize.instructions (guide embarqué) sont absents de HEAD — décision d'intégration non documentée dans le tronc.
- README.md « MCP Server » (l.350-364) est partiellement aspirational : il pointe @obrain-db/mcp (npm) et github.com/this-rs/obrain-mcp, inexistants dans le dépôt — l'URL de référence réelle est le binaire obrain-cli ; divergence doc/code.
- Aucune ressource ni prompt MCP : le serveur n'expose que tools — le README évoque « inspect schema, stats, and graph structure » mais pas de resources/prompts côté code.
- Doc d'en-tête obsolète : mcp_server.rs l.10-15 liste 6 outils alors que tools_list() en sert 35 ; le commentaire de commit « P1 parité 31 outils » est périmé.
- 6 outils de l'ancien serveur C (detect_changes, get_architecture, cliques_of/hyperedges_of, delete_project, weave_retisser, check_index_coverage) sont documentés comme NON implémentés — trous de parité codebase-memory assumés mais non comblés.
- index_status note que les métadonnées de couverture (parse_partial/miss-graph) ne sont pas persistées par obrain-indexer — couverture d'indexation incomplète côté bridge.
- trace_path ne porte pas les modes data_flow/cross_service (mentionné dans son inputSchema) — trou de parité partiel.
- selfd/mcp.rs déclare capabilities.tools.{listChanged:false} et version 2025-06-18 mais la réponse initialize ne renvoie pas instructions ; la confiance repose entièrement sur le lanceur (pas de jeton en mode MCP), ce qui est un choix documenté mais à vérifier côté sécurité.



### 8.5 Surfaces CLI + IAM auth (obrain-cli, packages/obrain-cli-*, obrain-iam, feat/iam-mint-endpoint)

_Fichier : `raw/v8-5-cli-iam-auth.json` — 23 composants · 9 relations · 10 constats · 9 trous_


**Synthèse.** La CLI obrain est un binaire Rust unique (crate obrain-cli, binaire `obrain`) construit sur clap, qui expose ~40 sous-commandes (query, init, shell/REPL, viz, index, backup, data, wal, topo, jump, journal, repair*, enrich, compare, continuity, mcp/bridge, iam…). La surface IAM est un noyau unique (crate obrain-iam, ~8500 lignes) partagé par trois surfaces : le CLI `obrain iam` (hors bande, sans plafond, agit sur un fichier .obrain), le serveur HTTP `viz serve` (plafonné par les droits de l'appelant), et l'exemple d'amorçage. L'authentification repose sur ObrainIamProvider (trait AuthProvider) : authenticate() accepte Internal/SessionToken/ApiKey/Password/OidcToken (OIDC non implémenté), create_session() émet un jeton `obt_…` de 256 bits dont seul l'empreinte SHA-256 est stockée, authorize() évalue les politiques à chaque requête via IamStore::evaluate_access. Le flux de connexion HTTP est `POST /api/auth/login` (mot de passe → jeton rendu dans le corps JSON + cookie HttpOnly), puis `/api/auth/whoami|refresh|logout|audit` et les routes `/api/iam/*` gardées par l'action nommée `iam:admin`. La garde (`Garde`) est fail-closed quand armée (`OBRAIN_IAM=1`) et bruyante quand non armée ; le plafond réseau se déduit des politiques de l'acteur (`Administration::plafonnee_par_l_acteur`), jamais déclaré par l'appelant. Les packages npm (`@obrain-db/cli`) et Python (`obrain-cli`) sont de simples lanceurs qui délèguent au binaire natif. La branche feat/iam-mint-endpoint (worktree wt-iam-mint, 2 commits en avance sur banc-obrain HEAD) ajoute l'endpoint `POST /api/iam/token` (mint de jeton de service) via `Garde::emettre_service`/`Garde::est_administrateur`, non encore fusionné dans banc-obrain.


**Composants clés.**
- crates/obrain-cli/src/main.rs — binaire `obrain`, enum clap `Cli`/`Commands`/`OutputFormat`/`QueryLanguage`/`GraphMode`, ~40 sous-commandes, codes de sortie (0/1/2)
- crates/obrain-cli/src/commands/iam.rs — surface `obrain iam`, enum `IamCommands` (21 variantes : amorcer, utilisateurs, utilisateur, ajouter, desactiver, reactiver, mot-de-passe, espace-ajouter/-retirer, admin-ajouter/-retirer, jetons, jeton, jeton-revoquer, roles, politiques, politique-supprimer, audit, exporter, effacer)
- crates/obrain-iam/src/admin.rs — noyau `Administration` (hors_bande / avec_plafond / plafonnee_par_l_acteur), garanties (plafond, verrou dernier-admin, trace), constantes ACTION_ADMIN/ACTION_LECTURE/ACTION_ECRITURE/ACTION_AUDIT
- crates/obrain-iam/src/provider.rs — trait `AuthProvider` + `ObrainIamProvider` (authenticate/authorize/create_session/revoke_session/create_federation_token/assume_role), hash_token SHA-256, generate_token 256 bits, bcrypt
- crates/obrain-iam/src/garde.rs — `Garde`, point d'entrée unique de contrôle (fail-closed armé, `OBRAIN_IAM=1/ACCOUNT/TOKEN/ACTION_*`)
- crates/obrain-iam/src/store.rs — `IamStore`, CRUD graphe-backed (create_user/role/policy/credential, evaluate_access, log_audit_event, list_audit_events, deactivate_user, delete_role/policy, purge_session_credentials)
- crates/obrain-iam/src/model.rs — types domaine (User/Role/Policy/Credential/AuditEvent, PolicyEffect/PolicyDecision/CredentialType/EntityStatus) + labels IamUser/IamRole/IamPolicy/IamCredential/IamAuditEvent et arêtes HAS_ROLE/HAS_POLICY/HAS_CREDENTIAL/PERFORMED
- crates/obrain-iam/src/orn.rs — ORN (`orn:obrain:{service}:{account}:{type}/{path}`), parsing, jokers, substitution
- crates/obrain-iam/src/policy.rs — moteur d'évaluation Allow/Deny/ImplicitDeny
- crates/obrain-iam/src/gdpr.rs — GdprManager (export_user_data, request_erasure → ErasureCertificate)
- crates/obrain-iam/src/session_ext.rs — AuthenticatedSession (intégration session/identité)
- crates/obrain-iam/src/audit_chaine.rs — audit chaîné/signé (ADR §4.1)
- crates/obrain-iam/src/error.rs — IamError/IamResult
- crates/obrain-cli/src/viz_serve.rs — serveur HTTP : /api/auth/{login,logout,whoami,refresh,audit} + /api/iam/* (plafonné) + pages /login et /iam
- crates/obrain-cli/examples/amorcer_iam.rs — amorçage hors bande du premier jeton (utilisateur+rôle+politiques+mot de passe+jeton 24h)
- packages/obrain-cli-npm/bin/obrain.js — lanceur Node (résout le binaire plateforme via optionalDependencies, sinon PATH)
- packages/obrain-cli-npm/package.json — @obrain-db/cli 0.0.1, optionalDependencies par plateforme
- packages/obrain-cli-npm/platform-template/package.json.tmpl — template de paquet plateforme (@obrain-db/cli-{{PLATFORM}})
- packages/obrain-cli-python/obrain_cli/__init__.py — lanceur Python (bundled > venv bin > PATH), __main__.py, pyproject.toml (entry point obrain=obrain_cli:main)
- scripts/acceptation-iam.sh — porte d'acceptation bout-en-bout CLI + HTTP (amorçage, gestes, garde/plafond, refus, piste)
- docs/ADMINISTRATION-IAM-2026-09-13.md — document d'administration IAM (défaut, surfaces, 3 garanties, pièges, hors-chaîne)
- feat/iam-mint-endpoint (worktree /home/triviere/projects/wt-iam-mint) — Garde::emettre_service + Garde::est_administrateur + route POST /api/iam/token (viz_serve.rs)
- feat/iam-groupes-tronc (worktree /home/triviere/projects/wt-iam-tronc) — groupes (User)-[:MEMBER_OF]->(Group)-[:HAS_ROLE]->(Role), argon2id, cache politiques par principal, Garde::controler_ressource

**Relations / dépendances.**
- obrain-cli → obrain-iam : dépendance OPTIONNELLE, feature `iam = ["obrain-engine/cognitive-iam", "dep:obrain-iam"]` (Cargo.toml) ; `default = ["gql","cypher","indexer","ai","iam","algos","parallel"]` embarque iam par défaut depuis 2026-09-15
- commands/iam.rs `ouvrir()` → `obrain_iam::admin::Administration::hors_bande(Arc<IamStore::new(db.graph_store()), compte, acteur)` — ouvre la base via `commands::open_existing` (jamais `ObrainDB::open`)
- main.rs `Commands::Iam(cmd) => commands::iam::run(cmd, cli.format)` — sans feature iam, run() bails « IAM NON COMPILÉ » au lieu de disparaître
- obrain-iam::admin → obrain-iam::provider (ObrainIamProvider) + obrain-iam::store (IamStore) + obrain-iam::gdpr (GdprManager)
- viz_serve.rs (HTTP) → Garde (GardeIam::partagee()) → ObrainIamProvider → IamStore — la décision de politique vit dans la bibliothèque, pas dans le binaire
- POST /api/auth/login → garde.connecter() → authenticate(AuthRequest::Password) → create_session() → jeton + cookie HttpOnly SameSite=Strict
- mint feat : POST /api/iam/token → garde_interieure() + garde.identite(jeton) + garde.est_administrateur(appelant) → garde.emettre_service(cible, espaces, ttl, acteur) → Administration::hors_bande → ajouter_utilisateur/accorder_espace → emettre_jeton
- packages/obrain-cli-npm & obrain-cli-python → spawn du binaire natif `obrain` (execFileSync / subprocess.run), aucune logique IAM propre
- IamStore opère sur le GraphStoreMut de l'appelant (labels IamUser/IamRole/IamPolicy/IamCredential/IamAuditEvent) — le « graphe nommé __system » n'apparaît que dans les doc-comments

**Points notables.**
- L'IAM est optionnel mais le défaut est « reachable, never imposed » : sans `--features iam`, `obrain iam` reste présent et s'annonce « IAM NON COMPILÉ » (message nommant la construction exacte), au lieu de disparaître — un choix de diagnostic documenté.
- feat/iam-mint-endpoint N'EST PAS dans banc-obrain HEAD (456d3794) : l'endpoint POST /api/iam/token et Garde::emettre_service n'existent que dans le worktree wt-iam-mint (branche feat/iam-mint-endpoint, commits 05ff1bbe « wip endpoint mint » puis a399665b « l'audit nomme l'appelant, plus la constante viz-mint »). garde.rs de HEAD a 0 occurrence de emettre_service.
- Le jeton de session était PRÉVISIBLE avant le 2026-09-13 (DefaultHasher + compteur, même jeton à chaque démarrage) ; corrigé en entropie système 256 bits (`rand::random::<u64>()`×4, préfixe `obt_`). Mots de passe bcrypt, jetons SHA-256.
- revoke_session était un NO-OP documenté ; corrigé en écriture d'une expiration dans le passé + tentative delete_node (la suppression pure est insuffisante sur base fichier — mesuré).
- Trois garanties d'administration : plafond LU (jamais déclaré) via plafonnee_par_l_acteur ; verrou du dernier administrateur actif (désactivation/dégradation/effacement/suppression de politique refusés) ; tout geste tracé sur l'action nommée (iam:user:create, iam:space:revoke…) avec principal=acteur et ressource=cible.
- La désactivation/révocation mord immédiatement sans purge : authenticate_by_token_hash relit l'utilisateur et authorize réévalue les politiques à CHAQUE requête (propriété de chaîne, pas une promesse du module admin).
- L'amorçage (amorcer) et l'émission de jeton pour autrui (emettre_jeton) refusent d'être appelés sur une administration plafonnée — contournements impossibles depuis le réseau.
- Le CLI est hors bande (plafond: None, autorité = système de fichiers) ; le serveur est plafonné et exige un espace NOMMÉ (refus 400 sinon) ; /api/iam/moi exige une identité mais aucun droit.
- Les packages npm/python sont des lanceurs minces : la version Python déclare __version__="0.4.4" dans __init__.py alors que pyproject.toml dit version 0.0.1 ; le template npm référence github.com/ObrainDB/obrain et une licence AGPL, tandis que package.json pointe github.com/this-rs/obrain en Apache-2.0.
- Deux branches IAM divergent en parallèle : feat/iam-mint-endpoint (wt-iam-mint) et feat/iam-groupes-tronc (wt-iam-tronc : groupes, argon2id PHC, cache de politiques par principal, Garde::controler_ressource) — consolidation non triviale.

**Trous / incohérences.**
- feat/iam-mint-endpoint non fusionné dans banc-obrain HEAD : le mint (POST /api/iam/token) n'existe que dans wt-iam-mint ; docs/ADMINISTRATION-IAM §garde dit encore « aucune route de remise de jeton » (obsolète vis-à-vis de la branche).
- OIDC non implémenté : AuthRequest::OidcToken rend InvalidParameter « requires T1.7 (JWT/Ed25519) » ; aucun JWKS/issuer vérifié.
- assume_role crée une session mais le commentaire admet que le scoping du rôle « will be enforced by AuthenticatedSession in T1.4 » — non appliqué aujourd'hui.
- create_federation_token « trust the caller » : aucune validation d'intersection des scoped_actions avec les permissions réelles (commentaire l'admet).
- revoke_session dépend de la visibilité de delete_node dans le substrat (incohérente fichier vs mémoire) ; l'expiration-dans-le-passé est le filet de sécurité.
- Le « graphe nommé __system » n'est que doc-commentaire : IamStore écrit dans le GraphStoreMut passé par l'appelant, sous labels IamUser/IamRole/… (pas d'isolation physique en graphe nommé).
- Incohérences de packaging : py __version__ 0.4.4 vs 0.0.1 ; template npm repo ObrainDB/obrain + AGPL vs this-rs/obrain + Apache-2.0.
- Pas de MFA, pas de renouvellement automatique côté client, pas d'énumération des sessions d'une identité (doc §6, inchangé).
- Divergence entre les deux branches IAM (mint vs groupes-tronc : argon2id remplace bcrypt, groupes, cache) — quel ordre de fusion / quelle autorité fait foi reste à trancher.




## Vague 9 — V9 Dashboard

### 9.1 packages/obrain-dashboard/src/ — front-end web du dashboard Obrain (React · Jotai · Three.js · Vite · SCSS)

_Fichier : `raw/v9-1-dashboard-src.json` — 42 composants · 16 relations · 11 constats · 10 trous_


**Synthèse.** Le dashboard est une application monopage React 18 + TypeScript (build Vite), pilotée par des atoms Jotai et rendue en 3D par Three.js — et non en Vue comme l'annonce le brief. L'UX est « canvas-first » : la représentation (scène WebGL) est maîtresse et les panneaux (réglages, commandes, projection, facettes) sont des volets « verre » dont la visibilité découle d'un système de priorité déclaratif (chromeAtom/PRIORITE, niveaux 1 à 4) et d'un geste de réduction. Une seule autorité d'état (`faceAtom` : cosmos|monde|graphe) remplace d'anciens booléens divergents, et quatre « crans » d'échelle (Espace → Monde → Groupe → Signaux) sont descendus en continu dans une même scène Cosmos. Le client parle au serveur `obrain viz serve` via `lib/api.ts`, un point de passage unifié (cache de session, single-flight, AbortSignal, transport WebSocket en streaming avec repli HTTP+gzip) vers ~20 endpoints REST (/api/stores, /api/viz/*, /api/query, /api/engine/*, /api/worlds, /api/iam/*). Les écrans sont : barre de commandes + autocomplétion de fonctions, inspecteur (cible/échelle/overlay/strates/requête GQL + projection multi-axes bornée), FacetBar (fratrie/co-fichiers/sœurs), Cosmos (système solaire + toile synaptique `weave`), Monde (constellation StoreOverview), Graphe (Graph3D, K3/K4/∂/synapses), chat plein écran (LLM local en SSE, markdown + KaTeX incrémentaux), Administration des comptes (IAM) et Ajout de monde (clone+indexation+enrichissement d'un dépôt GitHub). Les calculs décisionnels (cadrage, densité, échelle, fractale, fratrie, sélection, segmentation, raccord, conversations, projection, topologie) sont extraits en modules purs testés par des bancs Node `.mjs`. Un `uiStateBridge` persiste l'état d'interface (face, échelle, axe, panneaux, sujet) vers un serveur `this-rs` (`/ui/state/{portee}`) en se dégradant proprement sans lui.


**Composants clés.**
- packages/obrain-dashboard/package.json — @obrain-db/dashboard 0.1.0 (react 18, jotai, three, katex, react-markdown/remark/rehype, sass, vite)
- packages/obrain-dashboard/vite.config.ts — base '/app/', proxy /api→127.0.0.1:8432, plugin policesEnLigne (KaTeX woff2 en data:), emptyOutDir:false
- packages/obrain-dashboard/src/main.tsx — point d'entrée : monte <App/>, démarre demarrerPontEtat()
- packages/obrain-dashboard/src/app/App.tsx (712 l.) — shell racine : faces, crans d'échelle, interpréteur de commande, états vides/refusés, zoom directionnel
- packages/obrain-dashboard/src/app/store.ts (417 l.) — atoms Jotai d'autorité (face, niveau, écran, chrome, ancrage, sujet, chat, seuil) + dérivés
- packages/obrain-dashboard/src/app/uiStateBridge.ts (299 l.) — pont d'état UI → serveur /ui/state/{portee} (GET/PUT, débounce 400 ms, sonde unique, repli local)
- packages/obrain-dashboard/src/lib/api.ts (592 l.) — client réseau : cache session + single-flight + AbortSignal, WS subgraph, query, weave, activation, engine, worlds, entretien, ramifications
- packages/obrain-dashboard/src/lib/projection.ts (298 l.) — projection multi-axes bornée (TOP_GROUPES 64, SEUIL_PROJECTION_ROWS 20000, topK partiel)
- packages/obrain-dashboard/src/lib/topology.ts (53 l.) — strates simpliciales : adjacency, triangles, cliquesK4, hyperGroups, contourEdges, degreeOf
- packages/obrain-dashboard/src/components/CommandBar.tsx — barre de commandes (store, autocomplétion de fonctions via searchFunctions, interprétation naturelle/GQL/symbole)
- packages/obrain-dashboard/src/components/Inspector.tsx — panneau de réglages escamotable (cible, échelle/volume, overlay, strates, requête GQL + projection)
- packages/obrain-dashboard/src/components/FacetBar.tsx — fratrie/co-fichiers/sœurs/cycle/homonymes du nœud sélectionné (index fratrie)
- packages/obrain-dashboard/src/components/Projection.tsx — rendu des axes projetés (barres, liste fenêtrée, ancrage des groupes qn)
- packages/obrain-dashboard/src/components/StageContinu.tsx (1558 l.) — Atlas « même substrat » morph monde↔graphe (fetchSubgraph, fetchSignatures, fractal, densité, activation)
- packages/obrain-dashboard/src/components/StoreOverview.tsx (1496 l.) — face Monde « constellation » (fetchModules/fetchModuleArcs/fetchSpace/fetchLabelPairs, plongée par anneaux)
- packages/obrain-dashboard/src/components/CenterSearch.tsx — recherche/ancrage d'un centre (searchFunctions, rangement par pertinence)
- packages/obrain-dashboard/src/components/Chips.tsx — micro-composants ChoiceChips/Toggle
- packages/obrain-dashboard/src/components/CanvasBoundary.tsx — ErrorBoundary des scènes canvas (écrans vides expliqués)
- packages/obrain-dashboard/src/graph/Graph3D.tsx (459 l.) — moteur de représentation conceptuelle (nœuds instanciés, arêtes, faces/hyperfaces, ∂ contour, communautés, synapses)
- packages/obrain-dashboard/src/graph/CosmosWeave.tsx (7167 l.) — scène Cosmos : système solaire + toile de l'univers (fetchWeave), descente 4 crans, parenté/fratrie, activation
- packages/obrain-dashboard/src/graph/ChatPleinEcran.tsx (1139 l.) — chat plein écran en SSE (EventSource), conversations nommées persistées, think→réponse en temps réel
- packages/obrain-dashboard/src/graph/EngineAsk.tsx — « demander à l'engine » : LLM local sur le sujet visé (askEngine), contexte inspectable
- packages/obrain-dashboard/src/graph/AdminComptes.tsx — écran d'administration IAM (par espace nommé, /api/iam/*)
- packages/obrain-dashboard/src/graph/WorldAdd.tsx — ajout d'un monde GitHub (clone→indexation→enrichissement→fini, suivi worldJob)
- packages/obrain-dashboard/src/graph/RenduRiche.tsx — markdown + LaTeX incrémentaux (react-markdown, remark-gfm/math, rehype-katex), coût par bloc
- packages/obrain-dashboard/src/graph/ViewPanel.tsx — chrome commun des vues (ViewPanel/LevelCartouche/PlanLegend)
- packages/obrain-dashboard/src/graph/SubjectLine.tsx — ligne du sujet partagé + ramifications ancrables (fetchRamifications)
- packages/obrain-dashboard/src/graph/useActivation.ts — sondage du journal d'activation (fetchActivation, 1200 ms), peakOf(store)
- packages/obrain-dashboard/src/graph/grammar.ts — grammaire commune (plans, relief, caméra, renderer, touches, emphasis)
- packages/obrain-dashboard/src/graph/fractale.ts — loi d'échelle fractale + appels serveur (enfantsDe, pageServeur, rangRacine, rayons, directions)
- packages/obrain-dashboard/src/graph/fratrie.ts — parenté dérivée une fois (parentDe, fichierDe, moduleDe, strateDe, construireIndex, fratrieDe)
- packages/obrain-dashboard/src/graph/densite.ts — décision densifier/approfondir (decisionDensite, phraseDecision, pageSuivante)
- packages/obrain-dashboard/src/graph/echelle.ts — paliers/bandes des 4 crans (distancePourPalier, bandeDe, viserCran)
- packages/obrain-dashboard/src/graph/cadrage.ts — cadrage caméra pur (cadragePour, etape, avancer, stabilise)
- packages/obrain-dashboard/src/graph/selection.ts — sélection par pointage en pixels (projeter, classer, choisir)
- packages/obrain-dashboard/src/graph/segments.ts — découpage d'une réponse autour de ses appels d'outils
- packages/obrain-dashboard/src/graph/raccord.ts — raccord sans recouvrement d'une réponse reprise (SSE)
- packages/obrain-dashboard/src/graph/conversations.ts — état/tri/rangement/migration des conversations (persistance localStorage obrain.chat.v2.*)
- packages/obrain-dashboard/src/graph/natures.ts — nature de chaque type (cognition/code/inconnue), couleur
- packages/obrain-dashboard/src/graph/monde/util.ts — primitives Monde (couleurs, textures, forceLayout, cache agg)
- packages/obrain-dashboard/src/styles/{base.scss,_tokens.scss,_layout.scss,_controls.scss,_query.scss} — feuilles SCSS du chrome
- packages/obrain-dashboard/src/graph/{cadrage,conversations,densite,echelle,raccord,segments,selection}.test.mjs — bancs de test Node des modules purs

**Relations / dépendances.**
- main.tsx → app/App.tsx + app/uiStateBridge.ts (demarrerPontEtat) + styles/base.scss
- App.tsx → app/store.ts (atoms) et lib/api.ts (fetchStores, runQuery, resoudreAncres, fetchOverview) et tous les composants d'écran
- tous les composants lisent/écrivent les atoms de app/store.ts (faceAtom, niveauDemandeAtom, chromeAtom, subjectAtom, ancrerAtom, requestTokenAtom, …)
- app/store.ts → lib/api.ts (type JsonGraph) et définit les autorités uniques (ancrerAtom = périmètre+fil+rechargement atomiques)
- lib/api.ts → endpoints REST obrain viz serve : /api/stores, /api/info, /api/functions, /api/query, /api/viz/{subgraph,signatures,overview,modules,arcs,space,weave,activation,ramifications}, /api/entretien, /api/engine/{sante,ask}, /api/worlds(+POST,/status)
- lib/api.ts : fetchSubgraph = WS /api/viz/subgraph/stream (streamSubgraph) avec repli HTTP+gzip /api/viz/subgraph ; cache session 30 s + single-flight
- lib/api.ts : searchFunctions = debounce 120 ms + cache 60 s partagé entre CommandBar et CenterSearch ; resoudreAncres résout un nom nu en qn
- Graph3D.tsx / StageContinu.tsx / StoreOverview.tsx / CosmosWeave.tsx → graph/grammar.ts (THREE : makeCamera, makeRenderer, applyRelief, bindViewKeys) et graph/ViewPanel.tsx
- StageContinu.tsx / CosmosWeave.tsx → graph/fractale.ts + graph/densite.ts + graph/echelle.ts + graph/cadrage.ts + graph/selection.ts (lois pures)
- FacetBar.tsx / CosmosWeave.tsx → graph/fratrie.ts (construireIndex, fratrieDe) et lib/topology.ts (triangles, cliquesK4, contourEdges)
- ChatPleinEcran.tsx → graph/conversations.ts + graph/raccord.ts + graph/RenduRiche.tsx, transport EventSource (SSE) vers le serveur
- EngineAsk.tsx → lib/api.ts askEngine (/api/engine/ask) ; WorldAdd.tsx → addWorld/fetchWorlds/worldJob ; AdminComptes.tsx → /api/iam/moi + POST /api/iam/*
- uiStateBridge.ts → GET/PUT /ui/state/{portee} (surchargeable VITE_UI_STATE_BASE), s'abonne aux atoms d'autorité et applique le document reçu
- graph/monde/util.ts → importé par StoreOverview (agg, forceLayout, textures, couleurs) ; graph/natures.ts → nature/couleur des types
- modules purs (*.ts sans React/three) → vérifiés par les .mjs Node du même dossier (node --test)
- vite.config.ts : base '/app/' car servi par `obrain viz serve` sous /app ; dev proxy /api→127.0.0.1:8432

**Points notables.**
- Le dashboard est en React 18 + Jotai + Three.js + Vite + SCSS, PAS en Vue comme l'annonçait le brief (« Vue/Vite ») ; les occurrences de « vue » dans le code sont le mot français « vue », pas Vue.js.
- Aucune bibliothèque de routage (ni react-router) : la navigation est faite par l'atome faceAtom + des paramètres d'URL ad hoc (?store, ?dive, ?cosmos, ?portee) lus dans App.tsx.
- lib/api.ts est documenté comme le point de réunification de DEUX lignées divergentes : cache de session (tronc) + transport WebSocket en streaming avec repli HTTP (lignée servie, commit bfad86ff).
- Le sous-graphe est streamé par WebSocket (frames begin/nodes/edges/end/error) avec repli transparent HTTP+gzip ; un timeout 30 s sans première frame force le repli.
- Documentation en français très dense des DÉFAUTS MESURÉS avec chiffres (réponse subgraph jusqu'à 450 000 octets, /api/query jusqu'à 413 985 lignes, RangeError à 150 000 lignes, 407 événements SSE pour une réponse de 1 117 caractères).
- Le chat a été sorti de la scène WebGL (monté par App, plus par CosmosWeave) pour ne plus payer la charge GPU du canvas et ne plus être recouvert par les panneaux — décision documentée en détail.
- Quatre « crans » d'échelle (Espace/Monde/Groupe/Signaux) sont descendus en continu dans une seule scène Cosmos via un curseur vertical qui PORTE les données mesurées de chaque cran (__etages).
- CosmosWeave.tsx fait 7 167 lignes : les lois décisionnelles (cadrage, échelle, densité, sélection, fractale, fratrie) en ont été extraites en modules purs testables, pratique systématique du dépôt.
- Les états « aucune base accordée » et « espace accordé mais vide » sont explicitement expliqués à l'écran (avec le compte connecté et les bases refusées) au lieu d'un écran vide indistinguable d'une panne.
- Le fichier vite.config.ts inline les polices KaTeX woff2 en base64 (data:) car `obrain viz serve` sert /app/* en String::from_utf8_lossy, ce qui corrompt les binaires .woff2.
- Il existe un résidu App.tsx.bak-avant-fix à côté de App.tsx (sauvegarde non supprimée).

**Trous / incohérences.**
- App.tsx.bak-avant-fix : fichier de sauvegarde résiduel dans src/app/ (code mort à nettoyer).
- lib/api.ts fetchLabelPairs : la requête GQL `labels(a)[0] ... labels(b)[0]` dans un même GROUP BY est documentée comme FAUSSE (label cible recopié dans la source) → les agrégats cross-label ne sont pas fiables (à reprendre, cf. ADR-COSMOS-TOILE-2026-09-10).
- lib/api.ts streamSubgraph : les frames live/delta/epoch/heartbeat/resnapshot/bye sont « réservés au mode live=1 » mais non implémentés côté client (instantané one-shot uniquement).
- Aucun routeur : le routage par paramètres d'URL est ad hoc et non centralisé (risque de dérive entre ?store/?dive/?cosmos/?portee).
- vite.config.ts pose emptyOutDir:false : les bundles hachés périmés s'accumulent dans dist/ (assumé par le commentaire, mais pas de nettoyage).
- Les tests sont des scripts .mjs Node autonomes (node --test) : package.json n'a AUCUN script `test`, et `pnpm build` (tsc -b && vite build) ne les exécute pas.
- uiStateBridge.ts lit encore l'échelle héritée « noeud » (ECHELLE_NIVEAU) pour compatibilité ascendante — un document ancien peut écraser silencieusement le cran 3 (documenté).
- Le brief annonce « ~200 .md » docs et « Vue/Vite » : à vérifier si d'autres tranches supposent Vue ; ici la stack est React, ce qui peut invalider des hypothèses amont.
- pnpm-workspace.yaml / .npmrc (verify-deps-before-run=false) suggèrent un environnement de build particulier ; node_modules et dist/ sont présents dans le paquet.
- package.json description (« exploration, représentation conceptuelle et questions au moteur ») n'évoque ni WebSocket/SSE ni l'administration IAM — métadonnées en retard sur le code.



### 9.2 obrain-viz (couche de restitution visuelle) + dashboard-viz + projection/densité

_Fichier : `raw/v9-2-obrain-viz.json` — 29 composants · 14 relations · 8 constats · 9 trous_


**Synthèse.** obrain-viz est une lib Rust pure, volontairement sans dépendance substrate, qui convertit un sous-graphe rendable (IR `Subgraph` de nœuds/arêtes avec signaux cognitifs optionnels) en exports déterministes octet-à-octet : json-graph v1 (`obrain/json-graph@1`, source de vérité du diff/timeline), json-graph v2 (arêtes en colonnes + table de chaînes, −58 % d'octets / −65 % de JSON.parse), DOT, Mermaid, GraphML, et parse de plans EXPLAIN en arbre d'opérateurs. Les overlays cognitifs (ricci, energy, roles, tier, fsrs, engrams, pheromone) attachent des signaux persistés comme attributs standard `color_hint`/`size_hint` via des fonctions de normalisation documentées, avec une distinction assumée stables vs volatils (horodatage `generated_at`). Le crate n'expose que la couche de restitution : les extracteurs substrate et le serveur HTTP vivent côté `obrain-cli` (`viz_extract.rs`, `viz_serve.rs`), le serveur `obrain viz serve` exposant `/api/viz/subgraph`, `/api/viz/space`, `/api/viz/arcs`, `/api/viz/overview`, `/api/viz/modules`, `/api/viz/status` avec caches durables disque+mémoire. La projection/densité est théorisée dans `docs/viz-projection-densite.md` (trois échelles macro/meso/micro, densification par « slide cursor » continue, échantillonnage structurel hubs-ponts-plafond-par-anneau) et mesurée dans `docs/dashboard-viz/` (charge 2,1 Mo → 0,89 Mo, préchauffage arcs 4 243 ms → 0,7 ms). Le déploiement s'appuie sur `scripts/vizserve.sh` (port 8432, IAM armé, entretien des bases, préchauffage), `scripts/vizserve-isole.sh` (port 8433, vérification isolée), `scripts/obrain-viz.service` (systemd user) et la garde DoD `scripts/guard-viz.sh` + `scripts/viz_guard.py` (déterminisme, bien-formé, parité export_all==comptage store, bench).


**Composants clés.**
- crates/obrain-viz/Cargo.toml — manifest (deps serde/serde_json/thiserror uniquement, description « couche de restitution visuelle »)
- crates/obrain-viz/src/lib.rs — façade publique, enum VizError/VizResult, re-export des 11 modules
- crates/obrain-viz/src/ir.rs — IR `Subgraph`/`VizNode`/`VizEdge`/`Stats` + tri canonique + validation + sérialisation f32 courte (`f32_court`/`f32_depuis_json`)
- crates/obrain-viz/src/json_graph.rs — export json-graph v1 (`obrain/json-graph@1`), `into_subgraph` strict + `_lenient` (cache tronqué)
- crates/obrain-viz/src/json_graph_v2.rs — export json-graph v2 colonnaire (`obrain/json-graph@2`), `EdgeColumns` + table `strings`
- crates/obrain-viz/src/diff.rs — `diff_subgraphs`/`SubgraphDiff`/`TimelineStep`/`TimelineReport`/`timeline_subgraphs`
- crates/obrain-viz/src/overlay.rs — enum `Overlay` (7 variantes) + `apply_overlay`/`OverlayReport`/`color_ramp`/`ricci_color`/`pastel_for_id`/`pheromone_color`
- crates/obrain-viz/src/dot.rs — writer Graphviz DOT (`to_dot`/`DotOptions`), rendu fillcolor/color/penwidth
- crates/obrain-viz/src/mermaid.rs — writer Mermaid `flowchart LR` (`to_mermaid`), style/linkStyle, détection collision id
- crates/obrain-viz/src/graphml.rs — writer GraphML (`to_graphml`/`xml_escape`), clés qn/label/file/etype/weight/ricci
- crates/obrain-viz/src/plan.rs — `tree_subgraph_from_explain` : parse texte indenté EXPLAIN → Subgraph d'opérateurs (ids op_NNN, arêtes PLAN)
- crates/obrain-viz/src/sanitize.rs — échappements partagés `dot_escape`/`dot_quote`/`mermaid_label`/`mermaid_node_id`/`fnv1a64`
- crates/obrain-viz/tests/golden.rs — anti-régression de schéma par golden files (json/dot/mermaid/graphml + variante energy)
- crates/obrain-viz/examples/print_golden.rs — régénérateur des golden files (échantillon dupliqué volontairement)
- crates/obrain-viz/tests/golden/example.{json,dot,mmd,graphml,energy.json,energy.dot,energy.mmd} — octets de référence
- crates/obrain-cli/src/viz_extract.rs — extracteur partagé CLI↔MCP (résolution id|qn GQL, BFS borné, signaux energy/ricci/weight/stale/curvature/scc)
- crates/obrain-cli/src/viz_serve.rs — serveur HTTP `obrain viz serve` (endpoints /api/viz/*, caches durables disque+mémoire, WS `subgraph_for_stream`)
- docs/FULL-OBRAIN-VIZ-ROADMAP-2026-09-08.md — feuille de route complète P0 v01→v06 + V1 A/B/C + V2 D + V3 E + V4 (état/preuves commit:ligne)
- docs/viz-projection-densite.md — théorie projection/densité : 3 échelles, sens de représentation, slide cursor, échantillonnage structurel
- docs/dashboard-viz/GRAPH.md — base graph d'organisation du dashboard (besoins→leviers, décisions datées, backlog, matrice d'état)
- docs/dashboard-viz/perimeter-8432.md — inventaire endpoints /api/viz/* + caches + relief code (StoreOverview.tsx monolithe)
- docs/dashboard-viz/CHARGE-SUBGRAPH-2026-09-15.md — mesure charge /api/viz/subgraph (v1 vs v2, octets/gzip/JSON.parse par champ)
- docs/dashboard-viz/PAYLOAD-json-graph-2026-09-15.md — charge json-graph mesurée, correction f32, encodage colonnaire proposé
- docs/dashboard-viz/VERIFICATION-2026-09-15.md — vérifications reproductibles (froid arcs A/B, intégrité read-only, 1034 tests)
- scripts/vizserve.sh — lance/relance dashboard (port 8432, IAM armé, entretien-bases.sh, trigrammes, préfixe=intervalle, préchauffage)
- scripts/vizserve-isole.sh — seconde instance port 8433, arrêt par PID du port (jamais par motif)
- scripts/obrain-viz.service — unit systemd user (OBRAIN_DEV=1, PATH pnpm, OBRAIN_PREFIXE_INTERVALLE=0, IAM, Restart=always)
- scripts/guard-viz.sh — wrapper shell de la garde DoD (exec python3 viz_guard.py)
- scripts/viz_guard.py — garde DoD : déterminisme, bien-formé, bornes/truncated, parité export_all, serve, bench p50/p95

**Relations / dépendances.**
- obrain-viz/src/lib.rs → re-exporte ir/diff/dot/graphml/json_graph/json_graph_v2/mermaid/overlay/plan/sanitize (11 modules internes)
- obrain-viz/src/{dot,mermaid,graphml}.rs → dépendent de sanitize.rs (échappements partagés) et de ir.rs (Subgraph)
- obrain-viz/src/overlay.rs → lit les champs signal de ir.rs (energy/ricci/scc_id/fsrs_*/engram_*/pheromone) et pose color_hint/size_hint consommés par dot.rs/mermaid.rs
- obrain-viz/src/diff.rs → diff_subgraphs consomme ir::Subgraph ; timeline_subgraphs enchaîne les diffs de snapshots
- obrain-viz/src/json_graph_v2.rs → dépend de json_graph.rs (Stats partagées) et de ir.rs ; v2 reste additif, v1 inchangée
- obrain-viz/src/plan.rs → parse le texte produit par LogicalOperator::explain_tree du moteur (aucune dépendance moteur, contrat texte indenté 2 espaces)
- obrain-cli/src/viz_extract.rs → extrait un Subgraph (crate obrain-viz) depuis substrate (records read_node/read_edge, CALL obrain.strongly_connected_components)
- obrain-cli/src/viz_serve.rs → sert json-graph v1/v2 depuis viz_extract + cache Arc<str> SUBGRAPH_TTL 30s + voie WS subgraph_for_stream (from_json_str_lenient)
- obrain-cli/src/mcp_server.rs → tools viz_subgraph/viz_export/node_curvature/viz_topo appellent crate::viz_extract (parité CLI↔MCP par construction)
- scripts/guard-viz.sh → exec scripts/viz_guard.py ; viz_guard.py appelle `obrain viz subgraph|export|serve` + `obrain query` (parité export_all == MATCH count)
- scripts/vizserve.sh → lance ./target/release/obrain viz serve (port 8432) + scripts/entretien-bases.sh + préchauffage /api/viz/modules|overview|arcs
- scripts/obrain-viz.service → ExecStart ./target/release/obrain viz serve (port 8432, IAM, OBRAIN_DEV=1) — chemin alternatif à vizserve.sh
- docs/dashboard-viz/CHARGE-SUBGRAPH-2026-09-15.md → documente la mesure que json_graph_v2.rs implémente (v2 −58 %)
- docs/viz-projection-densite.md → définit les exigences (slide cursor, expansion delta, échantillonnage structurel) que viz_serve.rs doit encore implémenter

**Points notables.**
- Le crate obrain-viz est volontairement sans dépendance substrate : il ne connaît que l'IR `Subgraph` ; les extracteurs substrate (viz_extract.rs, 63 Ko) et le serveur HTTP (viz_serve.rs, 466 Ko) vivent dans obrain-cli — l'invariant « une seule implémentation » ne couvre que les writers/IR, pas l'extraction.
- Déterminisme octet-à-octet structurant : tri canonique (nœuds par id, arêtes par (src,dst,type)), échappements purs, aucun horodatage par défaut — `generated_at` n'est posé que pour les overlays volatils (fsrs/engrams/pheromone) dont le non-déterminisme temporel est documenté et assumé.
- Optimisation f32 documentée au niveau bit : `ricci` et `curvature` naissent en f32 (ricci_u8 décodé) mais étaient sérialisés en f64 sur 17 chiffres ; `serialize_f32` (ryu) écrit la forme courte qui rejoue exactement le même f32 — 2 314 165 → 2 102 520 octets (−9,1 %), 24 502/24 502 arêtes sans divergence. Un bug d'aller-retour asymétrique (écrire f32, relire f64) a été corrigé via `f32_depuis_json` + test `aller_retour_des_signaux_f32_est_exact` (commit 631055e8).
- Le repli « energy vide → degré » de l'overlay Energy est une vraie trouvaille d'ingénierie : sur les stores de code, energy=0.0 partout, donc l'overlay bascule sur le degré (centralité) avec une échelle LOGARITHMIQUE (loi de puissance), jamais linéaire, pour ne pas écraser la queue (hub 11 077 arêtes vs reste <181).
- L'overlay Tier encode une politique de cache matérielle dans la couleur : masse = 0.7·énergie + 0.3·degré normalisé, quantiles → chaud #ff5c4d (RAM/réflexe), tiède #f5d63d (mmap), froid #3b4cc0 (log seul) — « la masse EST la politique de cache ».
- Le gzip S'INVERSE entre v1 et v2 : v2 non compressée est −58 % plus petite, mais v2+gzip (125 799 o) est +56 % PLUS GROSSE que v1+gzip (80 567 o) — le gzip adore la redondance de la v1. Décision de déploiement (LAN → v2, lien faible → v1+gzip), donc la v2 n'est servie que sur demande `?format=json-graph-2`.
- Intégrité prouvée par recherche de chemins d'ÉCRITURE (VERIFICATION-2026-09-15) : la couche viz ouvre en read-only (LOCK_SH/cached_ro), ricci_u8 n'est jamais persisté depuis le json-graph, aucun split() alimente une écriture — le seul défaut (aller-retour f32 asymétrique) est verrouillé par test.
- Piège opérationnel documenté trois fois : `pgrep -f 'obrain viz serve'` matche la ligne de commande de l'appelant lui-même (kill -9 du shell exécutant) ; vizserve-isole.sh arrête PAR PID DU PORT (ss -ltnp) jamais par motif.

**Trous / incohérences.**
- Incohérence de chiffres v2 : l'en-tête de json_graph_v2.rs affirme « 2 102 520 → ~0,78 Mo (−63 %) », mais docs/dashboard-viz/CHARGE-SUBGRAPH-2026-09-15.md mesure v2 = 891 083 octets (−58 %) ; et le commentaire de test `v2_is_much_smaller` cite « ~2,7× sur la charge réelle » alors que 2 102 520/891 083 ≈ 2,36×.
- json-graph v2 n'indexe pas `src`/`dst` dans `nodes` (table `strings` de 24 506 entrées pour 500 nœuds) : ~270 Ko de `dst` dupliqués restent, ~175 Ko gagnables — non fait, documenté comme « seulement si la v2 est adoptée côté client » (dette d'optimisation volontaire).
- docs/FULL-OBRAIN-VIZ-ROADMAP-2026-09-08.md a un statut en tête « EN COURS » alors que tous les workstreams (P0 v01-v06, V1 A/B/C, V2 D, V3 E, V4 A-F/G) y sont marqués LIVRÉ — statut global périmé.
- Divergence de flags entre les deux chemins de lancement : obrain-viz.service pose OBRAIN_DEV=1 (reconstruction pnpm du dashboard) et OBRAIN_PREFIXE_INTERVALLE=0 (réécriture préfixe ÉTEINTE, 15-20× plus rapide mesurée), alors que vizserve.sh pose OBRAIN_PREFIXE_INTERVALLE=1 et pas de OBRAIN_DEV — deux comportements serveur différents pour le même binaire.
- viz_guard.py calcule le p95 sur 3 runs (`sorted(xs)[int(len(xs)*0.95)]` = index 2/3) — statistiquement sans signification ; le bench « p95 » est en réalité le max des 3 mesures.
- Le bench de la garde utilise par défaut `target/debug/obrain` (OBRAIN_BIN) alors que vizserve.sh/obrain-viz.service déploient `target/release/obrain` — les latences de la garde ne sont pas celles du déploiement réel.
- L'exigence « slide cursor » de viz-projection-densite.md (expansion delta `/api/viz/expand` frontier-only, layout BFS client O(E), échantillonnage structurel hubs/ponts/plafond-par-anneau, axe du temps, préchargement hover) est listée « À faire » et n'a pas de contrepartie implémentée visible dans le périmètre inspecté — écart entre théorie de projection et serveur réel.
- Le monolithe `StoreOverview.tsx` (~1200-1300 lignes, découpage « Étape F ») et le découpage Graph3D.tsx décrits dans GRAPH.md/perimeter-8432.md vivent dans packages/obrain-dashboard/src — hors périmètre de cette tranche, donc non vérifiés sur les fichiers réels (chemins cités de seconde main).
- Les hyperarêtes DISPATCH et les tables simpliciales (triangles/cliques) ne sont pas lisibles côté substrate : viz_topo calcule les triangles en Rust sur le sous-graphe extrait, les hyperarêtes sont différées — limitation documentée mais persistante (calcul local plutôt que primitive moteur).



### 9.3 scripts/ui-* — contrat UI, promotion de version du dashboard 8432, et vérifications d'écran (portes headless-Chrome)

_Fichier : `raw/v9-3-scripts-ui.json` — 22 composants · 10 relations · 9 constats · 8 trous_


**Synthèse.** La tranche décrit le cycle complet « build → mesurer → promouvoir → servir → vérifier » du dashboard obrain (packages/obrain-dashboard, servi sur le port 8432), né après l'accident du 14-15/09/2026 où un `pnpm build` d'une lignée sans markdown/KaTeX a écrasé la page servie sans qu'aucune porte ne s'ouvre. Le pivot est scripts/ui-deploy.sh : il mesure les capacités d'un dist via scripts/ui-capacites.mjs (fermeture ATTEIGNABLE des assets depuis index.html, et non le dossier-cimetière de bundles périmés), applique la porte scripts/ui-contrat.json (2 « bloquantes » : markdown + katex ; 7 « suivies » : fratrie, ws-client, projection, ramifications, admin-comptes, descente-fractale), refuse toute régression « jamais en arrière » sauf --accepter-perte explicite, gèle une version nommée en lecture seule, bascule atomiquement le lien `current`, puis vérifie par HTTP octet-à-octet via scripts/ui-verifier.sh. Le magasin de versions vit dans $HOME/.local/share/obrain-harness/ui (releases/ + current + etat.json + journal.log + .verrou flock), et ui-bascule.sh est la seule bascule manuelle légale sous verrou. En parallèle, un second groupe de sondes est une suite de portes « qualité d'écran » qui pilotent Chrome headless (via CDP) pour mesurer la géométrie réellement rendue, jamais le SCSS : scripts/ui-mobile-porte.mjs (3 tailles, seuil tactile 44 px, chevauchements, texte < 11 px), ui-chat-porte.mjs (le champ de saisie reçoit réellement le clic et l'envoi aboutit), ui-lire-ecran.mjs (carte mesurée de l'écran sans image) et ui-mobile-capture.mjs (PNG + rapport). Un troisième groupe vérifie des comportements fonctionnels précis du monde/Cosmos : facetbar-monde.mjs (la barre de fratrie se monte dans l'état par défaut), mobile-panels-check.mjs (menus rétractables), views-controls-check.mjs (grammaire de contrôles commune aux 3 vues + continuité de sujet), emphasis-check.mjs (l'effet visuel de sélection est prouvé en luminance, pas seulement calculé) et world-add-check.mjs (ajout d'un repo GitHub enrichi depuis l'interface). Les marqueurs de capacités correspondent 1:1 à des classes DOM réelles du dashboard (FacetBar.tsx, ViewPanel.tsx, ChatPleinEcran.tsx, Projection.tsx, CosmosWeave.tsx, App.tsx). L'état promu courant est la version 20260916-152354-392ef8bd (branche harness/continuite-dans-contrat-2026-09-15), avec les 8 capacités du contrat toutes à true.


**Composants clés.**
- scripts/ui-contrat.json — contrat UI : bloquantes (markdown, katex) + suivies (fratrie, ws-client, projection, ramifications, admin-comptes, descente-fractale)
- scripts/ui-capacites.mjs — mesure des capacités d'un dist via MARQUEURS sur la fermeture atteignable des assets (index.html → chunks hashés)
- scripts/ui-deploy.sh — build (tsc+vite) ou --depuis un dist, porte du contrat, gel de version lecture seule, bascule atomique, vérif HTTP
- scripts/ui-bascule.sh — bascule délibérée vers une version déjà promue (verrou flock, réécrit etat.json, journalise, vérifie)
- scripts/ui-verifier.sh — comparaison HTTP octet-à-octet (index.html + assets + dist découplé) de ce qui est servi vs version courante
- scripts/ui-chat-porte.mjs — porte conversation : champ de saisie visible/actif/non recouvert, écriture + envoi réels, indépendance du canvas
- scripts/ui-lire-ecran.mjs — carte mesurée de l'écran (boîtes, polices, récit vertical, débordements, chevauchements) sans image
- scripts/ui-mobile-capture.mjs — capture PNG mobile (390×844) + rapport JSON (accueil, après clic chat, commandes cosmos)
- scripts/ui-mobile-porte.mjs — porte tactile sur 3 tailles (390/360/320) : débordement, cibles ≥ 44 px, chevauchements, texte < 11 px
- scripts/facetbar-monde.mjs — la barre de facettes (fratrie) se monte-t-elle dans l'état par défaut Monde (clics/grammaire clavier réels)
- scripts/mobile-panels-check.mjs — menus .ov-panel rétractables sur mobile (repli mémorisé/localStorage, replié par défaut)
- scripts/views-controls-check.mjs — grammaire de contrôles commune aux 3 vues (Cosmos/Monde/Graphe) + continuité de sujet + scène nue (ffmpeg)
- scripts/emphasis-check.mjs — « baissage » : prouve l'effet visuel de sélection en luminance (scène figée ⏸, bruit de fond mesuré, ffmpeg)
- scripts/world-add-check.mjs — ajout d'un monde GitHub depuis l'interface (clone→indexation→enrichissement) + /api/viz/signatures
- packages/obrain-dashboard/src/components/FacetBar.tsx — .facetbar / .facetbar-totaux (fratrie)
- packages/obrain-dashboard/src/components/ViewPanel.tsx — .ov-panel / .ov-panel-repli (panneaux rétractables)
- packages/obrain-dashboard/src/graph/ChatPleinEcran.tsx — .chat-plein / .chat-reglages-bouton / .chat-reglage
- packages/obrain-dashboard/src/components/Projection.tsx — .proj-row (projection multi-axes)
- packages/obrain-dashboard/src/graph/CosmosWeave.tsx — .echelle-bar, descente fractale sous-groupes
- packages/obrain-dashboard/src/app/App.tsx — .stage (conteneur de la scène canvas)
- docs/PROCESSUS-UI-8432.md — processus complet de promotion/servage/vérification de l'UI 8432
- docs/plan-j5-bascule.md — plan de la bascule (dont le « PIÈGE dist du viz » cité par ui-deploy.sh)

**Relations / dépendances.**
- ui-deploy.sh → ui-capacites.mjs (mesure CAPS_JSON), ui-contrat.json (porte bloquantes/suivies), ui-verifier.sh (vérif HTTP finale)
- ui-bascule.sh → ui-verifier.sh (re-vérifie après bascule) ; ui-bascule.sh lit le même magasin releases/current/etat.json
- ui-capacites.mjs MARQUEURS ↔ ui-contrat.json : correspondance 1:1 (markdown, katex, fratrie, ws-client, projection, ramifications, admin-comptes, descente-fractale)
- ui-verifier.sh LIENS_ATTENDUS → /home/triviere/projects/banc-obrain-j5/packages/obrain-dashboard/dist (symlink vers .../ui/current, découplage du dist servi)
- marqueurs → composants dashboard : .facetbar→FacetBar.tsx ; .ov-panel/.ov-panel-repli→ViewPanel.tsx + StoreOverview.tsx ; .chat-plein/.chat-reglages-bouton→ChatPleinEcran.tsx ; .proj-row→Projection.tsx ; .stage→App.tsx ; sous-groupes/echelle-bar→CosmosWeave.tsx + StageContinu.tsx ; peut_administrer→AdminComptes.tsx
- magasin partagé : releases/ + current + etat.json + journal.log + .verrou (flock) dans $HOME/.local/share/obrain-harness/ui, écrit par ui-deploy.sh/ui-bascule.sh, lu par ui-verifier.sh
- deux patterns de pilotage Chrome : (a) spawn chromium headless dédié (ui-chat-porte 9254, ui-lire-ecran 9251, ui-mobile-capture 9250, ui-mobile-porte 9253) ; (b) endpoint CDP externe via fetch (facetbar-monde 9423, mobile-panels-check/views-controls-check/emphasis-check/world-add-check 9248 via CDP_URL)
- views-controls-check.mjs + emphasis-check.mjs → binaire externe ffmpeg (décodage rawvideo rgb24 pour luminance/différentiel)
- world-add-check.mjs → HTTP /api/viz/signatures (présence du monde) ; mobile-panels-check.mjs → localStorage obrain.panel.* (état de repli mémorisé)
- docs/PROCESSUS-UI-8432.md documente ui-verifier.sh --attendre dans ExecStartPost du service (obrain-viz.service)

**Points notables.**
- Incohérence de périmètre : la consigne nomme « scripts/ui-*.mjs (bascule, capacites, chat-porte, lire-ecran, mobile-capture, mobile-porte, deploy, verifier) » — mais bascule, deploy et verifier sont des .sh (ui-bascule.sh, ui-deploy.sh, ui-verifier.sh), pas des .mjs ; seuls 5 fichiers ui-*.mjs existent (capacites, chat-porte, lire-ecran, mobile-capture, mobile-porte)
- Origine datée 2026-09-15, écrite dans l'en-tête de ui-capacites.mjs et ui-deploy.sh : un `pnpm build` d'une lignée sans markdown/KaTeX a écrasé dist/index.html et le redémarrage de 21:50 a rendu la substitution effective, silencieusement — d'où la règle « on ne sert plus un dossier de build, on sert une VERSION »
- Le « cimetière » d'assets (dossier accumulant les bundles de tous les builds passés) faisait mentir la première mesure : ui-capacites.mjs mesure la fermeture ATTEIGNABLE depuis index.html, seule mesure qui décrive réellement la page servie
- L'état promu courant (etat.json) est 20260916-152354-392ef8bd sur la branche harness/continuite-dans-contrat-2026-09-15, avec les 8 capacités toutes à true — le journal.log confirme une chaîne de promotions successives sur la même branche
- emphasis-check.mjs a prouvé l'effet de sélection par luminance (−24,35 % puis −24,32 %, reproductible) uniquement après avoir GELÉ la scène (chip « ⏸ figer ») : la scène morph s'anime en permanence (19–30 % de pixels), ce qui noyait tout différentiel d'image
- Deux conventions de pilotage CDP coexistent et des ports/base différents par script (8432/8462/8466/8433 ; CDP 9248/9423/9250-9254) — signe d'une absence de normalisation, chaque sonde « spawn son chrome » ou « se branche sur un chrome déjà ouvert » selon le cas
- Les marqueurs de capacités sont des chaînes fragiles volontairement choisies et documentées (ex. « sous-groupes » pour descente-fractale, vérifié discriminant 3c042268 vs f106e190) ; ui-capacites.mjs documente explicitement lesquels discriminent
- facetbar-monde.mjs suppose un « veilleur » qui reconstruit et réessaie jusqu'à 5 fois (page blanche tolérée), contrairement aux autres sondes qui échouent net après un chargement borné
- ui-verifier.sh compare index.html via /app (sous IAM, « / » renvoie 303 vers la connexion) — un détail de routage IAM documenté dans PROCESSUS-UI-8432.md

**Trous / incohérences.**
- Ports/URLs par défaut non portables et divergents : BASE par défaut 192.168.1.103:8432 (IP privée codée en dur) pour plusieurs scripts, 127.0.0.1:8462/8466/8433 pour d'autres, sans repli auto ni variable d'environnement unifiée
- Chemin Chromium codé en dur /home/triviere/.cache/ms-playwright/chromium-1234/chrome-linux64/chrome (version 1234 figée) dans 4 scripts — cassera à la prochaine mise à jour de Playwright
- ui-verifier.sh utilise /tmp/ui-verifier-ko comme compteur inter-subshell (fichier partagé) : fragile sous exécutions concurrentes (pas de PID/timestamp dans le nom)
- Aucun orchestrateur n'agrège les « portes » : scripts/check-all.sh n'invoque que smoke-backend.sh + e2e-dashboard.mjs, et ignore ui-mobile-porte/ui-chat-porte/ui-lire-ecran/facetbar-monde/mobile-panels/views-controls/emphasis/world-add
- mobile-panels-check.mjs, views-controls-check.mjs, emphasis-check.mjs, world-add-check.mjs et facetbar-monde.mjs supposent un endpoint CDP déjà ouvert (9248/9423) sans spawn de repli : ils échouent ou SKIP si aucun Chrome n'est lancé avec --remote-debugging-port
- Le chemin LIENS_ATTENDUS de ui-verifier.sh est codé en dur vers /home/triviere/projects/banc-obrain-j5/.../dist (existe aujourd'hui en symlink) : un seul worktree surveillé, non découvert dynamiquement — toute nouvelle instance servie devra être ajoutée à la main
- Le marqueur « sous-groupes » (descente-fractale) est un mot français très commun : risque de faux positif documenté par son propre commentaire, sans liste négative d'exclusion
- La consigne « ui-*.mjs (deploy, verifier, bascule) » laisse entendre des fichiers .mjs qui n'existent pas : soit la nomenclature a changé (.mjs → .sh), soit la cartographie attendue est en décalage avec le dépôt réel



### 9.4 Horloge WASM et front statique (docs/WASM-HORLOGE, scripts/build-wasm*, scripts/verify-wasm.sh, docs/javascripts, docs/assets, docs/mermaid, docs/stylesheets)

_Fichier : `raw/v9-4-wasm-horloge.json` — 28 composants · 13 relations · 12 constats · 10 trous_


**Synthèse.** La « horloge » WASM n'est pas un composant mais un correctif documenté dans docs/WASM-HORLOGE-2026-09-17.md : la std Rust n'implémente aucune horloge sur wasm32-unknown-unknown, donc Instant::now()/SystemTime::now() paniquaient (« time not implemented on this platform » → trap unreachable), rendant le module @obrain-db/wasm inutilisable dès new Database(). Le remède retenu est web-time (identique à std::time sur natif, performance.now() sur wasm), appliqué à 187 lignes dans 142 fichiers répartis sur 7 crates (obrain-common, obrain-core, obrain-substrate, obrain-engine, obrain-adapters, obrain-cognitive, obrain-reactive). scripts/build-wasm.sh porte désormais une garde (Step 1bis) qui refuse un binaire wasm contenant la chaîne du panic, et scripts/verify-wasm.sh duplique cette garde puis exécute la suite wasm-bindgen-test (tests/web.rs, 22 tests) end-to-end. Le correctif a en outre révélé que le « mode mémoire » d'Obrain était en réalité un substrat fichier dans un répertoire temporaire (std::env::temp_dir), rendu inopérant sur wasm ; il a été remplacé par un vrai substrat RAM (RamZone/RamDir/WalMedium), mesuré 22/22 verts. Le front statique servi est un site MkDocs Material (mkdocs.yml) : stylesheet extra.css, MathJax via docs/javascripts/mathjax.js, overrides/main.html, hook copy_markdown.py, assets logo-full/logo-small/playground.png et 29 sources mermaid .mmd. Le livrable wasm est crates/bindings/wasm (paquet obrain-wasm, publié @obrain-db/wasm), avec l'artefact généré pkg/obrain_wasm_bg.wasm (~2,66 Mo brut).


**Composants clés.**
- docs/WASM-HORLOGE-2026-09-17.md — note de correctif « horloge wasm » (501 lignes), non suivie par git (untracked)
- scripts/build-wasm.sh — build wasm + garde horloge (Step 1bis) + wasm-bindgen + rapport taille
- scripts/verify-wasm.sh — build + garde horloge + exécution suite wasm-bindgen-test (untracked)
- scripts/build-wasm-all.sh — produit pkg/ (variant AI) et pkg-lite/ (GQL only) + package.json
- scripts/build-wasm.ps1 — équivalent Windows du build (sans garde horloge)
- scripts/build-wasm-all.ps1 — équivalent Windows de build-wasm-all (sans garde horloge)
- crates/bindings/wasm/Cargo.toml — paquet obrain-wasm, features browser/full/ai/cognitive/languages
- crates/bindings/wasm/src/lib.rs — struct Database (#[wasm_bindgen], 2013 lignes, 41 exports, forbid unsafe_code)
- crates/bindings/wasm/src/types.rs — conversion ligne→objet JS (row_to_js_object)
- crates/bindings/wasm/src/utils.rs — set_panic_hook (console_error_panic_hook)
- crates/bindings/wasm/src/cognitive_browser.rs — binding cognitif (feature cognitive)
- crates/bindings/wasm/tests/web.rs — 22 tests wasm-bindgen-test (réparés le 2026-09-17)
- crates/bindings/wasm/pkg/obrain_wasm_bg.wasm — artefact généré (2 657 621 octets, 2026-09-17)
- crates/bindings/wasm/pkg/obrain_wasm.js + obrain_wasm.d.ts — glue générée par wasm-bindgen
- crates/bindings/wasm/obrain_wasm.js — stub loader (remplacé au build, throw si non construit)
- crates/bindings/wasm/obrain_wasm.d.ts — .d.ts de référence (obsolète : déclare exportSnapshot/importSnapshot)
- crates/bindings/wasm/package.json + package-lite.json — @obrain-db/wasm et @obrain-db/wasm-lite (v0.5.24)
- crates/bindings/wasm/README.md — doc package (annonce snapshot export/import et 513/531 KB gzip)
- docs/api/wasm/index.md — référence API WASM (~660 KB gzip, documente exportSnapshot/importSnapshot)
- docs/ADR-2026-09-17-SUBSTRAT-EN-MEMOIRE.md — décision du vrai substrat en mémoire
- mkdocs.yml — config site statique (theme material, hooks, plugins, extra_css/extra_javascript)
- docs/stylesheets/extra.css — thème custom (Unified Minimal, light/dark, tabs, hero, tables)
- docs/javascripts/mathjax.js — config MathJax (arithmatex, inlineMath/displayMath)
- docs/overrides/main.html — override Jinja (banner announce commenté)
- docs/hooks/copy_markdown.py — hook MkDocs (copie CHANGELOG.md et md→site/md/)
- docs/assets/logo-full.png, logo-small.png, playground.png — images du site
- docs/includes/abbreviations.md — snippets auto-append pymdownx.snippets
- docs/mermaid/*.mmd — 29 sources mermaid (banc-obrain-*, engine-*, moteur-*, obrain-chat, etc.), non suivies

**Relations / dépendances.**
- scripts/build-wasm.sh -> cargo build -p obrain-wasm --target wasm32-unknown-unknown --profile minimal-size -> wasm-bindgen --target web -> crates/bindings/wasm/pkg/
- scripts/build-wasm.sh (Step 1bis garde horloge) -> strings -a obrain_wasm.wasm | grep 'time not implemented' ; sort en erreur (exit 1) si présent
- scripts/build-wasm-all.sh -> ./scripts/build-wasm.sh --features ai (pkg/) puis --out-dir pkg-lite (lite), écrit package.json @obrain-db/wasm et @obrain-db/wasm-lite
- scripts/verify-wasm.sh -> cargo build + garde horloge + CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner cargo test -p obrain-wasm --test web (22 tests)
- scripts/verify-wasm.sh -> lit la version wasm-bindgen dans Cargo.lock puis la cherche dans ~/.cache/.wasm-pack/wasm-bindgen-*/ (0.2.128)
- crates/bindings/wasm (obrain-wasm) -> obrain-engine (features gql, regex-lite), obrain-common, obrain-bindings-common (../common), wasm-bindgen, serde-wasm-bindgen, js-sys, console_error_panic_hook
- obrain-wasm features optionnelles -> obrain-cognitive/obrain-reactive (cognitive) et obrain-core (rdf), wasm-bindgen-test (dev)
- web-time remplace std::time::Instant/SystemTime dans 7 crates (obrain-common, obrain-core, obrain-substrate, obrain-engine, obrain-adapters, obrain-cognitive, obrain-reactive)
- crates/bindings/wasm/src/lib.rs (Database::new) -> obrain_engine::ObrainDB::new_in_memory() -> substrat mémoire (RamZone/RamDir/WalMedium, obrain-substrate)
- mkdocs.yml -> extra_css stylesheets/extra.css, extra_javascript javascripts/mathjax.js + polyfill.io + cdn mathjax, custom_dir docs/overrides, hooks docs/hooks/copy_markdown.py
- mkdocs.yml markdown_extensions -> pymdownx.arithmatex (MathJax), pymdownx.superfences custom_fence mermaid (rendu des blocs ```mermaid)
- docs/api/wasm/index.md et README.md documentent exportSnapshot()/importSnapshot() — surface RETIRÉE par le commit b0f5bd11 (T17 Wave A)
- docs/mermaid/*.mmd — aucun fichier .md ne référence ces sources .mmd (15 .md utilisent des blocs ```mermaid inline)

**Points notables.**
- docs/WASM-HORLOGE n'est pas un répertoire : le chemin réel est docs/WASM-HORLOGE-2026-09-17.md, et ce fichier est UNTRACKED (git status: ??) ainsi que scripts/verify-wasm.sh.
- Le défaut d'horloge était « invisible à la compilation » : le module se construisait (exit 0) et se chargeait, mais trapait à l'exécution (RuntimeError: unreachable) dès new Database().
- La garde horloge de build-wasm.sh a été éprouvée dans les deux sens (déclenchée sur un module témoin avec Instant::now(), muette après correctif) — une garde jamais vue déclencher ne prouve rien.
- Le correctif d'horloge a révélé un défaut masqué plus profond : le « mode mémoire » d'Obrain était un substrat FICHIER dans std::env::temp_dir(), donc inopérant sur wasm32-unknown-unknown ; il est devenu un vrai substrat RAM (RamZone/RamDir/WalMedium), mesuré 22/22 tests wasm verts.
- La suite de tests wasm (tests/web.rs) était morte depuis le commit b0f5bd11 (retrait export_snapshot/import_snapshot) : 7 erreurs de compilation ; réparée en lisant JsError.message via Reflect::get et en réécrivant 2 tests de snapshot.
- Dérive d'API snapshot non résolue : crates/bindings/wasm/obrain_wasm.d.ts (crate, lignes 68-90), docs/api/wasm/index.md (§Snapshots) et README.md documentent encore exportSnapshot()/importSnapshot(), alors que pkg/obrain_wasm.d.ts régénéré ne les expose plus (surface retirée par T17 Wave A).
- Leçon de méthode consignée : un exit 0 derrière un tube (cargo build | tail) est celui de tail, pas de cargo — mesures reprises avec set -o pipefail / PIPESTATUS.
- Épinglage outillage : le wasm-bindgen du PATH est 0.2.114 mais le crate exige 0.2.128, trouvé dans ~/.cache/.wasm-pack/wasm-bindgen-4f04094f43ba0220/ (avec son test-runner).
- Dérive de taille de bundle : le pkg/ régénéré pèse ~863 KB gzip, là où docs/api/wasm/index.md annonce « ~660 KB gzipped » et README « 513/531 KB » ; build-wasm.sh signale « Exceeds 400 KB gzip target ».
- Le module n'a été exécuté que sous Node (jamais dans un vrai navigateur) ; la cible wasm32 est une bibliothèque (obrain-wasm), il n'existe pas de binaire CLI wasm.
- Les 29 fichiers docs/mermaid/*.mmd ne sont pas suivis par git (0 tracked) et ne sont référencés par aucun .md : sources mermaid orphelines, probablement générées par un outil de cartographie/vision.
- mkdocs.yml référence assets/favicon.ico (theme.favicon) mais ce fichier n'existe pas dans docs/assets/ (seuls logo-full.png, logo-small.png, playground.png).

**Trous / incohérences.**
- Chemin du périmètre docs/WASM-HORLOGE inexistant en tant que répertoire ; seul docs/WASM-HORLOGE-2026-09-17.md existe, et il est non committé (untracked).
- API snapshot fantôme : exportSnapshot()/importSnapshot() documentés (obrain_wasm.d.ts crate, docs/api/wasm/index.md, README.md) mais retirés du code (commit b0f5bd11) et absents de pkg/obrain_wasm.d.ts — documentation/type defs obsolètes à nettoyer ou API à réintroduire (T17b).
- Asymétrie de garde : scripts/build-wasm.ps1 et build-wasm-all.ps1 (Windows) n'embarquent PAS la garde horloge (Step 1bis) présente dans les .sh — un build Windows peut produire un module qui trappe.
- docs/assets/favicon.ico référencé par mkdocs.yml mais absent du dépôt.
- docs/mermaid/*.mmd : 29 fichiers non suivis et non référencés — à rattacher, supprimer, ou documenter leur provenance (générateur ?).
- Taille de bundle incohérente entre les sources : ~660 KB (index.md), 513/531 KB (README), 400 KB (cible build-wasm.sh), 863 KB (pkg/ régénéré mesuré) — contribution propre du substrat mémoire non isolée.
- Le mode mémoire n'est pas checkpointable (mesuré : flush + std::process::id() panique sur wasm) et ne persiste rien ; aucun snapshot substrat natif n'existe (chantier T17b).
- Colonnes blob (blob_column.rs:647-648) non exercées en mode RAM : écriraient toujours sur path() et échoueraient.
- Aucune exécution en navigateur réel (Node uniquement) ; Cloudflare Workers marqué « Untested » dans README.md.
- README.md annonce « Snapshot export/import for IndexedDB persistence » comme fait [x], contredit par le retrait de l'API et l'absence de persistance du mode mémoire.



### 9.5 Docs UX / Dashboard obrain (8432)

_Fichier : `raw/v9-5-docs-ux-dashboard.json` — 35 composants · 12 relations · 8 constats · 9 trous_


**Synthèse.** L'UX cible d'obrain est définie par docs/UX-UI-SKILL.md : une compétence de design fondée sur le modèle mental (« où suis-je, que vois-je, que puis-je faire »), les 5 états de contrôle, la gestalt, le LOD densité/lisibilité et une checklist d'intégration stricte (tokens SCSS uniquement, a11y aria/focus-visible, cibles ≥44 px, 3 tailles d'écran, données réelles kernel 1,5 M). Le dashboard (packages/obrain-dashboard, React 18 + TS + Jotai + SCSS + Three.js) est une surface de représentation du graphe servie en lecture seule par `obrain viz serve` (port 8432), embarquable par URL/iframe et pilotée par une API json-graph v1 gelée (docs/DASHBOARD-EMBED.md). L'état consolidé (docs/ETAT-DASHBOARD-2026-09-09.md) décrit un chrome canvas multi-faces (Cosmos/Monde/Graphe/Signaux), un graphe 3D InstancedMesh à densité D0-D3 pilotée caméra, une facette de fratrie et un cache RO par store. Le chat est devenu un agent sur le graphe (docs/CHAT-AGENT-SUR-LE-GRAPHE-2026-09-11.md) doté de six outils moteur (obrain_query/echantillon/voisinage/mondes/recettes/recette), d'une boucle bornée, d'un ancrage synaptique fiable et d'une densification échantillonnée annoncée honnêtement. Le serveur a été durci protocole + vitesses (docs/8432-PROTOCOLE-ET-VITESSE-2026-09-13.md) : traversée de chemin colmatée, gzip/Vary, keep-alive borné, HEAD, ETag/304, et caches corrigés (arcs 2,5 s → 2,6 ms). La livraison UI a été industrialisée (docs/PROCESSUS-UI-8432.md) : on ne sert plus un dossier de build mais une VERSION nommée, gelée, gatée par capacités bloquantes/suivies et vérifiée après coup, avec verrou flock anti-promotions concurrentes. Le savoir de reprise est tissé vers ECHELLE-MULTI-NIVEAUX-REPRISE, CHAT-ET-ENGINE-REPRISE et l'ADR-2026-09-16-ECHELLE-MULTI-NIVEAUX.


**Composants clés.**
- docs/UX-UI-SKILL.md — compétence de design : cadre de pensée, checklist d'intégration, règles graphe/data-viz, process de travail
- docs/ETAT-DASHBOARD-2026-09-09.md — synthèse architecture dashboard (faces, Graph3D, FacetBar, endpoints, perf, limites)
- docs/DASHBOARD-EMBED.md — contrat d'embarquement iframe/URL + API RO (/api/info, /api/viz/*, /api/functions, /api/query)
- docs/BACKLOG-OBRAIN-DASHBOARD.md — graphe de tâches P1/P2/P3 avec commits de preuve (canvas unifié bloc C pending)
- docs/PROCESSUS-UI-8432.md — processus de versionnage UI (ui-deploy/ui-bascule/ui-verifier, capacités, réunification de lignées)
- docs/8432-PROTOCOLE-ET-VITESSE-2026-09-13.md — protocole HTTP (14 échecs/15) + vitesses endpoints mesurées
- docs/CHAT-AGENT-SUR-LE-GRAPHE-2026-09-11.md — chat agent, 6 outils moteur, ancrage synaptique, densification, mesures
- docs/CHAT-2026-09-15.md — historique localStorage, langue de la question, reasoning_effort (xhigh/medium/low), appels d'outils dans le texte
- docs/CHAT-ET-ENGINE-REPRISE-2026-09-16.md — reprise : chat découplé du graphe, conversations.ts, coupe-circuit répétition, client_parti, budget
- docs/CAPACITES-AGENT-OBRAIN-2026-09-05.md — capacités moteur interrogées par l'agent (CALL obrain.search_*/relief/compare/enrich)
- packages/obrain-dashboard/src/app/App.tsx — chrome canvas : topbar, quickbar, dock, fil d'Ariane, chat flottant, montage des faces
- packages/obrain-dashboard/src/app/store.ts — atomes Jotai (stores, centres, échelle, couches, overlay, historique, résultats moteur)
- packages/obrain-dashboard/src/lib/api.ts — transport HTTP/WS (getCached, streamSubgraph, fetchSubgraph, resoudreAncres)
- packages/obrain-dashboard/src/graph/Graph3D.tsx — graphe 3D InstancedMesh, segments vertexColors, labels LOD, densité caméra
- packages/obrain-dashboard/src/graph/CosmosWeave.tsx — face Cosmos, nuée de points par coquilles, fratrie, repli centre
- packages/obrain-dashboard/src/graph/ChatPleinEcran.tsx — chat plein écran, flux SSE, bouton flottant, outils visibles
- packages/obrain-dashboard/src/graph/conversations.ts — module pur : conversations multiples, titre, dossier, épingle, migration
- packages/obrain-dashboard/src/graph/segments.ts — module pur : découpe des appels d'outils aux positions du texte
- packages/obrain-dashboard/src/graph/RenduRiche.tsx — rendu markdown + KaTeX des réponses de l'agent
- packages/obrain-dashboard/src/components/FacetBar.tsx — barre bas-centre : fratrie, co-fichiers, sœurs, cycle, homonymes
- packages/obrain-dashboard/src/components/StoreOverview.tsx — face Monde, galaxie 1 point=1 item, couches énergie/organique
- packages/obrain-dashboard/src/components/Projection.tsx — projection multi-axes (projectAxes), ancrage sur le graphe
- crates/obrain-cli/src/viz_serve.rs — serveur HTTP std-only (routes /api/*, gzip, ETag, cache RO, garde chemin_sous)
- crates/obrain-cli/src/engine.rs — chaîne LLM/tool-calling du chat, budget par question, arrêt client_parti
- crates/obrain-cli/src/ancrage.rs — résolution du centre/self depuis la base synaptique (World.path)
- crates/obrain-garde/src/repetition.rs — coupe-circuit de répétition dégénérée (module pur)
- scripts/ui-deploy.sh — build, gate capacités, gel de version, promotion sous verrou flock
- scripts/ui-bascule.sh — seule bascule correcte de la version servie (réécrit etat.json, journalise)
- scripts/ui-verifier.sh — compare les octets servis à la version courante (branché ExecStartPost)
- scripts/ui-capacites.mjs — mesure les capacités d'un dist depuis la fermeture atteignable d'index.html
- scripts/ui-contrat.json — capacités bloquantes (markdown, katex) et suivies (fratrie, projection, ws-client)
- scripts/protocole-check.mjs — porte de non-régression protocole HTTP (15 contrôles)
- scripts/vitesse-8432.mjs — porte de mesure de vitesses endpoints (clé neuve/chaud)
- scripts/chat-ancrage-check.mjs — porte du geste utilisateur (ancrage chat, monde ajouté à chaud)
- scripts/fratrie-check.mjs — porte fratrie (CDP), facetbar-monde.mjs — porte barre de facettes face Monde

**Relations / dépendances.**
- docs/UX-UI-SKILL.md ← méthode de référence déclarée par docs/BACKLOG-OBRAIN-DASHBOARD.md et docs/ETAT-DASHBOARD-2026-09-09.md
- packages/obrain-dashboard (front) → crates/obrain-cli/src/viz_serve.rs via API lecture seule /api/stores, /api/info, /api/viz/*, /api/functions, /api/query
- crates/obrain-cli/src/ancrage.rs ← résout centre + self depuis /api/viz/weave (base synaptique World.path), repli slug canonique
- crates/obrain-cli/src/engine.rs ↔ crates/obrain-garde/src/repetition.rs (coupe-circuit) + post_sse (rappel continuer/arrêter)
- packages/obrain-dashboard/src/graph/ChatPleinEcran.tsx → packages/obrain-dashboard/src/graph/conversations.ts + segments.ts (modules purs testés sans navigateur)
- docs/CHAT-AGENT-SUR-LE-GRAPHE-2026-09-11.md → scripts/chat-ancrage-check.mjs (porte du geste utilisateur)
- docs/8432-PROTOCOLE-ET-VITESSE-2026-09-13.md → scripts/protocole-check.mjs + scripts/vitesse-8432.mjs
- docs/PROCESSUS-UI-8432.md → scripts/ui-deploy.sh / ui-bascule.sh / ui-verifier.sh / ui-capacites.mjs / ui-contrat.json
- docs/PROCESSUS-UI-8432.md §17 → docs/ECHELLE-MULTI-NIVEAUX-REPRISE-2026-09-16.md + docs/CHAT-ET-ENGINE-REPRISE-2026-09-16.md + docs/ADR-2026-09-16-ECHELLE-MULTI-NIVEAUX.md (tissage)
- packages/obrain-dashboard/src/lib/api.ts ← point de portage unique WS↔HTTP (getCached chargeur de remplacement, streamSubgraph, resoudreAncres)
- docs/CAPACITES-AGENT-OBRAIN-2026-09-05.md → CALL obrain.search_delta/surprise/by_role/conflicts/causes/relief + obrain compare/enrich (moteur)
- Invariant transverse : dashboard en lecture seule (cached_ro) ; écriture via journal append-only rejoué par le writer unique import_events (MCP) — mono-écrivain

**Points notables.**
- Le chat est un véritable agent : 6 outils moteur exécutés contre la base et rendus visibles en SSE (nom, args, monde, lignes, durée, erreur), boucle bornée, tool_choice forced quand un mot significatif est absent du contexte (anti-invention).
- La régulation de la verbosité n'était PAS le prompt mais le reasoning_effort du template llama.cpp : le chat subissait le défaut xhigh ; le passage à medium = ×1,53 plus rapide pour la même réponse juste, et 'non' = ×9 (4,3 s) tout en appelant quand même l'outil.
- Trois identités de Self divergeaient (self:triviere-dsh synaptique vs self:agent MCP vs self:agent:state:v72 docs) — l'ancrage du chat est désormais résolu depuis la base synaptique.
- Deux instruments de mesure étaient faux et l'ont dit : compter les réponses par regex ancrée (2 réponses sans séparateur) et mesurer la compression par fetch (dégonfle seul) — on lit Content-Length annoncé.
- La requête fetchLabelPairs est documentée comme FAUSSE (label de la cible recopié : Turn-[HAS_STEP]->Step devient Step,Step) et ~370 ms plus chère ; réécriture = décision de conception, non appliquée.
- Le serveur a été repassé derrière IAM (OBRAIN_IAM=1) : / répond 303→/login, /api/** répond 401 — les contrôles de page deviennent SKIP avec raison, pas FAIL.
- L'auto-guérison des colonnes de degrés incohérentes (48 M arêtes annoncées pour 28 k réelles) est refusée en lecture seule : le dashboard refuse le classement des hubs en le disant ; réparation via obrain repair-degrees service arrêté.
- Toute l'UX est vérifiée par des portes .mjs rejouables contre CDP/HTTP, avec une leçon répétée : quand le transport change (fetch→WebSocket), l'instrument doit changer avec lui (projection-check.mjs ne voyait pas le WS).

**Trous / incohérences.**
- QueryConsole.tsx est référencé dans docs/ETAT-DASHBOARD-2026-09-09.md §1 mais absent de l'arbre packages/obrain-dashboard/src (grep packages/ = 0) — renommé (Inspector.tsx/Projection.tsx ?) ou doc dérivée.
- docs/BACKLOG-OBRAIN-DASHBOARD.md est figé au 2026-09-09 : canvas unifié Monde↔Graphe (bloc C) toujours PENDING, vérif capture utilisateur BLOQUÉE, pas de statut post-09-09.
- Drift doc : docs/PROCESSUS-UI-8432.md §6 liste le bug 'ISOLÉ_PORT' comme ouvert, mais scripts/acceptation-8432.sh utilise déjà ISOLE_PORT=8433 (renommage appliqué) — liste 'reste ouvert' périmée.
- Registres moteur label→count et type-d'arête→count non écrits (545-563 ms et 684-723 ms par exécution) — bloqués car une autre session édite obrain-engine/obrain-substrate en parallèle.
- Couche sémantique absente : vector-index non compilé, n.embedding NULL partout → pas de recherche sémantique ; décrit comme le plus gros manque pour le chat.
- Degrés non interrogeables en GQL (n.deg_out=0) : le classement des hubs par degré est refusé sur runtime-dev/prod (registres incohérents), retombant sur un parcours à 14,1 s refusé.
- Détection du départ du client tardive (au moment où l'écriture SSE échoue) — à mesurer si l'arrêt immédiat est requis.
- fratrie-check.mjs ne lance aucun navigateur (connexion à un CDP externe, 127.0.0.1:9248) : la porte meurt en ECONNREFUSED sans chrome pré-démarré.
- fetchLabelPairs (api.ts:285) et la requête 'labels(a)[0], labels(b)[0] GROUP BY' restent non corrigées faute de décision de conception (coût 1,28 s sur binaire fusionné).




## Vague 10 — V10 Synthèse

### 10.1 Roadmap & phases restantes (roadmap.md, plan d'améliorations protégées, design des chantiers restants, FULL-OBRAIN-VIZ-ROADMAP, backlog dashboard)

_Fichier : `raw/v10-1-roadmap.json` — 21 composants · 12 relations · 8 constats · 9 trous_


**Synthèse.** Le périmètre roadmap d'obrain est éclaté en documents de statuts hétérogènes : un docs/roadmap.md public daté « March 2026 » (échelle de versions 0.1 Foundation → 0.5 Beta courante → 0.6 RC → 1.0 Stable) qui n'est pas synchronisé avec le travail cognitif/visualisation de septembre ; un plan d'améliorations protégées moteur (PLAN-AMELIORATIONS-PROTEGEES-2026-09-04.md) structuré en 10 contraintes C1-C10 et phases 0→5, quasi intégralement livré ; un design des chantiers restants (design-phases-restantes-2026-09-04.md) cadrant P3c/P4b/P5a/P5b ; et deux feuilles de route produit récentes (FULL-OBRAIN-VIZ-ROADMAP-2026-09-08.md et BACKLOG-OBRAIN-DASHBOARD.md). Le plan moteur est livré à ~90 % (P1a C5 fenêtres Z, P2a BM25 ≤1 ms, P2b C2 RAG topologique, P3a KernelEnricher, P3b κ→communautés, P4a ingestion native, P4b ordre temporel, P5a C3 fenêtre cognitive) ; ne restent que P5b (C6 mémoire de sessions cognitive) EN ATTENTE et P3c (plan sémantique) REPORTÉ par gate de mesure (−2,4 % de localité spectrale vs topologique). La feuille de route viz est complète (P0 v01-v06 → EXIT, V1 A/B/C overlays volatils/compare/graphml, V2 D explain, V3 E timeline, V4 mise en service) ; les reliquats sont G2b (exposition ffs/Caddy, décision opérateur), V5 dashboard-ui DGX différé, les hyperarêtes DISPATCH différées et le rendu binaire réel graphviz/mermaid (binaires absents de l'environnement). Le backlog dashboard reste le chantier le plus ouvert (canvas unifié « bloc C » gros chantier, flux d'exécution entre 2 ancres PENDING, vérif capture utilisateur BLOQUÉ). Le véritable état de fait consolidé vit dans docs/ledger-ameliorations-2026-09-04.md (106 Ko), un tableau daté commit-par-commit qui sert de registre de chantiers plus fiable que les roadmaps formelles. Il n'existe ni répertoire docs/roadmap/ ni roadmap racine unique réconciliant l'échelle de versions publique et les phases opérationnelles de septembre.


**Composants clés.**
- docs/roadmap.md — roadmap publique (0.1 Foundation, 0.2 Performance, 0.3 AI Compatibility, 0.4 Developer Accessibility, 0.5 Beta courante, 0.6 RC, 1.0 Stable) ; « Last updated: March 2026 », référence changelog.md et l'org GitHub this-rs
- docs/PLAN-AMELIORATIONS-PROTEGEES-2026-09-04.md — plan moteur protégé : 10 contraintes C1-C10 (additivité, zéro retour cassé, perf non régressive, RSS borné, un commit/chantier, dette zéro, chiffres datés, opt-in, pas de déploiement prod, invariants), phases 0→5, chantiers P1a-P5b avec DoD, séquencement et ledger
- docs/design-phases-restantes-2026-09-04.md — design des chantiers restants P3c (plan sémantique), P4b (ordre temporel journal), P5a (C3 fenêtre cognitive), P5b (C6 mémoire de sessions) ; ancres fichier:ligne vérifiées à HEAD cff0e200
- docs/FULL-OBRAIN-VIZ-ROADMAP-2026-09-08.md — feuille de route complète de la couche de restitution : P0 v01→v06, V1 A/B/C, V2 D, V3 E, V4 A-G2 mise en service, état par round 0-11 avec preuves commit:ligne et 10 décisions à trancher + résolutions actées
- docs/BACKLOG-OBRAIN-DASHBOARD.md — graphe de tâches dashboard (P1/P2/P3) avec dépendances, statuts (DONE/EN COURS/PENDING/BLOQUÉ) et commits de preuve
- docs/ledger-ameliorations-2026-09-04.md — registre exécutif daté (106 Ko) : une ligne par chantier (date | chantier | commit | tests | bench avant→après | golden diff | RSS | notes), incluant la campagne CDC/HKS/cdc_tail/open hors plan initial
- docs/continuity-spectral-spatial.md — design doc CH6 (continuité spectrale/spatiale) : reconstruction de contexte par signature spectrale + localité Hilbert, hooks context_build/memory_recall
- docs/cognitive/canonical-features-pipeline.md — pipeline de features canoniques (Hilbert/Kernel, Ph2 livré) référencé par le plan moteur
- docs/rfc/substrate/zorder-superposition.md — RFC z-order/superposition (propose C2 puis C5), source du plan protégé
- docs/zorder-RAPPORT-FINAL.md + docs/CHECKPOINT-ZORDER-2026-09-04.md + docs/RELEASE-RC-M6-ZORDER-2026-09-07.md — chaîne de livraison zorder (Hilbert physique / Z-order logique / cascade 64d)
- docs/ETAT-DASHBOARD-2026-09-09.md + docs/DASHBOARD-EMBED.md — état et intégration dashboard (compléments du backlog)
- docs/dashboard-viz/GRAPH.md + perimeter-8432.md + CHARGE-SUBGRAPH-2026-09-15.md + PAYLOAD-json-graph-2026-09-15.md + VERIFICATION-2026-09-15.md — dossier de suivi dashboard-viz (endpoints /api/viz/*, charge, payload json-graph)
- crates/obrain-viz/ — crate de restitution (ir.rs, json_graph.rs, overlay.rs, dot.rs, mermaid.rs, graphml.rs, diff.rs, plan.rs, sanitize.rs) livré par la roadmap viz
- crates/obrain-cli/src/viz_extract.rs + viz_serve.rs — extracteur partagé CLI↔MCP et serveur HTTP obrain viz serve (implémentation unique CLI↔bridge)
- crates/obrain-cli/src/mcp_server.rs — bridge MCP : expose context_build, memory_recall, import_events, viz_subgraph/viz_export/node_curvature/viz_topo
- crates/obrain-substrate/src/geometric.rs — compute_ricci_fast / refresh_all_ricci / compute_hks_descriptors (formes persistées ricci_u8)
- crates/obrain-substrate/src/hilbert.rs — grid_coords_from_features / compute_hilbert_permutation / bulk_sort_by_hilbert (plan physique Hilbert/Z-order)
- crates/obrain-cognitive/src/thinker/kernel_enricher.rs — KernelEnricher (P3a, ThinkerKind::KernelEnricher) + obrain-cognitive/src/engram/spectral.rs SpectralEncoder
- crates/obrain-cognitive/src/stigmergy.rs — PheromoneMap/évaporation (overlays volatils V1-A de la roadmap viz)
- scripts/guard-viz.sh + scripts/viz_guard.py — garde DoD de la roadmap viz (déterminisme, bien-formé, parité export_all, bench p50/p95)
- crates/obrain-engine/src/window_procedures.rs — CALL obrain.window_2d (P1a C5) et obrain.window_2d_energy (P5a C3)

**Relations / dépendances.**
- PLAN-AMELIORATIONS-PROTEGEES-2026-09-04.md → design-phases-restantes-2026-09-04.md (le design cadre P3c/P4b/P5a/P5b avant implémentation) et → ledger-ameliorations-2026-09-04.md (chaque chantier a sa ligne de résultat datée)
- design-phases-restantes §1 P3c / §4 P5b → docs/continuity-spectral-spatial.md (fil rouge CH6, rappel par signature spectrale + localité Hilbert) et → docs/cognitive/canonical-features-pipeline.md (cascade hilbert_features 64d, eigvecs 0-1)
- PLAN §2.1 Phase 0 → docs/bench-verify/ + docs/golden/2026-09-04/ (baseline gelée + golden outputs des 4 surfaces GQL/CLI/NAPI/bridge)
- FULL-OBRAIN-VIZ-ROADMAP P0 v01-v06 → crates/obrain-viz/* (IR + json-graph v1 + writers DOT/Mermaid/GraphML + overlays) → obrain-cli/src/viz_extract.rs (CLI) → obrain-cli/src/mcp_server.rs (bridge MCP, parité par construction)
- FULL-OBRAIN-VIZ-ROADMAP V4-G2a → obrain-cli/src/viz_serve.rs (serveur obrain viz serve) → scripts/vizserve.sh + obrain-viz.service (déploiement port 8432/8433) — tracé dans docs/dashboard-viz/
- BACKLOG-OBRAIN-DASHBOARD → docs/UX-UI-SKILL.md (méthode) et → packages/obrain-dashboard (cible Vue/Vite, suivi par todo+commits)
- P3b run_ldleiden_batch_ricci → geometric.rs compute_ricci_fast/ricci_u8 (κ comme poids dans LDleiden, A/B boost=0 bit-identique)
- P4b read_session_journal → engine/src/journal_zone.rs (zone substrate.journal.<session>) et → harness session-persistence (re-tri par seq à supprimer, action harness séparée)
- P5a obrain.window_2d_energy → window_procedures.rs (filtre énergie inline Q1.15 sur la fenêtre C5, seuil 0 = C5 inchangée)
- V1-A overlays volatils (fsrs/engrams/pheromone) → obrain-cognitive/src/engram/decay.rs (FSRS-5) + stigmergy.rs (phéromones) + engram/mod.rs (Hebbian Layer 0)
- ledger-ameliorations-2026-09-04.md (campagne CDC/HKS/cdc_tail, hors plan) → docs/rfc/substrate/multi-writer.md (RFC multi-writer) et docs/PLAN-TISSAGE-PERIMETRE-2026-09-06.md (carte du périmètre)
- docs/roadmap.md (publique) ↔ sans lien avec les phases de septembre : divergence non réconciliée (voir gaps)

**Points notables.**
- Le plan moteur protégé est livré à ~90 % et le reste est documenté honnêtement : P3c Étape B (Plan2D::Spectral) a été REPORTÉE car la mesure B3 montrait un recul de localité (~+2,4 % : topologique 8753,3 vs spectral 8962,7) — la gate de mesure a fonctionné, tradition « lecture honnête » du dépôt
- P5b (C6 mémoire de sessions cognitive) est le SEUL chantier du plan moteur resté EN ATTENTE, sans commit d'implémentation — c'est le fil rouge CH6 du design continuity-spectral-spatial, cadré mais non réalisé (payload context_build additif rétro-compatible à enrichir avec le rappel spectral-ensembles)
- La roadmap viz a atteint l'exhaustivité en 11 rounds sur une seule journée (08/09) : P0 EXIT + V1 A/B/C + V2 D + V3 E + V4 A-G2a LIVRÉS, avec une implémentation unique partagée CLI↔bridge MCP (parité garantie par construction) et une garde DoD 11/11 puis 13/13
- Le ledger-ameliorations-2026-09-04.md (106 Ko) déborde largement le plan initial : il consigne aussi une campagne de performance « à froid » (CDC by_ts, cdc_tail, load par blocs, HKS cache, seuil auto-checkpoint WAL 512 Mo → 8 Mo) qui n'apparaît dans aucune roadmap formelle
- Le docs/roadmap.md public est daté « March 2026 » et référence l'org GitHub this-rs (this-rs/obrain-server, this-rs/obrain-web…) : c'est un artefact d'upstream/fork non synchronisé avec la réalité de septembre (aucune mention des primitives cognitives energy/synapse/scar/fabric, du GDS, du MCP, ni des phases zorder/viz)
- Le backlog dashboard est le chantier le plus ouvert : canvas unifié = « bloc C » qualifié de « gros chantier », flux d'exécution entre 2 ancres PENDING, vérif capture utilisateur BLOQUÉ (dépend d'une capture humaine)
- Les références croisées documentaires sont majoritairement exactes (fichier:ligne vérifiés à HEAD cff0e200), mais le chemin canonical-features-pipeline.md cité dans le plan pointe vers docs/cognitive/canonical-features-pipeline.md en réalité (incohérence de chemin)
- La branche HEAD courante (consolidation/tronc-2026-09-15-plus-fusion) diffère des branches d'autorité des roadmaps (feat/zorder, feat/llama-obrain-index-persist) : la feuille de route est rédigée sur des branches features, consolidées ensuite

**Trous / incohérences.**
- Aucun répertoire docs/roadmap/ n'existe (mentionné dans le périmètre) ; le seul fichier roadmap est docs/roadmap.md, et aucun roadmap.md à la racine du dépôt
- docs/roadmap.md public est périmé/désynchronisé (March 2026, échelle 0.5/0.6/1.0, références this-rs) : il ignore totalement le travail cognitif, GDS, MCP, viz et zorder de septembre — pas de réconciliation avec les phases opérationnelles
- Absence de roadmap maîtresse unique : le plan moteur (PLAN-AMELIORATIONS-PROTEGEES), la roadmap viz (FULL-OBRAIN-VIZ-ROADMAP) et le backlog dashboard coexistent sans document agrégateur consolidant les phases restantes transverses
- P5b (C6 mémoire de sessions cognitive) est le seul chantier moteur non implémenté : aucun commit, statut « EN ATTENTE / round dédié » — reste à réaliser
- P3c plan sémantique : Étape B non implémentée (reportée par gate de mesure) mais encore listée comme chantier dans design-phases-restantes — statut ambigu entre « reporté » et « en attente d'un meilleur plan »
- FULL-OBRAIN-VIZ-ROADMAP reliquats non clos : G2b (exposition ffs/Caddy) = décision opérateur hors périmètre, V5 dashboard-ui DGX différé, hyperarêtes DISPATCH différées, et rendu binaire réel graphviz/mermaid jamais exécuté (binaires absents de l'environnement)
- Référence de chemin incohérente : PLAN-AMELIORATIONS-PROTEGEES cite « canonical-features-pipeline.md » à la racine docs/ alors que le fichier réel est docs/cognitive/canonical-features-pipeline.md
- Backlog dashboard : tâches BLOQUÉ (vérif capture utilisateur) et PENDING (flux d'exécution entre 2 ancres) + « bloc C » canvas unifié — la résolution des dépendances (AGENT A arcs, AGENT B couches) n'est pas datée dans le backlog lui-même (preuves « à intégrer »)
- Le ledger-ameliorations-2026-09-04.md n'est pas un fichier de phase (table plate) : pas d'index de statut consolidé « fait/reporté/en attente » par chantier — la reconstruction de l'état demande une lecture intégrale du tableau



### 10.2 Écosystème, manifeste et releases

_Fichier : `raw/v10-2-ecosystem-releases.json` — 24 composants · 9 relations · 7 constats · 7 trous_


**Synthèse.** L'écosystème obrain est décrit en trois lieux cohérents mais non identiques : la table « Ecosystem » de README.md (13 projets : obrain-server, obrain-web, gwp, boltr, obrain-langchain, obrain-llamaindex, obrain-mcp, obrain-memory, anywidget-graph, anywidget-vector, playground, graph-bench, ann-benchmarks), le hub docs/ecosystem/ (11 fichiers .md dont index.md, 10 docs projet : server, web, mcp, memory, langchain, llamaindex, anywidget-graph, anywidget-vector, graph-bench, performance), et la section Ecosystem de docs/llms.txt (9 projets, org GitHub différente). Les releases sont doublement suivies : CHANGELOG-grafeo.md (797 lignes, historique amont GrafeoDB en semver 0.2.x→0.5.24, 0.5.25 unreleased) et CHANGELOG.md (1802 lignes, journal narratif daté 2026-09-10→2026-09-15, sans en-têtes semver). Le manifeste machine docs/CONNAISSANCES-MANIFEST.json (2026-09-05) décrit 7 KnowledgeDoc + 7 Theme + liens + commits, re-seedable dans le graphe dsh-runtime via graph_link. La migration LpgStore→SubstrateStore est documentée dans docs/MIGRATION.md (outil obrain-migrate, dry-run/upgrade/rollback 7 jours), tandis que docs/FULL-OBRAIN-MIGRATION-2026-09-08.md est un plan/contrat de parité (538 lignes, 32 outils MCP) pour retirer le binaire C codebase-memory-mcp au profit du bridge obrain. Le release candidate docs/RELEASE-RC-M6-ZORDER-2026-09-07.md fige la montée dev/prod de feat/release-m6-zorder (merge 50f10d9a, substrate 822/822, engine 683/683, SUBSTRATE_FORMAT_VERSION 1→2). Licence Apache-2.0, contribution guidée par CONTRIBUTING.md (Rust 1.91.1+, conventional commits) et CONTRIBUTORS.md (un seul contributeur externe nommé, un mainteneur).


**Composants clés.**
- README.md — table Ecosystem (13 projets) + sections Examples/Documentation/License/lineage (fork GrafeoDB)
- docs/ecosystem/index.md — hub MkDocs « grid cards » listant 9 projets écosystème
- docs/ecosystem/obrain-server.md — server REST/GWP/Bolt, 4 tiers Docker (gwp/bolt/standard/full)
- docs/ecosystem/obrain-web.md — @obrain-db/web WASM + IndexedDB + hooks React/Vue/Svelte
- docs/ecosystem/obrain-mcp.md — serveur MCP 16 outils/4 catégories, transports stdio/SSE/streamable HTTP
- docs/ecosystem/obrain-memory.md — MemoryManager boucle extract/search/reconcile/execute, providers openai/mistral/anthropic
- docs/ecosystem/obrain-langchain.md — intégration LangChain (graph store, vector store, Graph RAG)
- docs/ecosystem/obrain-llamaindex.md — intégration LlamaIndex PropertyGraphStore
- docs/ecosystem/anywidget-graph.md — widget graphe pour notebooks Python
- docs/ecosystem/anywidget-vector.md — widget vecteurs 3D pour notebooks Python
- docs/ecosystem/graph-bench.md — suite de benchmark (8 moteurs, 25+ benchmarks)
- docs/ecosystem/performance.md — page performance comparée (LDBC/benchs)
- docs/llms.txt — guide LLM (features, 6 langages, API Python/Node/Rust, crates, ecosystem 9 projets)
- docs/index.md — page d'accueil docs « The graph database that learns » (7 couches biomimétiques, bindings)
- docs/README-public.md — README public alternatif (mmap instant-open, WAL crash-safe, neo4j2obrain)
- docs/MIGRATION.md — guide migration LpgStore→SubstrateStore (obrain-migrate, rollback 7 jours)
- docs/FULL-OBRAIN-MIGRATION-2026-09-08.md — plan/contrat parité 32 outils MCP, retrait codebase-memory-mcp C
- docs/RELEASE-RC-M6-ZORDER-2026-09-07.md — release candidate feat/release-m6-zorder, suites + pré-vol dev/prod
- docs/CONNAISSANCES-MANIFEST.json — manifeste machine des connaissances (KnowledgeDoc/Skill/Theme, commits)
- CHANGELOG.md — journal narratif Obrain daté (1802 lignes, 2026-09-10→2026-09-15)
- CHANGELOG-grafeo.md — historique amont GrafeoDB en semver 0.2.x→0.5.24 (797 lignes)
- CONTRIBUTORS.md — communauté, stack, contributeur CorvusYe, mainteneur S.T. Grond
- CONTRIBUTING.md — setup, branching, table des 12 crates, 13 projets écosystème this-rs
- LICENSE — Apache-2.0 (201 lignes), notices copyright GrafeoDB préservées

**Relations / dépendances.**
- README.md « Ecosystem » → docs/ecosystem/index.md : même liste projet mais README en liste 13 (inclut gwp, boltr, playground, ann-benchmarks) vs 9 dans le hub docs
- docs/llms.txt « Ecosystem » (9 projets, org github.com/ObrainDB/*) diverge des liens README/CONTRIBUTING/CONTRIBUTORS/docs/index (org github.com/this-rs/*)
- CONTRIBUTING.md table crates ↔ docs/llms.txt « Crate Structure » ↔ docs/README-public.md arborescence crates/ : trois vues de la topologie crates (noms qui diffèrent : obrain-bindings-common, obrain-python/node/c/csharp/dart)
- docs/MIGRATION.md référence docs/architecture/overview.md, docs/rfc/substrate/format-spec.md, docs/rfc/substrate/wal-spec.md, crates/obrain-substrate/README.md
- docs/RELEASE-RC-M6-ZORDER-2026-09-07.md ← feat/release-m6-zorder = feat/zorder (slots 512) + feat/m6-code-graph (obrain-indexer), binaire target/release/obrain sha256 c0d30668…71369
- docs/FULL-OBRAIN-MIGRATION-2026-09-08.md ↔ crates/obrain-cli/src/mcp_server.rs (portage des outils bridge), examples/persist.rs (migration SQLite→substrate), obrain-indexer (feature indexer)
- docs/CONNAISSANCES-MANIFEST.json → nœuds graphe dsh-runtime (KnowledgeDoc/Skill/Theme) re-seedable par graph_link ; liens doc↔theme↔skill et commits
- CHANGELOG.md référence CHANGELOG-grafeo.md (historique amont préservé) et le fork GrafeoDB/grafeo
- CONTRIBUTORS.md/CONTRIBUTING.md/LICENSE → org this-rs et licence Apache-2.0 ; docs/index.md/README-public.md héritent de la même licence/lineage

**Points notables.**
- Deux journaux de version coexistent sans schéma unifié : CHANGELOG-grafeo.md en semver (0.5.24, 0.5.25 unreleased) vs CHANGELOG.md narratif daté sans bump de version crate, ce qui rend le numéro de version courant ambigu (README montre obrain ^0.5.24, docs/index.md montre Dart obrain ^0.5.21).
- Le README-public.md vante un format « single-file .obrain » mmap, mais docs/MIGRATION.md décrit le Substrate comme un répertoire substrate.obrain/ (zones mmap + WAL) — tension de formulation entre le format legacy (fichier unique) et le format courant (répertoire).
- docs/FULL-OBRAIN-MIGRATION-2026-09-08.md est un chantier vivant de 538 lignes avec 36 sections chronologiques : découverte que le daemon C sert ses 32 outils depuis ~26 Go de stores SQLite (~/.cache/codebase-memory-mcp/*.db), distincts des stores substrate runtime-*.obrain (4,4-6,2 Go).
- Le micro-bench obrain vs Kuzu (section 28-30) revendique obrain ×2,3 sur count, ×37 sur lookup qualified_name avec index d'égalité (1,8 µs count natif GQL), mais concède que les parcours 3+ hops restent plus lents que Kuzu (54 ms vs 30 ms).
- docs/CONNAISSANCES-MANIFEST.json référence un chemin générique docs/PROPOSAL-SEARCH-*-2026-09-05.md (glob, pas un fichier réel) — le manifeste encode un pattern de fichier, pas un artefact unique.
- CONTRIBUTORS.md ne nomme qu'un seul contributeur externe (CorvusYe, Dart bindings #138) et un mainteneur (S.T. Grond), en décalage avec l'ampleur d'un écosystème de ~13 projets et ~24 crates.
- docs/index.md revendique « Fastest graph database tested on the LDBC SNB » tandis que graph-bench.md/index.md parlent de 8 moteurs ; README parle de 25+ benchmarks — les chiffres de benchmark ne sont pas strictement alignés entre fichiers.

**Trous / incohérences.**
- Les chemins docs/FULL-OBRAIN-MIGRATION et docs/RELEASE-RC-M6-ZORDER n'existent pas tels quels : les fichiers réels sont docs/FULL-OBRAIN-MIGRATION-2026-09-08.md et docs/RELEASE-RC-M6-ZORDER-2026-09-07.md (suffixes de date).
- Incohérence d'organisation GitHub : README/CONTRIBUTING/CONTRIBUTORS/docs/index utilisent github.com/this-rs/* alors que docs/llms.txt utilise github.com/ObrainDB/* (y compris pour le module Go) — une seule des deux orgs est canonique.
- gwp, boltr, playground et ann-benchmarks sont listés dans README Ecosystem mais n'ont pas de page dédiée dans docs/ecosystem/ (seuls 10 projets ont un .md) ; pas de documentation locale pour gwp/boltr/ann-benchmarks.
- docs/CONNAISSANCES-MANIFEST.json est daté 2026-09-05 et référence des docs (CAPACITES-AGENT-OBRAIN-2026-09-05.md, ETAT-2026-09-05-2015.md, etc.) à vérifier : le manifeste n'est pas revalidé automatiquement contre le système de fichiers.
- Absence d'un fichier VERSION unique ou d'une table de correspondance release↔commit↔format (SUBSTRATE_FORMAT_VERSION évolue 1→2→3 selon CHANGELOG.md et RELEASE-RC-M6-ZORDER, mais aucune cartographie centralisée version→numéro de format).
- Dérive de versions dans les exemples : README obrain ^0.5.24, docs/index.md Dart ^0.5.21, obrain-mcp >=0.4.4, obrain-memory >=0.5.1 — aucune source unique de vérité de version.
- Le statut réel des phases P1-P5 de FULL-OBRAIN-MIGRATION (P1 partiel ~25/32 outils, P2-P5 non exécutées, bascule conditionnée à décision utilisateur) n'est pas reflété dans CHANGELOG.md ni README — l'état de migration vit uniquement dans ce document.



### 10.3 Invariants transverses sécurité/perf (IAM, zero-copy, slots512, confinement statique, garantie confidentialité, réactif)

_Fichier : `raw/v10-3-invariants-transverse.json` — 29 composants · 14 relations · 12 constats · 10 trous_


**Synthèse.** Six invariants d'architecture traversent toutes les branches d'obrain, chacun adossé à une ADR ou un DESIGN mesuré. L'IAM (ADR-2026-09-13) est passé d'une bibliothèque sans appelant (5 395 lignes, 95 tests, 0 point d'application) à une garde fail-closed (ImplicitDeny) branchée à un point de contrôle unique dans viz_serve::handle() et au dispatch MCP, avec portée par base ('tenant', espace) et cinq défauts corrigés (jeton déterministe, révocation décorative, expiration jamais vérifiée, horodatage fictif, empreinte SipHash). Le zero-copy (DESIGN-PONT-MEMOIRE-ZERO-COPY) expose StrView<'store> qui emprunte le mmap PropsZone sous garde parking_lot (0,223 µs vs 1,65 µs, 7,4×, zéro unsafe nouveau). Les slots512 (DECISION/DESIGN-PROPSZONE-SLOTS-512) encodent une UnitéAddr 48 bits (bit 47 = slot) avec 7 unités de 512 o par carrier 4K et magic 0xC5A2_50C8, gouvernés par OBRAIN_PROPS_SLOTS_512 (défaut OFF) — codec et allocateur livrés, câblage unit-aware non branché pour éviter tout état hybride. Le confinement statique (CONFINEMENT-STATIQUE) verrouille /app/** par chemin_sous + percent_decode_chemin (verrou nom + verrou lien, réponse 404 jamais 403). La garantie de confidentialité (ADR-2026-09-15) fixe cinq invariants G1-G5, recommande V-B (self aveugle) comme plancher, et s'est matérialisée dans obrain-tls (TLS 1.3, encore non branché sur run_serve), obrain-coffre (AES-256-GCM + hiérarchie de clés + enveloppes à taille fixe + jetons de capacité 130 o), obrain-aveugle (Porteur sans clé qui ne peut pas desceller) et obrain-journal (chaîne signée Ed25519) relié à l'audit IAM par audit_chaine.rs. Enfin le réactif (obrain-reactive) publie les mutations sur un MutationBus tokio::broadcast drainé par un Scheduler vers 7 MutationListener cognitifs, mais coexiste avec un transport WebSocket (viz_ws.rs) qui sonde le mtime du store toutes les 400 ms sans s'abonner au bus.


**Composants clés.**
- docs/ADR-2026-09-13-chaine-iam-et-explosion-des-liens.md — ADR IAM (D1-D7) + les cinq défauts IAM corrigés + règles transférables
- docs/ADMINISTRATION-IAM-2026-09-13.md — noyau admin (un seul guichet), plafond déduit, dernier admin verrouillé, grammaire de noms fermée
- docs/IAM-BRANCHE-2026-09-13.md — chaîne complète (login/refresh/logout/audit) branchée et revérifiée par scripts/verif-chaine-iam.sh
- crates/obrain-iam/src/garde.rs — Garde::authentifier/check_graph_access/renouveler, PolicyDecision::ImplicitDeny (fail-closed)
- crates/obrain-iam/src/admin.rs — Administration::plafonnee_par_l_acteur (plafond DÉDUIT, défaut fermé)
- crates/obrain-iam/src/store.rs — IamStore, evaluate_access, node_to_policy/node_to_role (correctif iam_status=deleted)
- crates/obrain-iam/src/audit_chaine.rs — RegistreAudit : pont entre l'audit IAM et le journal signé obrain_journal
- crates/obrain-iam/src/model.rs + provider.rs — AuditEvent, bcrypt (DEFAULT_COST), hash_token SHA-256, jeton 256 bits système
- crates/obrain-cli/src/viz_serve.rs — GardeIam (OnceLock partagée), handle() gardé avant le match, chemin_sous + percent_decode_chemin
- crates/obrain-substrate/src/view.rs — StrView (Zone/Blob/Shared), FactView, StrViewSource ; zero-copy sur le mmap PropsZone
- crates/obrain-substrate/src/result_buf.rs — ResultBuf : arène réutilisable des chemins qui doivent copier (execute_into)
- crates/obrain-substrate/src/unite.rs — UniteAddr, SlotPage 512, CarrierControl, CarrierPack, CARRIER_PACKED_MAGIC=0xC5A2_50C8, UNITE_SLOT_FLAG=1<<47
- crates/obrain-substrate/src/props_zone.rs — PropsZone::slots_512_actifs() (OBRAIN_PROPS_SLOTS_512), walkers unit-aware
- crates/obrain-substrate/src/page.rs — PROP_PAGE_MAGIC_NODE=0xF507_5FA6 / EDGE=0xE507_5FA6, seal_crc32 (intégrité seule)
- docs/DECISION-SLOTS512-WIRING-2026-09-06.md — état figé + chemin d'exécution en 6 étapes (ordre imposé : pas d'état hybride)
- docs/DESIGN-PROPSZONE-SLOTS-512-2026-09-06.md — RFC mère, modèle d'adressage UnitéAddr, gouvernance de version, walkers
- docs/MIGRATION-SLOTS512-TEST-2026-09-07.md — mig_slots512 sur 4 copies réelles, triple vérification, zéro dérive
- docs/CONFINEMENT-STATIQUE-2026-09-15.md — faille /app/../../.. mesurée, double verrou nom+lien, 404 jamais 403
- docs/ADR-2026-09-15-GARANTIE-CONFIDENTIALITE-CALCUL.md — invariants G1-G5, échelle N1-N4, variantes V-A/V-B/V-C, 6 étapes
- crates/obrain-tls/src/lib.rs — FluxTls (Read+Write), EcouteTls, TLS 1.3 via rustls/ring (feature dédiée)
- crates/obrain-tls/examples/serveur_tls.rs — mode relais (tcpdump de substitution) + mode --clair (témoin)
- crates/obrain-coffre/src/lib.rs — AES-256-GCM, hiérarchie KEK→DEK→HKDF (coffre/enveloppe/jeton), enveloppes à taille fixe, jetons de capacité
- crates/obrain-aveugle/src/lib.rs — Porteur sans CleEspace (ne peut pas desceller), Charge scellée, Adresse tirée au sort (étape 3 V-B)
- crates/obrain-journal/src/lib.rs + entree.rs — Journal signé Ed25519, TAILLE_EMPREINTE=32, TAILLE_SIGNATURE=64, chaîne par empreinte
- crates/obrain-reactive/src/lib.rs — #![deny(unsafe_code)], façade InstrumentedStore→MutationBus→Scheduler→MutationListener
- crates/obrain-reactive/src/{bus,event,store,scheduler,listener}.rs — MutationBus tokio::broadcast, MutationEvent, InstrumentedStore, BatchConfig, TenantFilteredListener
- crates/obrain-cli/src/viz_ws.rs — primitives WebSocket std-only (SHA-1+base64 inline, handshake RFC6455)
- crates/obrain/benches/zero_copy.rs + crates/obrain-substrate/tests/zero_copy_views.rs + crates/obrain/examples/seeding_zero_copy.rs
- scripts/verif-chaine-iam.sh + acceptation-iam.sh + acceptation-tls.sh + migrate-slots512.sh + protocole-check.mjs

**Relations / dépendances.**
- obrain-iam::Garde::authentifier + check_graph_access(action, 'tenant', base) → appelé par viz_serve::handle() (avant le match des routes) ET par le dispatch MCP call_tool() — une règle, tous les points d'entrée
- obrain-iam::IamStore::evaluate_access collecte les politiques via node_to_role/node_to_policy → ces fonctions rendent désormais None si iam_status=deleted (bug 'supprimer ne retirait aucun droit')
- obrain-iam/audit_chaine.rs::RegistreAudit → obrain_journal::Journal::verifier_signatures (chaîne signée Ed25519) — répond à la réserve n°1 de l'ADR confidentialité (audit non chaîné)
- obrain-cli/viz_serve.rs::GardeIam (OnceLock statique) ouvre le magasin UNE fois et le partage entre viz serve et le pont MCP ; repli lecture seule déclaré
- obrain-substrate::view::StrView emprunte le mmap PropsZone sous RwLockReadGuard::try_map / read_arc (parking_lot arc_lock) ; borrow-checker empêche la vue de survivre au store (doc-test compile_fail)
- obrain-substrate::unite::UniteAddr (bit 47 = slot) est décodé par decode_unite ; les walkers PropsZone (walk_chain/collect_live_props_by_key/get_latest_for_key) doivent itérer des UnitéAddr au lieu de page_idx
- obrain-substrate::props_zone::slots_512_actifs() lit OBRAIN_PROPS_SLOTS_512 ; store.rs (sites d'écriture ~9121/9656/10126/10175) branche ecrire_debut_chainee_pack conditionnellement
- obrain-cli/viz_serve.rs::chemin_sous + percent_decode_chemin (sans décodage +) → canonicalize sous la racine ; protocole-check.mjs sonde 7..20 niveaux
- obrain-tls::FluxTls implémente Read+Write et expose set_read_timeout/set_write_timeout/set_nodelay pour être branché sur run_serve() de viz_serve.rs sans perdre l'anti-slowloris (contrat posé, branchement NON fait)
- obrain-coffre (clés/enveloppes/jetons) + obrain-aveugle (Porteur) réalisent l'étape 3 et la variante V-B de l'ADR confidentialité ; obrain-journal réalise l'étape 4 (reçus signés chaînés)
- obrain-engine/database/mod.rs crée MutationBus+Scheduler et les passe à CognitiveEngineBuilder::build() ; obrain-cognitive enregistre 7 MutationListener (Energy/Synapse/Fabric/CoChange/Kernel/Stigmergic/Episode)
- obrain-cli/viz_ws → obrain_viz (Subgraph::canonical) ; viz_serve route GET /api/viz/subgraph/stream → handshake → stream_subgraph → live_loop (poll mtime 400 ms)
- packages/obrain-dashboard/src/lib/api.ts → /api/viz/subgraph/stream (WebSocket) avec repli HTTP fetch('/api/viz/subgraph')
- scripts/verif-chaine-iam.sh (13 étapes, port 8435, exit code) et scripts/acceptation-tls.sh (16 vérifs, curl/openssl/grep/od) sont les portes de non-régression des invariants IAM et TLS

**Points notables.**
- L'IAM était une bibliothèque complète et testée (5 395 lignes, 9 modules, 95 tests) SANS aucun appelant : la feature 'iam' n'était ni dans default ni dans ai, et le serveur exposé sur 192.168.1.103:8432 servait tout le monde sans identité — rien ne le disait.
- La garde est fail-closed (ImplicitDeny) à l'inverse du middleware du framework 'this' où une route sans politique PASSE (Abstain) : deux IAM coexistent dans le parc, de sémantique opposée.
- Cinq défauts IAM ont été mesurés et corrigés le même jour : jeton déterministe (DefaultHasher+compteur), révocation décorative (magasin ouvert en lecture seule), expiration jamais testée, now_iso() = compteur formaté en date, empreinte SipHash-64 sans sel remplacée par SHA-256.
- Le zero-copy est réel et mesuré sur deux architectures : StrView = 223 ns (n3 x86_64) / 168 ns (DGX aarch64) contre 1 646/651 ns pour execute+JSON+re-parse, soit 7,4×/3,9× — avec ZÉRO bloc unsafe ajouté (emprunt porté par parking_lot).
- slots512 est env-gated par OBRAIN_PROPS_SLOTS_512 (défaut OFF) et le câblage complet est explicitement 'non branché à ce stade' (props_zone.rs:458) : la doctrine est qu'un état hybride legacy/slot est 'pire qu'aucun code', donc livraison en un tenant.
- SUBSTRATE_FORMAT_VERSION vaut déjà 3 (meta.rs:20), alors que le design slots512 (2026-09-06) anticipait un bump 1→2 : l'historique de format a évolué entre-temps (décalage doc/code à documenter).
- La faille de confinement a été ratée par le contrôle parce que le dist servi est devenu un LIEN symbolique vers un chemin plus profond : 7 '../' ne suffisaient plus, 9 à 20 réussissaient — le contrôle teste désormais la capacité d'échappement de 7 à 20 niveaux.
- La confidentialité est passée du stade 'proposé' à des crates réelles : obrain-coffre (AES-256-GCM, KEK Argon2id, enveloppes à 5 classes de taille, jetons de capacité de 130 o constants) et obrain-aveugle où le Porteur ne peut PAS desceller par construction de type (aucune CleEspace en sa possession).
- Le journal d'audit est désormais une chaîne signée Ed25519 (obrain-journal) reliée à l'IAM via audit_chaine.rs — mais le module avoue qu'il ne force personne à s'en servir : tant qu'une surface écrit son audit directement dans le store, cette surface reste sans garantie.
- Deux systèmes 'push' disjoints : le MutationBus in-process (tokio::broadcast) des cognitifs, et le WebSocket du viz qui POLLE le mtime du store (400 ms) au lieu de s'abonner au bus — le lien epoch=mtime vs epoch de transaction n'est pas documenté.
- Le handshake WebSocket est implémenté MAISON en std-only (~120 lignes, SHA-1+base64 inline) pour ne pas introduire de runtime async dans un serveur synchrone thread-par-connexion (refus explicite de tokio-tungstenite).
- obrain-garde n'est PAS une garde de sécurité : c'est la garde d'arrêt de génération (client_parti, budget total, répétition) portée une fois pour les boucles selfd/chat — nom trompeur dans le périmètre sécurité.

**Trous / incohérences.**
- obrain-tls vit À CÔTÉ du serveur : le branchement sur run_serve() (viz_serve.rs) n'est pas posé, donc le serveur réel sert toujours en clair et le §1.2 de l'ADR confidentialité reste vrai mot pour mot.
- Le câblage slots512 complet n'est pas branché (props_zone.rs:458 'non branché à ce stade') : codec (unite.rs) et allocateur CarrierPack existent, mais la chaîne 'unité-aware' des walkers+store n'est pas livrée — invariant en cours, non stabilisé.
- InstrumentedStore (la source d'événements du pipeline réactif) n'est construit nulle part en production : uniquement tests/doc et un commentaire dans substrate/store.rs — la capture des mutations reste latente/non branchée.
- Les jetons de capacité (obrain-coffre) n'ont pas d'anti-rejeu (pas d'état vérificateur) et l'horloge est fournie par l'appelant qui tourne chez l'opérateur : un opérateur qui recule l'horloge ressuscite un jeton expiré (nommé, non fermé).
- Défaut RGPD PRÉEXISTANT non corrigé (gdpr.rs) : horodatage gelé (erased_at=2026-01-01T00:00:01Z, exported_at=…00:00:00Z) et principal = cible au lieu de l'acteur — un certificat non daté ne date rien.
- hash_token est un SHA-256 SANS sel (le commentaire assume 'in production this would use bcrypt/argon2 with salt') — progrès vs DefaultHasher, pas une cible.
- Deux fichiers ADR IAM coexistent en quasi-doublon : ADR-2026-09-13-chaine-iam-et-explosion-des-liens.md (157 l.) et ADR-2026-09-13-CHAINE-IAM-ET-EXPLOSION-DES-LIENS.md (165 l.) — source de divergence possible.
- Le mode live/delta WebSocket est implémenté côté serveur mais non consommé par l'UI (api.ts ignore les frames live/delta/epoch) : fonctionnalité orpheline à ce stade.
- AUDIT-MULTI-AXE-2026-09-20.md, référencé par d'autres sessions (garde de classification lecture/écriture, abstention d'écriture en lecture seule, invariant mono-écrivain), est absent de ce checkout — hors périmètre des 6 invariants ici mais trou transverse à signaler.
- Le pont MCP n'est PAS armé en production (le harness ne pose pas OBRAIN_IAM=1) : la garde y est branchée dans le code mais inerte au déploiement, contrairement au serveur 8432 armé le 2026-09-13.



### 10.4 Gaps et dette (tranche v10-4) — TODO/FIXME/XXX, stubs, code mort, divergence docs vs code

_Fichier : `raw/v10-4-gaps-dette.json` — 22 composants · 10 relations · 11 constats · 15 trous_


**Synthèse.** La dette marquée par TODO/FIXME/XXX est étonnamment basse pour un dépôt de ~612 k lignes Rust : TODO=14, FIXME=0, XXX=11 (dont 9 faux positifs d'échappement unicode \uXXXX et de bundles hachés index-XXXX.js), todo!=0, unimplemented!=2 (uniquement en commentaires de doc). La vraie dette n'est pas dans les marqueurs mais dans la couche cognitive : trois des six Thinkers (Consolidator, Dreamer, Predictor) sont des stubs no-op dont le tick n'incrémente qu'un compteur, en attente de tickets T13-S2/S4/S5 qui ne sont tracés nulle part dans docs/. Le fichier le plus lourd, crates/obrain-substrate/src/store.rs (34 172 lignes), porte encore un en-tête de doc « Step 0 | Skeleton — every method unimplemented!() » historiquement périmé. Deux diagnostics datés documentent des dettes de fiabilité/de déploiement (DIAG-NON-PERSISTANCE 2026-09-05, ECART-DEPLOIEMENT-LIGNES 2026-09-14) et un banc connu reste rouge : scripts/echecs-connus.txt liste 1 échec attribué. On relève 64 marqueurs #[allow(dead_code)], du code mort orphelin (replay RDF WAL depuis le cutover T17) et des bindings Go/C#/Dart présents sur disque mais absents des membres du workspace Cargo.


**Composants clés.**
- crates/obrain-substrate/src/store.rs (34 172 lignes — fichier 'god', en-tête doc T4 périmé)
- crates/obrain-substrate/src/meta.rs (SUBSTRATE_FORMAT_VERSION = 3)
- crates/obrain-cognitive/src/thinker/consolidator.rs (tick no-op, TODO T13-S2 decay_energy_all)
- crates/obrain-cognitive/src/thinker/dreamer.rs (tick no-op, TODO T13-S5 bottleneck detection)
- crates/obrain-cognitive/src/thinker/predictor.rs (prefetch no-op, TODO T13-S4 community_id)
- crates/obrain-cognitive/src/thinker/warden.rs (réel — wrap CommunityWarden::tick)
- crates/obrain-cognitive/src/thinker/runtime.rs (1 thread par Thinker enregistré)
- crates/obrain-cognitive/src/thinker/mod.rs (ThinkerKind + stub trivial de test)
- crates/obrain-cognitive/src/gds_refresh.rs (refresh_stub no-op derrière feature gds-refresh)
- crates/obrain-cognitive/src/store_trait.rs (2 constantes #[deprecated] PROP_FABRIC_*)
- crates/obrain-engine/src/database/rdf_ops.rs (replay_rdf_wal_record orphelin #[allow(dead_code)])
- crates/obrain-engine/tests/null_and_coercion.rs (TODO coercion d'agrégat non corrigée)
- crates/obrain-engine/tests/regression_external.rs (TODO WITH...ORDER BY non supporté)
- crates/obrain-substrate/tests/operators_scan_vector.rs (feature vector-index non déclarée)
- crates/obrain-iam/src/provider.rs (TODO révoqué — revoke_session désormais réel)
- crates/obrain-adapters/src/plugins/algorithms/kernel_manager.rs (TODO audit remove_node_property)
- crates/bindings/{go,csharp,dart} (présents sur disque, hors membres workspace)
- Cargo.toml (29 membres workspace — python/node/c/wasm/common, pas go/csharp/dart)
- README.md (badge @obrain-db/web sans crate 'web' correspondante)
- scripts/echecs-connus.txt (1 échec connu : des_ancres_divergentes_ne_rendent_jamais_un_resultat_ampute)
- docs/DIAG-NON-PERSISTANCE-2026-09-05.md (non-persistance ~1 h, correctifs C1/C2 livrés côté harness)
- docs/ECART-DEPLOIEMENT-LIGNES-2026-09-14.md (binaire servi ≠ config, 6 réglages inertes, 3 lignées divergentes)

**Relations / dépendances.**
- thinker/runtime.rs -> Consolidator/Dreamer/Predictor/Warden/HilbertEnricher/KernelEnricher (1 std::thread par Thinker)
- Consolidator.tick / Dreamer.tick / Predictor.tick -> SubstrateStore (signature) mais corps no-op (dépendances réelles non branchées)
- Warden.tick -> obrain_substrate::CommunityWarden::with_trigger().tick() (seul Thinker de maintenance réel avec le consolidateur)
- gds_refresh.rs -> feature gds-refresh (refresh réel) / refresh_stub no-op sinon
- obrain-engine/database/rdf_ops.rs -> obrain_adapters::storage::wal::WalRecord (replay RDF orphelin depuis cutover T17)
- obrain-iam/provider.rs revoke_session -> store.find_node_by_label_and_id + delete_node (obrain-core graph/traits.rs)
- obrain-cognitive/store_trait.rs PROP_FABRIC_MUTATION_FREQ/ANNOTATION_DENSITY -> renommées (deprecated)
- Cargo.toml [workspace] members -> crates/bindings/{python,node,c,wasm,common} (go/csharp/dart exclus)
- ECART-DEPLOIEMENT-LIGNES.md -> lignées j4/j5/j6 + tronc (SUBSTRATE_FORMAT_VERSION v1 -> v3, consolidation fusion/tronc-plus-j6)
- DIAG-NON-PERSISTANCE.md -> packages/session/session-persistence graph-writer.ts (fire-and-forget) + coordinator.ts (re-push boot)

**Points notables.**
- TODO/FIXME/XXX quasi inexistants : FIXME=0, todo!=0, unimplemented!=2 (doc seulement) — la dette n'est pas marquée mais structurelle
- 3 Thinkers cognitifs sur 6 sont des stubs no-op (Consolidator/Dreamer/Predictor) : le cœur 'qui apprend' a sa boucle decay/bottleneck/community-prefetch désactivée
- Les tickets T13-S2/S4/S5 cités dans les TODO n'ont aucune trace dans docs/ (grep T13 = 0) — dette non tracée hors code
- crates/obrain-substrate/src/store.rs = 34 172 lignes, fichier 'god' avec en-tête 'Step 0 Skeleton — every method unimplemented!()' historiquement faux
- 64 marqueurs #[allow(dead_code)] répartis dans obrain-core/engine (annotator.rs, crud.rs, session/mod.rs, database/mod.rs)
- replay_rdf_wal_record orphelin depuis le cutover T17 (suppression LpgWal) mais conservé 'jusqu'à re-câblage'
- scripts/echecs-connus.txt : 1 seul échec connu et attribué (des_ancres_divergentes..., index ordonné, 2026-09-13)
- ECART-DEPLOIEMENT-LIGNES documente 6 réglages inertes + 3 lignées divergentes ; meta.rs est désormais v3 (consolidation avancée) mais le doc laisse 2 décisions ouvertes
- bindings go/csharp/dart présents sur disque (FFI) mais absents des membres Cargo.toml -> non construits/testés par le workspace
- README badge @obrain-db/web (l.10) sans crate de binding 'web' correspondante (seul 'wasm' existe)
- obrain-iam provider.rs : le TODO 'delete_node manquant' était périmé — la révocation est réelle depuis 2026-09-13

**Trous / incohérences.**
- Thinkers stubs non branchés : Consolidator (decay_energy_all), Dreamer (bottleneck_nodes_from_store), Predictor (community_id/prefetch) — T13-S2/S4/S5
- gds_refresh.rs : refresh_stub no-op quand feature gds-refresh désactivée (pas d'erreur, simple warn)
- replay_rdf_wal_record (obrain-engine/rdf_ops.rs) : code mort orphelin #[allow(dead_code)] depuis cutover T17
- Aggrégat : coercion non corrigée (test null_and_coercion.rs:406 attend Float64 mais rend Int64)
- WITH ... ORDER BY non supporté (test regression_external.rs:315)
- obrain-substrate ne déclare pas la feature vector-index (test operators_scan_vector.rs:265) — fonctionnalité vectorielle en attente
- 64 #[allow(dead_code)] : fonctions/structs réservées non exposées (annotator, crud, session) — à auditer
- crates/bindings/{go,csharp,dart} absents des membres workspace Cargo.toml (hors CI cargo)
- README badge @obrain-db/web sans crate 'web' (divergence docs vs code)
- En-tête doc de store.rs 'Step 0 Skeleton' / 'T4 progression' périmé sur fichier mature de 34 k lignes
- Tickets T13-S2/S4/S5 non tracés dans docs/ (référencés uniquement en commentaires de code)
- 2 constantes #[deprecated] PROP_FABRIC_MUTATION_FREQ / PROP_FABRIC_ANNOTATION_DENSITY (store_trait.rs) à retirer
- 1 échec de banc connu non résolu : des_ancres_divergentes_ne_rendent_jamais_un_resultat_ampute (index ordonné, 2026-09-13)
- DIAG-NON-PERSISTANCE : questions racine restantes (propriétaire GraphWriter, source du replay) bien que C1/C2 soient livrés côté harness
- ECART-DEPLOIEMENT-LIGNES : décisions ouvertes non tranchées (lignée de référence, sort des 6 réglages inertes)



### 10.5 nomenclature-coherence

_Fichier : `raw/v10-5-nomenclature-coherence.json` — 18 composants · 11 relations · 10 constats · 8 trous_


**Synthèse.** obrain est le rebrand officiel d'un fork de GrafeoDB/grafeo (README.md, CHANGELOG.md : « forked from GrafeoDB/grafeo and rebranded as Obrain (obrain.dev) »), hébergé sous l'org this-rs (github.com/this-rs/obrain) mais sans remote git configuré. Le workspace Cargo déclare 28 membres : 23 packages Rust obrain-* (plus obrain et neo4j2obrain) et un conteneur crates/bindings qui se déploie en 5 sous-crates (python, node, c, wasm, common) — la nomenclature Go/C#/Dart annoncée dans le brief n'apparaît pas sous crates/bindings (seul docs/api/go existe, pas de crate csharp/dart). Le dépôt porte 75 branches locales et 59 worktrees enregistrés, auxquels s'ajoutent ~13 répertoires non enregistrés dont des worktrees d'autres repos (obrain-chat-mobile, llama-obrain) et deux répertoires orphelins sans .git (wt-this-audit, wt-this-projets). La convention de nommage est globalement cohérente en obrain-*, avec trois exceptions majeures : le crate neo4j2obrain (préfixe source en tête), l'héritage grafeo toujours présent dans 97 fichiers (docs/rfc, CHANGELOG-grafeo.md, commentaires), et une paire d'ADR quasi-dupliqués ne différant que par la casse. Des dérives worktree↔branche existent (banc-obrain-fix-ecrivain pointe sur feat/reliefs-n3-outils-cognitifs, wt-selfd-edit sur fix/selfd-exec-arriere-plan), et la séquence j01/j2..j5 mélange zéro-padding et séquence plan-jN sans correspondance 1:1 avec les branches.


**Composants clés.**
- crates/ — 24 dossiers : obrain, obrain-adapters, obrain-aveugle, obrain-cli, obrain-coffre, obrain-cognitive, obrain-common, obrain-core, obrain-engine, obrain-garde, obrain-httpmin, obrain-iam, obrain-indexer, obrain-journal, obrain-migrate, obrain-rag, obrain-reactive, obrain-selfd, obrain-selflink, obrain-substrate, obrain-tls, obrain-viz, neo4j2obrain, bindings
- crates/bindings/ — conteneur sans Cargo.toml racine, 5 sous-crates : python, node, c, wasm, common (members déclarés dans Cargo.toml)
- Cargo.toml — workspace racine : 28 membres, repository = https://github.com/this-rs/obrain
- CHANGELOG.md — renommage officiel Grafeo→Obrain, pointe vers CHANGELOG-grafeo.md comme historique upstream
- CHANGELOG-grafeo.md — changelog historique Grafeo conservé (version 0.5.25)
- README.md — badges npm @obrain-db/js, @obrain-db/wasm, NuGet Obrain, PyPI obrain, crates.io obrain
- crates/obrain-engine/tests/coverage_rebrand.rs — test de couverture du renommage Grafeo→Obrain (ObrainDB::new_in_memory)
- crates/neo4j2obrain/ — crate de migration, seul à ne pas suivre la convention obrain-*
- docs/ — 383 fichiers .md (490 fichiers au total), 189 .md au niveau racine, 24 sous-dossiers
- docs/rfc/substrate/ — format-spec.md, migration-audit.md, propszone-pages-partagees.md, multi-writer.md, tier-persistence.md, double-journalisation-wal-props.md, cognitive-quality*.md (références grafeo)
- docs/api/ — c, go, node, python, rust, wasm, index.md (pas de csharp/dart)
- docs/algorithms/ — 7 fichiers ; docs/cognitive/ ; docs/architecture/ ; docs/ecosystem/
- docs/cartographie-obrain/raw/ — 45 fichiers JSON déjà produits (v1 à v9), le présent v10-5 s'y ajoute
- docs/ADR-2026-09-13-chaine-iam-et-explosion-des-liens.md — ADR (157 lignes), doublon casse du suivant
- docs/ADR-2026-09-13-CHAINE-IAM-ET-EXPLOSION-DES-LIENS.md — ADR (165 lignes), contenu réécrit ≠ du précédent
- packages/obrain-dashboard/ — app Vue/Vite ; packages/obrain-cli-npm/ ; packages/obrain-cli-python/
- docs/plan-j4.md .. plan-j9.md + plan-j5-bascule.md, plan-j9-protobuf.md — séquence plan-jN sans correspondance 1:1 avec les branches
- runtime/ runtime-dev/ runtime-dev.obrain/ runtime-prod/ self-demo/ x/ acceptation-chat-* agent-synapse-triviere-dsh — répertoires racine contenant des fichiers substrate (substrate.community, substrate.degrees.node.u32, substrate.dict, substrate.props.idx.v2)

**Relations / dépendances.**
- GrafeoDB/grafeo --fork/rebrand--> this-rs/obrain (README.md, CHANGELOG.md, Cargo.toml repository)
- CHANGELOG-grafeo.md --conservé comme historique--> CHANGELOG.md (obrain)
- crates/bindings/ --déploie--> 5 sous-crates python/node/c/wasm/common (members workspace)
- crates/neo4j2obrain --migration--> obrain (import neo4j)
- docs/rfc/* et docs/bench-verify/* --référencent encore--> 'grafeo' (97 fichiers au total)
- crates/obrain-engine/tests/coverage_rebrand.rs --garantit--> absence de grafeo dans la surface publique
- docs/cartographie-obrain/raw/*.json --consomme--> le dépôt (v1..v9 déjà produits)
- worktrees wt-chat-* --appartiennent à--> obrain-chat-mobile (gitdir ../obrain-chat-mobile/.git)
- worktree /mnt/obrain/wt-wht-dense --appartient à--> llama-obrain (gitdir ../llama-obrain/.git)
- branches feat/principal-natif-j01..j5 --alimentent--> worktrees banc-obrain-natif, banc-obrain-j2..j5
- branches feat/index-ordonne-j6, feat/payload-viz-j7, feat/orderby-epoque-j8, feat/transport-ws-j9 --alimentent--> worktrees banc-obrain-j6..j9 (thèmes ≠ principal-natif)

**Points notables.**
- Doublon d'ADR : ADR-2026-09-13-chaine-iam-et-explosion-des-liens.md vs ADR-2026-09-13-CHAINE-IAM-ET-EXPLOSION-DES-LIENS.md — même date/titre, casse seule différente, mais contenus réellement divergents (157 vs 165 lignes, md5 différents) → deux versions concurrentes du même ADR coexistent
- Renommage grafeo→obrain : le code est rebrandé (aucun package 'grafeo' dans les Cargo.toml, test coverage_rebrand.rs), mais 97 fichiers mentionnent encore 'grafeo' (docs/rfc, CHANGELOG-grafeo.md, commentaires de code) — l'assertion « aucune surface publique ne porte plus l'ancien nom » ne couvre pas la doc
- neo4j2obrain est le seul crate hors convention obrain-* (préfixe = source neo4j, pas préfixe obrain)
- Le brief annonce des bindings Python/Node/Go/C#/Dart/WASM, mais crates/bindings ne contient que python, node, c, wasm, common ; docs/api a go mais ni csharp ni dart → trou entre nomenclature annoncée et réalité
- Dérives worktree↔branche : banc-obrain-fix-ecrivain pointe sur feat/reliefs-n3-outils-cognitifs (pas fix/ecrivain-unique-bridge) ; wt-selfd-edit pointe sur fix/selfd-exec-arriere-plan (pas feat/selfd-edit) ; wt-projets-moteur sur integration/projets
- 75 branches locales (le brief disait ~50) ; 59 worktrees enregistrés + ~13 répertoires non enregistrés, dont des worktrees d'AUTRES repos (obrain-chat-mobile, llama-obrain) mélangés aux worktrees banc-obrain
- wt-this-audit et wt-this-projets ne sont PAS des repos git (aucun .git) — répertoires orphelins ; banc-obrain-release est un clone git plein (feat/data-load-bulk) non enregistré comme worktree
- Incohérence de zéro-padding : feat/principal-natif-j01 (zéro en tête) vs j2..j5 (sans) ; docs/plan-jN ne correspond pas 1:1 aux branches (j6..j9 = index-ordonne/payload-viz/orderby-epoque/transport-ws, pas principal-natif)
- Répertoires racine runtime/, runtime-dev/, runtime-dev.obrain/, runtime-prod/, self-demo/, x/, acceptation-chat-* contiennent des fichiers de données substrate (substrate.community, substrate.degrees.node.u32, substrate.props.idx.v2) — fuite de données runtime dans le dépôt, avec un nommage divergent runtime-dev vs runtime-dev.obrain
- Aucun remote git configuré (git remote -v vide) malgré les références github.com/this-rs/obrain et l'historique de fork GrafeoDB

**Trous / incohérences.**
- ADR dupliqué à réconcilier : choisir entre chaine-iam-et-explosion-des-liens (minuscules) et CHAINE-IAM-ET-EXPLOSION-DES-LIENS (majuscules) — une seule version canonique attendue
- Bindings Go/C#/Dart annoncés mais absents de crates/bindings (seul docs/api/go existe) — où vivent les bindings go/csharp/dart, ou s'agit-il d'un écart doc vs code ?
- 97 fichiers référencent encore 'grafeo' — la doc (docs/rfc, bench) est en retard sur le rebrand du code ; CHANGELOG-grafeo.md est-il volontairement conservé ou à migrer ?
- Worktrees orphelins wt-this-audit / wt-this-projets sans .git — à supprimer ou à ré-enregistrer ; banc-obrain-release est un clone non enregistré
- Branches sans worktree (18) : feat/selfd-edit, feat/slots512-wiring, feat/zorder, feat/release-m6-zorder, feat/llama-obrain-index-persist (déplacé vers repo llama-obrain ?), fix/ecrivain-unique-bridge, fix/iam-pont-lecture, fix/ch8b-engrams, fusion/j4-final, fusion/tronc-plus-j6, main, v4, tmp-base, sauvegarde/*, consolidation/tronc-release-2026-09-15 — branches mortes ou en attente de worktree ?
- Désalignement worktree/branche à clarifier : banc-obrain-fix-ecrivain (→feat/reliefs-n3-outils-cognitifs) et wt-selfd-edit (→fix/selfd-exec-arriere-plan) portent des noms qui ne reflètent pas leur branche
- Le brief annonce « ~200 .md » et « ~24 crates » : la réalité est 383 .md (490 fichiers) et 24 dossiers crates = 23 packages + conteneur bindings (28 membres workspace) — périmètre à recaler
- Données substrate (runtime*, x/, acceptation-chat-*, self-demo) présentes à la racine du dépôt : s'agit-il d'une fuite, d'un artefact de test ou d'un état voulu ? nommage runtime-dev.obrain vs runtime-dev incohérent



