use http_server::{HttpServer, Method, Response, Router, ServerConfig, WsMessage};
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
    router.ws("/ws", Arc::new(|_ctx, mut ws| {
        async move {
            info!("🚀 Real-time session established via /ws");
            while let Ok(Some(msg)) = ws.next_message().await {
                match msg {
                    WsMessage::Text(text) => {
                        info!("Echoing text message");
                        let _ = ws.send_text(&format!("Pulsar Echo: {}", text)).await;
                    }
                    WsMessage::Binary(data) => {
                        info!("Echoing binary message");
                        let _ = ws.send_binary(&data).await;
                    }
                    WsMessage::Ping => info!("Received ping frame"),
                    WsMessage::Pong => info!("Received pong frame"),
                }
            }
        }.boxed()
    }));

    let server = HttpServer::new(
        router,
        ServerConfig {
            max_conns: 100,
            ..ServerConfig::default()
        },
    );

    // BIND DUAL-STACK PORTS
    // HTTP: 8080 (Public/Dev)
    // HTTPS: 3443 (Secure/Prod)
    server
        .run_dual("127.0.0.1:8080", "127.0.0.1:3443", "cert.pem", "key.pem")
        .await?;

    Ok(())
}
