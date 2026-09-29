part of '../tsinghua_kit.dart';

/// One teaching-evaluation questionnaire the account may currently fill in.
class AssessmentItem {
  const AssessmentItem({
    required this.name,
    required this.evaluated,
    required this.referenceId,
  });

  /// The course name exactly as the service rendered it.
  final String name;

  /// True when the service marked this course as already evaluated.
  final bool evaluated;

  /// An opaque handle for this row's questionnaire inside the Rust session
  /// that produced it. It is **not** a service URL, and it stops resolving
  /// once that session is replaced or invalidated.
  final String referenceId;
}

/// The questionnaires the account may currently fill in.
///
/// A closed questionnaire window is reported as a
/// `TsinghuaKitException` with code `not_available`; it is never presented as
/// a valid empty list.
class AssessmentList {
  AssessmentList({required List<AssessmentItem> items})
      : items = List.unmodifiable(items);

  final List<AssessmentItem> items;

  bool get isEmpty => items.isEmpty;

  int get length => items.length;
}

/// One question of one questionnaire and the answer the service holds for it.
class AssessmentQuestion {
  const AssessmentQuestion({
    required this.text,
    required this.score,
    required this.comment,
  });

  /// The question text exactly as the service rendered it.
  final String text;

  /// The score the service currently holds for this question.
  final int score;

  /// The comment the service currently holds, or `null` when this question
  /// rendered no comment field at all. An empty string means the service
  /// rendered an empty comment box.
  final String? comment;

  /// Whether this question has a comment field a caller may write to.
  bool get commentEditable => comment != null;
}

/// One person a questionnaire asks about: a teacher or a teaching assistant.
class AssessmentPerson {
  AssessmentPerson({
    required this.name,
    required this.assistant,
    required List<AssessmentQuestion> questions,
  }) : questions = List.unmodifiable(questions);

  /// The person's name as the service rendered it.
  final String name;

  /// True for a teaching assistant, false for a teacher.
  final bool assistant;

  /// This person's questions, in the order the page listed them.
  final List<AssessmentQuestion> questions;

  int get questionCount => questions.length;
}

/// One questionnaire's questions and current answers, as an editor sees them.
///
/// It is a display copy: the service's own submission state is not part of it,
/// and nothing here can be posted back. Answers are authored as an
/// [AssessmentAnswers] value and applied inside Rust to the questionnaire this
/// copy came from.
class AssessmentForm {
  AssessmentForm({
    required this.course,
    required this.referenceId,
    required this.score,
    required this.comment,
    required this.commentEditable,
    required List<AssessmentPerson> teachers,
    required List<AssessmentPerson> assistants,
    required this.fieldCount,
  })  : teachers = List.unmodifiable(teachers),
        assistants = List.unmodifiable(assistants);

  /// The course this questionnaire evaluates, as the list named it.
  final String course;

  /// The opaque row handle the answers must name again when submitted.
  final String referenceId;

  /// The overall score the service currently holds.
  final int score;

  /// The overall comment the service currently holds, if any.
  final String? comment;

  /// Whether the service rendered an overall comment field at all.
  final bool commentEditable;

  /// The teachers this questionnaire asks about.
  final List<AssessmentPerson> teachers;

  /// The teaching assistants this questionnaire asks about.
  final List<AssessmentPerson> assistants;

  /// The number of name/value pairs a submission of this questionnaire will
  /// carry. It is a shape check for the caller, not a body.
  final int fieldCount;

  /// Every person, teachers first, in page order.
  List<AssessmentPerson> get people => [...teachers, ...assistants];
}

/// One question's answer.
class AssessmentQuestionAnswer {
  const AssessmentQuestionAnswer({required this.score, this.comment});

  /// The score to select. It must be within
  /// [AssessmentClient.minScore]..[AssessmentClient.maxScore]; a value outside
  /// that range is refused before any request.
  final int score;

  /// The comment to store, or `null` to leave the value the service already
  /// holds. An empty string clears the comment.
  final String? comment;
}

/// One person's answers, in the order that person's questions were listed.
class AssessmentPersonAnswers {
  AssessmentPersonAnswers({required List<AssessmentQuestionAnswer> questions})
      : questions = List.unmodifiable(questions);

  final List<AssessmentQuestionAnswer> questions;
}

