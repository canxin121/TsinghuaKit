//! Account-bound library reservations: the list read, the seat booking and the
//! cancellation.
//!
//! Two of these three operations change the library's own state, so they share
//! one rule: the request is built and dispatched exactly once, and an outcome
//! the service did not confirm is reported as *unconfirmed* rather than failed
//! and is never sent a second time.  The reservation read is an ordinary read
//! and may be repeated; it is the only thing that makes a cancellation selector
//! meaningful, because it is the only thing that produces one.
//!
//! Everything a booking needs is already evidence this Runtime returned:
//!
//! * the section must be one the current verified directory produced,
//! * the opening window must be one this Runtime confirmed for that same
//!   section, and
//! * the seat and its opaque `area_type` category come from the live seat
//!   inventory this Runtime read for that section, never from a caller.
//!
//! The booking token and the account's own student id are read inside the one
//! request that consumes them: neither is stored in a plan, carried across the
//! FFI boundary, or written to a log.

use super::*;
use crate::library_write::{LibraryWriteOperation, LibraryWriteOutcome};

/// Proves the library session is a write-capable, same-account session.
///
/// The four checks are the same ones the library readers make — both proofs,
/// the registry's own user, and the shared Cookie jar — plus the parent INFO
/// proof the WebVPN mapping depends on.  A booking must not open a session of
/// its own, so an account with no library session is refused here rather than
/// handed a second authentication path.
async fn ensure_library_write_session(runtime: &mut CampusRuntime) -> Result<UserIdentity, String> {
    runtime.ensure_library_reader_session().await?;
    let user = runtime.cache_user_for_read()?.ok_or_else(|| {
        runtime
            .record_error("图书馆服务会话未建立，请先登录")
            .to_owned()
    })?;
    let proven = runtime.service_session_is_proven(ServiceId::Identity)
        && runtime.service_session_is_proven(ServiceId::Info)
        && runtime.service_session_is_proven(ServiceId::Library);
    let same_account = runtime.library_adapter.as_ref().is_some_and(|adapter| {
        runtime
            .coordinator
            .registry()
            .snapshot_for(ServiceId::Library)
            .user
            .as_ref()
            == Some(&user)
            && Arc::ptr_eq(
                adapter.transport().cookie_jar(),
                runtime.identity.transport().cookie_jar(),
            )
    });
    if !proven || !same_account {
        return Err(runtime.record_error("图书馆服务会话未确认，请先打开图书馆页面".to_owned()));
    }
    Ok(user)
}

/// Returns the write adapter that reuses this Runtime's own library transport.
fn write_adapter(
    runtime: &CampusRuntime,
) -> Result<crate::library_write::LibraryWriteAdapter, String> {
    let Some(adapter) = runtime.library_adapter.as_ref() else {
        return Err("图书馆服务会话未建立".to_owned());
    };
    adapter.write_adapter().map_err(|error| {
        // The diagnostic code is stable and carries no URL, body or selector.
        format!("图书馆写入适配器不可用（{}）", error.diagnostic_code())
    })
}

/// Turns one dispatched write into this Runtime's own answer.
///
/// `Accepted` is the only success.  `Refused` and `Unrecognized` both mean the
/// request left and the account's reservations are unknown, so they are
/// reported as unconfirmed and nothing is replayed.  A login requirement
/// discards the library proof and asks the caller to establish it again through
/// the ordinary read path — deliberately not by re-sending this write.
fn finish_write(
    runtime: &mut CampusRuntime,
    stage: &'static str,
    outcome: Result<LibraryWriteOutcome, crate::library_read::LibraryAdapterError>,
) -> Result<CampusRuntimeStatusDto, String> {
    match outcome {
        Ok(LibraryWriteOutcome::Accepted) => {
            runtime.clear_library_failure_code();
            runtime.last_error = None;
            Ok(runtime.status())
        }
        Ok(LibraryWriteOutcome::Refused) | Ok(LibraryWriteOutcome::Unrecognized) => {
            Err(runtime.record_business_failure("library", stage, "library_write_unconfirmed"))
        }
        Ok(LibraryWriteOutcome::LoginRequired) => {
            runtime.invalidate_service_session(ServiceId::Library);
            Err(runtime.record_business_failure("library", stage, "library_session_expired"))
        }
        Err(error) if error.is_session_expired() => {
            runtime.invalidate_service_session(ServiceId::Library);
            Err(runtime.record_business_failure("library", stage, "library_session_expired"))
        }
        Err(error) => {
            Err(runtime.record_business_failure("library", stage, error.diagnostic_code()))
        }
    }
}

