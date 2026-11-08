[![CI](https://github.com/obrain-db/obrain/actions/workflows/ci.yml/badge.svg)](https://github.com/obrain-db/obrain/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![MSRV](https://img.shields.io/badge/MSRV-1.91.1-blue)](Cargo.toml)

# Obrain — base de données graphe embarquée, en Rust

Obrain est un moteur de graphe **embarqué** (une bibliothèque, pas un serveur) : stockage « topology-as-storage » sur fichiers mémoire-mappés, journal d'écriture anticipée (WAL) natif, transactions, langage GQL, algorithmes de graphe par procédures `CALL`, et vecteurs comme type de première classe.

Ce README décrit **ce que la v0.0.1 fait et prouve**. Chaque exemple de code ci-dessous est exécuté tel quel par `crates/obrain/tests/readme_examples.rs` ; chaque garantie renvoie au test qui la verrouille ; chaque chiffre renvoie à [docs/BENCHMARKS.md](docs/BENCHMARKS.md). Ce qui n'est pas vérifié est dans [Roadmap](#roadmap), pas ici. Les limites connues sont dans [docs/LIMITES-v0.0.1.md](docs/LIMITES-v0.0.1.md).

## Démarrage rapide (Rust)

```bash
cargo add obrain
```

```rust
use obrain::ObrainDB;

fn main() {
    let db = ObrainDB::new_in_memory();
    let session = db.session();

    // Build a graph
    session.execute("INSERT (:Person {name: 'Alix', age: 30})").unwrap();
    session.execute("INSERT (:Person {name: 'Gus', age: 25})").unwrap();
    session.execute(
        "MATCH (a:Person {name: 'Alix'}), (b:Person {name: 'Gus'})
         INSERT (a)-[:KNOWS {since: 2020}]->(b)"
    ).unwrap();

    // Query it
    let result = session.execute(
        "MATCH (p:Person)-[:KNOWS]->(friend)
         RETURN p.name, friend.name"
    ).unwrap();
    for row in result.iter() {
        println!("{} knows {}", row[0], row[1]);
    }

    // Run graph algorithms
    let pr = session.execute("CALL obrain.pagerank()").unwrap();
    for row in pr.iter() {
        println!("Node {} — PageRank {:.4}", row[0], row[1]);
    }
}
```

Base persistante : `ObrainDB::open("chemin/vers/base")` ouvre ou crée un répertoire `substrate.*` ; `db.close()` le ferme proprement. **`open` exige la feature `storage`** (`cargo add obrain --features storage`) : le profil `embedded` par défaut est en mémoire seulement. (Test : `stockage_persistant_rouvrir_et_relire`, sous `--features wal`.)

## Garanties

| Garantie | Ce qu'elle veut dire | Test qui la verrouille |
|---|---|---|
| **Un commit acquitté est durable** | Quand `commit()` (explicite, ou implicite pour chaque statement hors transaction) rend `Ok`, les écritures sont dans le WAL **et** acquittées par le disque (`fsync` — `F_FULLFSYNC` sur macOS). Elles survivent à un `kill -9` (testé ; la coupure de courant n'est pas testée). | `obrain-engine/tests/durabilite_commit_kill9.rs` |
| **Une transaction jamais commitée ne laisse rien** | Un processus qui disparaît avec une transaction ouverte ne publie rien à la réouverture : le WAL porte les frontières de transaction, le rejeu écarte ce qui n'a pas été scellé. | `durabilite_commit_kill9.rs` (test 2) |
| **Un commit refusé ne laisse rien** | Un conflit de validation (deux sessions, même entité, la seconde commitée après le début de la première) est refusé, et rien de la transaction refusée ne reste — ni en mémoire, ni sur disque après réouverture. | `obrain-engine/tests/echec_commit_rembobinage.rs` |
| **`ROLLBACK` ne retire que ses propres créations** | Deux sessions concurrentes : annuler l'une ne touche jamais aux commits de l'autre. | `obrain-engine/tests/rollback_isolation.rs` |
| **Isolation par instantané** | Une transaction ouverte ne voit pas un commit voisin postérieur à son début. | `readme_examples.rs::mvcc_snapshot_isolation_…` |
| **Reprise après crash** | Un WAL tronqué ou déchiré est rejoué jusqu'à la frontière valide, jamais au-delà ; un trou de zéros suivi de données est nommé, pas confondu avec une fin de fichier. | `obrain-substrate/tests/crash_safety.rs`, `wal_recovery_reelle.rs` |
| **Format on-disk figé (v3)** | Une base écrite par la v0.0.1 se rouvre par toute v0.0.x ; une base d'une version plus récente est refusée, jamais lue comme vide. | `obrain-engine/tests/format_fige_v3.rs`, fixture `tests/golden/v3-mini` |
| **Opcode WAL inconnu ⇒ refus nommé, jamais lu comme vide** | Un `substrate.wal` qui porte une frame complète (CRC valide) d'un opcode que ce binaire ne connaît pas a été écrit par une version plus récente : l'ouverture est refusée avec une erreur qui nomme l'opcode et l'offset, le WAL n'est ni rejoué partiellement ni tronqué ni modifié. Une queue déchirée (frame incomplète) reste une frontière de crash. | `obrain-substrate/tests/wal_opcode_inconnu.rs`, `obrain-engine/tests/format_fige_v3.rs` (tests `…opcode…`), fixture `tests/golden/v3-mini-opcode-futur` |
| **Un seul écrivain par base (`open`)** | Le répertoire est verrouillé (fcntl) ; un second processus est refusé, et le refus nomme le détenteur (Linux, macOS). | `obrain-substrate/tests/verrou_apres_drop.rs` |
| **Écriture partagée restreinte (`open_shared_write`)** | N processus créent des **nœuds et arêtes sans propriétés** : aucune perte, aucune collision de slots, et chacun voit les écritures des autres à sa **réouverture**. Une base héritée (sidecar non vide) est refusée, nommée, sans modification. Ne sont **pas** garantis : propriétés écrites par plusieurs processus, transactions concurrentes entre processus, visibilité sans réouverture (voir LIMITES). | `obrain-engine/tests/ecriture_multi_processus.rs`, `obrain-substrate/tests/wal_chaine_cas.rs`, `obrain-substrate/tests/mw_interprocess_append.rs`, `obrain-substrate/tests/base_heritee_shared_write.rs` |

Ce que la v0.0.1 **ne garantit pas** : l'écriture multi-processus au-delà du périmètre restreint ci-dessus (propriétés partagées, transactions concurrentes entre processus, visibilité sans réouverture), et tout ce qui est listé dans [docs/LIMITES-v0.0.1.md](docs/LIMITES-v0.0.1.md).

## Fonctionnalités (features par défaut du crate `obrain`)

- **Modèle** : graphe de propriétés étiqueté (LPG). RDF et les autres langages (Cypher, SPARQL, Gremlin, GraphQL, SQL/PGQ) existent derrière des features non activées par défaut et ne sont pas couverts par ce README.
- **Langage** : GQL (ISO/IEC 39075) — `INSERT`, `MATCH`, `SET`, `DELETE`, `RETURN`, agrégats, `CALL … YIELD`.
- **Transactions** : `begin_transaction` / `commit` / `rollback`, savepoints, auto-commit par statement, détection de conflit write-write, isolation par instantané.
- **Stockage** : en mémoire (`new_in_memory`, défaut) ou persistant (`open`, feature `storage`), format `substrate.*` v3, WAL + checkpoint.
- **Vecteurs** : `Value::Vector` stocké avec le graphe ; retrieval par paliers L0/L1/L2 (binaire 128 bits → Hamming 512 bits → cosinus f16) ; BM25 ; recherche hybride (feature `hybrid-search`, incluse dans `embedded`).
- **Algorithmes** (`CALL obrain.<nom>(…)`), chacun exécuté par le harnais sur un petit graphe :

| Famille | Procédures |
|---|---|
| Centralité | `pagerank`, `personalized_pagerank({seed_nodes})`, `betweenness_centrality`, `closeness_centrality`, `degree_centrality`, `hits` |
| Communautés | `louvain`, `leiden`, `label_propagation`, `stable_communities` |
| Plus courts chemins | `dijkstra(source)`, `astar(source, target)` (heuristique nulle sans coordonnées : exact, égal à Dijkstra), `bellman_ford(source)`, `sssp({source: 'nom'})` (source désignée par sa propriété `name`), `floyd_warshall` |
| Parcours et structure | `bfs(source)`, `dfs(source)`, `connected_components`, `strongly_connected_components`, `topological_sort`, `bridges`, `articulation_points`, `kcore` (alias `k_core`), `subgraph.khop(center, k)`, `irreducible_kernel`, `subgraph_contraction({node_ids})` |
| Agrégation et similarité | `clustering_coefficient`, `triangle_count`, `similarity(a, b, metric)` et `similarity.topk(a, k, metric)` avec `metric` ∈ `jaccard` (défaut), `overlap`, `cosine`, `adamic_adar` ; raccourcis `jaccard(a, b)`, `cosine_similarity(a, b)`, `adamic_adar(a, b)` |
| Flot et arbres couvrants | `max_flow({source, sink})`, `min_cost_max_flow({source, sink})` (identifiants de nœuds), `kruskal`, `prim` |
| Géométrie spectrale | `spectral_embedding`, `heat_kernel_diffusion({seed_nodes})`, `hilbert_features` |

Syntaxe :

```sql
CALL obrain.pagerank()
CALL obrain.pagerank({damping: 0.85, max_iterations: 20})
CALL obrain.louvain() YIELD node_id, community_id
CALL obrain.pagerank() YIELD node_id AS id, score AS rank
```

## Couche cognitive (feature `cognitive`)

Obrain porte une couche cognitive — énergie des nœuds à demi-vie, synapses hebbiennes, cicatrices, tissu de connaissance (`risk_score`), activation propagée. Depuis le crate `obrain` :

```bash
cargo add obrain --features cognitive       # moteur : énergie, synapses, UDF, CALL obrain.cognitive.spread / distill
cargo add obrain --features cognitive-full  # + detect_gaps, synthesize, engrams, décision
```

```sql
CALL obrain.cognitive.spread([0], {hops: 2})                 -- activation propagée depuis un nœud
CALL obrain.cognitive.distill({min_weight: 0.1})             -- connexions synaptiques fortes
MATCH (n:Person) RETURN n.name, obrain.energy(id(n)) AS energy
MATCH (n) RETURN obrain.risk(id(n)) AS risk ORDER BY risk DESC LIMIT 10
MATCH (n:Person {name: 'Alix'}) RETURN obrain.synapses(id(n)) AS connections
```

Ces appels sont exécutés par `readme_examples.rs` sous `--features cognitive` (module `cognitif`). Documentation du modèle : `crates/obrain-cognitive`.

## Performance

Les benchs reproductibles, la commande exacte, le matériel et les chiffres sont dans [docs/BENCHMARKS.md](docs/BENCHMARKS.md). Aucun chiffre n'est repris ici.

## Installation

```bash
cargo add obrain                                           # profil embedded : GQL + vecteurs/texte/hybride + algorithmes + parallélisme (en mémoire)
cargo add obrain --features storage                        # + bases persistantes (ObrainDB::open, WAL, checkpoint)
cargo add obrain --no-default-features --features gql      # minimal
cargo add obrain --no-default-features --features full     # tous les langages + stockage + RDF + CDC
```

MSRV : 1.91.1 (`rust-version` du workspace, vérifiée en CI).

## Exemples

`examples/rust/` (`cargo run -p obrain-examples --bin <nom>`) : `basic`, `transactions`, `parameterized`, `persistence`, `vector_search`, `algorithms`, `multi_language`, `social_network`, `knowledge_base`, `iot_stream`, `migrate-v2` (le nom du binaire contient un tiret).

## Roadmap

Ce qui n'est **pas** dans la v0.0.1 :

- **Bindings** Python, Node.js, Go, C, C#, Dart : aucun code dans ce dépôt. Les paquets `packages/obrain-cli-{npm,python}` sont des enveloppes du CLI. WebAssembly n'a qu'un paquet d'essai, non publié : `crates/obrain-web` (voir son README).
- **`CALL obrain.hybrid_search`** comme procédure GQL : les index texte/vecteur vivent dans `ObrainDB`, hors de portée du planificateur de `CALL`, et l'enchaînement `CALL … YIELD … MATCH` n'est pas consommé par le parseur. La recherche hybride existe en Rust : `ObrainDB::hybrid_search(label, text_property, vector_property, query_text, query_vector, k, fusion)` (fusion RRF ou pondérée).
- Serveur HTTP, protocole Bolt, playground, serveur MCP, intégrations LangChain/LlamaIndex : non implémentés dans ce dépôt, hors de cette version.
- Comparatif LDBC (`graph-bench`) rejoué avec une version épinglée.

## Documentation

- [docs/LIMITES-v0.0.1.md](docs/LIMITES-v0.0.1.md) — limites connues, chacune avec son test.
- [docs/BENCHMARKS.md](docs/BENCHMARKS.md) — méthode et chiffres.
- [docs/rfc/substrate/format-spec.md](docs/rfc/substrate/format-spec.md) — format on-disk (§ 0 : état réel v0.0.1).
- [docs/MIGRATION.md](docs/MIGRATION.md), [docs/architecture/overview.md](docs/architecture/overview.md).
- [CHANGELOG.md](CHANGELOG.md).

## Contribuer

Voir [CONTRIBUTING.md](CONTRIBUTING.md). La CI (`.github/workflows/ci.yml`) joue : format, clippy (cliquet, jamais en hausse), tests, compilation `--all-features`, MSRV, `cargo deny`, vérification des manifestes de publication et `cargo package` du premier crate de l'ordre de publication, les jobs navigateur (wasm32, `obrain-web`, persistance OPFS sous Chrome headless), et les tests d'intégrité sur macOS (job conditionné à la variable de dépôt `CI_MACOS`, sinon « skipped »).

## Remerciements

Obrain est un fork de [GrafeoDB](https://github.com/GrafeoDB/grafeo) (Apache-2.0) — voir [NOTICE](NOTICE). Le moteur d'exécution s'inspire de [DuckDB](https://duckdb.org/) (exécution vectorisée push-based, parallélisme morsel-driven) et de [Kuzu](https://github.com/kuzudb/kuzu) (adjacence CSR, traitement factorisé).

## Licence

Apache-2.0 — voir [LICENSE](LICENSE) et [NOTICE](NOTICE).
