# TsinghuaKit Rust workspace

This directory is a Cargo workspace with three packages that have different
consumers:

- `tsinghua_kit` in `crates/tsinghua-kit` is the Flutter-independent Rust SDK.
  Its current unreleased working-tree API is an initial slice for
  `Client::auth().identity()` and `Client::auth().self_service()`, two-slot
  account status, SelfService account/device/usage reads with explicit
  `DeviceRef`-based disconnection, service-hall reads, Registrar and calendar
  reads, library and classroom reads with client-bound references, campus-card
  and electricity reads, and INFO news catalog/list/search/detail/favorites/subscription reads using
  catalog-bound source/channel references, context-bound `ArticleRef`s, and
  opaque `NewsSubscriptionRef`s. Learn reads include courses, announcements,
  student assignments, course files/categories, and discussions; files save
  only through a recent opaque reference and an explicit caller-selected
  destination.
- `tsinghua_kit_engine` in `crates/tsinghua-kit-engine` contains the existing
  Rust-owned transport, sessions, parsers, and service adapters. It is an
  implementation dependency, not the supported external API.
- `tsinghua_kit_ffi` is the Flutter bridge compatibility package. Its Cargo
  package name differs from its native library name: Cargokit and platform
  glue continue to load `libtsinghua_kit` while the Dart facade is migrated.

The SDK owns one Runtime per `Client`. Unified Identity and SelfService are
separate account domains and may use different usernames. TUNet portal access
and Tsinghua Secure/EAP are local network operations; they are not a third
authenticated account. The Rust SDK supports Portal/EAP profile records,
version-bound form preparation, and explicit password filling. Profiles are
memory-only by default. A host may explicitly enable the Unix encrypted-
directory backend by providing both a private storage root and an application
namespace; it stores profile data only and never restores Auth sessions. Its
key and ciphertext share the private directory, so this is not an OS Keychain.
Auth sessions remain memory-only unless a host explicitly selects an
Identity snapshot policy; its directory is independent from the service
cache. `IdentitySessionStoragePolicy::HostSecureStorage`
accepts a separately stored 32-byte key through
`IdentitySessionStorageKey::from_bytes`; `EncryptedDirectory` is the legacy
option that stores its key beside the ciphertext. The snapshot key must
not be reused for network profiles or Auth passwords. `CredentialStoragePolicy`
selects a separate app-private credential directory. A successful Identity
login or completed SelfService captcha flow saves its account-domain record
only after explicit `remember_credentials(true)` opt-in; the two namespaces
remain separate even for identical account names. The vault uses its own
app-private file key and is not an OS Keychain. Either
snapshot opt-in restores the shared Identity-bound Cookie snapshot as
`RestoredUnverified`; the host must call
`identity.revalidate_restored_session()` to perform a read-only validation.
The snapshot is device-bound, expires after its bounded lifetime, and never
stores a password. The SDK writes the whole shared Cookie jar once at the first
successful Identity checkpoint; later business and SelfService responses do
not refresh the on-disk snapshot. Cookies already present at that checkpoint
may still be included, so this is not per-account Cookie partitioning. Schema
1 envelopes are rejected without migration. The legacy co-located-key file
backend is not an OS Keychain. Snapshot policies do not put passwords inside
the cookie snapshot or restore SelfService account state or authorization. The Flutter facade obtains the
Identity key from a separate Flutter Secure Storage entry when
`IdentitySessionPersistence.platformSecureStorage` is selected; this path has
not yet been verified against a real device keychain.
The Flutter facade exposes Auth, local profile CRUD/fill and explicit Portal
connect/disconnect operations, service hall, SelfService, Registrar/Calendar,
INFO, Learn, Library, Classroom, CampusCard and Electricity reads. The
operating-system Wi-Fi/EAP adapter is not implemented. Only an explicitly
configured Identity snapshot can be revalidated across processes; it does not
restore the SelfService account or bypass its captcha. Saved SelfService
passwords can start a new captcha flow, but SelfService session restoration is
not implemented.
Configuring Tsinghua Secure remains an operating-system operation and is not
inferred from a saved profile.

## Build the Rust SDK

From the repository root:

```sh
cargo check --locked --manifest-path rust/Cargo.toml -p tsinghua_kit
RUSTDOCFLAGS='-D warnings' cargo doc --locked --no-deps --manifest-path rust/Cargo.toml -p tsinghua_kit
```

