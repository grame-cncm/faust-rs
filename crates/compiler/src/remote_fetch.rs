//! Native HTTP(S) implementation of the parser's remote-source capability.
//!
//! # Source provenance and adaptation
//!
//! The observable defaults come from C++
//! `compiler/parser/sourcefetcher.hh/.cpp`: blocking GET, five-second
//! inactivity timeout, and three redirects. The implementation is deliberately
//! adapted: `ureq` supplies real TLS and modern HTTP parsing, response bodies
//! are bounded, redirect targets are re-authorized, and all state belongs to a
//! compiler host rather than process-global setters.
//!
//! This module exists only for native builds with the `network-imports`
//! feature. Parser-core depends solely on `RemoteSourceFetcher`, so another host
//! can inject a custom transport without depending on `ureq`.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use parser::{
    FetchedSource, RemoteFetchPolicy, RemoteFetchRequest, RemoteSourceFetcher, SourceFetchError,
    SourceFetchErrorKind, SourceLocator,
};
use url::Url;

/// Host-supplied authorization policy for initial and redirected URLs.
pub trait RemoteUrlPolicy: fmt::Debug + Send + Sync {
    /// Returns `Ok(())` when `url` may be requested.
    fn authorize(&self, url: &Url) -> Result<(), Box<str>>;
}

/// Explicit policy for a trusted native CLI invocation that permits every
/// syntactically valid HTTP(S) host.
#[derive(Clone, Copy, Debug, Default)]
pub struct AllowAllRemoteUrls;

impl RemoteUrlPolicy for AllowAllRemoteUrls {
    fn authorize(&self, _url: &Url) -> Result<(), Box<str>> {
        Ok(())
    }
}

/// Blocking native HTTP(S) fetcher backed by `ureq`.
///
/// Agents are cached by immutable [`RemoteFetchPolicy`] so a compilation that
/// loads several libraries can reuse connections without leaking cache state
/// across differently constrained requests.
pub struct UreqSourceFetcher {
    url_policy: Arc<dyn RemoteUrlPolicy>,
    agents: Mutex<HashMap<RemoteFetchPolicy, ureq::Agent>>,
}

impl fmt::Debug for UreqSourceFetcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UreqSourceFetcher")
            .field("url_policy", &self.url_policy)
            .finish_non_exhaustive()
    }
}

impl UreqSourceFetcher {
    /// Creates a fetcher with an explicit host authorization policy.
    #[must_use]
    pub fn new(url_policy: Arc<dyn RemoteUrlPolicy>) -> Self {
        Self {
            url_policy,
            agents: Mutex::new(HashMap::new()),
        }
    }

    fn agent(&self, policy: RemoteFetchPolicy) -> ureq::Agent {
        let mut agents = self
            .agents
            .lock()
            .expect("remote fetch agent cache poisoned");
        agents
            .entry(policy)
            .or_insert_with(|| {
                let config = ureq::Agent::config_builder()
                    .http_status_as_error(false)
                    // Redirects are followed manually so every target is
                    // checked by RemoteUrlPolicy.
                    .max_redirects(0)
                    .proxy(None)
                    .timeout_global(Some(policy.total_timeout))
                    .timeout_per_call(Some(policy.inactivity_timeout))
                    .timeout_connect(Some(policy.inactivity_timeout))
                    .user_agent(format!("faust-rs/{}", env!("CARGO_PKG_VERSION")))
                    .build();
                ureq::Agent::new_with_config(config)
            })
            .clone()
    }

    fn authorize(&self, url: &Url) -> Result<(), SourceFetchError> {
        if !url.username().is_empty() || url.password().is_some() {
            return Err(fetch_error(
                SourceFetchErrorKind::PolicyRejected,
                url,
                "URL user-info credentials are not allowed",
            ));
        }
        self.url_policy
            .authorize(url)
            .map_err(|message| fetch_error(SourceFetchErrorKind::PolicyRejected, url, message))
    }
}

impl RemoteSourceFetcher for UreqSourceFetcher {
    fn fetch(&self, request: &RemoteFetchRequest) -> Result<FetchedSource, SourceFetchError> {
        let requested = request.url.clone();
        let mut current = requested.clone();
        let agent = self.agent(request.policy);
        let mut redirects = 0u32;

        loop {
            self.authorize(&current)?;
            let mut response = agent
                .get(current.as_str())
                .call()
                .map_err(|error| map_ureq_error(&current, error))?;
            let status = response.status().as_u16();

            if (300..400).contains(&status) {
                if redirects >= request.policy.max_redirects {
                    return Err(fetch_error(
                        SourceFetchErrorKind::Redirect,
                        &current,
                        format!("redirect limit exceeded ({})", request.policy.max_redirects),
                    ));
                }
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|value| value.to_str().ok())
                    .ok_or_else(|| {
                        fetch_error(
                            SourceFetchErrorKind::Redirect,
                            &current,
                            format!("HTTP {status} response has no valid Location header"),
                        )
                    })?;
                let next = current.join(location).map_err(|error| {
                    fetch_error(
                        SourceFetchErrorKind::Redirect,
                        &current,
                        format!("invalid redirect target: {error}"),
                    )
                })?;
                current = match SourceLocator::from_remote_url(next) {
                    Ok(SourceLocator::Url(url)) => url,
                    Ok(SourceLocator::File(_) | SourceLocator::Virtual(_)) => unreachable!(),
                    Err(error) => {
                        return Err(fetch_error(
                            SourceFetchErrorKind::Redirect,
                            &current,
                            error.to_string(),
                        ));
                    }
                };
                redirects += 1;
                continue;
            }

            if !(200..300).contains(&status) {
                return Err(fetch_error(
                    SourceFetchErrorKind::HttpStatus,
                    &current,
                    format!("HTTP status {status}"),
                ));
            }

            let limit = u64::try_from(request.policy.max_response_bytes).unwrap_or(u64::MAX);
            let bytes = response
                .body_mut()
                .with_config()
                .limit(limit)
                .read_to_vec()
                .map_err(|error| map_ureq_error(&current, error))?;
            return Ok(FetchedSource {
                requested_url: requested,
                final_url: current,
                bytes,
            });
        }
    }
}

fn map_ureq_error(url: &Url, error: ureq::Error) -> SourceFetchError {
    let kind = match error {
        ureq::Error::HostNotFound => SourceFetchErrorKind::Dns,
        ureq::Error::ConnectionFailed => SourceFetchErrorKind::Connect,
        ureq::Error::Timeout(_) => SourceFetchErrorKind::Timeout,
        ureq::Error::Tls(_) | ureq::Error::Rustls(_) | ureq::Error::Pem(_) => {
            SourceFetchErrorKind::Tls
        }
        ureq::Error::StatusCode(_) => SourceFetchErrorKind::HttpStatus,
        ureq::Error::RedirectFailed | ureq::Error::TooManyRedirects => {
            SourceFetchErrorKind::Redirect
        }
        ureq::Error::BodyExceedsLimit(_) => SourceFetchErrorKind::ResponseTooLarge,
        _ => SourceFetchErrorKind::Transport,
    };
    fetch_error(kind, url, error.to_string())
}

fn fetch_error(
    kind: SourceFetchErrorKind,
    url: &Url,
    message: impl Into<Box<str>>,
) -> SourceFetchError {
    SourceFetchError {
        kind,
        url: sanitized_url(url),
        message: message.into(),
    }
}

fn sanitized_url(url: &Url) -> Url {
    let mut sanitized = url.clone();
    let _ = sanitized.set_username("");
    let _ = sanitized.set_password(None);
    sanitized
}
