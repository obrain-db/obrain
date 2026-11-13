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
| **Un commit acquitté est durable** | Quand `commit()` (explicite, ou implicite pour chaque statement hors transaction) rend `Ok`, les écritures sont dans le WAL **et** acquittées par le disque (`fsync` — `F_FULLFSYNC` sur macOS). Trois tests, trois couvertures distinctes : **crash du processus (`kill -9`)** — tout commit acquitté est là à la réouverture (ce test seul passe aussi sans `fsync` : le cache de pages survit au processus, mesuré) ; **oracle de `fsync`** — après chaque `commit()` acquitté, la ligne durable du WAL (`wal_octets_durables`, l'offset couvert par un `fsync` terminé) couvre la frame du commit (rouge sans `fsync`) ; **coupure simulée (WAL tronqué à la ligne durable)** — tout ce qui n'était pas `fsync`é est retiré du WAL, exactement les commits acquittés reviennent. Une vraie coupure de courant n'est pas testée : la troncature ne rejoue la coupure que pour le WAL, pas pour les zones mmap. | `obrain-engine/tests/durabilite_commit_kill9.rs` (`un_commit_acquitte_survit_a_un_kill_9`, oracle dans chaque enfant, `une_coupure_simulee_ne_garde_que_les_commits_durables`), `obrain-substrate/tests/wal_fsync_partage.rs` |
| **Une transaction jamais commitée ne laisse rien** | Un processus qui disparaît avec une transaction ouverte ne publie rien à la réouverture : le WAL porte les frontières de transaction, le rejeu écarte ce qui n'a pas été scellé. | `durabilite_commit_kill9.rs` (test 2) |
| **Un commit refusé ne laisse rien** | Un conflit de validation (deux sessions, même entité, la seconde commitée après le début de la première) est refusé, et rien de la transaction refusée ne reste — ni en mémoire, ni sur disque après réouverture. | `obrain-engine/tests/echec_commit_rembobinage.rs` |
| **`ROLLBACK` ne retire que ses propres créations** | Deux sessions concurrentes : annuler l'une ne touche jamais aux commits de l'autre. | `obrain-engine/tests/rollback_isolation.rs` |
| **Isolation par instantané** | Une transaction ouverte ne voit pas un commit voisin postérieur à son début. | `readme_examples.rs::mvcc_snapshot_isolation_…` |
| **Reprise après crash** | Un WAL tronqué ou déchiré est rejoué jusqu'à la frontière valide, jamais au-delà ; un trou de zéros suivi de données est nommé, pas confondu avec une fin de fichier. | `obrain-substrate/tests/crash_safety.rs`, `wal_recovery_reelle.rs` |
| **Format on-disk figé (v3)** | Une base écrite par la v0.0.1 se rouvre par toute v0.0.x ; une base d'une version plus récente est refusée, jamais lue comme vide. | `obrain-engine/tests/format_fige_v3.rs`, fixture `tests/golden/v3-mini` |
| **Opcode WAL inconnu ⇒ refus nommé, jamais lu comme vide** | Un `substrate.wal` qui porte une frame complète (CRC valide) d'un opcode que ce binaire ne connaît pas a été écrit par une version plus récente : l'ouverture est refusée avec une erreur qui nomme l'opcode et l'offset, le WAL n'est ni rejoué partiellement ni tronqué ni modifié. Une queue déchirée (frame incomplète) reste une frontière de crash. | `obrain-substrate/tests/wal_opcode_inconnu.rs`, `obrain-engine/tests/format_fige_v3.rs` (tests `…opcode…`), fixture `tests/golden/v3-mini-opcode-futur` |
| **La lecture seule n'écrit pas (`open_read_only`)** | Ouvrir une base en lecture seule, l'interroger et la fermer ne modifie **aucun octet d'aucun fichier**, n'en crée aucun, n'en supprime aucun — que la base ait été fermée proprement, laissée par un écrivain tué avant son flush, ou qu'un écrivain partagé soit vivant. Ce n'est pas une discipline du code : `substrate.meta`, le verrou, les zones et le WAL sont ouverts **sans droit d'écriture**, et meta et zones sont projetés en privé (copy-on-write). Le WAL en attente est rejoué **en mémoire** ; des degrés au CRC périmé sont recalculés en mémoire ; un graphe nommé s'ouvre lui aussi en lecture seule. Toute mutation est refusée : `Transaction(ReadOnly)` pour une requête, `LectureSeule` pour une opération du substrat (flush, checkpoint, réparation, append WAL), panique portant ce message pour les API directes infaillibles (`create_node` rend un identifiant, pas un `Result`). Un dossier qui n'est pas une base **échoue sans qu'une base y soit créée**. Sont **refusées** plutôt que servies en écrivant (`LectureSeuleRefusee`, qui dit quoi lancer) : une base d'un format antérieur à migrer, un sidecar hérité portant des vecteurs à migrer, une base sans `substrate.dirlock`. **Verrou** : partagé — il exclut l'écrivain exclusif (`open`) d'un autre processus, **pas** un écrivain partagé ni d'autres lecteurs ; face à un écrivain partagé vivant, le lecteur voit l'état de son ouverture, sans isolation au-delà (voir LIMITES). Seule exception, à la demande de l'opérateur : `OBRAIN_TRIGRAMMES_PERSISTER=1` publie le cache de trigrammes (fichiers à part, `rename` atomique). | `obrain-substrate/tests/lecture_seule_contrat.rs` (empreinte de tous les fichiers avant/après, un test par état ; écrivain vivant dans un autre processus ; base aux droits d'écriture retirés), `obrain-engine/tests/lecture_seule_contrat.rs` |
| **Un seul écrivain par base (`open`)** | Le répertoire est verrouillé (fcntl) ; un second processus est refusé, et le refus nomme le détenteur (Linux, macOS). | `obrain-substrate/tests/verrou_apres_drop.rs` |
| **Écriture partagée restreinte (`open_shared_write`)** | N processus créent des **nœuds et arêtes sans propriétés** : aucune perte, aucune collision de slots, et chacun voit les écritures des autres à sa **réouverture**. Une base héritée (sidecar non vide) est refusée, nommée, sans modification. Ne sont **pas** garantis : propriétés écrites par plusieurs processus, transactions concurrentes entre processus, visibilité sans réouverture (voir LIMITES). | `obrain-engine/tests/ecriture_multi_processus.rs`, `obrain-substrate/tests/wal_chaine_cas.rs`, `obrain-substrate/tests/mw_interprocess_append.rs`, `obrain-substrate/tests/base_heritee_shared_write.rs` |

