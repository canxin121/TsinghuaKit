//! Account-scoped access to the fixed public school-calendar origin. The
//! school-calendar server does not receive a password, token, or user ID.

use super::*;
use crate::school_calendar::SchoolCalendarReader;

pub(super) async fn read(
    runtime: &mut CampusRuntime,
    year: Option<u32>,
    semester: String,
    language: String,
) -> Result<SchoolCalendarDto, String> {
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
        return runtime.fail("学习平台账号会话未确认，请重新登录");
    }
    let source = runtime
        .learn_source
        .as_ref()
        .ok_or("学习平台服务会话未建立，请重新建立")?;
    if !Arc::ptr_eq(
        source.registrar().transport().cookie_jar(),
        runtime.identity.transport().cookie_jar(),
    ) {
        return runtime.fail("学习平台账号会话未确认，请重新登录");
    }
    let reader = SchoolCalendarReader::for_app_service(runtime.identity.transport().clone())
        .map_err(|error| {
            runtime.record_business_failure("learn", "school_calendar", error.diagnostic_code())
        })?;
    let image = reader
        .read(year, &semester, &language)
        .await
        .map_err(|error| {
            runtime.record_business_failure("learn", "school_calendar", error.diagnostic_code())
        })?;
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
        return runtime.fail("学校校历读取后账号会话已失效，请重新登录");
    }
    runtime.last_error = None;
    Ok(SchoolCalendarDto {
        latest_year: image.latest_year,
        year: image.year,
        semester: image.semester,
        language: image.language,
        image_bytes: image.image_bytes,
        generated_at: Utc::now().to_rfc3339(),
        source: "live".into(),
        status: "ready".into(),
    })
}
