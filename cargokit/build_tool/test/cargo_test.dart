import 'package:build_tool/src/cargo.dart';
import 'package:test/test.dart';

void main() {
  test('Cargo package and native library names can differ', () {
    final info = CrateInfo.parseManifest('''
[package]
name = "tsinghua_kit_ffi"

[lib]
name = "tsinghua_kit"
''');

    expect(info.packageName, 'tsinghua_kit_ffi');
    expect(info.libraryName, 'tsinghua_kit');
  });

  test('native library name defaults to the package name', () {
    final info = CrateInfo.parseManifest('''
[package]
name = "tsinghua_kit"
''');

    expect(info.packageName, 'tsinghua_kit');
    expect(info.libraryName, 'tsinghua_kit');
  });
}
