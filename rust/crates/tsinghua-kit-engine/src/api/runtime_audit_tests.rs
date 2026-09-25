//! Business-API regression fixtures; never use an account or a live campus URL.
use super::*;
use crate::reference_test_support::{FixtureServer, Reply};
use crate::tunet_client::TunetClientConfig;
use serde_json::{Value, json};

fn info_runtime(server: &FixtureServer) -> CampusRuntime {
    let mut runtime = runtime();
    prove(&mut runtime, ServiceId::Identity);
    let csrf = runtime.coordinator.registry().bind_csrf(
        ServiceId::Info,
        crate::protocol::CsrfToken::new("fixture-csrf").unwrap(),
    );
    runtime
        .coordinator
        .begin_authentication(ServiceId::Info)
        .unwrap();
    runtime
        .coordinator
        .mark_authenticated(ServiceId::Info, user(), None, Some(csrf), None)
        .unwrap();
    runtime.info_adapter = Some(
        InfoSessionAdapter::new(
            InfoWebVpnConfig::new(server.base(), "/target").unwrap(),
            runtime.identity.transport().clone(),
        )
        .unwrap(),
    );
    runtime.info_roaming_url =
        Some(crate::info::OpaqueUrl::new(format!("{}target/home", server.base())).unwrap());
    runtime
}

#[tokio::test]
async fn backend_repair_api_audit_info_detail_cannot_substitute_another_article() {
    let server = FixtureServer::new(vec![
        Reply::html("XSRF-TOKEN=fixture-csrf;"),
        Reply::json(
            r#"{"result":"success","object":{"xxDto":{"xxid":"other-id","bt":"Fixture","nr":"%3Cp%3Ebody%3C%2Fp%3E"}}}"#,
        ),
    ]);
    let mut runtime = info_runtime(&server);
    assert!(
        runtime
            .load_info_news_detail("requested-id".into())
            .await
            .is_err()
    );
    assert_eq!(server.requests().len(), 2);
    assert!(runtime.service_session_is_proven(ServiceId::Info));
}

