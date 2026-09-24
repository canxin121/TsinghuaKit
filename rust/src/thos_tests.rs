use super::*;
use crate::reference_test_support::{FixtureServer, Reply};

fn client(server: &FixtureServer) -> ThosClient {
    ThosClient::new(
        &Url::parse(server.base()).unwrap(),
        CampusHttpTransport::new("fixture-thos").unwrap(),
    )
    .unwrap()
}
fn task(process: &str, task_id: &str) -> Value {
    json!({"proc_inst_id":process,"task_id":task_id,"service_name":"合成待办",
        "CURRENT_ACT_NAME":"本人确认","apply_time":"2026-09-23 12:00","SCHEDULE":"50%"})
}
fn page(rows: Vec<Value>, total: u32, n: u32) -> Reply {
    Reply::json(&json!({"list":rows,"total":total,"pageNum":n}).to_string())
}

fn service(id: &str) -> Value {
    json!({"ID":id,"NAME":format!("合成服务{id}"),"UNIT_NAME":"合成部门",
        "UW_TYPE":"0","IS_TIME_VALID":"1"})
}

#[tokio::test]
async fn backend_repair_thos_reference_task_views_use_fixed_read_routes_and_shapes() {
    let cases = [
        (
            ThosListKind::Completed,
            COMPLETED,
            json!({
                "proc_inst_id":"completed-1","service_name":"合成已办","current_state":"4",
                "complete_time":"2026-09-23","SCHEDULE":"100%"
            }),
            "办理成功",
            "合成已办",
        ),
        (
            ThosListKind::Drafts,
            DRAFTS,
            json!({
                "processInstId":"draft-1","draftName":"合成草稿","modifyTime":"2026-09-23"
            }),
            "草稿",
            "合成草稿",
        ),
        (
            ThosListKind::Unread,
            UNREAD,
            json!({
                "procinst_id":"copy-1","task_id":"work-1","service_name":"合成抄送",
                "READSTATE":"1","curActName":"抄送节点"
            }),
            "已阅",
            "合成抄送",
        ),
        (
            ThosListKind::Phases,
            PHASES,
            json!({
                "AGG_PROC_ID":"phase-1","NAME":"合成阶段","STATE":"1",
                "NUMS":"1","SUMTEMP":"4"
            }),
            "正在办理",
            "合成阶段",
        ),
    ];
    for (kind, path, row, status, title) in cases {
        let server = FixtureServer::new(vec![page(vec![row], 1, 1)]);
        let result = client(&server).task_list(kind).await.unwrap();
        assert!(result.complete && result.error.is_none());
        assert_eq!(result.kind, kind.key());
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].status, status);
        assert_eq!(result.items[0].title, title);
        if kind == ThosListKind::Phases {
            assert_eq!(result.items[0].progress, Some(25));
            assert_eq!(result.items[0].node, "当前进度 1/4");
        }
        let request = &server.requests()[0];
        assert!(request.starts_with(&format!("POST /https/{MAPPING}{path} HTTP/1.1")));
        let body: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(
            body["pageNum"],
            if kind == ThosListKind::Completed {
                json!(1)
            } else {
                json!("1")
            }
        );
        assert_eq!(
            body["pageSize"],
            if kind == ThosListKind::Completed {
                json!(50)
            } else {
                json!("10")
            }
        );
    }
}

#[tokio::test]
async fn backend_repair_thos_phase_steps_use_pinned_post_and_hide_workflow_urls() {
    let server = FixtureServer::new(vec![
        page(
            vec![json!({"AGG_PROC_ID":"aggregate-one","NAME":"阶段性申请","STATE":"1"})],
            1,
            1,
        ),
        Reply::json(
            r#"[{"ACT_ORDER_ID":"1","ACT_NAME":"第一阶段","ACT_STATE":"4","itemInfo":[{"WORKITEM_ID":"private-work","SERVICE_ID":"private-service","ITEM_NAME":"阶段服务","ITEM_STATE":"1","ITEM_URL":"https://foreign.invalid/private"}]}]"#,
        ),
    ]);
    let client = client(&server);
    let list = client
        .task_list_with_selectors(ThosListKind::Phases)
        .await
        .unwrap();
    assert!(list.dto.complete);
    let task_id = &list.dto.items[0].id;
    assert_eq!(
        list.phase_selectors.get(task_id).map(String::as_str),
        Some("aggregate-one")
    );
    assert_ne!(task_id, "aggregate-one");
    let steps = client.phase_steps("aggregate-one").await.unwrap();
    assert_eq!(steps[0].name, "第一阶段");
    assert_eq!(steps[0].state, "已完成");
    assert_eq!(steps[0].items[0].name, "阶段服务");
    assert_eq!(steps[0].items[0].state, "正在办理");
    let presentation = format!("{steps:?}");
    assert!(!presentation.contains("private-work"));
    assert!(!presentation.contains("private-service"));
    assert!(!presentation.contains("foreign.invalid"));
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].starts_with(&format!(
        "POST /https/{MAPPING}/fp/aggregation/getActWork HTTP/1.1"
    )));
    let body: Value = serde_json::from_str(requests[1].split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body, json!({"agg_proc_id":"aggregate-one"}));
}

