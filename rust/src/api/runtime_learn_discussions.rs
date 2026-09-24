//! Live-only discussion list for one course in the current account.

use super::*;
use crate::learn_discussions::LearnDiscussionError;

pub(super) async fn read(
    runtime: &mut CampusRuntime,
    course_id: String,
) -> Result<LearnDiscussionListDto, String> {
    runtime.allow_live_operation()?;
    let course_id = course_id.trim().to_owned();
    if course_id.is_empty() || course_id.len() > 256 || course_id.chars().any(char::is_control) {
        return runtime.fail("网络学堂课程 ID 无效");
    }
    let (user, source) = learn_homework_runtime::prove_source(runtime).await?;
    if !runtime
        .learn_course_ids
        .iter()
        .any(|known| known == &course_id)
    {
        if !runtime.learn_course_ids.is_empty() {
            return runtime.fail("网络学堂课程不在当前真实课程列表中");
        }
        let courses = runtime.load_learn_courses().await?;
        if courses.status != "ready"
            || !courses
                .courses
                .iter()
                .any(|course| course.course_id == course_id)
        {
            return runtime.fail("网络学堂课程不在当前真实课程列表中");
        }
    }
    let read = match source.list_learn_discussions(&course_id).await {
        Ok(read) => read,
        Err(LearnDiscussionError::Session) => {
            runtime.invalidate_learn_session();
            return runtime.fail("学习平台服务会话已过期，请重新建立");
        }
        Err(LearnDiscussionError::Route) => {
            return runtime.fail("网络学堂课程讨论来源路径未确认，已停止读取");
        }
        Err(LearnDiscussionError::Network) => {
            return runtime.fail("网络学堂课程讨论网络连接失败，请稍后重试");
        }
        Err(LearnDiscussionError::Http(_)) => {
            return runtime.fail("网络学堂课程讨论请求失败，请稍后重试");
        }
        Err(LearnDiscussionError::Response) => {
            return runtime.fail("网络学堂课程讨论数据格式未确认，请稍后重试");
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
    let complete = read.complete;
    let result = LearnDiscussionListDto {
        course_id,
        items: read
            .items
            .into_iter()
            .map(|item| LearnDiscussionDto {
                selector: item.selector,
                title: item.title,
                publisher_name: item.publisher_name,
                published_at: item.published_at,
                last_reply_at: item.last_reply_at,
                reply_count: item.reply_count,
            })
            .collect(),
        complete,
        generated_at: Utc::now().to_rfc3339(),
        source: "live".into(),
        status: if complete { "ready" } else { "partial" }.into(),
        error: (!complete)
            .then(|| "课程讨论达到单次 200 条读取上限，列表可能未完整；请到网络学堂核对".into()),
    };
    runtime.last_error = None;
    runtime.persist_resume_state_after_live_read(&user, "learn");
    Ok(result)
}
