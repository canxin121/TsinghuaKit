//! Curated, context-bound Learn types for the public SDK.

use std::{fmt, time::Duration};

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::error::{Error, ErrorCode, Service};

const MAX_SELECTOR_LENGTH: usize = 256;
const HOMEWORK_REF_AGE: Duration = Duration::from_secs(300);
const COURSE_FILE_REF_AGE: Duration = Duration::from_secs(300);

/// An opaque course selector returned by this client's latest course read.
/// The upstream course identifier and client binding remain private.
#[derive(Clone, PartialEq, Eq)]
pub struct CourseRef {
    client_id: Uuid,
    generation: u64,
    course_id: String,
}

impl CourseRef {
    pub(crate) fn new(client_id: Uuid, generation: u64, course_id: String) -> Self {
        Self {
            client_id,
            generation,
            course_id,
        }
    }

    pub(crate) fn belongs_to(&self, client_id: Uuid, generation: u64) -> bool {
        self.client_id == client_id && self.generation == generation
    }

    pub(crate) fn course_id(&self) -> &str {
        &self.course_id
    }
}

impl fmt::Debug for CourseRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CourseRef")
            .field("selector", &"[opaque]")
            .finish()
    }
}

/// A course in one authenticated Learn observation.
#[derive(Clone, PartialEq, Eq)]
pub struct Course {
    reference: CourseRef,
    code: Option<String>,
    title: String,
    instructor: Option<String>,
    semester: Option<String>,
}

impl Course {
    pub(crate) fn new(
        reference: CourseRef,
        code: Option<String>,
        title: String,
        instructor: Option<String>,
        semester: Option<String>,
    ) -> Self {
        Self {
            reference,
            code,
            title,
            instructor,
            semester,
        }
    }

    /// Returns the opaque selector to use for reads belonging to this course.
    pub fn reference(&self) -> &CourseRef {
        &self.reference
    }

    /// Returns the display course code when Learn supplied one.
    pub fn code(&self) -> Option<&str> {
        self.code.as_deref()
    }

    /// Returns the course title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Returns the instructor name when Learn supplied one.
    pub fn instructor(&self) -> Option<&str> {
        self.instructor.as_deref()
    }

    /// Returns the semester label attached to this course when available.
    pub fn semester(&self) -> Option<&str> {
        self.semester.as_deref()
    }
}

impl fmt::Debug for Course {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Course")
            .field("reference", &self.reference)
            .field("title", &self.title)
            .field("code_present", &self.code.is_some())
            .field("instructor_present", &self.instructor.is_some())
            .field("semester_present", &self.semester.is_some())
            .finish()
    }
}

/// The current Learn course directory returned by one successful read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CourseCatalog {
    semester: String,
    courses: Vec<Course>,
}

impl CourseCatalog {
    pub(crate) fn new(semester: String, courses: Vec<Course>) -> Self {
        Self { semester, courses }
    }

    /// Returns the semester selected by the verified Learn runtime.
    pub fn semester(&self) -> &str {
        &self.semester
    }

    /// Returns all courses included in this complete directory observation.
    pub fn courses(&self) -> &[Course] {
        &self.courses
    }
}

/// One announcement associated with a Learn course.
#[derive(Clone, PartialEq, Eq)]
pub struct CourseAnnouncement {
    title: String,
    publisher: Option<String>,
    content: Option<String>,
    published_at: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
    read: Option<bool>,
    important: Option<bool>,
    favorited: Option<bool>,
    expired: bool,
}

