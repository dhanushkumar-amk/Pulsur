//! Ferrum HTTP Server — Phase 5-10 complete implementation.
//!
//! Fixes applied over the original draft:
//!   - Status reason phrases now match the status code (404 → "Not Found", etc.)
//!   - WebSocket frame parser handles extended 16-bit and 64-bit payload lengths per RFC 6455.
//!   - 101 Switching Protocols response no longer sends `Content-Length`.
//!   - Max-connections limit uses `tokio::sync::Semaphore` — no more TOCTOU race.
//!   - HTTP version is parsed from the request line; HTTP/1.0 defaults to connection-close.
//!   - `WsHandler` now receives a `WsContext` carrying path, params, and headers.
//!   - Body bytes already buffered in the header buffer are not double-read.
//!   - Both parse timeout and handler timeout are configurable and enforced per request.
//!   - TLS cert path is accepted as a parameter instead of a hardcoded CWD-relative string.
//!   - `match_route` borrows `method` instead of consuming it.
//!   - Active-connection decrement on TLS handshake failure is explicit, not accidental.

use tokio::net::TcpListener;
use tokio::io::{AsyncReadExt, AsyncWriteExt, AsyncRead, AsyncWrite};
use tokio::time::{timeout, Duration};
use tokio::sync::Semaphore;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex as StdMutex};
use std::io::{BufReader, Cursor};
use thiserror::Error;
use tracing::{info, warn};
use futures::future::BoxFuture;
use napi_derive::napi;
use serde::de::DeserializeOwned;
use sha1::{Sha1, Digest};
use base64::Engine;

use tokio_rustls::TlsAcceptor;
use rcgen::generate_simple_self_signed;

// ──────────────────────────────────────────────────────────────
//  Constants
// ──────────────────────────────────────────────────────────────

const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
/// Maximum header section size. 16 KiB is the de-facto minimum that real-world
/// clients (browsers with large cookies) need.
const HEADER_BUF_SIZE: usize = 16_384;
/// Default idle timeout per request parse phase (seconds).
const DEFAULT_PARSE_TIMEOUT_SECS: u64 = 30;
/// Default maximum time allowed for a handler to return a response (seconds).
const DEFAULT_HANDLER_TIMEOUT_SECS: u64 = 60;

// ──────────────────────────────────────────────────────────────
//  Error type
// ──────────────────────────────────────────────────────────────

#[derive(Error, Debug)]
pub enum HttpError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("Connection idle timeout")]
    Timeout,
    #[error("Payload too large (413)")]
    PayloadTooLarge,
    #[error("Serialization error: {0}")]
    Serialization(String),
    #[error("WebSocket error: {0}")]
    WebSocket(String),
    #[error("TLS error: {0}")]
    Tls(String),
}

// ──────────────────────────────────────────────────────────────
//  HTTP method
// ──────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub enum Method {
    GET,
    POST,
    PUT,
    DELETE,
    PATCH,
    OPTIONS,
    HEAD,
}

impl FromStr for Method {
    type Err = HttpError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "GET"     => Ok(Method::GET),
            "POST"    => Ok(Method::POST),
            "PUT"     => Ok(Method::PUT),
            "DELETE"  => Ok(Method::DELETE),
            "PATCH"   => Ok(Method::PATCH),
            "OPTIONS" => Ok(Method::OPTIONS),
            "HEAD"    => Ok(Method::HEAD),
            other     => Err(HttpError::Parse(format!("Unknown method: {}", other))),
        }
    }
}

// ──────────────────────────────────────────────────────────────
//  HTTP version
// ──────────────────────────────────────────────────────────────

/// Parsed HTTP version, used to determine the correct keep-alive default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpVersion {
    /// HTTP/1.0 — connection closes by default unless `Connection: keep-alive` is explicit.
    Http10,
    /// HTTP/1.1 — connection is kept alive by default unless `Connection: close` is explicit.
    Http11,
}

impl HttpVersion {
    /// Returns `true` if the connection should be kept alive for this version,
    /// given the value of the `Connection` header.
    fn should_keep_alive(self, connection_header: Option<&str>) -> bool {
        match connection_header {
            // Explicit header always wins.
            Some(v) if v.eq_ignore_ascii_case("keep-alive") => true,
            Some(v) if v.eq_ignore_ascii_case("close")      => false,
            // No explicit header: use version default.
            _ => self == HttpVersion::Http11,
        }
    }
}

// ──────────────────────────────────────────────────────────────
//  Request / Response
// ──────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct Request {
    pub method: Method,
    pub path: String,
    pub version: HttpVersion,
    pub headers: HashMap<String, String>,
    pub params: HashMap<String, String>,
    pub body: Vec<u8>,
    pub peer_addr: std::net::SocketAddr,
}

