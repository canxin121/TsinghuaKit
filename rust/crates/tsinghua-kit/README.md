# TsinghuaKit Rust SDK

`tsinghua_kit` is the Flutter-independent Rust SDK for Tsinghua campus
services. It owns one Rust runtime, keeps Identity and SelfService as separate
account domains, and exposes verified service results through a typed API.
The current unreleased working tree provides Identity login and explicit
second-factor steps, a separate SelfService captcha login, typed account and
service reads, service-hall workflow views, INFO news, Learn courses and
content, Registrar reports, calendars, library availability, classroom
availability, campus-card reads, and electricity reads. Context-sensitive
choices use opaque references tied to the current `Client`. Campus-card's
target-specific password is a one-shot service interaction, not a third
account.

`Client::network()` also manages local Portal/EAP fill profiles and reads
TUNet's registration status for the current IPv4 address. It can explicitly
connect or disconnect a Portal profile through the shared Rust transport.
Connections are local network operations, not Auth sessions; EAP profiles are
rejected by Portal calls and operating-system Wi-Fi/EAP configuration remains
outside the SDK. THOS write operations and Identity/SelfService credential
restoration also remain outside the current public API. The Flutter facade can use Flutter Secure
Storage to hold the encryption key for persistent local network profiles;
Rust receives that key only while constructing the Client and never writes it
beside the encrypted profile file.

The SDK does not depend on Flutter or `flutter_rust_bridge`. It does depend on
the internal `tsinghua_kit_engine` crate, which contains Rust-owned transport,
authentication, session coordination, parsers, and service adapters. External
applications should depend on this crate rather than the engine or the legacy
FFI package.

## Quick start

```rust,no_run
use tsinghua_kit::{
    Client,
    auth::{IdentityLoginOutcome, IdentityLoginRequest},
    service_hall::ServiceHallReadPolicy,
};

async fn read_pending_tasks() -> tsinghua_kit::Result<()> {
    let mut client = Client::builder().build()?;

    // A host UI collects these values. The password is not persisted.
    let login = IdentityLoginRequest::new("account", "password");
    match client.auth().identity().login(login).await? {
        IdentityLoginOutcome::Authenticated(_) => {}
        IdentityLoginOutcome::NeedsInteraction { .. } => {
            // Ask the user to perform an explicit second-factor step.
            return Ok(());
        }
    }

    let pending = client
        .service_hall()
        .pending(ServiceHallReadPolicy::PreferFreshCache)
        .await?;
    println!("{} tasks", pending.data().items().len());

    let courses = client.learn().courses().await?;
    if let Some(course) = courses.data().courses().first() {
        let assignments = client.learn().homework(course.reference()).await?;
        println!("{} assignments", assignments.data().items().len());
    }
    Ok(())
}
```

`Client::builder().build()` performs no network request and does not restore
credentials or cookies. Both Auth accounts stay in memory until the client is
dropped by default.
The default cache policy creates a private temporary directory and removes it
on drop; a host can select a private persistent directory for business read
caches. Flutter and Rust expose that as `ClientCachePersistence.directory` and
`ClientCachePolicy::Directory` respectively. The service-cache directory,
Identity snapshot directory, and NetworkProfile directory are configured
independently. A cache never creates an authenticated session.

Identity may opt into an encrypted shared-cookie snapshot, but the current
directory backend stores its key beside the ciphertext and the snapshot is not
a strict Identity-only cookie partition or an OS Keychain. Restored data starts
unverified and must be checked explicitly. SelfService session restoration is
not implemented. Do not treat the directory backend as high-assurance
credential storage.

Identity and SelfService may use different usernames. SelfService captcha
login requires a currently proven Identity/WebVPN access path, but its
credentials and verified business data stay in the SelfService account
domain. `client.self_service()` reads the account summary, complete online
device table, and usage/balance table only after a proven SelfService session.
Disconnecting an online device requires the opaque `DeviceRef` returned by
that client's latest complete list. A TUNet or Tsinghua Secure profile is a
local network input, not a third Auth account. Profiles default to memory-only;
the opt-in encrypted-directory backend is distinct from Auth persistence and
is not an OS credential vault. Tsinghua Secure's live connection and 802.1X
configuration remain owned by the operating system.

