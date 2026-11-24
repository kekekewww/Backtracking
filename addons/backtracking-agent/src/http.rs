//! The Streamable HTTP endpoint on 127.0.0.1: every request must name this server as its
//! `Host`, may come only from this server's own origin when it carries an `Origin` (the MCP
//! rule against DNS rebinding), and must carry the bearer token.

use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::sync::{Arc, RwLock};

use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use tokio_util::sync::CancellationToken;

use crate::bridge::Bridge;
use crate::tools::AgentServer;

/// The path the MCP endpoint is served at.
pub const PATH: &str = "/mcp";

/// What every request is checked against.
pub struct Guard {
    pub port: u16,
    pub token: Arc<RwLock<String>>,
}

/// Why a request was turned away, before it reached MCP.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    /// 403: a `Host` other than this server.
    Host,
    /// 403: an `Origin` that is not this server's own.
    Origin,
    /// 401: no bearer token, or another one.
    Token,
}

impl Guard {
    /// The hosts this server answers to.
    fn hosts(&self) -> [String; 2] {
        [
            format!("127.0.0.1:{}", self.port),
            format!("localhost:{}", self.port),
        ]
    }

    pub fn check(&self, headers: &HeaderMap) -> Result<(), Refusal> {
        let host = headers
            .get(header::HOST)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.trim().to_ascii_lowercase());
        if !host.is_some_and(|host| self.hosts().contains(&host)) {
            return Err(Refusal::Host);
        }
        if let Some(origin) = headers.get(header::ORIGIN) {
            let origin = origin
                .to_str()
                .map(|value| value.trim().to_ascii_lowercase());
            let allowed = self.hosts().map(|host| format!("http://{host}"));
            if !origin.is_ok_and(|origin| allowed.contains(&origin)) {
                return Err(Refusal::Origin);
            }
        }
        let presented = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| {
                let (scheme, token) = value.trim().split_once(' ')?;
                scheme.eq_ignore_ascii_case("bearer").then(|| token.trim())
            });
        let expected = self
            .token
            .read()
            .map(|token| token.clone())
            .unwrap_or_default();
        match presented {
            Some(token) if !expected.is_empty() && same(token.as_bytes(), expected.as_bytes()) => {
                Ok(())
            }
            _ => Err(Refusal::Token),
        }
    }
}

