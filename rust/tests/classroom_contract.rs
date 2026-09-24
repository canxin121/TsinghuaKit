use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    thread::{self, JoinHandle},
    time::Duration,
};

use tsinghua_kit::CampusHttpTransport;
use tsinghua_kit::classroom_read;
use tsinghua_kit::classroom_read::{
    CLASSROOM_LIST_PATH, CLASSROOM_LIST_QUERY, CLASSROOM_ROAM_SELECTOR, CLASSROOM_STATE_PATH,
    ClassroomAdapterError, ClassroomBusinessFailure, ClassroomHttpMethod, ClassroomParseError,
    ClassroomReadAdapter, ClassroomReadProfile, ClassroomRequestError, ClassroomSlotStatus,
};

const GB2312_BUILDING: &str = "清华园";
const GB2312_BUILDING_QUERY: &str = "%C7%E5%BB%AA%D4%B0";

struct FixtureResponse {
    status: u16,
    reason: &'static str,
    headers: Vec<&'static str>,
    body: String,
}

fn read_request(stream: &mut TcpStream) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .expect("fixture read timeout");
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let count = stream.read(&mut buffer).expect("fixture request");
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn write_response(stream: &mut TcpStream, response: &FixtureResponse) {
    let mut wire = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n",
        response.status,
        response.reason,
        response.body.len(),
    );
    for header in &response.headers {
        wire.push_str(header);
        wire.push_str("\r\n");
    }
    wire.push_str("\r\n");
    wire.push_str(&response.body);
    stream.write_all(wire.as_bytes()).expect("fixture response");
}

fn spawn_responder(responses: Vec<FixtureResponse>) -> (String, JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("fixture listener");
    let address = listener.local_addr().expect("fixture address");
    let server = thread::spawn(move || {
        let mut requests = Vec::with_capacity(responses.len());
        for response in responses {
            let (mut stream, _) = listener.accept().expect("fixture connection");
            requests.push(read_request(&mut stream));
            write_response(&mut stream, &response);
        }
        requests
    });
    (format!("http://{address}/opaque-mapping/"), server)
}

fn adapter(base_url: &str) -> ClassroomReadAdapter {
    let transport =
        CampusHttpTransport::with_timeout("THYou/classroom-contract", Duration::from_secs(5))
            .expect("fixture transport");
    ClassroomReadAdapter::try_with_transport(base_url.parse().expect("fixture base URL"), transport)
        .expect("classroom adapter")
}

fn ok(body: impl Into<String>) -> FixtureResponse {
    FixtureResponse {
        status: 200,
        reason: "OK",
        headers: Vec::new(),
        body: body.into(),
    }
}

fn list_html() -> String {
    format!(
        "<!doctype html><html><body><table><tr><td class=\"w30\"><a href=\"/http/opaque/pk.classroomctrl.do?m=qyClassroomState&amp;classroom={GB2312_BUILDING_QUERY}&amp;weeknumber=3\">第一教学楼</a></td></tr></table></body></html>"
    )
}

fn state_html(classroom_rows: usize, status_count: usize) -> String {
    let dates = (1..=7)
        .map(|day| format!("<th colspan=\"6\">周{day} (09.{:02})</th>", 11 + day))
        .collect::<String>();
    let mut rows = String::new();
    for row in 0..classroom_rows {
        let mut cells = String::from("<td>1</td><td><span>101:240</span></td><td>40</td>");
        for slot in 0..status_count {
            let class_name = match slot % 6 {
                0 => "colBound",
                1 => "colBound onteaching",
                2 => "onexam",
                3 => "onborrowed",
                4 => "ondisabled",
                _ => "onfuture",
            };
            cells.push_str(&format!("<td class=\"{class_name}\">{slot}</td>"));
        }
        rows.push_str(&format!("<tr data-row=\"{row}\">{cells}</tr>"));
    }
    format!(
        "<!doctype html><html><body><select id=\"weeknumber\"><option value=\"2\">2</option><option value=\"3\">3</option></select><div class=\"headers\">{dates}</div><div id=\"scrollContent\"><table><tbody>{rows}</tbody></table></div></body></html>"
    )
}

