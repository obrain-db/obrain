/* Obrain C API — en-tête GÉNÉRÉ par cbindgen (voir cbindgen.toml), ne pas éditer.
 *
 * Lier contre libobrain_c.so (Linux), libobrain_c.dylib (macOS) ou
 * obrain_c.dll (Windows), ou contre libobrain_c.a (statique).
 *
 * Contrat mémoire : voir README.md (qui libère quoi).
 * Erreurs : les fonctions rendent un ObrainStatus (0 = succès) ou un pointeur
 * nul ; obrain_last_error() donne le message, propre au thread courant.
 */

#ifndef OBRAIN_H
#define OBRAIN_H

#include <stdarg.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>

// Status codes returned by C FFI functions.
typedef enum ObrainStatus {
  // Success.
  OBRAIN_STATUS_OK = 0,
  // Database error not covered below (not found, type mismatch, invalid value…).
  OBRAIN_STATUS_ERROR_DATABASE = 1,
  // Query parsing, semantic or execution error.
  OBRAIN_STATUS_ERROR_QUERY = 2,
  // Transaction conflict, timeout, read-only store or invalid state.
  OBRAIN_STATUS_ERROR_TRANSACTION = 3,
  // Storage-layer error (disk full, corruption, recovery).
  OBRAIN_STATUS_ERROR_STORAGE = 4,
  // I/O error.
  OBRAIN_STATUS_ERROR_IO = 5,
  // Serialization / deserialization failure (including invalid JSON input).
  OBRAIN_STATUS_ERROR_SERIALIZATION = 6,
  // Internal error (should not happen in normal operation).
  OBRAIN_STATUS_ERROR_INTERNAL = 7,
  // A required pointer argument was null.
  OBRAIN_STATUS_ERROR_NULL_POINTER = 8,
  // A string argument was not valid UTF-8.
  OBRAIN_STATUS_ERROR_INVALID_UTF8 = 9,
} ObrainStatus;

// Opaque database handle. Created by `obrain_open*`, freed by `obrain_free_database`.
typedef struct ObrainDatabase ObrainDatabase;

// Structured edge returned by CRUD operations.
typedef struct ObrainEdge ObrainEdge;

// Structured node returned by CRUD operations.
typedef struct ObrainNode ObrainNode;

// Query result. Holds JSON-serialized rows and metadata.
typedef struct ObrainResult ObrainResult;

// Opaque transaction handle. Created by `obrain_begin_transaction*`, freed by `obrain_free_transaction`.
typedef struct ObrainTransaction ObrainTransaction;

