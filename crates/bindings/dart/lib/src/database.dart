/// The main ObrainDB database class.
///
/// Wraps the obrain-c shared library via FFI. Uses [NativeFinalizer] to
/// prevent leaks if [close] is not called explicitly.
library;

import 'dart:convert';
import 'dart:ffi';

import 'package:ffi/ffi.dart';

import 'error.dart';
import 'ffi/bindings.dart' as native;
import 'ffi/loader.dart';
import 'ffi/strings.dart';
import 'transaction.dart';
import 'types.dart';
import 'value.dart';

/// A Obrain graph database instance.
///
/// Create with [ObrainDB.memory] (in-memory), [ObrainDB.open] (existing store,
/// read-only, decision D4) or [ObrainDB.openWrite] (store opened for writing,
/// holds the write lease).
/// Always call [close] when done, or rely on [NativeFinalizer] as a safety net.
class ObrainDB implements Finalizable {
  final native.ObrainBindings _bindings;
  final DynamicLibrary _lib;
  Pointer<native.ObrainDatabase> _handle;
  bool _closed = false;

  static NativeFinalizer? _finalizer;

  ObrainDB._(this._handle, this._bindings, this._lib) {
    // Lazily create a finalizer that calls obrain_free_database on the handle.
    // The Rust Drop impl for the inner Arc<RwLock<ObrainDB>> flushes writes.
    _finalizer ??= NativeFinalizer(
      _lib.lookup<NativeFunction<Void Function(Pointer<Void>)>>(
        'obrain_free_database',
      ),
    );
    _finalizer!.attach(this, _handle.cast(), detach: this);
  }

  // ===========================================================================
  // Lifecycle
  // ===========================================================================

  /// Create a new in-memory database.
  static ObrainDB memory({String? libraryPath}) {
    final lib = loadNativeLibrary(libraryPath);
    final bindings = native.ObrainBindings(lib);
    final ptr = bindings.obrain_open_memory();
    if (ptr == nullptr) throwLastError(bindings);
    return ObrainDB._(ptr, bindings, lib);
  }

  /// Open an EXISTING database at [path] in READ-ONLY mode (decision D4:
  /// read-only is the default for every binding).
  ///
  /// Nothing is created or modified; mutations throw. Use [openWrite] to write.
  static ObrainDB open(String path, {String? libraryPath}) {
    final lib = loadNativeLibrary(libraryPath);
    final bindings = native.ObrainBindings(lib);
    final pathPtr = toNativeChars(path);
    try {
      final ptr = bindings.obrain_open(pathPtr);
      if (ptr == nullptr) throwLastError(bindings);
      return ObrainDB._(ptr, bindings, lib);
    } finally {
      malloc.free(pathPtr);
    }
  }

  /// Open or create the database at [path] for WRITING (shared-write mode,
  /// WAL-journaled).
  ///
  /// The library holds the write lease for [path] in this process: a second
  /// [openWrite] on the same store throws until this instance is closed.
  static ObrainDB openWrite(String path, {String? libraryPath}) {
    final lib = loadNativeLibrary(libraryPath);
    final bindings = native.ObrainBindings(lib);
    final pathPtr = toNativeChars(path);
    try {
      final ptr = bindings.obrain_open_write(pathPtr);
      if (ptr == nullptr) throwLastError(bindings);
      return ObrainDB._(ptr, bindings, lib);
    } finally {
      malloc.free(pathPtr);
    }
  }

  /// Close the database, flushing all writes and releasing the write lease
  /// (if any).
  ///
  /// Safe to call multiple times. After close, all other methods throw.
  void close() {
    if (_closed) return;
    _closed = true;
    _finalizer!.detach(this);
    final status = _bindings.obrain_close(_handle).value;
    _bindings.obrain_free_database(_handle);
    _handle = nullptr;
    if (status != ObrainStatus.ok.code) {
      throw classifyError(status, lastError(_bindings));
    }
  }

