//! Synthetic regressions for the two remaining failures in run 035936Z.
use super::*;
use crate::protocol::CsrfToken;
use crate::reference_test_support::{FixtureServer, Reply};

const REG_PATH: &str =
    "/http/77726476706e69737468656265737421eaff4b8b69336153301c9aa596522b20bc86e6e559a9b290/";
const EMPTY_EXAMS: &str = "<html><table><tr><th>开课系</th><th>课程号</th><th>课序号</th><th>课程名</th><th>课程分类</th><th>教师</th><th>人数</th><th>考试日期</th><th>考场</th></tr></table></html>";

fn user() -> UserIdentity {
    UserIdentity {
        username: "fixture-user".into(),
        display_name: None,
    }
}
fn student_user(username: &str) -> UserIdentity {
    UserIdentity {
        username: username.into(),
        display_name: None,
    }
}
fn base(server: &FixtureServer) -> String {
    format!("{}{}", server.base().trim_end_matches('/'), REG_PATH)
}
fn runtime_with_mode(server: &FixtureServer, graduate: bool, auto_detect: bool) -> CampusRuntime {
    runtime_with_mode_for_user(server, graduate, auto_detect, user())
}
fn runtime_with_mode_for_user(
    server: &FixtureServer,
    graduate: bool,
    auto_detect: bool,
    identity_user: UserIdentity,
) -> CampusRuntime {
    let cache =
        PathBuf::from(std::env::var("THYOU_SESSION_DIR").unwrap()).join("followup-cache.json");
    let mut r = if auto_detect {
        CampusRuntime::new_auto_with_persistence(
            "2026-2027-1".into(),
            graduate,
            cache.to_string_lossy().into_owned(),
            false,
        )
    } else {
        CampusRuntime::new_with_persistence(
            "2026-2027-1".into(),
            graduate,
            cache.to_string_lossy().into_owned(),
            false,
        )
    }
    .unwrap();
    for service in [ServiceId::Identity, ServiceId::Learn, ServiceId::Info] {
        r.coordinator.begin_authentication(service).unwrap();
        let csrf = (service != ServiceId::Identity).then(|| {
            r.coordinator
                .registry()
                .bind_csrf(service, CsrfToken::new("fixture-csrf").unwrap())
        });
        r.coordinator
            .mark_authenticated(service, identity_user.clone(), None, csrf, None)
            .unwrap();
    }
    r.portal_bootstrapped = true;
    r.info_adapter = Some(
        InfoSessionAdapter::new(
            InfoWebVpnConfig::new(server.base(), "/info/").unwrap(),
            r.identity.transport().clone(),
        )
        .unwrap(),
    );
    r.info_roaming_url =
        Some(crate::info::OpaqueUrl::new(format!("{}info/f/info/index", server.base())).unwrap());
    let learn = LearnClient::new(
        LearnClientConfig::new(&format!("{}learn/", server.base()), CourseRole::Student).unwrap(),
    );
    let registrar = registrar(&r, server, false);
    let source = r
        .build_learn_source(
            learn,
            registrar,
            r.coordinator.bound_csrf(ServiceId::Learn).unwrap(),
        )
        .unwrap();
    r.learn_source = Some(source);
    r
}
fn runtime(server: &FixtureServer, graduate: bool) -> CampusRuntime {
    runtime_with_mode(server, graduate, false)
}
fn auto_runtime(server: &FixtureServer, graduate: bool) -> CampusRuntime {
    runtime_with_mode(server, graduate, true)
}
fn auto_runtime_for_user(
    server: &FixtureServer,
    graduate: bool,
    identity_user: UserIdentity,
) -> CampusRuntime {
    runtime_with_mode_for_user(server, graduate, true, identity_user)
}
fn registrar(r: &CampusRuntime, server: &FixtureServer, isolated: bool) -> RegistrarClient {
    RegistrarClient::from_transport(
        RegistrarClientConfig {
            learn_base_url: format!("{}learn/", server.base()),
            registrar_base_url: base(server),
            ..Default::default()
        },
        if isolated {
            crate::transport::CampusHttpTransport::new("THYou/isolated-fixture").unwrap()
        } else {
            r.identity.transport().clone()
        },
    )
    .unwrap()
}
fn calendar(body: &str) -> Reply {
    Reply {
        status: 200,
        headers: "Content-Type: application/javascript\r\n".into(),
        body: body.into(),
    }
}
fn replies(proof: Reply) -> Vec<Reply> {
    vec![
        Reply::html("XSRF-TOKEN=fixture-csrf;"),
        Reply::json(
            r#"{"result":"success","object":{"roamingurl":"http://zhjw.cic.tsinghua.edu.cn/jxmh.do?ticket=FIXTURE%2Babc%3D"}}"#,
        ),
        Reply {
            status: 302,
            headers: format!(
                "Location: {REG_PATH}jxmh.do?m=home\r\nSet-Cookie: fixture-registrar=ready; Path={REG_PATH}\r\n"
            ),
            body: String::new(),
        },
        Reply::html(
            "<html><script src='/wengine-vpn/webvpn.js'></script><h1>Academic portal</h1></html>",
        ),
        proof,
    ]
}
fn assert_no_login_or_direct_ticket(requests: &[String]) {
    for request in requests {
        assert!(request.starts_with("GET "));
        for forbidden in [
            "/b/wlxt/common/auth/gnt",
            "appId=ALL_ZHJW",
            "/do/off/ui/auth/login",
            "/b/doubleAuth/",
            "i_pass",
            "password=",
        ] {
            assert!(
                !request.contains(forbidden),
                "unexpected authentication request"
            );
        }
    }
}

