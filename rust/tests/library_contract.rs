use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    thread::{self, JoinHandle},
    time::Duration,
};

use reqwest::Url;
use tsinghua_kit::library_read::{
    LibraryAdapterError, LibraryReadAdapter, LibraryRequestError, LibrarySocketState,
    LibrarySocketStatusAdapter,
};
use tsinghua_kit::{CampusHttpTransport, LibraryReadParseError};

fn read_request(stream: &mut TcpStream) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("fixture read timeout");
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 2048];
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

fn write_json(stream: &mut TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .expect("fixture response");
}

fn write_response(stream: &mut TcpStream, status: &str, headers: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .expect("fixture response");
}

fn spawn_responder(responses: Vec<&'static str>) -> (String, JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("fixture listener");
    let address = listener.local_addr().expect("fixture address");
    let base_url = format!("http://{address}/opaque-mapping/");
    let server = thread::spawn(move || {
        let mut requests = Vec::with_capacity(responses.len());
        for body in responses {
            let (mut stream, _) = listener.accept().expect("fixture connection");
            requests.push(read_request(&mut stream));
            write_json(&mut stream, body);
        }
        requests
    });
    (base_url, server)
}

fn adapter(base_url: &str) -> LibraryReadAdapter {
    let transport =
        CampusHttpTransport::with_timeout("THYou/library-contract", Duration::from_secs(5))
            .expect("fixture transport");
    LibraryReadAdapter::try_with_transport(base_url.parse().expect("fixture base URL"), transport)
        .expect("library adapter")
}

#[tokio::test]
async fn each_advertised_read_method_executes_its_real_route_and_envelope() {
    let (base_url, server) = spawn_responder(vec![
        r#"{"data":{"list":[{"id":35,"name":"北馆","isValid":1}]}}"#,
        r#"{"data":{"list":{"childArea":[{"id":351,"name":"一层","isValid":1}]}}}"#,
        r#"{"data":{"list":{"childArea":[{"id":351,"name":"一层","TotalCount":100,"UnavailableSpace":20}]}}}"#,
        r#"{"data":{"list":[{"day":"2026-09-11","startTime":{"date":"2026-09-11 08:00:00"},"endTime":{"date":"2026-09-11 22:00:00"},"id":9001}]}}"#,
        r#"{"data":{"list":[{"id":701,"name":"A 01","status":1,"area_type":2}]}}"#,
    ]);
    let adapter = adapter(&base_url);

    let tree = adapter.read_area_tree().await.expect("area tree");
    assert_eq!(tree.areas[0].id, 35);
    assert_eq!(tree.areas[0].name, "北馆");

    let children = adapter.read_area_children(35).await.expect("area children");
    assert_eq!(children.areas[0].id, 351);

    let dated = adapter
        .read_area_for_day(35, "2026-09-11")
        .await
        .expect("date areas");
    assert_eq!(dated.areas[0].available_count, Some(80));

    let segments = adapter.read_day_segments(351).await.expect("day segments");
    assert_eq!(segments.segments[0].id, 9001);
    assert_eq!(segments.segments[0].start_time, "08:00");

    let seats = adapter
        .read_seat_availability(351, 9001, "2026-09-11", "08:00:00", "22:00")
        .await
        .expect("seat availability");
    assert_eq!(seats.seats[0].id, 701);
    assert!(seats.seats[0].is_valid);

    let requests = server.join().expect("fixture server");
    assert_eq!(
        requests[0].lines().next().unwrap(),
        "GET /opaque-mapping/api.php/areas/1/tree/1 HTTP/1.1"
    );
    assert_eq!(
        requests[1].lines().next().unwrap(),
        "GET /opaque-mapping/api.php/areas/35 HTTP/1.1"
    );
    assert_eq!(
        requests[2].lines().next().unwrap(),
        "GET /opaque-mapping/api.php/areas/35/date/2026-09-11 HTTP/1.1"
    );
    assert_eq!(
        requests[3].lines().next().unwrap(),
        "GET /opaque-mapping/api.php/areadays/351 HTTP/1.1"
    );
    assert_eq!(
        requests[4].lines().next().unwrap(),
        "GET /opaque-mapping/api.php/spaces_old?area=351&segment=9001&day=2026-09-11&startTime=08%3A00&endTime=22%3A00 HTTP/1.1"
    );
}

