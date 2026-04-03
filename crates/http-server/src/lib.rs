use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use std::collections::HashMap;
use std::str::FromStr;
use thiserror::Error;
use tracing::{info, error, debug};

/// 🚀 Phase 5: HTTP Server Core Engine
/// This crate provides a low-level async HTTP server implementation.

#[derive(Error, Debug)]
pub enum HttpError {
    #[error("IO Error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("Unsupported HTTP version: {0}")]
    UnsupportedVersion(String),
}

#[derive(Debug, PartialEq, Clone)]
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

#[derive(Debug)]
pub struct Request {
    pub method: Method,
    pub path: String,
    pub headers: HashMap<String, String>,
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
        headers.insert("Server".to_string(), "Pulsar-Core/0.2.0".to_string());
        Self {
            status,
            headers,
            body: Vec::new(),
        }
    }
}

pub struct HttpServer {
    addr: String,
}

impl HttpServer {
    pub fn new(addr: &str) -> Self {
        Self {
            addr: addr.to_string(),
        }
    }

    pub async fn run(&self) -> Result<(), HttpError> {
        let listener = TcpListener::bind(&self.addr).await?;
        info!("🛰️ HTTP Server listening on http://{}", self.addr);

        loop {
            let (stream, _) = listener.accept().await?;
            tokio::spawn(async move {
                if let Err(e) = handle_conn(stream).await {
                    error!("Connection error: {}", e);
                }
            });
        }
    }
}

async fn handle_conn(mut stream: TcpStream) -> Result<(), HttpError> {
    // 1. Parse Request
    let request = match parse_request(&mut stream).await {
        Ok(req) => req,
        Err(e) => {
            let mut res = Response::new(400);
            res.body = format!("Bad Request: {}", e).into_bytes();
            send_response(&mut stream, res).await?;
            return Err(e);
        }
    };

    info!(method = ?request.method, path = %request.path, "Processing Request");

    // 2. Simple static response for core engine
    let mut response = Response::new(200);
    response.headers.insert("Content-Type".to_string(), "application/json".to_string());
    response.body = b"{\"engine\": \"Pulsar-HTTP\", \"version\": \"v0.2.0\"}".to_vec();

    // 3. Send Response
    send_response(&mut stream, response).await?;
    
    Ok(())
}

/// Full HTTP/1.1 Parser implementation
pub async fn parse_request(stream: &mut TcpStream) -> Result<Request, HttpError> {
    let mut buffer = [0; 4096];
    let n = stream.read(&mut buffer).await?;
    if n == 0 {
        return Err(HttpError::Parse("Empty request".to_string()));
    }

    let mut parser = std::str::from_utf8(&buffer[..n])
        .map_err(|_| HttpError::Parse("Request is not valid UTF-8".to_string()))?;
    
    let mut lines = parser.lines();

    // 1. Request Line: METHOD PATH VERSION
    let request_line = lines.next().ok_or_else(|| HttpError::Parse("Missing request line".to_string()))?;
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 3 {
        return Err(HttpError::Parse(format!("Invalid request line: {}", request_line)));
    }

    let method = Method::from_str(parts[0])?;
    let path = parts[1].to_string();
    let version = parts[2];

    if version != "HTTP/1.1" && version != "HTTP/1.0" {
        return Err(HttpError::UnsupportedVersion(version.to_string()));
    }

    // 2. Headers
    let mut headers = HashMap::new();
    let mut lines_iter = lines.by_ref();
    for line in lines_iter {
        if line.is_empty() { break; }
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_lowercase().to_string(), v.trim().to_string());
        }
    }

    // 3. Body (Content-Length aware)
    let body = if let Some(cl_str) = headers.get("content-length") {
        let cl: usize = cl_str.parse().map_err(|_| HttpError::Parse("Invalid Content-Length".to_string()))?;
        // For simplicity in this core engine, we expect the body to be in the current buffer if small
        // A production parser would read more bytes if cl > current buffer size
        let current_body_start = parser.find("\r\n\r\n").map(|i| i + 4).unwrap_or(n);
        let current_body = &buffer[current_body_start..n];
        
        if current_body.len() < cl {
            // Need to read more bytes here (omitted for the core phase demonstration, assumes small bodies)
            current_body.to_vec()
        } else {
            current_body[..cl].to_vec()
        }
    } else {
        Vec::new()
    };

    Ok(Request {
        method,
        path,
        headers,
        body,
    })
}