/// Compares without stopping at the first difference.
fn same(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn guard(State(guard): State<Arc<Guard>>, request: Request, next: Next) -> Response {
    match guard.check(request.headers()) {
        Ok(()) => next.run(request).await,
        Err(Refusal::Host) => (
            StatusCode::FORBIDDEN,
            "Forbidden: this server answers only to 127.0.0.1 and localhost on its own port.",
        )
            .into_response(),
        Err(Refusal::Origin) => (
            StatusCode::FORBIDDEN,
            "Forbidden: requests from web pages are not accepted.",
        )
            .into_response(),
        Err(Refusal::Token) => (
            StatusCode::UNAUTHORIZED,
            "Unauthorized: send Authorization: Bearer <token>. Backtracking's Add-ons page shows \
             the token under Backtracking Agent > Show connection details; clients started with \
             --stdio find it by themselves.",
        )
            .into_response(),
    }
}

/// A running HTTP server.
pub struct Server {
    pub port: u16,
    stop: CancellationToken,
}

impl Server {
    /// Binds 127.0.0.1:`port` (0 for any free port) now, so that a busy port is known at once,
    /// and serves MCP on it from `runtime`.
    pub fn start(
        runtime: &tokio::runtime::Runtime,
        port: u16,
        token: Arc<RwLock<String>>,
        bridge: Bridge,
    ) -> std::io::Result<Self> {
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let stop = CancellationToken::new();
        let guard = Arc::new(Guard { port, token });
        let hosts = guard.hosts();
        // rmcp checks the same Host and Origin rules again behind the guard.
        let config = StreamableHttpServerConfig::default()
            .with_allowed_hosts(hosts.clone())
            .with_allowed_origins(hosts.map(|host| format!("http://{host}")))
            .with_cancellation_token(stop.child_token());
        let service: StreamableHttpService<AgentServer, LocalSessionManager> =
            StreamableHttpService::new(
                move || Ok(AgentServer::new(bridge.clone())),
                Arc::new(LocalSessionManager::default()),
                config,
            );
        let app = axum::Router::new()
            .nest_service(PATH, service)
            .layer(middleware::from_fn_with_state(guard, self::guard));
        let _entered = runtime.enter();
        let listener = tokio::net::TcpListener::from_std(listener)?;
        let stopped = stop.clone();
        runtime.spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async move { stopped.cancelled_owned().await })
                .await;
        });
        Ok(Self { port, stop })
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}{PATH}", self.port)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn guard() -> Guard {
        Guard {
            port: 4000,
            token: Arc::new(RwLock::new("secret".into())),
        }
    }

    fn headers(pairs: &[(header::HeaderName, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(name.clone(), HeaderValue::from_str(value).unwrap());
        }
        map
    }

    #[test]
    fn only_this_server_with_the_token_and_no_foreign_origin_is_let_in() {
        let guard = guard();
        let ok = [
            (header::HOST, "127.0.0.1:4000"),
            (header::AUTHORIZATION, "Bearer secret"),
        ];
        assert_eq!(guard.check(&headers(&ok)), Ok(()));
        assert_eq!(
            guard.check(&headers(&[
                (header::HOST, "localhost:4000"),
                (header::AUTHORIZATION, "bearer  secret ")
            ])),
            Ok(())
        );
        // The server's own origin is fine; a browser page elsewhere is not.
        let own = [
            ok[0].clone(),
            ok[1].clone(),
            (header::ORIGIN, "http://127.0.0.1:4000"),
        ];
        assert_eq!(guard.check(&headers(&own)), Ok(()));
        for origin in [
            "https://example.com",
            "http://localhost:3000",
            "null",
            "http://127.0.0.1:4000.evil.com",
        ] {
            let foreign = [ok[0].clone(), ok[1].clone(), (header::ORIGIN, origin)];
            assert_eq!(
                guard.check(&headers(&foreign)),
                Err(Refusal::Origin),
                "{origin}"
            );
        }
        // DNS rebinding: a name that resolves here but is not this server.
        for host in [
            "evil.example:4000",
            "127.0.0.1:4001",
            "127.0.0.1",
            "0.0.0.0:4000",
        ] {
            let rebound = [(header::HOST, host), ok[1].clone()];
            assert_eq!(
                guard.check(&headers(&rebound)),
                Err(Refusal::Host),
                "{host}"
            );
        }
        assert_eq!(guard.check(&headers(&[ok[1].clone()])), Err(Refusal::Host));
        // Tokens.
        for bad in [
            "Bearer secreT",
            "Bearer",
            "Basic secret",
            "secret",
            "Bearer secret2",
        ] {
            let wrong = [ok[0].clone(), (header::AUTHORIZATION, bad)];
            assert_eq!(guard.check(&headers(&wrong)), Err(Refusal::Token), "{bad}");
        }
        assert_eq!(guard.check(&headers(&[ok[0].clone()])), Err(Refusal::Token));
    }

    #[test]
    fn a_new_token_replaces_the_old_one_at_once() {
        let guard = guard();
        *guard.token.write().unwrap() = "fresh".into();
        let old = [
            (header::HOST, "127.0.0.1:4000"),
            (header::AUTHORIZATION, "Bearer secret"),
        ];
        assert_eq!(guard.check(&headers(&old)), Err(Refusal::Token));
        let new = [
            (header::HOST, "127.0.0.1:4000"),
            (header::AUTHORIZATION, "Bearer fresh"),
        ];
        assert_eq!(guard.check(&headers(&new)), Ok(()));
        // An empty expected token lets nobody in.
        *guard.token.write().unwrap() = String::new();
        let empty = [
            (header::HOST, "127.0.0.1:4000"),
            (header::AUTHORIZATION, "Bearer "),
        ];
        assert_eq!(guard.check(&headers(&empty)), Err(Refusal::Token));
    }
}