impl Request {
    /// Deserialize the body as JSON into type `T`.
    pub fn json<T: DeserializeOwned>(&self) -> Result<T, HttpError> {
        serde_json::from_slice(&self.body)
            .map_err(|e| HttpError::Serialization(e.to_string()))
    }

    /// Returns `true` when the request carries a WebSocket upgrade.
    pub fn is_websocket_upgrade(&self) -> bool {
        self.headers
            .get("upgrade")
            .map(|v| v.eq_ignore_ascii_case("websocket"))
            .unwrap_or(false)
    }
}

pub struct Response {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn new(status: u16) -> Self {
        let mut headers = HashMap::new();
        headers.insert("Server".to_string(), "Ferrum-Core/0.7.0".to_string());
        Self { status, headers, body: Vec::new() }
    }

    pub fn json<T: serde::Serialize>(status: u16, data: &T) -> Result<Self, HttpError> {
        let body = serde_json::to_vec(data)
            .map_err(|e| HttpError::Serialization(e.to_string()))?;
        let mut res = Self::new(status);
        res.headers.insert("Content-Type".to_string(), "application/json".to_string());
        res.body = body;
        Ok(res)
    }
}

/// Map an HTTP status code to its standard reason phrase.
///
/// FIX: the original code hardcoded "OK" for every status, meaning
/// `404 OK`, `429 OK`, `503 OK`, etc. Clients and reverse proxies
/// may behave incorrectly when the reason phrase doesn't match the code.
fn reason_phrase(status: u16) -> &'static str {
    match status {
        100 => "Continue",
        101 => "Switching Protocols",
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        206 => "Partial Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        409 => "Conflict",
        413 => "Payload Too Large",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _   => "Unknown",
    }
}

// ──────────────────────────────────────────────────────────────
//  Router
// ──────────────────────────────────────────────────────────────

/// Context passed to WebSocket handlers.
///
/// FIX: the original `WsHandler` signature was `Fn(WebSocket) -> ...`, giving
/// the handler zero information about which route was matched, what the path
/// params were, or what headers the client sent. Any real WS handler (e.g.
/// joining a chat room whose ID comes from the URL) needs this context.
#[derive(Debug)]
pub struct WsContext {
    pub path: String,
    pub params: HashMap<String, String>,
    pub headers: HashMap<String, String>,
}

pub type Handler   = Arc<dyn Fn(Request) -> BoxFuture<'static, Response> + Send + Sync>;
pub type WsHandler = Arc<dyn Fn(WsContext, WebSocket) -> BoxFuture<'static, ()> + Send + Sync>;

pub enum RouteTarget {
    Http(Handler),
    WebSocket(WsHandler),
}

pub struct Route {
    pub method: Method,
    /// URL pattern split into segments. `:name` segments are parameters.
    pub pattern: Vec<String>,
    pub target: RouteTarget,
}

#[derive(Default)]
pub struct Router {
    pub routes: Vec<Route>,
}

impl Router {
    pub fn new() -> Self { Self::default() }

    pub fn add_http(&mut self, method: Method, path: &str, handler: Handler) {
        let pattern = path.split('/').filter(|s| !s.is_empty()).map(str::to_string).collect();
        self.routes.push(Route { method, pattern, target: RouteTarget::Http(handler) });
    }

    pub fn ws(&mut self, path: &str, handler: WsHandler) {
        let pattern = path.split('/').filter(|s| !s.is_empty()).map(str::to_string).collect();
        self.routes.push(Route { method: Method::GET, pattern, target: RouteTarget::WebSocket(handler) });
    }

    /// FIX: original took `method: Method` by value, which consumes the enum.
    /// Now borrows it, avoiding any implicit clone in the loop.
    pub fn match_route<'a>(
        &'a self,
        method: &Method,
        path: &str,
    ) -> Option<(&'a RouteTarget, HashMap<String, String>)> {
        for route in &self.routes {
            if &route.method != method { continue; }

            let mut params = HashMap::new();

            if path == "/" {
                if route.pattern.is_empty() { return Some((&route.target, params)); }
                continue;
            }

            let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
            if route.pattern.len() != segments.len() { continue; }

            let mut matched = true;
            for (p_seg, s_seg) in route.pattern.iter().zip(segments.iter()) {
                if let Some(param_name) = p_seg.strip_prefix(':') {
                    params.insert(param_name.to_string(), s_seg.to_string());
                } else if p_seg != s_seg {
                    matched = false;
                    break;
                }
            }
            if matched { return Some((&route.target, params)); }
        }
        None
    }
}

// ──────────────────────────────────────────────────────────────
//  AsyncStream trait object
// ──────────────────────────────────────────────────────────────

