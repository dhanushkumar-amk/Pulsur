use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::{timeout, Duration};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::io::Write;
use thiserror::Error;
use tracing::{info, warn, debug};
use futures::future::BoxFuture;
use serde::de::DeserializeOwned;
use flate2::write::GzEncoder;
use flate2::Compression;

/// 🍱 Phase 8: HTTP Server Body Parsing & Content Types
/// JSON, Form-data, Multipart, Size Limits, and Gzip Compression.

const MAX_BODY_SIZE: usize = 10 * 1024 * 1024; // 10MB Default Limit

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
    #[error("Router error: {0}")]
    Router(String),
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
    /// Parse JSON body into a typed struct
    pub fn json<T: DeserializeOwned>(&self) -> Result<T, HttpError> {
        serde_json::from_slice(&self.body).map_err(|e| HttpError::Serialization(e.to_string()))
    }

    /// Parse x-www-form-urlencoded body
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

    pub fn content_type(&self) -> Option<&String> {
        self.headers.get("content-type")
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
        headers.insert("Server".to_string(), "Ferrum-Core/0.5.0".to_string());
        Self { status, headers, body: Vec::new() }
    }

    pub fn json<T: serde::Serialize>(status: u16, val: &T) -> Result<Self, HttpError> {
        let mut res = Self::new(status);
        res.body = serde_json::to_vec(val).map_err(|e| HttpError::Serialization(e.to_string()))?;
        res.headers.insert("Content-Type".to_string(), "application/json".to_string());
        Ok(res)
    }
}

pub struct Route {
    pub method: Method,
    pub pattern: Vec<String>,
    pub handler: Handler,
}

#[derive(Default)]
pub struct Router {
    pub routes: Vec<Route>,
    pub middleware: Vec<Arc<dyn Middleware>>,
}

impl Router {
    pub fn new() -> Self { Self::default() }
    pub fn add_route(&mut self, method: Method, path: &str, handler: Handler) {
        let pattern = path.split('/').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();
        self.routes.push(Route { method, pattern, handler });
    }
    pub fn get(&mut self, path: &str, handler: Handler) { self.add_route(Method::GET, path, handler); }
    pub fn post(&mut self, path: &str, handler: Handler) { self.add_route(Method::POST, path, handler); }
    pub fn add_middleware(&mut self, mw: Arc<dyn Middleware>) { self.middleware.push(mw); }

    pub fn match_route(&self, method: Method, path: &str) -> Option<(Handler, HashMap<String, String>)> {
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        for route in &self.routes {
            if route.method != method { continue; }
            let mut params = HashMap::new();
            if route.pattern.is_empty() && segments.is_empty() { return Some((route.handler.clone(), params)); }
            if route.pattern.len() != segments.len() { continue; }
            let mut matched = true;
            for (p_seg, s_seg) in route.pattern.iter().zip(segments.iter()) {
                if p_seg.starts_with(':') { params.insert(p_seg[1..].to_string(), s_seg.to_string()); }
                else if p_seg != s_seg { matched = false; break; }
            }
            if matched { return Some((route.handler.clone(), params)); }
        }
        None
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
        info!("Ferrum Server [Phase 8]: 🛰️ http://{} [Gzip Support Enabled]", self.addr);

        let shutdown_signal = tokio::signal::ctrl_c();
        tokio::pin!(shutdown_signal);

        loop {
            tokio::select! {
                _ = &mut shutdown_signal => {
                    info!("Graceful Draining...");
                    break;
                }
                accept_res = listener.accept() => {
                    let (stream, _) = accept_res?;
                    if self.active_conns.load(Ordering::SeqCst) >= self.max_conns {
                        warn!("Capacity Full");
                        continue;
                    }
                    self.active_conns.fetch_add(1, Ordering::SeqCst);
                    let router = self.router.clone();
                    let counter = self.active_conns.clone();
                    tokio::spawn(async move {
                        let _ = handle_full_stack_loop(stream, router).await;
                        counter.fetch_sub(1, Ordering::SeqCst);
                    });
                }
            }
        }
        Ok(())
    }
}