#[tokio::test]
async fn every_read_method_preserves_a_wire_valid_empty_result() {
    let (base_url, server) = spawn_responder(vec![
        r#"{"data":{"list":[]}}"#,
        r#"{"data":{"list":{"childArea":[]}}}"#,
        r#"{"data":{"list":{"childArea":[]}}}"#,
        r#"{"data":{"list":[]}}"#,
        r#"{"data":{"list":[]}}"#,
    ]);
    let adapter = adapter(&base_url);

    assert!(
        adapter
            .read_area_tree()
            .await
            .expect("empty tree")
            .areas
            .is_empty()
    );
    assert!(
        adapter
            .read_area_children(35)
            .await
            .expect("empty children")
            .areas
            .is_empty()
    );
    assert!(
        adapter
            .read_area_for_day(35, "2026-09-11")
            .await
            .expect("empty date areas")
            .areas
            .is_empty()
    );
    assert!(
        adapter
            .read_day_segments(351)
            .await
            .expect("empty segments")
            .segments
            .is_empty()
    );
    assert!(
        adapter
            .read_seat_availability(351, 9001, "2026-09-11", "08:00", "22:00")
            .await
            .expect("empty seats")
            .seats
            .is_empty()
    );

    let requests = server.join().expect("fixture server");
    assert_eq!(requests.len(), 5);
}

#[tokio::test]
async fn adapter_does_not_turn_login_malformed_or_business_failure_into_empty_data() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("fixture listener");
    let address = listener.local_addr().expect("fixture address");
    let server = thread::spawn(move || {
        let responses = [
            (
                "text/html",
                "<html><form><input name=\"i_user\"><input name=\"i_pass\"></form></html>",
            ),
            ("application/json", "{"),
            ("application/json", r#"{"result":false,"data":{"list":[]}}"#),
        ];
        for (content_type, body) in responses {
            let (mut stream, _) = listener.accept().expect("fixture connection");
            let _ = read_request(&mut stream);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .expect("fixture response");
        }
    });
    let base_url = format!("http://{address}/opaque-mapping/");
    let adapter = adapter(&base_url);

    assert!(matches!(
        adapter.read_area_tree().await,
        Err(LibraryAdapterError::SessionExpired)
    ));
    assert!(matches!(
        adapter.read_area_tree().await,
        Err(LibraryAdapterError::Parse(
            LibraryReadParseError::MalformedJson { .. }
        ))
    ));
    assert!(matches!(
        adapter.read_area_tree().await,
        Err(LibraryAdapterError::Parse(
            LibraryReadParseError::FailureEnvelope
        ))
    ));
    server.join().expect("fixture server");
}

#[tokio::test]
async fn nested_or_message_only_business_failures_never_become_empty_lists() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("fixture listener");
    let address = listener.local_addr().expect("fixture address");
    let server = thread::spawn(move || {
        let responses = [
            r#"{"message":"seat service unavailable","data":{"list":[]}}"#,
            r#"{"data":{"success":false,"message":"seat query failed","list":[]}}"#,
        ];
        for body in responses {
            let (mut stream, _) = listener.accept().expect("fixture connection");
            let _ = read_request(&mut stream);
            write_json(&mut stream, body);
        }
    });
    let adapter = adapter(&format!("http://{address}/opaque-mapping/"));

    for _ in 0..2 {
        assert!(matches!(
            adapter.read_area_tree().await,
            Err(LibraryAdapterError::Parse(
                LibraryReadParseError::FailureEnvelope
            ))
        ));
    }
    server.join().expect("fixture server");
}

