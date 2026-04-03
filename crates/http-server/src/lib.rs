use tokio::net::TcpListener;
use tokio::io::{AsyncReadExt, AsyncWriteExt, AsyncRead, AsyncWrite};
use tokio::time::{timeout, Duration};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::io::{BufReader, Cursor};
use thiserror::Error;
use tracing::info;
use futures::future::BoxFuture;
use serde::de::DeserializeOwned;
use sha1::{Sha1, Digest};
use base64::Engine;

// TLS & Cryptography
use tokio_rustls::TlsAcceptor;
use rcgen::generate_simple_self_signed;

/// 🍱 Phase 10: HTTP Server TLS Support
/// HTTPS, Dual-Port Listeners, and Automatic Dev Cert Generation.

const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

#[derive(Error, Debug)]
pub enum HttpError {
    #[error("IO Error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("Timeout: Connection idle too long")]
    Timeout,
    #[error("Payload too large (413)")]
    PayloadTooLarge,
    #[error("Serialization error: {0}")]
    Serialization(String),
    #[error("WebSocket Error: {0}")]
    WebSocket(String),
    #[error("TLS Error: {0}")]
    Tls(String),
}

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub enum Method {
    GET, POST, PUT, DELETE, PATCH, OPTIONS, HEAD,
}

impl FromStr for Method {
    type Err = HttpError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "GET" => Ok(Method::GET),
            "POST" => Ok(Method::POST),
            "PUT" => Ok(Method::PUT),
            "DELETE" => Ok(Method::DELETE),
            "PATCH" => Ok(Method::PATCH),
            "OPTIONS" => Ok(Method::OPTIONS),
            "HEAD" => Ok(Method::HEAD),
            _ => Err(HttpError::Parse(format!("Unknown method: {}", s))),
        }
    }
}

pub type Handler = Arc<dyn Fn(Request) -> BoxFuture<'static, Response> + Send + Sync>;
pub type WsHandler = Arc<dyn Fn(WebSocket) -> BoxFuture<'static, ()> + Send + Sync>;

#[derive(Debug)]
pub struct Request {
    pub method: Method,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub params: HashMap<String, String>, 
    pub body: Vec<u8>,
}

impl Request {
    pub fn json<T: DeserializeOwned>(&self) -> Result<T, HttpError> {
        serde_json::from_slice(&self.body).map_err(|e| HttpError::Serialization(e.to_string()))
    }
    pub fn is_websocket_upgrade(&self) -> bool {
        self.headers.get("upgrade").map(|v| v.to_lowercase() == "websocket").unwrap_or(false)
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
        let body = serde_json::to_vec(data).map_err(|e| HttpError::Serialization(e.to_string()))?;
        let mut res = Self::new(status);
        res.headers.insert("Content-Type".to_string(), "application/json".to_string());
        res.body = body;
        Ok(res)
    }
}

pub enum RouteTarget {
    Http(Handler),
    WebSocket(WsHandler),
}

pub struct Route {
    pub method: Method,
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
        let pattern = path.split('/').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();
        self.routes.push(Route { method, pattern, target: RouteTarget::Http(handler) });
    }
    pub fn ws(&mut self, path: &str, handler: WsHandler) {
        let pattern = path.split('/').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();
        self.routes.push(Route { method: Method::GET, pattern, target: RouteTarget::WebSocket(handler) });
    }

    pub fn match_route(&self, method: Method, path: &str) -> Option<(&RouteTarget, HashMap<String, String>)> {
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        for route in &self.routes {
            if route.method != method { continue; }
            let mut params = HashMap::new();
            if route.pattern.is_empty() && segments.is_empty() { return Some((&route.target, params)); }
            if route.pattern.len() != segments.len() { continue; }
            let mut matched = true;
            for (p_seg, s_seg) in route.pattern.iter().zip(segments.iter()) {
                if p_seg.starts_with(':') { params.insert(p_seg[1..].to_string(), s_seg.to_string()); }
                else if p_seg != s_seg { matched = false; break; }
            }
            if matched { return Some((&route.target, params)); }
        }
        None
    }
}

