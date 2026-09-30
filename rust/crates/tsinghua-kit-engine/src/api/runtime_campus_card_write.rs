//! Account-bound campus-card state changes: loss reporting and its reversal, the
//! transaction-password change, the spending-limit change, and the one top-up
//! form that needs no payment link.
//!
//! Every operation here shares one rule with the library's writes: the request is
//! built and dispatched exactly once, and an outcome the service did not confirm
//! is reported as *unconfirmed* rather than failed and is never sent a second
//! time.  A card write is not retried on a session error either, which is the one
//! place this module deliberately differs from the library's reservation read:
//! re-authenticating and then re-sending a state change whose answer was lost
//! would be a replay of that state change.
//!
//! Three properties hold for everything in this file:
//!
//! * **No second session.**  The card SSO the read half already proved is the
//!   only session used.  A caller with no proven card session is refused here
//!   rather than handed another authentication path.
//! * **No secret outside one request.**  A transaction password arrives as
//!   [`CampusCardSecret`], is copied into exactly one body by the adapter, and is
//!   zeroized when the request is dropped. It is never logged, cached, or
//!   returned.
//! * **No payment credential.**  The top-up implemented here is the bank form,
//!   which the card service carries out itself and which answers with its own
//!   `returncode`.  The card's payment-link top-up is not implemented at all: its
//!   only output is a payment URL, and a pay code must not enter a DTO, a log or a
//!   ledger.  See [`crate::campus_card_write::CARD_QR_TOPUP_PATH`].

use super::*;
use crate::campus_card_write::{
    CampusCardWriteOperation, CampusCardWriteOutcome, CampusCardWritePlan,
};

/// Proves the card session is a same-account, same-transport session.
///
/// The checks are the ones the card readers make — the service proof, the
/// registry's own user, and the shared Cookie jar — because a state change must
/// never open a session of its own.  The account binding is re-proved here rather
/// than assumed, so a handoff that happened during an earlier read cannot make a
/// write address another account's card.
async fn ensure_campus_card_write_session(
    runtime: &mut CampusRuntime,
) -> Result<UserIdentity, String> {
    runtime.ensure_campus_card_reader_session().await?;
    let user = runtime
        .cache_user_for_read()?
        .ok_or_else(|| "校园卡服务会话未建立".to_owned())?;
    let proven = runtime.service_session_is_proven(ServiceId::Identity)
        && runtime.service_session_is_proven(ServiceId::CampusCard);
    let same_account = runtime.card_client.as_ref().is_some_and(|client| {
        runtime
            .coordinator
            .registry()
            .snapshot_for(ServiceId::CampusCard)
            .user
            .as_ref()
            == Some(&user)
            && Arc::ptr_eq(
                client.transport().cookie_jar(),
                runtime.identity.transport().cookie_jar(),
            )
    });
    if !proven || !same_account {
        return Err(runtime.record_error("校园卡服务会话未确认，请先打开校园卡页面".to_owned()));
    }
    Ok(user)
}

/// Turns one dispatched card write into this Runtime's own answer.
///
/// `Accepted` is the only success.  `Refused` and `Unrecognized` are reported
/// separately — a refusal is a definite "nothing was applied", an unreadable
/// answer is an unknown effect — but neither is ever resolved by sending the
/// request again, and both are recorded under the card write's own failure codes
/// so the SDK layer can map them to different public error codes.
fn finish_card_write(
    runtime: &mut CampusRuntime,
    stage: &'static str,
    outcome: Result<CampusCardWriteOutcome, crate::campus_card_adapter::CampusCardAdapterError>,
) -> Result<CampusRuntimeStatusDto, String> {
    match outcome {
        Ok(CampusCardWriteOutcome::Accepted) => {
            runtime.clear_campus_card_failure_code();
            runtime.last_error = None;
            Ok(runtime.status())
        }
        Ok(CampusCardWriteOutcome::Refused) => {
            Err(runtime.record_business_failure("campus_card", stage, "card_write_refused"))
        }
        Ok(CampusCardWriteOutcome::Unrecognized) => {
            Err(runtime.record_business_failure("campus_card", stage, "card_write_unconfirmed"))
        }
        Ok(CampusCardWriteOutcome::LoginRequired) => {
            runtime.invalidate_service_session(ServiceId::CampusCard);
            Err(runtime.record_business_failure("campus_card", stage, "card_write_session_expired"))
        }
        Err(error) => {
            use crate::campus_card_adapter::CampusCardAdapterError as Error;
            if matches!(error, Error::SessionExpired) {
                runtime.invalidate_service_session(ServiceId::CampusCard);
            }
            if matches!(error, Error::AccountMismatch) {
                // A conflicting account is a binding failure rather than a
                // transient outage, and a write must not proceed on a session
                // whose account is no longer the caller's.
                runtime.invalidate_service_session(ServiceId::CampusCard);
            }
            Err(runtime.record_business_failure("campus_card", stage, error.diagnostic_code()))
        }
    }
}

/// Dispatches one card state change and records whether the service confirmed it.
pub(super) async fn apply_card_write(
    runtime: &mut CampusRuntime,
    request: CampusCardWriteRequest,
) -> Result<CampusRuntimeStatusDto, String> {
    runtime.allow_live_operation()?;
    let operation = request.operation();
    let plan: CampusCardWritePlan = request.into_plan().map_err(|_| {
        runtime.record_business_failure("campus_card", "card_write", "card_write_request")
    })?;
    debug_assert_eq!(plan.operation(), operation);
    let _user = ensure_campus_card_write_session(runtime).await?;
    // The client is an owned handle onto the shared transport, and the session
    // is borrowed only for the dispatch below.  Nothing is cloned out of the
    // session: a second handle would outlive the proof that produced it, and the
    // whole point of the owner check is that a card session cannot be copied
    // away from the runtime that proved it.
    let Some(client) = runtime.card_client.clone() else {
        return Err(runtime.record_error("校园卡服务会话未建立".to_owned()));
    };
    let outcome = match runtime.card_session.as_ref() {
        Some(session) => client.execute_write(session, &plan).await,
        None => return Err(runtime.record_error("校园卡服务会话未建立".to_owned())),
    };
    let status = finish_card_write(runtime, card_write_stage(plan.operation()), outcome)?;
    runtime.persist_identity_resume_state_after_live_read("campus_card");
    Ok(status)
}

/// The stable stage name used in this Runtime's own telemetry for one operation.
fn card_write_stage(operation: CampusCardWriteOperation) -> &'static str {
    match operation {
        CampusCardWriteOperation::ReportLoss => "card_report_loss",
        CampusCardWriteOperation::CancelLoss => "card_cancel_loss",
        CampusCardWriteOperation::ChangeTransactionPassword => "card_change_password",
        CampusCardWriteOperation::ModifySpendingLimit => "card_modify_limit",
        CampusCardWriteOperation::TopUpFromBank => "card_bank_topup",
    }
}
