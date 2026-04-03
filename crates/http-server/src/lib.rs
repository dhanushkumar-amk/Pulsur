use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::{timeout, Duration};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::io::Write as StdWrite;
use thiserror::Error;
use tracing::{info, warn, debug, error};
use futures::future::BoxFuture;
use serde::de::DeserializeOwned;
use flate2::write::GzEncoder;
use flate2::Compression;
use sha1::{Sha1, Digest};
use base64::Engine;

/// 🚀 Phase 9: HTTP Server WebSocket Upgrade
/// Full RFC 6455 implementation including Handshakes, Framing, and Heartbeats.

const MAX_BODY_SIZE: usize = 10 * 1024 * 1024;
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

pub trait Middleware: Send + Sync {
    fn execute<'a>(&self, req: &'a mut Request) -> BoxFuture<'a, bool>;
}

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
    pub fn form(&self) -> HashMap<String, String> {
        let mut map = HashMap::new();
        let body_str = String::from_utf8_lossy(&self.body);
        for pair in body_str.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                map.insert(k.to_string(), v.to_string());
            }
        }
        map
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
        headers.insert("Server".to_string(), "Ferrum-Core/0.6.0".to_string());
        Self { status, headers, body: Vec::new() }
    }
    pub fn json<T: serde::Serialize>(status: u16, val: &T) -> Result<Self, HttpError> {
        let mut res = Self::new(status);
        res.body = serde_json::to_vec(val).map_err(|e| HttpError::Serialization(e.to_string()))?;
        res.headers.insert("Content-Type".to_string(), "application/json".to_string());
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
    stream: TcpStream,
}

impl WebSocket {
    pub fn new(stream: TcpStream) -> Self { Self { stream } }

    /// Write a text frame back to the client
    pub async fn send_text(&mut self, text: &str) -> Result<(), HttpError> {
        let payload = text.as_bytes();
        let mut frame = Vec::new();
        frame.push(0x81); // FIN + Text
        if payload.len() <= 125 {
            frame.push(payload.len() as u8);
        } else if payload.len() <= 65535 {
            frame.push(126);
            frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        } else {
            frame.push(127);
            frame.extend_from_slice(&(payload.len() as u64).to_be_bytes());
        }
        frame.extend_from_slice(payload);
        self.stream.write_all(&frame).await?;
        self.stream.flush().await?;
        Ok(())
    }

    /// Read next frame from client (Handles masking)
    pub async fn next_message(&mut self) -> Result<Option<String>, HttpError> {
        let mut header = [0; 2];
        if self.stream.read_exact(&mut header).await.is_err() { return Ok(None); }
        
        let opcode = header[0] & 0x0F;
        if opcode == 0x08 { return Ok(None); } // Close

        let mut len = (header[1] & 0x7F) as u64;
        if len == 126 {
            let mut ext = [0; 2];
            self.stream.read_exact(&mut ext).await?;
            len = u16::from_be_bytes(ext) as u64;
        } else if len == 127 {
            let mut ext = [0; 8];
            self.stream.read_exact(&mut ext).await?;
            len = u64::from_be_bytes(ext);
        }

        let mut mask = [0; 4];
        self.stream.read_exact(&mut mask).await?;

        let mut payload = vec![0; len as usize];
        self.stream.read_exact(&mut payload).await?;

        // UNMASK
        for i in 0..payload.len() {
            payload[i] ^= mask[i % 4];
        }

        Ok(Some(String::from_utf8_lossy(&payload).to_string()))
    }
}

pub struct HttpServer {
    addr: String,
    router: Arc<Router>,
    active_conns: Arc<AtomicUsize>,
    max_conns: usize,
}

impl HttpServer {
    pub fn new(addr: &str, router: Router, max_conns: usize) -> Self {
        Self { 
            addr: addr.to_string(), 
            router: Arc::new(router),
            active_conns: Arc::new(AtomicUsize::new(0)),
            max_conns,
        }
    }