pub struct WebSocket {
    stream: Box<dyn AsyncStream>,
}

pub trait AsyncStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> AsyncStream for T {}

impl WebSocket {
    pub fn new(stream: Box<dyn AsyncStream>) -> Self { Self { stream } }

    pub async fn send_text(&mut self, text: &str) -> Result<(), HttpError> {
        let payload = text.as_bytes();
        let mut frame = vec![0x81];
        if payload.len() <= 125 { frame.push(payload.len() as u8); } 
        else { frame.push(126); frame.extend_from_slice(&(payload.len() as u16).to_be_bytes()); }
        frame.extend_from_slice(payload);
        self.stream.write_all(&frame).await?;
        self.stream.flush().await?;
        Ok(())
    }

    pub async fn next_message(&mut self) -> Result<Option<String>, HttpError> {
        let mut header = [0; 2];
        if self.stream.read_exact(&mut header).await.is_err() { return Ok(None); }
        let mut mask = [0; 4];
        self.stream.read_exact(&mut mask).await?;
        let len = (header[1] & 0x7F) as usize;
        let mut payload = vec![0; len];
        self.stream.read_exact(&mut payload).await?;
        for i in 0..payload.len() { payload[i] ^= mask[i % 4]; }
        Ok(Some(String::from_utf8_lossy(&payload).to_string()))
    }
}

pub struct HttpServer {
    router: Arc<Router>,
    active_conns: Arc<AtomicUsize>,
    max_conns: usize,
}

impl HttpServer {
    pub fn new(router: Router, max_conns: usize) -> Self {
        Self { router: Arc::new(router), active_conns: Arc::new(AtomicUsize::new(0)), max_conns }
    }

    pub fn load_dev_cert() -> Result<TlsAcceptor, HttpError> {
        let cert_path = "cert.pem";
        let key_path = "key.pem";

        if !std::path::Path::new(cert_path).exists() {
            info!("Generating JIT Self-Signed Development Certificate... 🔐");
            let cert = generate_simple_self_signed(vec!["localhost".into(), "127.0.0.1".into()]).map_err(|e| HttpError::Tls(e.to_string()))?;
            std::fs::write(cert_path, cert.cert.pem()).map_err(|e| HttpError::Tls(e.to_string()))?;
            std::fs::write(key_path, cert.key_pair.serialize_pem()).map_err(|e| HttpError::Tls(e.to_string()))?;
        }

        let cert_file = std::fs::read(cert_path)?;
        let key_file = std::fs::read(key_path)?;

        let mut cert_reader = BufReader::new(Cursor::new(cert_file));
        let certs = rustls_pemfile::certs(&mut cert_reader).collect::<Result<Vec<_>, _>>().map_err(|e| HttpError::Tls(e.to_string()))?;
        
        let mut key_reader = BufReader::new(Cursor::new(key_file));
        let key = rustls_pemfile::private_key(&mut key_reader).map_err(|e| HttpError::Tls(e.to_string()))?.ok_or(HttpError::Tls("No key found".to_string()))?;

        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .map_err(|e| HttpError::Tls(e.to_string()))?;

        Ok(TlsAcceptor::from(Arc::new(config)))
    }

