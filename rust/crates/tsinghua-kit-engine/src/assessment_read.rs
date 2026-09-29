//! The questionnaire list, its form, and the submission of that form
//! (`教学评估`).
//!
//! The evaluation service is a legacy JSP application on its own campus host,
//! reached through the WebVPN mapping bound to selector
//! [`ASSESSMENT_WEBVPN_TARGET`].  One GET returns the list of courses the
//! student can evaluate; each row carries the course name, whether that course
//! has already been evaluated, and an inline JavaScript call that names the
//! form route for that row.
//!
//! Three service behaviours shape this module:
//!
//! * The service answers a 200 HTML page containing
//!   `对不起，现在不是填写问卷时间` while the questionnaire window is closed.
//!   That is an explicit "not open" answer, not an empty list, and it is
//!   reported as its own failure category so a caller cannot present a closed
//!   window as "nothing left to do".
//! * An empty list is also a failure.  The public reference throws on it, and
//!   the reasoning holds here: this page only exists during the window, so
//!   zero rows means the page did not render, not that every course is done.
//! * The list is a positional legacy table.  [`campus_html`] deliberately
//!   refuses positional indexing, so this module owns that decision and pays
//!   for it with a structural anchor: every accepted row must carry the inline
//!   `Body('…')` call in its action cell, and the route that call names must be
//!   a plain same-mapping path.  A page whose columns moved therefore fails
//!   instead of reporting one column's text as another column's value.
//!
//! The form route itself never leaves the engine.  A row exposes an opaque
//! [`AssessmentRef`] that only the adapter instance which produced it can
//! resolve, so a later stage can fetch and submit a form without a caller ever
//! holding — or forging — a service path.
//!
//! # The form and its submission
//!
//! [`AssessmentAdapter::read_form`] parses the page a row's route names into a
//! [`AssessmentForm`], and [`AssessmentAdapter::submit_form`] posts it.
//!
//! Two properties make the write safe to expose, and both are enforced by the
//! types rather than by a caller's discipline:
//!
//! * **The submitted body cannot be forged.**  A [`AssessmentForm`] is
//!   produced only by the parser, which reads it from the service's own form
//!   page and refuses a page that does not carry both the transaction form and
//!   the one-item-per-non-score-field structure the service renders.  A caller
//!   chooses a *score* for each question through the bounded setter on
//!   [`AssessmentPerson::set_question_score`]; every other name/value pair — the
//!   hidden transaction state above all — is the service's own text and cannot
//!   be supplied or edited from outside this module.
//! * **The submitted route cannot be forged either.**  The submit path is a
//!   module constant on the same WebVPN mapping as the list and the form, and
//!   a form carries the generation of the list read that named its route, so a
//!   form parsed before a newer list read is refused instead of being posted
//!   against state it no longer describes.
//!
//! A submission is one-shot.  The service answers JSON whose `result` field
//! must be exactly `success`; that is the only accepted completion.  Any other
//! answer — a reworded response, an HTML error page, a transport failure after
//! dispatch — is reported as an unconfirmed outcome, and this module never
//! replays the POST to find out what happened.
//!
//! This module reimplements the contract from public reference behavior.  It
//! does not copy source, fixtures, or assets.

use std::{
    fmt,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use reqwest::{
    StatusCode, Url,
    header::{CONTENT_TYPE, LOCATION},
};
use thiserror::Error;

use crate::campus_html::{self, PageClass, RawElement, ScanError};
use crate::transport::{CampusHttpTransport, TransportError};

/// The evaluation host's WebVPN roaming selector.
pub const ASSESSMENT_WEBVPN_TARGET: &str = "0D8B99BA23FD2BA22428D9C8AA0AB508";

/// The questionnaire list endpoint behind the selector above.  It takes no
/// query parameters.
pub const ASSESSMENT_LIST_PATH: &str = "/jxpg/f/jxpg/wj/xs/pgkcList";

/// The service's explicit "the questionnaire window is closed" answer.
pub const ASSESSMENT_NOT_OPEN_MARKER: &str = "对不起，现在不是填写问卷时间";

/// The endpoint that stores one filled-in questionnaire.  Like the list and
/// the form it is a module constant on the same mapping, so no caller value
/// ever chooses where a submission goes.
pub const ASSESSMENT_SUBMIT_PATH: &str = "/jxpg/b/jxpg/pgjg/xs/zancunjs";

/// The `result` value the service writes into its JSON answer for a
/// submission it accepted.
const SUBMIT_SUCCESS_RESULT: &str = "success";

/// The `name` each hidden transaction input must carry.
///
/// The service renders several hidden inputs into the form and submits them
/// back verbatim; one of them is the transaction identifier that makes the
/// submission belong to the page it was read from.  This module does not
/// interpret their values, but it does require the identifier to be present,
/// because a form page without it is not the page this endpoint expects.
const SUBMIT_TOKEN_NAME: &str = "wjid";

/// The container holding the questionnaire's transaction state.
const BASICS_CONTAINER_ID: &str = "xswjtxFormid";
/// The element holding the overall written comment, whose text is the editable
/// value for the overall comment field.
const OVERALL_COMMENT_ELEMENT_ID: &str = "kcpgjgDtos[0].jtjy";
/// The endpoint expects that comment under this name, which is the element id
/// with the container path the service uses for the overall record.
const OVERALL_COMMENT_INPUT_NAME: &str = "kcpgjgDtos[0].jtjy";
/// The input holding the overall score.
const OVERALL_SCORE_ID: &str = "kcpjfs";
/// The container holding one person's question table.
const PERSON_TABLE_CONTAINER_CLASS: &str = "tab-pane";

/// The service's own bound for one questionnaire answer: a score is an
/// integer from 1 to 7, the same range the public reference enforces before
/// posting.
pub const ASSESSMENT_MIN_SCORE: u32 = 1;
/// See [`ASSESSMENT_MIN_SCORE`].
pub const ASSESSMENT_MAX_SCORE: u32 = 7;

/// The list cell holding the course name, as the legacy table lays it out.
const NAME_CELL: usize = 5;
/// The list cell holding the evaluated flag.
const EVALUATED_CELL: usize = 9;
/// The list cell holding the action that names this row's form route.
const ACTION_CELL: usize = 11;
/// The lowest cell count an accepted row can have, since the action cell is
/// the last one this module reads.
const MIN_ROW_CELLS: usize = ACTION_CELL + 1;
/// The service writes exactly this text in the evaluated cell for a course that
/// has been evaluated.  Any other text means "not evaluated", which is the
/// observed reference behaviour and lets the service reword the negative case.
const EVALUATED_TEXT: &str = "是";
/// The inline call that carries the form route in the action cell.
const ACTION_CALL_OPEN: &str = "Body('";
/// The delimiter that ends the route inside that call.
const ACTION_CALL_CLOSE: &str = "') })";

const MAX_HTML_BYTES: usize = 4 * 1024 * 1024;
const MAX_ITEMS: usize = 2048;
const MAX_NAME_CHARS: usize = 256;
const MAX_ROUTE_CHARS: usize = 512;
/// Upper bound on the fields one parsed form may carry.
const MAX_FORM_FIELDS: usize = 4096;
/// Upper bound on the people one form may list.
const MAX_FORM_PERSONS: usize = 256;
/// Upper bound on the questions one person may be asked.
const MAX_FORM_QUESTIONS: usize = 256;
/// Upper bound on the length of any single name or value the form carries.
const MAX_FIELD_CHARS: usize = 4096;

/// The whole submission body is bounded too: a form that would render a
/// larger body is refused rather than sent, because a submission is not
/// replayable and a truncated one is worse than none.
const MAX_SUBMIT_BODY_BYTES: usize = 512 * 1024;
/// The submission answer is a small JSON object, so the read is bounded too.
const MAX_SUBMIT_RESPONSE_BYTES: usize = 64 * 1024;

static NEXT_ASSESSMENT_ADAPTER_BINDING: AtomicU64 = AtomicU64::new(1);

/// The methods this profile issues.  The list and the form are GETs; only the
/// fixed submit endpoint is a POST, and it is named by a module constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssessmentMethod {
    Get,
    Post,
}

/// The observed evaluation operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssessmentOperation {
    ReadList,
    ReadForm,
    SubmitForm,
}

/// An evaluation operation requires an already established INFO/WebVPN
/// session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssessmentSessionPrerequisite {
    ExistingInfoWebVpnSession,
}

/// A transport-neutral request plan: path and query only, never an absolute
/// WebVPN mapping, Cookie, or account value.
///
/// Every field is a module constant.  The one route in this module that is not
/// a constant — the form page — is never stored here: it stays inside the
/// adapter and is passed beside the plan, so it cannot be smuggled into the
/// plan type a caller can name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssessmentRequestPlan {
    pub operation: AssessmentOperation,
    pub method: AssessmentMethod,
    pub path: &'static str,
    pub query: &'static str,
    pub webvpn_target: &'static str,
    pub session_prerequisite: AssessmentSessionPrerequisite,
}

impl AssessmentRequestPlan {
    /// Returns the serialized query without the leading `?`.
    pub fn query_string(&self) -> &str {
        self.query
    }
}

/// Fixed evaluation route profile.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AssessmentProfile;

impl AssessmentProfile {
    pub const fn standard() -> Self {
        Self
    }

    pub const fn roaming_selector(self) -> &'static str {
        ASSESSMENT_WEBVPN_TARGET
    }

    pub fn list_request(self) -> AssessmentRequestPlan {
        AssessmentRequestPlan {
            operation: AssessmentOperation::ReadList,
            method: AssessmentMethod::Get,
            path: ASSESSMENT_LIST_PATH,
            query: "",
            webvpn_target: ASSESSMENT_WEBVPN_TARGET,
            session_prerequisite: AssessmentSessionPrerequisite::ExistingInfoWebVpnSession,
        }
    }

    /// The plan that fetches the form page one row names.  The plan itself is
    /// a constant; the route travels beside it and is re-validated by the
    /// adapter before every request.
    pub fn form_request(self) -> AssessmentRequestPlan {
        AssessmentRequestPlan {
            operation: AssessmentOperation::ReadForm,
            method: AssessmentMethod::Get,
            path: "",
            query: "",
            webvpn_target: ASSESSMENT_WEBVPN_TARGET,
            session_prerequisite: AssessmentSessionPrerequisite::ExistingInfoWebVpnSession,
        }
    }

    /// The one fixed plan that stores a filled-in questionnaire.
    pub fn submit_request(self) -> AssessmentRequestPlan {
        AssessmentRequestPlan {
            operation: AssessmentOperation::SubmitForm,
            method: AssessmentMethod::Post,
            path: ASSESSMENT_SUBMIT_PATH,
            query: "",
            webvpn_target: ASSESSMENT_WEBVPN_TARGET,
            session_prerequisite: AssessmentSessionPrerequisite::ExistingInfoWebVpnSession,
        }
    }
}

