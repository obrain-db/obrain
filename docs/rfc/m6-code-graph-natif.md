# RFC M6 — Indexeur de code natif obrain (remplace l'indexeur codebase-memory)

**Statut** : brouillon — exploration de l'indexeur C en cours, sections à compléter.
**Date** : 2026-09-06 — branche `feat/m6-code-graph` (worktree `banc-obrain-m6`),
dérivée de `feat/zorder` HEAD `b64f4928`.
**Décision utilisateur** : « l'export code→graph doit être un module d'obrain » —
l'indexeur de code devient natif substrate, remplaçant le rôle de
l'indexeur codebase-memory (C externe).

---

## 1. Problème

Obrain dépend d'un indexeur de code EXTERNE (le binaire C `codebase-memory-mcp`)
pour produire et servir le graphe de code (fonctions, méthodes, appels, usages).
Ce binaire est hors de l'arbre obrain ; son remplacement est le chantier M6 :
un module NATIF obrain qui indexe un dépôt de code source et écrit le graphe
directement dans le substrate.

## 2. Cible (contrat à répliquer)

### 2.1 Le schéma cible est OBSERVABLE dans le store obrain

Le projet `obrain-engine` (le repo banc-obrain lui-même) est DÉJÀ indexé par
l'indexeur C dans le graphe de code : 35 340 nœuds, 248 192 arêtes. C'est le
contrat exact que M6 doit produire nativement. Schéma observé (2026-09-06) :

**Labels de nœuds** (par volume) :
| Label | Compte | Notes |
|---|---|---|
| Function | 11 694 | fonctions libres, avec `qualified_name`, `file_path`, `complexity` (cyclomatic), `cognitive`, `loop_depth`, `transitive_loop_depth`, `linear_scan_in_loop`, `alloc_in_loop`, `recursive`, `param_count`, `signature`, `docstring`, `is_test`, `scc_id`... |
| Method | 10 943 | méthodes, + `parent_class` |
| Field | 5 040 | champs, + `parent_class` |
| Variable | 1 811 | |
| Struct | 1 410 | |
| File | 1 023 | + `change_count`, `last_modified` |
| Module | 1 011 | |
| Route | 964 | + `key_path`, `source` (routes HTTP détectées) |
| Class | 527 | + `base_classes` |
| Enum | 345 | |
| Interface | 66 | |
| Type | 53 | |
| EnvVar | 25 | + `env_key` (variables d'env détectées) |
| Package | 2 | |

**Types d'arêtes** (par volume) :
| Type | Compte | Sémantique |
|---|---|---|
| USAGE | 79 357 | référence/utilisation d'un symbole |
| CALLS | 70 716 | appel de fonction/méthode |
| DEFINES | 34 679 | conteneur → symbole qu'il définit |
| TESTS | 18 925 | test → symbole testé |
| DEFINES_METHOD | 10 711 | class/impl → méthode |
| SIMILAR_TO | 7 940 | similarité de code (jaccard) |
| WRITES | 4 936 | écriture de variable/champ |
| IMPORTS | 2 823 | import de module |
| GRAPHQL_CALLS | 2 075 | appels GraphQL (routes) |
| OVERRIDE | 1 510 | redéfinition de méthode |
| CONTAINS_FILE | 1 023 | dossier → fichier |
| SEMANTICALLY_RELATED | 585 | similarité sémantique |
| IMPLEMENTS | 312 | implémentation d'interface |
| CONFIGURES | 255 | config → clé |
| FILE_CHANGES_WITH | 151 | co-évolution de fichiers |
| HTTP_CALLS | 21 | appels HTTP entre routes |
| + rares | ~10 | CALL_REFERENCE, INHERITS, RAISES, THROWS, DEPENDS_ON... |

**Propriétés de complexité calculées** (sur Function/Method) : cyclomatic,
cognitive, loop_count/depth, transitive_loop_depth, linear_scan_in_loop,
alloc_in_loop, recursion_in_loop, unguarded_recursion, max_access_depth,
param_count, is_entry_point, is_exported, is_test — l'indexeur C calcule des
métriques statiques riches, pas seulement le graphe.

### 2.1bis Contrat EXHAUSTIF (rapport d'exploration du code C, 2026-09-06)

Le schéma est OUVERT (chaînes arbitraires — pas de liste blanche centralisée) :
les labels/arêtes ci-dessous sont l'union des sites d'émission dans `src/` +
introspection DB. C'est LE contrat de référence pour M6 (au-delà du sous-
ensemble observé en §2.1, qui dépend du corpus).