#[test]
fn backend_repair_thos_phase_steps_reject_malformed_arrays_and_login_envelopes() {
    for body in [
        r#"{"result":"success","object":[]}"#,
        r#"[{"ACT_NAME":"缺少事项数组"}]"#,
        r#"[{"ACT_NAME":"阶段","itemInfo":[{}]}]"#,
        r#"[{"ACT_NAME":"阶段","itemInfo":{}}]"#,
        r#"{"code":401,"message":"请重新登录"}"#,
    ] {
        let parsed = parse_body(body);
        if body.contains("401") {
            assert_eq!(parsed, Err(ThosError::Session));
        } else {
            assert!(parsed.and_then(|value| parse_phase_steps(&value)).is_err());
        }
    }
}

#[tokio::test]
async fn backend_repair_thos_task_views_require_complete_stable_pages_and_rows() {
    let completed = |id: usize| json!({"proc_inst_id":format!("p{id}"),"service_name":"合成已办"});
    let server = FixtureServer::new(vec![
        page((0..50).map(completed).collect(), 51, 1),
        page(vec![completed(50)], 51, 2),
    ]);
    let result = client(&server)
        .task_list(ThosListKind::Completed)
        .await
        .unwrap();
    assert!(result.complete);
    assert_eq!(result.items.len(), 51);
    assert_eq!(server.requests().len(), 2);

    for responses in [
        vec![page(vec![], 1, 1)],
        vec![
            page(vec![completed(0)], 2, 1),
            page(vec![completed(0)], 2, 2),
        ],
        vec![
            page(vec![completed(0)], 2, 1),
            page(vec![completed(1)], 1, 2),
        ],
    ] {
        let server = FixtureServer::new(responses);
        let result = client(&server)
            .task_list(ThosListKind::Completed)
            .await
            .unwrap();
        assert!(!result.complete && result.error.is_some());
    }
    for body in [
        r#"{}"#,
        r#"{"list":null,"total":0}"#,
        r#"{"list":[{"proc_inst_id":"p"}],"total":1}"#,
        r#"{"list":[],"total":-1}"#,
        r#"<html>maintenance</html>"#,
    ] {
        let server = FixtureServer::new(vec![Reply::json(body)]);
        assert!(
            client(&server)
                .task_list(ThosListKind::Completed)
                .await
                .is_err()
        );
        assert_eq!(server.requests().len(), 1);
    }
    let mut changed = completed(0);
    changed["service_name"] = json!("冲突已办");
    let server = FixtureServer::new(vec![
        page(vec![completed(0)], 2, 1),
        page(vec![changed], 2, 2),
    ]);
    assert!(matches!(
        client(&server).task_list(ThosListKind::Completed).await,
        Err(ThosError::Response)
    ));
    assert!(ThosListKind::parse("../todo").is_err());
}

#[tokio::test]
async fn backend_repair_thos_services_collects_complete_pages_with_reference_filter() {
    let server = FixtureServer::new(vec![
        page(
            (0..100).map(|n| service(&format!("s{n}"))).collect(),
            101,
            1,
        ),
        page(vec![service("s100")], 101, 2),
    ]);
    let result = client(&server).services().await.unwrap();
    assert!(result.complete);
    assert_eq!(result.items.len(), 101);
    assert_eq!(result.reported_total, 101);
    assert_eq!(result.items[0].kind.as_deref(), Some("form"));
    assert_eq!(result.items[0].in_open_period, Some(true));
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(|r| r.starts_with(&format!(
        "POST /https/{MAPPING}/fp/fp/formHome/AllSvsByConditionpage HTTP/1.1"
    ))));
    let bodies: Vec<Value> = requests
        .iter()
        .map(|r| serde_json::from_str(r.split("\r\n\r\n").nth(1).unwrap()).unwrap())
        .collect();
    assert_eq!(bodies[0]["pageSize"], 100);
    assert_eq!(bodies[0]["orderBy"], "defaultAsc");
    assert_eq!(bodies[0]["isCollect"], "all");
    assert_eq!(bodies[1]["pageNum"], 2);
}