Rustdoc is published from this package at
[`canxin121.github.io/TsinghuaKit`](https://canxin121.github.io/TsinghuaKit/tsinghua_kit/).
The SDK does not depend on Flutter or `flutter_rust_bridge`.

## Read an INFO news page

The detail request takes a reference returned by a list or search result. It
does not accept an arbitrary article ID or URL.

```rust,no_run
use tsinghua_kit::{
    Client,
    news::NewsQuery,
    read::ReadPolicy,
};

async fn read_first_article() -> tsinghua_kit::Result<()> {
    let mut client = Client::builder().build()?;
    let page = client
        .news()
        .articles(NewsQuery::list(1, 20)?, ReadPolicy::PreferFreshCache)
        .await?;

    if let Some(article) = page.data().items().first() {
        let detail = client
            .news()
            .article(article.reference(), ReadPolicy::RefreshOrCached)
            .await?;
        println!("{}", detail.data().title());
    }
    Ok(())
}
```

This example only documents the public call shape. Running it against INFO
requires completing the Identity login flow on the same `Client` first.

## Read Learn course content

Learn resources are selected from the latest course directory returned by the
same `Client`. File and homework selectors are opaque, short-lived references;
they do not expose upstream IDs or URLs. File and discussion reads preserve a
partial-coverage marker when the service reaches its 200-item bound.

```rust,no_run
use std::path::Path;
use tsinghua_kit::{Client, read::ReadCoverage};

async fn read_course_files(destination: &Path) -> tsinghua_kit::Result<()> {
    let mut client = Client::builder().build()?;
    let courses = client.learn().courses().await?;

    if let Some(course) = courses.data().courses().first() {
        let files = client.learn().files(course.reference()).await?;
        if files.metadata().coverage() == ReadCoverage::Complete {
            if let Some(file) = files.data().items().first() {
                let receipt = client
                    .learn()
                    .save_file(file.reference(), destination)
                    .await?;
                println!("saved {} bytes", receipt.bytes_written());
            }
        }
    }
    Ok(())
}
```

`file_categories()` and `discussions()` are also scoped to a `CourseRef` from
that directory. The save operation re-reads the file list before fetching and
will fail if the selected file disappeared or the destination already exists.
Discussion times remain source display labels because the service response
does not establish a timezone. These operations require a proven Identity and
Learn session on the same client.

## Read dorm electricity

Electricity values preserve the service's numeric representation because the
upstream page does not document a unit. Both reads keep live/cache provenance
in `ReadResult`; a cached balance is not presented as a fresh live reading.
The payment-history facade exposes only validated timestamps, numeric amounts,
and status labels, leaving the legacy unlabeled columns inside Rust.

```rust,no_run
use tsinghua_kit::Client;

async fn read_electricity() -> tsinghua_kit::Result<()> {
    let mut client = Client::builder().build()?;
    let mut electricity = client.electricity();
    let remainder = electricity.remainder().await?;
    println!("{}", remainder.data().value());
    println!("{:?}", remainder.metadata().source());

    let history = electricity.payment_history().await?;
    for payment in history.data().records() {
        println!("{} {}", payment.occurred_at(), payment.amount());
    }
    Ok(())
}
```

`remainder()` and `payment_history()` require the same Client's proven Identity
and electricity service session. The current Runtime applies the existing
account-bound fresh-cache/stale-fallback policy; this facade does not claim a
per-call cache policy.

Use `client.news().catalog()` to obtain source and channel choices. A catalog
read reports whether its channel directory is complete; if the directory
route is unavailable, the response may contain candidates from a recent news
page or no candidates. Filter queries take opaque references from that
catalog; they do not accept caller-supplied source or channel IDs. A new
successful catalog read invalidates the previous catalog references for that
client. `subscriptions()` returns account-bound rules, and a
`NewsSubscriptionRef` from that result selects one page at a time. `favorites()`
returns the complete bounded collection or an error when pagination cannot be
proved complete.
For example, a source filter is selected from the returned catalog rather than
constructed from a copied server ID:

```rust,no_run
use tsinghua_kit::{Client, news::NewsQuery};

async fn select_news_source(client: &mut Client) -> tsinghua_kit::Result<()> {
    let catalog = client.news().catalog().await?;
    if let Some(source) = catalog.data().sources().first() {
        let _query = NewsQuery::list(1, 20)?.with_source(source.reference())?;
    }
    Ok(())
}
```

## Build the Flutter compatibility library

```sh
cargo check --locked --manifest-path rust/Cargo.toml -p tsinghua_kit_ffi --lib
cargo build --locked --manifest-path rust/Cargo.toml -p tsinghua_kit_ffi --lib
```

The bridge exposes the legacy `CampusRuntime` for compatibility and a
generated `ClientHandle` facade over the public SDK. This is a staged bridge:
the THYou app still uses the legacy Runtime and its generated DTOs. Keep
business behavior and transport in Rust during the app migration; do not treat
the compatibility DTOs as the new Rust API contract.

The public SDK and engine share this workspace during development. The SDK
facade now owns its public client type and wraps engine service views, so its
API surface can evolve without re-exporting `CampusRuntime`. The FFI package
still uses the compatibility runtime directly. A `cargo package` check for
the SDK currently stops because `tsinghua_kit_engine` has not been published
to crates.io. Before a registry release, either publish the engine package
first or fold the private engine modules into the SDK package. GitHub
repository consumption and a crates.io release are separate delivery paths;
a clean external Git dependency check remains part of the release work.

## Account and network boundaries

The SDK has two Auth slots: unified Identity and network self-service
(USEREG). SelfService credentials and business data remain bound to that
second account even when Identity/WebVPN provides the required access path.
Campus network login information is local profile data for an explicit
operation. Portal credentials and system-managed EAP credentials have
separate purposes; saving either must not create an Auth session, and an
unknown system Wi-Fi result must not be presented as a successful connection.
`network().observe_portal_status()` reports only whether the TUNet portal
registered the current local IPv4 address. It does not prove general Internet
reachability or the status of system-managed EAP Wi-Fi such as Tsinghua Secure.

Network profiles are memory-only by default. A host may explicitly opt into
`NetworkProfileStoragePolicy::EncryptedDirectory` with a private application
directory and a stable host namespace. On Unix targets the SDK encrypts profile
records, atomically replaces the data file, and enforces owner-only directory
and file modes. The key is stored beside the ciphertext, so this backend is
not an OS keychain and does not protect against the same OS user or an
administrator. A host with its own secure key store can instead use
`NetworkProfileStoragePolicy::keychain_encrypted_directory(root, namespace, key)`;
the 32-byte key is zeroized by Rust and never written beside the ciphertext.
The Flutter facade obtains that key from Flutter Secure Storage. Both
persistent file modes are currently supported on Unix targets; non-Unix
targets return `Unsupported`. Profile persistence restores form data only and
never restores an Auth session or network-online proof. The selected store is
locked to one Client at a time. Switching an existing colocated-key store to
the secure-key policy migrates the encrypted records under that lock and
removes the old on-disk key after the new ciphertext has been committed.

Building Rustdoc or compiling these packages does not log in or contact
campus services.