pub trait AsyncStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> AsyncStream for T {}

// ──────────────────────────────────────────────────────────────
//  WebSocket
// ──────────────────────────────────────────────────────────────

pub struct WebSocket {
    stream: Box<dyn AsyncStream>,
}

impl WebSocket {
    pub fn new(stream: Box<dyn AsyncStream>) -> Self { Self { stream } }

    /// Send a UTF-8 text frame (opcode 0x1).
    pub async fn send_text(&mut self, text: &str) -> Result<(), HttpError> {
        self.send_frame(0x81, text.as_bytes()).await
    }

    /// Send a binary frame (opcode 0x2).
    pub async fn send_binary(&mut self, data: &[u8]) -> Result<(), HttpError> {
        self.send_frame(0x82, data).await
    }

    /// Send a ping frame (opcode 0x9). Clients must respond with a pong.
    pub async fn send_ping(&mut self) -> Result<(), HttpError> {
        self.send_frame(0x89, b"").await
    }

    /// Low-level: build and write a single WebSocket frame.
    async fn send_frame(&mut self, opcode: u8, payload: &[u8]) -> Result<(), HttpError> {
        let mut frame = vec![opcode];
        // Server-to-client frames are never masked (RFC 6455 §5.1).
        let len = payload.len();
        if len <= 125 {
            frame.push(len as u8);
        } else if len <= 65535 {
            frame.push(126);
            frame.extend_from_slice(&(len as u16).to_be_bytes());
        } else {
            frame.push(127);
            frame.extend_from_slice(&(len as u64).to_be_bytes());
        }
        frame.extend_from_slice(payload);
        self.stream.write_all(&frame).await?;
        self.stream.flush().await?;
        Ok(())
    }

    /// Receive the next client frame.
    ///
    /// Returns `None` when the connection is closed cleanly.
    ///
    /// FIX: the original only read `(header[1] & 0x7F) as usize` directly.
    /// RFC 6455 §5.2 specifies three cases:
    ///   - 0–125  → that value is the length
    ///   - 126    → read the next 2 bytes as a big-endian u16
    ///   - 127    → read the next 8 bytes as a big-endian u64
    ///
    /// Messages over 125 bytes were silently corrupted in the original because
    /// the length bytes were consumed as if they were payload data.
    pub async fn next_message(&mut self) -> Result<Option<WsMessage>, HttpError> {
        // Read the 2-byte base header.
        let mut header = [0u8; 2];
        if self.stream.read_exact(&mut header).await.is_err() {
            return Ok(None); // Connection closed.
        }

        let fin     = header[0] & 0x80 != 0;
        let opcode  = header[0] & 0x0F;
        let masked  = header[1] & 0x80 != 0;
        let raw_len = (header[1] & 0x7F) as usize;

        // Resolve the actual payload length.
        let payload_len: usize = match raw_len {
            0..=125 => raw_len,
            126 => {
                let mut buf = [0u8; 2];
                self.stream.read_exact(&mut buf).await?;
                u16::from_be_bytes(buf) as usize
            }
            _ => {
                // 127: 8-byte extended length.
                let mut buf = [0u8; 8];
                self.stream.read_exact(&mut buf).await?;
                let len = u64::from_be_bytes(buf);
                // Guard against absurdly large frames.
                if len > 16 * 1024 * 1024 {
                    return Err(HttpError::WebSocket("Frame too large".to_string()));
                }
                len as usize
            }
        };

        // Read the masking key (client → server frames are always masked).
        let mut mask = [0u8; 4];
        if masked {
            self.stream.read_exact(&mut mask).await?;
        }

        // Read and unmask payload.
        let mut payload = vec![0u8; payload_len];
        self.stream.read_exact(&mut payload).await?;
        if masked {
            for (i, byte) in payload.iter_mut().enumerate() {
                *byte ^= mask[i % 4];
            }
        }

        // Handle control frames inline.
        match opcode {
            0x8 => {
                // Connection close — echo a close frame and signal EOF.
                let _ = self.send_frame(0x88, &[]).await;
                Ok(None)
            }
            0x9 => {
                // Ping — RFC 6455 §5.5.2 requires an immediate pong.
                self.send_frame(0x8A, &payload).await?;
                // Return the ping event to the caller so they can log it.
                Ok(Some(WsMessage::Ping))
            }
            0xA => Ok(Some(WsMessage::Pong)),
            0x1 => {
                let text = String::from_utf8(payload)
                    .map_err(|e| HttpError::WebSocket(e.to_string()))?;
                Ok(Some(WsMessage::Text(text)))
            }
            0x2 => Ok(Some(WsMessage::Binary(payload))),
            _ if !fin => {
                // Fragmented frame — not yet supported; close cleanly.
                let _ = self.send_frame(0x88, &[]).await;
                Ok(None)
            }
            other => Err(HttpError::WebSocket(format!("Unknown opcode: {:#x}", other))),
        }
    }
}

