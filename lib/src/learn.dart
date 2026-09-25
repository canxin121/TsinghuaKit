part of '../tsinghua_kit.dart';

/// Opaque selector for a course from this Client's latest Learn directory.
class LearnCourseReference {
  const LearnCourseReference._(this._id);

  final String _id;
}

/// One course returned by the authenticated Learn course directory.
class LearnCourse {
  const LearnCourse({
    required this.reference,
    required this.title,
    this.code,
    this.instructor,
    this.semester,
  });

  final LearnCourseReference reference;
  final String title;
  final String? code;
  final String? instructor;
  final String? semester;
}

/// The current course directory and the semester label supplied by Learn.
class LearnCourseCatalog {
  LearnCourseCatalog(
      {required this.semester, required List<LearnCourse> courses})
      : courses = List.unmodifiable(courses);

  final String semester;
  final List<LearnCourse> courses;
}

/// One announcement associated with a Learn course.
class LearnAnnouncement {
  const LearnAnnouncement({
    required this.title,
    required this.publishedAt,
    required this.expired,
    this.publisher,
    this.content,
    this.expiresAt,
    this.isRead,
    this.isImportant,
    this.isFavorited,
  });

  final String title;
  final String? publisher;
  final String? content;

  /// A timezone-aware instant, normalized to UTC.
  final DateTime publishedAt;

  /// A timezone-aware instant, normalized to UTC, when provided by Learn.
  final DateTime? expiresAt;

  final bool? isRead;
  final bool? isImportant;
  final bool? isFavorited;
  final bool expired;
}

/// Announcements for one selected course.
class LearnAnnouncements {
  LearnAnnouncements({required List<LearnAnnouncement> items})
      : items = List.unmodifiable(items);

  final List<LearnAnnouncement> items;
}

/// State normalized from Learn's assignment buckets.
enum LearnHomeworkState { pending, submitted, graded, unknown }

/// Opaque selector for an assignment in this Client's latest homework read.
class LearnHomeworkReference {
  const LearnHomeworkReference._(this._id);

  final String _id;
}

/// One assignment from a complete Learn homework response.
class LearnHomework {
  const LearnHomework({
    required this.reference,
    required this.title,
    required this.state,
    required this.dueAt,
    required this.detailAvailable,
    this.lateDueAt,
    this.submittedAt,
    this.gradedAt,
  });

  final LearnHomeworkReference reference;
  final String title;
  final LearnHomeworkState state;

  /// A timezone-aware deadline instant, normalized to UTC.
  final DateTime dueAt;

  /// A timezone-aware late deadline instant, normalized to UTC.
  final DateTime? lateDueAt;

  /// The submitted instant, normalized to UTC.
  final DateTime? submittedAt;

  /// The grading instant, normalized to UTC.
  final DateTime? gradedAt;

  final bool detailAvailable;
}

/// All assignment buckets for one selected course.
class LearnHomeworkList {
  LearnHomeworkList({required List<LearnHomework> items})
      : items = List.unmodifiable(items);

  final List<LearnHomework> items;
}

/// One attachment entry in an assignment detail section.
enum LearnHomeworkAttachmentKind {
  assignment,
  answer,
  submitted,
  grade,
  unknown
}

class LearnHomeworkAttachment {
  const LearnHomeworkAttachment({
    required this.kind,
    required this.name,
    this.size,
  });

  final LearnHomeworkAttachmentKind kind;
  final String name;
  final String? size;
}

/// Assignment body fields and attachment metadata. Attachment URLs are not exposed.
class LearnHomeworkDetail {
  LearnHomeworkDetail({
    this.description,
    this.answerContent,
    this.submittedContent,
    required List<LearnHomeworkAttachment> attachments,
  }) : attachments = List.unmodifiable(attachments);

  final String? description;
  final String? answerContent;
  final String? submittedContent;
  final List<LearnHomeworkAttachment> attachments;
}

/// Opaque selector for a file returned by this Client's latest file read.
class LearnCourseFileReference {
  const LearnCourseFileReference._(this._id);

  final String _id;
}

/// One file in the bounded Learn course-file list.
class LearnCourseFile {
  const LearnCourseFile({
    required this.reference,
    required this.title,
    required this.suggestedFilename,
    this.description,
    this.sizeLabel,
    this.uploadedAtLabel,
    this.fileType,
  });

  final LearnCourseFileReference reference;
  final String title;
  final String suggestedFilename;
  final String? description;
  final String? sizeLabel;

  /// Display text as supplied by Learn; no timezone is inferred.
  final String? uploadedAtLabel;

  final String? fileType;
}

/// Files returned for a course, with the source's completeness classification.
class LearnCourseFiles {
  LearnCourseFiles({
    required List<LearnCourseFile> items,
    required this.coverage,
  }) : items = List.unmodifiable(items);

  final List<LearnCourseFile> items;
  final ReadCoverage coverage;
}

/// One display label for a course-file category.
class LearnCourseFileCategory {
  const LearnCourseFileCategory({required this.title});

  final String title;
}

/// File categories displayed by the selected course.
class LearnCourseFileCategories {
  LearnCourseFileCategories({required List<LearnCourseFileCategory> items})
      : items = List.unmodifiable(items);

