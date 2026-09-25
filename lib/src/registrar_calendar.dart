part of '../tsinghua_kit.dart';

/// Academic stage verified by the Registrar service.
enum AcademicStage { undergraduate, graduate, unknown }

/// Undergraduate curriculum selected for the grade report.
enum GradeReportKind { firstDegree, secondDegree, minor, unknown }

/// Normalized kind of a Registrar schedule event.
enum ScheduleEventKind { course, exam, event, deadline, reminder, unknown }

/// Weekday reported for a Registrar examination.
enum ExamWeekday {
  monday,
  tuesday,
  wednesday,
  thursday,
  friday,
  saturday,
  sunday,
  unknown,
}

/// Semester selector for a published school calendar.
enum SchoolCalendarSemester { autumn, spring, unknown }

/// Language selector for a published school calendar.
enum SchoolCalendarLanguage { chinese, english, unknown }

/// One Registrar schedule event.
class ScheduleEvent {
  const ScheduleEvent({
    required this.title,
    required this.kind,
    required this.startsAtUtc,
    required this.endsAtUtc,
    required this.isAllDay,
    required this.location,
    required this.description,
  });

  final String title;
  final ScheduleEventKind kind;

  /// The event start converted to UTC from the verified service timestamp.
  final DateTime startsAtUtc;

  /// The event end converted to UTC, when the service supplied one.
  final DateTime? endsAtUtc;

  final bool isAllDay;
  final String? location;
  final String? description;
}

/// A complete schedule for the semester selected by the Registrar runtime.
class SemesterSchedule {
  SemesterSchedule({
    required this.semester,
    required this.stage,
    required this.firstDay,
    required this.lastDay,
    required this.weekCount,
    required this.currentWeek,
    required List<ScheduleEvent> events,
  }) : events = List.unmodifiable(events);

  final String semester;
  final AcademicStage stage;

  /// ISO calendar date from the service, without an inferred time zone.
  final String firstDay;

  /// ISO calendar date from the service, without an inferred time zone.
  final String lastDay;

  final int weekCount;
  final int currentWeek;
  final List<ScheduleEvent> events;
}

/// One course result in a Registrar grade report.
class CourseGrade {
  const CourseGrade({
    required this.courseName,
    required this.credit,
    required this.grade,
    required this.gradePoint,
    required this.semester,
  });

  final String courseName;
  final double credit;
  final String grade;
  final double? gradePoint;
  final String semester;
}

/// The complete grade report for the authenticated academic stage.
class GradeReport {
  GradeReport({
    required this.stage,
    required this.kind,
    required List<CourseGrade> courses,
  }) : courses = List.unmodifiable(courses);

  final AcademicStage stage;
  final GradeReportKind? kind;
  final List<CourseGrade> courses;
}

/// One examination in the complete stage-specific report.
class Exam {
  const Exam({
    required this.courseCode,
    required this.courseSequence,
    required this.courseName,
    required this.month,
    required this.day,
    required this.weekday,
    required this.scheduleLabel,
    required this.location,
    required this.department,
    required this.category,
    required this.instructor,
    required this.headcount,
  });

  final String courseCode;
  final String courseSequence;
  final String courseName;

  /// Month and day are separate because the source report may omit a year.
  final int month;
  final int day;
  final ExamWeekday weekday;
  final String scheduleLabel;
  final String location;
  final String? department;
  final String? category;
  final String? instructor;
  final int? headcount;
}

/// A complete Registrar examination report.
class ExamReport {
  ExamReport({required this.stage, required List<Exam> exams})
      : exams = List.unmodifiable(exams);

  final AcademicStage stage;
  final List<Exam> exams;
}

/// Current and upcoming terms returned by the authenticated Learn calendar.
class LearnTermCalendar {
  LearnTermCalendar(
      {required this.current, required List<AcademicTerm> upcoming})
      : upcoming = List.unmodifiable(upcoming);

  final AcademicTerm current;
  final List<AcademicTerm> upcoming;
}

/// One Learn academic term.
class AcademicTerm {
  const AcademicTerm({
    required this.label,
    required this.startsOn,
    required this.endsOn,
    required this.teachingWeekOne,
    required this.weekCount,
  });

  final String label;

  /// ISO calendar date from Learn, without an inferred time zone.
  final String startsOn;

  /// ISO calendar date from Learn, without an inferred time zone.
  final String endsOn;

  /// ISO calendar date for the first teaching week, when provided.
  final String teachingWeekOne;
  final int weekCount;
}

/// Verified bytes for one published school-calendar image.
class SchoolCalendarImage {
  SchoolCalendarImage({
    required this.latestYear,
    required this.year,
    required this.semester,
    required this.language,
    required Uint8List bytes,
  }) : bytes = Uint8List.fromList(bytes);