/// One parsed row, before the adapter attaches its opaque reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssessmentRow {
    /// The course name exactly as the service rendered it, bounded in length.
    pub name: String,
    /// True when the service marked this course as already evaluated.
    pub evaluated: bool,
    /// The form route this row's action cell named.
    pub route: String,
}

/// An opaque reference to one row's evaluation form.
///
/// The route stays inside the adapter that produced it.  A reference from a
/// different adapter instance, from a superseded list, or beyond the range of
/// the list it came from does not resolve at all, so a caller cannot point a
/// later stage at a route of its own choosing.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct AssessmentRef {
    adapter_binding: u64,
    generation: u64,
    index: u32,
}

impl AssessmentRef {
    /// Binds one row reference to the adapter and list generation that
    /// produced it.  Crate-visible only: a reference always originates from a
    /// list read, so no public path in this crate can mint one.
    pub(crate) fn new(adapter_binding: u64, generation: u64, index: u32) -> Self {
        Self {
            adapter_binding,
            generation,
            index,
        }
    }

    /// The position of this row inside the list that produced it.
    pub fn index(&self) -> u32 {
        self.index
    }
}

impl fmt::Debug for AssessmentRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AssessmentRef")
            .field("index", &self.index)
            .finish()
    }
}

/// One course awaiting (or already given) a teaching evaluation.
#[derive(Debug, Clone, PartialEq)]
pub struct AssessmentItem {
    /// The course name exactly as the service rendered it.
    pub name: String,
    /// True when the service marked this course as already evaluated.
    pub evaluated: bool,
    /// Opaque handle for this row's form route.
    pub reference: AssessmentRef,
}

/// A validated questionnaire list.
#[derive(Debug, Clone, PartialEq)]
pub struct AssessmentList {
    pub items: Vec<AssessmentItem>,
}

impl AssessmentList {
    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// The routes named by one list read, matched to the rows by position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssessmentListRows {
    pub rows: Vec<AssessmentRow>,
}

/// One submission field of a questionnaire, as the service rendered it.
///
/// The name and value are the service's own submission state.  Nothing here is
/// caller-supplied: the parser is the only constructor, and only a comment the
/// caller was shown may be rewritten.  The type is crate-visible only — no
/// public path in this crate names it, so a caller can never hold one.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct AssessmentFormField {
    name: String,
    value: String,
}

impl AssessmentFormField {
    /// The current submission value.
    pub(crate) fn value(&self) -> &str {
        &self.value
    }

    /// Replaces the submission value, from this module only.
    fn write(&mut self, value: &str) -> Result<(), AssessmentInputError> {
        if !valid_field_component(value) {
            return Err(AssessmentInputError::InvalidValue);
        }
        self.value = value.to_owned();
        Ok(())
    }
}

impl fmt::Debug for AssessmentFormField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AssessmentFormField")
            .field("has_value", &(!self.value.is_empty()))
            .finish()
    }
}

/// One scored question of one person's questionnaire.
///
/// The score input the caller chooses is held apart from the field group the
/// service submitted with it, so no score can be written into a name the
/// service owns and no service value can be replaced by a score.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct AssessedQuestion {
    text: String,
    score: u32,
    score_name: String,
    others: Vec<AssessmentFormField>,
    suggestion: Option<AssessmentFormField>,
}

impl AssessedQuestion {
    /// The pairs this question contributes, in the order the service submitted
    /// them: its companion values, then the comment, then the score.
    fn pairs(&self) -> Vec<(&str, String)> {
        let mut pairs: Vec<(&str, String)> = self
            .others
            .iter()
            .map(|field| (field.name.as_str(), field.value().to_owned()))
            .collect();
        if let Some(suggestion) = self.suggestion.as_ref() {
            pairs.push((suggestion.name.as_str(), suggestion.value().to_owned()));
        }
        pairs.push((self.score_name.as_str(), self.score.to_string()));
        pairs
    }
}

impl fmt::Debug for AssessedQuestion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AssessedQuestion")
            .field("question", &self.text)
            .field("score", &self.score)
            .field("companion_field_count", &(self.others.len() + 1))
            .finish()
    }
}

/// One person the questionnaire asks about: a teacher or a teaching
/// assistant.
///
/// Questions are addressed by position.  A position is only ever obtained from
/// [`AssessmentPerson::question_count`] on this same person, so no caller can
/// name a question the service did not render.
#[derive(Clone, PartialEq, Eq)]
pub struct AssessmentPerson {
    name: String,
    role: AssessmentPersonRole,
    questions: Vec<AssessedQuestion>,
}

impl AssessmentPerson {
    /// The person's name as the service rendered it.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether this person is a teacher or a teaching assistant.
    pub fn role(&self) -> AssessmentPersonRole {
        self.role
    }

    /// How many questions this person was asked.
    pub fn question_count(&self) -> usize {
        self.questions.len()
    }

    /// One question's display text.
    pub fn question(&self, index: usize) -> Option<&str> {
        self.questions
            .get(index)
            .map(|question| question.text.as_str())
    }

    /// One question's currently selected score.
    pub fn question_score(&self, index: usize) -> Option<u32> {
        self.questions.get(index).map(|question| question.score)
    }

    /// One question's current comment, when the service rendered one for it.
    pub fn question_suggestion(&self, index: usize) -> Option<&str> {
        self.questions
            .get(index)?
            .suggestion
            .as_ref()
            .map(AssessmentFormField::value)
    }

    /// How many name/value pairs one question submits, the score included.
    pub fn question_field_count(&self, index: usize) -> Option<usize> {
        self.questions
            .get(index)
            .map(|question| question.others.len() + usize::from(question.suggestion.is_some()) + 1)
    }

    /// Selects one score, refusing anything outside the service's range.
    pub fn set_question_score(
        &mut self,
        index: usize,
        score: u32,
    ) -> Result<(), AssessmentInputError> {
        if !(ASSESSMENT_MIN_SCORE..=ASSESSMENT_MAX_SCORE).contains(&score) {
            return Err(AssessmentInputError::ScoreOutOfRange);
        }
        let Some(question) = self.questions.get_mut(index) else {
            return Err(AssessmentInputError::UnknownQuestion);
        };
        question.score = score;
        Ok(())
    }

    /// Sets one question's comment.
    pub fn set_question_suggestion(
        &mut self,
        index: usize,
        value: &str,
    ) -> Result<(), AssessmentInputError> {
        let Some(question) = self.questions.get_mut(index) else {
            return Err(AssessmentInputError::UnknownQuestion);
        };
        match question.suggestion.as_mut() {
            Some(field) => field.write(value),
            None => Err(AssessmentInputError::NoSuggestionField),
        }
    }

    /// This person's comment, which the service renders once per question.
    ///
    /// It is read from the first question that carries one and written to every
    /// question that does, matching how the page's own "more suggestions for
    /// this teacher" box behaves.
    pub fn suggestion(&self) -> Option<&str> {
        self.questions
            .iter()
            .find_map(|question| question.suggestion.as_ref())
            .map(AssessmentFormField::value)
    }

    /// Sets this person's comment on every question that carries one.
    pub fn set_suggestion(&mut self, value: &str) -> Result<(), AssessmentInputError> {
        if !valid_field_component(value) {
            return Err(AssessmentInputError::InvalidValue);
        }
        let mut written = false;
        for question in &mut self.questions {
            if let Some(field) = question.suggestion.as_mut() {
                field.write(value)?;
                written = true;
            }
        }
        if written {
            Ok(())
        } else {
            Err(AssessmentInputError::NoSuggestionField)
        }
    }

    /// Selects one score for every question this person was asked.
    pub fn set_all_scores(&mut self, score: u32) -> Result<(), AssessmentInputError> {
        if !(ASSESSMENT_MIN_SCORE..=ASSESSMENT_MAX_SCORE).contains(&score) {
            return Err(AssessmentInputError::ScoreOutOfRange);
        }
        for question in &mut self.questions {
            question.score = score;
        }
        Ok(())
    }

    /// The pairs this person contributes to a submission.
    fn submission_pairs(&self) -> Vec<(&str, String)> {
        let mut pairs: Vec<(&str, String)> = Vec::with_capacity(self.questions.len() * 4);
        for question in &self.questions {
            pairs.extend(question.pairs());
        }
        pairs
    }
}

impl fmt::Debug for AssessmentPerson {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AssessmentPerson")
            .field("role", &self.role)
            .field("name", &self.name)
            .field("question_count", &self.questions.len())
            .finish()
    }
}

/// Whether a person row is a teacher or a teaching assistant.  The service
/// renders the two groups in separate tables; this module labels them from the
/// table's own position rather than from a caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssessmentPersonRole {
    Teacher,
    Assistant,
}

/// A parsed questionnaire, ready to be filled in and submitted.
///
/// Only the module's own parser constructs one, so the transaction state
/// a submission carries is always the service's own.  A `Clone` of a form is a
/// second copy of the same answers, which is why the adapter refuses a
/// submission whose body has already been dispatched rather than trusting that
/// only one copy exists.
#[derive(Clone, PartialEq, Eq)]
pub struct AssessmentForm {
    transaction: Vec<AssessmentFormField>,
    suggestion: Option<AssessmentFormField>,
    score_name: String,
    score: u32,
    teachers: Vec<AssessmentPerson>,
    assistants: Vec<AssessmentPerson>,
}

impl AssessmentForm {
    /// The overall score currently selected.
    pub fn score(&self) -> u32 {
        self.score
    }

    /// Selects the overall score.
    pub fn set_score(&mut self, score: u32) -> Result<(), AssessmentInputError> {
        if !(ASSESSMENT_MIN_SCORE..=ASSESSMENT_MAX_SCORE).contains(&score) {
            return Err(AssessmentInputError::ScoreOutOfRange);
        }
        self.score = score;
        Ok(())
    }

    /// The overall written comment, when the service rendered one.
    pub fn suggestion(&self) -> Option<&str> {
        self.suggestion.as_ref().map(AssessmentFormField::value)
    }

    /// Sets the overall written comment.
    pub fn set_suggestion(&mut self, value: &str) -> Result<(), AssessmentInputError> {
        match self.suggestion.as_mut() {
            Some(field) => field.write(value),
            None => Err(AssessmentInputError::NoSuggestionField),
        }
    }

    pub fn teachers(&self) -> &[AssessmentPerson] {
        &self.teachers
    }

    pub fn teachers_mut(&mut self) -> &mut [AssessmentPerson] {
        &mut self.teachers
    }

    pub fn assistants(&self) -> &[AssessmentPerson] {
        &self.assistants
    }

    pub fn assistants_mut(&mut self) -> &mut [AssessmentPerson] {
        &mut self.assistants
    }

    /// The number of transaction fields the service rendered, none of which a
    /// caller can read or write.
    pub fn transaction_field_count(&self) -> usize {
        self.transaction.len()
    }

    /// Selects one score for every question of every person.  This is the bulk
    /// fill a caller performs before submitting; it changes scores only and
    /// never the service's transaction state.
    pub fn set_all_person_scores(&mut self, score: u32) -> Result<(), AssessmentInputError> {
        for person in self.teachers.iter_mut().chain(self.assistants.iter_mut()) {
            person.set_all_scores(score)?;
        }
        Ok(())
    }

