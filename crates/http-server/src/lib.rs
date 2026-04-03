use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use thiserror::Error;
use tracing::{info, error, warn};
use futures::future::BoxFuture;

/// 🚀 Phase 6: HTTP Server Routing Engine
/// Advanced path matching with parameterized routes, wildcards, and middleware.

#[derive(Error, Debug)]
pub enum HttpError {
    #[error("IO Error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("Unsupported HTTP version: {0}")]
    UnsupportedVersion(String),
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
        headers.insert("Server".to_string(), "Ferrum-Core/0.3.0".to_string());
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
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_route(&mut self, method: Method, path: &str, handler: Handler) {
        let pattern = path.split('/').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();
        self.routes.push(Route { method, pattern, handler });
    }

    pub fn get(&mut self, path: &str, handler: Handler) { self.add_route(Method::GET, path, handler); }
    pub fn post(&mut self, path: &str, handler: Handler) { self.add_route(Method::POST, path, handler); }

    pub fn add_middleware(&mut self, mw: Arc<dyn Middleware>) {
        self.middleware.push(mw);
    }

    pub fn match_route(&self, method: Method, path: &str) -> Option<(Handler, HashMap<String, String>)> {
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        
        for route in &self.routes {
            if route.method != method { continue; }
            
            let mut params = HashMap::new();
            let mut matched = true;

            if route.pattern.is_empty() && segments.is_empty() {
                return Some((route.handler.clone(), params));
            }

            if route.pattern.len() != segments.len() {
                if let Some(last) = route.pattern.last() {
                    if last == "*" && segments.len() >= route.pattern.len() - 1 {
                        return Some((route.handler.clone(), params));
                    }
                }
                continue;
            }

            for (p_segment, s_segment) in route.pattern.iter().zip(segments.iter()) {
                if p_segment.starts_with(':') {
                    let key = p_segment[1..].to_string();
                    params.insert(key, s_segment.to_string());
                } else if p_segment == "*" {
                    break;
                } else if p_segment != s_segment {
                    matched = false;
                    break;
                }
            }

            if matched {
                return Some((route.handler.clone(), params));
            }
        }
        None
    }
}

pub struct HttpServer {
    addr: String,
    router: Arc<Router>,
}

impl HttpServer {
    pub fn new(addr: &str, router: Router) -> Self {
        Self { 
            addr: addr.to_string(), 
            router: Arc::new(router) 
        }
    }

    pub async fn run(&self) -> Result<(), HttpError> {
        let listener = TcpListener::bind(&self.addr).await?;
        info!("Ferrum Server Ignition: 🚀 Listening on http://{}", self.addr);

        loop {
            let (stream, _) = listener.accept().await?;
            let router = self.router.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_conn(stream, router).await {
                    error!("Connection terminated: {}", e);
                }
            });
        }
    }
}

async fn handle_conn(mut stream: TcpStream, router: Arc<Router>) -> Result<(), HttpError> {
    let mut request = match parse_request(&mut stream).await {
        Ok(req) => req,
        Err(e) => {
            let mut res = Response::new(400);
            res.body = format!("Engine Error: {}", e).into_bytes();
            send_response(&mut stream, res).await?;
            return Err(e);
        }
    };

    for mw in &router.middleware {
        if !mw.execute(&mut request).await {
            let mut res = Response::new(403);
            res.body = b"Forbidden by Middleware".to_vec();
            send_response(&mut stream, res).await?;
            return Ok(());
        }
    }

    if let Some((handler, params)) = router.match_route(request.method.clone(), &request.path) {
        info!(method = ?request.method, path = %request.path, "Successfully Dispatched");
        request.params = params;
        let response = handler(request).await;
        send_response(&mut stream, response).await?;
    } else {
        warn!(path = %request.path, "Route Not Found");
        let mut response = Response::new(404);
        response.body = b"404 - Engine Error: Router could not find this destination".to_vec();
        send_response(&mut stream, response).await?;
    }
    
    Ok(())
}

pub async fn parse_request(stream: &mut TcpStream) -> Result<Request, HttpError> {
    let mut buffer = [0; 8192];
    let n = stream.read(&mut buffer).await?;
    if n == 0 { return Err(HttpError::Parse("Request Empty".to_string())); }

    let raw_str = std::str::from_utf8(&buffer[..n]).map_err(|_| HttpError::Parse("Request is not UTF-8".to_string()))?;
    let mut lines = raw_str.lines();
    
    let request_line = lines.next().ok_or(HttpError::Parse("No request line".to_string()))?;
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 3 { return Err(HttpError::Parse("Malformed status line".to_string())); }

    let method = Method::from_str(parts[0])?;
    let path = parts[1].to_string();
    
    let mut headers = HashMap::new();
    for line in lines {
        if line.is_empty() { break; }
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_lowercase().to_string(), v.trim().to_string());
        }
    }

    Ok(Request {
        method,
        path,
        headers,
        params: HashMap::new(),
        body: Vec::new(),
    })
}

pub async fn send_response(stream: &mut TcpStream, response: Response) -> Result<(), HttpError> {
    let mut buffer = Vec::new();
    buffer.extend_from_slice(format!("HTTP/1.1 {} OK\r\n", response.status).as_bytes());
    for (k, v) in &response.headers {
        buffer.extend_from_slice(format!("{}: {}\r\n", k, v).as_bytes());
    }
    buffer.extend_from_slice(format!("Content-Length: {}\r\n", response.body.len()).as_bytes());
    buffer.extend_from_slice(b"\r\n");
    buffer.extend_from_slice(&response.body);
    
    stream.write_all(&buffer).await?;
    stream.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_exact_match() {
        let mut router = Router::new();
        router.get("/health", Arc::new(|_| Box::pin(async { Response::new(200) })));
        
        let m: Option<(Handler, HashMap<String, String>)> = router.match_route(Method::GET, "/health");
        assert!(m.is_some());
    }

    #[tokio::test]
    async fn test_parameter_extraction() {
        let mut router = Router::new();
        router.get("/users/:id", Arc::new(|_| Box::pin(async { Response::new(200) })));
        
        if let Some((_, params)) = router.match_route(Method::GET, "/users/123") {
            assert_eq!(params.get("id").unwrap(), "123");
        } else {
            panic!("Route failed to match");
        }
    }

    #[tokio::test]
    async fn test_wildcard_matching() {
        let mut router = Router::new();
        router.get("/static/*", Arc::new(|_| Box::pin(async { Response::new(200) })));
        
        let m = router.match_route(Method::GET, "/static/images/logo.png");
        assert!(m.is_some());
    }

    #[tokio::test]
    async fn test_multiple_params() {
        let mut router = Router::new();
        router.get("/posts/:pid/comments/:cid", Arc::new(|_| Box::pin(async { Response::new(200) })));
        
        if let Some((_, params)) = router.match_route(Method::GET, "/posts/ferrum-rocks/comments/456") {
            assert_eq!(params.get("pid").unwrap(), "ferrum-rocks");
            assert_eq!(params.get("cid").unwrap(), "456");
        } else {
            panic!("Multiple params failure");
        }
    }

    #[tokio::test]
    async fn test_no_route() {
        let mut router = Router::new();
        router.get("/a", Arc::new(|_| Box::pin(async { Response::new(200) })));
        let m = router.match_route(Method::GET, "/b");
        assert!(m.is_none());
    }
}