#[test]
fn request_plans_use_the_observed_get_routes_and_real_gb2312_query() {
    let profile = ClassroomReadProfile::new();
    assert_eq!(profile.roaming_selector(), CLASSROOM_ROAM_SELECTOR);

    let list = profile.building_list_request();
    assert_eq!(
        list.operation,
        classroom_read::ClassroomOperation::BuildingList
    );
    assert_eq!(list.method, ClassroomHttpMethod::Get);
    assert_eq!(list.path, CLASSROOM_LIST_PATH);
    assert_eq!(list.query, CLASSROOM_LIST_QUERY);
    assert!(list.body.is_none());
    assert!(list.is_relative_path());

    let state = profile
        .weekly_state_request(GB2312_BUILDING, 3)
        .expect("state request");
    assert_eq!(state.method, ClassroomHttpMethod::Get);
    assert_eq!(state.path, CLASSROOM_STATE_PATH);
    assert_eq!(
        state.query,
        format!("m=qyClassroomState&classroom={GB2312_BUILDING_QUERY}&weeknumber=3")
    );
    assert!(state.body.is_none());

    assert_eq!(
        classroom_read::gb2312_percent_encode(GB2312_BUILDING).expect("GB2312"),
        GB2312_BUILDING_QUERY
    );
    assert!(matches!(
        profile.weekly_state_request("😀", 3),
        Err(ClassroomRequestError::UnsupportedCharacter)
    ));
}

#[tokio::test]
async fn building_list_and_weekly_state_share_the_cookie_and_preserve_all_statuses() {
    let mut state_response = ok(state_html(1, 42));
    let (base_url, server) = spawn_responder(vec![
        FixtureResponse {
            headers: vec!["Set-Cookie: classroom-session=shared-fixture; Path=/"],
            ..ok(list_html())
        },
        {
            // The server below replaces this response with 401 if the state
            // request did not carry the Cookie set by the list request.
            state_response.headers.push("X-Fixture: cookie-required");
            state_response
        },
    ]);
    let adapter = adapter(&base_url);

    let list = adapter.read_buildings().await.expect("building list");
    assert_eq!(list.buildings.len(), 1);
    assert!(adapter.business_proof_matches(list.business_proof()));
    assert_eq!(list.buildings[0].name, "第一教学楼");
    assert_eq!(list.buildings[0].search_name, GB2312_BUILDING);
    assert_eq!(list.buildings[0].week_number, 3);

    let state = adapter
        .read_state_from_list(&list, 0, 3)
        .await
        .expect("weekly state");
    assert_eq!(state.valid_week_numbers, vec![2, 3]);
    assert_eq!(state.current_week_number, 3);
    assert_eq!(state.dates_of_current_week[0], "09.12");
    assert_eq!(state.classroom_states.len(), 1);
    assert_eq!(state.classroom_states[0].status.len(), 42);
    assert_eq!(
        state.classroom_states[0].status[0],
        ClassroomSlotStatus::Free
    );
    assert_eq!(
        state.classroom_states[0].status[1],
        ClassroomSlotStatus::Occupied
    );
    assert_eq!(
        state.classroom_states[0].status[2],
        ClassroomSlotStatus::Exam
    );
    assert_eq!(
        state.classroom_states[0].status[3],
        ClassroomSlotStatus::Borrowed
    );
    assert_eq!(
        state.classroom_states[0].status[4],
        ClassroomSlotStatus::Disabled
    );
    assert_eq!(
        state.classroom_states[0].status[5],
        ClassroomSlotStatus::Unknown {
            class_name: "onfuture".to_owned()
        }
    );

    let requests = server.join().expect("fixture server");
    assert_eq!(
        requests[0].lines().next().unwrap(),
        "GET /opaque-mapping/portal3rd.do?url=/portal3rd.do&m=jasJy_Xs_Js_index HTTP/1.1"
    );
    assert_eq!(
        requests[1].lines().next().unwrap(),
        "GET /opaque-mapping/pk.classroomctrl.do?m=qyClassroomState&classroom=%C7%E5%BB%AA%D4%B0&weeknumber=3 HTTP/1.1"
    );
    assert!(
        requests[1].lines().any(|line| {
            line.to_ascii_lowercase() == "cookie: classroom-session=shared-fixture"
        }),
        "state request must use the list request's Cookie jar; headers: {:?}",
        requests[1]
            .lines()
            .filter(|line| line.to_ascii_lowercase().starts_with("cookie:"))
            .collect::<Vec<_>>()
    );

    let invalid_index = adapter
        .read_state_from_list(&list, 1, 3)
        .await
        .expect_err("an out-of-range building index must be rejected locally");
    assert!(matches!(
        invalid_index,
        ClassroomAdapterError::InvalidBuildingIndex
    ));
}

