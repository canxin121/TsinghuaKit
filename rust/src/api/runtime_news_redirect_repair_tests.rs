use super::*;

const PUBLICATION: &str = "/https/77726476706e69737468656265737421e3f5468534367f1e6d119aafd641303ceb8f9190006d6afc78336870";
const LEGACY: &str =
    "/http/77726476706e69737468656265737421fdee49932a3526446d0187ab9040227bca90a6e14cc9";

fn cookie() -> Reply {
    Reply::html("XSRF-TOKEN=fixture-info-csrf;")
}
fn redirect(target: &str) -> Reply {
    Reply {
        status: 302,
        headers: format!("Location: {target}\r\n"),
        body: String::new(),
    }
}

fn make_runtime(base: &std::path::Path, server: &FixtureServer) -> CampusRuntime {
    let mut runtime = CampusRuntime::new_with_persistence(
        "2026-2027-1".into(),
        false,
        base.to_string_lossy().into_owned(),
        false,
    )
    .unwrap();
    let user = fixture_user("fixture-news-owner");
    install_info_proof(&mut runtime, &user, server);
    runtime.info_news_link_owner = Some(user.username);
    runtime
        .info_news_links
        .insert("list-article".into(), "/article".into());
    runtime
}

#[tokio::test]
async fn backend_repair_news_redirect_runtime_publication_keeps_requested_id_in_live_and_cached_result()
 {
    let base = cache_base("publication-id");
    let server = FixtureServer::new(vec![
        cookie(),
        redirect(&format!("{PUBLICATION}/#/publish/upstream-document")),
        Reply::html("<html><div id=app>shell</div></html>"),
        Reply::json(r#"{"content":"<p>Fixture document body</p>"}"#),
    ]);
    let mut runtime = make_runtime(&base, &server);
    let live = runtime
        .load_info_news_detail_result("list-article".into())
        .await
        .unwrap();
    assert_eq!(live.id, "list-article");
    assert_eq!(live.source, "live");
    assert_eq!(live.summary, "Fixture document body");
    let cached = runtime
        .load_info_news_detail_result("list-article".into())
        .await
        .unwrap();
    assert_eq!(cached.id, "list-article");
    assert_eq!(cached.source, "cache");
    assert_eq!(cached.content_html, live.content_html);
    assert_eq!(server.requests().len(), 4);
    drop(runtime);
    let _ = std::fs::remove_dir_all(base.parent().unwrap());
}

#[tokio::test]
async fn backend_repair_news_redirect_runtime_titleless_legacy_article_keeps_real_body_and_cache() {
    let base = cache_base("titleless");
    let server = FixtureServer::new(vec![
        cookie(),
        redirect(&format!("{LEGACY}/notice.aspx")),
        Reply::html(
            "<html><div class=box3><table><tr><td>Verified fixture content</td></tr></table></div></html>",
        ),
    ]);
    let mut runtime = make_runtime(&base, &server);
    let live = runtime
        .load_info_news_detail_result("list-article".into())
        .await
        .unwrap();
    assert_eq!(live.id, "list-article");
    assert!(live.title.is_empty());
    assert!(live.summary.contains("Verified fixture content"));
    let cached = runtime
        .load_info_news_detail_result("list-article".into())
        .await
        .unwrap();
    assert_eq!(cached.source, "cache");
    assert!(cached.title.is_empty());
    assert_eq!(server.requests().len(), 3);
    drop(runtime);
    let _ = std::fs::remove_dir_all(base.parent().unwrap());
}

#[tokio::test]
async fn backend_repair_news_redirect_runtime_consumed_ticket_does_not_auto_reauthenticate_or_repeat()
 {
    let base = cache_base("consumed-ticket");
    let server = FixtureServer::new(vec![cookie(), redirect("/login")]);
    let mut runtime = make_runtime(&base, &server);
    runtime
        .info_news_links
        .insert("list-article".into(), "/article?ticket=fixture-once".into());
    let error = runtime
        .load_info_news_detail_result("list-article".into())
        .await
        .unwrap_err();
    assert!(error.contains("info_news_reauth_needs_user"));
    assert_eq!(server.requests().len(), 2);
    assert!(!runtime.service_session_is_proven(ServiceId::Info));
    assert!(runtime.service_session_is_proven(ServiceId::Identity));
    drop(runtime);
    let _ = std::fs::remove_dir_all(base.parent().unwrap());
}

#[tokio::test]
async fn backend_repair_news_redirect_runtime_consumed_navigation_preserves_only_verified_stale_cache()
 {
    let base = cache_base("stale-ticket");
    let server = FixtureServer::new(vec![
        cookie(),
        redirect("/target/viewer?ticket=fixture-once"),
        redirect("/login"),
    ]);
    let mut runtime = make_runtime(&base, &server);
    let payload = detail_payload(
        "fixture-news-owner",
        "list-article",
        "/article",
        Utc::now() - ChronoDuration::hours(7),
    );
    let cache = write_detail_cache_for_user(&base, "fixture-news-owner", &payload);
    let bytes = std::fs::read(&cache).unwrap();
    let result = runtime
        .load_info_news_detail_result("list-article".into())
        .await
        .unwrap();
    assert_eq!(result.source, "cache");
    assert_eq!(result.status, "stale");
    assert!(result.error.is_some());
    assert_eq!(result.content_html, payload.content_html);
    assert_eq!(server.requests().len(), 3);
    assert_eq!(std::fs::read(&cache).unwrap(), bytes);
    drop(runtime);
    let _ = std::fs::remove_dir_all(base.parent().unwrap());
}

#[tokio::test]
async fn backend_repair_news_redirect_runtime_unknown_mapping_is_not_a_login_failure() {
    let base = cache_base("unknown-target");
    let server = FixtureServer::new(vec![cookie(), redirect("/https/unknown/notice")]);
    let mut runtime = make_runtime(&base, &server);
    let error = runtime
        .load_info_news_detail_result("list-article".into())
        .await
        .unwrap_err();
    assert!(error.contains("info_mapping_rejected"));
    assert!(runtime.service_session_is_proven(ServiceId::Info));
    assert!(runtime.service_session_is_proven(ServiceId::Identity));
    assert_eq!(server.requests().len(), 2);
    drop(runtime);
    let _ = std::fs::remove_dir_all(base.parent().unwrap());
}
