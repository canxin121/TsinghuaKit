# TsinghuaKit

TsinghuaKit is split into a Flutter-independent Rust SDK, its internal Rust engine, and a Flutter FFI compatibility package. Rust owns authentication, HTTP transport, session state, parsing, caching, and domain rules. The public Rust SDK is `rust/crates/tsinghua-kit`; the engine is `rust/crates/tsinghua-kit-engine`; the existing bridge implementation remains under `rust/` while its Dart facade is migrated.

## Use from Flutter

The next Flutter API is an unreleased `0.2.0-alpha.1` facade in this working
tree. The existing `v0.1.1` tag remains the older compatibility API and does
not contain the new methods. During local development, consume the checkout by
path:

```yaml
dependencies:
  tsinghua_kit:
    path: ../TsinghuaKit
```

`tsinghua_kit.dart` is the convenience umbrella. Applications can instead
import `core.dart` for initialization, client creation, and common errors, then
add focused entry points such as `auth.dart`, `network.dart`,
`service_hall.dart`, `self_service.dart`, `registrar_calendar.dart`,
`news.dart`, or `learn.dart` for a feature's types. All domain clients still
come from the same `TsinghuaKitClient`; importing a domain library does not
create another Rust runtime.

The new facade exposes the two separate Auth domains, local campus-network
profile management, the online service hall, SelfService account data,
Registrar/Calendar, library and classroom reads, campus-card and electricity
reads, Learn, and INFO through one Rust Client:

The returned `TsinghuaKitClient` owns the native Client. Call `client.dispose()`
when its application scope ends to release the Rust session and any profile
store lock. Disposal is idempotent; its domain facades must not be used after
the Client is disposed.

`client.auth.status()` returns separate Identity and SelfService states and
their optional account names. The names are presentation data rather than
session proof; Rust `Debug` output redacts them.

```dart
import 'package:tsinghua_kit/tsinghua_kit.dart';

final client = await TsinghuaKit.createClient();
final auth = await client.auth.status();
final profiles = await client.network.profiles.list();
final pending = await client.serviceHall.pending(
  policy: ServiceHallReadPolicy.preferFreshCache,
);
final phases = await client.serviceHall.tasks(
  view: ServiceHallTaskView.phases,
  policy: ServiceHallReadPolicy.preferFreshCache,
);
final schedule = await client.registrar.semesterSchedule();
final terms = await client.calendar.learnTerms();
final schoolCalendar = await client.calendar.schoolCalendarImage(
  semester: SchoolCalendarSemester.autumn,
  language: SchoolCalendarLanguage.chinese,
);
final news = await client.news.articles(
  page: 1,
  policy: ReadPolicy.preferFreshCache,
);
// After an explicit Identity login:
final courses = await client.learn.courses();
if (courses.data.courses.isNotEmpty) {
  final course = courses.data.courses.first;
  final assignments = await client.learn.homework(course.reference);
}
if (phases.data.tasks.isNotEmpty) {
  final reference = phases.data.tasks.first.phaseReference;
  if (reference != null) {
    final details = await client.serviceHall.phaseDetails(
      reference: reference,
      policy: ServiceHallReadPolicy.preferFreshCache,
    );
  }
}
```

Service read caches are memory-only by default. The host may explicitly
choose a private cache directory; this is independent from Identity's
encrypted session snapshot and local network profiles. The cache directory
contains business read data only and never creates an authenticated session.

Flutter can separately opt in to the current Identity cookie snapshot backend,
with its key held by the platform secure store:

```dart
final client = await TsinghuaKit.createClient(
  cache: ClientCachePersistence.directory(
    root: appPrivateCacheDirectory,
  ),
  identitySession: IdentitySessionPersistence.platformSecureStorage(
    // Replace this host-provided value with an absolute app-private directory.
    root: appPrivateDataDirectory,
    namespace: 'org.example.campus-app',
  ),
);
```

That snapshot is device-bound and encrypted. Flutter Secure Storage holds a
separate 32-byte Identity-session key; it does not reuse the NetworkProfile
key. The snapshot never includes a password. Auth passwords use a separate
opt-in policy:

```dart
final client = await TsinghuaKit.createClient(
  authCredentials: AuthCredentialPersistence.encryptedDirectory(
    root: appPrivateDataDirectory,
    namespace: 'org.example.campus-app',
  ),
  identitySession: IdentitySessionPersistence.platformSecureStorage(
    root: appPrivateDataDirectory,
    namespace: 'org.example.campus-app',
  ),
);

await client.auth.identity.login(
  username: identityUsername,
  password: identityPassword,
  rememberCredentials: true,
);
```

Identity and SelfService credentials are encrypted in distinct internal
namespaces, even if both accounts use the same username. Identity credentials
are saved only after a successful login; cross-process recovery also requires
an Identity session snapshot. SelfService credentials are saved only after a
successful captcha login. `startSavedLogin(username: ...)` opens a new captcha
flow and never bypasses captcha entry. Setting either login's option to false
removes a previously saved password for that account. The credential vault
uses an app-private file key stored beside its ciphertext; it is not backed by
Flutter Secure Storage or an OS Keychain.

