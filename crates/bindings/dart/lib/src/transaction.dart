/// ACID transaction support for Obrain.
///
/// Uses [NativeFinalizer] to auto-rollback (via Rust Drop) if neither
/// [commit] nor [rollback] is called.
library;

import 'dart:ffi';

import 'package:ffi/ffi.dart';

import 'error.dart';
import 'ffi/bindings.dart' as native;
import 'ffi/strings.dart';
import 'types.dart';
import 'value.dart';

/// An ACID transaction on a Obrain database.
///
/// Obtain via [ObrainDB.beginTransaction]. Must be explicitly committed
/// or rolled back. If dropped without either, the Rust Drop impl
/// performs an automatic rollback.
class Transaction implements Finalizable {
  final native.ObrainBindings _bindings;
  final DynamicLibrary _lib;
  Pointer<native.ObrainTransaction> _handle;
  bool _finished = false;

  static NativeFinalizer? _finalizer;

  /// Create a transaction wrapper around a native handle.
  ///
  /// Typically called by [ObrainDB.beginTransaction], not directly.
  Transaction(this._handle, this._bindings, this._lib) {
    _finalizer ??= NativeFinalizer(
      _lib.lookup<NativeFunction<Void Function(Pointer<Void>)>>(
        'obrain_free_transaction',
      ),
    );
    _finalizer!.attach(this, _handle.cast(), detach: this);
  }

  void _checkActive() {
    if (_finished) {
      throw TransactionException(
        'Transaction already finished',
        ObrainStatus.transaction,
      );
    }
  }

  /// Execute a GQL query within this transaction.
  QueryResult execute(String query) {
    _checkActive();
    final queryPtr = toNativeChars(query);
    try {
      final resultPtr = _bindings.obrain_transaction_execute(_handle, queryPtr);
      if (resultPtr == nullptr) throwLastError(_bindings);
      return _buildResult(resultPtr);
    } finally {
      malloc.free(queryPtr);
    }
  }

  /// Execute a parameterized GQL query within this transaction.
  QueryResult executeWithParams(
    String query,
    Map<String, dynamic> params,
  ) {
    _checkActive();
    final queryPtr = toNativeChars(query);
    final paramsPtr = toNativeChars(encodeParams(params));
    try {
      final resultPtr = _bindings.obrain_transaction_execute_with_params(
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

  /// Commit the transaction, making all changes permanent.
  void commit() {
    _checkActive();
    _finished = true;
    _finalizer!.detach(this);
    final status = _bindings.obrain_commit(_handle).value;
    _bindings.obrain_free_transaction(_handle);
    _handle = nullptr;
    if (status != ObrainStatus.ok.code) {
      throw classifyError(status, lastError(_bindings));
    }
  }

  /// Rollback the transaction, discarding all changes.
  void rollback() {
    _checkActive();
    _finished = true;
    _finalizer!.detach(this);
    final status = _bindings.obrain_rollback(_handle).value;
    _bindings.obrain_free_transaction(_handle);
    _handle = nullptr;
    if (status != ObrainStatus.ok.code) {
      throw classifyError(status, lastError(_bindings));
    }
  }

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
}
