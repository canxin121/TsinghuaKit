part of '../tsinghua_kit.dart';

/// The two date choices supported by the library service.
enum LibraryDay { today, tomorrow }

/// Opaque location selector returned by this Client's latest library directory.
class LibraryReference {
  const LibraryReference._(this._id);

  final String _id;
}

/// One root library location.
class LibraryPlace {
  const LibraryPlace({
    required this.name,
    this.reference,
    this.englishName,
    this.isValid,
    this.totalSeats,
    this.availableSeats,
  });

  final LibraryReference? reference;
  final String name;
  final String? englishName;
  final bool? isValid;
  final BigInt? totalSeats;
  final BigInt? availableSeats;
}

/// Root locations and their source metadata.
class LibraryDirectory {
  LibraryDirectory({required List<LibraryPlace> libraries})
      : libraries = List.unmodifiable(libraries);

  final List<LibraryPlace> libraries;
}

/// Opaque floor selector from the latest read for its selected library.
class LibraryFloorReference {
  const LibraryFloorReference._(this._id);

  final String _id;
}

/// One floor beneath a selected library location.
class LibraryFloor {
  const LibraryFloor({
    required this.name,
    this.reference,
    this.isValid,
    this.totalSeats,
    this.availableSeats,
  });

  final LibraryFloorReference? reference;
  final String name;
  final bool? isValid;
  final BigInt? totalSeats;
  final BigInt? availableSeats;
}

class LibraryFloors {
  LibraryFloors({required List<LibraryFloor> items})
      : items = List.unmodifiable(items);

  final List<LibraryFloor> items;
}

/// Opaque area selector from a floor read for one campus date.
class LibrarySectionReference {
  const LibrarySectionReference._(this._id);

  final String _id;
}

class LibrarySection {
  const LibrarySection({
    required this.name,
    this.reference,
    this.isValid,
    this.totalSeats,
    this.availableSeats,
  });

  final LibrarySectionReference? reference;
  final String name;
  final bool? isValid;
  final BigInt? totalSeats;
  final BigInt? availableSeats;
}

/// A selected campus-local date and the sections returned for a floor.
class LibrarySections {
  LibrarySections(
      {required this.campusDate, required List<LibrarySection> items})
      : items = List.unmodifiable(items);

  /// ISO calendar date as returned for the campus. No timezone is inferred.
  final String campusDate;
  final List<LibrarySection> items;
}

/// Opaque opening-window selector from the current section result.
class LibraryTimeWindowReference {
  const LibraryTimeWindowReference._(this._id);

  final String _id;
}

class LibraryTimeWindow {
  const LibraryTimeWindow({
    required this.reference,
    required this.startsAt,
    required this.endsAt,
  });

  final LibraryTimeWindowReference reference;

  /// Campus-local display time in `HH:mm` form; no timezone is inferred.
  final String startsAt;

  /// Campus-local display time in `HH:mm` form; no timezone is inferred.
  final String endsAt;
}

class LibraryTimeWindows {
  LibraryTimeWindows(
      {required this.campusDate, required List<LibraryTimeWindow> items})
      : items = List.unmodifiable(items);

  final String campusDate;
  final List<LibraryTimeWindow> items;
}

/// Opaque selector used to request sockets for a live seat result.
class LibraryAvailabilityReference {
  const LibraryAvailabilityReference._(this._id);

  final String _id;
}

/// Opaque association key for a seat's separate socket-state result.
class LibrarySeatReference {
  const LibrarySeatReference._(this._id);

  final String _id;

  @override
  int get hashCode => _id.hashCode;

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is LibrarySeatReference && other._id == _id;
}

class LibrarySeat {
  const LibrarySeat({
    required this.reference,
    required this.name,
    required this.isAvailable,
  });

  final LibrarySeatReference reference;
  final String name;
  final bool isAvailable;
}

/// Live seat availability for a selected opening window.
class LibraryAvailability {
  LibraryAvailability({
    required this.reference,
    required this.campusDate,
    required List<LibrarySeat> seats,
  }) : seats = List.unmodifiable(seats);

  final LibraryAvailabilityReference reference;
  final String campusDate;
  final List<LibrarySeat> seats;
}

enum LibrarySocketState { available, unavailable, unknown }

class LibrarySocketStatus {
  const LibrarySocketStatus({required this.seat, required this.state});

  final LibrarySeatReference seat;
  final LibrarySocketState state;
}

class LibrarySockets {
  LibrarySockets({required List<LibrarySocketStatus> statuses})
      : statuses = List.unmodifiable(statuses);

  final List<LibrarySocketStatus> statuses;
}

