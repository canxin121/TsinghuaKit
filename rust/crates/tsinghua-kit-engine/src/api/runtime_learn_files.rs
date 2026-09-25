//! Course-file metadata is a live-only Learn read. Selection and account proof
//! are checked in the owning Runtime before reaching the fixed backend route.

use super::*;
use crate::learn_files::{LearnFileError, suggested_filename};

pub(super) async fn read(
    runtime: &mut CampusRuntime,
    course_id: String,
) -> Result<LearnFileListDto, String> {
    runtime.allow_live_operation()?;
    let course_id = course_id.trim().to_owned();
    if course_id.is_empty() || course_id.len() > 256 || course_id.chars().any(char::is_control) {
        return runtime.fail("网络学堂课程 ID 无效");
    }
    let user = runtime.ensure_identity_user_for_live_read().await?;
    runtime.apply_reference_academic_stage(&user);
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
    let read = match source.list_learn_files(&course_id).await {
        Ok(read) => read,
        Err(LearnFileError::Session) => {
            runtime.invalidate_learn_session();
            return runtime.fail("学习平台服务会话已过期，请重新建立");
        }
        Err(LearnFileError::Network) => {
            return runtime.fail("网络学堂课程资料网络连接失败，请稍后重试");
        }
        Err(LearnFileError::Http(_)) => {
            return runtime.fail("网络学堂课程资料请求失败，请稍后重试");
        }
        Err(LearnFileError::Route) => {
            return runtime.fail("网络学堂课程资料来源路径未确认，已停止读取");
        }
        Err(LearnFileError::Response) => {
            return runtime.fail("网络学堂课程资料数据格式未确认，请稍后重试");
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
    let result = LearnFileListDto {
        course_id,
        files: read
            .files
            .into_iter()
            .map(|file| {
                let filename = suggested_filename(&file.title, file.file_type.as_deref());
                LearnFileDto {
                    id: file.id,
                    title: file.title,
                    suggested_filename: filename,
                    description: file.description,
                    size: file.size,
                    uploaded_at: file.uploaded_at,
                    file_type: file.file_type,
                    category_selector: file.category_selector,
                }
            })
            .collect(),
        complete,
        generated_at: Utc::now().to_rfc3339(),
        source: "live".into(),
        status: if complete { "ready" } else { "partial" }.into(),
        error: (!complete)
            .then(|| "课程资料达到单次 200 条读取上限，列表可能未完整；请到网络学堂核对".into()),
    };
    runtime.last_error = None;
    runtime.persist_resume_state_after_live_read(&user, "learn");
    Ok(result)
}

pub(super) async fn read_categories(
    runtime: &mut CampusRuntime,
    course_id: String,
) -> Result<LearnFileCategoryListDto, String> {
    runtime.allow_live_operation()?;
    let course_id = course_id.trim().to_owned();
    if course_id.is_empty() || course_id.len() > 256 || course_id.chars().any(char::is_control) {
        return runtime.fail("网络学堂课程 ID 无效");
    }
    let user = runtime.ensure_identity_user_for_live_read().await?;
    runtime.apply_reference_academic_stage(&user);
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
    let categories = match source.list_learn_file_categories(&course_id).await {
        Ok(categories) => categories,
        Err(LearnFileError::Session) => {
            runtime.invalidate_learn_session();
            return runtime.fail("学习平台服务会话已过期，请重新建立");
        }
        Err(LearnFileError::Network) => {
            return runtime.fail("网络学堂课程资料分类网络连接失败，请稍后重试");
        }
        Err(LearnFileError::Http(_)) => {
            return runtime.fail("网络学堂课程资料分类请求失败，请稍后重试");
        }
        Err(LearnFileError::Route) => {
            return runtime.fail("网络学堂课程资料分类来源路径未确认，已停止读取");
        }
        Err(LearnFileError::Response) => {
            return runtime.fail("网络学堂课程资料分类数据格式未确认，请稍后重试");
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
    let result = LearnFileCategoryListDto {
        course_id,
        categories: categories
            .into_iter()
            .map(|category| LearnFileCategoryDto {
                selector: category.selector,
                title: category.title,
            })
            .collect(),
        generated_at: Utc::now().to_rfc3339(),
        source: "live".into(),
        status: "ready".into(),
    };
    runtime.last_error = None;
    runtime.persist_resume_state_after_live_read(&user, "learn");
    Ok(result)
}
