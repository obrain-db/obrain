/// Conversions between Dart strings and the `char *` pointers of obrain-c.
///
/// ffigen types every C `char *` as `Pointer<Char>`; the `package:ffi` helpers
/// work on `Pointer<Utf8>`. These two functions bridge the two.
library;

import 'dart:ffi';

import 'package:ffi/ffi.dart';

/// Allocates a NUL-terminated UTF-8 copy of [s] as a `char *`.
///
/// The caller owns the memory and must release it with `malloc.free`.
Pointer<Char> toNativeChars(String s, {Allocator allocator = malloc}) =>
    s.toNativeUtf8(allocator: allocator).cast<Char>();

/// Copies a NUL-terminated `char *` owned by obrain-c into a Dart string.
///
/// Does not free [p]. Call only on non-null pointers.
String charsToDartString(Pointer<Char> p) => p.cast<Utf8>().toDartString();