The SDK stores the
shared Cookie jar once at the first successful Identity checkpoint; later
business and SelfService responses do not refresh it. Cookies already present
at that checkpoint may be included, so this is not per-account Cookie
partitioning. Schema 1 snapshots are rejected without migration. Restored
Identity appears as `restoredUnverified`; call
`client.auth.identity.revalidateRestoredSession()` only when the app explicitly
wants Rust to perform the read-only check. SelfService status and network
online proof are not restored. The legacy
`IdentitySessionPersistence.encryptedDirectory` option keeps its key beside
the ciphertext and is for migration only. Identity snapshots do not restore
passwords or the second Auth slot. A saved SelfService password only starts a
new explicit captcha flow.

It also supports explicit captcha/code steps, profile editing, version-bound
form preparation, and user-requested password filling. Identity and
SelfService remain the only Auth accounts. Portal and system Wi-Fi/EAP data
are local profiles; creating or filling one does not connect the device or
restore either Auth session. Profile persistence is memory-only by default;
the legacy Unix encrypted-directory backend stores its key beside the data.
For persistent profiles, use
`NetworkProfilePersistence.platformSecureStorage(root: ..., namespace: ...)`:
Flutter Secure Storage keeps the encryption key in the platform secure store,
while Rust keeps the encrypted profile file under the selected private app
directory. This key protects Portal/EAP profile data only; it is never an Auth
password or session key. Identity-session persistence uses its own secure
storage entry and does not reuse profile keys or passwords.

Logout scope is explicit: `client.auth.identity.logout()` closes Identity and
its derived service proofs; a selected SelfService account remains visible as
expired if its shared Identity/WebVPN access route has closed.
`client.auth.selfService.logout()` clears only SelfService state and its
pending device targets, while `client.auth.logoutAll()` closes both account
domains. All three leave local network profiles and the current Wi-Fi
connection alone.

All successful service-hall, SelfService, Registrar, Calendar, Library,
Classroom, CampusCard, Electricity, Learn, and News reads carry source and
freshness metadata; bounded collections also report completeness.
Pending tasks, the service directory, completed items, drafts, copies, phase
workflows, and phase details are bridged; phase-detail references stay opaque
and Client-bound. Registrar exposes the full semester schedule, grades, and
exams. Calendar exposes Learn term dates and verified published calendar
images. INFO catalog/list/search/detail/favorites/subscription reads are now
bridged through opaque Client-bound references. Learn courses, announcements,
assignments, assignment details, files, categories, discussions, and explicit
file saving are now bridged through the same Client. Library locations, hours,
seats and sockets; classroom buildings and weekly availability; campus-card
account and bounded transactions; and electricity remainder and payment
history now have Flutter facades on the same Client. Local network observation
is bridged, while Portal connection execution and OS Wi-Fi/EAP configuration
remain unsupported. Only the opt-in Identity snapshot described above is
available for Auth restoration; it is not a complete release-ready two-account
solution. The THYou App remains pinned to `v0.1.1`; it is not migrated until
its required service calls use this same Client lifecycle and the new package
has a reproducible public revision.

The standalone Rust SDK `tsinghua_kit` is at `rust/crates/tsinghua-kit`. Its
unreleased working-tree API includes service-hall pending tasks, catalogue,
four task views, and Client-bound phase details; SelfService
account/device/usage reads with explicit `DeviceRef`-selected disconnection;
Registrar schedule/grade/exam reports; Learn term dates and published school
calendar images; Learn course/announcement/assignment/file/discussion reads
and explicitly selected file saving; and INFO news
catalog/list/search/detail/favorites/subscription reads selected with opaque
references, and Library, Classroom, CampusCard, and Electricity reads. See
the Rustdoc reference for the current Rust surface and the migration matrix
for the distinction between package facades and the still-unmigrated THYou
application.

The [Flutter API migration matrix](docs/flutter-api-migration-matrix.md) maps
the App's old gateway methods to their new Rust ownership and bridge status.

## Rust API reference

The [`tsinghua_kit` Rustdoc API reference](https://canxin121.github.io/TsinghuaKit/tsinghua_kit/) is built from the public SDK crate and published with GitHub Pages. The [Rust workspace guide](rust/README.md) covers the current FFI compatibility package and documentation workflow.

## Repository layout

- `rust/crates/tsinghua-kit/`: Flutter-independent Rust SDK crate.
- `rust/`: existing Flutter FFI compatibility package, terminal read-only verifier, and protocol contract fixtures.
- `android/`, `ios/`, `linux/`, `macos/`, `windows/`: Flutter FFI plugin platform glue.
- `cargokit/`: Rust build integration used by the Flutter plugin.

The crate declares Rust 1.85 as its minimum supported version. The Flutter bridge runtime and generated code use `flutter_rust_bridge` 2.13.0.