    /// The number of name/value pairs a submission will carry.
    pub fn field_count(&self) -> usize {
        let mut count = self.transaction.len() + 1 + usize::from(self.suggestion.is_some());
        for person in &self.teachers {
            count += person.submission_pairs().len();
        }
        for person in &self.assistants {
            count += person.submission_pairs().len();
        }
        count
    }

    /// Renders the submission body.
    ///
    /// The pair order mirrors the form: the service's transaction state first,
    /// then the overall comment and rating, then each person in the order the
    /// page listed them.  It is crate-visible so the shape a write produces is
    /// testable; no public path in this crate exposes it, so a caller cannot
    /// post a body of its own construction.
    pub(crate) fn serialize(&self) -> String {
        let mut pairs: Vec<(&str, String)> = Vec::with_capacity(self.field_count());
        for field in &self.transaction {
            pairs.push((field.name.as_str(), field.value().to_owned()));
        }
        if let Some(suggestion) = self.suggestion.as_ref() {
            pairs.push((suggestion.name.as_str(), suggestion.value().to_owned()));
        }
        pairs.push((self.score_name.as_str(), self.score.to_string()));
        for person in self.teachers.iter().chain(self.assistants.iter()) {
            pairs.extend(person.submission_pairs());
        }
        let mut body = String::new();
        for (name, value) in pairs {
            if !body.is_empty() {
                body.push('&');
            }
            body.push_str(&encode_form_component(name));
            body.push('=');
            body.push_str(&encode_form_component(&value));
        }
        body
    }
}

impl fmt::Debug for AssessmentForm {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AssessmentForm")
            .field("transaction_field_count", &self.transaction.len())
            .field("teacher_count", &self.teachers.len())
            .field("assistant_count", &self.assistants.len())
            .field("field_count", &self.field_count())
            .finish()
    }
}

/// One parsed form bound to the row and list generation it was read from,
/// together with the answers the caller filled in.
///
/// The binding is the reason this type exists rather than a bare
/// [`AssessmentForm`]: a submission must present the same reference the form
/// was read through, so a form cannot be posted against a row it does not
/// belong to, and a list read that superseded the form makes it unusable
/// rather than silently posting stale answers.
#[derive(Clone, PartialEq, Eq)]
pub struct AssessmentEvaluation {
    binding: u64,
    generation: u64,
    index: u32,
    course: String,
    form: AssessmentForm,
}

impl AssessmentEvaluation {
    /// Binds one parsed form to the row it was read from.
    ///
    /// It exists for the module's own tests, which exercise the display copy
    /// and the answer translation without a live adapter; only
    /// [`AssessmentAdapter::read_form`] produces one in production.
    #[cfg(test)]
    pub(crate) fn for_test(
        binding: u64,
        generation: u64,
        index: u32,
        course: String,
        form: AssessmentForm,
    ) -> Self {
        Self {
            binding,
            generation,
            index,
            course,
            form,
        }
    }

    /// The course this evaluation belongs to, as the list named it.
    pub fn course(&self) -> &str {
        &self.course
    }

    /// The row this evaluation was read from.
    pub fn reference(&self) -> AssessmentRef {
        AssessmentRef::new(self.binding, self.generation, self.index)
    }

    /// The answers currently filled in.
    pub fn form(&self) -> &AssessmentForm {
        &self.form
    }

    /// Mutable access to the answers.
    pub fn form_mut(&mut self) -> &mut AssessmentForm {
        &mut self.form
    }

    /// Selects one score for every question of every person, and for the
    /// overall rating.  This is the bulk fill a caller performs before
    /// submitting; it changes scores only and never the service's transaction
    /// state.
    pub fn set_all_scores(&mut self, score: u32) -> Result<(), AssessmentInputError> {
        self.form.set_score(score)?;
        self.form.set_all_person_scores(score)
    }

    /// A display copy of this questionnaire, with the service's submission
    /// state left out.
    ///
    /// The copy carries the reference, so the answers a caller authors from it
    /// name the row they belong to.  Handing the copy back never rebuilds a
    /// body: the runtime applies the answers to the form it already holds
    /// through the crate-visible translation on `AssessmentAnswers`.
    pub fn view(&self) -> AssessmentFormView {
        AssessmentFormView {
            reference: self.reference(),
            course: self.course.clone(),
            score: self.form.score(),
            suggestion: self.form.suggestion().map(str::to_owned),
            teachers: self.form.teachers().iter().map(person_view).collect(),
            assistants: self.form.assistants().iter().map(person_view).collect(),
            field_count: self.form.field_count(),
        }
    }
}

/// Copies one person's answers into a display value.
fn person_view(person: &AssessmentPerson) -> AssessmentPersonView {
    AssessmentPersonView {
        name: person.name().to_owned(),
        role: person.role(),
        questions: (0..person.question_count())
            .map(|index| AssessmentQuestionView {
                text: person.question(index).unwrap_or_default().to_owned(),
                score: person.question_score(index).unwrap_or(ASSESSMENT_MIN_SCORE),
                suggestion: person.question_suggestion(index).map(str::to_owned),
            })
            .collect(),
    }
}

impl fmt::Debug for AssessmentEvaluation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AssessmentEvaluation")
            .field("course", &self.course)
            .field("index", &self.index)
            .field("form", &self.form)
            .finish()
    }
}

/// One question of one questionnaire, as a caller sees it.
///
/// It carries the question's own text and the answer the service currently
/// holds.  A `None` comment means the service rendered no comment field for
/// this question, so there is nothing a caller could write here.
#[derive(Clone, PartialEq, Eq)]
pub struct AssessmentQuestionView {
    text: String,
    score: u32,
    suggestion: Option<String>,
}

impl AssessmentQuestionView {
    /// Rebuilds one display question from the parts a runtime layer carries.
    ///
    /// The view is a copy of values the adapter itself produced, so this
    /// constructor is crate-visible only: no public path in this crate can
    /// invent a question or an answer that the service never rendered.
    pub(crate) fn from_parts(text: String, score: u32, suggestion: Option<String>) -> Self {
        Self {
            text,
            score,
            suggestion,
        }
    }

    /// The question text exactly as the service rendered it.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The score the service currently holds for this question.
    pub fn score(&self) -> u32 {
        self.score
    }

    /// The comment the service currently holds, or `None` when this question
    /// has no comment field.
    pub fn suggestion(&self) -> Option<&str> {
        self.suggestion.as_deref()
    }
}

impl fmt::Debug for AssessmentQuestionView {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AssessmentQuestionView")
            .field("score", &self.score)
            .field("has_suggestion", &self.suggestion.is_some())
            .finish()
    }
}

/// One person a questionnaire asks about, as a caller sees it.
#[derive(Clone, PartialEq, Eq)]
pub struct AssessmentPersonView {
    name: String,
    role: AssessmentPersonRole,
    questions: Vec<AssessmentQuestionView>,
}

impl AssessmentPersonView {
    /// Rebuilds one display person from the parts a runtime layer carries.
    /// Crate-visible for the same reason as
    /// [`AssessmentQuestionView::from_parts`].
    pub(crate) fn from_parts(
        name: String,
        role: AssessmentPersonRole,
        questions: Vec<AssessmentQuestionView>,
    ) -> Self {
        Self {
            name,
            role,
            questions,
        }
    }

    /// The person's name as the service rendered it.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether this person is a teacher or a teaching assistant.
    pub fn role(&self) -> AssessmentPersonRole {
        self.role
    }

    /// How many questions this person was asked.
    pub fn question_count(&self) -> usize {
        self.questions.len()
    }

    /// One question, addressed by a position this person's own count bounds.
    pub fn question(&self, index: usize) -> Option<&AssessmentQuestionView> {
        self.questions.get(index)
    }

    pub fn questions(&self) -> &[AssessmentQuestionView] {
        &self.questions
    }
}

impl fmt::Debug for AssessmentPersonView {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AssessmentPersonView")
            .field("role", &self.role)
            .field("question_count", &self.questions.len())
            .finish()
    }
}

/// One questionnaire as a caller sees it.
///
/// This is a copy of the display values, not the object that gets submitted.
/// A caller fills in an [`AssessmentAnswers`] and hands it back, and the
/// runtime applies it to the form it holds; the service's submission state is
/// never part of what crosses this boundary in either direction.
#[derive(Clone, PartialEq, Eq)]
pub struct AssessmentFormView {
    reference: AssessmentRef,
    course: String,
    score: u32,
    suggestion: Option<String>,
    teachers: Vec<AssessmentPersonView>,
    assistants: Vec<AssessmentPersonView>,
    field_count: usize,
}

impl AssessmentFormView {
    /// Rebuilds one display form from the parts a runtime layer carries.
    /// Crate-visible for the same reason as
    /// [`AssessmentQuestionView::from_parts`].
    pub(crate) fn from_parts(
        reference: AssessmentRef,
        course: String,
        score: u32,
        suggestion: Option<String>,
        teachers: Vec<AssessmentPersonView>,
        assistants: Vec<AssessmentPersonView>,
        field_count: usize,
    ) -> Self {
        Self {
            reference,
            course,
            score,
            suggestion,
            teachers,
            assistants,
            field_count,
        }
    }

    /// The row this questionnaire belongs to.
    pub fn reference(&self) -> AssessmentRef {
        self.reference.clone()
    }

    /// The course this questionnaire evaluates, as the list named it.
    pub fn course(&self) -> &str {
        &self.course
    }

    /// The overall score the service currently holds.
    pub fn score(&self) -> u32 {
        self.score
    }

    /// The overall comment the service currently holds, when it rendered one.
    pub fn suggestion(&self) -> Option<&str> {
        self.suggestion.as_deref()
    }

    pub fn teachers(&self) -> &[AssessmentPersonView] {
        &self.teachers
    }

    pub fn assistants(&self) -> &[AssessmentPersonView] {
        &self.assistants
    }

    /// The number of name/value pairs a submission of this questionnaire will
    /// carry.  It is a shape check for a caller, not a body.
    pub fn field_count(&self) -> usize {
        self.field_count
    }
}

impl fmt::Debug for AssessmentFormView {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AssessmentFormView")
            .field("index", &self.reference.index)
            .field("score", &self.score)
            .field("teacher_count", &self.teachers.len())
            .field("assistant_count", &self.assistants.len())
            .finish()
    }
}

/// One question's answer, as a caller supplies it.
///
/// A `None` comment leaves the value the service currently holds untouched,
/// which is what makes it possible to change only a score.  An empty `Some("")`
/// is a deliberate clearing of the comment.
#[derive(Clone, PartialEq, Eq)]
pub struct AssessmentQuestionAnswer {
    score: u32,
    suggestion: Option<String>,
}