#[tokio::test]
async fn a_full_classroom_handoff_page_url_is_reduced_to_its_mapping_root() {
    let (mapping_base, server) = spawn_responder(vec![ok(list_html())]);
    let origin = mapping_base
        .strip_suffix("/opaque-mapping/")
        .expect("fixture origin");
    let handoff = format!("{origin}/http/opaque-token{CLASSROOM_LIST_PATH}?{CLASSROOM_LIST_QUERY}");

    let list = adapter(&handoff)
        .read_buildings()
        .await
        .expect("building list from full handoff URL");
    assert_eq!(list.buildings.len(), 1);

    let requests = server.join().expect("fixture server");
    assert_eq!(
        requests[0].lines().next().unwrap(),
        "GET /http/opaque-token/portal3rd.do?url=/portal3rd.do&m=jasJy_Xs_Js_index HTTP/1.1"
    );
}

#[tokio::test]
async fn a_structurally_valid_state_page_can_be_empty_but_an_empty_directory_is_an_error() {
    let (base_url, server) = spawn_responder(vec![
        FixtureResponse {
            headers: vec!["Set-Cookie: classroom-session=empty-fixture; Path=/"],
            ..ok(list_html())
        },
        ok(state_html(0, 0)),
    ]);
    let classroom_adapter = adapter(&base_url);
    let list = classroom_adapter
        .read_buildings()
        .await
        .expect("building list");
    let state = classroom_adapter
        .read_state_from_list(&list, 0, 3)
        .await
        .expect("valid empty state");
    assert!(state.classroom_states.is_empty());

    let requests = server.join().expect("fixture server");
    assert_eq!(requests.len(), 2);

    let (base_url, server) = spawn_responder(vec![ok(
        "<html><body><div class=\"w30\"></div></body></html>",
    )]);
    let error = adapter(&base_url)
        .read_buildings()
        .await
        .expect_err("empty directory must not become an empty success");
    assert!(matches!(
        error,
        ClassroomAdapterError::Parse(ClassroomParseError::EmptyBuildingList)
    ));
    server.join().expect("fixture server");
}

#[tokio::test]
async fn http_200_login_and_timeout_pages_are_classified_before_parsing() {
    let (base_url, server) = spawn_responder(vec![ok(
        "<html><head><title>清华大学WebVPN</title></head><body><form><input name=\"i_user\"><input name=\"i_pass\"></form></body></html>",
    )]);
    let error = adapter(&base_url)
        .read_buildings()
        .await
        .expect_err("login page");
    assert!(matches!(error, ClassroomAdapterError::SessionExpired));
    server.join().expect("fixture server");

    let (base_url, server) =
        spawn_responder(vec![ok("time out用户登陆超时或访问内容不存在。请重试")]);
    let error = adapter(&base_url)
        .read_buildings()
        .await
        .expect_err("timeout page");
    assert!(matches!(
        error,
        ClassroomAdapterError::BusinessFailure {
            kind: ClassroomBusinessFailure::UpstreamTimeoutOrMissingPage
        }
    ));
    server.join().expect("fixture server");
}

