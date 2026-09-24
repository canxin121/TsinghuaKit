use super::*;
use crate::reference_test_support::{FixtureServer, Reply};

#[test]
fn backend_repair_homework_parse_diagnostic_survives_closed_log_vocabulary() {
    let path = root();
    let mut log = LogSession::start(&path, LogConfig::parse("debug", false).unwrap()).unwrap();
    tracing::dispatcher::with_default(&log.dispatch, || {
        tracing::warn!(target:"tsinghua_kit::api",event="homework_parse_rejected",bucket="pending",parse_reason="missing_field",parse_field="deadline",body_shape="json_object",message="fixture-private-homework");
        tracing::warn!(target:"tsinghua_kit::api",event="homework_parse_rejected",bucket="fixture-private-bucket",parse_reason="fixture-private-reason",parse_field="fixture-private-field",body_shape="fixture-private-shape");
    });
    log.flush();
    let rows = events(&log.directory);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["fields"]["event"], "homework_parse_rejected");
    assert_eq!(rows[0]["fields"]["bucket"], "pending");
    assert_eq!(rows[0]["fields"]["parse_reason"], "missing_field");
    assert_eq!(rows[0]["fields"]["parse_field"], "deadline");
    assert_eq!(rows[0]["fields"]["body_shape"], "json_object");
    assert_eq!(rows[1]["redacted_fields"], 4);
    assert!(
        !serde_json::to_string(&rows)
            .unwrap()
            .contains("fixture-private")
    );
    drop(log);
    fs::remove_dir_all(path).unwrap();
}

#[test]
fn backend_repair_homework_date_shape_logs_only_fixed_vocabulary() {
    let path = root();
    let mut log = LogSession::start(&path, LogConfig::parse("debug", false).unwrap()).unwrap();
    tracing::dispatcher::with_default(&log.dispatch, || {
        tracing::warn!(target:"tsinghua_kit::api",event="homework_parse_rejected",bucket="pending",parse_reason="invalid_date",parse_field="deadline",body_shape="json_object",date_shape="iso_t_local",date_width="w19",message="fixture-private-date");
    });
    log.flush();
    let rows = events(&log.directory);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["fields"]["date_shape"], "iso_t_local");
    assert_eq!(rows[0]["fields"]["date_width"], "w19");
    assert!(
        !serde_json::to_string(&rows)
            .unwrap()
            .contains("fixture-private-date")
    );
    drop(log);
    fs::remove_dir_all(path).unwrap();
}

#[test]
fn backend_repair_followup_registrar_fixed_session_reason_survives_log_and_report() {
    let path = root();
    let mut log = LogSession::start(&path, LogConfig::parse("debug", false).unwrap()).unwrap();
    let code = crate::registrar_session::RegistrarSessionError::Client(
        crate::registrar_client::RegistrarClientError::LoginExpired {
            reason: "SYNTHETIC_PRIVATE_DETAIL".into(),
        },
    )
    .diagnostic_code();
    assert_eq!(
        diagnostic_reason(&format!("服务读取未确认（{code}），详情见脱敏日志")),
        code
    );
    tracing::dispatcher::with_default(&log.dispatch, || {
        tracing::warn!(target:"tsinghua_kit::api",event="business_read_failure",service="registrar",business_stage="registrar_handoff",reason=code);
    });
    log.flush();
    let rows = events(&log.directory);
    assert_eq!(rows[0]["fields"]["reason"], code);
    assert!(
        !serde_json::to_string(&rows)
            .unwrap()
            .contains("SYNTHETIC_PRIVATE_DETAIL")
    );
    drop(log);
    fs::remove_dir_all(path).unwrap();
}