/// Reads the current account's own reservation list.
///
/// Every row that carries a cancellation control is given a fresh opaque
/// selector and the service's own cancellation identifier is kept in this
/// Runtime under it.  The selector, its account and its instant are replaced
/// together, so a selector from an older read or from another account cannot be
/// used to cancel anything.
pub(super) async fn load_reservations(
    runtime: &mut CampusRuntime,
) -> Result<LibraryReservationsDto, String> {
    runtime.allow_live_operation()?;
    let user = ensure_library_write_session(runtime).await?;
    let adapter = write_adapter(runtime)?;
    let mut result = adapter.read_booking_records().await;
    if matches!(&result, Err(error) if error.is_session_expired()) {
        // The reservation page is an ordinary read, so recovering the session
        // and reading it once more is not a replay of a mutation.
        runtime.invalidate_service_session(ServiceId::Library);
        if let Err(error) = runtime
            .refresh_nonacademic_service_after_expiry(ServiceId::Library)
            .await
        {
            return Err(runtime.record_error(format!("图书馆预约记录自动续接失败: {error}")));
        }
        let adapter = write_adapter(runtime)?;
        result = adapter.read_booking_records().await;
        if matches!(&result, Err(error) if error.is_session_expired()) {
            runtime.invalidate_service_session(ServiceId::Library);
            return Err(runtime.record_business_failure(
                "library",
                "library_booking_records",
                "library_session_expired",
            ));
        }
    }
    let records = result.map_err(|error| {
        runtime.record_business_failure(
            "library",
            "library_booking_records",
            error.diagnostic_code(),
        )
    })?;
    // Re-prove the account after the requests: a handoff during the read must
    // not leave another account's rows labelled as this one's.
    runtime.allow_live_operation()?;
    let same_account = runtime
        .cache_user_for_read()?
        .is_some_and(|current| current == user);
    if !same_account || !runtime.service_session_is_proven(ServiceId::Library) {
        return Err(runtime.record_business_failure(
            "library",
            "library_booking_records",
            "library_account_changed",
        ));
    }
    let mut selectors = HashMap::new();
    let reservations = records
        .records
        .into_iter()
        .map(|record| {
            let selector = record.cancellation_id.map(|cancellation_id| {
                let selector = Uuid::new_v4().simple().to_string();
                selectors.insert(selector.clone(), cancellation_id);
                selector
            });
            LibraryReservationDto {
                selector,
                position: record.position,
                time: record.time,
                status: record.status,
            }
        })
        .collect();
    runtime.library_reservation_selectors = selectors;
    runtime.library_reservation_owner = Some(user.username.clone());
    runtime.library_reservation_at = Some(std::time::Instant::now());
    runtime.clear_library_failure_code();
    runtime.last_error = None;
    runtime.persist_identity_resume_state_after_live_read("library");
    Ok(LibraryReservationsDto {
        reservations,
        generated_at: Utc::now().to_rfc3339(),
        source: "live".to_owned(),
        status: "ready".to_owned(),
    })
}