impl CourseAnnouncement {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        title: String,
        publisher: Option<String>,
        content: Option<String>,
        published_at: DateTime<Utc>,
        expires_at: Option<DateTime<Utc>>,
        read: Option<bool>,
        important: Option<bool>,
        favorited: Option<bool>,
        expired: bool,
    ) -> Self {
        Self {
            title,
            publisher,
            content,
            published_at,
            expires_at,
            read,
            important,
            favorited,
            expired,
        }
    }

    /// Returns the announcement title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Returns the publisher when Learn supplied one.
    pub fn publisher(&self) -> Option<&str> {
        self.publisher.as_deref()
    }

    /// Returns the plain-text content when available from this result.
    pub fn content(&self) -> Option<&str> {
        self.content.as_deref()
    }

    /// Returns the publication time in UTC.
    pub fn published_at(&self) -> DateTime<Utc> {
        self.published_at
    }

    /// Returns the expiration time when Learn supplied one.
    pub fn expires_at(&self) -> Option<DateTime<Utc>> {
        self.expires_at
    }

    /// Returns the current-account read flag when Learn supplied it.
    pub fn is_read(&self) -> Option<bool> {
        self.read
    }

    /// Returns the important flag when Learn supplied it.
    pub fn is_important(&self) -> Option<bool> {
        self.important
    }

    /// Returns the current-account favorite flag when Learn supplied it.
    pub fn is_favorited(&self) -> Option<bool> {
        self.favorited
    }

    /// Returns whether Learn marked the announcement expired.
    pub fn is_expired(&self) -> bool {
        self.expired
    }
}

impl fmt::Debug for CourseAnnouncement {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CourseAnnouncement")
            .field("published_at", &self.published_at)
            .field("expires_at", &self.expires_at)
            .field("content_present", &self.content.is_some())
            .field("read", &self.read)
            .field("important", &self.important)
            .field("favorited", &self.favorited)
            .field("expired", &self.expired)
            .finish()
    }
}

/// Active and expired announcements from one course read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CourseAnnouncements {
    items: Vec<CourseAnnouncement>,
}

impl CourseAnnouncements {
    pub(crate) fn new(items: Vec<CourseAnnouncement>) -> Self {
        Self { items }
    }

    /// Returns all announcements included in this service observation.
    pub fn items(&self) -> &[CourseAnnouncement] {
        &self.items
    }
}

/// A short-lived opaque selector for one course file from this client's most
/// recent file-list observation. The upstream file and course IDs stay private.
#[derive(Clone, PartialEq, Eq)]
pub struct CourseFileRef {
    client_id: Uuid,
    course_generation: u64,
    file_generation: u64,
    course_id: String,
    file_id: String,
    selected_at: std::time::Instant,
}

impl CourseFileRef {
    pub(crate) fn new(
        client_id: Uuid,
        course_generation: u64,
        file_generation: u64,
        course_id: String,
        file_id: String,
    ) -> Self {
        Self {
            client_id,
            course_generation,
            file_generation,
            course_id,
            file_id,
            selected_at: std::time::Instant::now(),
        }
    }

    pub(crate) fn belongs_to(
        &self,
        client_id: Uuid,
        course_generation: u64,
        file_generation: u64,
    ) -> bool {
        self.client_id == client_id
            && self.course_generation == course_generation
            && self.file_generation == file_generation
            && self.selected_at.elapsed() < COURSE_FILE_REF_AGE
    }

    pub(crate) fn course_id(&self) -> &str {
        &self.course_id
    }

    pub(crate) fn file_id(&self) -> &str {
        &self.file_id
    }
}

impl fmt::Debug for CourseFileRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CourseFileRef")
            .field("selector", &"[opaque]")
            .finish()
    }
}

/// One file entry from a verified, bounded course-file read.
#[derive(Clone, PartialEq, Eq)]
pub struct CourseFile {
    reference: CourseFileRef,
    title: String,
    suggested_filename: String,
    description: Option<String>,
    size_label: Option<String>,
    uploaded_at_label: Option<String>,
    file_type: Option<String>,
}