#[tokio::test]
async fn backend_repair_followup_registrar_webvpn_uses_reference_roam_and_reuses_proof() {
    for graduate in [false, true] {
        let server = FixtureServer::new(replies(calendar("thyouCalendar([])")));
        let mut r = runtime(&server, graduate);
        let original_learn = r.learn_source.clone().unwrap();
        r.establish_registrar_runtime_at(&user(), &base(&server))
            .await
            .unwrap();
        assert!(r.service_session_is_proven(ServiceId::Registrar));
        assert!(r.service_session_is_proven(ServiceId::Learn));
        assert!(r.service_session_is_proven(ServiceId::Info));
        assert!(Arc::ptr_eq(
            &original_learn,
            r.learn_source.as_ref().unwrap()
        ));
        assert!(r.resolver.is_some());
        assert!(r.primary_password.is_none());
        r.establish_registrar_runtime_at(&user(), &base(&server))
            .await
            .unwrap();
        let requests = server.requests();
        assert_eq!(requests.len(), 5);
        let selector = if graduate {
            "BEABB32641DC4EC3510B048BAF42471A"
        } else {
            "287C0C6D90ABB364CD5FDF1495199962"
        };
        assert!(requests[1].contains(&format!("yyfwid={selector}")));
        assert!(requests[4].contains(if graduate {
            "m=yjs_jxrl_all"
        } else {
            "m=bks_jxrl_all"
        }));
        assert!(requests[4].contains("jsoncallback=thyouCalendar"));
        assert!(
            requests[4]
                .to_ascii_lowercase()
                .contains("cookie: fixture-registrar=ready")
        );
        assert_no_login_or_direct_ticket(&requests);
    }
}

fn info_cache_item(id: &str, link: &str) -> InfoNewsItemDto {
    InfoNewsItemDto {
        id: id.to_owned(),
        title: "Fixture announcement".to_owned(),
        link: link.to_owned(),
        published_at: "2026-09-20 08:00:00".to_owned(),
        source: "Fixture source".to_owned(),
        topped: false,
        channel: "NEWS".to_owned(),
        favorited: false,
    }
}

fn write_info_list_cache(
    runtime: &CampusRuntime,
    username: &str,
    page: u32,
    page_size: u32,
    generated_at: chrono::DateTime<chrono::Utc>,
) {
    let payload = InfoNewsPageCachePayload {
        account_scope: cache_account_scope(username),
        feed: "list".to_owned(),
        page,
        page_size,
        source_id: None,
        channel_id: None,
        keyword: String::new(),
        channel_filter_label: None,
        exact_match: false,
        generated_at,
        items: vec![info_cache_item(
            "fixture-news-id",
            "/info/article?entry=fixture",
        )],
    };
    assert!(info_news_cache_payload_is_valid(
        &payload, username, "list", page, page_size, None, None, "", None, false,
    ));
    let cache = JsonFileCache::<InfoNewsPageCachePayload>::new(
        info_news_cache_path(
            &runtime.cache_path,
            username,
            "list",
            page,
            page_size,
            None,
            None,
            "",
            None,
            false,
        ),
        INFO_NEWS_CACHE_SCHEMA_VERSION,
        INFO_NEWS_CACHE_SERVICE,
    );
    cache
        .write(&payload)
        .expect("write INFO news fixture cache");
}

fn write_info_search_cache(
    runtime: &CampusRuntime,
    username: &str,
    page: u32,
    keyword: &str,
    channel_filter_label: Option<&str>,
    exact_match: bool,
    generated_at: chrono::DateTime<chrono::Utc>,
) {
    let payload = InfoNewsPageCachePayload {
        account_scope: cache_account_scope(username),
        feed: "search".to_owned(),
        page,
        page_size: 0,
        source_id: None,
        channel_id: None,
        keyword: keyword.to_owned(),
        channel_filter_label: channel_filter_label.map(str::to_owned),
        exact_match,
        generated_at,
        items: vec![info_cache_item(
            "fixture-search-id",
            "/info/article?entry=search",
        )],
    };
    assert!(info_news_cache_payload_is_valid(
        &payload,
        username,
        "search",
        page,
        0,
        None,
        None,
        keyword,
        channel_filter_label,
        exact_match,
    ));
    let cache = JsonFileCache::<InfoNewsPageCachePayload>::new(
        info_news_cache_path(
            &runtime.cache_path,
            username,
            "search",
            page,
            0,
            None,
            None,
            keyword,
            channel_filter_label,
            exact_match,
        ),
        INFO_NEWS_CACHE_SCHEMA_VERSION,
        INFO_NEWS_CACHE_SERVICE,
    );
    cache
        .write(&payload)
        .expect("write INFO search fixture cache");
}

fn expire_identity_for_cache_read(runtime: &mut CampusRuntime, user: &UserIdentity) {
    runtime.coordinator.logout(ServiceId::Identity).unwrap();
    runtime
        .coordinator
        .begin_authentication(ServiceId::Identity)
        .unwrap();
    runtime
        .coordinator
        .mark_authenticated(
            ServiceId::Identity,
            user.clone(),
            None,
            None,
            Some(chrono::Utc::now() - chrono::Duration::seconds(1)),
        )
        .unwrap();
    runtime.resume_account_metadata = Some(
        crate::session_persistence::ResumeAccountMetadata::new(
            &user.username,
            Some(AcademicStage::Graduate),
            None,
            false,
        )
        .unwrap(),
    );
    runtime.invalidate_service_session(ServiceId::Info);
}

#[tokio::test]
async fn backend_repair_info_news_fresh_cache_skips_info_network() {
    let server = FixtureServer::new(vec![]);
    let mut runtime = runtime(&server, false);
    let user = user();
    write_info_list_cache(&runtime, &user.username, 1, 10, chrono::Utc::now());

    let page = runtime
        .load_info_news(1, 10, None, None)
        .await
        .expect("fresh INFO cache should be readable");
    assert_eq!(page.source.as_deref(), Some("cache"));
    assert_eq!(page.status.as_deref(), Some("ready"));
    assert_eq!(page.items.len(), 1);
    assert!(server.requests().is_empty());
    assert!(runtime.service_session_is_proven(ServiceId::Info));
}

