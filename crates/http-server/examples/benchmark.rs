use http_server::{HttpServer, Router, Method, Response, ServerConfig};
use std::sync::Arc;
use futures::future::FutureExt;
use serde_json::json;

/// 🚀 Ferrum Performance Laboratory
/// Zero-Copy Benchmark binary for the HTTP Server Engine.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // No logging for benchmarks to ensure pure measure of CPU/IO overhead
    
    let mut router = Router::new();

    // Matching the Node.js / Fastify payload exactly
    router.add_http(Method::GET, "/", Arc::new(|_| {
        async move {
            Response::json(200, &json!({
                "message": "Ferrum Engine Online",
                "version": "0.7.0"
            })).unwrap()
        }.boxed()
    }));

    let server = HttpServer::new(router, ServerConfig {
        max_conns: 1000,
        ..ServerConfig::default()
    });

    println!("Ferrum Benchmark Engine: http://127.0.0.1:8080");
    server
        .run_dual(
            "127.0.0.1:8080",
            "127.0.0.1:3443",
            "crates/http-server/cert.pem",
            "crates/http-server/key.pem",
        )
        .await?;

    Ok(())
}