**Labels** (24) : Project, Branch, Folder, File (`__file__`, extension
conservée), Module, Function, Method, Class, Interface (Protocol inclus),
Struct, Enum, Type, Trait (mappé Interface pour Rust ?), Field, Variable,
Macro (C/C++, full mode), Section (Markdown/TOML...), Route, Channel,
EnvVar, Decorator, Resource (K8s), Package (Helm), Table/View/Model
(SQL/dbt, cibles de lineage), Constructor (legacy). + Hyperedge (transitoire).

**Arêtes cœur** (graphe de code) : DEFINES, DEFINES_METHOD, IMPORTS
(local_name), CALLS (strategy/confidence/line/args), USAGE,
CALL_REFERENCE, CONTAINS_FILE, CONTAINS_FOLDER, HAS_BRANCH, INHERITS,
IMPLEMENTS, OVERRIDE, DECORATES, THROWS, RAISES, WRITES, READS, EMITS,
LISTENS_ON, TESTS, TESTS_FILE.

**Arêtes enrichissement** : SIMILAR_TO (MinHash, jaccard), SEMANTICALLY_RELATED
(embeddings, opt-in CBM_SEMANTIC_ENABLED=1), FILE_CHANGES_WITH (git coupling),
CONFIGURES (config/env, strategy/config_key).

**Arêtes web/routes** : HTTP_CALLS, ASYNC_CALLS, HANDLES (via framework),
INFRA_MAPS (K8s/IaC), DATA_FLOWS (inter-service).

**Arêtes cross-repo** (mode cross-repo-intelligence) : CROSS_HTTP_CALLS,
CROSS_ASYNC_CALLS, CROSS_CHANNEL, CROSS_GRPC_CALLS.

**Dérivés matérialisés** (hors graphe binaire) : hyperedges DISPATCH
(n-aires), triangles, cliques (K4/K5), deg_in/deg_out, scc_id/scc_size.

### 2.2 Parseur et langages

L'indexeur C utilise **tree-sitter** avec des grammaires VENDORED (compilées
dans le binaire). **159 langages** détectés (`internal/cbm/grammar_*.c`) :
ada, agda, apex, bash, c, cpp, c_sharp, clojure, cmake, cobol, dart,
devicetree, dockerfile, elixir, elm, erlang, fish, fortran, fsharp, go,
gomod, graphql, groovy, haskell, hcl, hlsl, html, java, javascript, json,
julia, kotlin, lean, lua, makefile, markdown, matlab, nix, objc, ocaml,
odin, pascal, perl, php, pine, pkl, po, pony, powershell, prisma, protobuf,
puppet, purescript, python, qml, racket, ruby, rust, scala, scheme, scss,
slang, smali, smithy, solidity, sql, squirrel, svelte, swift, systemverilog,
tcl, teal, templ, thrift, toml, tsx, typescript, typst, verilog, vhdl, vim,
vue, wgsl, xml, yaml, zig, zsh + ~80 autres.

→ **Périmètre M6 (décision utilisateur, 2026-09-06)** : « à minima ÉQUIVALENT
et on veut encore MIEUX ». Le module natif doit couvrir l'équivalent
fonctionnel complet de l'indexeur C (pas un sous-ensemble), puis le
dépasser. Implications :
- **Langages** : viser la couverture équivalente (les 159 grammaires à terme),
  pas un sous-ensemble — mais l'architecture reste extensible (une grammaire =
  un module), avec un ordre d'implémentation qui commence par les langages du
  repo obrain + écosystème courant, puis s'étend jusqu'à la parité.
