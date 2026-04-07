use std::env;
use tracing::info;

/// 🛠️ Pulsar Server Configuration
/// Managed via environment variables with sensible defaults for a professional setup.
#[derive(Debug, Clone)]
pub struct AppConfig {
    pub host: String,
    pub http_port: String,
    pub https_port: String,
    pub cert_path: String,
    pub key_path: String,
    pub max_conns: usize,
}

impl AppConfig {
    /// Loads configuration from environment variables.
    /// Senior Dev Tip: Using defaults allows for zero-config local development
    /// while providing full control for production environments.
    pub fn from_env() -> Self {
        // Load .env file if it exists
        if let Err(e) = dotenvy::dotenv() {
            // Note: We don't error out if .env is missing, as variables might be in the actual environment
            info!(
                "Skipped .env loading: {}. Using system environment variables.",
                e
            );
        }

        Self {
            host: env::var("PULSAR_HOST").unwrap_or_else(|_| "127.0.0.1".into()),
            http_port: env::var("PULSAR_HTTP_PORT").unwrap_or_else(|_| "8080".into()),
            https_port: env::var("PULSAR_HTTPS_PORT").unwrap_or_else(|_| "3443".into()),
            cert_path: env::var("PULSAR_CERT_PATH").unwrap_or_else(|_| "certs/cert.pem".into()),
            key_path: env::var("PULSAR_KEY_PATH").unwrap_or_else(|_| "certs/key.pem".into()),
            max_conns: env::var("PULSAR_MAX_CONNS")
                .unwrap_or_else(|_| "1000".into())
                .parse()
                .unwrap_or(1000),
        }
    }

    pub fn http_addr(&self) -> String {
        format!("{}:{}", self.host, self.http_port)
    }

    pub fn https_addr(&self) -> String {
        format!("{}:{}", self.host, self.https_port)
    }
}