Ce que la v0.0.1 **ne garantit pas** : l'écriture multi-processus au-delà du périmètre restreint ci-dessus (propriétés partagées, transactions concurrentes entre processus, visibilité sans réouverture), et tout ce qui est listé dans [docs/LIMITES-v0.0.1.md](docs/LIMITES-v0.0.1.md).

## Fonctionnalités (features par défaut du crate `obrain`)

- **Modèle** : graphe de propriétés étiqueté (LPG). RDF et les autres langages (Cypher, SPARQL, Gremlin, GraphQL, SQL/PGQ) existent derrière des features non activées par défaut et ne sont pas couverts par ce README.
- **Langage** : GQL (ISO/IEC 39075) — `INSERT`, `MATCH`, `SET`, `DELETE`, `RETURN`, agrégats, `CALL … YIELD`.
- **Transactions** : `begin_transaction` / `commit` / `rollback`, savepoints, auto-commit par statement, détection de conflit write-write, isolation par instantané.
- **Stockage** : en mémoire (`new_in_memory`, défaut) ou persistant (`open`, feature `storage`), format `substrate.*` v3, WAL + checkpoint.
- **Vecteurs** : `Value::Vector` stocké avec le graphe ; retrieval par paliers L0/L1/L2 (binaire 128 bits → Hamming 512 bits → cosinus f16) ; BM25 ; recherche hybride (feature `hybrid-search`, incluse dans `embedded`) en Rust (`ObrainDB::hybrid_search`) et en GQL (`CALL obrain.hybrid_search`, voir ci-dessous).
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

### Recherche hybride (feature `hybrid-search`)

`CALL obrain.hybrid_search` fusionne BM25 (index texte) et cosinus (index vectoriel) — par défaut en Reciprocal Rank Fusion. Il faut un index texte sur `:label(text_property)` (`create_text_index`) et, pour la part vectorielle, un index vectoriel sur `:label(vector_property)` (`create_vector_index`) ; sans index, l'erreur nomme ceux qu'elle attend. Le vecteur de requête est une liste de nombres ou un paramètre (`$vec`) ; sans vecteur, seul BM25 contribue. Les scores RRF sont ceux de la fusion (rang à base 1, `rrf_k` = 60) ; `fusion: 'weighted'` combine les scores normalisés avec `weights`.

```sql
CALL obrain.hybrid_search('Doc', 'title', 'emb', 'Rust graph', [1.0, 0.0, 0.0], {k: 3}) YIELD node_id, score
CALL obrain.hybrid_search('Doc', 'title', 'emb', 'Rust graph') YIELD node_id, score
CALL obrain.hybrid_search('Doc', 'title', 'emb', 'Rust graph', [1.0, 0.0, 0.0], {k: 2}) YIELD node_id MATCH (n) WHERE id(n) = node_id RETURN n.title
CALL obrain.hybrid_search('Doc', 'title', 'emb', 'Rust graph', [1.0, 0.0, 0.0], {k: 3, fusion: 'weighted', weights: [0.5, 0.5]}) YIELD node_id, score
```