impl CourseFile {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        reference: CourseFileRef,
        title: String,
        suggested_filename: String,
        description: Option<String>,
        size_label: Option<String>,
        uploaded_at_label: Option<String>,
        file_type: Option<String>,
    ) -> Self {
        Self {
            reference,
            title,
            suggested_filename,
            description,
            size_label,
            uploaded_at_label,
            file_type,
        }
    }

    /// Returns the opaque selector to pass to an explicit save operation.
    pub fn reference(&self) -> &CourseFileRef {
        &self.reference
    }

    /// Returns the display title reported by Learn.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Returns a sanitized filename suggestion; callers still choose the save path.
    pub fn suggested_filename(&self) -> &str {
        &self.suggested_filename
    }

    /// Returns the description when Learn supplied one.
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// Returns the source's display size label when available.
    pub fn size_label(&self) -> Option<&str> {
        self.size_label.as_deref()
    }

    /// Returns the source's displayed upload time without inferring a timezone.
    pub fn uploaded_at_label(&self) -> Option<&str> {
        self.uploaded_at_label.as_deref()
    }

    /// Returns the media/file-type label when Learn supplied one.
    pub fn file_type(&self) -> Option<&str> {
        self.file_type.as_deref()
    }
}

impl fmt::Debug for CourseFile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CourseFile")
            .field("reference", &self.reference)
            .field("title_present", &!self.title.is_empty())
            .field("description_present", &self.description.is_some())
            .field("size_present", &self.size_label.is_some())
            .field("uploaded_at_present", &self.uploaded_at_label.is_some())
            .field("file_type_present", &self.file_type.is_some())
            .finish()
    }
}

/// Course files returned by one live Learn observation.
#[derive(Clone, PartialEq, Eq)]
pub struct CourseFiles {
    items: Vec<CourseFile>,
}

impl CourseFiles {
    pub(crate) fn new(items: Vec<CourseFile>) -> Self {
        Self { items }
    }

    /// Returns the file entries included in this observation.
    pub fn items(&self) -> &[CourseFile] {
        &self.items
    }
}

impl fmt::Debug for CourseFiles {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CourseFiles")
            .field("item_count", &self.items.len())
            .finish()
    }
}

/// One course-file category label. Category selectors remain internal because
/// the public API currently exposes no category-specific file query.
#[derive(Clone, PartialEq, Eq)]
pub struct CourseFileCategory {
    title: String,
}

impl CourseFileCategory {
    pub(crate) fn new(title: String) -> Self {
        Self { title }
    }

    /// Returns the category's display label.
    pub fn title(&self) -> &str {
        &self.title
    }
}

impl fmt::Debug for CourseFileCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CourseFileCategory")
            .field("title_present", &!self.title.is_empty())
            .finish()
    }
}

/// Categories from one course's verified category endpoint.
#[derive(Clone, PartialEq, Eq)]
pub struct CourseFileCategories {
    items: Vec<CourseFileCategory>,
}

impl CourseFileCategories {
    pub(crate) fn new(items: Vec<CourseFileCategory>) -> Self {
        Self { items }
    }

    /// Returns the categories included in this observation.
    pub fn items(&self) -> &[CourseFileCategory] {
        &self.items
    }
}

impl fmt::Debug for CourseFileCategories {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CourseFileCategories")
            .field("item_count", &self.items.len())
            .finish()
    }
}

/// Confirmation returned after a course file was safely saved to the
/// caller-selected destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SavedCourseFile {
    bytes_written: u64,
}

impl SavedCourseFile {
    pub(crate) fn new(bytes_written: u64) -> Self {
        Self { bytes_written }
    }

    /// Returns the number of bytes written to the explicitly selected path.
    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }
}

/// One discussion topic in a course's live discussion-list observation.
#[derive(Clone, PartialEq, Eq)]
pub struct CourseDiscussion {
    title: String,
    publisher: String,
    published_at_label: String,
    last_reply_at_label: Option<String>,
    reply_count: u32,
}

impl CourseDiscussion {
    pub(crate) fn new(
        title: String,
        publisher: String,
        published_at_label: String,
        last_reply_at_label: Option<String>,
        reply_count: u32,
    ) -> Self {
        Self {
            title,
            publisher,
            published_at_label,
            last_reply_at_label,
            reply_count,
        }
    }

