//! A short-lived, account-bound selector for read-only Learn homework.
//! Raw student/base IDs remain in this Runtime and never enter a Flutter DTO.

use super::*;
use crate::{campus_live::CampusTodoFailureKind, learn_homework::HomeworkDetailError};

const MAX_HOMEWORK: usize = 500;
const SELECTOR_AGE: Duration = Duration::from_secs(300);

#[derive(Default)]
#[flutter_rust_bridge::frb(ignore)]
pub(super) struct HomeworkRuntimeState {
    owner: Option<UserIdentity>,
    transport: Option<crate::transport::CampusHttpTransport>,
    selected_at: Option<std::time::Instant>,
    selections: HashMap<String, HomeworkSelection>,
}

#[derive(Clone)]
struct HomeworkSelection {
    course_id: String,
    student_id: String,
    base_id: Option<String>,
}

impl HomeworkRuntimeState {
    pub(super) fn clear(&mut self) {
        self.selected_at = None;
        self.selections.clear();
    }

    fn bind(&mut self, user: &UserIdentity, transport: &crate::transport::CampusHttpTransport) {
        if self.owner.as_ref() != Some(user)
            || self
                .transport
                .as_ref()
                .is_none_or(|old| !Arc::ptr_eq(old.cookie_jar(), transport.cookie_jar()))
        {
            *self = Self {
                owner: Some(user.clone()),
                transport: Some(transport.clone()),
                ..Self::default()
            };
        }
    }

    fn selected(&self, user: &UserIdentity, selector: &str) -> Option<&HomeworkSelection> {
        (self.owner.as_ref() == Some(user)
            && self
                .selected_at
                .is_some_and(|at| at.elapsed() < SELECTOR_AGE))
        .then(|| self.selections.get(selector))
        .flatten()
    }
}

pub(super) async fn prove_source(
    runtime: &mut CampusRuntime,
) -> Result<(UserIdentity, Arc<CampusLiveDataSource>), String> {
    runtime.allow_live_operation()?;
    let user = runtime.ensure_identity_user_for_live_read().await?;
    runtime.apply_reference_academic_stage(&user);
    if !runtime.service_session_is_proven(ServiceId::Learn) {
        runtime
            .ensure_academic_reader_session(ServiceId::Learn)
            .await?;
    }
    if !runtime.service_session_is_proven(ServiceId::Identity)
        || !runtime.service_session_is_proven(ServiceId::Learn)
        || runtime
            .coordinator
            .registry()
            .snapshot_for(ServiceId::Learn)
            .user
            .as_ref()
            != Some(&user)
    {
        return runtime.fail("学习平台服务会话未建立，请重新建立");
    }
    let source = runtime
        .learn_source
        .clone()
        .ok_or("学习平台服务会话未建立，请重新建立")?;
    if !Arc::ptr_eq(
        source.registrar().transport().cookie_jar(),
        runtime.identity.transport().cookie_jar(),
    ) {
        return runtime.fail("网络学堂账号会话未确认，请重新登录");
    }
    Ok((user, source))
}

pub(super) async fn list(
    runtime: &mut CampusRuntime,
    course_id: String,
) -> Result<LearnHomeworkListDto, String> {
    let course_id = course_id.trim().to_owned();
    if course_id.is_empty() || course_id.len() > 256 || course_id.chars().any(char::is_control) {
        return runtime.fail("网络学堂课程 ID 无效");
    }
    if runtime.learn_course_records.is_none() {
        // A saved display cache can populate learn_course_ids. It cannot
        // authorize a live homework read until this Runtime has a fresh list.
        let evidence = runtime.capture_validation_learn_courses().await?;
        if !evidence.ids().iter().any(|id| id == &course_id) {
            return runtime.fail("网络学堂课程不在当前真实课程列表中");
        }
    } else if !runtime.learn_course_ids.iter().any(|id| id == &course_id) {
        return runtime.fail("网络学堂课程不在当前真实课程列表中");
    }
    let (user, source) = prove_source(runtime).await?;
    runtime
        .learn_homework
        .bind(&user, runtime.identity.transport());
    runtime.learn_homework.clear();
    let records = match source.list_learn_homework(&course_id).await {
        Ok(records) => records,
        Err(CampusTodoFailureKind::SessionExpired) => {
            runtime.invalidate_learn_session();
            return runtime.fail("学习平台服务会话已过期，请重新建立");
        }
        Err(CampusTodoFailureKind::Transport) => {
            return runtime.fail("网络学堂课程作业网络连接失败，请稍后重试");
        }
        Err(CampusTodoFailureKind::HttpStatus) => {
            return runtime.fail("网络学堂课程作业请求失败，请稍后重试");
        }
        Err(CampusTodoFailureKind::RouteDrift) => {
            return runtime.fail("网络学堂课程作业来源路径未确认，已停止读取");
        }
        Err(CampusTodoFailureKind::BusinessFailure | CampusTodoFailureKind::MalformedResponse) => {
            return runtime.fail("网络学堂课程作业数据格式未确认，请稍后重试");
        }
    };
    if records.len() >= MAX_HOMEWORK {
        return runtime.fail("课程作业达到单次读取上限，不能确认列表完整");
    }
    runtime.allow_live_operation()?;
    if !runtime.service_session_is_proven(ServiceId::Identity)
        || !runtime.service_session_is_proven(ServiceId::Learn)
        || runtime
            .coordinator
            .registry()
            .snapshot_for(ServiceId::Learn)
            .user
            .as_ref()
            != Some(&user)
    {
        return runtime.fail("网络学堂账号会话未确认，请重新登录");
    }
    let mut items = Vec::with_capacity(records.len());
    let mut selections = HashMap::new();
    for record in records {
        let (Some(student_id), Some(title), Some(due_at)) =
            (record.student_id, record.title, record.due_at)
        else {
            return runtime.fail("网络学堂课程作业数据格式未确认，请稍后重试");
        };
        let selector = Uuid::new_v4().to_string();
        selections.insert(
            selector.clone(),
            HomeworkSelection {
                course_id: course_id.clone(),
                student_id,
                base_id: record.base_id.clone(),
            },
        );
        items.push(LearnHomeworkDto {
            selector,
            title,
            state: match record.bucket {
                crate::learn_todos::HomeworkBucket::Pending => "pending",
                crate::learn_todos::HomeworkBucket::Submitted => "submitted",
                crate::learn_todos::HomeworkBucket::Graded => "graded",
            }
            .into(),
            due_at: due_at.to_rfc3339(),
            late_due_at: record.late_due_at.map(|date| date.to_rfc3339()),
            submitted_at: record.submitted_at.map(|date| date.to_rfc3339()),
            graded_at: record.graded_at.map(|date| date.to_rfc3339()),
            detail_available: record.base_id.is_some(),
        });
    }
    runtime.learn_homework.selections = selections;
    runtime.learn_homework.selected_at = Some(std::time::Instant::now());
    runtime.last_error = None;
    runtime.persist_resume_state_after_live_read(&user, "learn");
    Ok(LearnHomeworkListDto {
        course_id,
        items,
        generated_at: Utc::now().to_rfc3339(),
        source: "live".into(),
        status: "ready".into(),
    })
}