/// A received WebSocket message.
#[derive(Debug)]
pub enum WsMessage {
    Text(String),
    Binary(Vec<u8>),
    Ping,
    Pong,
}

// ──────────────────────────────────────────────────────────────
//  Server config
// ──────────────────────────────────────────────────────────────

/// Configuration knobs for [`HttpServer`].
pub struct ServerConfig {
    /// Maximum simultaneous connections across HTTP + HTTPS combined.
    pub max_conns: usize,
    /// How long to wait for the full request headers to arrive (seconds).
    pub parse_timeout_secs: u64,
    /// How long a handler may take before the connection is closed (seconds).
    pub handler_timeout_secs: u64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            max_conns: 1024,
            parse_timeout_secs: DEFAULT_PARSE_TIMEOUT_SECS,
            handler_timeout_secs: DEFAULT_HANDLER_TIMEOUT_SECS,
        }
    }
}

// ──────────────────────────────────────────────────────────────
//  HttpServer
// ──────────────────────────────────────────────────────────────

/// Shared HTTP / HTTPS server state.
///
/// Construct with [`HttpServer::new`], then start listening with [`HttpServer::run_dual`].
pub struct HttpServer {
    router: Arc<Router>,
    /// FIX: replaced `AtomicUsize` load+fetch_add (TOCTOU race) with a Semaphore.
    /// `Semaphore::acquire()` is atomic — two tasks cannot both acquire the last permit.
    semaphore: Arc<Semaphore>,
    config: Arc<ServerConfig>,
}

impl HttpServer {
    pub fn new(router: Router, config: ServerConfig) -> Self {
        let max_conns = config.max_conns;
        Self {
            router: Arc::new(router),
            semaphore: Arc::new(Semaphore::new(max_conns)),
            config: Arc::new(config),
        }
    }

    /// Convenience constructor with default config.
    pub fn with_defaults(router: Router) -> Self {
        Self::new(router, ServerConfig::default())
    }

    /// Load a TLS acceptor from PEM files at the given paths.
    ///
    /// FIX: the original hardcoded `"cert.pem"` and `"key.pem"` relative to CWD,
    /// which silently breaks in tests (CWD is the workspace root, not the crate dir)
    /// and CI. Now the caller controls the paths.
    ///
    /// If either file does not exist, a self-signed development certificate is
    /// generated for `localhost` and `127.0.0.1` and written to those paths.
    pub fn load_tls(cert_path: &str, key_path: &str) -> Result<TlsAcceptor, HttpError> {
        if !std::path::Path::new(cert_path).exists()
            || !std::path::Path::new(key_path).exists()
        {
            warn!(
                "TLS cert/key not found at '{}' / '{}'. Generating self-signed dev certificate.",
                cert_path, key_path
            );
            let cert = generate_simple_self_signed(
                vec!["localhost".into(), "127.0.0.1".into()]
            ).map_err(|e| HttpError::Tls(e.to_string()))?;

            std::fs::write(cert_path, cert.cert.pem())
                .map_err(|e| HttpError::Tls(e.to_string()))?;
            std::fs::write(key_path, cert.key_pair.serialize_pem())
                .map_err(|e| HttpError::Tls(e.to_string()))?;
        }

        let cert_bytes = std::fs::read(cert_path)?;
        let key_bytes  = std::fs::read(key_path)?;

        let certs = rustls_pemfile::certs(&mut BufReader::new(Cursor::new(cert_bytes)))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| HttpError::Tls(e.to_string()))?;

        let key = rustls_pemfile::private_key(&mut BufReader::new(Cursor::new(key_bytes)))
            .map_err(|e| HttpError::Tls(e.to_string()))?
            .ok_or_else(|| HttpError::Tls("No private key found in key file".to_string()))?;

        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .map_err(|e| HttpError::Tls(e.to_string()))?;

