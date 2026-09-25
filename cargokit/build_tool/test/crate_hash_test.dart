import 'dart:io';

import 'package:build_tool/src/crate_hash.dart';
import 'package:test/test.dart';

void main() {
  test('workspace member sources affect the native artifact key', () {
    final root = Directory.systemTemp.createTempSync('cargokit-workspace-');
    addTearDown(() => root.deleteSync(recursive: true));
    final rootPath = root.path;
    final ffi = Directory('$rootPath/src')..createSync(recursive: true);
    final sdk = Directory('$rootPath/crates/sdk/src')
      ..createSync(recursive: true);

    File('$rootPath/Cargo.toml').writeAsStringSync('''
[package]
name = "ffi"

[workspace]
members = ["crates/sdk"]
''');
    File('$rootPath/Cargo.lock').writeAsStringSync('version = 4\n');
    File('${ffi.path}/lib.rs').writeAsStringSync('// bridge\n');
    File('$rootPath/crates/sdk/Cargo.toml').writeAsStringSync('''
[package]
name = "sdk"
''');
    final sdkSource = File('${sdk.path}/lib.rs')
      ..writeAsStringSync('// first SDK version\n');

    final before = CrateHash.compute(rootPath);
    sdkSource.writeAsStringSync('// second SDK version\n');
    final after = CrateHash.compute(rootPath);

    expect(after, isNot(before));
  });
}