#ifdef __cplusplus
extern "C" {
#endif // __cplusplus

// Create a new in-memory database.
//
// Returns an opaque pointer, or null on error (check `obrain_last_error()`).
struct ObrainDatabase *obrain_open_memory(void);

// Open an EXISTING database at `path` in READ-ONLY mode (décision D4 : la
// lecture seule est le mode par défaut de toute liaison).
//
// Shared file lock: any number of processes can read the same store while a
// shared writer (`obrain_open_write`) journals its mutations. No file is
// created or modified; a directory that is not a store is an error (no empty
// store is created there). Mutations return an error; use
// `obrain_open_write` to write.
//
// Returns an opaque pointer, or null on error (check `obrain_last_error()`).
struct ObrainDatabase *obrain_open(const char *path);

// Synonym of `obrain_open` (read-only), kept for callers written before D4.
struct ObrainDatabase *obrain_open_read_only(const char *path);

// Open (or create) the database at `path` for WRITING, in the engine's
// shared-write mode (`ObrainDB::open_shared_write`): mutations are journaled
// in the WAL and readers opened with `obrain_open` see them when they open.
//
// Holds the library's WRITE LEASE (« bail ») for `path` in this process: a
// second `obrain_open_write` on the same store, while the first handle is
// neither closed nor freed, returns null with an explanatory
// `obrain_last_error()`. The lease is released by `obrain_close` or
// `obrain_free_database`.
//
// Returns an opaque pointer, or null on error (check `obrain_last_error()`).
struct ObrainDatabase *obrain_open_write(const char *path);

// Close the database, flushing pending writes, and release the write lease
// held by `obrain_open_write` (if any).
enum ObrainStatus obrain_close(struct ObrainDatabase *db);

// Free a database handle. Must be called after `obrain_close`. Releases the
// write lease if `obrain_close` was skipped.
void obrain_free_database(struct ObrainDatabase *db);

// Returns the library version string. The pointer is static and must NOT be freed.
const char *obrain_version(void);

// Execute a GQL query. Returns a result pointer, or null on error.
struct ObrainResult *obrain_execute(struct ObrainDatabase *db, const char *query);

// Execute a GQL query with JSON-encoded parameters.
struct ObrainResult *obrain_execute_with_params(struct ObrainDatabase *db,
                                                const char *query,
                                                const char *params_json);

#if defined(OBRAIN_FEATURE_CYPHER)
// Execute a Cypher query.
struct ObrainResult *obrain_execute_cypher(struct ObrainDatabase *db, const char *query);
#endif

#if defined(OBRAIN_FEATURE_GREMLIN)
// Execute a Gremlin query.
struct ObrainResult *obrain_execute_gremlin(struct ObrainDatabase *db, const char *query);
#endif

#if defined(OBRAIN_FEATURE_GRAPHQL)
// Execute a GraphQL query.
struct ObrainResult *obrain_execute_graphql(struct ObrainDatabase *db, const char *query);
#endif

#if defined(OBRAIN_FEATURE_SPARQL)
// Execute a SPARQL query.
struct ObrainResult *obrain_execute_sparql(struct ObrainDatabase *db, const char *query);
#endif

#if defined(OBRAIN_FEATURE_SQL_PGQ)
// Execute a SQL/PGQ query (SQL:2023 GRAPH_TABLE).
struct ObrainResult *obrain_execute_sql(struct ObrainDatabase *db, const char *query);
#endif

// Get the JSON-encoded result rows. Returns null if result is null.
// The pointer is valid until `obrain_free_result` is called.
const char *obrain_result_json(const struct ObrainResult *result);

// Get the number of rows in the result.
size_t obrain_result_row_count(const struct ObrainResult *result);

// Get execution time in milliseconds.
double obrain_result_execution_time_ms(const struct ObrainResult *result);

// Get estimated rows scanned.
uint64_t obrain_result_rows_scanned(const struct ObrainResult *result);

// Free a query result.
void obrain_free_result(struct ObrainResult *result);

// Create a node with labels (JSON array) and optional properties (JSON object).
// Returns the new node ID, or `u64::MAX` on error.
uint64_t obrain_create_node(struct ObrainDatabase *db,
                            const char *labels_json,
                            const char *properties_json);

// Get a node by ID. Writes into `out`. Returns `Ok` or an error status.
// On success, `out` must be freed with `obrain_free_node`.
enum ObrainStatus obrain_get_node(struct ObrainDatabase *db, uint64_t id, struct ObrainNode **out);

// Delete a node by ID. Returns 1 if deleted, 0 if not found.
int32_t obrain_delete_node(struct ObrainDatabase *db, uint64_t id);

// Set a property on a node. `value_json` is a JSON-encoded value.
enum ObrainStatus obrain_set_node_property(struct ObrainDatabase *db,
                                           uint64_t id,
                                           const char *key,
                                           const char *value_json);

// Remove a property from a node. Returns 1 if removed, 0 if not found.
int32_t obrain_remove_node_property(struct ObrainDatabase *db, uint64_t id, const char *key);

// Add a label to a node. Returns 1 if added, 0 if already present.
int32_t obrain_add_node_label(struct ObrainDatabase *db, uint64_t id, const char *label);

// Remove a label from a node. Returns 1 if removed, 0 if not present.
int32_t obrain_remove_node_label(struct ObrainDatabase *db, uint64_t id, const char *label);

// Get labels for a node as a JSON array string.
// Returns null if node not found. Caller must free with `obrain_free_string`.
char *obrain_get_node_labels(struct ObrainDatabase *db, uint64_t id);

// Free a `ObrainNode` returned by `obrain_get_node`.
void obrain_free_node(struct ObrainNode *node);

// Access the node ID from a `ObrainNode`.
uint64_t obrain_node_id(const struct ObrainNode *node);

// Access labels JSON from a `ObrainNode`. Valid until `obrain_free_node`.
const char *obrain_node_labels_json(const struct ObrainNode *node);

// Access properties JSON from a `ObrainNode`. Valid until `obrain_free_node`.
const char *obrain_node_properties_json(const struct ObrainNode *node);

// Create an edge. `properties_json` may be null for no properties.
// Returns the new edge ID, or `u64::MAX` on error.
uint64_t obrain_create_edge(struct ObrainDatabase *db,
                            uint64_t source_id,
                            uint64_t target_id,
                            const char *edge_type,
                            const char *properties_json);

// Get an edge by ID. Writes into `out`. Returns `Ok` or error status.
// On success, `out` must be freed with `obrain_free_edge`.
enum ObrainStatus obrain_get_edge(struct ObrainDatabase *db, uint64_t id, struct ObrainEdge **out);

// Delete an edge by ID. Returns 1 if deleted, 0 if not found.
int32_t obrain_delete_edge(struct ObrainDatabase *db, uint64_t id);

// Set a property on an edge.
enum ObrainStatus obrain_set_edge_property(struct ObrainDatabase *db,
                                           uint64_t id,
                                           const char *key,
                                           const char *value_json);

// Remove a property from an edge. Returns 1 if removed, 0 if not found.
int32_t obrain_remove_edge_property(struct ObrainDatabase *db, uint64_t id, const char *key);

// Free a `ObrainEdge` returned by `obrain_get_edge`.
void obrain_free_edge(struct ObrainEdge *edge);

// Access the edge ID.
uint64_t obrain_edge_id(const struct ObrainEdge *edge);

// Access source node ID.
uint64_t obrain_edge_source_id(const struct ObrainEdge *edge);

// Access target node ID.
uint64_t obrain_edge_target_id(const struct ObrainEdge *edge);

// Access edge type string. Valid until `obrain_free_edge`.
const char *obrain_edge_type(const struct ObrainEdge *edge);

// Access edge properties JSON. Valid until `obrain_free_edge`.
const char *obrain_edge_properties_json(const struct ObrainEdge *edge);

// Create a property index.
enum ObrainStatus obrain_create_property_index(struct ObrainDatabase *db, const char *property);

// Drop a property index. Returns 1 if dropped, 0 if not found.
int32_t obrain_drop_property_index(struct ObrainDatabase *db, const char *property);

// Check if a property index exists. Returns 1 if exists, 0 otherwise.
int32_t obrain_has_property_index(struct ObrainDatabase *db, const char *property);

// Find nodes by property value. Writes node IDs into `out_ids` and count
// into `out_count`. Caller must free `*out_ids` with `obrain_free_node_ids`.
enum ObrainStatus obrain_find_nodes_by_property(struct ObrainDatabase *db,
                                                const char *property,
                                                const char *value_json,
                                                uint64_t **out_ids,
                                                size_t *out_count);

// Free a node ID array from `obrain_find_nodes_by_property`.
void obrain_free_node_ids(uint64_t *ids, size_t count);

#if defined(OBRAIN_FEATURE_VECTOR_INDEX)
// Create a vector similarity index on a node property.
// `dimensions`, `m`, and `ef_construction` use -1 for default.
// `metric` may be null for default (cosine).
enum ObrainStatus obrain_create_vector_index(struct ObrainDatabase *db,
                                             const char *label,
                                             const char *property,
                                             int32_t dimensions,
                                             const char *metric,
                                             int32_t m,
                                             int32_t ef_construction);
#endif

#if defined(OBRAIN_FEATURE_VECTOR_INDEX)
// Drop a vector index for the given label and property.
// Returns 1 if removed, 0 if not found.
int32_t obrain_drop_vector_index(struct ObrainDatabase *db,
                                 const char *label,
                                 const char *property);
#endif

#if defined(OBRAIN_FEATURE_VECTOR_INDEX)
// Rebuild a vector index by rescanning all matching nodes.
// Preserves original configuration.
enum ObrainStatus obrain_rebuild_vector_index(struct ObrainDatabase *db,
                                              const char *label,
                                              const char *property);
#endif

#if defined(OBRAIN_FEATURE_VECTOR_INDEX)
// Search for k nearest neighbors of a query vector.
// Results written to `out_ids` and `out_distances` arrays of length `*out_count`.
// Caller must free with `obrain_free_vector_results`.
// `ef` uses -1 for default.
enum ObrainStatus obrain_vector_search(struct ObrainDatabase *db,
                                       const char *label,
                                       const char *property,
                                       const float *query,
                                       size_t query_len,
                                       size_t k,
                                       int32_t ef,
                                       uint64_t **out_ids,
                                       float **out_distances,
                                       size_t *out_count);
#endif

#if defined(OBRAIN_FEATURE_VECTOR_INDEX)
// Search for diverse nearest neighbors using Maximal Marginal Relevance (MMR).
// Results written to `out_ids` and `out_distances` arrays of length `*out_count`.
// Caller must free with `obrain_free_vector_results`.
// `fetch_k` and `ef` use -1 for defaults. `lambda` uses negative for default (0.5).
enum ObrainStatus obrain_mmr_search(struct ObrainDatabase *db,
                                    const char *label,
                                    const char *property,
                                    const float *query,
                                    size_t query_len,
                                    size_t k,
                                    int32_t fetch_k,
                                    float lambda,
                                    int32_t ef,
                                    uint64_t **out_ids,
                                    float **out_distances,
                                    size_t *out_count);
#endif

#if defined(OBRAIN_FEATURE_VECTOR_INDEX)
// Bulk-insert nodes with vector properties. Returns node IDs in `out_ids`.
// `vectors` is a flat array of `vector_count * dimensions` f32 values.
// Caller must free `*out_ids` with `obrain_free_node_ids`.
enum ObrainStatus obrain_batch_create_nodes(struct ObrainDatabase *db,
                                            const char *label,
                                            const char *property,
                                            const float *vectors,
                                            size_t vector_count,
                                            size_t dimensions,
                                            uint64_t **out_ids);
#endif

#if defined(OBRAIN_FEATURE_VECTOR_INDEX)
// Free vector search result arrays.
void obrain_free_vector_results(uint64_t *ids, float *distances, size_t count);
#endif

// Get the number of nodes.
size_t obrain_node_count(struct ObrainDatabase *db);

// Get the number of edges.
size_t obrain_edge_count(struct ObrainDatabase *db);

// Begin a transaction with default isolation (snapshot isolation).
struct ObrainTransaction *obrain_begin_transaction(struct ObrainDatabase *db);

// Begin a transaction with a specific isolation level.
// Levels: 0 = ReadCommitted, 1 = SnapshotIsolation, 2 = Serializable.
struct ObrainTransaction *obrain_begin_transaction_with_isolation(struct ObrainDatabase *db,
                                                                  int32_t isolation);

// Execute a query within a transaction.
struct ObrainResult *obrain_transaction_execute(struct ObrainTransaction *tx, const char *query);

// Execute a query with params within a transaction.
struct ObrainResult *obrain_transaction_execute_with_params(struct ObrainTransaction *tx,
                                                            const char *query,
                                                            const char *params_json);

// Commit a transaction.
enum ObrainStatus obrain_commit(struct ObrainTransaction *tx);

// Rollback a transaction.
enum ObrainStatus obrain_rollback(struct ObrainTransaction *tx);

// Free a transaction handle.
void obrain_free_transaction(struct ObrainTransaction *tx);

// Clear all cached query plans.
//
// Forces re-parsing and re-optimization on next execution.
// Called automatically after DDL operations, but can be invoked manually.
enum ObrainStatus obrain_clear_plan_cache(struct ObrainDatabase *db);

// Get database info as a JSON string. Caller must free with `obrain_free_string`.
char *obrain_info(struct ObrainDatabase *db);

// T17 final cutover (2026-04-23): `obrain_save` returns
// `ObrainStatus::Unsupported`. The legacy single-file `.obrain` v1/v2
// snapshot export surface was removed together with the LpgStore
// module; substrate persists directly to its own directory layout. C
// clients that need to copy a database should open with
// `obrain_open(path)` pointing at the directory and let substrate
// manage persistence, or copy the directory at the filesystem level.
enum ObrainStatus obrain_save(struct ObrainDatabase *_db, const char *_path);

// Trigger a WAL checkpoint.
enum ObrainStatus obrain_wal_checkpoint(struct ObrainDatabase *db);

// Free a string returned by any `obrain_*` function that documents
// "caller must free with `obrain_free_string`".
void obrain_free_string(char *s);

// Returns the last error message, or null if no error.
//
// The returned pointer is valid until the next FFI call on this thread.
// The caller must NOT free this pointer.
const char *obrain_last_error(void);

// Clears the last error.
void obrain_clear_error(void);

#ifdef __cplusplus
}  // extern "C"
#endif  // __cplusplus

#endif  /* OBRAIN_H */