#[test]
fn backend_repair_followup_handoff_stage_and_reason_logs_remain_closed_vocabulary() {
    let path = root();
    let mut log = LogSession::start(&path, LogConfig::parse("debug", false).unwrap()).unwrap();
    tracing::dispatcher::with_default(&log.dispatch, || {
        tracing::info!(target:"tsinghua_kit::auth",event="registrar_handoff",service="registrar",business_stage="registrar_info_roaming");
        tracing::info!(target:"tsinghua_kit::auth",event="registrar_handoff",service="registrar",business_stage="registrar_calendar_proof");
        tracing::info!(target:"tsinghua_kit::auth",event="registrar_handoff",business_stage="SYNTHETIC_PRIVATE_PATH",reason="SYNTHETIC_PRIVATE_TICKET");
    });
    log.flush();
    let rows = events(&log.directory);
    assert_eq!(
        rows[0]["fields"]["business_stage"],
        "registrar_info_roaming"
    );
    assert_eq!(
        rows[1]["fields"]["business_stage"],
        "registrar_calendar_proof"
    );
    assert_eq!(rows[2]["redacted_fields"], 2);
    assert!(
        !serde_json::to_string(&rows)
            .unwrap()
            .contains("SYNTHETIC_PRIVATE")
    );
    for code in [
        "registrar_session_expired",
        "registrar_network",
        "registrar_handoff_interrupted",
        "info_session_expired",
        "electricity_callback_query_rejected",
        "electricity_callback_path_rejected",
        "electricity_absolute_path_mapped",
    ] {
        assert_eq!(
            diagnostic_reason(&format!("服务读取未确认（{code}），详情见脱敏日志")),
            code
        );
    }
    drop(log);
    fs::remove_dir_all(path).unwrap();
}

#[test]
fn backend_repair_followup_registrar_body_decode_failure_is_not_a_network_failure() {
    let error = crate::registrar_session::RegistrarSessionError::Client(
        crate::registrar_client::RegistrarClientError::Transport(
            crate::transport::TransportError::DecodeBody {
                message: "SYNTHETIC_PRIVATE_RESPONSE".into(),
            },
        ),
    );
    let code = error.diagnostic_code();
    assert_eq!(code, "registrar_response_decode");
    assert_eq!(
        diagnostic_reason(&format!("服务读取未确认（{code}），详情见脱敏日志")),
        code
    );
}

fn root() -> PathBuf {
    std::env::temp_dir().join(format!(
        "thyou-log-fixture-{}",
        uuid::Uuid::new_v4().simple()
    ))
}

