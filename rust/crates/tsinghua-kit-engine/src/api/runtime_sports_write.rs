//! Sports-venue state changes: the booking, the withdrawal, and the image
//! challenge the order form requires.
//!
//! Every operation here shares one rule with the other write seams: the request
//! is built and dispatched exactly once, and an outcome the service did not
//! confirm is reported as *unconfirmed* rather than failed and is never sent a
//! second time.  A venue write is not retried on a session error either, which is
//! the one place this seam deliberately differs from the venue's own reads:
//! re-authenticating and then re-sending a state change whose answer was lost
//! would be a replay of that state change.
//!
//! Four properties hold for everything in this file:
//!
//! * **No second session.**  The venue mapping the read half already proved is
//!   the only session used, and the write adapter is built from the read
//!   adapter's own base URL and transport.  Anything that would need another
//!   handoff — the funding-settlement payment chain in particular — is recorded
//!   as a boundary constant rather than reached; see
//!   [`crate::sports_write::SPORTS_PAYMENT_MAPPING_TOKEN`].
//! * **A slot must be one this Runtime returned.**  A booking names a slot by the
//!   opaque selector the latest slot read minted for the same account, never by
//!   the venue's own booking hash or by a caller-supplied identifier.  The hash,
//!   the venue and item identifiers, the date and the cost all come from that
//!   same read, so the hash only ever travels from a read result into one request
//!   body.
//! * **A reservation must be one this Runtime returned.**  A withdrawal names a
//!   reservation by the opaque selector the latest reservation read minted for
//!   the same account, exactly as the library's cancellation does.
//! * **The contact number is never a caller argument.**  It is the number the
//!   venue itself reported for this account and this Runtime read back, so a
//!   caller cannot make the venue contact a third party.  It is copied into the
//!   one request that carries it and dropped with it.

use super::*;
use crate::sports_write::{
    SPORTS_MAX_SINGLE_PAYMENT_COST, SportsCaptcha, SportsCaptchaCode, SportsPhone,
    SportsWriteAdapterError, SportsWriteOperation, SportsWriteOutcome,
};

/// Proves the venue session is a same-account, same-transport session.
///
/// The checks are the ones the venue's reads make — the INFO and venue proofs,
/// the registry's own user, and the shared Cookie jar — because a state change
/// must never open a session of its own.  The account binding is re-proved here
/// rather than assumed, so a handoff that happened during an earlier read cannot
/// make a write address another account's booking.
async fn ensure_sports_write_session(runtime: &mut CampusRuntime) -> Result<UserIdentity, String> {
    let user = runtime.ensure_identity_user_for_live_read().await?;
    runtime.ensure_sports_reader_session(&user).await?;
    let proven = runtime.sports_service_is_proven();
    let same_account = runtime.sports_adapter.as_ref().is_some_and(|adapter| {
        runtime
            .coordinator
            .registry()
            .snapshot_for(ServiceId::Info)
            .user
            .as_ref()
            == Some(&user)
            && Arc::ptr_eq(
                adapter.transport().cookie_jar(),
                runtime.identity.transport().cookie_jar(),
            )
    });
    if !proven || !same_account {
        return Err(runtime.record_error("体育场馆服务会话未确认，请先打开体育场馆页面".to_owned()));
    }
    Ok(user)
}

/// Returns the write adapter that reuses this Runtime's own venue transport.
fn write_adapter(
    runtime: &CampusRuntime,
) -> Result<crate::sports_write::SportsWriteAdapter, String> {
    let Some(adapter) = runtime.sports_adapter.as_ref() else {
        return Err("体育场馆服务会话未建立".to_owned());
    };
    adapter.write_adapter().map_err(|error| {
        // The diagnostic code is stable and carries no URL, body or token.
        format!("体育场馆写入适配器不可用（{}）", error.diagnostic_code())
    })
}

