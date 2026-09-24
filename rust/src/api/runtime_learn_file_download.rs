//! One file is resolved from a fresh, account-bound course list immediately
//! before download. A Flutter selector can never become an upstream file ID.

use super::*;
use crate::{
    learn_file_download::{
        LearnFileDownloadError, checked_destination, probe_download, save_download,
    },
    learn_files::LearnFileError,
};

pub(super) async fn save(
    runtime: &mut CampusRuntime,
    course_id: String,
    file_id: String,
    output_path: String,
) -> Result<LearnFileDownloadDto, String> {
    let bytes = perform(runtime, &course_id, &file_id, Some(&output_path)).await?;
    Ok(LearnFileDownloadDto {
        course_id,
        file_id,
        bytes_written: bytes,
        generated_at: Utc::now().to_rfc3339(),
        source: "live".into(),
        status: "ready".into(),
    })
}

pub(super) async fn probe(
    runtime: &mut CampusRuntime,
    course_id: String,
    file_id: String,
) -> Result<u64, String> {
    perform(runtime, &course_id, &file_id, None).await
}

async fn perform(
    runtime: &mut CampusRuntime,
    course_id: &str,
    file_id: &str,
    output_path: Option<&str>,
) -> Result<u64, String> {
    runtime.allow_live_operation()?;
    if course_id.is_empty()
        || course_id.len() > 256
        || course_id.chars().any(char::is_control)
        || Uuid::parse_str(file_id).is_err()
    {
        return runtime.fail("网络学堂课程资料选择无效，请刷新列表");
    }
    if let Some(path) = output_path {
        let destination =
            checked_destination(path).map_err(|error| map_download_error(runtime, error))?;
        if destination.symlink_metadata().is_ok() {
            return runtime.fail("目标文件已存在，请另选保存位置");
        }
    }
    let user = runtime.ensure_identity_user_for_live_read().await?;
    runtime.apply_reference_academic_stage(&user);
    if !runtime
        .learn_course_ids
        .iter()
        .any(|known| known == course_id)
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
    let csrf = runtime
        .coordinator
        .bound_csrf(ServiceId::Learn)
        .ok_or("学习平台 CSRF 未确认")?
        .clone();
    let files = source
        .list_learn_files(course_id)
        .await
        .map_err(|error| map_list_error(runtime, error))?;
    let target = files
        .files
        .iter()
        .find(|record| record.id == file_id)
        .ok_or_else(|| fixed_download_error(runtime, "课程资料已变化，请刷新列表后重试"))?;
    let bytes = match output_path {
        Some(path) => {
            save_download(
                source.learn(),
                source.registrar().transport(),
                &csrf,
                &target.raw_file_id,
                path,
            )
            .await
        }
        None => {
            probe_download(
                source.learn(),
                source.registrar().transport(),
                &csrf,
                &target.raw_file_id,
            )
            .await
        }
    }
    .map_err(|error| map_download_error(runtime, error))?;
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
    Ok(bytes as u64)
}

fn map_list_error(runtime: &mut CampusRuntime, error: LearnFileError) -> String {
    match error {
        LearnFileError::Session => {
            runtime.invalidate_learn_session();
            fixed_download_error(runtime, "学习平台服务会话已过期，请重新建立")
        }
        LearnFileError::Network => {
            fixed_download_error(runtime, "网络学堂课程资料网络连接失败，请稍后重试")
        }
        LearnFileError::Http(_) => {
            fixed_download_error(runtime, "网络学堂课程资料请求失败，请稍后重试")
        }
        LearnFileError::Route => {
            fixed_download_error(runtime, "网络学堂课程资料来源路径未确认，已停止读取")
        }
        LearnFileError::Response => {
            fixed_download_error(runtime, "网络学堂课程资料数据格式未确认，请稍后重试")
        }
    }
}

fn map_download_error(runtime: &mut CampusRuntime, error: LearnFileDownloadError) -> String {
    match error {
        LearnFileDownloadError::Session => {
            runtime.invalidate_learn_session();
            fixed_download_error(runtime, "学习平台服务会话已过期，请重新建立")
        }
        LearnFileDownloadError::Route => {
            fixed_download_error(runtime, "网络学堂资料下载目标未确认，已停止读取")
        }
        LearnFileDownloadError::Network => {
            fixed_download_error(runtime, "网络学堂资料下载网络连接失败，请稍后重试")
        }
        LearnFileDownloadError::Http(_) => {
            fixed_download_error(runtime, "网络学堂资料下载请求失败，请稍后重试")
        }
        LearnFileDownloadError::TooLarge => {
            fixed_download_error(runtime, "课程资料超过 512 MB 下载上限")
        }
        LearnFileDownloadError::Content => {
            fixed_download_error(runtime, "网络学堂资料下载返回内容未确认，已停止保存")
        }
        LearnFileDownloadError::Exists => {
            fixed_download_error(runtime, "目标文件已存在，请另选保存位置")
        }
        LearnFileDownloadError::Storage => {
            fixed_download_error(runtime, "所选保存位置无法写入，请重新选择")
        }
    }
}

fn fixed_download_error(runtime: &mut CampusRuntime, message: &'static str) -> String {
    runtime.last_error = Some(message.to_owned());
    message.to_owned()
}