    pub async fn run_dual(&self, http_addr: &str, https_addr: &str) -> Result<(), HttpError> {
        let http_listener = TcpListener::bind(http_addr).await?;
        let https_listener = TcpListener::bind(https_addr).await?;
        let tls_acceptor = Self::load_dev_cert()?;

        info!("Ferrum Secure Stack: 🛰️ http://{} | 🔒 https://{}", http_addr, https_addr);

        let router_http = self.router.clone();
        let active_http = self.active_conns.clone();
        let max_conns = self.max_conns;

        let http_handle = tokio::spawn(async move {
            while let Ok((stream, _)) = http_listener.accept().await {
                if active_http.load(Ordering::SeqCst) < max_conns {
                    active_http.fetch_add(1, Ordering::SeqCst);
                    let counter = active_http.clone();
                    let r = router_http.clone();
                    tokio::spawn(async move {
                        let _ = handle_generic_stack(Box::new(stream), r).await;
                        counter.fetch_sub(1, Ordering::SeqCst);
                    });
                }
            }
        });

        let router_https = self.router.clone();
        let active_https = self.active_conns.clone();
        let https_handle = tokio::spawn(async move {
            while let Ok((stream, _)) = https_listener.accept().await {
                let acceptor = tls_acceptor.clone();
                if active_https.load(Ordering::SeqCst) < max_conns {
                    active_https.fetch_add(1, Ordering::SeqCst);
                    let counter = active_https.clone();
                    let r = router_https.clone();
                    tokio::spawn(async move {
                        if let Ok(tls_stream) = acceptor.accept(stream).await {
                             let _ = handle_generic_stack(Box::new(tls_stream), r).await;
                        }
                        counter.fetch_sub(1, Ordering::SeqCst);
                    });
                }
            }
        });

        let _ = tokio::join!(http_handle, https_handle);
        Ok(())
    }
}

async fn handle_generic_stack(mut stream: Box<dyn AsyncStream>, router: Arc<Router>) -> Result<(), HttpError> {
    let req = match timeout(Duration::from_secs(10), parse_complete_request(&mut stream)).await {
        Ok(Ok(r)) => r,
        _ => return Ok(()),
    };

    if let Some((target, _)) = router.match_route(req.method.clone(), &req.path) {
        match target {
            RouteTarget::Http(handler) => {
                let res = handler(req).await;
                send_response(&mut stream, res).await?;
            },
            RouteTarget::WebSocket(handler) => {
                let key = req.headers.get("sec-websocket-key").ok_or(HttpError::WebSocket("Key fail".to_string()))?;
                let mut hasher = Sha1::new();
                hasher.update(key.as_bytes()); hasher.update(WS_GUID.as_bytes());
                let accept = base64::engine::general_purpose::STANDARD.encode(hasher.finalize());
                let mut res = Response::new(101);
                res.headers.insert("Sec-WebSocket-Accept".to_string(), accept);
                send_response(&mut stream, res).await?;
                handler(WebSocket::new(stream)).await;
            }
        }
    }
    Ok(())
}

pub async fn parse_complete_request(stream: &mut Box<dyn AsyncStream>) -> Result<Request, HttpError> {
    let mut header_buf = [0; 4096];
    let mut n = 0;
    while n < header_buf.len() {
        let read = stream.read(&mut header_buf[n..n+1]).await?;
        if read == 0 { break; }
        n += 1;
        if n >= 4 && &header_buf[n-4..n] == b"\r\n\r\n" { break; }
    }
    let raw = String::from_utf8_lossy(&header_buf[..n]);
    let (head, _) = raw.split_once("\r\n\r\n").ok_or(HttpError::Parse("Head fail".to_string()))?;
    let mut lines = head.lines();
    let r_line = lines.next().ok_or(HttpError::Parse("Req line fail".to_string()))?;
    let parts: Vec<&str> = r_line.split_whitespace().collect();
    let method = Method::from_str(parts[0])?;
    let mut headers = HashMap::new();
    for l in lines { if let Some((k, v)) = l.split_once(':') { headers.insert(k.trim().to_lowercase().to_string(), v.trim().to_string()); } }
    Ok(Request { method, path: parts[1].to_string(), headers, params: HashMap::new(), body: Vec::new() })
}

pub async fn send_response(stream: &mut Box<dyn AsyncStream>, response: Response) -> Result<(), HttpError> {
    let mut buf = Vec::new();
    buf.extend_from_slice(format!("HTTP/1.1 {} OK\r\n", response.status).as_bytes());
    for (k, v) in &response.headers { buf.extend_from_slice(format!("{}: {}\r\n", k, v).as_bytes()); }
    buf.extend_from_slice(format!("Content-Length: {}\r\n\r\n", response.body.len()).as_bytes());
    buf.extend_from_slice(&response.body);
    stream.write_all(&buf).await?;
    stream.flush().await?;
    Ok(())
}