- **Schéma et métriques** : reproduire l'exhaustivité observée (§2.1 : labels,
  arêtes, complexité cyclomatic/cognitive, loop/alloc/recursion, scc...).
- **« Encore mieux »** : le natif substrate permet des choses que l'indexeur C
  externe ne fait pas — écriture zone-direct sans WAL par op (0,25 s pour
  400k/1,6M mesuré), incrementalité dans le store, co-évolution
  (FILE_CHANGES_WITH) calculable nativement, requêtage GQL direct sur le
  graphe produit, un seul binaire (plus de dépendance à l'externe).

### 2.3 Pipeline de l'indexeur C (exploration, 2026-09-06)

Architecture : 2 arbres — `src/` (pipeline, store SQLite, CLI, serveur MCP) et
`internal/cbm/` (extraction tree-sitter 159 grammaires + LSP in-process par
langage). Pipeline ORDONNÉ :

1. **structure** — découverte du repo (fichiers, dossiers, .cbmignore).
2. **extract / registry** — parsing tree-sitter par fichier → symboles +
   définitions (registre de symboles).
3. **cross-LSP** — résolution des appels inter-fichiers via LSP in-process.
4. **resolve** — résolution finale des références.
5. **tests / githistory** — arêtes TESTS, historique git.
6. **predump** — passes dérivées : similarity, semantic, complexity, hyperedges.
7. **dump** — écriture SQLite en staging + **rename atomique** (le store n'est
   visible qu'une fois complet).

### 2.4 Identité des symboles (FQN/QN)

- FQN = `project.chemin-sans-extension.name` (le File utilise `__file__` comme
  nom et GARDE l'extension dans le chemin).
- QN spéciaux : `__env__` (variables d'env), `__channel__`, `__route__`.

### 2.5 Résolution d'appels (CALLS)

Deux mécanismes combinés :
1. **Extraction AST** (tree-sitter) des appels directs.
2. **Hybrid LSP in-process** par langage (confiance ≥ 0,6) — un vrai serveur
   LSP embarqué par langue, pas une heuristique de texte.
3. **Registre de symboles** avec scores de confiance décroissants :
   import_map 0,95 · same_module 0,90 · unique_name 0,75 · suffix 0,55 ·
   fuzzy 0,40/0,30.

### 2.6 Entrée / sortie

- **Sortie** : base SQLite `<cache>/<project>.db` (tables nodes/edges + dérivés
  triangles/cliques/hyperedges) + artefact optionnel
  `.codebase-memory/graph.db.zst`.
- **Entrée** : serveur MCP stdio (défaut, 15 outils) ou CLI
  `index_repository '{"repo_path": …}'`.
- Aucune mention d'« obrain » ni « deepseek » dans le repo (grep exhaustif) —
  le lien est externe ; MCP est le canal produit.

## 3. Existant obrain réutilisable
- `ObrainDB::bulk_import` / `native_import` (ingestion zone-direct, pas de WAL
  par op — le chemin d'import 400k/1,6M mesuré ~0,25 s).
- Graphe de code déjà indexé dans le projet `obrain-engine` (labels
  Function/Method/Class/File..., edges CALLS/USAGE/IMPORTS/DEFINES/...) — le
  schéma cible est VISIBLE dans le store, produit par l'indexeur C (contrat
  complet §2.1bis).
- Bridge MCP (`crates/obrain-cli/src/mcp_server.rs`) : sert déjà le canal
  codebase-memory pour les SESSIONS (Session/Message/ToolCall/Goal) ;
  l'extension au graphe de code suivra.
- **Note** : l'indexeur C ne mentionne nulle part obrain/deepseek (grep
  exhaustif) — le lien est implicite et externe. M6 internalise ce rôle.

## 4. Questions ouvertes (exploration terminée — 2026-09-06)

Toutes résolues par l'exploration de l'indexeur C (§2.3-2.6) :

| Question | Réponse | Où |
|---|---|---|
| Pipeline ? | structure → extract/registry → cross-LSP → resolve → tests/githistory → predump → dump atomique | §2.3 |
| Labels/arêtes exhaustifs ? | **24 labels, 30+ arêtes** — contrat complet consolidé | §2.1bis |
| qualified_name ? | FQN = project.chemin-sans-ext.name ; QN spéciaux `__env__`/`__channel__`/`__route__` | §2.4 |
| Parseur ? | tree-sitter, 159 grammaires vendored (+ Hybrid LSP 12 langues) | §2.2 |
| Résolution d'appels ? | AST + Hybrid LSP in-process (≥ 0,6) + registre (import_map 0,95 → fuzzy 0,30) | §2.5 |
| CLI / sortie ? | MCP stdio (15 outils) ou `index_repository` ; sortie SQLite staging + rename atomique | §2.6 |

**Écart à consolider** : le schéma observé dans le graphe `obrain-engine` (§2.1)
ne montre qu'un SOUS-ensemble du contrat complet de l'indexeur (le graphe
indexé manque des labels Trait/Macro/Channel/Resource/Package/Table/View/Model
et des arêtes ASYNC_CALLS/HANDLES/INFRA_MAPS/DATA_FLOWS/EMITS/LISTENS_ON/
CROSS_* — ou les a sous d'autres noms). La liste du rapport d'exploration est
le contrat de référence ; l'écart vient peut-être d'une version antérieure de
l'indexeur. À vérifier sur un corpus témoin pendant l'implémentation.

## 5. Étapes envisagées (à affiner)

1. RFC validée (ce document complété).
2. **Module d'indexation** (nouveau crate `obrain-indexer` ou sous-module) :
   parse tree-sitter → symbols → graphe. Ordre de langages : Rust, C/C++,
   Python, JS/TS, Go, Java d'abord (écosystème obrain), puis extension à la
   parité (159 grammaires) — architecture « une grammaire = un module ».
3. **Résolution d'appels** : répliquer le registre à confiance décroissante
   (import_map/same_module/unique_name/suffix/fuzzy) ; l'Hybrid LSP in-process
   par langage est le plus coûteux — à répliquer après le registre.
4. **Write natif substrate** (bulk/zone-direct, pas de WAL par op).
5. **Métriques de complexité** (cyclomatic/cognitive/loop/alloc/scc) + passes
   dérivées (similarity, semantic, hyperedges) — les « prédump » du pipeline C.
6. **Bridge MCP** : servir le graphe de code indexé nativement (remplacer la
   dépendance à l'indexeur C).
7. Bench daté vs l'indexeur C (temps d'indexation, richesse du graphe).
8. Suites vertes, ledger, commits réversibles.

## 6. DoD (définition de done)

1. **Équivalence fonctionnelle prouvée** : sur un corpus témoin (ex: le repo
   banc-obrain lui-même, ou un repo multi-langage de référence), le graphe
   produit nativement contient les mêmes nœuds/arêtes que l'indexeur C (mêmes
   labels, mêmes relations CALLS/USAGE/..., mêmes FQN) — comparé par requêtes
   équivalentes, pas nécessairement bit-à-bit (le C calcule des scores).
2. **Requêtable via le bridge MCP** : les outils du canal codebase-memory
   (search_graph, trace_path, etc.) répondent depuis le graphe obrain natif.
3. **Langages de départ** : Rust, C/C++, Python, JS/TS verts sur corpus témoin.
4. **Perf** : indexation d'un repo moyen (ex: banc-obrain) dans un temps
   comparable ou meilleur que l'indexeur C (bench daté, même machine).
5. Toutes suites vertes (engine, substrate, core, invariants), ledger + commits
   réversibles, 0 rouge.
6. `SUBSTRATE_FORMAT_VERSION` inchangé (le module ÉCRIT dans le format
   existant, n'en crée pas un nouveau).