/// Books one seat of a section whose window this Runtime already confirmed.
pub(super) async fn book_seat(
    runtime: &mut CampusRuntime,
    section_id: u64,
    segment_id: u64,
    seat_id: u64,
) -> Result<CampusRuntimeStatusDto, String> {
    runtime.allow_live_operation()?;
    if section_id == 0 || segment_id == 0 || seat_id == 0 {
        return runtime.fail("图书馆座位选择无效");
    }
    let user = ensure_library_write_session(runtime).await?;
    if runtime.library_seat_hierarchy_active
        && !runtime.library_seat_section_ids.contains(&section_id)
    {
        return Err(runtime.record_business_failure(
            "library",
            "library_booking",
            "library_section_unconfirmed",
        ));
    }
    if runtime.library_seat_hierarchy_active
        && !runtime
            .library_confirmed_seat_windows
            .get(&section_id)
            .is_some_and(|windows| windows.iter().any(|window| window.id == segment_id))
    {
        return Err(runtime.record_business_failure(
            "library",
            "library_booking",
            "library_segment_unconfirmed",
        ));
    }
    // The seat's category is the inventory's own value for the seat this
    // Runtime read, so a caller can neither invent a seat nor choose a
    // category: `area_type` is not an argument of this function.  The map is
    // keyed by the window as well, so the seat proof comes from the same
    // inventory read as the window proof above.
    let Some(seat) = runtime
        .library_confirmed_seats
        .get(&(section_id, segment_id))
        .and_then(|seats| seats.get(&seat_id))
        .copied()
    else {
        return Err(runtime.record_business_failure(
            "library",
            "library_booking",
            "library_seat_unconfirmed",
        ));
    };
    if !seat.is_available {
        return Err(runtime.record_business_failure(
            "library",
            "library_booking",
            "library_seat_unavailable",
        ));
    }
    let adapter = write_adapter(runtime)?;
    let plan = match adapter.book_seat_request(seat_id, segment_id, seat.area_type) {
        Ok(plan) => plan,
        Err(error) => {
            let _ = &error;
            return Err(runtime.record_business_failure(
                "library",
                "library_booking",
                "library_write_request",
            ));
        }
    };
    debug_assert_eq!(plan.operation, LibraryWriteOperation::BookSeat);
    // The account's own id enters the request here and nowhere else.  The
    // adapter re-validates it as an all-digit student id before it can become a
    // form value.
    let outcome = adapter.book_seat(&plan, &user.username).await;
    let status = finish_write(runtime, "library_booking", outcome)?;
    // The service confirmed the booking, so this seat is no longer part of the
    // inventory this Runtime may book from and the caller's seat handle must not
    // be usable for a second dispatch.
    if let Some(seats) = runtime
        .library_confirmed_seats
        .get_mut(&(section_id, segment_id))
    {
        seats.remove(&seat_id);
    }
    runtime.persist_identity_resume_state_after_live_read("library");
    Ok(status)
}

/// Cancels one reservation selected from this Runtime's latest reservation
/// read.
///
/// The caller names a selector this Runtime minted for the same account, never
/// the service's own cancellation identifier.  An unknown, expired or
/// other-account selector is refused before any plan exists.
pub(super) async fn cancel_booking(
    runtime: &mut CampusRuntime,
    selector: String,
) -> Result<CampusRuntimeStatusDto, String> {
    runtime.allow_live_operation()?;
    let user = ensure_library_write_session(runtime).await?;
    let Some(cancellation_id) = selected_info_subscription_rule(
        runtime.library_reservation_owner.as_deref(),
        runtime.library_reservation_at,
        &runtime.library_reservation_selectors,
        &user.username,
        &selector,
    )
    .map(str::to_owned) else {
        return runtime.fail("图书馆预约选择已失效，请刷新预约记录");
    };
    let adapter = write_adapter(runtime)?;
    let plan = match adapter.cancel_booking_request(&cancellation_id) {
        Ok(plan) => plan,
        Err(error) => {
            let _ = &error;
            return Err(runtime.record_business_failure(
                "library",
                "library_booking",
                "library_write_request",
            ));
        }
    };
    debug_assert_eq!(plan.operation, LibraryWriteOperation::CancelBooking);
    let outcome = adapter.cancel_booking(&plan, &user.username).await;
    let status = finish_write(runtime, "library_booking", outcome)?;
    // The service itself confirmed the cancellation, so this selector retires:
    // the same row cannot be cancelled twice without a fresh reservation read.
    runtime.library_reservation_selectors.remove(&selector);
    runtime.persist_identity_resume_state_after_live_read("library");
    Ok(status)
}