/// Turns one dispatched venue write into this Runtime's own answer.
///
/// `Accepted` is the only success.  `Refused` is the venue's own worded refusal
/// of an order, which is a definite "nothing was booked", and it is kept apart
/// from an unreadable answer.  `Unrecognized` means the request left and its
/// effect is unknown, so it is reported as unconfirmed.  None of them is ever
/// resolved by sending the request again.
fn finish_sports_write(
    runtime: &mut CampusRuntime,
    stage: &'static str,
    outcome: Result<SportsWriteOutcome, SportsWriteAdapterError>,
) -> Result<CampusRuntimeStatusDto, String> {
    match outcome {
        Ok(SportsWriteOutcome::Accepted) => {
            runtime.last_sports_failure_code = None;
            runtime.last_error = None;
            Ok(runtime.status())
        }
        Ok(SportsWriteOutcome::Refused) => {
            Err(runtime.record_business_failure("sports", stage, "sports_write_refused"))
        }
        Ok(SportsWriteOutcome::Unrecognized) => {
            Err(runtime.record_business_failure("sports", stage, "sports_write_unconfirmed"))
        }
        Ok(SportsWriteOutcome::LoginRequired) => {
            // The venue has no `ServiceId` of its own: it rides the INFO/WebVPN
            // session, so that is the session whose proof is discarded.  A caller
            // re-establishes it through the ordinary read path, never by
            // re-sending this write.
            runtime.invalidate_sports_session();
            runtime.invalidate_service_session(ServiceId::Info);
            Err(runtime.record_business_failure("sports", stage, "sports_write_session_expired"))
        }
        Err(error) => {
            if error.is_session_expired() {
                runtime.invalidate_sports_session();
                runtime.invalidate_service_session(ServiceId::Info);
            }
            Err(runtime.record_business_failure("sports", stage, error.diagnostic_code()))
        }
    }
}

/// Reads the venue's own image challenge for the order form.
///
/// It is an ordinary read on the same session, so it may be repeated — a person
/// whose first image was unreadable asks for another.  What may not be repeated
/// is the order itself.
pub(super) async fn load_sports_captcha(
    runtime: &mut CampusRuntime,
) -> Result<SportsCaptcha, String> {
    runtime.allow_live_operation()?;
    let _user = ensure_sports_write_session(runtime).await?;
    let adapter = write_adapter(runtime)?;
    adapter.read_captcha().await.map_err(|error| {
        if error.is_session_expired() {
            runtime.invalidate_sports_session();
        }
        runtime.record_business_failure("sports", "sports_captcha", error.diagnostic_code())
    })
}

/// Books one slot of a venue whose slot list this Runtime already read.
///
/// The slot is named by the opaque selector the latest read minted, so the
/// venue's own booking hash never crosses the bridge.  The hash, the venue and
/// item identifiers, the date and the cost all come from that same read; the
/// captcha is the only value a caller supplies, and it is a person's
/// transcription of an image — never something this Runtime invents, guesses or
/// re-reads.
pub(super) async fn book_slot(
    runtime: &mut CampusRuntime,
    selector: String,
    captcha: String,
) -> Result<CampusRuntimeStatusDto, String> {
    runtime.allow_live_operation()?;
    // The captcha is the caller's own value, so it is validated first: a value
    // this module will not send must not cost a session or a handoff.
    let captcha = SportsCaptchaCode::new(&captcha).map_err(|_| {
        runtime.record_business_failure("sports", "sports_make_order", "sports_write_request")
    })?;
    let user = ensure_sports_write_session(runtime).await?;
    // The slot's provenance is this Runtime's own latest slot read for the same
    // account.  A stale, unknown or other-account selector is refused before a
    // plan exists, so the venue's booking hash is never reachable from a caller.
    let Some(slot) = selected_confirmed_slot(
        runtime.sports_slot_owner.as_deref(),
        runtime.sports_slot_at,
        &runtime.sports_slot_selectors,
        &user.username,
        &selector,
    )
    .cloned() else {
        return runtime.fail("体育场馆场地选择已失效，请刷新场地资源");
    };
    // The contact number is the one the venue reported for this account and this
    // Runtime read back.  A caller cannot supply one, so it cannot make the venue
    // call a third party.
    let Some(phone) = runtime.sports_confirmed_phone.clone() else {
        return runtime.fail("体育场馆尚未登记联系电话，请先在体育场馆页面填写");
    };
    let Ok(phone) = SportsPhone::new(&phone) else {
        return runtime.fail("体育场馆登记的联系电话格式异常，请先在体育场馆页面更正");
    };
    let adapter = write_adapter(runtime)?;
    let plan = match adapter.profile().order_request(
        slot.cost.as_deref().unwrap_or("0"),
        phone,
        &slot.gym_id,
        &slot.item_id,
        &slot.date,
        captcha,
        &slot.res_hash,
    ) {
        Ok(plan) => plan,
        Err(_error) => {
            return Err(runtime.record_business_failure(
                "sports",
                "sports_make_order",
                "sports_write_request",
            ));
        }
    };
    debug_assert_eq!(plan.operation(), SportsWriteOperation::MakeOrder);
    let outcome = adapter.make_order(&plan).await;
    let status = finish_sports_write(runtime, "sports_make_order", outcome)?;
    // The venue confirmed the order, so this slot is no longer part of the
    // inventory this Runtime may book from and the caller's selector must not be
    // usable for a second dispatch.  A captcha is single-use in any case.
    runtime.sports_slot_selectors.remove(&selector);
    runtime.persist_identity_resume_state_after_live_read("sports");
    Ok(status)
}