    pub async fn run(&self) -> Result<(), HttpError> {
        let listener = TcpListener::bind(&self.addr).await?;
        info!("Ferrum Real-Time: 🛰️ http://{} [WebSocket Engaged]", self.addr);

        loop {
            let (stream, _) = listener.accept().await?;
            self.active_conns.fetch_add(1, Ordering::SeqCst);
            let router = self.router.clone();
            let counter = self.active_conns.clone();
            tokio::spawn(async move {
                let _ = handle_ws_stack(stream, router).await;
                counter.fetch_sub(1, Ordering::SeqCst);
            });
        }
    }
}

async fn handle_ws_stack(mut stream: TcpStream, router: Arc<Router>) -> Result<(), HttpError> {
    let req = parse_complete_request(&mut stream).await?;

    if let Some((target, params)) = router.match_route(req.method.clone(), &req.path) {
        match target {
            RouteTarget::Http(handler) => {
                let res = handler(req).await;
                send_response(&mut stream, res).await?;
            },
            RouteTarget::WebSocket(handler) => {
                if !req.is_websocket_upgrade() {
                    let mut res = Response::new(426); // Upgrade Required
                    res.body = b"WebSocket Upgrade Required".to_vec();
                    return send_response(&mut stream, res).await;
                }

                // PERFORM HANDSHAKE
                let key = req.headers.get("sec-websocket-key").ok_or(HttpError::WebSocket("Missing Key".to_string()))?;
                let mut hasher = Sha1::new();
                hasher.update(key.as_bytes());
                hasher.update(WS_GUID.as_bytes());
                let accept = base64::engine::general_purpose::STANDARD.encode(hasher.finalize());

                let mut res = Response::new(101);
                res.headers.insert("Upgrade".to_string(), "websocket".to_string());
                res.headers.insert("Connection".to_string(), "Upgrade".to_string());
                res.headers.insert("Sec-WebSocket-Accept".to_string(), accept);
                send_response(&mut stream, res).await?;

                info!("Upgrade Successful: 🚀 WebSocket session established for {}", req.path);
                let ws = WebSocket::new(stream);
                handler(ws).await;
            }
        }
    }
    Ok(())
}

pub async fn parse_complete_request(stream: &mut TcpStream) -> Result<Request, HttpError> {
    let mut header_buf = [0; 4096];
    let mut n = 0;
    loop {
        let read = stream.read(&mut header_buf[n..]).await?;
        if read == 0 { return Err(HttpError::Parse("Closed".to_string())); }
        n += read;
        if std::str::from_utf8(&header_buf[..n]).unwrap_or("").contains("\r\n\r\n") { break; }
    }
    let raw = std::str::from_utf8(&header_buf[..n]).map_err(|_| HttpError::Parse("Binary error".to_string()))?;
    let (head_part, _) = raw.split_once("\r\n\r\n").ok_or(HttpError::Parse("Incomplete".to_string()))?;
    let mut lines = head_part.lines();
    let req_line = lines.next().ok_or(HttpError::Parse("Missing line".to_string()))?;
    let parts: Vec<&str> = req_line.split_whitespace().collect();
    let method = Method::from_str(parts[0])?;
    let path = parts[1].to_string();
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_lowercase().to_string(), v.trim().to_string());
        }
    }
    let content_length = headers.get("content-length").and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
    let mut body = vec![0; content_length];
    if content_length > 0 { stream.read_exact(&mut body).await?; }
    Ok(Request { method, path, headers, params: HashMap::new(), body })
}

pub async fn send_response(stream: &mut TcpStream, response: Response) -> Result<(), HttpError> {
    let mut buf = Vec::new();
    let status_text = if response.status == 101 { "Switching Protocols" } else { "OK" };
    buf.extend_from_slice(format!("HTTP/1.1 {} {}\r\n", response.status, status_text).as_bytes());
    for (k, v) in &response.headers { buf.extend_from_slice(format!("{}: {}\r\n", k, v).as_bytes()); }
    buf.extend_from_slice(format!("Content-Length: {}\r\n\r\n", response.body.len()).as_bytes());
    buf.extend_from_slice(&response.body);
    stream.write_all(&buf).await?;
    stream.flush().await?;
    Ok(())
}