/// Opaque cancellation selector from this Client's latest reservation read.
///
/// A reference exists only for a row the service itself still allows this
/// account to cancel, so an absent reference is the service's own statement
/// about that reservation rather than a read failure. A reference is spent by
/// one [LibraryClient.cancel] call.
class LibraryReservationReference {
  const LibraryReservationReference._(this._id);

  final String _id;
}

/// One reservation the account holds, as the service printed it.
class LibraryReservation {
  const LibraryReservation({
    this.reference,
    required this.position,
    required this.time,
    required this.status,
  });

  /// Present only when the service's own row carried a cancellation control.
  final LibraryReservationReference? reference;

  final String position;

  /// Campus-local display time as the service printed it; no timezone is
  /// inferred, and no format is asserted beyond what was received.
  final String time;

  final String status;
}

/// The account's reservation list as one read returned it.
class LibraryReservations {
  LibraryReservations({required List<LibraryReservation> reservations})
      : reservations = List.unmodifiable(reservations);

  final List<LibraryReservation> reservations;
}

/// Library reads using the Client's shared Runtime, transport, and account.
class LibraryClient {
  LibraryClient._(this._handle);

  final native.ClientHandle _handle;

  /// Reads the root library directory. A successful refresh invalidates all
  /// floor, section, opening-window, seat, and socket references.
  Future<ReadResult<LibraryDirectory>> directory() => _sdkCall(
        () async => _libraryDirectoryResult(await _handle.libraryDirectory()),
      );

  /// Reads floors for a location from this Client's latest directory.
  Future<ReadResult<LibraryFloors>> floors(LibraryReference library) =>
      _sdkCall(() async => _libraryFloorsResult(
            await _handle.libraryFloors(libraryReferenceId: library._id),
          ));

  /// Reads sections for today or tomorrow from the latest floor result.
  Future<ReadResult<LibrarySections>> sections({
    required LibraryFloorReference floor,
    required LibraryDay day,
  }) =>
      _sdkCall(() async => _librarySectionsResult(
            await _handle.librarySections(
              floorReferenceId: floor._id,
              day: switch (day) {
                LibraryDay.today => native.LibraryDayDto.today,
                LibraryDay.tomorrow => native.LibraryDayDto.tomorrow,
              },
            ),
          ));

  /// Reads opening windows for a section from the latest section result.
  Future<ReadResult<LibraryTimeWindows>> timeWindows(
    LibrarySectionReference section,
  ) =>
      _sdkCall(() async => _libraryTimeWindowsResult(
            await _handle.libraryTimeWindows(
              sectionReferenceId: section._id,
            ),
          ));

  /// Reads live seat availability for a current opening-window reference.
  Future<ReadResult<LibraryAvailability>> seats(
    LibraryTimeWindowReference window,
  ) =>
      _sdkCall(() async => _libraryAvailabilityResult(
            await _handle.librarySeats(windowReferenceId: window._id),
          ));

  /// Reads independent socket states for a prior seat availability result.
  Future<ReadResult<LibrarySockets>> sockets(
    LibraryAvailabilityReference availability,
  ) =>
      _sdkCall(() async => _librarySocketsResult(
            await _handle.librarySockets(
              availabilityReferenceId: availability._id,
            ),
          ));

  /// Reads the account's own reservation list.
  ///
  /// A successful read replaces every [LibraryReservationReference] this Client
  /// handed out before, because the previous list no longer describes the
  /// account.
  Future<ReadResult<LibraryReservations>> reservations() => _sdkCall(
        () async => _libraryReservationsResult(
          await _handle.libraryReservations(),
        ),
      );

  /// Reserves one seat of a window from this Client's latest [seats] result.
  ///
  /// Dispatched at most once: after this call the seat reference cannot be
  /// used again, so a result reported as unconfirmed is not sent a second time.
  /// Read [reservations] to learn what the account now holds.
  Future<void> reserve(LibraryTimeWindowReference window, LibrarySeat seat) =>
      _sdkCall(() => _handle.libraryReserve(
            windowReferenceId: window._id,
            seatReferenceId: seat.reference._id,
          ));

  /// Cancels one reservation returned by this Client's latest [reservations].
  ///
  /// The reference is spent exactly as in [reserve].
  Future<void> cancel(LibraryReservationReference reference) => _sdkCall(
        () => _handle.libraryCancel(reservationReferenceId: reference._id),
      );

  /// Turns the power socket of one seat on or off.
  ///
  /// The seat must belong to this Client's latest [seats] result for the same
  /// section. Dispatched at most once, and unlike [reserve] the seat reference is
  /// **not** spent, so [sockets] stays usable for re-reading the state this call
  /// changed after a result reported as unconfirmed.
  Future<void> setSocketState(
    LibraryAvailabilityReference availability,
    LibrarySeat seat, {
    required bool available,
  }) =>
      _sdkCall(() => _handle.librarySetSocketState(
            availabilityReferenceId: availability._id,
            seatReferenceId: seat.reference._id,
            isAvailable: available,
          ));
}