  /// Returns the obrain-c library version.
  ///
  /// The C function returns a pointer to a static string that must NOT
  /// be freed (it lives in the binary's read-only data segment).
  static String version({String? libraryPath}) {
    final lib = loadNativeLibrary(libraryPath);
    final bindings = native.ObrainBindings(lib);
    return charsToDartString(bindings.obrain_version());
  }

  void _checkOpen() {
    if (_closed) {
      throw DatabaseException('Database is closed', ObrainStatus.database);
    }
  }

  // ===========================================================================
  // Query execution
  // ===========================================================================

  /// Execute a GQL query and return a [QueryResult].
  QueryResult execute(String query) {
    _checkOpen();
    final queryPtr = toNativeChars(query);
    try {
      final resultPtr = _bindings.obrain_execute(_handle, queryPtr);
      if (resultPtr == nullptr) throwLastError(_bindings);
      return _buildResult(resultPtr);
    } finally {
      malloc.free(queryPtr);
    }
  }

  /// Execute a GQL query with parameters.
  ///
  /// Parameters are JSON-encoded using the obrain-bindings-common wire format.
  /// Temporal types (DateTime, Duration) are automatically encoded as
  /// `$timestamp_us`, `$duration`, etc.
  QueryResult executeWithParams(
    String query,
    Map<String, dynamic> params,
  ) {
    _checkOpen();
    final queryPtr = toNativeChars(query);
    final paramsPtr = toNativeChars(encodeParams(params));
    try {
      final resultPtr = _bindings.obrain_execute_with_params(
        _handle,
        queryPtr,
        paramsPtr,
      );
      if (resultPtr == nullptr) throwLastError(_bindings);
      return _buildResult(resultPtr);
    } finally {
      malloc.free(queryPtr);
      malloc.free(paramsPtr);
    }
  }

  /// Execute a Cypher query (requires `cypher` feature in obrain-c).
  QueryResult executeCypher(String query) => _executeLanguage(
    query,
    _bindings.obrain_execute_cypher,
  );

  /// Execute a Gremlin query (requires `gremlin` feature in obrain-c).
  QueryResult executeGremlin(String query) => _executeLanguage(
    query,
    _bindings.obrain_execute_gremlin,
  );

  /// Execute a GraphQL query (requires `graphql` feature in obrain-c).
  QueryResult executeGraphql(String query) => _executeLanguage(
    query,
    _bindings.obrain_execute_graphql,
  );

  /// Execute a SPARQL query (requires `sparql` feature in obrain-c).
  QueryResult executeSparql(String query) => _executeLanguage(
    query,
    _bindings.obrain_execute_sparql,
  );

  QueryResult _executeLanguage(
    String query,
    Pointer<native.ObrainResult> Function(
      Pointer<native.ObrainDatabase>,
      Pointer<Char>,
    )
    fn,
  ) {
    _checkOpen();
    final queryPtr = toNativeChars(query);
    try {
      final resultPtr = fn(_handle, queryPtr);
      if (resultPtr == nullptr) throwLastError(_bindings);
      return _buildResult(resultPtr);
    } finally {
      malloc.free(queryPtr);
    }
  }

  // ===========================================================================
  // Statistics and info
  // ===========================================================================

  /// Get the number of nodes in the database (O(1), synchronous).
  int get nodeCount {
    _checkOpen();
    return _bindings.obrain_node_count(_handle);
  }

  /// Get the number of edges in the database (O(1), synchronous).
  int get edgeCount {
    _checkOpen();
    return _bindings.obrain_edge_count(_handle);
  }

  /// Get database info as a parsed JSON map.
  ///
  /// The C function allocates a string that must be freed with
  /// obrain_free_string.
  Map<String, dynamic> info() {
    _checkOpen();
    final ptr = _bindings.obrain_info(_handle);
    if (ptr == nullptr) throwLastError(_bindings);
    try {
      return parseObject(charsToDartString(ptr));
    } finally {
      _bindings.obrain_free_string(ptr);
    }
  }

  // ===========================================================================
  // Transactions
  // ===========================================================================

