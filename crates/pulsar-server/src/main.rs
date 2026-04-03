use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::collections::HashMap;
use serde_json::json;
use thiserror::Error;
use tracing::{info, warn, error, Instrument};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// 🛸 Phase 4 — Rust Error Handling & Production Patterns
/// 
/// Custom error enum following 'thiserror' patterns for precise failure modes.
#[derive(Error, Debug)]
pub enum FerrumError {
    #[error("Failed to read from TCP stream: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Invalid HTTP request line: {0}")]
    InvalidRequestLine(String),

    #[error("Invalid header format: {0}")]
    InvalidHeader(String),

    #[error("Serialization failure: {0}")]
    SerializationError(String),

    #[error("Connection dropped prematurely")]
    ConnectionDropped,
}

/// Represents a simple HTTP request.
#[derive(Debug)]
struct HttpRequest {
    method: String,
    path: String,
    version: String,
    headers: HashMap<String, String>,
}

struct HttpResponse {
    status_code: u16,
    status_text: &'static str,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

impl HttpResponse {
    fn new(status_code: u16, status_text: &'static str) -> Self {
        let mut headers = HashMap::new();
        headers.insert("Server".to_string(), "Pulsar-Core/0.1.0".to_string());
        headers.insert("Content-Type".to_string(), "application/json".to_string());
        
        Self {
            status_code,
            status_text,
            headers,
            body: Vec::new(),
        }
    }

    fn to_bytes(&self) -> Vec<u8> {
        let mut output = Vec::new();
        output.extend_from_slice(format!("HTTP/1.1 {} {}\r\n", self.status_code, self.status_text).as_bytes());
        for (key, value) in &self.headers {
            output.extend_from_slice(format!("{}: {}\r\n", key, value).as_bytes());
        }
        if !self.body.is_empty() {
            output.extend_from_slice(format!("Content-Length: {}\r\n", self.body.len()).as_bytes());
        }
        output.extend_from_slice(b"\r\n");
        output.extend_from_slice(&self.body);
        output
    }
}

fn main() -> anyhow::Result<()> {
    // 🎨 Setup Structured Logging (tracing) with JSON format for production
    tracing_subscriber::registry()
        .with(fmt::layer().json()) // Enable JSON format
        .with(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .init();

    info!("--- 🛰️ PULSAR CORE: HTTP SERVER ENGINE ---");
    info!("--- 🏗️ Phase 4 Architecture: Robust Error Handling & Tracing ---");
    
    let listener = TcpListener::bind("127.0.0.1:3000")?;
    info!(address = "127.0.0.1:3000", "🚀 Starting Pulsar-Core");

    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                // Background processing logic (async placeholder)
                if let Err(e) = handle_connection(s) {
                    error!(error = %e, "Error handling connection");
                }
            }
            Err(e) => error!(error = %e, "Connection failed"),
        }
    }

    Ok(())
}

fn handle_connection(mut stream: TcpStream) -> Result<(), anyhow::Error> {
    let mut buffer = [0; 4096];
    let bytes_read = stream.read(&mut buffer)?;
    
    if bytes_read == 0 {
        warn!("Received empty request (connection dropped)");
        return Err(FerrumError::ConnectionDropped.into());
    }

    // Attempt to parse the request using our custom error-aware parser
    match parse_http_request(&buffer[..bytes_read]) {
        Ok(request) => {
            info!(method = %request.method, path = %request.path, "Request Received");
            
            let mut response = HttpResponse::new(200, "OK");
            let body = json!({
                "message": "Pulsar Engine: Operation Successful",
                "status": "online",
                "path": request.path
            });
            response.body = body.to_string().into_bytes();
            
            stream.write_all(&response.to_bytes())?;
        }
        Err(e) => {
            warn!(error = %e, "Handling malformed request");
            let mut response = HttpResponse::new(400, "Bad Request");
            response.body = json!({ "error": e.to_string() }).to_string().into_bytes();
            stream.write_all(&response.to_bytes())?;
            return Err(e.into());
        }
    }
    
    stream.flush()?;
    Ok(())
}

/// A specialized HTTP parser that returns FerrumError for granular failure tracking.
fn parse_http_request(raw_data: &[u8]) -> Result<HttpRequest, FerrumError> {
    let raw_str = std::str::from_utf8(raw_data)
        .map_err(|_| FerrumError::InvalidRequestLine("Non-UTF8 characters in request".to_string()))?;
    
    let mut lines = raw_str.lines();

    // Parse Status Line
    let status_line = lines.next()
        .ok_or_else(|| FerrumError::InvalidRequestLine("Empty request".to_string()))?;
    
    let parts: Vec<&str> = status_line.split_whitespace().collect();
    if parts.len() < 3 {
        return Err(FerrumError::InvalidRequestLine(status_line.to_string()));
    }

    let method = parts[0].to_string();
    let path = parts[1].to_string();
    let version = parts[2].to_string();

    // Parse Headers
    let mut header_map = HashMap::new();
    for line in lines {
        if line.is_empty() { break; }
        if let Some((k, v)) = line.split_once(':') {
            header_map.insert(k.trim().to_string(), v.trim().to_string());
        } else {
            return Err(FerrumError::InvalidHeader(line.to_string()));
        }
    }

    Ok(HttpRequest {
        method,
        path,
        version,
        headers: header_map,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_invalid_request_line() {
        let raw = b"GET"; // Missing path and version
        let result = parse_http_request(raw);
        assert!(matches!(result, Err(FerrumError::InvalidRequestLine(_))));
    }

    #[test]
    fn test_invalid_header_format() {
        let raw = b"GET / HTTP/1.1\r\nInvalidHeaderLine\r\n\r\n";
        let result = parse_http_request(raw);
        assert!(matches!(result, Err(FerrumError::InvalidHeader(_))));
    }
}