ReadResult<LibraryDirectory> _libraryDirectoryResult(
  native.LibraryDirectoryResultDto value,
) =>
    ReadResult(
      data: LibraryDirectory(
        libraries: value.data.libraries
            .map(
              (item) => LibraryPlace(
                reference: item.referenceId == null
                    ? null
                    : LibraryReference._(item.referenceId!),
                name: item.name,
                englishName: item.englishName,
                isValid: item.isValid,
                totalSeats: item.totalSeats,
                availableSeats: item.availableSeats,
              ),
            )
            .toList(growable: false),
      ),
      metadata: _readMetadata(value.metadata),
    );

ReadResult<LibraryFloors> _libraryFloorsResult(
  native.LibraryFloorsResultDto value,
) =>
    ReadResult(
      data: LibraryFloors(
        items: value.data.items
            .map(
              (item) => LibraryFloor(
                reference: item.referenceId == null
                    ? null
                    : LibraryFloorReference._(item.referenceId!),
                name: item.name,
                isValid: item.isValid,
                totalSeats: item.totalSeats,
                availableSeats: item.availableSeats,
              ),
            )
            .toList(growable: false),
      ),
      metadata: _readMetadata(value.metadata),
    );

ReadResult<LibrarySections> _librarySectionsResult(
  native.LibrarySectionsResultDto value,
) =>
    ReadResult(
      data: LibrarySections(
        campusDate: value.data.day,
        items: value.data.items
            .map(
              (item) => LibrarySection(
                reference: item.referenceId == null
                    ? null
                    : LibrarySectionReference._(item.referenceId!),
                name: item.name,
                isValid: item.isValid,
                totalSeats: item.totalSeats,
                availableSeats: item.availableSeats,
              ),
            )
            .toList(growable: false),
      ),
      metadata: _readMetadata(value.metadata),
    );

ReadResult<LibraryTimeWindows> _libraryTimeWindowsResult(
  native.LibraryTimeWindowsResultDto value,
) =>
    ReadResult(
      data: LibraryTimeWindows(
        campusDate: value.data.day,
        items: value.data.items
            .map(
              (item) => LibraryTimeWindow(
                reference: LibraryTimeWindowReference._(item.referenceId),
                startsAt: item.startsAt,
                endsAt: item.endsAt,
              ),
            )
            .toList(growable: false),
      ),
      metadata: _readMetadata(value.metadata),
    );

ReadResult<LibraryAvailability> _libraryAvailabilityResult(
  native.LibraryAvailabilityResultDto value,
) =>
    ReadResult(
      data: LibraryAvailability(
        reference: LibraryAvailabilityReference._(value.data.referenceId),
        campusDate: value.data.day,
        seats: value.data.seats
            .map(
              (seat) => LibrarySeat(
                reference: LibrarySeatReference._(seat.referenceId),
                name: seat.name,
                isAvailable: seat.isAvailable,
              ),
            )
            .toList(growable: false),
      ),
      metadata: _readMetadata(value.metadata),
    );

ReadResult<LibrarySockets> _librarySocketsResult(
  native.LibrarySocketsResultDto value,
) =>
    ReadResult(
      data: LibrarySockets(
        statuses: value.data.statuses
            .map(
              (item) => LibrarySocketStatus(
                seat: LibrarySeatReference._(item.seatReferenceId),
                state: switch (item.state) {
                  native.LibrarySocketStateDto.available =>
                    LibrarySocketState.available,
                  native.LibrarySocketStateDto.unavailable =>
                    LibrarySocketState.unavailable,
                  native.LibrarySocketStateDto.unknown =>
                    LibrarySocketState.unknown,
                },
              ),
            )
            .toList(growable: false),
      ),
      metadata: _readMetadata(value.metadata),
    );

ReadResult<LibraryReservations> _libraryReservationsResult(
  native.LibraryReservationsResultDto value,
) =>
    ReadResult(
      data: LibraryReservations(
        reservations: value.data.reservations
            .map(
              (row) => LibraryReservation(
                // The service decides whether a row can still be cancelled:
                // only a row with its own control gets a reference.
                reference: row.referenceId == null
                    ? null
                    : LibraryReservationReference._(row.referenceId!),
                position: row.position,
                time: row.time,
                status: row.status,
              ),
            )
            .toList(growable: false),
      ),
      metadata: _readMetadata(value.metadata),
    );