async fn handle_full_stack_loop(mut stream: TcpStream, router: Arc<Router>) -> Result<(), HttpError> {
    loop {
        let request_res = timeout(Duration::from_secs(30), parse_complete_request(&mut stream)).await;
        let mut request = match request_res {
            Ok(Ok(req)) => req,
            Ok(Err(e)) => {
                if let HttpError::PayloadTooLarge = e {
                    let mut res = Response::new(413);
                    res.body = b"413 Payload Too Large".to_vec();
                    send_response(&mut stream, res).await?;
                }
                return Err(e);
            },
            Err(_) => return Ok(()),
        };

        let keep_alive = request.headers.get("connection").map(|v| v.to_lowercase() == "keep-alive").unwrap_or(false);
        let accept_gzip = request.headers.get("accept-encoding").map(|v| v.contains("gzip")).unwrap_or(false);

        // Routing & Handlers
        let mut response = if let Some((handler, params)) = router.match_route(request.method.clone(), &request.path) {
            request.params = params;
            handler(request).await
        } else {
            let mut r = Response::new(404);
            r.body = b"Not Found".to_vec();
            r
        };

        // Gzip Compression Negotiation
        if accept_gzip && response.body.len() > 1024 {
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(&response.body)?;
            response.body = encoder.finish()?;
            response.headers.insert("Content-Encoding".to_string(), "gzip".to_string());
        }

        if keep_alive { response.headers.insert("Connection".to_string(), "keep-alive".to_string()); }
        send_response(&mut stream, response).await?;
        if !keep_alive { break; }
    }
    Ok(())
}

pub async fn parse_complete_request(stream: &mut TcpStream) -> Result<Request, HttpError> {
    let mut header_buf = [0; 4096]; // Headers only
    let mut n = 0;
    
    // Read until we find the double CRLF (end of headers)
    loop {
        let read = stream.read(&mut header_buf[n..]).await?;
        if read == 0 { return Err(HttpError::Parse("Unexpected close".to_string())); }
        n += read;
        if std::str::from_utf8(&header_buf[..n]).unwrap_or("").contains("\r\n\r\n") { break; }
        if n >= header_buf.len() { return Err(HttpError::Parse("Headers too large".to_string())); }
    }

    let raw = std::str::from_utf8(&header_buf[..n]).map_err(|_| HttpError::Parse("Encoding Error".to_string()))?;
    let (head_part, _) = raw.split_once("\r\n\r\n").ok_or(HttpError::Parse("Incomplete".to_string()))?;
    let mut lines = head_part.lines();
    
    let req_line = lines.next().ok_or(HttpError::Parse("Missing req line".to_string()))?;
    let parts: Vec<&str> = req_line.split_whitespace().collect();
    let method = Method::from_str(parts[0])?;
    let path = parts[1].to_string();
    
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_lowercase().to_string(), v.trim().to_string());
        }
    }

    // Handle Body
    let content_length = headers.get("content-length").and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
    if content_length > MAX_BODY_SIZE { return Err(HttpError::PayloadTooLarge); }

    let mut body = vec![0; content_length];
    if content_length > 0 {
        stream.read_exact(&mut body).await?;
    }

    Ok(Request { method, path, headers, params: HashMap::new(), body })
}

pub async fn send_response(stream: &mut TcpStream, response: Response) -> Result<(), HttpError> {
    let mut buf = Vec::new();
    buf.extend_from_slice(format!("HTTP/1.1 {} OK\r\n", response.status).as_bytes());
    for (k, v) in &response.headers { buf.extend_from_slice(format!("{}: {}\r\n", k, v).as_bytes()); }
    buf.extend_from_slice(format!("Content-Length: {}\r\n\r\n", response.body.len()).as_bytes());
    buf.extend_from_slice(&response.body);
    stream.write_all(&buf).await?;
    stream.flush().await?;
    Ok(())
}
