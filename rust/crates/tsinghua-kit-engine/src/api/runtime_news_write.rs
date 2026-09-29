//! Account-bound INFO news writes: favorites and subscription rules.
//!
//! Every write here is built and dispatched exactly once.  What a caller may
//! name is only a reference this runtime already returned for the *same*
//! proven account, so an article identifier, a source identifier and a rule
//! identifier can never be invented by the caller and can never outlive the
//! session that produced them.  The account's own student id, cookies, CSRF
//! value and rule identifiers stay in this Runtime and never enter a Flutter
//! DTO.
//!
//! A write whose result is not the service's own acceptance is reported as
//! unconfirmed rather than failed, and is *never* sent a second time: a second
//! dispatch of a write whose effect is unknown would duplicate it.

use super::*;
use crate::info_news::NewsWriteOutcome;
use crate::info_news::{NewsOperation, NewsProfile, NewsRequestPlan, NewsSubscriptionDraft};

/// One subscription condition this session has already dispatched.
///
/// Adding a condition is not idempotent on the service — the same condition
/// sent twice becomes two rules — so the exact wire condition is remembered
/// here and a repeat is refused before a second request is built.  Only a
/// fresh subscription read retires the record, because only that read is the
/// service's own answer about which rules now exist.
fn condition_fingerprint(
    channel_id: Option<&str>,
    source_id: Option<&str>,
    keyword: &str,
) -> String {
    serde_json::to_string(&(channel_id.unwrap_or(""), source_id.unwrap_or(""), keyword))
        .unwrap_or_else(|_| String::from("[]"))
}

