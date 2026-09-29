part of '../tsinghua_kit.dart';

/// One course result looked up by its course number.
///
/// No account-identifying value is part of this type: the student id the
/// service's query also needs is derived inside Rust from the proven identity
/// and exists only inside the request body.
class CourseScore {
  const CourseScore({
    required this.name,
    required this.credit,
    required this.grade,
    required this.empty,
  });

  /// The course's own name as the service rendered it.
  final String name;

  /// The credit value the service printed, when it printed one.
  ///
  /// `null` when the service printed nothing in that column — a blank column is
  /// not a zero credit.
  final double? credit;

  /// The grade as the service rendered it, empty when it printed none.
  final String grade;

  /// True when the service reported no result at all for this course.
  ///
  /// This is the service's own answer about this account and course, so it is a
  /// valid empty state rather than an error, and never a record filled with
  /// placeholders.
  final bool empty;
}

/// One course result looked up by course number.
///
/// The lookup is read live on every call and never served from a cached copy: a
/// grade can be revised by the registrar at any time, so a retained value would
/// present a superseded result as current.
///
/// The account's own student id, which the service's query also needs, is
/// derived inside Rust from the proven identity; it is never an argument, a
/// result field, or a log field.
class CourseScoreClient {
  CourseScoreClient._(this._handle);

  final native.ClientHandle _handle;

  /// Looks up one course result by the caller's course number.
  ///
  /// The course number is validated before any request, so a value this client
  /// will not send is reported as invalid input rather than becoming a
  /// service-side query.
  Future<ReadResult<CourseScore>> lookup({required String courseId}) =>
      _sdkCall(
        () async => _courseScore(
          await _handle.courseScoreResult(courseId: courseId),
        ),
      );
}

ReadResult<CourseScore> _courseScore(native.CourseScoreResultDto value) =>
    ReadResult(
      data: CourseScore(
        name: value.data.name,
        credit: value.data.credit,
        grade: value.data.grade,
        empty: value.data.empty,
      ),
      metadata: _readMetadata(value.metadata),
    );
