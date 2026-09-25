use super::*;

#[test]
fn backend_repair_info_personal_cli_includes_subscription_feed_selector_dependency() {
    for id in ["info_subscriptions", "info_favorites"] {
        let selected = select_cases(&[id.into()]).unwrap();
        assert_eq!(
            selected,
            BTreeSet::from([
                "identity_session".into(),
                "portal_bootstrap".into(),
                "info_session".into(),
                id.into(),
            ])
        );
        assert!(!selected.contains("info_catalog"));
        assert!(!selected.contains("info_news"));
    }
    assert_eq!(
        select_cases(&["info_subscription_feed".into()]).unwrap(),
        BTreeSet::from([
            "identity_session".into(),
            "portal_bootstrap".into(),
            "info_session".into(),
            "info_subscriptions".into(),
            "info_subscription_feed".into(),
        ])
    );
}
