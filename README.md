# TsinghuaKit

TsinghuaKit is a Rust campus service library with a Flutter FFI plugin. Rust owns authentication, HTTP transport, session state, parsing, caching, and domain rules. The Flutter plugin builds the Rust crate for Android, iOS, Linux, macOS, and Windows; generated Dart bindings remain in the consuming Flutter application.

## Use from Flutter

Add the public package to `pubspec.yaml`:

```yaml
dependencies:
  tsinghua_kit:
    git:
      url: https://github.com/canxin121/TsinghuaKit.git
      ref: v0.1.1
```

The Rust crate is at `rust/`. Its `Cargo.toml` builds the native library consumed by this repository's Flutter FFI plugin. A consumer should generate its Dart bridge against the crate API using the same `flutter_rust_bridge` version as this package.

## Rust API reference

The [`tsinghua_kit` Rustdoc API reference](https://canxin121.github.io/TsinghuaKit/tsinghua_kit/) is built from the Rust crate and published with GitHub Pages. The [crate guide](rust/README.md) covers the runtime model and local documentation workflow.

## Repository layout

- `rust/`: Rust library, terminal read-only verifier, and Rust contract fixtures.
- `android/`, `ios/`, `linux/`, `macos/`, `windows/`: Flutter FFI plugin platform glue.
- `cargokit/`: Rust build integration used by the Flutter plugin.

The crate declares Rust 1.85 as its minimum supported version. The Flutter bridge runtime and generated code use `flutter_rust_bridge` 2.13.0.