  final int latestYear;
  final int year;
  final SchoolCalendarSemester semester;
  final SchoolCalendarLanguage language;

  /// A caller-owned copy of the validated image bytes.
  final Uint8List bytes;
}

/// Registrar schedule, grade, and examination reads on this Client.
class RegistrarClient {
  RegistrarClient._(this._handle);

  final native.ClientHandle _handle;

  /// Reads the complete schedule selected by the current Registrar session.
  Future<ReadResult<SemesterSchedule>> semesterSchedule() => _sdkCall(
        () async => _semesterScheduleResult(
          await _handle.registrarSemesterSchedule(),
        ),
      );

  /// Reads the complete grade report for the authenticated academic stage.
  Future<ReadResult<GradeReport>> grades() => _sdkCall(
        () async => _gradeReportResult(await _handle.registrarGrades()),
      );

  /// Reads the complete examination report for the authenticated stage.
  Future<ReadResult<ExamReport>> exams() => _sdkCall(
        () async => _examReportResult(await _handle.registrarExams()),
      );
}

/// Learn academic terms and published school-calendar images on this Client.
class CalendarClient {
  CalendarClient._(this._handle);

  final native.ClientHandle _handle;

  /// Reads the current and upcoming Learn terms from the authenticated service.
  Future<ReadResult<LearnTermCalendar>> learnTerms() => _sdkCall(
        () async => _learnTermCalendarResult(
          await _handle.calendarLearnTerms(),
        ),
      );

  /// Reads the latest published school calendar when [year] is null, or the
  /// selected year otherwise. Years outside the service's supported range are
  /// rejected by Rust before any request is sent.
  Future<ReadResult<SchoolCalendarImage>> schoolCalendarImage({
    int? year,
    required SchoolCalendarSemester semester,
    required SchoolCalendarLanguage language,
  }) =>
      _sdkCall(() async => _schoolCalendarImageResult(
            await _handle.calendarSchoolImage(
              year: year,
              semester: _schoolCalendarSemesterDto(semester),
              language: _schoolCalendarLanguageDto(language),
            ),
          ));
}

ReadResult<SemesterSchedule> _semesterScheduleResult(
  native.SemesterScheduleResultDto value,
) =>
    ReadResult(
      data: SemesterSchedule(
        semester: value.data.semester,
        stage: _academicStage(value.data.stage),
        firstDay: value.data.firstDay,
        lastDay: value.data.lastDay,
        weekCount: value.data.weekCount,
        currentWeek: value.data.currentWeek,
        events: value.data.events
            .map(
              (event) => ScheduleEvent(
                title: event.title,
                kind: _scheduleEventKind(event.kind),
                startsAtUtc: DateTime.parse(event.startsAtUtc).toUtc(),
                endsAtUtc: event.endsAtUtc == null
                    ? null
                    : DateTime.parse(event.endsAtUtc!).toUtc(),
                isAllDay: event.isAllDay,
                location: event.location,
                description: event.description,
              ),
            )
            .toList(growable: false),
      ),
      metadata: _readMetadata(value.metadata),
    );

ReadResult<GradeReport> _gradeReportResult(native.GradeReportResultDto value) =>
    ReadResult(
      data: GradeReport(
        stage: _academicStage(value.data.stage),
        kind:
            value.data.kind == null ? null : _gradeReportKind(value.data.kind!),
        courses: value.data.courses
            .map(
              (course) => CourseGrade(
                courseName: course.courseName,
                credit: course.credit,
                grade: course.grade,
                gradePoint: course.gradePoint,
                semester: course.semester,
              ),
            )
            .toList(growable: false),
      ),
      metadata: _readMetadata(value.metadata),
    );

ReadResult<ExamReport> _examReportResult(native.ExamReportResultDto value) =>
    ReadResult(
      data: ExamReport(
        stage: _academicStage(value.data.stage),
        exams: value.data.exams
            .map(
              (exam) => Exam(
                courseCode: exam.courseCode,
                courseSequence: exam.courseSequence,
                courseName: exam.courseName,
                month: exam.month,
                day: exam.day,
                weekday: _examWeekday(exam.weekday),
                scheduleLabel: exam.scheduleLabel,
                location: exam.location,
                department: exam.department,
                category: exam.category,
                instructor: exam.instructor,
                headcount: exam.headcount,
              ),
            )
            .toList(growable: false),
      ),
      metadata: _readMetadata(value.metadata),
    );

ReadResult<LearnTermCalendar> _learnTermCalendarResult(
  native.LearnTermCalendarResultDto value,
) =>
    ReadResult(
      data: LearnTermCalendar(
        current: _academicTerm(value.data.current),
        upcoming:
            value.data.upcoming.map(_academicTerm).toList(growable: false),
      ),
      metadata: _readMetadata(value.metadata),
    );