`client.auth().status()` reports the selected username independently for each
Auth domain when known. The username is presentation data, not evidence of a
valid session, and the Rust `Debug` representation redacts it.

Use `NetworkAccessMethod::Portal` for credentials entered into the TUNet portal
and `NetworkAccessMethod::SystemWifiEap` for local form-fill data intended for
an operating-system-managed Wi-Fi profile such as Tsinghua Secure. These
profiles are independent of both Auth usernames and remain available when
either account signs out. Saving or preparing a profile performs no login and
does not change system Wi-Fi settings. `observe_portal_status()` reports only
whether the current local IPv4 address is registered at TUNet; `NotRegistered`
does not mean the device is offline, since the system Wi-Fi connection may
already provide network access.

`client.learn().courses()` returns source and freshness metadata alongside
the course catalog. Each `CourseRef` is bound to that client's latest course
read. `announcements()` returns active and expired course announcements with
the same cache-source metadata. `homework()` reads a complete live assignment
list for the selected course; each detail-capable item provides a short-lived
`HomeworkRef` for `homework_detail()`. Re-reading courses, assignments, or
changing the Identity session invalidates older Learn references. Assignment
detail text and announcement content are available through explicit getters
and are omitted from `Debug` output.

`client.registrar()` exposes the verified full-semester schedule, grade report,
and complete exam report. The Runtime chooses the academic stage and source;
cache results retain their provenance and freshness in `ReadResult`. Exam
entries preserve the source schedule text when the undergraduate source does
not provide a year. `client.calendar()` reads Learn's current/following term
timeline and the published school-calendar JPEG. School-calendar year,
semester, and language use typed selections; Rust fetches and validates the
image through the shared transport without exposing a service URL.

`client.library()` returns the root location directory with source and
freshness metadata. Library, floor, section, seat-window, and seat references
are opaque and bound to the Client and the current selection generation. The
Runtime rechecks each path before it sends a request. Floors, sections, seat
availability, and socket states are live reads; directory and opening-window
reads keep their account-bound Runtime cache behavior. A stale cache is only
returned when that Runtime's live refresh fails, and the result exposes the
stale source and refresh failure. Today/tomorrow are resolved in the campus
time zone and held to the selected date across authentication handoffs. Socket
state is read from its separate service and joined in Rust to the exact seat
availability result; a missing socket record is `Unknown`.

`client.classrooms()` reads a classroom building directory and one selected
building's weekly matrix. `BuildingRef` is opaque and bound to the Client's
latest directory; refreshing that directory invalidates its old references.
The week selector is a validated `ClassroomWeek`, or the default week supplied
with a building. Building-directory cache results preserve their source and
freshness metadata; weekly matrices are live reads. Rust validates the week
list, seven Monday-first dates, unique room names, and exactly 42 slot statuses
before returning a result. Unknown source statuses remain explicit as
`ClassroomSlotStatus::Unknown` rather than being treated as free.

`client.campus_card()` reads the account and a complete transaction range of
at most 31 days. Results retain live/cache source, freshness, and observation
time. Transaction identifiers remain internal and are used only for duplicate
page detection. If the first card SSO needs its target password, `account()`
returns `InteractionRequired` and `pending_interaction()` reports
`PasswordRequired`; submit the one-shot `CampusCardPasswordRequest` through the
same Client. The Runtime validates the current Identity owner and short-lived
challenge, zeroizes the entered password, and consumes that challenge before
I/O. A card second-factor challenge goes through
`client.auth().identity().interaction()` and the existing explicit code flow.
No card password or service session is persisted by the SDK.

Errors expose stable service and error codes only. They do not retain response
bodies, URLs, cookies, tickets, CSRF values, or passwords. Login, captcha, and
second-factor submissions remain explicit and are never automatically
replayed after an uncertain result.

`client.service_hall()` also provides `services()`, typed completed/draft/copy/
phase task views, and `phase_details()`. These reads share one
`ServiceHallReadPolicy` and preserve source plus page completeness in
`ReadResult`. A phase detail request must use the opaque
`WorkflowTask::phase_reference()` from this Client's latest complete phase
list. Refreshing that list, changing Identity, logging out, or using another
Client invalidates the old selection. Protocol IDs, selectors, and workflow
URLs remain inside Rust. The older `PendingReadPolicy` name remains an alias
for the shared service-hall policy.
