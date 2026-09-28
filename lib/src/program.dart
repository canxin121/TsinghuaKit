part of '../tsinghua_kit.dart';

/// The completion state of one course row.
enum ProgramCourseState {
  /// The course has a grade or an explicit completion mark.
  completed,

  /// The course is currently elected but not yet graded.
  elected,

  /// The course is not finished, including a withdrawn `W`/`F`/`I` mark.
  notCompleted,
}

/// The course-attribute group a course set belongs to.
enum ProgramCourseSetKind {
  compulsory,
  restricted,
  elective,

  /// Courses completed outside the plan, reported as their own group.
  excluded,
}

ProgramCourseState _programCourseState(String value) => switch (value) {
  'completed' => ProgramCourseState.completed,
  'elected' => ProgramCourseState.elected,
  _ => ProgramCourseState.notCompleted,
};

ProgramCourseSetKind _programCourseSetKind(String value) => switch (value) {
  'compulsory' => ProgramCourseSetKind.compulsory,
  'restricted' => ProgramCourseSetKind.restricted,
  'elective' => ProgramCourseSetKind.elective,
  _ => ProgramCourseSetKind.excluded,
};

/// One course row from the completion report.
class ProgramCourse {
  const ProgramCourse({
    required this.courseId,
    required this.name,
    required this.credit,
    required this.point,
    required this.grade,
    required this.state,
  });

  final String courseId;
  final String name;
  final double credit;

  /// Absent for courses that cannot carry a grade point.
  final double? point;

  /// Absent for unelected or unfinished courses.
  final String? grade;

  final ProgramCourseState state;
}

/// One course set with its own credit and course-count requirements.
class ProgramCourseSet {
  ProgramCourseSet({
    required this.name,
    required this.kind,
    required this.requiredCredit,
    required this.completedCredit,
    required this.requiredCourseCount,
    required this.completedCourseCount,
    required this.fullCompleted,
    required List<ProgramCourse> courses,
  }) : courses = List.unmodifiable(courses);

  final String name;
  final ProgramCourseSetKind kind;
  final double? requiredCredit;
  final double? completedCredit;
  final int? requiredCourseCount;
  final int? completedCourseCount;

  /// True when this group's own requirements are fully satisfied.
  final bool fullCompleted;

  final List<ProgramCourse> courses;
}

/// The plan-wide completion summary and its course sets.
class ProgramCompletion {
  ProgramCompletion({
    required this.completedCredit,
    required this.compulsoryCredit,
    required this.restrictedCredit,
    required this.electiveCredit,
    required List<String> duplicatedCourses,
    required this.excludedCredit,
    required List<ProgramCourseSet> courseSets,
  })  : duplicatedCourses = List.unmodifiable(duplicatedCourses),
        courseSets = List.unmodifiable(courseSets);

  /// The credits completed inside the plan, as the plan itself counts them.
  final double completedCredit;

  final double compulsoryCredit;
  final double restrictedCredit;
  final double electiveCredit;

  /// The service-reported duplicated course names, unchanged.
  final List<String> duplicatedCourses;

  /// Present only when the report carries the out-of-plan summary.
  final double? excludedCredit;

  final List<ProgramCourseSet> courseSets;
}

/// Degree-program completion through the shared Rust Client.
///
/// The report is read live on every call; a failed read is an error rather
/// than a stale report from an earlier call.
class ProgramClient {
  ProgramClient._(this._handle);

  final native.ClientHandle _handle;

  /// Reads the plan-wide completion report.
  Future<ReadResult<ProgramCompletion>> completion() => _sdkCall(
        () async =>
            _programCompletionResult(await _handle.programCompletionResult()),
      );
}

ReadResult<ProgramCompletion> _programCompletionResult(
  native.ProgramCompletionResultDto value,
) =>
    ReadResult(
      data: ProgramCompletion(
        completedCredit: value.data.completedCredit,
        compulsoryCredit: value.data.compulsoryCredit,
        restrictedCredit: value.data.restrictedCredit,
        electiveCredit: value.data.electiveCredit,
        duplicatedCourses: value.data.duplicatedCourses,
        excludedCredit: value.data.excludedCredit,
        courseSets: value.data.courseSets
            .map(
              (set) => ProgramCourseSet(
                name: set.name,
                kind: _programCourseSetKind(set.kind),
                requiredCredit: set.requiredCredit,
                completedCredit: set.completedCredit,
                requiredCourseCount: set.requiredCourseCount,
                completedCourseCount: set.completedCourseCount,
                fullCompleted: set.fullCompleted,
                courses: set.courses
                    .map(
                      (course) => ProgramCourse(
                        courseId: course.courseId,
                        name: course.name,
                        credit: course.credit,
                        point: course.point,
                        grade: course.grade,
                        state: _programCourseState(course.state),
                      ),
                    )
                    .toList(growable: false),
              ),
            )
            .toList(growable: false),
      ),
      metadata: _readMetadata(value.metadata),
    );