#[test]
fn backend_repair_service_followup_card_field_diagnostics_never_serialize_values() {
    let root = root();
    let mut log = LogSession::start(&root, LogConfig::parse("debug", false).unwrap()).unwrap();
    tracing::dispatcher::with_default(&log.dispatch, || {
        crate::campus_card_adapter::CampusCardAdapterError::Parse(
            crate::campus_card_read::CampusCardParseError::InvalidCents {
                field: crate::campus_card_read::CampusCardField::BalanceCents,
                row: None,
            },
        )
        .trace_read_failure("load_campus_card_account");
        tracing::warn!(target:"tsinghua_kit::auth",event="portal_navigation_failure",reason="portal_identity_entry_password_form",service="webvpn");
        tracing::warn!(target:"tsinghua_kit::api",event="card_read_failure",data_field="synthetic_card_number",reason="synthetic_bank_balance");
    });
    log.flush();
    let rows = events(&log.directory);
    assert_eq!(rows[0]["fields"]["reason"], "card_read_cents");
    assert_eq!(rows[0]["fields"]["data_field"], "balance_cents");
    assert_eq!(
        rows[1]["fields"]["reason"],
        "portal_identity_entry_password_form"
    );
    assert_eq!(rows[0]["redacted_fields"], 0);
    assert_eq!(rows[1]["redacted_fields"], 0);
    assert_eq!(rows[2]["redacted_fields"], 2);
    assert!(!serde_json::to_string(&rows).unwrap().contains("synthetic_"));
    drop(log);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn backend_repair_login_diagnosis_tunet_summary_preserves_unproven_ip_without_exposing_it() {
    let root = root();
    let mut session = LogSession::start(&root, LogConfig::parse("trace", false).unwrap()).unwrap();
    let server = FixtureServer::new(vec![Reply::json(
        r#"{"error":"ok","online_ip":"192.0.2.17","user_name":"synthetic-network-account"}"#,
    )]);
    tracing::dispatcher::with_default(&session.dispatch, || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let profile = crate::tunet::TunetProfile::current_with_overrides(
                    crate::tunet::AuthFamily::Auth4,
                    crate::tunet::TunetProfileOverrides {
                        endpoint: Some(
                            crate::tunet::HttpsEndpoint::http(
                                "127.0.0.1",
                                reqwest::Url::parse(server.base()).unwrap().port().unwrap(),
                            )
                            .unwrap(),
                        ),
                        ..Default::default()
                    },
                )
                .unwrap();
                let client = crate::tunet_client::TunetClient::with_transport(
                    crate::tunet_client::TunetClientConfig::new(profile).unwrap(),
                    crate::transport::CampusHttpTransport::new("THYou/tunet-diagnostic-fixture")
                        .unwrap(),
                )
                .unwrap();
                let record = client.status(&[("ip", "192.0.2.18")]).await.unwrap();
                assert!(!record.is_online_proven_for_ip("192.0.2.18"));
                assert!(!record.is_offline_proven_for_ip("192.0.2.18"));
            });
    });
    session.flush();
    let rows = events(&session.directory);
    let record = rows
        .iter()
        .find(|r| r["fields"]["event"] == "tunet_status_evidence")
        .unwrap();
    assert_eq!(record["fields"]["response_encoding"], "json");
    assert_eq!(record["fields"]["status_signal"], "positive");
    assert_eq!(record["fields"]["observed_state"], "online");
    assert_eq!(record["fields"]["bound_state"], "unknown");
    assert_eq!(record["fields"]["online_proven"], false);
    assert_eq!(record["redacted_fields"], 0);
    let text = serde_json::to_string(&rows).unwrap();
    for secret in ["192.0.2.17", "192.0.2.18", "synthetic-network-account"] {
        assert!(!text.contains(secret));
    }
    assert_eq!(server.requests().len(), 1);
    drop(session);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn backend_repair_login_diagnosis_new_evidence_fields_reject_non_vocabulary_values() {
    let root = root();
    let mut session = LogSession::start(&root, LogConfig::parse("trace", false).unwrap()).unwrap();
    tracing::dispatcher::with_default(&session.dispatch, || {
        tracing::warn!(target:"tsinghua_kit::auth",event="identity_failure_evidence",evidence_source="secret_account",evidence_marker="secret_password",submission_route="secret_route",response_encoding="secret_cookie",observed_state="secret_ip",bound_state="secret_ticket");
    });
    session.flush();
    let rows = events(&session.directory);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["redacted_fields"], 6);
    assert!(!serde_json::to_string(&rows).unwrap().contains("secret_"));
    drop(session);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn backend_repair_telemetry_rotation_io_failure_is_reported_not_silently_healthy() {
    let root = root();
    let mut config = LogConfig::parse("info", false).unwrap();
    config.max_file_bytes = 1024;
    let mut session = LogSession::start(&root, config).unwrap();
    // A conflicting future rotation file simulates a failed atomic create
    // without relying on root privileges or filling the user's filesystem.
    new_private_file(&session.directory.join("events.000002.jsonl")).unwrap();
    tracing::dispatcher::with_default(&session.dispatch, || {
        for id in 0..12_u64 {
            tracing::info!(target:"tsinghua_kit::api",event="operation_started",operation_id=id);
        }
    });
    session.flush();
    assert!(!session.healthy());
    drop(session);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn backend_repair_telemetry_allowed_field_names_do_not_accept_arbitrary_secret_values() {
    let root = root();
    let mut session = LogSession::start(&root, LogConfig::parse("trace", false).unwrap()).unwrap();
    tracing::dispatcher::with_default(&session.dispatch, || {
        tracing::info!(target:"tsinghua_kit::api",event="operation_started",operation="secret123",service="private_account",reason="private_ticket",method="credential123");
    });
    session.flush();
    let rows = events(&session.directory);
    let text = serde_json::to_string(&rows).unwrap();
    for value in [
        "secret123",
        "private_account",
        "private_ticket",
        "credential123",
    ] {
        assert!(!text.contains(value));
    }
    assert_eq!(rows[0]["redacted_fields"], 4);
    drop(session);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn backend_repair_thos_redirect_class_survives_log_without_location() {
    let root = root();
    let mut session = LogSession::start(&root, LogConfig::parse("debug", false).unwrap()).unwrap();
    tracing::dispatcher::with_default(&session.dispatch, || {
        tracing::warn!(target:"tsinghua_kit::security",event="thos_read_redirect",thos_target="webvpn_other_route",http_status=302_u64,url="https://private.invalid/SECRET_TICKET");
        tracing::warn!(target:"tsinghua_kit::security",event="thos_read_redirect",thos_target="PRIVATE_TARGET",http_status=302_u64);
    });
    session.flush();
    let rows = events(&session.directory);
    assert_eq!(rows[0]["fields"]["thos_target"], "webvpn_other_route");
    assert_eq!(rows[0]["fields"]["http_status"], 302);
    assert!(rows[1]["fields"].get("thos_target").is_none());
    assert!(
        !serde_json::to_string(&rows)
            .unwrap()
            .contains("SECRET_TICKET")
    );
    assert!(
        !serde_json::to_string(&rows)
            .unwrap()
            .contains("PRIVATE_TARGET")
    );
    drop(session);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn backend_repair_telemetry_warn_only_keeps_safe_operation_context() {
    let root = root();
    let mut session = LogSession::start(&root, LogConfig::parse("warn", false).unwrap()).unwrap();
    tracing::dispatcher::with_default(&session.dispatch, || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let result: Result<(), String> = observe("identity", "login", async {
                    Err("network failure with private-password".into())
                })
                .await;
                assert!(result.is_err());
            });
    });
    session.flush();
    let rows = events(&session.directory);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["spans"][0]["operation"], "login");
    assert!(
        !serde_json::to_string(&rows)
            .unwrap()
            .contains("private-password")
    );
    drop(session);
    fs::remove_dir_all(root).unwrap();
}
fn events(path: &Path) -> Vec<Value> {
    let mut paths = fs::read_dir(path)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("events.")
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths
        .into_iter()
        .flat_map(|p| {
            fs::read_to_string(p)
                .unwrap()
                .lines()
                .map(|l| serde_json::from_str(l).unwrap())
                .collect::<Vec<Value>>()
        })
        .collect()
}