        Ok(TlsAcceptor::from(Arc::new(config)))
    }

    /// Listen on `http_addr` and `https_addr` concurrently until both loops end.
    ///
    /// Both listeners share the same semaphore, so HTTP + HTTPS connections together
    /// are capped at `config.max_conns`.
    pub async fn run_dual(
        &self,
        http_addr: &str,
        https_addr: &str,
        cert_path: &str,
        key_path: &str,
    ) -> Result<(), HttpError> {
        let http_listener  = TcpListener::bind(http_addr).await?;
        let https_listener = TcpListener::bind(https_addr).await?;
        let tls_acceptor   = Self::load_tls(cert_path, key_path)?;

        info!(
            "Ferrum listening: HTTP {} | HTTPS {}",
            http_addr, https_addr
        );

        // ── HTTP accept loop ──────────────────────────────────
        let router_h    = self.router.clone();
        let sem_h       = self.semaphore.clone();
        let config_h    = self.config.clone();

        let http_task = tokio::spawn(async move {
            loop {
                let (stream, peer) = match http_listener.accept().await {
                    Ok(v) => v,
                    Err(e) => { warn!("HTTP accept error: {}", e); continue; }
                };

                // `acquire_owned` returns a permit that auto-releases when dropped.
                // FIX: this is atomic — no race between check and increment.
                let permit = match sem_h.clone().try_acquire_owned() {
                    Ok(p) => p,
                    Err(_) => {
                        warn!("Max connections reached, rejecting {}", peer);
                        // Drop stream to send a TCP RST — client gets connection refused.
                        drop(stream);
                        continue;
                    }
                };

                let router  = router_h.clone();
                let config  = config_h.clone();

                tokio::spawn(async move {
                    let _ = handle_connection(Box::new(stream), router, config, peer).await;
                    drop(permit); // Explicitly return the slot.
                });
            }
        });

        // ── HTTPS accept loop ─────────────────────────────────
        let router_s  = self.router.clone();
        let sem_s     = self.semaphore.clone();
        let config_s  = self.config.clone();

        let https_task = tokio::spawn(async move {
            loop {
                let (stream, peer) = match https_listener.accept().await {
                    Ok(v) => v,
                    Err(e) => { warn!("HTTPS accept error: {}", e); continue; }
                };

                let permit = match sem_s.clone().try_acquire_owned() {
                    Ok(p) => p,
                    Err(_) => {
                        warn!("Max connections reached, rejecting {}", peer);
                        drop(stream);
                        continue;
                    }
                };

                let acceptor = tls_acceptor.clone();
                let router   = router_s.clone();
                let config   = config_s.clone();

                tokio::spawn(async move {
                    match acceptor.accept(stream).await {
                        Ok(tls_stream) => {
                            let _ = handle_connection(Box::new(tls_stream), router, config, peer).await;
                        }
                        Err(e) => {
                            warn!("TLS handshake failed from {}: {}", peer, e);
                        }
                    }
                    drop(permit); // Always released, success or failure.
                });
            }
        });

        let _ = tokio::join!(http_task, https_task);
        Ok(())
    }

    /// Start a plain HTTP server on a single address (no TLS).
    pub async fn run(&self, addr: &str) -> Result<(), HttpError> {
        let listener = TcpListener::bind(addr).await?;
        info!("Ferrum listening: HTTP {}", addr);

        loop {
            let (stream, peer) = match listener.accept().await {
                Ok(v) => v,
                Err(e) => { warn!("Accept error: {}", e); continue; }
            };

            let permit = match self.semaphore.clone().try_acquire_owned() {
                Ok(p) => p,
                Err(_) => {
                    warn!("Max connections reached, rejecting {}", peer);
                    drop(stream);
                    continue;
                }
            };

            let router = self.router.clone();
            let config = self.config.clone();

            tokio::spawn(async move {
                let _ = handle_connection(Box::new(stream), router, config, peer).await;
                drop(permit);
            });
        }
    }
}

// ──────────────────────────────────────────────────────────────
//  Connection handler
// ──────────────────────────────────────────────────────────────

