//! Synthetic loopback-only HTTP fixtures for reference-alignment tests.
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

pub(crate) struct Reply {
    pub status: u16,
    pub headers: String,
    /// The answer's bytes, not a string: one route under test answers an image,
    /// and a fixture must be able to serve exactly the bytes the service would
    /// rather than a lossy re-encoding of them.
    pub body: Vec<u8>,
}

pub(crate) fn captcha_png() -> Vec<u8> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNIM575HwAEZwIymUV1gwAAAABJRU5ErkJggg==")
        .unwrap()
}

impl Reply {
    pub fn json(body: &str) -> Self {
        Self {
            status: 200,
            headers: "Content-Type: application/json\r\n".into(),
            body: body.as_bytes().to_vec(),
        }
    }

    pub fn html(body: &str) -> Self {
        Self {
            status: 200,
            headers: "Content-Type: text/html; charset=utf-8\r\n".into(),
            body: body.as_bytes().to_vec(),
        }
    }

    /// One answer of arbitrary bytes and an explicit content type.
    pub fn bytes(status: u16, content_type: &str, body: Vec<u8>) -> Self {
        Self {
            status,
            headers: format!("Content-Type: {content_type}\r\n"),
            body,
        }
    }
}

pub(crate) struct FixtureServer {
    base: String,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl FixtureServer {
    pub fn new(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        Self::from_listener(listener, replies)
    }

    pub(crate) fn from_listener(listener: TcpListener, replies: Vec<Reply>) -> Self {
        let base = format!("http://{}/", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let observed = requests.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let done = stop.clone();
        let worker = thread::spawn(move || {
            let mut replies = replies.into_iter();
            while !done.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        // On BSD-derived platforms an accepted socket inherits
                        // the listener's non-blocking flag, which would make the
                        // read below fail with `WouldBlock` before the client's
                        // bytes arrive and answer a half-written request. Clear
                        // it so the read timeout is what bounds the read.
                        stream.set_nonblocking(false).unwrap();
                        stream
                            .set_read_timeout(Some(Duration::from_secs(1)))
                            .unwrap();
                        observed.lock().unwrap().push(read_request(&mut stream));
                        let reply = replies.next().unwrap_or(Reply {
                            status: 500,
                            headers: String::new(),
                            body: b"unexpected fixture request".to_vec(),
                        });
                        let _ = stream.write_all(
                            format!(
                                "HTTP/1.1 {} Fixture\r\n{}Content-Length: {}\r\nConnection: close\r\n\r\n",
                                reply.status,
                                reply.headers,
                                reply.body.len()
                            )
                            .as_bytes(),
                        );
                        let _ = stream.write_all(&reply.body);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => panic!("loopback fixture accept: {error}"),
                }
            }
        });
        Self {
            base,
            requests,
            stop,
            worker: Some(worker),
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }
    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn read_request(stream: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(n) => bytes.extend_from_slice(&buffer[..n]),
        }
        if let Some(pos) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&bytes[..pos]);
            let length = headers
                .lines()
                .find_map(|line| {
                    line.split_once(':')
                        .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            if bytes.len() >= pos + 4 + length {
                break;
            }
        }
        assert!(
            bytes.len() < 128 * 1024,
            "fixture request unexpectedly large"
        );
    }
    String::from_utf8(bytes).unwrap()
}
