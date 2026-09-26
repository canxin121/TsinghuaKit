part of '../tsinghua_kit.dart';

/// Kind of schedule item returned by the daily overview.
enum OverviewScheduleKind { course, exam, event, deadline, reminder, unknown }

/// One verified event in the daily academic summary.
class OverviewSchedule {
  const OverviewSchedule({
    required this.title,
    required this.kind,
    required this.startsAtUtc,
    required this.endsAtUtc,
    required this.allDay,
    required this.location,
  });

  final String title;
  final OverviewScheduleKind kind;
  final DateTime startsAtUtc;
  final DateTime? endsAtUtc;
  final bool allDay;
  final String? location;
}

/// One current Learn assignment in the daily summary.
class OverviewTodo {
  const OverviewTodo({required this.title, required this.dueAtUtc});

  final String title;
  final DateTime? dueAtUtc;
}

/// Flags for independent academic sections that could not be read.
class OverviewSectionFailures {
  const OverviewSectionFailures({
    required this.courses,
    required this.schedule,
    required this.todos,
    required this.services,
    required this.updates,
  });

  final bool courses;
  final bool schedule;
  final bool todos;
  final bool services;
  final bool updates;

  bool get any => courses || schedule || todos || services || updates;
}

/// Account-bound daily summary aggregated inside the Rust runtime.
/// The assignment counts here come from Learn, not the service hall.
class DailyOverview {
  DailyOverview({
    required this.date,
    required this.semester,
    required this.courseCount,
    required this.pendingTodoCount,
    required this.completedTodoCount,
    required List<OverviewSchedule> todaySchedule,
    required List<OverviewTodo> upcomingTodos,
    required this.nextSchedule,
    required this.sectionFailures,
  })  : todaySchedule = List.unmodifiable(todaySchedule),
        upcomingTodos = List.unmodifiable(upcomingTodos);

  /// ISO campus date, without an inferred timezone.
  final String date;
  final String? semester;
  final int courseCount;
  final int pendingTodoCount;
  final int completedTodoCount;
  final List<OverviewSchedule> todaySchedule;
  final List<OverviewTodo> upcomingTodos;
  final OverviewSchedule? nextSchedule;
  final OverviewSectionFailures sectionFailures;
}

/// Account-bound daily reads on the owning Client's single Rust runtime.
class OverviewClient {
  OverviewClient._(this._handle);

  final native.ClientHandle _handle;

  /// Reads an ISO campus date under an explicit cache policy. `cacheOnly`
  /// never contacts campus services. Partial sections remain visible through
  /// [DailyOverview.sectionFailures] and [ReadMetadata.coverage].
  Future<ReadResult<DailyOverview>> day(
    String date, {
    ReadPolicy policy = ReadPolicy.preferFreshCache,
  }) =>
      _sdkCall(() async => _dailyOverviewResult(await _handle.overviewDay(
            date: date,
            policy: _readPolicyDto(policy),
          )));
}

OverviewScheduleKind _overviewScheduleKind(
  native.OverviewScheduleKindDto value,
) =>
    switch (value) {
      native.OverviewScheduleKindDto.course => OverviewScheduleKind.course,
      native.OverviewScheduleKindDto.exam => OverviewScheduleKind.exam,
      native.OverviewScheduleKindDto.event => OverviewScheduleKind.event,
      native.OverviewScheduleKindDto.deadline => OverviewScheduleKind.deadline,
      native.OverviewScheduleKindDto.reminder => OverviewScheduleKind.reminder,
      native.OverviewScheduleKindDto.unknown => OverviewScheduleKind.unknown,
    };

OverviewSchedule _overviewSchedule(native.OverviewScheduleDto value) =>
    OverviewSchedule(
      title: value.title,
      kind: _overviewScheduleKind(value.kind),
      startsAtUtc: DateTime.parse(value.startsAtUtc).toUtc(),
      endsAtUtc: value.endsAtUtc == null
          ? null
          : DateTime.parse(value.endsAtUtc!).toUtc(),
      allDay: value.allDay,
      location: value.location,
    );

ReadResult<DailyOverview> _dailyOverviewResult(
  native.DailyOverviewResultDto value,
) =>
    ReadResult(
      data: DailyOverview(
        date: value.data.date,
        semester: value.data.semester,
        courseCount: value.data.courseCount,
        pendingTodoCount: value.data.pendingTodoCount,
        completedTodoCount: value.data.completedTodoCount,
        todaySchedule: value.data.todaySchedule
            .map(_overviewSchedule)
            .toList(growable: false),
        upcomingTodos: value.data.upcomingTodos
            .map((todo) => OverviewTodo(
                  title: todo.title,
                  dueAtUtc: todo.dueAtUtc == null
                      ? null
                      : DateTime.parse(todo.dueAtUtc!).toUtc(),
                ))
            .toList(growable: false),
        nextSchedule: value.data.nextSchedule == null
            ? null
            : _overviewSchedule(value.data.nextSchedule!),
        sectionFailures: OverviewSectionFailures(
          courses: value.data.sectionFailures.courses,
          schedule: value.data.sectionFailures.schedule,
          todos: value.data.sectionFailures.todos,
          services: value.data.sectionFailures.services,
          updates: value.data.sectionFailures.updates,
        ),
      ),
      metadata: _readMetadata(value.metadata),
    );
