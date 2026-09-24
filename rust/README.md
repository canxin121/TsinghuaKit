# TsinghuaKit Rust crate

This directory contains the `tsinghua_kit` Cargo package. The root Flutter FFI plugin invokes Cargokit to build it for each native target. Rust owns network access, authentication, cookies, service handoffs, parsing, caching, and campus domain rules; the Flutter layer receives stable data transfer objects through `flutter_rust_bridge`.

The complete generated API reference is published at [canxin121.github.io/TsinghuaKit](https://canxin121.github.io/TsinghuaKit/tsinghua_kit/). It is regenerated from this crate whenever `main` changes. The [repository overview](../README.md) describes the Flutter package and supported platforms.

## Build and generate the docs locally

From the repository root, build the crate using its checked-in lockfile:

```sh
cargo build --locked --manifest-path rust/Cargo.toml
```

Generate the same rustdoc site used by the Pages workflow:

```sh
cargo doc --locked --no-deps --manifest-path rust/Cargo.toml
```

Open `rust/target/doc/tsinghua_kit/index.html` in a browser, or add `--open` to the command when working in a desktop environment. `--no-deps` keeps the published site focused on TsinghuaKit's API instead of generating documentation for every dependency.

## Library entry point

[`CampusRuntime`](https://canxin121.github.io/TsinghuaKit/tsinghua_kit/api/runtime/struct.CampusRuntime.html) is the opaque application runtime. Construct one with [`create_runtime`](https://canxin121.github.io/TsinghuaKit/tsinghua_kit/api/runtime/fn.create_runtime.html), retain it for the active account session, and call its asynchronous methods to read the services the user opens. Login inputs are passed to the runtime explicitly; authenticated cookies, service tickets, CSRF values, and in-memory session state stay inside Rust.

```rust,no_run
use tsinghua_kit::{create_runtime, CampusRuntime};

fn make_runtime(private_cache_file: String) -> Result<CampusRuntime, String> {
    // `auto` asks Rust to resolve the current semester from live evidence.
    // The cache path should be inside the application's private data directory.
    create_runtime("auto".to_owned(), false, private_cache_file)
}
```

The runtime is designed to live across multiple screen reads. Recreating it for every request discards its in-memory authenticated session and causes avoidable service handoffs. Errors are returned to the caller; a failed or unverified request is not represented as a successful empty result.

## Service areas

- `api::runtime` is the application-facing API and contains `CampusRuntime` and its DTOs.
- `api::service_catalog` describes available services and their coarse capability state without performing requests.
- `identity_session`, `learn_session`, `info_session`, and `registrar_session` perform service handoffs from unified identity authentication.
- `learn_todos`, `learn_announcements`, `info_news`, `library_read`, `classroom_read`, `dorm_electricity_read`, `campus_card_read`, `tunet`, and `usereg` contain service-specific models, request plans, and parsers.
- `transport`, `session`, and `protocol` provide the shared HTTP gate, service-session coordination, and protocol-neutral types.

The THU Info online service hall task list and Web Learning assignments are separate services with separate session paths; see `CampusRuntime::load_thos_pending` and the Learn homework methods in the API reference.

## Credential and request boundaries

Use an application-owned `CampusRuntime` instead of constructing parallel clients for each feature. The runtime owns shared authenticated state and routes HTTP through `CampusHttpTransport`; callers should not copy cookies or service credentials into Flutter DTOs, logs, cache files, or examples. The `terminal-check` Cargo feature exposes a read-only interactive verifier for maintainers and is disabled by default.

This crate implements clients for Tsinghua-operated services. Its live methods require valid institutional access and may require campus network access or interactive second-factor input. Local Rustdoc generation performs no login or network request to campus services.