  /// Begin a new transaction with the default isolation level.
  Transaction beginTransaction() {
    _checkOpen();
    final txPtr = _bindings.obrain_begin_transaction(_handle);
    if (txPtr == nullptr) throwLastError(_bindings);
    return Transaction(txPtr, _bindings, _lib);
  }

  /// Begin a transaction with a specific [isolationLevel].
  Transaction beginTransactionWithIsolation(IsolationLevel isolationLevel) {
    _checkOpen();
    final txPtr = _bindings.obrain_begin_transaction_with_isolation(
      _handle,
      isolationLevel.code,
    );
    if (txPtr == nullptr) throwLastError(_bindings);
    return Transaction(txPtr, _bindings, _lib);
  }

  // ===========================================================================
  // Node CRUD
  // ===========================================================================

  /// Create a node with [labels] and [properties]. Returns the new node ID.
  int createNode(List<String> labels, Map<String, dynamic> properties) {
    _checkOpen();
    final labelsJson = jsonEncode(labels);
    final propsJson = jsonEncode(
      properties.map((k, v) => MapEntry(k, _encodeValue(v))),
    );
    final labelsPtr = toNativeChars(labelsJson);
    final propsPtr = toNativeChars(propsJson);
    try {
      final id = _bindings.obrain_create_node(_handle, labelsPtr, propsPtr);
      if (id == -1) throwLastError(_bindings); // C returns u64::MAX on error
      return id;
    } finally {
      malloc.free(labelsPtr);
      malloc.free(propsPtr);
    }
  }

  /// Get a node by [id]. Returns a [Node] or throws on error.
  Node getNode(int id) {
    _checkOpen();
    final outPtr = malloc<Pointer<native.ObrainNode>>();
    try {
      final status = _bindings.obrain_get_node(_handle, id, outPtr).value;
      if (status != ObrainStatus.ok.code) throwStatus(_bindings, status);
      final nodePtr = outPtr.value;
      try {
        final nodeId = _bindings.obrain_node_id(nodePtr);
        final labelsJson =
            charsToDartString(_bindings.obrain_node_labels_json(nodePtr));
        final propsJson =
            charsToDartString(_bindings.obrain_node_properties_json(nodePtr));
        return Node(
          nodeId,
          parseStringArray(labelsJson),
          parseObject(propsJson),
        );
      } finally {
        _bindings.obrain_free_node(nodePtr);
      }
    } finally {
      malloc.free(outPtr);
    }
  }

  /// Delete a node by [id]. Returns true if the node was deleted.
  bool deleteNode(int id) {
    _checkOpen();
    final result = _bindings.obrain_delete_node(_handle, id);
    if (result < 0) throwLastError(_bindings);
    return result == 1;
  }

  /// Set a property on node [id].
  void setNodeProperty(int id, String key, dynamic value) {
    _checkOpen();
    final keyPtr = toNativeChars(key);
    final valuePtr = toNativeChars(encodeValue(value));
    try {
      final status = _bindings
          .obrain_set_node_property(_handle, id, keyPtr, valuePtr)
          .value;
      if (status != ObrainStatus.ok.code) throwStatus(_bindings, status);
    } finally {
      malloc.free(keyPtr);
      malloc.free(valuePtr);
    }
  }

  /// Remove a property from node [id].
  void removeNodeProperty(int id, String key) {
    _checkOpen();
    final keyPtr = toNativeChars(key);
    try {
      final result = _bindings.obrain_remove_node_property(_handle, id, keyPtr);
      if (result < 0) throwLastError(_bindings);
    } finally {
      malloc.free(keyPtr);
    }
  }

  /// Add a label to node [id].
  void addNodeLabel(int id, String label) {
    _checkOpen();
    final labelPtr = toNativeChars(label);
    try {
      final result = _bindings.obrain_add_node_label(_handle, id, labelPtr);
      if (result < 0) throwLastError(_bindings);
    } finally {
      malloc.free(labelPtr);
    }
  }