/// Proves the INFO session is usable for a write and rejects a session that
/// belongs to a different account than the one this Runtime proved.
///
/// The same four checks every INFO news reader performs — both proofs, the
/// registry's own user, and the shared cookie jar — are required here, because
/// a write must not start a handoff of its own: an ordinary first write on an
/// account with no INFO handoff yet is refused with a readable message rather
/// than silently opening a second authentication path.
fn ensure_info_write_session(runtime: &mut CampusRuntime) -> Result<UserIdentity, String> {
    let user = runtime.cache_user_for_read()?.ok_or_else(|| {
        runtime.record_error("INFO 服务会话未建立，请先登录后打开信息门户".to_owned())
    })?;
    let proven = runtime.service_session_is_proven(ServiceId::Identity)
        && runtime.service_session_is_proven(ServiceId::Info);
    let same_account = runtime.info_adapter.as_ref().is_some_and(|adapter| {
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
        return Err(
            runtime.record_error("INFO 服务会话未确认，请先打开信息门户建立会话".to_owned())
        );
    }
    Ok(user)
}

/// Resolves one article identifier from a reference this runtime returned.
///
/// A reference from another client, from a replaced link snapshot, or for an
/// article the current account never returned is rejected before any plan or
/// request exists.
fn resolve_article(
    runtime: &CampusRuntime,
    generation: u64,
    article_id: &str,
) -> Result<String, String> {
    if !runtime.info_news_article_reference_is_current(generation, article_id) {
        return Err("INFO 新闻条目引用已失效，请重新打开列表".to_owned());
    }
    let article_id = article_id.trim();
    if article_id.is_empty() {
        return Err("INFO 新闻编号无效".to_owned());
    }
    Ok(article_id.to_owned())
}

/// Builds one write plan from the profile the INFO session itself was
/// configured with, so a plan can never name a route this deployment's
/// allow-list did not establish.
fn build_plan<F>(runtime: &CampusRuntime, build: F) -> Result<NewsRequestPlan, String>
where
    F: FnOnce(&NewsProfile) -> Result<NewsRequestPlan, crate::info_news::NewsProfileError>,
{
    let Some(adapter) = runtime.info_adapter.as_ref() else {
        return Err("INFO 服务会话未建立".to_owned());
    };
    build(&adapter.config().news).map_err(|error| {
        // A profile or input rejection is decided locally, before any request
        // exists.  It is reported with one fixed message rather than the
        // error's own text: `NewsProfileError::InvalidPath` and
        // `InvalidWireName` both carry the offending value, which here is a
        // service-side identifier the caller is not otherwise given, and
        // `public_error` matches on substrings, so an unvetted message could
        // also be rewritten into an unrelated category.
        let _ = &error;
        "INFO 新闻写入参数无效，请刷新后重试".to_owned()
    })
}

/// Dispatches one plan and turns its outcome into the runtime's own answer.
///
/// `Accepted` is the only success.  `Refused` and `Unrecognized` both mean the
/// request left and the change is unknown, so they are reported as an
/// unconfirmed outcome and nothing is replayed.  A login requirement follows
/// the same single-bounded refresh the readers use, and even then the write is
/// *not* resent: the caller must ask again deliberately.
async fn dispatch(
    runtime: &mut CampusRuntime,
    operation: NewsOperation,
    plan: NewsRequestPlan,
) -> Result<CampusRuntimeStatusDto, String> {
    let Some(adapter) = runtime.info_adapter.as_ref() else {
        return Err(runtime.record_error("INFO 服务会话未建立".to_owned()));
    };
    let outcome = adapter.execute_news_write(&runtime.coordinator, plan).await;
    match outcome {
        Ok(NewsWriteOutcome::Accepted) => {
            if operation == NewsOperation::AddFavorite || operation == NewsOperation::RemoveFavorite
            {
                // A favorite write changes the `favorited` flag every cached
                // list/search page still carries.  Remember the account and
                // instant so a page older than this write is not reused as if
                // it were current.  The favorites read itself is never served
                // from cache, so it needs nothing.
                let owner = runtime
                    .cache_user_for_read()?
                    .map(|user| user.username)
                    .unwrap_or_default();
                runtime.info_news_favorite_write_epoch = Some((owner, Utc::now()));
            }
            runtime.clear_info_failure_code();
            runtime.last_error = None;
            Ok(runtime.status())
        }
        Ok(NewsWriteOutcome::Refused) | Ok(NewsWriteOutcome::Unrecognized) => Err(runtime
            .record_business_failure("info", "info_news_write", "info_news_write_unconfirmed")),
        Ok(NewsWriteOutcome::LoginRequired) => {
            runtime.invalidate_service_session(ServiceId::Info);
            Err(runtime.record_business_failure("info", "info_news_write", "info_session_expired"))
        }
        Err(error) => Err(runtime.record_business_failure(
            "info",
            "info_news_write",
            info_failure_code(&error),
        )),
    }
}

/// Adds one article to the current account's favorites.
pub(super) async fn add_favorite(
    runtime: &mut CampusRuntime,
    generation: u64,
    article_id: &str,
) -> Result<CampusRuntimeStatusDto, String> {
    runtime.allow_live_operation()?;
    let _user = ensure_info_write_session(runtime)?;
    let article_id = resolve_article(runtime, generation, article_id)?;
    let plan = build_plan(runtime, |profile| profile.add_favorite_request(&article_id))?;
    dispatch(runtime, NewsOperation::AddFavorite, plan).await
}

/// Removes one article from the current account's favorites.
pub(super) async fn remove_favorite(
    runtime: &mut CampusRuntime,
    generation: u64,
    article_id: &str,
) -> Result<CampusRuntimeStatusDto, String> {
    runtime.allow_live_operation()?;
    let _user = ensure_info_write_session(runtime)?;
    let article_id = resolve_article(runtime, generation, article_id)?;
    let plan = build_plan(runtime, |profile| {
        profile.remove_favorite_request(&article_id)
    })?;
    dispatch(runtime, NewsOperation::RemoveFavorite, plan).await
}

/// Adds one subscription rule for a source or channel of the current account.
///
/// The caller supplies the rule's body, never an identifier of its own: the
/// channel and the source arrive as wire values that the SDK layer has already
/// resolved from references it returned for this same client and catalog
/// generation, and the profile rejects any value that could extend a path or a
/// form field.  A rule that names neither a channel nor a source is refused
/// here rather than sent, because the service cannot express it.
pub(super) async fn add_subscription(
    runtime: &mut CampusRuntime,
    channel_id: Option<String>,
    source_id: Option<String>,
    keyword: Option<String>,
) -> Result<CampusRuntimeStatusDto, String> {
    runtime.allow_live_operation()?;
    let _user = ensure_info_write_session(runtime)?;
    if channel_id.is_none() && source_id.is_none() {
        return Err(runtime.record_error("INFO 订阅条件必须包含一个关注的来源或栏目".to_owned()));
    }
    let keyword = keyword.unwrap_or_default();
    let fingerprint = condition_fingerprint(
        channel_id.as_deref(),
        source_id.as_deref(),
        keyword.as_str(),
    );
    if runtime
        .info_news_dispatched_subscriptions
        .contains(&fingerprint)
    {
        // The service would store a second identical rule, so a repeat is
        // refused here rather than sent. Reading the subscription list again
        // is what makes the same condition addable once more.
        return Err(runtime.record_business_failure(
            "info",
            "info_news_write",
            "info_news_write_replayed",
        ));
    }
    let mut draft = NewsSubscriptionDraft::new();
    if let Some(channel_id) = channel_id {
        draft = draft.with_channel(channel_id);
    }
    if let Some(source_id) = source_id {
        draft = draft.with_source(source_id);
    }
    if !keyword.is_empty() {
        draft = draft.with_keyword(keyword);
    }
    let plan = build_plan(runtime, |profile| profile.add_subscription_request(&draft))?;
    // The condition is recorded as dispatched before the request is built:
    // after this point the outcome is the service's answer, or unknown, and
    // either way this runtime must not build the same rule a second time.
    runtime
        .info_news_dispatched_subscriptions
        .insert(fingerprint);
    dispatch(runtime, NewsOperation::AddSubscription, plan).await
}

/// Removes one subscription rule selected from this runtime's latest rule
/// read. Unknown, expired and other-account selectors are rejected before any
/// request.
pub(super) async fn remove_subscription(
    runtime: &mut CampusRuntime,
    selector: String,
) -> Result<CampusRuntimeStatusDto, String> {
    runtime.allow_live_operation()?;
    let user = ensure_info_write_session(runtime)?;
    let Some(rule_id) = selected_info_subscription_rule(
        runtime.info_subscription_owner.as_deref(),
        runtime.info_subscription_at,
        &runtime.info_subscription_selectors,
        &user.username,
        &selector,
    )
    .map(str::to_owned) else {
        return Err(runtime.record_error("INFO 订阅选择已失效，请刷新订阅规则".to_owned()));
    };
    let plan = build_plan(runtime, |profile| {
        profile.remove_subscription_request(&rule_id)
    })?;
    let status = dispatch(runtime, NewsOperation::RemoveSubscription, plan).await?;
    // The service itself confirmed the rule is gone, so this runtime retires
    // the selector: the same rule cannot be selected again without a fresh
    // rule read, which is also what makes its condition addable again.
    runtime.info_subscription_selectors.remove(&selector);
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::condition_fingerprint;

    #[test]
    fn subscription_fingerprint_separates_fields_and_missing_values() {
        let channel_only = condition_fingerprint(Some("c"), None, "");
        let source_only = condition_fingerprint(None, Some("c"), "");
        assert_ne!(channel_only, source_only);
        assert_eq!(channel_only, condition_fingerprint(Some("c"), None, ""));
        assert_ne!(
            condition_fingerprint(Some("c"), None, ""),
            condition_fingerprint(Some("c"), None, "keyword")
        );
    }
}
