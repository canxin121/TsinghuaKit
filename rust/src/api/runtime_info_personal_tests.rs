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