#[tokio::test]
async fn backend_repair_info_news_cache_remains_reachable_without_info_proof() {
    let server = FixtureServer::new(vec![]);
    let mut runtime = runtime(&server, false);
    let user = user();
    write_info_list_cache(&runtime, &user.username, 1, 30, chrono::Utc::now());
    runtime.invalidate_service_session(ServiceId::Info);

    let info = runtime
        .service_catalog()
        .services
        .into_iter()
        .find(|service| service.id == "info")
        .expect("INFO remains in the visible catalog");
    assert_eq!(info.availability, "degraded");
    assert_eq!(
        info.capabilities
            .iter()
            .map(|capability| capability.key.as_str())
            .collect::<Vec<_>>(),
        vec!["read_announcements"]
    );
    assert!(!runtime.service_session_is_proven(ServiceId::Info));

    let page = runtime
        .load_info_news(1, 30, None, None)
        .await
        .expect("the catalog-advertised cache read should be executable");
    assert_eq!(page.source.as_deref(), Some("cache"));
    assert_eq!(page.status.as_deref(), Some("ready"));
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn backend_repair_info_news_stale_cache_is_explicit_after_network_failure() {
    let server = FixtureServer::new(vec![Reply {
        status: 503,
        headers: "Content-Type: text/plain\r\n".into(),
        body: "fixture outage".into(),
    }]);
    let mut runtime = runtime(&server, false);
    let user = user();
    write_info_list_cache(
        &runtime,
        &user.username,
        1,
        10,
        chrono::Utc::now() - chrono::Duration::hours(2),
    );

    let page = runtime
        .load_info_news(1, 10, None, None)
        .await
        .expect("validated INFO cache should survive a network outage");
    assert_eq!(page.source.as_deref(), Some("cache"));
    assert_eq!(page.status.as_deref(), Some("stale"));
    assert!(page.error.is_some());
    assert_eq!(page.items.len(), 1);
    assert!(runtime.service_session_is_proven(ServiceId::Info));
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn backend_repair_info_news_cache_miss_lazily_establishes_info() {
    let server = FixtureServer::new(vec![
        Reply::html("XSRF-TOKEN=fixture-csrf;"),
        Reply::json(r#"{"object":{"dataList":[]}}"#),
        Reply::html("XSRF-TOKEN=fixture-csrf;"),
        Reply::json(
            r#"{"object":{"dataList":[{"bt":"Lazy news","url":"/article","xxid":"lazy-id","time":"2026-09-11 10:00:00","dwmc_show":"Fixture source","yxzd":"1-","lmid":"LM_JWGG","sfsc":false}]}}"#,
        ),
    ]);
    let mut runtime = runtime(&server, false);
    // Keep the validated portal prerequisite but remove only the downstream
    // INFO proof. A cache miss must establish INFO from this reader.
    runtime.coordinator.logout(ServiceId::Info).unwrap();

    let page = runtime
        .load_info_news(91, 13, None, None)
        .await
        .expect("INFO list cache miss should lazily establish INFO");

    assert_eq!(page.source.as_deref(), Some("live"));
    assert_eq!(page.status.as_deref(), Some("ready"));
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].id, "lazy-id");
    assert!(runtime.service_session_is_proven(ServiceId::Info));
    assert_eq!(server.requests().len(), 4);

    let cache_path = info_news_cache_path(
        &runtime.cache_path,
        "fixture-user",
        "list",
        91,
        13,
        None,
        None,
        "",
        None,
        false,
    );
    let cached = JsonFileCache::<InfoNewsPageCachePayload>::new(
        &cache_path,
        INFO_NEWS_CACHE_SCHEMA_VERSION,
        INFO_NEWS_CACHE_SERVICE,
    )
    .read()
    .expect("INFO cache reads")
    .expect("lazy live INFO read writes its cache");
    assert_eq!(cached.payload.items[0].id, "lazy-id");
    let _ = std::fs::remove_file(cache_path);
}

#[tokio::test]
async fn backend_repair_info_search_cache_miss_lazily_establishes_info() {
    let server = FixtureServer::new(vec![
        Reply::html("XSRF-TOKEN=fixture-csrf;"),
        Reply::json(r#"{"object":{"dataList":[]}}"#),
        Reply::html("XSRF-TOKEN=fixture-csrf;"),
        Reply::json(
            r#"{"result":"success","object":{"resultsList":[{"bt":"<strong>Lazy search</strong>","url":"/article","xxid":"lazy-search-id","time":"2026-09-11 10:00:00","dwmc_show":"Fixture source","yxzd":null,"lmid":"LM_BGTG","sfsc":false}]}}"#,
        ),
    ]);
    let mut runtime = runtime(&server, false);
    runtime.coordinator.logout(ServiceId::Info).unwrap();

    let page = runtime
        .search_info_news(
            37,
            "lazy-search-term".to_owned(),
            Some("办公通知".to_owned()),
            false,
        )
        .await
        .expect("INFO search cache miss should lazily establish INFO");

    assert_eq!(page.source.as_deref(), Some("live"));
    assert_eq!(page.status.as_deref(), Some("ready"));
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].id, "lazy-search-id");
    assert!(runtime.service_session_is_proven(ServiceId::Info));
    assert_eq!(server.requests().len(), 4);

    let cache_path = info_news_cache_path(
        &runtime.cache_path,
        "fixture-user",
        "search",
        37,
        0,
        None,
        None,
        "lazy-search-term",
        Some("办公通知"),
        false,
    );
    assert!(
        JsonFileCache::<InfoNewsPageCachePayload>::new(
            &cache_path,
            INFO_NEWS_CACHE_SCHEMA_VERSION,
            INFO_NEWS_CACHE_SERVICE,
        )
        .read()
        .expect("INFO search cache reads")
        .is_some()
    );
    let _ = std::fs::remove_file(cache_path);
}