    /// Returns the discussion title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Returns the display name of the publisher.
    pub fn publisher(&self) -> &str {
        &self.publisher
    }

    /// Returns the source's displayed publication time without inferring a timezone.
    pub fn published_at_label(&self) -> &str {
        &self.published_at_label
    }

    /// Returns the source's displayed last-reply time when available.
    pub fn last_reply_at_label(&self) -> Option<&str> {
        self.last_reply_at_label.as_deref()
    }

    /// Returns the verified reply count.
    pub fn reply_count(&self) -> u32 {
        self.reply_count
    }
}

impl fmt::Debug for CourseDiscussion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CourseDiscussion")
            .field("title_present", &!self.title.is_empty())
            .field("publisher_present", &!self.publisher.is_empty())
            .field("published_at_present", &!self.published_at_label.is_empty())
            .field("last_reply_at_present", &self.last_reply_at_label.is_some())
            .field("reply_count", &self.reply_count)
            .finish()
    }
}

/// Discussion topics returned for one course.
#[derive(Clone, PartialEq, Eq)]
pub struct CourseDiscussions {
    items: Vec<CourseDiscussion>,
}

impl CourseDiscussions {
    pub(crate) fn new(items: Vec<CourseDiscussion>) -> Self {
        Self { items }
    }

    /// Returns the topics included in this observation.
    pub fn items(&self) -> &[CourseDiscussion] {
        &self.items
    }
}

impl fmt::Debug for CourseDiscussions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CourseDiscussions")
            .field("item_count", &self.items.len())
            .finish()
    }
}

/// The business state of a student assignment in Learn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum HomeworkState {
    /// Assignment is awaiting a submission.
    Pending,
    /// Assignment has been submitted and is not yet graded.
    Submitted,
    /// Assignment has been graded.
    Graded,
}

/// A short-lived selector for the detail of one Learn assignment.
///
/// A reference is valid only for its creating Client, latest course directory,
/// and latest homework list. Its upstream selector and IDs are never exposed.
#[derive(Clone, PartialEq, Eq)]
pub struct HomeworkRef {
    client_id: Uuid,
    course_generation: u64,
    homework_generation: u64,
    course_id: String,
    selector: String,
    detail_available: bool,
    selected_at: std::time::Instant,
}

impl HomeworkRef {
    pub(crate) fn new(
        client_id: Uuid,
        course_generation: u64,
        homework_generation: u64,
        course_id: String,
        selector: String,
        detail_available: bool,
    ) -> Self {
        Self::new_at(
            client_id,
            course_generation,
            homework_generation,
            course_id,
            selector,
            detail_available,
            std::time::Instant::now(),
        )
    }

    pub(crate) fn new_at(
        client_id: Uuid,
        course_generation: u64,
        homework_generation: u64,
        course_id: String,
        selector: String,
        detail_available: bool,
        selected_at: std::time::Instant,
    ) -> Self {
        Self {
            client_id,
            course_generation,
            homework_generation,
            course_id,
            selector,
            detail_available,
            selected_at,
        }
    }

    pub(crate) fn belongs_to(
        &self,
        client_id: Uuid,
        course_generation: u64,
        homework_generation: u64,
    ) -> bool {
        self.client_id == client_id
            && self.course_generation == course_generation
            && self.homework_generation == homework_generation
            && self.selected_at.elapsed() < HOMEWORK_REF_AGE
    }

    pub(crate) fn selector(&self) -> &str {
        &self.selector
    }

    /// Returns whether Learn supplied the identifiers required to read detail.
    pub fn detail_available(&self) -> bool {
        self.detail_available
    }
}

impl fmt::Debug for HomeworkRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HomeworkRef")
            .field("selector", &"[opaque]")
            .field("detail_available", &self.detail_available)
            .finish()
    }
}

/// One assignment from a complete, live Learn homework read.
#[derive(Clone, PartialEq, Eq)]
pub struct Homework {
    reference: HomeworkRef,
    title: String,
    state: HomeworkState,
    due_at: DateTime<Utc>,
    late_due_at: Option<DateTime<Utc>>,
    submitted_at: Option<DateTime<Utc>>,
    graded_at: Option<DateTime<Utc>>,
}