#[tokio::test]
async fn explicit_unavailable_and_nested_error_envelopes_never_become_empty_lists() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("fixture listener");
    let address = listener.local_addr().expect("fixture address");
    let server = thread::spawn(move || {
        let responses = [
            r#"{"unavailable":true,"data":{"list":[]}}"#,
            r#"{"data":{"error":{"reason":"backend"},"list":[]}}"#,
            r#"{"error":false,"data":{"list":[]}}"#,
        ];
        for body in responses {
            let (mut stream, _) = listener.accept().expect("fixture connection");
            let _ = read_request(&mut stream);
            write_json(&mut stream, body);
        }
    });
    let adapter = adapter(&format!("http://{address}/opaque-mapping/"));

    for _ in 0..2 {
        assert!(matches!(
            adapter.read_area_tree().await,
            Err(LibraryAdapterError::Parse(
                LibraryReadParseError::FailureEnvelope
            ))
        ));
    }
    assert!(
        adapter
            .read_area_tree()
            .await
            .expect("error:false is benign")
            .areas
            .is_empty()
    );
    server.join().expect("fixture server");
}

#[tokio::test]
async fn adapter_rejects_a_cross_origin_location_before_parsing_the_body() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("fixture listener");
    let address = listener.local_addr().expect("fixture address");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("fixture connection");
        let _ = read_request(&mut stream);
        stream
            .write_all(
                b"HTTP/1.1 302 Found\r\nLocation: https://outside.example.test/library\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .expect("fixture response");
    });
    let base_url = format!("http://{address}/opaque-mapping/");
    let adapter = adapter(&base_url);

    assert!(matches!(
        adapter.read_area_tree().await,
        Err(LibraryAdapterError::UnexpectedOrigin)
    ));
    server.join().expect("fixture server");
}

#[tokio::test]
async fn socket_status_uses_exact_route_and_reuses_the_library_cookie_session() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("fixture listener");
    let address = listener.local_addr().expect("fixture address");
    let server = thread::spawn(move || {
        let (mut library_stream, _) = listener.accept().expect("library connection");
        let library_request = read_request(&mut library_stream);
        write_response(
            &mut library_stream,
            "200 OK",
            "Content-Type: application/json\r\nSet-Cookie: handoff-proof=present; Path=/\r\n",
            r#"{"data":{"list":[]}}"#,
        );

        let (mut socket_stream, _) = listener.accept().expect("socket connection");
        let socket_request = read_request(&mut socket_stream);
        write_response(
            &mut socket_stream,
            "200 OK",
            "Content-Type: application/json; charset=UTF-8\r\n",
            r#"[{"seatId":701,"status":"available"},{"seatId":"702","status":"unavailable"},{"seatId":703,"status":"unknown"}]"#,
        );
        (library_request, socket_request)
    });

    let transport =
        CampusHttpTransport::with_timeout("THYou/library-socket-contract", Duration::from_secs(5))
            .expect("fixture transport");
    let library_base = format!("http://{address}/opaque-mapping/");
    let library = LibraryReadAdapter::try_with_transport(
        library_base.parse().expect("library base URL"),
        transport,
    )
    .expect("library adapter");
    library
        .read_area_tree()
        .await
        .expect("library session handoff fixture");

    let socket_origin = Url::parse(&format!("http://{address}/")).expect("socket origin");
    let socket = library
        .socket_status_adapter(socket_origin)
        .expect("socket adapter");
    let statuses = socket
        .read_section_status(351)
        .await
        .expect("socket statuses");

    assert_eq!(statuses.records.len(), 3);
    assert_eq!(statuses.records[0].seat_id, 701);
    assert_eq!(statuses.records[0].status, LibrarySocketState::Available);
    assert_eq!(statuses.records[1].status, LibrarySocketState::Unavailable);
    assert_eq!(statuses.records[2].status, LibrarySocketState::Unknown);

    let (library_request, socket_request) = server.join().expect("fixture server");
    assert_eq!(
        library_request.lines().next().unwrap(),
        "GET /opaque-mapping/api.php/areas/1/tree/1 HTTP/1.1"
    );
    assert_eq!(
        socket_request.lines().next().unwrap(),
        "GET /api/socket?sectionid=351 HTTP/1.1"
    );
    assert!(
        socket_request
            .split("\r\n\r\n")
            .nth(1)
            .map_or(true, str::is_empty),
        "socket status is a GET with no request body"
    );
    assert!(
        socket_request
            .lines()
            .any(|line| line.to_ascii_lowercase().starts_with("cookie:")
                && line.contains("handoff-proof=present")),
        "socket request must reuse the Cookie jar from the library handoff"
    );
}