  /// Remove a label from node [id].
  void removeNodeLabel(int id, String label) {
    _checkOpen();
    final labelPtr = toNativeChars(label);
    try {
      final result = _bindings.obrain_remove_node_label(_handle, id, labelPtr);
      if (result < 0) throwLastError(_bindings);
    } finally {
      malloc.free(labelPtr);
    }
  }

  // ===========================================================================
  // Edge CRUD
  // ===========================================================================

  /// Create an edge from [sourceId] to [targetId] with the given [type] and
  /// [properties]. Returns the new edge ID.
  int createEdge(
    int sourceId,
    int targetId,
    String type,
    Map<String, dynamic> properties,
  ) {
    _checkOpen();
    final typePtr = toNativeChars(type);
    final propsJson = jsonEncode(
      properties.map((k, v) => MapEntry(k, _encodeValue(v))),
    );
    final propsPtr = toNativeChars(propsJson);
    try {
      final id = _bindings.obrain_create_edge(
        _handle,
        sourceId,
        targetId,
        typePtr,
        propsPtr,
      );
      if (id == -1) throwLastError(_bindings); // C returns u64::MAX on error
      return id;
    } finally {
      malloc.free(typePtr);
      malloc.free(propsPtr);
    }
  }

  /// Get an edge by [id]. Returns an [Edge] or throws on error.
  Edge getEdge(int id) {
    _checkOpen();
    final outPtr = malloc<Pointer<native.ObrainEdge>>();
    try {
      final status = _bindings.obrain_get_edge(_handle, id, outPtr).value;
      if (status != ObrainStatus.ok.code) throwStatus(_bindings, status);
      final edgePtr = outPtr.value;
      try {
        final edgeId = _bindings.obrain_edge_id(edgePtr);
        final sourceId = _bindings.obrain_edge_source_id(edgePtr);
        final targetId = _bindings.obrain_edge_target_id(edgePtr);
        final edgeType =
            charsToDartString(_bindings.obrain_edge_type(edgePtr));
        final propsJson =
            charsToDartString(_bindings.obrain_edge_properties_json(edgePtr));
        return Edge(
          edgeId,
          edgeType,
          sourceId,
          targetId,
          parseObject(propsJson),
        );
      } finally {
        _bindings.obrain_free_edge(edgePtr);
      }
    } finally {
      malloc.free(outPtr);
    }
  }

  /// Delete an edge by [id]. Returns true if the edge was deleted.
  bool deleteEdge(int id) {
    _checkOpen();
    final result = _bindings.obrain_delete_edge(_handle, id);
    if (result < 0) throwLastError(_bindings);
    return result == 1;
  }

  /// Set a property on edge [id].
  void setEdgeProperty(int id, String key, dynamic value) {
    _checkOpen();
    final keyPtr = toNativeChars(key);
    final valuePtr = toNativeChars(encodeValue(value));
    try {
      final status = _bindings
          .obrain_set_edge_property(_handle, id, keyPtr, valuePtr)
          .value;
      if (status != ObrainStatus.ok.code) throwStatus(_bindings, status);
    } finally {
      malloc.free(keyPtr);
      malloc.free(valuePtr);
    }
  }

  /// Remove a property from edge [id].
  void removeEdgeProperty(int id, String key) {
    _checkOpen();
    final keyPtr = toNativeChars(key);
    try {
      final result = _bindings.obrain_remove_edge_property(_handle, id, keyPtr);
      if (result < 0) throwLastError(_bindings);
    } finally {
      malloc.free(keyPtr);
    }
  }

  // ===========================================================================
  // Vector operations
  // ===========================================================================

  /// Drop a vector index. Returns true if the index existed and was removed.
  bool dropVectorIndex(String label, String property) {
    _checkOpen();
    final labelPtr = toNativeChars(label);
    final propertyPtr = toNativeChars(property);
    try {
      final result = _bindings.obrain_drop_vector_index(
        _handle,
        labelPtr,
        propertyPtr,
      );
      if (result < 0) throwLastError(_bindings);
      return result == 1;
    } finally {
      malloc.free(labelPtr);
      malloc.free(propertyPtr);
    }
  }

