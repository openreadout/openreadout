//! Streamable HTTP transport (feature `http`): rmcp's `StreamableHttpService` served by a minimal
//! hyper HTTP/1.1 server at `/mcp`.
//!
//! Security model (see `book/src/reference/mcp.md`): the server binds loopback addresses unless the caller opts
//! in to others; the `Host` header must name a loopback host (or the bound address), which defeats
//! DNS rebinding; requests carrying a browser `Origin` header are rejected, so web pages cannot
//! drive the server; and when a token is configured every request must carry
//! `Authorization: Bearer <token>`. The tools can read any file the user running the server can
//! read and write new files next to them, so anything that can reach the port has that power.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::{Request, Response, StatusCode};
use openreadout_core::Registry;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};

use crate::InstrumentServer;

/// Options of [`serve_http`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct HttpOptions {
    /// Address and port to listen on.
    pub addr: SocketAddr,
    /// Permit a non-loopback bind address.
    pub allow_remote: bool,
    /// Required bearer token, if any.
    pub token: Option<String>,
}

impl HttpOptions {
    /// Listen on `addr` (which must be a loopback address unless `allow_remote` is set
    /// afterwards), with no bearer token.
    pub fn new(addr: SocketAddr) -> Self {
        HttpOptions {
            addr,
            allow_remote: false,
            token: None,
        }
    }
}

type Body = http_body_util::combinators::BoxBody<Bytes, Infallible>;

fn plain(status: StatusCode, msg: &'static str) -> Response<Body> {
    let mut r = Response::new(Full::new(Bytes::from_static(msg.as_bytes())).boxed());
    *r.status_mut() = status;
    r
}

/// Constant-time comparison of two byte strings.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Serve MCP over Streamable HTTP until the process is stopped.
pub fn serve_http(registry: fn() -> Registry, opts: &HttpOptions) -> anyhow::Result<()> {
    if !opts.addr.ip().is_loopback() && !opts.allow_remote {
        anyhow::bail!(
            "{} is not a loopback address; pass allow_remote to bind it",
            opts.addr
        );
    }
    let mut hosts = vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "::1".to_string(),
        "[::1]".to_string(),
    ];
    if !opts.addr.ip().is_loopback() {
        hosts.push(opts.addr.ip().to_string());
    }
    let config = StreamableHttpServerConfig::default()
        .with_allowed_hosts(hosts)
        .enforce_origin_validation();
    let service = StreamableHttpService::new(
        move || Ok(InstrumentServer::new(registry)),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    let token = opts.token.clone().map(|t| format!("Bearer {t}"));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        let listener = tokio::net::TcpListener::bind(opts.addr).await?;
        eprintln!(
            "openreadout MCP server (Streamable HTTP) listening on http://{}/mcp{}",
            listener.local_addr()?,
            if token.is_some() {
                " (bearer token required)"
            } else {
                ""
            }
        );
        loop {
            let (stream, _) = listener.accept().await?;
            let service = service.clone();
            let token = token.clone();
            tokio::spawn(async move {
                let handler = hyper::service::service_fn(move |req: Request<Incoming>| {
                    let service = service.clone();
                    let token = token.clone();
                    async move {
                        if req.uri().path() != "/mcp" {
                            return Ok::<_, Infallible>(plain(
                                StatusCode::NOT_FOUND,
                                "not found: the MCP endpoint is /mcp\n",
                            ));
                        }
                        if let Some(t) = &token {
                            let ok = req
                                .headers()
                                .get(hyper::header::AUTHORIZATION)
                                .is_some_and(|v| same(v.as_bytes(), t.as_bytes()));
                            if !ok {
                                return Ok(plain(
                                    StatusCode::UNAUTHORIZED,
                                    "unauthorized: send Authorization: Bearer <OPENREADOUT_MCP_TOKEN>\n",
                                ));
                            }
                        }
                        Ok(service.handle(req).await)
                    }
                });
                let io = hyper_util::rt::TokioIo::new(stream);
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(io, handler)
                    .await;
            });
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_compare() {
        assert!(same(b"Bearer x", b"Bearer x"));
        assert!(!same(b"Bearer x", b"Bearer y"));
        assert!(!same(b"Bearer", b"Bearer x"));
    }

    #[test]
    fn refuses_remote_without_opt_in() {
        let e = serve_http(
            Registry::new,
            &HttpOptions {
                addr: "0.0.0.0:0".parse().unwrap(),
                allow_remote: false,
                token: None,
            },
        )
        .unwrap_err();
        assert!(e.to_string().contains("loopback"));
    }
}
