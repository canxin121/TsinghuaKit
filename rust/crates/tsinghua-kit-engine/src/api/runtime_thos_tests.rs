fn thos_fixture_runtime(base: &Path, server: &FixtureServer) -> CampusRuntime {
    let mut runtime = CampusRuntime::new_with_persistence(
        "auto".into(),
        false,
        base.to_string_lossy().into_owned(),
        false,
    )
    .unwrap();
    let user = test_user("fixture-thos-owner");
    install_identity(&mut runtime, &user);
    runtime
        .coordinator
        .begin_authentication(ServiceId::Info)
        .unwrap();
    runtime
        .coordinator
        .mark_authenticated(ServiceId::Info, user, None, None, None)
        .unwrap();
    runtime.info_adapter = Some(
        crate::info_session::InfoSessionAdapter::new(
            crate::info_session::InfoWebVpnConfig::new(server.base(), "/info").unwrap(),
            runtime.identity.transport().clone(),
        )
        .unwrap(),
    );
    runtime.info_roaming_url =
        Some(crate::info::OpaqueUrl::new(format!("{}info/home", server.base())).unwrap());
    runtime.portal_bootstrapped = true;
    runtime
}
fn thos_empty_replies() -> Vec<Reply> {
    vec![
        Reply::json(r#"{"zbNum":0,"AuditSvsNum":0}"#),
        Reply::json(r#"{"list":[],"total":0,"pageNum":1}"#),
        Reply::json(r#"{"list":[],"total":0,"pageNum":1}"#),
    ]
}

#[tokio::test]
async fn backend_repair_thos_phase_steps_require_proven_list_selector_and_reset_with_account() {
    let server = FixtureServer::new(vec![
        Reply::json(r#"{"zbNum":0,"AuditSvsNum":0}"#),
        Reply::json(
            r#"{"list":[{"AGG_PROC_ID":"fixture-aggregate","NAME":"合成阶段","STATE":"1"}],"total":1,"pageNum":1}"#,
        ),
        Reply::json(r#"{"zbNum":0,"AuditSvsNum":0}"#),
        Reply::json(
            r#"[{"ACT_ORDER_ID":"1","ACT_NAME":"合成步骤","ACT_STATE":"1","itemInfo":[]}]"#,
        ),
        Reply::json(r#"{"zbNum":0,"AuditSvsNum":0}"#),
        Reply::json(
            r#"[{"ACT_ORDER_ID":"1","ACT_NAME":"刷新步骤","ACT_STATE":"4","itemInfo":[]}]"#,
        ),
    ]);
    let base = unique_cache_base("thos-phase-steps-runtime");
    let mut runtime = thos_fixture_runtime(&base, &server);
    assert!(
        runtime
            .load_thos_phase_steps("forged".into(), false)
            .await
            .is_err()
    );
    assert!(server.requests().is_empty());
    let list = runtime
        .load_thos_task_list("phases".into(), true)
        .await
        .unwrap();
    assert!(list.complete && list.items.len() == 1);
    let task_id = list.items[0].id.clone();
    assert!(
        runtime
            .load_thos_phase_steps("forged".into(), false)
            .await
            .is_err()
    );
    assert_eq!(server.requests().len(), 2);
    let first = runtime
        .load_thos_phase_steps(task_id.clone(), false)
        .await
        .unwrap();
    assert_eq!(first.source, "live");
    assert_eq!(first.task_id, task_id);
    assert_eq!(first.steps[0].name, "合成步骤");
    assert_eq!(server.requests().len(), 4);
    let cached = runtime
        .load_thos_phase_steps(task_id.clone(), false)
        .await
        .unwrap();
    assert_eq!(cached.source, "cache");
    assert_eq!(server.requests().len(), 4);
    let refreshed = runtime
        .load_thos_phase_steps(task_id.clone(), true)
        .await
        .unwrap();
    assert_eq!(refreshed.source, "live");
    assert_eq!(refreshed.steps[0].name, "刷新步骤");
    let requests = server.requests();
    assert_eq!(requests.len(), 6);
    assert!(requests[3].contains("/fp/aggregation/getActWork"));
    assert!(requests[3].contains("fixture-aggregate"));
    assert!(!requests[3].contains(&task_id));
    runtime.reset_sessions();
    assert!(runtime.load_thos_phase_steps(task_id, false).await.is_err());
    assert_eq!(server.requests().len(), 6);
    drop(runtime);
    remove_cache_file(base);
}

#[tokio::test]
async fn backend_repair_thos_phase_steps_reject_response_error_without_empty_success() {
    let server = FixtureServer::new(vec![
        Reply::json(r#"{"zbNum":0,"AuditSvsNum":0}"#),
        Reply::json(
            r#"{"list":[{"AGG_PROC_ID":"fixture-aggregate","NAME":"合成阶段"}],"total":1,"pageNum":1}"#,
        ),
        Reply::json(r#"{"zbNum":0,"AuditSvsNum":0}"#),
        Reply::json(r#"{"result":"error","message":"fixture unavailable"}"#),
    ]);
    let base = unique_cache_base("thos-phase-steps-error");
    let mut runtime = thos_fixture_runtime(&base, &server);
    let list = runtime
        .load_thos_task_list("phases".into(), true)
        .await
        .unwrap();
    let error = runtime
        .load_thos_phase_steps(list.items[0].id.clone(), false)
        .await
        .unwrap_err();
    assert!(error.contains("格式未确认") || error.contains("查询"));
    assert_eq!(server.requests().len(), 4);
    drop(runtime);
    remove_cache_file(base);
}

#[tokio::test]
async fn backend_repair_thos_other_task_views_share_account_proof_and_invalidate_snapshots() {
    let responses = [
        r#"{"list":[{"proc_inst_id":"fixture-one","service_name":"合成已办"}],"total":1,"pageNum":1}"#,
        r#"{"list":[{"proc_inst_id":"fixture-one","service_name":"合成已办"},{"proc_inst_id":"fixture-two","service_name":"第二已办"}],"total":2,"pageNum":1}"#,
    ];
    let server = FixtureServer::new(vec![
        Reply::json(r#"{"zbNum":0,"AuditSvsNum":0}"#),
        Reply::json(responses[0]),
        Reply::json(r#"{"zbNum":0,"AuditSvsNum":0}"#),
        Reply::json(responses[1]),
    ]);
    let base = unique_cache_base("thos-other-views-runtime");
    let mut runtime = thos_fixture_runtime(&base, &server);
    assert!(
        runtime
            .load_thos_task_list("todo".into(), false)
            .await
            .is_err()
    );
    assert!(server.requests().is_empty());
    let first = runtime
        .load_thos_task_list("completed".into(), false)
        .await
        .unwrap();
    assert!(first.complete && first.error.is_none());
    assert_eq!(first.source, "live");
    assert_eq!(first.items.len(), 1);
    let cached = runtime
        .load_thos_task_list("completed".into(), false)
        .await
        .unwrap();
    assert_eq!(cached.source, "cache");
    assert_eq!(server.requests().len(), 2);
    let second = runtime
        .load_thos_task_list("completed".into(), true)
        .await
        .unwrap();
    assert_eq!(second.source, "live");
    assert_eq!(second.items.len(), 2);
    assert_eq!(server.requests().len(), 4);
    runtime.reset_sessions();
    assert!(
        runtime
            .load_thos_task_list("completed".into(), false)
            .await
            .is_err()
    );
    assert_eq!(server.requests().len(), 4);
    drop(runtime);
    remove_cache_file(base);
}

#[tokio::test]
async fn backend_repair_thos_service_catalog_uses_same_proven_runtime_and_fresh_snapshot() {
    let replies = || {
        vec![
            Reply::json(r#"{"zbNum":0,"AuditSvsNum":0}"#),
            Reply::json(
                r#"{"list":[{"ID":"fixture-service","NAME":"合成服务","UNIT_NAME":"合成部门","UW_TYPE":"1"}],"total":1,"pageNum":1}"#,
            ),
        ]
    };
    let server = FixtureServer::new(replies().into_iter().chain(replies()).collect());
    let base = unique_cache_base("thos-services-runtime");
    let mut runtime = thos_fixture_runtime(&base, &server);
    let result = runtime.load_thos_services(false).await.unwrap();
    assert_eq!(result.source, "live");
    assert!(result.complete && result.error.is_none());
    assert_eq!(result.items.len(), 1);
    assert_eq!(result.items[0].kind.as_deref(), Some("guide"));
    assert_eq!(
        runtime.load_thos_services(false).await.unwrap().source,
        "cache"
    );
    assert_eq!(server.requests().len(), 2);
    assert_eq!(
        runtime.load_thos_services(true).await.unwrap().source,
        "live"
    );
    assert_eq!(server.requests().len(), 4);
    runtime.reset_sessions();
    assert!(runtime.load_thos_services(false).await.is_err());
    assert_eq!(server.requests().len(), 4);
    drop(runtime);
    remove_cache_file(base);
}

#[tokio::test]
async fn backend_repair_thos_handoff_uses_reference_selector_and_proves_target_business() {
    let mut replies = vec![
        Reply {
            status: 401,
            headers: String::new(),
            body: String::new(),
        },
        Reply::html("XSRF-TOKEN=fixture-csrf;"),
        Reply::json(
            r#"{"result":"success","object":{"roamingurl":"https://thos.tsinghua.edu.cn/fp/login?ticket=fixture"}}"#,
        ),
        Reply {
            status: 302,
            headers: format!(
                "Location: /https/{}/fp/view?m=fp\r\nSet-Cookie: thos_fixture=proved; Path=/\r\n",
                crate::thos::MAPPING
            ),
            body: String::new(),
        },
        Reply::html("<html>service hall</html>"),
    ];
    replies.extend(thos_empty_replies());
    let server = FixtureServer::new(replies);
    let base = unique_cache_base("thos-handoff-success");
    let mut runtime = thos_fixture_runtime(&base, &server);
    let value = runtime.load_thos_pending(false).await.unwrap();
    assert!(value.complete && value.items.is_empty());
    let requests = server.requests();
    assert_eq!(requests.len(), 8);
    assert!(requests[2].contains(&format!("yyfwid={}", crate::thos::ROAM_ID)));
    assert!(requests[2].contains("_csrf=fixture-csrf"));
    assert!(requests[3].contains("/fp/login?ticket=fixture"));
    assert!(requests[5].contains("/fp/fp/formHome/allNum"));
    assert!(requests[5].contains("thos_fixture=proved"));
    drop(runtime);
    remove_cache_file(base);
}

#[tokio::test]
async fn backend_repair_thos_read_redirect_to_fixed_home_uses_one_pinned_handoff() {
    let mut replies = vec![
        Reply {
            status: 302,
            headers: format!(
                "Location: /https/{}/fp/view?m=fp#act=fp/formHome\r\n",
                crate::thos::MAPPING
            ),
            body: String::new(),
        },
        Reply::html("XSRF-TOKEN=fixture-csrf;"),
        Reply::json(
            r#"{"result":"success","object":{"roamingurl":"https://thos.tsinghua.edu.cn/fp/login?ticket=fixture"}}"#,
        ),
        Reply {
            status: 302,
            headers: format!(
                "Location: /https/{}/fp/view?m=fp\r\nSet-Cookie: thos_fixture=proved; Path=/\r\n",
                crate::thos::MAPPING
            ),
            body: String::new(),
        },
        Reply::html("<html>service hall</html>"),
    ];
    replies.extend(thos_empty_replies());
    let server = FixtureServer::new(replies);
    let base = unique_cache_base("thos-read-redirect-handoff");
    let mut runtime = thos_fixture_runtime(&base, &server);
    let value = runtime.load_thos_pending(true).await.unwrap();
    assert_eq!(value.source, "live");
    assert!(value.complete && value.items.is_empty());
    let requests = server.requests();
    assert_eq!(requests.len(), 8);
    assert!(requests[0].starts_with(&format!(
        "POST /https/{}/fp/fp/formHome/allNum HTTP/1.1",
        crate::thos::MAPPING
    )));
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.contains("/wengine-vpn/cookie"))
            .count(),
        1
    );
    assert!(requests[5].contains("/fp/fp/formHome/allNum"));
    assert!(requests[5].contains("thos_fixture=proved"));
    drop(runtime);
    remove_cache_file(base);
}

#[tokio::test]
async fn backend_repair_thos_gateway_home_redirect_renews_only_target_session() {
    let mut replies = vec![
        Reply {
            status: 302,
            headers: "Location: /\r\n".into(),
            body: String::new(),
        },
        Reply::html("XSRF-TOKEN=fixture-csrf;"),
        Reply::json(
            r#"{"result":"success","object":{"roamingurl":"https://thos.tsinghua.edu.cn/fp/login?ticket=fixture"}}"#,
        ),
        Reply {
            status: 302,
            headers: format!(
                "Location: /https/{}/fp/view?m=fp\r\nSet-Cookie: thos_fixture=proved; Path=/\r\n",
                crate::thos::MAPPING
            ),
            body: String::new(),
        },
        Reply::html("<html>service hall</html>"),
    ];
    replies.extend(thos_empty_replies());
    let server = FixtureServer::new(replies);
    let base = unique_cache_base("thos-webvpn-home-handoff");
    let mut runtime = thos_fixture_runtime(&base, &server);
    let value = runtime.load_thos_pending(true).await.unwrap();
    assert!(value.complete && value.items.is_empty());
    assert_eq!(value.source, "live");
    let requests = server.requests();
    assert_eq!(requests.len(), 8);
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.contains("/wengine-vpn/cookie"))
            .count(),
        1
    );
    assert!(requests[5].contains("/fp/fp/formHome/allNum"));
    assert!(requests[5].contains("thos_fixture=proved"));
    assert!(
        requests
            .iter()
            .all(|request| !request.starts_with("GET / HTTP/1.1"))
    );
    drop(runtime);
    remove_cache_file(base);
}

#[tokio::test]
async fn backend_repair_thos_dynamic_identity_form_redirect_uses_pinned_handoff() {
    const IDENTITY_MAPPING: &str =
        "77726476706e69737468656265737421f9f30f8834396657761d88e29d51367bcfe7";
    let mut replies = vec![
        Reply {
            status: 302,
            headers: format!(
                "Location: /https/{IDENTITY_MAPPING}/do/off/ui/auth/login/form/0123456789abcdef0123456789abcdef/0\r\n"
            ),
            body: String::new(),
        },
        Reply::html("XSRF-TOKEN=fixture-csrf;"),
        Reply::json(
            r#"{"result":"success","object":{"roamingurl":"https://thos.tsinghua.edu.cn/fp/login?ticket=fixture"}}"#,
        ),
        Reply {
            status: 302,
            headers: format!(
                "Location: /https/{}/fp/view?m=fp\r\nSet-Cookie: thos_fixture=proved; Path=/\r\n",
                crate::thos::MAPPING
            ),
            body: String::new(),
        },
        Reply::html("<html>service hall</html>"),
    ];
    replies.extend(thos_empty_replies());
    let server = FixtureServer::new(replies);
    let base = unique_cache_base("thos-dynamic-identity-redirect");
    let mut runtime = thos_fixture_runtime(&base, &server);
    let value = runtime.load_thos_pending(true).await.unwrap();
    assert!(value.complete && value.items.is_empty());
    let requests = server.requests();
    assert_eq!(requests.len(), 8);
    assert!(requests[5].contains("thos_fixture=proved"));
    assert!(
        requests
            .iter()
            .all(|request| !request.contains("/do/off/ui/auth/login/form/"))
    );
    drop(runtime);
    remove_cache_file(base);
}

#[tokio::test]
async fn backend_repair_thos_untrusted_read_redirects_never_start_handoff() {
    for location in [
        "https://unrelated.test/fp/view?m=fp#act=fp/formHome".to_owned(),
        "/https/another-mapping/fp/view?m=fp#act=fp/formHome".to_owned(),
        "/wengine-vpn/other".to_owned(),
        format!("/https/{}/fp/view?m=fp#act=unrelated", crate::thos::MAPPING),
        format!("/https/{}/fp/%2fother", crate::thos::MAPPING),
        "https://thos.tsinghua.edu.cn/other".to_owned(),
    ] {
        let server = FixtureServer::new(vec![Reply {
            status: 302,
            headers: format!("Location: {location}\r\n"),
            body: String::new(),
        }]);
        let base = unique_cache_base("thos-read-redirect-reject");
        let mut runtime = thos_fixture_runtime(&base, &server);
        assert!(
            runtime
                .load_thos_pending(true)
                .await
                .unwrap_err()
                .contains("未允许的地址")
        );
        assert_eq!(server.requests().len(), 1);
        drop(runtime);
        remove_cache_file(base);
    }
}

#[tokio::test]
async fn backend_repair_thos_runtime_works_without_academics_and_refresh_bypasses_snapshot() {
    let server = FixtureServer::new(
        thos_empty_replies()
            .into_iter()
            .chain(thos_empty_replies())
            .collect(),
    );
    let base = unique_cache_base("thos-independent");
    let mut runtime = thos_fixture_runtime(&base, &server);
    assert!(runtime.learn_source.is_none() && runtime.grades_source.is_none());
    let result = runtime.load_thos_pending(false).await.unwrap();
    assert_eq!(result.source, "live");
    assert!(result.complete && result.items.is_empty());
    let cached = runtime.load_thos_pending(false).await.unwrap();
    assert_eq!(cached.source, "cache");
    assert_eq!(server.requests().len(), 3);
    assert_eq!(
        runtime.load_thos_pending(true).await.unwrap().source,
        "live"
    );
    assert_eq!(server.requests().len(), 6);
    assert!(
        server
            .requests()
            .iter()
            .all(|s| s.contains(crate::thos::MAPPING))
    );
    runtime.reset_sessions();
    assert!(runtime.load_thos_pending(false).await.is_err());
    assert_eq!(server.requests().len(), 6);
    drop(runtime);
    remove_cache_file(base);
}

#[tokio::test]
async fn backend_repair_thos_failed_handoff_is_not_replayed_by_another_read() {
    let expired = || Reply {
        status: 401,
        headers: String::new(),
        body: String::new(),
    };
    let server = FixtureServer::new(vec![
        expired(),
        Reply {
            status: 503,
            headers: String::new(),
            body: String::new(),
        },
        expired(),
    ]);
    let base = unique_cache_base("thos-handoff-once");
    let mut runtime = thos_fixture_runtime(&base, &server);
    assert!(
        runtime
            .load_thos_pending(true)
            .await
            .unwrap_err()
            .contains("续接未确认")
    );
    assert!(
        runtime
            .load_thos_pending(true)
            .await
            .unwrap_err()
            .contains("会话已失效")
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.contains("/wengine-vpn/cookie"))
            .count(),
        1
    );
    assert!(runtime.service_session_is_proven(ServiceId::Identity));
    assert!(runtime.service_session_is_proven(ServiceId::Info));
    drop(runtime);
    remove_cache_file(base);
}

#[tokio::test]
async fn backend_repair_thos_business_failure_does_not_start_authentication() {
    let server = FixtureServer::new(vec![Reply::json(r#"{"result":"error","msg":"failure"}"#)]);
    let base = unique_cache_base("thos-no-auth-on-parse");
    let mut runtime = thos_fixture_runtime(&base, &server);
    let error = runtime.load_thos_pending(false).await.unwrap_err();
    assert!(
        error.contains("未能完成待办查询"),
        "unexpected error: {error}"
    );
    assert_eq!(server.requests().len(), 1);
    assert!(runtime.service_session_is_proven(ServiceId::Identity));
    drop(runtime);
    remove_cache_file(base);
}

#[tokio::test]
async fn backend_repair_thos_rejects_foreign_cookie_jar_and_mapping() {
    let server = FixtureServer::new(vec![]);
    let base = unique_cache_base("thos-binding");
    let mut runtime = thos_fixture_runtime(&base, &server);
    runtime.info_adapter = Some(
        crate::info_session::InfoSessionAdapter::new(
            crate::info_session::InfoWebVpnConfig::new(server.base(), "/info").unwrap(),
            crate::transport::CampusHttpTransport::new("fixture-foreign").unwrap(),
        )
        .unwrap(),
    );
    assert!(
        runtime
            .load_thos_pending(false)
            .await
            .unwrap_err()
            .contains("账号会话未确认")
    );
    assert!(server.requests().is_empty());
    let info = runtime.info_adapter.as_ref().unwrap();
    let mapped = info
        .map_additional_roaming(
            crate::thos::ROAM_ID,
            &crate::info::OpaqueUrl::new("https://thos.tsinghua.edu.cn/fp/login?ticket=fixture")
                .unwrap(),
        )
        .unwrap();
    assert!(mapped.as_str().contains(crate::thos::MAPPING));
    for wrong in [
        "https://unrelated.test/fp/login".to_owned(),
        format!("{}https/other/fp/login", server.base()),
    ] {
        assert!(
            info.map_additional_roaming(
                crate::thos::ROAM_ID,
                &crate::info::OpaqueUrl::new(wrong).unwrap()
            )
            .is_err()
        );
    }
    drop(runtime);
    remove_cache_file(base);
}

#[tokio::test]
async fn backend_repair_thos_debug_validation_requires_new_complete_live_read() {
    let server = FixtureServer::new(
        thos_empty_replies()
            .into_iter()
            .chain(thos_empty_replies())
            .collect(),
    );
    let base = unique_cache_base("thos-validation");
    let mut runtime = thos_fixture_runtime(&base, &server);
    runtime.load_thos_pending(false).await.unwrap();
    let mut ledger =
        crate::live_validation::load_at(base.with_file_name("thos-validation.json")).unwrap();
    ledger.select_cases("thos_pending").unwrap();
    runtime
        .run_debug_live_cases(&test_user("fixture-thos-owner"), &mut ledger)
        .await;
    assert_eq!(ledger.cases["thos_pending"].status, "passed");
    assert_eq!(ledger.cases["thos_pending"].attempts, 1);
    assert_eq!(server.requests().len(), 6);
    drop(runtime);
    remove_cache_file(base);
}