impl AssessmentQuestionAnswer {
    /// Builds one answer, refusing a score the service does not accept.
    pub fn new(score: u32, suggestion: Option<String>) -> Result<Self, AssessmentInputError> {
        if !(ASSESSMENT_MIN_SCORE..=ASSESSMENT_MAX_SCORE).contains(&score) {
            return Err(AssessmentInputError::ScoreOutOfRange);
        }
        if let Some(value) = suggestion.as_deref()
            && !valid_field_component(value)
        {
            return Err(AssessmentInputError::InvalidValue);
        }
        Ok(Self { score, suggestion })
    }

    /// The score this answer selects.
    pub fn score(&self) -> u32 {
        self.score
    }

    /// The comment this answer sets, or `None` to leave the current one.
    pub fn suggestion(&self) -> Option<&str> {
        self.suggestion.as_deref()
    }
}

impl fmt::Debug for AssessmentQuestionAnswer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AssessmentQuestionAnswer")
            .field("score", &self.score)
            .field("has_suggestion", &self.suggestion.is_some())
            .finish()
    }
}

/// One person's answers, position by position.
#[derive(Clone, PartialEq, Eq)]
pub struct AssessmentPersonAnswers {
    questions: Vec<AssessmentQuestionAnswer>,
}

impl AssessmentPersonAnswers {
    pub fn new(questions: Vec<AssessmentQuestionAnswer>) -> Self {
        Self { questions }
    }

    pub fn questions(&self) -> &[AssessmentQuestionAnswer] {
        &self.questions
    }
}

impl fmt::Debug for AssessmentPersonAnswers {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AssessmentPersonAnswers")
            .field("question_count", &self.questions.len())
            .finish()
    }
}

/// The answers a caller fills in for one questionnaire.
///
/// It carries scores and comments only.  No field here names a submission
/// field, so the body a submission sends is the service's own form with these
/// answers applied rather than a body the caller built; the transaction state
/// stays inside the runtime from the read to the write.
///
/// The answer set names the row it was authored for.  A runtime that holds a
/// form for a different row refuses it rather than applying the answers to
/// whatever questionnaire happens to be open.
#[derive(Clone, PartialEq, Eq)]
pub struct AssessmentAnswers {
    reference: AssessmentRef,
    score: u32,
    suggestion: Option<String>,
    teachers: Vec<AssessmentPersonAnswers>,
    assistants: Vec<AssessmentPersonAnswers>,
}

impl AssessmentAnswers {
    /// Starts an answer set for one row, with the given overall score.
    pub fn new(reference: AssessmentRef, score: u32) -> Result<Self, AssessmentInputError> {
        if !(ASSESSMENT_MIN_SCORE..=ASSESSMENT_MAX_SCORE).contains(&score) {
            return Err(AssessmentInputError::ScoreOutOfRange);
        }
        Ok(Self {
            reference,
            score,
            suggestion: None,
            teachers: Vec::new(),
            assistants: Vec::new(),
        })
    }

    /// The row these answers are for.
    pub fn reference(&self) -> AssessmentRef {
        self.reference.clone()
    }

    pub fn score(&self) -> u32 {
        self.score
    }

    /// Selects the overall score.
    pub fn set_score(&mut self, score: u32) -> Result<(), AssessmentInputError> {
        if !(ASSESSMENT_MIN_SCORE..=ASSESSMENT_MAX_SCORE).contains(&score) {
            return Err(AssessmentInputError::ScoreOutOfRange);
        }
        self.score = score;
        Ok(())
    }

    /// The overall comment these answers set, or `None` to leave the current
    /// one.
    pub fn suggestion(&self) -> Option<&str> {
        self.suggestion.as_deref()
    }

    /// Sets the overall comment.
    pub fn set_suggestion(&mut self, value: &str) -> Result<(), AssessmentInputError> {
        if !valid_field_component(value) {
            return Err(AssessmentInputError::InvalidValue);
        }
        self.suggestion = Some(value.to_owned());
        Ok(())
    }

    pub fn teachers(&self) -> &[AssessmentPersonAnswers] {
        &self.teachers
    }

    pub fn assistants(&self) -> &[AssessmentPersonAnswers] {
        &self.assistants
    }

    /// Supplies the per-person answers, in the order the form listed them.
    pub fn set_people(
        &mut self,
        teachers: Vec<AssessmentPersonAnswers>,
        assistants: Vec<AssessmentPersonAnswers>,
    ) {
        self.teachers = teachers;
        self.assistants = assistants;
    }

    /// Applies these answers to a form the runtime already holds.
    ///
    /// The shape must match the form position by position: a person or a
    /// question the form did not render is refused with
    /// [`AssessmentInputError::UnknownQuestion`] rather than being skipped, so
    /// an answer set built against a different questionnaire cannot silently
    /// leave part of this one unanswered.
    pub(crate) fn apply_to(&self, form: &mut AssessmentForm) -> Result<(), AssessmentInputError> {
        form.set_score(self.score)?;
        if let Some(value) = self.suggestion.as_deref() {
            form.set_suggestion(value)?;
        }
        apply_person_answers(form.teachers_mut(), &self.teachers)?;
        apply_person_answers(form.assistants_mut(), &self.assistants)
    }
}

impl fmt::Debug for AssessmentAnswers {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AssessmentAnswers")
            .field("index", &self.reference.index)
            .field("score", &self.score)
            .field("has_suggestion", &self.suggestion.is_some())
            .field("teacher_count", &self.teachers.len())
            .field("assistant_count", &self.assistants.len())
            .finish()
    }
}

/// Applies one group of answers to one group of people.
///
/// The counts must agree exactly.  A caller that sends fewer people than the
/// form holds is refused rather than leaving the rest at the service's
/// previous values, because "the answers I did not mention" and "the answers I
/// meant to leave alone" are not the same request.
fn apply_person_answers(
    people: &mut [AssessmentPerson],
    answers: &[AssessmentPersonAnswers],
) -> Result<(), AssessmentInputError> {
    if people.len() != answers.len() {
        return Err(AssessmentInputError::UnknownQuestion);
    }
    for (person, person_answers) in people.iter_mut().zip(answers) {
        if person.question_count() != person_answers.questions.len() {
            return Err(AssessmentInputError::UnknownQuestion);
        }
        for (index, answer) in person_answers.questions.iter().enumerate() {
            person.set_question_score(index, answer.score)?;
            if let Some(value) = answer.suggestion.as_deref() {
                person.set_question_suggestion(index, value)?;
            }
        }
    }
    Ok(())
}

/// A caller-supplied value that does not fit the operation.
///
/// These are the only failures a caller can cause directly.  They are distinct
/// from service failures so a form editor never has to guess whether the
/// service rejected an answer or a local bound did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum AssessmentInputError {
    #[error("the score is outside the range the service accepts")]
    ScoreOutOfRange,

    #[error("the value is not accepted for this field")]
    InvalidValue,

    #[error("this questionnaire has no comment field here")]
    NoSuggestionField,

    #[error("this person was not asked that many questions")]
    UnknownQuestion,

    #[error("the form no longer matches the list it was read from")]
    StaleForm,
}

/// Parser failures retain only stable names.  They never keep response bytes,
/// Cookie values, or server echoes.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AssessmentParseError {
    #[error("assessment response body is empty")]
    EmptyBody,

    #[error("assessment response is an HTML login page")]
    LoginPage,

    #[error("assessment response is an expired or timed-out page")]
    ExpiredPage,

    #[error("the teaching-evaluation questionnaire window is not open")]
    NotOpen,

    #[error("assessment page did not contain the questionnaire table")]
    MissingTable,

    #[error("the questionnaire list had no rows")]
    EmptyList,

    #[error("assessment row {row} has an unrecognized layout")]
    UnrecognizedRow { row: usize },

    #[error("assessment row {row} is missing its course name")]
    MissingName { row: usize },

    #[error("assessment row {row} has no readable form action")]
    MissingAction { row: usize },

    #[error("assessment row {row} named a form route outside the service mapping")]
    InvalidRoute { row: usize },

    #[error("assessment response exceeded the bounded element limit")]
    TooLarge,

    #[error("assessment form page is missing its transaction state")]
    MissingForm,

    #[error("assessment form page carried no questions")]
    EmptyForm,

    #[error("assessment form page carried an unreadable field")]
    UnrecognizedField,

    #[error("assessment form page carried an unusable score")]
    InvalidScore,
}

impl AssessmentParseError {
    /// Returns true when this failure is evidence of an unauthenticated or
    /// expired INFO session rather than a changed deployment.
    pub fn is_session_expired(&self) -> bool {
        matches!(self, Self::LoginPage | Self::ExpiredPage)
    }

    /// Returns true when the service explicitly reported a closed window.
    pub fn is_not_open(&self) -> bool {
        matches!(self, Self::NotOpen)
    }
}

impl From<ScanError> for AssessmentParseError {
    fn from(_: ScanError) -> Self {
        Self::TooLarge
    }
}

/// Adapter failures are body-free so an HTML login page cannot leak through a
/// debug or bridge DTO.
#[derive(Debug, Error)]
pub enum AssessmentAdapterError {
    #[error("assessment base URL is invalid")]
    InvalidBaseUrl,