#[tokio::test]
async fn combined_seat_read_joins_socket_state_by_seat_id_and_preserves_cookie_proof() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("fixture listener");
    let address = listener.local_addr().expect("fixture address");
    let server = thread::spawn(move || {
        let (mut seat_stream, _) = listener.accept().expect("seat connection");
        let seat_request = read_request(&mut seat_stream);
        write_response(
            &mut seat_stream,
            "200 OK",
            "Content-Type: application/json\r\nSet-Cookie: library-proof=present; Path=/\r\n",
            r#"{"data":{"list":[{"id":701,"name":"A 01","status":1,"area_type":2},{"id":702,"name":"A 02","status":0,"area_type":2}]}}"#,
        );

        let (mut socket_stream, _) = listener.accept().expect("socket connection");
        let socket_request = read_request(&mut socket_stream);
        write_response(
            &mut socket_stream,
            "200 OK",
            "Content-Type: application/json\r\n",
            r#"[{"seatId":701,"status":"available"},{"seatId":999,"status":"unavailable"}]"#,
        );
        (seat_request, socket_request)
    });

    let transport = CampusHttpTransport::with_timeout(
        "THYou/library-combined-contract",
        Duration::from_secs(5),
    )
    .expect("fixture transport");
    let library = LibraryReadAdapter::try_with_transport(
        format!("http://{address}/opaque-mapping/")
            .parse()
            .expect("library base URL"),
        transport,
    )
    .expect("library adapter");
    let socket = library
        .socket_status_adapter(format!("http://{address}/").parse().expect("socket URL"))
        .expect("socket adapter");

    let combined = library
        .read_seat_availability_with_socket(351, 9001, "2026-09-11", "08:00", "22:00", &socket)
        .await
        .expect("combined seat and socket read");
    assert_eq!(combined.seats.len(), 2);
    assert_eq!(combined.seats[0].id, 701);
    assert_eq!(
        combined.seats[0].socket_status,
        LibrarySocketState::Available
    );
    assert_eq!(
        combined.seats[1].socket_status,
        LibrarySocketState::Unknown,
        "a socket response without this seat must remain explicitly unknown"
    );

    let (seat_request, socket_request) = server.join().expect("fixture server");
    assert_eq!(
        seat_request.lines().next().unwrap(),
        "GET /opaque-mapping/api.php/spaces_old?area=351&segment=9001&day=2026-09-11&startTime=08%3A00&endTime=22%3A00 HTTP/1.1"
    );
    assert_eq!(
        socket_request.lines().next().unwrap(),
        "GET /api/socket?sectionid=351 HTTP/1.1"
    );
    assert!(
        socket_request
            .lines()
            .any(|line| line.to_ascii_lowercase().starts_with("cookie:")
                && line.contains("library-proof=present")),
        "socket GET must reuse the Cookie jar that proved the seat service response"
    );
}

#[tokio::test]
async fn combined_seat_read_propagates_socket_protocol_failure() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("fixture listener");
    let address = listener.local_addr().expect("fixture address");
    let server = thread::spawn(move || {
        let (mut seat_stream, _) = listener.accept().expect("seat connection");
        let _ = read_request(&mut seat_stream);
        write_json(
            &mut seat_stream,
            r#"{"data":{"list":[{"id":701,"name":"A 01","status":1,"area_type":2}]}}"#,
        );

        let (mut socket_stream, _) = listener.accept().expect("socket connection");
        let _ = read_request(&mut socket_stream);
        write_json(&mut socket_stream, "{");
    });

    let transport = CampusHttpTransport::with_timeout(
        "THYou/library-combined-error-contract",
        Duration::from_secs(5),
    )
    .expect("fixture transport");
    let library = LibraryReadAdapter::try_with_transport(
        format!("http://{address}/opaque-mapping/")
            .parse()
            .expect("library base URL"),
        transport,
    )
    .expect("library adapter");
    let socket = library
        .socket_status_adapter(format!("http://{address}/").parse().expect("socket URL"))
        .expect("socket adapter");

    assert!(matches!(
        library
            .read_seat_availability_with_socket(351, 9001, "2026-09-11", "08:00", "22:00", &socket)
            .await,
        Err(LibraryAdapterError::Parse(
            LibraryReadParseError::MalformedJson { .. }
        ))
    ));
    server.join().expect("fixture server");
}

