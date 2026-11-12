import 'dart:io';

import 'package:test/test.dart';

// Freshness guard: every obrain-c function declared in obrain.h must be bound
// in lib/src/ffi/bindings.dart. The byte-exact check is the CI job, which
// reruns `dart run ffigen --config ffigen.yaml` and fails on any diff.

void main() {
  test('bindings.dart binds every obrain-c function declared in obrain.h', () {
    final header = File('../c/obrain.h').readAsStringSync();
    final bindings = File('lib/src/ffi/bindings.dart').readAsStringSync();

    // Declarations start with a return type on a non-comment line; comments
    // and #if guards are skipped by the leading-identifier anchor.
    final declared = RegExp(
      r'^[A-Za-z_][^/\n]*?\b(obrain_\w+)\(',
      multiLine: true,
    ).allMatches(header).map((m) => m.group(1)!).toSet();

    expect(declared, isNotEmpty, reason: 'no declarations found in obrain.h');

    final missing = declared.where((n) => !bindings.contains("'$n'")).toList()
      ..sort();
    expect(
      missing,
      isEmpty,
      reason: 'missing from bindings.dart; regenerate with '
          '`dart run ffigen --config ffigen.yaml`',
    );
  });
}