#[tokio::test]
async fn backend_repair_info_news_stale_cache_attempts_lazy_refresh_then_falls_back() {
    let server = FixtureServer::new(vec![Reply {
        status: 503,
        headers: "Content-Type: text/plain\r\n".into(),
        body: "fixture outage".into(),
    }]);
    let mut runtime = runtime(&server, false);
    write_info_list_cache(
        &runtime,
        "fixture-user",
        89,
        11,
        chrono::Utc::now() - chrono::Duration::hours(2),
    );
    runtime.coordinator.logout(ServiceId::Info).unwrap();

    let page = runtime
        .load_info_news(89, 11, None, None)
        .await
        .expect("stale INFO cache should remain available after lazy refresh failure");

    assert_eq!(page.source.as_deref(), Some("cache"));
    assert_eq!(page.status.as_deref(), Some("stale"));
    assert!(page.error.is_some());
    assert_eq!(page.items.len(), 1);
    assert!(!runtime.service_session_is_proven(ServiceId::Info));
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn backend_repair_info_search_fresh_cache_skips_info_handoff_with_expired_identity() {
    let server = FixtureServer::new(vec![]);
    let mut runtime = runtime(&server, false);
    let user = user();
    write_info_search_cache(
        &runtime,
        &user.username,
        1,
        "fixture",
        Some("办公通知"),
        false,
        chrono::Utc::now(),
    );
    expire_identity_for_cache_read(&mut runtime, &user);

    let page = runtime
        .search_info_news(1, "fixture".to_owned(), Some("办公通知".to_owned()), false)
        .await
        .expect("fresh INFO search cache should be readable after identity expiry");

    assert_eq!(page.source.as_deref(), Some("cache"));
    assert_eq!(page.status.as_deref(), Some("ready"));
    assert_eq!(page.items.len(), 1);
    assert!(page.error.is_none());
    assert!(server.requests().is_empty());
    assert!(!runtime.private_credential_attempted);
}

#[tokio::test]
async fn backend_repair_info_search_stale_cache_is_explicit_without_live_identity() {
    let server = FixtureServer::new(vec![]);
    let mut runtime = runtime(&server, false);
    let user = user();
    write_info_search_cache(
        &runtime,
        &user.username,
        1,
        "fixture",
        Some("办公通知"),
        false,
        chrono::Utc::now() - chrono::Duration::hours(2),
    );
    expire_identity_for_cache_read(&mut runtime, &user);

    let page = runtime
        .search_info_news(1, "fixture".to_owned(), Some("办公通知".to_owned()), false)
        .await
        .expect("bounded stale INFO search cache should remain readable");

    assert_eq!(page.source.as_deref(), Some("cache"));
    assert_eq!(page.status.as_deref(), Some("stale"));
    assert!(page.error.is_some());
    assert_eq!(page.items.len(), 1);
    assert!(server.requests().is_empty());
    assert!(!runtime.private_credential_attempted);
}

#[test]
fn backend_repair_info_news_cache_context_rejects_account_and_query_drift() {
    let payload = InfoNewsPageCachePayload {
        account_scope: cache_account_scope("account-a"),
        feed: "search".to_owned(),
        page: 1,
        page_size: 0,
        source_id: None,
        channel_id: None,
        keyword: "fixture".to_owned(),
        channel_filter_label: Some("办公通知".to_owned()),
        exact_match: false,
        generated_at: chrono::Utc::now(),
        items: vec![info_cache_item(
            "fixture-news-id",
            "/info/article?entry=fixture",
        )],
    };
    assert!(info_news_cache_payload_is_valid(
        &payload,
        "account-a",
        "search",
        1,
        0,
        None,
        None,
        "fixture",
        Some("办公通知"),
        false,
    ));
    assert!(!info_news_cache_payload_is_valid(
        &payload,
        "account-b",
        "search",
        1,
        0,
        None,
        None,
        "fixture",
        Some("办公通知"),
        false,
    ));
    assert!(!info_news_cache_payload_is_valid(
        &payload,
        "account-a",
        "search",
        2,
        0,
        None,
        None,
        "fixture",
        Some("办公通知"),
        false,
    ));
    assert!(!info_news_cache_payload_is_valid(
        &payload,
        "account-a",
        "search",
        1,
        0,
        None,
        None,
        "other-query",
        Some("办公通知"),
        false,
    ));
}

#[tokio::test]
async fn backend_repair_identity_expiry_without_saved_credentials_does_not_read_or_replay_login() {
    let mut runtime = runtime(&FixtureServer::new(vec![]), false);
    let user = user();
    runtime.coordinator.logout(ServiceId::Identity).unwrap();
    runtime
        .coordinator
        .begin_authentication(ServiceId::Identity)
        .unwrap();
    runtime
        .coordinator
        .mark_authenticated(
            ServiceId::Identity,
            user,
            None,
            None,
            Some(chrono::Utc::now() - chrono::Duration::seconds(1)),
        )
        .unwrap();
    runtime.remember_credentials = false;
    runtime.credentials_persisted = false;
    runtime.credential_username = None;

    let error = runtime
        .refresh_identity_after_expiry()
        .await
        .expect_err("expiry without explicit credential opt-in must stop");
    assert!(error.contains("统一认证会话已过期"));
    assert!(!runtime.credential_recovery_attempted);
    assert_eq!(runtime.status().state, "expired");
}

#[test]
fn backend_repair_recovery_login_reset_preserves_one_shot_context() {
    let server = FixtureServer::new(vec![]);
    let mut runtime = runtime(&server, false);
    let metadata = crate::session_persistence::ResumeAccountMetadata::new(
        "fixture-user",
        Some(AcademicStage::Graduate),
        Some("0123456789abcdef0123456789abcdef"),
        true,
    )
    .expect("safe recovery metadata");
    runtime.resume_account_metadata = Some(metadata);
    runtime.remember_credentials = true;
    runtime.credentials_persisted = true;
    runtime.credential_username = Some("fixture-user".to_owned());
    runtime.automatic_refresh_attempted.insert(ServiceId::Info);
    runtime
        .automatic_refresh_results
        .insert(ServiceId::Learn, Err("bounded fixture failure".to_owned()));
    runtime.recovery_only_pending_second_factor = true;

    runtime.reset_for_login(true);

    assert!(
        runtime
            .automatic_refresh_attempted
            .contains(&ServiceId::Info)
    );
    assert_eq!(
        runtime.automatic_refresh_results.get(&ServiceId::Learn),
        Some(&Err("bounded fixture failure".to_owned()))
    );
    assert!(runtime.resume_account_metadata.is_some());
    assert!(runtime.credentials_persisted);
    assert!(!runtime.recovery_only_pending_second_factor);
    assert_eq!(
        runtime
            .coordinator
            .registry()
            .snapshot_for(ServiceId::Identity)
            .state,
        ServiceSessionState::Anonymous
    );
}

#[test]
fn backend_repair_status_exposes_only_matching_saved_recovery_context() {
    let server = FixtureServer::new(vec![]);
    let mut runtime = runtime(&server, false);
    runtime.resume_account_metadata = Some(
        crate::session_persistence::ResumeAccountMetadata::new(
            "fixture-user",
            Some(AcademicStage::Graduate),
            Some("0123456789abcdef0123456789abcdef"),
            true,
        )
        .expect("safe recovery metadata"),
    );
    runtime.credentials_persisted = true;
    runtime.credential_username = Some("fixture-user".to_owned());

    let status = runtime.status();
    assert!(status.automatic_recovery_enabled);
    let debug = format!("{status:?}").to_ascii_lowercase();
    for forbidden in ["password", "cookie", "ticket", "csrf"] {
        assert!(!debug.contains(forbidden));
    }

    runtime.credential_username = Some("other-account".to_owned());
    assert!(!runtime.status().automatic_recovery_enabled);
}

#[test]
fn backend_repair_logout_clears_saved_recovery_status_and_account_context() {
    let server = FixtureServer::new(vec![]);
    let mut runtime = runtime(&server, false);
    runtime.resume_account_metadata = Some(
        crate::session_persistence::ResumeAccountMetadata::new(
            "fixture-user",
            Some(AcademicStage::Graduate),
            Some("0123456789abcdef0123456789abcdef"),
            true,
        )
        .expect("safe recovery metadata"),
    );
    runtime.remember_credentials = true;
    runtime.credentials_persisted = true;
    runtime.credential_username = Some("fixture-user".to_owned());

    let status = runtime.logout().expect("logout clears recovery state");
    assert_eq!(status.state, "signed_out");
    assert!(status.username.is_none());
    assert!(!status.automatic_recovery_enabled);
    assert!(runtime.resume_account_metadata.is_none());
    assert!(!runtime.remember_credentials);
    assert!(!runtime.credentials_persisted);
    assert!(runtime.credential_username.is_none());
}

#[tokio::test]
async fn backend_repair_followup_registrar_info_result_object_can_use_direct_calendar_proof() {
    let server = FixtureServer::new(vec![
        Reply::html("XSRF-TOKEN=fixture-csrf;"),
        Reply::json(r#"{"result":"error","object":null}"#),
        calendar("thyouCalendar([])"),
    ]);
    let mut r = runtime(&server, false);
    r.establish_registrar_runtime_at(&user(), &base(&server))
        .await
        .expect("direct calendar proof");
    assert!(r.service_session_is_proven(ServiceId::Registrar));
    let requests = server.requests();
    assert_eq!(requests.len(), 3);
    assert!(requests[1].contains("onlineAppRedirect"));
    assert!(requests[2].contains("jxmh_out.do"));
    assert!(requests[2].contains("jsoncallback=thyouCalendar"));
    assert_no_login_or_direct_ticket(&requests);
}

#[tokio::test]
async fn backend_repair_followup_registrar_uses_reference_student_id_before_first_selector() {
    let cases = [
        (
            "2026000000",
            false,
            AcademicStage::Graduate,
            "BEABB32641DC4EC3510B048BAF42471A",
            "m=yjs_jxrl_all",
        ),
        (
            "2026113469",
            true,
            AcademicStage::Undergraduate,
            "287C0C6D90ABB364CD5FDF1495199962",
            "m=bks_jxrl_all",
        ),
    ];

    for (username, preferred_graduate, expected_stage, selector, calendar_route) in cases {
        let server = FixtureServer::new(replies(calendar("thyouCalendar([])")));
        let identity_user = student_user(username);
        let mut r = auto_runtime_for_user(&server, preferred_graduate, identity_user.clone());
        r.establish_registrar_runtime_at(&identity_user, &base(&server))
            .await
            .expect("reference stage selector calendar proof");

        assert_eq!(r.resolved_academic_stage(), Some(expected_stage));
        let requests = server.requests();
        assert_eq!(requests.len(), 5);
        assert!(requests[1].contains(&format!("yyfwid={selector}")));
        assert!(requests[4].contains(calendar_route));
        assert_no_login_or_direct_ticket(&requests);
    }
}

#[tokio::test]
async fn backend_repair_followup_explicit_stage_overrides_reference_rule() {
    let cases = [
        (
            "2026000000",
            AcademicStage::Undergraduate,
            "287C0C6D90ABB364CD5FDF1495199962",
            "m=bks_jxrl_all",
        ),
        (
            "2026113469",
            AcademicStage::Graduate,
            "BEABB32641DC4EC3510B048BAF42471A",
            "m=yjs_jxrl_all",
        ),
    ];

    for (username, selected_stage, selector, calendar_route) in cases {
        let server = FixtureServer::new(replies(calendar("thyouCalendar([])")));
        let identity_user = student_user(username);
        let mut r = auto_runtime_for_user(&server, false, identity_user.clone());

        // This is the state established by an explicit LoginPage selection.
        // It must remain authoritative even when the Reference digit rule
        // would choose the opposite stage.
        r.stage = selected_stage;
        r.stage_selection_explicit = true;
        r.stage_detected = false;

        r.establish_registrar_runtime_at(&identity_user, &base(&server))
            .await
            .expect("explicit stage calendar proof");

        assert_eq!(r.resolved_academic_stage(), Some(selected_stage));
        let requests = server.requests();
        assert_eq!(requests.len(), 5);
        assert!(requests[1].contains(&format!("yyfwid={selector}")));
        assert!(requests[4].contains(calendar_route));
        assert_no_login_or_direct_ticket(&requests);
    }
}

#[tokio::test]
async fn backend_repair_followup_explicit_stage_does_not_switch_after_info_failure() {
    let server = FixtureServer::new(vec![
        Reply::html("XSRF-TOKEN=fixture-csrf;"),
        Reply::json(r#"{"result":"error","object":null}"#),
        Reply::html("<html><input type='password' name='j_password'></html>"),
    ]);
    let identity_user = student_user("2026000000");
    let mut r = auto_runtime_for_user(&server, false, identity_user.clone());
    r.stage = AcademicStage::Undergraduate;
    r.stage_selection_explicit = true;
    r.stage_detected = false;

    assert!(
        r.establish_registrar_runtime_at(&identity_user, &base(&server))
            .await
            .is_err()
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 3);
    assert!(requests[1].contains("yyfwid=287C0C6D90ABB364CD5FDF1495199962"));
    assert!(
        requests
            .iter()
            .all(|request| !request.contains("yyfwid=BEABB32641DC4EC3510B048BAF42471A"))
    );
    assert_eq!(r.resolved_academic_stage(), None);
    assert!(!r.service_session_is_proven(ServiceId::Registrar));
    assert_no_login_or_direct_ticket(&requests);
}

#[tokio::test]
async fn backend_repair_followup_registrar_auto_detects_graduate_after_undergraduate_failure() {
    let server = FixtureServer::new(vec![
        // The preferred undergraduate selector is rejected by INFO.  The
        // direct calendar probe cannot prove that stage, so detection may
        // make exactly one opposite-selector attempt.
        Reply::html("XSRF-TOKEN=fixture-csrf;"),
        Reply::json(r#"{"result":"error","object":null}"#),
        Reply::html("<html><input type='password' name='j_password'></html>"),
        // The graduate selector receives a fresh INFO cookie bootstrap and a
        // valid target.  The mapped navigation is followed before the real
        // calendar JSONP proof is accepted.
        Reply::html("XSRF-TOKEN=fixture-csrf;"),
        Reply::json(
            r#"{"result":"success","object":{"roamingurl":"http://zhjw.cic.tsinghua.edu.cn/jxmh.do?ticket=FIXTURE%2Babc%3D"}}"#,
        ),
        Reply {
            status: 302,
            headers: format!(
                "Location: {REG_PATH}jxmh.do?m=home\r\nSet-Cookie: fixture-registrar=ready; Path={REG_PATH}\r\n"
            ),
            body: String::new(),
        },
        Reply::html(
            "<html><script src='/wengine-vpn/webvpn.js'></script><h1>Academic portal</h1></html>",
        ),
        calendar("thyouCalendar([])"),
    ]);
    let mut r = auto_runtime(&server, false);
    r.establish_registrar_runtime_at(&user(), &base(&server))
        .await
        .expect("graduate calendar proof");

    assert_eq!(r.resolved_academic_stage(), Some(AcademicStage::Graduate));
    assert!(r.service_session_is_proven(ServiceId::Registrar));
    let requests = server.requests();
    assert_eq!(requests.len(), 8);
    assert!(requests[1].contains("yyfwid=287C0C6D90ABB364CD5FDF1495199962"));
    assert!(requests[4].contains("yyfwid=BEABB32641DC4EC3510B048BAF42471A"));
    assert!(requests[2].contains("m=bks_jxrl_all"));
    assert!(requests[7].contains("m=yjs_jxrl_all"));
    assert!(requests[7].contains("jsoncallback=thyouCalendar"));
    assert_no_login_or_direct_ticket(&requests);
}

#[tokio::test]
async fn backend_repair_followup_registrar_auto_detection_does_not_switch_on_network_or_login() {
    let failures = [
        Reply {
            status: 503,
            headers: String::new(),
            body: "unavailable".into(),
        },
        Reply::html("<html><input type='password' name='i_pass'></html>"),
    ];
    for failure in failures {
        let server = FixtureServer::new(vec![Reply::html("XSRF-TOKEN=fixture-csrf;"), failure]);
        let mut r = auto_runtime(&server, false);
        assert!(
            r.establish_registrar_runtime_at(&user(), &base(&server))
                .await
                .is_err()
        );
        assert_eq!(r.resolved_academic_stage(), None);
        let requests = server.requests();
        assert_eq!(requests.len(), 2);
        assert!(requests[1].contains("yyfwid=287C0C6D90ABB364CD5FDF1495199962"));
        assert!(!requests[1].contains("yyfwid=BEABB32641DC4EC3510B048BAF42471A"));
        assert_no_login_or_direct_ticket(&requests);
    }
}

#[tokio::test]
async fn backend_repair_followup_registrar_proof_unlocks_grades_and_exams_without_new_handoffs() {
    let grade = "<html><table id='table1' cellspacing='1'><tr><th>序号</th><th>课程号</th><th>课程类别</th><th>课程名称</th><th>属性</th><th>学分</th><th>学时</th><th>成绩</th><th>备注</th><th>绩点</th><th>教师</th><th>学期</th></tr><tr><td>1</td><td>30240512</td><td>必修</td><td>Fixture course</td><td>必修</td><td>4</td><td>64</td><td>A</td><td></td><td>4</td><td>Fixture</td><td>2026-秋</td></tr></table></html>";
    let mut responses = replies(calendar("thyouCalendar([])"));
    responses.extend([Reply::html(grade), Reply::html(EMPTY_EXAMS)]);
    let server = FixtureServer::new(responses);
    let mut r = runtime(&server, false);
    let result = r
        .establish_registrar_runtime_at(&user(), &base(&server))
        .await;
    // Only locally generated request paths appear in this fixture failure;
    // production reports never print a request or a raw transport message.
    assert!(
        result.is_ok(),
        "synthetic handoff: {result:?}; requests: {:?}",
        server
            .requests()
            .iter()
            .map(|r| r.lines().next().unwrap_or(""))
            .collect::<Vec<_>>()
    );
    assert_eq!(r.load_grades().await.unwrap().courses.len(), 1);
    assert!(
        r.load_exams(String::new(), String::new())
            .await
            .unwrap()
            .exams
            .is_empty()
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 7);
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.contains("onlineAppRedirect"))
            .count(),
        1
    );
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.contains("jxmh_out.do"))
            .count(),
        1
    );
    assert_no_login_or_direct_ticket(&requests);
}