#[tokio::test]
async fn combined_seat_read_maps_an_explicit_empty_socket_array_to_unknown() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("fixture listener");
    let address = listener.local_addr().expect("fixture address");
    let server = thread::spawn(move || {
        let (mut seat_stream, _) = listener.accept().expect("seat connection");
        let _ = read_request(&mut seat_stream);
        write_json(
            &mut seat_stream,
            r#"{"data":{"list":[{"id":701,"name":"A 01","status":1,"area_type":2},{"id":702,"name":"A 02","status":1,"area_type":2}]}}"#,
        );

        let (mut socket_stream, _) = listener.accept().expect("socket connection");
        let _ = read_request(&mut socket_stream);
        write_json(&mut socket_stream, "[]");
    });

    let transport = CampusHttpTransport::with_timeout(
        "THYou/library-combined-empty-socket-contract",
        Duration::from_secs(5),
    )
    .expect("fixture transport");
    let library = LibraryReadAdapter::try_with_transport(
        format!("http://{address}/opaque-mapping/")
            .parse()
            .expect("library base URL"),
        transport,
    )
    .expect("library adapter");
    let socket = library
        .socket_status_adapter(format!("http://{address}/").parse().expect("socket URL"))
        .expect("socket adapter");

    let combined = library
        .read_seat_availability_with_socket(351, 9001, "2026-09-11", "08:00", "22:00", &socket)
        .await
        .expect("explicit empty socket response");
    assert!(
        combined
            .seats
            .iter()
            .all(|seat| seat.socket_status == LibrarySocketState::Unknown)
    );
    server.join().expect("fixture server");
}

#[tokio::test]
async fn socket_status_preserves_a_wire_valid_empty_array() {
    let (base_url, server) = spawn_responder(vec!["[]"]);
    let transport = CampusHttpTransport::with_timeout(
        "THYou/library-socket-empty-contract",
        Duration::from_secs(5),
    )
    .expect("fixture transport");
    let socket = LibrarySocketStatusAdapter::try_with_transport(
        base_url.parse().expect("socket base URL"),
        transport,
    )
    .expect("socket adapter");

    let statuses = socket
        .read_section_status(351)
        .await
        .expect("explicit empty socket list");
    assert!(statuses.records.is_empty());
    assert!(matches!(
        tsinghua_kit::library_read::parse_socket_status(r#"{"data":{"list":[]}}"#),
        Err(LibraryReadParseError::InvalidCollection { .. })
    ));

    let requests = server.join().expect("fixture server");
    assert_eq!(
        requests[0].lines().next().unwrap(),
        "GET /opaque-mapping/api/socket?sectionid=351 HTTP/1.1"
    );
}

#[tokio::test]
async fn socket_status_classifies_login_bad_json_business_failure_and_bad_record() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("fixture listener");
    let address = listener.local_addr().expect("fixture address");
    let server = thread::spawn(move || {
        let responses = [
            (
                "text/html; charset=UTF-8",
                "<html><form><input name=\"i_user\"><input name=\"i_pass\"></form></html>",
            ),
            ("application/json", "{"),
            (
                "application/json",
                r#"{"success":false,"message":"socket service unavailable"}"#,
            ),
            (
                "application/json",
                r#"[{"seatId":701,"status":"plugged-in"}]"#,
            ),
        ];
        for (content_type, body) in responses {
            let (mut stream, _) = listener.accept().expect("fixture connection");
            let _ = read_request(&mut stream);
            write_response(
                &mut stream,
                "200 OK",
                &format!("Content-Type: {content_type}\r\n"),
                body,
            );
        }
    });
    let transport = CampusHttpTransport::with_timeout(
        "THYou/library-socket-errors-contract",
        Duration::from_secs(5),
    )
    .expect("fixture transport");
    let socket = LibrarySocketStatusAdapter::try_with_transport(
        format!("http://{address}/")
            .parse()
            .expect("socket base URL"),
        transport,
    )
    .expect("socket adapter");

    assert!(matches!(
        socket.read_section_status(351).await,
        Err(LibraryAdapterError::SessionExpired)
    ));
    assert!(matches!(
        socket.read_section_status(351).await,
        Err(LibraryAdapterError::Parse(
            LibraryReadParseError::MalformedJson { .. }
        ))
    ));
    assert!(matches!(
        socket.read_section_status(351).await,
        Err(LibraryAdapterError::Parse(
            LibraryReadParseError::FailureEnvelope
        ))
    ));
    assert!(matches!(
        socket.read_section_status(351).await,
        Err(LibraryAdapterError::Parse(
            LibraryReadParseError::InvalidRecord { .. }
        ))
    ));
    server.join().expect("fixture server");
}