/// The answers filled in for one questionnaire.
///
/// The answer shape must match the form position by position: a person or a
/// question the form did not render is refused rather than silently skipped,
/// so answers built against a different questionnaire cannot half-apply.
class AssessmentAnswers {
  AssessmentAnswers({
    required this.referenceId,
    required this.score,
    this.comment,
    required List<AssessmentPersonAnswers> teachers,
    required List<AssessmentPersonAnswers> assistants,
  })  : teachers = List.unmodifiable(teachers),
        assistants = List.unmodifiable(assistants);

  /// The row these answers are for, as the list handed it out.
  final String referenceId;

  /// The overall score.
  final int score;

  /// The overall comment, or `null` to leave the current one.
  final String? comment;

  final List<AssessmentPersonAnswers> teachers;
  final List<AssessmentPersonAnswers> assistants;
}

/// Teaching-evaluation questionnaires through the shared Rust Client.
class AssessmentClient {
  AssessmentClient._(this._handle);

  final native.ClientHandle _handle;

  /// The highest score the service accepts.
  static const int maxScore = 7;

  /// The lowest score the service accepts.
  static const int minScore = 1;

  /// Reads the questionnaires this account may currently fill in.
  ///
  /// A closed questionnaire window is reported as a `not_available` failure,
  /// never as a valid empty list.
  Future<ReadResult<AssessmentList>> list() => _sdkCall(
        () async => _assessmentList(await _handle.assessmentListResult()),
      );

  /// Reads one questionnaire's questions and the answers the service holds for
  /// them.
  ///
  /// [referenceId] must come from this client's most recent [list] result and
  /// must be passed back unchanged when the answers are submitted.
  Future<AssessmentForm> form({required String referenceId}) => _sdkCall(
        () async => _assessmentForm(
          await _handle.assessmentFormResult(referenceId: referenceId),
        ),
      );

  /// Stores one filled-in questionnaire.
  ///
  /// This is a one-shot write: it is dispatched exactly once, and an
  /// unconfirmed outcome is reported as an `outcome_unconfirmed` failure
  /// rather than being retried. Reading [list] again is the only way to learn
  /// what the service holds.
  Future<void> submit({required AssessmentAnswers answers}) => _sdkCall(
        () async => _handle.assessmentSubmit(
          answers: _assessmentAnswersDto(answers),
        ),
      );
}

ReadResult<AssessmentList> _assessmentList(native.AssessmentListResultDto value) =>
    ReadResult(
      data: AssessmentList(
        items: value.data.items
            .map(
              (item) => AssessmentItem(
                name: item.name,
                evaluated: item.evaluated,
                referenceId: item.referenceId,
              ),
            )
            .toList(growable: false),
      ),
      metadata: _readMetadata(value.metadata),
    );

AssessmentForm _assessmentForm(native.AssessmentFormDto value) => AssessmentForm(
      course: value.course,
      referenceId: value.referenceId,
      score: value.score,
      comment: value.comment,
      commentEditable: value.commentEditable,
      teachers: value.teachers.map(_assessmentPerson).toList(growable: false),
      assistants:
          value.assistants.map(_assessmentPerson).toList(growable: false),
      fieldCount: value.fieldCount,
    );

AssessmentPerson _assessmentPerson(native.AssessmentPersonDto value) =>
    AssessmentPerson(
      name: value.name,
      assistant: value.assistant,
      questions: value.questions
          .map(
            (question) => AssessmentQuestion(
              text: question.text,
              score: question.score,
              comment: question.comment,
            ),
          )
          .toList(growable: false),
    );

native.AssessmentAnswersDto _assessmentAnswersDto(AssessmentAnswers answers) =>
    native.AssessmentAnswersDto(
      referenceId: answers.referenceId,
      score: answers.score,
      comment: answers.comment,
      teachers: answers.teachers
          .map(_assessmentPersonAnswersDto)
          .toList(growable: false),
      assistants: answers.assistants
          .map(_assessmentPersonAnswersDto)
          .toList(growable: false),
    );

native.AssessmentPersonAnswersDto _assessmentPersonAnswersDto(
  AssessmentPersonAnswers answers,
) =>
    native.AssessmentPersonAnswersDto(
      questions: answers.questions
          .map(
            (question) => native.AssessmentQuestionAnswerDto(
              score: question.score,
              comment: question.comment,
            ),
          )
          .toList(growable: false),
    );