  /// Rebuild a vector index.
  void rebuildVectorIndex(String label, String property) {
    _checkOpen();
    final labelPtr = toNativeChars(label);
    final propertyPtr = toNativeChars(property);
    try {
      final status = _bindings
          .obrain_rebuild_vector_index(_handle, labelPtr, propertyPtr)
          .value;
      if (status != ObrainStatus.ok.code) throwStatus(_bindings, status);
    } finally {
      malloc.free(labelPtr);
      malloc.free(propertyPtr);
    }
  }

  /// Perform an MMR (Maximal Marginal Relevance) vector search.
  ///
  /// [ef] and [fetchK] use -1 for the library default; [lambda] negative for 0.5.
  List<VectorResult> mmrSearch(
    String label,
    String property,
    List<double> query, {
    required int k,
    required int fetchK,
    required double lambda,
    required int ef,
  }) {
    _checkOpen();
    final labelPtr = toNativeChars(label);
    final propertyPtr = toNativeChars(property);
    final queryPtr = malloc<Float>(query.length);
    for (var i = 0; i < query.length; i++) {
      queryPtr[i] = query[i];
    }
    final outIdsPtr = malloc<Pointer<Uint64>>();
    final outDistsPtr = malloc<Pointer<Float>>();
    final outCountPtr = malloc<Size>();

    try {
      final status = _bindings.obrain_mmr_search(
        _handle,
        labelPtr,
        propertyPtr,
        queryPtr,
        query.length,
        k,
        fetchK,
        lambda,
        ef,
        outIdsPtr,
        outDistsPtr,
        outCountPtr,
      ).value;
      if (status != ObrainStatus.ok.code) throwStatus(_bindings, status);

      final count = outCountPtr.value;
      if (count == 0) return [];

      final ids = outIdsPtr.value;
      final dists = outDistsPtr.value;
      final results = <VectorResult>[
        for (var i = 0; i < count; i++) VectorResult(ids[i], dists[i]),
      ];

      _bindings.obrain_free_vector_results(ids, dists, count);
      return results;
    } finally {
      malloc.free(labelPtr);
      malloc.free(propertyPtr);
      malloc.free(queryPtr);
      malloc.free(outIdsPtr);
      malloc.free(outDistsPtr);
      malloc.free(outCountPtr);
    }
  }

  // ===========================================================================
  // Admin
  // ===========================================================================

  /// Force a WAL checkpoint.
  void walCheckpoint() {
    _checkOpen();
    final status = _bindings.obrain_wal_checkpoint(_handle).value;
    if (status != ObrainStatus.ok.code) throwStatus(_bindings, status);
  }

  // ===========================================================================
  // Internal helpers
  // ===========================================================================

  /// Extract a [QueryResult] from a native result pointer, then free it.
  QueryResult _buildResult(Pointer<native.ObrainResult> resultPtr) {
    try {
      final jsonString = charsToDartString(_bindings.obrain_result_json(resultPtr));
      final executionTimeMs =
          _bindings.obrain_result_execution_time_ms(resultPtr);
      final rowsScanned = _bindings.obrain_result_rows_scanned(resultPtr);

      final rows = parseRows(jsonString);
      final columns = extractColumns(rows);
      final (nodes, edges) = extractEntities(rows);

      return QueryResult(
        columns: columns,
        rows: rows,
        nodes: nodes,
        edges: edges,
        executionTimeMs: executionTimeMs,
        rowsScanned: rowsScanned,
      );
    } finally {
      _bindings.obrain_free_result(resultPtr);
    }
  }

  /// Encode a single value (used by CRUD methods for property values).
  static dynamic _encodeValue(dynamic value) {
    return switch (value) {
      null => null,
      bool b => b,
      int i => i,
      double d => d,
      String s => s,
      DateTime dt => {r'$timestamp_us': dt.toUtc().microsecondsSinceEpoch},
      _ => value.toString(),
    };
  }
}