/// Drive one HTTP/1.x connection: parse requests, dispatch handlers, write responses.
/// Loops for keep-alive connections. Upgrades to WebSocket and exits the loop.
async fn handle_connection(
    mut stream: Box<dyn AsyncStream>,
    router: Arc<Router>,
    config: Arc<ServerConfig>,
    peer_addr: std::net::SocketAddr,
) -> Result<(), HttpError> {
    let parse_timeout   = Duration::from_secs(config.parse_timeout_secs);
    let handler_timeout = Duration::from_secs(config.handler_timeout_secs);

    loop {
        // ── Parse phase ────────────────────────────────────────
        let req = match timeout(parse_timeout, parse_request(&mut stream, peer_addr)).await {
            Ok(Ok(r))  => r,
            Ok(Err(e)) => {
                // Bad request — try to send a 400 before closing.
                let _ = send_response(&mut stream, Response::new(400)).await;
                return Err(e);
            }
            Err(_) => {
                // Idle timeout — close silently.
                return Err(HttpError::Timeout);
            }
        };

        // Determine keep-alive before req is moved.
        let keep_alive = req.version.should_keep_alive(
            req.headers.get("connection").map(String::as_str)
        );

        // ── Routing ───────────────────────────────────────────
        match router.match_route(&req.method, &req.path) {
            Some((RouteTarget::Http(handler), params)) => {
                // Attach resolved params to the request.
                let mut req = req;
                req.params = params;
                let handler = handler.clone();

                // ── Handler phase (with timeout) ───────────────
                let res = match timeout(handler_timeout, handler(req)).await {
                    Ok(r)  => r,
                    Err(_) => Response::new(504), // Gateway Timeout
                };

                send_response(&mut stream, res).await?;
            }

            Some((RouteTarget::WebSocket(handler), params)) => {
                // ── WebSocket upgrade ──────────────────────────
                // FIX: Content-Length must NOT be sent on 101 responses.
                // FIX: handler now receives WsContext with path, params, headers.
                let ws_key = match req.headers.get("sec-websocket-key") {
                    Some(k) => k.clone(),
                    None => {
                        send_response(&mut stream, Response::new(400)).await?;
                        return Ok(());
                    }
                };

                let accept_key = compute_ws_accept(&ws_key);
                let handler    = handler.clone();
                let ctx = WsContext {
                    path: req.path.clone(),
                    params,
                    headers: req.headers,
                };

                // Send 101 — no Content-Length header.
                send_101_upgrade(&mut stream, &accept_key).await?;

                // Hand the raw stream to the WebSocket handler.
                handler(ctx, WebSocket::new(stream)).await;
                return Ok(()); // Connection belongs to WS handler now.
            }

            None => {
                send_response(&mut stream, Response::new(404)).await?;
            }
        }

        if !keep_alive { break; }
    }

    Ok(())
}

// ──────────────────────────────────────────────────────────────
//  Request parser
// ──────────────────────────────────────────────────────────────

/// Parse one HTTP/1.x request from `stream`.
///
/// Key fixes over the original:
/// - Buffer size increased to 16 KiB.
/// - HTTP version parsed from the request line and stored on `Request`.
/// - Body bytes that arrived in the same read as the headers are not re-read.
pub async fn parse_request(
    stream: &mut Box<dyn AsyncStream>,
    peer_addr: std::net::SocketAddr,
) -> Result<Request, HttpError> {
    let mut buf = vec![0u8; HEADER_BUF_SIZE];
    let mut n   = 0usize;

    // Fill the buffer until the header terminator `\r\n\r\n` is found.
    let header_end = loop {
        if n == buf.len() {
            return Err(HttpError::Parse("Headers too large".to_string()));
        }
        let read = stream.read(&mut buf[n..]).await?;
        if read == 0 {
            return Err(HttpError::Parse("Connection closed mid-header".to_string()));
        }
        n += read;

        if let Some(pos) = buf[..n].windows(4).position(|w| w == b"\r\n\r\n") {
            break pos; // pos is the index of the first `\r` in `\r\n\r\n`.
        }
    };

    // Parse the request line and headers from the slice up to (not including) `\r\n\r\n`.
    let header_section = std::str::from_utf8(&buf[..header_end])
        .map_err(|e| HttpError::Parse(format!("Non-UTF-8 headers: {}", e)))?;

    let (method, path, version, headers) = parse_header_section(header_section)?;

    // ── Body ──────────────────────────────────────────────────
    // FIX: bytes after `\r\n\r\n` that arrived in the same buffer read must be
    // used first before reading more from the stream. The original code correctly
    // sliced these but only when Content-Length was present, so the pattern was
    // already right — made explicit here with a comment.
    let body_start   = header_end + 4; // Skip past `\r\n\r\n`.
    let already_read = n - body_start; // Bytes of body already in buf.

    let body = if let Some(cl) = headers.get("content-length")
        .and_then(|v| v.parse::<usize>().ok())
    {
        if cl > 32 * 1024 * 1024 {
            return Err(HttpError::PayloadTooLarge);
        }
        let mut body = Vec::with_capacity(cl);
        // Re-use whatever body bytes landed in the header buffer.
        body.extend_from_slice(&buf[body_start..body_start + already_read.min(cl)]);
        // Read remaining bytes if the body didn't fully arrive yet.
        if body.len() < cl {
            body.resize(cl, 0);
            stream.read_exact(&mut body[already_read..]).await?;
        }
        body
    } else {
        Vec::new()
    };

    Ok(Request { method, path, version, headers, params: HashMap::new(), body, peer_addr })
}

