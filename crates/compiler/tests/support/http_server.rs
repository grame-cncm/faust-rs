//! Hermetic HTTP/1.0 server for remote-source integration tests.
//!
//! The production transport must never make public-Internet requests in tests.
//! This fixture binds an ephemeral loopback port, serves an immutable route
//! table, records requested targets, and shuts down when dropped.

#![allow(dead_code)]

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};

/// One immutable response served by [`HttpFixtureServer`].
#[derive(Clone, Debug)]
pub struct FixtureResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl FixtureResponse {
    /// Builds a successful UTF-8 response.
    #[must_use]
    pub fn text(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            headers: vec![(
                "Content-Type".to_owned(),
                "text/plain; charset=utf-8".to_owned(),
            )],
            body: body.into().into_bytes(),
        }
    }

    /// Builds a response with an arbitrary body and status.
    #[must_use]
    pub fn bytes(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.into(),
        }
    }

    /// Builds an HTTP redirect.
    #[must_use]
    pub fn redirect(location: impl Into<String>) -> Self {
        Self {
            status: 302,
            headers: vec![("Location".to_owned(), location.into())],
            body: Vec::new(),
        }
    }
}

/// Loopback HTTP server with deterministic route responses.
pub struct HttpFixtureServer {
    address: SocketAddr,
    requests: Arc<Mutex<Vec<String>>>,
    stopping: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl HttpFixtureServer {
    /// Starts a server on an operating-system assigned loopback port.
    pub fn start(routes: impl IntoIterator<Item = (String, FixtureResponse)>) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind fixture HTTP server");
        let address = listener.local_addr().expect("read fixture server address");
        let routes = Arc::new(routes.into_iter().collect::<HashMap<_, _>>());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stopping = Arc::new(AtomicBool::new(false));
        let thread_routes = Arc::clone(&routes);
        let thread_requests = Arc::clone(&requests);
        let thread_stopping = Arc::clone(&stopping);
        let thread = thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else {
                    break;
                };
                if thread_stopping.load(Ordering::Acquire) {
                    break;
                }
                serve_one(&mut stream, &thread_routes, &thread_requests);
            }
        });
        Self {
            address,
            requests,
            stopping,
            thread: Some(thread),
        }
    }

    /// Returns the server origin without a trailing slash.
    #[must_use]
    pub fn origin(&self) -> String {
        format!("http://{}", self.address)
    }

    /// Returns one absolute URL for `target`.
    #[must_use]
    pub fn url(&self, target: &str) -> String {
        format!("{}{}", self.origin(), target)
    }

    /// Returns requested path/query targets in arrival order.
    #[must_use]
    pub fn requests(&self) -> Vec<String> {
        self.requests
            .lock()
            .expect("fixture request log poisoned")
            .clone()
    }
}

impl Drop for HttpFixtureServer {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        let _ = TcpStream::connect(self.address);
        if let Some(thread) = self.thread.take() {
            thread.join().expect("join fixture HTTP server");
        }
    }
}

fn serve_one(
    stream: &mut TcpStream,
    routes: &HashMap<String, FixtureResponse>,
    requests: &Mutex<Vec<String>>,
) {
    let mut request = Vec::new();
    let mut chunk = [0u8; 1024];
    while request.len() < 16 * 1024 {
        let Ok(read) = stream.read(&mut chunk) else {
            return;
        };
        if read == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..read]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let first_line = request
        .split(|byte| *byte == b'\n')
        .next()
        .unwrap_or_default();
    let target = String::from_utf8_lossy(first_line)
        .split_ascii_whitespace()
        .nth(1)
        .unwrap_or("/")
        .to_owned();
    requests
        .lock()
        .expect("fixture request log poisoned")
        .push(target.clone());

    let response = routes
        .get(&target)
        .cloned()
        .unwrap_or_else(|| FixtureResponse::bytes(404, b"not found".to_vec()));
    let reason = match response.status {
        200 => "OK",
        302 => "Found",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "Fixture Status",
    };
    let mut head = format!(
        "HTTP/1.0 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        response.status,
        reason,
        response.body.len()
    );
    for (name, value) in response.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&response.body);
}