    #[error("assessment transport failed")]
    Transport(#[source] TransportError),

    #[error("assessment request returned HTTP status {status}")]
    HttpStatus { status: StatusCode },

    #[error("assessment response came from an unexpected origin")]
    UnexpectedOrigin,

    #[error("assessment response ended outside the configured mapping")]
    UnexpectedPath,

    #[error("assessment response is not an HTML document")]
    UnexpectedContentType,

    #[error("assessment INFO/WebVPN session has expired or is not established")]
    SessionExpired,

    #[error("assessment response is not the expected deployment")]
    UnexpectedDeployment,

    #[error("the teaching-evaluation questionnaire window is not open")]
    NotOpen,

    #[error("assessment response could not be parsed: {0}")]
    Parse(#[source] AssessmentParseError),

    #[error("the requested questionnaire row is not part of the current list")]
    UnknownForm,

    #[error("the questionnaire form belongs to a different evaluation session")]
    ForeignForm,

    #[error("the questionnaire form does not match the list it was read from")]
    StaleForm,

    #[error("the questionnaire answer is not accepted: {0}")]
    Input(#[source] AssessmentInputError),

    #[error("the questionnaire submission body is too large to send")]
    SubmitTooLarge,

    #[error("the questionnaire submission has already been dispatched")]
    SubmitAlreadyAttempted,

    #[error("the service did not confirm the questionnaire submission")]
    SubmitUnconfirmed,
}

impl AssessmentAdapterError {
    pub(crate) fn diagnostic_code(&self) -> &'static str {
        match self {
            Self::InvalidBaseUrl => "assessment_config",
            Self::Transport(_) => "assessment_network",
            Self::HttpStatus { .. } => "assessment_http",
            Self::UnexpectedOrigin => "assessment_origin",
            Self::UnexpectedPath => "assessment_path",
            Self::UnexpectedContentType => "assessment_content_type",
            Self::SessionExpired => "assessment_auth_required",
            Self::UnexpectedDeployment => "assessment_template",
            Self::NotOpen => "assessment_not_open",
            Self::Parse(AssessmentParseError::EmptyBody) => "assessment_body_empty",
            Self::Parse(AssessmentParseError::LoginPage) => "assessment_auth_required",
            Self::Parse(AssessmentParseError::ExpiredPage) => "assessment_auth_required",
            Self::Parse(AssessmentParseError::NotOpen) => "assessment_not_open",
            Self::Parse(AssessmentParseError::MissingTable) => "assessment_table_missing",
            Self::Parse(AssessmentParseError::EmptyList) => "assessment_list_empty",
            Self::Parse(AssessmentParseError::UnrecognizedRow { .. }) => "assessment_row_shape",
            Self::Parse(AssessmentParseError::MissingName { .. }) => "assessment_row_name",
            Self::Parse(AssessmentParseError::MissingAction { .. }) => "assessment_row_action",
            Self::Parse(AssessmentParseError::InvalidRoute { .. }) => "assessment_row_route",
            Self::Parse(AssessmentParseError::TooLarge) => "assessment_size",
            Self::Parse(AssessmentParseError::MissingForm) => "assessment_form_missing",
            Self::Parse(AssessmentParseError::EmptyForm) => "assessment_form_empty",
            Self::Parse(AssessmentParseError::UnrecognizedField) => "assessment_form_field",
            Self::Parse(AssessmentParseError::InvalidScore) => "assessment_form_score",
            Self::UnknownForm => "assessment_form_unknown",
            Self::StaleForm => "assessment_form_stale",
            Self::ForeignForm => "assessment_form_foreign",
            Self::Input(AssessmentInputError::ScoreOutOfRange) => "assessment_input_score",
            Self::Input(AssessmentInputError::InvalidValue) => "assessment_input_value",
            Self::Input(AssessmentInputError::NoSuggestionField) => "assessment_input_comment",
            Self::Input(AssessmentInputError::UnknownQuestion) => "assessment_input_question",
            Self::Input(AssessmentInputError::StaleForm) => "assessment_form_stale",
            Self::SubmitTooLarge => "assessment_submit_size",
            Self::SubmitAlreadyAttempted => "assessment_submit_replayed",
            Self::SubmitUnconfirmed => "assessment_submit_unconfirmed",
        }
    }

    pub fn is_session_expired(&self) -> bool {
        matches!(
            self,
            Self::SessionExpired
                | Self::Parse(AssessmentParseError::LoginPage)
                | Self::Parse(AssessmentParseError::ExpiredPage)
        )
    }

    /// True when the service explicitly answered that the window is closed.
    pub fn is_not_open(&self) -> bool {
        matches!(
            self,
            Self::NotOpen | Self::Parse(AssessmentParseError::NotOpen)
        )
    }

    /// True when a submission was dispatched but its effect is unknown.
    ///
    /// A caller must not retry on this: the service may have stored the
    /// questionnaire already, and a second POST would be a replay of a
    /// one-shot write.  A fresh list read is the only safe way to learn what
    /// happened.
    pub fn is_unconfirmed(&self) -> bool {
        matches!(self, Self::SubmitUnconfirmed)
    }
}

/// Configuration for a read-only evaluation adapter.
#[derive(Clone)]
pub struct AssessmentAdapterConfig {
    base_url: Url,
    user_agent: String,
    timeout: Duration,
}

impl AssessmentAdapterConfig {
    pub fn new(base_url: &str) -> Result<Self, AssessmentAdapterError> {
        Self::with_user_agent_and_timeout(
            base_url,
            "THYou/teaching-evaluation",
            Duration::from_secs(30),
        )
    }

    pub fn with_user_agent(
        base_url: &str,
        user_agent: impl Into<String>,
    ) -> Result<Self, AssessmentAdapterError> {
        Self::with_user_agent_and_timeout(base_url, user_agent, Duration::from_secs(30))
    }

    pub fn with_user_agent_and_timeout(
        base_url: &str,
        user_agent: impl Into<String>,
        timeout: Duration,
    ) -> Result<Self, AssessmentAdapterError> {
        let base_url = Url::parse(base_url).map_err(|_| AssessmentAdapterError::InvalidBaseUrl)?;
        let base_url = normalize_base_url(base_url)?;
        let user_agent = user_agent.into();
        if user_agent.trim().is_empty() || user_agent.chars().any(char::is_control) {
            return Err(AssessmentAdapterError::InvalidBaseUrl);
        }
        Ok(Self {
            base_url,
            user_agent,
            timeout,
        })
    }

    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    fn transport(&self) -> Result<CampusHttpTransport, AssessmentAdapterError> {
        CampusHttpTransport::with_timeout(&self.user_agent, self.timeout)
            .map_err(AssessmentAdapterError::Transport)
    }
}

impl fmt::Debug for AssessmentAdapterConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AssessmentAdapterConfig")
            .field("scheme", &self.base_url.scheme())
            .field("host", &self.base_url.host_str())
            .field("has_opaque_path", &(!self.base_url.path().is_empty()))
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// Proof that this adapter parsed one evaluation response.  It is opaque: no
/// Cookie, URL, account value, or response body.
#[derive(Clone, PartialEq, Eq)]
pub struct AssessmentBusinessProof {
    adapter_binding: u64,
    generation: u64,
    operation: AssessmentOperation,
}

impl fmt::Debug for AssessmentBusinessProof {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AssessmentBusinessProof")
            .field("operation", &self.operation)
            .finish()
    }
}

/// A validated list together with its business proof.
#[derive(Debug, Clone, PartialEq)]
pub struct AssessmentRead {
    pub value: AssessmentList,
    pub proof: AssessmentBusinessProof,
}

/// The form routes named by the most recent successful list read, plus the
/// one-shot bookkeeping the submission needs.
#[derive(Default)]
struct AssessmentRoutes {
    generation: u64,
    names: Vec<String>,
    paths: Vec<String>,
    /// The rows of this generation whose form has already been dispatched.
    ///
    /// One row is one questionnaire, so this is per row rather than per
    /// session: a student evaluates several courses.  A second submission of
    /// the same row is refused, because the body has already left and its
    /// effect is unknown.
    submitted: Vec<u32>,
    /// Set when a submission was dispatched and the service has not confirmed
    /// it.  It is cleared by the service's own success answer or by a fresh
    /// list read, which is the one event that re-establishes what the service
    /// actually holds.
    unconfirmed: bool,
}

/// The evaluation client: the questionnaire list, its form, and the one write
/// that stores a filled-in questionnaire.
///
/// `try_with_transport` is the normal runtime entry point: the transport must
/// be the one that already carries the identity/INFO/WebVPN cookie jar.
pub struct AssessmentAdapter {
    base_url: Url,
    transport: CampusHttpTransport,
    profile: AssessmentProfile,
    binding: u64,
    routes: Mutex<AssessmentRoutes>,
}

impl AssessmentAdapter {
    pub fn new(config: AssessmentAdapterConfig) -> Result<Self, AssessmentAdapterError> {
        let transport = config.transport()?;
        Self::try_with_transport(config.base_url, transport)
    }

    pub fn try_with_transport(
        base_url: Url,
        transport: CampusHttpTransport,
    ) -> Result<Self, AssessmentAdapterError> {
        let base_url = normalize_base_url(base_url)?;
        Ok(Self {
            base_url,
            transport,
            profile: AssessmentProfile::standard(),
            binding: NEXT_ASSESSMENT_ADAPTER_BINDING.fetch_add(1, Ordering::Relaxed),
            routes: Mutex::new(AssessmentRoutes::default()),
        })
    }

    pub fn transport(&self) -> &CampusHttpTransport {
        &self.transport
    }

    pub fn profile(&self) -> AssessmentProfile {
        self.profile
    }

    /// Reads the questionnaire list, replacing the routes retained by the
    /// previous read.
    pub async fn read_list(&self) -> Result<AssessmentList, AssessmentAdapterError> {
        self.read_list_with_proof().await.map(|read| read.value)
    }

    pub async fn read_list_with_proof(&self) -> Result<AssessmentRead, AssessmentAdapterError> {
        let plan = self.profile.list_request();
        let body = self.execute(&plan).await?;
        let parsed =
            parse_assessment_list_html(&body).map_err(AssessmentAdapter::map_parse_error)?;
        // The generation advances only after the page parsed, so a rejected
        // read leaves the previous routes resolvable instead of pointing them
        // at nothing.
        let generation = self
            .routes
            .lock()
            .map(|routes| routes.generation)
            .unwrap_or_default()
            .wrapping_add(1);
        let items = parsed
            .rows
            .iter()
            .enumerate()
            .map(|(index, row)| AssessmentItem {
                name: row.name.clone(),
                evaluated: row.evaluated,
                reference: AssessmentRef::new(self.binding, generation, index as u32),
            })
            .collect();
        if let Ok(mut routes) = self.routes.lock() {
            // A fresh list replaces the generation, its routes and its
            // per-row submission bookkeeping together, and it clears the
            // unconfirmed flag: the list is the service's own current answer
            // to what it holds, so a question an earlier submission stored now
            // reads back as evaluated rather than as unknown.
            *routes = AssessmentRoutes {
                generation,
                names: parsed.rows.iter().map(|row| row.name.clone()).collect(),
                paths: parsed.rows.iter().map(|row| row.route.clone()).collect(),
                submitted: Vec::new(),
                unconfirmed: false,
            };
        }
        Ok(AssessmentRead {
            value: AssessmentList { items },
            proof: AssessmentBusinessProof {
                adapter_binding: self.binding,
                generation,
                operation: AssessmentOperation::ReadList,
            },
        })
    }

    /// Checks that a business proof came from this adapter instance.
    pub fn business_proof_matches(&self, proof: &AssessmentBusinessProof) -> bool {
        proof.adapter_binding == self.binding
    }

    /// Resolves a row reference to the form route it named, if the reference
    /// still belongs to the list this adapter most recently read.
    pub fn form_path(&self, reference: &AssessmentRef) -> Option<String> {
        if reference.adapter_binding != self.binding {
            return None;
        }
        let routes = self.routes.lock().ok()?;
        if routes.generation != reference.generation {
            return None;
        }
        routes
            .paths
            .get(usize::try_from(reference.index).ok()?)
            .cloned()
    }