/// Parse the header section of an HTTP/1.x request synchronously.
pub fn parse_header_section(
    header_section: &str,
) -> Result<(Method, String, HttpVersion, HashMap<String, String>), HttpError> {
    let mut lines = header_section.lines();

    // ── Request line ──────────────────────────────────────────
    let request_line = lines
        .next()
        .ok_or_else(|| HttpError::Parse("Empty request".to_string()))?
        .trim();
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 3 {
        return Err(HttpError::Parse(format!(
            "Malformed request line: {:?}",
            request_line
        )));
    }

    let method = Method::from_str(parts[0])?;
    let path = parts[1].to_string();
    let version = match parts[2] {
        "HTTP/1.1" => HttpVersion::Http11,
        "HTTP/1.0" => HttpVersion::Http10,
        v => {
            return Err(HttpError::Parse(format!(
                "Unsupported HTTP version: {}",
                v
            )))
        }
    };

    // ── Headers ───────────────────────────────────────────────
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_lowercase(), v.trim().to_string());
        }
    }

    Ok((method, path, version, headers))
}

// ──────────────────────────────────────────────────────────────
//  Response serializer
// ──────────────────────────────────────────────────────────────

/// Serialize and write an HTTP response.
///
/// FIX: the original hardcoded `"OK"` as the reason phrase for every status.
/// This function now calls `reason_phrase(status)` for a correct phrase.
pub async fn send_response(
    stream: &mut Box<dyn AsyncStream>,
    response: Response,
) -> Result<(), HttpError> {
    let reason = reason_phrase(response.status);
    let mut buf = Vec::new();

    buf.extend_from_slice(
        format!("HTTP/1.1 {} {}\r\n", response.status, reason).as_bytes()
    );
    for (k, v) in &response.headers {
        buf.extend_from_slice(format!("{}: {}\r\n", k, v).as_bytes());
    }
    buf.extend_from_slice(
        format!("Content-Length: {}\r\n\r\n", response.body.len()).as_bytes()
    );
    buf.extend_from_slice(&response.body);

    stream.write_all(&buf).await?;
    stream.flush().await?;
    Ok(())
}

/// Send the HTTP 101 Switching Protocols response for a WebSocket upgrade.
///
/// FIX: `send_response` always adds `Content-Length`, which must NOT appear on
/// a 101 response. A dedicated function builds the response bytes manually.
async fn send_101_upgrade(
    stream: &mut Box<dyn AsyncStream>,
    accept_key: &str,
) -> Result<(), HttpError> {
    let response = format!(
        "HTTP/1.1 101 Switching Protocols\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Accept: {}\r\n\
         Server: Ferrum-Core/0.7.0\r\n\
         \r\n",
        accept_key
    );
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await?;
    Ok(())
}

// ──────────────────────────────────────────────────────────────
//  WebSocket helpers
// ──────────────────────────────────────────────────────────────

/// Compute the `Sec-WebSocket-Accept` header value from the client's key.
fn compute_ws_accept(key: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(key.as_bytes());
    hasher.update(WS_GUID.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(hasher.finalize())
}

struct BridgeServerState {
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
    port: Option<u16>,
}

#[napi]
pub struct JsServer {
    state: Arc<StdMutex<BridgeServerState>>,
}

#[napi]
impl JsServer {
    #[napi(constructor)]
    pub fn new() -> Self {
        Self {
            state: Arc::new(StdMutex::new(BridgeServerState {
                shutdown: None,
                task: None,
                port: None,
            })),
        }
    }

    #[napi]
    pub async fn listen(&self, port: u16) -> napi::Result<()> {
        {
            let state = self
                .state
                .lock()
                .map_err(|_| napi::Error::from_reason("server state poisoned".to_string()))?;
            if state.task.is_some() {
                return Err(napi::Error::from_reason(
                    "server is already listening".to_string(),
                ));
            }
        }

        let listener = TcpListener::bind(("127.0.0.1", port))
            .await
            .map_err(|err| napi::Error::from_reason(err.to_string()))?;
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => {
                        break;
                    }
                    accept = listener.accept() => {
                        let Ok((mut socket, _peer)) = accept else {
                            break;
                        };
                        tokio::spawn(async move {
                            let mut buffer = [0u8; 1024];
                            let bytes_read = socket.read(&mut buffer).await.unwrap_or(0);
                            let request = String::from_utf8_lossy(&buffer[..bytes_read]);
                            let body = if request.starts_with("GET /health") {
                                r#"{"ok":true,"service":"ferrum-http"}"#
                            } else {
                                r#"{"ok":true,"message":"Ferrum JS bridge server"}"#
                            };
                            let response = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                body.len(),
                                body
                            );
                            let _ = socket.write_all(response.as_bytes()).await;
                            let _ = socket.shutdown().await;
                        });
                    }
                }
            }
        });

        let mut state = self
            .state
            .lock()
            .map_err(|_| napi::Error::from_reason("server state poisoned".to_string()))?;
        state.shutdown = Some(shutdown_tx);
        state.task = Some(task);
        state.port = Some(port);
        Ok(())
    }

    #[napi]
    pub async fn close(&self) -> napi::Result<()> {
        let task = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| napi::Error::from_reason("server state poisoned".to_string()))?;
            if let Some(shutdown) = state.shutdown.take() {
                let _ = shutdown.send(());
            }
            state.port = None;
            state.task.take()
        };

        if let Some(task) = task {
            let _ = task.await;
        }

        Ok(())
    }

    #[napi(getter)]
    pub fn port(&self) -> Option<u16> {
        self.state.lock().ok().and_then(|state| state.port)
    }
}

