use anyhow::Result;
use http_server::{HttpServer, ServerConfig};
use pulsar_server::{AppConfig, build_router};
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

/// 🛸 Pulsar Server: Enterprise Orchestration Entry Point
/// Optimized for reliability, observability, and modularity.
#[tokio::main]
async fn main() -> Result<()> {
    // 🎨 Setup Unified Structured Logging
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    info!("--- 🛰️ PULSAR ENTERPRISE REFACTORED: ENGINE STARTING ---");

    // 1. Initialize Configuration (Modular & Environment-Aware)
    let config = AppConfig::from_env();
    info!("Configuration loaded: {:?}", config);

    // 2. Build Routing Table (Modular Handlers)
    let router = build_router();

    // 3. Initialize High-Performance HTTP Engine
    let server = HttpServer::new(
        router,
        ServerConfig {
            max_conns: config.max_conns,
            ..ServerConfig::default()
        },
    );

    // 4. Bind and Run Dual-Stack Ports (Secure By Default)
    info!("Binding to HTTP: {} | HTTPS: {}", config.http_addr(), config.https_addr());
    server
        .run_dual(
            &config.http_addr(),
            &config.https_addr(),
            &config.cert_path,
            &config.key_path,
        )
        .await?;

    Ok(())
}