impl Homework {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        reference: HomeworkRef,
        title: String,
        state: HomeworkState,
        due_at: DateTime<Utc>,
        late_due_at: Option<DateTime<Utc>>,
        submitted_at: Option<DateTime<Utc>>,
        graded_at: Option<DateTime<Utc>>,
    ) -> Self {
        Self {
            reference,
            title,
            state,
            due_at,
            late_due_at,
            submitted_at,
            graded_at,
        }
    }

    /// Returns the short-lived opaque selector for this homework item.
    pub fn reference(&self) -> &HomeworkRef {
        &self.reference
    }

    /// Returns the assignment title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Returns the normalized assignment state.
    pub fn state(&self) -> HomeworkState {
        self.state
    }

    /// Returns the required deadline in UTC.
    pub fn due_at(&self) -> DateTime<Utc> {
        self.due_at
    }

    /// Returns the later deadline, if the service supplied one.
    pub fn late_due_at(&self) -> Option<DateTime<Utc>> {
        self.late_due_at
    }

    /// Returns the submission time when the assignment was submitted.
    pub fn submitted_at(&self) -> Option<DateTime<Utc>> {
        self.submitted_at
    }

    /// Returns the grading time when the assignment was graded.
    pub fn graded_at(&self) -> Option<DateTime<Utc>> {
        self.graded_at
    }

    /// Returns whether a detail read is available for this assignment.
    pub fn detail_available(&self) -> bool {
        self.reference.detail_available()
    }
}

impl fmt::Debug for Homework {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Homework")
            .field("reference", &self.reference)
            .field("title_present", &!self.title.is_empty())
            .field("state", &self.state)
            .field("due_at", &self.due_at)
            .field("late_due_at_present", &self.late_due_at.is_some())
            .field("submitted_at_present", &self.submitted_at.is_some())
            .field("graded_at_present", &self.graded_at.is_some())
            .finish()
    }
}

/// A complete homework list for one course.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeworkList {
    items: Vec<Homework>,
}

impl HomeworkList {
    pub(crate) fn new(items: Vec<Homework>) -> Self {
        Self { items }
    }

    /// Returns every assignment included in this complete service response.
    pub fn items(&self) -> &[Homework] {
        &self.items
    }
}

/// Stable kind for a read-only attachment listed on an assignment detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum HomeworkAttachmentKind {
    /// File attached to the assignment prompt.
    Assignment,
    /// File attached to a reference answer.
    Answer,
    /// File submitted by the student.
    Submitted,
    /// File attached to grading feedback.
    Grade,
}

/// Metadata for an attachment. Download and preview URLs are not public.
#[derive(Clone, PartialEq, Eq)]
pub struct HomeworkAttachment {
    kind: HomeworkAttachmentKind,
    name: String,
    size: Option<String>,
}

impl HomeworkAttachment {
    pub(crate) fn new(kind: HomeworkAttachmentKind, name: String, size: Option<String>) -> Self {
        Self { kind, name, size }
    }

    /// Returns the attachment's assignment section.
    pub fn kind(&self) -> HomeworkAttachmentKind {
        self.kind
    }

    /// Returns its display filename.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the size label when Learn supplied one.
    pub fn size(&self) -> Option<&str> {
        self.size.as_deref()
    }
}

impl fmt::Debug for HomeworkAttachment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HomeworkAttachment")
            .field("kind", &self.kind)
            .field("name_present", &!self.name.is_empty())
            .field("size_present", &self.size.is_some())
            .finish()
    }
}

/// Read-only detail fields for a homework item selected from the latest list.
#[derive(Clone, PartialEq, Eq)]
pub struct HomeworkDetail {
    description: Option<String>,
    answer_content: Option<String>,
    submitted_content: Option<String>,
    attachments: Vec<HomeworkAttachment>,
}