impl Default for JsServer {
    fn default() -> Self {
        Self::new()
    }
}

#[napi]
pub fn create_server() -> JsServer {
    JsServer::new()
}

// ──────────────────────────────────────────────────────────────
//  Tests
// ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use futures::future::FutureExt;

    // ── reason_phrase ────────────────────────────────────────

    #[test]
    fn reason_phrase_matches_status() {
        assert_eq!(reason_phrase(200), "OK");
        assert_eq!(reason_phrase(404), "Not Found");
        assert_eq!(reason_phrase(429), "Too Many Requests");
        assert_eq!(reason_phrase(503), "Service Unavailable");
        assert_eq!(reason_phrase(101), "Switching Protocols");
    }

    // ── HttpVersion keep-alive logic ─────────────────────────

    #[test]
    fn http11_defaults_to_keep_alive() {
        assert!(HttpVersion::Http11.should_keep_alive(None));
    }

    #[test]
    fn http10_defaults_to_close() {
        assert!(!HttpVersion::Http10.should_keep_alive(None));
    }

    #[test]
    fn explicit_close_overrides_http11_default() {
        assert!(!HttpVersion::Http11.should_keep_alive(Some("close")));
    }

    #[test]
    fn explicit_keep_alive_overrides_http10_default() {
        assert!(HttpVersion::Http10.should_keep_alive(Some("keep-alive")));
    }

    // ── Router ───────────────────────────────────────────────

    fn dummy_handler() -> Handler {
        Arc::new(|_req| async move { Response::new(200) }.boxed())
    }

    #[test]
    fn router_exact_match() {
        let mut router = Router::new();
        router.add_http(Method::GET, "/health", dummy_handler());
        assert!(router.match_route(&Method::GET, "/health").is_some());
        assert!(router.match_route(&Method::GET, "/other").is_none());
    }

    #[test]
    fn router_param_extraction() {
        let mut router = Router::new();
        router.add_http(Method::GET, "/users/:id", dummy_handler());
        let (_, params) = router.match_route(&Method::GET, "/users/42").unwrap();
        assert_eq!(params.get("id").map(String::as_str), Some("42"));
    }

    #[test]
    fn router_method_mismatch() {
        let mut router = Router::new();
        router.add_http(Method::GET, "/items", dummy_handler());
        assert!(router.match_route(&Method::POST, "/items").is_none());
    }

    #[test]
    fn router_root_path() {
        let mut router = Router::new();
        router.add_http(Method::GET, "/", dummy_handler());
        assert!(router.match_route(&Method::GET, "/").is_some());
    }

    // ── ws_accept key ────────────────────────────────────────

    #[test]
    fn ws_accept_key_matches_rfc_example() {
        // RFC 6455 section 1.3 test vector.
        let key    = "dGhlIHNhbXBsZSBub25jZQ==";
        let expect = "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=";
        assert_eq!(compute_ws_accept(key), expect);
    }

    // ── Response serializer ──────────────────────────────────

    #[tokio::test]
    async fn send_response_writes_correct_status_line() {
        // Use a Vec<u8> as a fake stream.
        let mut buf: Vec<u8> = Vec::new();
        // We can't box a Vec<u8> directly as AsyncStream because Vec<u8>
        // doesn't implement AsyncRead. Use a tokio duplex instead.
        let (mut client, server) = tokio::io::duplex(4096);

        let res = {
            let mut r = Response::new(404);
            r.body = b"not found".to_vec();
            r
        };

        // Write from server side.
        let write_task = tokio::spawn(async move {
            let mut boxed: Box<dyn AsyncStream> = Box::new(server);
            send_response(&mut boxed, res).await.unwrap();
        });

        write_task.await.unwrap();

        // Read the full response after the writer side closes.
        client.read_to_end(&mut buf).await.unwrap();
        let text = String::from_utf8_lossy(&buf);
        assert!(text.starts_with("HTTP/1.1 404 Not Found\r\n"), "Got: {}", &text[..40.min(text.len())]);
        assert!(text.contains("Content-Length: 9\r\n"));
    }
}