#[test]
fn backend_repair_telemetry_secret_fields_and_dependency_logs_never_persist() {
    let root = root();
    let mut session = LogSession::start(&root, LogConfig::parse("trace", false).unwrap()).unwrap();
    tracing::dispatcher::with_default(&session.dispatch, || {
        tracing::info!(target:"tsinghua_kit::auth",event="factor_submission",password="fixture-password",username="fixture_username",cookie="fixture-cookie",url="https://id.example/private?ticket=fixture-ticket",body="fixture-body",message="fixture-message");
        tracing::error!(target:"reqwest",message="dependency-secret");
    });
    session.flush();
    let rows = events(&session.directory);
    assert_eq!(rows.len(), 1);
    let text = serde_json::to_string(&rows).unwrap();
    for secret in [
        "fixture-password",
        "fixture_username",
        "fixture-cookie",
        "fixture-ticket",
        "fixture-body",
        "fixture-message",
        "dependency-secret",
    ] {
        assert!(!text.contains(secret));
    }
    assert_eq!(rows[0]["redacted_fields"], 6);
    assert_eq!(rows[0]["module"], "auth");
    drop(session);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn backend_repair_telemetry_levels_modules_and_error_file_are_distinct() {
    let root = root();
    let mut session =
        LogSession::start(&root, LogConfig::parse("warn,http=trace", false).unwrap()).unwrap();
    tracing::dispatcher::with_default(&session.dispatch, || {
        tracing::info!(target:"tsinghua_kit::auth",event="factor_submission");
        tracing::trace!(target:"tsinghua_kit::http",event="request_dispatched",request_id=1_u64);
        tracing::warn!(target:"tsinghua_kit::session",event="transition_rejected");
    });
    session.flush();
    let rows = events(&session.directory);
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|r| r["level"] == "TRACE"));
    let errors = fs::read_to_string(session.directory.join("errors.000001.jsonl")).unwrap();
    let errors = errors
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["level"], "WARN");
    drop(session);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn backend_repair_telemetry_http_spans_correlate_without_url_headers_or_body() {
    let root = root();
    let mut session = LogSession::start(&root, LogConfig::parse("trace", false).unwrap()).unwrap();
    let server = FixtureServer::new(vec![Reply {
        status: 200,
        headers: "Set-Cookie: fixture_cookie=secret-cookie; Path=/\r\n".into(),
        body: "private-response-body".into(),
    }]);
    tracing::dispatcher::with_default(&session.dispatch, || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                observe("learn", "load_learn_courses", async {
                    let client =
                        crate::transport::CampusHttpTransport::new("THYou/log-fixture").unwrap();
                    client
                        .get_text(&format!(
                            "{}private-secret-path?ticket=secret-ticket",
                            server.base()
                        ))
                        .await
                })
                .await
                .unwrap();
            });
    });
    session.flush();
    let rows = events(&session.directory);
    let body = serde_json::to_string(&rows).unwrap();
    for secret in [
        "secret-cookie",
        "private-response-body",
        "private-secret-path",
        "secret-ticket",
    ] {
        assert!(!body.contains(secret));
    }
    let http = rows
        .iter()
        .find(|r| r["fields"]["event"] == "response_headers")
        .unwrap();
    assert_eq!(http["fields"]["cookie_updated"], true);
    assert_eq!(http["fields"]["http_status"], 200);
    assert_eq!(http["spans"][0]["operation"], "load_learn_courses");
    assert!(http["fields"]["request_id"].as_u64().is_some());
    assert!(session.healthy());
    drop(session);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn backend_repair_telemetry_cancelled_future_has_no_false_success_event() {
    use std::{future::poll_fn, task::Poll};
    let root = root();
    let mut session = LogSession::start(&root, LogConfig::parse("trace", false).unwrap()).unwrap();
    tracing::dispatcher::with_default(&session.dispatch, || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let mut future = Box::pin(observe(
                    "identity",
                    "login",
                    std::future::pending::<Result<(), String>>(),
                ));
                poll_fn(|cx| {
                    assert!(future.as_mut().poll(cx).is_pending());
                    Poll::Ready(())
                })
                .await;
                drop(future);
            });
    });
    session.flush();
    let rows = events(&session.directory);
    assert!(rows.iter().any(|r| r["fields"]["outcome"] == "cancelled"));
    assert!(!rows.iter().any(|r| r["fields"]["outcome"] == "success"));
    drop(session);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn backend_repair_telemetry_size_rotation_is_bounded_and_json_lines_are_complete() {
    let root = root();
    let mut config = LogConfig::parse("trace", false).unwrap();
    config.max_file_bytes = 1024;
    config.retained_files = 2;
    let mut session = LogSession::start(&root, config).unwrap();
    tracing::dispatcher::with_default(&session.dispatch, || {
        for i in 0..60_u64 {
            tracing::info!(target:"tsinghua_kit::api",event="operation_started",operation_id=i);
        }
    });
    session.flush();
    let paths = fs::read_dir(&session.directory)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("events.")
        })
        .collect::<Vec<_>>();
    assert_eq!(paths.len(), 2);
    assert!(!events(&session.directory).is_empty());
    for p in paths {
        assert!(fs::metadata(&p).unwrap().len() <= 1024);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&p).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
    drop(session);
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn backend_repair_telemetry_symlinks_and_overbroad_log_filters_fail_closed() {
    use std::os::unix::fs::symlink;
    let root = root();
    private_dir(&root).unwrap();
    let external = root.join("outside");
    private_dir(&external).unwrap();
    let link = root.join("logs");
    symlink(&external, &link).unwrap();
    assert!(LogSession::start(&link, LogConfig::default()).is_err());
    for filter in [
        "trace,reqwest=trace",
        "info,password=debug",
        "warn,http=anything",
        "",
        "trace,http=trace,info",
    ] {
        assert!(LogConfig::parse(filter, false).is_err());
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn backend_repair_telemetry_reports_replace_atomically_and_refuse_symlink_targets() {
    let root = root();
    private_dir(&root).unwrap();
    let report = root.join("report.json");
    write_private_json(&report, &json!({"state":"pending"})).unwrap();
    write_private_json(&report, &json!({"state":"passed"})).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&report).unwrap()).unwrap()["state"],
        "passed"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let link = root.join("link.json");
        symlink(&report, &link).unwrap();
        assert!(write_private_json(&link, &json!({"state":"bad"})).is_err());
    }
    assert_eq!(
        fs::read_dir(&root)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .count(),
        0
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn backend_repair_telemetry_active_sessions_are_not_deleted_by_retention() {
    let root = root();
    let mut sessions = Vec::new();
    for _ in 0..7 {
        sessions.push(LogSession::start(&root, LogConfig::default()).unwrap());
    }
    for session in &sessions {
        assert!(session.directory.exists());
    }
    for session in &mut sessions {
        session.flush();
    }
    drop(sessions);
    fs::remove_dir_all(root).unwrap();
}