/// Turns on or off the power socket of one seat whose inventory this Runtime
/// read.
///
/// The socket service is hosted by the campus app origin rather than the seat
/// mapping, and its request carries no library booking token, so this is the one
/// library write that goes through the socket writer instead of the booking
/// writer.  What it keeps from the booking path is the evidence rule: the seat
/// must be one this Runtime's own inventory read returned for that section, so a
/// caller cannot name a socket the service never reported.
///
/// Like every other write here, the request is dispatched exactly once.  An
/// outcome the service did not confirm is reported as unconfirmed and is never
/// sent again, so a caller learns the socket's state by reading it again.
pub(super) async fn set_socket_state(
    runtime: &mut CampusRuntime,
    section_id: u64,
    seat_id: u64,
    is_available: bool,
) -> Result<CampusRuntimeStatusDto, String> {
    runtime.allow_live_operation()?;
    if section_id == 0 || seat_id == 0 {
        return runtime.fail("图书馆插座选择无效");
    }
    runtime.ensure_library_reader_session().await?;
    if runtime.library_seat_hierarchy_active
        && !runtime.library_seat_section_ids.contains(&section_id)
    {
        return Err(runtime.record_business_failure(
            "library",
            "library_socket_write",
            "library_section_unconfirmed",
        ));
    }
    // The seat proof: this Runtime read this exact seat as part of the inventory
    // of a window belonging to this section.  The category and availability the
    // booking path needs are not consulted — the socket service has its own
    // state — but the seat's provenance is the same.
    let seat_is_confirmed =
        runtime
            .library_confirmed_seats
            .iter()
            .any(|((confirmed_section, _segment), seats)| {
                *confirmed_section == section_id && seats.contains_key(&seat_id)
            });
    if !seat_is_confirmed {
        return Err(runtime.record_business_failure(
            "library",
            "library_socket_write",
            "library_seat_unconfirmed",
        ));
    }
    let Some(adapter) = runtime.library_adapter.as_ref() else {
        return Err("图书馆服务会话未建立".to_owned());
    };
    let adapter = adapter
        .app_socket_write_adapter()
        .map_err(|error| format!("图书馆插座写入适配器不可用（{}）", error.diagnostic_code()))?;
    let plan = match adapter.socket_state_request(seat_id, is_available) {
        Ok(plan) => plan,
        Err(_error) => {
            return Err(runtime.record_business_failure(
                "library",
                "library_socket_write",
                "library_write_request",
            ));
        }
    };
    debug_assert_eq!(plan.operation, LibraryWriteOperation::SetSocketState);
    let outcome = adapter.set_socket_state(&plan).await;
    runtime.persist_identity_resume_state_after_live_read("library");
    finish_write(runtime, "library_socket_write", outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_selector_from_an_older_read_is_not_accepted() {
        let mut selectors = HashMap::new();
        selectors.insert("selector".to_owned(), "cancellation".to_owned());
        assert_eq!(
            selected_info_subscription_rule(
                Some("student"),
                Some(std::time::Instant::now()),
                &selectors,
                "student",
                "selector",
            ),
            Some("cancellation")
        );
        // Another account, an unknown selector and an expired read all fail the
        // same way, so a selector is never usable outside the read that made it.
        assert_eq!(
            selected_info_subscription_rule(
                Some("other"),
                Some(std::time::Instant::now()),
                &selectors,
                "student",
                "selector",
            ),
            None
        );
        assert_eq!(
            selected_info_subscription_rule(
                Some("student"),
                Some(std::time::Instant::now()),
                &selectors,
                "student",
                "unknown",
            ),
            None
        );
        assert_eq!(
            selected_info_subscription_rule(
                Some("student"),
                Some(std::time::Instant::now() - Duration::from_secs(301)),
                &selectors,
                "student",
                "selector",
            ),
            None
        );
    }
}