    /// Reads the questionnaire form named by one row of the list this adapter
    /// most recently parsed.
    ///
    /// The route is resolved inside the adapter from the row itself, so a
    /// caller never names it.  A reference this adapter did not produce, or one
    /// whose list read has been superseded, is refused rather than resolved
    /// against whatever route happens to be current.
    ///
    /// The returned form is bound to this adapter, to the list generation it
    /// came from, and to the row it belongs to.  A submission must present both
    /// the same reference and the same binding again, which is what keeps a
    /// stale or foreign form from being posted against state it does not
    /// describe.
    pub async fn read_form(
        &self,
        reference: &AssessmentRef,
    ) -> Result<AssessmentEvaluation, AssessmentAdapterError> {
        if reference.adapter_binding != self.binding {
            return Err(AssessmentAdapterError::ForeignForm);
        }
        let route = {
            let routes = self
                .routes
                .lock()
                .map_err(|_| AssessmentAdapterError::ForeignForm)?;
            if routes.generation != reference.generation {
                return Err(AssessmentAdapterError::StaleForm);
            }
            let index = usize::try_from(reference.index)
                .map_err(|_| AssessmentAdapterError::UnknownForm)?;
            routes
                .paths
                .get(index)
                .cloned()
                .ok_or(AssessmentAdapterError::UnknownForm)?
        };
        let plan = self.profile.form_request();
        let body = self.execute_page(&plan, &route).await?;
        let parsed =
            parse_assessment_form_html(&body).map_err(AssessmentAdapter::map_parse_error)?;
        Ok(AssessmentEvaluation {
            binding: self.binding,
            generation: reference.generation,
            index: reference.index,
            course: self.course_name(reference).unwrap_or_default(),
            form: AssessmentForm {
                transaction: parsed.transaction,
                suggestion: parsed.suggestion,
                score_name: parsed.score_name,
                score: parsed.score,
                teachers: parsed.teachers,
                assistants: parsed.assistants,
            },
        })
    }

    /// The course name the current list holds for one row.
    fn course_name(&self, reference: &AssessmentRef) -> Option<String> {
        let routes = self.routes.lock().ok()?;
        if routes.generation != reference.generation {
            return None;
        }
        routes
            .names
            .get(usize::try_from(reference.index).ok()?)
            .cloned()
    }

    /// Stores one filled-in questionnaire.
    ///
    /// This is a one-shot write.  It is dispatched exactly once: if the
    /// service did not answer with its own success result, the outcome is
    /// reported as unconfirmed and this module does not retry — a second POST
    /// would be a replay of a write whose effect is unknown.  A second
    /// submission of the same row is refused with
    /// [`AssessmentAdapterError::SubmitAlreadyAttempted`] until a fresh list
    /// read replaces the generation.
    ///
    /// A row the service marked as evaluated may still be submitted: the
    /// service renders its form again, and a student may revise an answer
    /// before the window closes.  What is refused is a *second* dispatch of the
    /// same row from this session, not a rewrite of a stored answer.
    pub async fn submit_form(
        &self,
        evaluation: &AssessmentEvaluation,
    ) -> Result<(), AssessmentAdapterError> {
        if evaluation.binding != self.binding {
            return Err(AssessmentAdapterError::ForeignForm);
        }
        {
            let routes = self
                .routes
                .lock()
                .map_err(|_| AssessmentAdapterError::ForeignForm)?;
            if routes.generation != evaluation.generation {
                return Err(AssessmentAdapterError::StaleForm);
            }
            if !routes
                .paths
                .iter()
                .enumerate()
                .any(|(index, _)| u32::try_from(index).ok() == Some(evaluation.index))
            {
                return Err(AssessmentAdapterError::UnknownForm);
            }
            if routes.submitted.contains(&evaluation.index) {
                return Err(AssessmentAdapterError::SubmitAlreadyAttempted);
            }
        }
        let body = evaluation.form.serialize();
        if body.len() > MAX_SUBMIT_BODY_BYTES {
            return Err(AssessmentAdapterError::SubmitTooLarge);
        }
        // The row is marked as dispatched before the request is built: once
        // the client holds the body, a transport failure cannot be
        // distinguished from a request the service already processed.
        {
            let mut routes = self
                .routes
                .lock()
                .map_err(|_| AssessmentAdapterError::ForeignForm)?;
            routes.submitted.push(evaluation.index);
        }
        let plan = self.profile.submit_request();
        match self.submit_body(&plan, body).await {
            Ok(()) => {
                if let Ok(mut routes) = self.routes.lock() {
                    routes.unconfirmed = false;
                }
                Ok(())
            }
            Err(error) => {
                if let Ok(mut routes) = self.routes.lock() {
                    routes.unconfirmed = true;
                }
                Err(error)
            }
        }
    }

    /// True when a submission was dispatched and the service has not
    /// confirmed it.  A caller surfaces this as "check the evaluation page";
    /// it is never a reason to submit again.
    pub fn has_unconfirmed_submission(&self) -> bool {
        self.routes
            .lock()
            .map(|routes| routes.unconfirmed)
            .unwrap_or(true)
    }

    /// The current route generation.  It advances once per accepted read.
    pub fn route_generation(&self) -> u64 {
        self.routes
            .lock()
            .map(|routes| routes.generation)
            .unwrap_or_default()
    }

    async fn execute(
        &self,
        plan: &AssessmentRequestPlan,
    ) -> Result<String, AssessmentAdapterError> {
        self.execute_route(plan, "").await
    }

    /// Runs one GET against the plan's constant path, or against the row route
    /// the plan names.
    async fn execute_route(
        &self,
        plan: &AssessmentRequestPlan,
        route: &str,
    ) -> Result<String, AssessmentAdapterError> {
        let route = (!route.is_empty()).then_some(route);
        let endpoint = self.endpoint(plan, route)?;
        let expected_path = endpoint.path().to_owned();
        let expected_query = endpoint.query().map(str::to_owned);
        let response = self
            .transport
            .send(self.transport.client().get(endpoint))
            .await
            .map_err(|error| AssessmentAdapterError::Transport(TransportError::Request(error)))?;
        let status = response.status();
        let final_url = response.url().clone();
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let location = response
            .headers()
            .get(LOCATION)
            .map(|value| value.to_str().map(str::to_owned))
            .transpose()
            .map_err(|_| AssessmentAdapterError::UnexpectedOrigin)?;
        let location_target = location
            .as_deref()
            .map(|value| resolve_location(&final_url, value))
            .transpose()?;
        if status == StatusCode::UNAUTHORIZED
            || status == StatusCode::FORBIDDEN
            || looks_like_login_url(&final_url)
            || location_target.as_ref().is_some_and(|target| {
                same_origin(&self.base_url, target) && looks_like_login_url(target)
            })
        {
            return Err(AssessmentAdapterError::SessionExpired);
        }
        if !same_origin(&self.base_url, &final_url)
            || location_target
                .as_ref()
                .is_some_and(|target| !same_origin(&self.base_url, target))
        {
            return Err(AssessmentAdapterError::UnexpectedOrigin);
        }
        if location_target.as_ref().is_some_and(|target| {
            target.fragment().is_some()
                || target.query().is_some()
                || !path_within_base(&self.base_url, target)
        }) {
            return Err(AssessmentAdapterError::UnexpectedPath);
        }
        let body = crate::telemetry::timing::read_text(response)
            .await
            .map_err(|error| AssessmentAdapterError::Transport(TransportError::Decode(error)))?;
        if body.len() > MAX_HTML_BYTES {
            return Err(AssessmentAdapterError::UnexpectedDeployment);
        }
        // The service's own session-expiry banner and its closed-window answer
        // both arrive as a 200 body, so they are classified here, before the
        // HTML scanner can turn them into a "no table" failure.
        match campus_html::classify_page(&body) {
            PageClass::Login | PageClass::Expired => {
                return Err(AssessmentAdapterError::SessionExpired);
            }
            PageClass::Unknown => {}
        }
        if body.contains(ASSESSMENT_NOT_OPEN_MARKER) {
            return Err(AssessmentAdapterError::NotOpen);
        }
        if status != StatusCode::OK {
            return Err(AssessmentAdapterError::HttpStatus { status });
        }
        if final_url.path() != expected_path
            || final_url.query().map(str::to_owned) != expected_query
            || final_url.fragment().is_some()
            || !path_within_base(&self.base_url, &final_url)
        {
            return Err(AssessmentAdapterError::UnexpectedPath);
        }
        if !is_html_content_type(content_type.as_deref()) {
            return Err(AssessmentAdapterError::UnexpectedContentType);
        }
        Ok(body)
    }

    /// Builds the request URL for one plan.
    ///
    /// `route` replaces the plan's path and query when the plan names a page:
    /// the form route comes from the service's own action call and carries a
    /// query, so it is validated as one unit here rather than being split
    /// between the constant path validation and a loose query check.
    fn endpoint(
        &self,
        plan: &AssessmentRequestPlan,
        route: Option<&str>,
    ) -> Result<Url, AssessmentAdapterError> {
        let (path, query) = match route {
            Some(route) => route_parts(route).ok_or(AssessmentAdapterError::UnknownForm)?,
            None => {
                if !valid_relative_path(plan.path) || plan.query.chars().any(char::is_control) {
                    return Err(AssessmentAdapterError::InvalidBaseUrl);
                }
                (
                    plan.path,
                    Some(plan.query).filter(|query| !query.is_empty()),
                )
            }
        };
        let mut endpoint = self.base_url.clone();
        let base_path = endpoint.path().trim_end_matches('/');
        endpoint.set_path(&format!("{base_path}{path}"));
        endpoint.set_query(query);
        endpoint.set_fragment(None);
        Ok(endpoint)
    }

    /// Reads and classifies the form page named by one validated row route.
    async fn execute_page(
        &self,
        plan: &AssessmentRequestPlan,
        route: &str,
    ) -> Result<String, AssessmentAdapterError> {
        // The route is re-validated here as well as in `endpoint`, because this
        // is the only place in the module where the URL in flight is not a
        // module constant.
        if route_parts(route).is_none() {
            return Err(AssessmentAdapterError::UnknownForm);
        }
        self.execute_route(plan, route).await
    }

    /// Dispatches one submission and classifies the service's answer.
    ///
    /// The only accepted completion is the service's own JSON object with
    /// `result` set to `success`.  Everything else — a reworded result, an
    /// error message, an HTML page, a transport failure after the body left —
    /// is an unconfirmed outcome, which this module never resolves by asking
    /// again.
    async fn submit_body(
        &self,
        plan: &AssessmentRequestPlan,
        body: String,
    ) -> Result<(), AssessmentAdapterError> {
        let endpoint = self.endpoint(plan, None)?;
        let expected_path = endpoint.path().to_owned();
        let response = self
            .transport
            .send(
                self.transport
                    .client()
                    .post(endpoint)
                    .header(
                        CONTENT_TYPE,
                        "application/x-www-form-urlencoded; charset=UTF-8",
                    )
                    .body(body),
            )
            .await
            .map_err(|_| AssessmentAdapterError::SubmitUnconfirmed)?;
        let status = response.status();
        let final_url = response.url().clone();
        if status == StatusCode::UNAUTHORIZED
            || status == StatusCode::FORBIDDEN
            || looks_like_login_url(&final_url)
        {
            return Err(AssessmentAdapterError::SessionExpired);
        }
        if !same_origin(&self.base_url, &final_url)
            || final_url.path() != expected_path
            || final_url.fragment().is_some()
            || !path_within_base(&self.base_url, &final_url)
        {
            return Err(AssessmentAdapterError::SubmitUnconfirmed);
        }
        let bytes =
            crate::telemetry::timing::read_bounded_bytes(response, MAX_SUBMIT_RESPONSE_BYTES)
                .await
                .map_err(|_| AssessmentAdapterError::SubmitUnconfirmed)?;
        let text = String::from_utf8_lossy(&bytes);
        match campus_html::classify_page(&text) {
            PageClass::Login | PageClass::Expired => {
                return Err(AssessmentAdapterError::SessionExpired);
            }
            PageClass::Unknown => {}
        }
        match submission_result(&text) {
            Some(true) => Ok(()),
            Some(false) => Err(AssessmentAdapterError::SubmitUnconfirmed),
            None => Err(AssessmentAdapterError::SubmitUnconfirmed),
        }
    }