#[tokio::test]
async fn cross_origin_and_same_origin_route_drift_are_rejected() {
    let (base_url, server) = spawn_responder(vec![FixtureResponse {
        status: 302,
        reason: "Found",
        headers: vec!["Location: https://outside.example.test/login"],
        body: String::new(),
    }]);
    let error = adapter(&base_url)
        .read_buildings()
        .await
        .expect_err("cross-origin redirect");
    assert!(matches!(error, ClassroomAdapterError::UnexpectedOrigin));
    server.join().expect("fixture server");

    let (base_url, server) = spawn_responder(vec![
        FixtureResponse {
            status: 302,
            reason: "Found",
            headers: vec!["Location: /opaque-mapping/other.do"],
            body: String::new(),
        },
        ok("<html><body>wrong route</body></html>"),
    ]);
    let error = adapter(&base_url)
        .read_buildings()
        .await
        .expect_err("same-origin route drift");
    assert!(matches!(error, ClassroomAdapterError::UnexpectedPath));
    server.join().expect("fixture server");
}

#[tokio::test]
async fn http_200_locations_and_same_origin_login_redirects_fail_closed() {
    let (base_url, server) = spawn_responder(vec![FixtureResponse {
        headers: vec!["Location: https://outside.example.test/login"],
        ..ok(list_html())
    }]);
    let error = adapter(&base_url)
        .read_buildings()
        .await
        .expect_err("cross-origin HTTP 200 location");
    assert!(matches!(error, ClassroomAdapterError::UnexpectedOrigin));
    server.join().expect("fixture server");

    let (base_url, server) = spawn_responder(vec![
        FixtureResponse {
            status: 302,
            reason: "Found",
            headers: vec!["Location: /opaque-mapping/login"],
            body: String::new(),
        },
        ok("<html><body>login</body></html>"),
    ]);
    let error = adapter(&base_url)
        .read_buildings()
        .await
        .expect_err("same-origin login redirect");
    assert!(matches!(error, ClassroomAdapterError::SessionExpired));
    server.join().expect("fixture server");

    let (base_url, server) = spawn_responder(vec![FixtureResponse {
        headers: vec!["Location: %ZZ"],
        ..ok(list_html())
    }]);
    let error = adapter(&base_url)
        .read_buildings()
        .await
        .expect_err("malformed location");
    assert!(matches!(error, ClassroomAdapterError::UnexpectedOrigin));
    server.join().expect("fixture server");
}

#[tokio::test]
async fn malformed_links_and_short_rows_are_rejected_without_fabricated_data() {
    let malformed_link = "<html><body><div class=\"w30\"><a href=\"/http/opaque/not-classroom.do?m=qyClassroomState&amp;classroom=x&amp;weeknumber=3\">x</a></div></body></html>";
    let (base_url, server) = spawn_responder(vec![ok(malformed_link)]);
    let error = adapter(&base_url)
        .read_buildings()
        .await
        .expect_err("route drift in building link");
    assert!(matches!(
        error,
        ClassroomAdapterError::Parse(ClassroomParseError::BuildingLinkPath)
    ));
    server.join().expect("fixture server");

    let (base_url, server) = spawn_responder(vec![
        FixtureResponse {
            headers: vec!["Set-Cookie: classroom-session=short-row; Path=/"],
            ..ok(list_html())
        },
        ok(state_html(1, 41)),
    ]);
    let adapter = adapter(&base_url);
    let list = adapter.read_buildings().await.expect("building list");
    let error = adapter
        .read_state_from_list(&list, 0, 3)
        .await
        .expect_err("41 slots must fail the boundary");
    assert!(matches!(
        error,
        ClassroomAdapterError::Parse(ClassroomParseError::InvalidRowLayout)
    ));
    server.join().expect("fixture server");
}

#[tokio::test]
async fn state_requires_the_proof_from_the_same_adapter() {
    let (base_url, server) = spawn_responder(vec![ok(list_html())]);
    let first = adapter(&base_url);
    let list = first.read_buildings().await.expect("building list");
    server.join().expect("fixture server");

    let second = adapter(&base_url);
    let error = second
        .read_state(list.business_proof(), &list.buildings[0], 3)
        .await
        .expect_err("proof must be adapter-bound");
    assert!(matches!(error, ClassroomAdapterError::SessionProofMismatch));
}