  final List<LearnCourseFileCategory> items;
}

/// One topic in a course's discussion list.
class LearnCourseDiscussion {
  const LearnCourseDiscussion({
    required this.title,
    required this.publisher,
    required this.publishedAtLabel,
    required this.replyCount,
    this.lastReplyAtLabel,
  });

  final String title;
  final String publisher;

  /// Display text as supplied by Learn; no timezone is inferred.
  final String publishedAtLabel;

  /// Display text as supplied by Learn; no timezone is inferred.
  final String? lastReplyAtLabel;

  final int replyCount;
}

/// Course discussions and their bounded-read completeness.
class LearnCourseDiscussions {
  LearnCourseDiscussions({
    required List<LearnCourseDiscussion> items,
    required this.coverage,
  }) : items = List.unmodifiable(items);

  final List<LearnCourseDiscussion> items;
  final ReadCoverage coverage;
}

/// Confirmation that a selected course file was saved to the requested path.
class SavedLearnCourseFile {
  const SavedLearnCourseFile({required this.bytesWritten});

  final BigInt bytesWritten;
}

/// Read-only Learn operations backed by the same Rust Client and Runtime as Auth.
class LearnClient {
  LearnClient._(this._handle);

  final native.ClientHandle _handle;

  /// Reads the authenticated course directory. A successful new directory
  /// replaces its course references and invalidates dependent selectors.
  Future<ReadResult<LearnCourseCatalog>> courses() => _sdkCall(
        () async => _learnCourseCatalogResult(await _handle.learnCourses()),
      );

  /// Reads announcements for a course from this Client's latest directory.
  Future<ReadResult<LearnAnnouncements>> announcements(
    LearnCourseReference course,
  ) =>
      _sdkCall(() async =>
          _learnAnnouncementsResult(await _handle.learnAnnouncements(
            courseReferenceId: course._id,
          )));

  /// Reads all assignment buckets for a course from the latest directory.
  /// A successful result replaces homework references from earlier reads.
  Future<ReadResult<LearnHomeworkList>> homework(
    LearnCourseReference course,
  ) =>
      _sdkCall(() async => _learnHomeworkListResult(
            await _handle.learnHomework(courseReferenceId: course._id),
          ));

  /// Reads detail for an assignment from this Client's latest homework result.
  Future<ReadResult<LearnHomeworkDetail>> homeworkDetail(
    LearnHomeworkReference homework,
  ) =>
      _sdkCall(() async => _learnHomeworkDetailResult(
            await _handle.learnHomeworkDetail(
              homeworkReferenceId: homework._id,
            ),
          ));

  /// Reads a bounded file list for a course from the latest directory.
  /// A successful result replaces prior file references.
  Future<ReadResult<LearnCourseFiles>> files(LearnCourseReference course) =>
      _sdkCall(() async => _learnCourseFilesResult(
            await _handle.learnFiles(courseReferenceId: course._id),
          ));

  /// Reads category labels for a course from the latest directory.
  Future<ReadResult<LearnCourseFileCategories>> fileCategories(
    LearnCourseReference course,
  ) =>
      _sdkCall(() async => _learnCourseFileCategoriesResult(
            await _handle.learnFileCategories(
              courseReferenceId: course._id,
            ),
          ));

  /// Reads the bounded live discussion list for a course.
  Future<ReadResult<LearnCourseDiscussions>> discussions(
    LearnCourseReference course,
  ) =>
      _sdkCall(() async => _learnCourseDiscussionsResult(
            await _handle.learnDiscussions(courseReferenceId: course._id),
          ));

  /// Saves a file from this Client's current file list to an explicitly
  /// selected destination. Rust validates the path and will not overwrite.
  Future<SavedLearnCourseFile> saveFile({
    required LearnCourseFileReference file,
    required String destinationPath,
  }) =>
      _sdkCall(() async {
        final result = await _handle.learnSaveFile(
          fileReferenceId: file._id,
          destinationPath: destinationPath,
        );
        return SavedLearnCourseFile(bytesWritten: result.bytesWritten);
      });
}

ReadResult<LearnCourseCatalog> _learnCourseCatalogResult(
  native.LearnCourseCatalogResultDto value,
) =>
    ReadResult(
      data: LearnCourseCatalog(
        semester: value.data.semester,
        courses: value.data.courses
            .map(
              (course) => LearnCourse(
                reference: LearnCourseReference._(course.referenceId),
                title: course.title,
                code: course.code,
                instructor: course.instructor,
                semester: course.semester,
              ),
            )
            .toList(growable: false),
      ),
      metadata: _readMetadata(value.metadata),
    );

ReadResult<LearnAnnouncements> _learnAnnouncementsResult(
  native.LearnAnnouncementsResultDto value,
) =>
    ReadResult(
      data: LearnAnnouncements(
        items: value.data.items
            .map(
              (announcement) => LearnAnnouncement(
                title: announcement.title,
                publisher: announcement.publisher,
                content: announcement.content,
                publishedAt: _learnUtc(announcement.publishedAtUtc),
                expiresAt: announcement.expiresAtUtc == null
                    ? null
                    : _learnUtc(announcement.expiresAtUtc!),
                isRead: announcement.read,
                isImportant: announcement.important,
                isFavorited: announcement.favorited,
                expired: announcement.expired,
              ),
            )
            .toList(growable: false),
      ),
      metadata: _readMetadata(value.metadata),
    );