#[tokio::test]
async fn backend_repair_thos_services_rejects_bad_rows_and_conflicts() {
    for body in [
        json!({"list":[{"NAME":"缺少编号"}],"total":1,"pageNum":1}),
        json!({"list":[{"ID":"id"}],"total":1,"pageNum":1}),
        json!({"list":[],"total":0,"pageNum":2}),
        json!({"list":[],"total":-1,"pageNum":1}),
        json!({"list":null,"total":0,"pageNum":1}),
    ] {
        let server = FixtureServer::new(vec![Reply::json(&body.to_string())]);
        assert!(client(&server).services().await.is_err());
        assert_eq!(server.requests().len(), 1);
    }
    let mut changed = service("same");
    changed["NAME"] = json!("另一服务");
    let server = FixtureServer::new(vec![
        page(vec![service("same")], 2, 1),
        page(vec![changed], 2, 2),
    ]);
    assert!(matches!(
        client(&server).services().await,
        Err(ThosError::Response)
    ));
}

#[tokio::test]
async fn backend_repair_thos_services_partial_pages_never_become_empty_success() {
    for replies in [
        vec![page(vec![], 1, 1)],
        vec![
            page(vec![service("one")], 2, 1),
            page(vec![service("one")], 2, 2),
        ],
        vec![
            page(vec![service("one")], 2, 1),
            page(vec![service("two")], 1, 2),
        ],
    ] {
        let server = FixtureServer::new(replies);
        let result = client(&server).services().await.unwrap();
        assert!(!result.complete);
        assert_eq!(result.status, "partial");
        assert!(result.error.is_some());
    }
}

