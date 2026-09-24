use super::*;
fn account() -> Value {
    serde_json::json!({"success":true,"resultData":{"idserial":"fixture-user","username":"Fixture","departname":"Fixture","departid":1,"identifyeffectdate":1767225600000_i64,"validatevalue":1893456000000_i64,"baseAccount":{"balance":12345},"cardInfos":[{"cardid":"fixture-card","accstatus":"0","lasttxdate":1789732800000_i64,"maxconstolamt":20000,"maxconsamt":5000}]}})
}
#[test]
fn backend_repair_lastmile_card_account_accepts_reference_numeric_date_milliseconds() {
    let body = account();
    let parsed = parse_card_account_response(
        &body.to_string(),
        Some(&CampusCardAccountBinding::new("fixture-user").unwrap()),
    )
    .unwrap();
    for (text, expected) in [
        (&parsed.effective_at, 1767225600000_i64),
        (&parsed.valid_until, 1893456000000_i64),
        (&parsed.last_transaction_at, 1789732800000_i64),
    ] {
        assert_eq!(
            DateTime::parse_from_rfc3339(text)
                .unwrap()
                .timestamp_millis(),
            expected
        );
    }
    assert_eq!(parsed.balance_cents, 12345);
}
#[test]
fn backend_repair_lastmile_card_bad_dates_do_not_fabricate_epoch_or_change_account_binding() {
    for value in [
        Value::Null,
        serde_json::json!(true),
        serde_json::json!([]),
        serde_json::json!(8640000000000001_i64),
        serde_json::json!(1.1),
    ] {
        let mut body = account();
        body["resultData"]["identifyeffectdate"] = value;
        assert!(parse_card_account_response(&body.to_string(), None).is_err());
    }
    assert!(matches!(
        parse_card_account_response(
            &account().to_string(),
            Some(&CampusCardAccountBinding::new("other-user").unwrap())
        ),
        Err(CampusCardParseError::AccountMismatch)
    ));
}
#[test]
fn backend_repair_lastmile_card_matches_actual_reference_date_outputs_without_touching_money() {
    let fixtures: Value =
        serde_json::from_str(include_str!("reference_lastmile_fixtures.json")).unwrap();
    let sample = &fixtures["card"];
    let parsed = parse_card_account_response(
        &sample["wire"].to_string(),
        Some(&CampusCardAccountBinding::new("fixture-user").unwrap()),
    )
    .unwrap();
    assert_eq!(
        DateTime::parse_from_rfc3339(&parsed.effective_at)
            .unwrap()
            .timestamp_millis(),
        sample["expected"]["effective_ms"].as_i64().unwrap()
    );
    assert_eq!(
        DateTime::parse_from_rfc3339(&parsed.valid_until)
            .unwrap()
            .timestamp_millis(),
        sample["expected"]["valid_ms"].as_i64().unwrap()
    );
    assert_eq!(
        DateTime::parse_from_rfc3339(&parsed.last_transaction_at)
            .unwrap()
            .timestamp_millis(),
        sample["expected"]["last_ms"].as_i64().unwrap()
    );
    assert_eq!(
        parsed.balance_cents,
        sample["expected"]["balance_cents"].as_i64().unwrap()
    );
    let mut body = sample["wire"].clone();
    body["resultData"]["username"] = serde_json::json!(12345);
    assert!(parse_card_account_response(&body.to_string(), None).is_err());
}
