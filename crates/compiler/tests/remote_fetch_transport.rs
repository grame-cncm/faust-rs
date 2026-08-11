//! Native `ureq` transport tests. All requests stay on loopback.

#![cfg(feature = "network-imports")]

mod support;

use std::sync::Arc;

use compiler::{
    Compiler,
    remote_fetch::{AllowAllRemoteUrls, RemoteUrlPolicy, UreqSourceFetcher},
};
use parser::{
    RemoteFetchPolicy, RemoteFetchRequest, RemoteSourceFetcher, SourceFetchErrorKind, SourceLocator,
};
use support::http_server::{FixtureResponse, HttpFixtureServer};
use url::Url;

fn request(url: &str, policy: RemoteFetchPolicy) -> RemoteFetchRequest {
    RemoteFetchRequest {
        url: Url::parse(url).unwrap(),
        policy,
    }
}

#[test]
fn fetches_utf8_source_and_follows_relative_redirect() {
    let server = HttpFixtureServer::start([
        (
            "/redirect.dsp".to_owned(),
            FixtureResponse::redirect("/main.dsp"),
        ),
        (
            "/main.dsp".to_owned(),
            FixtureResponse::text("process = _;\n"),
        ),
    ]);
    let fetcher = UreqSourceFetcher::new(Arc::new(AllowAllRemoteUrls));
    let fetched = fetcher
        .fetch(&request(
            &server.url("/redirect.dsp"),
            RemoteFetchPolicy::default(),
        ))
        .unwrap();
    assert_eq!(fetched.clone().into_utf8().unwrap(), "process = _;\n");
    assert_eq!(fetched.final_url.as_str(), server.url("/main.dsp"));
    assert_eq!(server.requests(), ["/redirect.dsp", "/main.dsp"]);
}

#[test]
fn maps_status_size_and_utf8_failures_to_stable_categories() {
    let server = HttpFixtureServer::start([
        (
            "/large.dsp".to_owned(),
            FixtureResponse::bytes(200, b"12345".to_vec()),
        ),
        (
            "/invalid.dsp".to_owned(),
            FixtureResponse::bytes(200, vec![0xff, 0xfe]),
        ),
    ]);
    let fetcher = UreqSourceFetcher::new(Arc::new(AllowAllRemoteUrls));

    let status = fetcher
        .fetch(&request(
            &server.url("/missing.dsp"),
            RemoteFetchPolicy::default(),
        ))
        .unwrap_err();
    assert_eq!(status.kind, SourceFetchErrorKind::HttpStatus);

    let small_policy = RemoteFetchPolicy {
        max_response_bytes: 4,
        ..RemoteFetchPolicy::default()
    };
    let large = fetcher
        .fetch(&request(&server.url("/large.dsp"), small_policy))
        .unwrap_err();
    assert_eq!(large.kind, SourceFetchErrorKind::ResponseTooLarge);

    let invalid = fetcher
        .fetch(&request(
            &server.url("/invalid.dsp"),
            RemoteFetchPolicy::default(),
        ))
        .unwrap()
        .into_utf8()
        .unwrap_err();
    assert_eq!(invalid.kind, SourceFetchErrorKind::InvalidUtf8);
}

#[derive(Debug)]
struct RejectBlockedPath;

impl RemoteUrlPolicy for RejectBlockedPath {
    fn authorize(&self, url: &Url) -> Result<(), Box<str>> {
        if url.path() == "/blocked.dsp" {
            Err("blocked by test host policy".into())
        } else {
            Ok(())
        }
    }
}

#[test]
fn reauthorizes_every_redirect_target() {
    let server = HttpFixtureServer::start([(
        "/redirect.dsp".to_owned(),
        FixtureResponse::redirect("/blocked.dsp"),
    )]);
    let fetcher = UreqSourceFetcher::new(Arc::new(RejectBlockedPath));
    let error = fetcher
        .fetch(&request(
            &server.url("/redirect.dsp"),
            RemoteFetchPolicy::default(),
        ))
        .unwrap_err();
    assert_eq!(error.kind, SourceFetchErrorKind::PolicyRejected);
    assert_eq!(server.requests(), ["/redirect.dsp"]);
}

#[test]
fn enforces_the_session_redirect_ceiling() {
    let server = HttpFixtureServer::start([
        ("/r0".to_owned(), FixtureResponse::redirect("/r1")),
        ("/r1".to_owned(), FixtureResponse::redirect("/r2")),
        ("/r2".to_owned(), FixtureResponse::text("process = _;\n")),
    ]);
    let fetcher = UreqSourceFetcher::new(Arc::new(AllowAllRemoteUrls));
    let policy = RemoteFetchPolicy {
        max_redirects: 1,
        ..RemoteFetchPolicy::default()
    };
    let error = fetcher
        .fetch(&request(&server.url("/r0"), policy))
        .unwrap_err();
    assert_eq!(error.kind, SourceFetchErrorKind::Redirect);
    assert_eq!(server.requests(), ["/r0", "/r1"]);
}

#[test]
fn rejects_user_info_before_network_io() {
    let fetcher = UreqSourceFetcher::new(Arc::new(AllowAllRemoteUrls));
    let locator = SourceLocator::remote("http://user:secret@example.com/main.dsp", None).unwrap();
    let error = fetcher
        .fetch(&RemoteFetchRequest {
            url: locator.as_url().unwrap().clone(),
            policy: RemoteFetchPolicy::default(),
        })
        .unwrap_err();
    assert_eq!(error.kind, SourceFetchErrorKind::PolicyRejected);
    assert!(!error.url.as_str().contains("secret"));
}

#[test]
fn compiler_requires_runtime_opt_in_for_remote_entries() {
    let error = Compiler::new()
        .compile_file_default(std::path::Path::new("https://example.test/main.dsp"))
        .expect_err("remote entry must remain disabled by default");
    assert_eq!(
        error.diagnostic_bundle().as_slice()[0].code,
        diagnostics::codes::SRC_NETWORK_DISABLED
    );
}

#[test]
fn compiler_expands_a_remote_relative_import_graph() {
    let server = HttpFixtureServer::start([
        (
            "/main.dsp".to_owned(),
            FixtureResponse::text("import(\"lib/identity.lib\");\nprocess = identity;\n"),
        ),
        (
            "/lib/identity.lib".to_owned(),
            FixtureResponse::text("identity = _;\n"),
        ),
    ]);
    let url = server.url("/main.dsp");
    let output = Compiler::new()
        .with_native_network_imports()
        .compile_file_default_to_signals(std::path::Path::new(&url))
        .expect("remote graph should compile through propagation");

    assert_eq!(output.process_arity.inputs, 1);
    assert_eq!(output.process_arity.outputs, 1);
    assert_eq!(server.requests(), ["/main.dsp", "/lib/identity.lib"]);
    assert_eq!(output.parse.used_sources.len(), 2);
}