pub(super) async fn detail(
    runtime: &mut CampusRuntime,
    selector: String,
) -> Result<LearnHomeworkDetailDto, String> {
    let (user, source) = prove_source(runtime).await?;
    let selected = runtime
        .learn_homework
        .selected(&user, &selector)
        .cloned()
        .ok_or("请先刷新当前课程作业列表，再打开详情")?;
    if !runtime
        .learn_homework
        .transport
        .as_ref()
        .is_some_and(|transport| {
            Arc::ptr_eq(
                transport.cookie_jar(),
                runtime.identity.transport().cookie_jar(),
            )
        })
    {
        runtime.learn_homework.clear();
        return runtime.fail("网络学堂账号会话未确认，请重新登录");
    }
    let base_id = selected
        .base_id
        .as_deref()
        .ok_or("本项作业未提供详情 ID，请在网络学堂查看")?;
    let result = match source
        .read_learn_homework_detail(&selected.course_id, &selected.student_id, base_id)
        .await
    {
        Ok(result) => result,
        Err(HomeworkDetailError::Session) => {
            runtime.invalidate_learn_session();
            return runtime.fail("学习平台服务会话已过期，请重新建立");
        }
        Err(HomeworkDetailError::Route) => {
            return runtime.fail("网络学堂作业详情来源路径未确认，已停止读取");
        }
        Err(HomeworkDetailError::Network) => {
            return runtime.fail("网络学堂作业详情网络连接失败，请稍后重试");
        }
        Err(HomeworkDetailError::Http(_)) => {
            return runtime.fail("网络学堂作业详情请求失败，请稍后重试");
        }
        Err(HomeworkDetailError::Response) => {
            return runtime.fail("网络学堂作业详情数据格式未确认，请稍后重试");
        }
    };
    runtime.allow_live_operation()?;
    if !runtime.service_session_is_proven(ServiceId::Identity)
        || !runtime.service_session_is_proven(ServiceId::Learn)
        || runtime
            .coordinator
            .registry()
            .snapshot_for(ServiceId::Learn)
            .user
            .as_ref()
            != Some(&user)
    {
        return runtime.fail("网络学堂账号会话未确认，请重新登录");
    }
    runtime.last_error = None;
    runtime.persist_resume_state_after_live_read(&user, "learn");
    Ok(LearnHomeworkDetailDto {
        selector,
        description: result.description,
        answer_content: result.answer_content,
        submitted_content: result.submitted_content,
        attachments: result
            .attachments
            .into_iter()
            .map(|file| LearnHomeworkAttachmentDto {
                kind: file.kind.into(),
                name: file.name,
                size: file.size,
            })
            .collect(),
        generated_at: Utc::now().to_rfc3339(),
        source: "live".into(),
        status: "ready".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_repair_homework_selector_dies_on_account_or_cookie_change() {
        let owner = UserIdentity {
            username: "fixture-owner".into(),
            display_name: None,
        };
        let other = UserIdentity {
            username: "fixture-other".into(),
            display_name: None,
        };
        let transport = crate::transport::CampusHttpTransport::new("fixture-homework").unwrap();
        let mut state = HomeworkRuntimeState::default();
        state.bind(&owner, &transport);
        state.selected_at = Some(std::time::Instant::now());
        state.selections.insert(
            "selector".into(),
            HomeworkSelection {
                course_id: "course".into(),
                student_id: "private-student".into(),
                base_id: Some("private-base".into()),
            },
        );
        assert!(state.selected(&owner, "selector").is_some());
        assert!(state.selected(&other, "selector").is_none());
        state.bind(&other, &transport);
        assert!(state.selected(&other, "selector").is_none());
        state.selected_at = Some(std::time::Instant::now());
        state.selections.insert(
            "second".into(),
            HomeworkSelection {
                course_id: "course".into(),
                student_id: "private-student".into(),
                base_id: None,
            },
        );
        state.bind(
            &other,
            &crate::transport::CampusHttpTransport::new("other-jar").unwrap(),
        );
        assert!(state.selected(&other, "second").is_none());
    }
}