ReadResult<LearnHomeworkList> _learnHomeworkListResult(
  native.LearnHomeworkListResultDto value,
) =>
    ReadResult(
      data: LearnHomeworkList(
        items: value.data.items
            .map(
              (homework) => LearnHomework(
                reference: LearnHomeworkReference._(homework.referenceId),
                title: homework.title,
                state: _learnHomeworkState(homework.state),
                dueAt: _learnUtc(homework.dueAtUtc),
                lateDueAt: homework.lateDueAtUtc == null
                    ? null
                    : _learnUtc(homework.lateDueAtUtc!),
                submittedAt: homework.submittedAtUtc == null
                    ? null
                    : _learnUtc(homework.submittedAtUtc!),
                gradedAt: homework.gradedAtUtc == null
                    ? null
                    : _learnUtc(homework.gradedAtUtc!),
                detailAvailable: homework.detailAvailable,
              ),
            )
            .toList(growable: false),
      ),
      metadata: _readMetadata(value.metadata),
    );

ReadResult<LearnHomeworkDetail> _learnHomeworkDetailResult(
  native.LearnHomeworkDetailResultDto value,
) =>
    ReadResult(
      data: LearnHomeworkDetail(
        description: value.data.description,
        answerContent: value.data.answerContent,
        submittedContent: value.data.submittedContent,
        attachments: value.data.attachments
            .map(
              (attachment) => LearnHomeworkAttachment(
                kind: _learnHomeworkAttachmentKind(attachment.kind),
                name: attachment.name,
                size: attachment.size,
              ),
            )
            .toList(growable: false),
      ),
      metadata: _readMetadata(value.metadata),
    );

ReadResult<LearnCourseFiles> _learnCourseFilesResult(
  native.LearnCourseFilesResultDto value,
) =>
    ReadResult(
      data: LearnCourseFiles(
        items: value.data.items
            .map(
              (file) => LearnCourseFile(
                reference: LearnCourseFileReference._(file.referenceId),
                title: file.title,
                suggestedFilename: file.suggestedFilename,
                description: file.description,
                sizeLabel: file.sizeLabel,
                uploadedAtLabel: file.uploadedAtLabel,
                fileType: file.fileType,
              ),
            )
            .toList(growable: false),
        coverage: _readCoverage(value.data.coverage),
      ),
      metadata: _readMetadata(value.metadata),
    );

ReadResult<LearnCourseFileCategories> _learnCourseFileCategoriesResult(
  native.LearnCourseFileCategoriesResultDto value,
) =>
    ReadResult(
      data: LearnCourseFileCategories(
        items: value.data.items
            .map((item) => LearnCourseFileCategory(title: item.title))
            .toList(growable: false),
      ),
      metadata: _readMetadata(value.metadata),
    );

ReadResult<LearnCourseDiscussions> _learnCourseDiscussionsResult(
  native.LearnCourseDiscussionsResultDto value,
) =>
    ReadResult(
      data: LearnCourseDiscussions(
        items: value.data.items
            .map(
              (discussion) => LearnCourseDiscussion(
                title: discussion.title,
                publisher: discussion.publisher,
                publishedAtLabel: discussion.publishedAtLabel,
                lastReplyAtLabel: discussion.lastReplyAtLabel,
                replyCount: discussion.replyCount,
              ),
            )
            .toList(growable: false),
        coverage: _readCoverage(value.data.coverage),
      ),
      metadata: _readMetadata(value.metadata),
    );

DateTime _learnUtc(String value) => DateTime.parse(value).toUtc();

LearnHomeworkState _learnHomeworkState(native.LearnHomeworkStateDto value) =>
    switch (value) {
      native.LearnHomeworkStateDto.pending => LearnHomeworkState.pending,
      native.LearnHomeworkStateDto.submitted => LearnHomeworkState.submitted,
      native.LearnHomeworkStateDto.graded => LearnHomeworkState.graded,
      native.LearnHomeworkStateDto.unknown => LearnHomeworkState.unknown,
    };

LearnHomeworkAttachmentKind _learnHomeworkAttachmentKind(
  native.LearnHomeworkAttachmentKindDto value,
) =>
    switch (value) {
      native.LearnHomeworkAttachmentKindDto.assignment =>
        LearnHomeworkAttachmentKind.assignment,
      native.LearnHomeworkAttachmentKindDto.answer =>
        LearnHomeworkAttachmentKind.answer,
      native.LearnHomeworkAttachmentKindDto.submitted =>
        LearnHomeworkAttachmentKind.submitted,
      native.LearnHomeworkAttachmentKindDto.grade =>
        LearnHomeworkAttachmentKind.grade,
      native.LearnHomeworkAttachmentKindDto.unknown =>
        LearnHomeworkAttachmentKind.unknown,
    };
