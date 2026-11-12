# obrain-c — ABI C d'Obrain

Liaison C (`cdylib` + `staticlib`) de la base de données graphe
[Obrain](https://github.com/obrain-db/obrain). C'est le **socle** des liaisons
sans runtime Rust (Go via cgo, C# via P/Invoke, Dart via `dart:ffi`), et elle
s'utilise directement depuis C/C++.

L'ABI n'est **pas figée** (`publish = false`) : les noms et signatures peuvent
encore changer entre deux versions ; l'en-tête commité `obrain.h` fait foi.

## Construire

```bash
cargo build --release -p obrain-c                 # profil `embedded` (GQL, index vectoriel, texte, CDC, algos)
cargo build --release -p obrain-c --features full  # + Cypher, Gremlin, GraphQL, SPARQL, SQL/PGQ, RDF
# target/release/libobrain_c.{so,dylib}  (dynamique)   target/release/libobrain_c.a  (statique)
```

L'en-tête est `crates/bindings/c/obrain.h`. Il est **généré par cbindgen
0.29.4** (version épinglée dans `build.rs`, `cbindgen.toml` et la CI) : si
l'outil est présent à cette version, `build.rs` le régénère à chaque
compilation (à commiter) ; sinon l'en-tête commité fait foi. Le test
`header_a_jour` (`cargo test -p obrain-c header_a_jour`) échoue si l'en-tête
commité diverge du généré. Installer l'outil : `cargo install cbindgen --locked
--version 0.29.4`.

### Features et macros

Les fonctions compilées sous une feature Cargo ne sont déclarées dans
`obrain.h` que derrière la macro C correspondante, pour que l'en-tête soit le
même quel que soit le profil. Définissez celles de votre build :

| feature Cargo  | macro C                         | fonctions                                   |
|----------------|---------------------------------|---------------------------------------------|
| `vector-index` | `OBRAIN_FEATURE_VECTOR_INDEX`   | `obrain_*_vector_index`, `obrain_vector_search`, `obrain_mmr_search`, `obrain_batch_create_nodes`, `obrain_free_vector_results` |
| `cypher`       | `OBRAIN_FEATURE_CYPHER`         | `obrain_execute_cypher`                     |
| `gremlin`      | `OBRAIN_FEATURE_GREMLIN`        | `obrain_execute_gremlin`                    |
| `graphql`      | `OBRAIN_FEATURE_GRAPHQL`        | `obrain_execute_graphql`                    |
| `sparql`       | `OBRAIN_FEATURE_SPARQL`         | `obrain_execute_sparql`                     |
| `sql-pgq`      | `OBRAIN_FEATURE_SQL_PGQ`        | `obrain_execute_sql`                        |

Le profil par défaut (`embedded`) inclut `vector-index`.

## Exemple

```c
#include "obrain.h"
#include <stdio.h>

int main(void) {
    ObrainDatabase *db = obrain_open_write("./ma_base");          /* crée la base si absente */
    if (!db) { fprintf(stderr, "%s\n", obrain_last_error()); return 1; }

    ObrainResult *r = obrain_execute(db, "INSERT (:Personne {nom: 'Alix', age: 30})");
    if (!r) { fprintf(stderr, "%s\n", obrain_last_error()); return 1; }
    obrain_free_result(r);

    r = obrain_execute(db, "MATCH (p:Personne) RETURN p.nom AS nom, p.age AS age");
    if (!r) { fprintf(stderr, "%s\n", obrain_last_error()); return 1; }
    printf("%zu ligne(s) : %s\n", obrain_result_row_count(r), obrain_result_json(r));
    obrain_free_result(r);                                        /* libère aussi le JSON */

    if (obrain_close(db) != OBRAIN_STATUS_OK) return 1;           /* rend le bail d'écriture */
    obrain_free_database(db);
    return 0;
}
```

```bash
cc exemple.c -I crates/bindings/c -L target/release -lobrain_c -Wl,-rpath,target/release -o exemple
```

Le programme `tests/smoke.c` est la version complète de cet exemple ; le test
Rust `smoke_c` le compile avec le `cc` de la machine, le lie à la bibliothèque
que `cargo test` vient de construire et l'exécute sur une base temporaire.

## Modes d'ouverture et bail (décision D4)

| fonction                     | mode                                   | base absente |
|------------------------------|----------------------------------------|--------------|
| `obrain_open_memory()`       | mémoire, sans fichier                  | —            |
| `obrain_open(path)`          | **lecture seule** (verrou partagé)     | erreur, rien n'est créé |
| `obrain_open_read_only(path)`| synonyme de `obrain_open`              | erreur       |
| `obrain_open_write(path)`    | écriture partagée (`open_shared_write`) + **bail** | créée |

- **La lecture seule est le défaut** : `obrain_open` ne modifie ni ne crée
  aucun fichier, et toute mutation (INSERT, SET, CRUD) rend une erreur. Un
  nombre quelconque de processus peut lire la même base pendant qu'un écrivain
  partagé journalise ses mutations ; un lecteur les voit à son ouverture
  (rejeu du WAL).
- **Le bail d'écriture** : `obrain_open_write` prend, dans le processus, un
  bail sur le chemin canonique de la base. Tant que ce handle n'est ni fermé
  (`obrain_close`) ni libéré (`obrain_free_database`), un second
  `obrain_open_write` sur la même base rend `NULL` et `obrain_last_error()`
  explique le refus (« bail d'écriture déjà tenu dans ce processus »). Ce bail
  est **tenu par cette bibliothèque, par processus** : le moteur, lui, admet
  sciemment plusieurs écrivains partagés sur une base (y compris depuis
  d'autres processus, MP-6/MP-8) ; un écrivain exclusif d'un autre processus
  est, lui, refusé par le verrou du substrat. Un écrivain unique **entre**
  processus relève du déploiement (un seul processus écrivain par base).
- `obrain_close` vide les écritures en attente et rend le bail ; il est
  idempotent. `obrain_free_database` rend aussi le bail si `obrain_close` a été
  omis.

## Exports

Lecture des erreurs : `obrain_last_error`, `obrain_clear_error`.

Cycle de vie : `obrain_open_memory`, `obrain_open`, `obrain_open_read_only`,
`obrain_open_write`, `obrain_close`, `obrain_free_database`, `obrain_version`.

Requêtes : `obrain_execute` (GQL), `obrain_execute_with_params` (GQL +
paramètres JSON), `obrain_execute_cypher`, `obrain_execute_gremlin`,
`obrain_execute_graphql`, `obrain_execute_sparql`, `obrain_execute_sql`
(sous macro de feature).

Résultats : `obrain_result_json` (tableau JSON d'objets colonne → valeur),
`obrain_result_row_count`, `obrain_result_execution_time_ms`,
`obrain_result_rows_scanned`, `obrain_free_result`.

Nœuds : `obrain_create_node`, `obrain_get_node`, `obrain_delete_node`,
`obrain_set_node_property`, `obrain_remove_node_property`,
`obrain_add_node_label`, `obrain_remove_node_label`, `obrain_get_node_labels`,
`obrain_node_id`, `obrain_node_labels_json`, `obrain_node_properties_json`,
`obrain_free_node`.

Arêtes : `obrain_create_edge`, `obrain_get_edge`, `obrain_delete_edge`,
`obrain_set_edge_property`, `obrain_remove_edge_property`, `obrain_edge_id`,
`obrain_edge_source_id`, `obrain_edge_target_id`, `obrain_edge_type`,
`obrain_edge_properties_json`, `obrain_free_edge`.

Index de propriété : `obrain_create_property_index`,
`obrain_drop_property_index`, `obrain_has_property_index`,
`obrain_find_nodes_by_property`, `obrain_free_node_ids`.

Index vectoriel (`OBRAIN_FEATURE_VECTOR_INDEX`) : `obrain_create_vector_index`,
`obrain_drop_vector_index`, `obrain_rebuild_vector_index`,
`obrain_vector_search`, `obrain_mmr_search`, `obrain_batch_create_nodes`,
`obrain_free_vector_results`.

Statistiques : `obrain_node_count`, `obrain_edge_count`.

Transactions : `obrain_begin_transaction`,
`obrain_begin_transaction_with_isolation` (0 = ReadCommitted,
1 = SnapshotIsolation, 2 = Serializable), `obrain_transaction_execute`,
`obrain_transaction_execute_with_params`, `obrain_commit`, `obrain_rollback`,
`obrain_free_transaction` (rollback automatique si ni commit ni rollback).

Administration : `obrain_info` (JSON), `obrain_wal_checkpoint`,
`obrain_clear_plan_cache`, `obrain_save` (rend toujours une erreur : plus de
format mono-fichier depuis la bascule substrat ; copier le répertoire).

Mémoire : `obrain_free_string`.

Valeurs : les propriétés et paramètres voyagent en JSON. Les types temporels
sont encodés en objets `{"$timestamp_us": …}`, `{"$date": "…"}`,
`{"$time": "…"}`, `{"$duration": "…"}`, `{"$zoned_datetime": "…"}` ; les
octets en tableau d'entiers ; les vecteurs en tableau de flottants ; un chemin
en `{"$path": {"nodes": […], "edges": […]}}`.

## Contrat mémoire (qui libère quoi)

| ce que vous recevez | propriétaire | à libérer par |
|---------------------|--------------|---------------|
| `ObrainDatabase*`, `ObrainTransaction*`, `ObrainResult*`, `ObrainNode*`, `ObrainEdge*` | vous | `obrain_free_database`, `obrain_free_transaction`, `obrain_free_result`, `obrain_free_node`, `obrain_free_edge` |
| `const char*` de `obrain_result_json`, `obrain_node_labels_json`, `obrain_node_properties_json`, `obrain_edge_type`, `obrain_edge_properties_json` | le handle parent | **rien** : valide jusqu'au `obrain_free_*` du parent |
| `char*` de `obrain_get_node_labels`, `obrain_info` | vous | `obrain_free_string` |
| `const char*` de `obrain_last_error`, `obrain_version` | la bibliothèque | **rien** (`obrain_last_error` : valide jusqu'au prochain appel sur le même thread) |
| `uint64_t*` de `obrain_find_nodes_by_property`, `obrain_batch_create_nodes` | vous | `obrain_free_node_ids(ids, count)` (`count` = nombre de vecteurs pour `batch_create_nodes`) |
| `uint64_t*` + `float*` de `obrain_vector_search`, `obrain_mmr_search` | vous | `obrain_free_vector_results(ids, distances, count)` |

Les chaînes que vous passez (`const char*`) sont UTF-8, terminées par NUL, et
ne sont lues que le temps de l'appel. Tout pointeur nul en argument est
détecté : statut `OBRAIN_STATUS_ERROR_NULL_POINTER`, pointeur `NULL` ou `-1`
selon la fonction, jamais un déréférencement. Un handle déjà libéré n'est pas
détectable : ne l'utilisez plus.

## Threads

- Un `ObrainDatabase*` peut être partagé entre threads : la bibliothèque
  tient le moteur derrière `Arc<RwLock>` et le moteur synchronise lui-même ses
  structures ; des appels concurrents sur le même handle sont sûrs.
- `obrain_last_error` est **propre au thread** : lisez-le sur le thread qui a
  reçu l'erreur, avant tout autre appel à la bibliothèque.
- Un `ObrainTransaction*` appartient à **un thread à la fois** (les appels
  sont sérialisés par un mutex, mais l'ordre begin → execute → commit n'a de
  sens que depuis un seul fil).
- `ObrainResult*`, `ObrainNode*`, `ObrainEdge*` sont immuables après
  création : lisibles depuis n'importe quel thread, à libérer une seule fois.

## Codes de statut

`OBRAIN_STATUS_OK` (0), `…_ERROR_DATABASE` (1), `…_ERROR_QUERY` (2),
`…_ERROR_TRANSACTION` (3), `…_ERROR_STORAGE` (4), `…_ERROR_IO` (5),
`…_ERROR_SERIALIZATION` (6), `…_ERROR_INTERNAL` (7), `…_ERROR_NULL_POINTER`
(8), `…_ERROR_INVALID_UTF8` (9). Les fonctions qui rendent un pointeur rendent
`NULL` en erreur ; celles qui rendent un entier rendent `-1` (ou `u64::MAX`
pour un identifiant) ; dans tous les cas `obrain_last_error()` porte le message.

## Tests

```bash
cargo test -p obrain-c            # unitaires (CRUD, transactions, bail, lecture seule), header_a_jour, smoke_c
```

Licence : Apache-2.0 (voir `LICENSE`).
