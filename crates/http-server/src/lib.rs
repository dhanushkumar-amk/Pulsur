use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::{timeout, Duration};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use thiserror::Error;
use tracing::{info, warn, debug};
use futures::future::BoxFuture;

/// 🛰️ Phase 7: HTTP Server Keep-Alive & Connection Management
/// High-performance connection pooling, resource limits, and graceful shutdown.

#[derive(Error, Debug)]
pub enum HttpError {
    #[error("IO Error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("Timeout: Connection idle too long")]
    Timeout,
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

pub struct Response {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn new(status: u16) -> Self {
        let mut headers = HashMap::new();
        headers.insert("Server".to_string(), "Ferrum-Core/0.4.0".to_string());
        Self { status, headers, body: Vec::new() }
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
        info!("Ferrum Engine Online: 🚀 http://{} [Limit: {} conns]", self.addr, self.max_conns);

        let shutdown_signal = tokio::signal::ctrl_c();
        tokio::pin!(shutdown_signal);

        loop {
            tokio::select! {
                _ = &mut shutdown_signal => {
                    info!("Graceful Shutdown Initiated: 🛑 Draining active connections...");
                    let _ = timeout(Duration::from_secs(5), async {
                        while self.active_conns.load(Ordering::SeqCst) > 0 {
                            tokio::task::yield_now().await;
                        }
                    }).await;
                    info!("Shutdown Complete. Safe to disconnect.");
                    break;
                }
                accept_res = listener.accept() => {
                    let (stream, _) = accept_res?;
                    
                    let active = self.active_conns.load(Ordering::SeqCst);
                    if active >= self.max_conns {
                        warn!("Maximum connections reached! [Active: {}]", active);
                        tokio::spawn(async move {
                            let mut s = stream;
                            let mut res = Response::new(503);
                            res.body = b"503 Service Unavailable: Maximum capacity reached".to_vec();
                            let _ = send_response(&mut s, res).await;
                        });
                        continue;
                    }

                    self.active_conns.fetch_add(1, Ordering::SeqCst);
                    let router = self.router.clone();
                    let counter = self.active_conns.clone();

                    tokio::spawn(async move {
                        if let Err(e) = handle_keepalive_loop(stream, router).await {
                            debug!("Connection closed: {}", e);
                        }
                        counter.fetch_sub(1, Ordering::SeqCst);
                    });
                }
            }
        }
        Ok(())
    }
}

async fn handle_keepalive_loop(mut stream: TcpStream, router: Arc<Router>) -> Result<(), HttpError> {
    loop {
        // IDLE TIMEOUT: 30 seconds (Requirement)
        let request_res = timeout(Duration::from_secs(30), parse_request(&mut stream)).await;
        
        let mut request = match request_res {
            Ok(Ok(req)) => req,
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                debug!("Keep-Alive timeout reached. Closing connection.");
                return Ok(());
            }
        };

        let keep_alive = request.headers.get("connection").map(|v| v.to_lowercase() == "keep-alive").unwrap_or(false);

        let mut mw_allowed = true;
        for mw in &router.middleware {
            if !mw.execute(&mut request).await {
                let mut res = Response::new(403);
                res.body = b"Forbidden by Policy".to_vec();
                send_response(&mut stream, res).await?;
                mw_allowed = false;
                break;
            }
        }
        if !mw_allowed { break; }

        if let Some((handler, params)) = router.match_route(request.method.clone(), &request.path) {
            request.params = params;
            let mut response = handler(request).await;
            if keep_alive { response.headers.insert("Connection".to_string(), "keep-alive".to_string()); }
            send_response(&mut stream, response).await?;
        } else {
            let mut response = Response::new(404);
            response.body = b"Not Found".to_vec();
            send_response(&mut stream, response).await?;
        }

        if !keep_alive { break; }
        debug!("Keep-Alive: Waiting for next request...");
    }
    Ok(())
}

pub async fn parse_request(stream: &mut TcpStream) -> Result<Request, HttpError> {
    let mut buffer = [0; 8192];
    let n = stream.read(&mut buffer).await?;
    if n == 0 { return Err(HttpError::Parse("Connection closed by client".to_string())); }

    let raw = std::str::from_utf8(&buffer[..n]).map_err(|_| HttpError::Parse("Non-UTF8 request".to_string()))?;
    let mut lines = raw.lines();
    let req_line = lines.next().ok_or(HttpError::Parse("Missing request line".to_string()))?;
    let parts: Vec<&str> = req_line.split_whitespace().collect();
    if parts.len() < 3 { return Err(HttpError::Parse("Invalid status line".to_string())); }

    let method = Method::from_str(parts[0])?;
    let path = parts[1].to_string();
    
    let mut headers = HashMap::new();
    for line in lines {
        if line.is_empty() { break; }
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_lowercase().to_string(), v.trim().to_string());
        }
    }

    Ok(Request { method, path, headers, params: HashMap::new(), body: Vec::new() })
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
