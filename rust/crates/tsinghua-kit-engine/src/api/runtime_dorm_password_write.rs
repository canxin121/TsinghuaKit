//! The dormitory account password reset, dispatched once on the session the
//! electricity reads already proved.
//!
//! One rule shapes everything here, and it is the one the campus-card writes
//! share: the request leaves exactly once.  The reset is not re-authenticated and
//! re-sent after a session error, and an answer the service did not confirm is
//! reported as *unconfirmed* rather than failed, because a password change whose
//! reply was lost may already be in effect.
//!
//! Two properties hold:
//!
//! * **No second session.**  The reset form is served under the same mapping
//!   root the electricity remainder read already reaches — the reference client's
//!   own reset URL carries that mapping token — so the base URL comes from
//!   [`CampusRuntime::configured_electricity_flow`] and the transport is the
//!   identity transport that holds the proven WebVPN cookies.  The reference
//!   client reaches this route through a separate identity roam that submits the
//!   account password to the campus ID service; this engine deliberately
//!   implements no such second login, so a caller with no proven electricity
//!   session is refused here rather than handed another authentication path.
//! * **No password outside one request.**  The new password arrives as
//!   [`DormPassword`], is copied into exactly one form body by the adapter, and is
//!   zeroized when that body is dropped.  It is never logged, cached, returned, or
//!   named in a recorded failure code.

use super::*;

/// Turns one dispatched reset into this Runtime's own answer.
///
/// `Accepted` is the only success.  `Unrecognized` is this route's expected
/// answer: the service's own client discards the reply, so most resets produce no
/// readable acceptance.  It is reported as *unconfirmed* — the request left and
/// its effect is unknown — and is never resolved by sending it again.
fn finish_dorm_password_reset(
    runtime: &mut CampusRuntime,
    stage: &'static str,
    outcome: Result<DormPasswordWriteOutcome, DormPasswordWriteAdapterError>,
) -> Result<CampusRuntimeStatusDto, String> {
    match outcome {
        Ok(DormPasswordWriteOutcome::Accepted) => {
            runtime.clear_dorm_password_failure_code();
            runtime.last_error = None;
            Ok(runtime.status())
        }
        Ok(DormPasswordWriteOutcome::Unrecognized) => {
            Err(runtime.record_business_failure("dorm_password", stage, "dorm_write_unconfirmed"))
        }
        Ok(DormPasswordWriteOutcome::LoginRequired) => {
            runtime.invalidate_electricity_session();
            Err(runtime.record_business_failure(
                "dorm_password",
                stage,
                "dorm_write_session_expired",
            ))
        }
        Err(error) => {
            if error.is_session_expired() {
                runtime.invalidate_electricity_session();
            }
            Err(runtime.record_business_failure("dorm_password", stage, error.diagnostic_code()))
        }
    }
}

/// Dispatches one dormitory password reset and records whether the service
/// confirmed it.
pub(super) async fn apply_password_reset(
    runtime: &mut CampusRuntime,
    password: DormPassword,
) -> Result<CampusRuntimeStatusDto, String> {
    runtime.allow_live_operation()?;
    // The proof gate comes first: a reset must never be the operation that
    // establishes the session, because the session it would establish belongs to
    // the read half and because a write must not fan out into a fresh handoff.
    if !runtime.electricity_service_is_proven() {
        return Err(runtime.record_error("宿舍服务会话未建立，请先打开宿舍电费页面".to_owned()));
    }
    let flow = runtime.configured_electricity_flow()?;
    let adapter = match DormPasswordWriteAdapter::try_with_transport(
        flow.mapped.clone(),
        runtime.identity.transport().clone(),
    ) {
        Ok(adapter) => adapter,
        Err(error) => {
            return finish_dorm_password_reset(runtime, "dorm_reset_password", Err(error));
        }
    };
    let plan = adapter.reset_request(password);
    // The service's own form is fetched first: its hidden state is what the
    // postback must carry, so a request can only ever be the postback of the page
    // this session actually received.
    let form = match adapter.read_reset_form().await {
        Ok(form) => form,
        Err(error) => return finish_dorm_password_reset(runtime, "dorm_read_form", Err(error)),
    };
    let outcome = adapter.reset_password(&plan, &form).await;
    finish_dorm_password_reset(runtime, "dorm_reset_password", outcome)
}