    fn map_parse_error(error: AssessmentParseError) -> AssessmentAdapterError {
        match error {
            AssessmentParseError::LoginPage | AssessmentParseError::ExpiredPage => {
                AssessmentAdapterError::SessionExpired
            }
            AssessmentParseError::NotOpen => AssessmentAdapterError::NotOpen,
            other => AssessmentAdapterError::Parse(other),
        }
    }
}

impl fmt::Debug for AssessmentAdapter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AssessmentAdapter")
            .field("scheme", &self.base_url.scheme())
            .field("host", &self.base_url.host_str())
            .field("has_opaque_path", &(!self.base_url.path().is_empty()))
            .field("profile", &self.profile)
            .field("route_generation", &self.route_generation())
            .field("transport", &"[cookie-aware transport]")
            .finish()
    }
}

/// Runs the session-level guards that must precede any structural parse.
fn guard_page(html: &str) -> Result<&str, AssessmentParseError> {
    let trimmed = html.strip_prefix('\u{feff}').unwrap_or(html).trim();
    if trimmed.is_empty() {
        return Err(AssessmentParseError::EmptyBody);
    }
    match campus_html::classify_page(trimmed) {
        PageClass::Login => return Err(AssessmentParseError::LoginPage),
        PageClass::Expired => return Err(AssessmentParseError::ExpiredPage),
        PageClass::Unknown => {}
    }
    if trimmed.contains(ASSESSMENT_NOT_OPEN_MARKER) {
        return Err(AssessmentParseError::NotOpen);
    }
    Ok(trimmed)
}

/// Parses one questionnaire list page into its rows.
///
/// Every accepted row must carry the inline `Body('…')` action, and the route
/// it names must be a plain path inside this service.  That anchor is what
/// makes a positional read safe: a row whose columns moved cannot satisfy it,
/// so the page is reported as unrecognized instead of being read with one
/// column's value standing in for another's.
///
/// A closed questionnaire window and an empty list are both errors, not an
/// empty result: see the module documentation.
pub fn parse_assessment_list_html(html: &str) -> Result<AssessmentListRows, AssessmentParseError> {
    let trimmed = guard_page(html)?;
    let bodies = campus_html::scan(trimmed, "tbody")?;
    if bodies.is_empty() {
        return Err(AssessmentParseError::MissingTable);
    }
    let mut rows = Vec::new();
    for body in &bodies {
        for row in campus_html::direct_children(body.inner(), "tr")? {
            if rows.len() >= MAX_ITEMS {
                return Err(AssessmentParseError::TooLarge);
            }
            let index = rows.len();
            let cells = campus_html::direct_children(row.inner(), "td")?;
            if cells.len() < MIN_ROW_CELLS {
                return Err(AssessmentParseError::UnrecognizedRow { row: index });
            }
            let name = cells[NAME_CELL].text();
            if name.is_empty() {
                return Err(AssessmentParseError::MissingName { row: index });
            }
            let route = action_route(&cells[ACTION_CELL])
                .ok_or(AssessmentParseError::MissingAction { row: index })?;
            if route_parts(&route).is_none() {
                return Err(AssessmentParseError::InvalidRoute { row: index });
            }
            rows.push(AssessmentRow {
                name: bounded_name(&name),
                evaluated: cells[EVALUATED_CELL].text() == EVALUATED_TEXT,
                route,
            });
        }
    }
    if rows.is_empty() {
        return Err(AssessmentParseError::EmptyList);
    }
    Ok(AssessmentListRows { rows })
}

/// Extracts the form route named by a row's action cell.
///
/// The legacy page writes an inline `javascript:…Body('/path…') })` call.  The
/// route is taken only when both delimiters are present and ordered, so a
/// reworded action is a missing action rather than a sliced-up fragment.
fn action_route(cell: &RawElement) -> Option<String> {
    let source = cell.inner();
    let start = source.find(ACTION_CALL_OPEN)? + ACTION_CALL_OPEN.len();
    let end = source[start..].find(ACTION_CALL_CLOSE)? + start;
    if end <= start {
        return None;
    }
    let route = source[start..end].trim();
    if route.is_empty() || route.len() > MAX_ROUTE_CHARS {
        return None;
    }
    Some(route.to_owned())
}

/// Keeps one course name inside the bounded public shape.
fn bounded_name(value: &str) -> String {
    value.chars().take(MAX_NAME_CHARS).collect()
}

/// Parses the questionnaire form page named by one list row.
///
/// The form is a legacy JSP page whose structure the submission depends on, so
/// three things are required before any value is accepted:
///
/// * the transaction container, the overall score input and the transaction
///   identifier must all be present — without the identifier a stored answer
///   would not belong to the page it was read from;
/// * at least one person table must be present, because a questionnaire with no
///   questions cannot be a completed one;
/// * every score input must be an integer in the range the service accepts, so
///   a page this module cannot score is reported as unrecognized rather than
///   posted with an empty score.
///
/// A page that fails these is a changed deployment, not a form with fewer
/// fields.
pub(crate) fn parse_assessment_form_html(
    html: &str,
) -> Result<AssessmentFormBody, AssessmentParseError> {
    let trimmed = guard_page(html)?;
    let basics = campus_html::scan_with_id(trimmed, "div", BASICS_CONTAINER_ID)?
        .into_iter()
        .next()
        .ok_or(AssessmentParseError::MissingForm)?
        .inner()
        .to_owned();
    let transaction = transaction_fields(&basics)?;
    let suggestion = overall_suggestion(trimmed)?;
    let (score_name, score) = overall_score(trimmed)?;
    let panes = campus_html::scan_with_class(trimmed, "div", PERSON_TABLE_CONTAINER_CLASS)?;
    let teachers = parse_person_tables(panes.first(), AssessmentPersonRole::Teacher)?;
    let assistants = parse_person_tables(panes.get(2), AssessmentPersonRole::Assistant)?;
    if teachers.is_empty() && assistants.is_empty() {
        return Err(AssessmentParseError::EmptyForm);
    }
    Ok(AssessmentFormBody {
        transaction,
        suggestion,
        score_name,
        score,
        teachers,
        assistants,
    })
}

/// The submission state one form page carried, before the adapter binds it to
/// the row and the list generation it was read from.
///
/// It is crate-visible rather than public: the only way to obtain a fillable
/// form is through [`AssessmentAdapter::read_form`], which is what binds the
/// answers to a row and a list generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AssessmentFormBody {
    pub(crate) transaction: Vec<AssessmentFormField>,
    pub(crate) suggestion: Option<AssessmentFormField>,
    pub(crate) score_name: String,
    pub(crate) score: u32,
    pub(crate) teachers: Vec<AssessmentPerson>,
    pub(crate) assistants: Vec<AssessmentPerson>,
}

impl AssessmentFormBody {
    /// Binds these answers to the form shape a test can edit.
    ///
    /// It exists for the module's own tests, which exercise the body builder
    /// without going through a live adapter.
    #[cfg(test)]
    pub(crate) fn into_form(self) -> AssessmentForm {
        AssessmentForm {
            transaction: self.transaction,
            suggestion: self.suggestion,
            score_name: self.score_name,
            score: self.score,
            teachers: self.teachers,
            assistants: self.assistants,
        }
    }
}

/// Reads the transaction inputs, requiring the transaction identifier.
///
/// These values are the service's own.  They are carried verbatim into the
/// submission and are never exposed as writable, because the identifier among
/// them is what makes the stored answer belong to this page.
fn transaction_fields(fragment: &str) -> Result<Vec<AssessmentFormField>, AssessmentParseError> {
    let mut fields = Vec::new();
    for element in campus_html::scan(fragment, "input")? {
        if fields.len() > MAX_FORM_FIELDS {
            return Err(AssessmentParseError::TooLarge);
        }
        let Some(name) = element.attr("name").filter(|name| !name.is_empty()) else {
            continue;
        };
        if !valid_field_component(name) {
            return Err(AssessmentParseError::UnrecognizedField);
        }
        let value = element.attr("value").unwrap_or_default();
        if !valid_field_component(value) {
            return Err(AssessmentParseError::UnrecognizedField);
        }
        fields.push(AssessmentFormField {
            name: name.to_owned(),
            value: value.to_owned(),
        });
    }
    if !fields
        .iter()
        .any(|field| field.name.eq_ignore_ascii_case(SUBMIT_TOKEN_NAME))
    {
        return Err(AssessmentParseError::MissingForm);
    }
    Ok(fields)
}

/// Reads the overall written comment, whose value is the element's own text.
fn overall_suggestion(fragment: &str) -> Result<Option<AssessmentFormField>, AssessmentParseError> {
    let mut candidates = campus_html::scan(fragment, "div")?;
    candidates.extend(campus_html::scan(fragment, "span")?);
    candidates.extend(campus_html::scan(fragment, "textarea")?);
    let Some(element) = candidates
        .into_iter()
        .find(|element| element.attr("id") == Some(OVERALL_COMMENT_ELEMENT_ID))
    else {
        return Ok(None);
    };
    let text = element.text();
    if !valid_field_component(&text) {
        return Err(AssessmentParseError::UnrecognizedField);
    }
    Ok(Some(AssessmentFormField {
        name: OVERALL_COMMENT_INPUT_NAME.to_owned(),
        value: text,
    }))
}

/// Reads the overall score input and the name the endpoint expects it under.
fn overall_score(fragment: &str) -> Result<(String, u32), AssessmentParseError> {
    let element = campus_html::scan(fragment, "input")?
        .into_iter()
        .find(|element| element.attr("id") == Some(OVERALL_SCORE_ID))
        .ok_or(AssessmentParseError::MissingForm)?;
    let name = element
        .attr("name")
        .filter(|name| valid_field_component(name) && !name.is_empty())
        .ok_or(AssessmentParseError::MissingForm)?;
    let value = element.attr("value").unwrap_or_default();
    let score = parse_score(value).ok_or(AssessmentParseError::InvalidScore)?;
    Ok((name.to_owned(), score))
}

/// Reads the person tables one container holds.
///
/// The container may hold one table per person as siblings, so only direct
/// children are read: a table nested inside another one is part of that
/// person's question layout, not a second person.
fn parse_person_tables(
    container: Option<&RawElement>,
    role: AssessmentPersonRole,
) -> Result<Vec<AssessmentPerson>, AssessmentParseError> {
    let Some(container) = container else {
        return Ok(Vec::new());
    };
    let mut persons = Vec::new();
    for table in campus_html::direct_children(container.inner(), "table")? {
        if persons.len() >= MAX_FORM_PERSONS {
            return Err(AssessmentParseError::TooLarge);
        }
        if let Some(person) = parse_person_table(&table, role)? {
            persons.push(person);
        }
    }
    Ok(persons)
}

