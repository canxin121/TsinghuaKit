part of '../tsinghua_kit.dart';

/// Opaque building selector returned by this Client's latest directory.
class ClassroomBuildingReference {
  const ClassroomBuildingReference._(this._id);

  final String _id;
}

/// One building from the current classroom directory.
class ClassroomBuilding {
  const ClassroomBuilding({
    required this.name,
    this.reference,
    this.defaultWeek,
  });

  final ClassroomBuildingReference? reference;
  final String name;
  final int? defaultWeek;
}

/// The complete building directory returned by the service.
class ClassroomBuildings {
  ClassroomBuildings({required List<ClassroomBuilding> items})
      : items = List.unmodifiable(items);

  final List<ClassroomBuilding> items;
}

/// Normalized status for one class-period slot.
enum ClassroomSlotStatus {
  free,
  occupied,
  exam,
  borrowed,
  disabled,
  unknown,
}

class ClassroomSlot {
  const ClassroomSlot({required this.status, this.unknownClassName});

  final ClassroomSlotStatus status;

  /// Original label, populated only for an unrecognized source category.
  final String? unknownClassName;
}

/// One classroom and its 42 Monday-first period statuses.
class ClassroomRoom {
  ClassroomRoom({required this.name, required List<ClassroomSlot> slots})
      : slots = List.unmodifiable(slots);

  final String name;
  final List<ClassroomSlot> slots;
}

/// Weekly availability for one selected building.
class ClassroomAvailability {
  ClassroomAvailability({
    required this.week,
    required List<int> validWeeks,
    required List<String> weekDates,
    required List<ClassroomRoom> rooms,
  })  : validWeeks = List.unmodifiable(validWeeks),
        weekDates = List.unmodifiable(weekDates),
        rooms = List.unmodifiable(rooms);

  final int week;
  final List<int> validWeeks;

  /// Seven Monday-first campus date labels. No timezone is inferred.
  final List<String> weekDates;

  final List<ClassroomRoom> rooms;
}

/// Classroom building and weekly availability reads.
class ClassroomsClient {
  ClassroomsClient._(this._handle);

  final native.ClientHandle _handle;

  /// Reads the current building directory. A successful refresh invalidates
  /// references returned by the previous directory.
  Future<ReadResult<ClassroomBuildings>> buildings() => _sdkCall(
        () async => _classroomBuildingsResult(
          await _handle.classroomBuildings(),
        ),
      );

  /// Reads a building's weekly room matrix using its current directory
  /// reference. Omit [week] to select the source-provided building default.
  /// Explicit week numbers are bounded and validated by Rust.
  Future<ReadResult<ClassroomAvailability>> availability({
    required ClassroomBuildingReference building,
    int? week,
  }) =>
      _sdkCall(() async => _classroomAvailabilityResult(
            await _handle.classroomAvailability(
              buildingReferenceId: building._id,
              week: week,
            ),
          ));
}

ReadResult<ClassroomBuildings> _classroomBuildingsResult(
  native.ClassroomBuildingsResultDto value,
) =>
    ReadResult(
      data: ClassroomBuildings(
        items: value.data.items
            .map(
              (building) => ClassroomBuilding(
                reference: building.referenceId == null
                    ? null
                    : ClassroomBuildingReference._(building.referenceId!),
                name: building.name,
                defaultWeek: building.defaultWeek,
              ),
            )
            .toList(growable: false),
      ),
      metadata: _readMetadata(value.metadata),
    );

ReadResult<ClassroomAvailability> _classroomAvailabilityResult(
  native.ClassroomAvailabilityResultDto value,
) =>
    ReadResult(
      data: ClassroomAvailability(
        week: value.data.week,
        validWeeks: value.data.validWeeks,
        weekDates: value.data.weekDates,
        rooms: value.data.rooms
            .map(
              (room) => ClassroomRoom(
                name: room.name,
                slots: room.slots
                    .map(
                      (slot) => ClassroomSlot(
                        status: switch (slot.status) {
                          native.ClassroomSlotStatusDto.free =>
                            ClassroomSlotStatus.free,
                          native.ClassroomSlotStatusDto.occupied =>
                            ClassroomSlotStatus.occupied,
                          native.ClassroomSlotStatusDto.exam =>
                            ClassroomSlotStatus.exam,
                          native.ClassroomSlotStatusDto.borrowed =>
                            ClassroomSlotStatus.borrowed,
                          native.ClassroomSlotStatusDto.disabled =>
                            ClassroomSlotStatus.disabled,
                          native.ClassroomSlotStatusDto.unknown =>
                            ClassroomSlotStatus.unknown,
                        },
                        unknownClassName: slot.unknownClassName,
                      ),
                    )
                    .toList(growable: false),
              ),
            )
            .toList(growable: false),
      ),
      metadata: _readMetadata(value.metadata),
    );
