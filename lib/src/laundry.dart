part of '../tsinghua_kit.dart';

/// One laundry building a vendor serves.
class LaundryBuilding {
  const LaundryBuilding({
    required this.id,
    required this.name,
    required this.provider,
  });

  /// The vendor's own building identifier, to be passed back to
  /// [LaundryClient.rooms]. It is only meaningful for [provider].
  final String id;

  /// The building name as the vendor renders it.
  final String name;

  /// The vendor's stable key, e.g. `jieli`.
  final String provider;
}

/// One vendor-defined group of laundry buildings.
class LaundryBuildingGroup {
  LaundryBuildingGroup({
    required this.key,
    required this.label,
    required List<LaundryBuilding> buildings,
  }) : buildings = List.unmodifiable(buildings);

  /// The group's stable key.
  final String key;

  /// The group's display label.
  final String label;

  final List<LaundryBuilding> buildings;
}

/// One washing machine and the state the vendor currently reports for it.
class LaundryMachine {
  const LaundryMachine({
    required this.name,
    required this.kind,
    required this.room,
    required this.status,
    required this.etaMinutes,
  });

  /// The machine's own name or number.
  final String name;

  /// The machine type the vendor reports, e.g. `洗衣机`.
  final String kind;

  /// The room label the vendor reports, which may be empty when it reports
  /// none.
  final String room;

  /// The state the vendor currently reports. See [LaundryStatus].
  final LaundryStatus status;

  /// Minutes the vendor expects the running program to take. It is the
  /// vendor's own estimate and is absent when the vendor reports none.
  final int? etaMinutes;
}

/// One laundry room and its machines.
class LaundryRoom {
  LaundryRoom({required this.name, required List<LaundryMachine> machines})
      : machines = List.unmodifiable(machines);

  final String name;
  final List<LaundryMachine> machines;
}

/// One building's rooms, and what the vendor did not answer for.
class LaundryRoomsReport {
  LaundryRoomsReport({
    required this.provider,
    required List<LaundryRoom> rooms,
    required List<String> failedCategories,
    required this.fetchedAt,
  })  : rooms = List.unmodifiable(rooms),
        failedCategories = List.unmodifiable(failedCategories);

  final String provider;
  final List<LaundryRoom> rooms;

  /// Vendor categories that answered with a failure. A non-empty list means
  /// this read is incomplete in exactly the way it says — show it as a partial
  /// result rather than as a complete room list.
  final List<String> failedCategories;

  /// The vendor's own snapshot time, absent when the vendor reports none.
  final DateTime? fetchedAt;
}

/// The laundry machine states a caller can rely on.
enum LaundryStatus {
  /// Free to use now.
  idle,

  /// Currently running.
  working,

  /// The vendor reports a fault.
  error,

  /// The vendor reports the machine offline.
  offline,

  /// Powered but not in a program.
  standby,

  /// The vendor reported a state this client does not recognise.
  unknown,
}

LaundryStatus _laundryStatus(String value) => switch (value) {
  'idle' => LaundryStatus.idle,
  'working' => LaundryStatus.working,
  'error' => LaundryStatus.error,
  'offline' => LaundryStatus.offline,
  'standby' => LaundryStatus.standby,
  _ => LaundryStatus.unknown,
};

/// One selectable laundry provider with its display label.
class LaundryProviderOption {
  const LaundryProviderOption({required this.key, required this.label});

  /// The stable key to pass to [LaundryClient.buildings].
  final String key;

  final String label;
}

/// Dormitory laundry rooms served by third-party vendors.
///
/// These vendors have no campus-account binding, so these reads carry no campus
/// credential and need no proven campus session. Nothing here is cached: the
/// vendor's own snapshot time is reported verbatim and device state changes
/// minute by minute.
///
/// Only building lists and device status are reachable. No ordering, payment,
/// or other write exists on this API.
class LaundryClient {
  LaundryClient._(this._handle);

  final native.ClientHandle _handle;

  /// The vendors this client reads, with their display labels.
  static Future<List<LaundryProviderOption>> providers() async {
    await TsinghuaKit.initialize();
    final values = await native.laundryProviders();
    return values
        .map(
          (value) => LaundryProviderOption(key: value.key, label: value.label),
        )
        .toList(growable: false);
  }

  /// The machine states this client reports, with their display labels.
  static Future<List<LaundryProviderOption>> statuses() async {
    await TsinghuaKit.initialize();
    final values = await native.laundryStatuses();
    return values
        .map(
          (value) => LaundryProviderOption(key: value.key, label: value.label),
        )
        .toList(growable: false);
  }

  /// Reads one vendor's building groups.
  ///
  /// A provider this client does not read is refused before any request is
  /// built.
  Future<List<LaundryBuildingGroup>> buildings({required String provider}) =>
      _sdkCall(
        () async => (await _handle.laundryBuildings(provider: provider))
            .map(_laundryGroup)
            .toList(growable: false),
      );

  /// Reads one building's rooms and machines.
  ///
  /// [buildingId] must come from a previous [buildings] call for the same
  /// [provider].
  Future<LaundryRoomsReport> rooms({
    required String provider,
    required String buildingId,
  }) =>
      _sdkCall(
        () async => _laundryRooms(
          await _handle.laundryRooms(
            provider: provider,
            buildingId: buildingId,
          ),
        ),
      );
}

LaundryBuildingGroup _laundryGroup(native.LaundryBuildingGroupDto value) =>
    LaundryBuildingGroup(
      key: value.key,
      label: value.label,
      buildings: value.buildings
          .map(
            (building) => LaundryBuilding(
              id: building.id,
              name: building.name,
              provider: building.provider,
            ),
          )
          .toList(growable: false),
    );

LaundryRoomsReport _laundryRooms(native.LaundryRoomsDto value) =>
    LaundryRoomsReport(
      provider: value.provider,
      rooms: value.rooms
          .map(
            (room) => LaundryRoom(
              name: room.name,
              machines: room.machines
                  .map(
                    (machine) => LaundryMachine(
                      name: machine.name,
                      kind: machine.kind,
                      room: machine.room,
                      status: _laundryStatus(machine.status),
                      etaMinutes: machine.etaMinutes,
                    ),
                  )
                  .toList(growable: false),
            ),
          )
          .toList(growable: false),
      failedCategories: value.failedCategories,
      fetchedAt: value.fetchedAtUnix == null
          ? null
          : DateTime.fromMillisecondsSinceEpoch(
              _platformInt64ToBigInt(value.fetchedAtUnix!).toInt() * 1000,
              isUtc: true,
            ),
    );