/// Reads one person's table: the name, then one row per question.
fn parse_person_table(
    table: &RawElement,
    role: AssessmentPersonRole,
) -> Result<Option<AssessmentPerson>, AssessmentParseError> {
    let rows = campus_html::direct_children(table.inner(), "tbody")?;
    let rows = match rows.first() {
        Some(body) => campus_html::direct_children(body.inner(), "tr")?,
        None => Vec::new(),
    };
    let Some(first) = rows.first() else {
        return Ok(None);
    };
    let name = campus_html::direct_children(first.inner(), "td")?
        .into_iter()
        .find_map(|cell| {
            let text = cell.text();
            (!text.is_empty()).then_some(text)
        })
        .ok_or(AssessmentParseError::UnrecognizedField)?;
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(AssessmentParseError::TooLarge);
    }
    let mut questions = Vec::new();
    for row in rows.iter().skip(1) {
        if questions.len() >= MAX_FORM_QUESTIONS {
            return Err(AssessmentParseError::TooLarge);
        }
        if let Some(question) = parse_question_row(row)? {
            questions.push(question);
        }
    }
    if questions.is_empty() {
        return Ok(None);
    }
    Ok(Some(AssessmentPerson {
        name,
        role,
        questions,
    }))
}

/// Reads one question row into its text, its comment, its score, and the
/// remaining values the service submits with it.
///
/// Which input is which comes from the row's own markup, exactly as the
/// service renders it: the comment is the input carrying a class, the score is
/// the input the layout wraps in a `<ul>`, and everything else is carried
/// verbatim.  A row that does not have exactly one of each is a changed
/// layout, and it is reported as unreadable rather than guessed at — a score
/// written into the comment's name would be a wrong answer, not a lost one.
fn parse_question_row(row: &RawElement) -> Result<Option<AssessedQuestion>, AssessmentParseError> {
    let cells = campus_html::direct_children(row.inner(), "td")?;
    if cells.is_empty() {
        return Ok(None);
    }
    // The service labels the question in the second cell of a four-cell row,
    // and in the first cell otherwise.
    let text = if cells.len() == 4 {
        cells.get(1)
    } else {
        cells.first()
    }
    .map(RawElement::text)
    .unwrap_or_default();
    let mut score_name = None;
    for list in campus_html::scan(row.inner(), "ul")? {
        for input in campus_html::direct_children(list.inner(), "input")? {
            let Some(name) = input.attr("name").filter(|name| !name.is_empty()) else {
                continue;
            };
            if score_name.is_some() {
                return Err(AssessmentParseError::UnrecognizedField);
            }
            score_name = Some(name.to_owned());
        }
    }
    let Some(score_name) = score_name else {
        return Ok(None);
    };
    let mut suggestion = None;
    let mut others = Vec::new();
    for input in campus_html::scan(row.inner(), "input")? {
        if Others::is_ignored(&input) {
            continue;
        }
        let Some(name) = input.attr("name").filter(|name| !name.is_empty()) else {
            continue;
        };
        if !valid_field_component(name) {
            return Err(AssessmentParseError::UnrecognizedField);
        }
        let value = input.attr("value").unwrap_or_default();
        if !valid_field_component(value) {
            return Err(AssessmentParseError::UnrecognizedField);
        }
        if name == score_name {
            continue;
        }
        let field = AssessmentFormField {
            name: name.to_owned(),
            value: value.to_owned(),
        };
        // A comment is the one input the service renders with a class.
        if input
            .attr("class")
            .is_some_and(|class| !class.trim().is_empty())
        {
            if suggestion.is_some() {
                return Err(AssessmentParseError::UnrecognizedField);
            }
            suggestion = Some(field);
            continue;
        }
        others.push(field);
    }
    Ok(Some(AssessedQuestion {
        text,
        score: ASSESSMENT_MIN_SCORE,
        score_name,
        others,
        suggestion,
    }))
}

/// Recognizes inputs the service renders but does not submit.
struct Others;

impl Others {
    fn is_ignored(element: &RawElement) -> bool {
        element.attr("type").is_some_and(|kind| {
            matches!(
                kind.trim().to_ascii_lowercase().as_str(),
                "button" | "submit" | "reset" | "image" | "file"
            )
        })
    }
}

/// Reads the service's answer to a submission.
///
/// The endpoint answers a small JSON object.  `Some(true)` means the service
/// stored the questionnaire, `Some(false)` means it explicitly declined, and
/// `None` means the answer was not the shape this endpoint is known to send —
/// which is an unconfirmed outcome, not a success and not a rejection.
fn submission_result(body: &str) -> Option<bool> {
    let trimmed = body.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_SUBMIT_RESPONSE_BYTES {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    let result = value.get("result")?.as_str()?;
    Some(result == SUBMIT_SUCCESS_RESULT)
}

/// Parses one score the way the reference does: a plain decimal integer and
/// nothing else.
fn parse_score(value: &str) -> Option<u32> {
    let trimmed = value.trim();
    if trimmed.is_empty() || !trimmed.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    trimmed.parse::<u32>().ok()
}

/// Returns whether a name or value may be carried into a submission.
///
/// The check is deliberately narrow.  Anything the service rendered is already
/// safe to carry, and the submission encodes every pair, so the only things
/// refused here are what no encoding could make safe or what no form could
/// legitimately hold: a control character, and a value past the module's
/// length bound.  A comment containing `&`, `=` or a space is ordinary text a
/// student may write, and it is encoded rather than rejected.
fn valid_field_component(value: &str) -> bool {
    value.len() <= MAX_FIELD_CHARS && !value.chars().any(char::is_control)
}

/// Percent-encodes one form component exactly as `encodeURIComponent` does.
///
/// The submission body is the service's own transaction state plus the
/// answers, and the endpoint reads it as a form.  Reproducing that encoder's
/// byte-for-byte output is what keeps an opaque transaction value reaching the
/// service unchanged; a different-but-valid encoding of the same value is not
/// something this module is in a position to assume the endpoint accepts.
fn encode_form_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        let unreserved = byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')'
            );
        if unreserved {
            out.push(*byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn is_html_content_type(content_type: Option<&str>) -> bool {
    content_type.is_none_or(|value| {
        let mime = value.split(';').next().unwrap_or_default().trim();
        mime.eq_ignore_ascii_case("text/html") || mime.eq_ignore_ascii_case("application/xhtml+xml")
    })
}

fn looks_like_login_url(url: &Url) -> bool {
    let path = url.path().to_ascii_lowercase();
    path == "/login"
        || path.ends_with("/login")
        || path.contains("/login/")
        || path.contains("/do/off/ui/auth/login")
}

fn same_origin(base_url: &Url, candidate: &Url) -> bool {
    base_url.scheme() == candidate.scheme()
        && base_url.host_str() == candidate.host_str()
        && base_url.port_or_known_default() == candidate.port_or_known_default()
        && candidate.username().is_empty()
        && candidate.password().is_none()
}

fn path_within_base(base_url: &Url, candidate: &Url) -> bool {
    let base_path = base_url.path().trim_end_matches('/');
    base_path.is_empty()
        || base_path == "/"
        || candidate.path() == base_path
        || candidate.path().starts_with(&format!("{base_path}/"))
}

fn normalize_base_url(mut base_url: Url) -> Result<Url, AssessmentAdapterError> {
    if !matches!(base_url.scheme(), "http" | "https")
        || base_url.host_str().is_none()
        || !base_url.username().is_empty()
        || base_url.password().is_some()
        || base_url.query().is_some()
        || base_url.fragment().is_some()
        || !safe_base_path(base_url.path())
    {
        return Err(AssessmentAdapterError::InvalidBaseUrl);
    }
    let path = base_url.path().trim_end_matches('/');
    let path = opaque_mapping_root(path).unwrap_or_else(|| {
        if path.is_empty() {
            "/".to_owned()
        } else {
            format!("{path}/")
        }
    });
    base_url.set_path(&path);
    Ok(base_url)
}

fn opaque_mapping_root(path: &str) -> Option<String> {
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    let scheme = segments.next()?;
    if !matches!(scheme, "http" | "https") {
        return None;
    }
    let token = segments.next()?;
    Some(format!("/{scheme}/{token}/"))
}

fn resolve_location(response_url: &Url, location: &str) -> Result<Url, AssessmentAdapterError> {
    if location.chars().any(char::is_control) || invalid_percent_encoding(location) {
        return Err(AssessmentAdapterError::UnexpectedOrigin);
    }
    let target = Url::parse(location)
        .or_else(|_| response_url.join(location))
        .map_err(|_| AssessmentAdapterError::UnexpectedOrigin)?;
    if !target.username().is_empty() || target.password().is_some() {
        return Err(AssessmentAdapterError::UnexpectedOrigin);
    }
    Ok(target)
}

fn safe_base_path(path: &str) -> bool {
    !path.contains(['\\', '?', '#'])
        && !path.contains("..")
        && !path.chars().any(char::is_control)
        && !invalid_percent_encoding(path)
        && !path_contains_encoded_escape(path)
}

fn valid_relative_path(path: &str) -> bool {
    path.starts_with('/')
        && path.len() <= MAX_ROUTE_CHARS
        && !path.contains("://")
        && !path.contains(['?', '#', '\\'])
        && !path.contains("..")
        && !path.chars().any(char::is_control)
        && !invalid_percent_encoding(path)
        && !path_contains_encoded_escape(path)
}

/// Splits one row's named route into its path and its query, both validated.
///
/// The legacy action is a server-rendered relative URL carrying a query, so a
/// route's query is accepted here — unlike the request plans, whose query is a
/// module constant.  Splitting it in one place is what keeps the parts from
/// being validated under looser rules than the plan they end up in.
fn route_parts(route: &str) -> Option<(&str, Option<&str>)> {
    if !route.starts_with('/')
        || route.len() > MAX_ROUTE_CHARS
        || route.contains("://")
        || route.contains(['#', '\\'])
        || route.chars().any(char::is_control)
    {
        return None;
    }
    let (path, query) = match route.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (route, None),
    };
    if !valid_relative_path(path) {
        return None;
    }
    if let Some(query) = query {
        if query.is_empty()
            || query.chars().any(char::is_control)
            || invalid_percent_encoding(query)
            || path_contains_encoded_escape(query)
        {
            return None;
        }
    }
    Some((path, query))
}

fn invalid_percent_encoding(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.iter().enumerate().any(|(index, byte)| {
        *byte == b'%'
            && (index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit())
    })
}

fn path_contains_encoded_escape(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.windows(3).any(|window| {
        window[0] == b'%'
            && hex_value(window[1])
                .zip(hex_value(window[2]))
                .is_some_and(|(high, low)| {
                    matches!((high << 4) | low, b'.' | b'/' | b'\\' | 0x00..=0x1f | 0x7f)
                })
    })
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}