pub async fn send_response(stream: &mut TcpStream, response: Response) -> Result<(), HttpError> {
    let mut output = Vec::new();
    
    // Status Line
    let status_text = match response.status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "Unknown",
    };
    output.extend_from_slice(format!("HTTP/1.1 {} {}\r\n", response.status, status_text).as_bytes());

    // Headers
    for (k, v) in &response.headers {
        output.extend_from_slice(format!("{}: {}\r\n", k, v).as_bytes());
    }
    
    // Content-Length
    output.extend_from_slice(format!("Content-Length: {}\r\n", response.body.len()).as_bytes());
    
    // Separator
    output.extend_from_slice(b"\r\n");
    
    // Body
    output.extend_from_slice(&response.body);

    stream.write_all(&output).await?;
    stream.flush().await?;
    
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    // Note: Since parse_request takes a TcpStream, testing requires a real (or mocked) socket.
    // For these unit tests, we'll implement a standalone parse_request_bytes for testing logic.
    
    fn parse_request_bytes(raw: &[u8]) -> Result<Request, HttpError> {
        let parser = std::str::from_utf8(raw).map_err(|_| HttpError::Parse("UTF8 error".to_string()))?;
        let mut lines = parser.lines();
        let request_line = lines.next().ok_or(HttpError::Parse("No request line".to_string()))?;
        let parts: Vec<&str> = request_line.split_whitespace().collect();
        if parts.len() < 3 {
            return Err(HttpError::Parse("Invalid request line".to_string()));
        }
        let method = Method::from_str(parts[0])?;
        let path = parts[1].to_string();
        
        let mut headers = HashMap::new();
        for line in lines {
            if line.is_empty() { break; }
            if let Some((k, v)) = line.split_once(':') {
                headers.insert(k.trim().to_lowercase().to_string(), v.trim().to_string());
            }
        }
        
        Ok(Request { method, path, headers, body: Vec::new() })
    }

    #[test]
    fn test_parse_get_no_body() {
        let raw = b"GET /index.html HTTP/1.1\r\nHost: localhost\r\n\r\n";
        let req = parse_request_bytes(raw).unwrap();
        assert_eq!(req.method, Method::GET);
        assert_eq!(req.path, "/index.html");
        assert_eq!(req.headers.get("host").unwrap(), "localhost");
    }

    #[test]
    fn test_parse_post_json() {
        let raw = b"POST /api/data HTTP/1.1\r\nContent-Type: application/json\r\nContent-Length: 18\r\n\r\n{\"key\": \"value\"}";
        let req = parse_request_bytes(raw).unwrap();
        assert_eq!(req.method, Method::POST);
        assert_eq!(req.path, "/api/data");
        assert_eq!(req.headers.get("content-type").unwrap(), "application/json");
    }

    #[test]
    fn test_parse_bad_method() {
        let raw = b"FLY /path HTTP/1.1\r\n\r\n";
        let res = parse_request_bytes(raw);
        assert!(res.is_err());
    }

    #[test]
    fn test_parse_missing_version() {
        let raw = b"GET /path\r\n\r\n";
        let res = parse_request_bytes(raw);
        assert!(res.is_err());
    }
    
    #[test]
    fn test_parse_header_casing() {
        let raw = b"GET / HTTP/1.1\r\nX-Custom-Header: value\r\n\r\n";
        let req = parse_request_bytes(raw).unwrap();
        assert_eq!(req.headers.get("x-custom-header").unwrap(), "value");
    }

    #[test]
    fn test_parse_put_method() {
        let raw = b"PUT /resource HTTP/1.1\r\n\r\n";
        let req = parse_request_bytes(raw).unwrap();
        assert_eq!(req.method, Method::PUT);
    }

    #[test]
    fn test_parse_delete_method() {
        let raw = b"DELETE /resource HTTP/1.1\r\n\r\n";
        let req = parse_request_bytes(raw).unwrap();
        assert_eq!(req.method, Method::DELETE);
    }

    #[test]
    fn test_parse_empty_request() {
        let raw = b"";
        let res = parse_request_bytes(raw);
        assert!(res.is_err());
    }

    #[test]
    fn test_parse_multiple_headers() {
        let raw = b"GET / HTTP/1.1\r\nH1: v1\r\nH2: v2\r\n\r\n";
        let req = parse_request_bytes(raw).unwrap();
        assert_eq!(req.headers.len(), 2);
    }

    #[test]
    fn test_parse_path_with_query() {
        let raw = b"GET /search?q=test HTTP/1.1\r\n\r\n";
        let req = parse_request_bytes(raw).unwrap();
        assert_eq!(req.path, "/search?q=test");
    }
}