#[tokio::test]
async fn backend_repair_followup_registrar_invalid_calendar_never_replays_or_clears_other_services()
{
    let failures = [
        (
            Reply::html("<html><input type='password' name='j_password'></html>"),
            "registrar_session_expired",
        ),
        (
            Reply::html("<html>neutral placeholder</html>"),
            "registrar_calendar_content_type",
        ),
        (calendar("wrongCallback([])"), "registrar_calendar_jsonp"),
        (
            Reply {
                status: 503,
                headers: String::new(),
                body: "unavailable".into(),
            },
            "registrar_http",
        ),
    ];
    for (proof, expected) in failures {
        let server = FixtureServer::new(replies(proof));
        let mut r = runtime(&server, false);
        let error = r
            .establish_registrar_runtime_at(&user(), &base(&server))
            .await
            .unwrap_err();
        assert_eq!(crate::telemetry::diagnostic_reason(&error), expected);
        assert_eq!(
            r.establish_registrar_runtime_at(&user(), &base(&server))
                .await
                .unwrap_err(),
            error
        );
        assert!(!r.service_session_is_proven(ServiceId::Registrar));
        assert!(r.service_session_is_proven(ServiceId::Identity));
        assert!(r.service_session_is_proven(ServiceId::Learn));
        assert!(r.service_session_is_proven(ServiceId::Info));
        assert!(r.grades_source.is_none() && r.resolver.is_none());
        assert_eq!(server.requests().len(), 5);
        assert_no_login_or_direct_ticket(&server.requests());
    }
}

