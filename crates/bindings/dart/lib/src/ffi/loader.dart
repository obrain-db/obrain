/// Platform-specific native library loading for obrain-c.
library;

import 'dart:ffi';
import 'dart:io';

/// Environment variable naming the directory that holds the obrain-c library.
///
/// Set it instead of hard-coding a path: `OBRAIN_C_LIB=/path/to/target/release`.
const String libDirEnvVar = 'OBRAIN_C_LIB';

/// Load the native library, optionally from a specific path.
///
/// When [path] is null, the platform library name is opened from the
/// directory in [libDirEnvVar] if it is set, or else from the system search
/// path (next to the executable, or bundled by Flutter).
DynamicLibrary loadNativeLibrary([String? path]) {
  if (path != null) {
    return DynamicLibrary.open(path);
  }
  return DynamicLibrary.open(_resolveLibraryName());
}

String _resolveLibraryName() {
  final String libraryName;
  if (Platform.isWindows) {
    libraryName = 'obrain_c.dll';
  } else if (Platform.isMacOS) {
    libraryName = 'libobrain_c.dylib';
  } else if (Platform.isLinux || Platform.isAndroid) {
    libraryName = 'libobrain_c.so';
  } else {
    throw UnsupportedError(
      'Unsupported platform: ${Platform.operatingSystem}',
    );
  }

  final dir = Platform.environment[libDirEnvVar];
  if (dir == null || dir.isEmpty) {
    return libraryName;
  }
  return '$dir${Platform.pathSeparator}$libraryName';
}
