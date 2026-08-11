//! Phase-0 fixtures for the remote-source implementation.
//!
//! The pinned C++ fetcher hard-codes port 80, so it cannot consume this
//! ephemeral-port fixture. The fixture instead freezes the HTTP contract used
//! by every Rust implementation phase without depending on public networking.

mod support;

use std::io::{Read, Write};
use std::net::TcpStream;

use support::http_server::{FixtureResponse, HttpFixtureServer};

#[test]
fn hermetic_server_freezes_remote_import_routes() {
    let server = HttpFixtureServer::start([
        (
            "/main.dsp".to_owned(),
            FixtureResponse::text("import(\"nested/child.lib\");\nprocess = child;\n"),
        ),
        (
            "/nested/child.lib".to_owned(),
            FixtureResponse::text("child = _;\n"),
        ),
        (
            "/redirect.dsp".to_owned(),
            FixtureResponse::redirect("/main.dsp"),
        ),
    ]);

    let main = raw_get(&server, "/main.dsp");
    assert!(main.starts_with("HTTP/1.0 200 OK\r\n"));
    assert!(main.ends_with("import(\"nested/child.lib\");\nprocess = child;\n"));

    let redirect = raw_get(&server, "/redirect.dsp");
    assert!(redirect.starts_with("HTTP/1.0 302 Found\r\n"));
    assert!(redirect.contains("Location: /main.dsp\r\n"));
    assert_eq!(server.requests(), ["/main.dsp", "/redirect.dsp"]);
}

fn raw_get(server: &HttpFixtureServer, target: &str) -> String {
    let mut stream = TcpStream::connect(server.origin().trim_start_matches("http://"))
        .expect("connect to fixture HTTP server");
    write!(stream, "GET {target} HTTP/1.0\r\nHost: fixture\r\n\r\n")
        .expect("write fixture request");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("read fixture response");
    response
}
