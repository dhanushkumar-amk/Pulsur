use http_server::{HttpServer, Router, Method, Response};
use std::sync::Arc;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;
use futures::future::FutureExt;
use serde_json::json;

/// 🛸 Pulsar Server: Enterprise Edition
/// High-Performance Async HTTP/HTTPS Unified Server.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 🎨 Setup Structured Logging for the entire server stack
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    info!("--- 🛰️ PULSAR ENTERPRISE: SECURE STACK ---");
    
    let mut router = Router::new();

    // Standard JSON API Route
    router.add_http(Method::GET, "/", Arc::new(|req| {
        async move {
            info!("Handling discovery request for path: {}", req.path);
            Response::json(200, &json!({
                "message": "Pulsar Secure Engine Online",
                "version": "0.7.0",
                "features": ["TLS", "WebSockets", "Dual-Stack"]
            })).unwrap()
        }.boxed()
    }));

    // Real-Time Echo WebSocket Route
    router.ws("/ws", Arc::new(|mut ws| {
        async move {
            info!("🚀 Real-time session established via /ws");
            while let Ok(Some(msg)) = ws.next_message().await {
                info!("Echoing message: {}", msg);
                let _ = ws.send_text(&format!("Pulsar Echo: {}", msg)).await;
            }
        }.boxed()
    }));

    let server = HttpServer::new(router, 100);

    // BIND DUAL-STACK PORTS
    // HTTP: 8080 (Public/Dev)
    // HTTPS: 3443 (Secure/Prod)
    server.run_dual("127.0.0.1:8080", "127.0.0.1:3443").await?;

    Ok(())
}