#[tokio::test]
async fn backend_repair_thos_collects_all_pages_and_returned_workflows() {
    let mut rows: Vec<_> = (0..49).map(|n| task(&format!("p{n}"), "t1")).collect();
    rows.push(task("p0", "t2")); // Same process, two independent pending tasks.
    let mut returned = task("returned", "");
    returned["BUTSTATUS"] = json!("1");
    let mut approve = task("approve", "");
    approve["APPROVE"] = json!("1");
    let server = FixtureServer::new(vec![
        Reply::json(r#"{"zbNum":"4","AuditSvsNum":51}"#),
        page(rows, 51, 1),
        page(vec![task("last", "t")], 51, 2),
        page(
            vec![task("p0", ""), returned, approve, task("waiting", "")],
            4,
            1,
        ),
    ]);
    let c = client(&server);
    let value = c.pending(c.counts().await.unwrap()).await.unwrap();
    assert!(value.complete);
    assert_eq!(value.items.len(), 53);
    assert_eq!(
        value
            .items
            .iter()
            .filter(|t| t.status == "退回修改")
            .count(),
        1
    );
    assert_eq!(
        value
            .items
            .iter()
            .map(|t| &t.id)
            .collect::<HashSet<_>>()
            .len(),
        53
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 4);
    let bodies: Vec<Value> = requests
        .iter()
        .map(|r| serde_json::from_str(r.split("\r\n\r\n").nth(1).unwrap()).unwrap())
        .collect();
    assert_eq!(bodies[0], json!({}));
    assert_eq!(bodies[1]["status"], "1");
    assert_eq!(bodies[1]["pageSize"], 50);
    assert_eq!(bodies[2]["pageNum"], 2);
    assert!(bodies[3].get("serviceName").is_some());
    assert!(bodies[3].get("status").is_none());
    assert!(
        requests
            .iter()
            .all(|r| r.starts_with(&format!("POST /https/{MAPPING}/fp/fp/")))
    );
}

#[tokio::test]
async fn backend_repair_thos_only_verified_empty_lists_are_empty_success() {
    let server = FixtureServer::new(vec![page(vec![], 0, 1), page(vec![], 0, 1)]);
    let result = client(&server)
        .pending(ThosCounts { todo: 0 })
        .await
        .unwrap();
    assert!(result.complete && result.items.is_empty() && result.error.is_none());
    for body in [
        r#"{}"#,
        r#"{"list":null,"total":0}"#,
        r#"{"list":[],"total":-1}"#,
        r#"{"list":[],"total":0,"pageNum":2}"#,
        r#"{"list":[{}],"total":1}"#,
        r#"{"result":"error","list":[],"total":0}"#,
        r#"<html>maintenance</html>"#,
    ] {
        let server = FixtureServer::new(vec![Reply::json(body)]);
        assert!(
            client(&server)
                .pending(ThosCounts { todo: 0 })
                .await
                .is_err()
        );
        assert_eq!(server.requests().len(), 1);
    }
}

#[tokio::test]
async fn backend_repair_thos_repeated_or_changing_pages_remain_incomplete() {
    for responses in [
        vec![
            page(vec![task("p", "t")], 2, 1),
            page(vec![task("p", "t")], 2, 2),
        ],
        vec![
            page(vec![task("p", "t")], 2, 1),
            page(vec![task("q", "t")], 1, 2),
        ],
        vec![page(vec![], 1, 1)],
    ] {
        let mut replies = responses;
        replies.push(page(vec![], 0, 1));
        let server = FixtureServer::new(replies);
        let result = client(&server)
            .pending(ThosCounts { todo: 2 })
            .await
            .unwrap();
        assert!(!result.complete);
        assert_eq!(result.status, "partial");
        assert!(result.error.is_some());
        assert!(server.requests().len() <= 3);
    }
}

#[tokio::test]
async fn backend_repair_thos_conflicting_tasks_and_counter_mismatch_are_explicit() {
    let mut changed = task("p", "t");
    changed["service_name"] = json!("冲突记录");
    let server = FixtureServer::new(vec![
        page(vec![task("p", "t")], 2, 1),
        page(vec![changed], 2, 2),
    ]);
    assert!(matches!(
        client(&server).pending(ThosCounts { todo: 2 }).await,
        Err(ThosError::Response)
    ));
    let server = FixtureServer::new(vec![page(vec![], 0, 1), page(vec![], 0, 1)]);
    let value = client(&server)
        .pending(ThosCounts { todo: 1 })
        .await
        .unwrap();
    assert!(!value.complete && value.items.is_empty() && value.error.is_some());
}

#[tokio::test]
async fn backend_repair_thos_never_replays_redirected_read_posts() {
    for status in [302, 307] {
        let server = FixtureServer::new(vec![Reply {
            status,
            headers: "Location: /unrelated\r\n".into(),
            body: String::new(),
        }]);
        assert!(matches!(
            client(&server).counts().await,
            Err(ThosError::Route)
        ));
        assert_eq!(server.requests().len(), 1);
    }
    let server = FixtureServer::new(vec![Reply {
        status: 503,
        headers: "Retry-After: 2\r\n".into(),
        body: String::new(),
    }]);
    assert!(matches!(
        client(&server).counts().await,
        Err(ThosError::Http)
    ));
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn backend_repair_thos_handoff_rejects_other_paths_before_dispatch() {
    let server = FixtureServer::new(vec![Reply {
        status: 302,
        headers: "Location: /https/another/fp/home\r\n".into(),
        body: String::new(),
    }]);
    let transport = CampusHttpTransport::new("fixture").unwrap();
    let base = Url::parse(server.base()).unwrap();
    for path in [
        "https://unrelated.test/fp/login".into(),
        format!("{}https/{MAPPING}/other/login", server.base()),
    ] {
        assert!(follow_handoff(&transport, &base, &path).await.is_err());
    }
    assert!(server.requests().is_empty());
    let target = format!("{}https/{MAPPING}/fp/login?ticket=fixture", server.base());
    assert!(matches!(
        follow_handoff(&transport, &base, &target).await,
        Err(ThosError::Route)
    ));
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn backend_repair_thos_reference_aliases_and_auth_failures_remain_typed() {
    let value = json!({"PROCINST_ID":"p","WORKITEM_INS_ID":"t","SERVICE_NAME":"事项",
        "task_name":"确认","START_TIME":"2026-09-23","BUTSTATUS":1,"SCHEDULE":"0"});
    let row = parse_task(&value, true).unwrap();
    assert_eq!(row.dto.status, "退回修改");
    assert_eq!(row.dto.node, "确认");
    assert_eq!(row.dto.date, "2026-09-23");
    assert_eq!(row.dto.progress, Some(0));
    for progress in ["", "NaN", "-1", "101", "Infinity"] {
        let mut value = value.clone();
        value["SCHEDULE"] = json!(progress);
        let task = parse_task(&value, true).unwrap();
        assert_eq!(task.dto.title, "事项");
        assert_eq!(task.dto.progress, None);
    }
    assert_eq!(
        parse_body(r#"{"code":401,"list":[],"total":0}"#),
        Err(ThosError::Session)
    );
    assert_eq!(
        parse_body(r#"<input name="i_user">"#),
        Err(ThosError::Session)
    );
    assert_eq!(
        parse_body(r#"<html>maintenance</html>"#),
        Err(ThosError::Response)
    );
    for counts in [
        json!({"AuditSvsNum":0}),
        json!({"zbNum":0,"AuditSvsNum":"NaN"}),
    ] {
        assert!(parse_counts(&counts).is_err());
    }
}

#[tokio::test]
async fn backend_repair_thos_home_fragment_preserves_handoff_path_boundary() {
    let transport = CampusHttpTransport::new("fixture-thos-home").unwrap();
    let server = FixtureServer::new(vec![
        Reply {
            status: 302,
            headers: format!("Location: /https/{MAPPING}/fp/view?m=fp#act=fp/formHome\r\n"),
            body: String::new(),
        },
        Reply::html("<html>service hall</html>"),
    ]);
    let base = Url::parse(server.base()).unwrap();
    for tail in [
        "fp/view?m=fp#act=unrelated",
        "fp/view?m=other#act=fp/formHome",
        "fp/%252e%252e/other",
        "fp/..%2fother",
        "fp/%255c..%255cother",
    ] {
        let url = format!("{}https/{MAPPING}/{tail}", server.base());
        assert!(matches!(
            follow_handoff(&transport, &base, &url).await,
            Err(ThosError::Route)
        ));
    }
    assert!(server.requests().is_empty());
    let entry = format!("{}https/{MAPPING}/fp/login?ticket=fixture", server.base());
    let home = follow_handoff(&transport, &base, &entry).await.unwrap();
    assert!(home.fragment().is_none());
    assert_eq!(home.path(), format!("/https/{MAPPING}/fp/view"));
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].starts_with(&format!("GET /https/{MAPPING}/fp/view?m=fp HTTP/1.1")));
    assert!(!requests[1].contains("#act="));
}

#[test]
fn backend_repair_thos_redirect_diagnostics_keep_other_mappings_untrusted() {
    let base = Url::parse("https://webvpn.tsinghua.edu.cn/").unwrap();
    for (path, category) in [
        (format!("/https/{MAPPING}"), "webvpn_thos_root"),
        (format!("/https/{MAPPING}/other"), "webvpn_thos_other"),
        ("/https/another/fp/login".into(), "webvpn_other_mapping_fp"),
        (
            "/https/another/do/off/ui/auth/login/form/fixed".into(),
            "webvpn_other_mapping_identity",
        ),
        (
            "/https/another/b/yyfw/service".into(),
            "webvpn_other_mapping_info",
        ),
        ("/https/another/route".into(), "webvpn_other_mapping_route"),
        ("/https/another".into(), "webvpn_other_mapping_root"),
        ("/https/another/fp/%2fother".into(), "unsafe_url"),
    ] {
        let target = base.join(&path).unwrap();
        assert_eq!(classify_read_redirect(&target, &base), category);
        assert!(!matches!(category, "mapped_thos" | "webvpn_login"));
    }
}

#[test]
fn backend_repair_thos_dynamic_identity_redirect_requires_fixed_mapping_and_form() {
    let base = Url::parse("https://webvpn.tsinghua.edu.cn/").unwrap();
    let identity_mapping = "77726476706e69737468656265737421f9f30f8834396657761d88e29d51367bcfe7";
    let valid = base
        .join(&format!(
            "/https/{identity_mapping}/do/off/ui/auth/login/form/0123456789abcdef0123456789abcdef/0"
        ))
        .unwrap();
    assert_eq!(
        classify_read_redirect(&valid, &base),
        "webvpn_identity_login"
    );
    for path in [
        format!("/https/{identity_mapping}/do/off/ui/auth/login/form/untrusted/0"),
        format!(
            "/https/{identity_mapping}/do/off/ui/auth/login/form/0123456789abcdef0123456789abcdef/extra"
        ),
        "/https/another/do/off/ui/auth/login/form/0123456789abcdef0123456789abcdef/0".into(),
    ] {
        let target = base.join(&path).unwrap();
        assert_ne!(
            classify_read_redirect(&target, &base),
            "webvpn_identity_login"
        );
    }
}