ReadResult<SchoolCalendarImage> _schoolCalendarImageResult(
  native.SchoolCalendarImageResultDto value,
) =>
    ReadResult(
      data: SchoolCalendarImage(
        latestYear: value.data.latestYear,
        year: value.data.year,
        semester: _schoolCalendarSemester(value.data.semester),
        language: _schoolCalendarLanguage(value.data.language),
        bytes: value.data.bytes,
      ),
      metadata: _readMetadata(value.metadata),
    );

AcademicTerm _academicTerm(native.AcademicTermDto value) => AcademicTerm(
      label: value.label,
      startsOn: value.startsOn,
      endsOn: value.endsOn,
      teachingWeekOne: value.teachingWeekOne,
      weekCount: value.weekCount,
    );

AcademicStage _academicStage(native.AcademicStageDto value) => switch (value) {
      native.AcademicStageDto.undergraduate => AcademicStage.undergraduate,
      native.AcademicStageDto.graduate => AcademicStage.graduate,
      native.AcademicStageDto.unknown => AcademicStage.unknown,
    };

GradeReportKind _gradeReportKind(native.GradeReportKindDto value) =>
    switch (value) {
      native.GradeReportKindDto.firstDegree => GradeReportKind.firstDegree,
      native.GradeReportKindDto.secondDegree => GradeReportKind.secondDegree,
      native.GradeReportKindDto.minor => GradeReportKind.minor,
      native.GradeReportKindDto.unknown => GradeReportKind.unknown,
    };

ScheduleEventKind _scheduleEventKind(native.ScheduleEventKindDto value) =>
    switch (value) {
      native.ScheduleEventKindDto.course => ScheduleEventKind.course,
      native.ScheduleEventKindDto.exam => ScheduleEventKind.exam,
      native.ScheduleEventKindDto.event => ScheduleEventKind.event,
      native.ScheduleEventKindDto.deadline => ScheduleEventKind.deadline,
      native.ScheduleEventKindDto.reminder => ScheduleEventKind.reminder,
      native.ScheduleEventKindDto.unknown => ScheduleEventKind.unknown,
    };

ExamWeekday _examWeekday(native.ExamWeekdayDto value) => switch (value) {
      native.ExamWeekdayDto.monday => ExamWeekday.monday,
      native.ExamWeekdayDto.tuesday => ExamWeekday.tuesday,
      native.ExamWeekdayDto.wednesday => ExamWeekday.wednesday,
      native.ExamWeekdayDto.thursday => ExamWeekday.thursday,
      native.ExamWeekdayDto.friday => ExamWeekday.friday,
      native.ExamWeekdayDto.saturday => ExamWeekday.saturday,
      native.ExamWeekdayDto.sunday => ExamWeekday.sunday,
      native.ExamWeekdayDto.unknown => ExamWeekday.unknown,
    };

SchoolCalendarSemester _schoolCalendarSemester(
  native.SchoolCalendarSemesterDto value,
) =>
    switch (value) {
      native.SchoolCalendarSemesterDto.autumn => SchoolCalendarSemester.autumn,
      native.SchoolCalendarSemesterDto.spring => SchoolCalendarSemester.spring,
      native.SchoolCalendarSemesterDto.unknown =>
        SchoolCalendarSemester.unknown,
    };

SchoolCalendarLanguage _schoolCalendarLanguage(
  native.SchoolCalendarLanguageDto value,
) =>
    switch (value) {
      native.SchoolCalendarLanguageDto.chinese =>
        SchoolCalendarLanguage.chinese,
      native.SchoolCalendarLanguageDto.english =>
        SchoolCalendarLanguage.english,
      native.SchoolCalendarLanguageDto.unknown =>
        SchoolCalendarLanguage.unknown,
    };

native.SchoolCalendarSemesterDto _schoolCalendarSemesterDto(
  SchoolCalendarSemester value,
) =>
    switch (value) {
      SchoolCalendarSemester.autumn => native.SchoolCalendarSemesterDto.autumn,
      SchoolCalendarSemester.spring => native.SchoolCalendarSemesterDto.spring,
      SchoolCalendarSemester.unknown =>
        native.SchoolCalendarSemesterDto.unknown,
    };

native.SchoolCalendarLanguageDto _schoolCalendarLanguageDto(
  SchoolCalendarLanguage value,
) =>
    switch (value) {
      SchoolCalendarLanguage.chinese =>
        native.SchoolCalendarLanguageDto.chinese,
      SchoolCalendarLanguage.english =>
        native.SchoolCalendarLanguageDto.english,
      SchoolCalendarLanguage.unknown =>
        native.SchoolCalendarLanguageDto.unknown,
    };