Options (map en dernier argument) : `k` (défaut 10), `fusion` (`'rrf'` défaut, `'weighted'`), `rrf_k` (défaut 60), `weights`. Colonnes : `node_id`, `score`. L'enchaînement `CALL … YIELD … MATCH … RETURN` est exécuté par `readme_examples.rs` (module `recherche_hybride`).

## Couche cognitive (feature `cognitive`)

Obrain porte une couche cognitive — énergie des nœuds à demi-vie, synapses hebbiennes, cicatrices, tissu de connaissance (`risk_score`), activation propagée. Depuis le crate `obrain` :

```bash
cargo add obrain --features cognitive       # moteur : énergie, synapses, UDF, CALL obrain.cognitive.spread / distill
cargo add obrain --features cognitive-full  # + detect_gaps, synthesize, engrams, décision
```

**Opt-in.** Ouvrir une base (`ObrainDB::open_substrate`, `new_in_memory`) ne lance **aucune** tâche cognitive : énergie et synapses restent à zéro, comme sans la feature. Pour que chaque mutation alimente la couche, ouvrir la base en mode observé :

```rust
let db = obrain::ObrainDB::open_substrate_observed("./ma_base")?; // ou new_in_memory_observed()
```

Le mode observé lance un runtime tokio dédié (1 worker) si aucun n'est courant et une tâche de vidage ; la co-activation passe en `edges`. Attention : en mode observé, les écritures de la couche dans le journal (WAL) sont asynchrones et non synchronisées : ne pas l'utiliser pour des données devant survivre à un crash avant la règle de durabilité correspondante.

```sql
CALL obrain.cognitive.spread([0], {hops: 2})                 -- activation propagée depuis un nœud
CALL obrain.cognitive.distill({min_weight: 0.1})             -- connexions synaptiques fortes
MATCH (n:Person) RETURN n.name, obrain.energy(id(n)) AS energy
MATCH (n) RETURN obrain.risk(id(n)) AS risk ORDER BY risk DESC LIMIT 10
MATCH (n:Person {name: 'Alix'}) RETURN obrain.synapses(id(n)) AS connections
```

Ces appels sont exécutés par `readme_examples.rs` sous `--features cognitive` (module `cognitif`) ; ce test vérifie qu'ils ne renvoient pas d'erreur, pas leurs valeurs. Documentation du modèle : `crates/obrain-cognitive`.

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
- Serveur HTTP, protocole Bolt, playground, serveur MCP, intégrations LangChain/LlamaIndex : non implémentés dans ce dépôt, hors de cette version.
- Comparatif LDBC (`graph-bench`) rejoué avec une version épinglée.

## Documentation

- [docs/LIMITES-v0.0.1.md](docs/LIMITES-v0.0.1.md) — limites connues, chacune avec son test.
- [docs/BENCHMARKS.md](docs/BENCHMARKS.md) — méthode et chiffres.
- [docs/rfc/substrate/format-spec.md](docs/rfc/substrate/format-spec.md) — format on-disk (§ 0 : état réel v0.0.1).
- [docs/MIGRATION.md](docs/MIGRATION.md), [docs/architecture/overview.md](docs/architecture/overview.md).
- [CHANGELOG.md](CHANGELOG.md).

## Contribuer

Voir [CONTRIBUTING.md](CONTRIBUTING.md). La CI (`.github/workflows/ci.yml`) joue : format, clippy et rustdoc sans aucun avertissement (`-D warnings`), tests, compilation `--all-features` (`-D warnings`), MSRV, `cargo deny`, vérification des manifestes de publication et `cargo package` du premier crate de l'ordre de publication, les jobs navigateur (wasm32, `obrain-web`, persistance OPFS sous Chrome headless), et les tests d'intégrité sur macOS (job conditionné à la variable de dépôt `CI_MACOS`, sinon « skipped »).

## Remerciements

Obrain est un fork de [GrafeoDB](https://github.com/GrafeoDB/grafeo) (Apache-2.0) — voir [NOTICE](NOTICE). Le moteur d'exécution s'inspire de [DuckDB](https://duckdb.org/) (exécution vectorisée push-based, parallélisme morsel-driven) et de [Kuzu](https://github.com/kuzudb/kuzu) (adjacence CSR, traitement factorisé).

## Licence

Apache-2.0 — voir [LICENSE](LICENSE) et [NOTICE](NOTICE).