#[tokio::test]
async fn backend_repair_followup_registrar_foreign_or_sibling_target_is_never_consumed() {
    for raw in [
        "http://evil.invalid/jxmh.do?ticket=FIXTURE",
        "http://zhjw.cic.tsinghua.edu.cn:8080/jxmh.do?ticket=FIXTURE",
        "https://zhjw.cic.tsinghua.edu.cn/jxmh.do?ticket=FIXTURE",
    ] {
        let server = FixtureServer::new(vec![
            Reply::html("XSRF-TOKEN=fixture;"),
            Reply::json(&serde_json::json!({"object":{"roamingurl":raw}}).to_string()),
        ]);
        let mut r = runtime(&server, false);
        assert!(
            r.establish_registrar_runtime_at(&user(), &base(&server))
                .await
                .is_err()
        );
        assert!(
            r.establish_registrar_runtime_at(&user(), &base(&server))
                .await
                .is_err()
        );
        assert_eq!(server.requests().len(), 2);
        assert!(!r.service_session_is_proven(ServiceId::Registrar));
    }
}

#[tokio::test]
async fn backend_repair_followup_registrar_account_and_transport_binding_fail_before_io() {
    let server = FixtureServer::new(vec![]);
    for mismatch in ["user", "info", "transport"] {
        let mut r = runtime(&server, false);
        let supplied = if mismatch == "user" {
            UserIdentity {
                username: "other-fixture".into(),
                display_name: None,
            }
        } else {
            user()
        };
        if mismatch == "info" {
            r.coordinator.logout(ServiceId::Info).unwrap();
            r.coordinator.begin_authentication(ServiceId::Info).unwrap();
            r.coordinator
                .mark_authenticated(
                    ServiceId::Info,
                    UserIdentity {
                        username: "other-fixture".into(),
                        display_name: None,
                    },
                    None,
                    None,
                    None,
                )
                .unwrap();
        }
        let client = registrar(&r, &server, mismatch == "transport");
        let today = campus_date_at(Utc::now());
        assert!(
            r.establish_registrar_from_info(
                &supplied,
                &client,
                CalendarWindow::new(today, today).unwrap()
            )
            .await
            .is_err()
        );
        assert!(!r.service_session_is_proven(ServiceId::Registrar));
    }
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn backend_repair_followup_registrar_unconfirmed_attempt_blocks_dependents_until_reset() {
    let server = FixtureServer::new(vec![]);
    let mut r = runtime(&server, false);
    r.registrar_handoff_failure = Some("registrar_handoff_interrupted");
    let result = r
        .establish_registrar_runtime_at(&user(), &base(&server))
        .await
        .unwrap_err();
    assert_eq!(
        crate::telemetry::diagnostic_reason(&result),
        "registrar_handoff_interrupted"
    );
    assert!(server.requests().is_empty());
    assert!(r.service_session_is_proven(ServiceId::Learn));
    r.reset_sessions();
    assert!(r.registrar_handoff_failure.is_none());
}

#[tokio::test]
async fn backend_repair_followup_registrar_info_failure_has_no_ticket_or_password_fallback() {
    let server = FixtureServer::new(vec![
        Reply::html("XSRF-TOKEN=fixture;"),
        Reply {
            status: 503,
            headers: String::new(),
            body: "unavailable".into(),
        },
    ]);
    let mut r = runtime(&server, false);
    assert!(
        r.establish_registrar_runtime_at(&user(), &base(&server))
            .await
            .is_err()
    );
    assert!(
        r.establish_registrar_runtime_at(&user(), &base(&server))
            .await
            .is_err()
    );
    assert_eq!(server.requests().len(), 2);
    assert_no_login_or_direct_ticket(&server.requests());
    assert!(r.service_session_is_proven(ServiceId::Learn));
    assert!(r.service_session_is_proven(ServiceId::Info));
}

fn run072714_grade_html(graduate: bool) -> String {
    let (head, data) = if graduate {
        (
            vec![
                "课程号",
                "课程名称",
                "学分",
                "考核方式",
                "成绩",
                "绩点",
                "学期",
            ],
            vec![
                "fixture-course",
                "Fixture",
                "3",
                "考试",
                "A",
                "4.0",
                "2026-2027-1",
            ],
        )
    } else {
        (
            vec!["课程号", "课程名称", "学分", "成绩", "绩点", "学期"],
            vec!["fixture-course", "Fixture", "2", "A", "4.0", "2026-2027-1"],
        )
    };
    let row = |cells: Vec<&str>| {
        format!(
            "<tr>\n{}\n</tr>",
            cells
                .iter()
                .map(|v| format!("<td>{v}</td>"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    format!(
        "<table id='table1' cellspacing='1'>{}{}</table>",
        row(head),
        row(data)
    )
}
fn run072714_grade_runtime(server: &FixtureServer, graduate: bool) -> CampusRuntime {
    let mut r = runtime(server, graduate);
    r.coordinator
        .begin_authentication(ServiceId::Registrar)
        .unwrap();
    r.coordinator
        .mark_authenticated(ServiceId::Registrar, user(), None, None, None)
        .unwrap();
    r.grades_source = r.learn_source.clone();
    r.stage_detected = true;
    r
}
#[tokio::test]
async fn backend_repair_run072714_grade_public_read_accepts_reference_table_without_new_sso() {
    for graduate in [false, true] {
        let server = FixtureServer::new(vec![Reply::html(&run072714_grade_html(graduate))]);
        let mut r = run072714_grade_runtime(&server, graduate);
        let result = r
            .load_grades()
            .await
            .expect("actual six/seven column report");
        assert_eq!(result.source.as_deref(), Some("live"));
        assert_eq!(result.courses.len(), 1);
        assert_eq!(server.requests().len(), 1);
        let method = if graduate { "yjs_cjdcx" } else { "bks_cjdcx" };
        assert!(server.requests()[0].contains(&format!("m={method}&cjdlx=zw")));
        assert!(r.registrar_grade_handoff_failure.is_none());
    }
}
#[tokio::test]
async fn backend_repair_run072714_grade_permission_uses_report_selector_not_calendar() {
    for graduate in [false, true] {
        let server = FixtureServer::new(vec![
            Reply::html("<html>没有权限</html>"),
            Reply::html("XSRF-TOKEN=fixture-csrf;"),
            Reply::json(
                r#"{"result":"success","object":{"roamingurl":"http://zhjw.cic.tsinghua.edu.cn/cj.cjCjbAll.do?m=home"}}"#,
            ),
            Reply::html("<html>Target report session established</html>"),
            Reply::html(&run072714_grade_html(graduate)),
        ]);
        let mut r = run072714_grade_runtime(&server, graduate);
        let result = r
            .load_grades()
            .await
            .expect("grade-specific SSO then same report read");
        assert_eq!(result.source.as_deref(), Some("live"));
        assert_eq!(result.courses.len(), 1);
        let requests = server.requests();
        assert_eq!(requests.len(), 5);
        let selector = if graduate {
            "E35232808C08C8C5F199F13BF6B7F5D0"
        } else {
            "B7EF0ADF9406335AD7905B30CD7B49B1"
        };
        assert!(requests[2].contains(selector));
        assert_eq!(requests[0].lines().next(), requests[4].lines().next());
        assert!(
            requests
                .iter()
                .all(|r| !r.contains("ALL_ZHJW") && !r.contains("login/check"))
        );
        assert!(r.registrar_grade_handoff_failure.is_none());
    }
}
#[tokio::test]
async fn backend_repair_run072714_grade_malformed_rows_do_not_trigger_handoff_and_keep_diagnostics()
{
    let server = FixtureServer::new(vec![Reply::html(
        &run072714_grade_html(true).replace("<td>3</td>", "<td>not-a-number</td>"),
    )]);
    let mut r = run072714_grade_runtime(&server, true);
    let error = r.load_grades().await.unwrap_err();
    assert_eq!(
        crate::telemetry::diagnostic_reason(&error),
        "registrar_grade_number"
    );
    assert!(!error.contains("not-a-number"));
    assert_eq!(server.requests().len(), 1);
    assert!(r.service_session_is_proven(ServiceId::Registrar));
}
#[tokio::test]
async fn backend_repair_run072714_grade_failed_report_handoff_is_not_replayed() {
    let server = FixtureServer::new(vec![
        Reply::html("<html>没有权限</html>"),
        Reply::html("XSRF-TOKEN=fixture-csrf;"),
        Reply::json(r#"{"result":"failed","message":"synthetic selector denied"}"#),
    ]);
    let mut r = run072714_grade_runtime(&server, true);
    assert!(r.load_grades().await.is_err());
    let before = server.requests().len();
    assert_eq!(before, 3);
    assert!(r.load_grades().await.is_err());
    assert_eq!(server.requests().len(), before);
    assert!(r.service_session_is_proven(ServiceId::Identity));
    assert!(r.service_session_is_proven(ServiceId::Registrar));
}
#[tokio::test]
async fn backend_repair_run072714_grade_outage_is_not_permission_for_report_handoff() {
    let server = FixtureServer::new(vec![Reply {
        status: 503,
        headers: String::new(),
        body: String::new(),
    }]);
    let mut r = run072714_grade_runtime(&server, true);
    let error = r.load_grades().await.unwrap_err();
    assert_eq!(server.requests().len(), 1);
    assert!(r.registrar_grade_handoff_failure.is_none());
    assert!(r.service_session_is_proven(ServiceId::Registrar));
    assert!(!error.is_empty());
}

#[tokio::test]
async fn backend_repair_run072714_grade_http_with_webvpn_wrapper_does_not_reauthenticate() {
    let report = run072714_grade_html(true).replace("Fixture", "SSO seminar");
    let page = format!(
        "<html><script src='/wengine-vpn/webvpn.js'></script><script>const failure='查询失败';</script><table><tr><td>{report}</td></tr></table></html>"
    );
    let server = FixtureServer::new(vec![Reply::html(&page)]);
    let mut r = run072714_grade_runtime(&server, true);
    let result = r.load_grades().await.unwrap();
    assert_eq!(result.courses.len(), 1);
    assert_eq!(result.source.as_deref(), Some("live"));
    assert_eq!(server.requests().len(), 1);
    assert!(r.registrar_grade_handoff_failure.is_none());
}

#[tokio::test]
async fn backend_repair_run113847_public_grade_read_accepts_spanned_header_and_footer_without_roam()
{
    let header = "<tr><th colspan='8'>研究生成绩单</th></tr><tr><th rowspan='2'>课程号</th><th rowspan='2'>课程名称</th><th rowspan='2'>学分</th><th rowspan='2'>考核方式</th><th colspan='2'>考核结果</th><th rowspan='2'>学期</th><th rowspan='2'>备注</th></tr><tr><th>成绩</th><th>绩点</th></tr>";
    let row = "<tr><td>C001</td><td>Fixture</td><td>3</td><td>考试</td><td>A</td><td>4.0</td><td>2026-2027-1</td><td></td></tr>";
    let footer = "<tfoot><tr><td colspan='8'>合计学分：3</td></tr></tfoot>";
    let server = FixtureServer::new(vec![Reply::html(&format!(
        "<table cellspacing='1'>{header}{row}{footer}</table>"
    ))]);
    let mut r = run072714_grade_runtime(&server, true);
    let report = r.load_grades().await.unwrap();
    assert_eq!(report.source.as_deref(), Some("live"));
    assert_eq!(report.courses.len(), 1);
    assert_eq!(report.courses[0].grade_point, Some(4.0));
    assert_eq!(server.requests().len(), 1);
    assert!(r.registrar_grade_handoff_failure.is_none());
}
#[tokio::test]
async fn backend_repair_run113847_public_grade_short_row_has_specific_diagnostic_without_relogin() {
    let html = run072714_grade_html(true).replace("<td>4.0</td>", "");
    let server = FixtureServer::new(vec![Reply::html(&html)]);
    let mut r = run072714_grade_runtime(&server, true);
    let error = r.load_grades().await.unwrap_err();
    assert_eq!(
        crate::telemetry::diagnostic_reason(&error),
        "registrar_grade_row_columns"
    );
    assert_eq!(server.requests().len(), 1);
    assert!(r.service_session_is_proven(ServiceId::Registrar));
    assert!(r.registrar_grade_handoff_failure.is_none());
}