impl HomeworkDetail {
    pub(crate) fn new(
        description: Option<String>,
        answer_content: Option<String>,
        submitted_content: Option<String>,
        attachments: Vec<HomeworkAttachment>,
    ) -> Self {
        Self {
            description,
            answer_content,
            submitted_content,
            attachments,
        }
    }

    /// Returns the assignment description when present.
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// Returns reference-answer text when the service exposes it.
    pub fn answer_content(&self) -> Option<&str> {
        self.answer_content.as_deref()
    }

    /// Returns the student's submitted text when the service exposes it.
    pub fn submitted_content(&self) -> Option<&str> {
        self.submitted_content.as_deref()
    }

    /// Returns attachment metadata without exposing protocol URLs.
    pub fn attachments(&self) -> &[HomeworkAttachment] {
        &self.attachments
    }
}

impl fmt::Debug for HomeworkDetail {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HomeworkDetail")
            .field("description_present", &self.description.is_some())
            .field("answer_content_present", &self.answer_content.is_some())
            .field(
                "submitted_content_present",
                &self.submitted_content.is_some(),
            )
            .field("attachment_count", &self.attachments.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_refactor_learn_homework_ref_expires_after_five_minutes() {
        let client_id = Uuid::new_v4();
        let mut reference = HomeworkRef::new(
            client_id,
            3,
            7,
            "private-course-id".into(),
            "private-assignment-selector".into(),
            true,
        );
        reference.selected_at -= Duration::from_secs(301);

        assert!(!reference.belongs_to(client_id, 3, 7));
    }

    #[test]
    fn backend_refactor_learn_file_ref_is_client_and_observation_bound() {
        let client_id = Uuid::new_v4();
        let reference = CourseFileRef::new(
            client_id,
            3,
            7,
            "private-course-id".into(),
            "private-file-selector".into(),
        );

        assert!(reference.belongs_to(client_id, 3, 7));
        assert!(!reference.belongs_to(Uuid::new_v4(), 3, 7));
        assert!(!reference.belongs_to(client_id, 4, 7));
        assert!(!reference.belongs_to(client_id, 3, 8));

        let mut expired = reference.clone();
        expired.selected_at -= Duration::from_secs(301);
        assert!(!expired.belongs_to(client_id, 3, 7));

        let debug = format!("{reference:?}");
        assert!(!debug.contains("private-course-id"));
        assert!(!debug.contains("private-file-selector"));
    }

    #[test]
    fn backend_refactor_learn_file_and_discussion_debug_redact_text_payloads() {
        let file = CourseFile::new(
            CourseFileRef::new(
                Uuid::new_v4(),
                3,
                7,
                "private-course-id".into(),
                "private-file-selector".into(),
            ),
            "private file title".into(),
            "private file title.pdf".into(),
            Some("private file description".into()),
            Some("2 MB".into()),
            Some("2026-09-25".into()),
            Some("pdf".into()),
        );
        let discussion = CourseDiscussion::new(
            "private discussion title".into(),
            "private publisher".into(),
            "2026-09-25 09:00".into(),
            None,
            3,
        );
        let debug = format!("{file:?} {discussion:?}");

        for private_value in [
            "private-course-id",
            "private-file-selector",
            "private file title",
            "private file description",
            "private discussion title",
            "private publisher",
        ] {
            assert!(!debug.contains(private_value));
        }
    }
}

pub(crate) fn parse_required_time(value: &str) -> Result<DateTime<Utc>, Error> {
    DateTime::parse_from_rfc3339(value)
        .map(|parsed| parsed.with_timezone(&Utc))
        .map_err(|_| Error::new(Service::Learn, ErrorCode::InvalidResponse))
}

pub(crate) fn parse_optional_time(value: Option<&str>) -> Result<Option<DateTime<Utc>>, Error> {
    value.map(parse_required_time).transpose()
}

pub(crate) fn valid_selector(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= MAX_SELECTOR_LENGTH
        && !value.chars().any(char::is_control)
}
