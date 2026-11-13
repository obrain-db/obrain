# Obrain Dart Bindings

Dart FFI bindings for the [Obrain](https://obrain.dev) graph database. Wraps the `obrain-c` shared library for native performance with a Dart-idiomatic API.

## Installation

```yaml
dependencies:
  obrain: ^0.5.24
```

You also need the `obrain-c` native library for your platform. See [Building from Source](#building-from-source) below.

## Quick Start

```dart
import 'package:obrain/obrain.dart';

void main() {
  final db = ObrainDB.memory();

  // Insert data
  db.execute("INSERT (:Person {name: 'Alix', age: 30})");
  db.execute("INSERT (:Person {name: 'Gus', age: 28})");

  // Query with parameters
  final result = db.executeWithParams(
    r'MATCH (p:Person) WHERE p.age > $minAge RETURN p.name, p.age',
    {'minAge': 25},
  );

  for (final row in result.rows) {
    print('${row['p.name']}: ${row['p.age']}');
  }

  // Transactions
  final tx = db.beginTransaction();
  tx.execute("INSERT (:City {name: 'Amsterdam'})");
  tx.execute("INSERT (:City {name: 'Berlin'})");
  tx.commit();

  // CRUD operations
  final nodeId = db.createNode(['Person'], {'name': 'Vincent'});
  db.setNodeProperty(nodeId, 'role', 'hitman');
  final node = db.getNode(nodeId);
  print(node); // Node(id, [Person], {name: Vincent, role: hitman})

  db.close();
}
```

## API Reference

### ObrainDB

| Method | Description |
|--------|-------------|
| `ObrainDB.memory()` | Open an in-memory database |
| `ObrainDB.open(path)` | Open an EXISTING store **read-only** (decision D4); mutations throw |
| `ObrainDB.openWrite(path)` | Open or create a store for writing; holds the write lease (a second `openWrite` on the same store throws until `close()`) |
| `ObrainDB.version()` | Get the library version string |
| `execute(query)` | Execute a GQL query |
| `executeWithParams(query, params)` | Execute with parameters |
| `executeCypher(query)` | Execute a Cypher query |
| `executeSparql(query)` | Execute a SPARQL query |
| `beginTransaction()` | Start an ACID transaction |
| `createNode(labels, properties)` | Create a node, returns ID |
| `getNode(id)` | Get a node by ID |
| `deleteNode(id)` | Delete a node |
| `createEdge(src, dst, type, props)` | Create an edge, returns ID |
| `getEdge(id)` | Get an edge by ID |
| `deleteEdge(id)` | Delete an edge |
| `setNodeProperty(id, key, value)` | Set a node property |
| `setEdgeProperty(id, key, value)` | Set an edge property |
| `nodeCount` | Number of nodes |
| `edgeCount` | Number of edges |
| `info()` | Database metadata as JSON map |
| `close()` | Close and flush |

### Transaction

| Method | Description |
|--------|-------------|
| `execute(query)` | Execute within transaction |
| `executeWithParams(query, params)` | Execute with parameters |
| `commit()` | Make changes permanent |
| `rollback()` | Discard changes |

### Types

- **`QueryResult`**: rows, columns, nodes, edges, executionTimeMs, rowsScanned
- **`Node`**: id, labels, properties
- **`Edge`**: id, type, sourceId, targetId, properties
- **`VectorResult`**: nodeId, distance

## Building from Source

```bash
# From the repository root: build the native library with the query-language
# and vector surface that the bindings expose
cargo build --release -p obrain-c --features languages

# Point the loader at the library directory (never hard-code a path)
export OBRAIN_C_LIB="$PWD/target/release"

cd crates/bindings/dart
dart pub get
dart test
```

`lib/src/ffi/bindings.dart` is generated from `crates/bindings/c/obrain.h` by the pinned ffigen (see `ffigen.yaml`). After a header change, regenerate and commit it:

```bash
dart run ffigen --config ffigen.yaml
```

CI reruns the generator and fails if the committed file differs.

## License

Apache-2.0. See [LICENSE](../../LICENSE) for details.
