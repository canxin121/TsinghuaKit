part of '../tsinghua_kit.dart';

/// One reservable study room in the library's study-room application
/// (`研读间`).
class LibraryRoom {
  const LibraryRoom({
    required this.deviceId,
    required this.name,
    required this.minReserveMinutes,
  });

  /// The service's own room identifier.
  ///
  /// It is printed on the room's page, so it is not a secret; it is carried as
  /// a plain number because it is the only handle a caller has for a room.
  final BigInt deviceId;

  /// The room name the service prints.
  final String name;

  /// The shortest reservation the service accepts for this room, in minutes.
  final BigInt minReserveMinutes;
}

/// One group of reservable rooms the service files under a shared kind.
class LibraryRoomKind {
  LibraryRoomKind({
    required this.kindId,
    required this.kindName,
    required List<LibraryRoom> rooms,
  }) : rooms = List.unmodifiable(rooms);

  final BigInt kindId;
  final String kindName;
  final List<LibraryRoom> rooms;
}

/// The reservable study-room catalogue.
class LibraryRoomCatalog {
  LibraryRoomCatalog({
    required List<LibraryRoomKind> kinds,
    required this.roomCount,
  }) : kinds = List.unmodifiable(kinds);

  final List<LibraryRoomKind> kinds;

  /// The number of reservable rooms across every kind.
  final int roomCount;

  bool get isEmpty => roomCount == 0;
}

/// One participant of a reservation, by printed name only.
///
/// The service also sends each participant's campus account name. It is
/// deliberately not exposed here.
class LibraryRoomMember {
  const LibraryRoomMember({required this.name});

  final String name;
}

/// One reservation held by this account.
///
/// [date], [beginTime] and [endTime] are carried exactly as the service wrote
/// them. The service supplies no zone, so none is converted here.
///
/// The service's own cancellation handle is deliberately not exposed: no
/// cancellation is reachable through this client.
class LibraryRoomRecord {
  LibraryRoomRecord({
    required this.name,
    required this.deviceName,
    required this.kindName,
    required this.date,
    required this.beginTime,
    required this.endTime,
    required List<LibraryRoomMember> members,
  }) : members = List.unmodifiable(members);

  /// The name the reservation is held under.
  final String name;

  final String deviceName;
  final String kindName;
  final String date;
  final String beginTime;
  final String endTime;
  final List<LibraryRoomMember> members;
}

/// The library's study-room application (`研读间`).
///
/// Both reads are live and account-bound to the INFO/WebVPN session this client
/// already holds, and neither is served from cache: a room can be withdrawn or
/// reopened between requests, so a retained catalogue would present an unusable
/// room as reservable.
///
/// This API is read-only by construction. The application's own campus login is
/// deliberately not implemented — the reference derives the application id it
/// would submit from a response, and this client performs no second campus login
/// — so an expired session is reported as an authentication failure rather than
/// answered with an empty catalogue.
class LibraryRoomClient {
  LibraryRoomClient._(this._handle);

  final native.ClientHandle _handle;

  /// Reads the reservable study-room catalogue.
  ///
  /// An empty catalogue is only ever the service's own answer that nothing is
  /// currently reservable; a response that did not carry the service's envelope
  /// is an error, never an empty catalogue.
  Future<ReadResult<LibraryRoomCatalog>> catalog() => _sdkCall(
        () async => _libraryRoomCatalog(
          await _handle.libraryRoomCatalogResult(),
        ),
      );

  /// Reads this account's own reservations for one date window.
  ///
  /// [begin] and [end] are `YYYY-MM-DD`. Both are validated inside Rust before
  /// any request, so a reversed, malformed, or over-wide window is refused as
  /// invalid input rather than becoming a service query.
  Future<ReadResult<List<LibraryRoomRecord>>> records({
    required String begin,
    required String end,
  }) =>
      _sdkCall(
        () async => _libraryRoomRecords(
          await _handle.libraryRoomRecordsResult(begin: begin, end: end),
        ),
      );

  /// The widest reservation window this client will read, in days.
  static const int maxWindowDays = 31;
}

ReadResult<LibraryRoomCatalog> _libraryRoomCatalog(
  native.LibraryRoomCatalogResultDto value,
) =>
    ReadResult(
      data: LibraryRoomCatalog(
        roomCount: value.data.roomCount,
        kinds: value.data.kinds
            .map(
              (kind) => LibraryRoomKind(
                kindId: _platformInt64ToBigInt(kind.kindId),
                kindName: kind.kindName,
                rooms: kind.rooms
                    .map(
                      (room) => LibraryRoom(
                        deviceId: _platformInt64ToBigInt(room.deviceId),
                        name: room.name,
                        minReserveMinutes:
                            _platformInt64ToBigInt(room.minReserveMinutes),
                      ),
                    )
                    .toList(growable: false),
              ),
            )
            .toList(growable: false),
      ),
      metadata: _readMetadata(value.metadata),
    );

ReadResult<List<LibraryRoomRecord>> _libraryRoomRecords(
  native.LibraryRoomRecordsResultDto value,
) =>
    ReadResult(
      data: value.data.records
          .map(
            (record) => LibraryRoomRecord(
              name: record.name,
              deviceName: record.deviceName,
              kindName: record.kindName,
              date: record.date,
              beginTime: record.beginTime,
              endTime: record.endTime,
              members: record.members
                  .map((member) => LibraryRoomMember(name: member.name))
                  .toList(growable: false),
            ),
          )
          .toList(growable: false),
      metadata: _readMetadata(value.metadata),
    );