#[tokio::test]
async fn socket_status_rejects_cross_origin_and_same_origin_wrong_path_responses() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("cross-origin listener");
    let address = listener.local_addr().expect("cross-origin address");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("cross-origin connection");
        let _ = read_request(&mut stream);
        write_response(
            &mut stream,
            "302 Found",
            "Location: https://outside.example.test/api/socket?sectionid=351\r\n",
            "",
        );
    });
    let transport = CampusHttpTransport::with_timeout(
        "THYou/library-socket-origin-contract",
        Duration::from_secs(5),
    )
    .expect("fixture transport");
    let socket = LibrarySocketStatusAdapter::try_with_transport(
        format!("http://{address}/")
            .parse()
            .expect("socket base URL"),
        transport,
    )
    .expect("socket adapter");
    assert!(matches!(
        socket.read_section_status(351).await,
        Err(LibraryAdapterError::UnexpectedOrigin)
    ));
    server.join().expect("cross-origin server");

    let listener = TcpListener::bind("127.0.0.1:0").expect("wrong-path listener");
    let address = listener.local_addr().expect("wrong-path address");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("initial socket connection");
        let _ = read_request(&mut stream);
        write_response(
            &mut stream,
            "302 Found",
            "Location: /other?sectionid=351\r\n",
            "",
        );
        let (mut redirected, _) = listener.accept().expect("redirected connection");
        let request = read_request(&mut redirected);
        write_json(&mut redirected, "[]");
        request
    });
    let transport = CampusHttpTransport::with_timeout(
        "THYou/library-socket-path-contract",
        Duration::from_secs(5),
    )
    .expect("fixture transport");
    let socket = LibrarySocketStatusAdapter::try_with_transport(
        format!("http://{address}/")
            .parse()
            .expect("socket base URL"),
        transport,
    )
    .expect("socket adapter");
    assert!(matches!(
        socket.read_section_status(351).await,
        Err(LibraryAdapterError::UnexpectedPath)
    ));
    let redirected_request = server.join().expect("wrong-path server");
    assert_eq!(
        redirected_request.lines().next().unwrap(),
        "GET /other?sectionid=351 HTTP/1.1"
    );
}

#[test]
fn request_validation_does_not_allow_url_or_invalid_time_injection() {
    let error = tsinghua_kit::LibraryReadProfile::new()
        .seat_availability_request(1, 2, "2026-09-11", "22:00", "08:00")
        .expect_err("reverse time range");
    assert_eq!(error, LibraryRequestError::InvalidTimeRange);

    let profile = tsinghua_kit::LibraryReadProfile::new();
    let plan = profile
        .socket_status_request(351)
        .expect("socket request plan");
    assert_eq!(
        plan.method,
        tsinghua_kit::library_read::LibraryReadMethod::Get
    );
    assert_eq!(plan.path, "/api/socket");
    assert_eq!(
        plan.query_parameters(),
        [("sectionid".to_owned(), "351".to_owned())]
    );
    assert_eq!(
        profile.socket_status_request(0),
        Err(LibraryRequestError::InvalidIdentifier {
            field: "section_id"
        })
    );
}