/// Withdraws one reservation selected from this Runtime's latest reservation
/// read.
///
/// The caller names a selector this Runtime minted for the same account, never
/// the venue's own booking identifier.  An unknown, expired or other-account
/// selector is refused before any plan exists.
pub(super) async fn cancel_reservation(
    runtime: &mut CampusRuntime,
    selector: String,
) -> Result<CampusRuntimeStatusDto, String> {
    runtime.allow_live_operation()?;
    let user = ensure_sports_write_session(runtime).await?;
    let Some(book_id) = selected_info_subscription_rule(
        runtime.sports_reservation_owner.as_deref(),
        runtime.sports_reservation_at,
        &runtime.sports_reservation_selectors,
        &user.username,
        &selector,
    )
    .map(str::to_owned) else {
        return runtime.fail("体育场馆预约选择已失效，请刷新预约记录");
    };
    let adapter = write_adapter(runtime)?;
    let Ok(plan) = adapter.profile().unsubscribe_request(&book_id) else {
        return Err(runtime.record_business_failure(
            "sports",
            "sports_unsubscribe",
            "sports_write_request",
        ));
    };
    debug_assert_eq!(plan.operation(), SportsWriteOperation::Unsubscribe);
    let outcome = adapter.unsubscribe(&plan).await;
    let status = finish_sports_write(runtime, "sports_unsubscribe", outcome)?;
    // The venue confirmed the withdrawal, so this selector retires: the same row
    // cannot be withdrawn twice without a fresh reservation read.
    runtime.sports_reservation_selectors.remove(&selector);
    runtime.persist_identity_resume_state_after_live_read("sports");
    Ok(status)
}

/// The single-payment ceiling, re-exported so the FFI documentation and the
/// module's own bound cannot drift apart.
pub(super) const fn max_single_payment_cost() -> u32 {
    SPORTS_MAX_SINGLE_PAYMENT_COST
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot() -> ConfirmedSportsSlot {
        ConfirmedSportsSlot {
            res_hash: "F3681513C5BC25CEDDF5FA7C3E8429F1769DBA48B7BD42CC".to_owned(),
            cost: Some("25".to_owned()),
            gym_id: "3998000".to_owned(),
            item_id: "4045681".to_owned(),
            date: "2024-09-20".to_owned(),
        }
    }

    #[test]
    fn backend_refactor_a_slot_selector_is_only_live_for_its_own_read_and_account() {
        let mut selectors = HashMap::new();
        selectors.insert("selector".to_owned(), slot());
        let now = Some(std::time::Instant::now());
        assert_eq!(
            selected_confirmed_slot(Some("student"), now, &selectors, "student", "selector"),
            Some(&slot())
        );
        // Another account's selector, an unknown selector, and a read that has
        // aged out all resolve to nothing, so a caller can never name a slot it
        // was not handed.
        assert_eq!(
            selected_confirmed_slot(Some("other"), now, &selectors, "student", "selector"),
            None
        );
        assert_eq!(
            selected_confirmed_slot(Some("student"), now, &selectors, "student", "unknown"),
            None
        );
        assert_eq!(
            selected_confirmed_slot(
                Some("student"),
                Some(std::time::Instant::now() - Duration::from_secs(301)),
                &selectors,
                "student",
                "selector"
            ),
            None
        );
        assert_eq!(
            selected_confirmed_slot(None, now, &selectors, "student", "selector"),
            None
        );
    }

    #[test]
    fn backend_refactor_the_single_payment_ceiling_is_the_observed_clients_own() {
        assert_eq!(max_single_payment_cost(), 42);
        assert_eq!(max_single_payment_cost(), SPORTS_MAX_SINGLE_PAYMENT_COST);
    }
}
