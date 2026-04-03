use axum::{
    body::Body,
    extract::Request,
    routing::any,
    Router,
};
use gateway::{Context, Pipeline, PassthroughPlugin, AuthPlugin};
use std::net::SocketAddr;
use std::sync::Arc;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    // 1. Setup Auth Plugin
    let auth_plugin = AuthPlugin::new(
        "super-secret-key".to_string(), // Shared secret
        vec!["/login".to_string(), "/health".to_string()] // Bypass list
    );

    // 2. Setup Passthrough Plugin
    let passthrough = PassthroughPlugin::default();

    // 3. Assemble Pipeline
    let pipeline = Arc::new(Pipeline::new(vec![
        Box::new(auth_plugin),
        Box::new(passthrough),
    ]));

    // Simple handler that runs the pipeline
    let app = Router::new().fallback(any(move |req: Request<Body>| {
        let pipeline = Arc::clone(&pipeline);
        async move {
            let mut ctx = Context::new(req);
            // Default upstream for Phase 13 testing
            ctx.upstream_url = Some("http://localhost:4000".to_string());
            pipeline.execute(ctx).await
        }
    }));

    let addr: SocketAddr = "127.0.0.1:3000".parse().unwrap();
    tracing::info!("Gateway listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
