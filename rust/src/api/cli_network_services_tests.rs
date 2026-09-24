//! Network-service acceptance contracts. All HTTP uses synthetic loopback.
use super::*;
use crate::reference_test_support::{FixtureServer, Reply};

#[test]
fn backend_repair_network_suite_includes_portal_captcha_reads_and_both_status_scopes() {
    let selected = network_service_cases().unwrap();
    assert_eq!(
        selected,
        [
            "identity_session",
            "portal_bootstrap",
            "usereg_session",
            "usereg_account",
            "usereg_balance",
            "usereg_devices",
            "tunet_status",
            "tunet_local_status"
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    );
    assert_eq!(
        select_cases(&["usereg_balance".into()]).unwrap(),
        [
            "identity_session",
            "portal_bootstrap",
            "usereg_session",
            "usereg_balance"
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    );
    assert!(
        !selected
            .iter()
            .any(|id| id.contains("disconnect") || id.contains("login_tunet"))
    );
}

#[test]
fn backend_repair_network_scope_exempts_both_status_reads_only_when_explicit() {
    for spec in CHECKS {
        assert!(
            NetworkEnvironment::Unspecified
                .skip_reason(spec.id)
                .is_none()
        );
        assert_eq!(
            NetworkEnvironment::OffCampus.skip_reason(spec.id).is_some(),
            matches!(spec.id, "tunet_status" | "tunet_local_status")
        );
    }
}

fn client(server: &FixtureServer) -> TunetClient {
    let url = Url::parse(server.base()).unwrap();
    let profile = crate::tunet::TunetProfile::current_with_overrides(
        crate::tunet::AuthFamily::Auth4,
        crate::tunet::TunetProfileOverrides {
            endpoint: Some(
                crate::tunet::HttpsEndpoint::http("127.0.0.1", url.port().unwrap()).unwrap(),
            ),
            ..Default::default()
        },
    )
    .unwrap();
    TunetClient::new(crate::tunet_client::TunetClientConfig::new(profile).unwrap()).unwrap()
}

#[tokio::test]
async fn backend_repair_network_local_probe_requires_ip_proof_and_never_authenticates() {
    for (payload, expected) in [
        (
            r#"{"error":"ok","online_ip":"192.0.2.10","user_name":"fixture-network"}"#,
            "tunet_local_online",
        ),
        (
            r#"{"error":"not_online_error","online_ip":"","online_ip6":"","online_device_total":0}"#,
            "tunet_local_offline",
        ),
        (
            r#"{"error":"ok","online_ip":"192.0.2.99"}"#,
            "tunet_address_mismatch",
        ),
        (r#"{"error":"ok"}"#, "tunet_state_unproven"),
    ] {
        let server = FixtureServer::new(vec![Reply::json(&format!("thyouStatus({payload});"))]);
        let result = local_network_status(&client(&server), "192.0.2.10").await;
        let actual = match result {
            Ok(Outcome::ObservedNetwork(reason)) => reason.to_owned(),
            Err(reason) => reason,
            _ => panic!("network probe must return explicit observed evidence"),
        };
        assert_eq!(actual, expected);
        let requests = server.requests();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].starts_with("GET /cgi-bin/rad_user_info?ip=192.0.2.10&"));
        assert!(!requests[0].contains("password") && !requests[0].contains("action="));
    }
}

#[tokio::test]
async fn backend_repair_network_probe_parse_and_http_failures_never_claim_offline_or_retry() {
    for reply in [
        Reply::html("<html>login required</html>"),
        Reply {
            status: 503,
            headers: "Retry-After: 1\r\n".into(),
            body: String::new(),
        },
    ] {
        let server = FixtureServer::new(vec![reply]);
        assert!(
            matches!(local_network_status(&client(&server), "192.0.2.10").await,
            Err(reason) if reason == "tunet_status_unconfirmed")
        );
        assert_eq!(server.requests().len(), 1);
    }
}

#[test]
fn backend_repair_network_status_uses_the_runtime_transport_without_exporting_session() {
    let runtime =
        CampusRuntime::new_with_persistence("2026-2027-1".into(), false, String::new(), false)
            .unwrap();
    let client = network_status_client(&runtime).unwrap();
    assert!(Arc::ptr_eq(
        client.transport().cookie_jar(),
        runtime.identity.transport().cookie_jar()
    ));
    assert!(!runtime.service_session_is_proven(ServiceId::Tunet));
}

#[test]
fn backend_repair_tunet_physical_ipv4_ignores_active_tunnel_and_rejects_ambiguity() {
    let candidate = |name: &str, address: &str, up: bool, running: bool| LocalIpv4Interface {
        name: name.into(),
        address: address.parse().unwrap(),
        up,
        running,
    };
    let physical = candidate("en0", "101.5.32.10", true, true);
    assert_eq!(
        select_physical_ipv4([
            candidate("utun5", "198.18.0.2", true, true),
            candidate("en1", "192.0.2.11", false, false),
            physical,
        ]),
        Ok("101.5.32.10".parse().unwrap())
    );
    assert!(select_physical_ipv4([candidate("utun5", "198.18.0.2", true, true)]).is_err());
    assert!(
        select_physical_ipv4([
            candidate("en0", "101.5.32.10", true, true),
            candidate("en1", "192.0.2.11", true, true),
        ])
        .is_err()
    );
    assert!(select_physical_ipv4([candidate("en0", "198.18.0.2", true, true)]).is_err());
}

#[test]
fn backend_repair_network_retry_keeps_only_unfinished_reads_with_fresh_portal_dependencies() {
    let root = std::env::temp_dir().join(format!("network-check-{}", Uuid::new_v4()));
    let path = root.join("report.json");
    let mut report = ReportWriter::new(
        path.clone(),
        "fixture".into(),
        &network_service_cases().unwrap(),
        false,
    )
    .unwrap();
    for id in network_service_cases().unwrap() {
        let failed = id == "usereg_balance";
        report
            .end(
                &id,
                if failed {
                    CheckStatus::Failed
                } else {
                    CheckStatus::Passed
                },
                if failed {
                    "usereg_balance_invalid"
                } else {
                    "verified"
                },
                None,
                1,
                1,
            )
            .unwrap();
    }
    report.finish().unwrap();
    let before = fs::read(&path).unwrap();
    let (retry, _) = retry_plan(&path).unwrap();
    assert_eq!(retry, select_cases(&["usereg_balance".into()]).unwrap());
    assert!(
        !retry.contains("usereg_devices")
            && !retry.contains("tunet_status")
            && !retry.contains("tunet_local_status")
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    drop(report);
    fs::remove_dir_all(root).unwrap();
}