#[tokio::test]
async fn backend_repair_api_audit_info_detail_missing_id_binds_request_only_after_valid_content() {
    let server = FixtureServer::new(vec![
        Reply::html("XSRF-TOKEN=fixture-csrf;"),
        Reply::json(
            r#"{"result":"success","object":{"xxDto":{"bt":"Fixture","nr":"%3Cp%3Ebody%3C%2Fp%3E"}}}"#,
        ),
    ]);
    let mut runtime = info_runtime(&server);
    let detail = runtime
        .load_info_news_detail("requested-id".into())
        .await
        .unwrap();
    assert_eq!(detail.id, "requested-id");
    assert_eq!(detail.summary, "body");
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn backend_repair_api_audit_info_query_drift_is_not_a_valid_requested_article() {
    let server = FixtureServer::new(vec![
        Reply::html("XSRF-TOKEN=fixture-csrf;"),
        Reply {
            status: 302,
            headers: "Location: /target/b/info/xxfb_fg/xnzx/template/detail?xxid=other\r\n".into(),
            body: String::new(),
        },
        Reply::json(
            r#"{"result":"success","object":{"xxDto":{"bt":"Fixture","nr":"%3Cp%3Ebody%3C%2Fp%3E"}}}"#,
        ),
    ]);
    let mut runtime = info_runtime(&server);
    assert!(
        runtime
            .load_info_news_detail("requested-id".into())
            .await
            .is_err()
    );
    assert!(runtime.service_session_is_proven(ServiceId::Info));
    assert_eq!(server.requests().len(), 3);
}

#[tokio::test]
async fn backend_repair_api_audit_card_later_page_auth_expiry_clears_only_card() {
    let (mut runtime, server) = card_runtime(vec![
        page(0, 100),
        Reply {
            status: 401,
            headers: String::new(),
            body: String::new(),
        },
    ])
    .await;
    assert!(
        runtime
            .load_campus_card_transactions("2026-09-17".into(), "2026-09-17".into())
            .await
            .is_err()
    );
    assert!(!runtime.service_session_is_proven(ServiceId::CampusCard));
    assert!(runtime.service_session_is_proven(ServiceId::Identity));
    assert_eq!(server.requests().len(), 3);
}

#[tokio::test]
async fn backend_repair_api_audit_card_full_then_empty_page_proves_exact_multiple() {
    let (mut runtime, server) = card_runtime(vec![page(0, 100), page(100, 0)]).await;
    assert_eq!(
        runtime
            .load_campus_card_transactions("2026-09-17".into(), "2026-09-17".into())
            .await
            .unwrap()
            .len(),
        100
    );
    assert_eq!(server.requests().len(), 3);
}

#[tokio::test]
async fn backend_repair_api_audit_tunet_other_device_status_preserves_but_does_not_prove_ours() {
    let server = FixtureServer::new(vec![Reply {
        status: 200,
        headers: "Content-Type: application/javascript\r\n".into(),
        body: "thyouStatus({\"error\":\"ok\",\"online_ip\":\"192.0.2.11\"});".into(),
    }]);
    let mut runtime = tunet_runtime(&server);
    assert!(load_tunet_target_status(&mut runtime).await.is_err());
    assert!(runtime.tunet_connection_target.is_some());
    assert_eq!(server.requests().len(), 1);
}

fn runtime() -> CampusRuntime {
    CampusRuntime::new_with_persistence(AUTO_SEMESTER.into(), false, String::new(), false).unwrap()
}

fn user() -> UserIdentity {
    UserIdentity {
        username: "2026000001".into(),
        display_name: None,
    }
}

fn prove(runtime: &mut CampusRuntime, service: ServiceId) {
    runtime.coordinator.begin_authentication(service).unwrap();
    runtime
        .coordinator
        .mark_authenticated(service, user(), None, None, None)
        .unwrap();
}

fn page(start: usize, count: usize) -> Reply {
    let rows: Vec<_> = (start..start + count).map(|i| json!({
        "id": i + 1, "summary": "synthetic meal", "txdate": "2026-09-17 12:00:00",
        "balance": 12000, "txamt": -345, "meraddr": "", "mername": "Fixture", "txname": "消费"
    })).collect();
    Reply::json(&json!({"success": true, "resultData": {"rows": rows}}).to_string())
}

async fn card_runtime(pages: Vec<Reply>) -> (CampusRuntime, FixtureServer) {
    let mut replies = vec![Reply::json(
        r#"{"success":true,"resultData":{"loginuser":"2026000001"}}"#,
    )];
    replies.extend(pages);
    let server = FixtureServer::new(replies);
    let mut runtime = runtime();
    prove(&mut runtime, ServiceId::Identity);
    runtime.card_client = Some(
        CampusCardClient::new(
            crate::campus_card_adapter::CampusCardAdapterConfig::new(server.base()).unwrap(),
            runtime.identity.transport().clone(),
        )
        .unwrap(),
    );
    runtime.probe_and_bind_card_session(&user()).await.unwrap();
    (runtime, server)
}

fn request_json(request: &str) -> Value {
    serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap()
}

#[tokio::test]
async fn backend_repair_api_audit_card_window_reads_beyond_first_page() {
    let (mut runtime, server) = card_runtime(vec![page(0, 100), page(100, 1)]).await;
    let result = runtime
        .load_campus_card_transactions("2026-09-17".into(), "2026-09-17".into())
        .await
        .unwrap();
    assert_eq!(
        result.transactions.len(),
        101,
        "a full page is not the complete date window"
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(request_json(&requests[1])["pageNumber"], 0);
    assert_eq!(request_json(&requests[2])["pageNumber"], 1);
    assert_eq!(request_json(&requests[2])["idserial"], "2026000001");
}

#[tokio::test]
async fn backend_repair_api_audit_card_later_page_failure_never_returns_partial_success() {
    let (mut runtime, server) = card_runtime(vec![
        page(0, 100),
        Reply {
            status: 503,
            headers: String::new(),
            body: "synthetic-unavailable".into(),
        },
    ])
    .await;
    assert!(
        runtime
            .load_campus_card_transactions("2026-09-17".into(), "2026-09-17".into())
            .await
            .is_err()
    );
    assert_eq!(server.requests().len(), 3);
    assert!(runtime.service_session_is_proven(ServiceId::CampusCard));
}

#[tokio::test]
async fn backend_repair_api_audit_card_repeated_page_is_not_silently_deduplicated() {
    let (mut runtime, server) = card_runtime(vec![page(0, 100), page(0, 100)]).await;
    assert!(
        runtime
            .load_campus_card_transactions("2026-09-17".into(), "2026-09-17".into())
            .await
            .is_err()
    );
    assert_eq!(
        server.requests().len(),
        3,
        "stop on an unstable or ignored pagination selector"
    );
    assert!(runtime.service_session_is_proven(ServiceId::CampusCard));
}

#[tokio::test]
async fn backend_repair_api_audit_card_pagination_bound_is_explicit_not_truncation() {
    let (mut runtime, server) = card_runtime((0..10).map(|i| page(i * 100, 100)).collect()).await;
    assert!(
        runtime
            .load_campus_card_transactions("2026-09-17".into(), "2026-09-17".into())
            .await
            .is_err()
    );
    assert_eq!(
        server.requests().len(),
        11,
        "ten bounded pages plus initial session proof"
    );
}

#[tokio::test]
async fn backend_repair_api_audit_card_empty_page_and_invalid_dates_are_distinct() {
    let (mut runtime, server) = card_runtime(vec![page(0, 0)]).await;
    assert!(
        runtime
            .load_campus_card_transactions("2026-09-18".into(), "2026-09-17".into())
            .await
            .is_err()
    );
    assert_eq!(
        server.requests().len(),
        1,
        "invalid range must not dispatch"
    );
    assert!(
        runtime
            .load_campus_card_transactions("2026-09-17".into(), "2026-09-17".into())
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn backend_repair_api_audit_card_oversized_page_fails_closed() {
    let (mut runtime, server) = card_runtime(vec![page(0, 101)]).await;
    assert!(
        runtime
            .load_campus_card_transactions("2026-09-17".into(), "2026-09-17".into())
            .await
            .is_err()
    );
    assert_eq!(server.requests().len(), 2);
}

fn usereg_runtime(server: &FixtureServer) -> CampusRuntime {
    let mut runtime = runtime();
    let config =
        UseregClientConfig::new(server.base(), crate::usereg::UseregProfile::default()).unwrap();
    runtime.usereg_adapter = Some(UseregAdapter::new(UseregClient::new(config).unwrap()));
    prove(&mut runtime, ServiceId::Usereg);
    runtime
}

#[tokio::test]
async fn backend_repair_api_audit_usereg_all_read_http_failures_preserve_session() {
    for operation in 0..3 {
        let server = FixtureServer::new(vec![Reply {
            status: 503,
            headers: String::new(),
            body: "synthetic-unavailable".into(),
        }]);
        let mut runtime = usereg_runtime(&server);
        let failed = match operation {
            0 => runtime.load_usereg_account().await.is_err(),
            1 => runtime.load_usereg_balance().await.is_err(),
            _ => runtime.load_usereg_devices().await.is_err(),
        };
        assert!(failed);
        assert!(
            runtime.service_session_is_proven(ServiceId::Usereg),
            "a server outage is not an expired session"
        );
        assert_eq!(server.requests().len(), 1);
    }
}

#[tokio::test]
async fn backend_repair_api_audit_usereg_explicit_unauthorized_clears_only_usereg() {
    let home = r#"<html>
      <input type="hidden" name="_csrf-8800" value="fixture">
      <div id="w1-container"><table><tbody><tr data-key="17">
        <td>192.0.2.10</td><td></td><td>2026-09-17 10:00:00</td><td>campus</td><td>AA-BB-CC</td>
      </tr></tbody></table></div>
      <div id="w3-container"><table><tbody><tr><td>学生</td><td>1G</td><td>2h</td><td>8.10</td><td>2026-10-01</td></tr></tbody></table></div>
    </html>"#;
    let server = FixtureServer::new(vec![
        Reply::html(home),
        Reply {
            status: 401,
            headers: String::new(),
            body: String::new(),
        },
    ]);
    let mut runtime = usereg_runtime(&server);
    prove(&mut runtime, ServiceId::Identity);
    assert_eq!(runtime.load_usereg_devices().await.unwrap().len(), 1);
    let old_generation = runtime.usereg_devices_generation;
    assert!(runtime.load_usereg_balance().await.is_err());
    assert!(!runtime.service_session_is_proven(ServiceId::Usereg));
    assert!(
        !runtime.usereg_device_reference_is_current(old_generation, 0),
        "an expired SelfService session must invalidate device references"
    );
    assert!(runtime.service_session_is_proven(ServiceId::Identity));
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn backend_repair_api_audit_usereg_failed_refresh_discards_device_actions_not_session() {
    let home = r#"<html><input type="hidden" name="_csrf-8800" value="fixture"><div id="w1-container"><table><tbody><tr data-key="17"><td>192.0.2.10</td><td></td><td>2026-09-17 10:00:00</td><td>campus</td><td>AA-BB-CC</td></tr></tbody></table></div><div id="w3-container"><table><tbody><tr><td>学生</td><td>1G</td><td>2h</td><td>8.10</td><td>2026-10-01</td></tr></tbody></table></div></html>"#;
    let server = FixtureServer::new(vec![
        Reply::html(home),
        Reply {
            status: 503,
            headers: String::new(),
            body: String::new(),
        },
    ]);
    let mut runtime = usereg_runtime(&server);
    assert_eq!(runtime.load_usereg_devices().await.unwrap().len(), 1);
    let old_generation = runtime.usereg_devices_generation;
    assert!(runtime.load_usereg_devices().await.is_err());
    assert!(
        runtime.usereg_devices.is_empty(),
        "do not keep stale disconnect targets"
    );
    assert!(
        !runtime.usereg_device_reference_is_current(old_generation, 0),
        "a failed refresh must invalidate handles into the previous list"
    );
    assert!(runtime.service_session_is_proven(ServiceId::Usereg));
    assert!(runtime.disconnect_usereg_device(0).await.is_err());
    assert_eq!(server.requests().len(), 2);
}

fn tunet_runtime(server: &FixtureServer) -> CampusRuntime {
    let mut runtime = runtime();
    let port = Url::parse(server.base()).unwrap().port().unwrap();
    let profile = crate::tunet::TunetProfile::current_with_overrides(
        crate::tunet::AuthFamily::Auth4,
        crate::tunet::TunetProfileOverrides {
            endpoint: Some(crate::tunet::HttpsEndpoint::http("127.0.0.1", port).unwrap()),
            ..Default::default()
        },
    )
    .unwrap();
    let client = TunetClient::with_transport(
        TunetClientConfig::new(profile).unwrap(),
        runtime.identity.transport().clone(),
    )
    .unwrap();
    runtime.tunet_connection_target = Some(super::TunetConnectionTarget {
        client,
        username: user().username,
        local_ipv4: "192.0.2.10".into(),
    });
    runtime
}

async fn load_tunet_target_status(
    runtime: &mut CampusRuntime,
) -> Result<super::TunetNetworkStatusDto, String> {
    let target = runtime.tunet_connection_target.as_ref().unwrap();
    runtime
        .load_tunet_status_using(
            target.client.clone(),
            target.local_ipv4.clone(),
            Some(target.username.clone()),
        )
        .await
}

#[tokio::test]
async fn backend_repair_api_audit_tunet_read_outage_preserves_login_capability() {
    let server = FixtureServer::new(vec![Reply {
        status: 503,
        headers: String::new(),
        body: String::new(),
    }]);
    let mut runtime = tunet_runtime(&server);
    assert!(load_tunet_target_status(&mut runtime).await.is_err());
    assert!(runtime.tunet_connection_target.is_some());
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn backend_repair_api_audit_tunet_unknown_status_does_not_claim_logout() {
    let server = FixtureServer::new(vec![Reply {
        status: 200,
        headers: "Content-Type: application/javascript\r\n".into(),
        body: "thyouStatus({\"error\":\"ok\"});".into(),
    }]);
    let mut runtime = tunet_runtime(&server);
    assert!(
        load_tunet_target_status(&mut runtime).await.is_err(),
        "not an online proof"
    );
    assert!(
        runtime.tunet_connection_target.is_some(),
        "not an offline proof either"
    );
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn backend_repair_api_audit_tunet_proven_offline_clears_disconnect_target() {
    let server = FixtureServer::new(vec![Reply { status: 200, headers: "Content-Type: application/javascript\r\n".into(), body: "thyouStatus({\"error\":\"not_online_error\",\"online_ip\":\"\",\"online_device_total\":0});".into() }]);
    let mut runtime = tunet_runtime(&server);
    assert!(load_tunet_target_status(&mut runtime).await.is_err());
    assert!(runtime.tunet_connection_target.is_none());
    assert_eq!(server.requests().len(), 1);
}
