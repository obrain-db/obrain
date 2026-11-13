# Obrain C# Bindings

C# bindings for the [Obrain](https://obrain.dev) graph database.

## Quick Start

```csharp
using Obrain;

// Create an in-memory database
await using var db = ObrainDB.Memory();

// Execute a GQL query
db.Execute("INSERT (:Person {name: 'Alix', age: 30})");

// Query with parameters
var result = db.ExecuteWithParams(
    "MATCH (p:Person) WHERE p.name = $name RETURN p.name, p.age",
    new Dictionary<string, object?> { ["name"] = "Alix" });

foreach (var row in result.Rows)
    Console.WriteLine($"{row["p.name"]}: {row["p.age"]}");

// Async execution
var asyncResult = await db.ExecuteAsync("MATCH (p:Person) RETURN p");

// ACID transactions with auto-rollback
using var tx = db.BeginTransaction();
tx.Execute("INSERT (:Person {name: 'Gus'})");
tx.Execute("INSERT (:Person {name: 'Vincent'})-[:KNOWS]->(:Person {name: 'Jules'})");
tx.Commit(); // rolls back automatically if not reached
```

## Opening a Database (read-only by default, decision D4)

- `ObrainDB.Open(path)`: opens an **existing** store **read-only**. Nothing is created or modified; mutations throw `ObrainException`.
- `ObrainDB.OpenWrite(path)`: opens or creates the store **for writing** (shared-write mode, WAL-journaled). The library holds the write lease for `path` in the process: a second `OpenWrite` on the same store throws until the first instance is disposed.
- `ObrainDB.Memory()`: in-memory database, no file.

The lease lives in the C ABI (`obrain-bindings-common`); the C# binding only calls `obrain_open_write`.

`Save(path)` is not offered: the C ABI no longer supports single-file snapshot export (`obrain_save` returns `Unsupported`). Open the store directory with `OpenWrite` instead.

## Building from Source

1. Build the Obrain C library with the `languages` profile (the P/Invoke surface includes the query languages and vector search):
   ```bash
   cargo build --release -p obrain-c --features languages
   ```

2. Build and test, pointing the runtime loader at the library directory:
   ```bash
   cd crates/bindings/csharp
   export OBRAIN_C_LIB=/path/to/dir/containing/libobrain_c
   LD_LIBRARY_PATH="$OBRAIN_C_LIB" DYLD_LIBRARY_PATH="$OBRAIN_C_LIB" dotnet build
   LD_LIBRARY_PATH="$OBRAIN_C_LIB" DYLD_LIBRARY_PATH="$OBRAIN_C_LIB" dotnet test
   ```

On Windows, put `obrain_c.dll` next to the test assembly or on `PATH`.

The P/Invoke declarations in `src/Obrain/Native/NativeMethods.cs` are written by hand from `crates/bindings/c/obrain.h`; they are not generated.

## Requirements

- .NET 8.0 SDK (CI pins 8.x)
- `obrain_c` native library (built from the `obrain-c` crate)

## License

Apache-2.0
