/// The library's study-room application (`研读间`).
///
/// Both reads are live and account-bound to the INFO/WebVPN session the client
/// already holds, and both are read-only: the application's own campus login is
/// deliberately not implemented, so an expired session is reported as an
/// authentication failure rather than answered with an empty catalogue.
library;

export 'tsinghua_kit.dart'
    show
        LibraryRoom,
        LibraryRoomCatalog,
        LibraryRoomClient,
        LibraryRoomKind,
        LibraryRoomMember,
        LibraryRoomRecord,
        ReadResult,
        TsinghuaKitClient;
