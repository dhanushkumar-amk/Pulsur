use http_server::{Method, Response, Router, WsMessage};
use std::sync::Arc;
use tracing::info;
use serde_json::json;
use futures::future::FutureExt;

/// 🚀 Router Initialization
/// Separating routing logic allows it to grow without cluttering the main entry point.
pub fn build_router() -> Router {
    let mut router = Router::new();

    // Standard JSON API Route (Health/Discovery)
    router.add_http(Method::GET, "/", Arc::new(|req| {
        async move {
            info!("📡 Discovery access from request path: {}", req.path);
            Response::json(200, &json!({
                "message": "Pulsar Secure Engine Online",
                "version": "0.7.0",
                "features": ["TLS", "WebSockets", "Dual-Stack"],
                "status": "Healthy"
            })).expect("Static JSON serialization failed")
        }.boxed()
    }));

    // Real-Time Echo WebSocket Route
    router.ws("/ws", Arc::new(|_ctx, mut ws| {
        async move {
            info!("⚡ WebSocket session established via /ws");
            while let Ok(Some(msg)) = ws.next_message().await {
                match msg {
                    WsMessage::Text(text) => {
                        info!("Echoing text via WS");
                        let _ = ws.send_text(&format!("Pulsar Echo: {}", text)).await;
                    }
                    WsMessage::Binary(data) => {
                        info!("Echoing binary via WS");
                        let _ = ws.send_binary(&data).await;
                    }
                    _ => {} // Ignore control frames for the echo logic
                }
            }
        }.boxed()
    }));

    router
}
