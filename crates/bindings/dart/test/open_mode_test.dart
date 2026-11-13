import 'dart:io';

import 'package:obrain/obrain.dart';
import 'package:test/test.dart';

// Decision D4: open is read-only; openWrite takes the write lease (bail).

void main() {
  late Directory root;
  late String storePath;

  setUp(() {
    root = Directory.systemTemp.createTempSync('obrain-d4-');
    storePath = '${root.path}${Platform.pathSeparator}store';
  });

  tearDown(() {
    if (root.existsSync()) root.deleteSync(recursive: true);
  });

  test('D4 openWrite creates a store and open reads it back', () {
    final w = ObrainDB.openWrite(storePath);
    w.execute("INSERT (:Person {name: 'Alix'})");
    w.close();

    final r = ObrainDB.open(storePath);
    addTearDown(r.close);
    expect(r.nodeCount, 1);
  });

  test('D4 read-only open rejects a mutation', () {
    final w = ObrainDB.openWrite(storePath);
    w.close();

    final r = ObrainDB.open(storePath);
    addTearDown(r.close);
    expect(
      () => r.execute("INSERT (:Person {name: 'Bob'})"),
      throwsA(isA<ObrainException>()),
    );
  });

  test('D4 open of a missing store throws and creates nothing', () {
    expect(() => ObrainDB.open(storePath), throwsA(isA<ObrainException>()));
    expect(Directory(storePath).existsSync(), isFalse);
  });

  test('D4 second openWrite on the same store throws while the lease is held',
      () {
    final first = ObrainDB.openWrite(storePath);
    addTearDown(first.close);
    expect(
      () => ObrainDB.openWrite(storePath),
      throwsA(isA<ObrainException>()),
    );
  });
}
