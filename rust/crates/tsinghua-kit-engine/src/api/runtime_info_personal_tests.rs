use super::*;

#[test]
fn backend_repair_info_subscription_selector_is_account_and_age_bound() {
    let mut selectors = HashMap::new();
    selectors.insert("opaque-selector".to_owned(), "server-rule".to_owned());
    let now = std::time::Instant::now();
    assert_eq!(
        selected_info_subscription_rule(
            Some("fixture-user"),
            Some(now),
            &selectors,
            "fixture-user",
            "opaque-selector",
        ),
        Some("server-rule"),
    );
    assert!(
        selected_info_subscription_rule(
            Some("another-user"),
            Some(now),
            &selectors,
            "fixture-user",
            "opaque-selector",
        )
        .is_none()
    );
    assert!(
        selected_info_subscription_rule(
            Some("fixture-user"),
            Some(now - Duration::from_secs(301)),
            &selectors,
            "fixture-user",
            "opaque-selector",
        )
        .is_none()
    );
    assert!(
        selected_info_subscription_rule(
            Some("fixture-user"),
            Some(now),
            &selectors,
            "fixture-user",
            "unknown-selector",
        )
        .is_none()
    );
}

#[test]
fn backend_refactor_news_page_cache_is_dropped_only_for_the_account_that_wrote() {
    let mut runtime =
        CampusRuntime::new_with_persistence("2026-fall".to_owned(), false, String::new(), false)
            .unwrap();
    let written_at = Utc::now();
    let older = written_at - chrono::Duration::minutes(5);
    let newer = written_at + chrono::Duration::minutes(1);

    // No write in this session: every readable page stays eligible.
    assert!(info_news_page_cache_is_still_current(
        &runtime,
        "account-a",
        &older
    ));

    runtime.info_news_favorite_write_epoch = Some(("account-a".to_owned(), written_at));
    // A page generated before the accepted write still carries the old
    // `favorited` flag, so it must not be presented as the account's current
    // list.
    assert!(!info_news_page_cache_is_still_current(
        &runtime,
        "account-a",
        &older
    ));
    assert!(info_news_page_cache_is_still_current(
        &runtime,
        "account-a",
        &newer
    ));
    // Another account's page is unaffected: the record is bound to the account
    // that performed the write.
    assert!(info_news_page_cache_is_still_current(
        &runtime,
        "account-b",
        &older
    ));
}
